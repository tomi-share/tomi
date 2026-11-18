//! Independent implementation of `spec/pairing-v1.md` (§3.3, §5, §6, §8) and of the signed
//! bytes of `spec/format-v1.md` (§7.3, §8.2), written from the text of the specifications and
//! from the primitives crates only (`curve25519-dalek`, `sha2`, `hkdf`, `chacha20poly1305`,
//! `ed25519-dalek`, `x25519-dalek`).
//!
//! It shares **no code** with `tomi_core::pairing`: the frozen vectors of
//! `spec/vectors/pairing-v1/` are produced and checked with it, then the pairing machines of
//! the core are checked against the same bytes. Included with `#[path]` by
//! `tests/pairing_vectors.rs` and `examples/gen_pairing_vectors.rs`.

#![allow(dead_code)]

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use ed25519_dalek::{Signer, SigningKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha256, Sha512};

pub const VERSION: u8 = 1;
pub const DSI: &[u8] = b"CPaceRistretto255";
pub const DSI_ISK: &[u8] = b"CPaceRistretto255_ISK";

/// Frame names and types, in the order of the nominal sequence (§6.1, §7.1).
pub const FRAME_NAMES: [&str; 7] = [
    "commit",
    "share",
    "reveal",
    "joiner_hello",
    "joiner_confirm",
    "provision",
    "joiner_ack",
];
pub const FRAME_TYPES: [u8; 7] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];

pub fn h(b: &[u8]) -> String {
    hex::encode(b)
}

pub fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).expect("hex")
}

pub fn arr<const N: usize>(b: &[u8]) -> [u8; N] {
    b.try_into().expect("length")
}

pub fn seq(start: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| start.wrapping_add(i as u8)).collect()
}

pub fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut s = Sha256::new();
    for p in parts {
        s.update(p);
    }
    s.finalize().into()
}

pub fn sha512(parts: &[&[u8]]) -> [u8; 64] {
    let mut s = Sha512::new();
    for p in parts {
        s.update(p);
    }
    s.finalize().into()
}

// ---------------------------------------------------------------------------
// CPace (draft-irtf-cfrg-cpace-21 §6.3, §7.2, §8.3, appendix A), pairing-v1.md §3.3
// ---------------------------------------------------------------------------

/// `prepend_len` of draft A.1.1, transcribed from the Python of the draft.
pub fn prepend_len(data: &[u8]) -> Vec<u8> {
    let mut length = data.len();
    let mut encoded = Vec::new();
    loop {
        if length < 128 {
            encoded.push(length as u8);
        } else {
            encoded.push(((length & 0x7f) + 0x80) as u8);
        }
        length >>= 7;
        if length == 0 {
            break;
        }
    }
    encoded.extend_from_slice(data);
    encoded
}

/// `lv_cat` of draft A.1.3.
pub fn lv_cat(args: &[&[u8]]) -> Vec<u8> {
    args.iter().flat_map(|a| prepend_len(a)).collect()
}

/// `generator_string` of draft A.2 with `s_in_bytes = 128`.
pub fn generator_string(prs: &[u8], ci: &[u8], sid: &[u8]) -> Vec<u8> {
    let len_zpad = 128usize.saturating_sub(1 + prepend_len(prs).len() + prepend_len(DSI).len());
    lv_cat(&[DSI, prs, &vec![0u8; len_zpad], ci, sid])
}

/// `g = element_derivation(SHA-512(gen_str))`.
pub fn generator(gen_str: &[u8]) -> RistrettoPoint {
    RistrettoPoint::from_uniform_bytes(&sha512(&[gen_str]))
}

/// The scalar of `sample_scalar`: little endian, below 2^252 (no reduction happens).
pub fn scalar(y: &[u8; 32]) -> Scalar {
    assert!(y[31] & 0xf0 == 0, "sample_scalar clears the bits above 251");
    Scalar::from_bytes_mod_order(*y)
}

/// `encode(y * g)`.
pub fn share(y: &[u8; 32], g: &RistrettoPoint) -> [u8; 32] {
    (scalar(y) * g).compress().to_bytes()
}

/// `scalar_mult_vfy(y, X)`: the neutral element `I` (32 zero bytes) if `decode(X)` fails.
pub fn scalar_mult_vfy(y: &[u8; 32], x: &[u8; 32]) -> [u8; 32] {
    match CompressedRistretto(*x).decompress() {
        Some(p) => (scalar(y) * p).compress().to_bytes(),
        None => [0u8; 32],
    }
}

