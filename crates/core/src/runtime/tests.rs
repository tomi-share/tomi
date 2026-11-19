//! Engine tests against the in-memory relay double and temporary SQLite files.
//!
//! Nothing here talks to a real relay: the double implements `RelayTransport`.

use std::sync::Arc;

use super::mock::{MockRelay, TestClock};
use super::transport::*;
use super::*;
use crate::{
    new_space_id, DeviceKeys, Item, ReadPolicy, SourceFormat, SpaceKey, TrustStore,
    UnreadableReason,
};

const T0: i64 = 1_793_836_800_000;

pub(crate) struct Node {
    pub(crate) engine: Arc<Engine>,
    id: String,
    seed: Vec<u8>,
    x: Vec<u8>,
    dir: tempfile::TempDir,
    cert: Vec<u8>,
    public: crate::DevicePublic,
}

impl Node {
    fn keys(&self) -> DeviceKeys {
        DeviceKeys::from_secrets(&self.seed, &self.x).unwrap()
    }
    fn db_path(&self) -> std::path::PathBuf {
        self.dir.path().join("tomi.db")
    }
    fn reopen(&self, relay: &MockRelay, clock: Arc<TestClock>) -> Engine {
        let store =
            Store::open(&self.db_path(), &self.dir.path().join("files"), &[9u8; 32]).unwrap();
        Engine::open(EngineConfig::default(), store, relay.transport(), clock).unwrap()
    }
}

pub(crate) struct World {
    pub(crate) relay: MockRelay,
    clock: Arc<TestClock>,
    space_id: String,
    pub(crate) nodes: Vec<Node>,
}

impl World {
    /// `n` provisioned devices that all know each other; the first `registered` are
    /// registered on the relay now, the others with [`join`](Self::join).
    pub(crate) fn new(n: usize, registered: usize) -> Self {
        let clock = TestClock::new(T0);
        let space_id = new_space_id().unwrap();
        let relay = MockRelay::new(&space_id, clock.clone());
        let space_key = SpaceKey::generate(1).unwrap();
        let all: Vec<DeviceKeys> = (0..n).map(|_| DeviceKeys::generate().unwrap()).collect();
        let founder =
            DeviceKeys::from_secrets(&all[0].ed25519_seed(), &all[0].x25519_secret()).unwrap();
        let mut nodes = Vec::new();
        for (i, k) in all.iter().enumerate() {
            let mut trust = TrustStore::new(&space_id, &k.public(), 1).unwrap();
            for (j, other) in all.iter().enumerate() {
                if i != j {
                    trust.add_verified_peer(&other.public()).unwrap();
                }
            }
            let cert = founder
                .sign_device_cert(&space_id, &k.public(), T0 as u64)
                .unwrap()
                .to_bytes()
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(
                &dir.path().join("tomi.db"),
                &dir.path().join("files"),
                &[9u8; 32],
            )
            .unwrap();
            let transport = relay.transport();
            let engine = Engine::open(
                EngineConfig {
                    platform: "android".into(),
                    ..EngineConfig::default()
                },
                store,
                transport.clone(),
                clock.clone(),
            )
            .unwrap();
            let public = k.public();
            let (seed, x) = (k.ed25519_seed(), k.x25519_secret());
            engine
                .provision(Provision {
                    keys: DeviceKeys::from_secrets(&seed, &x).unwrap(),
                    trust,
                    space_key: SpaceKey::from_bytes(1, &space_key.to_bytes()).unwrap(),
                    device_name: format!("Appareil {i}"),
                })
                .unwrap();
            nodes.push(Node {
                id: k.device_id(),
                engine: Arc::new(engine),
                seed,
                x,
                dir,
                cert,
                public,
            });
        }
        let w = Self {
            relay,
            clock,
            space_id,
            nodes,
        };
        for i in 0..registered {
            w.join(i);
        }
        w
    }

    fn join(&self, i: usize) {
        let n = &self.nodes[i];
        self.relay.add_device(DeviceRecord {
            device_id: n.id.clone(),
            name: format!("Appareil {i}"),
            platform: "android".into(),
            sign_pk: n.public.ed25519.clone(),
            kx_pk: n.public.x25519.clone(),
            certificate: n.cert.clone(),
            issuer_device_id: self.nodes[0].id.clone(),
            joined_seq: 0,
            registered_at_ms: T0,
            last_seen_at_ms: None,
            active: Some(false),
        });
    }

    fn e(&self, i: usize) -> &Engine {
        &self.nodes[i].engine
    }
}

fn text(s: &str) -> OutgoingDraft {
    OutgoingDraft {
        id: None,
        title: Some("Titre".into()),
        text: Some(DraftText {
            plain: s.into(),
            html: None,
            markdown: None,
            source_format: SourceFormat::Plain,
        }),
        url: None,
        attachments: vec![],
        secret: false,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
    }
}

fn plain_of(v: &ItemView) -> String {
    match &v.content {
        ItemContentView::Readable { text: Some(t), .. } => t.plain.clone(),
        other => panic!("not readable text: {other:?}"),
    }
}

fn send(w: &World, from: usize, d: OutgoingDraft) -> String {
    let id = w.e(from).enqueue(d).unwrap();
    assert_eq!(w.e(from).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    id
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[test]
fn session_is_opened_with_a_valid_challenge_signature_and_renewed() {
    let w = World::new(2, 2);
    let r = w.e(0).sync().unwrap();
    assert!(!r.offline);
    assert_eq!(w.relay.count("auth_challenge"), 1);
    assert_eq!(w.relay.count("auth_session"), 1);
    // A second sync reuses the session.
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("auth_session"), 1);
    // The relay forgets sessions (or one expires): a new challenge/response is done.
    w.relay.expire_sessions();
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("auth_session"), 2);
    // Close to expiry (< 5 min left): renewed before it fails.
    w.clock.advance(3_600_000 - 4 * 60_000);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("auth_session"), 3);
}

#[test]
fn expired_session_mid_call_is_renewed_transparently() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("auth_session"), 1);
    // The relay answers session_expired once: the engine opens a new session and retries.
    w.relay
        .fail_next("events", RelayError::api(401, "session_expired"));
    let id = send(&w, 1, text("apres expiration"));
    let r = w.e(0).sync().unwrap();
    assert_eq!(
        w.relay.count("auth_session"),
        3,
        "B opened one, A renewed one"
    );
    assert_eq!(r.new_items, 1);
    assert!(w.e(0).get_item(&id).unwrap().is_some());
}

#[test]
fn unknown_device_is_not_found_after_one_retry_and_asked_again_a_minute_later() {
    let w = World::new(2, 1); // device 1 never registered
    let err = w.e(1).sync().unwrap_err();
    assert!(matches!(err, crate::CoreError::Blocked { .. }));
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_eq!(
        w.relay.count("auth_session"),
        2,
        "exactly one retry with a new challenge"
    );
    assert_eq!(w.e(1).space_status(), SpaceStatus::NotFound);
    // Never written on disk.
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    // Within the minute: no network.
    assert!(w.e(1).sync().is_err());
    assert_eq!(w.relay.count("auth_session"), 2);
    // Once registered, a minute later, the relay is asked again and the state clears.
    w.join(1);
    w.clock.advance(61_000);
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(w.e(1).space_status(), SpaceStatus::Active);
}

#[test]
fn update_required_blocks_the_device() {
    let w = World::new(1, 1);
    w.relay.with(|s| s.min_client_format = 2);
    assert!(w.e(0).sync().is_err());
    assert!(matches!(
        w.e(0).blocked(),
        Some(Blocked::UpdateRequired {
            update_url: Some(_)
        })
    ));
    assert_eq!(w.e(0).space_status(), SpaceStatus::UpdateRequired);
}

#[test]
fn offline_sync_reports_offline_and_changes_nothing() {
    let w = World::new(2, 2);
    w.relay.set_offline(true);
    let r = w.e(0).sync().unwrap();
    assert!(r.offline);
    assert_eq!(r.cursor, 0);
}

// ---------------------------------------------------------------------------
// Send, receive, acknowledge
// ---------------------------------------------------------------------------

#[test]
fn send_then_ack_then_delivery_state() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();

    let id = w.e(0).enqueue(text("bonjour")).unwrap();
    let queued = w.e(0).get_item(&id).unwrap().unwrap();
    assert_eq!(queued.send_status, Some(SendStatus::Queued));
    assert!(queued
        .deliveries
        .iter()
        .all(|d| d.state == DeliveryState::Queued));
    assert_eq!(w.relay.count("post_item"), 0, "nothing is sent by enqueue");

    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert_eq!(w.relay.count("post_item"), 1);
    // Idempotent: dispatching again does not post again.
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert_eq!(w.relay.count("post_item"), 1);
    assert_eq!(
        w.e(0)
            .dispatch("00000000-0000-4000-8000-000000000000")
            .unwrap(),
        DispatchOutcome::NotFound
    );

    // B receives, decrypts and acknowledges within the same sync (outbox drained).
    let r = w.e(1).sync().unwrap();
    assert_eq!(r.new_items, 1);
    let got = w.e(1).get_item(&id).unwrap().unwrap();
    assert_eq!(plain_of(&got), "bonjour");
    assert_eq!(got.origin, ItemOrigin::Received);
    assert_eq!(got.source_device_id, w.nodes[0].id);
    assert_eq!(w.relay.count("ack_item"), 1);
    assert!(w.e(1).store.read(|db| db.pending_ops()).unwrap().is_empty());

    // A learns the delivery from the log.
    w.e(0).sync().unwrap();
    let sent = w.e(0).get_item(&id).unwrap().unwrap();
    assert_eq!(sent.send_status, Some(SendStatus::Sent));
    let d = sent
        .deliveries
        .iter()
        .find(|d| d.device_id == w.nodes[1].id)
        .unwrap();
    assert_eq!(d.state, DeliveryState::Delivered);
    // Its own item comes back through the log without a second copy.
    assert_eq!(w.e(0).list_items().unwrap().len(), 1);
}

#[test]
fn text_with_all_representations_and_secret_flag_round_trips() {
    let w = World::new(2, 2);
    let mut d = text("clair");
    d.text.as_mut().unwrap().html = Some("<b>clair</b>".into());
    d.text.as_mut().unwrap().markdown = Some("**clair**".into());
    d.text.as_mut().unwrap().source_format = SourceFormat::Markdown;
    d.secret = true;
    d.url = Some("https://example.org/x".into());
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    let ItemContentView::Readable {
        title,
        text,
        url,
        secret,
        ..
    } = v.content
    else {
        panic!()
    };
    assert_eq!(title.as_deref(), Some("Titre"));
    assert_eq!(url.as_deref(), Some("https://example.org/x"));
    assert!(secret);
    let t = text.unwrap();
    assert_eq!(t.html.as_deref(), Some("<b>clair</b>"));
    assert_eq!(t.source_format, SourceFormat::Markdown);
}

#[test]
fn local_database_never_holds_item_text_in_clear() {
    let w = World::new(2, 2);
    send(&w, 0, text("zorglub-secret-marker"));
    w.e(1).sync().unwrap();
    for i in 0..2 {
        let dir = w.nodes[i].dir.path();
        for f in ["tomi.db", "tomi.db-wal"] {
            let bytes = std::fs::read(dir.join(f)).unwrap_or_default();
            assert!(
                !bytes.windows(14).any(|x| x == b"zorglub-secret"),
                "plaintext found in {f} of node {i}"
            );
        }
    }
}

#[test]
fn attachment_is_encrypted_uploaded_downloaded_and_released() {
    let w = World::new(2, 2);
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let src = w.nodes[0].dir.path().join("photo.bin");
    std::fs::write(&src, &data).unwrap();
    let mut d = text("avec fichier");
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "photo.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();
    // The caller may delete its file once enqueue returned.
    std::fs::remove_file(&src).unwrap();
    // The sender can already open its own file.
    let own = w.e(0).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(own).unwrap(), data);
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert!(w.relay.count("put_blob_part") >= 1);
    // The relay only holds ciphertext.
    let stored = w.relay.with(|s| s.events.len());
    assert!(stored >= 1);

    w.e(1).sync().unwrap();
    // Not downloaded yet, and the ack does not release the blob.
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    let ItemContentView::Readable { attachments, .. } = &v.content else {
        panic!()
    };
    assert_eq!(attachments[0].local_path, None);
    assert_eq!(attachments[0].size, data.len() as u64);
    let p = w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), data);
    // A "release" ack is now queued and goes out at the next sync.
    let before = w.relay.count("ack_item");
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("ack_item"), before + 1);
}

#[test]
fn blob_upload_resumes_from_the_relay_offset() {
    let w = World::new(2, 2);
    let data = vec![7u8; 2_500_000]; // 3 parts of at most 1 MiB
    let src = w.nodes[0].dir.path().join("big.bin");
    std::fs::write(&src, &data).unwrap();
    let mut d = text("gros");
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "big.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();
    // The connection is cut after the first part: it stays on the relay.
    w.relay
        .fail_next_after("put_blob_part", 1, RelayError::Network("cut".into()));
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::RetryLater);
    assert_eq!(
        w.e(0).get_item(&id).unwrap().unwrap().send_status,
        Some(SendStatus::Queued)
    );
    assert_eq!(w.relay.count("put_blob_part"), 2);
    w.relay.with(|s| s.calls.clear());
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert_eq!(
        w.relay.count("put_blob_part"),
        2,
        "resumed: parts 2 and 3 only"
    );
    w.e(1).sync().unwrap();
    let p = w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), data);
}

/// A queued item with one staged attachment: its blob row and its `files/{item}_{idx}.enc` file.
fn queued_with_staged_file(
    w: &World,
    expires_at_ms: Option<i64>,
) -> (String, Vec<super::store::BlobUpload>) {
    let src = w.nodes[0].dir.path().join("joint.bin");
    std::fs::write(&src, vec![3u8; 2_000]).unwrap();
    let mut d = text("en attente");
    d.expires_at_ms = expires_at_ms;
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "joint.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();
    let staged: Vec<super::store::BlobUpload> = w
        .e(0)
        .store
        .read(|db| {
            let mut all = Vec::new();
            for op in db.ops_for_item(&id)? {
                all.extend(db.blob_uploads("op", &op.op_id.to_string())?);
            }
            Ok(all)
        })
        .unwrap();
    assert_eq!(staged.len(), 1, "one staged blob");
    assert!(
        w.e(0).store.file_path(&staged[0].path).exists(),
        "its encrypted copy is on disk"
    );
    (id, staged)
}

fn staged_rows(w: &World) -> usize {
    w.e(0)
        .store
        .read(|db| {
            let mut n = 0;
            for op in db.pending_ops()? {
                n += db.blob_uploads("op", &op.op_id.to_string())?.len();
            }
            Ok(n)
        })
        .unwrap()
}

#[test]
fn expiry_maintenance_removes_the_staged_blobs_of_a_queued_item() {
    let w = World::new(2, 2);
    let (id, staged) = queued_with_staged_file(&w, Some(T0 + 60_000));
    w.clock.advance(61_000);
    w.e(0).maintenance().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().is_none(), "erased on time");
    assert!(w.e(0).store.read(|db| db.pending_ops()).unwrap().is_empty());
    assert_eq!(staged_rows(&w), 0);
    assert!(
        !w.e(0).store.file_path(&staged[0].path).exists(),
        "the staged file is deleted with the rows"
    );
}

#[test]
fn erasing_everywhere_removes_the_staged_blobs_of_a_queued_item() {
    let w = World::new(2, 2);
    let (id, staged) = queued_with_staged_file(&w, None);
    let now = T0 + 5;
    let paths = w
        .e(0)
        .with_crypto(|c, db, _a| Engine::erase_item_everywhere(c, db, &id, now))
        .unwrap();
    assert!(
        paths.contains(&staged[0].path),
        "the staged file is handed back to be removed after the commit"
    );
    assert!(w.e(0).store.read(|db| db.pending_ops()).unwrap().is_empty());
    assert_eq!(staged_rows(&w), 0);
    for p in paths {
        let _ = std::fs::remove_file(w.e(0).store.file_path(&p));
    }
    assert!(!w.e(0).store.file_path(&staged[0].path).exists());
}

#[test]
fn offline_send_is_kept_and_cancel_is_atomic() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    w.relay.set_offline(true);
    let id = w.e(0).enqueue(text("hors ligne")).unwrap();
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::RetryLater);
    assert_eq!(w.e(0).list_items().unwrap().len(), 1);
    assert_eq!(w.e(0).store.read(|db| db.pending_ops()).unwrap().len(), 1);

    // Back online: the sync drains the outbox.
    w.relay.set_offline(false);
    let r = w.e(0).sync().unwrap();
    assert_eq!(r.outbox_pending, 0);
    assert_eq!(
        w.e(0).get_item(&id).unwrap().unwrap().send_status,
        Some(SendStatus::Sent)
    );
    // Too late to cancel.
    assert!(!w.e(0).cancel_if_queued(&id).unwrap());
    assert!(w.e(0).get_item(&id).unwrap().is_some());

    // Still queued: cancel erases it and nothing is ever sent.
    w.relay.set_offline(true);
    let id2 = w.e(0).enqueue(text("annule")).unwrap();
    assert!(w.e(0).cancel_if_queued(&id2).unwrap());
    assert!(w.e(0).get_item(&id2).unwrap().is_none());
    w.relay.set_offline(false);
    w.e(0).sync().unwrap();
    let posted = w
        .relay
        .events()
        .iter()
        .filter(|e| matches!(e.data, EventData::Item { .. }))
        .count();
    assert_eq!(posted, 1, "the cancelled item never reached the relay");
    assert_eq!(w.e(0).dispatch(&id2).unwrap(), DispatchOutcome::NotFound);
}

#[test]
fn enqueue_is_idempotent_for_a_given_id() {
    let w = World::new(1, 1);
    let mut d = text("une fois");
    d.id = Some(crate::new_item_id().unwrap());
    let a = w.e(0).enqueue(d.clone()).unwrap();
    let b = w.e(0).enqueue(d).unwrap();
    assert_eq!(a, b);
    assert_eq!(w.e(0).list_items().unwrap().len(), 1);
    assert_eq!(w.e(0).store.read(|db| db.pending_ops()).unwrap().len(), 1);
}

#[test]
fn item_that_expired_before_sending_is_not_posted() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    let mut d = text("trop tard");
    d.expires_at_ms = Some(T0 + 1000);
    let id = w.e(0).enqueue(d).unwrap();
    w.clock.advance(5000);
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Failed);
    assert_eq!(w.relay.count("post_item"), 0);
}

#[test]
fn relay_refusals_are_classified() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    let id = w.e(0).enqueue(text("x")).unwrap();
    // relay_full: kept, retried later (not before 5 minutes).
    w.relay
        .fail_next("post_item", RelayError::api(507, "relay_full"));
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::RetryLater);
    let op = w.e(0).store.read(|db| db.pending_ops()).unwrap().remove(0);
    assert!(op.next_attempt_at_ms >= T0 + 300_000);
    // Not due yet: a sync leaves it, then it goes after the delay.
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item"), 1);
    w.clock.advance(300_001);
    w.e(0).sync().unwrap();
    assert_eq!(
        w.e(0).get_item(&id).unwrap().unwrap().send_status,
        Some(SendStatus::Sent)
    );
    // A permanent refusal marks the item failed and removes the operation.
    let id2 = w.e(0).enqueue(text("y")).unwrap();
    w.relay
        .fail_next("post_item", RelayError::api(413, "payload_too_large"));
    assert_eq!(w.e(0).dispatch(&id2).unwrap(), DispatchOutcome::Failed);
    assert!(w.e(0).store.read(|db| db.pending_ops()).unwrap().is_empty());
    // 429 honours Retry-After.
    let id3 = w.e(0).enqueue(text("z")).unwrap();
    w.relay.fail_next(
        "post_item",
        RelayError::problem(ApiProblem {
            status: 429,
            code: "rate_limited".into(),
            retry_after_secs: Some(42),
            ..ApiProblem::default()
        }),
    );
    assert_eq!(w.e(0).dispatch(&id3).unwrap(), DispatchOutcome::RetryLater);
    let op = w.e(0).store.read(|db| db.pending_ops()).unwrap().remove(0);
    assert_eq!(op.next_attempt_at_ms, w.clock_now() + 42_000);
}

impl World {
    fn clock_now(&self) -> i64 {
        use crate::runtime::Clock;
        self.clock.now_ms()
    }
}

// ---------------------------------------------------------------------------
// Tombstones
// ---------------------------------------------------------------------------

#[test]
fn tombstone_deletes_everywhere_and_is_not_resurrected() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("ephemere"));
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_some());

    w.e(0).delete_everywhere(&id).unwrap();
    assert!(
        w.e(0).get_item(&id).unwrap().is_none(),
        "local effect is immediate"
    );
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_tombstone"), 1);

    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    assert!(w.e(1).store.read(|db| db.is_tombstoned(&id)).unwrap());
    // Replaying the whole log (cursor reset) does not bring the item back.
    w.e(1).store.tx(|db| db.set_cursor(0)).unwrap();
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
}

#[test]
fn tombstone_of_an_unknown_signer_is_ignored() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("reste"));
    w.e(1).sync().unwrap();
    let stranger = DeviceKeys::generate().unwrap();
    let forged = stranger
        .sign_tombstone(
            &w.space_id,
            &id,
            T0 as u64,
            crate::DeletionScope::Everywhere,
        )
        .unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::Tombstone {
                item_id: id.clone(),
                device_id: "ffffffffffffffffffffffffffffffff".into(),
                deleted_at_ms: T0,
                tombstone: forged.to_bytes().unwrap(),
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(
        w.e(1).get_item(&id).unwrap().is_some(),
        "forged tombstone must not delete"
    );
    let diag = w.e(1).store.read(|db| db.diagnostics()).unwrap();
    assert!(diag
        .iter()
        .any(|(_, k, d)| k == "tombstone_refused" && d == "untrusted_signer"));
}

#[test]
fn a_tombstone_whose_transaction_fails_keeps_the_files_and_the_retry_removes_them() {
    let w = World::new(2, 2);
    let src = w.nodes[0].dir.path().join("joint.bin");
    std::fs::write(&src, vec![5u8; 3_000]).unwrap();
    let mut d = text("avec fichier");
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "joint.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    w.e(1).sync().unwrap();
    // Downloaded: the encrypted copy and the decrypted cache are on disk on device 1.
    w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    let on_disk = || -> Vec<std::path::PathBuf> {
        w.e(1)
            .store
            .read(|db| {
                let mut all = Vec::new();
                for f in db.files(&id)? {
                    all.push(super::store::cache_file_name(&id, f.idx));
                    all.extend(f.path);
                }
                Ok(all)
            })
            .unwrap()
            .iter()
            .map(|p| w.e(1).store.file_path(p))
            .filter(|p| p.exists())
            .collect()
    };
    let before = on_disk();
    assert!(!before.is_empty(), "files are on disk before the tombstone");

    w.e(0).delete_everywhere(&id).unwrap();
    w.e(0).sync().unwrap();
    // The cursor update fails: the whole transaction (rows included) is rolled back.
    w.e(1).store.exec_for_test(
        "CREATE TRIGGER no_cursor BEFORE UPDATE ON sync_state BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    );
    assert!(w.e(1).sync().is_err());
    assert!(w.e(1).get_item(&id).unwrap().is_some(), "rows are kept");
    assert_eq!(on_disk(), before, "and so are the files");

    w.e(1).store.exec_for_test("DROP TRIGGER no_cursor;");
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    for p in before {
        assert!(!p.exists(), "removed after the commit: {p:?}");
    }
}

#[test]
fn after_first_read_consumes_the_item_for_everyone() {
    let w = World::new(2, 2);
    let mut d = text("code a usage unique");
    d.read_policy = ReadPolicy::AfterFirstRead;
    d.secret = true;
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_some());
    w.e(1).mark_read(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(
        w.e(0).get_item(&id).unwrap().is_none(),
        "sender erases its copy too"
    );
}

#[test]
fn mark_read_sends_a_read_event_and_does_not_touch_unreadable_items() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("lu"));
    w.e(1).sync().unwrap();
    w.e(1).mark_read(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().read);
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("read_item"), 1);
    // Marking read twice sends once.
    w.e(1).mark_read(&id).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("read_item"), 1);
}

// ---------------------------------------------------------------------------
// Pin (spec/pin-contract.md)
// ---------------------------------------------------------------------------

#[test]
fn pin_state_event_reaches_the_other_device() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("important"));
    w.e(1).sync().unwrap();

    w.e(0).pin(&id).unwrap();
    let mine = w.e(0).get_item(&id).unwrap().unwrap();
    assert!(mine.pinned, "immediate local effect");
    assert!(mine.pinned_at_ms.is_some());
    assert_eq!(
        w.relay.count("post_item_state"),
        0,
        "the decision leaves through the outbox"
    );
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), 1);

    w.e(1).sync().unwrap();
    let theirs = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(theirs.pinned);
    assert_eq!(theirs.pinned_at_ms, mine.pinned_at_ms);
    // No download, no release, no read triggered by the pin.
    assert_eq!(w.relay.count("get_blob_range"), 0);
    assert_eq!(w.relay.count("read_item"), 0);

    // Idempotent: pinning again sends nothing.
    w.e(0).pin(&id).unwrap();
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), 1);

    // Unpin converges too.
    w.clock.advance(1000);
    w.e(1).unpin(&id).unwrap();
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(!w.e(0).get_item(&id).unwrap().unwrap().pinned);
    // Unknown id: no effect, no error.
    w.e(0).pin("00000000-0000-4000-8000-000000000000").unwrap();
}

