//! `DELETE /v1/space`: the last device of a space deletes the whole space (`api-v1.md` §4.4.1).
//!
//! The deletion itself is the one of `admin reset` (`AppState::delete_space`); these tests are
//! about who may ask for it, what it leaves behind, and what it does under concurrency.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::future::Future;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use common::*;
use futures_util::FutureExt;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
use tomi_relay::install::{self, TokenState};
use tomi_relay::spaces::DestroyStep;

/// The `spaces.deleted-*` directories a deletion would leave in `data_dir`.
fn trash(env: &Env) -> Vec<String> {
    std::fs::read_dir(env.data_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("spaces.deleted-"))
        .collect()
}

fn member_count(devices: &Value) -> usize {
    devices["devices"].as_array().unwrap().len()
}

/// `POST /v1/space/receipt` without any credential.
async fn receipt(env: &Env, deletion_id: &[u8]) -> Resp {
    env.send(
        "POST",
        "/v1/space/receipt",
        &[],
        Some(json!({ "deletion_id": b64(deletion_id) })),
    )
    .await
}

/// `DELETE /v1/space` with a `deletion_id`.
async fn delete_with(env: &Env, d: &Dev, deletion_id: &[u8; 32]) -> Resp {
    env.bearer(
        "DELETE",
        "/v1/space",
        d.tok(),
        Some(json!({ "deletion_id": b64(deletion_id) })),
    )
    .await
}

/// Every row of `space_deletions`: `(receipt_hash, deleted_at)`.
fn receipt_rows(env: &Env) -> Vec<(Vec<u8>, i64)> {
    env.state
        .spaces
        .routing()
        .read_sync(|tx| {
            let mut st =
                tx.prepare("SELECT receipt_hash, deleted_at FROM space_deletions ORDER BY 1")?;
            let v = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .unwrap()
}

/// Whether the socket is still open **and alive**, once the answer of the relay is in. The frames
/// already queued when the answer came (an earlier ping) prove nothing: they are read first and
/// dropped, any close among them failing. Then every frame is read for at least 700 ms (the relay
/// pings every 300 ms in the tests): any close, or an end of the stream, fails (a wrong `4004` may
/// follow the answer by a moment), and at least one ping must arrive, that is **after** the
/// answer. A slow machine only makes that ping late: the reading goes on until it comes, and the
/// longer deadline fails a socket that never gets one.
async fn still_open(ws: &mut Ws) -> bool {
    still_open_all(&mut [ws]).await
}

/// [`still_open`] for several sockets **at the same time**: a socket that nobody reads does not
/// answer the pings of the relay, which drops it after 300 ms more (`ws_pong_timeout`), so they
/// must all be read during the same 700 ms.
async fn still_open_all(sockets: &mut [&mut Ws]) -> bool {
    let watched = sockets.iter_mut().map(|ws| watch(ws));
    futures_util::future::join_all(watched)
        .await
        .into_iter()
        .all(|alive| alive)
}

async fn watch(ws: &mut Ws) -> bool {
    use futures_util::StreamExt;
    use tokio::time::{timeout, Instant};

    // What was already waiting.
    while let Ok(next) = timeout(Duration::ZERO, ws.s.next()).await {
        match next {
            None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return false,
            Some(Ok(_)) => {}
        }
    }
    let at_least = Instant::now() + Duration::from_millis(700);
    let no_ping_at_all = Instant::now() + Duration::from_secs(30);
    let mut pings = 0;
    loop {
        let until = if pings == 0 { no_ping_at_all } else { at_least };
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match timeout(left, ws.s.next()).await {
            Err(_) => break,
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(WsMessage::Close(_)))) => return false,
            Ok(Some(Ok(WsMessage::Ping(_)))) => pings += 1,
            Ok(Some(Ok(_))) => {}
        }
    }
    pings >= 1
}

/// A socket read in the background, from the moment it is opened until [`Reading::finish`]. A test
/// client that nobody reads does not answer the pings of the relay (every 300 ms, dropped 300 ms
/// later in the tests), so a slow machine could make a deletion that must keep the socket open
/// look like one that closed it. The pings are answered here, whatever the test does meanwhile.
struct Reading {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<(Ws, bool)>,
}

fn reading(ws: Ws) -> Reading {
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        use futures_util::StreamExt;
        let mut ws = ws;
        let mut closed = false;
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                next = ws.s.next() => match next {
                    None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => {
                        closed = true;
                        break;
                    }
                    Some(Ok(_)) => {}
                },
            }
        }
        (ws, closed)
    });
    Reading {
        stop: Some(stop),
        task,
    }
}

impl Reading {
    /// Stops reading and gives the socket back. A close (or an end of the stream) seen meanwhile
    /// fails the test: these tests are about a socket that must stay open.
    async fn finish(mut self) -> Ws {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let (ws, closed) = self.task.await.expect("the reading task ended abnormally");
        assert!(
            !closed,
            "the socket was closed while the test was not reading it"
        );
        ws
    }
}

// ===== the deletion ==================================================================

#[tokio::test]
async fn s01_the_last_device_deletes_the_space_and_everything_in_it() {
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(1).await;
    let founder_token = env.install_token.clone();
    let routing = env.state.spaces.routing().clone();

    // One of everything the space holds: a blob, an item, a quota override, a socket, and a
    // pending invitation made for somebody else.
    let blob = env.upload_blob(&devs[0], &[7u8; 300], 300).await;
    env.send_item(&devs[0], &[&blob]).await.1.ok(201);
    let (space_id, space_dir) = (env.space().id, env.space().dir.clone());
    env.state
        .spaces
        .set_quota_override(&space_id, Some(Some(5_000_000)))
        .unwrap();
    let for_somebody_else = install::create(&routing, env.clock.now_ms()).unwrap();
    assert!(space_dir.join("relay.db").is_file());
    assert_eq!(
        install::state(&routing, &founder_token).unwrap(),
        TokenState::Consumed
    );
    let (addr, stop, _server) = serve(&env).await;
    // Read in the background from the opening (unread, the relay cuts it after a missed ping).
    let ws = Ws::open(addr, &devs[0]).await.watch_close();

    let x = [0x5a_u8; 32];
    let ask = {
        let (env, dev) = (env.clone(), clone_dev(&devs[0]));
        tokio::spawn(async move { delete_with(&env, &dev, &x).await })
    };
    // The WebSocket of the space is closed with 4004 (space_reset), and only once the receipt
    // is written: at that moment the receipt already proves the deletion.
    assert_eq!(ws.code(5000).await, Some(4004));
    assert_eq!(
        receipt(&env, &x).await.ok(200),
        json!({ "space_deleted": true })
    );
    let answer = ask.await.unwrap().ok(200);
    assert_eq!(answer, json!({ "space_deleted": true }));
    // The receipt is only `SHA-256(deletion_id)` and the time.
    assert_eq!(
        receipt_rows(&env),
        vec![(tomi_relay::wire::sha256(&x).to_vec(), env.clock.now_ms())]
    );
    // Database, blobs and the directory itself are gone, with nothing renamed left behind.
    assert!(!space_dir.exists());
    assert!(trash(&env).is_empty(), "{:?}", trash(&env));
    // Routing, quota override and the invitation the space had consumed are gone.
    assert!(env.state.spaces.is_empty());
    assert!(env.state.spaces.get(&space_id).is_none());
    assert!(env.state.spaces.by_device(&devs[0].id).is_none());
    assert_eq!(env.state.spaces.quota_override(&space_id), None);
    assert_eq!(
        install::state(&routing, &founder_token).unwrap(),
        TokenState::Unknown
    );
    // The invitation made for somebody else stays, and the relay made none: it cannot show one.
    let pending = install::list(&routing).unwrap();
    assert_eq!(pending.len(), 1, "no new invitation");
    assert!(install::is_pending(&routing, &for_somebody_else).unwrap());

    // The relay is empty: the same answers as after `admin reset` of the only space.
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], false);
    env.get(&devs[0], "/v1/space")
        .await
        .err(409, "space_not_initialized");
    env.challenge(&devs[0].id)
        .await
        .err(409, "space_not_initialized");
    env.delete(&devs[0], "/v1/space")
        .await
        .err(409, "space_not_initialized");

    // The owner starts again: the old token is a wrong token, a new one works, and the keys of
    // the deleted space are free again.
    env.create_space_raw(&devs[0], &founder_token)
        .await
        .err(403, "install_token_invalid");
    env.create_space_raw(&devs[0], &for_somebody_else)
        .await
        .ok(201);
    let _ = stop.send(());
}

#[tokio::test]
async fn s02_a_relay_left_empty_gets_an_invitation_at_its_next_start() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    env.delete(&devs[0], "/v1/space").await.ok(200);
    assert!(install::list(env.state.spaces.routing())
        .unwrap()
        .is_empty());

    // What `tomi-relay` does at every start: a relay with neither space nor pending invitation
    // creates one and logs its token once (§3.1). `admin invite --new` does the same on demand.
    let token = tomi_relay::app::announce_invitation(&env.state)
        .unwrap()
        .expect("an empty relay announces an invitation");
    assert_eq!(
        tomi_relay::app::announce_invitation(&env.state).unwrap(),
        None
    );
    env.create_space_raw(&devs[0], &token).await.ok(201);
    env.login(&mut clone_dev(&devs[0]), 1, 1).await;
}

