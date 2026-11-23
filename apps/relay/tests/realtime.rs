//! `api-v1.md` §11.7 (WebSocket doorbell, ring decision, push) and the restart scenario of
//! §11.6 (A61).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use tomi_relay::fcm::{HttpPost, HttpReply};

async fn register_push(env: &Env, d: &Dev, token: &str) {
    env.put(
        d,
        "/v1/me/push",
        json!({ "provider": "fcm", "token": token }),
    )
    .await
    .ok(204);
}

async fn set_present(env: &Env, d: &Dev, present: bool) {
    env.put(d, "/v1/me/presence", json!({ "present": present }))
        .await
        .ok(204);
}

/// A boolean member (`present`, `push`, `active`) of a device in `GET /v1/devices`.
async fn device_flag(env: &Env, viewer: &Dev, target: &str, flag: &str) -> bool {
    let list = env.get(viewer, "/v1/devices").await.ok(200);
    list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["device_id"] == target)
        .unwrap()[flag]
        .as_bool()
        .unwrap()
}

fn ring_of(events: &[Value], item_id: &str) -> bool {
    events
        .iter()
        .find(|e| e["type"] == "item" && e["data"]["item_id"] == item_id)
        .unwrap()["data"]["ring"]
        .as_bool()
        .unwrap()
}

// ===== WebSocket ==================================================================

#[tokio::test]
async fn a70_no_authentication_message_closes_the_socket() {
    let env = Env::new().await;
    let _ = env.space_of(1).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::connect(addr).await;
    // Nothing is sent: the relay closes with 4001 once the (reduced) delay has run.
    assert_eq!(ws.recv_close(3000).await, Some(4001));
    let _ = stop.send(());
}

#[tokio::test]
async fn a71_ready_then_a_wake_for_each_new_event() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::connect(addr).await;
    let ready = ws.auth(devs[1].tok()).await;
    assert_eq!(ready["type"], "ready");
    assert_eq!(ready["head_seq"].as_i64().unwrap(), env.head_seq().await);
    assert!(ready["session_expires_at"].is_string());

    for _ in 0..3 {
        let (_, r) = env.send_item(&devs[0], &[]).await;
        let seq = r.ok(201)["seq"].as_i64().unwrap();
        let wake = ws.recv_json(1000).await.unwrap();
        assert_eq!(wake["type"], "wake");
        assert!(wake["seq"].as_i64().unwrap() >= seq);
        assert!(wake["ring"].is_boolean());
    }
    // An event of its own device does not wake it; a text message from the client is ignored.
    ws.send_json(json!({ "type": "hello" })).await;
    env.send_item(&devs[1], &[]).await.1.ok(201);
    assert!(ws.recv_json(300).await.is_none());
    let _ = stop.send(());
}

#[tokio::test]
async fn a72_the_url_never_authenticates() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::connect_path(addr, &format!("/v1/ws?token={}", devs[0].tok())).await;
    // No `ready`: the socket closes with 4001 once the delay has run.
    let m = ws.recv(3000).await;
    match m {
        Some(WsMessage::Close(Some(f))) => assert_eq!(u16::from(f.code), 4001),
        other => panic!("expected a close with 4001, got {other:?}"),
    }
    let _ = stop.send(());
}