#[test]
fn offline_pin_is_applied_locally_and_sent_later() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("a epingler"));
    w.e(1).sync().unwrap();
    w.relay.set_offline(true);
    w.e(1).pin(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().pinned);
    assert!(w.e(1).sync().unwrap().offline);
    w.relay.set_offline(false);
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().unwrap().pinned);
}

#[test]
fn pin_event_id_conflict_is_reissued_once() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("conflit"));
    w.e(1).sync().unwrap();
    w.e(1).pin(&id).unwrap();
    w.relay
        .fail_next("post_item_state", RelayError::api(409, "event_id_conflict"));
    w.e(1).sync().unwrap();
    // Same declaration under a new event id: it eventually reaches the relay.
    assert!(w.relay.count("post_item_state") >= 2);
    w.e(0).sync().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().unwrap().pinned);
    assert!(w.e(1).store.read(|db| db.pending_ops()).unwrap().is_empty());
}

#[test]
fn pin_of_an_unreadable_epoch_is_kept_and_applied_when_readable() {
    // An item_state announcing a key we do not have is kept raw in pending_states.
    let w = World::new(2, 2);
    let id = send(&w, 0, text("x"));
    w.e(1).sync().unwrap();
    let key9 = SpaceKey::generate(9).unwrap();
    let st = w.nodes[0]
        .keys()
        .sign_pin(&w.space_id, &id, T0 as u64 + 5, true)
        .unwrap();
    let eid = crate::new_item_id().unwrap();
    let env = crate::seal_item_state(&st, &eid, &key9).unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::ItemState {
                event_id: eid.clone(),
                device_id: w.nodes[0].id.clone(),
                key_epoch: 9,
                envelope: env,
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(!w.e(1).get_item(&id).unwrap().unwrap().pinned);
    assert_eq!(
        w.e(1).store.read(|db| db.pending_states()).unwrap().len(),
        1
    );
}

// ---------------------------------------------------------------------------
// Archive (spec/archive-contract.md)
// ---------------------------------------------------------------------------

const UNKNOWN_ITEM: &str = "00000000-0000-4000-8000-000000000000";

/// A sealed archive decision of node `from` for `item_id`, as the relay would list it.
fn archive_event(w: &World, from: usize, item_id: &str, at_ms: u64, archived: bool) -> EventData {
    let st = w.nodes[from]
        .keys()
        .sign_archive(&w.space_id, item_id, at_ms, archived)
        .unwrap();
    let key = w
        .e(from)
        .read_crypto(|c, _| Ok(c.ring.current()?.clone()))
        .unwrap();
    let eid = crate::new_item_id().unwrap();
    EventData::ItemState {
        envelope: crate::seal_item_state(&st, &eid, &key).unwrap(),
        event_id: eid,
        device_id: w.nodes[from].id.clone(),
        key_epoch: key.epoch(),
    }
}

fn archived_of(w: &World, i: usize, id: &str) -> bool {
    w.e(i).get_item(id).unwrap().unwrap().archived
}

fn archive_state_of(w: &World, i: usize, id: &str) -> Option<crate::ArchiveState> {
    w.e(i).store.read(|db| db.archive_book()).unwrap().state(id)
}

#[test]
fn archive_state_event_reaches_the_other_device() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("a ranger"));
    w.e(1).sync().unwrap();
    // The device is told of every change.
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = changes.clone();
    w.e(0).subscribe(Arc::new(move || {
        c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));

    w.e(0).archive(&id).unwrap();
    let mine = w.e(0).get_item(&id).unwrap().unwrap();
    assert!(mine.archived, "immediate local effect");
    assert!(mine.archived_at_ms.is_some());
    assert!(!mine.pinned);
    assert!(changes.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(w.relay.count("post_item_state"), 0, "through the outbox");
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), 1);

    let (reads, blobs) = (w.relay.count("read_item"), w.relay.count("get_blob_range"));
    w.e(1).sync().unwrap();
    let theirs = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(theirs.archived);
    assert_eq!(theirs.archived_at_ms, mine.archived_at_ms);
    // Still in the feed list (the filter is the UI's), and not read.
    assert!(w.e(1).list_items().unwrap().iter().any(|v| v.id == id));
    assert!(!theirs.read);
    assert_eq!(w.relay.count("read_item"), reads);
    assert_eq!(w.relay.count("get_blob_range"), blobs);

    // Idempotent: archiving again sends nothing.
    w.e(0).archive(&id).unwrap();
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), 1);

    // Unarchive converges too.
    w.clock.advance(1000);
    w.e(1).unarchive(&id).unwrap();
    assert!(!archived_of(&w, 1, &id));
    assert_eq!(w.e(1).get_item(&id).unwrap().unwrap().archived_at_ms, None);
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(!archived_of(&w, 0, &id));
    // Unarchiving again, or an unknown id: no effect, no error.
    w.e(0).unarchive(&id).unwrap();
    w.e(0).archive(UNKNOWN_ITEM).unwrap();
    w.e(0).unarchive(UNKNOWN_ITEM).unwrap();
    assert_eq!(archive_state_of(&w, 0, UNKNOWN_ITEM), None);
}

#[test]
fn archiving_never_reads_consumes_or_releases() {
    let w = World::new(2, 2);
    let mut d = text("code a usage unique");
    d.read_policy = ReadPolicy::AfterFirstRead;
    d.secret = true;
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    let (acks, reads, blobs, tombs) = (
        w.relay.count("ack_item"),
        w.relay.count("read_item"),
        w.relay.count("get_blob_range"),
        w.relay.count("post_tombstone"),
    );
    w.e(1).archive(&id).unwrap();
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    // The secret survives on both devices, unread, still a secret.
    for i in [0, 1] {
        let v = w.e(i).get_item(&id).unwrap().expect("still there");
        if i == 1 {
            assert!(!v.read, "archiving never marks read");
        }
        assert_eq!(v.read_policy, ReadPolicy::AfterFirstRead);
        assert!(matches!(
            v.content,
            ItemContentView::Readable { secret: true, .. }
        ));
    }
    assert!(archived_of(&w, 0, &id) && archived_of(&w, 1, &id));
    assert_eq!(w.relay.count("ack_item"), acks);
    assert_eq!(w.relay.count("read_item"), reads);
    assert_eq!(w.relay.count("get_blob_range"), blobs);
    assert_eq!(w.relay.count("post_tombstone"), tombs);
    // The archive never queues anything but its own state event.
    assert!(w.e(1).store.read(|db| db.pending_ops()).unwrap().is_empty());
    // Opening it afterwards still consumes it, archived or not.
    w.e(1).mark_read(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
}

#[test]
fn archiving_leaves_explicit_expiration_unchanged() {
    let w = World::new(2, 2);
    let mut d = text("expire quand meme");
    d.expires_at_ms = Some(T0 + 60_000);
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    w.e(1).archive(&id).unwrap();
    w.clock.advance(61_000);
    assert!(w.e(1).list_items().unwrap().is_empty());
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none(), "erased on time");
    assert_eq!(
        archive_state_of(&w, 1, &id),
        None,
        "its archive state is forgotten with it"
    );
    // The local cleanup (`LocalRetention::Cleanup`) is not wired into the engine yet
    // (`maintenance` passes no cutoff), so nothing is asserted about it here. The pure rule,
    // which spares pinned items only and ignores the archive, is covered by
    // `pin::tests::retention_rules`.
}

#[test]
fn offline_archive_is_applied_locally_and_sent_later() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("hors ligne"));
    w.e(1).sync().unwrap();
    w.relay.set_offline(true);
    w.e(1).archive(&id).unwrap();
    assert!(archived_of(&w, 1, &id));
    assert!(w.e(1).sync().unwrap().offline);
    w.relay.set_offline(false);
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(archived_of(&w, 0, &id));
}

#[test]
fn archive_survives_a_restart() {
    let w = World::new(1, 1);
    let id = send(&w, 0, text("persiste"));
    w.e(0).archive(&id).unwrap();
    let at = w.e(0).get_item(&id).unwrap().unwrap().archived_at_ms;
    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    let v = again.get_item(&id).unwrap().unwrap();
    assert!(v.archived);
    assert_eq!(v.archived_at_ms, at);
}

#[test]
fn archive_event_id_conflict_is_reissued_once() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("conflit"));
    w.e(1).sync().unwrap();
    w.e(1).archive(&id).unwrap();
    w.relay
        .fail_next("post_item_state", RelayError::api(409, "event_id_conflict"));
    w.e(1).sync().unwrap();
    assert!(w.relay.count("post_item_state") >= 2);
    w.e(0).sync().unwrap();
    assert!(archived_of(&w, 0, &id));
    assert!(w.e(1).store.read(|db| db.pending_ops()).unwrap().is_empty());
}

#[test]
fn archive_of_an_unreadable_epoch_is_kept_and_applied_when_readable() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("x"));
    w.e(1).sync().unwrap();
    let key9 = SpaceKey::generate(9).unwrap();
    let st = w.nodes[0]
        .keys()
        .sign_archive(&w.space_id, &id, T0 as u64 + 5, true)
        .unwrap();
    let eid = crate::new_item_id().unwrap();
    let env = crate::seal_item_state(&st, &eid, &key9).unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::ItemState {
                event_id: eid.clone(),
                device_id: w.nodes[0].id.clone(),
                key_epoch: 9,
                envelope: env,
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(!archived_of(&w, 1, &id));
    assert_eq!(
        w.e(1).store.read(|db| db.pending_states()).unwrap().len(),
        1
    );
}

#[test]
fn replayed_and_older_archive_events_change_nothing() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("lww"));
    w.e(1).sync().unwrap();
    let t = T0 as u64;
    // Newest first: archived at t+100, then older decisions arrive late.
    let newest = archive_event(&w, 0, &id, t + 100, true);
    let older_unarchive = archive_event(&w, 0, &id, t + 50, false);
    w.relay.with(|s| {
        s.push(newest.clone(), None);
        s.push(older_unarchive, None);
        // Replay of the very same event (same event id, new seq).
        s.push(newest, None);
    });
    w.e(1).sync().unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(v.archived, "the older unarchive loses");
    assert_eq!(v.archived_at_ms, Some(t as i64 + 100));
    // A tie goes to "archived"; a later decision wins.
    w.relay.with(|s| {
        s.push(archive_event(&w, 0, &id, t + 100, false), None);
    });
    w.e(1).sync().unwrap();
    assert!(archived_of(&w, 1, &id), "tie: archived wins");
    w.relay.with(|s| {
        s.push(archive_event(&w, 0, &id, t + 101, false), None);
    });
    w.e(1).sync().unwrap();
    assert!(!archived_of(&w, 1, &id));
    // A user decision made after the newest state supersedes it even if the clock is behind.
    w.e(1).archive(&id).unwrap();
    let again = w.e(1).get_item(&id).unwrap().unwrap();
    assert_eq!(again.archived_at_ms, Some(t as i64 + 102));
}

#[test]
fn a_state_of_an_unknown_kind_is_kept_raw_without_error_or_effect() {
    // What a device without archive support sees: the same envelope shape and a kind it does
    // not know. It changes nothing and logs nothing, but the raw envelope is kept in
    // `pending_states` so that a later version can replay it (spec §8.8.2 step 6).
    let w = World::new(2, 2);
    let id = send(&w, 0, text("x"));
    w.e(1).sync().unwrap();
    let key = w
        .e(1)
        .read_crypto(|c, _| Ok(c.ring.current()?.clone()))
        .unwrap();
    let eid = crate::new_item_id().unwrap();
    let env = crate::pin::seal_item_state_raw(
        b"tomi/v1/sig/future\0whatever",
        &uuid_bytes(&eid),
        &key,
        &[7; 24],
    )
    .unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::ItemState {
                event_id: eid.clone(),
                device_id: w.nodes[0].id.clone(),
                key_epoch: key.epoch(),
                envelope: env.clone(),
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(!archived_of(&w, 1, &id));
    assert!(!w.e(1).get_item(&id).unwrap().unwrap().pinned);
    let kept = w.e(1).store.read(|db| db.pending_states()).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].0, eid);
    assert_eq!(kept[0].3, env, "the raw envelope, untouched");
    assert!(w.e(1).store.read(|db| db.diagnostics()).unwrap().is_empty());

    // Another sync and the retry after an update with the same version: still kept, once.
    w.e(1).retry_update_required().unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(
        w.e(1).store.read(|db| db.pending_states()).unwrap().len(),
        1
    );
    assert!(w.e(1).store.read(|db| db.diagnostics()).unwrap().is_empty());
}

#[test]
fn kept_item_states_are_replayed_by_the_retry_after_an_update() {
    // A device that kept an archive decision raw (as a version without archive would) applies
    // it once it can read it; a decision of a type still unknown stays kept.
    let w = World::new(2, 2);
    let id = send(&w, 0, text("rattrape"));
    w.e(1).sync().unwrap();
    let t = T0 as u64;
    let EventData::ItemState {
        event_id, envelope, ..
    } = archive_event(&w, 0, &id, t + 10, true)
    else {
        unreachable!()
    };
    let key = w
        .e(1)
        .read_crypto(|c, _| Ok(c.ring.current()?.clone()))
        .unwrap();
    let future_id = crate::new_item_id().unwrap();
    let future = crate::pin::seal_item_state_raw(
        b"tomi/v1/sig/future\0whatever",
        &uuid_bytes(&future_id),
        &key,
        &[7; 24],
    )
    .unwrap();
    w.e(1)
        .store
        .tx(|db| {
            db.put_pending_state(&event_id, 5, key.epoch(), &envelope)?;
            db.put_pending_state(&future_id, 6, key.epoch(), &future)
        })
        .unwrap();
    assert!(!archived_of(&w, 1, &id));

    w.e(1).retry_update_required().unwrap();
    assert!(archived_of(&w, 1, &id), "the kept archive is applied");
    let left = w.e(1).store.read(|db| db.pending_states()).unwrap();
    assert_eq!(left.len(), 1, "only the still-unknown type remains");
    assert_eq!(left[0].0, future_id);
    // The retry is idempotent.
    w.e(1).retry_update_required().unwrap();
    assert!(archived_of(&w, 1, &id));
    assert_eq!(
        w.e(1).store.read(|db| db.pending_states()).unwrap().len(),
        1
    );
}

#[test]
fn a_sync_replays_kept_states_only_when_the_decoder_changed() {
    use super::engine::REPLAYED_VERSION_KEY;
    let w = World::new(2, 2);
    let id = send(&w, 0, text("rattrape"));
    w.e(1).sync().unwrap();
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.meta_get_string(REPLAYED_VERSION_KEY))
            .unwrap(),
        Some(Engine::decoder_version()),
        "the first sync records the version"
    );
    assert!(!w
        .e(1)
        .store
        .read(|db| db.has_update_required_work())
        .unwrap());

    let EventData::ItemState {
        event_id, envelope, ..
    } = archive_event(&w, 0, &id, T0 as u64 + 10, true)
    else {
        unreachable!()
    };
    w.e(1)
        .store
        .tx(|db| db.put_pending_state(&event_id, 5, 1, &envelope))
        .unwrap();
    assert!(w
        .e(1)
        .store
        .read(|db| db.has_update_required_work())
        .unwrap());

    // Same decoder: a sync neither reads nor rewrites what is kept.
    w.e(1).sync().unwrap();
    assert!(!archived_of(&w, 1, &id));
    assert_eq!(
        w.e(1).store.read(|db| db.pending_states()).unwrap().len(),
        1
    );

    // After an update (another version recorded), the next sync replays, then records.
    w.e(1)
        .store
        .tx(|db| db.meta_set(REPLAYED_VERSION_KEY, b"0.0.0+f0"))
        .unwrap();
    w.e(1).sync().unwrap();
    assert!(archived_of(&w, 1, &id), "the kept archive is applied");
    assert!(w
        .e(1)
        .store
        .read(|db| db.pending_states())
        .unwrap()
        .is_empty());
    assert_eq!(
        w.e(1)
            .store
            .read(|db| db.meta_get_string(REPLAYED_VERSION_KEY))
            .unwrap(),
        Some(Engine::decoder_version())
    );
}

#[test]
fn a_forged_archive_is_refused_and_logged_without_content() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("x"));
    w.e(1).sync().unwrap();
    // Signed by a device that is not a member of the space.
    let stranger = DeviceKeys::generate().unwrap();
    let st = stranger
        .sign_archive(&w.space_id, &id, T0 as u64, true)
        .unwrap();
    let key = w
        .e(1)
        .read_crypto(|c, _| Ok(c.ring.current()?.clone()))
        .unwrap();
    let eid = crate::new_item_id().unwrap();
    let env = crate::seal_item_state(&st, &eid, &key).unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::ItemState {
                event_id: eid,
                device_id: w.nodes[0].id.clone(),
                key_epoch: key.epoch(),
                envelope: env,
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    assert!(!archived_of(&w, 1, &id));
    let diag = w.e(1).store.read(|db| db.diagnostics()).unwrap();
    assert!(diag
        .iter()
        .any(|(_, k, d)| k == "archive_refused" && d == "untrusted_signer"));
}

#[test]
fn archive_and_pin_are_mutually_exclusive() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("epingle puis archive"));
    w.e(1).sync().unwrap();
    w.e(0).pin(&id).unwrap();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().pinned);
    let posted = w.relay.count("post_item_state");

    // Archiving a pinned item unpins it first: two decisions, one local step.
    w.clock.advance(1000);
    w.e(1).archive(&id).unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(v.archived && !v.pinned && v.pinned_at_ms.is_none());
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), posted + 2);
    w.e(0).sync().unwrap();
    let on_a = w.e(0).get_item(&id).unwrap().unwrap();
    assert!(on_a.archived && !on_a.pinned);

    // Pinning an archived item unarchives it first.
    w.clock.advance(1000);
    w.e(0).pin(&id).unwrap();
    let v = w.e(0).get_item(&id).unwrap().unwrap();
    assert!(v.pinned && !v.archived && v.archived_at_ms.is_none());
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    let on_b = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(on_b.pinned && !on_b.archived);

    // Unpinning alone does not archive.
    w.clock.advance(1000);
    w.e(1).unpin(&id).unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(!v.pinned && !v.archived);
    // Nothing to do when already in the target place: no event.
    let n = w.relay.count("post_item_state");
    w.e(1).unarchive(&id).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_item_state"), n + 1, "only the unpin");
}

#[test]
fn when_both_states_are_true_the_pin_wins_at_display() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("croisement"));
    w.e(1).sync().unwrap();
    // A archives while B pins, neither having seen the other's decision.
    w.e(0).archive(&id).unwrap();
    w.e(1).pin(&id).unwrap();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    for i in [0, 1] {
        let v = w.e(i).get_item(&id).unwrap().unwrap();
        assert!(v.pinned, "the pin wins");
        assert!(!v.archived && v.archived_at_ms.is_none());
        // The register itself still says archived (independent registers).
        assert!(archive_state_of(&w, i, &id).is_some_and(|s| s.archived));
    }
    // Archiving from the crossing state unpins (the user wants it archived).
    w.clock.advance(1000);
    w.e(0).archive(&id).unwrap();
    let v = w.e(0).get_item(&id).unwrap().unwrap();
    assert!(v.archived && !v.pinned);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(v.archived && !v.pinned);
}

#[test]
fn an_unreadable_or_queued_item_can_be_archived() {
    let w = World::new(2, 2);
    // Unreadable (format unknown to this build).
    let mut newer = vec![0, 0, 0, 2, 0, 0, 0, 1];
    newer.extend_from_slice(&[0xAA; 100]);
    let id = crate::new_item_id().unwrap();
    w.relay.with(|s| {
        s.push(
            EventData::Item {
                item_id: id.clone(),
                sender_device_id: w.nodes[0].id.clone(),
                created_at_ms: T0,
                expires_at_ms: T0 + 100_000,
                key_epoch: 1,
                envelope: newer,
                blobs: vec![],
                ring: false,
            },
            None,
        )
    });
    w.e(1).sync().unwrap();
    w.e(1).archive(&id).unwrap();
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(v.archived);
    assert!(matches!(v.content, ItemContentView::Unreadable { .. }));
    // The core is permissive for an outgoing item still in the queue (the UI is not).
    let queued = w.e(0).enqueue(text("en file")).unwrap();
    w.e(0).archive(&queued).unwrap();
    assert!(archived_of(&w, 0, &queued));
    assert_eq!(w.e(0).dispatch(&queued).unwrap(), DispatchOutcome::Sent);
    assert!(archived_of(&w, 0, &queued));
}

#[test]
fn deleting_an_item_forgets_its_archive_and_ignores_later_archive_events() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("a supprimer"));
    w.e(1).sync().unwrap();
    w.e(1).archive(&id).unwrap();
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(archived_of(&w, 0, &id));
    // Deleted everywhere by A: both devices forget the state.
    w.e(0).delete_everywhere(&id).unwrap();
    assert_eq!(archive_state_of(&w, 0, &id), None);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    assert_eq!(archive_state_of(&w, 1, &id), None);
    // An archive event that comes after the tombstone is ignored.
    w.relay.with(|s| {
        s.push(archive_event(&w, 0, &id, T0 as u64 + 999_999, true), None);
    });
    w.e(1).sync().unwrap();
    assert_eq!(archive_state_of(&w, 1, &id), None);
}

#[test]
fn delete_local_forgets_the_archive_state() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("supprime ici"));
    w.e(1).sync().unwrap();
    w.e(1).archive(&id).unwrap();
    w.e(1).delete_local(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    assert_eq!(archive_state_of(&w, 1, &id), None);
    // Archiving a deleted item does nothing.
    w.e(1).archive(&id).unwrap();
    assert_eq!(archive_state_of(&w, 1, &id), None);
    // The other device keeps its copy; it receives the state decided before the deletion.
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().is_some());
}

#[test]
fn archive_state_is_transferred_with_the_history_secrets_included() {
    let (w, ids, _) = world_with_history();
    // A archives a secret (it was pinned: archiving unpins it) and an ordinary item.
    w.e(0).archive(&ids[1]).unwrap();
    w.e(0).archive(&ids[0]).unwrap();
    let at = w.e(0).get_item(&ids[0]).unwrap().unwrap().archived_at_ms;
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).transfers().unwrap()[0].state, "done");
    let first = w.e(1).get_item(&ids[0]).unwrap().unwrap();
    assert!(first.archived);
    assert_eq!(first.archived_at_ms, at);
    let secret = w.e(1).get_item(&ids[1]).unwrap().unwrap();
    assert!(secret.archived && !secret.pinned);
    assert!(matches!(
        secret.content,
        ItemContentView::Readable { secret: true, .. }
    ));
    assert!(!w.e(1).get_item(&ids[2]).unwrap().unwrap().archived);
    assert_eq!(w.relay.count("read_item"), 0);
}

#[test]
fn a_newer_local_archive_decision_is_not_overwritten_by_an_older_import() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("archive"));
    w.e(1).sync().unwrap();
    w.e(0).archive(&id).unwrap();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert!(archived_of(&w, 1, &id));
    // B unarchives; A has not seen it when it exports its (older) state to B.
    w.clock.advance(10_000);
    w.e(1).unarchive(&id).unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).transfers().unwrap()[0].state, "done");
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(
        !v.archived,
        "the newer local decision wins over the imported one"
    );
    assert_eq!(v.archived_at_ms, None);
    w.e(0).sync().unwrap();
    assert!(!archived_of(&w, 0, &id));
}

// ---------------------------------------------------------------------------
// Cursor, resume, resync
// ---------------------------------------------------------------------------

#[test]
fn cursor_survives_restart_and_resumes_without_replay() {
    let w = World::new(2, 2);
    for i in 0..3 {
        send(&w, 0, text(&format!("m{i}")));
    }
    let r = w.e(1).sync().unwrap();
    assert_eq!(r.new_items, 3);
    assert!(r.cursor >= 3);
    // Restart: a new engine on the same files resumes from the stored cursor.
    let e1 = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert!(e1.is_provisioned());
    send(&w, 0, text("m3"));
    let r = e1.sync().unwrap();
    assert_eq!(r.new_items, 1, "only new events are applied");
    // The 3 acks B itself posted + the new item; the first 3 items are not replayed.
    assert_eq!(r.events_applied, 4);
    assert_eq!(e1.list_items().unwrap().len(), 4);
    assert!(r.cursor >= 9);
}

#[test]
fn event_application_and_cursor_move_together() {
    let w = World::new(2, 2);
    send(&w, 0, text("a"));
    send(&w, 0, text("b"));
    // The database refuses the second item: the cursor must stay on the first event.
    w.e(1).store.tx(|db| {
        db.conn_exec("CREATE TRIGGER stop BEFORE INSERT ON items WHEN (SELECT count(*) FROM items) >= 1 BEGIN SELECT RAISE(ABORT, 'full'); END;")
    }).unwrap();
    let err = w.e(1).sync().unwrap_err();
    assert_eq!(err.code(), "storage");
    let cursor = w.e(1).store.read(|db| db.cursor()).unwrap();
    let items = w.e(1).store.read(|db| db.items()).unwrap();
    assert_eq!(items.len(), 1);
    // The cursor is on the event that was applied, not beyond.
    assert_eq!(items[0].seq, cursor);
}

