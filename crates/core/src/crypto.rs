//! Thin wrappers over audited primitives (spec §2, §4).
//!
//! Nothing here implements a primitive: XChaCha20-Poly1305 comes from
//! `chacha20poly1305`, HKDF-SHA256 from `hkdf` + `sha2`, X25519 / Ed25519 from the
//! dalek crates, randomness from the OS through `getrandom`.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::{CoreError, Result};
use crate::format::SPACE_KEY_EPOCH_MAX;

/// Symmetric key length (space key, CEK, file key, derived subkeys).
pub const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 nonce length.
pub const NONCE_LEN: usize = 24;
/// STREAM nonce prefix length for attachments (24 - 4 counter - 1 flag).
pub const NONCE_PREFIX_LEN: usize = 19;

/// Domain-separation labels. Each byte string is used for exactly one purpose (spec §4).
pub mod labels {
    /// HKDF info: space key -> CEK wrapping key.
    pub const HKDF_CEK_WRAP: &[u8] = b"tomi/v1/hkdf/cek-wrap";
    /// HKDF info: X25519 shared secret -> space key transport key.
    pub const HKDF_KEY_TRANSPORT: &[u8] = b"tomi/v1/hkdf/key-transport";
    /// AAD label of the wrapped CEK.
    pub const AAD_CEK_WRAP: &[u8] = b"tomi/v1/aad/cek-wrap";
    /// AAD label of the item payload.
    pub const AAD_PAYLOAD: &[u8] = b"tomi/v1/aad/payload";
    /// AAD label of an attachment chunk.
    pub const AAD_CHUNK: &[u8] = b"tomi/v1/aad/chunk";
    /// AAD label of a transported space key.
    pub const AAD_KEY_TRANSPORT: &[u8] = b"tomi/v1/aad/key-transport";
    /// Hash label of the device id derivation.
    pub const DEVICE_ID: &[u8] = b"tomi/v1/device-id";
    /// Signature label of a device certificate.
    pub const SIG_DEVICE_CERT: &[u8] = b"tomi/v1/sig/device-cert";
    /// Signature label of a member list.
    pub const SIG_MEMBER_LIST: &[u8] = b"tomi/v1/sig/member-list";
    /// Signature label of a key rotation.
    pub const SIG_KEY_ROTATION: &[u8] = b"tomi/v1/sig/key-rotation";
    /// Signature label of a deletion tombstone.
    pub const SIG_TOMBSTONE: &[u8] = b"tomi/v1/sig/tombstone";
    /// Signature label of the removal of one attachment (spec §8.10). Its own label: an item
    /// tombstone can never be replayed as an attachment removal, nor the other way round.
    pub const SIG_ATTACHMENT_TOMBSTONE: &[u8] = b"tomi/v1/sig/attachment-tombstone";
    /// HKDF info: X25519 shared secret -> history transfer key transport key.
    pub const HKDF_HISTORY_TRANSPORT: &[u8] = b"tomi/v1/hkdf/history-transport";
    /// AAD label of a sealed history transfer key.
    pub const AAD_HISTORY_TRANSPORT: &[u8] = b"tomi/v1/aad/history-transport";
    /// AAD label of a chunk of a history index part.
    pub const AAD_HISTORY_CHUNK: &[u8] = b"tomi/v1/aad/history-chunk";
    /// Signature label of a history transfer manifest.
    pub const SIG_HISTORY_TRANSFER: &[u8] = b"tomi/v1/sig/history-transfer";
    /// AAD label of an item re-sealed under the device-local key (never leaves the device).
    pub const AAD_LOCAL_ITEM: &[u8] = b"tomi/v1/aad/local-item";
    /// HKDF info: space key -> key of the encrypted item-state events (pin, spec §8.8).
    pub const HKDF_ITEM_STATE: &[u8] = b"tomi/v1/hkdf/item-state";
    /// AAD label of an encrypted item-state event.
    pub const AAD_ITEM_STATE: &[u8] = b"tomi/v1/aad/item-state";
    /// Signature label of a pin / unpin statement (inside an item-state event).
    pub const SIG_PIN: &[u8] = b"tomi/v1/sig/pin";
    /// Signature label of an archive / unarchive statement (inside an item-state event,
    /// spec §8.9). Its own label: a pin statement can never be replayed as an archive one.
    pub const SIG_ARCHIVE: &[u8] = b"tomi/v1/sig/archive";
    /// Signature label of the relay authentication challenge (`api-v1.md` §3.6). Reserved
    /// for that use: no other statement can be replayed as an authentication proof.
    pub const SIG_RELAY_AUTH_RESERVED: &[u8] = b"tomi/v1/sig/relay-auth";
    /// AAD label of a secret (device keys, local key, space keys, history key) sealed in the
    /// local database under the wrapping key given by the OS secure storage (never exchanged).
    pub const AAD_LOCAL_SECRET: &[u8] = b"tomi/v1/aad/local-secret";

