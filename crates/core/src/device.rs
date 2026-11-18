//! Device identities, space key transport, signed membership statements and the local
//! trust store (spec §7, §8).
//!
//! Each device owns an Ed25519 signing key (statements, relay authentication) and an
//! X25519 key (receiving the space key). A device only encrypts the space key for X25519
//! keys held in its own [`TrustStore`], never for a key handed over by the relay.

use std::collections::{BTreeMap, BTreeSet};

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as XPublic, StaticSecret};
use zeroize::Zeroizing;

use crate::crypto::{
    aead_open, aead_seal, check_space_key_epoch, format_uuid, hkdf_key, labels, parse_uuid,
    random_array, sha256, to_array, Reader, SpaceKey, Writer, KEY_LEN, NONCE_LEN,
};
use crate::error::{CoreError, Result};
use crate::format::SPACE_KEY_EPOCH_MAX;

/// Ed25519 / X25519 public key length.
pub const PUBLIC_KEY_LEN: usize = 32;
/// Ed25519 signature length.
pub const SIGNATURE_LEN: usize = 64;
/// Length of a sealed space key: ephemeral public (32) || nonce (24) || ciphertext (48).
pub const SEALED_SPACE_KEY_LEN: usize = PUBLIC_KEY_LEN + NONCE_LEN + KEY_LEN + 16;
/// Upper bound on list sizes in statements (sanity limit, spec §8.1).
pub const MAX_STATEMENT_ENTRIES: u32 = 1024;

type Pk = [u8; PUBLIC_KEY_LEN];

/// Public half of a device identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DevicePublic {
    /// Ed25519 verifying key (32 bytes).
    pub ed25519: Vec<u8>,
    /// X25519 public key (32 bytes).
    pub x25519: Vec<u8>,
}

impl DevicePublic {
    pub(crate) fn arrays(&self) -> Result<(Pk, Pk)> {
        let ed = to_array(&self.ed25519, "ed25519 public key")?;
        check_ed25519_public(&ed)?;
        let x = to_array(&self.x25519, "x25519 public key")?;
        Ok((ed, x))
    }

    /// Device id derived from the Ed25519 key (spec §7.1).
    pub fn device_id(&self) -> Result<String> {
        device_id_from_ed25519(&self.ed25519)
    }
}

/// `hex(SHA-256("tomi/v1/device-id" || 0x00 || ed25519_public)[0..16])`.
pub fn device_id_from_ed25519(ed25519_public: &[u8]) -> Result<String> {
    let ed: Pk = to_array(ed25519_public, "ed25519 public key")?;
    let h = sha256(&[labels::DEVICE_ID, &[0u8], &ed]);
    Ok(hex::encode(&h[..16]))
}

/// Rejects keys that do not decode or have small order (spec §2.4).
fn check_ed25519_public(ed: &Pk) -> Result<VerifyingKey> {
    let vk = VerifyingKey::from_bytes(ed)
        .map_err(|_| CoreError::input("ed25519 public key does not decode"))?;
    if vk.is_weak() {
        return Err(CoreError::input("ed25519 public key has small order"));
    }
    Ok(vk)
}

/// Strict Ed25519 verification (spec §2.4).
pub(crate) fn verify_signature(ed: &Pk, message: &[u8], signature: &[u8]) -> Result<()> {
    let sig: [u8; SIGNATURE_LEN] =
        to_array(signature, "signature").map_err(|_| CoreError::InvalidSignature)?;
    let vk = check_ed25519_public(ed).map_err(|_| CoreError::InvalidSignature)?;
    vk.verify_strict(message, &Signature::from_bytes(&sig))
        .map_err(|_| CoreError::InvalidSignature)
}

/// Secret identity of this device. Zeroized on drop; `Debug` shows only the id.
pub struct DeviceKeys {
    signing: SigningKey,
    x_secret: StaticSecret,
}

impl DeviceKeys {
    /// Generates both key pairs from the OS CSPRNG (once, at installation).
    pub fn generate() -> Result<Self> {
        let ed: Zeroizing<[u8; 32]> = Zeroizing::new(random_array()?);
        let x: Zeroizing<[u8; 32]> = Zeroizing::new(random_array()?);
        Self::from_secrets(ed.as_slice(), x.as_slice())
    }

    /// Restores keys from secure storage: 32-byte Ed25519 seed and 32-byte X25519 secret.
    pub fn from_secrets(ed25519_seed: &[u8], x25519_secret: &[u8]) -> Result<Self> {
        let seed = Zeroizing::new(to_array::<32>(ed25519_seed, "ed25519 seed")?);
        let xs = Zeroizing::new(to_array::<32>(x25519_secret, "x25519 secret")?);
        Ok(Self {
            signing: SigningKey::from_bytes(&seed),
            x_secret: StaticSecret::from(*xs),
        })
    }

    /// Ed25519 seed, only to persist it in secure storage.
    pub fn ed25519_seed(&self) -> Vec<u8> {
        self.signing.to_bytes().to_vec()
    }