#[tokio::test]
async fn s03_another_space_on_the_relay_is_not_touched() {
    let a = Env::new().await;
    let b = a.another_space().await;
    let (da, _) = a.space_of(1).await;
    let (db, _) = b.space_of(2).await;
    let blob_b = b.upload_blob(&db[0], &[3u8; 400], 400).await;
    let (item_b, r) = b.send_item(&db[0], &[&blob_b]).await;
    r.ok(201);
    let (dir_a, dir_b) = (a.space().dir.clone(), b.space().dir.clone());
    let (addr, stop, _server) = serve(&a).await;
    let wa = Ws::open(addr, &da[0]).await.watch_close();
    let wb = Ws::open(addr, &db[1]).await.watch_close();
    let seq_b = b.head_seq().await;
    let old_token = da[0].tok().to_owned();

    a.delete(&da[0], "/v1/space").await.ok(200);

    assert_eq!(wa.code(5000).await, Some(4004));
    assert!(wb.still_open(300).await, "B's socket stays open");
    assert!(!dir_a.exists());
    assert!(dir_b.join("relay.db").is_file());
    // The relay still hosts B: the deleted space is an unknown one (401), not "no space" (409),
    // and no invitation is made.
    assert!(!a.state.spaces.is_empty());
    assert!(install::list(a.state.spaces.routing()).unwrap().is_empty());
    a.bearer("GET", "/v1/space", &old_token, None)
        .await
        .err(401, "unauthorized");
    a.delete(&da[0], "/v1/space").await.err(401, "unauthorized");
    // B answers as before: members, item, blob, log; and its own rule still applies.
    assert_eq!(member_count(&b.get(&db[0], "/v1/devices").await.ok(200)), 2);
    b.get(&db[1], &format!("/v1/items/{item_b}")).await.ok(200);
    assert_eq!(b.download(&db[1], &blob_b, None).await.body.len(), 400);
    assert_eq!(b.head_seq().await, seq_b);
    b.delete(&db[0], "/v1/space")
        .await
        .err(409, "not_last_device");
    let _ = stop.send(());
}

// ===== who may not =====================================================================

#[tokio::test]
async fn s04_nobody_but_a_member_alone_can_delete_the_space() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let dir = env.space().dir.clone();

    // No credential, a bad one, one of another scheme, a session that expired.
    env.send("DELETE", "/v1/space", &[], None)
        .await
        .err(401, "unauthorized");
    env.bearer("DELETE", "/v1/space", "tms1.not-a-session", None)
        .await
        .err(401, "unauthorized");
    env.send(
        "DELETE",
        "/v1/space",
        &[("authorization", "Pairing tmp1.nothing")],
        None,
    )
    .await
    .err(401, "unauthorized");
    // The token in the query string is not read (A16).
    let uri = format!("/v1/space?token={}", devs[0].tok());
    env.send("DELETE", &uri, &[], None)
        .await
        .err(401, "unauthorized");
    assert!(dir.join("relay.db").is_file());
    env.get(&devs[0], "/v1/space").await.ok(200);

    env.clock.advance_secs(2 * 3600);
    env.delete(&devs[0], "/v1/space")
        .await
        .err(401, "session_expired");
    assert!(dir.join("relay.db").is_file());
    assert!(!env.state.spaces.is_empty());
}

#[tokio::test]
async fn s05_refused_while_another_member_remains_and_for_a_revoked_device() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let dir = env.space().dir.clone();
    let seq = env.head_seq().await;

    // Two members: both are refused, with a stable code, and nothing changes.
    for d in &devs {
        env.delete(d, "/v1/space").await.err(409, "not_last_device");
    }
    assert_eq!(
        member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
        2
    );
    assert_eq!(env.head_seq().await, seq, "a refusal writes nothing");
    assert!(dir.join("relay.db").is_file());

    // The second device leaves: it is revoked and cannot delete anything, even though the
    // first one is now alone.
    env.delete(&devs[1], "/v1/me").await.ok(200);
    env.delete(&devs[1], "/v1/space").await.err(403, "revoked");
    assert!(dir.join("relay.db").is_file());
    assert_eq!(
        member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
        1
    );

    // And `DELETE /v1/me` keeps refusing the last device, pointing at the new route.
    let refused = env.delete(&devs[0], "/v1/me").await;
    refused.err(409, "last_device");
    assert!(refused.json()["detail"]
        .as_str()
        .unwrap()
        .contains("DELETE /v1/space"));

    // Alone, the first one can.
    env.delete(&devs[0], "/v1/space").await.ok(200);
    assert!(!dir.exists());
    assert!(env.state.spaces.is_empty());
}

#[tokio::test]
async fn s06_a_device_revoked_by_the_administrator_cannot_delete_the_space() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let out = tomi_relay::admin::run(
        &env.state,
        tomi_relay::admin::Cmd::Revoke {
            space: None,
            target: devs[1].id[..12].to_owned(),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    env.delete(&devs[1], "/v1/space").await.err(403, "revoked");
    assert!(env.space().dir.join("relay.db").is_file());
    env.delete(&devs[0], "/v1/space").await.ok(200);
}

// ===== concurrency =====================================================================

/// Two members ask at the same time: neither may delete a space that has two members.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s07_two_members_asking_together_delete_nothing() {
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(2).await;
    let devs: Vec<Arc<Dev>> = devs.into_iter().map(Arc::new).collect();
    for _ in 0..10 {
        let calls: Vec<_> = devs
            .iter()
            .map(|d| {
                let (env, d) = (env.clone(), d.clone());
                tokio::spawn(async move { env.delete(&d, "/v1/space").await })
            })
            .collect();
        for c in calls {
            c.await.unwrap().err(409, "not_last_device");
        }
    }
    assert_eq!(
        member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
        2
    );
    assert!(env.space().dir.join("relay.db").is_file());
}

/// The same device asks twice at the same time (a retry that overtakes the first call): one
/// deletes, the other finds the space gone. Nothing is left half deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s08_the_same_device_asking_twice_deletes_once() {
    for _ in 0..6 {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(1).await;
        let dev = Arc::new(devs.into_iter().next().unwrap());
        let dir = env.space().dir.clone();
        let calls: Vec<_> = (0..2)
            .map(|_| {
                let (env, d) = (env.clone(), dev.clone());
                tokio::spawn(async move { env.delete(&d, "/v1/space").await })
            })
            .collect();
        let mut answers = Vec::new();
        for c in calls {
            answers.push(c.await.unwrap());
        }
        assert_eq!(
            answers.iter().filter(|r| r.status == 200).count(),
            1,
            "{answers:?}"
        );
        // The other call finds the space gone: no space left, or (if it had already looked the
        // space up) the closed database, which asks to retry.
        for r in answers.iter().filter(|r| r.status != 200) {
            assert!(
                matches!(
                    (r.status, r.code().as_str()),
                    (409, "space_not_initialized") | (503, "unavailable")
                ),
                "{r:?}"
            );
        }
        assert!(env.state.spaces.is_empty());
        assert!(!dir.exists());
        assert!(trash(&env).is_empty());
    }
}

/// Waits until `n` requests wait for the registry lock. The lock is fair (first come, first
/// served), so a test that holds it, lets the requests queue one by one and then releases it
/// decides the order in which they run: no timing involved.
async fn queued(env: &Env, n: usize) {
    let started = std::time::Instant::now();
    while env.state.spaces.registry_waiters() < n {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{n} requests never queued on the registry lock"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn spawn_delete(env: &Arc<Env>, dev: &Arc<Dev>) -> JoinHandle<Resp> {
    let (env, dev) = (env.clone(), dev.clone());
    tokio::spawn(async move { env.delete(&dev, "/v1/space").await })
}

fn spawn_register(env: &Arc<Env>, dev: &Arc<Dev>, body: Value) -> JoinHandle<Resp> {
    let (env, dev) = (env.clone(), dev.clone());
    tokio::spawn(async move { env.post(&dev, "/v1/devices", body).await })
}

/// One of two members leaves while the other asks to delete the space. The leaver is never
/// refused (`last_device` needs a sole member, and deleting needs the leaver to be gone), and
/// the space is deleted only if the leaver went first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s09_leaving_and_deleting_together_are_ordered() {
    // The leaver goes first: the deletion, which waits for the registry lock, finds its caller
    // alone and deletes. (The call of `DELETE /v1/me` does not need the registry lock.)
    {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(2).await;
        let devs: Vec<Arc<Dev>> = devs.into_iter().map(Arc::new).collect();
        let dir = env.space().dir.clone();
        let registry = env.state.spaces.lock_registry().await;
        let delete = spawn_delete(&env, &devs[0]);
        queued(&env, 1).await;
        env.delete(&devs[1], "/v1/me").await.ok(200);
        drop(registry);
        let answer = delete.await.unwrap().ok(200);
        assert_eq!(answer, json!({ "space_deleted": true }));
        assert!(env.state.spaces.is_empty());
        assert!(!dir.exists());
    }
    // The deletion is judged first: two members, `409`, and the leaver then leaves.
    {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(2).await;
        let devs: Vec<Arc<Dev>> = devs.into_iter().map(Arc::new).collect();
        let dir = env.space().dir.clone();
        let registry = env.state.spaces.lock_registry().await;
        let delete = spawn_delete(&env, &devs[0]);
        queued(&env, 1).await;
        drop(registry);
        delete.await.unwrap().err(409, "not_last_device");
        assert!(dir.join("relay.db").is_file());
        env.delete(&devs[1], "/v1/me").await.ok(200);
        assert_eq!(
            member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
            1
        );
    }
}

/// The same race without any help: whatever the order, the leaver is not refused and the
/// space is either gone or intact with one member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s09b_leaving_and_deleting_at_once_never_leave_a_half_state() {
    for _ in 0..10 {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(2).await;
        let devs: Vec<Arc<Dev>> = devs.into_iter().map(Arc::new).collect();
        let dir = env.space().dir.clone();
        let (e1, d1) = (env.clone(), devs[1].clone());
        let leave = tokio::spawn(async move { e1.delete(&d1, "/v1/me").await });
        let delete = spawn_delete(&env, &devs[0]);
        let (leave, delete) = (leave.await.unwrap(), delete.await.unwrap());

        leave.ok(200);
        match delete.status {
            200 => {
                assert!(env.state.spaces.is_empty());
                assert!(!dir.exists());
            }
            409 => {
                delete.err(409, "not_last_device");
                assert!(dir.join("relay.db").is_file());
                assert_eq!(
                    member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
                    1
                );
            }
            other => panic!("unexpected answer {other}: {delete:?}"),
        }
    }
}