/// `ISK = SHA-512(lv_cat(DSI_ISK, sid, K) || lv_cat(Ya, ADa) || lv_cat(Yb, ADb))`.
pub fn isk(sid: &[u8], k: &[u8], ya: &[u8], ada: &[u8], yb: &[u8], adb: &[u8]) -> [u8; 64] {
    let mut m = lv_cat(&[DSI_ISK, sid, k]);
    m.extend(lv_cat(&[ya, ada]));
    m.extend(lv_cat(&[yb, adb]));
    sha512(&[&m])
}

// ---------------------------------------------------------------------------
// Pairing (pairing-v1.md §3.3, §4, §5, §6)
// ---------------------------------------------------------------------------

pub fn prs(ranks: [u16; 2]) -> [u8; 4] {
    let mut out = [0u8; 4];
    out[..2].copy_from_slice(&ranks[0].to_be_bytes());
    out[2..].copy_from_slice(&ranks[1].to_be_bytes());
    out
}

/// `CI = lv_cat("tomi/v1/pair-ci", relay_authority, channel_id)`.
pub fn ci(relay_authority: &str, channel_id: &str) -> Vec<u8> {
    lv_cat(&[
        b"tomi/v1/pair-ci",
        relay_authority.as_bytes(),
        channel_id.as_bytes(),
    ])
}

/// `label || 0x00 || u8(PAIR_VERSION)`.
pub fn associated_data(label: &str) -> Vec<u8> {
    let mut v = label.as_bytes().to_vec();
    v.push(0);
    v.push(VERSION);
    v
}

pub fn ada() -> Vec<u8> {
    associated_data("tomi/v1/pair-inviter")
}

pub fn adb() -> Vec<u8> {
    associated_data("tomi/v1/pair-joiner")
}

/// `commit = SHA-256("tomi/v1/pair-commit" || 0x00 || sid || Ya)`.
pub fn commit(sid: &[u8], ya: &[u8]) -> [u8; 32] {
    sha256(&[b"tomi/v1/pair-commit", &[0], sid, ya])
}

/// `HKDF(∅, ISK, info, len)`.
pub fn kdf(isk: &[u8], info: &str, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(None, isk)
        .expand(info.as_bytes(), &mut out)
        .expect("hkdf length");
    out
}

/// `SAS = u64be(s8) mod 1 000 000`.
pub fn sas(isk: &[u8]) -> u32 {
    let s8 = kdf(isk, "tomi/v1/hkdf/pair-sas", 8);
    (u64::from_be_bytes(arr(&s8)) % 1_000_000) as u32
}

/// `AAD(t) = "tomi/v1/aad/pair" || 0x00 || u8(PAIR_VERSION) || u8(t) || sid`.
pub fn aad(t: u8, sid: &[u8]) -> Vec<u8> {
    let mut v = b"tomi/v1/aad/pair".to_vec();
    v.push(0);
    v.push(VERSION);
    v.push(t);
    v.extend_from_slice(sid);
    v
}

/// `nonce(n) = zero_bytes(20) || u32be(n)`.
pub fn nonce(n: u32) -> [u8; 24] {
    let mut out = [0u8; 24];
    out[20..].copy_from_slice(&n.to_be_bytes());
    out
}

pub fn xc_seal(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    XChaCha20Poly1305::new(&Key::from(*key))
        .encrypt(&XNonce::from(*nonce), Payload { msg: pt, aad })
        .expect("seal")
}

pub fn xc_open(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    XChaCha20Poly1305::new(&Key::from(*key))
        .decrypt(&XNonce::from(*nonce), Payload { msg: ct, aad })
        .ok()
}

/// The two directional keys (§5.2).
#[derive(Clone)]
pub struct Keys {
    pub k_i: [u8; 32],
    pub k_j: [u8; 32],
}

impl Keys {
    pub fn from_isk(isk: &[u8]) -> Self {
        Self {
            k_i: arr(&kdf(isk, "tomi/v1/hkdf/pair-inviter", 32)),
            k_j: arr(&kdf(isk, "tomi/v1/hkdf/pair-joiner", 32)),
        }
    }
}