    // --- Pairing (`spec/pairing-v1.md` §13): additive, one label per use. ---

    /// Prefix of the CPace channel identifier `CI` (`pairing-v1.md` §3.3).
    pub const PAIR_CI: &[u8] = b"tomi/v1/pair-ci";
    /// CPace associated data `ADa` of the inviter: `label || 0x00 || u8(PAIR_VERSION)`.
    pub const PAIR_INVITER: &[u8] = b"tomi/v1/pair-inviter";
    /// CPace associated data `ADb` of the joiner: `label || 0x00 || u8(PAIR_VERSION)`.
    pub const PAIR_JOINER: &[u8] = b"tomi/v1/pair-joiner";
    /// Hash prefix of the inviter's commitment to its CPace share (§5.1).
    pub const PAIR_COMMIT: &[u8] = b"tomi/v1/pair-commit";
    /// HKDF info: session key sealing the inviter -> joiner frames (§5.2).
    pub const HKDF_PAIR_INVITER: &[u8] = b"tomi/v1/hkdf/pair-inviter";
    /// HKDF info: session key sealing the joiner -> inviter frames (§5.2).
    pub const HKDF_PAIR_JOINER: &[u8] = b"tomi/v1/hkdf/pair-joiner";
    /// HKDF info: verification code (SAS) derivation (§5.2).
    pub const HKDF_PAIR_SAS: &[u8] = b"tomi/v1/hkdf/pair-sas";
    /// AAD label of the sealed pairing frames (§5.4).
    pub const AAD_PAIR: &[u8] = b"tomi/v1/aad/pair";
}

/// Fills an array from the OS CSPRNG.
pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).map_err(|_| CoreError::Randomness)?;
    Ok(out)
}

/// Same as [`random_array`] but zeroized on drop (for keys).
pub(crate) fn random_key() -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    getrandom::fill(out.as_mut()).map_err(|_| CoreError::Randomness)?;
    Ok(out)
}

/// XChaCha20-Poly1305 encryption. Output = ciphertext || 16-byte tag.
pub(crate) fn aead_seal(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CoreError::Internal {
            reason: "aead encryption failed".into(),
        })
}

/// XChaCha20-Poly1305 decryption; any failure is [`CoreError::DecryptionFailed`].
pub(crate) fn aead_open(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    cipher
        .decrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CoreError::DecryptionFailed)
}

/// HKDF-SHA256 (RFC 5869) with an arbitrary output length.
pub(crate) fn hkdf_sha256(
    salt: Option<&[u8]>,
    ikm: &[u8],
    info: &[u8],
    len: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let hk = Hkdf::<Sha256>::new(salt, ikm);
    let mut okm = Zeroizing::new(vec![0u8; len]);
    hk.expand(info, okm.as_mut_slice())
        .map_err(|_| CoreError::input("hkdf output length too large"))?;
    Ok(okm)
}

/// HKDF-SHA256 producing a 32-byte key.
pub(crate) fn hkdf_key(
    salt: Option<&[u8]>,
    ikm: &[u8],
    info: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let okm = hkdf_sha256(salt, ikm, info, KEY_LEN)?;
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    out.copy_from_slice(okm.as_slice());
    Ok(out)
}

/// SHA-256 digest.
pub(crate) fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Converts a slice to a fixed-size array or fails with `InvalidInput`.
pub(crate) fn to_array<const N: usize>(bytes: &[u8], what: &str) -> Result<[u8; N]> {
    <[u8; N]>::try_from(bytes)
        .map_err(|_| CoreError::input(format!("{what}: expected {N} bytes, got {}", bytes.len())))
}

/// A space key (spec §4.1): 32 random bytes identified by its `key_epoch`.
///
/// Zeroized on drop. `Debug` never prints the key.
#[derive(Clone)]
pub struct SpaceKey {
    epoch: u32,
    key: Zeroizing<[u8; KEY_LEN]>,
}

/// A space key epoch is in `1..=SPACE_KEY_EPOCH_MAX`; higher values are reserved for
/// key identifiers that are not space keys (spec §5.8.2).
pub(crate) fn check_space_key_epoch(epoch: u32) -> Result<()> {
    if epoch == 0 {
        return Err(CoreError::input("key_epoch must be >= 1"));
    }
    if epoch > SPACE_KEY_EPOCH_MAX {
        return Err(CoreError::input(
            "key_epoch above 2^31 - 1 is reserved (not a space key epoch)",
        ));
    }
    Ok(())
}

