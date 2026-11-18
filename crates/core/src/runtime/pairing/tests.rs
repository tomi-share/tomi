//! Tests of the pairing runtime against the in-memory relay: the end-to-end cases P18 to P30 of
//! `spec/pairing-v1.md` §14.2, and the double itself (the checks of `api-v1.md` §11.10, A100 to
//! A107, which the lot 3 will run against the real relay with P31).
//!
//! Both devices are interleaved on one thread with `poll(0)`: every test is deterministic and
//! independent of timing, and a protocol that gets stuck panics instead of hanging. The blocking
//! calls (`wait_for_peer`, `wait_done`...) are run on two threads in
//! `blocking_calls_on_two_threads`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::pending::Stored;
use super::*;
use crate::pairing::{self, Joiner, JoinerParams, PairingCode};
use crate::runtime::mock::{MockRelay, MockTransport, TestClock};
use crate::runtime::transport::*;
use crate::runtime::{
    DeviceState, DispatchOutcome, DraftAttachment, DraftText, EngineConfig, ItemContentView,
    ItemOrigin, OutgoingDraft, Store,
};
use crate::{DeviceKeys, ReadPolicy, SourceFormat};

const T0: i64 = 1_793_836_800_000;
const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";
const WRAP: [u8; 32] = [9u8; 32];

// ---------------------------------------------------------------------------
// The world: one relay, one clock, devices that each have their own store
// ---------------------------------------------------------------------------

struct Dev {
    engine: Arc<Engine>,
    dir: tempfile::TempDir,
    cfg: EngineConfig,
}

struct World {
    relay: MockRelay,
    clock: Arc<TestClock>,
}

impl World {
    /// A relay that was just installed: no space yet, and the install token of `TOKEN`.
    fn new() -> Self {
        let clock = TestClock::new(T0);
        let relay = MockRelay::uninitialized(clock.clone(), TOKEN);
        Self { relay, clock }
    }

    fn cfg(platform: &str) -> EngineConfig {
        EngineConfig {
            platform: platform.into(),
            ..EngineConfig::default()
        }
    }

    /// A device that is not provisioned.
    fn device(&self, platform: &str) -> Dev {
        self.device_with(Self::cfg(platform))
    }

    fn device_with(&self, cfg: EngineConfig) -> Dev {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(
            &dir.path().join("tomi.db"),
            &dir.path().join("files"),
            &WRAP,
        )
        .unwrap();
        let engine = Engine::open(
            cfg.clone(),
            store,
            self.relay.transport(),
            self.clock.clone(),
        )
        .unwrap();
        Dev {
            engine: Arc::new(engine),
            dir,
            cfg,
        }
    }

    /// The first device of a space.
    fn founder(&self, name: &str) -> Dev {
        let d = self.device("windows");
        d.engine.create_space(TOKEN, name).unwrap();
        d
    }

    /// Pairs `n` through `a`, the two users comparing the codes. Both devices are driven on this
    /// thread.
    fn pair(&self, a: &Dev, n: &Dev, name: &str, history: bool) -> (InviteOutcome, JoinOutcome) {
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.code().unwrap(), name).unwrap();
        compare_and_confirm(&inv, &js, history);
        run(&inv, &js, |a, n| {
            a == InvitePhase::Done && n == JoinPhase::Done
        });
        (inv.wait_done().unwrap(), js.wait_done().unwrap())
    }
}

impl Dev {
    /// The app stops and starts again on the same files.
    fn restart(self, w: &World) -> Dev {
        let Dev { engine, dir, cfg } = self;
        drop(engine);
        let store = Store::open(
            &dir.path().join("tomi.db"),
            &dir.path().join("files"),
            &WRAP,
        )
        .unwrap();
        let engine =
            Engine::open(cfg.clone(), store, w.relay.transport(), w.clock.clone()).unwrap();
        Dev {
            engine: Arc::new(engine),
            dir,
            cfg,
        }
    }

    fn id(&self) -> String {
        self.engine.device_id().unwrap()
    }

    fn trust_json(&self) -> String {
        self.engine.read_crypto(|c, _| c.trust.to_json()).unwrap()
    }

    fn knows(&self, other: &Dev) -> bool {
        let ed = other.engine.active().unwrap().keys.public().ed25519;
        self.knows_key(&ed)
    }

    fn knows_key(&self, ed25519: &[u8]) -> bool {
        self.engine
            .read_crypto(|c, _| Ok(c.trust.is_member(ed25519)))
            .unwrap()
    }

    /// The Ed25519 key and the id of a device that is not provisioned yet but holds a verified
    /// batch.
    fn pending_identity(&self) -> (Vec<u8>, String) {
        let Stored::Found(lot) = self.engine.load_lot().unwrap() else {
            panic!("no batch is sealed")
        };
        (lot.keys.public().ed25519, lot.keys.device_id())
    }
}

/// Polls both sessions, alternately, until `until` holds. Panics if it does not within a bound.
fn run(inv: &InviteSession, js: &JoinSession, until: impl Fn(InvitePhase, JoinPhase) -> bool) {
    for _ in 0..60 {
        let a = inv.poll(0).unwrap();
        let n = js.poll(0).unwrap();
        if until(a, n) {
            return;
        }
    }
    panic!(
        "the pairing did not get there: inviter {:?}, new device {:?}",
        inv.phase(),
        js.phase()
    );
}

/// Both screens show the verification code; the users compare it and press the button.
fn compare_and_confirm(inv: &InviteSession, js: &JoinSession, history: bool) {
    run(inv, js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    let (pa, pn) = (inv.prompt().unwrap(), js.prompt().unwrap());
    assert_eq!(pa.sas, pn.sas, "both screens show the same code");
    assert_eq!(pa.sas.len(), 7, "six digits in two groups of three");
    inv.confirm().unwrap();
    js.confirm(history).unwrap();
}

fn draft(plain: &str, secret: bool) -> OutgoingDraft {
    OutgoingDraft {
        id: None,
        title: Some("Titre".into()),
        text: Some(DraftText {
            plain: plain.into(),
            html: None,
            markdown: None,
            source_format: SourceFormat::Plain,
        }),
        url: None,
        attachments: vec![],
        secret,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
    }
}

fn send(d: &Dev, draft: OutgoingDraft) -> String {
    let id = d.engine.enqueue(draft).unwrap();
    assert_eq!(d.engine.dispatch(&id).unwrap(), DispatchOutcome::Sent);
    id
}

fn plain_of(engine: &Engine, id: &str) -> String {
    match engine.get_item(id).unwrap().unwrap().content {
        ItemContentView::Readable { text: Some(t), .. } => t.plain,
        other => panic!("not readable text: {other:?}"),
    }
}

/// `code` with its second word replaced by the next word of the list: a well-formed code that
/// is not the one shown.
fn with_wrong_word(code: &str) -> String {
    let mut parts: Vec<String> = code.split('-').map(str::to_owned).collect();
    let rank = pairing::rank_of(&parts[2]).unwrap();
    parts[2] = pairing::word((rank + 1) % 2048).unwrap().to_owned();
    parts.join("-")
}

/// A third party that knows the code and speaks the joiner's side of the protocol by hand,
/// outside any engine.
struct Rogue {
    tr: Arc<MockTransport>,
    channel: String,
    token: String,
    machine: Joiner,
    after: u64,
}

impl Rogue {
    fn claim(relay: &MockRelay, code: &str) -> Self {
        let parsed = PairingCode::parse(code).unwrap();
        let machine = Joiner::new(JoinerParams {
            relay_authority: "relay.example.org".into(),
            code: parsed.clone(),
            device_name: "Intrus".into(),
            platform: "android".into(),
        })
        .unwrap();
        let tr = relay.transport();
        let claim = tr.claim_pairing_channel(parsed.channel_id()).unwrap();
        Self {
            tr,
            channel: parsed.channel_id().to_owned(),
            token: claim.pairing_token,
            machine,
            after: 0,
        }
    }

    fn post(&self, frame: &[u8]) {
        self.tr
            .post_pairing_message(&self.channel, PairingAuth::Joiner(&self.token), frame)
            .unwrap();
    }

    /// Reads what the inviter deposited and answers.
    fn step(&mut self) -> Result<()> {
        let got = self
            .tr
            .pairing_messages(
                &self.channel,
                PairingAuth::Joiner(&self.token),
                self.after,
                0,
            )
            .unwrap();
        for m in got.messages {
            self.after = m.index;
            if let Some(frame) = self.machine.receive(&m.body)?.send {
                self.post(&frame);
            }
        }
        Ok(())
    }

    fn confirm(&mut self, history: bool) {
        let frame = self.machine.confirm(history).unwrap();
        self.post(&frame);
    }
}

// ---------------------------------------------------------------------------
// Files at rest
// ---------------------------------------------------------------------------

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The files of a device (database, write-ahead log, staged files) that contain `needle`.
fn holding(d: &Dev, needle: &[u8]) -> Vec<PathBuf> {
    assert!(needle.len() >= 6, "a needle that short proves nothing");
    let mut files = Vec::new();
    walk(d.dir.path(), &mut files);
    files
        .into_iter()
        .filter(|p| {
            let bytes = std::fs::read(p).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle)
        })
        .collect()
}

/// A guard that makes a hung test fail: after `secs` of real time the relay goes offline, so
/// every blocked call ends with a network error.
struct Watchdog(std::sync::mpsc::Sender<()>);

impl Watchdog {
    fn start(relay: &MockRelay, secs: u64) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let relay = relay.clone();
        std::thread::spawn(move || {
            if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
                relay.set_offline(true);
            }
        });
        Self(tx)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

// ---------------------------------------------------------------------------
// P18: nominal
// ---------------------------------------------------------------------------

#[test]
fn p18_nominal_pairing_registers_the_device_and_the_space_works() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    // A third member first, so that the batch carries a real member list.
    let c = w.device("android");
    w.pair(&a, &c, "Tablette", false);

    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Open));
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Claimed));
    compare_and_confirm(&inv, &js, false);
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    let (invited, joined) = (inv.wait_done().unwrap(), js.wait_done().unwrap());

    // The protocol wrote 3 frames on the inviter's side and 4 on the new device's (§7.1), and
    // the registration completed the channel.
    assert_eq!(w.relay.channel_messages(&channel), Some((3, 4)));
    assert_eq!(
        w.relay.channel_state(&channel),
        Some(ChannelState::Completed)
    );

    // The new device is provisioned, in the space of the inviter, with nothing left pending.
    assert!(n.engine.is_provisioned());
    assert_eq!(joined.space_id, a.engine.space_id().unwrap());
    assert_eq!(joined.device_id, n.id());
    assert!(joined.history.is_none());
    assert!(!n.engine.has_pending_pairing().unwrap());
    assert!(n.engine.awaited_history().unwrap().is_none());

    // A sees it, verified, with the name and platform it announced.
    assert_eq!(invited.device_id, n.id());
    assert_eq!(
        (invited.name.as_str(), invited.platform.as_str()),
        ("Pixel", "android")
    );
    let listed = a.engine.list_devices().unwrap();
    let pixel = listed.iter().find(|d| d.id == n.id()).unwrap();
    assert_eq!(pixel.state, DeviceState::Verified);
    assert_eq!(pixel.name, "Pixel");
    assert!(a.knows(&n));
    // The relay lists it too, with the certificate A issued.
    let registered = a.engine.transport.devices().unwrap();
    let record = registered.iter().find(|d| d.device_id == n.id()).unwrap();
    assert_eq!(record.issuer_device_id, a.id());

    // N knows A (by name), C, and itself: the three members of the space.
    let at_n = n.engine.list_devices().unwrap();
    assert_eq!(at_n.len(), 3);
    assert!(at_n.iter().all(|d| d.state == DeviceState::Verified));
    let inviter_at_n = at_n.iter().find(|d| d.id == a.id()).unwrap();
    assert_eq!(
        (inviter_at_n.name.as_str(), inviter_at_n.platform.as_str()),
        ("PC-Bureau", "windows"),
        "what the inviter announced on the authenticated channel"
    );
    assert!(n.knows(&a) && n.knows(&c));

    // N opened a session and synced: it can use the space.
    assert_eq!(w.relay.sessions_of(&n.id()), 1);
    assert_eq!(n.engine.space_status(), crate::runtime::SpaceStatus::Active);

    // C learns the new device from the log and verifies its certificate.
    c.engine.sync().unwrap();
    let at_c = c.engine.list_devices().unwrap();
    let seen = at_c.iter().find(|d| d.id == n.id()).unwrap();
    assert_eq!(seen.state, DeviceState::Verified);
    assert!(c.knows(&n));

    // An item of C is read by N, under the same space key.
    let id = send(&c, draft("de la tablette", false));
    n.engine.sync().unwrap();
    assert_eq!(plain_of(&n.engine, &id), "de la tablette");
    // And one of N reaches A.
    let back = send(&n, draft("du Pixel", false));
    a.engine.sync().unwrap();
    assert_eq!(plain_of(&a.engine, &back), "du Pixel");
}

#[test]
fn the_link_of_the_qr_code_joins_like_the_typed_code() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let (code, link) = (inv.code().unwrap(), inv.link().unwrap());
    assert_eq!(link, format!("https://relay.example.org/l/pair#{code}"));
    let js = n.engine.join_space(&link, "Pixel").unwrap();
    compare_and_confirm(&inv, &js, false);
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
}