/// Key and counter of a sealed frame type (§5.4 table).
pub fn sealing(t: u8) -> Option<(bool, u32)> {
    // (true = K_I, counter)
    match t {
        0x03 => Some((true, 0)),
        0x06 => Some((true, 1)),
        0x04 => Some((false, 0)),
        0x05 => Some((false, 1)),
        0x07 => Some((false, 2)),
        _ => None,
    }
}

/// `box = XC_Seal(K_dir, nonce(n), AAD(t), body)` for the sealed frame type `t`.
pub fn seal_box(keys: &Keys, t: u8, sid: &[u8], body: &[u8]) -> Vec<u8> {
    let (inviter, n) = sealing(t).expect("sealed type");
    let key = if inviter { &keys.k_i } else { &keys.k_j };
    xc_seal(key, &nonce(n), &aad(t, sid), body)
}

pub fn open_box(keys: &Keys, t: u8, sid: &[u8], boxed: &[u8]) -> Option<Vec<u8>> {
    let (inviter, n) = sealing(t)?;
    let key = if inviter { &keys.k_i } else { &keys.k_j };
    xc_open(key, &nonce(n), &aad(t, sid), boxed)
}

/// `u8(PAIR_VERSION) || u8(type) || payload`.
pub fn frame(t: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![VERSION, t];
    v.extend_from_slice(payload);
    v
}

/// A sealed frame (not `reveal`, whose payload starts with `Ya`).
pub fn sealed_frame(keys: &Keys, t: u8, sid: &[u8], body: &[u8]) -> Vec<u8> {
    frame(t, &seal_box(keys, t, sid, body))
}

/// A `reveal` frame: `Ya || box(K_I, 0, body)`.
pub fn reveal_frame(keys: &Keys, sid: &[u8], ya: &[u8], body: &[u8]) -> Vec<u8> {
    let mut payload = ya.to_vec();
    payload.extend(seal_box(keys, 0x03, sid, body));
    frame(0x03, &payload)
}