/// A claimed pairing channel of `inviter` and the body of the `POST /v1/devices` that would
/// register `newcomer` on it.
async fn claimed_channel(env: &Env, inviter: &Dev, newcomer: &Dev) -> Value {
    let channel = env
        .post(inviter, "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let channel_id = channel["channel_id"].as_str().unwrap().to_owned();
    env.send(
        "POST",
        &format!("/v1/pairing/channels/{channel_id}/claim"),
        &[],
        Some(json!({})),
    )
    .await
    .ok(200);
    json!({
        "channel_id": channel_id,
        "key_epoch": 1,
        "name": newcomer.name,
        "platform": "android",
        "certificate": b64(&env.cert(&inviter.keys, &newcomer.keys)),
    })
}

/// The last device registers a new device and deletes the space at the same time; the
/// registration got to the registry lock first. The deletion sees two members and is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10_a_device_joining_first_makes_the_deletion_refused() {
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(1).await;
    let inviter = Arc::new(devs.into_iter().next().unwrap());
    let newcomer = Dev::new("newcomer");
    let dir = env.space().dir.clone();
    let body = claimed_channel(&env, &inviter, &newcomer).await;

    let registry = env.state.spaces.lock_registry().await;
    let join = spawn_register(&env, &inviter, body);
    queued(&env, 1).await;
    let delete = spawn_delete(&env, &inviter);
    queued(&env, 2).await;
    drop(registry);
    let (join, delete) = (join.await.unwrap(), delete.await.unwrap());

    join.ok(201);
    delete.err(409, "not_last_device");
    assert!(env.state.spaces.device_known(&newcomer.id));
    assert!(dir.join("relay.db").is_file());
    assert_eq!(
        member_count(&env.get(&inviter, "/v1/devices").await.ok(200)),
        2
    );
}

/// The same pair, the deletion first: it finds its caller alone and deletes. The registration
/// had passed the gate and waits for the lock; once it has it, the space is closed: `503`, and
/// the new device is neither in the index nor anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10b_the_deletion_first_leaves_the_joining_device_nowhere() {
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(1).await;
    let inviter = Arc::new(devs.into_iter().next().unwrap());
    let newcomer = Dev::new("newcomer");
    let dir = env.space().dir.clone();
    let body = claimed_channel(&env, &inviter, &newcomer).await;

    let registry = env.state.spaces.lock_registry().await;
    let delete = spawn_delete(&env, &inviter);
    queued(&env, 1).await;
    let join = spawn_register(&env, &inviter, body);
    queued(&env, 2).await;
    drop(registry);
    let (delete, join) = (delete.await.unwrap(), join.await.unwrap());

    assert_eq!(delete.ok(200), json!({ "space_deleted": true }));
    join.err(503, "unavailable");
    assert!(!env.state.spaces.device_known(&newcomer.id));
    assert!(env.state.spaces.by_device(&newcomer.id).is_none());
    assert!(env.state.spaces.is_empty());
    assert!(!dir.exists());
    assert!(trash(&env).is_empty());
}

/// The same race without any help, the deletion starting a little later each round: the
/// outcome is always one of the two above, never a deleted space that gained a member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s10c_joining_and_deleting_at_once_never_both_win() {
    for round in 0..16u64 {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(1).await;
        let inviter = Arc::new(devs.into_iter().next().unwrap());
        let newcomer = Dev::new("newcomer");
        let dir = env.space().dir.clone();
        let body = claimed_channel(&env, &inviter, &newcomer).await;

        let (e1, d1) = (env.clone(), inviter.clone());
        // A busy wait: the timers of tokio are too coarse.
        let wait = Duration::from_micros(round * 100);
        let ask = tokio::spawn(async move {
            let start = std::time::Instant::now();
            while start.elapsed() < wait {
                tokio::task::yield_now().await;
            }
            e1.delete(&d1, "/v1/space").await
        });
        let join = spawn_register(&env, &inviter, body);
        let (delete, join) = (ask.await.unwrap(), join.await.unwrap());

        match delete.status {
            200 => {
                // Deleted: the registration failed, with the answer of a space that is gone (or
                // closing), and the new device is nowhere.
                assert!(
                    matches!(
                        (join.status, join.code().as_str()),
                        (503, "unavailable")
                            | (401, "unauthorized")
                            | (409, "space_not_initialized")
                    ),
                    "{join:?}"
                );
                assert!(!env.state.spaces.device_known(&newcomer.id));
                assert!(env.state.spaces.is_empty());
                assert!(!dir.exists());
                assert!(trash(&env).is_empty());
            }
            409 => {
                delete.err(409, "not_last_device");
                assert_eq!(join.status, 201, "{join:?}");
                assert!(env.state.spaces.device_known(&newcomer.id));
                assert!(dir.join("relay.db").is_file());
                assert_eq!(
                    member_count(&env.get(&inviter, "/v1/devices").await.ok(200)),
                    2
                );
            }
            other => panic!("unexpected answer {other}: {delete:?}"),
        }
    }
}

// ===== a disk that refuses ===============================================================
//
// `Spaces::inject_destroy_fault` makes a step of the destruction fail, so that the paths a real
// refusal would take (a file held open on Windows, a read-only directory) are tested the same
// way on every platform. After each, the relay is started again on the same data directory.

/// What a space holds, and what the tests below look for afterwards.
struct Rich {
    dev: Dev,
    blob: String,
    item: String,
    space_id: tomi_relay::spaces::SpaceId,
    dir: std::path::PathBuf,
    founder_token: String,
}

/// A space of one device with a blob, an item and a quota override; its founding invitation is
/// consumed.
async fn rich_space(env: &Env) -> Rich {
    let (devs, _) = env.space_of(1).await;
    let dev = devs.into_iter().next().unwrap();
    let blob = env.upload_blob(&dev, &[7u8; 300], 300).await;
    let (item, r) = env.send_item(&dev, &[&blob]).await;
    r.ok(201);
    let space = env.space();
    env.state
        .spaces
        .set_quota_override(&space.id, Some(Some(5_000_000)))
        .unwrap();
    Rich {
        dev,
        blob,
        item,
        space_id: space.id,
        dir: space.dir.clone(),
        founder_token: env.install_token.clone(),
    }
}

/// The relay stopped and started again on the same data directory.
async fn restarted(env: Env) -> Env {
    env.state.spaces.close_all();
    env.restart().await
}

fn quota_rows(env: &Env) -> i64 {
    env.state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(tx.query_row("SELECT count(*) FROM space_quotas", [], |r| {
                r.get::<_, i64>(0)
            })?)
        })
        .unwrap()
}

fn invitation(env: &Env, token: &str) -> TokenState {
    install::state(env.state.spaces.routing(), token).unwrap()
}

/// `500 internal`, and nothing in the body that names the space or a place on the disk.
fn assert_opaque_500(env: &Env, answer: &Resp) {
    answer.err(500, "internal");
    let body = String::from_utf8_lossy(&answer.body).to_lowercase();
    let folder = env.data_dir().file_name().unwrap().to_string_lossy();
    for secret in [
        env.space_id.to_lowercase(),
        folder.to_lowercase(),
        "relay.db".to_owned(),
        "spaces".to_owned(),
    ] {
        assert!(!body.contains(&secret), "{secret:?} in {body}");
    }
}

/// Nothing could be deleted: not the folder, not even `relay.db`. The answer is `500`, and the
/// space is **registered again** under the lock still held: a retry before any restart is
/// neither `409 space_not_initialized` (the relay has no other space) nor `401` (it has one),
/// both of which a client takes as proof that its space is gone. Once the disk obeys, the same
/// call deletes.
#[tokio::test]
async fn s11_a_deletion_the_disk_refuses_leaves_the_space_in_place() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let (addr, stop, _server) = serve(&env).await;
    let reads = reading(Ws::open(addr, &r.dev).await);
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename, DestroyStep::RemoveDatabase]);

    let x = [0x11_u8; 32];
    let answer = delete_with(&env, &r.dev, &x).await;
    assert_opaque_500(&env, &answer);
    // Nothing moved, `routing.db` was not touched, there is no receipt, and the socket of the
    // space was not told `4004` (A189).
    assert!(r.dir.join("relay.db").is_file());
    assert!(trash(&env).is_empty());
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    let mut ws = reads.finish().await;
    assert!(still_open(&mut ws).await, "the socket stays open");

    // Without a restart: the space is there, with the same session, and nothing is missing.
    assert!(!env.state.spaces.is_empty());
    assert!(env.state.spaces.get(&r.space_id).is_some());
    assert!(env.state.spaces.device_known(&r.dev.id), "ids stay taken");
    let devices = env.get(&r.dev, "/v1/devices").await.ok(200);
    assert_eq!(member_count(&devices), 1);
    env.get(&r.dev, &format!("/v1/items/{}", r.item))
        .await
        .ok(200);
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(
        env.state.spaces.quota_override(&r.space_id),
        Some(Some(5_000_000))
    );

    // A retry while the disk still refuses is answered like the first call, never as "gone".
    let again = env.delete(&r.dev, "/v1/space").await;
    assert_opaque_500(&env, &again);
    assert!(!env.state.spaces.is_empty());

    // The same, with other spaces on the relay.
    let other = env.another_space().await;
    let (others, _) = other.space_of(2).await;
    let again = env.delete(&r.dev, "/v1/space").await;
    assert_opaque_500(&env, &again);
    other.get(&others[0], "/v1/space").await.ok(200);

    // The defect is gone: the same call (same `deletion_id`) deletes, and nothing else. (The
    // socket is not looked at any more: a test client that does not read for this long misses
    // the pings and is dropped by the relay; s22 checks the `4004` of a success.)
    env.state.spaces.inject_destroy_fault(&[]);
    drop(ws);
    assert_eq!(
        delete_with(&env, &r.dev, &x).await.ok(200),
        json!({ "space_deleted": true })
    );
    receipt(&env, &x).await.ok(200);
    assert!(!r.dir.exists());
    assert!(trash(&env).is_empty());
    assert_eq!(quota_rows(&env), 0);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Unknown);
    other.get(&others[0], "/v1/space").await.ok(200);
    let _ = stop.send(());
}

/// The same refusal on a relay that hosts nothing else, then a restart: the space comes back
/// whole (member, item, blob, quota, consumed invitation), as before the call.
#[tokio::test]
async fn s11b_a_refused_deletion_is_whole_after_a_restart_too() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename, DestroyStep::RemoveDatabase]);
    assert_opaque_500(&env, &env.delete(&r.dev, "/v1/space").await);

    let env = restarted(env).await;
    assert!(!env.state.spaces.is_empty());
    let devices = env.get(&r.dev, "/v1/devices").await.ok(200);
    assert_eq!(member_count(&devices), 1, "no phantom member, none missing");
    env.get(&r.dev, &format!("/v1/items/{}", r.item))
        .await
        .ok(200);
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(
        env.state.spaces.quota_override(&r.space_id),
        Some(Some(5_000_000))
    );
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);

    env.delete(&r.dev, "/v1/space").await.ok(200);
    assert!(!r.dir.exists());
    assert!(trash(&env).is_empty());
}