#[tokio::test]
async fn ws_protocol_errors_and_session_states() {
    let env = Env::with(|cfg, _| cfg.session_ttl_secs = 2).await;
    let (devs, ts) = env.space_of(2).await;
    let (addr, stop, _server) = serve(&env).await;

    // An unknown token: 4001.
    let mut ws = Ws::connect(addr).await;
    ws.send_json(json!({ "type": "auth", "token": "tms1.nope" }))
        .await;
    assert_eq!(ws.recv_close(1000).await, Some(4001));
    // Not an auth message: 4000. Binary: 4000.
    let mut ws = Ws::connect(addr).await;
    ws.send_json(json!({ "type": "something" })).await;
    assert_eq!(ws.recv_close(1000).await, Some(4000));
    let mut ws = Ws::connect(addr).await;
    {
        use futures_util::SinkExt;
        ws.s.send(WsMessage::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
    }
    assert_eq!(ws.recv_close(1000).await, Some(4000));
    // Too large after ready: 4000.
    let mut ws = Ws::open(addr, &devs[0]).await;
    ws.send_json(json!({ "type": "x", "pad": "y".repeat(5000) }))
        .await;
    assert_eq!(ws.recv_close(1000).await, Some(4000));

    // The session ends: 4002 (the session lives 2 seconds here).
    let mut ws = Ws::open(addr, &devs[1]).await;
    let closed = ws.recv_close(5000).await;
    assert_eq!(closed, Some(4002));
    // An expired token: 4002.
    env.clock.advance_secs(10);
    let mut ws = Ws::connect(addr).await;
    ws.send_json(json!({ "type": "auth", "token": devs[1].tok() }))
        .await;
    assert_eq!(ws.recv_close(1000).await, Some(4002));
    // A revoked device's token: 4003.
    let mut a = same_device(&devs[0]);
    env.login(&mut a, 1, 1).await;
    let (rot, _) = env.rotation_for(&ts, &a, &[&devs[1]]);
    env.post_rotation(&a, &rot).await.ok(201);
    let mut ws = Ws::connect(addr).await;
    ws.send_json(json!({ "type": "auth", "token": devs[1].tok() }))
        .await;
    assert_eq!(ws.recv_close(1000).await, Some(4003));
    let _ = stop.send(());
}

#[tokio::test]
async fn ws_a_fourth_connection_replaces_the_oldest() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut first = Ws::open(addr, &devs[0]).await;
    let _second = Ws::open(addr, &devs[0]).await;
    let _third = Ws::open(addr, &devs[0]).await;
    let _fourth = Ws::open(addr, &devs[0]).await;
    assert_eq!(first.recv_close(1000).await, Some(4008));
    assert_eq!(env.state.hub.connection_count(&devs[0].id), 3);
    let _ = stop.send(());
}

#[tokio::test]
async fn ws_the_relay_pings() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::open(addr, &devs[0]).await;
    let mut pings = 0;
    for _ in 0..4 {
        if let Some(WsMessage::Ping(_)) = ws.recv(1000).await {
            pings += 1;
        }
    }
    assert!(pings >= 2, "pings: {pings}");
    let _ = stop.send(());
}

// ===== ring decision =============================================================

#[tokio::test]
async fn a73_only_the_present_device_rings() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let (phone, pc1, pc2) = (&devs[0], &devs[1], &devs[2]);
    set_present(&env, pc1, true).await;
    let (addr, stop, _server) = serve(&env).await;
    let mut ws1 = Ws::open(addr, pc1).await;
    let mut ws2 = Ws::open(addr, pc2).await;
    let (id, r) = env.send_item(phone, &[]).await;
    r.ok(201);
    assert!(ring_of(&env.all_events(pc1).await, &id));
    assert!(!ring_of(&env.all_events(pc2).await, &id));
    assert_eq!(ws1.recv_json(1000).await.unwrap()["ring"], true);
    assert_eq!(ws2.recv_json(1000).await.unwrap()["ring"], false);
    let _ = stop.send(());
}

#[tokio::test]
async fn a74_nobody_present_everybody_rings_but_the_sender() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let (id, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    assert!(ring_of(&env.all_events(&devs[1]).await, &id));
    assert!(ring_of(&env.all_events(&devs[2]).await, &id));
    assert!(
        !ring_of(&env.all_events(&devs[0]).await, &id),
        "the sender never rings"
    );
    // The presence of the sender does not count.
    set_present(&env, &devs[0], true).await;
    let (id2, r) = env.send_item(&devs[0], &[]).await;
    r.ok(201);
    assert!(ring_of(&env.all_events(&devs[1]).await, &id2));
}

#[tokio::test]
async fn a75_presence_expires_after_two_minutes() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    assert!(!device_flag(&env, &devs[0], &devs[1].id, "present").await);
    set_present(&env, &devs[1], true).await;
    assert!(device_flag(&env, &devs[0], &devs[1].id, "present").await);
    env.clock.advance_secs(119);
    assert!(device_flag(&env, &devs[0], &devs[1].id, "present").await);
    env.clock.advance_secs(2);
    assert!(
        !device_flag(&env, &devs[0], &devs[1].id, "present").await,
        "121 seconds without a refresh"
    );
    // A refresh extends it; `false` clears it.
    set_present(&env, &devs[1], true).await;
    assert!(device_flag(&env, &devs[0], &devs[1].id, "present").await);
    set_present(&env, &devs[1], false).await;
    assert!(!device_flag(&env, &devs[0], &devs[1].id, "present").await);
    // At most 12 a minute (default limits).
    let env =
        Env::with(|cfg, _| cfg.rl_device_presence = Config::default().rl_device_presence).await;
    let (devs, _) = env.space_of(1).await;
    for _ in 0..4 {
        env.put(&devs[0], "/v1/me/presence", json!({ "present": true }))
            .await
            .ok(204);
    }
    env.put(&devs[0], "/v1/me/presence", json!({ "present": true }))
        .await
        .err(429, "rate_limited");
}