#[test]
fn blocking_calls_on_two_threads() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let _dog = Watchdog::start(&w.relay, 60);
    // Long polls in the double wait at most 20 ms of real time, enough to interleave.
    let inv = a.engine.start_invitation().unwrap();
    let code = inv.code().unwrap();
    let (sas_a, go_a) = std::sync::mpsc::channel::<String>();
    let (sas_n, go_n) = std::sync::mpsc::channel::<String>();
    let (release_a, wait_a) = std::sync::mpsc::channel::<()>();
    let (release_n, wait_n) = std::sync::mpsc::channel::<()>();
    let (inv_ref, joiner_engine, code_ref) = (&inv, &n.engine, &code);
    std::thread::scope(|scope| {
        let inviter = scope.spawn(move || {
            let prompt = inv_ref.wait_for_peer().unwrap();
            assert_eq!(prompt.peer.name, "Pixel");
            sas_a.send(prompt.sas).unwrap();
            wait_a.recv().unwrap();
            inv_ref.confirm().unwrap();
            inv_ref.wait_done().unwrap()
        });
        let joiner = scope.spawn(move || {
            let js = joiner_engine.join_space(code_ref, "Pixel").unwrap();
            let prompt = js.wait_for_inviter().unwrap();
            assert_eq!(prompt.inviter.name, "PC-Bureau");
            assert_eq!(prompt.member_count, Some(1));
            sas_n.send(prompt.sas).unwrap();
            wait_n.recv().unwrap();
            js.confirm(false).unwrap();
            js.wait_done().unwrap()
        });
        // The users compare the codes on their screens.
        assert_eq!(go_a.recv().unwrap(), go_n.recv().unwrap());
        release_a.send(()).unwrap();
        release_n.send(()).unwrap();
        let invited = inviter.join().unwrap();
        let joined = joiner.join().unwrap();
        assert_eq!(invited.device_id, joined.device_id);
    });
    assert!(n.engine.is_provisioned());
}

// ---------------------------------------------------------------------------
// P19: wrong code
// ---------------------------------------------------------------------------

#[test]
fn p19_a_wrong_code_closes_the_channel_and_the_inviter_starts_again() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let code = inv.code().unwrap();
    let channel = inv.channel_id().to_owned();
    // The new device types a word next to the right one: a valid code, claimed with success.
    let js = n
        .engine
        .join_space(&with_wrong_word(&code), "Pixel")
        .unwrap();

    let mut failure = None;
    for _ in 0..10 {
        inv.poll(0).unwrap();
        if let Err(e) = js.poll(0) {
            failure = Some(e);
            break;
        }
    }
    // N cannot open the inviter's first sealed frame: "wrong code", and nothing more precise.
    assert_eq!(failure, Some(CoreError::PairingFailed));
    assert_eq!(js.phase(), JoinPhase::Failed);
    // It closed the channel: the inviter sees it, and so does any later claim.
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert_eq!(inv.phase(), InvitePhase::Cancelled);
    let other = w.device("android");
    assert_eq!(
        other.engine.join_space(&code, "Autre").unwrap_err(),
        CoreError::PairingExpired,
        "a closed channel answers 409 channel_closed: \"expired\" for the user"
    );
    // Nothing happened to the space, and neither device kept anything.
    assert_eq!(a.trust_json(), before);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
    assert_eq!(a.engine.list_devices().unwrap().len(), 1);
    // The inviter offers a new code, on a new channel.
    let again = a.engine.start_invitation().unwrap();
    assert_ne!(again.channel_id(), channel);
    assert_ne!(again.code().unwrap(), code);
    // The session that failed gave its secrets up.
    assert!(inv.code().is_none() && inv.prompt().is_none());
}

// ---------------------------------------------------------------------------
// P20: a channel somebody else claimed
// ---------------------------------------------------------------------------

#[test]
fn p20_a_claimed_channel_is_taken_and_the_batch_waits_for_both_humans() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let code = inv.code().unwrap();
    let channel = inv.channel_id().to_owned();
    let before = a.trust_json();

    // Someone who saw the code claims first.
    let mut rogue = Rogue::claim(&w.relay, &code);
    // The device of the user finds the code used, and says so: the user will not confirm.
    assert_eq!(
        n.engine.join_space(&code, "Pixel").unwrap_err(),
        CoreError::PairingTaken
    );
    assert!(!n.engine.has_pending_pairing().unwrap());

    // The intruder has the code, so its CPace run succeeds and the inviter shows a code.
    for _ in 0..6 {
        inv.poll(0).unwrap();
        rogue.step().unwrap();
    }
    assert_eq!(inv.phase(), InvitePhase::Verify);
    assert!(inv.prompt().is_some());
    // Case 1: the intruder answers "yes" at once, the user of A has not confirmed. Nothing leaves.
    rogue.confirm(false);
    for _ in 0..5 {
        inv.poll(0).unwrap();
    }
    assert_eq!(
        w.relay.channel_messages(&channel),
        Some((2, 3)),
        "commit and reveal only: no batch before the user of the inviter confirmed"
    );
    // The user of A sees the other device is not the expected one, and refuses.
    inv.cancel();
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(a.trust_json(), before);
    assert_eq!(a.engine.list_devices().unwrap().len(), 1);
}

#[test]
fn p20_the_user_of_the_inviter_alone_does_not_release_the_space_key() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let mut rogue = Rogue::claim(&w.relay, &inv.code().unwrap());
    for _ in 0..6 {
        inv.poll(0).unwrap();
        rogue.step().unwrap();
    }
    assert_eq!(inv.phase(), InvitePhase::Verify);
    // Case 2: the user of A pressed "Add", but the other side never confirmed.
    inv.confirm().unwrap();
    assert_eq!(inv.phase(), InvitePhase::Provisioning);
    for _ in 0..8 {
        inv.poll(0).unwrap();
    }
    assert_eq!(
        w.relay.channel_messages(&channel),
        Some((2, 2)),
        "no joiner_confirm: the inviter sent nothing but commit and reveal"
    );
    // Once the other side does confirm, the batch goes out: the lock is exactly that.
    rogue.confirm(false);
    inv.poll(0).unwrap();
    assert_eq!(w.relay.channel_messages(&channel), Some((3, 3)));
}

#[test]
fn p20_a_wrong_code_gets_an_intruder_nowhere() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let mut rogue = Rogue::claim(&w.relay, &with_wrong_word(&inv.code().unwrap()));
    let mut failure = None;
    for _ in 0..10 {
        inv.poll(0).unwrap();
        if let Err(e) = rogue.step() {
            failure = Some(e);
            break;
        }
    }
    assert_eq!(failure, Some(CoreError::PairingFailed));
    // A never opened a joiner_hello: it still waits for the peer, and wrote only commit and reveal.
    assert_eq!(inv.phase(), InvitePhase::AwaitPeer);
    assert!(inv.prompt().is_none());
    assert_eq!(w.relay.channel_messages(&channel), Some((2, 1)));
}

// ---------------------------------------------------------------------------
// P21: expiration
// ---------------------------------------------------------------------------

#[test]
fn p21_a_code_nobody_used_expires_and_a_new_one_is_made() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let code = inv.code().unwrap();
    w.clock.advance(121_000);
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingExpired);
    assert_eq!(
        n.engine.join_space(&code, "Pixel").unwrap_err(),
        CoreError::PairingExpired
    );
    assert!(w.relay.channel_ids().is_empty(), "the relay dropped it");
    // The invitation starts again.
    let again = a.engine.start_invitation().unwrap();
    assert_ne!(again.code().unwrap(), code);
}

#[test]
fn p21_a_claimed_channel_without_confirmation_expires_on_both_sides() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // Nobody presses anything for two minutes.
    w.clock.advance(121_000);
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingExpired);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingExpired);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
    assert_eq!(a.trust_json(), before, "the store of A did not change");
    assert_eq!(a.engine.list_devices().unwrap().len(), 1);
    assert_eq!(a.engine.transport.devices().unwrap().len(), 1);
}

#[test]
fn p21_a_session_is_given_up_after_600_seconds_whatever_the_relay_says() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    // A relay that never expires its channels.
    w.relay.with(|s| s.pairing_ttl_ms = i64::MAX / 4);
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    w.clock.advance(599_000);
    inv.poll(0).unwrap();
    w.clock.advance(2_000);
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingExpired);
    assert_eq!(
        w.relay.channel_state(&channel),
        Some(ChannelState::Closed),
        "the channel is closed on the way"
    );
    // Same on the joining side.
    let inv = a.engine.start_invitation().unwrap();
    let n = w.device("android");
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    w.clock.advance(601_000);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingExpired);
}

// ---------------------------------------------------------------------------
// P22, P23: refusals
// ---------------------------------------------------------------------------

#[test]
fn p22_a_refusal_on_the_new_device_closes_the_channel_and_registers_nothing() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // "They are different."
    js.cancel();
    assert_eq!(js.phase(), JoinPhase::Cancelled);
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert_eq!(inv.phase(), InvitePhase::Cancelled);
    assert_eq!(a.trust_json(), before);
    assert_eq!(a.engine.transport.devices().unwrap().len(), 1);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
    assert!(js.prompt().is_none(), "the secrets of the session are gone");
}

#[test]
fn p23_a_refusal_on_the_inviter_closes_the_channel_and_n_stays_unprovisioned() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    js.confirm(false).unwrap();
    inv.cancel();
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert_eq!(js.phase(), JoinPhase::Cancelled);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
    assert_eq!(a.trust_json(), before);
    // A cancelled session stays cancelled.
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert_eq!(inv.confirm().unwrap_err(), CoreError::PairingCancelled);
}

// ---------------------------------------------------------------------------
// P24: the relay refuses the registration
// ---------------------------------------------------------------------------

/// Drives both devices to the point where the new device acknowledged the batch and the
/// inviter has not yet read the acknowledgement.
fn up_to_the_acknowledgement(inv: &InviteSession, js: &JoinSession) {
    compare_and_confirm(inv, js, false);
    inv.poll(0).unwrap();
    assert_eq!(js.poll(0).unwrap(), JoinPhase::Completing);
}

#[test]
fn p24_a_rotation_during_the_pairing_ends_it_and_the_store_of_a_is_unchanged() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    assert!(
        n.engine.has_pending_pairing().unwrap(),
        "sealed before the ack"
    );

    // A rotation passes between the start of the invitation and the registration.
    w.relay.with(|s| s.key_epoch = 2);
    assert_eq!(
        inv.poll(0).unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "epoch_conflict".into()
        }
    );
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(a.trust_json(), before);
    assert_eq!(a.engine.list_devices().unwrap().len(), 1);
    // N sees the channel closed: the batch is thrown away.
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert!(!n.engine.is_provisioned());
    assert!(!n.engine.has_pending_pairing().unwrap());
}

#[test]
fn p24_a_device_the_relay_already_knows_ends_it_too() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The relay holds these keys already (for a registration this device does not remember).
    let Stored::Found(lot) = n.engine.load_lot().unwrap() else {
        panic!("sealed")
    };
    let public = lot.keys.public();
    w.relay.add_device(DeviceRecord {
        device_id: lot.keys.device_id(),
        name: "Fantome".into(),
        platform: "android".into(),
        sign_pk: public.ed25519,
        kx_pk: public.x25519,
        certificate: vec![],
        issuer_device_id: a.id(),
        joined_seq: 0,
        registered_at_ms: T0,
        last_seen_at_ms: None,
        active: Some(false),
    });
    assert_eq!(
        inv.poll(0).unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "device_exists".into()
        }
    );
    assert_eq!(a.trust_json(), before);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
}

#[test]
fn a_frame_of_another_version_is_unsupported_and_a_damaged_one_is_a_failure() {
    // A relay that rewrites the version byte of the first frame: "update Tomi" for the new
    // device. Anything else it alters is simply a failed pairing.
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    w.relay.tamper_message(&channel, 1, |b| b[0] = 2);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingUnsupported);
    assert_eq!(js.phase(), JoinPhase::Failed);
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingCancelled);

    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    js.poll(0).unwrap();
    inv.poll(0).unwrap();
    // The inviter's `reveal` (index 3) loses a bit of its sealed part.
    w.relay.tamper_message(&channel, 3, |b| {
        let last = b.len() - 1;
        b[last] ^= 1;
    });
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingFailed);
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
}

#[test]
fn a_relay_that_lies_about_the_state_cannot_complete_a_pairing() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // "Completed", although nobody was registered: only the inviter completes a channel.
    w.relay
        .force_channel_state(&channel, ChannelState::Completed);
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingFailed);
    // The inviter closed the channel on the way; the same lie told to the new device.
    w.relay
        .force_channel_state(&channel, ChannelState::Completed);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingFailed);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
    assert_eq!(a.trust_json(), before);
}

#[test]
fn p24_a_full_space_ends_it_the_same_way() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The space filled up meanwhile (as far as the relay is concerned).
    w.relay.with(|s| s.limits.max_devices = 1);
    assert_eq!(
        inv.poll(0).unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "too_many_devices".into()
        }
    );
    assert_eq!(a.trust_json(), before);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());
}

