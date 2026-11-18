//! Attachment encryption in 64 KiB chunks, STREAM construction (spec §6).
//!
//! `nonce_i = nonce_prefix(19) || u32be(i) || last_flag(1)`; the last chunk carries
//! flag `0x01`, all others `0x00`, so truncation and reordering are detected.
//! Objects use `&self` + an inner `Mutex` so they can be exported as UniFFI objects.

use std::sync::{Mutex, MutexGuard};

use zeroize::Zeroizing;

use crate::crypto::{
    aead_open, aead_seal, labels, parse_uuid, random_array, random_key, to_array, Writer, KEY_LEN,
    NONCE_LEN, NONCE_PREFIX_LEN,
};
use crate::error::{CoreError, Result};
use crate::format::{CHUNK_CIPHERTEXT_SIZE, CHUNK_SIZE, FORMAT_MAX, TAG_LEN};
use crate::item::Attachment;

/// `AAD_chunk` (spec §6.3).
pub(crate) fn aad_chunk(format: u32, item_id: &[u8; 16], attachment_index: u32) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_CHUNK);
    w.u32(format).bytes(item_id).u32(attachment_index);
    w.finish()
}

/// `nonce_i` (spec §6.2).
pub(crate) fn chunk_nonce(
    prefix: &[u8; NONCE_PREFIX_LEN],
    counter: u32,
    last: bool,
) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    n[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    n[NONCE_PREFIX_LEN..NONCE_PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    n[NONCE_LEN - 1] = u8::from(last);
    n
}

/// Appends `data` to a buffer of sensitive bytes without ever leaving an unzeroed copy
/// behind: when the capacity is short, the contents move to a new zeroizing buffer and the
/// old one is wiped on drop, instead of letting `Vec` reallocate (which frees the old block
/// without erasing it).
pub(crate) fn extend_zeroizing(buf: &mut Zeroizing<Vec<u8>>, data: &[u8]) {
    if buf.capacity() - buf.len() < data.len() {
        let wanted = buf.len().saturating_add(data.len());
        let mut bigger = Zeroizing::new(Vec::with_capacity(
            wanted.max(buf.capacity().saturating_mul(2)),
        ));
        bigger.extend_from_slice(buf);
        // The previous buffer is zeroized as it is dropped here.
        *buf = bigger;
    }
    buf.extend_from_slice(data);
}

/// Encrypts one chunk with explicit parameters (used by the stream objects and the
/// test-vector hooks).
pub(crate) fn encrypt_chunk_raw(
    file_key: &[u8; KEY_LEN],
    prefix: &[u8; NONCE_PREFIX_LEN],
    aad: &[u8],
    counter: u32,
    last: bool,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    aead_seal(
        file_key,
        &chunk_nonce(prefix, counter, last),
        aad,
        plaintext,
    )
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    m.lock().map_err(|_| CoreError::Internal {
        reason: "poisoned lock".into(),
    })
}

/// Output of [`ChunkEncryptor::finalize`]: the last ciphertext bytes plus the
/// parameters to store in the item's [`Attachment`].
#[derive(Clone, PartialEq, Eq)]
pub struct FinalizedAttachment {
    /// Last chunk ciphertext, to append to the blob.
    pub ciphertext: Vec<u8>,
    pub file_key: Zeroizing<Vec<u8>>,
    pub nonce_prefix: Vec<u8>,
    /// Total plaintext size.
    pub size: u64,
}

impl std::fmt::Debug for FinalizedAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalizedAttachment")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

struct EncState {
    key: Zeroizing<[u8; KEY_LEN]>,
    prefix: [u8; NONCE_PREFIX_LEN],
    aad: Vec<u8>,
    pending: Zeroizing<Vec<u8>>,
    counter: u32,
    size: u64,
    done: bool,
}

/// Streaming encryptor of one attachment. Memory use is bounded by one chunk plus
/// the data passed to a single [`update`](Self::update) call.
pub struct ChunkEncryptor {
    state: Mutex<EncState>,
}

impl ChunkEncryptor {
    /// New encryptor with a fresh random file key and nonce prefix.
    ///
    /// `item_id` must be the id of the item that will reference the attachment and
    /// `attachment_index` its position in `attachments` (both are bound in the AAD).
    pub fn new(item_id: &str, attachment_index: u32) -> Result<Self> {
        let key = random_key()?;
        let prefix: [u8; NONCE_PREFIX_LEN] = random_array()?;
        Self::with_params(item_id, attachment_index, key, prefix)
    }

    pub(crate) fn with_params(
        item_id: &str,
        attachment_index: u32,
        key: Zeroizing<[u8; KEY_LEN]>,
        prefix: [u8; NONCE_PREFIX_LEN],
    ) -> Result<Self> {
        let id = parse_uuid(item_id, "item id")?;
        Ok(Self::with_aad(
            aad_chunk(FORMAT_MAX, &id, attachment_index),
            key,
            prefix,
        ))
    }

    /// Same STREAM construction under another AAD (history index parts, spec §8.7.3).
    pub(crate) fn with_aad(
        aad: Vec<u8>,
        key: Zeroizing<[u8; KEY_LEN]>,
        prefix: [u8; NONCE_PREFIX_LEN],
    ) -> Self {
        Self {
            state: Mutex::new(EncState {
                key,
                prefix,
                aad,
                pending: Zeroizing::new(Vec::with_capacity(CHUNK_SIZE)),
                counter: 0,
                size: 0,
                done: false,
            }),
        }
    }

    /// Feeds plaintext; returns the ciphertext of every chunk that is now complete.
    /// A full chunk is held back until more data (or `finalize`) tells whether it is
    /// the last one.
    pub fn update(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut s = lock(&self.state)?;
        if s.done {
            return Err(CoreError::StreamFinalized);
        }
        s.size = s
            .size
            .checked_add(data.len() as u64)
            .ok_or_else(|| CoreError::input("attachment too large"))?;
        extend_zeroizing(&mut s.pending, data);
        let mut out = Vec::new();
        let mut offset = 0;
        while s.pending.len() - offset > CHUNK_SIZE {
            let ct = encrypt_chunk_raw(
                &s.key,
                &s.prefix,
                &s.aad,
                s.counter,
                false,
                &s.pending[offset..offset + CHUNK_SIZE],
            )?;
            out.extend_from_slice(&ct);
            s.counter = s
                .counter
                .checked_add(1)
                .ok_or_else(|| CoreError::input("too many chunks"))?;
            offset += CHUNK_SIZE;
        }
        s.pending.drain(..offset);
        Ok(out)
    }

    /// Encrypts the remaining data as the last chunk (possibly empty if the file is
    /// empty) and returns the attachment parameters.
    pub fn finalize(&self) -> Result<FinalizedAttachment> {
        let mut s = lock(&self.state)?;
        if s.done {
            return Err(CoreError::StreamFinalized);
        }
        let ct = encrypt_chunk_raw(&s.key, &s.prefix, &s.aad, s.counter, true, &s.pending)?;
        s.done = true;
        s.pending.clear();
        Ok(FinalizedAttachment {
            ciphertext: ct,
            file_key: Zeroizing::new(s.key.to_vec()),
            nonce_prefix: s.prefix.to_vec(),
            size: s.size,
        })
    }
}

struct DecState {
    key: Zeroizing<[u8; KEY_LEN]>,
    prefix: [u8; NONCE_PREFIX_LEN],
    aad: Vec<u8>,
    pending: Vec<u8>,
    counter: u32,
    total_chunks: u32,
    last_ct_len: usize,
    done: bool,
}

/// Streaming decryptor of one attachment.
///
/// Plaintext returned by [`update`](Self::update) is authentic chunk by chunk but the
/// file is only complete once [`finalize`](Self::finalize) succeeds: write to a
/// temporary file and expose it only after `finalize` (spec §6.5).
pub struct ChunkDecryptor {
    state: Mutex<DecState>,
}

impl ChunkDecryptor {
    /// `attachment` comes from the authenticated payload; its `size` fixes the number
    /// of chunks.
    pub fn new(item_id: &str, attachment_index: u32, attachment: &Attachment) -> Result<Self> {
        let id = parse_uuid(item_id, "item id")?;
        let key = Zeroizing::new(to_array::<KEY_LEN>(&attachment.file_key, "file key")?);
        let prefix = to_array::<NONCE_PREFIX_LEN>(&attachment.nonce_prefix, "nonce prefix")?;
        let chunk = CHUNK_SIZE as u64;
        let total = if attachment.size == 0 {
            1
        } else {
            attachment.size.div_ceil(chunk)
        };
        let total_chunks =
            u32::try_from(total).map_err(|_| CoreError::input("attachment too large"))?;
        let last_plain = attachment.size - (total - 1) * chunk;
        let last_ct_len = usize::try_from(last_plain)
            .map_err(|_| CoreError::input("attachment too large"))?
            + TAG_LEN;
        Ok(Self {
            state: Mutex::new(DecState {
                key,
                prefix,
                aad: aad_chunk(FORMAT_MAX, &id, attachment_index),
                pending: Vec::new(),
                counter: 0,
                total_chunks,
                last_ct_len,
                done: false,
            }),
        })
    }

    /// Feeds ciphertext bytes (any split); returns the plaintext of completed chunks.
    pub fn update(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut s = lock(&self.state)?;
        if s.done {
            return if data.is_empty() {
                Ok(Vec::new())
            } else {
                Err(CoreError::StreamTrailingData)
            };
        }
        s.pending.extend_from_slice(data);
        let mut out = Vec::new();
        let mut offset = 0;
        while !s.done {
            let last = s.counter == s.total_chunks - 1;
            let need = if last {
                s.last_ct_len
            } else {
                CHUNK_CIPHERTEXT_SIZE
            };
            if s.pending.len() - offset < need {
                break;
            }
            let nonce = chunk_nonce(&s.prefix, s.counter, last);
            let pt = aead_open(&s.key, &nonce, &s.aad, &s.pending[offset..offset + need])?;
            out.extend_from_slice(&pt);
            offset += need;
            if last {
                s.done = true;
            } else {
                s.counter += 1;
            }
        }
        s.pending.drain(..offset);
        if s.done && !s.pending.is_empty() {
            return Err(CoreError::StreamTrailingData);
        }
        Ok(out)
    }

    /// Succeeds only if the last chunk was received and authenticated.
    pub fn finalize(&self) -> Result<()> {
        let s = lock(&self.state)?;
        if s.done {
            Ok(())
        } else {
            Err(CoreError::StreamTruncated)
        }
    }
}

/// STREAM decryption when the plaintext size is not authenticated beforehand (history
/// index parts, spec §8.7.3): every full ciphertext chunk followed by more data is a
/// non-final chunk; what remains at [`finalize`](Self::finalize) is the final chunk.
pub(crate) struct OpenEndedDecryptor {
    key: Zeroizing<[u8; KEY_LEN]>,
    prefix: [u8; NONCE_PREFIX_LEN],
    aad: Vec<u8>,
    pending: Vec<u8>,
    counter: u32,
    done: bool,
}

impl OpenEndedDecryptor {
    pub(crate) fn new(
        key: Zeroizing<[u8; KEY_LEN]>,
        prefix: [u8; NONCE_PREFIX_LEN],
        aad: Vec<u8>,
    ) -> Self {
        Self {
            key,
            prefix,
            aad,
            pending: Vec::new(),
            counter: 0,
            done: false,
        }
    }

    /// Feeds ciphertext; returns the plaintext of the non-final chunks now known.
    pub(crate) fn update(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if self.done {
            return if data.is_empty() {
                Ok(Vec::new())
            } else {
                Err(CoreError::StreamTrailingData)
            };
        }
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        let mut offset = 0;
        while self.pending.len() - offset > CHUNK_CIPHERTEXT_SIZE {
            let nonce = chunk_nonce(&self.prefix, self.counter, false);
            let pt = aead_open(
                &self.key,
                &nonce,
                &self.aad,
                &self.pending[offset..offset + CHUNK_CIPHERTEXT_SIZE],
            )?;
            out.extend_from_slice(&pt);
            offset += CHUNK_CIPHERTEXT_SIZE;
            self.counter = self
                .counter
                .checked_add(1)
                .ok_or_else(|| CoreError::input("too many chunks"))?;
        }
        self.pending.drain(..offset);
        Ok(out)
    }

    /// Opens the remaining bytes as the final chunk and returns its plaintext.
    ///
    /// A remainder that authenticates as a *non-final* chunk means the stream was cut
    /// after a chunk boundary: [`CoreError::StreamTruncated`].
    pub(crate) fn finalize(&mut self) -> Result<Vec<u8>> {
        if self.done {
            return Err(CoreError::StreamFinalized);
        }
        if self.pending.len() < TAG_LEN {
            return Err(CoreError::StreamTruncated);
        }
        let last = chunk_nonce(&self.prefix, self.counter, true);
        match aead_open(&self.key, &last, &self.aad, &self.pending) {
            Ok(pt) => {
                self.done = true;
                self.pending.clear();
                Ok(pt)
            }
            Err(e) => {
                let not_last = chunk_nonce(&self.prefix, self.counter, false);
                if self.pending.len() == CHUNK_CIPHERTEXT_SIZE
                    && aead_open(&self.key, &not_last, &self.aad, &self.pending).is_ok()
                {
                    Err(CoreError::StreamTruncated)
                } else {
                    Err(e)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "00000000-0000-4000-8000-000000000001";

    #[test]
    fn extend_zeroizing_keeps_content_when_it_has_to_grow() {
        let mut buf = Zeroizing::new(Vec::with_capacity(4));
        extend_zeroizing(&mut buf, &[1, 2, 3]);
        let first = buf.capacity();
        extend_zeroizing(&mut buf, &[4, 5, 6, 7, 8, 9]);
        assert_eq!(&buf[..], &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(buf.capacity() >= 9 && buf.capacity() > first);
        extend_zeroizing(&mut buf, &[]);
        assert_eq!(buf.len(), 9);
    }

    fn roundtrip(size: usize, split: usize) {
        let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let enc = ChunkEncryptor::new(ID, 0).unwrap();
        let mut blob = Vec::new();
        for part in data.chunks(split.max(1)) {
            blob.extend(enc.update(part).unwrap());
        }
        let fin = enc.finalize().unwrap();
        blob.extend(&fin.ciphertext);
        assert_eq!(fin.size, size as u64);
        assert_eq!(
            blob.len() as u64,
            crate::format::encrypted_attachment_len(size as u64)
        );
        let att = Attachment {
            blob_id: ID.into(),
            name: "f".into(),
            mime: "application/octet-stream".into(),
            size: fin.size,
            file_key: fin.file_key.to_vec(),
            nonce_prefix: fin.nonce_prefix,
            thumbnail: None,
        };
        let dec = ChunkDecryptor::new(ID, 0, &att).unwrap();
        let mut out = Vec::new();
        for part in blob.chunks(7_777) {
            out.extend(dec.update(part).unwrap());
        }
        dec.finalize().unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn roundtrips() {
        for size in [
            0,
            1,
            CHUNK_SIZE - 1,
            CHUNK_SIZE,
            CHUNK_SIZE + 1,
            3 * CHUNK_SIZE,
        ] {
            roundtrip(size, 10_000);
            roundtrip(size, 200_000);
        }
    }
}