// ===== push ======================================================================

#[tokio::test]
async fn a76_push_goes_to_sleeping_devices_only_and_by_urgency() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    let (a, b, c) = (&devs[0], &devs[1], &devs[2]);
    register_push(&env, a, "tok-A").await;
    register_push(&env, b, "tok-B").await;

    // An item that must ring B: one wake-up, HIGH.
    let (id, r) = env.send_item(a, &[]).await;
    r.ok(201);
    env.settle(200).await;
    assert_eq!(env.push.calls(), vec![("tok-B".to_owned(), Urgency::High)]);

    // C acknowledges: the sender hears about it, but an ack never goes through push.
    env.settle(600).await;
    env.push.clear();
    env.post(c, &format!("/v1/items/{id}/ack"), json!({}))
        .await
        .ok(200);
    env.settle(800).await;
    assert_eq!(env.push.count(), 0, "no push for an ack");

    // C reads: A and B get a NORMAL wake-up.
    env.post(c, &format!("/v1/items/{id}/read"), json!({}))
        .await
        .ok(200);
    env.settle(300).await;
    let mut calls = env.push.calls();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("tok-A".to_owned(), Urgency::Normal),
            ("tok-B".to_owned(), Urgency::Normal)
        ]
    );

    // With a WebSocket open, no push at all.
    env.settle(700).await;
    env.push.clear();
    let (addr, stop, _server) = serve(&env).await;
    let mut ws = Ws::open(addr, b).await;
    env.send_item(a, &[]).await.1.ok(201);
    assert_eq!(ws.recv_json(1000).await.unwrap()["type"], "wake");
    env.settle(300).await;
    assert!(
        env.push.calls().iter().all(|(t, _)| t != "tok-B"),
        "{:?}",
        env.push.calls()
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn a77_a_burst_makes_two_pushes_at_most() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    register_push(&env, &devs[1], "tok-B").await;
    for _ in 0..5 {
        env.send_item(&devs[0], &[]).await.1.ok(201);
    }
    env.settle(1300).await;
    let calls = env.push.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(calls.iter().all(|(_, u)| *u == Urgency::High));
}

#[tokio::test]
async fn a77b_merged_wakeups_keep_the_highest_urgency() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(3).await;
    register_push(&env, &devs[1], "tok-B").await;
    // First: an item that does not ring B (C is present), then one that does.
    set_present(&env, &devs[2], true).await;
    env.send_item(&devs[0], &[]).await.1.ok(201); // B: NORMAL, sent at once
    set_present(&env, &devs[1], true).await;
    env.send_item(&devs[0], &[]).await.1.ok(201); // B: HIGH, merged
    env.send_item(&devs[0], &[]).await.1.ok(201); // B: HIGH, merged
    env.settle(1200).await;
    let calls = env.push.calls();
    assert_eq!(
        calls.iter().map(|(_, u)| *u).collect::<Vec<_>>(),
        vec![Urgency::Normal, Urgency::High],
        "{calls:?}"
    );
}

#[tokio::test]
async fn a78_a_dead_token_is_removed() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    register_push(&env, &devs[1], "tok-dead").await;
    assert!(device_flag(&env, &devs[0], &devs[1].id, "push").await);
    env.push.push_result(Err(PushError::Unregistered));
    env.send_item(&devs[0], &[]).await.1.ok(201);
    env.settle(300).await;
    assert!(
        !device_flag(&env, &devs[0], &devs[1].id, "push").await,
        "push = false once FCM answered UNREGISTERED"
    );
    assert_eq!(env.sql_count("SELECT count(*) FROM push_registrations"), 0);
    // The token never appears in an answer.
    let devices = env.get(&devs[0], "/v1/devices").await;
    assert!(!String::from_utf8_lossy(&devices.body).contains("tok-dead"));
    // Registering again, and replacing the token of another device (reinstall).
    register_push(&env, &devs[1], "tok-new").await;
    register_push(&env, &devs[0], "tok-new").await;
    assert_eq!(env.sql_count("SELECT count(*) FROM push_registrations"), 1);
    env.delete(&devs[0], "/v1/me/push").await.ok(204);
    env.delete(&devs[0], "/v1/me/push").await.ok(204);
    env.put(
        &devs[0],
        "/v1/me/push",
        json!({ "provider": "unifiedpush", "token": "x" }),
    )
    .await
    .err(422, "validation_failed");
    env.put(
        &devs[0],
        "/v1/me/push",
        json!({ "provider": "fcm", "token": "" }),
    )
    .await
    .err(422, "validation_failed");
    // An Apple device: its token is hexadecimal, for the production or the development endpoint.
    let apple = "a1".repeat(32);
    for provider in ["apns", "apns-sandbox"] {
        env.put(
            &devs[0],
            "/v1/me/push",
            json!({ "provider": provider, "token": apple }),
        )
        .await
        .ok(204);
    }
    assert_eq!(
        env.sql_count("SELECT count(*) FROM push_registrations WHERE provider = 'apns-sandbox'"),
        1
    );
    // It ends in the path of the request to Apple: nothing but hexadecimal.
    for bad in ["tok-new", "../3/device/x", "a1a1"] {
        env.put(
            &devs[0],
            "/v1/me/push",
            json!({ "provider": "apns", "token": bad }),
        )
        .await
        .err(422, "validation_failed");
    }
}