#[test]
fn a_member_learned_while_the_batch_is_in_flight_stops_the_registration() {
    // The new device would never hear of a member A learns after the batch was sealed: its log
    // starts at its own arrival. A does not register it; the user starts again.
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let b = w.device("android");
    w.pair(&a, &b, "Tablette", false);
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);

    // Meanwhile B pairs another device, and A (its doorbell) syncs and learns it.
    let x = w.device("android");
    w.pair(&b, &x, "Montre", false);
    a.engine.sync().unwrap();
    assert!(a.knows(&x));
    let with_x = a.trust_json();

    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingFailed);
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    // Nothing was registered, A still knows X, and the new device threw its batch away.
    assert_eq!(a.trust_json(), with_x);
    assert_eq!(a.engine.transport.devices().unwrap().len(), 3);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert!(!n.engine.is_provisioned() && !n.engine.has_pending_pairing().unwrap());

    // Started again, it carries X.
    let n2 = w.device("android");
    w.pair(&a, &n2, "Pixel", false);
    assert!(n2.knows(&x) && n2.knows(&a) && n2.knows(&b));
}

#[test]
fn a_member_learned_before_the_batch_is_built_is_in_the_batch() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let b = w.device("android");
    w.pair(&a, &b, "Tablette", false);
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // X joins through B while the users compare the codes; A learns it before it builds the
    // batch (it reads its store at the moment of A10, not at the start of the invitation).
    let x = w.device("android");
    w.pair(&b, &x, "Montre", false);
    a.engine.sync().unwrap();
    inv.confirm().unwrap();
    js.confirm(false).unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(
        n.knows(&x),
        "the batch carried the member learned after the code was shown"
    );
    assert!(a.knows(&x) && a.knows(&n));
}

#[test]
fn a_full_space_is_seen_before_a_channel_is_opened() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    w.relay.with(|s| s.limits.max_devices = 1);
    w.relay.expire_sessions();
    assert_eq!(
        a.engine.start_invitation().unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "too_many_devices".into()
        }
    );
    assert!(w.relay.channel_ids().is_empty());
}

// ---------------------------------------------------------------------------
// P25: lost answers
// ---------------------------------------------------------------------------

#[test]
fn p25_a_lost_answer_to_the_registration_is_replayed_and_found_in_the_list() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The relay registers the device and the answer never comes back.
    let (ed, id) = n.pending_identity();
    w.relay.lose_next_response("post_device");
    assert_eq!(inv.poll(0).unwrap(), InvitePhase::Done);
    assert_eq!(w.relay.count("post_device"), 2, "replayed once");
    assert!(w.relay.count("devices") >= 1, "and the list was read");
    let outcome = inv.wait_done().unwrap();
    assert_eq!(outcome.device_id, id);
    assert!(a.knows_key(&ed));
    let listed = a.engine.list_devices().unwrap();
    assert_eq!(
        listed.iter().find(|d| d.id == id).unwrap().state,
        DeviceState::Verified
    );
    // The relay registered it once.
    assert_eq!(a.engine.transport.devices().unwrap().len(), 2);
    // And the new device was not left behind.
    js.poll(0).unwrap();
    assert!(n.engine.is_provisioned());
    assert_eq!(n.id(), id);
}

#[test]
fn a_registration_that_gets_no_answer_at_all_can_be_polled_again() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The relay is out of reach for every attempt of this call.
    let (ed, _) = n.pending_identity();
    w.relay.set_offline(true);
    let err = inv.poll(0).unwrap_err();
    assert!(matches!(err, CoreError::Network { .. }), "{err:?}");
    assert_eq!(
        inv.phase(),
        InvitePhase::Provisioning,
        "the session is alive"
    );
    assert!(!a.knows_key(&ed));
    // Back online: the registration resumes where it was.
    w.relay.set_offline(false);
    assert_eq!(inv.poll(0).unwrap(), InvitePhase::Done);
    assert!(a.knows_key(&ed));
}

#[test]
fn a_frame_deposited_twice_after_a_lost_answer_is_read_once() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    // The share of the new device reaches the relay twice: the first answer is lost.
    w.relay.lose_next_response("post_pairing_message");
    js.poll(0).unwrap();
    assert_eq!(w.relay.channel_messages(&channel), Some((1, 2)));
    // The inviter's reveal is deposited twice as well.
    w.relay.lose_next_response("post_pairing_message");
    inv.poll(0).unwrap();
    assert_eq!(w.relay.channel_messages(&channel), Some((3, 2)));
    compare_and_confirm(&inv, &js, false);
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
    // commit, reveal twice, provision | share twice, hello, confirm, ack: every frame was
    // read once, in order, and the machines never saw a copy.
    assert_eq!(w.relay.channel_messages(&channel), Some((4, 5)));
}

// ---------------------------------------------------------------------------
// P26: the new device stops between the acknowledgement and the provisioning
// ---------------------------------------------------------------------------

#[test]
fn p26_a_stop_after_the_acknowledgement_resumes_when_the_inviter_registered_the_device() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The inviter registers the device; the new device does not read "completed"...
    assert_eq!(inv.poll(0).unwrap(), InvitePhase::Done);
    // ... because the app is killed.
    drop(js);
    let n = n.restart(&w);
    assert!(!n.engine.is_provisioned());
    assert!(n.engine.has_pending_pairing().unwrap());

    assert_eq!(
        n.engine.resume_pairing().unwrap(),
        ResumeOutcome::Provisioned
    );
    assert!(n.engine.is_provisioned());
    assert!(!n.engine.has_pending_pairing().unwrap());
    assert_eq!(n.engine.space_id().unwrap(), a.engine.space_id().unwrap());
    assert!(n.knows(&a));
    // It works, and nothing is left to resume.
    let id = send(&a, draft("apres le redemarrage", false));
    n.engine.sync().unwrap();
    assert_eq!(plain_of(&n.engine, &id), "apres le redemarrage");
    assert_eq!(n.engine.resume_pairing().unwrap(), ResumeOutcome::Nothing);
}

#[test]
fn p26_a_stop_after_the_acknowledgement_discards_the_batch_when_the_inviter_did_not_register() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    // The inviter gives up; the app of the new device is killed before it learns it.
    inv.cancel();
    drop(js);
    let n = n.restart(&w);
    assert!(n.engine.has_pending_pairing().unwrap());

    // No network: nothing is decided, the batch stays.
    w.relay.set_offline(true);
    assert!(matches!(
        n.engine.resume_pairing().unwrap_err(),
        CoreError::Network { .. }
    ));
    assert!(n.engine.has_pending_pairing().unwrap());
    w.relay.set_offline(false);

    // The relay does not know the device (401): the batch is erased.
    assert_eq!(n.engine.resume_pairing().unwrap(), ResumeOutcome::Discarded);
    assert!(!n.engine.has_pending_pairing().unwrap());
    assert!(!n.engine.is_provisioned());
    // The device can join again from scratch.
    let inv = a.engine.start_invitation().unwrap();
    n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
}

#[test]
fn p26_a_channel_purged_before_completed_was_read_is_settled_with_the_relay() {
    // The inviter registered the device, but the new device only looks after the channel is gone.
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    assert_eq!(inv.poll(0).unwrap(), InvitePhase::Done);
    w.clock.advance(121_000);
    assert_eq!(js.poll(0).unwrap(), JoinPhase::Done);
    assert!(n.engine.is_provisioned());

    // And when the inviter did not register it: "expired", the batch is gone.
    let n2 = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n2
        .engine
        .join_space(&inv.code().unwrap(), "Tablette")
        .unwrap();
    up_to_the_acknowledgement(&inv, &js);
    w.clock.advance(121_000);
    assert_eq!(js.poll(0).unwrap_err(), CoreError::PairingExpired);
    assert!(!n2.engine.is_provisioned() && !n2.engine.has_pending_pairing().unwrap());
}

#[test]
fn no_logging_macro_in_the_pairing_runtime_sources() {
    // The runtime has no logger either: a `println!` of a code, a verification code or a token
    // would have to be added on purpose, to this list.
    for src in [
        include_str!("mod.rs"),
        include_str!("founder.rs"),
        include_str!("history.rs"),
        include_str!("invite.rs"),
        include_str!("join.rs"),
        include_str!("pending.rs"),
    ] {
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
fn sessions_and_the_engine_can_be_shared_between_threads() {
    fn shareable<T: Send + Sync>() {}
    shareable::<Engine>();
    shareable::<InviteSession>();
    shareable::<JoinSession>();
    shareable::<Arc<InviteSession>>();
    shareable::<Arc<JoinSession>>();
}

#[test]
fn a_resumption_leaves_the_batch_of_a_live_session_alone() {
    // The app resumes pairings when it comes back to the foreground, while the screen of a
    // pairing is still up: the relay does not know the device *yet*, and the keys must not go.
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    assert_eq!(n.engine.resume_pairing().unwrap(), ResumeOutcome::Nothing);
    assert!(n.engine.has_pending_pairing().unwrap(), "still sealed");
    // The pairing goes on and succeeds with those keys.
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
    assert!(!n.engine.has_pending_pairing().unwrap());
}

#[test]
fn an_abandoned_session_gives_its_batch_back_to_the_resumption() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    inv.poll(0).unwrap();
    // The app drops the session without a word; the process lives on.
    drop(js);
    assert_eq!(
        n.engine.resume_pairing().unwrap(),
        ResumeOutcome::Provisioned
    );
    assert!(n.engine.is_provisioned());
}

#[test]
fn a_second_session_cannot_overwrite_the_sealed_batch_of_the_first() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let first = a.engine.start_invitation().unwrap();
    let second = a.engine.start_invitation().unwrap();
    let js1 = n
        .engine
        .join_space(&first.code().unwrap(), "Pixel")
        .unwrap();
    let js2 = n
        .engine
        .join_space(&second.code().unwrap(), "Pixel")
        .unwrap();
    up_to_the_acknowledgement(&first, &js1);
    let (ed, _) = n.pending_identity();
    // The second reaches the point of sealing its batch: refused, and that session ends.
    compare_and_confirm(&second, &js2, false);
    second.poll(0).unwrap();
    assert!(matches!(
        js2.poll(0).unwrap_err(),
        CoreError::InvalidInput { .. }
    ));
    assert_eq!(js2.phase(), JoinPhase::Failed);
    // The first one's batch is still there, untouched, and cancelling the second one did not
    // erase it either.
    js2.cancel();
    assert_eq!(n.pending_identity().0, ed);
    run(&first, &js1, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
    assert_eq!(n.engine.local_device().unwrap().id, n.id());
}

#[test]
fn the_inviter_renews_its_session_in_the_middle_of_a_pairing() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // The relay forgets the sessions while the users compare the codes.
    w.relay.expire_sessions();
    inv.confirm().unwrap();
    js.confirm(false).unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
}

#[test]
fn a_batch_that_cannot_be_sealed_ends_the_pairing_and_the_acknowledgement_never_leaves() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let before = a.trust_json();
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    compare_and_confirm(&inv, &js, false);
    inv.poll(0).unwrap();
    // The protected storage fails when the new device verifies the batch.
    n.engine
        .store
        .tx(|db| db.conn_exec("ALTER TABLE secrets RENAME TO secrets_away"))
        .unwrap();
    let err = js.poll(0).unwrap_err();
    assert_eq!(err.code(), "internal", "not a transient error: {err:?}");
    assert_eq!(js.phase(), JoinPhase::Failed);
    // Nothing was acknowledged, and the channel is closed: the inviter registers nothing.
    assert_eq!(w.relay.channel_messages(&channel).map(|m| m.1), Some(0));
    assert_eq!(w.relay.channel_state(&channel), Some(ChannelState::Closed));
    assert_eq!(inv.poll(0).unwrap_err(), CoreError::PairingCancelled);
    assert_eq!(a.trust_json(), before);
    assert_eq!(a.engine.transport.devices().unwrap().len(), 1);
    // The device can try again once its storage is back.
    n.engine
        .store
        .tx(|db| db.conn_exec("ALTER TABLE secrets_away RENAME TO secrets"))
        .unwrap();
    assert!(!n.engine.has_pending_pairing().unwrap());
    let (_, joined) = w.pair(&a, &n, "Pixel", false);
    assert_eq!(joined.device_id, n.id());
}

#[test]
fn the_batch_is_sealed_before_the_acknowledgement_and_provisioning_erases_it() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    compare_and_confirm(&inv, &js, false);
    inv.poll(0).unwrap();
    // The acknowledgement is refused by the network: it did not leave, the batch is sealed.
    w.relay
        .fail_next("post_pairing_message", RelayError::Network("cut".into()));
    w.relay
        .fail_next("post_pairing_message", RelayError::Network("cut".into()));
    w.relay
        .fail_next("post_pairing_message", RelayError::Network("cut".into()));
    w.relay
        .fail_next("post_pairing_message", RelayError::Network("cut".into()));
    assert!(matches!(js.poll(0).unwrap_err(), CoreError::Network { .. }));
    assert_eq!(
        w.relay.channel_messages(&channel),
        Some((3, 3)),
        "no acknowledgement yet"
    );
    assert!(n.engine.has_pending_pairing().unwrap(), "sealed first");
    assert_eq!(js.phase(), JoinPhase::Completing);
    // The next poll deposits it; the inviter registers; the new device provisions from the
    // sealed copy and erases it.
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(n.engine.is_provisioned());
    assert!(!n.engine.has_pending_pairing().unwrap());
}

// ---------------------------------------------------------------------------
// Reads that begin after the change of state they wait for
// ---------------------------------------------------------------------------

/// The relay asks to slow down (`429`, `Retry-After: 2`), once, on the next read of a channel.
fn slow_down_next_read(w: &World) {
    w.relay.fail_next(
        "pairing_messages",
        RelayError::problem(ApiProblem {
            status: 429,
            code: "rate_limited".into(),
            retry_after_secs: Some(2),
            ..Default::default()
        }),
    );
}

