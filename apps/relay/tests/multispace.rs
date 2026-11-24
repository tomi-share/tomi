//! One relay, several independent spaces (`api-v1.md` §0.2, §1.3, §3.1, §4.6, §8.4, §8.7, §10):
//! isolation between the spaces, the global uniqueness of the identifiers that route a request,
//! the shared and per-space quotas, `admin --space`, and the migration of the older layout.
//!
//! Two spaces A and B are made with `Env::another_space`: same relay, same clock, same disk.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::collections::HashSet;

use common::*;
use serde_json::{json, Value};
use tomi_relay::admin::{self, Cmd};
use tomi_relay::maintenance;
use tomi_relay::spaces::Route;

/// Two spaces of `n` devices each, on one relay.
async fn two_spaces_with(
    n: usize,
    tweak: impl FnOnce(&mut Config, &mut Options),
) -> (Env, Env, Vec<Dev>, Vec<Dev>) {
    let a = Env::with(tweak).await;
    let b = a.another_space().await;
    let (da, _) = a.space_of(n).await;
    let (db, _) = b.space_of(n).await;
    (a, b, da, db)
}

async fn two_spaces(n: usize) -> (Env, Env, Vec<Dev>, Vec<Dev>) {
    two_spaces_with(n, |_, _| {}).await
}

async fn run(env: &Env, cmd: Cmd) -> admin::Output {
    admin::run(&env.state, cmd).await
}

fn prefix(env: &Env) -> Option<String> {
    Some(env.space_id[..8].to_owned())
}

async fn register_push(env: &Env, d: &Dev, token: &str) {
    env.put(
        d,
        "/v1/me/push",
        json!({ "provider": "fcm", "token": token }),
    )
    .await
    .ok(204);
}

fn ids(list: &Value) -> HashSet<String> {
    list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["device_id"].as_str().unwrap().to_owned())
        .collect()
}

// ===== layout and independence ====================================================

#[tokio::test]
async fn two_spaces_share_a_relay_and_nothing_else() {
    let (a, b, da, db) = two_spaces(2).await;
    let hello = a.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], true);

    // One directory and one database per space, one routing database for the relay.
    let (sa, sb) = (a.space(), b.space());
    assert_ne!(sa.dir, sb.dir);
    for (env, space) in [(&a, &sa), (&b, &sb)] {
        assert_eq!(
            space.dir,
            env.data_dir().join("spaces").join(&env.space_id),
            "<data_dir>/spaces/<space_id>"
        );
        assert!(space.dir.join("relay.db").is_file());
    }
    assert!(a.data_dir().join("routing.db").is_file());
    assert!(!a.data_dir().join("tomi.db").exists());
    assert!(!a.data_dir().join("blobs").exists(), "blobs are per space");

    // Each session sees its own space, its own devices and its own log.
    assert_eq!(
        a.get(&da[0], "/v1/space").await.ok(200)["space_id"],
        a.space_id
    );
    assert_eq!(
        b.get(&db[0], "/v1/space").await.ok(200)["space_id"],
        b.space_id
    );
    let (la, lb) = (
        a.get(&da[0], "/v1/devices").await.ok(200),
        b.get(&db[0], "/v1/devices").await.ok(200),
    );
    assert_eq!(
        ids(&la),
        HashSet::from([da[0].id.clone(), da[1].id.clone()])
    );
    assert_eq!(
        ids(&lb),
        HashSet::from([db[0].id.clone(), db[1].id.clone()])
    );
    for (env, d, own) in [(&a, &da[0], &da), (&b, &db[0], &db)] {
        let events = env.all_events(d).await;
        assert_eq!(events.len(), 2, "two device_added, nothing else");
        for e in events {
            let id = e["data"]["device"]["device_id"].as_str().unwrap();
            assert!(own.iter().any(|x| x.id == id));
        }
    }
}

#[tokio::test]
async fn a_session_of_one_space_reads_and_writes_nothing_of_the_other() {
    let (a, b, da, db) = two_spaces(2).await;
    let blob = a.upload_blob(&da[0], &[7u8; 500], 500).await;
    let (item, r) = a.send_item(&da[0], &[&blob]).await;
    r.ok(201);
    let item_path = format!("/v1/items/{item}");

    // Items, receipts, blobs: unknown to B, answered as an unknown id.
    b.get(&db[0], &item_path).await.err(404, "not_found");
    b.post(&db[0], &format!("{item_path}/ack"), json!({}))
        .await
        .err(404, "not_found");
    b.post(&db[0], &format!("{item_path}/read"), json!({}))
        .await
        .err(404, "not_found");
    b.download(&db[0], &blob, None).await.err(404, "not_found");
    b.get(&db[0], &format!("/v1/blobs/{blob}/status"))
        .await
        .err(404, "not_found");
    b.delete(&db[0], &format!("/v1/blobs/{blob}"))
        .await
        .err(404, "not_found");
    // B cannot attach a blob of A to an item of its own.
    let own = new_uuid();
    let env = Env::envelope(&uuid_bytes(&own), 1, 300);
    b.post(
        &db[0],
        "/v1/items",
        b.item_body(&own, 1, DAY_MS, &env, &[&blob]),
    )
    .await
    .err(404, "not_found");
    // Its log holds none of A's events, its device list none of A's devices.
    assert!(b
        .all_events(&db[0])
        .await
        .iter()
        .all(|e| e["type"] == "device_added"));
    b.bearer(
        "PATCH",
        &format!("/v1/devices/{}", da[0].id),
        db[0].tok(),
        Some(json!({ "name": "mine now" })),
    )
    .await
    .err(404, "not_found");
    assert_eq!(
        a.get(&da[1], "/v1/devices").await.ok(200)["devices"][0]["name"],
        "dev0"
    );

    // Identifiers are per space: the same item and blob ids are free in B, and A cannot tell.
    b.create_blob(&db[0], &blob, 100).await.ok(201);
    b.post(
        &db[0],
        "/v1/items",
        b.item_body(
            &item,
            1,
            DAY_MS,
            &Env::envelope(&uuid_bytes(&item), 1, 300),
            &[],
        ),
    )
    .await
    .ok(201);
    // A tombstone of B for A's item is accepted (any id is) and purges nothing of A.
    b.post_tombstone(&db[0], &item).await.ok(201);
    a.get(&da[1], &item_path).await.ok(200);
    let got = a.download(&da[1], &blob, None).await;
    assert_eq!((got.status, got.body.len()), (200, 500));
}

