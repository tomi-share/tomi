//! Conformance of `tomi-core` to the frozen vectors of `spec/vectors/v1/`.
//!
//! Each vector is checked twice:
//! 1. **independently**, by rebuilding every value from the primitives crates and the
//!    byte layouts of `spec/format-v1.md` (what a Kotlin implementation would do);
//! 2. through the **public API** of `tomi-core` (open, decrypt, trust decisions).
//!
//! CI entry point: `cargo test -p tomi-core --test spec_vectors`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, VerifyingKey};
use hkdf::Hkdf;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tomi_core::{kat, *};

/// Every file of the directory must be listed here (and therefore checked below).
const KNOWN_FILES: &[&str] = &[
    "aad.json",
    "archive.json",
    "attachment-tombstone.json",
    "device.json",
    "failures.json",
    "history-transfer.json",
    "hkdf.json",
    "item-attachment.json",
    "item-text.json",
    "payload-extensions.json",
    "payload-json.json",
    "pin.json",
    "primitives.json",
    "reservations.json",
    "rotation.json",
    "tombstone.json",
];

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/v1")
}

fn load(name: &str) -> Value {
    let raw = fs::read_to_string(dir().join(name)).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("expected string, got {v}"))
}

fn hx(v: &Value) -> Vec<u8> {
    hex::decode(s(v)).unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64()
        .unwrap_or_else(|| panic!("expected integer, got {v}"))
}

// ---------------------------------------------------------------------------
// Independent re-implementation of the spec's building blocks.
// ---------------------------------------------------------------------------

fn xseal(key: &[u8], nonce: &[u8], aad: &[u8], pt: &[u8]) -> Vec<u8> {
    let key: [u8; 32] = key.try_into().unwrap();
    let nonce: [u8; 24] = nonce.try_into().unwrap();
    XChaCha20Poly1305::new(&Key::from(key))
        .encrypt(&XNonce::from(nonce), Payload { msg: pt, aad })
        .unwrap()
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let salt = if salt.is_empty() { None } else { Some(salt) };
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(salt, ikm)
        .expand(info, &mut out)
        .unwrap();
    out
}

fn sha(b: &[u8]) -> Vec<u8> {
    Sha256::digest(b).to_vec()
}

fn labeled(label: &str) -> Vec<u8> {
    let mut v = label.as_bytes().to_vec();
    v.push(0);
    v
}

fn uuid16(s: &str) -> Vec<u8> {
    uuid::Uuid::parse_str(s).unwrap().as_bytes().to_vec()
}

fn be32(v: u64) -> [u8; 4] {
    u32::try_from(v).unwrap().to_be_bytes()
}

fn aad_item(label: &str, format: u64, epoch: u64, item_id: &str) -> Vec<u8> {
    let mut v = labeled(label);
    v.extend(be32(format));
    v.extend(be32(epoch));
    v.extend(uuid16(item_id));
    v
}

fn aad_chunk(format: u64, item_id: &str, index: u64) -> Vec<u8> {
    let mut v = labeled("tomi/v1/aad/chunk");
    v.extend(be32(format));
    v.extend(uuid16(item_id));
    v.extend(be32(index));
    v
}

fn aad_transport(space_id: &str, epoch: u64, recipient_ed: &[u8]) -> Vec<u8> {
    let mut v = labeled("tomi/v1/aad/key-transport");
    v.extend(uuid16(space_id));
    v.extend(be32(epoch));
    v.extend(recipient_ed);
    v
}

fn pad(json: &[u8]) -> Vec<u8> {
    let mut v = json.to_vec();
    v.push(0x80);
    while !v.len().is_multiple_of(256) {
        v.push(0);
    }
    v
}

fn x25519_pub(secret: &[u8]) -> Vec<u8> {
    let s = x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(secret).unwrap());
    x25519_dalek::PublicKey::from(&s).to_bytes().to_vec()
}

fn x25519(secret: &[u8], public: &[u8]) -> Vec<u8> {
    let s = x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(secret).unwrap());
    let p = x25519_dalek::PublicKey::from(<[u8; 32]>::try_from(public).unwrap());
    s.diffie_hellman(&p).to_bytes().to_vec()
}

fn ed_verify(public: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let vk = VerifyingKey::from_bytes(&public.try_into().unwrap()).unwrap();
    let sig = Signature::from_bytes(&sig.try_into().unwrap());
    vk.verify_strict(msg, &sig).is_ok()
}

/// Rebuilds a sealed space key from its parts (spec §7.3).
fn sealed_key(
    space_id: &str,
    epoch: u64,
    space_key: &[u8],
    eph_secret: &[u8],
    nonce: &[u8],
    recipient_ed: &[u8],
    recipient_x: &[u8],
) -> Vec<u8> {
    let eph_pub = x25519_pub(eph_secret);
    let shared = x25519(eph_secret, recipient_x);
    let mut salt = eph_pub.clone();
    salt.extend(recipient_x);
    let k = hkdf(&salt, &shared, b"tomi/v1/hkdf/key-transport", 32);
    let mut out = eph_pub;
    out.extend(nonce);
    out.extend(xseal(
        &k,
        nonce,
        &aad_transport(space_id, epoch, recipient_ed),
        space_key,
    ));
    out
}

// ---------------------------------------------------------------------------
// Devices from device.json
// ---------------------------------------------------------------------------

struct Devices(BTreeMap<String, (Value, DeviceKeys)>);

impl Devices {
    fn load() -> Self {
        let v = load("device.json");
        let mut map = BTreeMap::new();
        for d in v["devices"].as_array().unwrap() {
            let keys = DeviceKeys::from_secrets(&hx(&d["ed25519_seed"]), &hx(&d["x25519_secret"]))
                .unwrap();
            map.insert(s(&d["name"]).to_owned(), (d.clone(), keys));
        }
        Self(map)
    }
    fn keys(&self, name: &str) -> &DeviceKeys {
        &self.0[name].1
    }
    fn public(&self, name: &str) -> DevicePublic {
        self.keys(name).public()
    }
    fn name_of(&self, ed: &[u8]) -> String {
        self.0
            .iter()
            .find(|(_, (_, k))| k.public().ed25519 == ed)
            .map(|(n, _)| n.clone())
            .unwrap()
    }
    /// `{ self, key_epoch, members }` -> trust store of `self` with every member
    /// added as a face-to-face verified peer.
    fn store(&self, space_id: &str, setup: &Value) -> TrustStore {
        let me = s(&setup["self"]);
        let mut st =
            TrustStore::new(space_id, &self.public(me), u(&setup["key_epoch"]) as u32).unwrap();
        for m in setup["members"].as_array().unwrap() {
            st.add_verified_peer(&self.public(s(m))).unwrap();
        }
        st
    }
}

const SPACE_ID: &str = "5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn manifest_is_complete_and_unchanged() {
    let sums = fs::read_to_string(dir().join("SHA256SUMS")).unwrap();
    let mut listed = BTreeSet::new();
    for line in sums.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line.split_once("  ").unwrap();
        let content = fs::read_to_string(dir().join(name)).unwrap();
        // Tolerate a CRLF checkout; the frozen bytes are LF.
        let normalized = content.replace("\r\n", "\n");
        assert_eq!(
            hex::encode(sha(normalized.as_bytes())),
            hash,
            "{name} was modified: published vectors are frozen (create vectors/v2 instead)"
        );
        listed.insert(name.to_owned());
    }
    let on_disk: BTreeSet<String> = fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    assert_eq!(on_disk, listed, "every vector file must be in SHA256SUMS");
    let known: BTreeSet<String> = KNOWN_FILES.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        on_disk, known,
        "every vector file must be checked by this test"
    );
    assert!(dir().join("README.md").exists());
}

#[test]
fn primitives() {
    let v = load("primitives.json");
    for t in v["xchacha20poly1305"].as_array().unwrap() {
        let ct = xseal(
            &hx(&t["key"]),
            &hx(&t["nonce"]),
            &hx(&t["aad"]),
            &hx(&t["plaintext"]),
        );
        assert_eq!(ct, hx(&t["ciphertext_and_tag"]));
        let via_core = kat::xchacha20poly1305_seal(
            &hx(&t["key"]),
            &hx(&t["nonce"]),
            &hx(&t["aad"]),
            &hx(&t["plaintext"]),
        )
        .unwrap();
        assert_eq!(via_core, ct);
        let pt = kat::xchacha20poly1305_open(&hx(&t["key"]), &hx(&t["nonce"]), &hx(&t["aad"]), &ct)
            .unwrap();
        assert_eq!(pt, hx(&t["plaintext"]));
    }
    for t in v["hkdf_sha256"].as_array().unwrap() {
        let len = u(&t["length"]) as usize;
        let okm = hkdf(&hx(&t["salt"]), &hx(&t["ikm"]), &hx(&t["info"]), len);
        assert_eq!(okm, hx(&t["okm"]));
        let via_core =
            kat::hkdf_sha256(&hx(&t["salt"]), &hx(&t["ikm"]), &hx(&t["info"]), len).unwrap();
        assert_eq!(via_core, okm);
    }
    for t in v["x25519"].as_array().unwrap() {
        assert_eq!(x25519_pub(&hx(&t["alice_secret"])), hx(&t["alice_public"]));
        assert_eq!(x25519_pub(&hx(&t["bob_secret"])), hx(&t["bob_public"]));
        assert_eq!(
            x25519(&hx(&t["alice_secret"]), &hx(&t["bob_public"])),
            hx(&t["shared"])
        );
        assert_eq!(
            x25519(&hx(&t["bob_secret"]), &hx(&t["alice_public"])),
            hx(&t["shared"])
        );
        assert_eq!(
            kat::x25519_shared(&hx(&t["alice_secret"]), &hx(&t["bob_public"])).unwrap(),
            hx(&t["shared"])
        );
    }
    for t in v["ed25519"].as_array().unwrap() {
        let sig = kat::ed25519_sign(&hx(&t["seed"]), &hx(&t["message"])).unwrap();
        assert_eq!(sig, hx(&t["signature"]));
        assert!(ed_verify(&hx(&t["public"]), &hx(&t["message"]), &sig));
        kat::ed25519_verify(&hx(&t["public"]), &hx(&t["message"]), &sig).unwrap();
    }
}

