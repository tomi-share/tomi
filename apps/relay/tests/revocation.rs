//! `api-v1.md` §11.3 (revocation), §11.4 (versioning) and §11.11 (rotation).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::json;

fn seqs_of(events: &[serde_json::Value], kind: &str) -> Vec<i64> {
    events
        .iter()
        .filter(|e| e["type"] == kind)
        .map(|e| e["seq"].as_i64().unwrap())
        .collect()
}

// ===== 11.3 =======================================================================

#[tokio::test]
async fn a20_a_rotation_revokes_and_cuts_the_websocket_first() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    let (addr, stop, _server) = serve(&env).await;
    let mut ws_b = Ws::open(addr, b).await;

    let (statement, _) = env.rotation_for(&ts, a, &[b]);
    let r = env.post_rotation(a, &statement).await.ok(201);
    assert_eq!(r["key_epoch"], 2);
    assert_eq!(r["revoked_device_ids"], json!([b.id]));

    // (2) The socket of B was closed with 4003 before A got its answer: the frame is
    // already waiting.
    assert_eq!(ws_b.recv_close(300).await, Some(4003));

    // (3) B is refused everywhere, the authentication included.
    let revoked = env.get(b, "/v1/space").await;
    revoked.err(403, "revoked");
    assert!(revoked.json()["revoked_at"].is_string());
    env.login_raw(b, 1, 1).await.err(403, "revoked");

    // (4) The state of the space.
    let space = env.get(a, "/v1/space").await.ok(200);
    assert_eq!(space["key_epoch"], 2);
    assert_eq!(space["rotation_required"], false);

    // (5) `rotation` then `device_revoked`.
    let events = env.all_events(a).await;
    let rot_seq = seqs_of(&events, "rotation")[0];
    assert_eq!(rot_seq, r["seq"].as_i64().unwrap());
    let revoked_ev = events
        .iter()
        .find(|e| e["type"] == "device_revoked")
        .unwrap();
    assert!(revoked_ev["seq"].as_i64().unwrap() > rot_seq);
    assert_eq!(revoked_ev["data"]["device_id"], b.id);
    assert_eq!(revoked_ev["data"]["via"], "rotation");
    assert_eq!(revoked_ev["data"]["revoked_by"], a.id);
    assert_eq!(revoked_ev["data"]["rotation_seq"], rot_seq);
    // The members that stay can still work.
    env.get(c, "/v1/space").await.ok(200);
    let _ = stop.send(());
}

#[tokio::test]
async fn a21_a_revoked_key_can_never_be_registered_again() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[&devs[1]]);
    env.post_rotation(&devs[0], &statement).await.ok(201);
    let epoch = env.key_epoch().await;
    // A new certificate for the same keys.
    env.pair_raw(&devs[0], &devs[1], epoch)
        .await
        .err(409, "device_exists");
}

#[tokio::test]
async fn a22_leaving_by_oneself() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    env.delete(&devs[0], "/v1/me").await.err(409, "last_device");

    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let r = env.delete(&devs[1], "/v1/me").await.ok(200);
    assert_eq!(r["rotation_required"], true);
    // The session used becomes invalid at once.
    env.get(&devs[1], "/v1/space").await.err(403, "revoked");
    let space = env.get(&devs[0], "/v1/space").await.ok(200);
    assert_eq!(space["rotation_required"], true);
    let ev = env.events_of_type(&devs[0], "device_revoked").await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["data"]["via"], "self");
    assert_eq!(ev[0]["data"]["revoked_by"], devs[1].id);
    assert_eq!(ev[0]["seq"], r["seq"]);
    assert!(ev[0]["data"].get("rotation_seq").is_none());
}

#[tokio::test]
async fn a23_replaying_a_rotation_changes_nothing() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[&devs[2]]);
    let first = env.post_rotation(&devs[0], &statement).await.ok(201);
    let before = env.all_events(&devs[0]).await.len();
    let again = env.post_rotation(&devs[0], &statement).await.ok(200);
    assert_eq!(again["seq"], first["seq"]);
    assert_eq!(again["key_epoch"], 2);
    assert_eq!(again["revoked_device_ids"], first["revoked_device_ids"]);
    assert_eq!(env.all_events(&devs[0]).await.len(), before, "no new event");
}

