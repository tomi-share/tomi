//! Sealed history transfer to one device (spec §7.4, §8.7; decision D2).
//!
//! A member exports its **whole** feed (secrets and files included: there is no partial
//! option, by design), encrypts it for a single recipient and drops it on the relay as
//! temporary blobs:
//!
//! * a random one-shot history key `HK`, sealed for the recipient's X25519 key taken from
//!   the sender's [`TrustStore`] (D38);
//! * a signed [`HistoryTransfer`] manifest listing the index parts;
//! * the index itself, JSON Lines ([`HistoryIndexWriter`] / [`HistoryIndexReader`]), cut
//!   into parts encrypted with STREAM under `HK` ([`HistoryPartEncryptor`] /
//!   [`HistoryPartDecryptor`]);
//! * every transferred file re-encrypted under a fresh file key (ordinary attachment
//!   STREAM, [`crate::ChunkEncryptor`]); its key travels in the index.
//!
//! No space key epoch is involved: the recipient only needs its own device keys.

use std::collections::BTreeSet;
use std::sync::{Mutex, MutexGuard};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::Zeroizing;

use crate::archive::{ArchiveState, ARCHIVE_AT_MS_MAX};
use crate::crypto::{
    aead_open, aead_seal, format_uuid, hkdf_key, labels, parse_uuid, random_array, random_key,
    to_array, Reader, Writer, KEY_LEN, NONCE_LEN, NONCE_PREFIX_LEN,
};
use crate::device::{
    device_id_from_ed25519, verify_signature, DeviceKeys, TrustStore, MAX_STATEMENT_ENTRIES,
    PUBLIC_KEY_LEN, SIGNATURE_LEN,
};
use crate::error::{CoreError, Result};
use crate::format::{CHUNK_CIPHERTEXT_SIZE, CHUNK_SIZE, FORMAT_MAX, TAG_LEN};
use crate::item::{b64_decode, json_category, validate_device_id, Attachment, Item};
use crate::pin::{PinState, PIN_AT_MS_MAX};
use crate::stream::{ChunkEncryptor, OpenEndedDecryptor};

type Pk = [u8; PUBLIC_KEY_LEN];

/// Sealed history key: ephemeral public (32) || nonce (24) || ciphertext (32 + 16).
pub const HISTORY_SEALED_KEY_LEN: usize = PUBLIC_KEY_LEN + NONCE_LEN + KEY_LEN + TAG_LEN;
/// Wire length of a [`HistoryTransfer`] without parts; add [`HISTORY_PART_ENTRY_LEN`] per part.
pub const HISTORY_TRANSFER_BASE_LEN: usize = labels::SIG_HISTORY_TRANSFER.len()
    + 1
    + 16
    + 16
    + 8
    + 8
    + PUBLIC_KEY_LEN
    + PUBLIC_KEY_LEN
    + HISTORY_SEALED_KEY_LEN
    + 4
    + SIGNATURE_LEN;
/// Bytes per part in the manifest: `uuid16(blob_id) || nonce_prefix (19)`.
pub const HISTORY_PART_ENTRY_LEN: usize = 16 + NONCE_PREFIX_LEN;
/// Maximum number of index parts in one transfer.
pub const MAX_HISTORY_PARTS: u32 = MAX_STATEMENT_ENTRIES;
/// Version of the JSON Lines index (`header.version`).
pub const HISTORY_INDEX_VERSION: u32 = 1;
/// Maximum length of one index line (an item payload is at most 1 MiB).
pub const MAX_HISTORY_LINE_BYTES: usize = 2_097_152;

/// Wire length of a manifest with `n_parts` parts (spec §8.7.1).
pub fn history_transfer_len(n_parts: usize) -> usize {
    HISTORY_TRANSFER_BASE_LEN + n_parts * HISTORY_PART_ENTRY_LEN
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    m.lock().map_err(|_| CoreError::Internal {
        reason: "poisoned lock".into(),
    })
}

// ---------------------------------------------------------------------------
// History key and its sealing (spec §7.4)
// ---------------------------------------------------------------------------

/// One-shot key of a history transfer. Zeroized on drop; `Debug` never prints it.
#[derive(Clone)]
pub struct HistoryKey {
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl HistoryKey {
    /// Fresh random key, one per transfer. Never reuse it for another transfer.
    pub fn generate() -> Result<Self> {
        Ok(Self { key: random_key()? })
    }

    /// Restores a key persisted by a resumable export or import task.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            key: Zeroizing::new(to_array(bytes, "history key")?),
        })
    }

    /// Raw key bytes, only to persist a resumable task in secure storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_vec()
    }

    pub(crate) fn raw(&self) -> &[u8; KEY_LEN] {
        &self.key
    }
}

impl std::fmt::Debug for HistoryKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HistoryKey(<redacted>)")
    }
}

pub(crate) fn aad_history_transport(
    space_id: &[u8; 16],
    transfer_id: &[u8; 16],
    recipient_ed: &Pk,
) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_HISTORY_TRANSPORT);
    w.bytes(space_id).bytes(transfer_id).bytes(recipient_ed);
    w.finish()
}

pub(crate) fn history_transport_key(
    shared: &[u8; 32],
    ephemeral_public: &Pk,
    recipient_x: &Pk,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral_public);
    salt[32..].copy_from_slice(recipient_x);
    hkdf_key(Some(&salt), shared, labels::HKDF_HISTORY_TRANSPORT)
}

pub(crate) fn seal_history_key_raw(
    recipient_ed: &Pk,
    recipient_x: &Pk,
    space_id: &[u8; 16],
    transfer_id: &[u8; 16],
    key: &HistoryKey,
    ephemeral_secret: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>> {
    let eph = StaticSecret::from(*ephemeral_secret);
    let eph_pub = XPublic::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&XPublic::from(*recipient_x));
    if !shared.was_contributory() {
        return Err(CoreError::input("recipient x25519 key has small order"));
    }
    let k = history_transport_key(shared.as_bytes(), &eph_pub, recipient_x)?;
    let ct = aead_seal(
        &k,
        nonce,
        &aad_history_transport(space_id, transfer_id, recipient_ed),
        key.raw(),
    )?;
    let mut out = Vec::with_capacity(HISTORY_SEALED_KEY_LEN);
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn open_history_key(
    own: &DeviceKeys,
    space_id: &[u8; 16],
    transfer_id: &[u8; 16],
    sealed: &[u8],
) -> Result<HistoryKey> {
    if sealed.len() != HISTORY_SEALED_KEY_LEN {
        return Err(CoreError::DecryptionFailed);
    }
    let eph_pub: Pk = to_array(&sealed[..32], "ephemeral key")?;
    let nonce: [u8; NONCE_LEN] = to_array(&sealed[32..56], "nonce")?;
    let shared = own.x_agree(&eph_pub).ok_or(CoreError::DecryptionFailed)?;
    let k = history_transport_key(&shared, &eph_pub, &own.x_public())?;
    let key = Zeroizing::new(aead_open(
        &k,
        &nonce,
        &aad_history_transport(space_id, transfer_id, &own.ed_public()),
        &sealed[56..],
    )?);
    HistoryKey::from_bytes(&key).map_err(|_| CoreError::DecryptionFailed)
}

// ---------------------------------------------------------------------------
// Signed manifest (spec §8.7.1)
// ---------------------------------------------------------------------------

/// One encrypted part of the index, in part order.
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryPart {
    /// Relay blob holding the encrypted part.
    pub blob_id: String,
    /// 19-byte STREAM nonce prefix, random and distinct for every part.
    pub nonce_prefix: Vec<u8>,
}

impl std::fmt::Debug for HistoryPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryPart")
            .field("blob_id", &self.blob_id)
            .finish_non_exhaustive()
    }
}