#[test]
fn page_boundaries_are_followed() {
    let w = World::new(2, 2);
    // More events than one page (200): the sync follows `has_more`.
    w.relay.with(|s| {
        for i in 0..450 {
            s.push(
                EventData::Unknown {
                    kind: format!("future-{i}"),
                },
                None,
            );
        }
    });
    let r = w.e(1).sync().unwrap();
    assert_eq!(r.cursor, w.relay.head());
    assert!(w.relay.count("events") >= 3);
}

#[test]
fn cursor_expired_triggers_a_full_resync_and_keeps_local_items() {
    let w = World::new(2, 2);
    let old = send(&w, 0, text("avant"));
    w.e(1).sync().unwrap();
    // The relay compacts its log; B is behind and misses an item.
    let missed = send(&w, 0, text("manque"));
    let head = w.relay.head();
    send(&w, 0, text("apres"));
    w.relay.compact_to(head);
    let r = w.e(1).sync().unwrap();
    assert!(r.full_resync);
    assert_eq!(r.cursor, w.relay.head());
    assert!(
        w.e(1).get_item(&old).unwrap().is_some(),
        "local items are never deleted"
    );
    assert!(
        w.e(1).get_item(&missed).unwrap().is_none(),
        "items missed are not recovered by the log"
    );
    assert!(w.e(1).take_resync_notice());
    assert!(!w.e(1).take_resync_notice());
    // Syncing keeps working afterwards.
    let later = send(&w, 0, text("encore"));
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&later).unwrap().is_some());
}

#[test]
fn cursor_ahead_after_a_relay_restore_resets_the_cursor() {
    let w = World::new(2, 2);
    send(&w, 0, text("1"));
    send(&w, 0, text("2"));
    w.e(1).sync().unwrap();
    let head = w.relay.head();
    w.relay.rewind_head(head - 3);
    let r = w.e(1).sync().unwrap();
    assert!(r.full_resync);
    assert_eq!(r.cursor, head - 3);
    assert_eq!(w.e(1).list_items().unwrap().len(), 2);
}

#[test]
fn a_relay_that_says_has_more_without_progress_does_not_make_sync_loop() {
    let w = World::new(2, 2);
    send(&w, 0, text("1"));
    w.relay.with(|s| s.stall_events = true);
    // Returns (the test would hang otherwise); nothing is lost, the cursor does not reach the head.
    let r = w.e(1).sync().unwrap();
    assert!(r.cursor < w.relay.head());
    assert_eq!(w.e(1).list_items().unwrap().len(), 0);
    // Once the relay behaves again the next sync recovers everything.
    w.relay.with(|s| s.stall_events = false);
    let r = w.e(1).sync().unwrap();
    assert!(r.cursor >= w.relay.head() - 1);
    assert_eq!(w.e(1).list_items().unwrap().len(), 1);
}

#[test]
fn a_malformed_event_is_logged_and_the_cursor_advances_past_it() {
    let w = World::new(2, 2);
    w.relay.with(|s| {
        s.push(
            EventData::Unknown {
                kind: format!("{MALFORMED_EVENT_PREFIX}item"),
            },
            None,
        )
    });
    let id = send(&w, 0, text("apres"));
    let r = w.e(1).sync().unwrap();
    assert!(
        r.cursor >= w.relay.head() - 1,
        "past the malformed event and the item"
    );
    assert!(w.e(1).get_item(&id).unwrap().is_some());
}

#[test]
fn unknown_event_types_are_skipped_and_the_cursor_advances() {
    let w = World::new(2, 2);
    w.relay.with(|s| {
        s.push(
            EventData::Unknown {
                kind: "from_the_future".into(),
            },
            None,
        )
    });
    let r = w.e(1).sync().unwrap();
    assert_eq!(r.cursor, w.relay.head());
}

#[test]
fn wake_message_triggers_a_sync_only_when_the_log_grew() {
    let w = World::new(2, 2);
    w.e(1).sync().unwrap();
    let cursor = w.e(1).store.read(|db| db.cursor()).unwrap();
    assert!(w
        .e(1)
        .handle_wake(Wake {
            seq: cursor,
            ring: false
        })
        .unwrap()
        .is_none());
    let before = w.relay.count("events");
    let id = send(&w, 0, text("sonnette"));
    let report = w
        .e(1)
        .handle_wake(Wake {
            seq: w.relay.head(),
            ring: true,
        })
        .unwrap()
        .unwrap();
    assert_eq!(report.new_items, 1);
    assert!(w.relay.count("events") > before);
    assert!(w.e(1).get_item(&id).unwrap().is_some());
}

#[test]
fn doorbell_loop_syncs_on_wake_and_reports_the_close() {
    let w = World::new(2, 2);
    let mut source = w.relay.wake_source(&w.nodes[1].id);
    let id = send(&w, 0, text("par la sonnette"));
    // `run_wake_source` catches up first, then handles the pending wake; the silent
    // double ends the connection after a short while.
    let stop = std::sync::atomic::AtomicBool::new(false);
    let closed = w.e(1).run_wake_source(&mut source, &stop);
    assert_eq!(closed.code, None);
    assert!(w.e(1).get_item(&id).unwrap().is_some());
}

#[test]
fn doorbell_loop_syncs_on_wake_and_stops_when_asked() {
    let w = World::new(2, 2);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s2) = (w.nodes[1].engine.clone(), stop.clone());
    let h = std::thread::spawn(move || b.run_doorbell(&s2));
    std::thread::sleep(std::time::Duration::from_millis(300));
    let id = send(&w, 0, text("par la sonnette"));
    let mut got = false;
    for _ in 0..60 {
        if w.e(1).get_item(&id).unwrap().is_some() {
            got = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    h.join().unwrap();
    assert!(got, "the item arrived through the doorbell loop");
}

#[test]
fn doorbell_loop_gives_up_when_the_device_is_revoked() {
    let w = World::new(2, 2);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s2) = (w.nodes[1].engine.clone(), stop.clone());
    let h = std::thread::spawn(move || b.run_doorbell(&s2));
    std::thread::sleep(std::time::Duration::from_millis(300));
    // The double ends a silent connection after 200 ms (then the loop reconnects with a
    // short backoff): keep closing with 4003 until the loop has a connection to close.
    for _ in 0..100 {
        w.relay.close_doorbells(4003);
        if h.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(h.is_finished(), "no reconnection after 4003");
    h.join().unwrap();
    assert_eq!(w.e(1).blocked(), Some(Blocked::Revoked));
    assert!(!stop.load(std::sync::atomic::Ordering::SeqCst));
}

// ---------------------------------------------------------------------------
// Unreadable items are never deleted
// ---------------------------------------------------------------------------

#[test]
fn unreadable_items_are_kept_with_their_reason() {
    let w = World::new(2, 2);
    let mk = |id: &str, envelope: Vec<u8>, epoch: u32| EventData::Item {
        item_id: id.into(),
        sender_device_id: w.nodes[0].id.clone(),
        created_at_ms: T0,
        expires_at_ms: T0 + 100_000,
        key_epoch: epoch,
        envelope,
        blobs: vec![],
        ring: false,
    };
    // Format 2 (unknown to this build).
    let mut newer = vec![0, 0, 0, 2, 0, 0, 0, 1];
    newer.extend_from_slice(&[0xAA; 100]);
    let id_update = crate::new_item_id().unwrap();
    // Valid header of format 1, epoch 1, but random content.
    let id_garbage = crate::new_item_id().unwrap();
    let mut garbage = vec![0, 0, 0, 1, 0, 0, 0, 1];
    garbage.extend_from_slice(&uuid_bytes(&id_garbage));
    garbage.extend_from_slice(&[0x55; 200]);
    // Sealed with an epoch this device does not have yet.
    let id_future = crate::new_item_id().unwrap();
    let k3 = SpaceKey::generate(3).unwrap();
    let future = crate::seal_item(
        &Item {
            id: id_future.clone(),
            source_device: w.nodes[0].id.clone(),
            created_at_ms: T0,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            secret: false,
            title: None,
            text: None,
            url: Some("https://example.org".into()),
            attachments: vec![],
            raw_payload: None,
        },
        &k3,
    )
    .unwrap();
    w.relay.with(|s| {
        s.push(mk(&id_update, newer.clone(), 1), None);
        s.push(mk(&id_garbage, garbage.clone(), 1), None);
        s.push(mk(&id_future, future.clone(), 3), None);
    });
    w.e(1).sync().unwrap();
    let reason = |id: &str| match w.e(1).get_item(id).unwrap().unwrap().content {
        ItemContentView::Unreadable { reason } => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(reason(&id_update), UnreadableReason::UpdateRequired);
    assert_eq!(reason(&id_garbage), UnreadableReason::Undecryptable);
    assert_eq!(reason(&id_future), UnreadableReason::KeyPending);
    // Raw envelopes are kept; they appear in the feed; an ack was queued for each (the
    // sender sees "delivered"), and marking one read does nothing.
    let rows = w.e(1).store.read(|db| db.items()).unwrap();
    assert!(rows
        .iter()
        .all(|r| r.raw_envelope.is_some() && r.sealed.is_none()));
    assert_eq!(
        w.relay.count("ack_item"),
        3,
        "each item was acknowledged (404: unknown to the double)"
    );
    assert!(w.e(1).store.read(|db| db.pending_ops()).unwrap().is_empty());
    w.e(1).mark_read(&id_update).unwrap();
    assert!(w.e(1).get_item(&id_update).unwrap().is_some());
    assert_eq!(w.e(1).list_items().unwrap().len(), 3);
}

pub(super) fn uuid_bytes(s: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(s).unwrap().as_bytes()
}

// ---------------------------------------------------------------------------
// Rotation, revocation, stale epoch
// ---------------------------------------------------------------------------

#[test]
fn revocation_rotates_the_key_and_a_stale_sender_resends_after_sync() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    // A revokes C: new epoch, C is out.
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert_eq!(w.relay.with(|s| s.key_epoch), 2);
    // B has not seen the rotation yet and sends: the relay answers stale_key_epoch, B
    // catches up with the log (applies the rotation) and re-seals the same item.
    let id = w.e(1).enqueue(text("apres rotation")).unwrap();
    assert_eq!(w.e(1).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert!(w.relay.count("post_item") >= 2);
    w.e(0).sync().unwrap();
    assert_eq!(
        plain_of(&w.e(0).get_item(&id).unwrap().unwrap()),
        "apres rotation"
    );
    // The revoked device is told so, and cannot open a session any more.
    assert!(w.e(2).sync().is_err());
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    assert_eq!(w.e(2).space_status(), SpaceStatus::Revoked);
    // The state is persistent.
    let again = w.nodes[2].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.blocked(), Some(Blocked::Revoked));
    // B marks C as revoked after applying the rotation.
    let c = w
        .e(1)
        .list_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.id == w.nodes[2].id)
        .unwrap();
    assert_eq!(c.state, DeviceState::Revoked);
}

#[test]
fn old_epoch_item_in_transit_is_still_read_after_a_rotation() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    let id = send(&w, 0, text("avant"));
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    // B only now processes: the item (epoch 1) comes before the rotation in the log.
    w.e(1).sync().unwrap();
    assert_eq!(plain_of(&w.e(1).get_item(&id).unwrap().unwrap()), "avant");
}

/// An item event sealed under the epoch-1 key of device 0 (which a revoked device still holds),
/// as a hostile relay colluding with that device would forge it.
fn forged_old_epoch_item(w: &World, seq: u64) -> (String, Event) {
    let key = w
        .e(0)
        .read_crypto(|c, _| Ok(c.ring.key(1).cloned().expect("epoch 1 key is kept")))
        .unwrap();
    let id = crate::new_item_id().unwrap();
    let item = Item {
        id: id.clone(),
        source_device: w.nodes[2].id.clone(),
        created_at_ms: T0,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
        secret: false,
        title: Some("Mot de passe change".into()),
        text: None,
        url: Some("https://evil.example/login".into()),
        attachments: vec![],
        raw_payload: None,
    };
    let envelope = crate::envelope::seal_item(&item, &key).unwrap();
    let event = Event {
        seq,
        at_ms: T0,
        data: EventData::Item {
            item_id: id.clone(),
            sender_device_id: w.nodes[2].id.clone(),
            created_at_ms: T0,
            expires_at_ms: T0 + 86_400_000,
            key_epoch: 1,
            envelope,
            blobs: vec![],
            ring: false,
        },
    };
    (id, event)
}

#[test]
fn hostile_relay_cannot_slip_an_old_epoch_item_under_a_low_seq() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    let rotation_seq = w.relay.head();
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).store.read(|db| db.cursor()).unwrap(), rotation_seq);

    // The relay now serves an item sealed under the replaced key, numbered below the rotation
    // (before the fix the epoch rule only looked at that number), and a few legitimate ones.
    let (forged, event) = forged_old_epoch_item(&w, rotation_seq - 1);
    w.relay.inject_events(vec![event]);
    let honest = send(&w, 0, text("vrai"));
    w.e(1).sync().unwrap();
    assert_eq!(
        plain_of(&w.e(1).get_item(&honest).unwrap().unwrap()),
        "vrai"
    );
    assert!(
        w.e(1).get_item(&forged).unwrap().is_none(),
        "an event at or below the cursor is never applied"
    );

    // Same with a number above the cursor: the epoch rule itself applies.
    let (forged2, event2) = forged_old_epoch_item(&w, w.relay.head() + 1);
    w.relay.inject_events(vec![event2]);
    send(&w, 0, text("encore"));
    w.e(1).sync().unwrap();
    let v = w.e(1).get_item(&forged2).unwrap().unwrap();
    assert!(matches!(
        v.content,
        ItemContentView::Unreadable {
            reason: UnreadableReason::SuspectEpoch
        }
    ));
}

#[test]
fn a_replayed_event_is_ignored_and_never_moves_the_cursor_back() {
    let w = World::new(2, 2);
    w.e(1).sync().unwrap();
    let id = send(&w, 0, text("une fois"));
    w.e(1).sync().unwrap();
    let cursor = w.e(1).store.read(|db| db.cursor()).unwrap();
    let replay = w.relay.with(|s| s.events.clone());
    w.relay.inject_events(replay);
    w.e(1).sync().unwrap();
    // The device's own ack event may have moved the log on, never back.
    let after = w.e(1).store.read(|db| db.cursor()).unwrap();
    assert!(after >= cursor);
    assert_eq!(after, w.relay.head());
    assert_eq!(w.e(1).list_items().unwrap().len(), 1);
    assert!(w.e(1).get_item(&id).unwrap().is_some());
}

#[test]
fn a_rewound_relay_log_freezes_the_old_epochs() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    w.e(1).sync().unwrap();
    // The relay claims its log is far shorter than the cursor (restore from a backup, or a lie):
    // the cursor follows it (the only way to hear what comes next)...
    let rewound = w.relay.head() - 2;
    w.relay.rewind_head(rewound);
    // ...and an item of the replaced epoch numbered right after the new head is suspect
    // whatever its number compares to the rotation's.
    let (forged, event) = forged_old_epoch_item(&w, rewound + 1);
    w.relay.inject_events(vec![event]);
    let r = w.e(1).sync().unwrap();
    assert!(r.full_resync);
    let v = w.e(1).get_item(&forged).unwrap().unwrap();
    assert!(matches!(
        v.content,
        ItemContentView::Unreadable {
            reason: UnreadableReason::SuspectEpoch
        }
    ));
}

// ---------------------------------------------------------------------------
// Housekeeping
// ---------------------------------------------------------------------------

#[test]
fn expired_items_are_erased_even_when_pinned() {
    let w = World::new(2, 2);
    let mut d = text("bientot expire");
    d.expires_at_ms = Some(T0 + 60_000);
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    w.e(1).pin(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().pinned);
    w.clock.advance(61_000);
    assert!(
        w.e(1).list_items().unwrap().is_empty(),
        "hidden as soon as it expires"
    );
    w.e(1).sync().unwrap();
    assert!(
        w.e(1).get_item(&id).unwrap().is_none(),
        "erased by the housekeeping"
    );
}

#[test]
fn pinned_item_stays_after_the_relay_purge() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("garde"));
    w.e(1).sync().unwrap();
    w.e(1).pin(&id).unwrap();
    // The relay forgets the log and the item.
    w.relay.compact_to(w.relay.head());
    w.clock.advance(40 * 86_400_000);
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().pinned);
}

#[test]
fn delete_local_removes_the_item_and_its_pin_and_releases_the_relay_copy() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("supprime ici"));
    w.e(1).sync().unwrap();
    w.e(1).pin(&id).unwrap();
    w.e(1).delete_local(&id).unwrap();
    assert!(w.e(1).get_item(&id).unwrap().is_none());
    assert_eq!(
        w.e(1).store.read(|db| db.pin_book()).unwrap().state(&id),
        None
    );
    let acks = w.relay.count("ack_item");
    w.e(1).sync().unwrap();
    assert!(
        w.relay.count("ack_item") > acks,
        "a release ack frees the relay copy"
    );
    // The other device keeps its copy.
    w.e(0).sync().unwrap();
    assert!(w.e(0).get_item(&id).unwrap().is_some());
}

#[test]
fn provisioned_state_survives_a_restart_and_needs_the_right_wrapping_key() {
    let w = World::new(1, 1);
    let id = send(&w, 0, text("persiste"));
    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.device_id().unwrap(), w.nodes[0].id);
    assert_eq!(plain_of(&again.get_item(&id).unwrap().unwrap()), "persiste");
    let wrong = Store::open(
        &w.nodes[0].db_path(),
        &w.nodes[0].dir.path().join("files"),
        &[1u8; 32],
    )
    .unwrap();
    let r = Engine::open(
        EngineConfig::default(),
        wrong,
        w.relay.transport(),
        w.clock.clone(),
    );
    assert_eq!(r.unwrap_err(), crate::CoreError::DecryptionFailed);
}

// ---------------------------------------------------------------------------
// History transfer
// ---------------------------------------------------------------------------

/// A has a history (text, secret, pinned, with a file); B joins afterwards.
fn world_with_history() -> (World, Vec<String>, Vec<u8>) {
    let w = World::new(2, 1);
    w.e(0).sync().unwrap();
    let file = (0..150_000u32)
        .map(|i| (i % 199) as u8)
        .collect::<Vec<u8>>();
    let src = w.nodes[0].dir.path().join("doc.bin");
    std::fs::write(&src, &file).unwrap();
    let mut ids = Vec::new();
    ids.push(send(&w, 0, text("premier")));
    let mut secret = text("mot de passe");
    secret.secret = true;
    ids.push(send(&w, 0, secret));
    let mut with_file = text("avec fichier");
    with_file.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "doc.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    ids.push(send(&w, 0, with_file));
    w.e(0).pin(&ids[1]).unwrap();
    w.e(0).sync().unwrap();
    // B arrives after all of that: the log does not show it the history.
    w.join(1);
    (w, ids, file)
}

#[test]
fn new_device_does_not_see_the_past_without_a_transfer() {
    let (w, _ids, _) = world_with_history();
    w.e(1).sync().unwrap();
    assert!(w.e(1).list_items().unwrap().is_empty());
}

#[test]
fn history_export_and_import_end_to_end() {
    let (w, ids, file) = world_with_history();
    let tid = w.e(0).start_export(&w.nodes[1].id).unwrap();
    let t = w.e(0).transfers().unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].state, "posted");
    assert_eq!(t[0].items_total, 3);
    assert_eq!(w.relay.count("post_transfer"), 1);

    // B syncs: announcement, verification, download, import, files, close.
    let r = w.e(1).sync().unwrap();
    assert!(!r.offline);
    let bt = w.e(1).transfers().unwrap();
    assert_eq!(bt.len(), 1);
    assert_eq!(bt[0].state, "done", "{bt:?}");
    assert_eq!(bt[0].items_done, 3);
    assert_eq!(bt[0].outcome.as_deref(), Some("imported"));
    let feed = w.e(1).list_items().unwrap();
    assert_eq!(feed.len(), 3);
    for id in &ids {
        let v = w.e(1).get_item(id).unwrap().unwrap();
        assert_eq!(v.origin, ItemOrigin::Imported);
        assert!(v.read, "imported items raise no unread badge");
    }
    // Secrets come along, still flagged secret.
    match w.e(1).get_item(&ids[1]).unwrap().unwrap().content {
        ItemContentView::Readable {
            secret,
            text: Some(t),
            ..
        } => {
            assert!(secret);
            assert_eq!(t.plain, "mot de passe");
        }
        other => panic!("{other:?}"),
    }
    // The pin is transferred.
    let pinned = w.e(1).get_item(&ids[1]).unwrap().unwrap();
    assert!(pinned.pinned);
    assert!(!w.e(1).get_item(&ids[0]).unwrap().unwrap().pinned);
    // The file was re-encrypted, transferred and decrypted on B.
    let p = w.e(1).attachment_path(&ids[2], 0).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), file);
    // No ack / read / release was sent for the imported items.
    assert_eq!(w.relay.count("ack_item"), 0);
    assert_eq!(w.relay.count("read_item"), 0);
    // The relay dropped the blobs (transfer closed), and A learns the outcome.
    assert_eq!(
        w.relay.with(|s| s
            .events
            .iter()
            .filter(|e| matches!(e.data, EventData::HistoryTransferClosed { .. }))
            .count()),
        1
    );
    w.e(0).sync().unwrap();
    let at = w.e(0).transfers().unwrap();
    assert_eq!(at[0].state, "done");
    assert_eq!(at[0].transfer_id, tid);
    // Staging directories and keys are gone.
    assert!(!w.nodes[0]
        .dir
        .path()
        .join("files/export")
        .join(&tid)
        .exists());
    assert!(w
        .e(1)
        .store
        .read(|db| db.transfer(&tid))
        .unwrap()
        .unwrap()
        .history_key_sealed
        .is_none());
}

#[test]
fn history_export_resumes_after_a_cut_upload() {
    let (w, _ids, _) = world_with_history();
    w.relay
        .fail_next("put_blob_part", RelayError::Network("cut".into()));
    let tid = w.e(0).start_export(&w.nodes[1].id).unwrap();
    let t = w.e(0).transfers().unwrap();
    assert_eq!(t[0].state, "uploading", "waiting for the network");
    assert_eq!(w.relay.count("post_transfer"), 0);
    // Even a restart of the app keeps the task.
    let a2 = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert_eq!(a2.transfers().unwrap()[0].transfer_id, tid);
    a2.sync().unwrap();
    assert_eq!(a2.transfers().unwrap()[0].state, "posted");
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).list_items().unwrap().len(), 3);
}

#[test]
fn history_import_resumes_after_a_cut_download_and_is_idempotent() {
    let (w, _ids, _) = world_with_history();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.relay
        .fail_next("get_blob_range", RelayError::Network("cut".into()));
    let r = w.e(1).sync().unwrap();
    assert!(!r.offline);
    let st = w.e(1).transfers().unwrap()[0].state.clone();
    assert_eq!(st, "downloading");
    assert!(w.e(1).list_items().unwrap().is_empty());
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).transfers().unwrap()[0].state, "done");
    assert_eq!(w.e(1).list_items().unwrap().len(), 3);
    // Re-running the index import (as after a crash) neither duplicates nor fails.
    assert_eq!(w.e(1).store.read(|db| db.items()).unwrap().len(), 3);
}

#[test]
fn history_import_waits_for_acceptance_when_auto_import_is_off() {
    let (w, _ids, _) = world_with_history();
    // Reopen B with auto_import = false on the same files.
    let store = Store::open(
        &w.nodes[1].db_path(),
        &w.nodes[1].dir.path().join("files"),
        &[9u8; 32],
    )
    .unwrap();
    let b = Engine::open(
        EngineConfig {
            auto_import: false,
            ..EngineConfig::default()
        },
        store,
        w.relay.transport(),
        w.clock.clone(),
    )
    .unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    b.sync().unwrap();
    let t = b.transfers().unwrap();
    assert_eq!(t[0].state, "received");
    assert!(b.list_items().unwrap().is_empty());
    b.accept_import(&t[0].transfer_id).unwrap();
    assert_eq!(b.transfers().unwrap()[0].state, "done");
    assert_eq!(b.list_items().unwrap().len(), 3);
}

#[test]
fn history_import_can_be_declined() {
    let (w, _ids, _) = world_with_history();
    let store = Store::open(
        &w.nodes[1].db_path(),
        &w.nodes[1].dir.path().join("files"),
        &[9u8; 32],
    )
    .unwrap();
    let b = Engine::open(
        EngineConfig {
            auto_import: false,
            ..EngineConfig::default()
        },
        store,
        w.relay.transport(),
        w.clock.clone(),
    )
    .unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    b.sync().unwrap();
    let id = b.transfers().unwrap()[0].transfer_id.clone();
    b.decline_import(&id).unwrap();
    assert_eq!(b.transfers().unwrap()[0].state, "declined");
    assert!(b.list_items().unwrap().is_empty());
    w.e(0).sync().unwrap();
    assert_eq!(
        w.e(0).transfers().unwrap()[0].state,
        "failed",
        "the sender is told it was not imported"
    );
}