/// Nothing could be deleted and the database cannot be opened again either. The space stays
/// out of reach until the next start, but it is not "gone": its token gets `503 unavailable`
/// (retry), the relay still counts as hosting a space (no `409 space_not_initialized`, with
/// or without another space), and the identifiers of the space stay taken. The next start has
/// it back whole.
#[tokio::test]
async fn s11c_a_space_that_cannot_be_reopened_is_unavailable_not_gone() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let faults = [
        DestroyStep::Rename,
        DestroyStep::RemoveDatabase,
        DestroyStep::Reopen,
    ];
    env.state.spaces.inject_destroy_fault(&faults);
    let (addr, stop, _server) = serve(&env).await;
    let reads = reading(Ws::open(addr, &r.dev).await);

    // Nothing was deleted but the space could not be registered again: it stays pending, and
    // the answer says to try again (`503`, §4.4.1 case (a)), never `200`, `401` nor `409`.
    let first = env.delete(&r.dev, "/v1/space").await;
    first.err(503, "unavailable");
    assert_eq!(first.header("retry-after").as_deref(), Some("5"));
    // The socket opened before stays open (nothing was deleted); a new one is told 1013.
    let mut open = reads.finish().await;
    assert!(still_open(&mut open).await);
    let mut late = Ws::connect(addr).await;
    late.send_json(json!({ "type": "auth", "token": r.dev.tok() }))
        .await;
    assert_eq!(late.recv_close(2000).await, Some(1013));
    let _ = stop.send(());
    assert!(r.dir.join("relay.db").is_file());
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);

    // Not registered, not empty, not gone.
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert!(!env.state.spaces.is_empty());
    assert!(env.state.spaces.device_known(&r.dev.id));
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);
    for _ in 0..2 {
        env.delete(&r.dev, "/v1/space")
            .await
            .err(503, "unavailable");
    }
    env.get(&r.dev, "/v1/space").await.err(503, "unavailable");
    // A token nobody knows is an unknown token (the relay is not empty).
    env.bearer("GET", "/v1/space", "tms1.not-a-session", None)
        .await
        .err(401, "unauthorized");
    // Clearing the fault does not bring it back: only a start does.
    env.state.spaces.inject_destroy_fault(&[]);
    env.delete(&r.dev, "/v1/space")
        .await
        .err(503, "unavailable");

    // Another space on the relay is served as usual.
    let other = env.another_space().await;
    let (others, _) = other.space_of(2).await;
    other.get(&others[0], "/v1/space").await.ok(200);
    env.get(&r.dev, "/v1/space").await.err(503, "unavailable");

    let env = restarted(env).await;
    let devices = env.get(&r.dev, "/v1/devices").await.ok(200);
    assert_eq!(member_count(&devices), 1);
    env.get(&r.dev, &format!("/v1/items/{}", r.item))
        .await
        .ok(200);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    env.delete(&r.dev, "/v1/space").await.ok(200);
}

/// The rename is refused but the files can be removed: `relay.db` goes first, then the rest;
/// the deletion succeeds and nothing comes back.
#[tokio::test]
async fn s12_a_refused_rename_falls_back_to_removing_in_place() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename]);

    env.delete(&r.dev, "/v1/space").await.ok(200);
    assert!(!r.dir.exists());
    assert!(trash(&env).is_empty());
    assert_eq!(quota_rows(&env), 0);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Unknown);

    let env = restarted(env).await;
    assert!(env.state.spaces.is_empty());
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert!(!r.dir.exists());
}

/// The rename is refused, `relay.db` is removed in place (the positive act), and the removal of
/// the rest of the folder is refused. The deletion is proved (`200`, receipt): what is left on
/// the disk is cleaned at best and never fails the call. The debris left in `spaces/<id>/` has
/// no `relay.db` and is not loaded as a space, half empty or otherwise, at the next start.
#[tokio::test]
async fn s13_an_in_place_removal_whose_debris_resists_is_still_a_deletion() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename, DestroyStep::RemoveDirectory]);

    let x = [0x13_u8; 32];
    assert_eq!(
        delete_with(&env, &r.dev, &x).await.ok(200),
        json!({ "space_deleted": true })
    );
    receipt(&env, &x).await.ok(200);
    assert!(!r.dir.join("relay.db").exists(), "the database went first");
    assert!(r.dir.exists(), "the blobs are still there");
    assert!(env.state.spaces.is_empty());
    assert_eq!(quota_rows(&env), 0);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Unknown);

    let env = restarted(env).await;
    assert!(env.state.spaces.is_empty(), "no half empty space");
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert!(!r.dir.join("relay.db").exists());
    env.get(&r.dev, "/v1/space")
        .await
        .err(409, "space_not_initialized");
    receipt(&env, &x).await.ok(200);
}

/// The folder is renamed (the space is gone for good) but the transaction of `routing.db`
/// (receipt, quota, invitation) fails (A187, second part). No proof can be given: `500`, the
/// receipt answers `404`, the tokens of the space are then unknown ones (`409
/// space_not_initialized` here, the relay is empty), and the quota and the consumed invitation
/// stay in `routing.db`, also after a restart (A191: a start never forgets them).
#[tokio::test]
async fn s14_a_receipt_that_cannot_be_written_gives_no_200() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let (addr, stop, _server) = serve(&env).await;
    let ws = Ws::open(addr, &r.dev).await.watch_close();
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::ForgetRouting]);

    let x = [0x14_u8; 32];
    assert_opaque_500(&env, &delete_with(&env, &r.dev, &x).await);
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    assert!(!r.dir.exists());
    assert!(trash(&env).is_empty());
    assert!(env.state.spaces.is_empty());
    // The space is gone: its identifiers are free, its socket closed with `4004`.
    assert_eq!(ws.code(5000).await, Some(4004));
    assert!(!env.state.spaces.device_known(&r.dev.id));
    env.get(&r.dev, "/v1/space")
        .await
        .err(409, "space_not_initialized");
    delete_with(&env, &r.dev, &x)
        .await
        .err(409, "space_not_initialized");
    // The rows are still there.
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    let _ = stop.send(());

    let env = restarted(env).await;
    assert!(env.state.spaces.is_empty());
    assert_eq!(quota_rows(&env), 1, "a start does not forget a quota");
    assert_eq!(
        invitation(&env, &r.founder_token),
        TokenState::Consumed,
        "a start does not forget an invitation"
    );
    assert_eq!(env.state.spaces.quota_override(&r.space_id), None);
    receipt(&env, &x).await.err(404, "not_found");
}

/// The folder is renamed and the space is deleted, but the renamed folder cannot be removed. The
/// call succeeds (`200`), a `spaces.deleted-*` stays in `data_dir`, and the next start removes
/// it without bringing the space back.
#[tokio::test]
async fn s15_a_renamed_folder_that_resists_is_removed_at_the_next_start() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::RemoveDirectory]);

    assert_eq!(
        env.delete(&r.dev, "/v1/space").await.ok(200),
        json!({ "space_deleted": true })
    );
    assert!(!r.dir.exists(), "gone from spaces/");
    let left = trash(&env);
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(env.state.spaces.is_empty());
    assert_eq!(quota_rows(&env), 0);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Unknown);

    let env = restarted(env).await;
    assert!(trash(&env).is_empty(), "{:?}", trash(&env));
    assert!(env.state.spaces.is_empty(), "the space does not come back");
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert!(!r.dir.exists());
    env.get(&r.dev, "/v1/space")
        .await
        .err(409, "space_not_initialized");
}

/// What a deletion must have left once it is over, whoever waited for it: nothing of the space,
/// and the proof for the client whose call was dropped.
async fn assert_deleted_for_good(env: &Env, r: &Rich, deletion_id: &[u8; 32]) {
    assert!(!r.dir.exists());
    assert!(trash(env).is_empty(), "{:?}", trash(env));
    assert!(env.state.spaces.is_empty());
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert_eq!(quota_rows(env), 0);
    assert_eq!(invitation(env, &r.founder_token), TokenState::Unknown);
    receipt(env, deletion_id).await.ok(200);
}

/// A client that hangs up does not interrupt the deletion, and does not release the registry
/// lock under it: the lock is carried by the task that deletes. The request future is dropped
/// while the deletion is held by its pause, as the server does when the connection closes;
/// whoever then takes the lock finds the deletion complete.
///
/// No timing is involved: the deletion cannot end before `resume` gets its permit, however slow
/// or fast the machine is. The only deadline fails a deletion that never ends.
#[tokio::test]
async fn s16_a_request_that_is_dropped_does_not_interrupt_the_deletion() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let space = env.space();
    let x = [9u8; 32];
    let pause = env.state.spaces.pause_next_deletion();
    let registry = env.state.spaces.lock_registry().await;

    let mut call = Box::pin(env.state.delete_space(&space, registry, Some(x)));
    tokio::select! {
        outcome = &mut call => panic!("the deletion ended during its pause: {outcome:?}"),
        reached = pause.reached.acquire() => reached.unwrap().forget(),
    }
    drop(call);
    // Whatever the drop may have set off runs now.
    tokio::task::yield_now().await;

    // The deletion is in flight: the space is pending, nothing is deleted, no receipt, and the
    // lock stays with the task.
    assert!(
        env.state.spaces.lock_registry().now_or_never().is_none(),
        "dropping the request released the registry lock under the deletion"
    );
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert!(r.dir.join("relay.db").is_file());
    receipt(&env, &x).await.err(404, "not_found");

    // Nobody waits for the deletion any more; it goes on all the same. The lock comes back only
    // when the task is over.
    pause.resume.add_permits(1);
    let _later = tokio::time::timeout(Duration::from_secs(60), env.state.spaces.lock_registry())
        .await
        .expect("the registry lock was never released");
    assert_deleted_for_good(&env, &r, &x).await;
}