/// `tlv = u8(tag) || u16be(len) || value`.
pub fn tlv(parts: &[(u8, &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    for (tag, value) in parts {
        v.push(*tag);
        v.extend_from_slice(
            &u16::try_from(value.len())
                .expect("tlv length")
                .to_be_bytes(),
        );
        v.extend_from_slice(value);
    }
    v
}

pub fn inviter_hello(
    ed: &[u8; 32],
    x: &[u8; 32],
    name: &str,
    platform: &str,
    member_count: Option<u16>,
    offer: Option<(u64, u64)>,
) -> Vec<u8> {
    let count = member_count.map(|c| c.to_be_bytes());
    let offer_bytes = offer.map(|(items, bytes)| {
        let mut v = items.to_be_bytes().to_vec();
        v.extend(bytes.to_be_bytes());
        v
    });
    let mut parts: Vec<(u8, &[u8])> = vec![
        (1, ed),
        (2, x),
        (3, name.as_bytes()),
        (4, platform.as_bytes()),
    ];
    if let Some(c) = &count {
        parts.push((5, c));
    }
    if let Some(o) = &offer_bytes {
        parts.push((6, o));
    }
    tlv(&parts)
}

pub fn joiner_hello(ed: &[u8; 32], x: &[u8; 32], name: &str, platform: &str) -> Vec<u8> {
    tlv(&[
        (1, ed),
        (2, x),
        (3, name.as_bytes()),
        (4, platform.as_bytes()),
    ])
}

pub fn joiner_confirm(history: bool) -> Vec<u8> {
    tlv(&[(1, &[u8::from(history)])])
}

/// `Provision` body (§6.6).
pub fn provision_body(
    space_id: &[u8; 16],
    key_epoch: u32,
    sealed: &[u8],
    cert: &[u8],
    list: &[u8],
    history: Option<([u8; 16], u64, u64)>,
) -> Vec<u8> {
    let epoch = key_epoch.to_be_bytes();
    let hist = history.map(|(id, items, bytes)| {
        let mut v = id.to_vec();
        v.extend(items.to_be_bytes());
        v.extend(bytes.to_be_bytes());
        v
    });
    let mut parts: Vec<(u8, &[u8])> = vec![
        (1, space_id),
        (2, &epoch),
        (3, sealed),
        (4, cert),
        (5, list),
    ];
    if let Some(hv) = &hist {
        parts.push((6, hv));
    }
    tlv(&parts)
}

// ---------------------------------------------------------------------------
// Signed bytes of format-v1.md (§7.1, §7.3, §8.2)
// ---------------------------------------------------------------------------

/// A device of `spec/vectors/v1/device.json`: seeds, and public keys computed from them.
#[derive(Clone)]
pub struct Device {
    pub name: &'static str,
    pub ed_seed: [u8; 32],
    pub x_secret: [u8; 32],
    pub ed_pk: [u8; 32],
    pub x_pk: [u8; 32],
}

impl Device {
    pub fn new(name: &'static str, ed: u8, x: u8) -> Self {
        let ed_seed: [u8; 32] = arr(&seq(ed, 32));
        let x_secret: [u8; 32] = arr(&seq(x, 32));
        let ed_pk = SigningKey::from_bytes(&ed_seed).verifying_key().to_bytes();
        let x_pk =
            x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(x_secret)).to_bytes();
        Self {
            name,
            ed_seed,
            x_secret,
            ed_pk,
            x_pk,
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        SigningKey::from_bytes(&self.ed_seed)
            .sign(msg)
            .to_bytes()
            .to_vec()
    }

    /// `device_id = hex(SHA-256("tomi/v1/device-id" || 0x00 || ed_pk)[0..16])`.
    pub fn device_id(&self) -> String {
        h(&sha256(&[b"tomi/v1/device-id", &[0], &self.ed_pk])[..16])
    }
}

/// The devices A, B, C of `device.json`.
pub fn device_a() -> Device {
    Device::new("A", 0xa0, 0x10)
}
pub fn device_b() -> Device {
    Device::new("B", 0xc0, 0x30)
}
pub fn device_c() -> Device {
    Device::new("C", 0xe0, 0x50)
}

pub fn uuid16(s: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(s).expect("uuid").as_bytes()
}

/// `DeviceCert` (§8.2): `TBS || signature`, 208 bytes.
pub fn device_cert(
    issuer: &Device,
    space_id: &str,
    subject_ed: &[u8; 32],
    subject_x: &[u8; 32],
    issued_at_ms: u64,
) -> Vec<u8> {
    let mut tbs = b"tomi/v1/sig/device-cert".to_vec();
    tbs.push(0);
    tbs.extend(uuid16(space_id));
    tbs.extend(subject_ed);
    tbs.extend(subject_x);
    tbs.extend(issued_at_ms.to_be_bytes());
    tbs.extend(issuer.ed_pk);
    let mut wire = tbs.clone();
    wire.extend(issuer.sign(&tbs));
    wire
}

/// `MemberList` (§8.2): `TBS || signature`, members sorted by Ed25519 key.
pub fn member_list(
    issuer: &Device,
    space_id: &str,
    key_epoch: u32,
    issued_at_ms: u64,
    members: &[([u8; 32], [u8; 32])],
) -> Vec<u8> {
    let mut sorted = members.to_vec();
    sorted.sort();
    let mut tbs = b"tomi/v1/sig/member-list".to_vec();
    tbs.push(0);
    tbs.extend(uuid16(space_id));
    tbs.extend(key_epoch.to_be_bytes());
    tbs.extend(issued_at_ms.to_be_bytes());
    tbs.extend(issuer.ed_pk);
    tbs.extend((sorted.len() as u32).to_be_bytes());
    for (ed, x) in &sorted {
        tbs.extend(ed);
        tbs.extend(x);
    }
    let mut wire = tbs.clone();
    wire.extend(issuer.sign(&tbs));
    wire
}

/// The clear space key sealed for a device (§7.3): 104 bytes.
pub fn sealed_space_key(
    space_id: &str,
    key_epoch: u32,
    space_key: &[u8; 32],
    ephemeral_secret: &[u8; 32],
    nonce: &[u8; 24],
    recipient_ed: &[u8; 32],
    recipient_x: &[u8; 32],
) -> Vec<u8> {
    let eph = x25519_dalek::StaticSecret::from(*ephemeral_secret);
    let eph_pub = x25519_dalek::PublicKey::from(&eph).to_bytes();
    let shared = eph
        .diffie_hellman(&x25519_dalek::PublicKey::from(*recipient_x))
        .to_bytes();
    let mut salt = eph_pub.to_vec();
    salt.extend(recipient_x);
    let mut k = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&salt), &shared)
        .expand(b"tomi/v1/hkdf/key-transport", &mut k)
        .expect("hkdf");
    let mut aad = b"tomi/v1/aad/key-transport".to_vec();
    aad.push(0);
    aad.extend(uuid16(space_id));
    aad.extend(key_epoch.to_be_bytes());
    aad.extend(recipient_ed);
    let mut out = eph_pub.to_vec();
    out.extend(nonce);
    out.extend(xc_seal(&k, nonce, &aad, space_key));
    out
}