#[test]
fn imported_tombstoned_items_are_skipped_and_pins_merge_into_existing_items() {
    let (w, ids, _) = world_with_history();
    // B learns of a deletion (tombstone from A) of the first item *before* importing.
    w.e(0).delete_everywhere(&ids[0]).unwrap();
    w.e(0).sync().unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    w.e(1).sync().unwrap();
    // The exported feed no longer holds the deleted item at all (A erased it).
    assert!(w.e(1).get_item(&ids[0]).unwrap().is_none());
    assert_eq!(w.e(1).list_items().unwrap().len(), 2);
    // A tombstone seen by B before the import blocks the import of that id.
    let (w2, ids2, _) = world_with_history();
    w2.e(1)
        .store
        .tx(|db| db.put_tombstone(&ids2[0], T0, true, T0))
        .unwrap();
    w2.e(0).start_export(&w2.nodes[1].id).unwrap();
    w2.e(1).sync().unwrap();
    assert!(w2.e(1).get_item(&ids2[0]).unwrap().is_none());
    assert_eq!(w2.e(1).list_items().unwrap().len(), 2);
}

#[test]
fn history_transfer_is_refused_to_an_unknown_recipient_or_oneself() {
    let (w, _ids, _) = world_with_history();
    assert!(w.e(0).start_export(&w.nodes[0].id).is_err());
    assert!(w
        .e(0)
        .start_export("00000000000000000000000000000000")
        .is_err());
}

#[test]
fn tampered_transfer_manifest_is_rejected_by_the_recipient() {
    let (w, _ids, _) = world_with_history();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    // The relay (or an attacker) alters the stored manifest.
    let tid = w.e(0).transfers().unwrap()[0].transfer_id.clone();
    w.relay.corrupt_manifest(&tid);
    w.e(1).sync().unwrap();
    let t = w.e(1).transfers().unwrap();
    assert_eq!(t[0].state, "failed");
    assert!(w.e(1).list_items().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Data at rest, bounded sync, listeners (review of the runtime)
// ---------------------------------------------------------------------------

const MARKER: &[u8] = b"<<PIECE-JOINTE-EN-CLAIR>>";

fn marked_file(len: usize) -> Vec<u8> {
    MARKER.iter().copied().cycle().take(len).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Files kept by the engine of `n` (database and files directory) whose bytes contain
/// `needle`. Draft sources written by a test at the root of the directory are not the
/// engine's.
fn holding(n: &Node, needle: &[u8]) -> Vec<std::path::PathBuf> {
    let mut all = Vec::new();
    walk(&n.dir.path().join("files"), &mut all);
    for f in ["tomi.db", "tomi.db-wal"] {
        all.push(n.dir.path().join(f));
    }
    all.into_iter()
        .filter(|p| std::fs::read(p).is_ok_and(|b| contains(&b, needle)))
        .collect()
}

fn local_path_of(e: &Engine, id: &str) -> Option<String> {
    match e.get_item(id).unwrap().unwrap().content {
        ItemContentView::Readable { attachments, .. } => attachments[0].local_path.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn attachments_are_never_in_clear_at_rest() {
    let w = World::new(2, 2);
    let data = marked_file(300_000);
    let outside = tempfile::tempdir().unwrap();
    let src = outside.path().join("secret.pdf");
    std::fs::write(&src, &data).unwrap();
    let mut d = text("fichier secret");
    d.secret = true;
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "secret.pdf".into(),
        mime: "application/pdf".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();

    // Sender: only the encrypted copy, which is also the upload.
    assert!(
        holding(&w.nodes[0], MARKER).is_empty(),
        "sender, after enqueue"
    );
    let enc0 = w.e(0).store.file_path(&format!("files/{id}_0.enc"));
    assert_eq!(
        std::fs::metadata(&enc0).unwrap().len(),
        crate::encrypted_attachment_len(data.len() as u64)
    );
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    assert!(enc0.exists(), "the sender keeps its encrypted copy");
    assert!(
        holding(&w.nodes[0], MARKER).is_empty(),
        "sender, after upload"
    );

    // Receiver: downloaded and kept encrypted, decrypted on demand into the cache only.
    w.e(1).sync().unwrap();
    assert!(
        holding(&w.nodes[1], MARKER).is_empty(),
        "receiver, after sync"
    );
    assert_eq!(local_path_of(w.e(1), &id), None);
    let p = w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), data);
    let cache = w.e(1).store.file_path(super::store::CACHE_DIR);
    let clear = holding(&w.nodes[1], MARKER);
    assert_eq!(clear.len(), 1, "{clear:?}");
    assert!(
        clear[0].starts_with(&cache),
        "plaintext outside the cache: {clear:?}"
    );
    let enc1 = w.e(1).store.file_path(&format!("files/{id}_0.enc"));
    assert!(enc1.exists());
    assert_eq!(
        local_path_of(w.e(1), &id).as_deref(),
        Some(p.to_string_lossy().as_ref())
    );

    // Going to the background purges the cache: no plaintext left on the disk.
    w.e(1).purge_plaintext_cache().unwrap();
    assert!(
        holding(&w.nodes[1], MARKER).is_empty(),
        "receiver, after purge"
    );
    assert_eq!(local_path_of(w.e(1), &id), None);
    // Decrypted again on demand, without the network.
    w.relay.set_offline(true);
    let again = w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), data);
    w.relay.set_offline(false);

    // Deleting the item erases both copies.
    w.e(1).delete_local(&id).unwrap();
    assert!(!enc1.exists());
    assert!(!again.exists());
    assert!(holding(&w.nodes[1], MARKER).is_empty());

    // A decrypted copy left by a killed process is purged at the next start.
    let own = w.e(0).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(&own).unwrap(), data);
    let _restarted = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert!(!own.exists());
    assert!(holding(&w.nodes[0], MARKER).is_empty());
}

#[test]
fn imported_files_are_kept_encrypted_and_decrypted_on_demand() {
    let (w, ids, file) = world_with_history();
    let needle = &file[1000..1100];
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    // Re-encrypting for the transfer never writes the plaintext.
    assert!(holding(&w.nodes[0], needle).is_empty());
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).transfers().unwrap()[0].state, "done");
    assert!(
        holding(&w.nodes[1], needle).is_empty(),
        "plaintext at rest after the import"
    );
    assert!(w
        .e(1)
        .store
        .file_path(&format!("files/{}_0.enc", ids[2]))
        .exists());
    let p = w.e(1).attachment_path(&ids[2], 0).unwrap().unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), file);
    w.e(1).purge_plaintext_cache().unwrap();
    assert!(holding(&w.nodes[1], needle).is_empty());
}

#[test]
fn database_files_hold_no_key_and_no_title_in_clear() {
    let w = World::new(2, 2);
    let title = "Titre-zorglub-confidentiel";
    let mut d = text("corps");
    d.title = Some(title.into());
    d.url = Some("https://zorglub-url.example/x".into());
    d.secret = true;
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    w.e(1).pin(&id).unwrap();
    w.e(1).sync().unwrap();
    w.e(0).sync().unwrap();
    for (i, n) in w.nodes.iter().enumerate() {
        let lsk = n.engine.active().unwrap().lsk.to_bytes();
        let space = n
            .engine
            .read_crypto(|c, _| Ok(c.ring.current()?.to_bytes()))
            .unwrap();
        assert_eq!(
            (lsk.len(), space.len(), n.seed.len(), n.x.len()),
            (32, 32, 32, 32)
        );
        let needles: [(&str, &[u8]); 6] = [
            ("ed25519 seed", &n.seed),
            ("x25519 secret", &n.x),
            ("local key LSK", &lsk),
            ("space key", &space),
            ("title", title.as_bytes()),
            ("url", b"zorglub-url"),
        ];
        for f in ["tomi.db", "tomi.db-wal"] {
            let bytes = std::fs::read(n.dir.path().join(f)).unwrap_or_default();
            for (what, needle) in needles {
                assert!(
                    !contains(&bytes, needle),
                    "{what} in clear in {f} of node {i}"
                );
            }
        }
    }
}

#[test]
fn unknown_payload_fields_survive_reception_local_storage_and_export() {
    let w = World::new(3, 2);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    // An item written by a newer device: an additive field this build does not know.
    let id = crate::new_item_id().unwrap();
    let known = Item {
        id: id.clone(),
        source_device: w.nodes[0].id.clone(),
        created_at_ms: T0,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
        secret: false,
        title: Some("du futur".into()),
        text: None,
        url: None,
        attachments: vec![],
        raw_payload: None,
    };
    let mut json: serde_json::Value = serde_json::from_slice(&known.to_json().unwrap()).unwrap();
    json["champ_du_futur"] = serde_json::json!({ "niveau": 2, "liste": [1, 2] });
    let item = Item::from_json_unchecked_id(&serde_json::to_vec(&json).unwrap()).unwrap();
    let envelope = w
        .e(0)
        .read_crypto(|c, _| crate::envelope::seal_item(&item, c.ring.current()?))
        .unwrap();
    w.e(0).ensure_session().unwrap();
    w.e(0)
        .transport
        .post_item(&PostItem {
            item_id: id.clone(),
            expires_at_ms: T0 + 86_400_000,
            key_epoch: 1,
            envelope,
            blob_ids: vec![],
        })
        .unwrap();
    let stored = |e: &Engine| -> Vec<u8> {
        let row = e.store.read(|db| db.item(&id)).unwrap().unwrap();
        let item = e
            .active()
            .unwrap()
            .lsk
            .open_item(row.sealed.as_ref().unwrap(), &id)
            .unwrap();
        item.raw_payload.unwrap().as_bytes().to_vec()
    };
    // B receives it and re-seals it under its local key: the field is kept.
    w.e(1).sync().unwrap();
    assert!(contains(&stored(w.e(1)), b"champ_du_futur"));
    // C joins; B exports its history to C: the field crosses the transfer as well.
    w.join(2);
    w.e(1).start_export(&w.nodes[2].id).unwrap();
    w.e(2).sync().unwrap();
    assert_eq!(
        w.e(2).get_item(&id).unwrap().unwrap().origin,
        ItemOrigin::Imported
    );
    let on_c: serde_json::Value = serde_json::from_slice(&stored(w.e(2))).unwrap();
    assert_eq!(on_c["champ_du_futur"]["niveau"], 2);
    assert_eq!(on_c["champ_du_futur"]["liste"], serde_json::json!([1, 2]));
}

#[test]
fn sync_terminates_on_an_unresolvable_stale_key_epoch() {
    use super::outbox::MAX_STALE_ATTEMPTS;
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    // B's item reaches A before things go wrong: A's ack for it must still leave.
    let other = send(&w, 1, text("avant la panne"));
    // The relay now wants an epoch no rotation leads to (refused rotation, unknown
    // issuer, buggy relay).
    w.relay.with(|s| s.key_epoch = 7);
    let id = w.e(0).enqueue(text("jamais accepte")).unwrap();
    let (posts, acks) = (w.relay.count("post_item"), w.relay.count("ack_item"));
    let r = w.e(0).sync().unwrap();
    assert!(!r.offline);
    let tried = w.relay.count("post_item") - posts;
    assert_eq!(
        tried, 2,
        "one attempt, one catch-up sync, one retry: then wait"
    );
    assert!(
        w.relay.count("ack_item") > acks,
        "the other operations go on"
    );
    assert!(w.e(0).get_item(&other).unwrap().is_some());
    let op = w
        .e(0)
        .store
        .read(|db| db.ops_for_item(&id))
        .unwrap()
        .remove(0);
    assert_eq!(op.stale_attempts, 2);
    assert!(
        op.next_attempt_at_ms > w.clock_now(),
        "deferred with a backoff"
    );
    // dispatch returns as well (it calls sync).
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::RetryLater);
    // Not due yet: a new sync does not hammer the relay.
    let posts = w.relay.count("post_item");
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_item"), posts);
    // Past the threshold the operation is given up: failed, and nothing else blocked.
    for _ in 0..MAX_STALE_ATTEMPTS {
        w.clock.advance(600_000);
        w.e(0).sync().unwrap();
    }
    assert_eq!(
        w.e(0).get_item(&id).unwrap().unwrap().send_status,
        Some(SendStatus::Failed)
    );
    assert!(w
        .e(0)
        .store
        .read(|db| db.ops_for_item(&id))
        .unwrap()
        .is_empty());
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Failed);
    let posts = w.relay.count("post_item");
    w.clock.advance(600_000);
    assert_eq!(w.e(0).sync().unwrap().outbox_pending, 0);
    assert_eq!(w.relay.count("post_item"), posts);
}

#[test]
fn a_newer_local_pin_is_not_overwritten_by_an_older_import() {
    let w = World::new(2, 2);
    let id = send(&w, 0, text("epingle"));
    w.e(1).sync().unwrap();
    w.e(0).pin(&id).unwrap();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert!(w.e(1).get_item(&id).unwrap().unwrap().pinned);
    // Later, B unpins; A has not seen it when it exports its (older) state to B.
    w.clock.advance(10_000);
    w.e(1).unpin(&id).unwrap();
    w.e(0).start_export(&w.nodes[1].id).unwrap();
    assert!(w.e(0).get_item(&id).unwrap().unwrap().pinned);
    w.e(1).sync().unwrap();
    assert_eq!(w.e(1).transfers().unwrap()[0].state, "done");
    let v = w.e(1).get_item(&id).unwrap().unwrap();
    assert!(
        !v.pinned,
        "the newer local decision wins over the imported one"
    );
    assert_eq!(v.pinned_at_ms, None);
    // And A converges to it through the log.
    w.e(0).sync().unwrap();
    assert!(!w.e(0).get_item(&id).unwrap().unwrap().pinned);
}

#[test]
fn a_replayed_auth_challenge_is_refused_by_the_double() {
    let w = World::new(1, 1);
    let t = w.relay.transport();
    let n = &w.nodes[0];
    let c = t.auth_challenge(&n.id).unwrap();
    let req = SessionRequest {
        device_id: n.id.clone(),
        signature: n.keys().sign_relay_auth(&w.space_id, &c.challenge).unwrap(),
        challenge: c.challenge,
        client: ClientInfo {
            format_min: crate::FORMAT_MIN,
            format_max: crate::FORMAT_MAX,
            app_version: "0.1.0".into(),
            platform: "cli".into(),
        },
    };
    assert!(t.auth_session(&req).is_ok());
    let replay = t.auth_session(&req).unwrap_err();
    assert!(
        matches!(&replay, RelayError::Api(p) if p.status == 401 && p.code == "unauthorized"),
        "{replay:?}"
    );
}

#[test]
fn a_failing_or_reentrant_listener_neither_poisons_nor_deadlocks_the_engine() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let w = World::new(2, 2);
    let e = w.nodes[0].engine.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let under_lock = Arc::new(AtomicBool::new(false));
    let weak = Arc::downgrade(&e);
    let (c, u) = (calls.clone(), under_lock.clone());
    e.subscribe(Arc::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = weak.upgrade() {
            // Only this thread uses the engine: a busy lock means it is held right here.
            let held = e.dispatch_lock.try_lock().is_err()
                || e.transfer_lock.try_lock().is_err()
                || e.sync_lock.try_lock().is_err()
                || e.inner.try_lock().is_err()
                || e.flags.try_lock().is_err();
            if held {
                u.store(true, Ordering::SeqCst);
            }
            // Re-entering the engine from the callback is safe.
            let _ = e.list_items();
        }
        // Like an exception thrown by the app's callback.
        panic!("listener failure (expected by this test)");
    }));
    let id = e.enqueue(text("malgre le listener")).unwrap();
    assert_eq!(e.dispatch(&id).unwrap(), DispatchOutcome::Sent);
    let id2 = e.enqueue(text("annule")).unwrap();
    assert!(e.cancel_if_queued(&id2).unwrap());
    e.delete_everywhere(&id).unwrap();
    assert!(!e.sync().unwrap().offline);
    assert!(calls.load(Ordering::SeqCst) >= 5);
    assert!(
        !under_lock.load(Ordering::SeqCst),
        "a listener ran under an engine lock"
    );
    // Nothing is poisoned: the engine keeps working.
    let id3 = e.enqueue(text("encore")).unwrap();
    assert_eq!(e.dispatch(&id3).unwrap(), DispatchOutcome::Sent);
    e.delete_local(&id3).unwrap();
}

#[test]
fn an_imported_unread_item_can_be_marked_read_locally_only() {
    let w = World::new(3, 2);
    w.e(0).sync().unwrap();
    let id = send(&w, 1, text("pas encore lu"));
    w.e(0).sync().unwrap();
    assert!(!w.e(0).get_item(&id).unwrap().unwrap().read);
    w.join(2);
    w.e(0).start_export(&w.nodes[2].id).unwrap();
    w.e(2).sync().unwrap();
    let v = w.e(2).get_item(&id).unwrap().unwrap();
    assert_eq!(v.origin, ItemOrigin::Imported);
    assert!(!v.read, "still unread on the exporting device");
    let (reads, acks) = (w.relay.count("read_item"), w.relay.count("ack_item"));
    w.e(2).mark_read(&id).unwrap();
    assert!(w.e(2).get_item(&id).unwrap().unwrap().read);
    w.e(2).sync().unwrap();
    assert_eq!(w.relay.count("read_item"), reads, "no read receipt");
    assert_eq!(w.relay.count("ack_item"), acks, "no ack, no release");
    assert!(w.e(2).store.read(|db| db.pending_ops()).unwrap().is_empty());
}

#[test]
fn the_key_epoch_of_an_item_comes_from_its_envelope_not_from_the_relay() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    let id = crate::new_item_id().unwrap();
    let item = Item {
        id: id.clone(),
        source_device: w.nodes[0].id.clone(),
        created_at_ms: T0,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
        secret: false,
        title: Some("epoque".into()),
        text: None,
        url: None,
        attachments: vec![],
        raw_payload: None,
    };
    let envelope = w
        .e(0)
        .read_crypto(|c, _| crate::envelope::seal_item(&item, c.ring.current()?))
        .unwrap();
    // A lying relay announces another epoch for it.
    let sender = w.nodes[0].id.clone();
    w.relay.with(|s| {
        s.push(
            EventData::Item {
                item_id: id.clone(),
                sender_device_id: sender,
                created_at_ms: T0,
                expires_at_ms: T0 + 86_400_000,
                key_epoch: 42,
                envelope,
                blobs: vec![],
                ring: false,
            },
            None,
        );
    });
    w.e(1).sync().unwrap();
    let row = w.e(1).store.read(|db| db.item(&id)).unwrap().unwrap();
    assert_eq!(row.key_epoch, 1);
    assert_eq!(row.status, super::store::ItemStatus::Readable);
}

#[test]
fn an_upload_that_does_not_advance_is_retried_later_not_looped() {
    let w = World::new(2, 2);
    let src = w.nodes[0].dir.path().join("f.bin");
    std::fs::write(&src, vec![3u8; 50_000]).unwrap();
    let mut d = text("bloque");
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "f.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = w.e(0).enqueue(d).unwrap();
    w.relay.with(|s| s.stall_uploads = true);
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::RetryLater);
    assert_eq!(w.relay.count("put_blob_part"), 1);
    w.relay.with(|s| s.stall_uploads = false);
    w.clock.advance(600_000);
    assert_eq!(w.e(0).dispatch(&id).unwrap(), DispatchOutcome::Sent);
    w.e(1).sync().unwrap();
    let p = w.e(1).attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), vec![3u8; 50_000]);
}

#[test]
fn plaintext_files_of_an_older_build_are_erased_at_start_and_fetched_again() {
    let w = World::new(2, 2);
    let data = marked_file(40_000);
    let src = w.nodes[0].dir.path().join("ancien.bin");
    std::fs::write(&src, &data).unwrap();
    let mut d = text("ancien format");
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "ancien.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let id = send(&w, 0, d);
    w.e(1).sync().unwrap();
    // What an older build left: the decrypted file at rest, marked done.
    let legacy = format!("files/{id}_0");
    std::fs::write(w.e(1).store.file_path(&legacy), &data).unwrap();
    w.e(1)
        .store
        .tx(|db| {
            let f = db.files(&id)?.remove(0);
            db.put_file(&super::store::FileRow {
                state: super::store::FileState::Done,
                path: Some(legacy.clone()),
                ..f
            })
        })
        .unwrap();
    let b = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert!(holding(&w.nodes[1], MARKER).is_empty());
    let f = b.store.read(|db| db.files(&id)).unwrap().remove(0);
    assert_eq!(f.state, super::store::FileState::Remote);
    let p = b.attachment_path(&id, 0).unwrap().unwrap();
    assert_eq!(std::fs::read(p).unwrap(), data);
}

// ---------------------------------------------------------------------------
// Leaving the space and deleting it (spec/api-v1.md §7.9)
// ---------------------------------------------------------------------------

use super::store::WipeGuard;
use super::{DepartureKind as K, DepartureOutcome as O, PendingReason as P, RefusalReason as R};

const MIN: i64 = 60_000;

fn leave_state(e: &Engine) -> Option<Vec<u8>> {
    e.store.read(|db| db.meta_get("leave_state")).unwrap()
}

fn requested(e: &Engine) -> bool {
    leave_state(e).as_deref() == Some(b"requested".as_slice())
}

fn deleting(e: &Engine) -> bool {
    leave_state(e).as_deref() == Some(b"deleting".as_slice())
}

fn stored_deletion_id(e: &Engine) -> Option<Vec<u8>> {
    e.store.read(|db| db.meta_get("deletion_id")).unwrap()
}

/// Every table is empty (`sync_state` keeps its single row), and so is `meta`.
fn assert_erased(e: &Engine) {
    for (name, count) in e.store.row_counts() {
        let want = i64::from(name == "sync_state");
        assert_eq!(count, want, "table {name}");
    }
}

/// Nothing was erased: provisioned, the items, the device keys and the space keys are there.
fn assert_intact(e: &Engine, items: usize) {
    assert!(e.is_provisioned());
    assert_eq!(e.list_items().unwrap().len(), items);
    assert!(e
        .store
        .read(|db| db.raw_secret_record("device_keys"))
        .unwrap()
        .is_some());
    assert!(e
        .store
        .row_counts()
        .iter()
        .any(|(n, c)| n == "space_keys" && *c > 0));
}

fn assert_files_dirs_empty(n: &Node) {
    for sub in ["files", "files/cache", "outbox", "dl", "export", "import"] {
        let dir = n.dir.path().join("files").join(sub);
        assert!(dir.is_dir(), "{sub} is recreated");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            usize::from(sub == "files")
        );
    }
}

fn departures(w: &World) -> Vec<(String, String)> {
    w.relay
        .events()
        .into_iter()
        .filter_map(|ev| match ev.data {
            EventData::DeviceRevoked { device_id, via } => Some((device_id, via)),
            _ => None,
        })
        .collect()
}

fn problem(status: u16, code: &str) -> RelayError {
    RelayError::api(status, code)
}

/// Writes a departure marker, as a stop right after the gesture would leave it.
fn write_marker(e: &Engine, marker: &[u8], since: Option<i64>) {
    e.store
        .tx(|db| {
            db.meta_set("leave_state", marker)?;
            if let Some(t) = since {
                db.meta_set("departure_since", t.to_string().as_bytes())?;
            }
            Ok(())
        })
        .unwrap();
}

/// Two members; device 1 holds one item.
fn pair_with_item() -> World {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    send(&w, 0, text("a garder"));
    w.e(1).sync().unwrap();
    w
}

/// The only member, holding one item, after "Sign out" was refused (`last_device`).
fn alone_after_last_device() -> World {
    let w = World::new(1, 1);
    send(&w, 0, text("seul exemplaire"));
    assert_eq!(w.e(0).leave_space().unwrap(), O::Refused(R::LastDevice));
    assert_eq!(leave_state(w.e(0)), None);
    w
}

/// `b` for the identity the engine holds now (what an answer of the relay does).
fn mark_blocked(e: &Engine, b: Blocked) {
    let live = e.active().unwrap();
    e.set_blocked(&live, b);
}

/// "Not found" for the identity the engine holds now.
fn mark_not_found(e: &Engine) {
    let live = e.active().unwrap();
    e.set_not_found(&live);
}

fn refused_input(r: crate::Result<()>) -> bool {
    matches!(r, Err(crate::CoreError::InvalidInput { .. }))
}

#[test]
fn leaving_revokes_this_device_and_erases_everything_local() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    let text_id = send(&w, 0, text("avant le depart"));
    let mut secret = text("mot de passe");
    secret.secret = true;
    send(&w, 0, secret);
    let src = w.nodes[0].dir.path().join("piece.bin");
    std::fs::write(&src, marked_file(50_000)).unwrap();
    let mut with_file = text("avec fichier");
    with_file.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "piece.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    let file_id = send(&w, 0, with_file);
    w.e(0).pin(&text_id).unwrap();
    w.e(0).sync().unwrap();
    for i in 1..3 {
        w.e(i).sync().unwrap();
    }
    assert_eq!(w.e(2).list_items().unwrap().len(), 3);
    // The decrypted copy of an attachment is in the cache.
    let cached = w.e(2).attachment_path(&file_id, 0).unwrap().unwrap();
    assert!(cached.exists());
    assert!(!holding(&w.nodes[2], &marked_file(50_000)[..1000]).is_empty());

    assert_eq!(w.e(2).leave_space().unwrap(), O::Erased);

    assert_eq!(w.relay.count("leave_space"), 1);
    assert_eq!(
        departures(&w),
        vec![(w.nodes[2].id.clone(), "self".to_owned())]
    );
    assert!(!w.e(2).is_provisioned());
    assert!(w.e(2).list_items().is_err());
    assert!(!cached.exists());
    assert_erased(w.e(2));
    assert_eq!(leave_state(w.e(2)), None);
    assert!(w.e(2).departure().is_none());
    assert_files_dirs_empty(&w.nodes[2]);
    assert!(holding(&w.nodes[2], &marked_file(50_000)[..1000]).is_empty());
    // The others keep everything and see the departure.
    w.e(0).sync().unwrap();
    assert_eq!(w.e(0).list_items().unwrap().len(), 3);
    let left = w
        .e(0)
        .list_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.id == w.nodes[2].id)
        .unwrap();
    assert_eq!(left.state, DeviceState::RevokedByRelay);

    // After a restart: not provisioned, nothing pending, nothing to settle.
    let again = w.nodes[2].reopen(&w.relay, w.clock.clone());
    assert!(!again.is_provisioned());
    assert!(!again.has_pending_pairing().unwrap());
    assert_eq!(again.continue_departure().unwrap(), O::NothingPending);
    assert_erased(&again);
}