#[test]
fn a_read_repeated_after_a_failure_only_looks_and_a_wait_for_a_state_is_short() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    up_to_the_acknowledgement(&inv, &js);
    let waits_since = |from: usize| w.relay.pairing_waits().split_off(from);
    let mark = || w.relay.pairing_waits().len();

    // The new device waits for "completed": a change of state, so the wait is short, and a read
    // that is repeated after the relay asked to slow down does not wait at all. (The real relay
    // wakes a long poll on a change that happens *while it waits*: a repeat that begins after
    // the inviter registered the device would sleep through the whole wait. This is what
    // happened to the first run against the real relay: 25 s.)
    let at = mark();
    assert_eq!(js.poll(25).unwrap(), JoinPhase::Completing);
    assert_eq!(waits_since(at), vec![STATE_POLL_SECS]);
    let at = mark();
    slow_down_next_read(&w);
    assert_eq!(js.poll(25).unwrap(), JoinPhase::Completing);
    assert_eq!(waits_since(at), vec![STATE_POLL_SECS, 0]);

    // The inviter waits for a frame: the long wait is what it is for, and its repeat only looks.
    let at = mark();
    slow_down_next_read(&w);
    assert_eq!(inv.poll(25).unwrap(), InvitePhase::Done);
    assert_eq!(waits_since(at), vec![LONG_POLL_SECS, 0]);

    // The channel is completed now, before the next read of the new device begins: the read
    // that looks finds it at once.
    assert_eq!(js.poll(25).unwrap(), JoinPhase::Done);
    assert!(n.engine.is_provisioned());
}

#[test]
fn a_wait_for_frames_is_passed_on_to_the_relay_up_to_its_limit() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    // Before the acknowledgement the new device waits for frames: the wait it was asked for,
    // never more than the 25 s the relay accepts (it answers `422` beyond).
    let at = w.relay.pairing_waits().len();
    js.poll(25).unwrap();
    js.poll(7).unwrap();
    js.poll(60).unwrap();
    assert_eq!(
        w.relay.pairing_waits().split_off(at),
        vec![LONG_POLL_SECS, 7, LONG_POLL_SECS]
    );
    drop(inv);
}

#[test]
fn a108_a_long_poll_on_a_terminated_channel_returns_at_once() {
    // The double does what the real relay does (`apps/relay` `get_messages`, lot 3): a channel
    // already `completed` or `closed` answers immediately, whatever `wait` asks — no message can
    // arrive any more. A read that waited on it would only burn `rl_pairing_poll` on the relay.
    let r = raw();
    let c = r.a.engine.transport.create_pairing_channel().unwrap();
    let token = r
        .outsider
        .claim_pairing_channel(&c.channel_id)
        .unwrap()
        .pairing_token;
    let joiner = PairingAuth::Joiner(&token);
    r.w.relay.with(|s| s.long_poll_cap_ms = 400);

    r.a.engine
        .transport
        .close_pairing_channel(&c.channel_id, PairingAuth::Inviter)
        .unwrap();
    // Closed before the poll: even a `wait = 25` read returns at once (not after the cap).
    let started = Instant::now();
    let got = r
        .outsider
        .pairing_messages(&c.channel_id, joiner, 0, 25)
        .unwrap();
    assert_eq!(got.state, ChannelState::Closed);
    assert!(got.messages.is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "a terminated channel does not wait: {:?}",
        started.elapsed()
    );
    // `wait = 0` likewise.
    let started = Instant::now();
    let got = r
        .outsider
        .pairing_messages(&c.channel_id, joiner, 0, 0)
        .unwrap();
    assert_eq!(got.state, ChannelState::Closed);
    assert!(started.elapsed() < Duration::from_millis(300));
}

// ---------------------------------------------------------------------------
// P27, P28, P29: the history
// ---------------------------------------------------------------------------

/// A has a history (a text, a secret, a pinned item with a file); returns the ids.
fn with_history(a: &Dev) -> (Vec<String>, Vec<u8>) {
    let file: Vec<u8> = (0..90_000u32).map(|i| (i % 199) as u8).collect();
    let src = a.dir.path().join("doc.bin");
    std::fs::write(&src, &file).unwrap();
    let first = send(a, draft("premier", false));
    let secret = send(a, draft("mot de passe", true));
    let mut with_file = draft("avec fichier", false);
    with_file.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "doc.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let third = send(a, with_file);
    a.engine.pin(&secret).unwrap();
    a.engine.sync().unwrap();
    (vec![first, secret, third], file)
}

#[test]
fn p27_the_announced_history_is_exported_with_its_id_and_imported_without_a_question() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let (ids, file) = with_history(&a);
    // The new device would wait for the user before importing anything...
    let n = w.device_with(EngineConfig {
        platform: "android".into(),
        auto_import: false,
        ..EngineConfig::default()
    });

    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    // ... but the single question is asked at pairing, with what the inviter offers.
    let offer = js.prompt().unwrap().history.unwrap();
    assert_eq!(offer.items, 3);
    assert!(offer.bytes > 90_000);
    js.confirm(true).unwrap();
    inv.confirm().unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    let invited = inv.wait_done().unwrap();
    let joined = js.wait_done().unwrap();

    // Both sides hold the same announcement.
    let announced = joined.history.clone().unwrap();
    assert_eq!(invited.history, joined.history);
    assert_eq!(
        (announced.items, announced.bytes),
        (offer.items, offer.bytes)
    );
    uuid::Uuid::parse_str(&announced.transfer_id).unwrap();
    let awaited = n.engine.awaited_history().unwrap().unwrap();
    assert_eq!(awaited.transfer_id, announced.transfer_id);
    assert_eq!(awaited.from_device_id, a.id());
    assert_eq!(
        w.relay.count("post_transfer"),
        0,
        "confirm did not wait for the export: it is a task of the next sync"
    );

    // The next sync of A starts the export, with exactly that id.
    a.engine.sync().unwrap();
    let exports = a.engine.transfers().unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].transfer_id, announced.transfer_id);
    assert_eq!(exports[0].role, "export");
    assert_eq!(exports[0].state, "posted");
    assert_eq!(w.relay.count("post_transfer"), 1);

    // N imports it as soon as it learns of it, without accept_import.
    n.engine.sync().unwrap();
    let imports = n.engine.transfers().unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].transfer_id, announced.transfer_id);
    assert_eq!(imports[0].state, "done", "{imports:?}");
    assert_eq!(imports[0].items_done, 3);
    assert!(n.engine.awaited_history().unwrap().is_none());
    assert_eq!(n.engine.list_items().unwrap().len(), 3);
    for id in &ids {
        assert_eq!(
            n.engine.get_item(id).unwrap().unwrap().origin,
            ItemOrigin::Imported
        );
    }
    match n.engine.get_item(&ids[1]).unwrap().unwrap().content {
        ItemContentView::Readable { secret, text, .. } => {
            assert!(secret, "a secret stays a secret");
            assert_eq!(text.unwrap().plain, "mot de passe");
        }
        other => panic!("{other:?}"),
    }
    assert!(n.engine.get_item(&ids[1]).unwrap().unwrap().pinned);
    let path = n.engine.attachment_path(&ids[2], 0).unwrap().unwrap();
    assert_eq!(std::fs::read(path).unwrap(), file);
    // The relay was told "imported", and no receipt was sent for imported items.
    let closed: Vec<String> = w
        .relay
        .events()
        .into_iter()
        .filter_map(|e| match e.data {
            EventData::HistoryTransferClosed { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(closed, vec!["imported".to_owned()]);
    assert_eq!(w.relay.count("ack_item"), 0);
}

#[test]
fn p28_a_no_to_the_history_announces_and_exports_nothing() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    with_history(&a);
    let n = w.device("android");
    let (invited, joined) = w.pair(&a, &n, "Pixel", false);
    assert!(invited.history.is_none() && joined.history.is_none());
    assert!(n.engine.awaited_history().unwrap().is_none());
    a.engine.sync().unwrap();
    n.engine.sync().unwrap();
    assert_eq!(w.relay.count("post_transfer"), 0);
    assert!(a.engine.transfers().unwrap().is_empty());
    assert!(n.engine.transfers().unwrap().is_empty());
    assert!(
        n.engine.list_items().unwrap().is_empty(),
        "a fresh, empty feed"
    );
}

#[test]
fn p28_nothing_to_offer_means_no_question_and_no_transfer() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    assert_eq!(
        js.prompt().unwrap().history,
        None,
        "an empty feed: no question"
    );
    // Even a yes is ignored: there is nothing to announce.
    js.confirm(true).unwrap();
    inv.confirm().unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    assert!(inv.wait_done().unwrap().history.is_none());
    assert!(js.wait_done().unwrap().history.is_none());
    a.engine.sync().unwrap();
    assert_eq!(w.relay.count("post_transfer"), 0);
}

#[test]
fn p28_a_relay_without_transfers_offers_none() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    with_history(&a);
    w.relay.with(|s| s.limits.transfer_max_open = None);
    w.relay.expire_sessions();
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    assert_eq!(js.prompt().unwrap().history, None);
}

#[test]
fn p29_a_new_device_that_ignores_the_announcement_still_gets_the_transfer_from_the_log() {
    for auto_import in [true, false] {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        with_history(&a);
        let n = w.device_with(EngineConfig {
            platform: "android".into(),
            auto_import,
            ..EngineConfig::default()
        });
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
        compare_and_confirm(&inv, &js, true);
        inv.poll(0).unwrap();
        js.poll(0).unwrap();
        // A reader that does not know the tag: the batch it kept has no announcement.
        let Stored::Found(mut lot) = n.engine.load_lot().unwrap() else {
            panic!("the batch is sealed")
        };
        assert!(lot.history.is_some());
        lot.history = None;
        n.engine.replace_lot(&lot).unwrap();
        run(&inv, &js, |a, n| {
            a == InvitePhase::Done && n == JoinPhase::Done
        });
        assert!(js.wait_done().unwrap().history.is_none());
        assert!(n.engine.awaited_history().unwrap().is_none());

        // A still exports with the id it announced; N meets a transfer like any other.
        a.engine.sync().unwrap();
        n.engine.sync().unwrap();
        let imports = n.engine.transfers().unwrap();
        assert_eq!(imports.len(), 1);
        if auto_import {
            assert_eq!(imports[0].state, "done");
            assert_eq!(n.engine.list_items().unwrap().len(), 3);
        } else {
            assert_eq!(imports[0].state, "received", "the usual rule: ask first");
            n.engine.accept_import(&imports[0].transfer_id).unwrap();
            assert_eq!(n.engine.transfers().unwrap()[0].state, "done");
            assert_eq!(n.engine.list_items().unwrap().len(), 3);
        }
    }
}

#[test]
fn an_announced_export_survives_a_cut_network_after_the_registration() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    with_history(&a);
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
    compare_and_confirm(&inv, &js, true);
    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    // The export intent was written with the device row: a restart of A does not lose it.
    drop((inv, js));
    let a = a.restart(&w);
    w.relay.set_offline(true);
    assert!(a.engine.sync().unwrap().offline);
    assert!(a.engine.transfers().unwrap().is_empty());
    w.relay.set_offline(false);
    a.engine.sync().unwrap();
    let exports = a.engine.transfers().unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].state, "posted");
    n.engine.sync().unwrap();
    assert_eq!(n.engine.list_items().unwrap().len(), 3);
}

// ---------------------------------------------------------------------------
// P30: the founder
// ---------------------------------------------------------------------------

#[test]
fn p30_the_founder_creates_the_space_and_is_provisioned() {
    let w = World::new();
    let d = w.device("windows");
    assert!(!w.relay.with(|s| s.initialized));
    d.engine.create_space(TOKEN, "  PC-Bureau ").unwrap();
    assert!(d.engine.is_provisioned());
    assert!(
        !d.engine.has_pending_pairing().unwrap(),
        "the keys are not sealed any more"
    );
    assert!(w.relay.with(|s| s.initialized));
    assert_eq!(d.engine.local_device().unwrap().name, "PC-Bureau");
    assert_eq!(d.engine.space_status(), crate::runtime::SpaceStatus::Active);
    // A session is open: it can send, and nothing else is needed.
    let id = send(&d, draft("premier", false));
    assert_eq!(plain_of(&d.engine, &id), "premier");
    // The relay now has a space: nobody creates another one, with any token.
    let other = w.device("android");
    assert_eq!(
        other.engine.create_space(TOKEN, "Autre").unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "space_exists".into()
        }
    );
    assert!(!other.engine.has_pending_pairing().unwrap());
    // Already provisioned: no second space.
    assert!(d.engine.create_space(TOKEN, "Encore").is_err());
}

#[test]
fn p30_a_wrong_token_provisions_nothing_and_another_try_keeps_the_same_keys() {
    let w = World::new();
    let d = w.device("windows");
    let err = d
        .engine
        .create_space("aaaa-bbbb-cccc-dddd", "PC")
        .unwrap_err();
    assert_eq!(
        err,
        CoreError::Relay {
            status: 403,
            code: "install_token_invalid".into()
        }
    );
    assert!(!d.engine.is_provisioned());
    assert!(!w.relay.with(|s| s.initialized));
    // The keys are sealed, to be reused: the token was not consumed.
    assert!(d.engine.has_pending_pairing().unwrap());
    let Stored::Found(m) = d.engine.load_founder().unwrap() else {
        panic!("sealed")
    };
    let id = m.device_id();
    d.engine.create_space(TOKEN, "PC").unwrap();
    assert!(d.engine.is_provisioned());
    assert_eq!(d.id(), id, "same keys as the first try");
    assert!(!d.engine.has_pending_pairing().unwrap());
    // Blank and absurd tokens never reach the relay.
    let e = w.device("android");
    assert!(e.engine.create_space("   ", "PC").is_err());
    assert!(e.engine.create_space(&"x".repeat(500), "PC").is_err());
    assert!(e.engine.create_space(TOKEN, "").is_err());
    assert_eq!(w.relay.count("post_space"), 2);
}