/// The same, with the request dropped right after its first poll. On this runtime of one thread
/// the task exists and has the lock, but has not run yet: nothing has started when the only one
/// waiting for it goes away.
#[tokio::test]
async fn s16b_a_request_dropped_at_its_first_poll_still_deletes() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let space = env.space();
    let x = [0x16_u8; 32];
    let registry = env.state.spaces.lock_registry().await;

    let mut call = Box::pin(env.state.delete_space(&space, registry, Some(x)));
    let first = std::future::poll_fn(|cx| Poll::Ready(call.as_mut().poll(cx))).await;
    assert!(first.is_pending(), "the task cannot have run yet");
    drop(call);
    assert!(
        env.state.spaces.lock_registry().now_or_never().is_none(),
        "the task does not carry the registry lock"
    );
    assert!(r.dir.join("relay.db").is_file());

    let _later = tokio::time::timeout(Duration::from_secs(60), env.state.spaces.lock_registry())
        .await
        .expect("the registry lock was never released");
    assert_deleted_for_good(&env, &r, &x).await;
}

// ===== fail-closed deletion and receipt (§4.4.1, §4.4.2, A184 to A194) =====================

/// A space of one device that a deletion holds paused once it is pending (the registry lock
/// held, nothing deleted yet). The returned task is the `DELETE /v1/space` call.
async fn paused_deletion(
    env: &Arc<Env>,
    dev: &Dev,
    deletion_id: [u8; 32],
) -> (tomi_relay::spaces::DeletionPause, JoinHandle<Resp>) {
    let pause = env.state.spaces.pause_next_deletion();
    let ask = {
        let (env, dev) = (env.clone(), clone_dev(dev));
        tokio::spawn(async move { delete_with(&env, &dev, &deletion_id).await })
    };
    pause.reached.acquire().await.unwrap().forget();
    (pause, ask)
}

/// A184. The answer of a successful deletion is lost: the retry finds the space gone, which
/// proves nothing (`409 space_not_initialized`, or `401` on a relay that hosts other spaces),
/// and the receipt gives the proof, as often as asked. Any other identifier gets the same
/// `404`, a malformed one `422`.
#[tokio::test]
async fn s17_a_receipt_proves_a_deletion_whose_answer_was_lost() {
    let env = Env::new().await;
    let other = env.another_space().await;
    let (others, _) = other.space_of(1).await;
    let (devs, _) = env.space_of(1).await;
    let x = [0x17_u8; 32];

    // The deletion happens; its answer never reaches the client.
    let _lost = delete_with(&env, &devs[0], &x).await;

    // The retry: the space is gone, but `401` is no proof (another space is hosted).
    delete_with(&env, &devs[0], &x)
        .await
        .err(401, "unauthorized");
    env.challenge(&devs[0].id).await.ok(200);
    env.login_raw(&devs[0], 1, 1).await.err(401, "unauthorized");
    // The receipt, without any credential, twice.
    for _ in 0..2 {
        let proof = receipt(&env, &x).await.ok(200);
        assert_eq!(proof, json!({ "space_deleted": true }));
        assert_eq!(proof["space_deleted"], Value::Bool(true));
    }
    // A session header changes nothing (the route reads none).
    env.bearer(
        "POST",
        "/v1/space/receipt",
        others[0].tok(),
        Some(json!({ "deletion_id": b64(&x) })),
    )
    .await
    .ok(200);
    // Another identifier: `404`, the same answer as for one never seen.
    let unknown = receipt(&env, &[0x18_u8; 32]).await;
    unknown.err(404, "not_found");
    let never = receipt(&env, &[0u8; 32]).await;
    never.err(404, "not_found");
    assert_eq!(unknown.json()["detail"], never.json()["detail"]);
    assert!(!String::from_utf8_lossy(&unknown.body).contains(&env.space_id));
    // Malformed: 31 bytes, 33 bytes, not base64url, absent.
    receipt(&env, &[1u8; 31])
        .await
        .err(422, "validation_failed");
    receipt(&env, &[1u8; 33])
        .await
        .err(422, "validation_failed");
    env.send(
        "POST",
        "/v1/space/receipt",
        &[],
        Some(json!({ "deletion_id": "not base64!" })),
    )
    .await
    .err(422, "validation_failed");
    env.send("POST", "/v1/space/receipt", &[], Some(json!({})))
        .await
        .err(422, "validation_failed");
    // The other space is untouched.
    other.get(&others[0], "/v1/space").await.ok(200);
}

/// A185. Without a body, or with `{}`, the deletion succeeds and no receipt is written. A body
/// that is not one (malformed `deletion_id`, wrong `Content-Type`) deletes nothing.
#[tokio::test]
async fn s18_a_deletion_without_body_writes_no_receipt_and_a_bad_body_deletes_nothing() {
    // A bad body first: nothing is deleted, the space answers as before.
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let dir = env.space().dir.clone();
    for bad in [
        json!({ "deletion_id": "abc" }),
        json!({ "deletion_id": b64(&[1u8; 31]) }),
        json!({ "deletion_id": 32 }),
    ] {
        env.bearer("DELETE", "/v1/space", devs[0].tok(), Some(bad))
            .await
            .err(422, "validation_failed");
    }
    let raw = Request::builder()
        .method("DELETE")
        .uri("/v1/space")
        .header("authorization", format!("Bearer {}", devs[0].tok()))
        .header("content-type", "text/plain")
        .body(Body::from(format!(
            "{{\"deletion_id\":\"{}\"}}",
            b64(&[2u8; 32])
        )))
        .unwrap();
    env.call(raw).await.err(415, "unsupported_media_type");
    // Both bodies are capped at 1 KiB, far below the generic limit of a JSON body.
    let padded = json!({ "deletion_id": b64(&[3u8; 32]), "pad": "x".repeat(2048) });
    env.bearer("DELETE", "/v1/space", devs[0].tok(), Some(padded.clone()))
        .await
        .err(413, "payload_too_large");
    env.send("POST", "/v1/space/receipt", &[], Some(padded))
        .await
        .err(413, "payload_too_large");
    assert!(dir.join("relay.db").is_file());
    assert!(!env.state.spaces.is_empty());
    env.get(&devs[0], "/v1/space").await.ok(200);
    // `{}`: deleted, no receipt.
    assert_eq!(
        env.bearer("DELETE", "/v1/space", devs[0].tok(), Some(json!({})))
            .await
            .ok(200),
        json!({ "space_deleted": true })
    );
    assert!(receipt_rows(&env).is_empty());

    // No body at all (a client of before the receipt): deleted, no receipt.
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    assert_eq!(
        env.delete(&devs[0], "/v1/space").await.ok(200),
        json!({ "space_deleted": true })
    );
    assert!(receipt_rows(&env).is_empty());
}

/// A186. The space looks absent when it is about to be deleted (volume not mounted, folder
/// moved by hand): nothing is deleted, the answer is never `200` (here `503`: the space stays
/// pending, there is nothing to open again), no receipt, the relay is not empty, and once the
/// disk is back a restart has the space whole.
#[tokio::test]
async fn s19_a_missing_space_folder_is_never_reported_deleted() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Presence]);
    let x = [0x19_u8; 32];
    for _ in 0..2 {
        let answer = delete_with(&env, &r.dev, &x).await;
        answer.err(503, "unavailable");
        assert_eq!(answer.header("retry-after").as_deref(), Some("5"));
    }
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);
    assert!(r.dir.join("relay.db").is_file());
    assert_eq!(quota_rows(&env), 1);

    let env = restarted(env).await;
    assert_eq!(
        member_count(&env.get(&r.dev, "/v1/devices").await.ok(200)),
        1
    );
    env.get(&r.dev, &format!("/v1/items/{}", r.item))
        .await
        .ok(200);
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(
        env.state.spaces.quota_override(&r.space_id),
        Some(Some(5_000_000))
    );
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
}

/// A186 on a real disk (Unix: a folder can be moved while its database is open). The folder
/// is moved out of `data_dir`, then `relay.db` is replaced by a directory, then removed: never
/// `200`, never a receipt, and the moved-back folder is whole after a restart.
#[cfg(unix)]
#[tokio::test]
async fn s19b_a_space_folder_moved_away_is_never_reported_deleted() {
    // Moved out of data_dir.
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked");
    std::fs::rename(&r.dir, &parked).unwrap();
    let x = [0x1b_u8; 32];
    delete_with(&env, &r.dev, &x).await.err(503, "unavailable");
    receipt(&env, &x).await.err(404, "not_found");
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);
    assert!(parked.join("relay.db").is_file());
    std::fs::rename(&parked, &r.dir).unwrap();
    let env = restarted(env).await;
    assert_eq!(
        member_count(&env.get(&r.dev, "/v1/devices").await.ok(200)),
        1
    );
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);

    // `relay.db` replaced by a directory.
    let db = r.dir.join("relay.db");
    let aside = r.dir.join("relay.db.aside");
    std::fs::rename(&db, &aside).unwrap();
    std::fs::create_dir(&db).unwrap();
    delete_with(&env, &r.dev, &x).await.err(503, "unavailable");
    receipt(&env, &x).await.err(404, "not_found");
    std::fs::remove_dir(&db).unwrap();
    std::fs::rename(&aside, &db).unwrap();
    let env = restarted(env).await;
    env.get(&r.dev, &format!("/v1/items/{}", r.item))
        .await
        .ok(200);

    // `relay.db` removed just before the call.
    std::fs::remove_file(&db).unwrap();
    delete_with(&env, &r.dev, &x).await.err(503, "unavailable");
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    assert!(!env.state.spaces.is_empty());
}

/// Recursively copies a folder (the new folder has other inodes than the original).
#[cfg(unix)]
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