/// Signed manifest of a history transfer from `sender` to `recipient`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTransfer {
    pub space_id: String,
    pub transfer_id: String,
    /// Relay `seq` of the snapshot; everything after it reaches the recipient by the log.
    pub as_of_seq: u64,
    pub created_at_ms: u64,
    pub sender_ed25519: Vec<u8>,
    pub recipient_ed25519: Vec<u8>,
    /// [`HISTORY_SEALED_KEY_LEN`] bytes, spec §7.4.
    pub sealed_key: Vec<u8>,
    /// In part order (not sorted), 1 to [`MAX_HISTORY_PARTS`], no duplicate.
    pub parts: Vec<HistoryPart>,
    pub signature: Vec<u8>,
}

fn check_parts(parts: &[HistoryPart]) -> Result<Vec<([u8; 16], [u8; NONCE_PREFIX_LEN])>> {
    let n = u32::try_from(parts.len()).map_err(|_| CoreError::input("too many parts"))?;
    if n == 0 || n > MAX_HISTORY_PARTS {
        return Err(CoreError::input("history transfer needs 1 to 1024 parts"));
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        out.push((
            parse_uuid(&p.blob_id, "blob_id")?,
            to_array::<NONCE_PREFIX_LEN>(&p.nonce_prefix, "nonce prefix")?,
        ));
    }
    if !all_distinct(&out) {
        return Err(CoreError::input("duplicate part blob_id or nonce prefix"));
    }
    Ok(out)
}

fn all_distinct(parts: &[([u8; 16], [u8; NONCE_PREFIX_LEN])]) -> bool {
    let ids: BTreeSet<_> = parts.iter().map(|p| p.0).collect();
    let prefixes: BTreeSet<_> = parts.iter().map(|p| p.1).collect();
    ids.len() == parts.len() && prefixes.len() == parts.len()
}

