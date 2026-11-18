//! Acceptance tests P1-P17 of `spec/pairing-v1.md` §14.2 (pure layer, no network).
//!
//! P1 and P2 (CPace, `prepend_len`) live in `cpace.rs`, P3 in `wordlist.rs`, P4 to P6 in
//! `code.rs`, the TLV rules of P15 in `tlv.rs`. This file holds the session tests: both state
//! machines against each other with fixed randomness (so every frame is reproducible), the
//! attacks of P9 to P14, the batch refusals of P16 and the secrets check of P17.

use super::batch;
use super::entropy::Entropy;
use super::frame::{self, FrameKind, MAX_FRAME_LEN};
use super::session::{self, SessionKeys};
use super::tlv::{self, InviterHello, JoinerConfirm, JoinerHello, ProvisionBody};
use super::*;
use crate::crypto::{new_space_id, parse_uuid};
use crate::{
    open_item, seal_item, DeviceCert, DeviceKeys, DevicePublic, Item, ItemText, MemberList,
    ReadPolicy, SourceFormat, SpaceKey, TrustStore,
};

const AUTHORITY: &str = "relay.example.org";
const CHANNEL: &str = "4821";
const NOW_MS: u64 = 1_793_750_460_000;
const OFFER: HistoryOffer = HistoryOffer {
    items: 812,
    bytes: 356_515_840,
};

fn seq(start: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| start.wrapping_add(i as u8)).collect()
}

fn hx32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn dev(ed: u8, x: u8) -> DeviceKeys {
    DeviceKeys::from_secrets(&seq(ed, 32), &seq(x, 32)).unwrap()
}

fn pk(p: &DevicePublic) -> ([u8; 32], [u8; 32]) {
    (
        p.ed25519.clone().try_into().unwrap(),
        p.x25519.clone().try_into().unwrap(),
    )
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The space of the tests: A (inviter) and C (another member); N joins.
struct World {
    a: DeviceKeys,
    space_id: String,
    space_key: SpaceKey,
    trust_a: TrustStore,
}

fn world() -> World {
    let a = dev(0xa0, 0x10);
    let c = dev(0xe0, 0x50);
    let space_id = "5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10".to_owned();
    let space_key = SpaceKey::from_bytes(1, &seq(0, 32)).unwrap();
    let mut trust_a = TrustStore::new(&space_id, &a.public(), 1).unwrap();
    trust_a.add_verified_peer(&c.public()).unwrap();
    World {
        a,
        space_id,
        space_key,
        trust_a,
    }
}

fn keys_n() -> DeviceKeys {
    dev(0xc0, 0x30)
}

/// The randomness of one session: draft scalars for `ya` / `yb`, `sid = 00..0f`.
#[derive(Clone)]
struct Rand {
    ranks: [u16; 2],
    sid: [u8; 16],
    ya: [u8; 32],
    yb: [u8; 32],
    seal_ephemeral: [u8; 32],
    seal_nonce: [u8; 24],
    transfer_id: [u8; 16],
}

fn rand() -> Rand {
    Rand {
        ranks: [465, 1633],
        sid: std::array::from_fn(|i| i as u8),
        ya: hx32("da3d23700a9e5699258aef94dc060dfda5ebb61f02a5ea77fad53f4ff0976d08"),
        yb: hx32("d2316b454718c35362d83d69df6320f38578ed5984651435e2949762d900b80d"),
        seal_ephemeral: [0x5c; 32],
        seal_nonce: std::array::from_fn(|i| 0xd0 + i as u8),
        transfer_id: std::array::from_fn(|i| 0x40 + i as u8),
    }
}

impl Rand {
    fn inviter_script(&self) -> Vec<Vec<u8>> {
        vec![
            self.ranks[0].to_be_bytes().to_vec(),
            self.ranks[1].to_be_bytes().to_vec(),
            self.sid.to_vec(),
            self.ya.to_vec(),
            self.seal_ephemeral.to_vec(),
            self.seal_nonce.to_vec(),
            self.transfer_id.to_vec(),
        ]
    }

    fn joiner_script(&self) -> Vec<Vec<u8>> {
        vec![self.yb.to_vec()]
    }

    fn code(&self) -> PairingCode {
        PairingCode::from_parts(CHANNEL, self.ranks).unwrap()
    }
}

fn inviter_params(offer: Option<HistoryOffer>) -> InviterParams {
    InviterParams {
        relay_authority: AUTHORITY.into(),
        channel_id: CHANNEL.into(),
        device_name: "PC-Bureau".into(),
        platform: "windows".into(),
        history_offer: offer,
    }
}

fn joiner_params(code: PairingCode) -> JoinerParams {
    JoinerParams {
        relay_authority: AUTHORITY.into(),
        code,
        device_name: "Pixel".into(),
        platform: "android".into(),
    }
}

fn start_inviter(w: &World, r: &Rand, offer: Option<HistoryOffer>) -> Inviter {
    kat::inviter_start(inviter_params(offer), &w.a, &w.trust_a, r.inviter_script()).unwrap()
}

fn start_joiner(r: &Rand, code: PairingCode) -> Joiner {
    kat::joiner_new(joiner_params(code), keys_n(), r.joiner_script()).unwrap()
}

/// The frame indices of the nominal sequence.
const COMMIT: usize = 0;
const SHARE: usize = 1;
const REVEAL: usize = 2;
const HELLO: usize = 3;
const CONFIRM: usize = 4;
const PROVISION: usize = 5;
const ACK: usize = 6;
const KINDS: [FrameKind; 7] = [
    FrameKind::Commit,
    FrameKind::Share,
    FrameKind::Reveal,
    FrameKind::JoinerHello,
    FrameKind::JoinerConfirm,
    FrameKind::Provision,
    FrameKind::JoinerAck,
];

/// Frames travel over a wire that a test can tamper with.
struct Wire<'a> {
    /// Every frame produced by a machine, in order, before any tampering.
    produced: Vec<Vec<u8>>,
    tamper: &'a mut dyn FnMut(usize, &mut Vec<u8>),
}

impl Wire<'_> {
    fn carry(&mut self, frame: Vec<u8>) -> Vec<u8> {
        let index = self.produced.len();
        self.produced.push(frame.clone());
        let mut frame = frame;
        (self.tamper)(index, &mut frame);
        frame
    }
}

/// The whole sequence between two machines, both users confirming. Stops at the first error.
fn pump(
    a: &mut Inviter,
    n: &mut Joiner,
    w: &World,
    import_history: bool,
    wire: &mut Wire<'_>,
) -> Result<(), CoreError> {
    let f = wire.carry(a.commit_frame().to_vec());
    let share = n.receive(&f)?.send.unwrap();
    let f = wire.carry(share);
    let reveal = a.receive(&f)?.send.unwrap();
    let f = wire.carry(reveal);
    let hello = n.receive(&f)?.send.unwrap();
    let f = wire.carry(hello);
    a.receive(&f)?;
    a.confirm()?;
    let f = wire.carry(n.confirm(import_history)?);
    a.receive(&f)?;
    let f = wire.carry(a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS)?);
    let ack = n.receive(&f)?.send.unwrap();
    let f = wire.carry(ack);
    a.receive(&f)?;
    Ok(())
}

/// Result of a run: the frames the machines produced, and how it ended.
struct Run {
    produced: Vec<Vec<u8>>,
    result: Result<(), CoreError>,
}

fn run_with(
    w: &World,
    r: &Rand,
    offer: Option<HistoryOffer>,
    import_history: bool,
    n_code: PairingCode,
    tamper: &mut dyn FnMut(usize, &mut Vec<u8>),
) -> (Run, Inviter, Joiner) {
    let mut a = start_inviter(w, r, offer);
    let mut n = start_joiner(r, n_code);
    let mut wire = Wire {
        produced: Vec::new(),
        tamper,
    };
    let result = pump(&mut a, &mut n, w, import_history, &mut wire);
    (
        Run {
            produced: wire.produced,
            result,
        },
        a,
        n,
    )
}

/// A nominal run, both users confirming, history offered and requested.
fn nominal(w: &World, r: &Rand) -> (Run, Inviter, Joiner) {
    run_with(w, r, Some(OFFER), true, r.code(), &mut |_, _| {})
}

/// The seven frames of the nominal session.
fn pristine(w: &World, r: &Rand) -> Vec<Vec<u8>> {
    let (run, ..) = nominal(w, r);
    run.result.unwrap();
    assert_eq!(run.produced.len(), 7);
    run.produced
}

/// Both machines after the first `until` actions of the nominal sequence:
///
/// ```text
/// 1 N <- commit      2 A <- share        3 N <- reveal       4 A <- joiner_hello
/// 5 A user confirms  6 N user confirms   7 A <- joiner_confirm
/// 8 A provision      9 N <- provision   10 A <- joiner_ack
/// ```
fn advance(w: &World, r: &Rand, until: usize) -> (Inviter, Joiner, Vec<Vec<u8>>) {
    let mut a = start_inviter(w, r, Some(OFFER));
    let mut n = start_joiner(r, r.code());
    let mut f = vec![a.commit_frame().to_vec()];
    if until >= 1 {
        f.push(n.receive(&f[COMMIT]).unwrap().send.unwrap());
    }
    if until >= 2 {
        f.push(a.receive(&f[SHARE]).unwrap().send.unwrap());
    }
    if until >= 3 {
        f.push(n.receive(&f[REVEAL]).unwrap().send.unwrap());
    }
    if until >= 4 {
        a.receive(&f[HELLO]).unwrap();
    }
    if until >= 5 {
        a.confirm().unwrap();
    }
    if until >= 6 {
        f.push(n.confirm(true).unwrap());
    }
    if until >= 7 {
        a.receive(&f[CONFIRM]).unwrap();
    }
    if until >= 8 {
        f.push(a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS).unwrap());
    }
    if until >= 9 {
        f.push(n.receive(&f[PROVISION]).unwrap().send.unwrap());
    }
    if until >= 10 {
        a.receive(&f[ACK]).unwrap();
    }
    (a, n, f)
}