/// A186, identity (§4.4.1 step 2). What a lazily unmounted volume (`umount -l`) leaves under the
/// mount point of a relay that runs: an **old copy** of `spaces/<id>/`, found by its path and
/// perfectly renamable, while the real space (and `routing.db`) is elsewhere and comes back with
/// the next mount. The path is right but the folder is not the one the space was loaded from:
/// never `200`, never a receipt, nothing renamed, the space is pending (`503`), and the real
/// folder is whole after a restart. Same for a `relay.db` replaced by a copy.
#[cfg(unix)]
#[tokio::test]
async fn s19c_a_copy_in_place_of_the_space_folder_is_never_reported_deleted() {
    // The whole folder.
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked");
    std::fs::rename(&r.dir, &parked).unwrap();
    copy_dir(&parked, &r.dir);
    let x = [0x1c_u8; 32];
    for _ in 0..2 {
        let answer = delete_with(&env, &r.dev, &x).await;
        answer.err(503, "unavailable");
        assert_eq!(answer.header("retry-after").as_deref(), Some("5"));
    }
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    assert!(trash(&env).is_empty(), "the copy was not renamed");
    assert!(r.dir.join("relay.db").is_file(), "the copy was not touched");
    assert!(parked.join("relay.db").is_file());
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);
    env.get(&r.dev, "/v1/space").await.err(503, "unavailable");

    // The mount comes back: the real folder is there again, whole.
    std::fs::remove_dir_all(&r.dir).unwrap();
    std::fs::rename(&parked, &r.dir).unwrap();
    let env = restarted(env).await;
    assert_eq!(
        member_count(&env.get(&r.dev, "/v1/devices").await.ok(200)),
        1
    );
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);

    // Only the database.
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let db = r.dir.join("relay.db");
    let copy = r.dir.join("relay.db.copy");
    std::fs::copy(&db, &copy).unwrap();
    std::fs::rename(&copy, &db).unwrap();
    delete_with(&env, &r.dev, &x).await.err(503, "unavailable");
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    assert!(trash(&env).is_empty());
    assert!(db.is_file());
    assert!(!env.state.spaces.is_empty());
}

/// Starts the relay on the data directory of `env` (without the lock, which `env` holds), both as
/// the server and as a read-only `admin` command, and returns the reasons of the refusals. A
/// start that succeeds fails the test: no state, so no invitation can be announced.
#[cfg(unix)]
async fn refused_starts(env: &Env) -> Vec<String> {
    let options = |read_only: bool| Options {
        clock: Arc::new(ManualClock::new(T0)),
        disk: Arc::new(FixedDisk::new(1 << 40)),
        timing: fast_timing(),
        push: PushChoice::Disabled,
        background: false,
        hold_lock: false,
        read_only,
        cleanup: !read_only,
        fcm_http: None,
        fcm_base: None,
    };
    let mut why = Vec::new();
    for read_only in [false, true] {
        match open(Config::for_tests(env.data_dir()), options(read_only)).await {
            Ok(_) => panic!("a relay started with a space it cannot load (read_only {read_only})"),
            Err(e) => {
                // The scripts of `infra/` do not go back to the previous binary on this code.
                assert_eq!(
                    tomi_relay::spaces::start_exit_code(&e),
                    tomi_relay::spaces::EXIT_DATA_REFUSED,
                    "exit code of {e:#}"
                );
                why.push(format!("{e:#}"));
            }
        }
    }
    match tomi_relay::spaces::Spaces::open(env.data_dir()) {
        Ok(_) => panic!("Spaces::open accepted a space it cannot load"),
        Err(e) => {
            assert_eq!(
                tomi_relay::spaces::start_exit_code(&e),
                tomi_relay::spaces::EXIT_DATA_REFUSED,
                "exit code of {e:#}"
            );
            why.push(format!("{e:#}"));
        }
    }
    why
}

/// §10.1. A space whose folder is a symbolic link makes the start FAIL (it is not skipped): the
/// presence check of a deletion does not follow links, so such a space could never be deleted,
/// and skipping it would answer `401` / `409 space_not_initialized` to a whole space (and, for
/// the only space, create an installation token). The message says to put the real folder back,
/// and names no path. Nothing is touched.
#[cfg(unix)]
#[tokio::test]
async fn s19d_a_space_folder_that_is_a_symbolic_link_refuses_the_start() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked");
    std::fs::rename(&r.dir, &parked).unwrap();
    std::os::unix::fs::symlink(&parked, &r.dir).unwrap();

    for why in refused_starts(&env).await {
        assert!(why.contains("symbolic link"), "{why}");
        assert!(why.contains("Replace the link by the real folder"), "{why}");
        assert!(why.contains(&env.space_id.to_lowercase()[..8]), "{why}");
        assert!(
            !why.contains(&env.space_id.to_lowercase()),
            "the whole id in {why}"
        );
        assert!(
            !why.contains(&*away.path().to_string_lossy()),
            "a path in {why}"
        );
    }
    assert!(std::fs::symlink_metadata(&r.dir)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(parked.join("relay.db").is_file());

    // The real folder put back: the relay starts and the space is there.
    std::fs::remove_file(&r.dir).unwrap();
    std::fs::rename(&parked, &r.dir).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
}

/// Same for the database: `relay.db` as a symbolic link refuses the start.
#[cfg(unix)]
#[tokio::test]
async fn s19e_a_database_that_is_a_symbolic_link_refuses_the_start() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked.db");
    let db = r.dir.join("relay.db");
    std::fs::rename(&db, &parked).unwrap();
    std::os::unix::fs::symlink(&parked, &db).unwrap();

    for why in refused_starts(&env).await {
        assert!(why.contains("symbolic link"), "{why}");
        assert!(
            why.contains("Replace the link by the real relay.db"),
            "{why}"
        );
        assert!(
            !why.contains(&env.space_id.to_lowercase()),
            "the whole id in {why}"
        );
        assert!(
            !why.contains(&*away.path().to_string_lossy()),
            "a path in {why}"
        );
    }
    assert!(std::fs::symlink_metadata(&db)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(parked.is_file());

    std::fs::remove_file(&db).unwrap();
    std::fs::rename(&parked, &db).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
}

/// §10.1. `spaces/` is a link: allowed when its target is there (the identity of the folder is
/// what counts), REFUSED when it is not (volume not mounted): `is_dir()` false must not read as
/// "no space", or every device would get `409 space_not_initialized` and, with the guard
/// satisfied by the link itself, an installation token would be created. No path in the message.
#[cfg(unix)]
#[tokio::test]
async fn s19f_a_spaces_link_to_a_missing_target_refuses_the_start() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let spaces = env.data_dir().join("spaces");
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked");
    std::fs::rename(&spaces, &parked).unwrap();

    // A link whose target exists: allowed, the space is found through it.
    std::os::unix::fs::symlink(&parked, &spaces).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
    env.state.spaces.close_all();
    std::fs::remove_file(&spaces).unwrap();

    // The same link, the target gone (volume not mounted): the start is refused.
    let gone = away.path().join("not-mounted");
    std::os::unix::fs::symlink(&gone, &spaces).unwrap();
    for why in refused_starts(&env).await {
        assert!(why.contains("spaces folder"), "{why}");
        assert!(why.contains("refusing to start"), "{why}");
        assert!(
            !why.contains(&*away.path().to_string_lossy()),
            "a path in {why}"
        );
    }
    assert!(std::fs::symlink_metadata(&spaces)
        .unwrap()
        .file_type()
        .is_symlink());

    // Mounted again: the relay starts and the space is there.
    std::fs::remove_file(&spaces).unwrap();
    std::fs::rename(&parked, &spaces).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
}

/// §10.1. A space folder the relay is not allowed to look into (`EACCES`, as after a restore made
/// as root without `chown`) is not "a space without a database": the start is refused instead of
/// ignoring the space (which would answer `401` / `409` to its devices). Skipped as root, for
/// whom the permission bits mean nothing.
#[cfg(unix)]
#[tokio::test]
async fn s19g_an_unreadable_space_folder_refuses_the_start() {
    use std::os::unix::fs::PermissionsExt;

    /// Puts the permissions back whatever happens, so that the temporary folder can be removed.
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let _restore = Restore(r.dir.clone());
    std::fs::set_permissions(&r.dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::symlink_metadata(r.dir.join("relay.db")).is_ok() {
        eprintln!("s19g skipped: the permission bits are not enforced (root)");
        return;
    }

    for why in refused_starts(&env).await {
        assert!(why.contains("refusing to start"), "{why}");
        assert!(why.contains(&env.space_id.to_lowercase()[..8]), "{why}");
        assert!(
            !why.contains(&env.space_id.to_lowercase()),
            "the whole id in {why}"
        );
        assert!(
            !why.contains(&*env.data_dir().to_string_lossy()),
            "a path in {why}"
        );
    }

    // Allowed again: the relay starts and the space is there.
    std::fs::set_permissions(&r.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
}

/// A database the relay cannot open (`relay.db` unreadable) refuses the start with the data exit
/// code, and the message names neither the path nor the whole `space_id` (rusqlite would put the
/// path in it): the result code of SQLite and 8 characters of the id. Skipped as root.
#[cfg(unix)]
#[tokio::test]
async fn s19h_an_unreadable_database_refuses_the_start_without_its_path() {
    use std::os::unix::fs::PermissionsExt;

    /// Puts the permissions back whatever happens, so that the temporary folder can be removed.
    struct Restore(std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600));
        }
    }

    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let db = r.dir.join("relay.db");
    let _restore = Restore(db.clone());
    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&db).is_ok() {
        eprintln!("s19h skipped: the permission bits are not enforced (root)");
        return;
    }

    for why in refused_starts(&env).await {
        assert!(why.contains("refusing to start"), "{why}");
        assert!(why.contains("SQLite error"), "{why}");
        assert!(why.contains(&env.space_id.to_lowercase()[..8]), "{why}");
        assert!(
            !why.contains(&env.space_id.to_lowercase()),
            "the whole id in {why}"
        );
        assert!(
            !why.contains(&*env.data_dir().to_string_lossy()),
            "a path in {why}"
        );
    }

    std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o600)).unwrap();
    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_some());
}

/// The data exit code is for the data only: a relay that cannot start for another reason (here
/// `relay.lock` held by another relay) exits `1`, which the scripts of `infra/` treat as "go back
/// to the previous binary if it is not healthy".
#[tokio::test]
async fn s19i_a_start_refused_for_another_reason_is_not_the_data_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let options = || Options {
        clock: Arc::new(ManualClock::new(T0)),
        disk: Arc::new(FixedDisk::new(1 << 40)),
        timing: fast_timing(),
        push: PushChoice::Disabled,
        background: false,
        hold_lock: true,
        read_only: false,
        cleanup: true,
        fcm_http: None,
        fcm_base: None,
    };
    let first = open(Config::for_tests(dir.path()), options())
        .await
        .unwrap();
    let e = match open(Config::for_tests(dir.path()), options()).await {
        Ok(_) => panic!("two relays on one data directory"),
        Err(e) => e,
    };
    assert!(format!("{e:#}").contains("another tomi-relay"), "{e:#}");
    assert_eq!(tomi_relay::spaces::start_exit_code(&e), 1);
    assert_eq!(tomi_relay::spaces::EXIT_DATA_REFUSED, 78);
    first.spaces.close_all();
}