// --- T1: the fresh session --------------------------------------------------

#[test]
fn t1_an_open_session_goes_on_to_the_request() {
    let w = pair_with_item();
    let sessions = w.relay.count("auth_session");
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    assert_eq!(
        w.relay.count("auth_session"),
        sessions + 1,
        "always a fresh session, even with one in memory"
    );
    assert_eq!(w.relay.count("leave_space"), 1);
    assert_erased(w.e(1));
    assert!(!w.e(1).can_erase_without_proof());
}

#[test]
fn t1_revoked_at_the_fresh_session_erases_and_the_kept_state_is_asked_again() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    send(&w, 0, text("avant la revocation"));
    w.e(2).sync().unwrap();
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert!(w.e(2).sync().is_err());
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    // Revoked by someone else: the data are still there.
    assert_intact(w.e(2), 1);
    let sessions = w.relay.count("auth_session");

    assert_eq!(w.e(2).leave_space().unwrap(), O::Erased);

    assert!(
        w.relay.count("auth_session") > sessions,
        "the persisted revocation was asked again"
    );
    assert_eq!(w.relay.count("leave_space"), 0);
    assert_erased(w.e(2));
    assert_files_dirs_empty(&w.nodes[2]);
}

#[test]
fn a_persisted_revocation_that_the_relay_denies_goes_on_with_the_request() {
    let w = pair_with_item();
    // A wrong local state: the relay still counts this device as a member.
    mark_blocked(w.e(1), Blocked::Revoked);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.blocked(), Some(Blocked::Revoked));
    assert_eq!(again.leave_space().unwrap(), O::Erased);
    assert_eq!(w.relay.count("leave_space"), 1, "DELETE /v1/me was sent");
    assert_eq!(
        departures(&w),
        vec![(w.nodes[1].id.clone(), "self".to_owned())]
    );
    assert_erased(&again);
}

#[test]
fn t1_unauthorized_twice_keeps_the_departure_pending_as_not_found() {
    let w = pair_with_item();
    w.relay.forget_device(&w.nodes[1].id);
    let sessions = w.relay.count("auth_session");
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert_eq!(
        w.relay.count("auth_session"),
        sessions + 2,
        "a second fresh challenge"
    );
    assert_eq!(w.relay.count("leave_space"), 0);
    assert!(requested(w.e(1)), "never dropped in silence on a 401");
    assert_intact(w.e(1), 1);
    assert!(w.e(1).can_erase_without_proof());
    assert_eq!(w.e(1).space_status(), SpaceStatus::NotFound);
}

#[test]
fn t1_space_not_initialized_keeps_the_departure_pending_as_not_found() {
    let w = pair_with_item();
    w.relay.uninitialize();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(w.e(1).can_erase_without_proof());
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None,
        "never persisted"
    );
}

#[test]
fn t1_update_required_refuses_and_drops_the_marker() {
    for (status, code) in [
        (409, "update_required"),
        (409, "format_unsupported"),
        (410, "update_required"),
    ] {
        let w = pair_with_item();
        w.relay.fail_next("auth_session", problem(status, code));
        assert_eq!(
            w.e(1).leave_space().unwrap(),
            O::Refused(R::UpdateRequired),
            "{status} {code}"
        );
        assert_eq!(leave_state(w.e(1)), None);
        assert_intact(w.e(1), 1);
        assert!(!w.e(1).can_erase_without_proof());
        assert_eq!(w.relay.count("leave_space"), 0);
    }
}

#[test]
fn t1_server_errors_keep_the_departure_pending() {
    for (method, status, code) in [
        ("auth_session", 503, "unavailable"),
        ("auth_session", 500, "internal"),
        ("auth_challenge", 502, "bad_gateway"),
        ("auth_session", 429, "rate_limited"),
    ] {
        let w = pair_with_item();
        w.relay.fail_next(method, problem(status, code));
        assert_eq!(
            w.e(1).leave_space().unwrap(),
            O::Pending(P::Server),
            "{status}"
        );
        assert!(requested(w.e(1)));
        assert_intact(w.e(1), 1);
        assert!(!w.e(1).can_erase_without_proof());
        assert_eq!(w.relay.count("leave_space"), 0);
    }
}

#[test]
fn t1_network_errors_keep_the_departure_pending_and_visible() {
    let w = pair_with_item();
    w.relay.set_offline(true);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(!w.e(1).can_erase_without_proof());
    let view = w.e(1).departure().unwrap();
    assert_eq!(
        (view.kind, view.wiping, view.since_ms, view.last),
        (K::Leave, false, Some(T0), Some(P::Network))
    );
}

#[test]
fn t1_another_4xx_refuses_and_drops_the_marker() {
    let w = pair_with_item();
    w.relay
        .fail_next("auth_session", problem(400, "invalid_request"));
    assert_eq!(w.e(1).leave_space().unwrap(), O::Refused(R::Unexpected));
    assert_eq!(leave_state(w.e(1)), None);
    assert_intact(w.e(1), 1);
    assert!(!w.e(1).can_erase_without_proof());
}

// --- T2: DELETE /v1/me --------------------------------------------------------

#[test]
fn t2_ok_with_seq_erases() {
    let w = pair_with_item();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    assert_eq!(
        departures(&w),
        vec![(w.nodes[1].id.clone(), "self".to_owned())]
    );
    assert_erased(w.e(1));
    assert_eq!(leave_state(w.e(1)), None);
}

#[test]
fn t2_an_answer_without_seq_proves_nothing() {
    let w = pair_with_item();
    // What the HTTP transport makes of a `2xx` without an integer `seq` (see http.rs).
    w.relay.fail_next(
        "leave_space",
        RelayError::Network("unexpected answer: leave without seq".into()),
    );
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(!w.e(1).can_erase_without_proof());
}

#[test]
fn t2_revoked_erases() {
    let w = pair_with_item();
    w.relay.fail_next("leave_space", problem(403, "revoked"));
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    assert_erased(w.e(1));
}

#[test]
fn t2_last_device_refuses_and_keeps_everything() {
    let w = alone_after_last_device();
    assert_intact(w.e(0), 1);
    assert_eq!(w.e(0).blocked(), None);
    assert!(!w.e(0).can_erase_without_proof());
    assert!(departures(&w).is_empty());
    assert!(w.e(0).departure().is_none());
}

#[test]
fn t2_unauthorized_after_a_renewal_or_space_not_initialized_is_not_found() {
    let w = pair_with_item();
    w.relay
        .fail_next("leave_space", problem(401, "unauthorized"));
    w.relay
        .fail_next("leave_space", problem(401, "unauthorized"));
    let sessions = w.relay.count("auth_session");
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert_eq!(
        w.relay.count("auth_session"),
        sessions + 2,
        "the fresh session, then one renewal"
    );
    assert_eq!(w.relay.count("leave_space"), 2);
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(w.e(1).can_erase_without_proof());

    let w = pair_with_item();
    w.relay
        .fail_next("leave_space", problem(409, "space_not_initialized"));
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(w.e(1).can_erase_without_proof());

    // A single `401` is renewed and the request goes through.
    let w = pair_with_item();
    w.relay
        .fail_next("leave_space", problem(401, "session_expired"));
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
}

#[test]
fn t2_update_required_refuses() {
    let w = pair_with_item();
    for code in ["update_required", "format_unsupported"] {
        w.relay.fail_next("leave_space", problem(409, code));
        assert_eq!(
            w.e(1).leave_space().unwrap(),
            O::Refused(R::UpdateRequired),
            "{code}"
        );
        assert_eq!(leave_state(w.e(1)), None);
        assert_intact(w.e(1), 1);
        assert!(!w.e(1).can_erase_without_proof());
    }
}

#[test]
fn t2_server_errors_keep_the_departure_pending() {
    let w = pair_with_item();
    for (status, code) in [
        (503, "unavailable"),
        (500, "internal"),
        (429, "rate_limited"),
    ] {
        w.relay.fail_next("leave_space", problem(status, code));
        assert_eq!(
            w.e(1).leave_space().unwrap(),
            O::Pending(P::Server),
            "{status}"
        );
        assert!(requested(w.e(1)));
        assert_intact(w.e(1), 1);
        assert!(!w.e(1).can_erase_without_proof());
    }
    assert!(departures(&w).is_empty());
}

#[test]
fn t2_a_lost_answer_keeps_the_departure_pending() {
    let w = pair_with_item();
    w.relay.lose_next_response("leave_space");
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    assert_eq!(departures(&w).len(), 1, "the relay did process it");
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);
    assert!(!w.e(1).can_erase_without_proof());
}

#[test]
fn t2_another_4xx_refuses_and_drops_the_marker() {
    let w = pair_with_item();
    for (status, code) in [
        (400, "invalid_request"),
        (404, "not_found"),
        (422, "validation_failed"),
    ] {
        w.relay.fail_next("leave_space", problem(status, code));
        assert_eq!(
            w.e(1).leave_space().unwrap(),
            O::Refused(R::Unexpected),
            "{status}"
        );
        assert_eq!(leave_state(w.e(1)), None);
        assert_intact(w.e(1), 1);
        assert!(!w.e(1).can_erase_without_proof());
    }
    assert!(departures(&w).is_empty());
}

// --- T3: DELETE /v1/space -----------------------------------------------------

#[test]
fn t3_space_deleted_erases_and_leaves_a_receipt() {
    let w = alone_after_last_device();
    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
    assert!(w.relay.with(|s| s.space_deleted));
    assert!(w.relay.only_receipt().is_some());
    assert_eq!(w.relay.count("delete_space"), 1);
    assert_erased(w.e(0));
    assert_files_dirs_empty(&w.nodes[0]);
    assert_eq!(leave_state(w.e(0)), None);
}

#[test]
fn t3_an_answer_without_the_boolean_proves_nothing() {
    let w = alone_after_last_device();
    w.relay.fail_next(
        "delete_space",
        RelayError::Network("unexpected answer: delete without space_deleted".into()),
    );
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
    assert!(!w.e(0).can_erase_without_proof());
    assert!(stored_deletion_id(w.e(0)).is_some_and(|id| id.len() == 32));
}

#[test]
fn t3_revoked_erases() {
    let w = alone_after_last_device();
    w.relay.fail_next("delete_space", problem(403, "revoked"));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
    assert_erased(w.e(0));
}

#[test]
fn t3_not_last_device_goes_back_to_an_ordinary_departure() {
    let w = pair_with_item();
    assert_eq!(w.e(1).delete_space().unwrap(), O::Erased);
    assert_eq!(w.relay.count("delete_space"), 1);
    assert_eq!(w.relay.count("leave_space"), 1);
    assert_eq!(
        departures(&w),
        vec![(w.nodes[1].id.clone(), "self".to_owned())]
    );
    assert!(!w.relay.with(|s| s.space_deleted));
    assert_erased(w.e(1));

    // The other member left meanwhile: `last_device`, the interface asks again.
    let w = pair_with_item();
    w.relay
        .fail_next("leave_space", problem(409, "last_device"));
    assert_eq!(w.e(1).delete_space().unwrap(), O::Refused(R::LastDevice));
    assert_eq!(leave_state(w.e(1)), None);
    assert_intact(w.e(1), 1);
}

#[test]
fn t3_unauthorized_or_space_not_initialized_asks_the_receipt_again() {
    // No receipt: "not found", D4 offered.
    let w = alone_after_last_device();
    w.relay
        .fail_next("delete_space", problem(409, "space_not_initialized"));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::NotFound));
    assert_eq!(
        w.relay.count("space_deletion_receipt"),
        2,
        "before and after"
    );
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
    assert!(w.e(0).can_erase_without_proof());

    let w = alone_after_last_device();
    w.relay
        .fail_next("delete_space", problem(401, "unauthorized"));
    w.relay
        .fail_next("delete_space", problem(401, "unauthorized"));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::NotFound));
    assert_eq!(w.relay.count("space_deletion_receipt"), 2);
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
    assert!(w.e(0).can_erase_without_proof());

    // A receipt by then: proven.
    let w = alone_after_last_device();
    let (relay, e) = (w.relay.clone(), w.nodes[0].engine.clone());
    w.relay
        .fail_next("delete_space", problem(409, "space_not_initialized"));
    w.relay.during_next("delete_space", move || {
        let id: [u8; 32] = stored_deletion_id(&e).unwrap().try_into().unwrap();
        relay.add_receipt(id);
    });
    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
    assert_erased(w.e(0));
}

#[test]
fn t3_a_relay_without_the_route_is_too_old() {
    let w = alone_after_last_device();
    for (status, code) in [(404, "not_found"), (405, "method_not_allowed")] {
        w.relay.fail_next("delete_space", problem(status, code));
        assert_eq!(
            w.e(0).delete_space().unwrap(),
            O::Refused(R::RelayTooOld),
            "{status}"
        );
        assert_eq!(leave_state(w.e(0)), None);
        assert_intact(w.e(0), 1);
        assert!(w.e(0).can_erase_without_proof());
    }
}

#[test]
fn t3_update_required_refuses() {
    let w = alone_after_last_device();
    for code in ["update_required", "format_unsupported"] {
        w.relay.fail_next("delete_space", problem(409, code));
        assert_eq!(
            w.e(0).delete_space().unwrap(),
            O::Refused(R::UpdateRequired),
            "{code}"
        );
        assert_eq!(leave_state(w.e(0)), None);
        assert_intact(w.e(0), 1);
        assert!(!w.e(0).can_erase_without_proof());
    }
}

#[test]
fn t3_server_errors_keep_the_departure_pending() {
    let w = alone_after_last_device();
    for (status, code) in [
        (500, "internal"),
        (503, "unavailable"),
        (502, "bad_gateway"),
        (429, "rate_limited"),
    ] {
        w.relay.fail_next("delete_space", problem(status, code));
        assert_eq!(
            w.e(0).delete_space().unwrap(),
            O::Pending(P::Server),
            "{status}"
        );
        assert!(deleting(w.e(0)));
        assert_intact(w.e(0), 1);
        assert!(!w.e(0).can_erase_without_proof());
    }
}

#[test]
fn t3_network_errors_keep_the_departure_pending() {
    let w = alone_after_last_device();
    w.relay
        .fail_next("delete_space", RelayError::Network("timeout".into()));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
    assert!(!w.e(0).can_erase_without_proof());
    assert!(!w.relay.with(|s| s.space_deleted));
}

#[test]
fn t3_another_4xx_refuses() {
    let w = alone_after_last_device();
    for (status, code) in [
        (400, "invalid_request"),
        (415, "unsupported_media_type"),
        (422, "validation_failed"),
    ] {
        w.relay.fail_next("delete_space", problem(status, code));
        assert_eq!(
            w.e(0).delete_space().unwrap(),
            O::Refused(R::Unexpected),
            "{status}"
        );
        assert_eq!(leave_state(w.e(0)), None);
        assert_intact(w.e(0), 1);
        assert!(!w.e(0).can_erase_without_proof());
    }
}

// --- T4: the receipt ------------------------------------------------------------

#[test]
fn t4_a_receipt_proves_a_deletion_whose_answer_was_lost() {
    let w = alone_after_last_device();
    w.relay.lose_next_response("delete_space");
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    assert!(w.relay.with(|s| s.space_deleted), "the relay did delete it");
    assert_intact(w.e(0), 1);
    assert!(!w.e(0).can_erase_without_proof());
    let id: [u8; 32] = stored_deletion_id(w.e(0)).unwrap().try_into().unwrap();
    assert!(w.relay.has_receipt(&id));

    assert_eq!(w.e(0).retry_departure().unwrap(), O::Erased);

    assert_eq!(
        w.relay.count("delete_space"),
        1,
        "the receipt answered: no second DELETE"
    );
    assert_erased(w.e(0));
}

#[test]
fn t4_no_receipt_goes_on_with_the_session_and_the_request() {
    let w = alone_after_last_device();
    let from = w.relay.with(|s| s.calls.len());
    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
    let calls: Vec<String> = w.relay.with(|s| s.calls[from..].to_vec());
    assert_eq!(
        calls,
        [
            "space_deletion_receipt",
            "auth_challenge",
            "auth_session",
            "delete_space"
        ]
    );
}

#[test]
fn t4_any_other_answer_of_the_receipt_is_never_a_proof() {
    for err in [
        problem(503, "unavailable"),
        problem(429, "rate_limited"),
        problem(405, "method_not_allowed"),
        RelayError::Network("unexpected answer: receipt without space_deleted".into()),
    ] {
        let w = alone_after_last_device();
        w.relay.fail_next("space_deletion_receipt", err);
        w.relay
            .fail_next("delete_space", problem(503, "unavailable"));
        assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Server));
        assert_intact(w.e(0), 1);
        assert_eq!(w.relay.count("delete_space"), 1, "went on to the request");
    }
}

// --- Stops at every step (plan §9) and resumption ------------------------------

#[test]
fn nothing_pending_means_no_network_and_nothing_to_retry() {
    let w = pair_with_item();
    let calls = w.relay.with(|s| s.calls.len());
    assert_eq!(w.e(1).continue_departure().unwrap(), O::NothingPending);
    assert_eq!(w.relay.with(|s| s.calls.len()), calls);
    assert!(w.e(1).departure().is_none());
    assert!(matches!(
        w.e(1).retry_departure().unwrap_err(),
        crate::CoreError::InvalidInput { .. }
    ));
    assert!(!w.e(1).cancel_departure().unwrap());
    assert_intact(w.e(1), 1);
}

#[test]
fn a_stop_after_the_marker_resends_within_15_minutes() {
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", Some(T0));
    w.clock.advance(14 * MIN);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert!(again.departure().is_some(), "visible after a restart");
    assert_eq!(again.continue_departure().unwrap(), O::Erased);
    assert_eq!(w.relay.count("leave_space"), 1);
    assert_erased(&again);
}

#[test]
fn a_stop_after_the_marker_only_verifies_after_15_minutes() {
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", Some(T0));
    w.clock.advance(16 * MIN);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(
        w.relay.count("leave_space"),
        0,
        "never resent long after the gesture"
    );
    assert!(requested(&again));
    assert_intact(&again, 1);
    assert_eq!(again.departure().unwrap().last, Some(P::NotSent));
    // "Retry" is a gesture: it sends.
    assert_eq!(again.retry_departure().unwrap(), O::Erased);
    assert_eq!(w.relay.count("leave_space"), 1);
}

#[test]
fn an_old_marker_without_departure_since_only_verifies() {
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", None);
    assert_eq!(w.e(1).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.count("leave_space"), 0);
    assert_eq!(w.e(1).departure().unwrap().since_ms, None);
    // A deletion: the receipt and the session only.
    let w = alone_after_last_device();
    write_marker(w.e(0), b"deleting", None);
    assert_eq!(w.e(0).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.count("delete_space"), 0);
    assert_eq!(w.relay.count("space_deletion_receipt"), 1);
    assert_intact(w.e(0), 1);
}

#[test]
fn leaving_offline_keeps_a_visible_pending_departure() {
    let w = pair_with_item();
    w.relay.set_offline(true);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    assert_eq!(w.e(1).departure().unwrap().kind, K::Leave);
    assert_intact(w.e(1), 1);
    // The network is back within the window: the request is sent.
    w.relay.set_offline(false);
    w.clock.advance(5 * MIN);
    assert_eq!(w.e(1).continue_departure().unwrap(), O::Erased);
    assert_erased(w.e(1));

    // Back after the window: verification only, nothing sent, nothing erased.
    let w = pair_with_item();
    w.relay.set_offline(true);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    w.relay.set_offline(false);
    w.clock.advance(16 * MIN);
    assert_eq!(w.e(1).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.count("leave_space"), 0);
    assert_intact(w.e(1), 1);
}

#[test]
fn a_lost_answer_is_settled_at_the_next_start() {
    for wait in [5 * MIN, 20 * MIN] {
        let w = pair_with_item();
        w.relay.lose_next_response("leave_space");
        assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
        assert_eq!(departures(&w).len(), 1);
        w.clock.advance(wait);
        let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
        assert_intact(&again, 1);
        assert_eq!(again.continue_departure().unwrap(), O::Erased);
        assert_eq!(
            w.relay.count("leave_space"),
            1,
            "the revocation proves it: no second request"
        );
        assert_erased(&again);
        assert_files_dirs_empty(&w.nodes[1]);
        assert_eq!(again.continue_departure().unwrap(), O::NothingPending);
    }
}

#[test]
fn a_lost_deletion_is_settled_by_the_receipt_at_the_next_start() {
    let w = alone_after_last_device();
    w.relay.lose_next_response("delete_space");
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    w.clock.advance(30 * MIN);
    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.departure().unwrap().kind, K::DeleteSpace);
    assert_eq!(again.continue_departure().unwrap(), O::Erased);
    assert_eq!(w.relay.count("delete_space"), 1);
    assert_erased(&again);
}

#[test]
fn a_refused_erasing_transaction_erases_nothing_and_the_next_attempt_finishes() {
    let w = pair_with_item();
    w.e(1).store.exec_for_test(
        "CREATE TRIGGER no_wipe BEFORE DELETE ON meta BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    );
    let err = w.e(1).leave_space().unwrap_err();
    assert!(matches!(err, crate::CoreError::Storage { .. }), "{err:?}");
    assert!(requested(w.e(1)), "the marker stays");
    assert_intact(w.e(1), 1);
    assert_eq!(departures(&w).len(), 1, "the relay let the device go");
    w.e(1).store.exec_for_test("DROP TRIGGER no_wipe;");
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Erased);
    assert_eq!(w.relay.count("leave_space"), 1, "proven by the revocation");
    assert_erased(w.e(1));
}

#[test]
fn an_interrupted_wipe_is_finished_at_the_next_open() {
    let w = pair_with_item();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    // A stop between the erasing transaction and the end of the cleaning: the marker is
    // there and a file is left over.
    let orphan = w.e(1).store.file_path("files/orphelin.enc");
    std::fs::write(&orphan, b"reste").unwrap();
    write_marker(w.e(1), b"wiping", None);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert!(!orphan.exists());
    assert_eq!(leave_state(&again), None);
    assert_erased(&again);
    assert_files_dirs_empty(&w.nodes[1]);
}

#[test]
fn a_wipe_stopped_before_the_compaction_is_finished_at_the_next_open() {
    let w = pair_with_item();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    // The files are gone, the compaction was not done: only the marker is left.
    write_marker(w.e(1), b"wiping", None);
    assert!(w.e(1).departure().unwrap().wiping);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_eq!(leave_state(&again), None);
    assert_erased(&again);
}

#[test]
fn a_departure_marker_on_a_device_that_is_not_provisioned_is_removed() {
    for marker in [b"requested".as_slice(), b"deleting".as_slice()] {
        let w = pair_with_item();
        assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
        write_marker(w.e(1), marker, Some(T0));
        let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
        assert_eq!(leave_state(&again), None);
        assert_erased(&again);
        assert_eq!(again.continue_departure().unwrap(), O::NothingPending);
    }
}

#[test]
fn the_erasing_transaction_checks_the_marker() {
    let w = pair_with_item();
    // No marker: a departure erasure erases nothing.
    assert!(!w
        .e(1)
        .store
        .wipe_everything(WipeGuard::DepartureMarker)
        .unwrap());
    assert_intact(w.e(1), 1);
    write_marker(w.e(1), b"wiping", None);
    assert!(!w
        .e(1)
        .store
        .wipe_everything(WipeGuard::DepartureMarker)
        .unwrap());
    assert_intact(w.e(1), 1);
    write_marker(w.e(1), b"deleting", None);
    assert!(w
        .e(1)
        .store
        .wipe_everything(WipeGuard::DepartureMarker)
        .unwrap());
    assert_eq!(leave_state(w.e(1)).as_deref(), Some(b"wiping".as_slice()));
}

// --- Never a proof: kept states, empty relay, 4004 -----------------------------

#[test]
fn a_persisted_space_deleted_flag_is_ignored_and_proves_nothing() {
    let w = pair_with_item();
    // Written by an earlier version.
    w.e(1)
        .store
        .tx(|db| db.meta_set("blocked", b"space_deleted"))
        .unwrap();
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.blocked(), None);
    assert_eq!(
        again.store.read(|db| db.meta_get("blocked")).unwrap(),
        None,
        "removed at the opening"
    );
    w.relay.uninitialize();
    let from = w.relay.with(|s| s.calls.len());

    assert_eq!(again.leave_space().unwrap(), O::Pending(P::NotFound));

    assert!(
        w.relay
            .with(|s| s.calls[from..].iter().any(|c| c == "auth_challenge")),
        "the relay was asked"
    );
    assert!(requested(&again));
    assert_intact(&again, 1);
    assert!(again.can_erase_without_proof());
}