#[test]
fn p30_a_lost_answer_then_space_exists_is_the_creation_of_this_device() {
    let w = World::new();
    let d = w.device("windows");
    // The relay creates the space; this device never hears about it, in four attempts.
    w.relay.lose_next_response("post_space");
    let cut = RelayError::Network("cut".into());
    w.relay.fail_next_after("post_space", 1, cut.clone());
    w.relay.fail_next("post_space", cut.clone());
    w.relay.fail_next("post_space", cut);
    assert!(matches!(
        d.engine.create_space(TOKEN, "PC").unwrap_err(),
        CoreError::Network { .. }
    ));
    assert!(w.relay.with(|s| s.initialized), "the space exists");
    assert!(!d.engine.is_provisioned());
    assert!(d.engine.has_pending_pairing().unwrap());

    // Another try: `409 space_exists`, then the challenge/response succeeds: it was this device.
    d.engine.create_space(TOKEN, "PC").unwrap();
    assert!(d.engine.is_provisioned());
    assert!(!d.engine.has_pending_pairing().unwrap());
    assert_eq!(
        d.engine.space_id().unwrap(),
        w.relay.with(|s| s.space_id.clone())
    );
}

#[test]
fn p30_a_lost_answer_is_settled_by_resume_pairing_without_the_token() {
    let w = World::new();
    let d = w.device("windows");
    w.relay.lose_next_response("post_space");
    let cut = RelayError::Network("cut".into());
    w.relay.fail_next_after("post_space", 1, cut.clone());
    w.relay.fail_next("post_space", cut.clone());
    w.relay.fail_next("post_space", cut);
    assert!(d.engine.create_space(TOKEN, "PC").is_err());
    // The app is killed and starts again: the token is not needed to finish.
    let d = d.restart(&w);
    assert!(d.engine.has_pending_pairing().unwrap());
    assert_eq!(
        d.engine.resume_pairing().unwrap(),
        ResumeOutcome::Provisioned
    );
    assert!(d.engine.is_provisioned());
    assert!(!d.engine.has_pending_pairing().unwrap());
}

#[test]
fn p30_a_space_made_by_another_device_erases_the_pending_keys() {
    let w = World::new();
    let slow = w.device("windows");
    // This device's request never gets through; the other one creates the space first.
    w.relay
        .fail_next("post_space", RelayError::api(400, "bad_request"));
    assert!(slow.engine.create_space(TOKEN, "Lent").is_err());
    assert!(slow.engine.has_pending_pairing().unwrap());
    let _winner = w.founder("Rapide");
    // Its own try now meets `space_exists`, the challenge/response fails (401): not its space.
    assert_eq!(
        slow.engine.create_space(TOKEN, "Lent").unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "space_exists".into()
        }
    );
    assert!(!slow.engine.has_pending_pairing().unwrap());
    assert!(!slow.engine.is_provisioned());
}

#[test]
fn a_pending_founder_whose_space_the_relay_does_not_know_is_kept_by_resume_pairing() {
    let w = World::new();
    let hesitant = w.device("windows");
    // A wrong token: the relay has no space, and the keys are sealed for the next try.
    assert!(hesitant.engine.create_space("0000-0000", "X").is_err());
    assert!(hesitant.engine.has_pending_pairing().unwrap());
    // It may be that the request never left: nothing is decided, nothing is erased.
    assert_eq!(
        hesitant.engine.resume_pairing().unwrap(),
        ResumeOutcome::Nothing
    );
    assert!(hesitant.engine.has_pending_pairing().unwrap());
    hesitant.engine.create_space(TOKEN, "X").unwrap();
    assert!(hesitant.engine.is_provisioned());
}

#[test]
fn the_founder_then_joins_nothing_and_invites() {
    // The whole story from a fresh relay: found, invite twice, and the three devices talk.
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let (b, c) = (w.device("android"), w.device("android"));
    w.pair(&a, &b, "Pixel", false);
    w.pair(&b, &c, "Tablette", false);
    for d in [&a, &b, &c] {
        d.engine.sync().unwrap();
    }
    // B invited C: A learned C from the log, with the certificate B issued.
    assert!(a.knows(&c) && b.knows(&c) && c.knows(&a) && c.knows(&b));
    let id = send(&c, draft("de C", false));
    a.engine.sync().unwrap();
    b.engine.sync().unwrap();
    assert_eq!(plain_of(&a.engine, &id), "de C");
    assert_eq!(plain_of(&b.engine, &id), "de C");
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

#[test]
fn an_invitation_needs_a_provisioned_usable_device_and_the_network() {
    let w = World::new();
    let blank = w.device("android");
    assert_eq!(
        blank.engine.start_invitation().unwrap_err().code(),
        "invalid_input"
    );

    let a = w.founder("PC-Bureau");
    w.relay.set_offline(true);
    assert!(matches!(
        a.engine.start_invitation().unwrap_err(),
        CoreError::Network { .. }
    ));
    w.relay.set_offline(false);
    assert!(w.relay.channel_ids().is_empty());

    // A device the relay no longer knows as a member cannot invite.
    let live = a.engine.active().unwrap();
    a.engine
        .set_blocked(&live, crate::runtime::Blocked::Revoked);
    assert!(matches!(
        a.engine.start_invitation().unwrap_err(),
        CoreError::Blocked { .. }
    ));
}

#[test]
fn only_two_invitations_can_be_open_at_once() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let _one = a.engine.start_invitation().unwrap();
    let _two = a.engine.start_invitation().unwrap();
    assert_eq!(
        a.engine.start_invitation().unwrap_err(),
        CoreError::Relay {
            status: 409,
            code: "channel_limit".into()
        }
    );
}

#[test]
fn a_bad_code_is_refused_before_anything_is_claimed() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let code = inv.code().unwrap();
    for bad in [
        "",
        "0-crayon-renard",
        "4821-crayon",
        "4821-zzzzzz-renard",
        "http://relay.example.org/l/pair#4821-crayon-renard",
        "https://relay.example.org/other#4821-crayon-renard",
    ] {
        assert_eq!(
            n.engine.join_space(bad, "Pixel").unwrap_err().code(),
            "invalid_input",
            "{bad:?}"
        );
    }
    // Nor is the name of the device accepted if the relay would refuse it.
    assert!(n.engine.join_space(&code, "").is_err());
    assert!(n.engine.join_space(&code, "a\u{7}b").is_err());
    assert_eq!(
        w.relay.count("claim_pairing_channel"),
        0,
        "no claim was made"
    );
    assert_eq!(
        w.relay.channel_state(inv.channel_id()),
        Some(ChannelState::Open),
        "the code can still be used"
    );
    // A code for a channel that does not exist is claimed and found expired.
    let missing = if inv.channel_id() == "9999" {
        "1"
    } else {
        "9999"
    };
    assert_eq!(
        n.engine
            .join_space(&format!("{missing}-crayon-renard"), "Pixel")
            .unwrap_err(),
        CoreError::PairingExpired
    );
}

#[test]
fn a_provisioned_device_does_not_join_another_space() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let b = w.founder_of_a_second_relay();
    let inv = b.engine.start_invitation().unwrap();
    assert_eq!(
        a.engine
            .join_space(&inv.code().unwrap(), "Encore")
            .unwrap_err()
            .code(),
        "invalid_input"
    );
}

impl World {
    /// A device provisioned on another relay (so that the channel numbers do not collide).
    fn founder_of_a_second_relay(&self) -> Dev {
        let other = World::new();
        other.founder("Autre")
    }
}

// ---------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------

#[test]
fn p17_nothing_secret_is_in_clear_at_rest_or_in_a_debug_output() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let inv = a.engine.start_invitation().unwrap();
    let channel = inv.channel_id().to_owned();
    let code = inv.code().unwrap();
    let js = n.engine.join_space(&code, "Pixel").unwrap();
    let token = w.relay.channel_token(&channel).unwrap();
    run(&inv, &js, |a, n| {
        a == InvitePhase::Verify && n == JoinPhase::Verify
    });
    let sas = inv.prompt().unwrap().sas;
    let sas_plain = sas.replace(' ', "");
    inv.confirm().unwrap();
    js.confirm(false).unwrap();
    inv.poll(0).unwrap();
    js.poll(0).unwrap();
    assert_eq!(js.phase(), JoinPhase::Completing);

    // In the window where the new device holds a batch for a device that is not registered
    // yet: the batch is sealed, so its keys are in no file in the clear.
    let Stored::Found(lot) = n.engine.load_lot().unwrap() else {
        panic!("sealed")
    };
    let mut secrets: Vec<(&str, Vec<u8>)> = vec![
        ("device seed", lot.keys.ed25519_seed()),
        ("device x25519 secret", lot.keys.x25519_secret()),
        ("space key", lot.space_key.to_bytes()),
    ];
    for (what, bytes) in &secrets {
        assert!(
            holding(&n, bytes).is_empty(),
            "{what} in clear while pending"
        );
    }

    run(&inv, &js, |a, n| {
        a == InvitePhase::Done && n == JoinPhase::Done
    });
    secrets.push(("code", code.clone().into_bytes()));
    secrets.push(("sas", sas.clone().into_bytes()));
    secrets.push(("sas digits", sas_plain.clone().into_bytes()));
    secrets.push(("pairing token", token.clone().into_bytes()));
    secrets.push(("install token", TOKEN.as_bytes().to_vec()));
    let a_keys = a.engine.active().unwrap();
    secrets.push(("seed of A", a_keys.keys.ed25519_seed()));
    secrets.push(("x25519 secret of A", a_keys.keys.x25519_secret()));
    for dev in [&a, &n] {
        for (what, bytes) in &secrets {
            assert!(
                holding(dev, bytes).is_empty(),
                "{what} found in clear in {:?}",
                holding(dev, bytes)
            );
        }
    }
    // The relay saw seven opaque frames, of the sizes of the spec (§6.1), and none of the
    // secrets in them. (The pairing token is in a header; the code is in a URL fragment.)
    let bodies = w.relay.channel_bodies(&channel);
    let sizes: Vec<usize> = bodies.iter().map(Vec::len).collect();
    assert_eq!(sizes.len(), 7);
    assert_eq!(
        (sizes[0], sizes[1], sizes[4], sizes[6]),
        (50, 34, 22, 18),
        "{sizes:?}"
    );
    for body in &bodies {
        for (what, bytes) in &secrets {
            assert!(
                !body.windows(bytes.len()).any(|w| w == bytes.as_slice()),
                "{what} visible to the relay"
            );
        }
    }
    // Nor in what the types print.
    let printed = format!(
        "{inv:?} {js:?} {:?} {:?} {:?} {:?}",
        inv.wait_done().unwrap(),
        js.wait_done().unwrap(),
        PairingClaim {
            pairing_token: token.clone(),
            expires_at_ms: 0
        },
        PairingAuth::Joiner(&token),
    );
    for needle in [&code, &sas, &sas_plain, &token] {
        assert!(!printed.contains(needle.as_str()), "{needle} printed");
    }
    let prompt = format!(
        "{:?}",
        VerifyPrompt {
            sas: sas.clone(),
            peer: PairingPeer {
                name: "x".into(),
                platform: "y".into()
            }
        }
    );
    assert!(!prompt.contains(&sas));
    let mut words = code.split('-').skip(1);
    let (w1, w2) = (words.next().unwrap(), words.next().unwrap());
    assert!(!printed.contains(w1) && !printed.contains(w2));
    let request = format!(
        "{:?}",
        NewSpace {
            install_token: TOKEN.into(),
            space_id: "x".into(),
            name: "x".into(),
            platform: "x".into(),
            certificate: vec![]
        }
    );
    assert!(!request.contains(TOKEN));
}

// ---------------------------------------------------------------------------
// The double itself (api-v1 §11.10)
// ---------------------------------------------------------------------------

/// Two devices of a space and a raw transport for a third party.
struct Raw {
    w: World,
    a: Dev,
    b: Dev,
    outsider: Arc<MockTransport>,
}

fn raw() -> Raw {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let b = w.device("android");
    w.pair(&a, &b, "Tablette", false);
    let outsider = w.relay.transport();
    Raw { w, a, b, outsider }
}

#[test]
fn a100_a_channel_id_is_one_to_four_digits_and_open() {
    let r = raw();
    let c = r.a.engine.transport.create_pairing_channel().unwrap();
    assert_eq!(c.state, ChannelState::Open);
    assert!((1..=4).contains(&c.channel_id.len()));
    assert!(c.channel_id.bytes().all(|b| b.is_ascii_digit()) && !c.channel_id.starts_with('0'));
    // At most two channels are open at once.
    r.a.engine.transport.create_pairing_channel().unwrap();
    let err = r.b.engine.transport.create_pairing_channel().unwrap_err();
    assert_eq!(err.code(), Some("channel_limit"));
}