/// A recorded POST: `(url, headers, body)`.
type RecordedPost = (String, Vec<(String, String)>, Vec<u8>);

/// Records every POST the sender makes and answers like Google would.
#[derive(Default)]
struct FakeGoogle {
    posts: Mutex<Vec<RecordedPost>>,
}

#[async_trait]
impl HttpPost for FakeGoogle {
    async fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> Result<HttpReply, String> {
        self.posts.lock().unwrap().push((
            url.to_owned(),
            headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            body,
        ));
        let reply = if url.ends_with("/token") {
            json!({ "access_token": "ya29.fake-access-token", "expires_in": 3600, "token_type": "Bearer" })
        } else {
            json!({ "name": "projects/tomi-test/messages/1" })
        };
        Ok(HttpReply {
            status: 200,
            retry_after: None,
            body: reply.to_string().into_bytes(),
        })
    }
}

#[tokio::test]
async fn a79_the_message_sent_to_fcm_is_constant() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().join("service-account.json");
    std::fs::write(
        &account,
        rsa_key::service_account_json("https://oauth.test/token"),
    )
    .unwrap();
    let google = Arc::new(FakeGoogle::default());
    let g = google.clone();
    let env = Env::in_dir(dir, move |cfg, opts| {
        cfg.fcm_service_account_file = Some(account.clone());
        opts.push = PushChoice::FromConfig;
        opts.fcm_http = Some(g.clone());
    })
    .await;
    let (devs, _) = env.space_of(3).await;
    register_push(&env, &devs[1], "fcm-token-of-B").await;
    register_push(&env, &devs[2], "fcm-token-of-C").await;
    set_present(&env, &devs[2], true).await;
    // B must ring? No: C is present, so only C rings. B gets NORMAL, C gets HIGH.
    env.send_item(&devs[0], &[]).await.1.ok(201);
    env.settle(500).await;

    let posts = google.posts.lock().unwrap().clone();
    let tokens: Vec<_> = posts.iter().filter(|p| p.0.ends_with("/token")).collect();
    let sends: Vec<_> = posts
        .iter()
        .filter(|p| p.0.contains("/messages:send"))
        .collect();
    assert_eq!(sends.len(), 2, "{} posts", posts.len());
    assert_eq!(
        tokens.len(),
        1,
        "the OAuth token is obtained once and cached"
    );

    // The OAuth assertion: RS256 JWT with the documented claims, verified with the public key.
    let form: Vec<(String, String)> = form_urlencoded::parse(&tokens[0].2).into_owned().collect();
    assert_eq!(
        form[0],
        (
            "grant_type".into(),
            "urn:ietf:params:oauth:grant-type:jwt-bearer".into()
        )
    );
    let jwt = &form[1].1;
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    let header: Value = serde_json::from_slice(&unb64(parts[0])).unwrap();
    let claims: Value = serde_json::from_slice(&unb64(parts[1])).unwrap();
    assert_eq!(header, json!({ "alg": "RS256", "typ": "JWT" }));
    assert_eq!(claims["iss"], "relay@tomi-test.iam.gserviceaccount.com");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/firebase.messaging"
    );
    assert_eq!(claims["aud"], "https://oauth.test/token");
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        3600
    );
    let creds = tomi_relay::fcm::FcmCredentials::from_json(
        &rsa_key::service_account_json("https://oauth.test/token"),
        None,
    )
    .unwrap();
    let pk = ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        creds.public_key(),
    );
    pk.verify(
        format!("{}.{}", parts[0], parts[1]).as_bytes(),
        &unb64(parts[2]),
    )
    .unwrap();

    // The messages: the same shape for everybody, only `token` and `priority` differ.
    let mut bodies: Vec<Value> = sends
        .iter()
        .map(|p| serde_json::from_slice(&p.2).unwrap())
        .collect();
    bodies.sort_by_key(|b| b["message"]["token"].as_str().unwrap().to_owned());
    assert_eq!(bodies[0]["message"]["token"], "fcm-token-of-B");
    assert_eq!(bodies[0]["message"]["android"]["priority"], "NORMAL");
    assert_eq!(bodies[1]["message"]["token"], "fcm-token-of-C");
    assert_eq!(bodies[1]["message"]["android"]["priority"], "HIGH");
    for b in &bodies {
        assert_eq!(b["message"]["data"], json!({ "w": "1" }));
        assert_eq!(b["message"]["android"]["collapse_key"], "tomi");
        assert_eq!(b["message"]["android"]["ttl"], "86400s");
        assert_eq!(
            b["message"].as_object().unwrap().len(),
            3,
            "token, data, android: no notification block"
        );
    }
    for (url, headers, _) in &sends {
        assert_eq!(
            url,
            "https://fcm.googleapis.com/v1/projects/tomi-test/messages:send"
        );
        assert!(headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer ya29.fake-access-token"));
    }
    // Nothing that identifies the item, its type or the sender reaches Google.
    let all = sends
        .iter()
        .map(|p| String::from_utf8_lossy(&p.2).into_owned())
        .collect::<String>();
    assert!(!all.contains(&devs[0].id) && !all.contains("seq") && !all.contains("item"));
}