#[test]
fn an_empty_relay_never_erases_anything() {
    // A relay started on an unmounted volume answers `409 space_not_initialized` everywhere.
    let w = pair_with_item();
    w.relay.uninitialize();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::NotFound));
    assert_eq!(
        w.e(1).continue_departure().unwrap(),
        O::Pending(P::NotFound)
    );
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.blocked(), None, "never kept on disk");
    assert_eq!(again.continue_departure().unwrap(), O::Pending(P::NotFound));
    assert_intact(&again, 1);
    // The volume is back: the device is still a member, the departure goes through.
    w.relay.with(|s| s.initialized = true);
    assert_eq!(again.continue_departure().unwrap(), O::Erased);
    assert_erased(&again);

    // Same for a deletion.
    let w = alone_after_last_device();
    w.relay.uninitialize();
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::NotFound));
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);

    // And for a relay that forgot the device.
    let w = pair_with_item();
    w.relay.forget_device(&w.nodes[1].id);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::NotFound));
    assert_intact(w.e(1), 1);
}

#[test]
fn a_4004_is_not_a_proof() {
    // Still a member: the doorbell goes on. A recent departure is pending: it stays so.
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let sessions = w.relay.count("auth_session");
    assert!(w.e(1).still_member_after_space_reset());
    assert_eq!(
        w.relay.count("auth_session"),
        sessions + 1,
        "a fresh session"
    );
    assert_eq!(w.e(1).blocked(), None);
    assert_marker_kept(w.e(1));
    // An empty relay: the doorbell stops, nothing is erased, nothing is kept on disk.
    w.relay.uninitialize();
    assert!(!w.e(1).still_member_after_space_reset());
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
    // Revoked: stops too.
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    write_recent_marker(w.e(2));
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert!(!w.e(2).still_member_after_space_reset());
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    assert!(w.e(2).is_provisioned());
    assert_marker_kept(w.e(2));
}

#[test]
fn a_4004_followed_by_a_transient_or_unexpected_answer_is_not_a_reason_to_stop() {
    // Server error, network error, then answers a proxy or a validation layer may give on a
    // live space (403 without a `revoked` code, 422): the doorbell may go on in every case.
    for fault in [
        problem(503, "unavailable"),
        RelayError::Network("coupure".into()),
        problem(403, "forbidden"),
        problem(422, "validation_failed"),
    ] {
        let w = pair_with_item();
        write_recent_marker(w.e(1));
        w.relay.fail_next("auth_challenge", fault);
        assert!(w.e(1).still_member_after_space_reset());
        assert_eq!(w.e(1).blocked(), None);
        assert_intact(w.e(1), 1);
        assert_marker_kept(w.e(1));
    }
}

#[test]
fn the_doorbell_ends_on_a_4004_from_an_empty_relay_and_erases_nothing() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The first connection is closed with a `4004` while the relay has just lost its space (the
    // catch-up after `ready` is where it happens): no timing involved. A `NotFound` met at any
    // other moment would not end the loop, only a `4004` verified by a fresh session does.
    w.relay.close_next_doorbell(4004);
    let relay = w.relay.clone();
    w.relay.during_next("events", move || relay.uninitialize());
    let (b, s2) = (w.nodes[1].engine.clone(), stop.clone());
    let h = std::thread::spawn(move || b.run_doorbell(&s2));
    let started = std::time::Instant::now();
    while !h.is_finished() && started.elapsed() < std::time::Duration::from_secs(15) {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let finished = h.is_finished();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    h.join().unwrap();
    assert!(finished, "the doorbell loop must end");
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

// --- Cancelling -----------------------------------------------------------------

#[test]
fn cancelling_during_a_departure_erases_nothing_and_the_device_shows_revoked() {
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    w.relay.during_next("leave_space", move || {
        assert!(e1.cancel_departure().unwrap())
    });

    assert_eq!(w.e(1).leave_space().unwrap(), O::Cancelled);

    assert_eq!(departures(&w).len(), 1, "the relay let it go");
    assert_eq!(leave_state(w.e(1)), None);
    assert_intact(w.e(1), 1);
    assert_eq!(w.e(1).blocked(), Some(Blocked::Revoked));
    assert_eq!(w.e(1).space_status(), SpaceStatus::Revoked);
    // "Sign out" again: the relay says revoked, the device is erased.
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    assert_erased(w.e(1));
}

#[test]
fn cancelling_during_a_deletion_keeps_the_deletion_id_and_the_receipt_proves_it_later() {
    let w = alone_after_last_device();
    let e0 = w.nodes[0].engine.clone();
    w.relay.during_next("delete_space", move || {
        assert!(e0.cancel_departure().unwrap())
    });

    assert_eq!(w.e(0).delete_space().unwrap(), O::Cancelled);

    assert!(w.relay.with(|s| s.space_deleted));
    assert_eq!(w.e(0).space_status(), SpaceStatus::NotFound);
    assert_intact(w.e(0), 1);
    assert_eq!(leave_state(w.e(0)), None);
    assert!(w.e(0).departure().is_none());
    let id: [u8; 32] = stored_deletion_id(w.e(0)).unwrap().try_into().unwrap();
    assert!(w.relay.has_receipt(&id), "the same identifier is kept");
    // A new "Delete space": same identifier, proven by the receipt.
    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
    assert_eq!(w.relay.count("delete_space"), 1);
    assert_erased(w.e(0));
}

#[test]
fn cancelling_while_wiping_is_refused() {
    let w = pair_with_item();
    write_marker(w.e(1), b"wiping", None);
    assert!(!w.e(1).cancel_departure().unwrap());
    assert_eq!(leave_state(w.e(1)).as_deref(), Some(b"wiping".as_slice()));
    assert!(w.e(1).departure().unwrap().wiping);
}

#[test]
fn the_pending_departure_is_visible_with_its_kind_and_reason() {
    let w = alone_after_last_device();
    w.relay.set_offline(true);
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    let v = w.e(0).departure().unwrap();
    assert_eq!(
        (v.kind, v.wiping, v.since_ms, v.last),
        (K::DeleteSpace, false, Some(T0), Some(P::Network))
    );
    w.clock.advance(MIN);
    w.relay.set_offline(false);
    w.relay
        .fail_next("delete_space", problem(503, "unavailable"));
    assert_eq!(w.e(0).retry_departure().unwrap(), O::Pending(P::Server));
    let v = w.e(0).departure().unwrap();
    assert_eq!(v.since_ms, Some(T0 + MIN), "a retry is a gesture");
    assert_eq!(v.last, Some(P::Server));
    // After a restart the reason is unknown until the next attempt.
    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    let v = again.departure().unwrap();
    assert_eq!((v.kind, v.last), (K::DeleteSpace, None));
}

// --- "Erase anyway" (D4) --------------------------------------------------

#[test]
fn erasing_without_proof_needs_a_recent_not_found_or_too_old_verdict() {
    let w = pair_with_item();
    // No verdict at all.
    assert!(!w.e(1).can_erase_without_proof());
    assert!(refused_input(w.e(1).erase_without_proof()));
    // A network error.
    w.relay.set_offline(true);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Pending(P::Network));
    assert!(refused_input(w.e(1).erase_without_proof()));
    w.relay.set_offline(false);
    // A server error, a rate limit.
    for (status, code) in [(503, "unavailable"), (429, "rate_limited")] {
        w.relay.fail_next("auth_session", problem(status, code));
        assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::Server));
        assert!(refused_input(w.e(1).erase_without_proof()));
    }
    // "Not found", then a network error: the last verdict counts.
    w.relay.uninitialize();
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::NotFound));
    assert!(w.e(1).can_erase_without_proof());
    w.relay.set_offline(true);
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::Network));
    assert!(refused_input(w.e(1).erase_without_proof()));
    w.relay.set_offline(false);
    // "Not found" 11 minutes old.
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::NotFound));
    w.clock.advance(11 * MIN);
    assert!(refused_input(w.e(1).erase_without_proof()));
    // A restart forgets the verdict.
    assert_eq!(w.e(1).retry_departure().unwrap(), O::Pending(P::NotFound));
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert!(!again.can_erase_without_proof());
    assert_intact(w.e(1), 1);
}

#[test]
fn erasing_without_proof_empties_everything_and_calls_no_relay() {
    let w = World::new(1, 1);
    w.e(0).sync().unwrap();
    let file_id = send_secret_and_file(&w, 0);
    let cached = w.e(0).attachment_path(&file_id, 0).unwrap().unwrap();
    assert!(cached.exists());
    assert!(!holding(&w.nodes[0], &marked_file(20_000)[..1000]).is_empty());
    // `tomi-relay admin reset` done by hand (D5): no receipt, "space not found".
    w.relay.uninitialize();
    assert_eq!(w.e(0).leave_space().unwrap(), O::Pending(P::NotFound));
    assert!(w.e(0).can_erase_without_proof());
    let calls = w.relay.with(|s| s.calls.len());

    w.e(0).erase_without_proof().unwrap();

    assert_eq!(w.relay.with(|s| s.calls.len()), calls, "no request at all");
    assert!(departures(&w).is_empty(), "the device did not leave");
    assert!(!w.e(0).is_provisioned());
    assert!(w.e(0).list_items().is_err());
    assert!(!cached.exists());
    assert_erased(w.e(0));
    assert_eq!(leave_state(w.e(0)), None);
    assert_files_dirs_empty(&w.nodes[0]);
    assert!(holding(&w.nodes[0], &marked_file(20_000)[..1000]).is_empty());

    // After a restart: a new device.
    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert!(!again.is_provisioned());
    assert_eq!(again.continue_departure().unwrap(), O::NothingPending);
    assert_erased(&again);
}

#[test]
fn erasing_without_proof_after_a_relay_too_old() {
    let w = alone_after_last_device();
    w.relay.fail_next("delete_space", problem(404, "not_found"));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Refused(R::RelayTooOld));
    assert!(w.e(0).can_erase_without_proof());
    w.e(0).erase_without_proof().unwrap();
    assert_erased(w.e(0));
    assert!(!w.relay.with(|s| s.space_deleted), "the relay was not told");
}

#[test]
fn a_local_erasure_stopped_after_the_transaction_is_finished_at_the_next_open() {
    let w = World::new(1, 1);
    send_secret_and_file(&w, 0);
    // The stop comes right after the commit of the erasing transaction: tables empty, marker
    // written with them, files still there.
    assert!(w
        .e(0)
        .store
        .wipe_everything(WipeGuard::Unconditional)
        .unwrap());
    assert_eq!(leave_state(w.e(0)).as_deref(), Some(b"wiping".as_slice()));
    let left = std::fs::read_dir(w.nodes[0].dir.path().join("files/files"))
        .unwrap()
        .count();
    assert!(left > 0);

    let again = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert!(!again.is_provisioned());
    assert_eq!(leave_state(&again), None);
    assert_erased(&again);
    assert_files_dirs_empty(&w.nodes[0]);
}

#[test]
fn erasing_without_proof_twice_or_when_not_provisioned_is_refused() {
    let w = World::new(1, 1);
    w.relay.uninitialize();
    assert_eq!(w.e(0).leave_space().unwrap(), O::Pending(P::NotFound));
    w.e(0).erase_without_proof().unwrap();
    for _ in 0..2 {
        assert!(refused_input(w.e(0).erase_without_proof()));
    }
    assert_erased(w.e(0));

    // A device that never was provisioned.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(
        &dir.path().join("tomi.db"),
        &dir.path().join("files"),
        &[9u8; 32],
    )
    .unwrap();
    let blank = Engine::open(
        EngineConfig::default(),
        store,
        w.relay.transport(),
        w.clock.clone(),
    )
    .unwrap();
    assert!(refused_input(blank.erase_without_proof()));
    assert!(matches!(
        blank.leave_space().unwrap_err(),
        crate::CoreError::InvalidInput { .. }
    ));
    assert!(matches!(
        blank.delete_space().unwrap_err(),
        crate::CoreError::InvalidInput { .. }
    ));
    assert_eq!(blank.continue_departure().unwrap(), O::NothingPending);
    assert!(!blank.cancel_departure().unwrap());
    assert!(blank.departure().is_none());
    assert!(!blank.can_erase_without_proof());
}

#[test]
fn after_the_space_is_deleted_the_device_can_create_a_new_space() {
    const NEW_TOKEN: &str = "p6fj-a5ye-q2ns-h7k2-m9qv-d4xr-8tbn-c3wz";
    for deleted_by_the_relay in [true, false] {
        let w = alone_after_last_device();
        if deleted_by_the_relay {
            assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);
        } else {
            // A relay without `DELETE /v1/space`, then "Erase anyway".
            w.relay
                .fail_next("delete_space", problem(405, "method_not_allowed"));
            assert_eq!(w.e(0).delete_space().unwrap(), O::Refused(R::RelayTooOld));
            w.e(0).erase_without_proof().unwrap();
        }
        assert!(!w.e(0).is_provisioned());
        assert_erased(w.e(0));

        // The owner made a new invitation (`tomi-relay admin invite --new`).
        let fresh = MockRelay::uninitialized(w.clock.clone(), NEW_TOKEN);
        let again = w.nodes[0].reopen(&fresh, w.clock.clone());
        assert!(!again.is_provisioned());
        again.create_space(NEW_TOKEN, "Mon appareil").unwrap();
        assert!(again.is_provisioned());
        assert_eq!(again.blocked(), None);
        assert!(again.list_items().unwrap().is_empty());
        again.sync().unwrap();
        assert_eq!(again.blocked(), None);
        assert!(
            stored_deletion_id(&again).is_none(),
            "a new provisioning starts clean"
        );
    }
}

// --- No write after the erasure ---------------------------------------------------

#[test]
fn a_rename_racing_with_the_erasure_writes_nothing() {
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    w.relay.during_next("rename_device", move || {
        // The verdict D4 needs, prepared; the erasure lands while the rename is in flight.
        mark_not_found(&e1);
        e1.erase_without_proof().unwrap();
    });
    let me = w.nodes[1].id.clone();
    let err = w.e(1).rename_device(&me, "Nouveau nom").unwrap_err();
    assert!(
        matches!(err, crate::CoreError::InvalidInput { .. }),
        "{err:?}"
    );
    assert_erased(w.e(1));
}

#[test]
fn no_write_of_an_erased_identity_lands_after_the_erasure() {
    let w = pair_with_item();
    let id = w.e(1).list_items().unwrap()[0].id.clone();
    // What a `mark_read` (or a download, an export) that read before the erasure holds.
    let stale = w.e(1).active().unwrap();
    mark_not_found(w.e(1));
    w.e(1).erase_without_proof().unwrap();
    let err = w
        .e(1)
        .live_tx(&stale, |db| db.set_item_read(&id, true))
        .unwrap_err();
    assert!(
        matches!(err, crate::CoreError::InvalidInput { .. }),
        "{err:?}"
    );
    assert!(refused_input(w.e(1).mark_read(&id)));
    assert_erased(w.e(1));
}

#[test]
fn mark_read_racing_with_the_erasure_writes_nothing() {
    for _ in 0..5 {
        let w = pair_with_item();
        let id = w.e(1).list_items().unwrap()[0].id.clone();
        let e1 = w.nodes[1].engine.clone();
        let reader = std::thread::spawn(move || {
            while e1.mark_read(&id).is_ok() {
                // Unread again, so that every call writes.
                let _ = e1.store.tx(|db| db.set_item_read(&id, false));
            }
        });
        mark_not_found(w.e(1));
        w.e(1).erase_without_proof().unwrap();
        reader.join().unwrap();
        assert_erased(w.e(1));
    }
}

// --- Automatic rotation (§7.6) ------------------------------------------------------

fn epoch(w: &World) -> u32 {
    w.relay.with(|s| s.key_epoch)
}

fn rotation_flags(e: &Engine) -> (bool, Option<i64>) {
    let f = e.flags.lock().unwrap();
    (f.rotation_required, f.rotation_due_at_ms)
}

/// A doorbell connection that only ticks (never a `wake`): the clock moves at each tick.
struct Ticks {
    clock: Arc<TestClock>,
    left: u32,
    step_ms: i64,
}

impl WakeSource for Ticks {
    fn next(&mut self) -> std::result::Result<Option<Wake>, WakeClosed> {
        if self.left == 0 {
            return Err(WakeClosed { code: None });
        }
        self.left -= 1;
        self.clock.advance(self.step_ms);
        Ok(None)
    }
}

/// Three members, device 2 left: devices 0 and 1 owe a rotation.
fn after_a_departure() -> World {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    assert_eq!(w.e(2).leave_space().unwrap(), O::Erased);
    w
}

#[test]
fn the_remaining_member_rotates_by_itself_after_a_departure() {
    let w = after_a_departure();
    let report = w.e(0).sync().unwrap();
    assert!(report.rotation_required);
    assert_eq!(epoch(&w), 1, "not before the random delay");
    let (required, due) = rotation_flags(w.e(0));
    assert!(required);
    assert!((T0..=T0 + 30_000).contains(&due.unwrap()));

    w.clock.advance(30_001);
    w.e(0).sync().unwrap();

    assert_eq!(epoch(&w), 2, "rotated without any call of the app");
    assert_eq!(rotation_flags(w.e(0)), (false, None), "the flag is cleared");
    assert_eq!(w.relay.count("post_rotation"), 1);
    // The other member sees it done and does not rotate.
    w.e(1).sync().unwrap();
    w.clock.advance(30_001);
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 1);
    assert_eq!(rotation_flags(w.e(1)), (false, None));
    let id = send(&w, 0, text("apres la rotation"));
    w.e(1).sync().unwrap();
    assert_eq!(
        plain_of(&w.e(1).get_item(&id).unwrap().unwrap()),
        "apres la rotation"
    );
}

#[test]
fn the_doorbell_loop_runs_a_due_rotation_without_any_wake() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    assert_eq!(epoch(&w), 1);
    let mut ticks = Ticks {
        clock: w.clock.clone(),
        left: 2,
        step_ms: 31_000,
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    w.e(0).run_wake_source(&mut ticks, &stop);
    assert_eq!(epoch(&w), 2);
    assert_eq!(rotation_flags(w.e(0)), (false, None));
}

#[test]
fn no_automatic_rotation_while_this_device_is_leaving() {
    let w = after_a_departure();
    write_marker(w.e(1), b"requested", Some(T0));
    w.e(1).sync().unwrap();
    w.clock.advance(31_000);
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 0);
    assert!(matches!(
        w.e(1).revoke_device(&w.nodes[0].id).unwrap_err(),
        crate::CoreError::InvalidInput { .. }
    ));
    assert!(w.e(1).cancel_departure().unwrap());
    w.e(1).sync().unwrap();
    assert_eq!(epoch(&w), 2);
}

#[test]
fn a_failed_automatic_rotation_waits_for_its_new_due_date() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.clock.advance(31_000);
    w.relay
        .fail_next("post_rotation", RelayError::Network("coupure".into()));
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 1);
    assert_eq!(epoch(&w), 1);
    let (required, due) = rotation_flags(w.e(0));
    assert!(required);
    let now = w.clock.now_ms();
    assert!((now..=now + 60_000).contains(&due.unwrap()));
    w.e(0).sync().unwrap();
    assert_eq!(
        w.relay.count("post_rotation"),
        1,
        "not before the new due date"
    );
    w.clock.advance(60_001);
    w.e(0).sync().unwrap();
    assert_eq!(epoch(&w), 2);
    assert_eq!(rotation_flags(w.e(0)), (false, None));
}

#[test]
fn with_two_remaining_members_only_one_rotation_is_made() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    w.clock.advance(31_000);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 1);
    assert_eq!(epoch(&w), 2);
    for i in 0..2 {
        assert_eq!(rotation_flags(w.e(i)), (false, None));
    }
}

#[test]
fn the_other_members_rotate_after_a_departure_when_the_app_asks() {
    // Compatibility: `maybe_rotate` (FFI) still works.
    let w = after_a_departure();
    let report = w.e(0).sync().unwrap();
    assert!(report.rotation_required);
    assert!(w.e(0).maybe_rotate().unwrap());
    assert_eq!(epoch(&w), 2);
    assert_eq!(rotation_flags(w.e(0)), (false, None));
    let state_of = |e: &Engine| {
        e.list_devices()
            .unwrap()
            .into_iter()
            .find(|d| d.id == w.nodes[2].id)
            .unwrap()
            .state
    };
    assert_eq!(state_of(w.e(0)), DeviceState::Revoked);
    let id = send(&w, 0, text("apres la rotation"));
    w.e(1).sync().unwrap();
    assert_eq!(
        plain_of(&w.e(1).get_item(&id).unwrap().unwrap()),
        "apres la rotation"
    );
    assert_eq!(state_of(w.e(1)), DeviceState::Revoked);
    // Once rotated, the relay no longer asks.
    assert!(!w.e(0).maybe_rotate().unwrap());
}

#[test]
fn an_epoch_conflict_that_leaves_the_rotation_owed_is_followed_by_a_second_try_that_posts() {
    // The conflict is injected in the double: nobody rotated, so the relay still owes one.
    // Without a target (`maybe_rotate`).
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.relay
        .fail_next("post_rotation", problem(409, "epoch_conflict"));
    assert!(w.e(0).maybe_rotate().unwrap());
    assert_eq!(w.relay.count("post_rotation"), 2, "refused, then posted");
    assert_eq!(epoch(&w), 2);
    assert_eq!(rotation_flags(w.e(0)), (false, None));

    // Without a target, by itself (the automatic rotation, §7.6).
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.clock.advance(31_000);
    w.relay
        .fail_next("post_rotation", problem(409, "epoch_conflict"));
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 2);
    assert_eq!(epoch(&w), 2);
    assert_eq!(rotation_flags(w.e(0)), (false, None));

    // With a target (`revoke_device`).
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    w.relay
        .fail_next("post_rotation", problem(409, "epoch_conflict"));
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert_eq!(w.relay.count("post_rotation"), 2, "refused, then posted");
    assert_eq!(epoch(&w), 2);
    let revoked = w
        .e(0)
        .list_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.id == w.nodes[2].id)
        .unwrap()
        .state;
    assert_eq!(revoked, DeviceState::Revoked);
}

#[test]
fn inviting_while_a_departure_is_pending_is_refused() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    write_marker(w.e(0), b"requested", Some(T0));
    let err = match w.nodes[0].engine.start_invitation() {
        Err(e) => e,
        Ok(_) => panic!("an invitation was opened during a departure"),
    };
    assert!(
        matches!(err, crate::CoreError::InvalidInput { .. }),
        "{err:?}"
    );
    assert_eq!(w.relay.count("create_pairing_channel"), 0);
}

// --- Other guarantees kept from the first version ------------------------------------

#[test]
fn a_revocation_by_another_member_keeps_the_local_data() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    send(&w, 0, text("lisible apres revocation"));
    w.e(2).sync().unwrap();
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert!(w.e(2).sync().is_err());
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    let calls = w.relay.with(|s| s.calls.len());
    // No departure was requested: nothing is settled, nothing is asked, nothing is erased.
    assert_eq!(w.e(2).continue_departure().unwrap(), O::NothingPending);
    assert_eq!(w.relay.with(|s| s.calls.len()), calls, "no network");
    assert_intact(w.e(2), 1);
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    // Also after a restart.
    let again = w.nodes[2].reopen(&w.relay, w.clock.clone());
    assert_eq!(again.continue_departure().unwrap(), O::NothingPending);
    assert_intact(&again, 1);
}

#[test]
fn a_device_can_be_provisioned_again_after_leaving() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    // Revoked first (the blocked flag is persisted), then leaves: the flag must not survive.
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    assert!(w.e(2).sync().is_err());
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
    assert_eq!(w.e(2).leave_space().unwrap(), O::Erased);
    assert_eq!(w.e(2).blocked(), None);

    let space_id = provision_fresh(w.e(2));
    assert!(w.e(2).is_provisioned());
    assert_eq!(w.e(2).blocked(), None);
    assert_eq!(w.e(2).space_status(), SpaceStatus::Active);
    assert!(w.e(2).list_items().unwrap().is_empty());
    assert_eq!(w.e(2).space_id().unwrap(), space_id);
    assert_eq!(
        w.e(2).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert_eq!(leave_state(w.e(2)), None);
    assert!(w.e(2).departure().is_none());
}

#[test]
fn the_doorbell_ends_and_nothing_is_written_after_the_wipe() {
    let w = World::new(2, 2);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s2) = (w.nodes[1].engine.clone(), stop.clone());
    let h = std::thread::spawn(move || b.run_doorbell(&s2));
    std::thread::sleep(std::time::Duration::from_millis(300));

    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);

    let started = std::time::Instant::now();
    while !h.is_finished() && started.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(h.is_finished(), "the doorbell loop must end");
    h.join().unwrap();
    // The `4003` that closed the doorbell wrote nothing once the device was erased.
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert_erased(w.e(1));
}