/// `SessionKeys` and verification code of the session of `r` when N typed the code of `ranks`.
fn session_keys(r: &Rand, ranks: [u16; 2]) -> (SessionKeys, Sas) {
    let code = PairingCode::from_parts(CHANNEL, ranks).unwrap();
    let ci = session::channel_identifier(AUTHORITY, CHANNEL);
    let trace = kat::cpace_trace(
        code.prs().as_slice(),
        &ci,
        &r.sid,
        &session::ad_inviter(),
        &session::ad_joiner(),
        &r.ya,
        &r.yb,
    )
    .unwrap();
    let isk: [u8; 64] = trace.isk_initiator.try_into().unwrap();
    session::derive(&isk).unwrap()
}

/// A sealed frame built by hand (an attacker, or a peer of another version).
fn sealed(keys: &SessionKeys, kind: FrameKind, sid: &[u8; 16], body: &[u8]) -> Vec<u8> {
    frame::encode(kind, &frame::seal_body(keys, kind, sid, body).unwrap()).unwrap()
}

fn kind_of(frame: &[u8]) -> Option<FrameKind> {
    FrameKind::from_byte(frame[1])
}

fn no_provision_was_sent(run: &Run) -> bool {
    run.produced
        .iter()
        .all(|f| kind_of(f) != Some(FrameKind::Provision))
}

// ---------------------------------------------------------------------------
// P7: nominal session
// ---------------------------------------------------------------------------

#[test]
fn p07_nominal_session() {
    let w = world();
    let r = rand();

    // Both devices show the same verification code once the hello is exchanged (A7, N7).
    let (a4, n4, _) = advance(&w, &r, 4);
    let sas = a4.sas().unwrap();
    assert_eq!(Some(sas), n4.sas());
    assert_eq!(sas.digits().len(), 6);
    assert_eq!(sas.grouped().len(), 7);
    drop((a4, n4));

    let (run, mut a, mut n) = nominal(&w, &r);
    run.result.unwrap();
    // Sizes of the seven frames (§6.1): 50, 34, 50+116, 18+88, 22, 552+64*3, 18.
    let sizes: Vec<usize> = run.produced.iter().map(Vec::len).collect();
    assert_eq!(sizes, [50, 34, 166, 106, 22, 744, 18]);
    for (f, kind) in run.produced.iter().zip(KINDS) {
        assert_eq!(f[0], PAIR_VERSION);
        assert_eq!(kind_of(f), Some(kind));
    }
    assert_eq!(a.phase(), InviterPhase::Done);
    assert_eq!(n.phase(), JoinerPhase::Done);
    assert!(a.sas().is_some(), "A keeps the code until it finishes");
    assert!(
        n.sas().is_none(),
        "N erased its code once the batch is verified"
    );

    // What each side learned about the other.
    let peer = a.peer().unwrap().clone();
    assert_eq!(
        (peer.name.as_str(), peer.platform.as_str()),
        ("Pixel", "android")
    );
    assert_eq!(peer.public, keys_n().public());
    let info = n.inviter().unwrap().clone();
    assert_eq!(info.peer.name, "PC-Bureau");
    assert_eq!(info.peer.platform, "windows");
    assert_eq!(info.peer.public, w.a.public());
    assert_eq!(info.member_count, Some(2));
    assert_eq!(info.history_offer, Some(OFFER));

    // A's result: its store plus the new device, the certificate, the announced history.
    let outcome = a.finish().unwrap();
    assert_eq!(outcome.key_epoch, 1);
    assert_eq!(outcome.peer.public, keys_n().public());
    assert!(outcome.staged_trust.is_member(&keys_n().public().ed25519));
    assert_eq!(outcome.staged_trust.members().len(), 3);
    assert!(
        !w.trust_a.is_member(&keys_n().public().ed25519),
        "A's own store is untouched"
    );
    let history = outcome.history.clone().unwrap();
    assert_eq!(history.transfer_id, "40414243-4445-4647-8849-4a4b4c4d4e4f");
    assert_eq!((history.items, history.bytes), (OFFER.items, OFFER.bytes));
    assert_eq!(
        a.finish().unwrap_err().code(),
        "invalid_input",
        "taken once"
    );
    // The session is over: A erased the code, the verification code and the peer.
    assert!(a.code().is_none() && a.sas().is_none() && a.peer().is_none());

    // N's result: A's keys, the other members, the space key.
    let p = n.take_provision().unwrap();
    assert_eq!(p.device_name, "Pixel");
    assert_eq!(p.keys.public(), keys_n().public());
    assert_eq!(p.inviter.public, w.a.public());
    assert_eq!(p.history, Some(history));
    assert_eq!(p.trust.space_id(), w.space_id);
    assert_eq!(p.trust.key_epoch(), 1);
    for d in [dev(0xa0, 0x10), dev(0xc0, 0x30), dev(0xe0, 0x50)] {
        assert!(p.trust.is_member(&d.public().ed25519));
    }
    assert_eq!(p.trust.members().len(), 3);
    assert_eq!(p.space_key.to_bytes(), w.space_key.to_bytes());
    assert_eq!(n.take_provision().unwrap_err().code(), "invalid_input");
    assert!(
        n.inviter().is_none(),
        "N erased what it learned once the batch is taken"
    );

    // The space key opens an item sealed by A.
    let item = Item {
        id: crate::new_item_id().unwrap(),
        source_device: w.a.device_id(),
        created_at_ms: 1,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
        secret: false,
        title: None,
        text: Some(ItemText {
            plain: "bonjour".into(),
            html: None,
            markdown: None,
            source_format: SourceFormat::Plain,
        }),
        url: None,
        attachments: vec![],
        raw_payload: None,
    };
    let envelope = seal_item(&item, &w.space_key).unwrap();
    let opened = open_item(&envelope, &item.id, &p.space_key).unwrap();
    assert_eq!(opened.item.text.unwrap().plain, "bonjour");

    // A third member C verifies the certificate of N with A's key (`accept_device_cert`).
    let cert = DeviceCert::from_bytes(&outcome.device_cert).unwrap();
    assert_eq!(cert.subject, keys_n().public());
    assert_eq!(cert.issuer_ed25519, w.a.public().ed25519);
    let c = dev(0xe0, 0x50);
    let mut trust_c = TrustStore::new(&w.space_id, &c.public(), 1).unwrap();
    trust_c.add_verified_peer(&w.a.public()).unwrap();
    trust_c.accept_device_cert(&cert).unwrap();
    assert!(trust_c.is_member(&keys_n().public().ed25519));
}

#[test]
fn nominal_session_with_os_randomness() {
    // The production path: OS entropy for the code, sid, scalars, sealing, fresh device keys.
    let w = world();
    let mut a = Inviter::start(inviter_params(None), &w.a, &w.trust_a).unwrap();
    let code = PairingCode::read(&a.code().unwrap().link(AUTHORITY), AUTHORITY).unwrap();
    let mut n = Joiner::new(joiner_params(code)).unwrap();
    let share = n.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal = a.receive(&share).unwrap().send.unwrap();
    let hello = n.receive(&reveal).unwrap().send.unwrap();
    a.receive(&hello).unwrap();
    assert_eq!(a.sas(), n.sas());
    a.confirm().unwrap();
    let confirm = n.confirm(true).unwrap();
    assert_eq!(n.inviter().unwrap().history_offer, None);
    a.receive(&confirm).unwrap();
    let provision = a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS).unwrap();
    let ack = n.receive(&provision).unwrap().send.unwrap();
    a.receive(&ack).unwrap();
    let outcome = a.finish().unwrap();
    assert!(outcome.history.is_none(), "no offer, nothing announced");
    let p = n.take_provision().unwrap();
    assert_eq!(p.space_key.to_bytes(), w.space_key.to_bytes());
    assert_eq!(outcome.peer.public, p.keys.public());
}

#[cfg(feature = "runtime")]
#[test]
fn verified_batch_feeds_the_engine() {
    use crate::runtime::mock::{MockRelay, TestClock};
    use crate::runtime::{Engine, EngineConfig, Store};

    let w = world();
    let (_, _, mut n) = nominal(&w, &rand());
    let (provision, history) = n.take_provision().unwrap().into_provision();
    assert!(history.is_some());
    assert_eq!(provision.device_name, "Pixel");
    assert_eq!(provision.keys.public(), keys_n().public());
    assert_eq!(provision.trust.members().len(), 3);

    let clock = TestClock::new(1_793_836_800_000);
    let relay = MockRelay::new(&w.space_id, clock.clone());
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("tomi.db"),
        &dir.path().join("files"),
        &[9u8; 32],
    )
    .unwrap();
    let engine = Engine::open(EngineConfig::default(), store, relay.transport(), clock).unwrap();
    assert!(!engine.is_provisioned());
    engine.provision(provision).unwrap();
    assert!(engine.is_provisioned());
}

// ---------------------------------------------------------------------------
// P8: size of the batch
// ---------------------------------------------------------------------------