    /// X25519 secret, only to persist it in secure storage.
    pub fn x25519_secret(&self) -> Vec<u8> {
        self.x_secret.to_bytes().to_vec()
    }

    /// Public keys of this device.
    pub fn public(&self) -> DevicePublic {
        DevicePublic {
            ed25519: self.ed_public().to_vec(),
            x25519: XPublic::from(&self.x_secret).to_bytes().to_vec(),
        }
    }

    /// Device id of this device.
    pub fn device_id(&self) -> String {
        let h = sha256(&[labels::DEVICE_ID, &[0u8], &self.ed_public()]);
        hex::encode(&h[..16])
    }

    pub(crate) fn ed_public(&self) -> Pk {
        self.signing.verifying_key().to_bytes()
    }

    pub(crate) fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing.sign(message).to_bytes().to_vec()
    }

    /// X25519 public key of this device.
    pub(crate) fn x_public(&self) -> Pk {
        XPublic::from(&self.x_secret).to_bytes()
    }

    /// X25519 agreement with `peer`; `None` if the result is all zeros (small-order key).
    pub(crate) fn x_agree(&self, peer: &Pk) -> Option<Zeroizing<[u8; 32]>> {
        let shared = self.x_secret.diffie_hellman(&XPublic::from(*peer));
        if shared.was_contributory() {
            Some(Zeroizing::new(shared.to_bytes()))
        } else {
            None
        }
    }

    /// Signs a [`DeviceCert`] vouching for `subject` in `space_id` (pairing, spec §8.2).
    pub fn sign_device_cert(
        &self,
        space_id: &str,
        subject: &DevicePublic,
        issued_at_ms: u64,
    ) -> Result<DeviceCert> {
        let mut cert = DeviceCert {
            space_id: space_id.to_owned(),
            subject: subject.clone(),
            issued_at_ms,
            issuer_ed25519: self.ed_public().to_vec(),
            signature: Vec::new(),
        };
        cert.signature = self.sign(&cert.tbs()?);
        Ok(cert)
    }

    /// Opens a space key sealed for this device (pairing or rotation, spec §7.3).
    pub fn open_space_key(
        &self,
        space_id: &str,
        key_epoch: u32,
        sealed: &[u8],
    ) -> Result<SpaceKey> {
        let sid = parse_uuid(space_id, "space id")?;
        open_sealed(self, &sid, key_epoch, sealed)
    }
}

impl std::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("device_id", &self.device_id())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Space key transport (spec §7.3)
// ---------------------------------------------------------------------------

pub(crate) fn aad_key_transport(space_id: &[u8; 16], key_epoch: u32, recipient_ed: &Pk) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_KEY_TRANSPORT);
    w.bytes(space_id).u32(key_epoch).bytes(recipient_ed);
    w.finish()
}

pub(crate) fn transport_key(
    shared: &[u8; 32],
    ephemeral_public: &Pk,
    recipient_x: &Pk,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(ephemeral_public);
    salt[32..].copy_from_slice(recipient_x);
    hkdf_key(Some(&salt), shared, labels::HKDF_KEY_TRANSPORT)
}