impl HistoryTransfer {
    fn tbs(&self) -> Result<Vec<u8>> {
        let sid = parse_uuid(&self.space_id, "space id")?;
        let tid = parse_uuid(&self.transfer_id, "transfer id")?;
        let sender: Pk = to_array(&self.sender_ed25519, "sender")?;
        let recipient: Pk = to_array(&self.recipient_ed25519, "recipient")?;
        if sender == recipient {
            return Err(CoreError::input("sender and recipient must differ"));
        }
        if self.sealed_key.len() != HISTORY_SEALED_KEY_LEN {
            return Err(CoreError::input("sealed history key: wrong length"));
        }
        let parts = check_parts(&self.parts)?;
        let mut w = Writer::labeled(labels::SIG_HISTORY_TRANSFER);
        w.bytes(&sid)
            .bytes(&tid)
            .u64(self.as_of_seq)
            .u64(self.created_at_ms)
            .bytes(&sender)
            .bytes(&recipient)
            .bytes(&self.sealed_key)
            .u32(parts.len() as u32);
        for (blob, prefix) in &parts {
            w.bytes(blob).bytes(prefix);
        }
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature` ([`history_transfer_len`] bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.signature.len() != SIGNATURE_LEN {
            return Err(CoreError::input("signature: expected 64 bytes"));
        }
        let mut out = self.tbs()?;
        out.extend_from_slice(&self.signature);
        Ok(out)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SIGNATURE_LEN {
            return Err(CoreError::statement("too short"));
        }
        let (tbs, sig) = bytes.split_at(bytes.len() - SIGNATURE_LEN);
        let mut r = Reader::new(tbs);
        r.expect_label(labels::SIG_HISTORY_TRANSFER)?;
        let sid: [u8; 16] = r.array()?;
        let tid: [u8; 16] = r.array()?;
        let as_of_seq = r.u64()?;
        let created_at_ms = r.u64()?;
        let sender: Pk = r.array()?;
        let recipient: Pk = r.array()?;
        let sealed = r.take(HISTORY_SEALED_KEY_LEN)?.to_vec();
        let n = r.u32()?;
        if n == 0 || n > MAX_HISTORY_PARTS {
            return Err(CoreError::statement(
                "history transfer needs 1 to 1024 parts",
            ));
        }
        let mut raw = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let blob: [u8; 16] = r.array()?;
            let prefix: [u8; NONCE_PREFIX_LEN] = r.array()?;
            raw.push((blob, prefix));
        }
        if r.remaining() != 0 {
            return Err(CoreError::statement("trailing bytes"));
        }
        if !all_distinct(&raw) {
            return Err(CoreError::statement(
                "duplicate part blob_id or nonce prefix",
            ));
        }
        if sender == recipient {
            return Err(CoreError::statement("sender and recipient must differ"));
        }
        Ok(Self {
            space_id: format_uuid(&sid),
            transfer_id: format_uuid(&tid),
            as_of_seq,
            created_at_ms,
            sender_ed25519: sender.to_vec(),
            recipient_ed25519: recipient.to_vec(),
            sealed_key: sealed,
            parts: raw
                .iter()
                .map(|(b, p)| HistoryPart {
                    blob_id: format_uuid(b),
                    nonce_prefix: p.to_vec(),
                })
                .collect(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `sender_ed25519` only (what the relay checks;
    /// devices use [`TrustStore::verify_history_transfer`]).
    pub fn verify_signature(&self) -> Result<()> {
        let sender: Pk = to_array(&self.sender_ed25519, "sender")?;
        verify_signature(&sender, &self.tbs()?, &self.signature)
    }

    /// `device_id` of the sender.
    pub fn sender_device_id(&self) -> Result<String> {
        device_id_from_ed25519(&self.sender_ed25519)
    }

    /// `device_id` of the recipient.
    pub fn recipient_device_id(&self) -> Result<String> {
        device_id_from_ed25519(&self.recipient_ed25519)
    }

    /// Blob ids of the index parts, in part order.
    pub fn part_blob_ids(&self) -> Vec<String> {
        self.parts.iter().map(|p| p.blob_id.clone()).collect()
    }
}

impl TrustStore {
    /// Seals `history_key` for `recipient_ed25519` (a trusted member of this store, never
    /// a key handed over by the relay) and signs the manifest (sender side, spec §8.7.1).
    #[allow(clippy::too_many_arguments)]
    pub fn create_history_transfer(
        &self,
        issuer: &DeviceKeys,
        recipient_ed25519: &[u8],
        transfer_id: &str,
        as_of_seq: u64,
        created_at_ms: u64,
        history_key: &HistoryKey,
        parts: &[HistoryPart],
    ) -> Result<HistoryTransfer> {
        let eph: Zeroizing<[u8; 32]> = Zeroizing::new(random_array()?);
        let nonce: [u8; NONCE_LEN] = random_array()?;
        self.create_history_transfer_raw(
            issuer,
            recipient_ed25519,
            transfer_id,
            as_of_seq,
            created_at_ms,
            history_key,
            parts,
            &eph,
            &nonce,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_history_transfer_raw(
        &self,
        issuer: &DeviceKeys,
        recipient_ed25519: &[u8],
        transfer_id: &str,
        as_of_seq: u64,
        created_at_ms: u64,
        history_key: &HistoryKey,
        parts: &[HistoryPart],
        ephemeral_secret: &[u8; 32],
        nonce: &[u8; NONCE_LEN],
    ) -> Result<HistoryTransfer> {
        let issuer_ed = issuer.ed_public();
        if issuer_ed != self.own_ed() {
            return Err(CoreError::input("keys do not belong to this store"));
        }
        self.require_signer(&issuer_ed)?;
        let recipient: Pk = to_array(recipient_ed25519, "recipient")?;
        if recipient == issuer_ed {
            return Err(CoreError::input("cannot transfer history to oneself"));
        }
        let recipient_x = self
            .member_x25519(&recipient)
            .ok_or_else(|| CoreError::input("recipient is not a trusted member"))?;
        let sid = self.space_id_bytes();
        let tid = parse_uuid(transfer_id, "transfer id")?;
        let sealed = seal_history_key_raw(
            &recipient,
            &recipient_x,
            &sid,
            &tid,
            history_key,
            ephemeral_secret,
            nonce,
        )?;
        let mut m = HistoryTransfer {
            space_id: self.space_id(),
            transfer_id: transfer_id.to_owned(),
            as_of_seq,
            created_at_ms,
            sender_ed25519: issuer_ed.to_vec(),
            recipient_ed25519: recipient.to_vec(),
            sealed_key: sealed,
            parts: parts.to_vec(),
            signature: Vec::new(),
        };
        m.signature = issuer.sign(&m.tbs()?);
        Ok(m)
    }

    /// Recipient side (spec §8.7.4), in this order: same space (`space_mismatch`); sender
    /// trusted and not revoked (`untrusted_signer` / `revoked_device`); signature
    /// (`invalid_signature`); recipient is this device (`not_a_recipient`); sealed key
    /// opens (`decryption_failed`). Returns the history key.
    pub fn verify_history_transfer(
        &self,
        manifest: &HistoryTransfer,
        own: &DeviceKeys,
    ) -> Result<HistoryKey> {
        if own.ed_public() != self.own_ed() {
            return Err(CoreError::input("keys do not belong to this store"));
        }
        let sid = parse_uuid(&manifest.space_id, "space id")?;
        if sid != self.space_id_bytes() {
            return Err(CoreError::SpaceMismatch);
        }
        let sender: Pk = to_array(&manifest.sender_ed25519, "sender")?;
        self.require_signer(&sender)?;
        manifest.verify_signature()?;
        if manifest.recipient_ed25519 != self.own_ed() {
            return Err(CoreError::NotARecipient);
        }
        let tid = parse_uuid(&manifest.transfer_id, "transfer id")?;
        open_history_key(own, &sid, &tid, &manifest.sealed_key)
    }
}

// ---------------------------------------------------------------------------
// Encrypted index parts (spec §8.7.2)
// ---------------------------------------------------------------------------

pub(crate) fn aad_history_chunk(
    format: u32,
    transfer_id: &[u8; 16],
    part_index: u32,
    n_parts: u32,
) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_HISTORY_CHUNK);
    w.u32(format)
        .bytes(transfer_id)
        .u32(part_index)
        .u32(n_parts);
    w.finish()
}

fn check_part_index(part_index: u32, n_parts: u32) -> Result<()> {
    if n_parts == 0 || n_parts > MAX_HISTORY_PARTS || part_index >= n_parts {
        return Err(CoreError::input("invalid part index or count"));
    }
    Ok(())
}

/// Largest plaintext of one part whose ciphertext fits in `max_blob_bytes` (whole chunks).
pub fn history_part_capacity(max_blob_bytes: u64) -> Result<u64> {
    let chunks = max_blob_bytes / CHUNK_CIPHERTEXT_SIZE as u64;
    if chunks == 0 {
        return Err(CoreError::input("max_blob_bytes smaller than one chunk"));
    }
    Ok(chunks * CHUNK_SIZE as u64)
}

/// Number of parts for an index of `index_len` plaintext bytes (at least 1).
pub fn history_part_count(index_len: u64, max_blob_bytes: u64) -> Result<u32> {
    let cap = history_part_capacity(max_blob_bytes)?;
    let n = index_len.div_ceil(cap).max(1);
    u32::try_from(n)
        .ok()
        .filter(|&n| n <= MAX_HISTORY_PARTS)
        .ok_or_else(|| CoreError::input("history index too large"))
}

/// Streaming encryptor of one index part (STREAM under the history key).
pub struct HistoryPartEncryptor {
    inner: ChunkEncryptor,
    prefix: [u8; NONCE_PREFIX_LEN],
}

impl HistoryPartEncryptor {
    /// Part `part_index` of `n_parts` (both bound in the AAD), fresh random nonce prefix.
    pub fn new(key: &HistoryKey, transfer_id: &str, part_index: u32, n_parts: u32) -> Result<Self> {
        Self::with_prefix(key, transfer_id, part_index, n_parts, random_array()?)
    }

    pub(crate) fn with_prefix(
        key: &HistoryKey,
        transfer_id: &str,
        part_index: u32,
        n_parts: u32,
        prefix: [u8; NONCE_PREFIX_LEN],
    ) -> Result<Self> {
        check_part_index(part_index, n_parts)?;
        let tid = parse_uuid(transfer_id, "transfer id")?;
        let aad = aad_history_chunk(FORMAT_MAX, &tid, part_index, n_parts);
        Ok(Self {
            inner: ChunkEncryptor::with_aad(aad, Zeroizing::new(*key.raw()), prefix),
            prefix,
        })
    }

    /// Nonce prefix to list in the manifest for this part.
    pub fn nonce_prefix(&self) -> Vec<u8> {
        self.prefix.to_vec()
    }

    /// Feeds plaintext; returns the ciphertext of the chunks now complete.
    pub fn update(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.inner.update(data)
    }

    /// Encrypts the last chunk; returns its ciphertext (to append to the blob).
    pub fn finalize(&self) -> Result<Vec<u8>> {
        Ok(self.inner.finalize()?.ciphertext)
    }
}

/// Streaming decryptor of one index part. The plaintext is only complete once
/// [`finalize`](Self::finalize) succeeds.
pub struct HistoryPartDecryptor {
    inner: Mutex<OpenEndedDecryptor>,
}

impl HistoryPartDecryptor {
    /// Part `part_index` of a manifest already verified with
    /// [`TrustStore::verify_history_transfer`].
    pub fn new(key: &HistoryKey, manifest: &HistoryTransfer, part_index: u32) -> Result<Self> {
        let n = u32::try_from(manifest.parts.len()).map_err(|_| CoreError::input("parts"))?;
        check_part_index(part_index, n)?;
        let part = &manifest.parts[part_index as usize];
        let prefix = to_array::<NONCE_PREFIX_LEN>(&part.nonce_prefix, "nonce prefix")?;
        let tid = parse_uuid(&manifest.transfer_id, "transfer id")?;
        Ok(Self {
            inner: Mutex::new(OpenEndedDecryptor::new(
                Zeroizing::new(*key.raw()),
                prefix,
                aad_history_chunk(FORMAT_MAX, &tid, part_index, n),
            )),
        })
    }

    /// Feeds ciphertext bytes (any split); returns the plaintext of completed chunks.
    pub fn update(&self, data: &[u8]) -> Result<Vec<u8>> {
        lock(&self.inner)?.update(data)
    }

    /// Opens the final chunk and returns its plaintext.
    pub fn finalize(&self) -> Result<Vec<u8>> {
        lock(&self.inner)?.finalize()
    }
}

// ---------------------------------------------------------------------------
// JSON Lines index (spec §8.7.3)
// ---------------------------------------------------------------------------

/// First line of the index; must match the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryHeader {
    pub space_id: String,
    pub transfer_id: String,
    pub as_of_seq: u64,
    /// `device_id` of the sender.
    pub source_device: String,
}

impl HistoryHeader {
    /// Header expected for `manifest`.
    pub fn for_transfer(manifest: &HistoryTransfer) -> Result<Self> {
        Ok(Self {
            space_id: manifest.space_id.clone(),
            transfer_id: manifest.transfer_id.clone(),
            as_of_seq: manifest.as_of_seq,
            source_device: manifest.sender_device_id()?,
        })
    }
}

/// A device known to the sender, so that the recipient can name the author of an item
/// even if that device was revoked since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryDevice {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub revoked: bool,
}

/// Location and fresh key of a re-encrypted file (never the original file key).
#[derive(Clone, PartialEq, Eq)]
pub struct TransferredFile {
    pub blob_id: String,
    /// 32-byte file key `FK'`.
    pub file_key: Vec<u8>,
    /// 19-byte fresh STREAM nonce prefix.
    pub nonce_prefix: Vec<u8>,
}

impl std::fmt::Debug for TransferredFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferredFile")
            .field("blob_id", &self.blob_id)
            .finish_non_exhaustive()
    }
}