/// A space whose inviter already has `others` other members; N makes it `others + 2`.
fn crowded_world(others: usize) -> World {
    let a = DeviceKeys::generate().unwrap();
    let space_id = new_space_id().unwrap();
    let mut trust_a = TrustStore::new(&space_id, &a.public(), 1).unwrap();
    for _ in 0..others {
        trust_a
            .add_verified_peer(&DeviceKeys::generate().unwrap().public())
            .unwrap();
    }
    World {
        a,
        space_id,
        space_key: SpaceKey::from_bytes(1, &seq(0, 32)).unwrap(),
        trust_a,
    }
}

#[test]
fn p08_provision_with_119_members_fits_and_with_120_fails_before_writing() {
    let r = rand();
    // 118 existing members + N = 119 in the list: 552 + 64 * 119 = 8168 bytes.
    let w = crowded_world(117);
    assert_eq!(w.trust_a.members().len(), 118);
    let (run, ..) = run_with(&w, &r, Some(OFFER), true, r.code(), &mut |_, _| {});
    run.result.unwrap();
    assert_eq!(run.produced[PROVISION].len(), 552 + 64 * 119);
    assert!(run.produced[PROVISION].len() <= MAX_FRAME_LEN);
    // Without the history announcement: 517 + 64 * 119.
    let (run, ..) = run_with(&w, &r, None, false, r.code(), &mut |_, _| {});
    run.result.unwrap();
    assert_eq!(run.produced[PROVISION].len(), 517 + 64 * 119);

    // 119 existing members + N = 120 in the list: the frame would be 8232 bytes.
    let w = crowded_world(118);
    assert_eq!(w.trust_a.members().len(), 119);
    let (run, a, n) = run_with(&w, &r, Some(OFFER), true, r.code(), &mut |_, _| {});
    assert_eq!(run.result.clone().unwrap_err(), CoreError::PairingFailed);
    // A failed before writing anything: no provision frame, N never received a key.
    assert_eq!(run.produced.len(), 5);
    assert!(no_provision_was_sent(&run));
    assert_eq!(a.phase(), InviterPhase::Failed);
    assert_eq!(n.phase(), JoinerPhase::AwaitProvision);

    // 20 devices (the default maximum) stay far below the limit: 1832 bytes.
    let w = crowded_world(18);
    let (run, ..) = run_with(&w, &r, Some(OFFER), true, r.code(), &mut |_, _| {});
    run.result.unwrap();
    assert_eq!(run.produced[PROVISION].len(), 552 + 64 * 20);
    assert_eq!(552 + 64 * 20, 1832);
}

// ---------------------------------------------------------------------------
// P9: wrong code. The causes are indistinguishable.
// ---------------------------------------------------------------------------

#[test]
fn p09_a_neighbouring_word_fails_at_n_and_a_never_opens_a_hello() {
    let w = world();
    let r = rand();
    for wrong in [[466, 1633], [465, 1632], [464, 1633], [465, 1634], [0, 0]] {
        let code = PairingCode::from_parts(CHANNEL, wrong).unwrap();
        let (run, a, n) = run_with(&w, &r, Some(OFFER), true, code, &mut |_, _| {});
        assert_eq!(
            run.result.unwrap_err(),
            CoreError::PairingFailed,
            "{wrong:?}"
        );
        // commit, share, reveal were produced; N failed on the reveal; no joiner_hello exists.
        assert_eq!(run.produced.len(), 3);
        assert_eq!(n.phase(), JoinerPhase::Failed);
        assert_eq!(a.phase(), InviterPhase::AwaitJoinerHello);
        assert!(a.peer().is_none(), "A never opened a joiner_hello");
        assert!(
            n.sas().is_none() && n.inviter().is_none(),
            "nothing kept on N"
        );
    }
}

#[test]
fn p09_every_cause_of_a_failed_key_agreement_is_the_same_error() {
    let w = world();
    let r = rand();
    let sid = r.sid;
    let keys = session_keys(&r, r.ranks).0;
    let (ed_pk, x_pk) = pk(&w.a.public());
    let hello = InviterHello {
        ed_pk,
        x_pk,
        name: "PC".into(),
        platform: "windows".into(),
        member_count: None,
        history_offer: None,
    }
    .encode()
    .unwrap();
    let genuine = pristine(&w, &r);
    let ya_good: [u8; 32] = genuine[REVEAL][2..34].try_into().unwrap();

    // Attacker-chosen "Ya" values with a commit that matches them, so only the point check fires.
    let reveal_with = |ya: [u8; 32], box_bytes: &[u8]| {
        let mut payload = ya.to_vec();
        payload.extend_from_slice(box_bytes);
        frame::encode(FrameKind::Reveal, &payload).unwrap()
    };
    let commit_for = |ya: &[u8; 32]| {
        let mut payload = sid.to_vec();
        payload.extend_from_slice(&session::commitment(&sid, ya));
        frame::encode(FrameKind::Commit, &payload).unwrap()
    };
    let invalid_point = hx32("2b3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c");
    let neutral = [0u8; 32];
    let genuine_box = frame::seal_body(&keys, FrameKind::Reveal, &sid, &hello).unwrap();

    let mut errors = Vec::new();
    // 1. Wrong code (the box does not open).
    let wrong = PairingCode::from_parts(CHANNEL, [465, 1634]).unwrap();
    errors.push(
        run_with(&w, &r, None, false, wrong, &mut |_, _| {})
            .0
            .result
            .unwrap_err(),
    );
    // 2. Betrayed engagement: another Ya than the committed one.
    errors.push(
        run_with(&w, &r, None, false, r.code(), &mut |i, f| {
            if i == REVEAL {
                f[2..34].copy_from_slice(&neutral);
            }
        })
        .0
        .result
        .unwrap_err(),
    );
    // 3. Ya that does not decode, 4. Ya = neutral element (K = I): the commit matches them.
    for bad in [invalid_point, neutral] {
        let mut n = start_joiner(&r, r.code());
        n.receive(&commit_for(&bad)).unwrap();
        errors.push(n.receive(&reveal_with(bad, &genuine_box)).unwrap_err());
        assert_eq!(n.phase(), JoinerPhase::Failed);
    }
    // 5. A flipped bit in the box.
    errors.push(
        run_with(&w, &r, None, false, r.code(), &mut |i, f| {
            if i == REVEAL {
                let last = f.len() - 1;
                f[last] ^= 1;
            }
        })
        .0
        .result
        .unwrap_err(),
    );
    // 6. A reveal whose box is too short to hold a tag.
    let mut n = start_joiner(&r, r.code());
    n.receive(&commit_for(&ya_good)).unwrap();
    errors.push(
        n.receive(&reveal_with(ya_good, &genuine_box[..10]))
            .unwrap_err(),
    );
    // Identical values, not merely identical codes.
    assert_eq!(errors.len(), 6);
    assert!(
        errors.iter().all(|e| *e == CoreError::PairingFailed),
        "{errors:?}"
    );
    assert!(errors
        .iter()
        .all(|e| e.to_string() == CoreError::PairingFailed.to_string()));
}

#[test]
fn p09_a_hello_sealed_under_the_wrong_keys_fails_the_inviter_the_same_way() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    let wrong_keys = session_keys(&r, [465, 1634]).0;
    let (ed_pk, x_pk) = pk(&keys_n().public());
    let hello = JoinerHello {
        ed_pk,
        x_pk,
        name: "Pixel".into(),
        platform: "android".into(),
    }
    .encode()
    .unwrap();
    let mut a = start_inviter(&w, &r, None);
    a.receive(&f[SHARE]).unwrap();
    let forged = sealed(&wrong_keys, FrameKind::JoinerHello, &r.sid, &hello);
    assert_eq!(a.receive(&forged).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_inviter(&a);
}

// ---------------------------------------------------------------------------
// P10: betrayed engagement
// ---------------------------------------------------------------------------