/// A `relay.lock` that cannot be opened (here a folder in its place, on every platform) is the
/// data's fault: `78`, not `1` (R1). Only a lock really held by another relay is `1` (s19i).
#[tokio::test]
async fn s19j_a_relay_lock_that_cannot_be_opened_is_the_data_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("relay.lock")).unwrap();
    let e = match open(
        Config::for_tests(dir.path()),
        Options {
            clock: Arc::new(ManualClock::new(T0)),
            disk: Arc::new(FixedDisk::new(1 << 40)),
            timing: fast_timing(),
            push: PushChoice::Disabled,
            background: false,
            hold_lock: true,
            read_only: false,
            cleanup: true,
            fcm_http: None,
            fcm_base: None,
        },
    )
    .await
    {
        Ok(_) => panic!("a relay started with a folder as relay.lock"),
        Err(e) => e,
    };
    assert!(format!("{e:#}").contains("relay.lock"), "{e:#}");
    assert_eq!(
        tomi_relay::spaces::start_exit_code(&e),
        tomi_relay::spaces::EXIT_DATA_REFUSED
    );
}

/// A187, first part. The folder is renamed (the positive act) but the directories cannot be
/// synced: `500`, no receipt, no `4004`, and no retry is answered `200` during this start (the
/// space stays pending, `503`). The quota and the invitation are not forgotten.
#[tokio::test]
async fn s20_an_fsync_failure_gives_no_200_and_no_4004() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let (addr, stop, _server) = serve(&env).await;
    let reads = reading(Ws::open(addr, &r.dev).await);
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::SyncDir]);
    let x = [0x20_u8; 32];

    assert_opaque_500(&env, &delete_with(&env, &r.dev, &x).await);
    receipt(&env, &x).await.err(404, "not_found");
    assert!(receipt_rows(&env).is_empty());
    let mut ws = reads.finish().await;
    assert!(still_open(&mut ws).await, "no 4004 without a proof");
    // Pending until the next start, whatever the fault does now.
    env.state.spaces.inject_destroy_fault(&[]);
    for _ in 0..2 {
        delete_with(&env, &r.dev, &x).await.err(503, "unavailable");
    }
    env.get(&r.dev, "/v1/space").await.err(503, "unavailable");
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    let _ = stop.send(());

    // Here the rename did reach the disk: the next start removes the renamed folder (§10.1),
    // and still writes no receipt nor forgets the quota and the invitation.
    let env = restarted(env).await;
    assert!(trash(&env).is_empty());
    assert!(env.state.spaces.is_empty());
    receipt(&env, &x).await.err(404, "not_found");
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
}

/// A188. While a deletion runs (paused right after the unregistering, registry lock held), and
/// then while the space stays pending, every identifier of the space gets `503`: challenge,
/// session, `Bearer` routes, pairing routes; a WebSocket is closed with `1013`; the relay is not
/// empty; `POST /v1/space` with its `space_id` gets `503` and touches nothing. Never `401`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s21_a_pending_space_answers_503_everywhere() {
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(1).await;
    let dev = &devs[0];
    let dir = env.space().dir.clone();
    let (addr, stop, _server) = serve(&env).await;
    // Before the deletion: a fresh challenge, a claimed channel (with its pairing token) and an
    // open one.
    let ch = env.challenge(&dev.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let claimed = env
        .post(dev, "/v1/pairing/channels", json!({}))
        .await
        .ok(201)["channel_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let pairing = env
        .send(
            "POST",
            &format!("/v1/pairing/channels/{claimed}/claim"),
            &[],
            Some(json!({})),
        )
        .await
        .ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let open_channel = env
        .post(dev, "/v1/pairing/channels", json!({}))
        .await
        .ok(201)["channel_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // The positive act will be refused at the presence check: the space stays pending after.
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Presence]);

    let x = [0x21_u8; 32];
    let reads = reading(Ws::open(addr, dev).await);
    let (pause, ask) = paused_deletion(&env, dev, x).await;
    // The sockets already open stay open while the space is pending (read all along: a test
    // client that does not read misses the pings and is dropped).
    let mut early = reads.finish().await;
    assert!(still_open(&mut early).await);
    let check = |phase: &'static str| {
        let env = env.clone();
        let dev = clone_dev(dev);
        let (challenge, claimed, pairing, open_channel) = (
            challenge.clone(),
            claimed.clone(),
            pairing.clone(),
            open_channel.clone(),
        );
        async move {
            let r = env.challenge(&dev.id).await;
            r.err(503, "unavailable");
            assert_eq!(r.header("retry-after").as_deref(), Some("5"), "{phase}");
            env.send(
                "POST",
                "/v1/auth/session",
                &[],
                Some(env.session_body(&dev, &challenge, 1, 1, "windows")),
            )
            .await
            .err(503, "unavailable");
            for path in ["/v1/space", "/v1/devices", "/v1/events?after=0"] {
                let r = env.get(&dev, path).await;
                assert_eq!(
                    (r.status, r.code()),
                    (503_u16, "unavailable".to_owned()),
                    "{phase} {path}"
                );
            }
            env.delete(&dev, "/v1/me").await.err(503, "unavailable");
            env.send(
                "GET",
                &format!("/v1/pairing/channels/{claimed}/messages?after=0"),
                &[("authorization", format!("Pairing {pairing}").as_str())],
                None,
            )
            .await
            .err(503, "unavailable");
            env.send(
                "POST",
                &format!("/v1/pairing/channels/{open_channel}/claim"),
                &[],
                Some(json!({})),
            )
            .await
            .err(503, "unavailable");
            // A token nobody knows is still an unknown one.
            env.bearer("GET", "/v1/space", "tms1.not-a-session", None)
                .await
                .err(401, "unauthorized");
            let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
            assert_eq!(hello["space_initialized"], true, "{phase}");
        }
    };

    // 1. During the deletion.
    check("during").await;
    let mut late = Ws::connect(addr).await;
    late.send_json(json!({ "type": "auth", "token": dev.tok() }))
        .await;
    assert_eq!(late.recv_close(2000).await, Some(1013));
    drop(early);
    receipt(&env, &x).await.err(404, "not_found");

    // 2. The deletion ends in an unknown state: the space stays pending.
    pause.resume.add_permits(1);
    ask.await.unwrap().err(503, "unavailable");
    check("after").await;
    let mut later = Ws::connect(addr).await;
    later
        .send_json(json!({ "type": "auth", "token": dev.tok() }))
        .await;
    assert_eq!(later.recv_close(2000).await, Some(1013));
    // `POST /v1/space` with this `space_id` and a valid invitation: `503`, nothing touched, the
    // invitation still pending.
    let token = install::create(env.state.spaces.routing(), env.clock.now_ms()).unwrap();
    let founder = Dev::new("someone else");
    env.create_space_raw(&founder, &token)
        .await
        .err(503, "unavailable");
    assert!(install::is_pending(env.state.spaces.routing(), &token).unwrap());
    assert!(dir.join("relay.db").is_file());
    let _ = stop.send(());
}

/// A189. A deletion that deletes nothing leaves every WebSocket of the space open; a deletion
/// that succeeds closes them with `4004`, after the receipt.
#[tokio::test]
async fn s22_a_failed_deletion_keeps_the_websockets_open() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let (addr, stop, _server) = serve(&env).await;
    let reads_a = reading(Ws::open(addr, &r.dev).await);
    let reads_b = reading(Ws::open(addr, &r.dev).await);
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename, DestroyStep::RemoveDatabase]);
    let x = [0x22_u8; 32];
    assert_opaque_500(&env, &delete_with(&env, &r.dev, &x).await);
    let (mut a, mut b) = (reads_a.finish().await, reads_b.finish().await);
    assert!(still_open_all(&mut [&mut a, &mut b]).await);
    // Read again in the background until the close: unread, they would be cut by the relay
    // (missed ping) before the `4004` this test waits for.
    let (a, b) = (a.watch_close(), b.watch_close());

    env.state.spaces.inject_destroy_fault(&[]);
    delete_with(&env, &r.dev, &x).await.ok(200);
    assert_eq!(a.code(5000).await, Some(4004));
    assert_eq!(b.code(5000).await, Some(4004));
    receipt(&env, &x).await.ok(200);
    let _ = stop.send(());
}

/// A190. The same device leaves twice at once (a retry that overtakes the first call): one
/// `200` with an integer `seq`, the other `403 revoked`, never `401` nor `409 last_device`.
/// Then every call of that device, and a new session, are `403 revoked`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s23_leaving_twice_at_once_gives_403_not_401() {
    for _ in 0..12 {
        let env = Arc::new(Env::new().await);
        let (devs, _) = env.space_of(2).await;
        let leaver = Arc::new(clone_dev(&devs[1]));
        let calls: Vec<_> = (0..2)
            .map(|_| {
                let (env, d) = (env.clone(), leaver.clone());
                tokio::spawn(async move { env.delete(&d, "/v1/me").await })
            })
            .collect();
        let mut answers = Vec::new();
        for c in calls {
            answers.push(c.await.unwrap());
        }
        let ok: Vec<&Resp> = answers.iter().filter(|r| r.status == 200).collect();
        assert_eq!(ok.len(), 1, "{answers:?}");
        assert!(ok[0].json()["seq"].is_i64(), "{:?}", ok[0]);
        for r in answers.iter().filter(|r| r.status != 200) {
            r.err(403, "revoked");
        }
        env.delete(&leaver, "/v1/me").await.err(403, "revoked");
        env.login_raw(&leaver, 1, 1).await.err(403, "revoked");
        assert_eq!(
            member_count(&env.get(&devs[0], "/v1/devices").await.ok(200)),
            1
        );
    }
}