#[test]
fn hkdf_derivations() {
    let v = load("hkdf.json");
    for t in v["cek_wrap"].as_array().unwrap() {
        assert_eq!(hx(&t["info"]), s(&t["info_ascii"]).as_bytes());
        assert_eq!(s(&t["info_ascii"]).as_bytes(), labels::HKDF_CEK_WRAP);
        let okm = hkdf(&[], &hx(&t["space_key"]), &hx(&t["info"]), 32);
        assert_eq!(okm, hx(&t["okm"]));
        let sk = SpaceKey::from_bytes(1, &hx(&t["space_key"])).unwrap();
        assert_eq!(kat::cek_wrap_key(&sk).unwrap(), okm);
    }
    for t in v["key_transport"].as_array().unwrap() {
        assert_eq!(
            x25519_pub(&hx(&t["ephemeral_secret"])),
            hx(&t["ephemeral_public"])
        );
        assert_eq!(
            x25519_pub(&hx(&t["recipient_x25519_secret"])),
            hx(&t["recipient_x25519_public"])
        );
        let shared = x25519(
            &hx(&t["ephemeral_secret"]),
            &hx(&t["recipient_x25519_public"]),
        );
        assert_eq!(shared, hx(&t["shared_secret"]));
        let mut salt = hx(&t["ephemeral_public"]);
        salt.extend(hx(&t["recipient_x25519_public"]));
        assert_eq!(salt, hx(&t["salt"]));
        assert_eq!(s(&t["info_ascii"]).as_bytes(), labels::HKDF_KEY_TRANSPORT);
        let okm = hkdf(&salt, &shared, &hx(&t["info"]), 32);
        assert_eq!(okm, hx(&t["okm"]));
        assert_eq!(
            kat::transport_key(
                &shared,
                &hx(&t["ephemeral_public"]),
                &hx(&t["recipient_x25519_public"])
            )
            .unwrap(),
            okm
        );
    }
}

#[test]
fn aad_encoding() {
    let v = load("aad.json");
    for t in v["vectors"].as_array().unwrap() {
        let expected = hx(&t["aad"]);
        let (independent, via_core) = match s(&t["kind"]) {
            "cek_wrap" => (
                aad_item(
                    "tomi/v1/aad/cek-wrap",
                    u(&t["format"]),
                    u(&t["key_epoch"]),
                    s(&t["item_id"]),
                ),
                kat::aad_cek_wrap(
                    u(&t["format"]) as u32,
                    u(&t["key_epoch"]) as u32,
                    s(&t["item_id"]),
                )
                .unwrap(),
            ),
            "payload" => (
                aad_item(
                    "tomi/v1/aad/payload",
                    u(&t["format"]),
                    u(&t["key_epoch"]),
                    s(&t["item_id"]),
                ),
                kat::aad_payload(
                    u(&t["format"]) as u32,
                    u(&t["key_epoch"]) as u32,
                    s(&t["item_id"]),
                )
                .unwrap(),
            ),
            "chunk" => (
                aad_chunk(u(&t["format"]), s(&t["item_id"]), u(&t["attachment_index"])),
                kat::aad_chunk(
                    u(&t["format"]) as u32,
                    s(&t["item_id"]),
                    u(&t["attachment_index"]) as u32,
                )
                .unwrap(),
            ),
            "key_transport" => (
                aad_transport(
                    s(&t["space_id"]),
                    u(&t["key_epoch"]),
                    &hx(&t["recipient_ed25519"]),
                ),
                kat::aad_key_transport(
                    s(&t["space_id"]),
                    u(&t["key_epoch"]) as u32,
                    &hx(&t["recipient_ed25519"]),
                )
                .unwrap(),
            ),
            other => panic!("unknown aad kind {other}"),
        };
        assert_eq!(independent, expected);
        assert_eq!(via_core, expected);
    }
}

/// Checks one envelope vector; returns the opened item.
fn check_envelope_vector(t: &Value) -> OpenedItem {
    let name = s(&t["name"]);
    let format = u(&t["format"]);
    let epoch = u(&t["key_epoch"]);
    let id = s(&t["item_id"]);
    let space_key = hx(&t["space_key"]);
    let cek = hx(&t["cek"]);
    let json = hx(&t["payload_json_hex"]);
    assert_eq!(json, s(&t["payload_json"]).as_bytes(), "{name}");

    // Independent construction.
    let kw = hkdf(&[], &space_key, b"tomi/v1/hkdf/cek-wrap", 32);
    assert_eq!(kw, hx(&t["cek_wrap_key"]), "{name}: K_wrap");
    let aad_w = aad_item("tomi/v1/aad/cek-wrap", format, epoch, id);
    let aad_p = aad_item("tomi/v1/aad/payload", format, epoch, id);
    assert_eq!(aad_w, hx(&t["aad_cek_wrap"]), "{name}");
    assert_eq!(aad_p, hx(&t["aad_payload"]), "{name}");
    let padded = pad(&json);
    assert_eq!(padded, hx(&t["padded_payload"]), "{name}: padding");
    let wrapped = xseal(&kw, &hx(&t["wrap_nonce"]), &aad_w, &cek);
    assert_eq!(wrapped, hx(&t["wrapped_cek"]), "{name}");
    let ct = xseal(&cek, &hx(&t["payload_nonce"]), &aad_p, &padded);
    assert_eq!(ct, hx(&t["payload_ciphertext"]), "{name}");
    let mut env = Vec::new();
    env.extend(be32(format));
    env.extend(be32(epoch));
    env.extend(uuid16(id));
    env.extend(hx(&t["wrap_nonce"]));
    env.extend(&wrapped);
    env.extend(hx(&t["payload_nonce"]));
    env.extend(&ct);
    assert_eq!(env, hx(&t["envelope"]), "{name}: layout");

    // Public API.
    let sk = SpaceKey::from_bytes(epoch as u32, &space_key).unwrap();
    let sealed = kat::seal_envelope(
        &sk,
        id,
        &cek,
        &hx(&t["wrap_nonce"]),
        &hx(&t["payload_nonce"]),
        &json,
    )
    .unwrap();
    assert_eq!(sealed, env, "{name}: writer");
    let header = read_envelope_header(&env).unwrap();
    assert_eq!(header.format as u64, format);
    assert_eq!(header.key_epoch as u64, epoch);
    assert_eq!(header.item_id, id);
    let opened = open_item(&env, id, &sk).unwrap();
    assert_eq!(
        kat::item_to_value(&opened.item),
        t["expected_item"],
        "{name}: item"
    );
    // Randomized writer round-trip on the same item.
    let again = seal_item(&opened.item, &sk).unwrap();
    assert_ne!(again, env);
    assert_eq!(open_item(&again, id, &sk).unwrap().item, opened.item);
    opened
}

#[test]
fn item_text() {
    let v = load("item-text.json");
    let vectors = v["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 3);
    for t in vectors {
        check_envelope_vector(t);
    }
}

#[test]
fn item_attachment() {
    let v = load("item-attachment.json");
    let opened = check_envelope_vector(&v["item"]);
    let item_id = s(&v["item"]["item_id"]);
    let mut multi_chunk = false;
    for a in v["attachments"].as_array().unwrap() {
        let idx = u(&a["attachment_index"]);
        let size = u(&a["size"]) as usize;
        let key = hx(&a["file_key"]);
        let prefix = hx(&a["nonce_prefix"]);
        let data: Vec<u8> = (0..size).map(|j| (j % 251) as u8).collect();
        assert_eq!(sha(&data), hx(&a["plaintext_sha256"]));
        let blob = hx(&a["blob"]);
        assert_eq!(blob.len() as u64, u(&a["blob_len"]));
        assert_eq!(sha(&blob), hx(&a["blob_sha256"]));
        assert_eq!(blob.len() as u64, encrypted_attachment_len(size as u64));
        let aad = aad_chunk(1, item_id, idx);
        assert_eq!(aad, hx(&a["aad_chunk"]));

        // Independent chunking.
        let chunks = a["chunks"].as_array().unwrap();
        let expected_n = if size == 0 { 1 } else { size.div_ceil(65_536) };
        assert_eq!(chunks.len(), expected_n);
        multi_chunk |= chunks.len() > 1;
        let mut rebuilt = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            let last = i == chunks.len() - 1;
            assert_eq!(c["last"].as_bool().unwrap(), last);
            let mut nonce = prefix.clone();
            nonce.extend((i as u32).to_be_bytes());
            nonce.push(u8::from(last));
            assert_eq!(nonce, hx(&c["nonce"]));
            let start = i * 65_536;
            let end = (start + 65_536).min(size);
            assert_eq!(u(&c["plaintext_offset"]) as usize, start);
            assert_eq!(u(&c["plaintext_len"]) as usize, end - start);
            let ct = xseal(&key, &nonce, &aad, &data[start..end]);
            assert_eq!(ct.len() as u64, u(&c["ciphertext_len"]));
            assert_eq!(sha(&ct), hx(&c["ciphertext_sha256"]));
            let off = u(&c["ciphertext_offset"]) as usize;
            assert_eq!(&blob[off..off + ct.len()], &ct[..]);
            rebuilt.extend(ct);
        }
        assert_eq!(rebuilt, blob);

        // Public API: the attachment parameters come from the decrypted payload.
        let att = &opened.item.attachments[idx as usize];
        assert_eq!(att.file_key, key);
        assert_eq!(att.nonce_prefix, prefix);
        assert_eq!(att.size as usize, size);
        assert_eq!(att.blob_id, s(&a["blob_id"]));
        let dec = ChunkDecryptor::new(item_id, idx as u32, att).unwrap();
        let mut out = Vec::new();
        for part in blob.chunks(4_099) {
            out.extend(dec.update(part).unwrap());
        }
        dec.finalize().unwrap();
        assert_eq!(out, data);

        let enc = kat::chunk_encryptor(item_id, idx as u32, &key, &prefix).unwrap();
        let mut produced = Vec::new();
        for part in data.chunks(65_536) {
            produced.extend(enc.update(part).unwrap());
        }
        produced.extend(enc.finalize().unwrap().ciphertext);
        assert_eq!(produced, blob);
    }
    assert!(
        multi_chunk,
        "at least one attachment must span several chunks"
    );
}