/// One item of the exported feed, as decrypted by the sender, plus its local metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub item: Item,
    /// `seq` of the original `item` event.
    pub origin_seq: u64,
    /// When the sender received (or sent) it, ms since the Unix epoch.
    pub received_at_ms: i64,
    pub read: bool,
    /// "Keep" was chosen on the sender (D33).
    pub kept: bool,
    /// One entry per `item.attachments[i]`; `None` = the file is not on the sender
    /// (never downloaded, or erased): the recipient shows the thumbnail only.
    pub files: Vec<Option<TransferredFile>>,
    /// Slots of `item.attachments` removed on the sender (spec §8.10.5), sorted, no duplicate,
    /// each below `attachments.len()`. Their `files` entry is `None` and they are not counted as
    /// unavailable. Empty for an item without removal (the field is then not written).
    pub removed_attachments: Vec<u32>,
    /// Pin state known by the sender (pinned **or** unpinned, with its signed time), so
    /// that the recipient converges with the events of the log (spec §8.8.5). `None` = no
    /// pin decision known. The recipient merges it into its [`crate::PinBook`] even if
    /// it already has the item.
    pub pin: Option<PinState>,
    /// Archive state known by the sender (archived **or** unarchived, with its signed
    /// time), same rules as `pin` (spec §8.9.4). `None` = no archive decision known. The
    /// recipient merges it into its [`crate::ArchiveBook`] even if it already has the item.
    pub archive: Option<ArchiveState>,
}

impl HistoryItem {
    /// Attachment `index` as the recipient decrypts it: metadata from the payload, blob
    /// and key of the transferred copy. `None` if the file was not transferred.
    pub fn import_attachment(&self, index: usize) -> Option<Attachment> {
        let base = self.item.attachments.get(index)?;
        let f = self.files.get(index)?.as_ref()?;
        Some(Attachment {
            blob_id: f.blob_id.clone(),
            file_key: f.file_key.clone(),
            nonce_prefix: f.nonce_prefix.clone(),
            ..base.clone()
        })
    }
}

/// One entry of the index, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryEntry {
    Device(HistoryDevice),
    /// Boxed: an item entry is much larger than a device entry.
    Item(Box<HistoryItem>),
}

/// Content of the final `end` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySummary {
    /// Number of `item` lines (checked by the reader).
    pub items: u64,
    /// Items the sender could not read and therefore did not export.
    pub skipped_unreadable: u64,
    /// Attachments listed with `null` (informative).
    pub files_unavailable: u64,
}

#[derive(Deserialize)]
struct WireKind {
    kind: String,
}

#[derive(Serialize, Deserialize)]
struct WireHeader {
    kind: String,
    version: u32,
    space_id: String,
    transfer_id: String,
    as_of_seq: u64,
    source_device: String,
}

#[derive(Serialize, Deserialize)]
struct WireDevice {
    kind: String,
    device_id: String,
    name: String,
    platform: String,
    #[serde(default)]
    revoked: bool,
}

#[derive(Serialize, Deserialize)]
struct WireFile {
    blob_id: String,
    key: String,
    nonce_prefix: String,
}

#[derive(Serialize, Deserialize)]
struct WireItemLine {
    kind: String,
    payload: Box<RawValue>,
    origin_seq: u64,
    received_at: i64,
    #[serde(default)]
    read: bool,
    #[serde(default)]
    kept: bool,
    #[serde(default)]
    files: Vec<Option<WireFile>>,
    /// Additive field (spec §8.10.5): slots of removed attachments, `files[i]` is `null` there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed_attachments: Vec<u32>,
    /// Additive field (spec §8.8.5): absent in indexes written before the pin existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pin: Option<WirePin>,
    /// Additive field (spec §8.9.4): absent in indexes written before the archive existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archive: Option<WireArchive>,
}

#[derive(Serialize, Deserialize)]
struct WirePin {
    pinned: bool,
    at: u64,
}

#[derive(Serialize, Deserialize)]
struct WireArchive {
    archived: bool,
    at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct WireSkipped {
    #[serde(default)]
    unreadable: u64,
    #[serde(default)]
    files_unavailable: u64,
}

#[derive(Serialize, Deserialize)]
struct WireEnd {
    kind: String,
    items: u64,
    #[serde(default)]
    skipped: WireSkipped,
}

const KIND_HEADER: &str = "header";
const KIND_DEVICE: &str = "device";
const KIND_ITEM: &str = "item";
const KIND_END: &str = "end";

fn parse_line<'a, T: Deserialize<'a>>(line: &'a [u8]) -> Result<T> {
    serde_json::from_slice(line)
        .map_err(|e| CoreError::payload(format!("history index: {}", json_category(&e))))
}

fn to_line<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let mut out = serde_json::to_vec(v).map_err(|_| CoreError::Internal {
        reason: "json serialization failed".into(),
    })?;
    if out.len() > MAX_HISTORY_LINE_BYTES {
        return Err(CoreError::input("history index line too long"));
    }
    out.push(b'\n');
    Ok(out)
}

/// Removes the raw `\n` and `\r` of a JSON text, which the index (JSON Lines) cannot carry.
/// In valid JSON they can only be whitespace between tokens (inside a string they must be
/// escaped), so the value is unchanged and a payload without line breaks is returned as is.
fn without_line_breaks(json: &[u8]) -> Vec<u8> {
    if !json.iter().any(|&b| matches!(b, b'\n' | b'\r')) {
        return json.to_vec();
    }
    let mut out = Vec::with_capacity(json.len());
    let (mut in_string, mut escaped) = (false, false);
    for &b in json {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if matches!(b, b'\n' | b'\r') {
            continue;
        }
        out.push(b);
    }
    out
}

fn check_removed(len: usize, removed: &[u32]) -> bool {
    removed.windows(2).all(|w| w[0] < w[1]) && removed.iter().all(|s| (*s as usize) < len)
}