#[tokio::test]
async fn keys_and_rotations_stay_in_their_space() {
    let a = Env::new().await;
    let b = a.another_space().await;
    let (da, ts) = a.space_of(2).await;
    let (db, _) = b.space_of(2).await;
    let (rotation, _) = a.rotation_for(&ts, &da[0], &[&da[1]]);
    a.post_rotation(&da[0], &rotation).await.ok(201);

    let ka = a.get(&da[0], "/v1/keys").await.ok(200);
    assert_eq!(
        (
            ka["key_epoch"].as_u64(),
            ka["rotations"].as_array().unwrap().len()
        ),
        (Some(2), 1)
    );
    let kb = b.get(&db[0], "/v1/keys").await.ok(200);
    assert_eq!(
        (
            kb["key_epoch"].as_u64(),
            kb["rotations"].as_array().unwrap().len()
        ),
        (Some(1), 0)
    );
    // A's rotation means nothing in B: it names another space.
    let r = b.post_rotation(&db[0], &rotation).await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/rotation");
    // B's devices and epoch are untouched; A's revoked device is not B's business.
    assert_eq!(b.get(&db[0], "/v1/space").await.ok(200)["key_epoch"], 1);
    assert!(b.events_of_type(&db[0], "device_revoked").await.is_empty());
    a.get(&da[1], "/v1/space").await.err(403, "revoked");
    b.get(&db[1], "/v1/space").await.ok(200);
}

#[tokio::test]
async fn websockets_and_wakes_stay_in_their_space() {
    let (a, b, da, db) = two_spaces(2).await;
    let (addr, stop, _server) = serve(&a).await;
    let mut wa = Ws::connect(addr).await;
    let ready_a = wa.auth(da[1].tok()).await;
    let mut wb = Ws::connect(addr).await;
    let ready_b = wb.auth(db[1].tok()).await;
    assert_eq!(ready_a["head_seq"], a.head_seq().await);
    assert_eq!(ready_b["head_seq"], b.head_seq().await);

    a.send_item(&da[0], &[]).await.1.ok(201);
    assert_eq!(wa.recv_json(1000).await.unwrap()["type"], "wake");
    assert!(
        wb.recv_json(400).await.is_none(),
        "the other space heard nothing"
    );
    b.send_item(&db[0], &[]).await.1.ok(201);
    assert_eq!(wb.recv_json(1000).await.unwrap()["type"], "wake");
    assert!(wa.recv_json(400).await.is_none());
    let _ = stop.send(());
}

#[tokio::test]
async fn push_goes_to_the_space_of_the_device() {
    let (a, b, da, db) = two_spaces(2).await;
    register_push(&a, &da[1], "tok-A1").await;
    register_push(&b, &db[1], "tok-B1").await;
    a.send_item(&da[0], &[]).await.1.ok(201);
    a.settle(300).await;
    assert_eq!(a.push.calls(), vec![("tok-A1".to_owned(), Urgency::High)]);
    a.settle(600).await;
    a.push.clear();
    b.send_item(&db[0], &[]).await.1.ok(201);
    b.settle(300).await;
    assert_eq!(b.push.calls(), vec![("tok-B1".to_owned(), Urgency::High)]);
    // The same token in two spaces is two registrations: it is not taken from the first.
    register_push(&a, &da[0], "tok-shared").await;
    register_push(&b, &db[0], "tok-shared").await;
    assert_eq!(
        a.sql_count("SELECT count(*) FROM push_registrations WHERE token = 'tok-shared'"),
        1
    );
    assert_eq!(
        b.sql_count("SELECT count(*) FROM push_registrations WHERE token = 'tok-shared'"),
        1
    );
}

// ===== identifiers that route ======================================================

#[tokio::test]
async fn a_device_of_one_space_has_no_challenge_in_another() {
    let (a, b, da, db) = two_spaces(1).await;
    // A challenge asked for A's device is A's. Signed for B's space it opens nothing.
    let ch = a.challenge(&da[0].id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    a.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(b.session_body(&da[0], &challenge, 1, 1, "windows")),
    )
    .await
    .err(401, "unauthorized");
    // A challenge of B's device cannot be used for A's device.
    let chb = b.challenge(&db[0].id).await.ok(200);
    let challenge_b = unb64(chb["challenge"].as_str().unwrap());
    a.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(a.session_body(&da[0], &challenge_b, 1, 1, "windows")),
    )
    .await
    .err(401, "unauthorized");
    // The right one works, in the right space.
    let mut d = clone_dev(&da[0]);
    a.login(&mut d, 1, 1).await;
    let mut e = clone_dev(&db[0]);
    b.login(&mut e, 1, 1).await;

    // A device that no space has is answered like one that exists, then refused like one.
    let stranger = Dev::new("stranger");
    let known = a.challenge(&da[0].id).await;
    let unknown = a.challenge(&stranger.id).await;
    assert_eq!((known.status, unknown.status), (200, 200));
    a.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(a.session_body(
            &stranger,
            &unb64(unknown.json()["challenge"].as_str().unwrap()),
            1,
            1,
            "windows",
        )),
    )
    .await
    .err(401, "unauthorized");
}