/// Seals with explicit ephemeral secret and nonce (KAT hook and internal use).
pub(crate) fn seal_space_key_raw(
    recipient: &DevicePublic,
    space_id: &[u8; 16],
    space_key: &SpaceKey,
    ephemeral_secret: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>> {
    let (recipient_ed, recipient_x) = recipient.arrays()?;
    let eph = StaticSecret::from(*ephemeral_secret);
    let eph_pub = XPublic::from(&eph).to_bytes();
    let shared = eph.diffie_hellman(&XPublic::from(recipient_x));
    if !shared.was_contributory() {
        return Err(CoreError::input("recipient x25519 key has small order"));
    }
    let k = transport_key(shared.as_bytes(), &eph_pub, &recipient_x)?;
    let ct = aead_seal(
        &k,
        nonce,
        &aad_key_transport(space_id, space_key.epoch(), &recipient_ed),
        space_key.raw(),
    )?;
    let mut out = Vec::with_capacity(SEALED_SPACE_KEY_LEN);
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn seal_space_key_random(
    recipient: &DevicePublic,
    space_id: &[u8; 16],
    space_key: &SpaceKey,
) -> Result<Vec<u8>> {
    let eph: Zeroizing<[u8; 32]> = Zeroizing::new(random_array()?);
    let nonce: [u8; NONCE_LEN] = random_array()?;
    seal_space_key_raw(recipient, space_id, space_key, &eph, &nonce)
}

/// Seals `space_key` for `recipient` (used at pairing, spec §8.2). The recipient's
/// keys must come from a verified source (PAKE channel or trust store).
pub fn seal_space_key(
    recipient: &DevicePublic,
    space_id: &str,
    space_key: &SpaceKey,
) -> Result<Vec<u8>> {
    let sid = parse_uuid(space_id, "space id")?;
    seal_space_key_random(recipient, &sid, space_key)
}

fn open_sealed(
    own: &DeviceKeys,
    space_id: &[u8; 16],
    key_epoch: u32,
    sealed: &[u8],
) -> Result<SpaceKey> {
    if sealed.len() != SEALED_SPACE_KEY_LEN {
        return Err(CoreError::input("sealed space key: wrong length"));
    }
    let eph_pub: Pk = to_array(&sealed[..32], "ephemeral key")?;
    let nonce: [u8; NONCE_LEN] = to_array(&sealed[32..56], "nonce")?;
    let shared = own.x_secret.diffie_hellman(&XPublic::from(eph_pub));
    if !shared.was_contributory() {
        return Err(CoreError::DecryptionFailed);
    }
    let own_x = XPublic::from(&own.x_secret).to_bytes();
    let k = transport_key(shared.as_bytes(), &eph_pub, &own_x)?;
    let key = Zeroizing::new(aead_open(
        &k,
        &nonce,
        &aad_key_transport(space_id, key_epoch, &own.ed_public()),
        &sealed[56..],
    )?);
    SpaceKey::from_bytes(key_epoch, &key)
}

// ---------------------------------------------------------------------------
// Signed statements (spec §8.1). Wire form = TBS || signature (64 bytes).
// ---------------------------------------------------------------------------

fn split_signature(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    if bytes.len() < SIGNATURE_LEN {
        return Err(CoreError::statement("too short"));
    }
    Ok(bytes.split_at(bytes.len() - SIGNATURE_LEN))
}

fn wire(tbs: Vec<u8>, signature: &[u8]) -> Result<Vec<u8>> {
    if signature.len() != SIGNATURE_LEN {
        return Err(CoreError::input("signature: expected 64 bytes"));
    }
    let mut out = tbs;
    out.extend_from_slice(signature);
    Ok(out)
}

fn check_count(n: usize) -> Result<u32> {
    u32::try_from(n)
        .ok()
        .filter(|&n| n <= MAX_STATEMENT_ENTRIES)
        .ok_or_else(|| CoreError::input("too many entries"))
}

fn read_count(r: &mut Reader<'_>) -> Result<u32> {
    let n = r.u32()?;
    if n > MAX_STATEMENT_ENTRIES {
        return Err(CoreError::statement("too many entries"));
    }
    Ok(n)
}

fn strictly_sorted(keys: &[Pk]) -> bool {
    keys.windows(2).all(|w| w[0] < w[1])
}

/// The issuer vouches that `subject` belongs to the space (spec §8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCert {
    pub space_id: String,
    pub subject: DevicePublic,
    pub issued_at_ms: u64,
    pub issuer_ed25519: Vec<u8>,
    pub signature: Vec<u8>,
}

impl DeviceCert {
    fn tbs(&self) -> Result<Vec<u8>> {
        let sid = parse_uuid(&self.space_id, "space id")?;
        let (ed, x) = self.subject.arrays()?;
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        let mut w = Writer::labeled(labels::SIG_DEVICE_CERT);
        w.bytes(&sid)
            .bytes(&ed)
            .bytes(&x)
            .u64(self.issued_at_ms)
            .bytes(&issuer);
        Ok(w.finish())
    }

    /// Bytes to sign (exposed for test vectors and the relay).
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature`.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        wire(self.tbs()?, &self.signature)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (tbs, sig) = split_signature(bytes)?;
        let mut r = Reader::new(tbs);
        r.expect_label(labels::SIG_DEVICE_CERT)?;
        let sid: [u8; 16] = r.array()?;
        let ed: Pk = r.array()?;
        let x: Pk = r.array()?;
        let issued_at_ms = r.u64()?;
        let issuer: Pk = r.array()?;
        if r.remaining() != 0 {
            return Err(CoreError::statement("trailing bytes"));
        }
        Ok(Self {
            space_id: format_uuid(&sid),
            subject: DevicePublic {
                ed25519: ed.to_vec(),
                x25519: x.to_vec(),
            },
            issued_at_ms,
            issuer_ed25519: issuer.to_vec(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `issuer_ed25519` only (no trust decision).
    pub fn verify_signature(&self) -> Result<()> {
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        verify_signature(&issuer, &self.tbs()?, &self.signature)
    }

    /// A self-signed certificate proves key possession only; it never grants trust.
    pub fn is_self_signed(&self) -> bool {
        self.issuer_ed25519 == self.subject.ed25519
    }
}

/// Snapshot of the members, signed by the inviter for a new device (spec §8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberList {
    pub space_id: String,
    pub key_epoch: u32,
    pub issued_at_ms: u64,
    pub issuer_ed25519: Vec<u8>,
    /// Sorted by Ed25519 key, ascending, no duplicates.
    pub members: Vec<DevicePublic>,
    pub signature: Vec<u8>,
}

impl MemberList {
    fn tbs(&self) -> Result<Vec<u8>> {
        let sid = parse_uuid(&self.space_id, "space id")?;
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        let members = self
            .members
            .iter()
            .map(DevicePublic::arrays)
            .collect::<Result<Vec<_>>>()?;
        let eds: Vec<Pk> = members.iter().map(|m| m.0).collect();
        if !strictly_sorted(&eds) {
            return Err(CoreError::input("members must be sorted and unique"));
        }
        let mut w = Writer::labeled(labels::SIG_MEMBER_LIST);
        w.bytes(&sid)
            .u32(self.key_epoch)
            .u64(self.issued_at_ms)
            .bytes(&issuer)
            .u32(check_count(members.len())?);
        for (ed, x) in &members {
            w.bytes(ed).bytes(x);
        }
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature`.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        wire(self.tbs()?, &self.signature)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (tbs, sig) = split_signature(bytes)?;
        let mut r = Reader::new(tbs);
        r.expect_label(labels::SIG_MEMBER_LIST)?;
        let sid: [u8; 16] = r.array()?;
        let key_epoch = r.u32()?;
        if key_epoch > SPACE_KEY_EPOCH_MAX {
            return Err(CoreError::statement("key_epoch in the reserved range"));
        }
        let issued_at_ms = r.u64()?;
        let issuer: Pk = r.array()?;
        let n = read_count(&mut r)?;
        let mut members = Vec::new();
        let mut eds = Vec::new();
        for _ in 0..n {
            let ed: Pk = r.array()?;
            let x: Pk = r.array()?;
            eds.push(ed);
            members.push(DevicePublic {
                ed25519: ed.to_vec(),
                x25519: x.to_vec(),
            });
        }
        if r.remaining() != 0 {
            return Err(CoreError::statement("trailing bytes"));
        }
        if !strictly_sorted(&eds) {
            return Err(CoreError::statement("members not sorted or duplicated"));
        }
        Ok(Self {
            space_id: format_uuid(&sid),
            key_epoch,
            issued_at_ms,
            issuer_ed25519: issuer.to_vec(),
            members,
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `issuer_ed25519` only.
    pub fn verify_signature(&self) -> Result<()> {
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        verify_signature(&issuer, &self.tbs()?, &self.signature)
    }
}

/// One wrapped copy of the new space key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationRecipient {
    pub ed25519: Vec<u8>,
    /// [`SEALED_SPACE_KEY_LEN`] bytes, see spec §7.3.
    pub sealed_key: Vec<u8>,
}

/// Signed key rotation: new epoch, revoked devices, new key sealed per member (spec §8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRotation {
    pub space_id: String,
    pub new_epoch: u32,
    pub issued_at_ms: u64,
    pub issuer_ed25519: Vec<u8>,
    /// Revoked Ed25519 keys, sorted ascending, unique.
    pub revoked: Vec<Vec<u8>>,
    /// Sorted by Ed25519 key, ascending, unique; disjoint from `revoked`.
    pub recipients: Vec<RotationRecipient>,
    pub signature: Vec<u8>,
}

impl KeyRotation {
    fn tbs(&self) -> Result<Vec<u8>> {
        let sid = parse_uuid(&self.space_id, "space id")?;
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        let revoked = self
            .revoked
            .iter()
            .map(|k| to_array::<32>(k, "revoked key"))
            .collect::<Result<Vec<_>>>()?;
        if !strictly_sorted(&revoked) {
            return Err(CoreError::input("revoked must be sorted and unique"));
        }
        let mut rec_eds = Vec::with_capacity(self.recipients.len());
        for r in &self.recipients {
            rec_eds.push(to_array::<32>(&r.ed25519, "recipient key")?);
            if r.sealed_key.len() != SEALED_SPACE_KEY_LEN {
                return Err(CoreError::input("sealed key: wrong length"));
            }
        }
        if !strictly_sorted(&rec_eds) {
            return Err(CoreError::input("recipients must be sorted and unique"));
        }
        let mut w = Writer::labeled(labels::SIG_KEY_ROTATION);
        w.bytes(&sid)
            .u32(self.new_epoch)
            .u64(self.issued_at_ms)
            .bytes(&issuer)
            .u32(check_count(revoked.len())?);
        for k in &revoked {
            w.bytes(k);
        }
        w.u32(check_count(self.recipients.len())?);
        for r in &self.recipients {
            w.bytes(&r.ed25519).bytes(&r.sealed_key);
        }
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature`.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        wire(self.tbs()?, &self.signature)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let (tbs, sig) = split_signature(bytes)?;
        let mut r = Reader::new(tbs);
        r.expect_label(labels::SIG_KEY_ROTATION)?;
        let sid: [u8; 16] = r.array()?;
        let new_epoch = r.u32()?;
        if new_epoch > SPACE_KEY_EPOCH_MAX {
            return Err(CoreError::statement("new_epoch in the reserved range"));
        }
        let issued_at_ms = r.u64()?;
        let issuer: Pk = r.array()?;
        let n_rev = read_count(&mut r)?;
        let mut revoked = Vec::new();
        for _ in 0..n_rev {
            revoked.push(r.array::<32>()?);
        }
        let n_rec = read_count(&mut r)?;
        let mut recipients = Vec::new();
        let mut rec_eds = Vec::new();
        for _ in 0..n_rec {
            let ed: Pk = r.array()?;
            let sealed = r.take(SEALED_SPACE_KEY_LEN)?;
            rec_eds.push(ed);
            recipients.push(RotationRecipient {
                ed25519: ed.to_vec(),
                sealed_key: sealed.to_vec(),
            });
        }
        if r.remaining() != 0 {
            return Err(CoreError::statement("trailing bytes"));
        }
        if !strictly_sorted(&revoked) || !strictly_sorted(&rec_eds) {
            return Err(CoreError::statement("lists not sorted or duplicated"));
        }
        Ok(Self {
            space_id: format_uuid(&sid),
            new_epoch,
            issued_at_ms,
            issuer_ed25519: issuer.to_vec(),
            revoked: revoked.iter().map(|k| k.to_vec()).collect(),
            recipients,
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `issuer_ed25519` only.
    pub fn verify_signature(&self) -> Result<()> {
        let issuer: Pk = to_array(&self.issuer_ed25519, "issuer")?;
        verify_signature(&issuer, &self.tbs()?, &self.signature)
    }
}

/// New rotation plus the new key (for the issuer).
#[derive(Debug, Clone)]
pub struct RotationBundle {
    pub rotation: KeyRotation,
    pub new_space_key: SpaceKey,
}

/// Effect of [`TrustStore::apply_rotation`] on this device.
#[derive(Debug, Clone)]
pub struct RotationOutcome {
    /// New space key for this device (`None` if this device was revoked).
    pub new_space_key: Option<SpaceKey>,
    /// `true` if this device is in the revoked list: it must wipe its space keys.
    pub self_revoked: bool,
}

// ---------------------------------------------------------------------------
// Trust store (spec §8.4)
// ---------------------------------------------------------------------------

/// Local, persisted view of the trusted members of the space.
///
/// Wrap in a `Mutex`-holding object for UniFFI (methods take `&mut self`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustStore {
    space_id: [u8; 16],
    own_ed: Pk,
    key_epoch: u32,
    members: BTreeMap<Pk, Pk>,
    revoked: BTreeSet<Pk>,
}

#[derive(Serialize, Deserialize)]
struct PersistedTrust {
    version: u32,
    space_id: String,
    own_ed25519: String,
    key_epoch: u32,
    members: Vec<PersistedMember>,
    revoked: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct PersistedMember {
    ed25519: String,
    x25519: String,
}

fn hex32(s: &str) -> Result<Pk> {
    let v = hex::decode(s).map_err(|_| CoreError::input("invalid hex"))?;
    to_array(&v, "key")
}

impl TrustStore {
    /// New store for this device (`own`, always a member) at `key_epoch`.
    pub fn new(space_id: &str, own: &DevicePublic, key_epoch: u32) -> Result<Self> {
        let sid = parse_uuid(space_id, "space id")?;
        let (ed, x) = own.arrays()?;
        check_space_key_epoch(key_epoch)?;
        let mut members = BTreeMap::new();
        members.insert(ed, x);
        Ok(Self {
            space_id: sid,
            own_ed: ed,
            key_epoch,
            members,
            revoked: BTreeSet::new(),
        })
    }

    pub fn space_id(&self) -> String {
        format_uuid(&self.space_id)
    }

    /// Latest accepted space key epoch.
    pub fn key_epoch(&self) -> u32 {
        self.key_epoch
    }

    /// Trusted members, sorted by Ed25519 key.
    pub fn members(&self) -> Vec<DevicePublic> {
        self.members
            .iter()
            .map(|(ed, x)| DevicePublic {
                ed25519: ed.to_vec(),
                x25519: x.to_vec(),
            })
            .collect()
    }

    pub fn is_member(&self, ed25519: &[u8]) -> bool {
        to_array::<32>(ed25519, "key").is_ok_and(|k| self.members.contains_key(&k))
    }

    pub fn is_revoked(&self, ed25519: &[u8]) -> bool {
        to_array::<32>(ed25519, "key").is_ok_and(|k| self.revoked.contains(&k))
    }

    pub(crate) fn space_id_bytes(&self) -> [u8; 16] {
        self.space_id
    }

    pub(crate) fn own_ed(&self) -> Pk {
        self.own_ed
    }

    /// X25519 key of a trusted, non-revoked member (D38: never a key from the relay).
    pub(crate) fn member_x25519(&self, ed: &Pk) -> Option<Pk> {
        if self.revoked.contains(ed) {
            return None;
        }
        self.members.get(ed).copied()
    }

    pub(crate) fn require_signer(&self, ed: &Pk) -> Result<()> {
        if self.revoked.contains(ed) {
            return Err(CoreError::RevokedDevice);
        }
        if !self.members.contains_key(ed) {
            return Err(CoreError::UntrustedSigner);
        }
        Ok(())
    }

    fn check_insert(&self, ed: &Pk, x: &Pk) -> Result<bool> {
        if self.revoked.contains(ed) {
            return Err(CoreError::RevokedDevice);
        }
        match self.members.get(ed) {
            Some(existing) if existing != x => Err(CoreError::KeyConflict),
            Some(_) => Ok(false),
            None => Ok(true),
        }
    }

    /// Adds a peer verified face to face (PAKE / QR channel), spec §8.2.
    pub fn add_verified_peer(&mut self, peer: &DevicePublic) -> Result<()> {
        let (ed, x) = peer.arrays()?;
        if self.check_insert(&ed, &x)? {
            self.members.insert(ed, x);
        }
        Ok(())
    }

    /// Accepts a certificate issued by a trusted member (how existing devices learn a
    /// new device).
    pub fn accept_device_cert(&mut self, cert: &DeviceCert) -> Result<()> {
        if parse_uuid(&cert.space_id, "space id")? != self.space_id {
            return Err(CoreError::SpaceMismatch);
        }
        let issuer: Pk = to_array(&cert.issuer_ed25519, "issuer")?;
        if cert.is_self_signed() {
            return Err(CoreError::UntrustedSigner);
        }
        self.require_signer(&issuer)?;
        cert.verify_signature()?;
        let (ed, x) = cert.subject.arrays()?;
        if self.check_insert(&ed, &x)? {
            self.members.insert(ed, x);
        }
        Ok(())
    }

    /// Accepts a member list signed by a trusted member at the current epoch
    /// (how a new device learns the others).
    pub fn accept_member_list(&mut self, list: &MemberList) -> Result<()> {
        if parse_uuid(&list.space_id, "space id")? != self.space_id {
            return Err(CoreError::SpaceMismatch);
        }
        let issuer: Pk = to_array(&list.issuer_ed25519, "issuer")?;
        self.require_signer(&issuer)?;
        list.verify_signature()?;
        if list.key_epoch != self.key_epoch {
            return Err(CoreError::EpochOutOfOrder {
                expected: self.key_epoch,
                got: list.key_epoch,
            });
        }
        let mut to_add = Vec::new();
        for m in &list.members {
            let (ed, x) = m.arrays()?;
            if self.revoked.contains(&ed) {
                continue;
            }
            if self.check_insert(&ed, &x)? {
                to_add.push((ed, x));
            }
        }
        self.members.extend(to_add);
        Ok(())
    }

    /// Signs the current member list (inviter side of pairing).
    pub fn issue_member_list(&self, issuer: &DeviceKeys, issued_at_ms: u64) -> Result<MemberList> {
        self.require_signer(&issuer.ed_public())?;
        let mut list = MemberList {
            space_id: self.space_id(),
            key_epoch: self.key_epoch,
            issued_at_ms,
            issuer_ed25519: issuer.ed_public().to_vec(),
            members: self.members(),
            signature: Vec::new(),
        };
        list.signature = issuer.sign(&list.tbs()?);
        Ok(list)
    }

    /// Creates a rotation to `key_epoch + 1` that revokes `revoke` (Ed25519 keys) and
    /// seals a fresh space key for every other member, using only keys of this store.
    pub fn create_rotation(
        &self,
        issuer: &DeviceKeys,
        revoke: &[Vec<u8>],
        issued_at_ms: u64,
    ) -> Result<RotationBundle> {
        let new_epoch = self
            .key_epoch
            .checked_add(1)
            .ok_or_else(|| CoreError::input("epoch overflow"))?;
        let new_key = SpaceKey::generate(new_epoch)?;
        let n = self.rotation_recipients(issuer, revoke)?.len();
        let mut randomness = Vec::with_capacity(n);
        for _ in 0..n {
            randomness.push((Zeroizing::new(random_array()?), random_array()?));
        }
        self.create_rotation_raw(issuer, revoke, issued_at_ms, new_key, &randomness)
    }

    fn rotation_recipients(&self, issuer: &DeviceKeys, revoke: &[Vec<u8>]) -> Result<Vec<Pk>> {
        let issuer_ed = issuer.ed_public();
        self.require_signer(&issuer_ed)?;
        let mut rev = BTreeSet::new();
        for k in revoke {
            let k: Pk = to_array(k, "revoked key")?;
            if !self.members.contains_key(&k) {
                return Err(CoreError::input("cannot revoke a non-member"));
            }
            if k == issuer_ed {
                return Err(CoreError::input("issuer cannot revoke itself"));
            }
            rev.insert(k);
        }
        Ok(self
            .members
            .keys()
            .filter(|k| !rev.contains(*k))
            .copied()
            .collect())
    }

    pub(crate) fn create_rotation_raw(
        &self,
        issuer: &DeviceKeys,
        revoke: &[Vec<u8>],
        issued_at_ms: u64,
        new_key: SpaceKey,
        randomness: &[(Zeroizing<[u8; 32]>, [u8; NONCE_LEN])],
    ) -> Result<RotationBundle> {
        let recipients = self.rotation_recipients(issuer, revoke)?;
        if randomness.len() != recipients.len() {
            return Err(CoreError::input("randomness count mismatch"));
        }
        if new_key.epoch() != self.key_epoch.saturating_add(1) {
            return Err(CoreError::input("new key epoch must be current + 1"));
        }
        let mut revoked: Vec<Pk> = revoke
            .iter()
            .map(|k| to_array::<32>(k, "revoked key"))
            .collect::<Result<_>>()?;
        revoked.sort();
        revoked.dedup();
        let mut entries = Vec::with_capacity(recipients.len());
        for (ed, (eph, nonce)) in recipients.iter().zip(randomness) {
            let x = self.members.get(ed).ok_or_else(|| CoreError::Internal {
                reason: "recipient vanished".into(),
            })?;
            let public = DevicePublic {
                ed25519: ed.to_vec(),
                x25519: x.to_vec(),
            };
            entries.push(RotationRecipient {
                ed25519: ed.to_vec(),
                sealed_key: seal_space_key_raw(&public, &self.space_id, &new_key, eph, nonce)?,
            });
        }
        let mut rotation = KeyRotation {
            space_id: self.space_id(),
            new_epoch: new_key.epoch(),
            issued_at_ms,
            issuer_ed25519: issuer.ed_public().to_vec(),
            revoked: revoked.iter().map(|k| k.to_vec()).collect(),
            recipients: entries,
            signature: Vec::new(),
        };
        rotation.signature = issuer.sign(&rotation.tbs()?);
        Ok(RotationBundle {
            rotation,
            new_space_key: new_key,
        })
    }

    /// Verifies and applies a rotation (spec §8.3, steps 1-8). Atomic: on error the
    /// store is unchanged.
    pub fn apply_rotation(
        &mut self,
        rotation: &KeyRotation,
        own: &DeviceKeys,
    ) -> Result<RotationOutcome> {
        if own.ed_public() != self.own_ed {
            return Err(CoreError::input("keys do not belong to this store"));
        }
        if parse_uuid(&rotation.space_id, "space id")? != self.space_id {
            return Err(CoreError::SpaceMismatch);
        }
        let issuer: Pk = to_array(&rotation.issuer_ed25519, "issuer")?;
        self.require_signer(&issuer)?;
        rotation.verify_signature()?;
        let expected = self.key_epoch.saturating_add(1);
        if rotation.new_epoch != expected {
            return Err(CoreError::EpochOutOfOrder {
                expected,
                got: rotation.new_epoch,
            });
        }
        let revoked: BTreeSet<Pk> = rotation
            .revoked
            .iter()
            .map(|k| to_array::<32>(k, "revoked key"))
            .collect::<Result<_>>()?;
        if revoked.contains(&issuer) {
            return Err(CoreError::statement("issuer revokes itself"));
        }
        if rotation
            .recipients
            .iter()
            .any(|r| to_array::<32>(&r.ed25519, "recipient").is_ok_and(|k| revoked.contains(&k)))
        {
            return Err(CoreError::statement("revoked device is also a recipient"));
        }
        let new_space_key = if revoked.contains(&self.own_ed) {
            None
        } else {
            let entry = rotation
                .recipients
                .iter()
                .find(|r| r.ed25519 == self.own_ed)
                .ok_or(CoreError::NotARecipient)?;
            Some(open_sealed(
                own,
                &self.space_id,
                rotation.new_epoch,
                &entry.sealed_key,
            )?)
        };
        for k in &revoked {
            self.members.remove(k);
            self.revoked.insert(*k);
        }
        self.key_epoch = rotation.new_epoch;
        Ok(RotationOutcome {
            self_revoked: new_space_key.is_none(),
            new_space_key,
        })
    }

    /// Verifies a tombstone: same space, signer trusted and not revoked, valid signature.
    pub fn verify_tombstone(&self, tombstone: &crate::tombstone::Tombstone) -> Result<()> {
        if parse_uuid(&tombstone.space_id, "space id")? != self.space_id {
            return Err(CoreError::SpaceMismatch);
        }
        let signer: Pk = to_array(&tombstone.signer_ed25519, "signer")?;
        self.require_signer(&signer)?;
        tombstone.verify_signature()
    }

    /// Verifies the removal of an attachment (spec §8.10): same space, signer trusted and not
    /// revoked, valid signature.
    pub fn verify_attachment_tombstone(
        &self,
        removal: &crate::attachment_tombstone::AttachmentTombstone,
    ) -> Result<()> {
        if parse_uuid(&removal.space_id, "space id")? != self.space_id {
            return Err(CoreError::SpaceMismatch);
        }
        let signer: Pk = to_array(&removal.signer_ed25519, "signer")?;
        self.require_signer(&signer)?;
        removal.verify_signature()
    }

    /// Serializes the store for local persistence (JSON, public data only).
    pub fn to_json(&self) -> Result<String> {
        let p = PersistedTrust {
            version: 1,
            space_id: self.space_id(),
            own_ed25519: hex::encode(self.own_ed),
            key_epoch: self.key_epoch,
            members: self
                .members
                .iter()
                .map(|(ed, x)| PersistedMember {
                    ed25519: hex::encode(ed),
                    x25519: hex::encode(x),
                })
                .collect(),
            revoked: self.revoked.iter().map(hex::encode).collect(),
        };
        serde_json::to_string(&p).map_err(|_| CoreError::Internal {
            reason: "json serialization failed".into(),
        })
    }

    /// Restores a store saved with [`to_json`](Self::to_json).
    pub fn from_json(json: &str) -> Result<Self> {
        let p: PersistedTrust =
            serde_json::from_str(json).map_err(|_| CoreError::input("invalid trust store json"))?;
        if p.version != 1 {
            return Err(CoreError::input("unknown trust store version"));
        }
        let mut members = BTreeMap::new();
        for m in &p.members {
            members.insert(hex32(&m.ed25519)?, hex32(&m.x25519)?);
        }
        let revoked = p
            .revoked
            .iter()
            .map(|s| hex32(s))
            .collect::<Result<BTreeSet<_>>>()?;
        let own_ed = hex32(&p.own_ed25519)?;
        if !members.contains_key(&own_ed) && !revoked.contains(&own_ed) {
            return Err(CoreError::input("own key missing from trust store"));
        }
        Ok(Self {
            space_id: parse_uuid(&p.space_id, "space id")?,
            own_ed,
            key_epoch: p.key_epoch,
            members,
            revoked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACE: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn pairing_and_rotation_flow() {
        let a = DeviceKeys::generate().unwrap();
        let b = DeviceKeys::generate().unwrap();
        let c = DeviceKeys::generate().unwrap();
        let k1 = SpaceKey::generate(1).unwrap();

        let mut ta = TrustStore::new(SPACE, &a.public(), 1).unwrap();
        // A invites B (face to face).
        ta.add_verified_peer(&b.public()).unwrap();
        let sealed = seal_space_key(&b.public(), SPACE, &k1).unwrap();
        let list = ta.issue_member_list(&a, 10).unwrap();
        let mut tb = TrustStore::new(SPACE, &b.public(), 1).unwrap();
        tb.add_verified_peer(&a.public()).unwrap();
        tb.accept_member_list(&list).unwrap();
        assert_eq!(
            b.open_space_key(SPACE, 1, &sealed).unwrap().to_bytes(),
            k1.to_bytes()
        );

        // B invites C; A learns C through B's certificate.
        let cert = b.sign_device_cert(SPACE, &c.public(), 20).unwrap();
        let cert = DeviceCert::from_bytes(&cert.to_bytes().unwrap()).unwrap();
        ta.accept_device_cert(&cert).unwrap();
        tb.add_verified_peer(&c.public()).unwrap();
        let mut tc = TrustStore::new(SPACE, &c.public(), 1).unwrap();
        tc.add_verified_peer(&b.public()).unwrap();
        tc.accept_member_list(&tb.issue_member_list(&b, 21).unwrap())
            .unwrap();
        assert!(tc.is_member(&a.public().ed25519));

        // A revokes B.
        let bundle = ta.create_rotation(&a, &[b.public().ed25519], 30).unwrap();
        let rot = KeyRotation::from_bytes(&bundle.rotation.to_bytes().unwrap()).unwrap();
        let out_c = tc.apply_rotation(&rot, &c).unwrap();
        assert_eq!(
            out_c.new_space_key.unwrap().to_bytes(),
            bundle.new_space_key.to_bytes()
        );
        let out_b = tb.apply_rotation(&rot, &b).unwrap();
        assert!(out_b.self_revoked);
        let out_a = ta.apply_rotation(&rot, &a).unwrap();
        assert!(out_a.new_space_key.is_some());
        assert!(ta.is_revoked(&b.public().ed25519));
        // Replaying the same rotation is refused.
        assert_eq!(
            tc.apply_rotation(&rot, &c).unwrap_err().code(),
            "epoch_out_of_order"
        );
        // A revoked device can no longer vouch for anybody.
        let d = DeviceKeys::generate().unwrap();
        let bad = b.sign_device_cert(SPACE, &d.public(), 40).unwrap();
        assert_eq!(
            ta.accept_device_cert(&bad).unwrap_err().code(),
            "revoked_device"
        );

        let restored = TrustStore::from_json(&ta.to_json().unwrap()).unwrap();
        assert_eq!(restored, ta);
    }
}