fn check_files(item: &Item, files: &[Option<TransferredFile>], removed: &[u32]) -> Result<()> {
    if files.len() != item.attachments.len() {
        return Err(CoreError::input("files must match attachments"));
    }
    if !check_removed(item.attachments.len(), removed) {
        return Err(CoreError::input(
            "removed_attachments: sorted, unique, below attachments.len()",
        ));
    }
    if removed.iter().any(|s| files[*s as usize].is_some()) {
        return Err(CoreError::input("a removed attachment has no file"));
    }
    for f in files.iter().flatten() {
        parse_uuid(&f.blob_id, "file blob_id")?;
        to_array::<KEY_LEN>(&f.file_key, "file key")?;
        to_array::<NONCE_PREFIX_LEN>(&f.nonce_prefix, "file nonce prefix")?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexState {
    ExpectHeader,
    Body,
    Ended,
}

/// Produces the index lines in order: [`header_line`](Self::header_line), then any
/// number of device and item lines, then [`end_line`](Self::end_line). Each call returns
/// the bytes to append (one line ending with `\n`), so the index can be written to a
/// temporary file and encrypted in parts afterwards.
///
/// Wrap in a `Mutex`-holding object for UniFFI (methods take `&mut self`).
#[derive(Debug)]
pub struct HistoryIndexWriter {
    header: HistoryHeader,
    state: IndexState,
    ids: BTreeSet<String>,
    files_unavailable: u64,
}

impl HistoryIndexWriter {
    pub fn new(header: &HistoryHeader) -> Result<Self> {
        parse_uuid(&header.space_id, "space id")?;
        parse_uuid(&header.transfer_id, "transfer id")?;
        if !validate_device_id(&header.source_device) {
            return Err(CoreError::input("source_device: expected 32 lowercase hex"));
        }
        Ok(Self {
            header: header.clone(),
            state: IndexState::ExpectHeader,
            ids: BTreeSet::new(),
            files_unavailable: 0,
        })
    }

    fn expect(&self, state: IndexState) -> Result<()> {
        if self.state == state {
            Ok(())
        } else {
            Err(CoreError::input("history index lines out of order"))
        }
    }

    pub fn header_line(&mut self) -> Result<Vec<u8>> {
        self.expect(IndexState::ExpectHeader)?;
        let line = to_line(&WireHeader {
            kind: KIND_HEADER.into(),
            version: HISTORY_INDEX_VERSION,
            space_id: self.header.space_id.clone(),
            transfer_id: self.header.transfer_id.clone(),
            as_of_seq: self.header.as_of_seq,
            source_device: self.header.source_device.clone(),
        })?;
        self.state = IndexState::Body;
        Ok(line)
    }

    pub fn device_line(&mut self, device: &HistoryDevice) -> Result<Vec<u8>> {
        self.expect(IndexState::Body)?;
        if !validate_device_id(&device.device_id) {
            return Err(CoreError::input("device_id: expected 32 lowercase hex"));
        }
        to_line(&WireDevice {
            kind: KIND_DEVICE.into(),
            device_id: device.device_id.clone(),
            name: device.name.clone(),
            platform: device.platform.clone(),
            revoked: device.revoked,
        })
    }

    pub fn item_line(&mut self, entry: &HistoryItem) -> Result<Vec<u8>> {
        self.expect(IndexState::Body)?;
        if self.ids.contains(&entry.item.id) {
            return Err(CoreError::input("duplicate item in history index"));
        }
        check_files(&entry.item, &entry.files, &entry.removed_attachments)?;
        if entry.pin.is_some_and(|p| p.at_ms > PIN_AT_MS_MAX) {
            return Err(CoreError::input("pin.at above 2^53 - 1"));
        }
        if entry.archive.is_some_and(|a| a.at_ms > ARCHIVE_AT_MS_MAX) {
            return Err(CoreError::input("archive.at above 2^53 - 1"));
        }
        // The original payload bytes when the item was parsed (unknown fields included,
        // spec §5.4.4), minus raw line breaks outside strings: one line per item.
        let json = Zeroizing::new(without_line_breaks(&entry.item.to_json()?));
        let text = std::str::from_utf8(&json).map_err(|_| CoreError::Internal {
            reason: "payload json is not utf-8".into(),
        })?;
        let payload = RawValue::from_string(text.to_owned()).map_err(|_| CoreError::Internal {
            reason: "payload json rejected".into(),
        })?;
        let line = Zeroizing::new(to_line(&WireItemLine {
            kind: KIND_ITEM.into(),
            payload,
            origin_seq: entry.origin_seq,
            received_at: entry.received_at_ms,
            read: entry.read,
            kept: entry.kept,
            files: entry
                .files
                .iter()
                .map(|f| {
                    f.as_ref().map(|f| WireFile {
                        blob_id: f.blob_id.clone(),
                        key: B64.encode(&f.file_key),
                        nonce_prefix: B64.encode(&f.nonce_prefix),
                    })
                })
                .collect(),
            removed_attachments: entry.removed_attachments.clone(),
            pin: entry.pin.map(|p| WirePin {
                pinned: p.pinned,
                at: p.at_ms,
            }),
            archive: entry.archive.map(|a| WireArchive {
                archived: a.archived,
                at: a.at_ms,
            }),
        })?);
        self.ids.insert(entry.item.id.clone());
        self.files_unavailable += entry
            .files
            .iter()
            .enumerate()
            .filter(|(i, f)| {
                f.is_none() && !entry.removed_attachments.iter().any(|s| *s as usize == *i)
            })
            .count() as u64;
        Ok(line.to_vec())
    }

    /// Final line; `skipped_unreadable` = items the sender could not read (not exported).
    pub fn end_line(&mut self, skipped_unreadable: u64) -> Result<Vec<u8>> {
        self.expect(IndexState::Body)?;
        let line = to_line(&WireEnd {
            kind: KIND_END.into(),
            items: self.ids.len() as u64,
            skipped: WireSkipped {
                unreadable: skipped_unreadable,
                files_unavailable: self.files_unavailable,
            },
        })?;
        self.state = IndexState::Ended;
        Ok(line)
    }
}

/// Parses the decrypted index (concatenation of the parts, fed in any split) and checks
/// it against the manifest: header first and matching (`malformed_payload`), no duplicate
/// item (`malformed_payload`), `end` present (`history_incomplete`) with the right count.
/// Lines of an unknown `kind` are ignored (additive changes).
///
/// Wrap in a `Mutex`-holding object for UniFFI (methods take `&mut self`).
pub struct HistoryIndexReader {
    expected: HistoryHeader,
    state: IndexState,
    pending: Zeroizing<Vec<u8>>,
    ids: BTreeSet<String>,
    summary: Option<HistorySummary>,
}

impl std::fmt::Debug for HistoryIndexReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryIndexReader")
            .field("transfer_id", &self.expected.transfer_id)
            .field("items", &self.ids.len())
            .finish_non_exhaustive()
    }
}

impl HistoryIndexReader {
    pub fn new(manifest: &HistoryTransfer) -> Result<Self> {
        Ok(Self {
            expected: HistoryHeader::for_transfer(manifest)?,
            state: IndexState::ExpectHeader,
            pending: Zeroizing::new(Vec::new()),
            ids: BTreeSet::new(),
            summary: None,
        })
    }