#[test]
fn no_sealed_key_survives_in_the_database_files() {
    let mut w = World::new(2, 2);
    w.e(1).sync().unwrap();
    send(&w, 0, text("avant"));
    w.e(1).sync().unwrap();
    let sealed = |name: &str| {
        w.e(1)
            .store
            .read(|db| db.raw_secret_record(name))
            .unwrap()
            .unwrap()
    };
    let (device_keys, local_key) = (sealed("device_keys"), sealed("local_key"));
    // The sealed space key ring and the sealed body of a secret item.
    let mut secret = text("mot de passe");
    secret.secret = true;
    send(&w, 0, secret);
    w.e(1).sync().unwrap();
    let rings = w.e(1).store.read(|db| db.raw_sealed_records()).unwrap();
    assert!(rings.iter().any(|(k, _)| *k == "space_key"));
    assert!(rings.iter().any(|(k, _)| *k == "secret_item"));
    let dir = w.nodes[1].dir.path().to_path_buf();
    let on_disk = |needle: &[u8]| {
        ["tomi.db", "tomi.db-wal"]
            .iter()
            .any(|f| std::fs::read(dir.join(f)).is_ok_and(|b| contains(&b, needle)))
    };
    assert!(on_disk(&device_keys), "the check can see a sealed record");
    assert!(on_disk(&local_key));
    for (kind, record) in &rings {
        assert!(on_disk(record), "the check can see a sealed {kind}");
    }

    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    assert!(!on_disk(&device_keys), "device keys, engine still open");
    assert!(!on_disk(&local_key), "local key, engine still open");
    for (kind, record) in &rings {
        assert!(!on_disk(record), "{kind}, engine still open");
    }

    // The engine is gone: the files are all there is.
    let Node {
        engine, dir: tmp, ..
    } = w.nodes.remove(1);
    drop(engine);
    assert!(!on_disk(&device_keys), "device keys, engine closed");
    assert!(!on_disk(&local_key), "local key, engine closed");
    for (kind, record) in &rings {
        assert!(!on_disk(record), "{kind}, engine closed");
    }
    drop(tmp);
}

/// Provisions an erased device again, alone in a space of its own.
fn provision_fresh(e: &Engine) -> String {
    let keys = DeviceKeys::generate().unwrap();
    let space_id = new_space_id().unwrap();
    let trust = TrustStore::new(&space_id, &keys.public(), 1).unwrap();
    e.provision(Provision {
        keys,
        trust,
        space_key: SpaceKey::generate(1).unwrap(),
        device_name: "Nouvel espace".into(),
    })
    .unwrap();
    space_id
}

/// A secret and an attachment sent by `from`, so that the erasure has something to find.
fn send_secret_and_file(w: &World, from: usize) -> String {
    let mut secret = text("mot de passe");
    secret.secret = true;
    send(w, from, secret);
    let src = w.nodes[from].dir.path().join("piece.bin");
    std::fs::write(&src, marked_file(20_000)).unwrap();
    let mut with_file = text("avec fichier");
    with_file.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "piece.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    send(w, from, with_file)
}

#[test]
fn leaving_with_an_outdated_app_reports_an_update_and_erases_nothing() {
    let w = pair_with_item();
    w.relay.with(|s| s.min_client_format = 2);
    w.e(1).drop_session();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Refused(R::UpdateRequired));
    assert_eq!(w.relay.count("leave_space"), 0, "never reached the request");
    assert!(matches!(
        w.e(1).blocked(),
        Some(Blocked::UpdateRequired { .. })
    ));
    assert_intact(w.e(1), 1);
    assert_eq!(leave_state(w.e(1)), None);
}

#[test]
fn resuming_does_not_erase_a_device_the_relay_does_not_recognise() {
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", Some(T0));
    w.relay.forget_device(&w.nodes[1].id);
    assert_eq!(
        w.e(1).continue_departure().unwrap(),
        O::Pending(P::NotFound)
    );
    assert_intact(w.e(1), 1);
    assert!(requested(w.e(1)), "kept, visible, with \"Erase anyway\"");
}

#[test]
fn resuming_forgets_a_departure_the_relay_refuses_in_a_final_way() {
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", None);
    w.relay.with(|s| s.min_client_format = 2);
    assert_eq!(
        w.e(1).continue_departure().unwrap(),
        O::Refused(R::UpdateRequired)
    );
    assert_eq!(leave_state(w.e(1)), None);
    assert_intact(w.e(1), 1);
}

#[test]
fn a_late_session_does_not_come_back_after_the_wipe() {
    // The doorbell opens a session while the device is erased: the answer arrives after the
    // erasure and must neither install a token nor write into the emptied database, nor
    // serve a space joined later in the same process.
    let w = World::new(2, 2);
    w.e(1).sync().unwrap();
    w.e(1).drop_session();
    let engine = w.nodes[1].engine.clone();
    w.relay.after_next_auth_session(move || {
        mark_not_found(&engine);
        engine.erase_without_proof().unwrap();
    });

    assert!(w.e(1).ensure_session().is_err());
    assert!(!w.e(1).is_provisioned());
    assert_erased(w.e(1)); // no `meta.limits` written back
    assert_eq!(w.e(1).blocked(), None);

    // Same process, same node, same device keys, joins again: a new session is really opened.
    let keys = w.nodes[1].keys();
    let trust = TrustStore::new(&w.space_id, &keys.public(), 1).unwrap();
    w.e(1)
        .provision(Provision {
            keys,
            trust,
            space_key: SpaceKey::generate(1).unwrap(),
            device_name: "Retour".into(),
        })
        .unwrap();
    let before = w.relay.count("auth_session");
    w.e(1).ensure_session().unwrap();
    assert_eq!(
        w.relay.count("auth_session"),
        before + 1,
        "nothing was served from the cache of the erased identity"
    );
    assert_eq!(w.e(1).blocked(), None);
}

#[test]
fn a_late_revocation_writes_nothing_after_the_wipe() {
    let w = World::new(2, 2);
    let stale = w.e(1).active().unwrap();
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    // The `4003` of the doorbell, or a `403`, arriving after the erasure.
    w.e(1).set_blocked(&stale, Blocked::Revoked);
    w.e(1).set_not_found(&stale);
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert_erased(w.e(1));
}

#[test]
fn provisioning_finishes_a_pending_wipe_first() {
    let w = World::new(2, 2);
    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);
    // A stop left the marker and files of the previous space.
    let orphan = w.e(1).store.file_path("files/orphelin.enc");
    let in_cache = w.e(1).store.file_path("files/cache/clair.bin");
    let staged = w.e(1).store.file_path("outbox/brouillon");
    std::fs::write(&orphan, b"reste").unwrap();
    std::fs::write(&in_cache, b"reste").unwrap();
    std::fs::write(&staged, b"reste").unwrap();
    write_marker(w.e(1), b"wiping", None);

    provision_fresh(w.e(1));

    assert!(w.e(1).is_provisioned());
    assert!(!orphan.exists() && !in_cache.exists() && !staged.exists());
    assert_eq!(leave_state(w.e(1)), None);
    assert_files_dirs_empty(&w.nodes[1]);
}

#[test]
fn a_wiping_marker_on_a_provisioned_device_erases_nothing() {
    let w = pair_with_item();
    write_marker(w.e(1), b"wiping", None);
    let again = w.nodes[1].reopen(&w.relay, w.clock.clone());
    assert_intact(&again, 1);
    assert_eq!(leave_state(&again), None, "a leftover marker is removed");
}

#[test]
fn leaving_while_a_sync_runs_on_another_thread() {
    let w = World::new(2, 2);
    w.e(1).sync().unwrap();
    for i in 0..15 {
        send(&w, 0, text(&format!("element {i}")));
    }
    let e1 = w.nodes[1].engine.clone();
    let syncing = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while e1.is_provisioned() && started.elapsed() < std::time::Duration::from_secs(20) {
            let _ = e1.sync();
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(5));

    assert_eq!(w.e(1).leave_space().unwrap(), O::Erased);

    syncing.join().unwrap();
    assert!(!w.e(1).is_provisioned());
    assert_erased(w.e(1));
    assert_eq!(leave_state(w.e(1)), None);
    assert_eq!(w.e(1).blocked(), None);
    assert_files_dirs_empty(&w.nodes[1]);
    assert_eq!(departures(&w).len(), 1);
}

// --- Second review of lot C ----------------------------------------------------------

// Rotations made at once (§7.6)

#[test]
fn two_rotations_made_at_once_post_only_one_by_the_loser() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    assert_eq!(epoch(&w), 1);
    // Member 1 starts to rotate; member 0 gets in first: the synchronisation a rotation begins
    // with is answered before member 0's rotation exists, so member 1 posts a stale epoch.
    let e0 = w.nodes[0].engine.clone();
    w.relay.during_next("events", move || {
        assert!(e0.maybe_rotate().unwrap());
    });

    let posted = w.e(1).maybe_rotate().unwrap();

    assert!(!posted, "member 0 had done it: nothing to post again");
    assert_eq!(epoch(&w), 2, "no second rotation after the conflict");
    assert_eq!(
        w.relay.count("post_rotation"),
        2,
        "the winner's, and the loser's single refused try"
    );
    assert_eq!(rotation_flags(w.e(1)), (false, None));
    // Member 1 follows the key of the winner.
    let id = send(&w, 1, text("apres la course"));
    w.e(0).sync().unwrap();
    assert_eq!(
        plain_of(&w.e(0).get_item(&id).unwrap().unwrap()),
        "apres la course"
    );
}

#[test]
fn two_members_revoking_the_same_device_make_one_rotation() {
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    let target = w.nodes[2].id.clone();
    let (e0, t0) = (w.nodes[0].engine.clone(), target.clone());
    w.relay
        .during_next("events", move || e0.revoke_device(&t0).unwrap());

    // Done by the other one meanwhile: not an error, not a second rotation.
    w.e(1).revoke_device(&target).unwrap();

    assert_eq!(epoch(&w), 2);
    assert_eq!(w.relay.count("post_rotation"), 2);
    // An unknown device is still an error when nobody did anything.
    assert!(matches!(
        w.e(1).revoke_device("inconnu").unwrap_err(),
        crate::CoreError::InvalidInput { .. }
    ));
}

#[test]
fn a_session_that_says_a_rotation_is_owed_starts_the_rotation() {
    let w = World::new(2, 2);
    w.e(0).sync().unwrap();
    w.e(1).sync().unwrap();
    // An administrator revoked someone and this device missed the event.
    w.relay.with(|s| s.rotation_required = true);
    w.e(0).drop_session();
    w.e(0).ensure_session().unwrap();
    let (required, due) = rotation_flags(w.e(0));
    assert!(required);
    assert!((T0..=T0 + 30_000).contains(&due.unwrap()), "{due:?}");
    assert_eq!(epoch(&w), 1, "not before the random delay");

    w.clock.advance(30_001);
    w.e(0).sync().unwrap();

    assert_eq!(
        epoch(&w),
        2,
        "started by the session, without any call of the app"
    );
    assert_eq!(rotation_flags(w.e(0)), (false, None));
}

#[test]
fn a_device_set_mismatch_waits_an_hour_and_then_tries_again() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.clock.advance(31_000);
    for _ in 0..2 {
        w.relay
            .fail_next("post_rotation", problem(409, "device_set_mismatch"));
    }

    w.e(0).sync().unwrap();

    assert_eq!(
        w.relay.count("post_rotation"),
        2,
        "the try and its single retry"
    );
    assert_eq!(epoch(&w), 1);
    let now = w.clock.now_ms();
    assert_eq!(rotation_flags(w.e(0)), (true, Some(now + 60 * MIN)));
    assert_eq!(w.e(0).flags.lock().unwrap().rotation_failures, 0);
    // Not before the hour.
    w.clock.advance(59 * MIN);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("post_rotation"), 2);
    w.clock.advance(2 * MIN);
    w.e(0).sync().unwrap();
    assert_eq!(epoch(&w), 2);
}

#[test]
fn no_automatic_rotation_while_not_found_revoked_or_blocked() {
    for blocked in [
        Some(Blocked::NotFound),
        Some(Blocked::Revoked),
        Some(Blocked::UpdateRequired { update_url: None }),
    ] {
        let w = after_a_departure();
        w.e(0).sync().unwrap();
        match blocked.clone().unwrap() {
            Blocked::NotFound => mark_not_found(w.e(0)),
            other => mark_blocked(w.e(0), other),
        }
        w.clock.advance(31_000);
        let _ = w.e(0).sync();
        w.e(0).run_due_rotation();
        assert_eq!(w.relay.count("post_rotation"), 0, "{blocked:?}");
        assert_eq!(epoch(&w), 1, "{blocked:?}");
        assert_eq!(w.e(0).blocked(), blocked);
        assert_intact(w.e(0), 0);
    }
}

#[test]
fn the_rotation_failures_are_forgotten_once_no_rotation_is_owed() {
    let w = after_a_departure();
    w.e(0).sync().unwrap();
    w.clock.advance(31_000);
    w.relay
        .fail_next("post_rotation", RelayError::Network("coupure".into()));
    w.e(0).sync().unwrap();
    assert_eq!(w.e(0).flags.lock().unwrap().rotation_failures, 1);
    // Member 1 rotates meanwhile.
    w.e(1).sync().unwrap();
    w.clock.advance(31_000);
    w.e(1).sync().unwrap();
    assert_eq!(epoch(&w), 2);

    // The next session of member 0 says that nothing is owed any more.
    w.e(0).drop_session();
    w.e(0).ensure_session().unwrap();

    let f = w.e(0).flags.lock().unwrap();
    assert_eq!(
        (
            f.rotation_failures,
            f.rotation_due_at_ms,
            f.rotation_required
        ),
        (0, None, false)
    );
}

// A `403 revoked` from the challenge route (§7.9.1)

#[test]
fn a_403_revoked_from_the_challenge_route_is_not_a_proof() {
    let w = pair_with_item();
    w.relay.fail_next("auth_challenge", problem(403, "revoked"));
    assert_eq!(w.e(1).leave_space().unwrap(), O::Refused(R::Unexpected));
    assert_eq!(leave_state(w.e(1)), None);
    assert_intact(w.e(1), 1);
    assert_eq!(w.e(1).blocked(), None, "not blocked either");
    assert!(!w.e(1).can_erase_without_proof());
    assert_eq!(w.relay.count("leave_space"), 0);

    // The same answer to a deletion and to a verification: nothing erased.
    let w = alone_after_last_device();
    w.relay.fail_next("auth_challenge", problem(403, "revoked"));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Refused(R::Unexpected));
    assert_intact(w.e(0), 1);
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", Some(T0));
    w.relay.fail_next("auth_challenge", problem(403, "revoked"));
    assert_eq!(
        w.e(1).continue_departure().unwrap(),
        O::Refused(R::Unexpected)
    );
    assert_intact(w.e(1), 1);

    // The ordinary session of the engine does not take it for a revocation.
    let w = pair_with_item();
    w.e(1).drop_session();
    w.relay.fail_next("auth_challenge", problem(403, "revoked"));
    assert!(w.e(1).sync().is_err());
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None,
        "nothing persisted"
    );
    assert_intact(w.e(1), 1);
}

// The doorbell (§5.4)

/// The stop flag and the back-off counter `Engine::doorbell_round` is driven with.
fn doorbell_state() -> (std::sync::atomic::AtomicBool, u32) {
    (std::sync::atomic::AtomicBool::new(false), 0)
}

/// A departure requested just now, as a gesture followed by a stop would leave it: within the
/// 15 minutes during which the request may be sent again.
fn write_recent_marker(e: &Engine) {
    write_marker(e, b"requested", Some(T0));
}

/// The departure marker written by [`write_recent_marker`] is still there, with its date:
/// whatever the relay said, nothing was erased and nothing was cancelled.
fn assert_marker_kept(e: &Engine) {
    assert!(requested(e), "the departure marker was kept");
    assert_eq!(
        e.store.read(|db| db.meta_get("departure_since")).unwrap(),
        Some(T0.to_string().into_bytes()),
        "with its date"
    );
}

#[test]
fn a_not_found_met_elsewhere_does_not_end_the_doorbell_and_it_comes_back_with_the_relay() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    // A synchronisation meets a relay whose volume is not mounted yet.
    w.relay.uninitialize();
    assert!(w.e(1).sync().is_err());
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));

    // The doorbell starts within the minute: it does not give up, it waits at least that
    // minute (and not much more), without any request.
    let calls = w.relay.with(|s| s.calls.len());
    let wait = w
        .e(1)
        .doorbell_round(&stop, &mut attempt)
        .expect("the doorbell goes on");
    assert_eq!(wait, 60_000, "the first answer: the minute, no more");
    assert_eq!(w.relay.with(|s| s.calls.len()), calls, "no request yet");

    // The relay is back; the wait has passed. Nobody told the doorbell.
    w.relay.with(|s| s.initialized = true);
    w.clock.advance(i64::try_from(wait).unwrap());
    let next = w.e(1).doorbell_round(&stop, &mut attempt);

    assert!(next.is_some(), "connected, then closed by the double");
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(w.relay.count("open_wake"), 1, "the doorbell is open again");
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

#[test]
fn a_doorbell_that_still_finds_nothing_asks_again_with_a_fresh_session_and_never_erases() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();

    // The session in hand is still valid: the doorbell meets the empty relay when it connects.
    let first = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert!(first >= 60_000);
    assert_eq!(w.relay.count("open_wake"), 1);
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));

    // Back too soon (the app starts the doorbell again): no request.
    let calls = w.relay.with(|s| s.calls.len());
    w.clock.advance(30_000);
    let again = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert!(again >= 60_000);
    assert_eq!(w.relay.with(|s| s.calls.len()), calls);

    // A minute after the last answer: asked again, from a fresh challenge.
    w.clock.advance(30_000);
    let challenges = w.relay.count("auth_challenge");
    let later = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert!(later >= 60_000);
    assert_eq!(w.relay.count("auth_challenge"), challenges + 1);
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
}

#[test]
fn the_doorbell_goes_on_after_a_4004_when_the_fresh_session_says_member() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.close_next_doorbell(4004);
    let sessions = w.relay.count("auth_session");

    let wait = w
        .e(1)
        .doorbell_round(&stop, &mut attempt)
        .expect("a fresh session said: still a member");

    assert!(wait <= 1000, "the normal delay of a first attempt: {wait}");
    assert_eq!(
        w.relay.count("auth_session"),
        sessions + 1,
        "the 4004 was verified by a fresh session"
    );
    assert_eq!(attempt, 1);
    // The loop starts again by itself.
    let second = w.e(1).doorbell_round(&stop, &mut attempt);
    assert!(second.is_some());
    assert_eq!(w.relay.count("open_wake"), 2, "connected a second time");
    assert_eq!(w.e(1).blocked(), None);
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

#[test]
fn the_doorbell_stops_after_a_4004_only_once_a_fresh_session_has_not_found_the_space() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.close_next_doorbell(4004);
    let relay = w.relay.clone();
    // The relay loses its space while the connection is up (the catch-up after `ready`).
    w.relay.during_next("events", move || relay.uninitialize());

    assert_eq!(w.e(1).doorbell_round(&stop, &mut attempt), None);

    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
}

/// A `4004`, then a check of the membership that meets `fault`: the doorbell goes on with the
/// normal back-off, connects again, and nothing is blocked nor erased.
fn doorbell_goes_on_after_a_4004_and(fault: RelayError) {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.close_next_doorbell(4004);
    // The connection is up and closed at once; the fresh session that follows meets the fault.
    w.relay.fail_next("auth_challenge", fault);
    let sessions = w.relay.count("auth_session");

    let wait = w
        .e(1)
        .doorbell_round(&stop, &mut attempt)
        .expect("the doorbell goes on");

    assert!(wait <= 1000, "the normal delay of a first attempt: {wait}");
    assert_eq!(attempt, 1);
    assert_eq!(w.relay.count("auth_session"), sessions, "the check failed");
    assert_eq!(w.e(1).blocked(), None);
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));

    // Next turn: a session opens again and the doorbell connects a second time.
    assert!(w.e(1).doorbell_round(&stop, &mut attempt).is_some());
    assert_eq!(w.relay.count("open_wake"), 2);
    assert_eq!(w.e(1).blocked(), None);
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

#[test]
fn the_doorbell_goes_on_after_a_4004_when_the_fresh_session_is_transient() {
    doorbell_goes_on_after_a_4004_and(problem(503, "unavailable"));
    doorbell_goes_on_after_a_4004_and(RelayError::Network("coupure".into()));
}

#[test]
fn the_doorbell_goes_on_after_a_4004_when_the_fresh_session_answers_something_unexpected() {
    // A `403` of a proxy (no `revoked` code), a `422`: on a live space, not a reason to stop.
    doorbell_goes_on_after_a_4004_and(problem(403, "forbidden"));
    doorbell_goes_on_after_a_4004_and(problem(422, "validation_failed"));
}

#[test]
fn the_not_found_wait_doubles_up_to_half_an_hour_and_never_drops_below_the_minute() {
    use super::engine::{not_found_ceiling_ms, not_found_wait_ms};
    let ceilings: Vec<u64> = (1..=8).map(not_found_ceiling_ms).collect();
    assert_eq!(
        ceilings,
        [60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000, 1_800_000]
    );
    assert_eq!(not_found_ceiling_ms(0), 60_000);
    assert_eq!(not_found_ceiling_ms(u32::MAX), 1_800_000);
    for streak in [0, 1, 2, 3, 6, 50, u32::MAX] {
        let c = not_found_ceiling_ms(streak);
        for _ in 0..50 {
            let w = not_found_wait_ms(streak);
            assert!(w <= c && w >= 60_000 && w >= c / 4 * 3, "{streak}: {w}");
        }
    }
}

#[test]
fn consecutive_not_found_answers_slow_the_doorbell_down_until_a_session_opens() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();

    // Every turn asks the relay, hears "not found", and waits longer than the one before.
    let mut ceiling = 30_000;
    for turn in 1..=8 {
        let challenges = w.relay.count("auth_challenge");
        let wait = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
        if turn > 1 {
            assert_eq!(
                w.relay.count("auth_challenge"),
                challenges + 1,
                "turn {turn}"
            );
        }
        ceiling = (ceiling * 2).min(1_800_000);
        assert!(
            ((ceiling / 4 * 3).max(60_000)..=ceiling).contains(&wait),
            "turn {turn}: {wait} for {ceiling}"
        );
        assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
        w.clock.advance(i64::try_from(wait).unwrap());
    }
    assert_eq!(ceiling, 1_800_000, "capped at half an hour");

    // The turn after that still waits at most half an hour. Asked again too soon (the app
    // restarts the doorbell), the next one makes no request and waits no longer.
    let capped = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert!((1_350_000..=1_800_000).contains(&capped), "{capped}");
    let calls = w.relay.with(|s| s.calls.len());
    w.clock.advance(1_000);
    let capped_again = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert!((1_350_000..=1_800_000).contains(&capped_again));
    assert_eq!(w.relay.with(|s| s.calls.len()), calls, "no request");

    // The relay is back: a session opens, the doorbell connects, the count starts over.
    w.relay.with(|s| s.initialized = true);
    w.clock.advance(61_000);
    assert!(w.e(1).doorbell_round(&stop, &mut attempt).is_some());
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.relay.count("open_wake"),
        2,
        "the failed call and this one"
    );

    // And empty again: the first answer waits the minute again, not the half hour.
    w.relay.uninitialize();
    w.clock.advance(61_000);
    let wait = w.e(1).doorbell_round(&stop, &mut attempt).unwrap();
    assert_eq!(wait, 60_000);
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

/// The "not found" streak of the back-off, as the engine counts it now.
fn not_found_streak(e: &Engine) -> u32 {
    e.flags.lock().unwrap().not_found_streak
}

/// Waits (up to 10 s) for a condition that other threads bring about, looking at it every 5 ms.
fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !cond() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "never happened: {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_wait_after_not_found_ends_as_soon_as_a_session_opens_elsewhere() {
    use super::engine::DoorbellWait;
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();
    let wait = w.e(1).doorbell_turn(&stop, &mut attempt).unwrap();
    assert!(wait.after_not_found);
    assert!(wait.ms >= 60_000);
    assert_eq!(not_found_streak(w.e(1)), 1);

    // The foreground `sync` finds the relay back after a minute and opens a session, at the
    // third slice of the wait.
    let mut slices = 0;
    w.e(1).doorbell_pause(&stop, &wait, &mut |ms| {
        assert_eq!(ms, 100, "slices of 100 ms");
        slices += 1;
        if slices == 3 {
            w.relay.with(|s| s.initialized = true);
            w.clock.advance(60_000);
            w.e(1).ensure_session().unwrap();
            assert_eq!(w.e(1).blocked(), None);
        }
    });
    assert_eq!(slices, 3, "the wait ended with the state it was about");
    assert_eq!(not_found_streak(w.e(1)), 0);

    // The next turn connects at once; nothing was erased.
    let opened = w.relay.count("open_wake");
    assert!(w.e(1).doorbell_turn(&stop, &mut attempt).is_some());
    assert_eq!(w.relay.count("open_wake"), opened + 1);
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));

    // A wait that is not about a "not found" is not cut short by a session: whole.
    let plain = DoorbellWait {
        ms: 1_050,
        after_not_found: false,
    };
    let mut total = 0;
    w.e(1).doorbell_pause(&stop, &plain, &mut |ms| total += ms);
    assert_eq!(total, 1_050);
}

#[test]
fn a_wait_after_not_found_lasts_the_whole_time_while_the_relay_still_finds_nothing() {
    let w = pair_with_item();
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();
    let wait = w.e(1).doorbell_turn(&stop, &mut attempt).unwrap();

    let mut total = 0;
    w.e(1).doorbell_pause(&stop, &wait, &mut |ms| total += ms);

    assert_eq!(total, wait.ms);
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
}

