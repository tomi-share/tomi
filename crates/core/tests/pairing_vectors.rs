//! Conformance of `tomi-core` to the frozen vectors of `spec/vectors/pairing-v1/`
//! (`spec/pairing-v1.md` §14.1).
//!
//! Each vector is checked twice:
//! 1. **independently**, by rebuilding every value from the primitives crates and the text of the
//!    specifications (`tests/common/pairing_reference.rs`, no code of `tomi_core::pairing`);
//! 2. through the **pairing machines** of `tomi-core` (public API and `pairing::kat` only),
//!    which must produce the same bytes and refuse what the vectors refuse.
//!
//! CI entry point: `cargo test -p tomi-core --test pairing_vectors`.

#[path = "common/pairing_driver.rs"]
mod driver;
#[path = "common/pairing_reference.rs"]
mod reference;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use curve25519_dalek::ristretto::CompressedRistretto;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tomi_core::pairing::{self, kat, PairingCode};
use tomi_core::{DeviceCert, MemberList, TrustStore};

use driver::{run_case, run_mitm, run_nominal, Attacker, Fixture, Frames};
use reference as r;
use reference::h;

/// Every file of the directory must be listed here (and therefore checked below).
const KNOWN_FILES: &[&str] = &["cpace-draft21.json", "pairing.json", "wordlist.json"];

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/pairing-v1")
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

fn a32(v: &Value) -> [u8; 32] {
    r::arr(&hx(v))
}