#[tokio::test]
async fn a24_the_administrator_revokes_while_the_relay_runs() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    let (addr, stop, _server) = serve(&env).await;
    let mut ws_b = Ws::open(addr, b).await;

    let out = tomi_relay::admin::run(
        &env.state,
        tomi_relay::admin::Cmd::Revoke {
            target: b.id[..10].to_owned(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(ws_b.recv_close(500).await, Some(4003));

    let ev = env.events_of_type(a, "device_revoked").await;
    assert_eq!(ev[0]["data"]["via"], "admin");
    assert!(ev[0]["data"]["revoked_by"].is_null());
    assert_eq!(
        env.get(a, "/v1/space").await.ok(200)["rotation_required"],
        true
    );
    env.get(b, "/v1/space").await.err(403, "revoked");

    // A later rotation that lists B in `revoked` is accepted, and clears the flag.
    let (statement, _) = env.rotation_for(&ts, a, &[b]);
    let r = env.post_rotation(a, &statement).await.ok(201);
    assert_eq!(r["revoked_device_ids"], json!([]), "B was already revoked");
    assert_eq!(
        env.get(c, "/v1/space").await.ok(200)["rotation_required"],
        false
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn a25_a_revoked_device_loses_its_blobs() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (a, b) = (&devs[0], &devs[1]);
    let blob = env.upload_blob(a, &[9u8; 1000], 1000).await;
    let (_, sent) = env.send_item(a, &[&blob]).await;
    sent.ok(201);
    assert_eq!(env.download(b, &blob, None).await.status, 200);
    let (statement, _) = env.rotation_for(&ts, a, &[b]);
    env.post_rotation(a, &statement).await.ok(201);
    env.download(b, &blob, None).await.err(403, "revoked");
}

#[tokio::test]
async fn a26_a_rotation_cannot_revoke_the_caller_or_an_unknown_key() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let a = &devs[0];
    let members: Vec<Vec<u8>> = devs.iter().map(Dev::pk).collect();
    // The caller's own key.
    let own = forge_rotation(&env, a, 2, &[a.pk()], &members[1..]);
    let r = env.post_rotation(a, &own).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/rotation");
    // A key nobody knows.
    let stranger = Dev::new("stranger");
    let unknown = forge_rotation(&env, a, 2, &[stranger.pk()], &members);
    env.post_rotation(a, &unknown)
        .await
        .err(422, "validation_failed");
    // Nothing happened.
    assert_eq!(env.key_epoch().await, 1);
}

// ===== 11.4 =======================================================================

async fn session_as(env: &Env, d: &mut Dev, fmin: u32, fmax: u32, platform: &str) -> Resp {
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let r = env
        .send(
            "POST",
            "/v1/auth/session",
            &[],
            Some(env.session_body(d, &challenge, fmin, fmax, platform)),
        )
        .await;
    if r.status == 200 {
        d.token = Some(r.json()["session_token"].as_str().unwrap().to_owned());
    }
    r
}

#[tokio::test]
async fn a30_a_device_that_is_too_old_must_update() {
    let env = Env::new().await;
    let mut a = Dev::new("A");
    env.create_space(&mut a).await;
    // A announces format_max = 2 (re-login).
    session_as(&env, &mut a, 1, 2, "windows").await.ok(200);
    let mut newcomer = Dev::new("Pixel");
    env.pair_raw(&a, &newcomer, 1).await.ok(201);
    let r = session_as(&env, &mut newcomer, 1, 1, "android").await;
    r.err(409, "update_required");
    let body = r.json();
    assert_eq!(body["effective_format"], 2);
    assert_eq!(
        body["update_url"],
        "https://relay.test/updates/android/latest.json"
    );
    // Still not active, and no session was created.
    let devices = env.get(&a, "/v1/devices").await.ok(200);
    let n = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["device_id"] == newcomer.id)
        .unwrap();
    assert_eq!(n["active"], false);
    assert!(n.get("last_seen_at").is_none());
    assert!(n.get("format_max").is_none());
}

#[tokio::test]
async fn a31_the_lowest_format_of_the_active_devices_wins() {
    let env = Env::new().await;
    let mut a = Dev::new("A");
    env.create_space(&mut a).await; // format 1..1
    let mut b = Dev::new("B");
    env.pair_raw(&a, &b, 1).await.ok(201);
    let v = session_as(&env, &mut b, 1, 2, "android").await.ok(200);
    assert_eq!(v["space"]["effective_format"], 1);
    assert_eq!(v["space"]["effective_format_limited_by"], json!([a.id]));
}

#[tokio::test]
async fn a32_an_inactive_device_no_longer_holds_the_version() {
    let env = Env::new().await;
    let mut a = Dev::new("A");
    env.create_space(&mut a).await; // A: 1..1, seen at T0
    let mut b = Dev::new("B");
    env.pair_raw(&a, &b, 1).await.ok(201);
    env.clock.advance_secs(31 * 86_400);
    let v = session_as(&env, &mut b, 1, 2, "android").await.ok(200);
    assert_eq!(v["space"]["effective_format"], 2);
    // A comes back after 31 days of silence with an older format: it must update.
    let r = session_as(&env, &mut a, 1, 1, "windows").await;
    r.err(409, "update_required");
    assert_eq!(r.json()["effective_format"], 2);
}

#[tokio::test]
async fn a33_a_device_that_cannot_write_the_effective_format_is_refused() {
    let env = Env::new().await;
    let mut a = Dev::new("A");
    env.create_space(&mut a).await; // effective format 1
    let mut n = Dev::new("N");
    env.pair_raw(&a, &n, 1).await.ok(201);
    let r = session_as(&env, &mut n, 2, 2, "android").await;
    r.err(409, "format_unsupported");
    let body = r.json();
    assert_eq!(body["effective_format"], 1);
    assert_eq!(body["limited_by"], json!([a.id]));
}

#[tokio::test]
async fn a34_versions_of_the_api() {
    let env = Env::new().await;
    let r = env
        .send(
            "GET",
            "/v0/hello",
            &[("user-agent", "tomi-android/0.1.0")],
            None,
        )
        .await;
    r.err(410, "update_required");
    assert_eq!(
        r.json()["update_url"],
        "https://relay.test/updates/android/latest.json"
    );
    let r = env.send("GET", "/v0/hello", &[], None).await;
    assert_eq!(r.json()["update_url"], "https://relay.test/updates/");
    env.send("GET", "/v9/hello", &[], None)
        .await
        .err(404, "not_found");
    env.send("GET", "/v1/hello", &[], None).await.ok(200);
}

// ===== 11.11 ======================================================================

#[tokio::test]
async fn a110_a_rotation_that_revokes_one_member() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[&devs[2]]);
    let r = env.post_rotation(&devs[0], &statement).await.ok(201);
    assert_eq!(r["revoked_device_ids"], json!([devs[2].id]));
    assert_eq!(
        env.get(&devs[0], "/v1/space").await.ok(200)["rotation_required"],
        false
    );
    let events = env.all_events(&devs[1]).await;
    let rot = seqs_of(&events, "rotation");
    let rev = seqs_of(&events, "device_revoked");
    assert_eq!((rot.len(), rev.len()), (1, 1));
    assert!(rot[0] < rev[0]);
    assert_eq!(
        events
            .iter()
            .find(|e| e["type"] == "device_revoked")
            .unwrap()["data"]["device_id"],
        devs[2].id
    );
}