#[test]
fn a101_a_channel_is_claimed_once() {
    let r = raw();
    let tr = &r.a.engine.transport;
    let c = tr.create_pairing_channel().unwrap();
    let first = r.outsider.claim_pairing_channel(&c.channel_id).unwrap();
    assert!(first.pairing_token.starts_with("tmp1."));
    assert_eq!(
        r.outsider
            .claim_pairing_channel(&c.channel_id)
            .unwrap_err()
            .code(),
        Some("channel_claimed")
    );
    // The first arrived keeps the channel.
    tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, b"x")
        .unwrap();
    let got = r
        .outsider
        .pairing_messages(
            &c.channel_id,
            PairingAuth::Joiner(&first.pairing_token),
            0,
            0,
        )
        .unwrap();
    assert_eq!(got.state, ChannelState::Claimed);
    assert_eq!(got.messages.len(), 1);
    // A closed channel cannot be claimed; an unknown one is not found.
    let d = tr.create_pairing_channel().unwrap();
    tr.close_pairing_channel(&d.channel_id, PairingAuth::Inviter)
        .unwrap();
    assert_eq!(
        r.outsider
            .claim_pairing_channel(&d.channel_id)
            .unwrap_err()
            .code(),
        Some("channel_closed")
    );
    assert_eq!(
        r.outsider.claim_pairing_channel("9999").unwrap_err().code(),
        Some("not_found")
    );
}

#[test]
fn a102_each_side_reads_only_the_other_one_in_order_and_a_long_poll_wakes_up() {
    let r = raw();
    let tr = &r.a.engine.transport;
    let c = tr.create_pairing_channel().unwrap();
    let token = r
        .outsider
        .claim_pairing_channel(&c.channel_id)
        .unwrap()
        .pairing_token;
    let joiner = PairingAuth::Joiner(&token);
    assert_eq!(
        tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, b"a1")
            .unwrap(),
        1
    );
    assert_eq!(
        r.outsider
            .post_pairing_message(&c.channel_id, joiner, b"n2")
            .unwrap(),
        2
    );
    assert_eq!(
        tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, b"a3")
            .unwrap(),
        3
    );
    let seen_by_joiner = r
        .outsider
        .pairing_messages(&c.channel_id, joiner, 0, 0)
        .unwrap();
    let indexes: Vec<(u64, PairingSide)> = seen_by_joiner
        .messages
        .iter()
        .map(|m| (m.index, m.side))
        .collect();
    assert_eq!(
        indexes,
        vec![(1, PairingSide::Inviter), (3, PairingSide::Inviter)]
    );
    let seen_by_inviter = tr
        .pairing_messages(&c.channel_id, PairingAuth::Inviter, 0, 0)
        .unwrap();
    assert_eq!(seen_by_inviter.messages.len(), 1);
    assert_eq!(seen_by_inviter.messages[0].body, b"n2");
    // `after` skips what was read.
    assert!(r
        .outsider
        .pairing_messages(&c.channel_id, joiner, 3, 0)
        .unwrap()
        .messages
        .is_empty());

    // A long poll with nothing to return waits (here at most the cap of the double), and one
    // that is waiting wakes up as soon as a message arrives.
    r.w.relay.with(|s| s.long_poll_cap_ms = 3_000);
    let started = Instant::now();
    let empty = r
        .outsider
        .pairing_messages(&c.channel_id, joiner, 3, 0)
        .unwrap();
    assert!(empty.messages.is_empty() && started.elapsed() < Duration::from_millis(500));
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            let t = Instant::now();
            let got = r
                .outsider
                .pairing_messages(&c.channel_id, joiner, 3, 25)
                .unwrap();
            (got, t.elapsed())
        });
        std::thread::sleep(Duration::from_millis(100));
        tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, b"a4")
            .unwrap();
        let (got, took) = waiting.join().unwrap();
        assert_eq!(got.messages.len(), 1);
        assert!(
            took < Duration::from_millis(2_500),
            "woken by the message: {took:?}"
        );
    });
}

#[test]
fn a103_a_channel_nobody_claimed_is_gone_after_two_minutes() {
    let r = raw();
    let c = r.a.engine.transport.create_pairing_channel().unwrap();
    r.w.clock.advance(119_000);
    assert!(r.outsider.claim_pairing_channel(&c.channel_id).is_ok());
    let d = r.a.engine.transport.create_pairing_channel().unwrap();
    r.w.clock.advance(121_000);
    assert_eq!(
        r.outsider
            .claim_pairing_channel(&d.channel_id)
            .unwrap_err()
            .code(),
        Some("not_found")
    );
    // A claim prolongs the life of the channel by two minutes from the claim.
    let e = r.a.engine.transport.create_pairing_channel().unwrap();
    r.w.clock.advance(100_000);
    let t = r
        .outsider
        .claim_pairing_channel(&e.channel_id)
        .unwrap()
        .pairing_token;
    r.w.clock.advance(100_000);
    assert!(r
        .outsider
        .pairing_messages(&e.channel_id, PairingAuth::Joiner(&t), 0, 0)
        .is_ok());
    r.w.clock.advance(21_000);
    assert_eq!(
        r.outsider
            .pairing_messages(&e.channel_id, PairingAuth::Joiner(&t), 0, 0)
            .unwrap_err()
            .code(),
        Some("not_found")
    );
}

#[test]
fn a104_a_side_has_eight_messages_of_8192_bytes() {
    let r = raw();
    let tr = &r.a.engine.transport;
    let c = tr.create_pairing_channel().unwrap();
    for i in 0..8u8 {
        tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, &[i; 8192])
            .unwrap();
    }
    assert_eq!(
        tr.post_pairing_message(&c.channel_id, PairingAuth::Inviter, b"9th")
            .unwrap_err()
            .code(),
        Some("channel_limit")
    );
    let d = tr.create_pairing_channel().unwrap();
    let err = tr
        .post_pairing_message(&d.channel_id, PairingAuth::Inviter, &[0u8; 8193])
        .unwrap_err();
    assert_eq!(err.code(), Some("payload_too_large"));
    assert!(tr
        .post_pairing_message(&d.channel_id, PairingAuth::Inviter, &[])
        .is_err());
}

/// A channel of `inviter` that `outsider` claimed.
fn claimed_channel(r: &Raw, inviter: &Dev) -> String {
    let c = inviter.engine.transport.create_pairing_channel().unwrap();
    r.outsider.claim_pairing_channel(&c.channel_id).unwrap();
    c.channel_id
}

fn cert_for(issuer: &Dev, space_id: &str, subject: &DeviceKeys) -> Vec<u8> {
    issuer
        .engine
        .active()
        .unwrap()
        .keys
        .sign_device_cert(space_id, &subject.public(), 1)
        .unwrap()
        .to_bytes()
        .unwrap()
}

#[test]
fn a105_the_inviter_registers_the_device_and_the_new_device_opens_a_session() {
    let r = raw();
    let id = claimed_channel(&r, &r.a);
    let space_id = r.a.engine.space_id().unwrap();
    let newcomer = DeviceKeys::generate().unwrap();
    let head = r.w.relay.head();
    let record =
        r.a.engine
            .transport
            .post_device(&NewDevice {
                channel_id: id.clone(),
                key_epoch: 1,
                name: "Pixel".into(),
                platform: "android".into(),
                certificate: cert_for(&r.a, &space_id, &newcomer),
            })
            .unwrap();
    // The id is derived from the key (A106), the join point is the head before the addition,
    // the event is logged, and the channel is completed.
    assert_eq!(record.device_id, newcomer.device_id());
    assert_eq!(record.joined_seq, head);
    assert!(r.w.relay.events().iter().any(|e| matches!(
        &e.data,
        EventData::DeviceAdded { device } if device.device_id == record.device_id
    )));
    assert_eq!(r.w.relay.channel_state(&id), Some(ChannelState::Completed));
    // The new device authenticates with its keys, like any member.
    let tr = r.w.relay.transport();
    let ch = tr.auth_challenge(&record.device_id).unwrap();
    let grant = tr
        .auth_session(&SessionRequest {
            device_id: record.device_id.clone(),
            signature: newcomer.sign_relay_auth(&space_id, &ch.challenge).unwrap(),
            challenge: ch.challenge,
            client: ClientInfo {
                format_min: 1,
                format_max: 1,
                app_version: "t".into(),
                platform: "android".into(),
            },
        })
        .unwrap();
    assert!(grant.token.starts_with("nvs1."));
    // A completed channel takes no message but can still be read.
    assert_eq!(
        r.a.engine
            .transport
            .post_pairing_message(&id, PairingAuth::Inviter, b"late")
            .unwrap_err()
            .code(),
        Some("channel_closed")
    );
    assert_eq!(
        r.a.engine
            .transport
            .pairing_messages(&id, PairingAuth::Inviter, 0, 0)
            .unwrap()
            .state,
        ChannelState::Completed
    );
}

#[test]
fn a106_the_registration_is_refused_in_the_cases_of_the_spec() {
    let r = raw();
    let space_id = r.a.engine.space_id().unwrap();
    let id = claimed_channel(&r, &r.a);
    let newcomer = DeviceKeys::generate().unwrap();
    let request = |channel: &str, epoch: u32, cert: Vec<u8>| NewDevice {
        channel_id: channel.into(),
        key_epoch: epoch,
        name: "Pixel".into(),
        platform: "android".into(),
        certificate: cert,
    };
    let a_tr = &r.a.engine.transport;
    // A certificate issued by another member than the caller: 422.
    let by_b = cert_for(&r.b, &space_id, &newcomer);
    assert_eq!(
        a_tr.post_device(&request(&id, 1, by_b)).unwrap_err().code(),
        Some("validation_failed")
    );
    // A self-signed one, a damaged one, one for another space: 422.
    let own = {
        let k = &r.a.engine.active().unwrap().keys;
        k.sign_device_cert(&space_id, &k.public(), 1)
            .unwrap()
            .to_bytes()
            .unwrap()
    };
    for cert in [own, vec![1, 2, 3]] {
        assert_eq!(
            a_tr.post_device(&request(&id, 1, cert)).unwrap_err().code(),
            Some("validation_failed")
        );
    }
    let foreign = cert_for(&r.a, &crate::new_space_id().unwrap(), &newcomer);
    assert_eq!(
        a_tr.post_device(&request(&id, 1, foreign))
            .unwrap_err()
            .code(),
        Some("validation_failed")
    );
    // Another epoch than the current one: 409 epoch_conflict, with the current epoch.
    let good = cert_for(&r.a, &space_id, &newcomer);
    let RelayError::Api(p) = a_tr
        .post_device(&request(&id, 2, good.clone()))
        .unwrap_err()
    else {
        panic!()
    };
    assert_eq!((p.code.as_str(), p.key_epoch), ("epoch_conflict", Some(1)));
    // A channel another member created: 404. A channel nobody claimed: 409.
    assert_eq!(
        r.b.engine
            .transport
            .post_device(&request(&id, 1, good.clone()))
            .unwrap_err()
            .code(),
        Some("not_found")
    );
    let open = a_tr.create_pairing_channel().unwrap();
    assert_eq!(
        a_tr.post_device(&request(&open.channel_id, 1, good.clone()))
            .unwrap_err()
            .code(),
        Some("channel_closed")
    );
    // A device that is already a member: 409 device_exists.
    let known = cert_for(&r.a, &space_id, &r.b.engine.active().unwrap().keys);
    assert_eq!(
        a_tr.post_device(&request(&id, 1, known))
            .unwrap_err()
            .code(),
        Some("device_exists")
    );
    // A full space: 409 too_many_devices.
    r.w.relay.with(|s| s.limits.max_devices = 2);
    assert_eq!(
        a_tr.post_device(&request(&id, 1, good)).unwrap_err().code(),
        Some("too_many_devices")
    );
    // Nothing was registered by any of these.
    assert_eq!(a_tr.devices().unwrap().len(), 2);
    assert_eq!(r.w.relay.channel_state(&id), Some(ChannelState::Claimed));
}

#[test]
fn a107_a_pairing_token_is_not_a_session() {
    let r = raw();
    let id = claimed_channel(&r, &r.a);
    let token = r.w.relay.channel_token(&id).unwrap();
    let tr = r.w.relay.transport();
    tr.set_session_token(Some(token.clone()));
    assert_eq!(tr.events(0, 10).unwrap_err().code(), Some("unauthorized"));
    assert_eq!(tr.devices().unwrap_err().code(), Some("unauthorized"));
    // And a session is not a pairing token, nor is the session of another device the creator's.
    assert_eq!(
        r.b.engine
            .transport
            .pairing_messages(&id, PairingAuth::Inviter, 0, 0)
            .unwrap_err()
            .code(),
        Some("not_found")
    );
    assert_eq!(
        r.outsider
            .pairing_messages(&id, PairingAuth::Joiner("tmp1.nope"), 0, 0)
            .unwrap_err()
            .code(),
        Some("unauthorized")
    );
    assert_eq!(
        r.outsider
            .post_pairing_message(&id, PairingAuth::Inviter, b"x")
            .unwrap_err()
            .code(),
        Some("unauthorized")
    );
}