#[test]
fn p10_a_reveal_with_another_ya_than_the_committed_one_is_refused() {
    let w = world();
    let r = rand();
    // The attacker committed to the real Ya (the genuine commit frame) but reveals Ya', with a
    // perfectly valid box under the session keys derived from Ya' and the real Yb.
    let genuine = pristine(&w, &r);
    let other_ya = hx32("1111111111111111111111111111111111111111111111111111111111111101");
    let ci = session::channel_identifier(AUTHORITY, CHANNEL);
    let trace = kat::cpace_trace(
        r.code().prs().as_slice(),
        &ci,
        &r.sid,
        &session::ad_inviter(),
        &session::ad_joiner(),
        &other_ya,
        &r.yb,
    )
    .unwrap();
    assert_ne!(&trace.ya_public[..], &genuine[REVEAL][2..34]);
    let isk: [u8; 64] = trace.isk_initiator.clone().try_into().unwrap();
    let (keys2, _) = session::derive(&isk).unwrap();
    let (ed_pk, x_pk) = pk(&w.a.public());
    let hello = InviterHello {
        ed_pk,
        x_pk,
        name: "PC-Bureau".into(),
        platform: "windows".into(),
        member_count: None,
        history_offer: None,
    }
    .encode()
    .unwrap();
    let boxed = frame::seal_body(&keys2, FrameKind::Reveal, &r.sid, &hello).unwrap();
    let mut payload = trace.ya_public.to_vec();
    payload.extend_from_slice(&boxed);
    let betrayal = frame::encode(FrameKind::Reveal, &payload).unwrap();

    let mut n = start_joiner(&r, r.code());
    n.receive(&genuine[COMMIT]).unwrap();
    assert_eq!(n.receive(&betrayal).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_joiner(&mut n);

    // Control: had the attacker committed to Ya' first, the very same reveal would be accepted.
    // Only the engagement stops it.
    let mut commit_payload = r.sid.to_vec();
    commit_payload.extend_from_slice(&session::commitment(&r.sid, &trace.ya_public));
    let honest_commit = frame::encode(FrameKind::Commit, &commit_payload).unwrap();
    let mut n = start_joiner(&r, r.code());
    n.receive(&honest_commit).unwrap();
    let step = n.receive(&betrayal).unwrap();
    assert_eq!(step.event, JoinerEvent::InviterHello);
}

// ---------------------------------------------------------------------------
// P11: man in the middle who knows the code
// ---------------------------------------------------------------------------

/// A relay that knows the code and runs one session with A (as a joiner) and one with N (as an
/// inviter). Returns the verification codes of A and of N, and the commit it sent to N.
fn mitm(w: &World, r: &Rand, yb_n: [u8; 32]) -> (Sas, Sas, Vec<u8>) {
    // Honest A, honest N (which got the same code), and the attacker's two halves.
    let mut a = start_inviter(w, r, None);
    let mut n = kat::joiner_new(joiner_params(r.code()), keys_n(), vec![yb_n.to_vec()]).unwrap();
    let keys_m = DeviceKeys::generate().unwrap();
    let space_m = new_space_id().unwrap();
    let trust_m = TrustStore::new(&space_m, &keys_m.public(), 1).unwrap();
    let mut rm = rand();
    rm.sid = std::array::from_fn(|i| 0xf0 + i as u8);
    rm.ya = hx32("0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f00");
    rm.yb = hx32("a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf00");
    let mut toward_n = kat::inviter_start(
        InviterParams {
            device_name: "Relais".into(),
            platform: "linux".into(),
            ..inviter_params(None)
        },
        &keys_m,
        &trust_m,
        rm.inviter_script(),
    )
    .unwrap();
    let mut toward_a = kat::joiner_new(
        joiner_params(r.code()),
        DeviceKeys::generate().unwrap(),
        rm.joiner_script(),
    )
    .unwrap();

    // With A: the attacker plays N, with the code.
    let share_m = toward_a.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal_a = a.receive(&share_m).unwrap().send.unwrap();
    toward_a.receive(&reveal_a).unwrap();
    // With N: the attacker plays A. Its commit goes out before it sees the share of N.
    let commit_m = toward_n.commit_frame().to_vec();
    let share_n = n.receive(&commit_m).unwrap().send.unwrap();
    let reveal_m = toward_n.receive(&share_n).unwrap().send.unwrap();
    n.receive(&reveal_m).unwrap();

    // The attacker knows both codes, but they come from two independent session keys.
    assert_eq!(toward_a.sas(), a.sas());
    assert_eq!(toward_n.sas(), n.sas());
    (a.sas().unwrap().clone(), n.sas().unwrap().clone(), commit_m)
}

#[test]
fn p11_a_relay_that_knows_the_code_cannot_make_the_two_codes_agree() {
    let w = world();
    let r = rand();
    let yb_1 = r.yb;
    let yb_2 = hx32("0f0e0d0c0b0a09080706050403020100ffeeddccbbaa99887766554433221100");
    let (sas_a_1, sas_n_1, commit_1) = mitm(&w, &r, yb_1);
    let (sas_a_2, sas_n_2, commit_2) = mitm(&w, &r, yb_2);
    // The two sessions have independent keys: the codes differ.
    assert_ne!(sas_a_1, sas_n_1);
    assert_ne!(sas_a_2, sas_n_2);
    // The attacker fixed its randomness and committed before seeing the share of N: its commit,
    // and the code of A, do not depend on `yb`; the code of N does, and the attacker can not
    // recompute it before it has committed.
    assert_eq!(commit_1, commit_2);
    assert_eq!(sas_a_1, sas_a_2);
    assert_ne!(sas_n_1, sas_n_2);
}

// ---------------------------------------------------------------------------
// P12: altered, unknown, truncated, oversized frames
// ---------------------------------------------------------------------------

#[test]
fn p12_a_flipped_bit_anywhere_fails_the_session() {
    let w = world();
    let r = rand();
    let pristine = pristine(&w, &r);
    for (index, original) in pristine.iter().enumerate() {
        let len = original.len();
        // Header (version, type), start / middle / end of the payload, last byte (the tag).
        for offset in [0, 1, 2, len / 2, len - 1] {
            let (run, ..) = run_with(&w, &r, Some(OFFER), true, r.code(), &mut |i, f| {
                if i == index {
                    f[offset] ^= 0x01;
                }
            });
            let expected = if offset == 0 && (index == COMMIT || index == SHARE) {
                // The first frame each side reads: an unknown version means "update Tomi".
                CoreError::PairingUnsupported
            } else {
                CoreError::PairingFailed
            };
            assert_eq!(
                run.result.clone().unwrap_err(),
                expected,
                "frame {index} ({:?}), byte {offset}",
                KINDS[index]
            );
            // Before the batch is built, the space key never leaves A.
            if index < PROVISION {
                assert!(
                    no_provision_was_sent(&run),
                    "frame {index}, byte {offset}: a key was sent after a tampered frame"
                );
            }
        }
    }
}

#[test]
fn p12_version_two_in_the_first_frame_is_unsupported_later_it_is_a_failure() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    // The first frame of N, the first frame of A.
    let mut n = start_joiner(&r, r.code());
    let mut v2 = f[COMMIT].clone();
    v2[0] = 2;
    assert_eq!(n.receive(&v2).unwrap_err(), CoreError::PairingUnsupported);
    assert_eq!(n.phase(), JoinerPhase::Failed);
    let mut a = start_inviter(&w, &r, None);
    let mut v2 = f[SHARE].clone();
    v2[0] = 2;
    assert_eq!(a.receive(&v2).unwrap_err(), CoreError::PairingUnsupported);
    // Any other version, even on a frame whose type is unknown.
    for version in [0u8, 3, 0x80, 0xff] {
        let mut n = start_joiner(&r, r.code());
        let mut bad = f[COMMIT].clone();
        bad[0] = version;
        bad[1] = 0x42;
        assert_eq!(n.receive(&bad).unwrap_err(), CoreError::PairingUnsupported);
    }
    // Later frames: a failure, not "unsupported".
    let (_, mut n, _) = advance(&w, &r, 1);
    let mut v2 = f[REVEAL].clone();
    v2[0] = 2;
    assert_eq!(n.receive(&v2).unwrap_err(), CoreError::PairingFailed);
}

/// A way to damage a frame.
type Change = Box<dyn Fn(&mut Vec<u8>)>;