// ===== restart (A61) =============================================================

fn same_device(d: &Dev) -> Dev {
    Dev {
        keys: DeviceKeys::from_secrets(&d.keys.ed25519_seed(), &d.keys.x25519_secret()).unwrap(),
        id: d.id.clone(),
        name: d.name.clone(),
        token: d.token.clone(),
    }
}

/// What a device does after a wake-up: read the log from its cursor, remember the items.
async fn catch_up(env: &Env, d: &Dev, cursor: &mut i64, seen: &mut Vec<String>) {
    loop {
        let page = env.events(d, *cursor, 100).await;
        for e in page["events"].as_array().unwrap() {
            if e["type"] == "item" {
                seen.push(e["data"]["item_id"].as_str().unwrap().to_owned());
            }
        }
        *cursor = page["next_after"].as_i64().unwrap();
        if !page["has_more"].as_bool().unwrap() {
            return;
        }
    }
}

#[tokio::test]
async fn a61_a_restart_loses_nothing_and_duplicates_nothing() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (same_device(&devs[0]), same_device(&devs[1]));
    let (addr, stop, server) = serve(&env).await;
    let mut ws = Ws::open(addr, &b).await;

    // B's view: a cursor and the items it has seen.
    let mut cursor = env.head_seq().await;
    let mut seen: Vec<String> = Vec::new();

    // Item 1 before the restart: the doorbell rings and B reads it.
    let (i1, r) = env.send_item(&a, &[]).await;
    r.ok(201);
    assert_eq!(ws.recv_json(1000).await.unwrap()["type"], "wake");
    catch_up(&env, &b, &mut cursor, &mut seen).await;

    // The relay restarts: the sockets are closed with 1001 and the process state is gone.
    let _ = stop.send(());
    assert_eq!(ws.recv_close(2000).await, Some(1001));
    server.await.unwrap();
    let env = env.restart().await;

    // Items 2 and 3 arrive on the new process (the sessions live in the base).
    let (i2, r) = env.send_item(&a, &[]).await;
    r.ok(201);
    let (i3, r) = env.send_item(&a, &[]).await;
    r.ok(201);

    // B reconnects and catches up by the log.
    let (addr2, stop2, _server2) = serve(&env).await;
    let mut ws2 = Ws::open(addr2, &b).await;
    catch_up(&env, &b, &mut cursor, &mut seen).await;
    assert_eq!(seen, vec![i1, i2, i3], "three items, once each, in order");
    // And the doorbell works again.
    let (_, r) = env.send_item(&a, &[]).await;
    r.ok(201);
    assert_eq!(ws2.recv_json(1000).await.unwrap()["type"], "wake");
    let _ = stop2.send(());
}