#[test]
fn closing_a_channel_erases_its_messages_and_is_repeatable() {
    let r = raw();
    let tr = &r.a.engine.transport;
    let id = claimed_channel(&r, &r.a);
    tr.post_pairing_message(&id, PairingAuth::Inviter, b"x")
        .unwrap();
    assert_eq!(r.w.relay.channel_messages(&id), Some((1, 0)));
    tr.close_pairing_channel(&id, PairingAuth::Inviter).unwrap();
    tr.close_pairing_channel(&id, PairingAuth::Inviter).unwrap();
    assert_eq!(r.w.relay.channel_messages(&id), Some((0, 0)));
    assert_eq!(r.w.relay.channel_state(&id), Some(ChannelState::Closed));
    // The other side can read the state, and close it too.
    let token = r.w.relay.channel_token(&id).unwrap();
    let got = r
        .outsider
        .pairing_messages(&id, PairingAuth::Joiner(&token), 0, 0)
        .unwrap();
    assert_eq!((got.state, got.messages.len()), (ChannelState::Closed, 0));
    r.outsider
        .close_pairing_channel(&id, PairingAuth::Joiner(&token))
        .unwrap();
}

// ---------------------------------------------------------------------------
// A claim refused with 429 is retried once
// ---------------------------------------------------------------------------

fn rate_limited_claim() -> RelayError {
    RelayError::problem(ApiProblem {
        status: 429,
        code: "rate_limited".into(),
        retry_after_secs: Some(1),
        ..Default::default()
    })
}

#[test]
fn a_claim_refused_once_with_429_is_retried_and_succeeds() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let code = a.engine.start_invitation().unwrap().code().unwrap();
    w.relay
        .fail_next("claim_pairing_channel", rate_limited_claim());
    assert!(n.engine.join_space(&code, "Pixel").is_ok());
}

#[test]
fn a_claim_refused_twice_with_429_gives_rate_limited_and_the_code_is_not_spent() {
    let w = World::new();
    let a = w.founder("PC-Bureau");
    let n = w.device("android");
    let code = a.engine.start_invitation().unwrap().code().unwrap();
    w.relay
        .fail_next("claim_pairing_channel", rate_limited_claim());
    w.relay
        .fail_next("claim_pairing_channel", rate_limited_claim());
    assert_eq!(
        n.engine.join_space(&code, "Pixel").unwrap_err(),
        CoreError::Relay {
            status: 429,
            code: "rate_limited".into()
        }
    );
    // Nobody claimed the channel: the same code still works.
    assert!(n.engine.join_space(&code, "Pixel").is_ok());
}

// ---------------------------------------------------------------------------
// The relay of the device: from a link, a field or the build (`pairing-v1.md` §4.4, §10)
// ---------------------------------------------------------------------------

mod relay_choice {
    use super::*;

    /// The relay the double answers to.
    const HERE: &str = "https://relay.example.org";
    /// Another relay: the double does not answer there.
    const ELSEWHERE: &str = "https://other.example.net";

    /// A device that is not provisioned, with the relay `transport_url` and the build default
    /// `default_relay_url`.
    fn open_at(
        w: &World,
        dir: tempfile::TempDir,
        transport_url: Option<&str>,
        default_relay_url: Option<&str>,
    ) -> Dev {
        let cfg = EngineConfig {
            platform: "android".into(),
            default_relay_url: default_relay_url.map(str::to_owned),
            ..EngineConfig::default()
        };
        let store = Store::open(
            &dir.path().join("tomi.db"),
            &dir.path().join("files"),
            &WRAP,
        )
        .unwrap();
        let engine = Engine::open(
            cfg.clone(),
            store,
            w.relay.transport_at(transport_url),
            w.clock.clone(),
        )
        .unwrap();
        Dev {
            engine: Arc::new(engine),
            dir,
            cfg,
        }
    }

    fn device_at(w: &World, url: Option<&str>, default: Option<&str>) -> Dev {
        open_at(w, tempfile::tempdir().unwrap(), url, default)
    }

    /// The app stops and starts again on the same files, with a new transport.
    fn restart_at(w: &World, d: Dev, url: Option<&str>) -> Dev {
        let Dev { engine, dir, cfg } = d;
        drop(engine);
        open_at(w, dir, url, cfg.default_relay_url.clone().as_deref())
    }

    fn kept(d: &Dev) -> Option<String> {
        d.engine
            .store
            .read(|db| db.meta_get_string("relay_url"))
            .unwrap()
    }

    fn pending(d: &Dev) -> Option<String> {
        d.engine
            .store
            .read(|db| db.meta_get_string("pending_relay_url"))
            .unwrap()
    }

    fn done(inv: &InviteSession, js: &JoinSession) {
        run(inv, js, |a, n| {
            a == InvitePhase::Done && n == JoinPhase::Done
        });
    }