// ---------------------------------------------------------------------------
// A whole session
// ---------------------------------------------------------------------------

/// Everything that is fixed in a session of the vectors.
#[derive(Clone)]
pub struct Inputs {
    pub relay_authority: String,
    pub channel_id: String,
    pub ranks: [u16; 2],
    pub sid: [u8; 16],
    pub ya: [u8; 32],
    pub yb: [u8; 32],
    pub inviter: Device,
    pub joiner: Device,
    /// Another member of the space, listed with the others.
    pub other: Device,
    pub space_id: String,
    pub key_epoch: u32,
    pub space_key: [u8; 32],
    pub seal_ephemeral: [u8; 32],
    pub seal_nonce: [u8; 24],
    pub now_ms: u64,
    pub inviter_name: String,
    pub inviter_platform: String,
    pub joiner_name: String,
    pub joiner_platform: String,
    pub member_count: Option<u16>,
    pub offer: Option<(u64, u64)>,
    pub joiner_wants_history: bool,
    /// The 16 random bytes behind the transfer id (version and variant bits are set on them).
    pub transfer_random: [u8; 16],
}

/// UUID v4 from 16 random bytes (RFC 9562 §5.4).
pub fn uuid_v4(random: &[u8; 16]) -> [u8; 16] {
    let mut b = *random;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b
}

pub fn uuid_string(b: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*b).hyphenated().to_string()
}

/// The nominal inputs of `pairing.json` (pairing-v1.md §14.1).
pub fn nominal_inputs() -> Inputs {
    Inputs {
        relay_authority: "relay.example.org".into(),
        channel_id: "4821".into(),
        ranks: [465, 1633],
        sid: arr(&seq(0, 16)),
        ya: arr(&unhex(
            "da3d23700a9e5699258aef94dc060dfda5ebb61f02a5ea77fad53f4ff0976d08",
        )),
        yb: arr(&unhex(
            "d2316b454718c35362d83d69df6320f38578ed5984651435e2949762d900b80d",
        )),
        inviter: device_a(),
        joiner: device_b(),
        other: device_c(),
        space_id: "5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10".into(),
        key_epoch: 1,
        space_key: arr(&seq(0, 32)),
        seal_ephemeral: [0x5c; 32],
        seal_nonce: arr(&seq(0xd0, 24)),
        now_ms: 1_793_750_460_000,
        inviter_name: "PC-Bureau".into(),
        inviter_platform: "windows".into(),
        joiner_name: "Pixel".into(),
        joiner_platform: "android".into(),
        member_count: Some(2),
        offer: Some((812, 356_515_840)),
        joiner_wants_history: true,
        transfer_random: arr(&seq(0x40, 16)),
    }
}

/// Every value of a session, computed from the inputs.
pub struct Session {
    pub prs: [u8; 4],
    pub ci: Vec<u8>,
    pub ada: Vec<u8>,
    pub adb: Vec<u8>,
    pub gen_str: Vec<u8>,
    pub gen_hash: [u8; 64],
    pub g: [u8; 32],
    pub ya_pub: [u8; 32],
    pub yb_pub: [u8; 32],
    pub commit: [u8; 32],
    pub k: [u8; 32],
    pub isk: [u8; 64],
    pub keys: Keys,
    pub s8: Vec<u8>,
    pub sas: u32,
    pub inviter_hello: Vec<u8>,
    pub joiner_hello: Vec<u8>,
    pub joiner_confirm: Vec<u8>,
    pub sealed: Vec<u8>,
    pub cert: Vec<u8>,
    pub member_list: Vec<u8>,
    /// `(transfer_id, items, bytes)` when a history is announced.
    pub history: Option<([u8; 16], u64, u64)>,
    pub provision_body: Vec<u8>,
    /// The seven frames, in the order of `FRAME_NAMES`.
    pub frames: Vec<Vec<u8>>,
    /// The seven clear bodies (`commit` and `share` have none: their payload is in clear).
    pub bodies: Vec<Vec<u8>>,
}