#[test]
fn p12_unknown_types_and_truncated_or_oversized_frames_are_failures() {
    let w = world();
    let r = rand();
    let pristine = pristine(&w, &r);
    for index in 0..7 {
        let len = pristine[index].len();
        let cases: Vec<(&str, Change)> = vec![
            ("type 0x08", Box::new(|f| f[1] = 0x08)),
            ("type 0x00", Box::new(|f| f[1] = 0x00)),
            ("type 0xff", Box::new(|f| f[1] = 0xff)),
            ("one byte short", Box::new(move |f| f.truncate(len - 1))),
            ("header only", Box::new(|f| f.truncate(2))),
            ("one byte", Box::new(|f| f.truncate(1))),
            ("empty", Box::new(|f| f.clear())),
            ("one byte long", Box::new(|f| f.push(0))),
            ("8193 bytes", Box::new(|f| f.resize(MAX_FRAME_LEN + 1, 0))),
        ];
        for (name, change) in cases {
            let (run, ..) = run_with(&w, &r, Some(OFFER), true, r.code(), &mut |i, f| {
                if i == index {
                    change(f);
                }
            });
            assert_eq!(
                run.result.unwrap_err(),
                CoreError::PairingFailed,
                "frame {index} ({:?}), {name}",
                KINDS[index]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// P13: order
// ---------------------------------------------------------------------------

fn assert_wiped_inviter(a: &Inviter) {
    assert_eq!(a.phase(), InviterPhase::Failed);
    assert!(a.sas().is_none() && a.peer().is_none() && a.code().is_none());
}

fn assert_wiped_joiner(n: &mut Joiner) {
    assert_eq!(n.phase(), JoinerPhase::Failed);
    assert!(n.sas().is_none() && n.inviter().is_none());
    assert!(n.take_provision().is_err());
}

#[test]
fn p13_a_frame_out_of_order_fails_and_wipes() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);

    // reveal before share (A), reveal before commit (N).
    let (mut a, mut n, _) = advance(&w, &r, 0);
    assert_eq!(a.receive(&f[REVEAL]).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_inviter(&a);
    assert_eq!(n.receive(&f[REVEAL]).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_joiner(&mut n);

    // Two commits (N).
    let (_, mut n, _) = advance(&w, &r, 1);
    assert_eq!(n.receive(&f[COMMIT]).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_joiner(&mut n);

    // joiner_confirm before joiner_hello (A).
    let (mut a, _, _) = advance(&w, &r, 2);
    assert_eq!(
        a.receive(&f[CONFIRM]).unwrap_err(),
        CoreError::PairingFailed
    );
    assert_wiped_inviter(&a);

    // provision before joiner_confirm (N, while the user compares the codes).
    let (_, mut n, _) = advance(&w, &r, 3);
    assert_eq!(n.phase(), JoinerPhase::AwaitUserConfirm);
    assert_eq!(
        n.receive(&f[PROVISION]).unwrap_err(),
        CoreError::PairingFailed
    );
    assert_wiped_joiner(&mut n);

    // joiner_ack before the provision was sent (A, still waiting for the confirmations).
    let (mut a, _, _) = advance(&w, &r, 5);
    assert_eq!(a.receive(&f[ACK]).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_inviter(&a);

    // A second joiner_confirm, a second hello, a second share (A).
    let (mut a, _, _) = advance(&w, &r, 7);
    assert_eq!(
        a.receive(&f[CONFIRM]).unwrap_err(),
        CoreError::PairingFailed
    );
    assert_wiped_inviter(&a);
    let (mut a, _, _) = advance(&w, &r, 4);
    assert_eq!(a.receive(&f[HELLO]).unwrap_err(), CoreError::PairingFailed);
    let (mut a, _, _) = advance(&w, &r, 2);
    assert_eq!(a.receive(&f[SHARE]).unwrap_err(), CoreError::PairingFailed);

    // A second reveal (N, waiting for the provision).
    let (_, mut n, _) = advance(&w, &r, 6);
    assert_eq!(n.phase(), JoinerPhase::AwaitProvision);
    assert_eq!(n.receive(&f[REVEAL]).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_joiner(&mut n);
}

#[test]
fn p13_a_frame_replayed_to_its_own_sender_fails() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    // A receives frames that only A writes, in every state where it waits.
    for until in [0, 2, 4, 8] {
        for index in [COMMIT, REVEAL, PROVISION] {
            let (mut a, _, _) = advance(&w, &r, until);
            assert_eq!(
                a.receive(&f[index]).unwrap_err(),
                CoreError::PairingFailed,
                "A, step {until}, own frame {index}"
            );
            assert_wiped_inviter(&a);
        }
    }
    // N receives frames that only N writes.
    for until in [0, 1, 6] {
        for index in [SHARE, HELLO, CONFIRM, ACK] {
            let (_, mut n, _) = advance(&w, &r, until);
            assert_eq!(
                n.receive(&f[index]).unwrap_err(),
                CoreError::PairingFailed,
                "N, step {until}, own frame {index}"
            );
            assert_wiped_joiner(&mut n);
        }
    }
}

#[test]
fn p13_frames_after_the_last_expected_one_are_ignored() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    // A after joiner_ack.
    let (mut a, _, _) = advance(&w, &r, 10);
    assert_eq!(a.phase(), InviterPhase::Done);
    for junk in [
        f[ACK].clone(),
        f[COMMIT].clone(),
        vec![],
        vec![9; 9000],
        vec![1],
    ] {
        let step = a.receive(&junk).unwrap();
        assert_eq!(step.event, InviterEvent::Ignored);
        assert!(step.send.is_none());
    }
    assert_eq!(a.phase(), InviterPhase::Done);
    assert!(a.finish().is_ok(), "the result survives the ignored frames");
    // N after provision.
    let (_, mut n, _) = advance(&w, &r, 9);
    assert_eq!(n.phase(), JoinerPhase::Done);
    for junk in [
        f[PROVISION].clone(),
        f[REVEAL].clone(),
        vec![],
        vec![9; 9000],
    ] {
        let step = n.receive(&junk).unwrap();
        assert_eq!(step.event, JoinerEvent::Ignored);
        assert!(step.send.is_none());
    }
    assert!(n.take_provision().is_ok());
}

// ---------------------------------------------------------------------------
// P14: replay from another session
// ---------------------------------------------------------------------------

#[test]
fn p14_a_frame_of_another_session_fails() {
    let w = world();
    let r1 = rand();
    let mut r2 = rand();
    r2.sid = std::array::from_fn(|i| 0x80 + i as u8);
    r2.ya = hx32("0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f00");
    r2.yb = hx32("a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf00");
    let session_1 = pristine(&w, &r1);
    let session_2 = pristine(&w, &r2);
    assert_ne!(session_1, session_2);
    // Every frame of session 1 put at its place in session 2 breaks session 2 (same code, same
    // channel). The five sealed frames fail at once; the two clear ones (commit, share) can not be
    // told apart from the real ones, so the key agreement fails further on.
    for index in 0..7 {
        let (run, ..) = run_with(&w, &r2, Some(OFFER), true, r2.code(), &mut |i, f| {
            if i == index {
                *f = session_1[index].clone();
            }
        });
        assert_eq!(
            run.result.clone().unwrap_err(),
            CoreError::PairingFailed,
            "frame {index} ({:?}) of session 1 replayed in session 2",
            KINDS[index]
        );
        assert!(index >= PROVISION || no_provision_was_sent(&run));
    }
}

#[test]
fn p14_the_session_id_is_bound_in_the_aad_even_with_the_same_keys() {
    // Same key, same counter, same type, another sid: the box does not open.
    let r = rand();
    let (keys, _) = session_keys(&r, r.ranks);
    let boxed = frame::seal_body(&keys, FrameKind::JoinerHello, &r.sid, b"x").unwrap();
    assert!(frame::open_body(&keys, FrameKind::JoinerHello, &r.sid, &boxed).is_ok());
    let mut other = r.sid;
    other[15] ^= 1;
    assert_eq!(
        frame::open_body(&keys, FrameKind::JoinerHello, &other, &boxed).unwrap_err(),
        CoreError::PairingFailed
    );
}

// ---------------------------------------------------------------------------
// P15 at the machine level: what a sealed body may contain
// ---------------------------------------------------------------------------

fn raw_tlv(parts: &[(u8, &[u8])]) -> Vec<u8> {
    let mut v = Vec::new();
    for (t, val) in parts {
        v.push(*t);
        v.extend_from_slice(&(val.len() as u16).to_be_bytes());
        v.extend_from_slice(val);
    }
    v
}

#[test]
fn p15_unknown_tag_is_ignored_and_a_critical_one_is_unsupported_inside_a_real_session() {
    let w = world();
    let r = rand();
    let (keys, _) = session_keys(&r, r.ranks);
    let (ed, x) = pk(&w.a.public());
    let genuine = pristine(&w, &r);
    let ya = genuine[REVEAL][2..34].to_vec();
    let reveal_with = |extra: Option<(u8, &[u8])>| {
        let mut parts: Vec<(u8, &[u8])> =
            vec![(1, &ed), (2, &x), (3, b"PC-Bureau"), (4, b"windows")];
        parts.extend(extra);
        let boxed = frame::seal_body(&keys, FrameKind::Reveal, &r.sid, &raw_tlv(&parts)).unwrap();
        let mut payload = ya.clone();
        payload.extend_from_slice(&boxed);
        frame::encode(FrameKind::Reveal, &payload).unwrap()
    };
    // A tag 0x40 from the future is ignored.
    let mut n = start_joiner(&r, r.code());
    n.receive(&genuine[COMMIT]).unwrap();
    let step = n.receive(&reveal_with(Some((0x40, b"future")))).unwrap();
    assert_eq!(step.event, JoinerEvent::InviterHello);
    assert_eq!(n.inviter().unwrap().peer.name, "PC-Bureau");
    // A critical tag 0x90 makes the session unsupported, not failed.
    let mut n = start_joiner(&r, r.code());
    n.receive(&genuine[COMMIT]).unwrap();
    assert_eq!(
        n.receive(&reveal_with(Some((0x90, b"x")))).unwrap_err(),
        CoreError::PairingUnsupported
    );
    assert_eq!(n.phase(), JoinerPhase::Failed);
}

#[test]
fn p15_malformed_bodies_in_real_frames_fail() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    let (keys, _) = session_keys(&r, r.ranks);
    let (ed, x) = pk(&keys_n().public());
    let long_name = "x".repeat(65);
    let bad_hellos: Vec<Vec<u8>> = vec![
        // duplicate key, disorder, missing name, bad lengths, invalid name / platform
        raw_tlv(&[(1, &ed), (1, &ed), (2, &x), (3, b"Pixel"), (4, b"android")]),
        raw_tlv(&[(2, &x), (1, &ed), (3, b"Pixel"), (4, b"android")]),
        raw_tlv(&[(1, &ed), (2, &x), (4, b"android")]),
        raw_tlv(&[(1, &ed[..31]), (2, &x), (3, b"Pixel"), (4, b"android")]),
        raw_tlv(&[(1, &ed), (2, &x), (3, b""), (4, b"android")]),
        raw_tlv(&[
            (1, &ed),
            (2, &x),
            (3, long_name.as_bytes()),
            (4, b"android"),
        ]),
        raw_tlv(&[(1, &ed), (2, &x), (3, b"Pi\x07xel"), (4, b"android")]),
        raw_tlv(&[(1, &ed), (2, &x), (3, b"Pixel "), (4, b"android")]),
        raw_tlv(&[(1, &ed), (2, &x), (3, b"Pixel"), (4, b"Android")]),
        // a weak Ed25519 key (small order point)
        raw_tlv(&[(1, &[0u8; 32]), (2, &x), (3, b"Pixel"), (4, b"android")]),
        vec![1, 0, 32],
        vec![],
    ];
    for body in bad_hellos {
        let mut a = start_inviter(&w, &r, None);
        a.receive(&f[SHARE]).unwrap();
        let frame = sealed(&keys, FrameKind::JoinerHello, &r.sid, &body);
        assert_eq!(a.receive(&frame).unwrap_err(), CoreError::PairingFailed);
        assert!(a.peer().is_none());
    }
    // joiner_confirm: 0x00 / 0x01 only, nothing else.
    for body in [
        raw_tlv(&[(1, &[2])]),
        raw_tlv(&[(1, &[0, 0])]),
        raw_tlv(&[]),
        raw_tlv(&[(2, &[0])]),
    ] {
        let (mut a, _, _) = advance(&w, &r, 5);
        let frame = sealed(&keys, FrameKind::JoinerConfirm, &r.sid, &body);
        assert_eq!(a.receive(&frame).unwrap_err(), CoreError::PairingFailed);
    }
    // joiner_ack: the body must be empty.
    let (mut a, _, _) = advance(&w, &r, 8);
    let frame = sealed(&keys, FrameKind::JoinerAck, &r.sid, b"x");
    assert_eq!(a.receive(&frame).unwrap_err(), CoreError::PairingFailed);
}

// ---------------------------------------------------------------------------
// P16: a refused batch
// ---------------------------------------------------------------------------

/// A batch built by `A`, decoded, to be altered field by field.
struct Batch {
    body: ProvisionBody,
    w: World,
    n: DeviceKeys,
    c: DeviceKeys,
}

fn batch_fixture() -> Batch {
    let w = world();
    let n = keys_n();
    let built = batch::build(
        &w.trust_a,
        &w.a,
        &w.space_key,
        &n.public(),
        NOW_MS,
        None,
        &mut Entropy::os(),
    )
    .unwrap();
    Batch {
        body: ProvisionBody::decode(&built.body).unwrap(),
        w,
        n,
        c: dev(0xe0, 0x50),
    }
}

impl Batch {
    fn verify(&self, body: &ProvisionBody) -> Result<(), CoreError> {
        batch::verify(&self.n, &self.w.a.public(), body).map(|_| ())
    }

    /// A member list signed by `issuer` with the given content.
    fn list(
        &self,
        issuer: &DeviceKeys,
        space_id: &str,
        key_epoch: u32,
        members: &[DevicePublic],
    ) -> Vec<u8> {
        let mut members = members.to_vec();
        members.sort_by(|a, b| a.ed25519.cmp(&b.ed25519));
        let mut list = MemberList {
            space_id: space_id.to_owned(),
            key_epoch,
            issued_at_ms: NOW_MS,
            issuer_ed25519: issuer.public().ed25519,
            members,
            signature: Vec::new(),
        };
        list.signature = issuer.sign(&list.signed_bytes().unwrap());
        list.to_bytes().unwrap()
    }

    fn everyone(&self) -> Vec<DevicePublic> {
        vec![self.w.a.public(), self.c.public(), self.n.public()]
    }
}

#[test]
fn p16_the_unaltered_batch_is_accepted() {
    let b = batch_fixture();
    b.verify(&b.body).unwrap();
}

#[test]
fn p16_certificate_checks() {
    let b = batch_fixture();
    let space = b.w.space_id.clone();
    let other_space = new_space_id().unwrap();
    let cases: Vec<(&str, DeviceCert)> = vec![
        (
            "subject is not N",
            b.w.a
                .sign_device_cert(&space, &b.c.public(), NOW_MS)
                .unwrap(),
        ),
        (
            "issuer is not A",
            b.c.sign_device_cert(&space, &b.n.public(), NOW_MS).unwrap(),
        ),
        (
            "self-signed by N",
            b.n.sign_device_cert(&space, &b.n.public(), NOW_MS).unwrap(),
        ),
        (
            "another space",
            b.w.a
                .sign_device_cert(&other_space, &b.n.public(), NOW_MS)
                .unwrap(),
        ),
    ];
    for (name, cert) in cases {
        let mut body = b.body.clone();
        body.device_cert = cert.to_bytes().unwrap();
        assert_eq!(
            b.verify(&body).unwrap_err().code(),
            "pairing_failed",
            "{name}"
        );
    }
    // A signature that does not verify.
    let mut body = b.body.clone();
    let last = body.device_cert.len() - 1;
    body.device_cert[last] ^= 1;
    assert_eq!(b.verify(&body).unwrap_err().code(), "invalid_signature");
    // Not a certificate at all.
    let mut body = b.body.clone();
    body.device_cert = vec![0; tlv::DEVICE_CERT_LEN];
    assert_eq!(b.verify(&body).unwrap_err().code(), "malformed_statement");
}

#[test]
fn p16_member_list_checks() {
    let b = batch_fixture();
    let space = b.w.space_id.clone();
    let everyone = b.everyone();
    let check = |name: &str, list: Vec<u8>, code: &str| {
        let mut body = b.body.clone();
        body.member_list = list;
        assert_eq!(b.verify(&body).unwrap_err().code(), code, "{name}");
    };
    check(
        "signed by another member",
        b.list(&b.c, &space, 1, &everyone),
        "pairing_failed",
    );
    check(
        "another epoch",
        b.list(&b.w.a, &space, 2, &everyone),
        "pairing_failed",
    );
    check(
        "without A",
        b.list(&b.w.a, &space, 1, &[b.c.public(), b.n.public()]),
        "pairing_failed",
    );
    check(
        "another space",
        b.list(&b.w.a, &new_space_id().unwrap(), 1, &everyone),
        "pairing_failed",
    );
    // A listed with another X25519 key than in InviterHello: key_conflict.
    let mut forged_a = b.w.a.public();
    forged_a.x25519 = dev(0x01, 0x02).public().x25519;
    check(
        "A with another x25519 key",
        b.list(&b.w.a, &space, 1, &[forged_a, b.c.public(), b.n.public()]),
        "key_conflict",
    );
    // N listed with another X25519 key than its own: key_conflict too.
    let mut forged_n = b.n.public();
    forged_n.x25519 = dev(0x01, 0x02).public().x25519;
    check(
        "N with another x25519 key",
        b.list(&b.w.a, &space, 1, &[b.w.a.public(), b.c.public(), forged_n]),
        "key_conflict",
    );
    // Right content, signature broken.
    let mut broken = b.body.member_list.clone();
    let last = broken.len() - 1;
    broken[last] ^= 1;
    check("signature", broken, "invalid_signature");
    // A reserved epoch is refused when the list is decoded.
    check(
        "reserved epoch",
        b.list(&b.w.a, &space, 0x8000_0001, &everyone),
        "malformed_statement",
    );
}

#[test]
fn p16_sealed_key_checks() {
    let b = batch_fixture();
    let space = b.w.space_id.clone();
    let other_space = new_space_id().unwrap();
    let key = &b.w.space_key;
    let key2 = SpaceKey::from_bytes(2, &seq(0, 32)).unwrap();
    let check = |name: &str, sealed: Vec<u8>, epoch: u32| {
        let mut body = b.body.clone();
        body.sealed = sealed;
        body.key_epoch = epoch;
        let code = if epoch == 1 {
            "decryption_failed"
        } else {
            // The list is for epoch 1: caught before the key is opened.
            "pairing_failed"
        };
        assert_eq!(b.verify(&body).unwrap_err().code(), code, "{name}");
    };
    check(
        "sealed for another device",
        crate::seal_space_key(&b.c.public(), &space, key).unwrap(),
        1,
    );
    check(
        "sealed for another space",
        crate::seal_space_key(&b.n.public(), &other_space, key).unwrap(),
        1,
    );
    check(
        "sealed for another epoch",
        crate::seal_space_key(&b.n.public(), &space, &key2).unwrap(),
        1,
    );
    check(
        "epoch says 2, the list says 1",
        crate::seal_space_key(&b.n.public(), &space, &key2).unwrap(),
        2,
    );
    let mut altered = b.body.sealed.clone();
    altered[60] ^= 1;
    check("altered ciphertext", altered, 1);
}

#[test]
fn p16_space_id_and_epoch_fields_must_agree() {
    let b = batch_fixture();
    let mut body = b.body.clone();
    body.space_id = parse_uuid(&new_space_id().unwrap(), "space").unwrap();
    assert_eq!(b.verify(&body).unwrap_err().code(), "pairing_failed");
    // A reserved or null epoch in the body: refused when the body is decoded.
    let mut raw = b.body.encode().unwrap();
    // Tag 0x02 (key_epoch) follows the 19-byte space_id TLV; its value starts 3 bytes later.
    assert_eq!(&raw[19..22], &[0x02, 0x00, 0x04]);
    raw[22..26].copy_from_slice(&0x8000_0001u32.to_be_bytes());
    assert_eq!(
        ProvisionBody::decode(&raw).unwrap_err(),
        CoreError::PairingFailed
    );
    raw[22..26].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(
        ProvisionBody::decode(&raw).unwrap_err(),
        CoreError::PairingFailed
    );
}

#[test]
fn p16_a_refused_batch_fails_the_session_and_keeps_nothing() {
    // Through the whole machine: A is replaced, from the provision frame on, by a peer that
    // knows the session keys and sends a batch about someone else.
    let w = world();
    let r = rand();
    let b = batch_fixture();
    let (keys, _) = session_keys(&r, r.ranks);
    let c = dev(0xe0, 0x50);
    let mut evil = b.body.clone();
    evil.device_cert = c
        .sign_device_cert(&w.space_id, &keys_n().public(), NOW_MS)
        .unwrap()
        .to_bytes()
        .unwrap();
    let frame = sealed(&keys, FrameKind::Provision, &r.sid, &evil.encode().unwrap());
    let (_, mut n, _) = advance(&w, &r, 6);
    assert_eq!(n.receive(&frame).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_joiner(&mut n);
    // Control: the same construction with the genuine batch is accepted, so the refusal above
    // is about the certificate and nothing else.
    let good = sealed(
        &keys,
        FrameKind::Provision,
        &r.sid,
        &b.body.encode().unwrap(),
    );
    let (_, mut n, _) = advance(&w, &r, 6);
    let step = n.receive(&good).unwrap();
    assert_eq!(step.event, JoinerEvent::ProvisionVerified);
}

// ---------------------------------------------------------------------------
// The user decisions
// ---------------------------------------------------------------------------

#[test]
fn the_space_key_is_never_sent_before_both_users_confirmed() {
    let w = world();
    let r = rand();
    let provision = |a: &mut Inviter| a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS);
    // Nothing yet.
    let (mut a, _, f) = advance(&w, &r, 3);
    assert!(!a.ready_to_provision());
    assert_eq!(provision(&mut a).unwrap_err().code(), "invalid_input");
    assert_eq!(
        a.confirm().unwrap_err().code(),
        "invalid_input",
        "no hello yet"
    );
    // The hello arrived; neither user confirmed.
    a.receive(&f[HELLO]).unwrap();
    assert_eq!(a.phase(), InviterPhase::AwaitConfirmations);
    assert!(!a.ready_to_provision());
    assert_eq!(provision(&mut a).unwrap_err().code(), "invalid_input");
    // Only the user of N confirmed (joiner_confirm received): A still refuses.
    let (_, mut n, _) = advance(&w, &r, 3);
    let confirm = n.confirm(true).unwrap();
    a.receive(&confirm).unwrap();
    assert!(!a.ready_to_provision());
    assert_eq!(provision(&mut a).unwrap_err().code(), "invalid_input");
    // Then the user of A confirms: now it can, and the refusals did not break the session.
    a.confirm().unwrap();
    a.confirm().unwrap();
    assert!(a.ready_to_provision());
    assert!(provision(&mut a).is_ok());
    assert_eq!(a.phase(), InviterPhase::AwaitAck);
    assert!(!a.ready_to_provision());
    assert_eq!(
        provision(&mut a).unwrap_err().code(),
        "invalid_input",
        "only once"
    );
    // The other order: the user of A first, then joiner_confirm.
    let (mut a, _, _) = advance(&w, &r, 5);
    assert!(!a.ready_to_provision());
    a.receive(&confirm).unwrap();
    assert!(a.ready_to_provision());
}

#[test]
fn provision_checks_the_keys_and_the_epoch_it_is_given() {
    let w = world();
    let r = rand();
    let (mut a, _, _) = advance(&w, &r, 7);
    let stranger = DeviceKeys::generate().unwrap();
    assert_eq!(
        a.provision(&stranger, &w.trust_a, &w.space_key, NOW_MS)
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    let key2 = SpaceKey::from_bytes(2, &seq(0, 32)).unwrap();
    assert_eq!(
        a.provision(&w.a, &w.trust_a, &key2, NOW_MS)
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    // A trust store of another space is refused too.
    let other = TrustStore::new("00000000-0000-4000-8000-000000000000", &w.a.public(), 1).unwrap();
    assert_eq!(
        a.provision(&w.a, &other, &w.space_key, NOW_MS)
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    // The refusals left the session usable.
    assert!(a.ready_to_provision());
    a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS).unwrap();
}

#[test]
fn history_is_announced_only_if_offered_and_requested() {
    let w = world();
    let r = rand();
    for (offer, import, announced) in [
        (Some(OFFER), true, true),
        (Some(OFFER), false, false),
        (None, true, false),
        (None, false, false),
        // An offer of zero items is no offer: nothing is shown, nothing is announced.
        (Some(HistoryOffer { items: 0, bytes: 0 }), true, false),
    ] {
        let (run, mut a, mut n) = run_with(&w, &r, offer, import, r.code(), &mut |_, _| {});
        run.result.unwrap();
        let with_tag = run.produced[PROVISION].len() == 552 + 64 * 3;
        assert_eq!(with_tag, announced, "{offer:?} {import}");
        assert_eq!(a.finish().unwrap().history.is_some(), announced);
        assert_eq!(n.take_provision().unwrap().history.is_some(), announced);
        // joiner_confirm has a constant size, whatever the answer.
        assert_eq!(run.produced[CONFIRM].len(), 22);
    }
}

#[test]
fn n_answers_no_when_nothing_was_offered_even_if_the_ui_says_yes() {
    let w = world();
    let r = rand();
    let (keys, _) = session_keys(&r, r.ranks);
    let history_flag = |confirm: &[u8]| {
        let body =
            frame::open_body(&keys, FrameKind::JoinerConfirm, &r.sid, &confirm[2..]).unwrap();
        JoinerConfirm::decode(&body).unwrap().history
    };
    // With an offer, the answer is what the user chose.
    let (_, mut n, _) = advance(&w, &r, 3);
    assert!(history_flag(&n.confirm(true).unwrap()));
    let (_, mut n, _) = advance(&w, &r, 3);
    assert!(!history_flag(&n.confirm(false).unwrap()));
    // Without an offer, "yes" is sent as "no".
    let mut a = start_inviter(&w, &r, None);
    let mut n = start_joiner(&r, r.code());
    let share = n.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal = a.receive(&share).unwrap().send.unwrap();
    n.receive(&reveal).unwrap();
    assert!(!history_flag(&n.confirm(true).unwrap()));
}

#[test]
fn a_device_already_in_the_space_or_removed_from_it_is_refused_at_the_hello() {
    let w = world();
    let r = rand();
    // C is a member: a device with C's keys can not join again.
    let mut a = start_inviter(&w, &r, None);
    let mut n =
        kat::joiner_new(joiner_params(r.code()), dev(0xe0, 0x50), r.joiner_script()).unwrap();
    let share = n.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal = a.receive(&share).unwrap().send.unwrap();
    let hello = n.receive(&reveal).unwrap().send.unwrap();
    assert_eq!(a.receive(&hello).unwrap_err(), CoreError::PairingFailed);
    assert_wiped_inviter(&a);
    // A device that was revoked can not come back with the same keys.
    let c = dev(0xe0, 0x50);
    let mut trust = w.trust_a.clone();
    let bundle = trust
        .create_rotation(&w.a, &[c.public().ed25519], NOW_MS)
        .unwrap();
    trust.apply_rotation(&bundle.rotation, &w.a).unwrap();
    assert!(trust.is_revoked(&c.public().ed25519));
    let mut a = kat::inviter_start(inviter_params(None), &w.a, &trust, r.inviter_script()).unwrap();
    let mut n =
        kat::joiner_new(joiner_params(r.code()), dev(0xe0, 0x50), r.joiner_script()).unwrap();
    let share = n.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal = a.receive(&share).unwrap().send.unwrap();
    let hello = n.receive(&reveal).unwrap().send.unwrap();
    assert_eq!(a.receive(&hello).unwrap_err(), CoreError::PairingFailed);
    // A device can not be paired with itself.
    let mut a = start_inviter(&w, &r, None);
    let mut n =
        kat::joiner_new(joiner_params(r.code()), dev(0xa0, 0x10), r.joiner_script()).unwrap();
    let share = n.receive(a.commit_frame()).unwrap().send.unwrap();
    let reveal = a.receive(&share).unwrap().send.unwrap();
    assert_eq!(n.receive(&reveal).unwrap_err(), CoreError::PairingFailed);
}

#[test]
fn cancel_wipes_and_later_calls_report_the_cancellation() {
    let w = world();
    let r = rand();
    let f = pristine(&w, &r);
    let (mut a, mut n, _) = advance(&w, &r, 4);
    assert!(a.sas().is_some() && a.peer().is_some() && a.code().is_some());
    a.cancel();
    assert_eq!(a.phase(), InviterPhase::Cancelled);
    assert!(a.sas().is_none() && a.peer().is_none() && a.code().is_none());
    assert_eq!(
        a.receive(&f[CONFIRM]).unwrap_err(),
        CoreError::PairingCancelled
    );
    assert_eq!(a.phase(), InviterPhase::Cancelled);
    assert_eq!(a.confirm().unwrap_err().code(), "invalid_input");
    assert_eq!(
        a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS)
            .unwrap_err()
            .code(),
        "invalid_input"
    );
    assert_eq!(a.finish().unwrap_err().code(), "invalid_input");
    // N, with the codes on screen, refuses.
    assert!(n.sas().is_some());
    n.cancel();
    assert_eq!(n.phase(), JoinerPhase::Cancelled);
    assert!(n.sas().is_none() && n.inviter().is_none());
    assert_eq!(n.confirm(true).unwrap_err().code(), "invalid_input");
    assert_eq!(
        n.receive(&f[PROVISION]).unwrap_err(),
        CoreError::PairingCancelled
    );
    assert_eq!(n.take_provision().unwrap_err().code(), "invalid_input");
}

#[test]
fn parameters_are_checked_before_anything_is_drawn() {
    let w = world();
    let r = rand();
    let bad: Vec<InviterParams> = vec![
        InviterParams {
            relay_authority: "Relay.Example.Org".into(),
            ..inviter_params(None)
        },
        InviterParams {
            channel_id: "0".into(),
            ..inviter_params(None)
        },
        InviterParams {
            device_name: " ".into(),
            ..inviter_params(None)
        },
        InviterParams {
            platform: "Windows".into(),
            ..inviter_params(None)
        },
    ];
    for p in bad {
        let e = kat::inviter_start(p, &w.a, &w.trust_a, r.inviter_script())
            .err()
            .unwrap();
        assert_eq!(e.code(), "invalid_input");
    }
    // A device that is not a member can not invite.
    let stranger = DeviceKeys::generate().unwrap();
    let e = kat::inviter_start(
        inviter_params(None),
        &stranger,
        &w.trust_a,
        r.inviter_script(),
    )
    .err()
    .unwrap();
    assert_eq!(e.code(), "invalid_input");
    let e = kat::joiner_new(
        JoinerParams {
            device_name: String::new(),
            ..joiner_params(r.code())
        },
        keys_n(),
        r.joiner_script(),
    )
    .err()
    .unwrap();
    assert_eq!(e.code(), "invalid_input");
    // A script that runs dry is an error, never silent OS randomness.
    let e = kat::inviter_start(inviter_params(None), &w.a, &w.trust_a, vec![vec![0; 2]])
        .err()
        .unwrap();
    assert_eq!(e.code(), "internal");
}

// ---------------------------------------------------------------------------
// P17: no secret in anything that can be logged
// ---------------------------------------------------------------------------

/// Everything a log line could contain: the `Debug` and `Display` of every value the API hands
/// out during a session, and of the machines themselves.
struct LogCapture(String);

impl LogCapture {
    fn new() -> Self {
        Self(String::new())
    }

    fn note(&mut self, v: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{v:?}\n{v:#?}\n"));
    }

    fn note_err(&mut self, e: &CoreError) {
        self.0
            .push_str(&format!("{e}\n{e:?}\n{e:#?}\n{}\n", e.code()));
    }
}

/// The secrets of the session of `rand()` with the code `4821-crayon-renard`, as the strings a
/// log could contain: hexadecimal, `Debug` list of bytes, the digits and the words.
fn secrets(r: &Rand) -> Vec<(String, String)> {
    let (keys, sas) = session_keys(r, r.ranks);
    let ci = session::channel_identifier(AUTHORITY, CHANNEL);
    let trace = kat::cpace_trace(
        r.code().prs().as_slice(),
        &ci,
        &r.sid,
        &session::ad_inviter(),
        &session::ad_joiner(),
        &r.ya,
        &r.yb,
    )
    .unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, bytes: &[u8]| {
        let debug = format!("{bytes:?}");
        out.push((format!("{name} (hex)"), hex::encode(bytes)));
        out.push((
            format!("{name} (list)"),
            debug.trim_matches(|c| c == '[' || c == ']').to_owned(),
        ));
        out.push((format!("{name} (debug)"), debug));
    };
    add("isk", &trace.isk_initiator);
    add("k", &trace.k_initiator);
    add("K_I", &*keys.inviter);
    add("K_J", &*keys.joiner);
    add("ya", &r.ya);
    add("yb", &r.yb);
    out.push(("sas digits".into(), sas.digits()));
    out.push(("sas grouped".into(), sas.grouped()));
    out.push(("word 1".into(), "crayon".into()));
    out.push(("word 2".into(), "renard".into()));
    out.push(("code".into(), "crayon-renard".into()));
    out.push((
        "pairing token".into(),
        "tmp1.uFJcfepRayMOiriiRhh-eL7yHvoUg3Ej".into(),
    ));
    out
}

fn assert_clean(capture: &LogCapture, secrets: &[(String, String)]) {
    for (name, value) in secrets {
        assert!(
            !capture.0.contains(value.as_str()),
            "{name} appears in the captured logs: {value}"
        );
    }
}

#[test]
fn p17_no_secret_in_the_debug_or_display_of_a_complete_session() {
    let w = world();
    let r = rand();
    let mut log = LogCapture::new();
    let mut a = start_inviter(&w, &r, Some(OFFER));
    let mut n = start_joiner(&r, r.code());
    log.note(&a);
    log.note(&n);
    log.note(a.code().unwrap());
    let share = n.receive(a.commit_frame()).unwrap();
    log.note(&share);
    let reveal = a.receive(share.send.as_ref().unwrap()).unwrap();
    log.note(&reveal);
    let hello = n.receive(reveal.send.as_ref().unwrap()).unwrap();
    log.note(&hello);
    log.note(&a.sas());
    log.note(&n.sas());
    log.note(&n.inviter());
    let peer = a.receive(hello.send.as_ref().unwrap()).unwrap();
    log.note(&peer);
    log.note(&a.peer());
    a.confirm().unwrap();
    let confirm = n.confirm(true).unwrap();
    log.note(&confirm);
    log.note(&a.receive(&confirm).unwrap());
    let provision = a.provision(&w.a, &w.trust_a, &w.space_key, NOW_MS).unwrap();
    log.note(&provision);
    let ack = n.receive(&provision).unwrap();
    log.note(&ack);
    log.note(&a.receive(ack.send.as_ref().unwrap()).unwrap());
    log.note(&a);
    log.note(&n);
    let outcome = a.finish().unwrap();
    log.note(&outcome);
    let p = n.take_provision().unwrap();
    log.note(&p);
    log.note(&p.trust);
    log.note(&p.space_key);
    log.note(&p.keys);
    // The batch holds the space key: its debug output never shows it.
    assert!(!log.0.contains(&hex::encode(w.space_key.to_bytes())));
    assert!(!log.0.contains(&format!("{:?}", w.space_key.to_bytes())));
    assert_clean(&log, &secrets(&r));
    // The check can see a leak: the code and the verification code are in what the UI shows.
    let shown = format!(
        "{} {}",
        r.code().to_code_string(),
        session_keys(&r, r.ranks).1.grouped()
    );
    let mut leaky = LogCapture::new();
    leaky.0.push_str(&shown);
    assert!(secrets(&r)
        .iter()
        .any(|(_, v)| leaky.0.contains(v.as_str())));
}

#[test]
fn p17_no_secret_in_the_logs_of_a_failed_session() {
    let w = world();
    let r = rand();
    let mut log = LogCapture::new();
    let wrong = PairingCode::from_parts(CHANNEL, [465, 1634]).unwrap();
    let (run, a, n) = run_with(&w, &r, Some(OFFER), true, wrong, &mut |_, _| {});
    let e = run.result.clone().unwrap_err();
    log.note_err(&e);
    log.note(&a);
    log.note(&n);
    log.note(&run.produced);
    // Failing machines again, with the errors of every kind of failure.
    let f = pristine(&w, &r);
    for (until, index) in [(0, REVEAL), (1, COMMIT), (3, PROVISION), (7, CONFIRM)] {
        let (mut a, mut n, _) = advance(&w, &r, until);
        if let Err(e) = a.receive(&f[index]) {
            log.note_err(&e);
        }
        if let Err(e) = n.receive(&f[index]) {
            log.note_err(&e);
        }
        log.note(&a);
        log.note(&n);
        log.note(&a.sas());
        log.note(&n.sas());
    }
    // The error of an invalid typed code or link does not echo it either.
    let e = PairingCode::parse("4821-crayon-licorn-renard").unwrap_err();
    log.note_err(&e);
    let e = PairingCode::from_link("https://evil.example/l/pair#4821-crayon-renard", AUTHORITY)
        .unwrap_err();
    log.note_err(&e);
    assert!(!log.0.contains("crayon") && !log.0.contains("renard") && !log.0.contains("licorn"));
    assert_clean(&log, &secrets(&r));
}

#[test]
fn p17_no_logging_macro_in_the_pairing_sources() {
    // The pure layer has no logger; this keeps it so: a future `println!` of a secret would
    // have to be added to this list on purpose.
    let sources = [
        include_str!("batch.rs"),
        include_str!("code.rs"),
        include_str!("cpace.rs"),
        include_str!("entropy.rs"),
        include_str!("frame.rs"),
        include_str!("info.rs"),
        include_str!("inviter.rs"),
        include_str!("joiner.rs"),
        include_str!("kat.rs"),
        include_str!("mod.rs"),
        include_str!("session.rs"),
        include_str!("tlv.rs"),
        include_str!("wordlist.rs"),
    ];
    for src in sources {
        for forbidden in [
            "println!",
            "eprintln!",
            "print!(",
            "eprint!(",
            "dbg!",
            "tracing::",
            "log::",
        ] {
            assert!(!src.contains(forbidden), "{forbidden} in a pairing source");
        }
    }
}

#[test]
fn the_cpace_layer_does_not_expose_k() {
    // `Share::finish` returns the session key only; `shared_point` (K) is `pub(super)` for the
    // known-answer hooks and used by nothing else. Checked on the sources of the machines.
    for src in [
        include_str!("inviter.rs"),
        include_str!("joiner.rs"),
        include_str!("session.rs"),
    ] {
        assert!(!src.contains("shared_point"), "K must stay inside cpace.rs");
    }
}

// ---------------------------------------------------------------------------
// §11.5: the error codes added to the core
// ---------------------------------------------------------------------------

#[test]
fn pairing_error_codes_are_stable_and_say_nothing_about_the_session() {
    for (error, code) in [
        (CoreError::PairingFailed, "pairing_failed"),
        (CoreError::PairingExpired, "pairing_expired"),
        (CoreError::PairingTaken, "pairing_taken"),
        (CoreError::PairingCancelled, "pairing_cancelled"),
        (CoreError::PairingUnsupported, "pairing_unsupported"),
    ] {
        assert_eq!(error.code(), code);
        // A plain message: no code, no verification code, no frame byte.
        let message = error.to_string();
        assert!(message.starts_with("pairing"), "{message}");
        assert!(!message.chars().any(|c| c.is_ascii_digit()), "{message}");
    }
}