impl SpaceKey {
    /// Generates a fresh random space key for `epoch` (1 ≤ epoch ≤ 2^31 − 1).
    pub fn generate(epoch: u32) -> Result<Self> {
        check_space_key_epoch(epoch)?;
        Ok(Self {
            epoch,
            key: random_key()?,
        })
    }

    /// Rebuilds a space key from secure storage (Keystore, DPAPI).
    pub fn from_bytes(epoch: u32, bytes: &[u8]) -> Result<Self> {
        check_space_key_epoch(epoch)?;
        Ok(Self {
            epoch,
            key: Zeroizing::new(to_array(bytes, "space key")?),
        })
    }

    /// Epoch of this key.
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Raw key bytes, only to persist them in secure storage. Never log them.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_vec()
    }

    pub(crate) fn raw(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// Subkey wrapping the per-item CEKs (spec §4.2).
    pub(crate) fn cek_wrap_key(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        hkdf_key(None, self.key.as_slice(), labels::HKDF_CEK_WRAP)
    }

    /// Subkey of the encrypted item-state events (spec §8.8.2).
    pub(crate) fn item_state_key(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        hkdf_key(None, self.key.as_slice(), labels::HKDF_ITEM_STATE)
    }
}

impl std::fmt::Debug for SpaceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceKey")
            .field("epoch", &self.epoch)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// Canonical binary writer for AAD and signed statements (big-endian integers).
#[derive(Default)]
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    /// Starts a buffer with `label || 0x00`.
    pub(crate) fn labeled(label: &[u8]) -> Self {
        let mut w = Self(Vec::with_capacity(label.len() + 128));
        w.bytes(label);
        w.u8(0);
        w
    }
    pub(crate) fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub(crate) fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub(crate) fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub(crate) fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// Canonical binary reader matching [`Writer`].
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| CoreError::statement("unexpected end of data"))?;
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let s = self.take(N)?;
        <[u8; N]>::try_from(s).map_err(|_| CoreError::statement("length"))
    }
    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    pub(crate) fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    /// Consumes `label || 0x00` or fails.
    pub(crate) fn expect_label(&mut self, label: &[u8]) -> Result<()> {
        let got = self.take(label.len() + 1)?;
        if &got[..label.len()] != label || got[label.len()] != 0 {
            return Err(CoreError::statement("unexpected label"));
        }
        Ok(())
    }
    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
}

/// Parses a canonical (lowercase, hyphenated) UUID into its 16 bytes (spec §3.2).
pub(crate) fn parse_uuid(s: &str, what: &str) -> Result<[u8; 16]> {
    let u =
        uuid::Uuid::parse_str(s).map_err(|_| CoreError::input(format!("{what}: not a UUID")))?;
    if u.hyphenated().to_string() != s {
        return Err(CoreError::input(format!(
            "{what}: UUID must be lowercase hyphenated"
        )));
    }
    Ok(*u.as_bytes())
}

/// Formats 16 bytes as a canonical UUID string.
pub(crate) fn format_uuid(bytes: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*bytes).hyphenated().to_string()
}

/// Generates a random UUID v4 string (item id, space id, blob id).
pub(crate) fn new_uuid() -> Result<String> {
    let mut b: [u8; 16] = random_array()?;
    // RFC 9562 §5.4: version 4, variant 10.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format_uuid(&b))
}

/// New random item id (UUID v4).
pub fn new_item_id() -> Result<String> {
    new_uuid()
}

/// New random space id (UUID v4), generated once when the space is created.
pub fn new_space_id() -> Result<String> {
    new_uuid()
}

/// New random attachment blob id (UUID v4).
pub fn new_blob_id() -> Result<String> {
    new_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_roundtrip_and_canonical() {
        let id = new_item_id().unwrap();
        let b = parse_uuid(&id, "id").unwrap();
        assert_eq!(format_uuid(&b), id);
        assert!(parse_uuid(&id.to_uppercase(), "id").is_err());
        assert!(parse_uuid(&id.replace('-', ""), "id").is_err());
    }

    #[test]
    fn space_key_debug_is_redacted() {
        let k = SpaceKey::from_bytes(1, &[7u8; 32]).unwrap();
        let dbg = format!("{k:?}");
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains("[7, 7"));
        assert!(SpaceKey::from_bytes(0, &[0u8; 32]).is_err());
    }
}