/// Computes a whole session from the primitives and the text of the specification.
pub fn session(i: &Inputs) -> Session {
    let prs = prs(i.ranks);
    let ci = ci(&i.relay_authority, &i.channel_id);
    let (ada, adb) = (ada(), adb());
    let gen_str = generator_string(&prs, &ci, &i.sid);
    let gen_hash = sha512(&[&gen_str]);
    let g = generator(&gen_str);
    let ya_pub = share(&i.ya, &g);
    let yb_pub = share(&i.yb, &g);
    let k_a = scalar_mult_vfy(&i.ya, &yb_pub);
    let k_n = scalar_mult_vfy(&i.yb, &ya_pub);
    assert_eq!(k_a, k_n, "both sides agree on K");
    assert_ne!(k_a, [0u8; 32], "K is not the neutral element");
    let isk_value = isk(&i.sid, &k_a, &ya_pub, &ada, &yb_pub, &adb);
    let keys = Keys::from_isk(&isk_value);
    let commit_value = commit(&i.sid, &ya_pub);

    let inviter_hello = inviter_hello(
        &i.inviter.ed_pk,
        &i.inviter.x_pk,
        &i.inviter_name,
        &i.inviter_platform,
        i.member_count,
        i.offer,
    );
    let joiner_hello = joiner_hello(
        &i.joiner.ed_pk,
        &i.joiner.x_pk,
        &i.joiner_name,
        &i.joiner_platform,
    );
    // The user of N answers "yes" only if A offered something.
    let history_asked = i.joiner_wants_history && i.offer.is_some_and(|(items, _)| items > 0);
    let joiner_confirm = joiner_confirm(history_asked);

    let sealed = sealed_space_key(
        &i.space_id,
        i.key_epoch,
        &i.space_key,
        &i.seal_ephemeral,
        &i.seal_nonce,
        &i.joiner.ed_pk,
        &i.joiner.x_pk,
    );
    let cert = device_cert(
        &i.inviter,
        &i.space_id,
        &i.joiner.ed_pk,
        &i.joiner.x_pk,
        i.now_ms,
    );
    let member_list = member_list(
        &i.inviter,
        &i.space_id,
        i.key_epoch,
        i.now_ms,
        &[
            (i.inviter.ed_pk, i.inviter.x_pk),
            (i.other.ed_pk, i.other.x_pk),
            (i.joiner.ed_pk, i.joiner.x_pk),
        ],
    );
    let history = if history_asked {
        i.offer
            .map(|(items, bytes)| (uuid_v4(&i.transfer_random), items, bytes))
    } else {
        None
    };
    let provision_body = provision_body(
        &uuid16(&i.space_id),
        i.key_epoch,
        &sealed,
        &cert,
        &member_list,
        history,
    );

    let mut commit_payload = i.sid.to_vec();
    commit_payload.extend(commit_value);
    let frames = vec![
        frame(0x01, &commit_payload),
        frame(0x02, &yb_pub),
        reveal_frame(&keys, &i.sid, &ya_pub, &inviter_hello),
        sealed_frame(&keys, 0x04, &i.sid, &joiner_hello),
        sealed_frame(&keys, 0x05, &i.sid, &joiner_confirm),
        sealed_frame(&keys, 0x06, &i.sid, &provision_body),
        sealed_frame(&keys, 0x07, &i.sid, &[]),
    ];
    let bodies = vec![
        Vec::new(),
        Vec::new(),
        inviter_hello.clone(),
        joiner_hello.clone(),
        joiner_confirm.clone(),
        provision_body.clone(),
        Vec::new(),
    ];
    Session {
        prs,
        ci,
        ada,
        adb,
        gen_str,
        gen_hash,
        g: g.compress().to_bytes(),
        ya_pub,
        yb_pub,
        commit: commit_value,
        k: k_a,
        isk: isk_value,
        keys,
        s8: kdf(&isk_value, "tomi/v1/hkdf/pair-sas", 8),
        sas: sas(&isk_value),
        inviter_hello,
        joiner_hello,
        joiner_confirm,
        sealed,
        cert,
        member_list,
        history,
        provision_body,
        frames,
        bodies,
    }
}