#[tokio::test]
async fn pairing_channels_belong_to_their_space() {
    let (a, b, da, db) = two_spaces(1).await;
    let ch = a
        .post(&da[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let id = ch["channel_id"].as_str().unwrap().to_owned();
    let messages = format!("/v1/pairing/channels/{id}/messages");

    // B's session finds it unknown: it can neither write, read, close nor register through it.
    b.post(&db[0], &messages, json!({ "body": b64(b"hi") }))
        .await
        .err(404, "not_found");
    b.get(&db[0], &format!("{messages}?after=0"))
        .await
        .err(404, "not_found");
    b.delete(&db[0], &format!("/v1/pairing/channels/{id}"))
        .await
        .err(404, "not_found");
    let newcomer = Dev::new("newcomer");
    b.post(
        &db[0],
        "/v1/devices",
        json!({
            "channel_id": id, "key_epoch": 1, "name": "x", "platform": "android",
            "certificate": b64(&b.cert(&db[0].keys, &newcomer.keys)),
        }),
    )
    .await
    .err(404, "not_found");

    // Anyone may claim an open channel (the protocol), and gets a token for that channel only.
    let claimed = b
        .send(
            "POST",
            &format!("/v1/pairing/channels/{id}/claim"),
            &[],
            Some(json!({})),
        )
        .await
        .ok(200);
    let token = claimed["pairing_token"].as_str().unwrap().to_owned();
    let auth = format!("Pairing {token}");
    let other = b
        .post(&db[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let other_id = other["channel_id"].as_str().unwrap().to_owned();
    assert_ne!(id, other_id, "channel numbers are unique on the relay");
    b.send(
        "POST",
        &format!("/v1/pairing/channels/{other_id}/messages"),
        &[("authorization", auth.as_str())],
        Some(json!({ "body": b64(b"x") })),
    )
    .await
    .err(401, "unauthorized");
    // On its own channel the token works, and A's inviter reads the message.
    a.send(
        "POST",
        &messages,
        &[("authorization", auth.as_str())],
        Some(json!({ "body": b64(b"x") })),
    )
    .await
    .ok(201);
    let read = a.get(&da[0], &format!("{messages}?after=0")).await.ok(200);
    assert_eq!(read["messages"].as_array().unwrap().len(), 1);
    // The registration of the new device is the inviter's, in its space.
    a.pair_raw(&da[0], &Dev::new("third"), 1).await.ok(201);
}

#[tokio::test]
async fn channel_numbers_are_unique_across_the_relay() {
    let (a, b, da, db) = two_spaces(1).await;
    let space_a = uuid_bytes(&a.space_id);
    // A holds 5000 of the 9999 numbers (closed channels, which keep their number until they
    // expire). Without a relay-wide check, half of B's numbers would be A's.
    a.db()
        .write_sync(|tx| {
            for n in 1..=5000 {
                tx.execute(
                    "INSERT INTO pairing_channels (channel_id, creator, state, created_at, expires_at)
                     VALUES (?1, ?2, 'closed', ?3, ?4)",
                    rusqlite::params![n.to_string(), "someone", T0, T0 + DAY_MS],
                )?;
            }
            Ok(tomi_relay::db::Tx::Commit(()))
        })
        .unwrap();
    for n in 1..=5000 {
        a.state.spaces.add_channel(&n.to_string(), space_a);
    }
    for _ in 0..25 {
        let ch = b
            .post(&db[0], "/v1/pairing/channels", json!({}))
            .await
            .ok(201);
        let id = ch["channel_id"].as_str().unwrap().to_owned();
        assert!(
            id.parse::<u32>().unwrap() > 5000,
            "channel {id} is A's, B was given it"
        );
        b.delete(&db[0], &format!("/v1/pairing/channels/{id}"))
            .await
            .ok(204);
    }
    // And alternating between the spaces, a number is never given twice.
    let mut seen = HashSet::new();
    for _ in 0..20 {
        for (env, d) in [(&a, &da[0]), (&b, &db[0])] {
            let ch = env.post(d, "/v1/pairing/channels", json!({})).await.ok(201);
            let id = ch["channel_id"].as_str().unwrap().to_owned();
            assert!(seen.insert(id.clone()), "channel {id} was given twice");
            let owner = env.state.spaces.by_channel(&id).unwrap();
            assert_eq!(owner.id, uuid_bytes(&env.space_id));
            env.delete(d, &format!("/v1/pairing/channels/{id}"))
                .await
                .ok(204);
        }
    }
}
#[tokio::test]
async fn a_key_known_in_another_space_is_refused() {
    let (a, b, da, db) = two_spaces(2).await;
    // A device of A cannot be registered in B, by the very error of a duplicate in a space.
    b.pair_raw(&db[0], &da[0], 1)
        .await
        .err(409, "device_exists");
    // (the abandoned channels of the failed attempts expire: two may be open at a time)
    b.clock.advance_secs(130);
    // Nor a revoked one.
    let out = run(
        &a,
        Cmd::Revoke {
            target: da[1].id.clone(),
            space: prefix(&a),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    b.pair_raw(&db[0], &da[1], 1)
        .await
        .err(409, "device_exists");
    b.clock.advance_secs(130);
    // The same device cannot found a third space, and the invitation is not spent.
    let c = a.another_space().await;
    let founder = clone_dev(&da[0]);
    c.create_space_raw(&founder, &c.install_token)
        .await
        .err(409, "device_exists");
    assert!(tomi_relay::install::is_pending(c.state.spaces.routing(), &c.install_token).unwrap());
    // A key nobody has is welcome in B (the sessions are one hour long: still good).
    b.pair_raw(&db[0], &Dev::new("fresh"), 1).await.ok(201);
    c.create_space_raw(&Dev::new("founder of C"), &c.install_token)
        .await
        .ok(201);
}

#[tokio::test]
async fn the_same_key_cannot_enter_two_spaces_at_once() {
    let (a, b, da, db) = two_spaces(1).await;
    let newcomer = Dev::new("newcomer");
    let (ra, rb) = tokio::join!(
        a.pair_raw(&da[0], &newcomer, 1),
        b.pair_raw(&db[0], &newcomer, 1)
    );
    let mut statuses = [(ra.status, ra.code()), (rb.status, rb.code())];
    statuses.sort();
    assert_eq!(statuses[0].0, 201);
    assert_eq!(statuses[1], (409, "device_exists".to_owned()));
}

#[tokio::test]
async fn space_not_initialized_is_for_an_empty_relay_only() {
    let a = Env::new().await;
    // Empty: every route says so.
    a.bearer("GET", "/v1/space", "tms1.whatever", None)
        .await
        .err(409, "space_not_initialized");
    a.challenge(&Dev::new("x").id)
        .await
        .err(409, "space_not_initialized");
    a.send(
        "POST",
        "/v1/pairing/channels/42/claim",
        &[],
        Some(json!({})),
    )
    .await
    .err(409, "space_not_initialized");
    let mut founder = Dev::new("PC");
    a.create_space(&mut founder).await;

    // Not empty: an identifier that designates no space is an unknown identifier.
    a.bearer("GET", "/v1/space", "tms1.whatever", None)
        .await
        .err(401, "unauthorized");
    a.bearer("GET", "/v1/space", "garbage", None)
        .await
        .err(401, "unauthorized");
    a.send("GET", "/v1/space", &[], None)
        .await
        .err(401, "unauthorized");
    let c = a.challenge(&Dev::new("x").id).await;
    assert_eq!(c.status, 200);
    a.send(
        "POST",
        "/v1/pairing/channels/42/claim",
        &[],
        Some(json!({})),
    )
    .await
    .err(404, "not_found");
    a.send(
        "POST",
        "/v1/pairing/channels/42/messages",
        &[("authorization", "Pairing tmp1.nothing")],
        Some(json!({ "body": b64(b"x") })),
    )
    .await
    .err(404, "not_found");
    a.send(
        "GET",
        "/v1/pairing/channels/42/messages",
        &[("authorization", "Pairing tmp1.nothing")],
        None,
    )
    .await
    .err(404, "not_found");
    assert_eq!(
        a.send("GET", "/v1/hello", &[], None).await.ok(200)["space_initialized"],
        true
    );
}

// ===== reset ========================================================================

#[tokio::test]
async fn the_reset_of_one_space_leaves_the_other_intact() {
    let (a, b, da, db) = two_spaces(2).await;
    let blob_b = b.upload_blob(&db[0], &[3u8; 400], 400).await;
    let (item_b, r) = b.send_item(&db[0], &[&blob_b]).await;
    r.ok(201);
    a.send_item(&da[0], &[]).await.1.ok(201);
    let (addr, stop, _server) = serve(&a).await;
    // Both read in the background from the opening: unread, they would be cut by the relay
    // (ping 300 ms, dropped 300 ms later) before the close this test waits for.
    let wa = Ws::open(addr, &da[1]).await.watch_close();
    let wb = Ws::open(addr, &db[1]).await.watch_close();
    let (dir_a, dir_b) = (a.space().dir.clone(), b.space().dir.clone());
    let old_token = da[1].tok().to_owned();

    // Two spaces and no --space: a usage error that lists them, nothing happens.
    let ambiguous = run(
        &a,
        Cmd::Reset {
            confirm: Some(a.space_id[..8].to_owned()),
            space: None,
        },
    )
    .await;
    assert_eq!(ambiguous.exit, 2);
    assert!(ambiguous.stderr.contains(&a.space_id) && ambiguous.stderr.contains(&b.space_id));
    assert!(dir_a.is_dir());
    // The confirmation is the one of the space asked for, not of another.
    let wrong = run(
        &a,
        Cmd::Reset {
            confirm: Some(b.space_id[..8].to_owned()),
            space: prefix(&a),
        },
    )
    .await;
    assert_eq!(wrong.exit, 2);
    a.get(&da[0], "/v1/space").await.ok(200);

    let out = run(
        &a,
        Cmd::Reset {
            confirm: Some(a.space_id[..8].to_owned()),
            space: prefix(&a),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(out.stdout.contains(&a.space_id[..8]), "{}", out.stdout);
    // No token: B is still there, the relay is not empty.
    assert!(tomi_relay::install::list(a.state.spaces.routing())
        .unwrap()
        .is_empty());
    assert_eq!(wa.code(5000).await, Some(4004));
    assert!(wb.still_open(300).await, "B's socket stays open");
    assert!(!dir_a.exists(), "A's database and blobs are gone");
    assert!(dir_b.join("relay.db").is_file());

    // B works as before.
    b.get(&db[0], "/v1/space").await.ok(200);
    b.get(&db[1], &format!("/v1/items/{item_b}")).await.ok(200);
    assert_eq!(b.download(&db[1], &blob_b, None).await.body.len(), 400);
    b.send_item(&db[0], &[]).await.1.ok(201);
    // A is unknown, not "not initialized".
    a.bearer("GET", "/v1/space", &old_token, None)
        .await
        .err(401, "unauthorized");
    assert_eq!(a.challenge(&da[0].id).await.status, 200);
    let ch = a.challenge(&da[0].id).await.ok(200);
    let late = a.session_body(
        &da[0],
        &unb64(ch["challenge"].as_str().unwrap()),
        1,
        1,
        "windows",
    );
    a.send("POST", "/v1/auth/session", &[], Some(late))
        .await
        .err(401, "unauthorized");
    assert!(a.state.spaces.by_device(&da[0].id).is_none());
    // Its keys are free again: the same device can found a new space.
    let c = a.another_space().await;
    c.create_space_raw(&clone_dev(&da[0]), &c.install_token)
        .await
        .ok(201);

    // Resetting the last space gives a token, and the relay is empty again.
    let out = run(
        &c,
        Cmd::Reset {
            confirm: Some(c.space_id[..8].to_owned()),
            space: prefix(&c),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert!(out.stdout.contains("deleted"), "B is left: no token");
    let out = run(
        &b,
        Cmd::Reset {
            confirm: Some(b.space_id[..8].to_owned()),
            space: prefix(&b),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let token = out.stdout.trim().to_owned();
    assert!(tomi_relay::install::is_pending(b.state.spaces.routing(), &token).unwrap());
    assert!(b.state.spaces.is_empty());
    b.bearer("GET", "/v1/space", db[0].tok(), None)
        .await
        .err(409, "space_not_initialized");
    // On an empty relay `reset` has nothing to delete: it adds an invitation and keeps the pending ones.
    let again = run(
        &b,
        Cmd::Reset {
            confirm: None,
            space: None,
        },
    )
    .await;
    assert_eq!(again.exit, 0, "{}", again.stderr);
    assert!(tomi_relay::install::is_pending(b.state.spaces.routing(), &token).unwrap());
    assert!(
        tomi_relay::install::is_pending(b.state.spaces.routing(), again.stdout.trim()).unwrap()
    );
    let _ = stop.send(());
}

// ===== quotas ========================================================================

#[tokio::test]
async fn the_total_is_shared_and_a_full_answer_names_only_the_own_usage() {
    let (a, b, da, db) = two_spaces_with(1, |cfg, _| cfg.max_relay_bytes = 10_000).await;
    a.create_blob(&da[0], &new_uuid(), 6000).await.ok(201);
    let own = new_uuid();
    b.create_blob(&db[0], &own, 3000).await.ok(201);

    // B is told its own usage and a ceiling: what it uses plus the room left in the total.
    let full = b.create_blob(&db[0], &new_uuid(), 2000).await;
    full.err(507, "relay_full");
    assert_eq!(full.json()["used_bytes"], 3000);
    assert_eq!(full.json()["max_bytes"], 4000);
    assert_eq!(
        b.get(&db[0], "/v1/space").await.ok(200)["quota"],
        json!({ "used_bytes": 3000, "max_bytes": 4000 })
    );
    assert_eq!(
        a.get(&da[0], "/v1/space").await.ok(200)["quota"],
        json!({ "used_bytes": 6000, "max_bytes": 7000 })
    );
    // The login answer says the same.
    let mut d = clone_dev(&da[0]);
    let login = a.login(&mut d, 1, 1).await;
    assert_eq!(
        login["space"]["quota"],
        json!({ "used_bytes": 6000, "max_bytes": 7000 })
    );

    // A takes the last 1000; one byte more is refused.
    a.create_blob(&da[0], &new_uuid(), 1000).await.ok(201);
    a.create_blob(&da[0], &new_uuid(), 1)
        .await
        .err(507, "relay_full");
    // Room freed in B is room for A.
    b.delete(&db[0], &format!("/v1/blobs/{own}")).await.ok(204);
    a.create_blob(&da[0], &new_uuid(), 2000).await.ok(201);
    // Items count too: B's ceiling is now what it uses plus the 1000 bytes left.
    let (_, r) = b.send_item(&db[0], &[]).await;
    r.ok(201);
    let quota = b.get(&db[0], "/v1/space").await.ok(200)["quota"].clone();
    assert!(quota["used_bytes"].as_u64().unwrap() > 0);
    assert_eq!(
        quota["max_bytes"].as_u64().unwrap() - quota["used_bytes"].as_u64().unwrap(),
        1000 - quota["used_bytes"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn two_spaces_never_overbook_the_total() {
    // Both ask for 6000 of 10 000 at the same moment: exactly one gets it.
    for round in 0..6 {
        let (a, b, da, db) = two_spaces_with(1, |cfg, _| cfg.max_relay_bytes = 10_000).await;
        let (id_a, id_b) = (new_uuid(), new_uuid());
        let (ra, rb) = tokio::join!(
            a.create_blob(&da[0], &id_a, 6000),
            b.create_blob(&db[0], &id_b, 6000)
        );
        let mut statuses = [ra.status, rb.status];
        statuses.sort_unstable();
        assert_eq!(statuses, [201, 507], "round {round}");
    }
}

#[tokio::test]
async fn a_quota_per_space_is_optional_and_set_by_the_administrator() {
    let (a, b, da, db) = two_spaces(1).await;
    // No quota by default: only the total limits a space.
    a.create_blob(&da[0], &new_uuid(), 20_000_000).await.ok(201);
    a.create_blob(&da[0], &new_uuid(), 20_000_000).await.ok(201);
    assert_eq!(
        a.get(&da[0], "/v1/space").await.ok(200)["quota"]["max_bytes"],
        1_073_741_824u64
    );

    // A quota of 30 MB for A: already above it, nothing more goes in. B is not concerned.
    let set = run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "30000000".into(),
        },
    )
    .await;
    assert_eq!(set.exit, 0, "{}", set.stderr);
    let full = a.create_blob(&da[0], &new_uuid(), 1000).await;
    full.err(507, "relay_full");
    assert_eq!(full.json()["max_bytes"], 30_000_000);
    assert_eq!(full.json()["used_bytes"], 40_000_000);
    assert_eq!(
        a.get(&da[0], "/v1/space").await.ok(200)["quota"]["max_bytes"],
        30_000_000
    );
    b.create_blob(&db[0], &new_uuid(), 20_000_000).await.ok(201);

    // A bigger one, then none, then back to the default (which is none).
    for value in ["50000000", "none", "default"] {
        let set = run(
            &a,
            Cmd::Quota {
                space: prefix(&a),
                value: value.into(),
            },
        )
        .await;
        assert_eq!(set.exit, 0, "{value}: {}", set.stderr);
        a.create_blob(&da[0], &new_uuid(), 1000).await.ok(201);
    }
    // Bad values and bad targets.
    for value in ["0", "-5", "lots", ""] {
        let out = run(
            &a,
            Cmd::Quota {
                space: prefix(&a),
                value: value.into(),
            },
        )
        .await;
        assert_eq!(out.exit, 2, "{value:?}");
    }
    let unknown = run(
        &a,
        Cmd::Quota {
            space: Some("ffffffff".into()),
            value: "1000".into(),
        },
    )
    .await;
    assert_eq!(unknown.exit, 1);
    let missing = run(
        &a,
        Cmd::Quota {
            space: None,
            value: "1000".into(),
        },
    )
    .await;
    assert_eq!(missing.exit, 2, "two spaces: --space is needed");
    assert!(missing.stderr.contains(&a.space_id));

    // The override survives a restart, and so do the sessions.
    run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "50000000".into(),
        },
    )
    .await;
    let (space_a, space_b) = (a.space_id.clone(), b.space_id.clone());
    let a = a.restart().await;
    let b = a.for_space(&space_b);
    assert_eq!(a.space_id, space_a);
    assert_eq!(
        a.state
            .spaces
            .space_max(&uuid_bytes(&space_a), &a.state.cfg),
        Some(50_000_000)
    );
    assert_eq!(
        a.state
            .spaces
            .space_max(&uuid_bytes(&space_b), &a.state.cfg),
        None
    );
    assert_eq!(
        a.get(&da[0], "/v1/space").await.ok(200)["quota"]["max_bytes"],
        50_000_000
    );
    b.get(&db[0], "/v1/space").await.ok(200);
}

#[tokio::test]
async fn max_space_bytes_of_the_configuration_is_the_default_of_every_space() {
    let (a, b, da, db) = two_spaces_with(1, |cfg, _| cfg.max_space_bytes = Some(5000)).await;
    for (env, d) in [(&a, &da[0]), (&b, &db[0])] {
        env.create_blob(d, &new_uuid(), 4000).await.ok(201);
        let r = env.create_blob(d, &new_uuid(), 2000).await;
        r.err(507, "relay_full");
        assert_eq!(r.json()["max_bytes"], 5000);
    }
    // An override beats the default, either way.
    run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "none".into(),
        },
    )
    .await;
    a.create_blob(&da[0], &new_uuid(), 2000).await.ok(201);
    run(
        &b,
        Cmd::Quota {
            space: prefix(&b),
            value: "6000".into(),
        },
    )
    .await;
    b.create_blob(&db[0], &new_uuid(), 2000).await.ok(201);
    b.create_blob(&db[0], &new_uuid(), 1)
        .await
        .err(507, "relay_full");
    // `default` gives the default back.
    run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "default".into(),
        },
    )
    .await;
    a.create_blob(&da[0], &new_uuid(), 1)
        .await
        .err(507, "relay_full");
}

// ===== maintenance ====================================================================

#[tokio::test]
async fn the_purge_runs_on_every_space_and_deletes_only_its_own_files() {
    let (a, b, mut da, mut db) = two_spaces(2).await;
    let blob_a = a.upload_blob(&da[0], &[1u8; 300], 300).await;
    let blob_b = b.upload_blob(&db[0], &[2u8; 300], 300).await;
    // A's item expires in one day, B's in seven.
    let ia = new_uuid();
    a.post(
        &da[0],
        "/v1/items",
        a.item_body(
            &ia,
            1,
            DAY_MS,
            &Env::envelope(&uuid_bytes(&ia), 1, 300),
            &[&blob_a],
        ),
    )
    .await
    .ok(201);
    let (ib, r) = b.send_item(&db[0], &[&blob_b]).await;
    r.ok(201);
    let file_a = tomi_relay::blobstore::path(&a.space().dir, &uuid_bytes(&blob_a));
    let file_b = tomi_relay::blobstore::path(&b.space().dir, &uuid_bytes(&blob_b));
    assert!(file_a.is_file() && file_b.is_file());

    a.clock.advance_secs(2 * 86_400);
    let report = maintenance::run_once(&a.state).await;
    assert_eq!(report.items_purged, 1, "only A's item is due");
    assert!(!file_a.exists());
    assert!(file_b.is_file(), "B's blob is untouched");
    a.login(&mut da[0], 1, 1).await;
    b.login(&mut db[1], 1, 1).await;
    b.get(&db[1], &format!("/v1/items/{ib}")).await.ok(200);

    a.clock.advance_secs(6 * 86_400);
    let report = maintenance::run_once(&a.state).await;
    assert_eq!(report.items_purged, 1);
    assert!(!file_b.exists());
    // Compaction runs on both too.
    a.clock.advance_secs(50 * 86_400);
    let report = maintenance::compact(&a.state).await;
    assert!(report.events_compacted >= 4, "{report:?}");
    assert_eq!(a.sql_count("SELECT count(*) FROM events"), 0);
    assert_eq!(b.sql_count("SELECT count(*) FROM events"), 0);
}

#[tokio::test]
async fn expired_sessions_and_channels_leave_the_routing() {
    fn live_token(env: &Env, hash: &[u8; 32]) -> bool {
        matches!(env.state.spaces.route_token(hash), Route::Live(_))
    }
    let (a, _b, da, _db) = two_spaces(1).await;
    let token_hash = tomi_relay::wire::sha256(da[0].tok().as_bytes());
    let ch = a
        .post(&da[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let id = ch["channel_id"].as_str().unwrap().to_owned();
    assert!(live_token(&a, &token_hash));
    assert!(a.state.spaces.by_channel(&id).is_some());
    a.clock.advance_secs(3 * 86_400);
    maintenance::run_once(&a.state).await;
    assert!(!live_token(&a, &token_hash));
    assert!(a.state.spaces.by_channel(&id).is_none());
    assert!(!a.state.spaces.channel_taken(&id));
    // A logout takes the token out at once.
    let mut d = clone_dev(&da[0]);
    a.login(&mut d, 1, 1).await;
    let hash = tomi_relay::wire::sha256(d.tok().as_bytes());
    assert!(live_token(&a, &hash));
    a.delete(&d, "/v1/auth/session").await.ok(204);
    assert!(!live_token(&a, &hash));
}

// ===== restart, routing, migration =====================================================

#[tokio::test]
async fn the_routing_is_rebuilt_from_the_space_databases_at_every_start() {
    let (a, b, da, db) = two_spaces(2).await;
    let ch = a
        .post(&da[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let channel = ch["channel_id"].as_str().unwrap().to_owned();
    let space_b = b.space_id.clone();
    let space_a = a.space_id.clone();
    // (no handle on the old relay may outlive the restart: the files are replaced below)
    drop(b);

    let a = a.restart().await;
    let b = a.for_space(&space_b);
    assert_eq!(a.space_id, space_a);
    // Sessions, devices, channels route to their space again.
    a.get(&da[1], "/v1/space").await.ok(200);
    b.get(&db[1], "/v1/space").await.ok(200);
    assert_eq!(a.challenge(&da[0].id).await.status, 200);
    a.send(
        "POST",
        &format!("/v1/pairing/channels/{channel}/claim"),
        &[],
        Some(json!({})),
    )
    .await
    .ok(200);
    assert_eq!(a.state.spaces.list().len(), 2);

    // Even without routing.db (only invitations and quotas live there).
    a.state.spaces.close_all();
    for name in ["routing.db", "routing.db-wal", "routing.db-shm"] {
        let _ = std::fs::remove_file(a.data_dir().join(name));
    }
    let a = a.restart().await;
    let b = a.for_space(&space_b);
    a.get(&da[1], "/v1/space").await.ok(200);
    b.get(&db[1], "/v1/space").await.ok(200);
    let mut d = clone_dev(&da[0]);
    a.login(&mut d, 1, 1).await;
}

#[tokio::test]
async fn a_relay_whose_spaces_share_an_identifier_does_not_start() {
    let (a, b, da, _db) = two_spaces(1).await;
    a.state.spaces.close_all();
    let dir = a.data_dir().to_path_buf();
    // Put A's device in B's database too, as a careless restore could.
    let conn = rusqlite::Connection::open(b.space().dir.join("relay.db")).unwrap();
    conn.execute(
        "INSERT INTO devices (device_id, name, platform, issuer_device_id, joined_seq, registered_at)
         VALUES (?1, 'copy', 'windows', ?1, 0, 0)",
        [&da[0].id],
    )
    .unwrap();
    drop(conn);
    let err = tomi_relay::spaces::Spaces::open(&dir).err().unwrap();
    let text = format!("{err:#}");
    assert!(text.contains("two spaces"), "{text}");
    assert!(
        !text.contains(&da[0].id),
        "no identifier in the message: {text}"
    );
}

#[tokio::test]
async fn the_single_space_layout_is_migrated_at_start() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let blob = env.upload_blob(&devs[0], &[5u8; 600], 600).await;
    let (item, r) = env.send_item(&devs[0], &[&blob]).await;
    r.ok(201);
    env.state.spaces.close_all();

    // Back to the layout of the single-space relay.
    let root = env.data_dir().to_path_buf();
    let sdir = root.join("spaces").join(&env.space_id);
    for suffix in ["", "-wal", "-shm"] {
        let from = sdir.join(format!("relay.db{suffix}"));
        if from.exists() {
            std::fs::rename(from, root.join(format!("tomi.db{suffix}"))).unwrap();
        }
    }
    std::fs::rename(sdir.join("blobs"), root.join("blobs")).unwrap();
    std::fs::remove_dir_all(root.join("spaces")).unwrap();
    for name in ["routing.db", "routing.db-wal", "routing.db-shm"] {
        let _ = std::fs::remove_file(root.join(name));
    }
    assert!(root.join("tomi.db").is_file() && root.join("blobs").is_dir());

    let env = env.restart().await;
    assert!(!root.join("tomi.db").exists());
    assert!(!root.join("blobs").exists());
    assert!(!root.join("spaces.migrating").exists());
    assert_eq!(env.space().dir, sdir);
    assert!(sdir.join("relay.db").is_file() && sdir.join("blobs").is_dir());
    // Nothing was lost: the session, the item, the blob.
    env.get(&devs[1], "/v1/space").await.ok(200);
    env.get(&devs[1], &format!("/v1/items/{item}"))
        .await
        .ok(200);
    assert_eq!(env.download(&devs[1], &blob, None).await.body.len(), 600);
    // And the migrated space is a space like another: a second one comes next to it.
    let other = env.another_space().await;
    other.space_of(1).await;
    assert_eq!(env.state.spaces.list().len(), 2);
}

#[tokio::test]
async fn an_unclear_data_directory_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("spaces.migrating")).unwrap();
    let err = open(Config::for_tests(dir.path()), Options::default())
        .await
        .err()
        .unwrap();
    assert!(format!("{err:#}").contains("interrupted"), "{err:#}");
}

// ===== administration ====================================================================

#[tokio::test]
async fn admin_commands_take_a_space_and_the_relay_has_a_global_view() {
    let (a, b, da, db) = two_spaces(2).await;
    a.create_blob(&da[0], &new_uuid(), 700).await.ok(201);
    b.create_blob(&db[0], &new_uuid(), 300).await.ok(201);
    run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "5000".into(),
        },
    )
    .await;

    // `spaces`
    let out = run(&a, Cmd::Spaces { json: true }).await;
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    let rows = v["spaces"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = |env: &Env| {
        rows.iter()
            .find(|r| r["space_id"] == env.space_id)
            .unwrap()
            .clone()
    };
    assert_eq!(row(&a)["used_bytes"], 700);
    assert_eq!(row(&a)["max_bytes"], 5000);
    assert_eq!(row(&a)["members"], 2);
    assert_eq!(row(&b)["used_bytes"], 300);
    assert!(row(&b)["max_bytes"].is_null());
    let text = run(&a, Cmd::Spaces { json: false }).await.stdout;
    assert!(
        text.contains(&a.space_id) && text.contains(&b.space_id),
        "{text}"
    );

    // `status`: global without --space, one space with it.
    let global: Value = serde_json::from_str(
        &run(
            &a,
            Cmd::Status {
                json: true,
                space: None,
            },
        )
        .await
        .stdout,
    )
    .unwrap();
    assert_eq!(global["spaces"], 2);
    assert_eq!(global["quota"]["used_bytes"], 1000);
    assert_eq!(global["quota"]["max_bytes"], 1_073_741_824u64);
    assert!(global["disk_free_bytes"].as_u64().unwrap() > 0);
    assert_eq!(global["invitations_pending"], 0);
    let one: Value = serde_json::from_str(
        &run(
            &a,
            Cmd::Status {
                json: true,
                space: Some(a.space_id.clone()),
            },
        )
        .await
        .stdout,
    )
    .unwrap();
    assert_eq!(one["space_id"], a.space_id);
    assert_eq!(one["quota"]["space_max_bytes"], 5000);
    assert_eq!(one["quota"]["used_bytes"], 700);

    // `devices` and `revoke` need to know which space.
    let devices = |space: Option<String>| Cmd::Devices { json: true, space };
    let out = run(&a, devices(None)).await;
    assert_eq!(out.exit, 2);
    assert!(out.stderr.contains(&a.space_id) && out.stderr.contains(&b.space_id));
    let listed: Value = serde_json::from_str(&run(&a, devices(prefix(&b))).await.stdout).unwrap();
    assert_eq!(
        ids(&listed),
        HashSet::from([db[0].id.clone(), db[1].id.clone()])
    );
    // A device of A is not found in B.
    let out = run(
        &a,
        Cmd::Revoke {
            target: da[1].id.clone(),
            space: prefix(&b),
        },
    )
    .await;
    assert_eq!(out.exit, 1);
    a.get(&da[1], "/v1/space").await.ok(200);
    let out = run(
        &a,
        Cmd::Revoke {
            target: da[1].id.clone(),
            space: prefix(&a),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    a.get(&da[1], "/v1/space").await.err(403, "revoked");
    b.get(&db[1], "/v1/space").await.ok(200);

    // `invite`: one invitation per person to come.
    let out = run(
        &a,
        Cmd::Invite {
            rotate: false,
            new: true,
            cancel: false,
        },
    )
    .await;
    let token = out.stdout.trim().to_owned();
    let c = a.for_space(&new_uuid());
    c.create_space_raw(&Dev::new("third"), &token).await.ok(201);
    assert_eq!(a.state.spaces.list().len(), 3);
    let none = run(
        &a,
        Cmd::Invite {
            rotate: false,
            new: false,
            cancel: false,
        },
    )
    .await;
    assert!(
        none.stdout.contains("no pending invitation"),
        "{}",
        none.stdout
    );
}

#[tokio::test]
async fn a_relay_with_one_space_needs_no_space_argument() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let out = run(
        &env,
        Cmd::Devices {
            json: true,
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let out = run(
        &env,
        Cmd::Quota {
            space: None,
            value: "123456".into(),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(
        env.get(&devs[0], "/v1/space").await.ok(200)["quota"]["max_bytes"],
        123_456
    );
    let out = run(
        &env,
        Cmd::Revoke {
            target: devs[1].id.clone(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
}

// ===== the default quota of a space ==========================================================

async fn space_json(env: &Env, d: &Dev) -> Value {
    env.get(d, "/v1/space").await.ok(200)["quota"].clone()
}

/// Nothing is set (no web panel value, no `max_space_bytes`): there is NO limit per space (the
/// behaviour of production); only the total of the relay (1 Gio in the tests) bounds a space.
#[tokio::test]
async fn nothing_set_means_no_limit_per_space_only_the_total_of_the_relay_applies() {
    let (a, b, da, db) = two_spaces_with(1, |_, _| {}).await;
    assert_eq!(a.state.cfg.max_space_bytes, None);
    assert_eq!(
        a.state.spaces.default_space_quota(&a.state.cfg),
        (None, "none")
    );
    assert_eq!(space_json(&a, &da[0]).await["max_bytes"], 1_073_741_824u64);
    assert_eq!(space_json(&b, &db[0]).await["max_bytes"], 1_073_741_824u64);
    // A goes past 256 Mio (eleven blobs of 26 000 000 bytes): no quota of its own refuses it.
    for _ in 0..11 {
        a.create_blob(&da[0], &new_uuid(), 26_000_000).await.ok(201);
    }
    assert_eq!(space_json(&a, &da[0]).await["used_bytes"], 286_000_000);
    // B sees what is left of the total, nothing more.
    assert_eq!(
        space_json(&b, &db[0]).await["max_bytes"],
        1_073_741_824u64 - 286_000_000
    );
    // The listing of the administrator: `max_bytes: null` (no limit), the quota is the default's.
    let out = run(&a, Cmd::Spaces { json: true }).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let rows: Value = serde_json::from_str(&out.stdout).unwrap();
    for row in rows["spaces"].as_array().unwrap() {
        assert!(row["max_bytes"].is_null(), "{row}");
        assert_eq!(row["quota_source"], "default");
    }
}

#[tokio::test]
async fn the_configuration_sets_the_default_and_zero_means_no_limit() {
    let a = Env::with(|cfg, _| cfg.max_space_bytes = Some(5_000_000)).await;
    let (da, _) = a.space_of(1).await;
    assert_eq!(space_json(&a, &da[0]).await["max_bytes"], 5_000_000);
    assert_eq!(
        a.state.spaces.default_space_quota(&a.state.cfg),
        (Some(5_000_000), "config")
    );
    let b = Env::with(|cfg, _| cfg.max_space_bytes = Some(0)).await;
    let (db, _) = b.space_of(1).await;
    // No limit of its own: what is left of the total (1 Gio) is the ceiling.
    assert_eq!(space_json(&b, &db[0]).await["max_bytes"], 1_073_741_824u64);
    assert_eq!(
        b.state.spaces.default_space_quota(&b.state.cfg),
        (None, "none"),
        "0 is the same as nothing: no limit"
    );
}

#[tokio::test]
async fn changing_the_default_applies_at_once_wins_over_the_configuration_and_survives_a_restart() {
    let (a, b, da, db) = two_spaces_with(1, |cfg, _| cfg.max_space_bytes = Some(5_000_000)).await;
    let spaces = &a.state.spaces;
    let cfg = &a.state.cfg;
    assert_eq!(spaces.default_space_quota(cfg), (Some(5_000_000), "config"));
    a.create_blob(&da[0], &new_uuid(), 4_000_000).await.ok(201);

    // 1 Mio: A (4 MB used) can no longer add, B can still add under 1 Mio; no restart.
    spaces
        .set_default_quota(Some(Some(1_048_576)), a.clock.now_ms())
        .unwrap();
    assert_eq!(spaces.default_space_quota(cfg), (Some(1_048_576), "panel"));
    assert_eq!(space_json(&a, &da[0]).await["max_bytes"], 1_048_576);
    a.create_blob(&da[0], &new_uuid(), 10)
        .await
        .err(507, "relay_full");
    b.create_blob(&db[0], &new_uuid(), 1_000_000).await.ok(201);
    b.create_blob(&db[0], &new_uuid(), 100_000)
        .await
        .err(507, "relay_full");

    // A quota of its own (the CLI) wins over the default, whatever the default.
    let set = run(
        &a,
        Cmd::Quota {
            space: prefix(&a),
            value: "9000000".into(),
        },
    )
    .await;
    assert_eq!(set.exit, 0, "{}", set.stderr);
    a.create_blob(&da[0], &new_uuid(), 4_000_000).await.ok(201);
    assert_eq!(space_json(&a, &da[0]).await["max_bytes"], 9_000_000);
    let rows: Value =
        serde_json::from_str(&run(&a, Cmd::Spaces { json: true }).await.stdout).unwrap();
    let source = |env: &Env| {
        rows["spaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["space_id"].as_str().unwrap() == env.space_id)
            .unwrap()["quota_source"]
            .clone()
    };
    assert_eq!(source(&a), "space");
    assert_eq!(source(&b), "default");

    // No limit per space: only the total remains. The panel's value outlives a restart.
    spaces
        .set_default_quota(Some(None), a.clock.now_ms())
        .unwrap();
    assert_eq!(spaces.default_space_quota(cfg), (None, "panel"));
    assert_eq!(
        space_json(&b, &db[0]).await["max_bytes"],
        1_073_741_824u64 - 8_000_000
    );
    spaces
        .set_default_quota(Some(Some(2_000_000)), a.clock.now_ms())
        .unwrap();
    let a = a.restart().await;
    assert_eq!(
        a.state.spaces.default_space_quota(&a.state.cfg),
        (Some(2_000_000), "panel")
    );
    // Reset: the configuration again (the restarted relay has the tests' configuration: nothing,
    // so no limit).
    a.state
        .spaces
        .set_default_quota(None, a.clock.now_ms())
        .unwrap();
    assert_eq!(
        a.state.spaces.default_space_quota(&a.state.cfg),
        (None, "none")
    );
    let a = a.restart().await;
    assert_eq!(
        a.state.spaces.panel_default_quota(),
        None,
        "the row is gone"
    );
}

#[tokio::test]
async fn the_status_of_the_relay_shows_the_default_quota_and_its_origin() {
    let a = Env::with(|_, _| {}).await;
    let out = run(
        &a,
        Cmd::Status {
            json: true,
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(v["default_space_quota"]["bytes"].is_null(), "no limit");
    assert_eq!(v["default_space_quota"]["source"], "none");
    assert!(v["default_space_quota"].get("builtin_bytes").is_none());
    let out = run(
        &a,
        Cmd::Status {
            json: false,
            space: None,
        },
    )
    .await;
    assert!(
        out.stdout
            .contains("default quota of a space: no limit (from none)"),
        "{}",
        out.stdout
    );
    a.state
        .spaces
        .set_default_quota(Some(Some(7_340_032)), a.clock.now_ms())
        .unwrap();
    let out = run(
        &a,
        Cmd::Status {
            json: false,
            space: None,
        },
    )
    .await;
    assert!(
        out.stdout
            .contains("default quota of a space: 7340032 bytes (from panel)"),
        "{}",
        out.stdout
    );
}

#[tokio::test]
async fn the_default_quota_table_is_added_to_an_older_routing_database() {
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = rusqlite::Connection::open(dir.path().join("routing.db")).unwrap();
        conn.execute_batch(tomi_relay::db::ROUTING_SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let env = Env::in_dir(dir, |_, _| {}).await;
    let (version, table): (i64, i64) = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok((
                tx.query_row("PRAGMA user_version", [], |r| r.get(0))?,
                tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name = 'default_space_quota'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!((version, table), (1, 1));
    assert_eq!(env.state.spaces.panel_default_quota(), None);
}