#[test]
fn devices_certs_and_key_transport() {
    let v = load("device.json");
    let devs = Devices::load();
    let space = s(&v["space_id"]);
    assert_eq!(space, SPACE_ID);
    for d in v["devices"].as_array().unwrap() {
        let keys = devs.keys(s(&d["name"]));
        let p = keys.public();
        assert_eq!(p.ed25519, hx(&d["ed25519_public"]));
        assert_eq!(p.x25519, hx(&d["x25519_public"]));
        assert_eq!(x25519_pub(&hx(&d["x25519_secret"])), p.x25519);
        let mut pre = labeled("tomi/v1/device-id");
        pre.extend(&p.ed25519);
        assert_eq!(hex::encode(&sha(&pre)[..16]), s(&d["device_id"]));
        assert_eq!(keys.device_id(), s(&d["device_id"]));
        assert_eq!(
            device_id_from_ed25519(&p.ed25519).unwrap(),
            s(&d["device_id"])
        );
    }

    // Device certificate.
    let c = &v["device_cert"];
    let issuer = devs.public(s(&c["issuer"]));
    let subject = devs.public(s(&c["subject"]));
    let mut tbs = labeled("tomi/v1/sig/device-cert");
    tbs.extend(uuid16(space));
    tbs.extend(&subject.ed25519);
    tbs.extend(&subject.x25519);
    tbs.extend(u(&c["issued_at_ms"]).to_be_bytes());
    tbs.extend(&issuer.ed25519);
    assert_eq!(tbs, hx(&c["signed_bytes"]));
    assert!(ed_verify(&issuer.ed25519, &tbs, &hx(&c["signature"])));
    let mut wire = tbs.clone();
    wire.extend(hx(&c["signature"]));
    assert_eq!(wire, hx(&c["wire"]));
    let cert = DeviceCert::from_bytes(&wire).unwrap();
    cert.verify_signature().unwrap();
    assert_eq!(cert.subject, subject);
    let resigned = devs
        .keys(s(&c["issuer"]))
        .sign_device_cert(space, &subject, u(&c["issued_at_ms"]))
        .unwrap();
    assert_eq!(resigned.to_bytes().unwrap(), wire);
    // B knows A face to face, and learns C through A's certificate.
    let mut store_b = devs.store(
        space,
        &serde_json::json!({"self": "B", "key_epoch": 1, "members": ["A"]}),
    );
    store_b.accept_device_cert(&cert).unwrap();
    assert!(store_b.is_member(&subject.ed25519));

    // Member list.
    let m = &v["member_list"];
    let list = MemberList::from_bytes(&hx(&m["wire"])).unwrap();
    let mut tbs = labeled("tomi/v1/sig/member-list");
    tbs.extend(uuid16(space));
    tbs.extend(be32(u(&m["key_epoch"])));
    tbs.extend(u(&m["issued_at_ms"]).to_be_bytes());
    tbs.extend(&devs.public(s(&m["issuer"])).ed25519);
    let order = m["members_order"].as_array().unwrap();
    tbs.extend(be32(order.len() as u64));
    let mut eds = Vec::new();
    for name in order {
        let p = devs.public(s(name));
        eds.push(p.ed25519.clone());
        tbs.extend(&p.ed25519);
        tbs.extend(&p.x25519);
    }
    let mut sorted = eds.clone();
    sorted.sort();
    assert_eq!(eds, sorted, "members sorted by ed25519 key");
    assert_eq!(tbs, hx(&m["signed_bytes"]));
    assert!(ed_verify(
        &devs.public(s(&m["issuer"])).ed25519,
        &tbs,
        &hx(&m["signature"])
    ));
    list.verify_signature().unwrap();
    // New device C bootstraps from A (verified face to face) and A's list.
    let mut store_c = devs.store(
        space,
        &serde_json::json!({"self": "C", "key_epoch": 1, "members": ["A"]}),
    );
    store_c.accept_member_list(&list).unwrap();
    for name in ["A", "B", "C"] {
        assert!(store_c.is_member(&devs.public(name).ed25519));
    }
    assert!(!store_c.is_member(&devs.public("D").ed25519));

    // Sealed space key.
    let t = &v["sealed_space_key"];
    let r = devs.public(s(&t["recipient"]));
    let rebuilt = sealed_key(
        space,
        u(&t["key_epoch"]),
        &hx(&t["space_key"]),
        &hx(&t["ephemeral_secret"]),
        &hx(&t["nonce"]),
        &r.ed25519,
        &r.x25519,
    );
    assert_eq!(rebuilt, hx(&t["sealed"]));
    assert_eq!(
        aad_transport(space, u(&t["key_epoch"]), &r.ed25519),
        hx(&t["aad"])
    );
    let opened = devs
        .keys(s(&t["recipient"]))
        .open_space_key(space, u(&t["key_epoch"]) as u32, &rebuilt)
        .unwrap();
    assert_eq!(opened.to_bytes(), hx(&t["space_key"]));
    assert_eq!(opened.epoch() as u64, u(&t["key_epoch"]));
    // Another device cannot open it.
    assert!(devs.keys("A").open_space_key(space, 1, &rebuilt).is_err());
}

#[test]
fn tombstones() {
    let v = load("tombstone.json");
    let devs = Devices::load();
    let store_a = devs.store(
        SPACE_ID,
        &serde_json::json!({"self": "A", "key_epoch": 1, "members": ["A", "B", "C"]}),
    );
    for t in v["vectors"].as_array().unwrap() {
        let signer = devs.public(s(&t["signer"]));
        let scope = match s(&t["scope"]) {
            "local" => 1u8,
            "everywhere" => 2u8,
            other => panic!("unknown scope {other}"),
        };
        let mut tbs = labeled("tomi/v1/sig/tombstone");
        tbs.extend(uuid16(s(&t["space_id"])));
        tbs.extend(uuid16(s(&t["item_id"])));
        tbs.extend(u(&t["deleted_at_ms"]).to_be_bytes());
        tbs.push(scope);
        tbs.extend(&signer.ed25519);
        assert_eq!(tbs, hx(&t["signed_bytes"]));
        assert!(ed_verify(&signer.ed25519, &tbs, &hx(&t["signature"])));
        let mut wire = tbs.clone();
        wire.extend(hx(&t["signature"]));
        assert_eq!(wire, hx(&t["wire"]));
        assert_eq!(wire.len(), TOMBSTONE_LEN);

        let parsed = Tombstone::from_bytes(&wire).unwrap();
        parsed.verify_signature().unwrap();
        store_a.verify_tombstone(&parsed).unwrap();
        assert_eq!(parsed.item_id, s(&t["item_id"]));
        let expected_scope = if scope == 1 {
            DeletionScope::Local
        } else {
            DeletionScope::Everywhere
        };
        assert_eq!(parsed.scope, expected_scope);
        let resigned = devs
            .keys(s(&t["signer"]))
            .sign_tombstone(
                s(&t["space_id"]),
                s(&t["item_id"]),
                u(&t["deleted_at_ms"]),
                expected_scope,
            )
            .unwrap();
        assert_eq!(resigned.to_bytes().unwrap(), wire);
    }
}

#[test]
fn attachment_tombstones() {
    let v = load("attachment-tombstone.json");
    let devs = Devices::load();
    let store_a = devs.store(
        SPACE_ID,
        &serde_json::json!({"self": "A", "key_epoch": 1, "members": ["A", "B", "C"]}),
    );
    for t in v["vectors"].as_array().unwrap() {
        let signer = devs.public(s(&t["signer"]));
        let slot = u(&t["slot"]);
        let at = u(&t["deleted_at_ms"]);
        assert!(at < (1 << 53));
        // Independent TBS (spec §8.10.1).
        let mut tbs = labeled("tomi/v1/sig/attachment-tombstone");
        tbs.extend(uuid16(s(&t["space_id"])));
        tbs.extend(uuid16(s(&t["item_id"])));
        tbs.extend(uuid16(s(&t["blob_id"])));
        tbs.extend(be32(slot));
        tbs.extend(at.to_be_bytes());
        tbs.extend(&signer.ed25519);
        assert_eq!(tbs, hx(&t["signed_bytes"]));
        assert!(ed_verify(&signer.ed25519, &tbs, &hx(&t["signature"])));
        let mut wire = tbs.clone();
        wire.extend(hx(&t["signature"]));
        assert_eq!(wire, hx(&t["wire"]));
        assert_eq!(wire.len(), ATTACHMENT_TOMBSTONE_LEN);
        assert_eq!(wire.len(), 189);

        let parsed = AttachmentTombstone::from_bytes(&wire).unwrap();
        parsed.verify_signature().unwrap();
        store_a.verify_attachment_tombstone(&parsed).unwrap();
        assert_eq!(parsed.item_id, s(&t["item_id"]));
        assert_eq!(parsed.blob_id, s(&t["blob_id"]));
        assert_eq!(u64::from(parsed.slot), slot);
        let resigned = devs
            .keys(s(&t["signer"]))
            .sign_attachment_tombstone(
                s(&t["space_id"]),
                s(&t["item_id"]),
                s(&t["blob_id"]),
                slot as u32,
                at,
            )
            .unwrap();
        assert_eq!(resigned.to_bytes().unwrap(), wire);
    }
    for t in v["failures"].as_array().unwrap() {
        let name = s(&t["name"]);
        let wire = hx(&t["wire"]);
        let store = history_store(&devs, &t["trust_setup"]);
        let res = match s(&t["kind"]) {
            "attachment_tombstone" => AttachmentTombstone::from_bytes(&wire)
                .and_then(|ts| store.verify_attachment_tombstone(&ts)),
            "tombstone" => Tombstone::from_bytes(&wire).and_then(|ts| store.verify_tombstone(&ts)),
            other => panic!("unknown statement kind {other}"),
        };
        assert_eq!(res.unwrap_err().code(), s(&t["expected_error"]), "{name}");
    }
}