#[tokio::test]
async fn a111_the_recipients_must_be_exactly_the_remaining_members() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    let mut expected = vec![a.id.clone(), b.id.clone()];
    expected.sort();
    // Revokes C but seals for C as well.
    let with_c = forge_rotation(&env, a, 2, &[c.pk()], &[a.pk(), b.pk(), c.pk()]);
    let r = env.post_rotation(a, &with_c).await;
    r.err(409, "device_set_mismatch");
    assert_eq!(r.json()["expected_device_ids"], json!(expected));
    // Revokes C but forgets B.
    let without_b = forge_rotation(&env, a, 2, &[c.pk()], &[a.pk()]);
    let r = env.post_rotation(a, &without_b).await;
    r.err(409, "device_set_mismatch");
    assert_eq!(r.json()["expected_device_ids"], json!(expected));
    assert_eq!(env.key_epoch().await, 1, "nothing was applied");
}

#[tokio::test]
async fn a112_two_concurrent_rotations_one_wins() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (s1, _) = env.rotation_for(&ts, &devs[0], &[]);
    let (s2, _) = env.rotation_for(&ts, &devs[0], &[]);
    assert_ne!(s1, s2, "two different keys");
    let (r1, r2) = tokio::join!(
        env.post_rotation(&devs[0], &s1),
        env.post_rotation(&devs[0], &s2)
    );
    let mut codes = [(r1.status, r1.code()), (r2.status, r2.code())];
    codes.sort();
    assert_eq!(codes[0].0, 201);
    assert_eq!(codes[1], (409, "epoch_conflict".to_owned()));
    let loser = if r1.status == 409 { &r1 } else { &r2 };
    assert_eq!(loser.json()["key_epoch"], 2);
}