fn u(v: &Value) -> u64 {
    v.as_u64()
        .unwrap_or_else(|| panic!("expected integer, got {v}"))
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

// ---------------------------------------------------------------------------
// The manifest
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
            sha(normalized.as_bytes()),
            hash,
            "{name} was modified: published vectors are frozen (create vectors/pairing-v2 instead)"
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

// ---------------------------------------------------------------------------
// cpace-draft21.json (P1, P2)
// ---------------------------------------------------------------------------

#[test]
fn cpace_vectors_of_the_draft_are_found_byte_for_byte() {
    let v = load("cpace-draft21.json");
    let gen = &v["calculate_generator"];
    let prs = hx(&gen["PRS"]);
    assert_eq!(prs, b"Password");
    let (ci, sid) = (hx(&gen["CI"]), hx(&gen["sid"]));
    let (ya, yb) = (a32(&v["message_a"]["ya"]), a32(&v["message_b"]["yb"]));
    let (ada, adb) = (hx(&v["message_a"]["ADa"]), hx(&v["message_b"]["ADb"]));
    assert_eq!((ada.as_slice(), adb.as_slice()), (&b"ADa"[..], &b"ADb"[..]));

    // Values quoted by the specification itself (§14.1).
    assert!(s(&gen["g"]).starts_with("222b6b19") && s(&gen["g"]).ends_with("7e56"));
    assert!(s(&v["shared"]["K_initiator"]).starts_with("80b69a8a"));
    assert!(s(&v["shared"]["K_initiator"]).ends_with("b40f"));
    assert!(s(&v["isk_initiator_responder"]["ISK"]).starts_with("b69effbf"));
    assert!(s(&v["isk_initiator_responder"]["ISK"]).ends_with("0447"));
    assert!(s(&v["message_a"]["Ya"]).len() == 64 && s(&gen["sid"]).starts_with("7e4b4791"));
    assert!(s(&gen["sid"]).ends_with("2c57"));
    assert_eq!(
        s(&gen["CI"]),
        "0b415f696e69746961746f720b425f726573706f6e646572"
    );

    // 1. From the primitives.
    assert_eq!(u(&gen["zpad_len"]), 100);
    let gen_str = r::generator_string(&prs, &ci, &sid);
    assert_eq!(gen_str.len(), 170);
    assert_eq!(gen_str, hx(&gen["generator_string"]));
    assert_eq!(r::sha512(&[&gen_str]).to_vec(), hx(&gen["hash"]));
    let g = r::generator(&gen_str);
    assert_eq!(g.compress().to_bytes().to_vec(), hx(&gen["g"]));
    let (ya_pub, yb_pub) = (r::share(&ya, &g), r::share(&yb, &g));
    assert_eq!(ya_pub.to_vec(), hx(&v["message_a"]["Ya"]));
    assert_eq!(yb_pub.to_vec(), hx(&v["message_b"]["Yb"]));
    let k_a = r::scalar_mult_vfy(&ya, &yb_pub);
    let k_b = r::scalar_mult_vfy(&yb, &ya_pub);
    assert_eq!(k_a.to_vec(), hx(&v["shared"]["K_initiator"]));
    assert_eq!(k_b.to_vec(), hx(&v["shared"]["K_responder"]));
    assert_eq!(k_a, k_b);
    let isk = &v["isk_initiator_responder"];
    let mut transcript = r::lv_cat(&[&ya_pub, &ada]);
    transcript.extend(r::lv_cat(&[&yb_pub, &adb]));
    assert_eq!(transcript.len(), 74);
    assert_eq!(transcript, hx(&isk["transcript_ir"]));
    let mut input = r::lv_cat(&[r::DSI_ISK, &sid, &k_a]);
    input.extend(&transcript);
    assert_eq!(input.len(), 146);
    assert_eq!(input, hx(&isk["lv_cat_dsi_sid_k_then_transcript"]));
    assert_eq!(
        r::isk(&sid, &k_a, &ya_pub, &ada, &yb_pub, &adb).to_vec(),
        hx(&isk["ISK"])
    );
    assert_eq!(s(&v["suite"]["DSI"]), h(b"CPaceRistretto255"));
    assert_eq!(s(&v["suite"]["DSI_ISK"]), h(b"CPaceRistretto255_ISK"));

    // 2. Through the pairing module.
    let t = kat::cpace_trace(&prs, &ci, &sid, &ada, &adb, &ya, &yb).unwrap();
    assert_eq!(t.generator_string, hx(&gen["generator_string"]));
    assert_eq!(t.generator_hash, hx(&gen["hash"]));
    assert_eq!(t.generator.to_vec(), hx(&gen["g"]));
    assert_eq!(t.ya_public.to_vec(), hx(&v["message_a"]["Ya"]));
    assert_eq!(t.yb_public.to_vec(), hx(&v["message_b"]["Yb"]));
    assert_eq!(t.k_initiator.to_vec(), hx(&v["shared"]["K_initiator"]));
    assert_eq!(t.k_responder.to_vec(), hx(&v["shared"]["K_responder"]));
    assert_eq!(t.isk_initiator, hx(&isk["ISK"]));
    assert_eq!(t.isk_responder, hx(&isk["ISK"]));
}

#[test]
fn cpace_invalid_inputs_give_the_neutral_element_and_abort() {
    let v = load("cpace-draft21.json");
    let sv = &v["scalar_mult_vfy"];
    let (sc, x) = (a32(&sv["valid"]["s"]), a32(&sv["valid"]["X"]));
    let want = hx(&sv["valid"]["result"]);
    assert_eq!(r::scalar_mult_vfy(&sc, &x).to_vec(), want);
    assert_eq!(kat::scalar_mult_vfy(&sc, &x).to_vec(), want);
    let invalid = sv["invalid"].as_array().unwrap();
    assert_eq!(invalid.len(), 2);
    for case in invalid {
        let (sc, y) = (a32(&case["s"]), a32(&case["Y"]));
        let identity = [0u8; 32];
        assert_eq!(hx(&case["result"]), identity);
        assert_eq!(s(&case["protocol"]), "abort");
        assert_eq!(r::scalar_mult_vfy(&sc, &y), identity);
        assert_eq!(kat::scalar_mult_vfy(&sc, &y), identity);
    }
    // Y_i1 really is not a ristretto255 encoding, and the second one is the neutral element.
    assert!(CompressedRistretto(a32(&invalid[0]["Y"]))
        .decompress()
        .is_none());
    assert_eq!(a32(&invalid[1]["Y"]), [0u8; 32]);
}

#[test]
fn prepend_len_and_lv_cat_vectors() {
    let v = load("cpace-draft21.json");
    for case in v["prepend_len"].as_array().unwrap() {
        let data = hx(&case["data"]);
        assert_eq!(r::prepend_len(&data), hx(&case["prepend_len"]));
        assert_eq!(kat::prepend_len(&data), hx(&case["prepend_len"]));
    }
    // A.1.2: bytes(range(127)) and bytes(range(128)) are in the file.
    let lens: Vec<usize> = v["prepend_len"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| hx(&c["data"]).len())
        .collect();
    assert_eq!(lens, [0, 4, 127, 128]);
    let lv = &v["lv_cat"][0];
    let args: Vec<Vec<u8>> = lv["args"].as_array().unwrap().iter().map(hx).collect();
    let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
    assert_eq!(r::lv_cat(&refs), hx(&lv["result"]));
    assert_eq!(kat::lv_cat(&refs), hx(&lv["result"]));
    assert_eq!(s(&lv["result"]), "043132333401350003363738");
}

// ---------------------------------------------------------------------------
// wordlist.json (P3, P5, P6)
// ---------------------------------------------------------------------------

#[test]
fn word_list_digests_and_ranks() {
    let v = load("wordlist.json");
    // The list embedded in the crate, through the public API.
    let words: Vec<&str> = (0..2048u16).map(|i| pairing::word(i).unwrap()).collect();
    assert!(pairing::word(2048).is_none());
    let mut text = words.join("\n");
    text.push('\n');
    let folded = &v["folded"];
    assert_eq!(text.len() as u64, u(&folded["bytes"]));
    assert_eq!(text.len(), 16_001);
    assert_eq!(sha(text.as_bytes()), s(&folded["sha256"]));
    assert_eq!(
        s(&folded["sha256"]),
        "cf8db448ad5ca0b68c3c8f88b51458f20362a5939fef236fac64a303f1485e87"
    );
    assert_eq!(
        s(&v["source"]["file_sha256"]),
        "ebc3959ab7801a1df6bac4fa7d970652f1df76b683cd2f4003c941c63d517e59"
    );
    assert_eq!(u(&v["source"]["file_bytes"]), 16_777);
    // The file shipped in the source tree has the same bytes (it is what `include_str!` embeds).
    let file =
        fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pairing/wordlist_fr.txt"))
            .unwrap();
    assert_eq!(file, text.as_bytes());
    // 2048 distinct words of 5 to 8 letters a-z, distinct on their 4 first letters.
    let distinct: BTreeSet<&str> = words.iter().copied().collect();
    let prefixes: BTreeSet<&str> = words.iter().map(|w| &w[..4]).collect();
    assert_eq!((distinct.len(), prefixes.len()), (2048, 2048));
    assert!(words
        .iter()
        .all(|w| (5..=8).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_lowercase())));
    for e in v["ranks"].as_array().unwrap() {
        let (rank, word) = (u(&e["rank"]) as u16, s(&e["word"]));
        assert_eq!(pairing::word(rank), Some(word));
        assert_eq!(pairing::rank_of(word), Some(rank));
        assert_eq!(words[rank as usize], word);
        assert_eq!(
            hx(&e["original_nfc_utf8"]),
            s(&e["original_nfc"]).as_bytes()
        );
    }
    let ranks: Vec<(u64, &str)> = v["ranks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (u(&e["rank"]), s(&e["word"])))
        .collect();
    for want in [
        (0, "abaisser"),
        (465, "crayon"),
        (1633, "renard"),
        (2047, "zoologie"),
    ] {
        assert!(ranks.contains(&want), "{want:?}");
    }
    // PRS = u16be(i1) || u16be(i2).
    let prs = &v["prs"][0];
    assert_eq!(
        r::prs([u(&prs["ranks"][0]) as u16, u(&prs["ranks"][1]) as u16]).to_vec(),
        hx(&prs["prs"])
    );
    assert_eq!(s(&prs["prs"]), "01d10661");
}

#[test]
fn reading_a_typed_code() {
    let v = load("wordlist.json");
    let cases = v["reading"].as_array().unwrap();
    assert!(cases.len() >= 25);
    let (mut accepted, mut refused) = (0, 0);
    for c in cases {
        // The input is the UTF-8 bytes of the file: the normalisation form is part of the vector.
        let input = String::from_utf8(hx(&c["input_utf8"])).unwrap();
        assert_eq!(input, s(&c["input"]));
        let result = PairingCode::parse(&input);
        if c["accepted"].as_bool().unwrap() {
            accepted += 1;
            let code = result.unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(code.channel_id(), s(&c["channel_id"]), "{input:?}");
            let want = [u(&c["ranks"][0]) as u16, u(&c["ranks"][1]) as u16];
            assert_eq!(code.ranks(), want, "{input:?}");
            // Reading and showing are consistent.
            assert_eq!(PairingCode::parse(&code.to_code_string()).unwrap(), code);
        } else {
            refused += 1;
            assert!(
                result.is_err(),
                "{input:?} must be refused: {}",
                s(&c["why"])
            );
        }
    }
    assert!(accepted >= 12 && refused >= 12);
}

#[test]
fn reading_a_link_and_showing_a_code() {
    let v = load("wordlist.json");
    let authority = s(&v["links"]["relay_authority"]);
    assert_eq!(authority, "relay.example.org");
    for c in v["links"]["cases"].as_array().unwrap() {
        let result = PairingCode::from_link(s(&c["input"]), authority);
        if c["accepted"].as_bool().unwrap() {
            let code = result.unwrap();
            assert_eq!(code.channel_id(), s(&c["channel_id"]));
            assert_eq!(
                code.ranks(),
                [u(&c["ranks"][0]) as u16, u(&c["ranks"][1]) as u16]
            );
        } else {
            assert!(result.is_err(), "{} must be refused", s(&c["input"]));
        }
    }
    let shown = &v["shown"];
    let code = PairingCode::parse(s(&shown["code"])).unwrap();
    assert_eq!(code.to_code_string(), s(&shown["code"]));
    assert_eq!(code.link(authority), s(&shown["link"]));
    assert_eq!(code.deep_link(), s(&shown["deep_link"]));
}

// ---------------------------------------------------------------------------
// pairing.json
// ---------------------------------------------------------------------------

/// The inputs recorded in the file, rebuilt as the reference implementation wants them.
fn inputs_of(v: &Value) -> r::Inputs {
    let i = &v["inputs"];
    let device = |d: &Value, name: &'static str, ed: u8, x: u8| {
        let dev = r::Device::new(name, ed, x);
        assert_eq!(h(&dev.ed_pk), s(&d["ed25519_public"]), "{name}");
        assert_eq!(h(&dev.x_pk), s(&d["x25519_public"]), "{name}");
        assert_eq!(dev.device_id(), s(&d["device_id"]), "{name}");
        dev
    };
    let offer = i["history_offer"]
        .as_object()
        .map(|o| (u(&o["items"]), u(&o["bytes"])));
    r::Inputs {
        relay_authority: s(&i["relay_authority"]).into(),
        channel_id: s(&i["channel_id"]).into(),
        ranks: [u(&i["ranks"][0]) as u16, u(&i["ranks"][1]) as u16],
        sid: a32_16(&i["sid"]),
        ya: a32(&i["ya"]),
        yb: a32(&i["yb"]),
        inviter: device(&i["inviter"], "A", 0xa0, 0x10),
        joiner: device(&i["joiner"], "B", 0xc0, 0x30),
        other: device(&i["other_member"], "C", 0xe0, 0x50),
        space_id: s(&i["space_id"]).into(),
        key_epoch: u(&i["key_epoch"]) as u32,
        space_key: a32(&i["space_key"]),
        seal_ephemeral: a32(&i["seal_ephemeral_secret"]),
        seal_nonce: r::arr(&hx(&i["seal_nonce"])),
        now_ms: u(&i["now_ms"]),
        inviter_name: s(&i["inviter_name"]).into(),
        inviter_platform: s(&i["inviter_platform"]).into(),
        joiner_name: s(&i["joiner_name"]).into(),
        joiner_platform: s(&i["joiner_platform"]).into(),
        member_count: i["member_count"].as_u64().map(|c| c as u16),
        offer,
        joiner_wants_history: i["joiner_wants_history"].as_bool().unwrap(),
        transfer_random: r::arr(&hx(&i["transfer_id_random"])),
    }
}