    #[test]
    fn the_relay_of_a_link_is_shown_kept_only_once_paired_and_never_changes() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        // The build says another relay: the link wins for this pairing only.
        let n = device_at(&w, Some(ELSEWHERE), Some(ELSEWHERE));
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));
        let inv = a.engine.start_invitation().unwrap();
        let link = inv.link().unwrap();
        assert_eq!(link, format!("{HERE}/l/pair#{}", inv.code().unwrap()));
        let js = n.engine.join_space(&link, "Pixel").unwrap();
        assert_eq!(js.relay_authority(), "relay.example.org");
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(kept(&n), None, "nothing is kept before the pairing worked");
        compare_and_confirm(&inv, &js, false);
        assert_eq!(js.prompt().unwrap().relay_authority, "relay.example.org");
        assert_eq!(kept(&n), None, "not even once the user compared the codes");
        done(&inv, &js);
        assert!(n.engine.is_provisioned());
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(pending(&n), None);
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        // The relay of the space is the device own, whatever the build says at the next start.
        let n = restart_at(&w, n, Some(ELSEWHERE));
        assert!(n.engine.is_provisioned());
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        // ... and nothing can change it.
        assert!(matches!(
            n.engine.set_relay("https://third.example.net"),
            Err(CoreError::InvalidInput { .. })
        ));
        assert!(matches!(
            n.engine
                .join_space_at("1-crayon-renard", "X", Some("https://third.example.net")),
            Err(CoreError::InvalidInput { .. })
        ));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        let id = send(&a, draft("apres", false));
        n.engine.sync().unwrap();
        assert_eq!(plain_of(&n.engine, &id), "apres");
    }

    #[test]
    fn a_cancelled_or_failed_pairing_puts_the_previous_relay_back() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, Some(ELSEWHERE), Some(ELSEWHERE));
        let inv = a.engine.start_invitation().unwrap();
        let link = inv.link().unwrap();

        // Cancelled by the user.
        let js = n.engine.join_space(&link, "Pixel").unwrap();
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        js.cancel();
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));
        assert_eq!((kept(&n), pending(&n)), (None, None));
        inv.cancel();

        // A code that does not exist at that relay: expired.
        let err = n
            .engine
            .join_space(&format!("{HERE}/l/pair#9999-crayon-renard"), "Pixel")
            .unwrap_err();
        assert_eq!(err, CoreError::PairingExpired);
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));

        // A relay that does not answer.
        let err = n
            .engine
            .join_space("https://nowhere.example.net/l/pair#4821-crayon-renard", "P")
            .unwrap_err();
        assert!(matches!(err, CoreError::Network { .. }), "{err:?}");
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));

        // A session that is dropped without ending: same.
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.link().unwrap(), "Pixel").unwrap();
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        drop(js);
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));
        assert_eq!(kept(&n), None);
    }

    #[test]
    fn a_device_without_a_relay_asks_for_one_and_sends_nothing() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        assert_eq!(n.engine.relay_url(), None);
        let inv = a.engine.start_invitation().unwrap();
        let code = inv.code().unwrap();
        let calls = w.relay.with(|s| s.calls.len());
        let err = n.engine.join_space(&code, "Pixel").unwrap_err();
        assert_eq!(err, CoreError::RelayNotConfigured);
        assert_eq!(err.code(), "relay_not_configured");
        // The `tomi-share://` form names no relay either.
        let err = n
            .engine
            .join_space(&format!("tomi-share://pair#{code}"), "Pixel")
            .unwrap_err();
        assert_eq!(err, CoreError::RelayNotConfigured);
        let err = n.engine.create_space(TOKEN, "Pixel").unwrap_err();
        assert_eq!(err, CoreError::RelayNotConfigured);
        assert_eq!(w.relay.with(|s| s.calls.len()), calls, "nothing was sent");
        assert_eq!(n.engine.relay_url(), None);

        // The relay typed next to the code is enough.
        let js = n.engine.join_space_at(&code, "Pixel", Some(HERE)).unwrap();
        assert_eq!(js.relay_authority(), "relay.example.org");
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        assert_eq!(kept(&n).as_deref(), Some(HERE));
    }

    #[test]
    fn a_link_and_a_field_that_disagree_are_refused_before_anything_happens() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        let link = inv.link().unwrap();
        let claims = w.relay.count("claim_pairing_channel");
        let err = n
            .engine
            .join_space_at(&link, "Pixel", Some(ELSEWHERE))
            .unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput { .. }), "{err:?}");
        assert_eq!(w.relay.count("claim_pairing_channel"), claims);
        // The same relay, written differently, agrees.
        let js = n
            .engine
            .join_space_at(&link, "Pixel", Some("https://Relay.Example.org:443/"))
            .unwrap();
        assert_eq!(js.relay_authority(), "relay.example.org");
        js.cancel();
    }

    #[test]
    fn an_invalid_candidate_is_refused_and_nothing_is_consumed_or_switched() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, Some(HERE), None);
        let inv = a.engine.start_invitation().unwrap();
        let code = inv.code().unwrap();
        let claims = w.relay.count("claim_pairing_channel");
        for bad in [
            "http://relay.example.org",
            "ftp://relay.example.org",
            "relay.example.org",
            "https://user:pw@relay.example.org",
            "https://relay.example.org@evil.example",
            "https://relay.example.org?x=1",
            "https://relay.example.org#frag",
            "https://relay.example.org:0",
            "https://relay.example.org:99999",
            "https://r\u{435}lay.example.org",
        ] {
            let err = n
                .engine
                .join_space_at(&code, "Pixel", Some(bad))
                .unwrap_err();
            assert!(
                matches!(err, CoreError::InvalidInput { .. }),
                "{bad}: {err:?}"
            );
            assert!(n.engine.set_relay(bad).is_err(), "{bad}");
            assert!(n.engine.create_space_at(TOKEN, "Pixel", Some(bad)).is_err());
        }
        assert_eq!(w.relay.count("claim_pairing_channel"), claims);
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(kept(&n), None);
        // The code was not consumed.
        assert!(n.engine.join_space(&code, "Pixel").is_ok());
    }

    #[test]
    fn a_relay_chosen_with_set_relay_survives_a_restart() {
        let w = World::new();
        let n = device_at(&w, None, None);
        n.engine.set_relay("https://Relay.Example.org/").unwrap();
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        let n = restart_at(&w, n, None);
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        // A default of the build does not override what the user chose.
        let n = restart_at(&w, n, Some(ELSEWHERE));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        // Chosen again, it replaces.
        n.engine.set_relay(ELSEWHERE).unwrap();
        assert_eq!(kept(&n).as_deref(), Some(ELSEWHERE));
    }

    #[test]
    fn a_session_under_way_holds_the_relay() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, Some(HERE), None);
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
        assert!(matches!(
            n.engine.set_relay(ELSEWHERE),
            Err(CoreError::InvalidInput { .. })
        ));
        // Another relay for a second session is refused too.
        let inv2 = a.engine.start_invitation().unwrap();
        assert!(matches!(
            n.engine
                .join_space_at(&inv2.code().unwrap(), "P", Some(ELSEWHERE)),
            Err(CoreError::InvalidInput { .. })
        ));
        js.cancel();
        n.engine.set_relay(ELSEWHERE).unwrap();
    }

    #[test]
    fn the_founder_keeps_the_relay_of_a_creation_link_once_the_space_exists() {
        let w = World::new();
        let n = device_at(&w, Some(ELSEWHERE), Some(ELSEWHERE));
        // Nothing answers at the other relay: the call fails, the relay is back.
        let err = n.engine.create_space(TOKEN, "PC").unwrap_err();
        assert!(matches!(err, CoreError::Network { .. }), "{err:?}");
        n.engine.create_space_at(TOKEN, "PC", Some(HERE)).unwrap();
        assert!(n.engine.is_provisioned());
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(pending(&n), None);
        let n = restart_at(&w, n, Some(ELSEWHERE));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        n.engine.sync().unwrap();
    }

    #[test]
    fn a_creation_that_met_the_wrong_token_keeps_its_relay_with_its_sealed_keys() {
        let w = World::new();
        let n = device_at(&w, None, None);
        let err = n
            .engine
            .create_space_at("0000-0000", "PC", Some(HERE))
            .unwrap_err();
        assert_eq!(
            err,
            CoreError::Relay {
                status: 403,
                code: "install_token_invalid".into()
            }
        );
        // The keys are sealed for the next try, and the relay they belong to with them.
        assert!(n.engine.has_pending_pairing().unwrap());
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(pending(&n).as_deref(), Some(HERE));
        assert_eq!(kept(&n), None);
        // A stop does not lose it.
        let n = restart_at(&w, n, None);
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        n.engine.create_space(TOKEN, "PC").unwrap();
        assert!(n.engine.is_provisioned());
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(pending(&n), None);
    }

    #[test]
    fn keys_sealed_for_another_relay_are_settled_before_a_creation_elsewhere() {
        let w = World::new();
        let n = device_at(&w, None, None);
        assert!(n
            .engine
            .create_space_at("0000-0000", "PC", Some(HERE))
            .is_err());
        assert_eq!(pending(&n).as_deref(), Some(HERE));
        // The user tries another relay, which does not answer: the old keys are settled with
        // their relay (it did not know the device) and dropped, the call fails on the network.
        let err = n
            .engine
            .create_space_at(TOKEN, "PC", Some(ELSEWHERE))
            .unwrap_err();
        assert!(matches!(err, CoreError::Network { .. }), "{err:?}");
        // Back at the first one: nothing of the earlier attempt is left to confuse it.
        n.engine.create_space_at(TOKEN, "PC", Some(HERE)).unwrap();
        assert!(n.engine.is_provisioned());
    }

    #[test]
    fn a_sealed_batch_keeps_its_relay_over_a_stop_and_gives_it_up_when_discarded() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.link().unwrap(), "Pixel").unwrap();
        up_to_the_acknowledgement(&inv, &js);
        assert_eq!(pending(&n).as_deref(), Some(HERE));
        assert_eq!(kept(&n), None);
        // The inviter gives up and the app is killed before it learns it.
        inv.cancel();
        drop(js);
        let n = restart_at(&w, n, None);
        assert_eq!(
            n.engine.relay_url().as_deref(),
            Some(HERE),
            "the relay of the sealed batch"
        );
        assert_eq!(n.engine.resume_pairing().unwrap(), ResumeOutcome::Discarded);
        assert_eq!(n.engine.relay_url(), None, "back to what the device keeps");
        assert_eq!((kept(&n), pending(&n)), (None, None));
    }

    #[test]
    fn a_sealed_batch_that_the_inviter_registered_provisions_at_its_relay_after_a_stop() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.link().unwrap(), "Pixel").unwrap();
        up_to_the_acknowledgement(&inv, &js);
        assert_eq!(inv.poll(0).unwrap(), InvitePhase::Done);
        drop(js);
        let n = restart_at(&w, n, None);
        assert_eq!(
            n.engine.resume_pairing().unwrap(),
            ResumeOutcome::Provisioned
        );
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(pending(&n), None);
    }

    #[test]
    fn leaving_the_space_forgets_its_relay() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, Some(ELSEWHERE));
        let inv = a.engine.start_invitation().unwrap();
        let js = n
            .engine
            .join_space_at(&inv.code().unwrap(), "Pixel", Some(HERE))
            .unwrap();
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        n.engine.leave_space().unwrap();
        assert!(!n.engine.is_provisioned());
        assert_eq!(kept(&n), None);
        assert_eq!(n.engine.relay_url().as_deref(), Some(ELSEWHERE));
    }

    // --- the gate: decisions about the relay are atomic ----------------------------------------

    use crate::runtime::relay::RelayHold;
    use std::sync::Barrier;

    const ONE: &str = "https://one.example.net";
    const TWO: &str = "https://two.example.net";

    fn not_ok<T>(r: Result<T>) -> bool {
        matches!(r.err(), Some(CoreError::InvalidInput { .. }))
    }

    #[test]
    fn two_sessions_at_two_relays_never_overlap() {
        let w = World::new();
        let n = device_at(&w, Some(HERE), None);
        for round in 0..100 {
            let barrier = Arc::new(Barrier::new(2));
            let go = |url: &'static str| {
                let (e, b) = (n.engine.clone(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    RelayHold::begin(e, Some(url), None).ok()
                })
            };
            let (t1, t2) = (go(ONE), go(TWO));
            let (h1, h2) = (t1.join().unwrap(), t2.join().unwrap());
            let (winner, url) = match (&h1, &h2) {
                (Some(_), None) => (1, ONE),
                (None, Some(_)) => (2, TWO),
                _ => panic!("round {round}: exactly one session may switch the relay"),
            };
            // The transport is where the session that holds the relay put it: the other one did
            // not take it away, and its claim would not leave for the wrong relay.
            assert_eq!(
                n.engine.relay_url().as_deref(),
                Some(url),
                "winner {winner}"
            );
            assert_eq!(n.engine.relay_sessions(), 1);
            drop((h1, h2));
            assert_eq!(n.engine.relay_sessions(), 0);
            assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        }
    }

    #[test]
    fn set_relay_cannot_move_the_relay_under_a_session_that_starts() {
        let w = World::new();
        for round in 0..40 {
            let n = device_at(&w, Some(HERE), None);
            let barrier = Arc::new(Barrier::new(2));
            let (e, b) = (n.engine.clone(), barrier.clone());
            let setter = std::thread::spawn(move || {
                b.wait();
                e.set_relay(ONE)
            });
            let (e, b) = (n.engine.clone(), barrier.clone());
            let starter = std::thread::spawn(move || {
                b.wait();
                RelayHold::begin(e, Some(TWO), None)
            });
            let set = setter.join().unwrap();
            let hold = starter.join().unwrap();
            let hold = match hold {
                Ok(h) => h,
                Err(e) => panic!("round {round}: the session has no reason to fail: {e:?}"),
            };
            // Either the relay was chosen first (and the session switched from it), or the
            // session got there first and the choice was refused; never both on the transport.
            assert_eq!(n.engine.relay_url().as_deref(), Some(TWO), "round {round}");
            if set.is_err() {
                assert!(matches!(set, Err(CoreError::InvalidInput { .. })));
                assert_eq!(kept(&n), None);
            } else {
                assert_eq!(kept(&n).as_deref(), Some(ONE));
            }
            drop(hold);
            let back = if set.is_ok() { ONE } else { HERE };
            assert_eq!(n.engine.relay_url().as_deref(), Some(back), "round {round}");
        }
    }

    #[test]
    fn a_session_is_refused_on_a_device_that_became_provisioned() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        // This caller read "not provisioned" and was held up; meanwhile the device was paired.
        let js = n
            .engine
            .join_space_at(&inv.code().unwrap(), "Pixel", Some(HERE))
            .unwrap();
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        assert!(n.engine.is_provisioned());
        let calls = w.relay.with(|s| s.calls.len());
        assert!(not_ok(RelayHold::begin(n.engine.clone(), Some(ONE), None)));
        assert!(not_ok(RelayHold::begin(n.engine.clone(), None, Some(ONE))));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE), "not moved");
        assert_eq!(n.engine.relay_sessions(), 0, "not counted");
        assert_eq!(w.relay.with(|s| s.calls.len()), calls);
        // The session token of the device was not thrown away either: it syncs as before.
        let id = send(&a, draft("apres", false));
        n.engine.sync().unwrap();
        assert_eq!(plain_of(&n.engine, &id), "apres");
    }

    #[test]
    fn a_session_cannot_move_the_relay_away_from_a_sealed_batch_that_is_still_there() {
        let w = World::new();
        let n = device_at(&w, Some(HERE), None);
        // A batch sealed by a session that just ended: it belongs to the relay the transport
        // points at, and moving the transport would have it probed (and dropped) elsewhere.
        n.engine
            .store
            .tx(|db| db.secret_put("pairing_pending", b"sealed"))
            .unwrap();
        assert!(not_ok(RelayHold::begin(n.engine.clone(), Some(ONE), None)));
        assert!(not_ok(RelayHold::begin(n.engine.clone(), None, Some(ONE))));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE), "not moved");
        assert_eq!(n.engine.relay_sessions(), 0, "not counted");
        // The relay it belongs to is still usable.
        let hold = RelayHold::begin(n.engine.clone(), Some(HERE), None).unwrap();
        assert_eq!(n.engine.relay_sessions(), 1);
        drop(hold);
    }

    #[test]
    fn a_creation_during_a_joining_does_not_move_the_relay_of_the_joining() {
        let w = World::new();
        let n = device_at(&w, Some(HERE), None);
        // Keys sealed by an earlier creation (the token was wrong); as far as the device knows
        // they belong to another relay than the one of the joining.
        assert!(n
            .engine
            .create_space_at("0000-0000", "PC", Some(HERE))
            .is_err());
        n.engine
            .store
            .tx(|db| db.meta_set("pending_relay_url", ELSEWHERE.as_bytes()))
            .unwrap();
        let a = w.founder("PC-Bureau");
        let inv = a.engine.start_invitation().unwrap();
        let js = n.engine.join_space(&inv.code().unwrap(), "Pixel").unwrap();
        assert_eq!(n.engine.relay_sessions(), 1);
        let (challenges, posts) = (w.relay.count("auth_challenge"), w.relay.count("post_space"));
        // A creation arrives during the joining: it would have to go and ask the other relay.
        let err = n.engine.create_space_at(TOKEN, "PC", None).unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput { .. }), "{err:?}");
        // The relay of the joining did not move, the keys were not touched, nothing was sent.
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(
            n.engine.relay_sessions(),
            1,
            "the creation gave its hold back"
        );
        assert_eq!(pending(&n).as_deref(), Some(ELSEWHERE));
        assert!(n.engine.has_pending_pairing().unwrap());
        assert_eq!(w.relay.count("auth_challenge"), challenges);
        assert_eq!(w.relay.count("post_space"), posts);
        // And the joining goes on at its relay.
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(n.engine.relay_sessions(), 0);
    }

    #[test]
    fn a_session_is_given_back_even_when_the_thread_panics() {
        let w = World::new();
        let n = device_at(&w, Some(HERE), None);
        let e = n.engine.clone();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _hold = RelayHold::begin(e, Some(ONE), None).ok().unwrap();
            assert_eq!(n.engine.relay_url().as_deref(), Some(ONE));
            panic!("between the start of the session and the session");
        }));
        assert!(panicked.is_err());
        assert_eq!(n.engine.relay_sessions(), 0);
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        // The relay is free again.
        n.engine.set_relay(TWO).unwrap();
        // A hold that is released, then dropped, is given back once.
        let hold = RelayHold::begin(n.engine.clone(), None, None).ok().unwrap();
        assert_eq!(n.engine.relay_sessions(), 1);
        hold.release();
        hold.release();
        assert_eq!(n.engine.relay_sessions(), 0);
        drop(hold);
        assert_eq!(n.engine.relay_sessions(), 0);
    }

    // --- a provisioned device that keeps no relay ----------------------------------------------

    #[test]
    fn a_device_paired_before_the_relay_was_kept_gets_one_once() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        let js = n
            .engine
            .join_space_at(&inv.code().unwrap(), "Pixel", Some(HERE))
            .unwrap();
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        // An older version did not keep it, and this build has no default: the device has none.
        n.engine.store.tx(|db| db.meta_delete("relay_url")).unwrap();
        let n = restart_at(&w, n, None);
        assert!(n.engine.is_provisioned());
        assert_eq!(n.engine.relay_url(), None);
        assert_eq!(kept(&n), None);
        // It may choose one, which is then the relay of its space.
        n.engine.set_relay("https://Relay.Example.org/").unwrap();
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        let id = send(&a, draft("apres", false));
        n.engine.sync().unwrap();
        assert_eq!(plain_of(&n.engine, &id), "apres");
        // Once: now it has one.
        assert!(matches!(
            n.engine.set_relay(ELSEWHERE),
            Err(CoreError::InvalidInput { .. })
        ));
        assert!(matches!(
            n.engine.set_relay(HERE),
            Err(CoreError::InvalidInput { .. })
        ));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        let n = restart_at(&w, n, Some(ELSEWHERE));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(kept(&n).as_deref(), Some(HERE));
    }

    #[test]
    fn a_device_paired_with_its_relay_never_changes_it() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, None, None);
        let inv = a.engine.start_invitation().unwrap();
        let js = n
            .engine
            .join_space_at(&inv.code().unwrap(), "Pixel", Some(HERE))
            .unwrap();
        compare_and_confirm(&inv, &js, false);
        done(&inv, &js);
        for url in [ELSEWHERE, HERE, ONE] {
            assert!(matches!(
                n.engine.set_relay(url),
                Err(CoreError::InvalidInput { .. })
            ));
        }
        assert_eq!(kept(&n).as_deref(), Some(HERE));
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
    }

    #[test]
    fn a_device_without_a_relay_and_not_paired_still_chooses_one_as_before() {
        let w = World::new();
        let n = device_at(&w, None, None);
        n.engine.set_relay(HERE).unwrap();
        // ... and still not while a pairing is pending.
        assert!(n
            .engine
            .create_space_at("0000-0000", "PC", Some(HERE))
            .is_err());
        assert!(n.engine.has_pending_pairing().unwrap());
        assert!(matches!(
            n.engine.set_relay(ELSEWHERE),
            Err(CoreError::InvalidInput { .. })
        ));
    }

    #[test]
    fn a_port_with_a_sign_or_a_leading_zero_is_not_a_relay() {
        let w = World::new();
        let a = w.founder("PC-Bureau");
        let n = device_at(&w, Some(HERE), None);
        let code = a.engine.start_invitation().unwrap().code().unwrap();
        for bad in [
            "https://relay.example.org:+8443",
            "https://relay.example.org:0443",
            "https://relay.example.org:-1",
        ] {
            assert!(n.engine.set_relay(bad).is_err(), "{bad}");
            assert!(n.engine.join_space_at(&code, "P", Some(bad)).is_err());
            assert!(n.engine.create_space_at(TOKEN, "P", Some(bad)).is_err());
            let link = format!("{bad}/l/pair#{code}");
            assert!(n.engine.join_space(&link, "P").is_err(), "{link}");
        }
        assert_eq!(n.engine.relay_url().as_deref(), Some(HERE));
        assert_eq!(n.engine.relay_sessions(), 0);
        // What is shown is what is used.
        let js = n
            .engine
            .join_space_at(&code, "P", Some("https://Relay.Example.org:443"))
            .unwrap();
        assert_eq!(js.relay_authority(), "relay.example.org");
        js.cancel();
    }
}