    /// Feeds decrypted index bytes; returns the entries of the lines now complete.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<HistoryEntry>> {
        crate::stream::extend_zeroizing(&mut self.pending, data);
        let mut out = Vec::new();
        let mut start = 0;
        while let Some(rel) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + rel;
            let line = self.pending[start..end].to_vec();
            let line = Zeroizing::new(line);
            if let Some(entry) = self.line(&line)? {
                out.push(entry);
            }
            start = end + 1;
        }
        self.pending.drain(..start);
        if self.pending.len() > MAX_HISTORY_LINE_BYTES {
            return Err(CoreError::payload("history index line too long"));
        }
        if self.state == IndexState::Ended && !self.pending.is_empty() {
            return Err(CoreError::payload(
                "data after the end of the history index",
            ));
        }
        Ok(out)
    }

    /// Succeeds only once the `end` line was read and nothing follows it.
    pub fn finish(&mut self) -> Result<HistorySummary> {
        match (self.state, self.summary) {
            (IndexState::Ended, Some(s)) if self.pending.is_empty() => Ok(s),
            (IndexState::Ended, _) => Err(CoreError::payload("data after the end")),
            _ => Err(CoreError::HistoryIncomplete),
        }
    }

    fn line(&mut self, line: &[u8]) -> Result<Option<HistoryEntry>> {
        if line.len() > MAX_HISTORY_LINE_BYTES {
            return Err(CoreError::payload("history index line too long"));
        }
        if self.state == IndexState::Ended {
            return Err(CoreError::payload(
                "data after the end of the history index",
            ));
        }
        let kind: WireKind = parse_line(line)?;
        match (self.state, kind.kind.as_str()) {
            (IndexState::ExpectHeader, KIND_HEADER) => {
                self.header(parse_line(line)?)?;
                self.state = IndexState::Body;
                Ok(None)
            }
            (IndexState::ExpectHeader, _) | (_, KIND_HEADER) => Err(CoreError::payload(
                "history index must start with one header",
            )),
            (_, KIND_DEVICE) => {
                let d: WireDevice = parse_line(line)?;
                if !validate_device_id(&d.device_id) {
                    return Err(CoreError::payload("device_id: expected 32 lowercase hex"));
                }
                Ok(Some(HistoryEntry::Device(HistoryDevice {
                    device_id: d.device_id,
                    name: d.name,
                    platform: d.platform,
                    revoked: d.revoked,
                })))
            }
            (_, KIND_ITEM) => self.item(parse_line(line)?).map(Some),
            (_, KIND_END) => {
                let e: WireEnd = parse_line(line)?;
                if e.items != self.ids.len() as u64 {
                    return Err(CoreError::payload(
                        "end.items does not match the item lines",
                    ));
                }
                self.summary = Some(HistorySummary {
                    items: e.items,
                    skipped_unreadable: e.skipped.unreadable,
                    files_unavailable: e.skipped.files_unavailable,
                });
                self.state = IndexState::Ended;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn header(&self, h: WireHeader) -> Result<()> {
        if h.version != HISTORY_INDEX_VERSION {
            return Err(CoreError::payload("unknown history index version"));
        }
        let got = HistoryHeader {
            space_id: h.space_id,
            transfer_id: h.transfer_id,
            as_of_seq: h.as_of_seq,
            source_device: h.source_device,
        };
        if got != self.expected {
            return Err(CoreError::payload(
                "history header does not match the manifest",
            ));
        }
        Ok(())
    }

    fn item(&mut self, w: WireItemLine) -> Result<HistoryEntry> {
        let item = Item::from_json_unchecked_id(w.payload.get().as_bytes())?;
        if self.ids.contains(&item.id) {
            return Err(CoreError::payload("duplicate item in history index"));
        }
        let files_ok = w.files.len() == item.attachments.len();
        if !files_ok {
            return Err(CoreError::payload("files must match attachments"));
        }
        if !check_removed(item.attachments.len(), &w.removed_attachments) {
            return Err(CoreError::payload(
                "removed_attachments: sorted, unique, below attachments.len()",
            ));
        }
        let removed = w.removed_attachments;
        let mut files = Vec::with_capacity(w.files.len());
        for (i, f) in w.files.into_iter().enumerate() {
            // Never fetched: the attachment was removed on the sender.
            let f = if removed.iter().any(|s| *s as usize == i) {
                None
            } else {
                f
            };
            files.push(match f {
                None => None,
                Some(f) => {
                    if parse_uuid(&f.blob_id, "blob_id").is_err() {
                        return Err(CoreError::payload("file blob_id: not a canonical UUID"));
                    }
                    let file_key = b64_decode(&f.key, "file key")?;
                    let nonce_prefix = b64_decode(&f.nonce_prefix, "file nonce prefix")?;
                    if file_key.len() != KEY_LEN || nonce_prefix.len() != NONCE_PREFIX_LEN {
                        return Err(CoreError::payload("file key or nonce prefix length"));
                    }
                    Some(TransferredFile {
                        blob_id: f.blob_id,
                        file_key,
                        nonce_prefix,
                    })
                }
            });
        }
        self.ids.insert(item.id.clone());
        Ok(HistoryEntry::Item(Box::new(HistoryItem {
            item,
            origin_seq: w.origin_seq,
            received_at_ms: w.received_at,
            read: w.read,
            kept: w.kept,
            files,
            removed_attachments: removed,
            pin: match w.pin {
                Some(p) if p.at > PIN_AT_MS_MAX => {
                    return Err(CoreError::payload("pin.at above 2^53 - 1"));
                }
                p => p.map(|p| PinState {
                    pinned: p.pinned,
                    at_ms: p.at,
                }),
            },
            archive: match w.archive {
                Some(a) if a.at > ARCHIVE_AT_MS_MAX => {
                    return Err(CoreError::payload("archive.at above 2^53 - 1"));
                }
                a => a.map(|a| ArchiveState {
                    archived: a.archived,
                    at_ms: a.at,
                }),
            },
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{ItemText, ReadPolicy, SourceFormat};
    use crate::new_item_id;

    const SPACE: &str = "11111111-1111-4111-8111-111111111111";
    const TRANSFER: &str = "22222222-2222-4222-8222-222222222222";

    fn item(secret: bool) -> Item {
        Item {
            id: new_item_id().unwrap(),
            source_device: "0123456789abcdef0123456789abcdef".into(),
            created_at_ms: 1,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            secret,
            title: Some("Titre".into()),
            text: Some(ItemText {
                plain: "ligne 1\nligne 2".into(),
                html: None,
                markdown: None,
                source_format: SourceFormat::Plain,
            }),
            url: None,
            attachments: vec![],
            raw_payload: None,
        }
    }

    #[test]
    fn manifest_length_constant() {
        assert_eq!(HISTORY_SEALED_KEY_LEN, 104);
        assert_eq!(labels::SIG_HISTORY_TRANSFER.len(), 28);
        assert_eq!(history_transfer_len(0), 313);
        assert_eq!(history_transfer_len(2), 383);
    }

    #[test]
    fn end_to_end() {
        let a = DeviceKeys::generate().unwrap();
        let b = DeviceKeys::generate().unwrap();
        let c = DeviceKeys::generate().unwrap();
        let mut ta = TrustStore::new(SPACE, &a.public(), 1).unwrap();
        ta.add_verified_peer(&b.public()).unwrap();
        ta.add_verified_peer(&c.public()).unwrap();
        let mut tb = TrustStore::new(SPACE, &b.public(), 1).unwrap();
        tb.add_verified_peer(&a.public()).unwrap();
        let mut tc = TrustStore::new(SPACE, &c.public(), 1).unwrap();
        tc.add_verified_peer(&a.public()).unwrap();

        // Sender: index, then one part, then the manifest.
        let hk = HistoryKey::generate().unwrap();
        let header = HistoryHeader {
            space_id: SPACE.into(),
            transfer_id: TRANSFER.into(),
            as_of_seq: 42,
            source_device: a.device_id(),
        };
        let items = [item(false), item(true)];
        let mut w = HistoryIndexWriter::new(&header).unwrap();
        let mut index = w.header_line().unwrap();
        for it in &items {
            index.extend(
                w.item_line(&HistoryItem {
                    item: it.clone(),
                    origin_seq: 7,
                    received_at_ms: 8,
                    read: true,
                    kept: false,
                    files: vec![],
                    removed_attachments: vec![],
                    pin: it.secret.then_some(PinState {
                        pinned: true,
                        at_ms: 99,
                    }),
                    archive: it.secret.then_some(ArchiveState {
                        archived: true,
                        at_ms: 77,
                    }),
                })
                .unwrap(),
            );
        }
        assert!(w
            .item_line(&HistoryItem {
                item: items[0].clone(),
                origin_seq: 0,
                received_at_ms: 0,
                read: false,
                kept: false,
                files: vec![],
                removed_attachments: vec![],
                pin: None,
                archive: None,
            })
            .is_err());
        index.extend(w.end_line(3).unwrap());
        assert_eq!(
            history_part_count(index.len() as u64, 26_279_936).unwrap(),
            1
        );
        let enc = HistoryPartEncryptor::new(&hk, TRANSFER, 0, 1).unwrap();
        let mut blob = enc.update(&index).unwrap();
        blob.extend(enc.finalize().unwrap());
        let part = HistoryPart {
            blob_id: crate::new_blob_id().unwrap(),
            nonce_prefix: enc.nonce_prefix(),
        };
        let m = ta
            .create_history_transfer(&a, &b.public().ed25519, TRANSFER, 42, 9, &hk, &[part])
            .unwrap();
        let wire = m.to_bytes().unwrap();
        assert_eq!(wire.len(), history_transfer_len(1));
        let m = HistoryTransfer::from_bytes(&wire).unwrap();

        // Recipient.
        let hk_b = tb.verify_history_transfer(&m, &b).unwrap();
        assert_eq!(hk_b.to_bytes(), hk.to_bytes());
        assert_eq!(
            tc.verify_history_transfer(&m, &c).unwrap_err(),
            CoreError::NotARecipient
        );
        let dec = HistoryPartDecryptor::new(&hk_b, &m, 0).unwrap();
        let mut plain = Vec::new();
        for chunk in blob.chunks(1000) {
            plain.extend(dec.update(chunk).unwrap());
        }
        plain.extend(dec.finalize().unwrap());
        assert_eq!(plain, index);
        let mut r = HistoryIndexReader::new(&m).unwrap();
        let mut got = Vec::new();
        for chunk in plain.chunks(5) {
            got.extend(r.feed(chunk).unwrap());
        }
        let summary = r.finish().unwrap();
        assert_eq!(summary.items, 2);
        assert_eq!(summary.skipped_unreadable, 3);
        assert_eq!(got.len(), 2);
        match &got[1] {
            HistoryEntry::Item(h) => {
                assert_eq!(h.item, items[1]);
                assert!(h.read);
                assert_eq!(
                    h.pin,
                    Some(PinState {
                        pinned: true,
                        at_ms: 99
                    })
                );
                assert_eq!(
                    h.archive,
                    Some(ArchiveState {
                        archived: true,
                        at_ms: 77
                    })
                );
            }
            other => panic!("unexpected {other:?}"),
        }

        // Missing end line.
        let cut = index.len() - w_end_len(&index);
        let mut r = HistoryIndexReader::new(&m).unwrap();
        r.feed(&index[..cut]).unwrap();
        assert_eq!(r.finish().unwrap_err(), CoreError::HistoryIncomplete);
    }

    fn w_end_len(index: &[u8]) -> usize {
        let body = &index[..index.len() - 1];
        index.len() - body.iter().rposition(|&b| b == b'\n').unwrap() - 1
    }

    fn unsigned_manifest() -> HistoryTransfer {
        HistoryTransfer {
            space_id: SPACE.into(),
            transfer_id: TRANSFER.into(),
            as_of_seq: 5,
            created_at_ms: 0,
            sender_ed25519: vec![1; 32],
            recipient_ed25519: vec![2; 32],
            sealed_key: vec![0; HISTORY_SEALED_KEY_LEN],
            parts: vec![HistoryPart {
                blob_id: crate::new_blob_id().unwrap(),
                nonce_prefix: vec![3; NONCE_PREFIX_LEN],
            }],
            signature: vec![0; 64],
        }
    }

    /// Exports `item` as the only item of an index and reads it back.
    fn export_then_import(item: &Item) -> (Vec<u8>, Item) {
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        let mut index = w.header_line().unwrap();
        index.extend(
            w.item_line(&HistoryItem {
                item: item.clone(),
                origin_seq: 1,
                received_at_ms: 2,
                read: false,
                kept: false,
                files: vec![None; item.attachments.len()],
                removed_attachments: vec![],
                pin: None,
                archive: None,
            })
            .unwrap(),
        );
        index.extend(w.end_line(0).unwrap());
        // header, item, end: the payload never splits its line.
        assert_eq!(index.iter().filter(|&&b| b == b'\n').count(), 3);
        let mut r = HistoryIndexReader::new(&m).unwrap();
        let mut entries = r.feed(&index).unwrap();
        assert_eq!(r.finish().unwrap().items, 1);
        let Some(HistoryEntry::Item(h)) = entries.pop() else {
            panic!("expected an item entry");
        };
        (index, h.item)
    }

    /// Reads back an index whose only item line carries `extra` as additional fields
    /// (written by hand: the writer refuses what these tests need to feed the reader).
    fn import_with_fields(extra: &[(&str, serde_json::Value)]) -> Result<Vec<HistoryEntry>> {
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        let mut index = w.header_line().unwrap();
        let line = w
            .item_line(&HistoryItem {
                item: item(false),
                origin_seq: 1,
                received_at_ms: 2,
                read: false,
                kept: false,
                files: vec![],
                removed_attachments: vec![],
                pin: None,
                archive: None,
            })
            .unwrap();
        let mut v: serde_json::Value = serde_json::from_slice(&line).unwrap();
        for (k, f) in extra {
            v.as_object_mut().unwrap().insert((*k).into(), f.clone());
        }
        index.extend(serde_json::to_vec(&v).unwrap());
        index.push(b'\n');
        index.extend(w.end_line(0).unwrap());
        let mut r = HistoryIndexReader::new(&m).unwrap();
        let entries = r.feed(&index)?;
        r.finish()?;
        Ok(entries)
    }

    fn only_item(entries: Vec<HistoryEntry>) -> Box<HistoryItem> {
        match entries.into_iter().next() {
            Some(HistoryEntry::Item(h)) => h,
            other => panic!("expected an item entry, got {other:?}"),
        }
    }

    fn item_with_two_files() -> Item {
        let mut it = item(false);
        for i in 0..2u8 {
            it.attachments.push(Attachment {
                blob_id: crate::new_blob_id().unwrap(),
                name: format!("f{i}.bin"),
                mime: "application/octet-stream".into(),
                size: 3,
                file_key: vec![i + 1; KEY_LEN],
                nonce_prefix: vec![9; NONCE_PREFIX_LEN],
                thumbnail: None,
            });
        }
        it
    }

    fn two_file_entry(removed: Vec<u32>, files: Vec<Option<TransferredFile>>) -> HistoryItem {
        HistoryItem {
            item: item_with_two_files(),
            origin_seq: 1,
            received_at_ms: 2,
            read: false,
            kept: false,
            files,
            removed_attachments: removed,
            pin: None,
            archive: None,
        }
    }

    fn transferred() -> TransferredFile {
        TransferredFile {
            blob_id: crate::new_blob_id().unwrap(),
            file_key: vec![5; KEY_LEN],
            nonce_prefix: vec![6; NONCE_PREFIX_LEN],
        }
    }

    #[test]
    fn removed_attachments_roundtrip_with_null_files_and_are_not_unavailable() {
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        let mut index = w.header_line().unwrap();
        // Slot 0 removed (null), slot 1 simply not on the sender (null, unavailable).
        let line = w
            .item_line(&two_file_entry(vec![0], vec![None, None]))
            .unwrap();
        // `files` keeps one entry per attachment (an old reader requires it).
        let v: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(v["files"], serde_json::json!([null, null]));
        assert_eq!(v["removed_attachments"], serde_json::json!([0]));
        index.extend(line);
        index.extend(w.end_line(0).unwrap());
        let end: serde_json::Value =
            serde_json::from_slice(index.split(|b| *b == b'\n').rev().nth(1).unwrap()).unwrap();
        assert_eq!(end["skipped"]["files_unavailable"], 1);
        let mut r = HistoryIndexReader::new(&m).unwrap();
        let entries = r.feed(&index).unwrap();
        assert_eq!(r.finish().unwrap().files_unavailable, 1);
        let h = only_item(entries);
        assert_eq!(h.removed_attachments, vec![0]);
        assert_eq!(h.files.len(), 2);
    }

    #[test]
    fn an_item_without_removal_writes_no_removed_attachments_field() {
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        w.header_line().unwrap();
        let line = w
            .item_line(&two_file_entry(vec![], vec![None, Some(transferred())]))
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&line).unwrap();
        assert!(v.get("removed_attachments").is_none());
    }

    #[test]
    fn the_writer_refuses_a_bad_removed_list() {
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        w.header_line().unwrap();
        for bad in [vec![1, 0], vec![0, 0], vec![2]] {
            assert!(
                w.item_line(&two_file_entry(bad, vec![None, None])).is_err(),
                "sorted, unique, in range"
            );
        }
        // A removed attachment has no file.
        assert!(w
            .item_line(&two_file_entry(vec![0], vec![Some(transferred()), None]))
            .is_err());
    }

    #[test]
    fn the_reader_refuses_a_bad_removed_list_and_never_fetches_a_removed_slot() {
        use serde_json::json;
        let m = unsigned_manifest();
        let line_for = |extra: serde_json::Value| -> Result<Vec<HistoryEntry>> {
            let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
            let mut index = w.header_line().unwrap();
            let line = w
                .item_line(&two_file_entry(vec![], vec![None, Some(transferred())]))
                .unwrap();
            let mut v: serde_json::Value = serde_json::from_slice(&line).unwrap();
            v["removed_attachments"] = extra;
            index.extend(serde_json::to_vec(&v).unwrap());
            index.push(b'\n');
            index.extend(w.end_line(0).unwrap());
            let mut r = HistoryIndexReader::new(&m).unwrap();
            let entries = r.feed(&index)?;
            r.finish()?;
            Ok(entries)
        };
        for bad in [
            json!([1, 0]),
            json!([0, 0]),
            json!([2]),
            json!([-1]),
            json!("0"),
        ] {
            assert_eq!(
                line_for(bad).unwrap_err().code(),
                "malformed_payload",
                "malformed removed_attachments"
            );
        }
        // A file listed for a removed slot is dropped by the reader, not downloaded.
        let h = only_item(line_for(json!([1])).unwrap());
        assert_eq!(h.removed_attachments, vec![1]);
        assert!(h.files[1].is_none());
    }

    #[test]
    fn pin_and_archive_times_above_2_pow_53_are_refused_on_import() {
        use serde_json::json;
        let max = 9_007_199_254_740_991_u64;
        // The bound itself is accepted, one more is `malformed_payload`.
        let ok = only_item(
            import_with_fields(&[
                ("pin", json!({"pinned": true, "at": max})),
                ("archive", json!({"archived": true, "at": max})),
            ])
            .unwrap(),
        );
        assert_eq!(ok.pin.unwrap().at_ms, max);
        assert_eq!(ok.archive.unwrap().at_ms, max);
        let err =
            import_with_fields(&[("pin", json!({"pinned": true, "at": max + 1}))]).unwrap_err();
        assert_eq!(err.code(), "malformed_payload");
        let err = import_with_fields(&[("archive", json!({"archived": true, "at": max + 1}))])
            .unwrap_err();
        assert_eq!(err.code(), "malformed_payload");
        // Even an unarchive decision is bounded.
        let err = import_with_fields(&[("archive", json!({"archived": false, "at": u64::MAX}))])
            .unwrap_err();
        assert_eq!(err.code(), "malformed_payload");
    }

    #[test]
    fn an_archive_line_without_at_is_refused_and_null_means_no_decision() {
        use serde_json::json;
        // `archived` and `at` are both mandatory (spec §8.9.4).
        for bad in [
            json!({"archived": true}),
            json!({"at": 5}),
            json!({"archived": true, "at": -1}),
            json!({"archived": "yes", "at": 5}),
            json!("archived"),
        ] {
            let err = import_with_fields(&[("archive", bad.clone())]).unwrap_err();
            assert_eq!(err.code(), "malformed_payload", "{bad}");
        }
        let err = import_with_fields(&[("pin", json!({"pinned": true}))]).unwrap_err();
        assert_eq!(err.code(), "malformed_payload");
        // `null` is the absence of a decision: nothing to merge, not an error.
        let h = only_item(import_with_fields(&[("archive", json!(null))]).unwrap());
        assert_eq!(h.archive, None);
        let h = only_item(import_with_fields(&[("pin", json!(null))]).unwrap());
        assert_eq!(h.pin, None);
    }

    #[test]
    fn unknown_payload_fields_survive_export_and_import() {
        use crate::item::testing::payload_with_unknown_fields;
        use serde_json::Value;

        let id = new_item_id().unwrap();
        let pretty = payload_with_unknown_fields(&id);
        let local = crate::LocalKey::generate().unwrap();

        // Payload as a compact single line: the export is byte for byte the original.
        let compact = without_line_breaks(&pretty);
        assert_ne!(compact, pretty);
        let received = Item::from_json(&compact, &id).unwrap();
        let (index, imported) = export_then_import(&received);
        let needle = String::from_utf8(compact.clone()).unwrap();
        assert!(String::from_utf8(index).unwrap().contains(&needle));
        assert_eq!(imported, received);
        assert_eq!(
            imported.raw_payload.as_ref().unwrap().as_bytes(),
            &compact[..]
        );

        // Payload with raw line breaks between tokens (legal JSON): only those breaks go,
        // every field and every other byte is kept.
        let received = Item::from_json(&pretty, &id).unwrap();
        let (_, imported) = export_then_import(&received);
        let got = imported.raw_payload.as_ref().unwrap().as_bytes();
        assert_eq!(got, &compact[..]);
        assert_eq!(
            serde_json::from_slice::<Value>(got).unwrap(),
            serde_json::from_slice::<Value>(&pretty).unwrap()
        );

        // Then the importing device re-seals it locally and exports it again (a relay of a
        // relay): still the same bytes.
        let rec = local.seal_item(&imported).unwrap();
        let reopened = local.open_item(&rec, &id).unwrap();
        let (_, again) = export_then_import(&reopened);
        assert_eq!(again.raw_payload.unwrap().as_bytes(), &compact[..]);
    }

    #[test]
    fn edited_item_is_not_exported_with_a_stale_payload() {
        let id = new_item_id().unwrap();
        let payload = crate::item::testing::payload_with_unknown_fields(&id);
        let mut item = Item::from_json(&payload, &id).unwrap();
        item.secret = false;
        let m = unsigned_manifest();
        let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&m).unwrap()).unwrap();
        w.header_line().unwrap();
        let entry = HistoryItem {
            item,
            origin_seq: 1,
            received_at_ms: 2,
            read: false,
            kept: false,
            files: vec![None],
            removed_attachments: vec![],
            pin: None,
            archive: None,
        };
        assert_eq!(w.item_line(&entry).unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn line_breaks_are_removed_outside_strings_only() {
        let json = b"{\n \"a\" :\r\n \"x\\\"\\n\\\\\" ,\"b\":[1,\n2]}\n";
        assert_eq!(
            without_line_breaks(json),
            b"{ \"a\" : \"x\\\"\\n\\\\\" ,\"b\":[1,2]}".to_vec()
        );
        assert_eq!(without_line_breaks(b"{\"a\":1}"), b"{\"a\":1}".to_vec());
    }

    #[test]
    fn truncated_part_is_reported() {
        let hk = HistoryKey::generate().unwrap();
        let enc = HistoryPartEncryptor::new(&hk, TRANSFER, 0, 1).unwrap();
        let data = vec![7u8; CHUNK_SIZE + 10];
        let mut blob = enc.update(&data).unwrap();
        blob.extend(enc.finalize().unwrap());
        let m = HistoryTransfer {
            space_id: SPACE.into(),
            transfer_id: TRANSFER.into(),
            as_of_seq: 0,
            created_at_ms: 0,
            sender_ed25519: vec![1; 32],
            recipient_ed25519: vec![2; 32],
            sealed_key: vec![0; HISTORY_SEALED_KEY_LEN],
            parts: vec![HistoryPart {
                blob_id: crate::new_blob_id().unwrap(),
                nonce_prefix: enc.nonce_prefix(),
            }],
            signature: vec![0; 64],
        };
        let dec = HistoryPartDecryptor::new(&hk, &m, 0).unwrap();
        dec.update(&blob[..CHUNK_CIPHERTEXT_SIZE]).unwrap();
        assert_eq!(dec.finalize().unwrap_err(), CoreError::StreamTruncated);
        let dec = HistoryPartDecryptor::new(&hk, &m, 0).unwrap();
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        dec.update(&bad).unwrap();
        assert_eq!(dec.finalize().unwrap_err(), CoreError::DecryptionFailed);
    }
}