fn a32_16(v: &Value) -> [u8; 16] {
    r::arr(&hx(v))
}

fn frames_of(v: &Value) -> Vec<Vec<u8>> {
    v["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| hx(&f["frame"]))
        .collect()
}

#[test]
fn the_inputs_are_those_of_the_specification() {
    let v = load("pairing.json");
    let i = &v["inputs"];
    // §14.1
    assert_eq!(s(&i["relay_authority"]), "relay.example.org");
    assert_eq!(s(&i["channel_id"]), "4821");
    assert_eq!(i["words"], serde_json::json!(["crayon", "renard"]));
    assert_eq!(i["ranks"], serde_json::json!([465, 1633]));
    assert_eq!(s(&i["sid"]), "000102030405060708090a0b0c0d0e0f");
    assert_eq!(
        s(&i["ya"]),
        "da3d23700a9e5699258aef94dc060dfda5ebb61f02a5ea77fad53f4ff0976d08"
    );
    assert_eq!(
        s(&i["yb"]),
        "d2316b454718c35362d83d69df6320f38578ed5984651435e2949762d900b80d"
    );
    assert_eq!(s(&i["space_id"]), "5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10");
    assert_eq!(u(&i["key_epoch"]), 1);
    assert_eq!(s(&i["inviter_name"]), "PC-Bureau");
    assert_eq!(s(&i["joiner_name"]), "Pixel");
    // The devices are A, B (and C, already a member) of the frozen v1 vectors.
    let devices: Value = serde_json::from_str(
        &fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/vectors/v1/device.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for (key, name) in [("inviter", "A"), ("joiner", "B"), ("other_member", "C")] {
        let d = devices["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| s(&d["name"]) == name)
            .unwrap();
        assert_eq!(i[key]["name"], d["name"]);
        for f in ["ed25519_public", "x25519_public", "device_id"] {
            assert_eq!(i[key][f], d[f], "{name} {f}");
        }
        let dev = r::Device::new(
            match name {
                "A" => "A",
                "B" => "B",
                _ => "C",
            },
            hx(&d["ed25519_seed"])[0],
            hx(&d["x25519_secret"])[0],
        );
        assert_eq!(h(&dev.ed_seed), s(&d["ed25519_seed"]));
        assert_eq!(h(&dev.x_secret), s(&d["x25519_secret"]));
    }
    assert_eq!(s(&i["devices_file"]), "spec/vectors/v1/device.json");
    assert_eq!(
        inputs_of(&v).space_id,
        devices["space_id"].as_str().unwrap()
    );
}

#[test]
fn nominal_session_is_recomputed_from_the_primitives() {
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let s_ = r::session(&inputs);
    let o = &v["outputs"];
    // Intermediate values.
    assert_eq!(h(&s_.prs), s(&o["prs"]));
    assert_eq!(h(&s_.ci), s(&o["ci"]));
    assert_eq!(s(&o["ci"]).len(), 78, "CI is 39 bytes");
    assert_eq!(h(&s_.ada), s(&o["ada"]));
    assert_eq!(h(&s_.adb), s(&o["adb"]));
    assert_eq!(s_.ada.len(), 22);
    assert_eq!(s_.adb.len(), 21);
    assert_eq!(h(&s_.gen_str), s(&o["generator_string"]));
    assert_eq!(s_.gen_str.len(), 185, "the example of §3.3");
    assert_eq!(u(&o["generator_string_len"]), 185);
    assert_eq!(h(&s_.gen_hash), s(&o["generator_hash"]));
    assert_eq!(h(&s_.g), s(&o["g"]));
    assert_eq!(h(&s_.ya_pub), s(&o["Ya"]));
    assert_eq!(h(&s_.yb_pub), s(&o["Yb"]));
    assert_eq!(h(&s_.commit), s(&o["commit"]));
    assert_eq!(h(&s_.k), s(&o["K"]));
    assert_eq!(h(&s_.isk), s(&o["ISK"]));
    assert_eq!(h(&s_.keys.k_i), s(&o["K_I"]));
    assert_eq!(h(&s_.keys.k_j), s(&o["K_J"]));
    assert_eq!(h(&s_.s8), s(&o["sas_s8"]));
    assert_eq!(format!("{:06}", s_.sas), s(&o["sas"]));
    assert_eq!(
        format!("{:03} {:03}", s_.sas / 1000, s_.sas % 1000),
        s(&o["sas_grouped"])
    );
    assert_eq!(
        r::uuid_string(&r::uuid_v4(&inputs.transfer_random)),
        s(&o["transfer_id"])
    );
    // The example of §3.3: PRS and the start of gen_str.
    assert_eq!(s(&o["prs"]), "01d10661");
    assert!(s(&o["generator_string"]).starts_with("1143506163655269737472657474"));
    assert_eq!(&s_.gen_str[18..24], &[0x04, 0x01, 0xd1, 0x06, 0x61, 0x68]);
    assert_eq!(
        s(&o["ci"]),
        "0f746f6d692f76312f706169722d63691172656c61792e6578616d706c652e6f72670434383231"
    );
    // A example of §5.4: AAD(0x03).
    assert_eq!(
        h(&r::aad(0x03, &inputs.sid)),
        "746f6d692f76312f6161642f70616972000103000102030405060708090a0b0c0d0e0f"
    );
    // The seven frames, and each box opened with the primitives.
    let frames = v["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 7);
    let sizes = [50, 34, 166, 106, 22, 744, 18];
    for (k, f) in frames.iter().enumerate() {
        assert_eq!(s(&f["name"]), r::FRAME_NAMES[k]);
        assert_eq!(u(&f["type"]) as u8, r::FRAME_TYPES[k]);
        assert_eq!(u(&f["size"]) as usize, sizes[k]);
        assert_eq!(hx(&f["frame"]), s_.frames[k], "frame {k}");
        assert_eq!(s_.frames[k][0], r::VERSION);
        assert_eq!(s_.frames[k][1], r::FRAME_TYPES[k]);
        if let Some((from_inviter, n)) = r::sealing(r::FRAME_TYPES[k]) {
            assert_eq!(s(&f["key"]), if from_inviter { "K_I" } else { "K_J" });
            assert_eq!(u(&f["nonce_counter"]) as u32, n);
            assert_eq!(hx(&f["aad"]), r::aad(r::FRAME_TYPES[k], &inputs.sid));
            assert_eq!(hx(&f["nonce"]), r::nonce(n));
            let boxed = hx(&f["box"]);
            let start = if k == 2 { 34 } else { 2 };
            assert_eq!(&s_.frames[k][start..], boxed.as_slice());
            let opened = r::open_box(&s_.keys, r::FRAME_TYPES[k], &inputs.sid, &boxed)
                .unwrap_or_else(|| panic!("frame {k} does not open"));
            assert_eq!(opened, hx(&f["body"]));
            // Altering the type byte, the sid or the counter breaks the box.
            let other_sid = r::seq(1, 16);
            assert!(r::open_box(&s_.keys, r::FRAME_TYPES[k], &other_sid, &boxed).is_none());
        } else {
            assert!(f["key"].is_null() && f["nonce_counter"].is_null());
        }
    }
    // The clear frames: sizes and payload.
    assert_eq!(&s_.frames[0][2..18], &inputs.sid);
    assert_eq!(&s_.frames[0][18..], &s_.commit);
    assert_eq!(&s_.frames[1][2..], &s_.yb_pub);
    assert_eq!(&s_.frames[2][2..34], &s_.ya_pub);
}

#[test]
fn the_batch_signatures_and_clear_key_are_valid() {
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let p = &v["provision"];
    assert_eq!(s(&p["space_id"]), inputs.space_id);
    // DeviceCert(B), signed by A: 208 bytes, strict verification.
    let cert = hx(&p["device_cert"]["wire"]);
    assert_eq!(cert.len(), 208);
    let (tbs, sig) = cert.split_at(cert.len() - 64);
    let a = VerifyingKey::from_bytes(&inputs.inviter.ed_pk).unwrap();
    a.verify_strict(tbs, &Signature::from_bytes(&r::arr(sig)))
        .unwrap();
    assert_eq!(&tbs[24 + 16..24 + 16 + 32], &inputs.joiner.ed_pk);
    assert_eq!(&tbs[tbs.len() - 32..], &inputs.inviter.ed_pk);
    // MemberList, signed by A: 152 + 64 * 3 bytes, sorted by Ed25519 key.
    let list = hx(&p["member_list"]["wire"]);
    assert_eq!(list.len(), 152 + 64 * 3);
    let (tbs, sig) = list.split_at(list.len() - 64);
    a.verify_strict(tbs, &Signature::from_bytes(&r::arr(sig)))
        .unwrap();
    let list = MemberList::from_bytes(&list).unwrap();
    assert_eq!(list.members.len(), 3);
    assert!(list.members.windows(2).all(|w| w[0].ed25519 < w[1].ed25519));
    let names: Vec<&str> = p["member_list"]["members_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(s)
        .collect();
    assert_eq!(
        names,
        ["C", "A", "B"],
        "the order of device.json's member list"
    );
    // The sealed key opens with B's keys only and gives the clear space key.
    let b_keys = driver::device_keys(&inputs.joiner);
    let sealed = hx(&p["sealed"]["hex"]);
    assert_eq!(sealed.len(), 104);
    let key = b_keys
        .open_space_key(&inputs.space_id, inputs.key_epoch, &sealed)
        .unwrap();
    assert_eq!(key.to_bytes(), inputs.space_key);
    assert!(driver::device_keys(&inputs.inviter)
        .open_space_key(&inputs.space_id, inputs.key_epoch, &sealed)
        .is_err());
    // The decoded body is the body of the sixth frame.
    let s_ = r::session(&inputs);
    assert_eq!(hx(&p["body"]), s_.provision_body);
    let history = &p["history"];
    assert_eq!(u(&history["items"]), 812);
    assert_eq!(u(&history["bytes"]), 356_515_840);
    assert_eq!(
        s(&history["transfer_id"]),
        "40414243-4445-4647-8849-4a4b4c4d4e4f"
    );
    // A third member C accepts the certificate of B, signed by A.
    let c_keys = driver::device_keys(&inputs.other);
    let mut trust_c = TrustStore::new(&inputs.space_id, &c_keys.public(), 1).unwrap();
    trust_c
        .add_verified_peer(&driver::device_keys(&inputs.inviter).public())
        .unwrap();
    trust_c
        .accept_device_cert(&DeviceCert::from_bytes(&hx(&p["device_cert"]["wire"])).unwrap())
        .unwrap();
    assert!(trust_c.is_member(&inputs.joiner.ed_pk));
}

#[test]
fn the_machines_produce_the_bytes_of_the_vector() {
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let fx = Fixture::new(&inputs);
    let run = run_nominal(&fx);
    assert_eq!(run.frames, frames_of(&v));
    assert_eq!(run.sas_inviter, s(&v["outputs"]["sas"]));
    assert_eq!(run.sas_joiner, s(&v["outputs"]["sas"]));
    // N's trust store, exactly as the core serialises it.
    assert_eq!(
        run.provision.trust.to_json().unwrap(),
        s(&v["trust_store_joiner"])
    );
    assert_eq!(
        run.outcome.staged_trust.to_json().unwrap(),
        s(&v["trust_store_inviter_staged"])
    );
    let trust = TrustStore::from_json(s(&v["trust_store_joiner"])).unwrap();
    for d in [&inputs.inviter, &inputs.joiner, &inputs.other] {
        assert!(trust.is_member(&d.ed_pk), "{}", d.name);
    }
    assert_eq!(trust.members().len(), 3);
    assert_eq!(trust.key_epoch(), 1);
    // The result of each side.
    assert_eq!(
        run.outcome.device_cert,
        hx(&v["provision"]["device_cert"]["wire"])
    );
    assert_eq!(run.outcome.key_epoch, 1);
    let history = run.outcome.history.clone().unwrap();
    assert_eq!(history.transfer_id, s(&v["outputs"]["transfer_id"]));
    assert_eq!(run.provision.history, Some(history));
    assert_eq!(
        run.provision.space_key.to_bytes(),
        hx(&v["inputs"]["space_key"])
    );
    assert_eq!(run.provision.device_name, "Pixel");
    assert_eq!(run.provision.inviter.name, "PC-Bureau");
}

#[test]
fn another_session_is_recomputed_too() {
    let v = load("pairing.json");
    let mut inputs = inputs_of(&v);
    let o = &v["other_session"];
    inputs.sid = a32_16(&o["sid"]);
    inputs.ya = a32(&o["ya"]);
    inputs.yb = a32(&o["yb"]);
    let other = r::session(&inputs);
    let want: Vec<Vec<u8>> = o["frames"].as_array().unwrap().iter().map(hx).collect();
    assert_eq!(other.frames, want);
    assert_ne!(other.frames, frames_of(&v), "P14 needs a different session");
    // None of the frames of the other session opens in the first one.
    let nominal = r::session(&inputs_of(&v));
    for k in 2..7 {
        let boxed = &other.frames[k][if k == 2 { 34 } else { 2 }..];
        assert!(r::open_box(&nominal.keys, r::FRAME_TYPES[k], &inputs_of(&v).sid, boxed).is_none());
    }
}

#[test]
fn man_in_the_middle_runs() {
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let fx = Fixture::new(&inputs);
    let m = &v["mitm"];
    let attacker = Attacker {
        sid: a32_16(&m["attacker"]["sid"]),
        ya: a32(&m["attacker"]["ya"]),
        yb: a32(&m["attacker"]["yb_toward_inviter"]),
    };
    let runs = m["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    for run in runs {
        let yb_n = a32(&run["yb_joiner"]);
        // Reference: the session between A and the attacker, and the one between the attacker
        // (which committed first) and N.
        let mut with_a = inputs.clone();
        with_a.yb = attacker.yb;
        let mut with_n = inputs.clone();
        with_n.sid = attacker.sid;
        with_n.ya = attacker.ya;
        with_n.yb = yb_n;
        let (sa, sn) = (r::session(&with_a), r::session(&with_n));
        assert_eq!(format!("{:06}", sa.sas), s(&run["sas_inviter"]));
        assert_eq!(format!("{:06}", sn.sas), s(&run["sas_joiner"]));
        assert_eq!(h(&sn.frames[0]), s(&run["attacker_commit_frame"]));
        // The machines agree.
        let (sas_a, sas_n, commit) = run_mitm(&fx, &attacker, yb_n);
        assert_eq!(sas_a, s(&run["sas_inviter"]));
        assert_eq!(sas_n, s(&run["sas_joiner"]));
        assert_eq!(h(&commit), s(&run["attacker_commit_frame"]));
        assert_ne!(sas_a, sas_n, "the two sessions have independent keys");
    }
    // A's code and the commit do not depend on yb of N; N's code does.
    assert_eq!(runs[0]["sas_inviter"], runs[1]["sas_inviter"]);
    assert_eq!(
        runs[0]["attacker_commit_frame"],
        runs[1]["attacker_commit_frame"]
    );
    assert_ne!(runs[0]["sas_joiner"], runs[1]["sas_joiner"]);
}

/// The frames the failure cases may refer to.
fn case_frames(v: &Value) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    (
        frames_of(v),
        v["other_session"]["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(hx)
            .collect(),
    )
}

#[test]
fn every_failure_case_is_replayed_through_the_machines() {
    let v = load("pairing.json");
    let fx = Fixture::new(&inputs_of(&v));
    let (nominal, other) = case_frames(&v);
    let frames = Frames {
        nominal: &nominal,
        other: &other,
    };
    let cases = v["failures"].as_array().unwrap();
    // P9 to P16 are all present (P11 is `mitm`, P17 is a test of the crate).
    let mut by_pxx = std::collections::BTreeMap::<&str, usize>::new();
    let mut ids = BTreeSet::new();
    for c in cases {
        *by_pxx.entry(s(&c["pxx"])).or_default() += 1;
        assert!(ids.insert(s(&c["id"])), "duplicate id {}", s(&c["id"]));
        run_case(&fx, c, &frames).unwrap_or_else(|e| panic!("{e}"));
    }
    for pxx in ["P9", "P10", "P12", "P13", "P14", "P15", "P16"] {
        assert!(
            by_pxx.get(pxx).copied().unwrap_or(0) > 0,
            "no case for {pxx}"
        );
    }
    // Every family has more than a handful of cases.
    assert!(by_pxx["P12"] >= 60 && by_pxx["P15"] >= 20 && by_pxx["P16"] >= 20);
    assert_eq!(cases.len(), by_pxx.values().sum::<usize>());
}

#[test]
fn the_interpreter_notices_a_wrong_expectation() {
    // A control: changing what a case expects makes the replay fail, so a passing replay means
    // something.
    let v = load("pairing.json");
    let fx = Fixture::new(&inputs_of(&v));
    let (nominal, other) = case_frames(&v);
    let frames = Frames {
        nominal: &nominal,
        other: &other,
    };
    let pick = |id: &str| -> Value {
        v["failures"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| s(&c["id"]) == id)
            .unwrap_or_else(|| panic!("no case {id}"))
            .clone()
    };
    for id in [
        "p10-betrayed-commitment",
        "p13-reveal-before-share",
        "p16-cert-self-signed",
        "p15-hello-unknown-tag-0x40",
    ] {
        run_case(&fx, &pick(id), &frames).unwrap();
        // Wrong final error.
        let mut wrong = pick(id);
        wrong["final_error"] = match wrong["final_error"].as_str() {
            Some("pairing_failed") => Value::from("pairing_unsupported"),
            _ => Value::from("pairing_failed"),
        };
        assert!(
            run_case(&fx, &wrong, &frames).is_err(),
            "{id}: wrong final error"
        );
        // Wrong step expectation.
        let mut wrong = pick(id);
        let steps = wrong["steps"].as_array_mut().unwrap();
        let last_receive = steps.iter().rposition(|st| st["do"] == "receive").unwrap();
        steps[last_receive]["expect"] = Value::from("ignored");
        assert!(
            run_case(&fx, &wrong, &frames).is_err(),
            "{id}: wrong step expectation"
        );
        // A produced frame that is not the one the machine writes (first step that succeeds).
        let mut wrong = pick(id);
        if wrong["steps"][0]["expect"].as_str().unwrap_or("ok") == "ok" {
            wrong["steps"][0]["produces"] = serde_json::json!({"hex": "0102"});
            assert!(
                run_case(&fx, &wrong, &frames).is_err(),
                "{id}: wrong produced frame"
            );
        }
    }
    // A case that should fail but whose frames are all genuine ends without error.
    let mut wrong = pick("p16-control-nominal-batch");
    wrong["final_error"] = Value::from("pairing_failed");
    assert!(run_case(&fx, &wrong, &frames).is_err());
}

#[test]
fn bodies_of_the_tlv_and_batch_cases_are_sealed_with_the_session_keys() {
    // The frame of each P15 / P16 case is the body sealed by the reference implementation.
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let nominal = r::session(&inputs);
    let mut checked = 0;
    for c in v["failures"].as_array().unwrap() {
        let Some(body) = c.get("body") else { continue };
        let body = hx(body);
        let step = c["steps"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|st| st["do"] == "receive")
            .unwrap();
        let frame = hx(&step["frame"]["hex"]);
        let t = frame[1];
        let rebuilt = if t == 0x03 {
            r::reveal_frame(&nominal.keys, &inputs.sid, &nominal.ya_pub, &body)
        } else {
            r::sealed_frame(&nominal.keys, t, &inputs.sid, &body)
        };
        assert_eq!(rebuilt, frame, "{}", s(&c["id"]));
        let start = if t == 0x03 { 34 } else { 2 };
        assert_eq!(
            r::open_box(&nominal.keys, t, &inputs.sid, &frame[start..]).as_deref(),
            Some(body.as_slice()),
            "{}",
            s(&c["id"])
        );
        checked += 1;
    }
    assert!(checked >= 40, "{checked} sealed cases");
}

#[test]
fn p9_cases_use_the_reference_computation_of_a_wrong_code() {
    // The frames of a wrong code come from the primitives, not from the machines.
    let v = load("pairing.json");
    let inputs = inputs_of(&v);
    let nominal = r::session(&inputs);
    let case = v["failures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| s(&c["id"]) == "p09-wrong-code-466-1633")
        .unwrap();
    let wrong_prs = r::prs([466, 1633]);
    let g = r::generator(&r::generator_string(&wrong_prs, &nominal.ci, &inputs.sid));
    let share = r::frame(0x02, &r::share(&inputs.yb, &g));
    assert_eq!(
        hx(&case["steps"][0]["produces"]["hex"]),
        share,
        "the share of a wrong word"
    );
    assert_ne!(share, nominal.frames[1]);
    // And that share leaves A with other keys, so the reveal can not open for N.
    let k = r::scalar_mult_vfy(&inputs.ya, &share[2..].try_into().unwrap());
    let isk = r::isk(
        &inputs.sid,
        &k,
        &nominal.ya_pub,
        &nominal.ada,
        &share[2..],
        &nominal.adb,
    );
    assert_ne!(isk, nominal.isk);
}