/// A191. The relay starts while the folder of a space is missing (a volume not mounted, a
/// folder moved by hand): it starts, and keeps the invitation that space consumed and its
/// quota override in `routing.db`. Once the folder is back, the space is whole.
#[tokio::test]
async fn s24_a_start_without_a_space_folder_keeps_its_invitation_and_quota() {
    let env = Env::new().await;
    let r = rich_space(&env).await;
    env.state.spaces.close_all();
    let away = tempfile::tempdir().unwrap();
    let parked = away.path().join("parked");
    std::fs::rename(&r.dir, &parked).unwrap();

    let env = env.restart().await;
    assert!(env.state.spaces.get(&r.space_id).is_none());
    assert_eq!(quota_rows(&env), 1);
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
    // Unknown here (the start did not find it): no proof either way.
    assert!(receipt_rows(&env).is_empty());
    env.state.spaces.close_all();
    std::fs::rename(&parked, &r.dir).unwrap();

    let env = env.restart().await;
    assert_eq!(
        member_count(&env.get(&r.dev, "/v1/devices").await.ok(200)),
        1
    );
    assert_eq!(env.download(&r.dev, &r.blob, None).await.body.len(), 300);
    assert_eq!(
        env.state.spaces.quota_override(&r.space_id),
        Some(Some(5_000_000))
    );
    assert_eq!(invitation(&env, &r.founder_token), TokenState::Consumed);
}

/// A192. `admin status` counts a pending space apart and gives the `space_initialized` of
/// `GET /v1/hello`, during a deletion and after one left the space pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s25_admin_status_and_hello_agree_on_a_pending_space() {
    use tomi_relay::admin::{run, Cmd};
    let env = Arc::new(Env::new().await);
    let (devs, _) = env.space_of(1).await;
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Presence]);
    let status = |env: Arc<Env>| async move {
        let out = run(
            &env.state,
            Cmd::Status {
                json: true,
                space: None,
            },
        )
        .await;
        assert_eq!(out.exit, 0, "{}", out.stderr);
        serde_json::from_str::<Value>(&out.stdout).unwrap()
    };
    let before = status(env.clone()).await;
    assert_eq!(
        (&before["spaces"], &before["pending_spaces"]),
        (&json!(1), &json!(0))
    );

    let agree = |phase: &'static str| {
        let env = env.clone();
        async move {
            let v = status(env.clone()).await;
            let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
            assert_eq!(v["spaces"], 0, "{phase}");
            assert_eq!(v["pending_spaces"], 1, "{phase}");
            assert_eq!(
                v["space_initialized"], hello["space_initialized"],
                "{phase}"
            );
            assert_eq!(v["space_initialized"], true, "{phase}");
        }
    };
    let (pause, ask) = paused_deletion(&env, &devs[0], [0x25; 32]).await;
    agree("during").await;
    pause.resume.add_permits(1);
    ask.await.unwrap().err(503, "unavailable");
    agree("after").await;
    let text = run(
        &env.state,
        Cmd::Status {
            json: false,
            space: None,
        },
    )
    .await;
    assert!(text.stdout.contains("pending spaces: 1"), "{}", text.stdout);
    assert!(
        text.stdout.contains("space initialized: true"),
        "{}",
        text.stdout
    );
}

/// A193. `require_existing_data_dir = true`: a `data_dir` that is empty or missing is refused
/// (an error, so a non-zero exit of `tomi-relay`) and nothing is created in it; with a
/// `routing.db` the relay starts.
#[tokio::test]
async fn s26_an_empty_data_dir_is_refused_when_required() {
    let options = || Options {
        clock: Arc::new(ManualClock::new(T0)),
        disk: Arc::new(FixedDisk::new(1 << 40)),
        timing: fast_timing(),
        push: PushChoice::Disabled,
        background: false,
        hold_lock: true,
        read_only: false,
        cleanup: true,
        fcm_http: None,
        fcm_base: None,
    };
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("not-mounted");
    for data_dir in [dir.path().to_path_buf(), missing.clone()] {
        let mut cfg = Config::for_tests(&data_dir);
        cfg.require_existing_data_dir = true;
        let why = match open(cfg, options()).await {
            Ok(_) => panic!("a relay started on an empty data_dir"),
            Err(e) => {
                assert_eq!(
                    tomi_relay::spaces::start_exit_code(&e),
                    tomi_relay::spaces::EXIT_DATA_REFUSED
                );
                format!("{e:#}")
            }
        };
        assert!(why.contains("data_dir looks empty"), "{why}");
        assert!(why.contains("require_existing_data_dir = false"), "{why}");
    }
    assert!(!missing.exists(), "nothing created");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "nothing created"
    );

    // A first installation (the key is false), then the key on: the relay starts.
    let first = open(Config::for_tests(dir.path()), options())
        .await
        .unwrap();
    assert!(dir.path().join("routing.db").is_file());
    first.spaces.close_all();
    drop(first);
    let mut cfg = Config::for_tests(dir.path());
    cfg.require_existing_data_dir = true;
    open(cfg, options()).await.unwrap();
}

/// A194. A folder `spaces/<id>/` whose `relay.db` has a `space` row, but that the start did not
/// load (simulated: copied in while the relay runs). `POST /v1/space` with this `space_id`:
/// `409 space_exists`, the folder is not touched and the invitation is given back.
#[tokio::test]
async fn s27_creating_over_an_unloaded_space_folder_touches_nothing() {
    // The space to copy, made by another relay.
    let source = Env::new().await;
    let _ = source.space_of(1).await;
    let src_dir = source.space().dir.clone();
    source.state.spaces.close_all();
    let bytes = std::fs::read(src_dir.join("relay.db")).unwrap();

    let env = Env::new().await;
    let target = tomi_relay::spaces::space_dir(env.data_dir(), &uuid_bytes(&source.space_id));
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("relay.db"), &bytes).unwrap();
    // The journal too, if the close left one: the copy must hold the `space` row.
    let wal = src_dir.join("relay.db-wal");
    if wal.is_file() {
        std::fs::copy(&wal, target.join("relay.db-wal")).unwrap();
    }

    let as_source = env.for_space(&source.space_id);
    let token = env.install_token.clone();
    as_source
        .create_space_raw(&Dev::new("newcomer"), &token)
        .await
        .err(409, "space_exists");
    assert_eq!(std::fs::read(target.join("relay.db")).unwrap(), bytes);
    assert!(install::is_pending(env.state.spaces.routing(), &token).unwrap());
    assert!(env.state.spaces.is_empty());

    // A folder without a `space` row is the debris of an interrupted creation: cleared.
    std::fs::remove_dir_all(&target).unwrap();
    std::fs::create_dir_all(target.join("blobs")).unwrap();
    as_source
        .create_space_raw(&Dev::new("founder"), &token)
        .await
        .ok(201);
}

/// A184, end. The receipt is kept `deletion_receipt_ttl_secs` (90 days by default), then
/// answers `404`, and the maintenance deletes it.
#[tokio::test]
async fn s28_the_receipt_expires() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let x = [0x28_u8; 32];
    delete_with(&env, &devs[0], &x).await.ok(200);
    let ttl = env.state.cfg.deletion_receipt_ttl_secs;
    assert_eq!(ttl, 90 * 86_400);

    env.clock.advance_secs(i64::try_from(ttl).unwrap() - 1);
    receipt(&env, &x).await.ok(200);
    tomi_relay::maintenance::run_once(&env.state).await;
    assert_eq!(receipt_rows(&env).len(), 1, "not expired yet");

    env.clock.advance_secs(2);
    receipt(&env, &x).await.err(404, "not_found");
    let report = tomi_relay::maintenance::run_once(&env.state).await;
    assert_eq!(report.receipts_expired, 1);
    assert!(receipt_rows(&env).is_empty());
    receipt(&env, &x).await.err(404, "not_found");
}

/// The receipt route never says `space_not_initialized`: not on a relay that never had a space,
/// not on one that a deletion left empty.
#[tokio::test]
async fn s29_the_receipt_route_never_says_space_not_initialized() {
    let env = Env::new().await;
    assert!(env.state.spaces.is_empty());
    receipt(&env, &[7u8; 32]).await.err(404, "not_found");
    receipt(&env, &[7u8; 3]).await.err(422, "validation_failed");

    let (devs, _) = env.space_of(1).await;
    let x = [0x29_u8; 32];
    delete_with(&env, &devs[0], &x).await.ok(200);
    assert!(env.state.spaces.is_empty());
    env.get(&devs[0], "/v1/space")
        .await
        .err(409, "space_not_initialized");
    receipt(&env, &x).await.ok(200);
    receipt(&env, &[7u8; 32]).await.err(404, "not_found");
}

/// `admin reset` writes no receipt (§4.4.2): a reset by hand gives the devices no proof.
#[tokio::test]
async fn s30_admin_reset_writes_no_receipt() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let out = tomi_relay::admin::run(
        &env.state,
        tomi_relay::admin::Cmd::Reset {
            space: None,
            confirm: Some(env.space_id[..8].to_owned()),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(receipt_rows(&env).is_empty());
    env.get(&devs[0], "/v1/space")
        .await
        .err(409, "space_not_initialized");
}

/// `admin reset` follows the same fail-closed rules: a deletion that deletes nothing, or that
/// cannot be made durable, is an error (exit 1) and the space is not gone.
#[tokio::test]
async fn s31_admin_reset_fails_closed() {
    use tomi_relay::admin::{run, Cmd};
    let env = Env::new().await;
    let r = rich_space(&env).await;
    let reset = || Cmd::Reset {
        space: None,
        confirm: Some(env.space_id[..8].to_owned()),
    };
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::Rename, DestroyStep::RemoveDatabase]);
    let out = run(&env.state, reset()).await;
    assert_eq!(out.exit, 1, "{}", out.stdout);
    assert!(!out.stderr.contains(&env.space_id), "{}", out.stderr);
    env.get(&r.dev, "/v1/space").await.ok(200);

    // The routing transaction fails: the space is gone, the command says so, and stays 0.
    env.state
        .spaces
        .inject_destroy_fault(&[DestroyStep::ForgetRouting]);
    let out = run(&env.state, reset()).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(
        out.stderr.contains("routing.db could not be cleaned"),
        "{}",
        out.stderr
    );
    assert!(!r.dir.exists());
    assert_eq!(quota_rows(&env), 1);
}