#[tokio::test]
async fn a113_the_rotations_are_listed_for_the_members_only() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (statement, _) = env.rotation_for(&ts, &devs[0], &[&devs[2]]);
    env.post_rotation(&devs[0], &statement).await.ok(201);
    let keys = env.get(&devs[1], "/v1/keys").await.ok(200);
    assert_eq!(keys["key_epoch"], 2);
    let list = keys["rotations"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["key_epoch"], 2);
    assert_eq!(unb64(list[0]["rotation"].as_str().unwrap()), statement);
    env.get(&devs[2], "/v1/keys").await.err(403, "revoked");
}

// ===== a revocation between the access control and the transaction ===========================
//
// The gate reads the session and the device before the body of a request is read, and a body may
// take minutes. A device revoked (or logged out) while its request is in flight must not
// complete it, and must be told exactly what a device revoked before the request is told.

/// The body of an error without what differs from one request to the next.
fn same_answer(r: &Resp) -> (u16, serde_json::Value) {
    let mut v = r.json();
    v.as_object_mut().unwrap().remove("request_id");
    (r.status, v)
}

#[tokio::test]
async fn a_rotation_whose_device_is_revoked_meanwhile_changes_nothing() {
    let env = Env::new().await;
    let (devs, ts) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    // B prepares a rotation that revokes C, and sends it slowly.
    let (statement, _) = env.rotation_for(&ts, b, &[c]);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.post_slow(
        b,
        "/v1/rotations",
        json!({ "rotation": b64(&statement) }),
        wait,
    );
    slow.body_started().await;
    // A revokes B in the meantime (the rotation of an honest owner).
    let (revoke_b, _) = env.rotation_for(&ts, a, &[b]);
    env.post_rotation(a, &revoke_b).await.ok(201);
    assert_eq!(env.key_epoch().await, 2);

    let _ = release.send(());
    let late = slow.answer().await;
    // The answer of a device revoked before the request: same status, same body.
    let normal = env.get(b, "/v1/space").await;
    assert_eq!((normal.status, normal.code()), (403, "revoked".to_owned()));
    assert_eq!(same_answer(&late), same_answer(&normal));
    // The epoch of the relay did not move again, C was not revoked, no second rotation.
    assert_eq!(env.key_epoch().await, 2);
    assert_eq!(env.sql_count("SELECT count(*) FROM rotations"), 1);
    env.get(c, "/v1/space").await.ok(200);
}