#[test]
fn a_wait_after_not_found_ends_when_the_device_is_erased_or_blocked_and_stop_is_honoured() {
    let w = pair_with_item();
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();
    let wait = w.e(1).doorbell_turn(&stop, &mut attempt).unwrap();

    // Revoked elsewhere: the next turn must learn it (and end the doorbell).
    let mut slices = 0;
    w.e(1).doorbell_pause(&stop, &wait, &mut |_| {
        slices += 1;
        if slices == 2 {
            mark_blocked(w.e(1), Blocked::Revoked);
        }
    });
    assert_eq!(slices, 2);
    assert_eq!(w.e(1).doorbell_turn(&stop, &mut attempt), None);

    // Stop asked meanwhile.
    let w = pair_with_item();
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();
    let wait = w.e(1).doorbell_turn(&stop, &mut attempt).unwrap();
    let mut slices = 0;
    w.e(1).doorbell_pause(&stop, &wait, &mut |_| {
        slices += 1;
        if slices == 4 {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });
    assert_eq!(slices, 4);

    // The device is erased: the wait ends, and the turn after it ends the doorbell.
    let w = pair_with_item();
    let (stop, mut attempt) = doorbell_state();
    w.relay.uninitialize();
    let wait = w.e(1).doorbell_turn(&stop, &mut attempt).unwrap();
    let mut slices = 0;
    w.e(1).doorbell_pause(&stop, &wait, &mut |_| {
        slices += 1;
        if slices == 1 {
            w.e(1).erase_without_proof().unwrap();
        }
    });
    assert_eq!(slices, 1);
    assert_eq!(w.e(1).doorbell_turn(&stop, &mut attempt), None);
}

#[test]
fn a_second_doorbell_loop_waits_for_the_first_and_never_runs_beside_it() {
    use std::sync::atomic::Ordering;
    let w = World::new(2, 2);
    let e = w.nodes[1].engine.clone();
    let stop1 = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s) = (e.clone(), stop1.clone());
    let first = std::thread::spawn(move || b.run_doorbell(&s));
    wait_for("the first loop connects", || {
        w.relay.count("open_wake") >= 1
    });
    assert!(
        e.doorbell_gate.try_lock().is_err(),
        "the running loop holds the gate"
    );

    // A second call, with the first one still running: it waits, and connects to nothing.
    let stop2 = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s) = (e.clone(), stop2.clone());
    let second = std::thread::spawn(move || b.run_doorbell(&s));
    wait_for("the second call waits", || {
        e.doorbell_waiting.load(Ordering::SeqCst) == 1
    });
    assert!(!first.is_finished());
    assert!(!second.is_finished(), "it does not return at once");

    // The first loop ends: the second one takes over, on its own.
    stop1.store(true, Ordering::SeqCst);
    first.join().unwrap();
    let opened = w.relay.count("open_wake");
    wait_for("the second loop connects", || {
        w.relay.count("open_wake") > opened
    });
    assert_eq!(e.doorbell_waiting.load(Ordering::SeqCst), 0);
    assert!(e.doorbell_gate.try_lock().is_err(), "held by the second");
    assert!(!second.is_finished());
    stop2.store(true, Ordering::SeqCst);
    second.join().unwrap();
    assert!(e.doorbell_gate.try_lock().is_ok(), "released at the end");
}

#[test]
fn a_second_doorbell_loop_told_to_stop_while_it_waits_never_connects() {
    use std::sync::atomic::Ordering;
    let w = World::new(2, 2);
    let e = w.nodes[1].engine.clone();
    // A loop is running (the gate is held, as `run_doorbell` holds it).
    let gate = e.doorbell_gate.lock().unwrap();
    let (challenges, opened) = (w.relay.count("auth_challenge"), w.relay.count("open_wake"));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (b, s) = (e.clone(), stop.clone());
    let waiting = std::thread::spawn(move || b.run_doorbell(&s));
    wait_for("the call waits", || {
        e.doorbell_waiting.load(Ordering::SeqCst) == 1
    });
    assert_eq!(w.relay.count("open_wake"), opened, "no connection");
    assert_eq!(w.relay.count("auth_challenge"), challenges, "no session");

    stop.store(true, Ordering::SeqCst);
    waiting.join().unwrap();

    assert_eq!(e.doorbell_waiting.load(Ordering::SeqCst), 0);
    assert_eq!(w.relay.count("open_wake"), opened, "it never connected");
    assert_eq!(w.relay.count("auth_challenge"), challenges);
    drop(gate);
    assert!(e.doorbell_gate.try_lock().is_ok());
}

#[test]
fn the_not_found_streak_starts_over_after_the_identity_is_forgotten_and_after_a_new_provisioning() {
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let stale = w.e(1).active().unwrap();
    mark_not_found(w.e(1));
    mark_not_found(w.e(1));
    assert_eq!(not_found_streak(w.e(1)), 2);

    w.e(1).erase_without_proof().unwrap();
    assert!(!w.e(1).is_provisioned());
    assert_eq!(not_found_streak(w.e(1)), 0, "forgotten with the identity");
    assert_eq!(w.e(1).blocked(), None);

    provision_fresh(w.e(1));
    assert_eq!(not_found_streak(w.e(1)), 0, "a new device starts from zero");
    assert_eq!(w.e(1).blocked(), None);
    // A late answer of the old identity counts for nothing in the new one.
    w.e(1).set_not_found(&stale);
    assert_eq!(not_found_streak(w.e(1)), 0);
    // And the first answer for the new identity is the first of a streak: the minute.
    mark_not_found(w.e(1));
    assert_eq!(not_found_streak(w.e(1)), 1);
    assert_eq!(super::engine::not_found_ceiling_ms(1), 60_000);
}

/// The identity is replaced while `authed` renews the session after a `session_expired`, and
/// the second call then gets `second`: it is read for the identity the request was made for,
/// and marks nothing in the one that replaced it. Nothing is erased on this path.
fn the_second_call_of_authed_marks_nothing_for_a_replaced_identity(second: RelayError) {
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    let relay = w.relay.clone();
    w.relay
        .fail_next("rename_device", problem(401, "session_expired"));
    w.relay.fail_next("rename_device", second);
    // The first call returns `session_expired`; the replacement lands during the second one,
    // after the session was renewed, before its answer is read.
    w.relay.during_next("rename_device", move || {
        relay.during_next("rename_device", move || {
            mark_not_found(&e1);
            e1.erase_without_proof().unwrap();
            provision_fresh(&e1);
        });
    });
    let me = w.nodes[1].id.clone();
    let sessions = w.relay.count("auth_session");

    assert!(w.e(1).rename_device(&me, "Nouveau nom").is_err());

    assert_eq!(w.relay.count("auth_session"), sessions + 1, "renewed once");
    assert_eq!(w.relay.count("rename_device"), 2, "both calls were made");
    assert!(w.e(1).is_provisioned());
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(not_found_streak(w.e(1)), 0);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert!(!w.e(1).can_erase_without_proof(), "no verdict either");
}

#[test]
fn the_second_call_of_authed_marks_nothing_for_an_identity_replaced_during_the_renewal() {
    the_second_call_of_authed_marks_nothing_for_a_replaced_identity(problem(
        409,
        "space_not_initialized",
    ));
    the_second_call_of_authed_marks_nothing_for_a_replaced_identity(problem(403, "revoked"));
}

// A late answer of an erased identity (§7.9.6)

#[test]
fn a_late_answer_of_an_erased_identity_never_marks_the_identity_that_replaced_it() {
    let w = pair_with_item();
    let stale = w.e(1).active().unwrap();
    mark_not_found(w.e(1));
    w.e(1).erase_without_proof().unwrap();
    provision_fresh(w.e(1));
    assert!(w.e(1).is_provisioned());

    w.e(1).set_blocked(&stale, Blocked::Revoked);
    w.e(1)
        .set_blocked(&stale, Blocked::UpdateRequired { update_url: None });
    w.e(1).set_not_found(&stale);

    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
    assert!(!w.e(1).can_erase_without_proof(), "no verdict either");
    // The live identity is still marked normally.
    mark_blocked(w.e(1), Blocked::Revoked);
    assert_eq!(w.e(1).blocked(), Some(Blocked::Revoked));
}

#[test]
fn a_403_answering_a_request_of_an_erased_identity_does_not_block_its_successor() {
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    w.relay.fail_next("rename_device", problem(403, "revoked"));
    w.relay.during_next("rename_device", move || {
        // The erasure and a new provisioning land while the request is in flight.
        mark_not_found(&e1);
        e1.erase_without_proof().unwrap();
        provision_fresh(&e1);
    });
    let me = w.nodes[1].id.clone();

    assert!(w.e(1).rename_device(&me, "Nouveau nom").is_err());

    assert!(w.e(1).is_provisioned());
    assert_eq!(w.e(1).blocked(), None);
    assert_eq!(
        w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
        None
    );
}

#[test]
fn the_answer_after_a_session_renewal_marks_the_device_like_the_first_one_and_erases_nothing() {
    let marker = |e: &Engine| e.store.read(|db| db.meta_get("blocked")).unwrap();

    // `403 revoked` at the second call: REVOKED, persisted, data kept.
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let me = w.nodes[1].id.clone();
    w.relay
        .fail_next("rename_device", problem(401, "session_expired"));
    w.relay.fail_next("rename_device", problem(403, "revoked"));
    let sessions = w.relay.count("auth_session");
    assert!(w.e(1).rename_device(&me, "Nouveau nom").is_err());
    assert_eq!(w.relay.count("auth_session"), sessions + 1, "renewed once");
    assert_eq!(w.e(1).blocked(), Some(Blocked::Revoked));
    assert_eq!(marker(w.e(1)).as_deref(), Some(b"revoked".as_slice()));
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));

    // `409 space_not_initialized` at the second call: NOT FOUND, in memory only.
    let w = pair_with_item();
    write_recent_marker(w.e(1));
    let me = w.nodes[1].id.clone();
    w.relay
        .fail_next("rename_device", problem(401, "session_expired"));
    w.relay
        .fail_next("rename_device", problem(409, "space_not_initialized"));
    assert!(w.e(1).rename_device(&me, "Nouveau nom").is_err());
    assert_eq!(w.e(1).blocked(), Some(Blocked::NotFound));
    assert_eq!(marker(w.e(1)), None, "never written");
    assert_intact(w.e(1), 1);
    assert_marker_kept(w.e(1));
}

#[test]
fn a_late_answer_after_a_session_renewal_does_not_block_the_identity_that_replaced_it() {
    for second in [
        problem(403, "revoked"),
        problem(409, "space_not_initialized"),
    ] {
        let w = pair_with_item();
        let (e1, relay) = (w.nodes[1].engine.clone(), w.relay.clone());
        w.relay
            .fail_next("rename_device", problem(401, "session_expired"));
        w.relay.fail_next("rename_device", second);
        // The erasure and a new provisioning land while the second request is in flight.
        w.relay.during_next("rename_device", move || {
            relay.during_next("rename_device", move || {
                mark_not_found(&e1);
                e1.erase_without_proof().unwrap();
                provision_fresh(&e1);
            });
        });
        let me = w.nodes[1].id.clone();

        assert!(w.e(1).rename_device(&me, "Nouveau nom").is_err());

        assert!(w.e(1).is_provisioned());
        assert_eq!(w.e(1).blocked(), None);
        assert_eq!(
            w.e(1).store.read(|db| db.meta_get("blocked")).unwrap(),
            None
        );
    }
}

// Departures: receipt, cancellation, window

#[test]
fn t1_a_deletion_whose_receipt_appears_at_the_second_check_is_proven() {
    let w = alone_after_last_device();
    w.relay.uninitialize();
    let (relay, e) = (w.relay.clone(), w.nodes[0].engine.clone());
    // The first look at the receipt finds nothing; the deletion, whose answer was lost, shows
    // right after.
    w.relay.during_next("space_deletion_receipt", move || {
        let id: [u8; 32] = stored_deletion_id(&e).unwrap().try_into().unwrap();
        relay.add_receipt(id);
    });

    assert_eq!(w.e(0).delete_space().unwrap(), O::Erased);

    assert_eq!(w.relay.count("space_deletion_receipt"), 2);
    assert_eq!(w.relay.count("delete_space"), 0, "nothing was sent again");
    assert_erased(w.e(0));
}

#[test]
fn cancelling_during_the_fresh_session_erases_nothing_whatever_it_answers() {
    // The session answers a server error: pending, but cancelled meanwhile.
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    w.relay
        .fail_next("auth_session", problem(503, "unavailable"));
    w.relay.during_next("auth_session", move || {
        assert!(e1.cancel_departure().unwrap())
    });
    assert_eq!(w.e(1).leave_space().unwrap(), O::Cancelled);
    assert_eq!(leave_state(w.e(1)), None);
    assert!(w.e(1).departure().is_none());
    assert_eq!(w.relay.count("leave_space"), 0);
    assert_intact(w.e(1), 1);

    // The session says "revoked" (a proof of departure): nothing is erased, the device shows
    // REVOQUE and its data stay readable.
    let w = World::new(3, 3);
    for i in 0..3 {
        w.e(i).sync().unwrap();
    }
    send(&w, 0, text("avant la revocation"));
    w.e(2).sync().unwrap();
    w.e(0).revoke_device(&w.nodes[2].id).unwrap();
    let e2 = w.nodes[2].engine.clone();
    w.relay.during_next("auth_session", move || {
        assert!(e2.cancel_departure().unwrap())
    });
    assert_eq!(w.e(2).leave_space().unwrap(), O::Cancelled);
    assert_eq!(leave_state(w.e(2)), None);
    assert_intact(w.e(2), 1);
    assert_eq!(w.e(2).blocked(), Some(Blocked::Revoked));
}

#[test]
fn cancelling_while_a_deletion_turns_into_a_departure_erases_nothing() {
    let w = pair_with_item();
    let e1 = w.nodes[1].engine.clone();
    // `409 not_last_device` comes back; the user cancels before the marker is switched.
    w.relay.during_next("delete_space", move || {
        assert!(e1.cancel_departure().unwrap())
    });

    assert_eq!(w.e(1).delete_space().unwrap(), O::Cancelled);

    assert_eq!(leave_state(w.e(1)), None, "not switched to `requested`");
    assert_eq!(w.relay.count("leave_space"), 0, "no departure was tried");
    assert!(departures(&w).is_empty());
    assert!(w.e(1).departure().is_none());
    assert_intact(w.e(1), 1);
    assert!(
        stored_deletion_id(w.e(1)).is_some(),
        "kept by a cancellation"
    );
}

#[test]
fn a_departure_dated_in_the_future_only_verifies() {
    // The clock was moved back: the gesture "is" later than now, which is not a recent gesture.
    let w = pair_with_item();
    write_marker(w.e(1), b"requested", Some(T0 + 10 * MIN));
    assert_eq!(w.e(1).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.count("leave_space"), 0);
    assert!(requested(w.e(1)));
    assert_intact(w.e(1), 1);

    let w = alone_after_last_device();
    write_marker(w.e(0), b"deleting", Some(T0 + 10 * MIN));
    assert_eq!(w.e(0).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.count("delete_space"), 0);
    assert_eq!(
        w.relay.count("space_deletion_receipt"),
        1,
        "the receipt only"
    );
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
}

#[test]
fn a_deletion_resent_within_the_window_carries_the_same_deletion_id() {
    let w = alone_after_last_device();
    w.relay
        .fail_next("delete_space", RelayError::Network("coupure".into()));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    let id = stored_deletion_id(w.e(0)).unwrap();
    w.clock.advance(14 * MIN);

    assert_eq!(w.e(0).continue_departure().unwrap(), O::Erased);

    let sent = w.relay.deletion_ids_sent();
    assert_eq!(
        sent.len(),
        2,
        "the failed try and the one that went through"
    );
    assert_eq!(sent[0], sent[1], "the same identifier");
    assert_eq!(sent[0].as_slice(), id.as_slice());
    assert_eq!(w.relay.only_receipt(), Some(sent[0]));
    assert_erased(w.e(0));

    // Outside the window nothing is sent again, only verified.
    let w = alone_after_last_device();
    w.relay
        .fail_next("delete_space", RelayError::Network("coupure".into()));
    assert_eq!(w.e(0).delete_space().unwrap(), O::Pending(P::Network));
    w.clock.advance(16 * MIN);
    assert_eq!(w.e(0).continue_departure().unwrap(), O::Pending(P::NotSent));
    assert_eq!(w.relay.deletion_ids_sent().len(), 1);
    assert!(deleting(w.e(0)));
    assert_intact(w.e(0), 1);
}

#[test]
fn an_attachment_over_the_relay_limit_is_refused_before_anything_is_written() {
    let w = World::new(1, 1);
    let limit = w.e(0).limits().max_blob_bytes;
    let src = w.nodes[0].dir.path().join("huge.bin");
    // Sparse file: costs nothing to create, one byte over what a blob can hold.
    let f = std::fs::File::create(&src).unwrap();
    f.set_len(limit + 1).unwrap();
    drop(f);
    let id = "00000000-0000-4000-8000-0000000000a1";
    let mut d = text("trop gros");
    d.id = Some(id.into());
    d.attachments.push(DraftAttachment {
        path: src.to_string_lossy().into_owned(),
        name: "huge.bin".into(),
        mime: "application/octet-stream".into(),
        thumbnail: None,
    });
    // The encrypted copy would go to `files/files/{id}_0.enc` (the store's files directory
    // is `files`). A directory sits there: creating the copy would fail with an I/O error,
    // so only a refusal made BEFORE the copy is created can give the size error below.
    let dest = w.nodes[0]
        .dir
        .path()
        .join("files")
        .join(crate::runtime::store::encrypted_file_name(id, 0));
    std::fs::create_dir_all(&dest).unwrap();
    let err = w.e(0).enqueue(d).unwrap_err();
    assert!(
        err.to_string().contains("larger than the relay limit"),
        "{err}"
    );
    assert!(w.e(0).list_items().unwrap().is_empty());
    // No encrypted copy was written anywhere under the files directory (recursively).
    let mut written = Vec::new();
    walk(&w.nodes[0].dir.path().join("files"), &mut written);
    assert!(written.is_empty(), "files written: {written:?}");
}

#[path = "removal_tests.rs"]
mod attachment_removal;

// ---------------------------------------------------------------------------
// Activity of the devices (`last_seen_at` of `GET /v1/devices`, api-v1 §4.3)
// ---------------------------------------------------------------------------

fn last_seen(e: &Engine, device_id: &str) -> Option<i64> {
    e.list_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.id == device_id)
        .unwrap()
        .last_seen_at_ms
}

const ACTIVITY_REFRESH_MS: i64 = 5 * 60_000;

#[test]
fn the_time_of_pairing_is_not_an_activity() {
    let w = World::new(2, 2);
    // Provisioned, never synced: nothing is known about anybody.
    for n in &w.nodes {
        assert_eq!(last_seen(w.e(0), &n.id), None);
    }
}

#[test]
fn last_seen_is_the_value_of_the_relay() {
    let w = World::new(2, 2);
    let seen = T0 - 3 * 86_400_000;
    w.relay
        .set_device_activity(&w.nodes[1].id, Some(seen), Some(true));
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &w.nodes[1].id), Some(seen));
    // A device that never connected: absent, not a made-up date.
    assert_eq!(w.relay.count("devices"), 1);
}

#[test]
fn an_absent_last_seen_replaces_an_older_value_when_the_relay_reports_activity() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    w.relay
        .set_device_activity(&id, Some(T0 - 1_000), Some(true));
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 1_000));
    // The relay was reset: that device never connected to it.
    w.relay.set_device_activity(&id, None, Some(false));
    w.clock.advance(ACTIVITY_REFRESH_MS);
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), None);
}

#[test]
fn an_older_relay_that_reports_nothing_changes_nothing() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    // Nothing known and nothing reported: still nothing, never the pairing time.
    w.relay.set_device_activity(&id, None, None);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("devices"), 1);
    assert_eq!(last_seen(w.e(0), &id), None);
    // A value learnt earlier is not erased by a relay that says nothing.
    w.relay
        .set_device_activity(&id, Some(T0 - 5_000), Some(true));
    w.clock.advance(ACTIVITY_REFRESH_MS);
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 5_000));
    w.relay.set_device_activity(&id, None, None);
    w.clock.advance(ACTIVITY_REFRESH_MS);
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 5_000));
}

#[test]
fn a_stale_value_is_replaced_at_the_next_refresh_and_not_before() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    w.relay
        .set_device_activity(&id, Some(T0 - 60_000), Some(true));
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 60_000));

    // The device is used again: the relay moves on, this one reads it at its next turn only.
    w.relay
        .set_device_activity(&id, Some(T0 + 120_000), Some(true));
    w.clock.advance(ACTIVITY_REFRESH_MS - 1);
    w.e(0).sync().unwrap();
    assert_eq!(
        w.relay.count("devices"),
        1,
        "not asked again before the delay"
    );
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 60_000));

    w.clock.advance(1);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("devices"), 2);
    assert_eq!(last_seen(w.e(0), &id), Some(T0 + 120_000));
}

#[test]
fn the_apps_can_refresh_the_list_whenever_they_open_it() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    w.relay.set_device_activity(&id, Some(T0 - 1), Some(true));
    w.e(0).sync().unwrap();
    w.relay
        .set_device_activity(&id, Some(T0 + 60_000), Some(true));
    w.e(0).refresh_devices().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 + 60_000));
}

#[test]
fn a_refresh_that_fails_does_not_fail_the_sync_and_is_tried_again_in_a_minute() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    w.relay.set_device_activity(&id, Some(T0 - 1), Some(true));
    w.relay
        .fail_next("devices", RelayError::Network("coupure".into()));
    let r = w.e(0).sync().unwrap();
    assert!(!r.offline, "the events were fetched");
    assert_eq!(last_seen(w.e(0), &id), None);
    w.clock.advance(30_000);
    w.e(0).sync().unwrap();
    assert_eq!(w.relay.count("devices"), 1, "not before a minute");
    w.clock.advance(30_000);
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 - 1));
}

#[test]
fn a_full_resync_stores_the_activity_too() {
    let w = World::new(2, 2);
    let id = w.nodes[0].id.clone();
    w.e(1).sync().unwrap();
    send(&w, 0, text("manque"));
    let head = w.relay.head();
    send(&w, 0, text("apres"));
    w.relay.compact_to(head);
    w.relay
        .set_device_activity(&id, Some(T0 - 42_000), Some(true));
    // The activity is read before the schedule would do it: let the first refresh pass.
    w.clock.advance(1);
    let r = w.e(1).sync().unwrap();
    assert!(r.full_resync);
    assert_eq!(last_seen(w.e(1), &id), Some(T0 - 42_000));
}

#[test]
fn the_doorbell_loop_refreshes_the_activity_while_the_log_is_quiet() {
    let w = World::new(2, 2);
    let id = w.nodes[1].id.clone();
    w.relay.set_device_activity(&id, Some(T0 - 1), Some(true));
    w.e(0).sync().unwrap();
    w.relay
        .set_device_activity(&id, Some(T0 + 400_000), Some(true));
    let mut ticks = Ticks {
        clock: w.clock.clone(),
        left: 2,
        step_ms: ACTIVITY_REFRESH_MS + 1_000,
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    w.e(0).run_wake_source(&mut ticks, &stop);
    assert_eq!(last_seen(w.e(0), &id), Some(T0 + 400_000));
}

#[test]
fn an_event_never_sets_back_the_activity() {
    // A third device joins: its `device_added` snapshot and the known rows do not touch the
    // value the relay reported for the others.
    let w = World::new(3, 2);
    let id = w.nodes[1].id.clone();
    w.relay.set_device_activity(&id, Some(T0 + 5), Some(true));
    w.e(0).sync().unwrap();
    w.join(2);
    w.e(0).sync().unwrap();
    assert_eq!(last_seen(w.e(0), &id), Some(T0 + 5));
    assert_eq!(last_seen(w.e(0), &w.nodes[2].id), None);
}

#[test]
fn values_written_by_earlier_builds_are_dropped_once_then_replaced_by_the_relay() {
    let w = World::new(2, 2);
    let (me, other) = (w.nodes[0].id.clone(), w.nodes[1].id.clone());
    // The state of a database of an earlier build: the pairing time as "last seen", no marker.
    w.e(0)
        .store
        .tx(|db| {
            db.meta_delete(super::engine::LAST_SEEN_FROM_RELAY)?;
            db.set_device_last_seen(&me, Some(T0))?;
            db.set_device_last_seen(&other, Some(T0))
        })
        .unwrap();
    assert_eq!(last_seen(w.e(0), &other), Some(T0));

    // Reopened by this build, with a relay that reports nothing: the false value is gone.
    w.relay.set_device_activity(&other, None, None);
    let e = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert_eq!(last_seen(&e, &other), None);
    assert_eq!(last_seen(&e, &me), None);
    e.sync().unwrap();
    assert_eq!(
        last_seen(&e, &other),
        None,
        "an older relay cannot revive it"
    );

    // With a relay that reports the activity, the real value arrives.
    w.relay
        .set_device_activity(&other, Some(T0 - 86_400_000), Some(true));
    w.clock.advance(ACTIVITY_REFRESH_MS);
    e.sync().unwrap();
    assert_eq!(last_seen(&e, &other), Some(T0 - 86_400_000));

    // The cleanup happens once: what the relay said survives a new opening.
    drop(e);
    let e = w.nodes[0].reopen(&w.relay, w.clock.clone());
    assert_eq!(last_seen(&e, &other), Some(T0 - 86_400_000));
}