#[test]
fn key_rotation() {
    let v = load("rotation.json");
    let devs = Devices::load();
    let space = s(&v["space_id"]);
    let new_epoch = u(&v["new_epoch"]);
    let new_key = hx(&v["new_space_key"]);
    let issuer = devs.public(s(&v["issuer"]));

    // Independent TBS.
    let mut tbs = labeled("tomi/v1/sig/key-rotation");
    tbs.extend(uuid16(space));
    tbs.extend(be32(new_epoch));
    tbs.extend(u(&v["issued_at_ms"]).to_be_bytes());
    tbs.extend(&issuer.ed25519);
    let mut revoked: Vec<Vec<u8>> = v["revoked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| devs.public(s(n)).ed25519)
        .collect();
    revoked.sort();
    tbs.extend(be32(revoked.len() as u64));
    for k in &revoked {
        tbs.extend(k);
    }
    let recipients = v["recipients"].as_array().unwrap();
    tbs.extend(be32(recipients.len() as u64));
    let mut prev: Option<Vec<u8>> = None;
    let mut randomness = Vec::new();
    for r in recipients {
        let p = devs.public(s(&r["device"]));
        assert_eq!(p.ed25519, hx(&r["ed25519_public"]));
        if let Some(prev) = &prev {
            assert!(prev < &p.ed25519, "recipients sorted by ed25519 key");
        }
        prev = Some(p.ed25519.clone());
        let sealed = sealed_key(
            space,
            new_epoch,
            &new_key,
            &hx(&r["ephemeral_secret"]),
            &hx(&r["nonce"]),
            &p.ed25519,
            &p.x25519,
        );
        assert_eq!(sealed, hx(&r["sealed_key"]));
        assert_eq!(aad_transport(space, new_epoch, &p.ed25519), hx(&r["aad"]));
        assert_eq!(
            x25519(&hx(&r["ephemeral_secret"]), &p.x25519),
            hx(&r["shared_secret"])
        );
        tbs.extend(&p.ed25519);
        tbs.extend(&sealed);
        randomness.push((hx(&r["ephemeral_secret"]), hx(&r["nonce"])));
    }
    assert_eq!(tbs, hx(&v["signed_bytes"]));
    assert!(ed_verify(&issuer.ed25519, &tbs, &hx(&v["signature"])));
    let mut wire = tbs;
    wire.extend(hx(&v["signature"]));
    assert_eq!(wire, hx(&v["wire"]));

    // Writer reproduces the exact rotation.
    let mut setup = v["trust_setup"].clone();
    setup["self"] = v["issuer"].clone();
    let issuer_store = devs.store(space, &setup);
    let bundle = kat::create_rotation(
        &issuer_store,
        devs.keys(s(&v["issuer"])),
        &revoked,
        u(&v["issued_at_ms"]),
        SpaceKey::from_bytes(new_epoch as u32, &new_key).unwrap(),
        &randomness,
    )
    .unwrap();
    assert_eq!(bundle.rotation.to_bytes().unwrap(), wire);

    // Each member applies it.
    let rotation = KeyRotation::from_bytes(&wire).unwrap();
    for (name, expected) in v["expected"].as_object().unwrap() {
        let mut setup = v["trust_setup"].clone();
        setup["self"] = Value::String(name.clone());
        let mut store = devs.store(space, &setup);
        let out = store.apply_rotation(&rotation, devs.keys(name)).unwrap();
        if expected["self_revoked"].as_bool() == Some(true) {
            assert!(out.self_revoked, "{name}");
            assert!(out.new_space_key.is_none());
        } else {
            let k = out.new_space_key.unwrap();
            assert_eq!(k.to_bytes(), hx(&expected["new_space_key"]), "{name}");
            assert_eq!(k.epoch() as u64, new_epoch);
            assert_eq!(store.key_epoch() as u64, new_epoch);
            for r in &revoked {
                assert!(store.is_revoked(r));
                assert!(!store.is_member(r));
                assert_eq!(devs.name_of(r), "C");
            }
        }
    }
}

#[test]
fn payload_json() {
    let v = load("payload-json.json");
    for t in v["vectors"].as_array().unwrap() {
        let name = s(&t["name"]);
        let bytes = hx(&t["json_hex"]);
        if let Some(text) = t.get("json") {
            assert_eq!(s(text).as_bytes(), &bytes[..], "{name}");
        }
        let res = kat::parse_payload(&bytes, s(&t["expected_item_id"]));
        match (t.get("expected_item"), t.get("expected_error")) {
            (Some(item), None) => {
                assert_eq!(&kat::item_to_value(&res.unwrap()), item, "{name}")
            }
            (None, Some(code)) => assert_eq!(res.unwrap_err().code(), s(code), "{name}"),
            _ => panic!("{name}: needs expected_item xor expected_error"),
        }
    }
}

#[test]
fn failures() {
    let v = load("failures.json");
    let devs = Devices::load();

    for t in v["envelope"].as_array().unwrap() {
        let name = s(&t["name"]);
        let key = SpaceKey::from_bytes(u(&t["key_epoch"]) as u32, &hx(&t["space_key"])).unwrap();
        let err = open_item(&hx(&t["envelope"]), s(&t["expected_item_id"]), &key).unwrap_err();
        assert_eq!(err.code(), s(&t["expected_error"]), "{name}");
    }

    let att = load("item-attachment.json");
    let item = &att["item"];
    let opened = open_item(
        &hx(&item["envelope"]),
        s(&item["item_id"]),
        &SpaceKey::from_bytes(u(&item["key_epoch"]) as u32, &hx(&item["space_key"])).unwrap(),
    )
    .unwrap();
    for t in v["stream"].as_array().unwrap() {
        let name = s(&t["name"]);
        let idx = u(&t["attachment_index"]) as usize;
        let mut blob = hx(&att["attachments"][idx]["blob"]);
        for op in t["transform"].as_array().unwrap() {
            match s(&op["op"]) {
                "truncate" => blob.truncate(u(&op["len"]) as usize),
                "append" => blob.extend(hx(&op["data"])),
                "splice" => {
                    blob.truncate(u(&op["offset"]) as usize);
                    blob.extend(hx(&op["data"]));
                }
                other => panic!("unknown op {other}"),
            }
        }
        let dec = ChunkDecryptor::new(
            s(&item["item_id"]),
            u(&t["decrypt_with_index"]) as u32,
            &opened.item.attachments[idx],
        )
        .unwrap();
        let err = dec.update(&blob).and_then(|_| dec.finalize()).unwrap_err();
        assert_eq!(err.code(), s(&t["expected_error"]), "{name}");
    }

    for t in v["statements"].as_array().unwrap() {
        let name = s(&t["name"]);
        let wire = hx(&t["wire"]);
        let mut store = devs.store(SPACE_ID, &t["trust_setup"]);
        let res = match s(&t["kind"]) {
            "tombstone" => Tombstone::from_bytes(&wire).and_then(|ts| store.verify_tombstone(&ts)),
            "device_cert" => {
                DeviceCert::from_bytes(&wire).and_then(|c| store.accept_device_cert(&c))
            }
            "key_rotation" => KeyRotation::from_bytes(&wire).and_then(|r| {
                store
                    .apply_rotation(&r, devs.keys(s(&t["apply_as"])))
                    .map(|_| ())
            }),
            other => panic!("unknown statement kind {other}"),
        };
        let before = devs.store(SPACE_ID, &t["trust_setup"]);
        assert_eq!(res.unwrap_err().code(), s(&t["expected_error"]), "{name}");
        assert_eq!(
            store, before,
            "{name}: a rejected statement must not change the store"
        );
    }
}

// ---------------------------------------------------------------------------
// History transfer (format-v1.md §7.4, §8.7)
// ---------------------------------------------------------------------------

fn history_entry_value(e: &HistoryEntry) -> Value {
    match e {
        HistoryEntry::Device(d) => serde_json::json!({
            "kind": "device", "device_id": d.device_id, "name": d.name,
            "platform": d.platform, "revoked": d.revoked,
        }),
        HistoryEntry::Item(i) => serde_json::json!({
            "kind": "item",
            "item": kat::item_to_value(&i.item),
            "origin_seq": i.origin_seq,
            "received_at_ms": i.received_at_ms,
            "read": i.read,
            "kept": i.kept,
            "files": i.files.iter().map(|f| f.as_ref().map(|f| serde_json::json!({
                "blob_id": f.blob_id,
                "file_key": hex::encode(&f.file_key),
                "nonce_prefix": hex::encode(&f.nonce_prefix),
            }))).collect::<Vec<_>>(),
        }),
    }
}

/// Independent STREAM encryption of `plain` (chunks of 64 KiB, final flag on the last).
fn stream_seal(key: &[u8], prefix: &[u8], aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let n = if plain.is_empty() {
        1
    } else {
        plain.len().div_ceil(65_536)
    };
    let mut out = Vec::new();
    for i in 0..n {
        let last = i == n - 1;
        let mut nonce = prefix.to_vec();
        nonce.extend((i as u32).to_be_bytes());
        nonce.push(u8::from(last));
        let end = ((i + 1) * 65_536).min(plain.len());
        out.extend(xseal(key, &nonce, aad, &plain[i * 65_536..end]));
    }
    out
}

fn history_store(devs: &Devices, setup: &Value) -> TrustStore {
    let me = s(&setup["self"]);
    let mut st = devs.store(SPACE_ID, setup);
    if setup.get("apply_rotation").is_some() {
        let rot = load(s(&setup["apply_rotation"]));
        st.apply_rotation(
            &KeyRotation::from_bytes(&hx(&rot["wire"])).unwrap(),
            devs.keys(me),
        )
        .unwrap();
    }
    st
}

#[test]
fn history_transfer() {
    let v = load("history-transfer.json");
    let devs = Devices::load();
    let space = s(&v["space_id"]);
    let tid = s(&v["transfer_id"]);
    let sender = devs.public(s(&v["sender"]));
    let recipient = devs.public(s(&v["recipient"]));
    let hk = hx(&v["history_key"]);

    // Sealed history key, rebuilt from the primitives.
    let t = &v["sealed_key"];
    let eph = hx(&t["ephemeral_secret"]);
    let eph_pub = x25519_pub(&eph);
    assert_eq!(eph_pub, hx(&t["ephemeral_public"]));
    assert_eq!(recipient.x25519, hx(&t["recipient_x25519_public"]));
    let shared = x25519(&eph, &recipient.x25519);
    assert_eq!(shared, hx(&t["shared_secret"]));
    let mut salt = eph_pub.clone();
    salt.extend(&recipient.x25519);
    assert_eq!(salt, hx(&t["salt"]));
    assert_eq!(
        s(&t["info_ascii"]).as_bytes(),
        labels::HKDF_HISTORY_TRANSPORT
    );
    let tk = hkdf(&salt, &shared, b"tomi/v1/hkdf/history-transport", 32);
    assert_eq!(tk, hx(&t["transport_key"]));
    let mut aad = labeled("tomi/v1/aad/history-transport");
    aad.extend(uuid16(space));
    aad.extend(uuid16(tid));
    aad.extend(&recipient.ed25519);
    assert_eq!(aad, hx(&t["aad"]));
    let mut sealed = eph_pub.clone();
    sealed.extend(hx(&t["nonce"]));
    sealed.extend(xseal(&tk, &hx(&t["nonce"]), &aad, &hk));
    assert_eq!(sealed, hx(&t["sealed"]));
    assert_eq!(sealed.len(), HISTORY_SEALED_KEY_LEN);

    // Manifest TBS.
    let m = &v["manifest"];
    let parts = v["parts"].as_array().unwrap();
    let mut tbs = labeled("tomi/v1/sig/history-transfer");
    tbs.extend(uuid16(space));
    tbs.extend(uuid16(tid));
    tbs.extend(u(&m["as_of_seq"]).to_be_bytes());
    tbs.extend(u(&m["created_at_ms"]).to_be_bytes());
    tbs.extend(&sender.ed25519);
    tbs.extend(&recipient.ed25519);
    tbs.extend(&sealed);
    tbs.extend(be32(parts.len() as u64));
    for p in parts {
        tbs.extend(uuid16(s(&p["blob_id"])));
        tbs.extend(hx(&p["nonce_prefix"]));
    }
    assert_eq!(tbs, hx(&m["signed_bytes"]));
    assert!(ed_verify(&sender.ed25519, &tbs, &hx(&m["signature"])));
    let mut wire = tbs;
    wire.extend(hx(&m["signature"]));
    assert_eq!(wire, hx(&m["wire"]));
    assert_eq!(wire.len() as u64, u(&m["wire_len"]));
    assert_eq!(wire.len(), 313 + 35 * parts.len());
    assert_eq!(wire.len(), history_transfer_len(parts.len()));

    // Index parts.
    let index = s(&v["index"]).as_bytes();
    assert_eq!(sha(index), hx(&v["index_sha256"]));
    let mut blobs = Vec::new();
    let mut multi_chunk = false;
    for (p, part) in parts.iter().enumerate() {
        assert_eq!(u(&part["part_index"]) as usize, p);
        let mut aad = labeled("tomi/v1/aad/history-chunk");
        aad.extend(be32(1));
        aad.extend(uuid16(tid));
        aad.extend(be32(p as u64));
        aad.extend(be32(parts.len() as u64));
        assert_eq!(aad, hx(&part["aad"]));
        let off = u(&part["plaintext_offset"]) as usize;
        let len = u(&part["plaintext_len"]) as usize;
        let blob = stream_seal(
            &hk,
            &hx(&part["nonce_prefix"]),
            &aad,
            &index[off..off + len],
        );
        assert_eq!(blob, hx(&part["blob"]));
        assert_eq!(blob.len() as u64, u(&part["blob_len"]));
        assert_eq!(sha(&blob), hx(&part["blob_sha256"]));
        let chunks = part["chunks"].as_array().unwrap();
        multi_chunk |= chunks.len() > 1;
        for c in chunks {
            let o = u(&c["ciphertext_offset"]) as usize;
            let l = u(&c["ciphertext_len"]) as usize;
            assert_eq!(sha(&blob[o..o + l]), hx(&c["ciphertext_sha256"]));
        }
        blobs.push(blob);
    }
    assert!(multi_chunk, "one part must span several chunks");
    let total: u64 = parts.iter().map(|p| u(&p["plaintext_len"])).sum();
    assert_eq!(total as usize, index.len());

    // Public API, recipient side.
    let mut setup_b = v["trust_setup"].clone();
    setup_b["self"] = v["recipient"].clone();
    let store_b = devs.store(space, &setup_b);
    let manifest = HistoryTransfer::from_bytes(&wire).unwrap();
    assert_eq!(manifest.to_bytes().unwrap(), wire);
    assert_eq!(
        manifest.sender_device_id().unwrap(),
        sender.device_id().unwrap()
    );
    let key = store_b
        .verify_history_transfer(&manifest, devs.keys(s(&v["recipient"])))
        .unwrap();
    assert_eq!(key.to_bytes(), hk);
    let mut reader = HistoryIndexReader::new(&manifest).unwrap();
    let mut entries = Vec::new();
    for (p, blob) in blobs.iter().enumerate() {
        let dec = HistoryPartDecryptor::new(&key, &manifest, p as u32).unwrap();
        for piece in blob.chunks(5_003) {
            entries.extend(reader.feed(&dec.update(piece).unwrap()).unwrap());
        }
        entries.extend(reader.feed(&dec.finalize().unwrap()).unwrap());
    }
    let summary = reader.finish().unwrap();
    let got: Vec<Value> = entries.iter().map(history_entry_value).collect();
    assert_eq!(Value::Array(got), v["expected_entries"]);
    assert_eq!(summary.items, u(&v["expected_summary"]["items"]));
    assert_eq!(
        summary.skipped_unreadable,
        u(&v["expected_summary"]["skipped_unreadable"])
    );
    assert_eq!(
        summary.files_unavailable,
        u(&v["expected_summary"]["files_unavailable"])
    );

    // Writer side reproduces the exact bytes.
    let mut w = HistoryIndexWriter::new(&HistoryHeader::for_transfer(&manifest).unwrap()).unwrap();
    let mut rewritten = w.header_line().unwrap();
    for e in &entries {
        rewritten.extend(match e {
            HistoryEntry::Device(d) => w.device_line(d).unwrap(),
            HistoryEntry::Item(i) => w.item_line(i).unwrap(),
        });
    }
    rewritten.extend(w.end_line(summary.skipped_unreadable).unwrap());
    assert_eq!(rewritten, index);
    let mut setup_a = v["trust_setup"].clone();
    setup_a["self"] = v["sender"].clone();
    let store_a = devs.store(space, &setup_a);
    let history_key = HistoryKey::from_bytes(&hk).unwrap();
    let rebuilt = kat::create_history_transfer(
        &store_a,
        devs.keys(s(&v["sender"])),
        &recipient.ed25519,
        tid,
        u(&m["as_of_seq"]),
        u(&m["created_at_ms"]),
        &history_key,
        &manifest.parts,
        &eph,
        &hx(&t["nonce"]),
    )
    .unwrap();
    assert_eq!(rebuilt.to_bytes().unwrap(), wire);
    for (p, part) in parts.iter().enumerate() {
        let enc = kat::history_part_encryptor(
            &history_key,
            tid,
            p as u32,
            parts.len() as u32,
            &hx(&part["nonce_prefix"]),
        )
        .unwrap();
        let off = u(&part["plaintext_offset"]) as usize;
        let len = u(&part["plaintext_len"]) as usize;
        let mut produced = enc.update(&index[off..off + len]).unwrap();
        produced.extend(enc.finalize().unwrap());
        assert_eq!(produced, blobs[p]);
    }

    // Re-encrypted files: fresh key, same AAD_chunk as the original attachment.
    let att_item = entries
        .iter()
        .find_map(|e| match e {
            HistoryEntry::Item(i) if !i.item.attachments.is_empty() => Some(i),
            _ => None,
        })
        .unwrap();
    for f in v["transferred_files"].as_array().unwrap() {
        let item_id = s(&f["item_id"]);
        let idx = u(&f["attachment_index"]) as usize;
        let size = u(&f["size"]) as usize;
        let data: Vec<u8> = (0..size).map(|j| (j % 251) as u8).collect();
        assert_eq!(sha(&data), hx(&f["plaintext_sha256"]));
        let aad = aad_chunk(1, item_id, idx as u64);
        assert_eq!(aad, hx(&f["aad_chunk"]));
        let blob = stream_seal(&hx(&f["file_key"]), &hx(&f["nonce_prefix"]), &aad, &data);
        assert_eq!(sha(&blob), hx(&f["blob_sha256"]));
        assert_eq!(blob.len() as u64, u(&f["blob_len"]));
        if let Some(b) = f.get("blob") {
            assert_eq!(blob, hx(b));
        }
        let att = att_item.import_attachment(idx).unwrap();
        assert_eq!(att.blob_id, s(&f["blob_id"]));
        assert_ne!(att.file_key, att_item.item.attachments[idx].file_key);
        let dec = ChunkDecryptor::new(item_id, idx as u32, &att).unwrap();
        let out = dec.update(&blob).unwrap();
        dec.finalize().unwrap();
        assert_eq!(out, data);
    }
    assert!(att_item.import_attachment(1).is_none());

    // D1 defaults and masked secret titles.
    for p in v["payloads"].as_array().unwrap() {
        let name = s(&p["name"]);
        let bytes = hx(&p["json_hex"]);
        assert_eq!(s(&p["json"]).as_bytes(), &bytes[..], "{name}");
        let it = kat::parse_payload(&bytes, s(&p["expected_item_id"])).unwrap();
        assert_eq!(kat::item_to_value(&it), p["expected_item"], "{name}");
        assert_eq!(
            it.visible_title(),
            p["expected_visible_title"].as_str(),
            "{name}"
        );
        assert_eq!(
            it.searchable_text().as_deref(),
            p["expected_searchable_text"].as_str(),
            "{name}"
        );
    }

    // Failures.
    let f = &v["failures"];
    for c in f["manifest"].as_array().unwrap() {
        let name = s(&c["name"]);
        let me = s(&c["trust_setup"]["self"]);
        let st = history_store(&devs, &c["trust_setup"]);
        let res = HistoryTransfer::from_bytes(&hx(&c["wire"]))
            .and_then(|t| st.verify_history_transfer(&t, devs.keys(me)));
        assert_eq!(res.unwrap_err().code(), s(&c["expected_error"]), "{name}");
    }
    for c in f["part"].as_array().unwrap() {
        let name = s(&c["name"]);
        let mut blob = blobs[u(&c["blob_of_part"]) as usize].clone();
        for op in c["transform"].as_array().unwrap() {
            assert_eq!(s(&op["op"]), "truncate");
            blob.truncate(u(&op["len"]) as usize);
        }
        let dec =
            HistoryPartDecryptor::new(&key, &manifest, u(&c["decrypt_as_part"]) as u32).unwrap();
        let err = dec.update(&blob).and_then(|_| dec.finalize()).unwrap_err();
        assert_eq!(err.code(), s(&c["expected_error"]), "{name}");
    }
    for c in f["index"].as_array().unwrap() {
        let name = s(&c["name"]);
        let mut r = HistoryIndexReader::new(&manifest).unwrap();
        let text = s(&c["index"]).as_bytes();
        let res = r.feed(text).and_then(|_| r.finish());
        match (c.get("expected_error"), res) {
            (Some(code), Err(e)) => assert_eq!(e.code(), s(code), "{name}"),
            (None, Ok(sum)) => assert_eq!(sum.items, u(&c["expected_summary"]["items"]), "{name}"),
            (_, other) => panic!("{name}: unexpected {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Pin: signed, encrypted, converging item state (format-v1.md §8.8)
// ---------------------------------------------------------------------------

fn pin_state_of(v: &Value) -> PinState {
    PinState {
        pinned: v["pinned"].as_bool().unwrap(),
        at_ms: u(&v["at_ms"]),
    }
}

fn space_key_of(v: &Value, epoch: u64) -> SpaceKey {
    let k = v["item_state_keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| u(&k["key_epoch"]) == epoch)
        .unwrap();
    SpaceKey::from_bytes(epoch as u32, &hx(&k["space_key"])).unwrap()
}

#[test]
fn pin() {
    let v = load("pin.json");
    let devs = Devices::load();
    let space = s(&v["space_id"]);

    // K_state = HKDF(∅, SK_e, "tomi/v1/hkdf/item-state", 32).
    for k in v["item_state_keys"].as_array().unwrap() {
        assert_eq!(hx(&k["info"]), b"tomi/v1/hkdf/item-state");
        let okm = hkdf(&[], &hx(&k["space_key"]), &hx(&k["info"]), 32);
        assert_eq!(okm, hx(&k["okm"]));
        let sk = SpaceKey::from_bytes(u(&k["key_epoch"]) as u32, &hx(&k["space_key"])).unwrap();
        assert_eq!(kat::item_state_key(&sk).unwrap(), okm);
    }

    // Statements: TBS rebuilt by hand, strict Ed25519, then the public API.
    let trust_b = devs.store(
        space,
        &serde_json::json!({"self": "B", "key_epoch": 1, "members": ["A", "B", "C"]}),
    );
    let mut statements = BTreeMap::new();
    for t in v["statements"].as_array().unwrap() {
        let signer = devs.public(s(&t["signer"]));
        let mut tbs = labeled("tomi/v1/sig/pin");
        tbs.extend(uuid16(s(&t["space_id"])));
        tbs.extend(uuid16(s(&t["item_id"])));
        tbs.extend(u(&t["at_ms"]).to_be_bytes());
        tbs.push(u8::from(t["pinned"].as_bool().unwrap()));
        tbs.extend(&signer.ed25519);
        assert_eq!(tbs.len(), 89);
        assert_eq!(tbs, hx(&t["signed_bytes"]));
        assert!(ed_verify(&signer.ed25519, &tbs, &hx(&t["signature"])));
        let mut wire = tbs;
        wire.extend(hx(&t["signature"]));
        assert_eq!(wire, hx(&t["wire"]));
        assert_eq!(wire.len(), PIN_STATEMENT_LEN);
        assert_eq!(u(&t["wire_len"]) as usize, PIN_STATEMENT_LEN);

        let parsed = PinStatement::from_bytes(&wire).unwrap();
        trust_b.verify_pin(&parsed).unwrap();
        assert_eq!(parsed.item_id, s(&t["item_id"]));
        assert_eq!(parsed.state().at_ms, u(&t["at_ms"]));
        let resigned = devs
            .keys(s(&t["signer"]))
            .sign_pin(
                s(&t["space_id"]),
                s(&t["item_id"]),
                u(&t["at_ms"]),
                t["pinned"].as_bool().unwrap(),
            )
            .unwrap();
        assert_eq!(resigned.to_bytes().unwrap(), wire);
        statements.insert(s(&t["name"]).to_owned(), (wire, parsed));
    }

    // Envelopes: pad, AAD and AEAD rebuilt by hand; then open through the API.
    let mut envelopes = BTreeMap::new();
    for e in v["envelopes"].as_array().unwrap() {
        let epoch = u(&e["key_epoch"]);
        let eid = s(&e["event_id"]);
        let (inner, stmt) = &statements[s(&e["statement"])];
        let key = hkdf(&[], &hx(&e["space_key"]), b"tomi/v1/hkdf/item-state", 32);
        assert_eq!(key, hx(&e["item_state_key"]));
        let aad = aad_item("tomi/v1/aad/item-state", u(&e["format"]), epoch, eid);
        assert_eq!(aad.len(), 47);
        assert_eq!(aad, hx(&e["aad"]));
        let padded = pad(inner);
        assert_eq!(padded.len(), 256);
        assert_eq!(padded, hx(&e["padded"]));
        let ct = xseal(&key, &hx(&e["nonce"]), &aad, &padded);
        assert_eq!(ct, hx(&e["ciphertext"]));
        let mut env = be32(u(&e["format"])).to_vec();
        env.extend(be32(epoch));
        env.extend(uuid16(eid));
        env.extend(hx(&e["nonce"]));
        env.extend(&ct);
        assert_eq!(env, hx(&e["envelope"]));
        assert_eq!(env.len(), ITEM_STATE_ENVELOPE_LEN);
        assert_eq!(u(&e["envelope_len"]) as usize, ITEM_STATE_ENVELOPE_LEN);
        // The relay sees neither the item id nor the state byte in clear.
        let iid = uuid16(&stmt.item_id);
        assert!(!env.windows(16).any(|w| w == iid.as_slice()));

        let sk = space_key_of(&v, epoch);
        let header = read_item_state_header(&env).unwrap();
        assert_eq!(header.event_id, eid);
        assert_eq!(u64::from(header.key_epoch), epoch);
        let opened = open_item_state(&env, eid, &sk).unwrap();
        assert_eq!(opened.state, ItemStateKind::Pin(stmt.clone()));
        assert_eq!(
            kat::seal_item_state(inner, eid, &sk, &hx(&e["nonce"])).unwrap(),
            env
        );
        envelopes.insert(s(&e["name"]).to_owned(), (env, eid.to_owned()));
    }

    // Convergence: the same result whatever the order (last-writer-wins on (at_ms, pinned)).
    let item = s(&v["statements"][0]["item_id"]);
    for c in v["convergence"].as_array().unwrap() {
        let mut book = PinBook::new();
        let mut folded = PinState::default();
        for ev in c["events"].as_array().unwrap() {
            book.merge(item, pin_state_of(ev)).unwrap();
            folded = folded.merge(pin_state_of(ev));
        }
        let expected = pin_state_of(&c["expected"]);
        assert_eq!(book.state(item), Some(expected), "{}", s(&c["name"]));
        assert_eq!(folded, expected);
        assert_eq!(book.is_pinned(item), expected.pinned);
    }
    for n in v["next_at"].as_array().unwrap() {
        let cur = (!n["current"].is_null()).then(|| pin_state_of(&n["current"]));
        assert_eq!(
            PinState::next_at_ms(cur, u(&n["now_ms"])).unwrap(),
            u(&n["expected_at_ms"])
        );
    }

    // Epoch order rule, as for items.
    let er = &v["epoch_rule"];
    let k1 = space_key_of(&v, 1);
    let k2 = space_key_of(&v, 2);
    let mut ring = SpaceKeyRing::new(k1);
    let (env2, eid2) = &envelopes[s(&er["key_pending"]["envelope_of"])];
    assert_eq!(
        ring.open_received_state(env2, eid2, u(&er["rotation_seq"]) + 5),
        ReceivedItemState::Unreadable(UnreadableReason::KeyPending)
    );
    ring.add_rotation_key(k2, u(&er["rotation_seq"]), 0)
        .unwrap();
    for c in er["cases"].as_array().unwrap() {
        let (env, eid) = &envelopes[s(&c["envelope_of"])];
        let got = match ring.open_received_state(env, eid, u(&c["seq"])) {
            ReceivedItemState::Opened(_) => "opened",
            ReceivedItemState::Unreadable(r) => r.code(),
        };
        assert_eq!(got, s(&c["expected"]), "{}", s(&c["name"]));
    }

    // History index: additive `pin` field, read with the manifest of history-transfer.json.
    let hv = load("history-transfer.json");
    let manifest = HistoryTransfer::from_bytes(&hx(&hv["manifest"]["wire"])).unwrap();
    let hi = &v["history_index"];
    assert_eq!(s(&hi["manifest_from"]), "history-transfer.json");
    let text = s(&hi["index"]);
    let mut reader = HistoryIndexReader::new(&manifest).unwrap();
    let entries = reader.feed(text.as_bytes()).unwrap();
    reader.finish().unwrap();
    let expected = hi["expected_pins"].as_array().unwrap();
    assert_eq!(entries.len(), expected.len());
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|l: &Value| l["kind"] == "item")
        .collect();
    for ((entry, exp), line) in entries.iter().zip(expected).zip(&lines) {
        let HistoryEntry::Item(h) = entry else {
            panic!("item expected")
        };
        assert_eq!(h.item.id, s(&exp["item_id"]));
        let want = (!exp["pin"].is_null()).then(|| pin_state_of(&exp["pin"]));
        assert_eq!(h.pin, want);
        // Independent reading of the JSON line (`pin: {pinned, at}`).
        match want {
            None => assert!(line.get("pin").is_none()),
            Some(p) => {
                assert_eq!(line["pin"]["pinned"].as_bool(), Some(p.pinned));
                assert_eq!(line["pin"]["at"].as_u64(), Some(p.at_ms));
            }
        }
    }

    // Unknown inner kind: ignored, not an error.
    let uk = &v["unknown_kind"];
    let o = open_item_state(
        &hx(&uk["envelope"]),
        s(&uk["event_id"]),
        &space_key_of(&v, 1),
    )
    .unwrap();
    assert_eq!(o.state, ItemStateKind::Unknown);

    // Failures.
    for c in v["failures"]["envelope"].as_array().unwrap() {
        let sk = space_key_of(&v, u(&c["open_with_key_epoch"]));
        let err =
            open_item_state(&hx(&c["envelope"]), s(&c["announced_event_id"]), &sk).unwrap_err();
        assert_eq!(err.code(), s(&c["expected_error"]), "{}", s(&c["name"]));
    }
    for c in v["failures"]["statement"].as_array().unwrap() {
        let st = history_store(&devs, &c["trust_setup"]);
        let err = PinStatement::from_bytes(&hx(&c["wire"]))
            .and_then(|p| st.verify_pin(&p))
            .unwrap_err();
        assert_eq!(err.code(), s(&c["expected_error"]), "{}", s(&c["name"]));
    }
}

// ---------------------------------------------------------------------------
// Reservations for a future multi-recipient sharing (spec §5.8)
// ---------------------------------------------------------------------------

/// `{ keys: [{epoch, key}], rotations: [{from_epoch, rotation_seq}] }` -> key ring.
fn ring_of(v: &Value) -> SpaceKeyRing {
    let keys = v["keys"].as_array().unwrap();
    let key = |k: &Value| SpaceKey::from_bytes(u(&k["epoch"]) as u32, &hx(&k["key"])).unwrap();
    let mut ring = SpaceKeyRing::new(key(&keys[0]));
    for (k, r) in keys[1..].iter().zip(v["rotations"].as_array().unwrap()) {
        assert_eq!(u(&r["from_epoch"]) + 1, u(&k["epoch"]));
        ring.add_rotation_key(key(k), u(&r["rotation_seq"]), 0)
            .unwrap();
    }
    ring
}

#[test]
fn reservations() {
    let v = load("reservations.json");

    // `audience` is ignored whatever its value: same item as the equivalent JSON.
    let payload = v["payload"].as_array().unwrap();
    assert!(payload.len() >= 5);
    for t in payload {
        let name = s(&t["name"]);
        let bytes = hx(&t["json_hex"]);
        let equivalent = hx(&t["equivalent_json_hex"]);
        assert_eq!(s(&t["json"]).as_bytes(), &bytes[..], "{name}");
        assert_eq!(
            s(&t["equivalent_json"]).as_bytes(),
            &equivalent[..],
            "{name}"
        );
        let root = serde_json::from_slice::<Value>(&bytes).unwrap();
        assert!(root.get("audience").is_some(), "{name}: carries audience");
        assert!(
            serde_json::from_slice::<Value>(&equivalent)
                .unwrap()
                .get("audience")
                .is_none(),
            "{name}: equivalent has no audience"
        );
        let id = s(&t["expected_item_id"]);
        let got = kat::parse_payload(&bytes, id).unwrap();
        assert_eq!(got, kat::parse_payload(&equivalent, id).unwrap(), "{name}");
        assert_eq!(kat::item_to_value(&got), t["expected_item"], "{name}");
    }

    // A full envelope carrying `audience` opens under the space key like any other.
    for t in v["envelope"].as_array().unwrap() {
        let opened = check_envelope_vector(t);
        let equivalent = s(&t["equivalent_payload_json"]).as_bytes();
        assert_eq!(
            opened.item,
            kat::parse_payload(equivalent, s(&t["item_id"])).unwrap()
        );
    }

    // Reserved key ids: never a space key, received -> KeyPending.
    let r = &v["reserved_key_epoch"];
    assert_eq!(u(&r["space_key_epoch_max"]), u64::from(SPACE_KEY_EPOCH_MAX));
    assert_eq!(u(&r["reserved_min"]), 1 << 31);
    assert_eq!(u(&r["reserved_max"]), u64::from(u32::MAX));
    assert_eq!(
        SpaceKey::from_bytes(SPACE_KEY_EPOCH_MAX + 1, &[0u8; 32])
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    assert!(SpaceKey::from_bytes(SPACE_KEY_EPOCH_MAX, &[0u8; 32]).is_ok());

    for t in r["items"].as_array().unwrap() {
        let name = s(&t["name"]);
        let i = &t["inputs"];
        let (epoch, id) = (u(&i["key_epoch"]), s(&i["item_id"]));
        assert!(epoch > u64::from(SPACE_KEY_EPOCH_MAX), "{name}");
        // Independent construction of the bytes (§5 layout, arbitrary key).
        let kw = hkdf(&[], &hx(&i["key"]), b"tomi/v1/hkdf/cek-wrap", 32);
        let aad_w = aad_item("tomi/v1/aad/cek-wrap", u(&i["format"]), epoch, id);
        let aad_p = aad_item("tomi/v1/aad/payload", u(&i["format"]), epoch, id);
        let mut env = Vec::new();
        env.extend(be32(u(&i["format"])));
        env.extend(be32(epoch));
        env.extend(uuid16(id));
        env.extend(hx(&i["wrap_nonce"]));
        env.extend(xseal(&kw, &hx(&i["wrap_nonce"]), &aad_w, &hx(&i["cek"])));
        env.extend(hx(&i["payload_nonce"]));
        env.extend(xseal(
            &hx(&i["cek"]),
            &hx(&i["payload_nonce"]),
            &aad_p,
            &pad(s(&i["payload_json"]).as_bytes()),
        ));
        assert_eq!(env, hx(&t["envelope"]), "{name}: layout");
        // Public API: the header reads, the item is kept as KeyPending.
        assert_eq!(read_envelope_header(&env).unwrap().key_epoch as u64, epoch);
        let ring = ring_of(&t["ring"]);
        match ring.open_received(&env, s(&t["announced_item_id"]), u(&t["item_seq"])) {
            ReceivedItem::Unreadable(reason) => {
                assert_eq!(reason.code(), s(&t["expected_reason"]), "{name}")
            }
            other => panic!("{name}: {other:?}"),
        }
    }

    for t in r["item_states"].as_array().unwrap() {
        let name = s(&t["name"]);
        let i = &t["inputs"];
        let (epoch, eid) = (u(&i["key_epoch"]), s(&i["event_id"]));
        let key = hkdf(&[], &hx(&i["key"]), b"tomi/v1/hkdf/item-state", 32);
        let mut aad = labeled("tomi/v1/aad/item-state");
        aad.extend(be32(u(&i["format"])));
        aad.extend(be32(epoch));
        aad.extend(uuid16(eid));
        let mut env = Vec::new();
        env.extend(be32(u(&i["format"])));
        env.extend(be32(epoch));
        env.extend(uuid16(eid));
        env.extend(hx(&i["nonce"]));
        env.extend(xseal(
            &key,
            &hx(&i["nonce"]),
            &aad,
            &hx(&i["padded_content"]),
        ));
        assert_eq!(env, hx(&t["envelope"]), "{name}: layout");
        assert_eq!(env.len(), ITEM_STATE_ENVELOPE_LEN, "{name}");
        let ring = ring_of(&t["ring"]);
        match ring.open_received_state(&env, s(&t["announced_event_id"]), u(&t["seq"])) {
            ReceivedItemState::Unreadable(reason) => {
                assert_eq!(reason.code(), s(&t["expected_reason"]), "{name}")
            }
            other => panic!("{name}: {other:?}"),
        }
    }

    let devs = Devices::load();
    for t in r["statements"].as_array().unwrap() {
        let name = s(&t["name"]);
        let kind = s(&t["kind"]);
        let a = devs.public(s(&t["signer"]));
        let mut tbs = labeled(match kind {
            "key_rotation" => "tomi/v1/sig/key-rotation",
            "member_list" => "tomi/v1/sig/member-list",
            _ => panic!("{name}: kind"),
        });
        tbs.extend(uuid16(SPACE_ID));
        tbs.extend(be32(u(&t["key_epoch"])));
        tbs.extend(u(&t["issued_at_ms"]).to_be_bytes());
        tbs.extend(&a.ed25519);
        if kind == "key_rotation" {
            tbs.extend(be32(0));
            tbs.extend(be32(0));
        } else {
            tbs.extend(be32(1));
            tbs.extend(&a.ed25519);
            tbs.extend(&a.x25519);
        }
        assert_eq!(tbs, hx(&t["tbs"]), "{name}: tbs");
        let wire = hx(&t["wire"]);
        assert_eq!(&wire[..tbs.len()], &tbs[..], "{name}");
        assert!(
            ed_verify(&a.ed25519, &tbs, &wire[tbs.len()..]),
            "{name}: signature"
        );
        let err = match kind {
            "key_rotation" => KeyRotation::from_bytes(&wire).map(|_| ()),
            _ => MemberList::from_bytes(&wire).map(|_| ()),
        }
        .unwrap_err();
        assert_eq!(err.code(), s(&t["expected_error"]), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Payload extensibility: duplicate keys (§5.4) and unknown fields (§5.4.4)
// ---------------------------------------------------------------------------

/// Manifest only used to open a history index (no cryptography involved).
fn plain_manifest() -> HistoryTransfer {
    HistoryTransfer {
        space_id: "5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10".into(),
        transfer_id: "c0c1c2c3-c4c5-4cc7-88c9-cacbcccdcecf".into(),
        as_of_seq: 9,
        created_at_ms: 0,
        sender_ed25519: vec![1; 32],
        recipient_ed25519: vec![2; 32],
        sealed_key: vec![0; HISTORY_SEALED_KEY_LEN],
        parts: vec![HistoryPart {
            blob_id: "d0d1d2d3-d4d5-4dd7-98d9-dadbdcdddedf".into(),
            nonce_prefix: vec![3; NONCE_PREFIX_LEN],
        }],
        signature: vec![0; 64],
    }
}

#[test]
fn payload_extensions() {
    let v = load("payload-extensions.json");

    // Any repeated key, at any depth, known or not, escapes decoded: malformed_payload.
    let dups = v["duplicate_keys"].as_array().unwrap();
    assert!(dups.len() >= 8);
    for t in dups {
        let name = s(&t["name"]);
        let bytes = hx(&t["json_hex"]);
        assert_eq!(s(&t["json"]).as_bytes(), &bytes[..], "{name}");
        // The JSON text is otherwise valid: only the repeated key makes it fail.
        assert!(serde_json::from_slice::<Value>(&bytes).is_ok(), "{name}");
        let err = kat::parse_payload(&bytes, s(&t["expected_item_id"])).unwrap_err();
        assert_eq!(err.code(), s(&t["expected_error"]), "{name}");
        assert_eq!(err.code(), "malformed_payload", "{name}");
    }

    for t in v["accepted"].as_array().unwrap() {
        let name = s(&t["name"]);
        let bytes = hx(&t["json_hex"]);
        assert_eq!(s(&t["json"]).as_bytes(), &bytes[..], "{name}");
        let item = kat::parse_payload(&bytes, s(&t["expected_item_id"])).unwrap();
        assert_eq!(kat::item_to_value(&item), t["expected_item"], "{name}");
    }

    // Unknown fields survive local re-sealing and history export/import, byte for byte.
    let preserved = v["preserved"].as_array().unwrap();
    assert!(preserved.len() >= 3);
    let local = LocalKey::generate().unwrap();
    let manifest = plain_manifest();
    for t in preserved {
        let name = s(&t["name"]);
        let json = hx(&t["json_hex"]);
        let id = s(&t["expected_item_id"]);
        assert_eq!(s(&t["json"]).as_bytes(), &json[..], "{name}");
        assert!(!json.contains(&b'\n'), "{name}: compact, one line");
        let item = kat::parse_payload(&json, id).unwrap();
        assert_eq!(kat::item_to_value(&item), t["expected_item"], "{name}");
        assert_eq!(item.raw_payload.as_ref().unwrap().as_bytes(), &json[..]);

        // Local record: decrypt it independently, the plaintext is the original payload.
        let record = local.seal_item(&item).unwrap();
        let mut aad = labeled("tomi/v1/aad/local-item");
        aad.push(LOCAL_RECORD_VERSION);
        aad.extend(uuid16(id));
        let plain = kat::xchacha20poly1305_open(
            &local.to_bytes(),
            &record[1..1 + NONCE_LEN],
            &aad,
            &record[1 + NONCE_LEN..],
        )
        .unwrap();
        assert_eq!(plain, json, "{name}: local record");
        let reopened = local.open_item(&record, id).unwrap();
        assert_eq!(
            reopened.raw_payload.unwrap().as_bytes(),
            &json[..],
            "{name}"
        );

        // New envelope under a space key (re-sealing after `stale_key_epoch`).
        let sk = SpaceKey::generate(1).unwrap();
        let opened = open_item(&seal_item(&item, &sk).unwrap(), id, &sk).unwrap();
        assert_eq!(
            opened.item.raw_payload.unwrap().as_bytes(),
            &json[..],
            "{name}"
        );

        // History index: the item line carries the payload verbatim; the reader hands it
        // back unchanged.
        let header = HistoryHeader::for_transfer(&manifest).unwrap();
        let mut w = HistoryIndexWriter::new(&header).unwrap();
        let mut index = w.header_line().unwrap();
        let line = w
            .item_line(&HistoryItem {
                item: item.clone(),
                origin_seq: 3,
                received_at_ms: 4,
                read: false,
                kept: false,
                files: vec![None; item.attachments.len()],
                removed_attachments: vec![],
                pin: None,
                archive: None,
            })
            .unwrap();
        let text = std::str::from_utf8(&line).unwrap();
        let expected_head = format!("\"payload\":{},\"origin_seq\"", s(&t["json"]));
        assert!(
            text.contains(&expected_head),
            "{name}: payload verbatim in the line"
        );
        index.extend(&line);
        index.extend(w.end_line(0).unwrap());
        let mut r = HistoryIndexReader::new(&manifest).unwrap();
        let mut entries = r.feed(&index).unwrap();
        r.finish().unwrap();
        match entries.pop() {
            Some(HistoryEntry::Item(h)) => {
                assert_eq!(h.item, item, "{name}");
                assert_eq!(h.item.raw_payload.unwrap().as_bytes(), &json[..], "{name}");
            }
            other => panic!("{name}: unexpected {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Archive: signed, encrypted, converging item state (format-v1.md §8.9)
// ---------------------------------------------------------------------------

fn archive_state_of(v: &Value) -> ArchiveState {
    ArchiveState {
        archived: v["archived"].as_bool().unwrap(),
        at_ms: u(&v["at_ms"]),
    }
}

#[test]
fn archive() {
    let v = load("archive.json");
    let devs = Devices::load();
    let space = s(&v["space_id"]);

    for k in v["item_state_keys"].as_array().unwrap() {
        let okm = hkdf(&[], &hx(&k["space_key"]), b"tomi/v1/hkdf/item-state", 32);
        assert_eq!(okm, hx(&k["okm"]));
    }

    // Statements: TBS rebuilt by hand under the archive label (93 bytes), then the API.
    let trust_b = devs.store(
        space,
        &serde_json::json!({"self": "B", "key_epoch": 1, "members": ["A", "B", "C"]}),
    );
    let mut statements = BTreeMap::new();
    for t in v["statements"].as_array().unwrap() {
        let signer = devs.public(s(&t["signer"]));
        let mut tbs = labeled("tomi/v1/sig/archive");
        tbs.extend(uuid16(s(&t["space_id"])));
        tbs.extend(uuid16(s(&t["item_id"])));
        tbs.extend(u(&t["at_ms"]).to_be_bytes());
        tbs.push(u8::from(t["archived"].as_bool().unwrap()));
        tbs.extend(&signer.ed25519);
        assert_eq!(tbs.len(), 93);
        assert_eq!(tbs, hx(&t["signed_bytes"]));
        assert!(ed_verify(&signer.ed25519, &tbs, &hx(&t["signature"])));
        let mut wire = tbs;
        wire.extend(hx(&t["signature"]));
        assert_eq!(wire, hx(&t["wire"]));
        assert_eq!(wire.len(), ARCHIVE_STATEMENT_LEN);
        assert_eq!(u(&t["wire_len"]) as usize, ARCHIVE_STATEMENT_LEN);

        let parsed = ArchiveStatement::from_bytes(&wire).unwrap();
        trust_b.verify_archive(&parsed).unwrap();
        assert_eq!(parsed.state().at_ms, u(&t["at_ms"]));
        let resigned = devs
            .keys(s(&t["signer"]))
            .sign_archive(
                s(&t["space_id"]),
                s(&t["item_id"]),
                u(&t["at_ms"]),
                t["archived"].as_bool().unwrap(),
            )
            .unwrap();
        assert_eq!(resigned.to_bytes().unwrap(), wire);
        statements.insert(s(&t["name"]).to_owned(), (wire, parsed));
    }

    // Envelopes: same 320-byte shape as a pin; the relay cannot tell the kinds apart.
    let mut envelopes = BTreeMap::new();
    for e in v["envelopes"].as_array().unwrap() {
        let epoch = u(&e["key_epoch"]);
        let eid = s(&e["event_id"]);
        let (inner, stmt) = &statements[s(&e["statement"])];
        let key = hkdf(&[], &hx(&e["space_key"]), b"tomi/v1/hkdf/item-state", 32);
        let aad = aad_item("tomi/v1/aad/item-state", u(&e["format"]), epoch, eid);
        assert_eq!(aad, hx(&e["aad"]));
        let padded = pad(inner);
        assert_eq!(padded.len(), 256);
        assert_eq!(padded, hx(&e["padded"]));
        let ct = xseal(&key, &hx(&e["nonce"]), &aad, &padded);
        assert_eq!(ct, hx(&e["ciphertext"]));
        let mut env = be32(u(&e["format"])).to_vec();
        env.extend(be32(epoch));
        env.extend(uuid16(eid));
        env.extend(hx(&e["nonce"]));
        env.extend(&ct);
        assert_eq!(env, hx(&e["envelope"]));
        assert_eq!(env.len(), ITEM_STATE_ENVELOPE_LEN);
        let iid = uuid16(&stmt.item_id);
        assert!(!env.windows(16).any(|w| w == iid.as_slice()));
        let sk = SpaceKey::from_bytes(epoch as u32, &hx(&e["space_key"])).unwrap();
        let opened = open_item_state(&env, eid, &sk).unwrap();
        assert_eq!(opened.state, ItemStateKind::Archive(stmt.clone()));
        envelopes.insert(s(&e["name"]).to_owned(), (env, eid.to_owned()));
    }

    // Convergence.
    let item = s(&v["statements"][0]["item_id"]);
    for c in v["convergence"].as_array().unwrap() {
        let mut book = ArchiveBook::new();
        let mut folded = ArchiveState::default();
        for ev in c["events"].as_array().unwrap() {
            book.merge(item, archive_state_of(ev)).unwrap();
            folded = folded.merge(archive_state_of(ev));
        }
        let expected = archive_state_of(&c["expected"]);
        assert_eq!(book.state(item), Some(expected), "{}", s(&c["name"]));
        assert_eq!(folded, expected);
    }
    for n in v["next_at"].as_array().unwrap() {
        let cur = (!n["current"].is_null()).then(|| archive_state_of(&n["current"]));
        assert_eq!(
            ArchiveState::next_at_ms(cur, u(&n["now_ms"])).unwrap(),
            u(&n["expected_at_ms"])
        );
    }

    // Epoch order rule.
    let er = &v["epoch_rule"];
    let key_of = |epoch: u64| {
        let k = v["item_state_keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| u(&k["key_epoch"]) == epoch)
            .unwrap();
        SpaceKey::from_bytes(epoch as u32, &hx(&k["space_key"])).unwrap()
    };
    let mut ring = SpaceKeyRing::new(key_of(1));
    let (env2, eid2) = &envelopes[s(&er["key_pending"]["envelope_of"])];
    assert_eq!(
        ring.open_received_state(env2, eid2, u(&er["rotation_seq"]) + 5),
        ReceivedItemState::Unreadable(UnreadableReason::KeyPending)
    );
    ring.add_rotation_key(key_of(2), u(&er["rotation_seq"]), 0)
        .unwrap();
    for c in er["cases"].as_array().unwrap() {
        let (env, eid) = &envelopes[s(&c["envelope_of"])];
        let got = match ring.open_received_state(env, eid, u(&c["seq"])) {
            ReceivedItemState::Opened(_) => "opened",
            ReceivedItemState::Unreadable(r) => r.code(),
        };
        assert_eq!(got, s(&c["expected"]), "{}", s(&c["name"]));
    }

    // History index: additive `archive` field.
    let hv = load("history-transfer.json");
    let manifest = HistoryTransfer::from_bytes(&hx(&hv["manifest"]["wire"])).unwrap();
    let hi = &v["history_index"];
    let text = s(&hi["index"]);
    let mut reader = HistoryIndexReader::new(&manifest).unwrap();
    let entries = reader.feed(text.as_bytes()).unwrap();
    reader.finish().unwrap();
    let expected = hi["expected_states"].as_array().unwrap();
    assert_eq!(entries.len(), expected.len());
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|l: &Value| l["kind"] == "item")
        .collect();
    for ((entry, exp), line) in entries.iter().zip(expected).zip(&lines) {
        let HistoryEntry::Item(h) = entry else {
            panic!("item expected")
        };
        assert_eq!(h.item.id, s(&exp["item_id"]));
        let want = (!exp["archive"].is_null()).then(|| archive_state_of(&exp["archive"]));
        assert_eq!(h.archive, want);
        match want {
            None => assert!(line.get("archive").is_none()),
            Some(a) => {
                assert_eq!(line["archive"]["archived"].as_bool(), Some(a.archived));
                assert_eq!(line["archive"]["at"].as_u64(), Some(a.at_ms));
            }
        }
        let want_pin = (!exp["pin"].is_null()).then(|| pin_state_of(&exp["pin"]));
        assert_eq!(h.pin, want_pin);
    }

    // Kinds: the inner label decides.
    let k = &v["kinds"];
    let sk1 = key_of(1);
    let o = open_item_state(
        &hx(&k["pin_in_state_envelope"]),
        s(&k["pin_event_id"]),
        &sk1,
    )
    .unwrap();
    assert!(matches!(o.state, ItemStateKind::Pin(_)));
    let uk = &k["unknown_kind"];
    let o = open_item_state(&hx(&uk["envelope"]), s(&uk["event_id"]), &sk1).unwrap();
    assert_eq!(o.state, ItemStateKind::Unknown);

    // Failures.
    for c in v["failures"]["envelope"].as_array().unwrap() {
        let err =
            open_item_state(&hx(&c["envelope"]), s(&c["announced_event_id"]), &sk1).unwrap_err();
        assert_eq!(err.code(), s(&c["expected_error"]), "{}", s(&c["name"]));
    }
    for c in v["failures"]["statement"].as_array().unwrap() {
        let st = history_store(&devs, &c["trust_setup"]);
        let err = ArchiveStatement::from_bytes(&hx(&c["wire"]))
            .and_then(|p| st.verify_archive(&p))
            .unwrap_err();
        assert_eq!(err.code(), s(&c["expected_error"]), "{}", s(&c["name"]));
    }
}