#[tokio::test]
async fn a_tombstone_whose_device_is_revoked_meanwhile_changes_nothing() {
    let env = Env::new().await;
    let (devs, _ts) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let (item, sent) = env.send_item(a, &[]).await;
    sent.ok(201);
    let t = env.tombstone(&b.keys, &item, DeletionScope::Everywhere);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.post_slow(b, "/v1/tombstones", json!({ "tombstone": b64(&t) }), wait);
    slow.body_started().await;
    // The administrator revokes B in the meantime.
    let out = tomi_relay::admin::run(
        &env.state,
        tomi_relay::admin::Cmd::Revoke {
            target: b.id[..10].to_owned(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);

    let _ = release.send(());
    let late = slow.answer().await;
    let normal = env.get(b, "/v1/space").await;
    assert_eq!((normal.status, normal.code()), (403, "revoked".to_owned()));
    assert_eq!(same_answer(&late), same_answer(&normal));
    // The item is still there: nothing was deleted, no event was written.
    assert_eq!(env.sql_count("SELECT count(*) FROM tombstones"), 0);
    assert_eq!(
        env.sql_count("SELECT count(*) FROM items WHERE purged_at IS NULL"),
        1
    );
    assert!(env.events_of_type(a, "tombstone").await.is_empty());
}

#[tokio::test]
async fn a_request_whose_session_is_closed_meanwhile_is_answered_like_a_closed_session() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let (item, sent) = env.send_item(a, &[]).await;
    sent.ok(201);
    let t = env.tombstone(&b.keys, &item, DeletionScope::Everywhere);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.post_slow(b, "/v1/tombstones", json!({ "tombstone": b64(&t) }), wait);
    slow.body_started().await;
    // B logs out (another request of the same device) while the slow one is in flight.
    env.delete(b, "/v1/auth/session").await.ok(204);

    let _ = release.send(());
    let late = slow.answer().await;
    let normal = env.get(b, "/v1/space").await;
    assert_eq!(
        (normal.status, normal.code()),
        (401, "unauthorized".to_owned())
    );
    assert_eq!(same_answer(&late), same_answer(&normal));
    assert_eq!(env.sql_count("SELECT count(*) FROM tombstones"), 0);
}

#[tokio::test]
async fn a_member_that_stays_is_not_disturbed_by_the_new_check() {
    // The control: the same slow request, nobody is revoked, goes through.
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let (item, sent) = env.send_item(a, &[]).await;
    sent.ok(201);
    let t = env.tombstone(&b.keys, &item, DeletionScope::Everywhere);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.post_slow(b, "/v1/tombstones", json!({ "tombstone": b64(&t) }), wait);
    slow.body_started().await;
    let _ = release.send(());
    let late = slow.answer().await;
    assert_eq!(late.status, 201, "{}", String::from_utf8_lossy(&late.body));
    assert_eq!(env.sql_count("SELECT count(*) FROM tombstones"), 1);
}

async fn admin_revoke(env: &Env, d: &Dev) {
    let out = tomi_relay::admin::run(
        &env.state,
        tomi_relay::admin::Cmd::Revoke {
            target: d.id[..10].to_owned(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
}

#[tokio::test]
async fn an_item_whose_device_is_revoked_meanwhile_is_not_stored() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let id = new_uuid();
    let epoch = env.key_epoch().await;
    let envelope = Env::envelope(&uuid_bytes(&id), epoch, 300);
    let body = env.item_body(&id, epoch, 7 * DAY_MS, &envelope, &[]);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.post_slow(b, "/v1/items", body, wait);
    slow.body_started().await;
    admin_revoke(&env, b).await;

    let _ = release.send(());
    let late = slow.answer().await;
    let normal = env.get(b, "/v1/space").await;
    assert_eq!((normal.status, normal.code()), (403, "revoked".to_owned()));
    assert_eq!(same_answer(&late), same_answer(&normal));
    // Nothing was stored, nothing announced, nothing counted against the quota.
    assert_eq!(env.sql_count("SELECT count(*) FROM items"), 0);
    assert!(env.events_of_type(a, "item").await.is_empty());
    assert_eq!(
        env.get(a, "/v1/space").await.ok(200)["quota"]["used_bytes"],
        0
    );
}

/// The case that really is long: the end of a part of a blob. The bytes of the first half are
/// already in the file when the device is revoked; the part must not be recorded and the file
/// must go back to where it was.
#[tokio::test]
async fn the_end_of_a_blob_part_whose_device_is_revoked_meanwhile_is_not_recorded() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let b = &devs[1];
    let id = new_uuid();
    env.create_blob(b, &id, 4000).await.ok(201);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.patch_blob_slow(b, &id, 0, vec![7u8; 4000], wait);
    slow.body_started().await;
    admin_revoke(&env, b).await;

    let _ = release.send(());
    let late = slow.answer().await;
    let normal = env.get(b, "/v1/space").await;
    assert_eq!((normal.status, normal.code()), (403, "revoked".to_owned()));
    assert_eq!(same_answer(&late), same_answer(&normal));
    // The progress did not move, and the half part written is cut off.
    assert_eq!(env.sql_count("SELECT upload_offset FROM blobs"), 0);
    assert_eq!(env.sql_count("SELECT complete FROM blobs"), 0);
    let path = tomi_relay::blobstore::path(&env.space().dir, &uuid_bytes(&id));
    assert_eq!(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), 0);
}

#[tokio::test]
async fn a_blob_part_of_a_member_that_stays_goes_through_the_slow_path() {
    // The control: the same slow PATCH, nobody is revoked.
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let b = &devs[1];
    let id = new_uuid();
    env.create_blob(b, &id, 4000).await.ok(201);
    let (release, wait) = tokio::sync::oneshot::channel();
    let mut slow = env.patch_blob_slow(b, &id, 0, vec![7u8; 4000], wait);
    slow.body_started().await;
    let _ = release.send(());
    let late = slow.answer().await;
    assert_eq!(late.status, 204, "{}", String::from_utf8_lossy(&late.body));
    assert_eq!(env.sql_count("SELECT upload_offset FROM blobs"), 4000);
    assert_eq!(env.sql_count("SELECT complete FROM blobs"), 1);
}
