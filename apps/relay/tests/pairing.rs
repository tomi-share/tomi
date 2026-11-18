//! `api-v1.md` §11.10: the pairing rendezvous; §11.13: the rate limits of its `Pairing`
//! credential (§8.5); and a real session with the frames of the protocol of
//! `pairing-v1.md` (P31).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::Request;
use common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tomi_core::pairing::{
    Inviter, InviterEvent, InviterParams, Joiner, JoinerEvent, JoinerParams, PairingCode,
    MAX_FRAME_LEN,
};

async fn open_channel(env: &Env, inviter: &Dev) -> String {
    env.post(inviter, "/v1/pairing/channels", json!({}))
        .await
        .ok(201)["channel_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn claim(env: &Env, channel: &str) -> Resp {
    env.send(
        "POST",
        &format!("/v1/pairing/channels/{channel}/claim"),
        &[],
        Some(json!({})),
    )
    .await
}

async fn say(env: &Env, token: &str, scheme: &str, channel: &str, body: &[u8]) -> Resp {
    let auth = format!("{scheme} {token}");
    env.send(
        "POST",
        &format!("/v1/pairing/channels/{channel}/messages"),
        &[("authorization", auth.as_str())],
        Some(json!({ "body": b64(body) })),
    )
    .await
}

async fn listen(
    env: &Env,
    token: &str,
    scheme: &str,
    channel: &str,
    after: i64,
    wait: u64,
) -> Resp {
    let auth = format!("{scheme} {token}");
    env.send(
        "GET",
        &format!("/v1/pairing/channels/{channel}/messages?after={after}&wait={wait}"),
        &[("authorization", auth.as_str())],
        None,
    )
    .await
}

#[tokio::test]
async fn a100_a_member_opens_a_channel() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let r = env
        .post(&devs[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    let id = r["channel_id"].as_str().unwrap();
    assert!(
        (1..=4).contains(&id.len())
            && id.bytes().all(|b| b.is_ascii_digit())
            && !id.starts_with('0'),
        "{id}"
    );
    assert_eq!(r["state"], "open");
    assert_eq!(r["expires_at"], ts(T0 + 120_000));
    // At most two channels at a time.
    env.post(&devs[0], "/v1/pairing/channels", json!({}))
        .await
        .ok(201);
    env.post(&devs[0], "/v1/pairing/channels", json!({}))
        .await
        .err(409, "channel_limit");
    // A body that is not an object is refused.
    env.send(
        "POST",
        "/v1/pairing/channels",
        &[("authorization", &format!("Bearer {}", devs[0].tok()))],
        None,
    )
    .await
    .err(409, "channel_limit");
}

#[tokio::test]
async fn a101_one_claim_per_channel() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let first = claim(&env, &ch).await.ok(200);
    let token = first["pairing_token"].as_str().unwrap();
    assert!(token.starts_with("tmp1."), "{token}");
    assert_eq!(unb64(&token["tmp1.".len()..]).len(), 24);
    assert_eq!(first["expires_at"], ts(T0 + 120_000));
    claim(&env, &ch).await.err(409, "channel_claimed");
    // The token is stored as a hash only.
    assert_eq!(
        env.sql_count(&format!(
            "SELECT count(*) FROM pairing_channels WHERE token_hash = x'{}'",
            hex::encode(Sha256::digest(token.as_bytes()))
        )),
        1
    );
    // Unknown channels and malformed numbers.
    claim(&env, "9999").await.err(404, "not_found");
    claim(&env, "0").await.err(422, "validation_failed");
    claim(&env, "12345").await.err(422, "validation_failed");
}

#[tokio::test]
async fn a102_each_side_reads_only_the_other() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let a = &devs[0];
    let ch = open_channel(&env, a).await;
    // The inviter writes before the claim; the newcomer reads it afterwards.
    let bearer = a.tok().to_owned();
    assert_eq!(
        say(&env, &bearer, "Bearer", &ch, b"pake-1").await.ok(201)["index"],
        1
    );
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        say(&env, &token, "Pairing", &ch, b"pake-2").await.ok(201)["index"],
        2
    );
    assert_eq!(
        say(&env, &bearer, "Bearer", &ch, b"sealed-3").await.ok(201)["index"],
        3
    );

    let seen_by_joiner = listen(&env, &token, "Pairing", &ch, 0, 0).await.ok(200);
    assert_eq!(seen_by_joiner["state"], "claimed");
    let msgs = seen_by_joiner["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(
        (msgs[0]["index"].as_i64(), msgs[0]["side"].as_str()),
        (Some(1), Some("inviter"))
    );
    assert_eq!(unb64(msgs[0]["body"].as_str().unwrap()), b"pake-1");
    assert_eq!(msgs[1]["index"], 3);
    let seen_by_inviter = listen(&env, &bearer, "Bearer", &ch, 0, 0).await.ok(200);
    let msgs = seen_by_inviter["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(
        (msgs[0]["index"].as_i64(), msgs[0]["side"].as_str()),
        (Some(2), Some("joiner"))
    );
    // `after` moves the cursor.
    let rest = listen(&env, &token, "Pairing", &ch, 1, 0).await.ok(200);
    assert_eq!(rest["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a102b_a_long_poll_answers_as_soon_as_a_message_arrives() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let bearer = devs[0].tok().to_owned();
    let started = std::time::Instant::now();
    let (poll, _) = tokio::join!(listen(&env, &token, "Pairing", &ch, 0, 10), async {
        env.settle(300).await;
        say(&env, &bearer, "Bearer", &ch, b"hello").await.ok(201);
    });
    let body = poll.ok(200);
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "answered early"
    );
}

#[tokio::test]
async fn a102c_an_empty_long_poll_lasts_the_requested_time() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let bearer = devs[0].tok().to_owned();
    let started = std::time::Instant::now();
    let body = listen(&env, &bearer, "Bearer", &ch, 0, 10).await.ok(200);
    let took = started.elapsed();
    assert_eq!(body["messages"], json!([]));
    assert!(
        took >= std::time::Duration::from_secs(9) && took <= std::time::Duration::from_secs(11),
        "{took:?}"
    );
}

#[tokio::test]
async fn a102d_a_change_of_state_ends_the_wait() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let bearer = devs[0].tok().to_owned();
    let (poll, _) = tokio::join!(listen(&env, &bearer, "Bearer", &ch, 0, 10), async {
        env.settle(200).await;
        claim(&env, &ch).await.ok(200);
    });
    assert_eq!(poll.ok(200)["state"], "claimed");
    // The wait is capped at 25 seconds.
    listen(&env, &bearer, "Bearer", &ch, 0, 26)
        .await
        .err(422, "validation_failed");
}

#[tokio::test]
async fn a103_an_unclaimed_channel_dies_after_two_minutes() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    env.clock.advance_secs(121);
    claim(&env, &ch).await.err(404, "not_found");
    let mut again = Dev {
        keys: DeviceKeys::from_secrets(&devs[0].keys.ed25519_seed(), &devs[0].keys.x25519_secret())
            .unwrap(),
        id: devs[0].id.clone(),
        name: "x".into(),
        token: None,
    };
    env.login(&mut again, 1, 1).await;
    // Neither can the inviter use it.
    say(&env, again.tok(), "Bearer", &ch, b"x")
        .await
        .err(404, "not_found");
}

#[tokio::test]
async fn a104_message_limits() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let bearer = devs[0].tok().to_owned();
    for i in 0..8u8 {
        assert_eq!(
            say(&env, &bearer, "Bearer", &ch, &[i + 1]).await.ok(201)["index"],
            i64::from(i) + 1
        );
    }
    say(&env, &bearer, "Bearer", &ch, b"ninth")
        .await
        .err(409, "channel_limit");
    // The other side has its own allowance.
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    say(&env, &token, "Pairing", &ch, b"mine").await.ok(201);
    // Sizes.
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let bearer = devs[0].tok().to_owned();
    let r = say(&env, &bearer, "Bearer", &ch, &vec![1u8; 8193]).await;
    r.err(413, "payload_too_large");
    assert_eq!(r.json()["limit"], 8192);
    say(&env, &bearer, "Bearer", &ch, &vec![1u8; 8192])
        .await
        .ok(201);
    say(&env, &bearer, "Bearer", &ch, &[])
        .await
        .err(422, "validation_failed");
}

#[tokio::test]
async fn a105_the_inviter_registers_the_new_device() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let mut x = Dev::new("Pixel");
    let ch = open_channel(&env, a).await;
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let registered = env
        .post(
            a,
            "/v1/devices",
            json!({
                "channel_id": ch, "key_epoch": 1, "name": "Pixel", "platform": "android",
                "certificate": b64(&env.cert(&a.keys, &x.keys)),
            }),
        )
        .await
        .ok(201);
    assert_eq!(registered["device_id"], x.id);
    assert_eq!(registered["issuer_device_id"], a.id);
    assert_eq!(registered["name"], "Pixel");
    assert_eq!(registered["platform"], "android");
    assert_eq!(registered["active"], false);
    assert!(registered.get("last_seen_at").is_none());
    // The event, for the others.
    let added = env.events_of_type(b, "device_added").await;
    assert!(added
        .iter()
        .any(|e| e["data"]["device"]["device_id"] == x.id));
    // The channel is completed; the newcomer can still read the last message.
    env.post(
        a,
        &format!("/v1/pairing/channels/{ch}/messages"),
        json!({ "body": b64(b"late") }),
    )
    .await
    .err(409, "channel_closed");
    assert_eq!(
        listen(&env, &token, "Pairing", &ch, 0, 0).await.ok(200)["state"],
        "completed"
    );
    // Registering twice through the same channel is refused.
    env.post(
        a,
        "/v1/devices",
        json!({"channel_id": ch, "key_epoch": 1, "name": "Pixel", "platform": "android", "certificate": b64(&env.cert(&a.keys, &x.keys))}),
    )
    .await
    .err(409, "channel_closed");
    // X opens a session.
    env.login(&mut x, 1, 1).await;
    env.get(&x, "/v1/devices").await.ok(200);
    // The listing shows three members, X with its certificate.
    let list = env.get(&x, "/v1/devices").await.ok(200);
    assert_eq!(list["devices"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a106_the_registration_is_checked() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let (a, b) = (&devs[0], &devs[1]);
    let x = Dev::new("X");
    let ch = open_channel(&env, a).await;
    claim(&env, &ch).await.ok(200);
    let body = |cert: Vec<u8>, epoch: u32, channel: &str| json!({"channel_id": channel, "key_epoch": epoch, "name": "X", "platform": "android", "certificate": b64(&cert)});
    // A certificate issued by another device than the caller.
    let r = env
        .post(a, "/v1/devices", body(env.cert(&b.keys, &x.keys), 1, &ch))
        .await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/certificate");
    // A bad signature, another space.
    let mut bad = env.cert(&a.keys, &x.keys);
    let last = bad.len() - 1;
    bad[last] ^= 1;
    env.post(a, "/v1/devices", body(bad, 1, &ch))
        .await
        .err(422, "validation_failed");
    let foreign = a
        .keys
        .sign_device_cert(&new_uuid(), &x.keys.public(), 1)
        .unwrap()
        .to_bytes()
        .unwrap();
    env.post(a, "/v1/devices", body(foreign, 1, &ch))
        .await
        .err(422, "validation_failed");
    // Another member's channel.
    env.post(b, "/v1/devices", body(env.cert(&b.keys, &x.keys), 1, &ch))
        .await
        .err(404, "not_found");
    // An unknown channel.
    env.post(
        a,
        "/v1/devices",
        body(env.cert(&a.keys, &x.keys), 1, "9998"),
    )
    .await
    .err(404, "not_found");
    // The epoch must be the current one.
    let r = env
        .post(a, "/v1/devices", body(env.cert(&a.keys, &x.keys), 7, &ch))
        .await;
    r.err(409, "epoch_conflict");
    assert_eq!(r.json()["key_epoch"], 1);
    // A channel that is not `claimed`.
    let open = open_channel(&env, a).await;
    env.post(a, "/v1/devices", body(env.cert(&a.keys, &x.keys), 1, &open))
        .await
        .err(409, "channel_closed");
    // The device id is derived from the key.
    let good = env
        .post(a, "/v1/devices", body(env.cert(&a.keys, &x.keys), 1, &ch))
        .await
        .ok(201);
    let mut h = Sha256::new();
    h.update(b"tomi/v1/device-id");
    h.update([0u8]);
    h.update(x.pk());
    assert_eq!(good["device_id"], hex::encode(&h.finalize()[..16]));
    assert_eq!(good["sign_pk"], b64(&x.pk()));
}

#[tokio::test]
async fn a106b_limits_of_the_space() {
    let env = Env::with(|cfg, _| cfg.max_devices = 2).await;
    let (devs, _) = env.space_of(2).await;
    let x = Dev::new("X");
    let r = env.pair_raw(&devs[0], &x, 1).await;
    r.err(409, "too_many_devices");
    assert_eq!(r.json()["max_devices"], 2);
    // An existing member cannot be registered again either.
    let ch = open_channel(&env, &devs[0]).await;
    claim(&env, &ch).await.ok(200);
    env.post(
        &devs[0],
        "/v1/devices",
        json!({"channel_id": ch, "key_epoch": 1, "name": "again", "platform": "android", "certificate": b64(&env.cert(&devs[0].keys, &devs[1].keys))}),
    )
    .await
    .err(409, "device_exists");
}

#[tokio::test]
async fn a107_a_pairing_token_opens_no_other_door() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let ch = open_channel(&env, &devs[0]).await;
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let auth = format!("Pairing {token}");
    for (method, path) in [
        ("GET", "/v1/events?after=0"),
        ("GET", "/v1/space"),
        ("GET", "/v1/devices"),
        ("GET", "/v1/keys"),
    ] {
        env.send(method, path, &[("authorization", auth.as_str())], None)
            .await
            .err(401, "unauthorized");
    }
    // The same token as a Bearer credential is not a session either.
    env.bearer("GET", "/v1/space", &token, None)
        .await
        .err(401, "unauthorized");
    // A token of another channel does not open this one.
    let other = open_channel(&env, &devs[0]).await;
    let other_token = claim(&env, &other).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    say(&env, &other_token, "Pairing", &ch, b"x")
        .await
        .err(401, "unauthorized");
    // A session of another member is not the creator.
    env.send(
        "GET",
        &format!("/v1/pairing/channels/{ch}/messages"),
        &[("authorization", &format!("Bearer {}", devs[1].tok()))],
        None,
    )
    .await
    .err(404, "not_found");
}

#[tokio::test]
async fn a107b_closing_a_channel_erases_its_messages() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let bearer = devs[0].tok().to_owned();
    say(&env, &bearer, "Bearer", &ch, b"secret-ish")
        .await
        .ok(201);
    let token = claim(&env, &ch).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned();
    env.send(
        "DELETE",
        &format!("/v1/pairing/channels/{ch}"),
        &[("authorization", &format!("Pairing {token}"))],
        None,
    )
    .await
    .ok(204);
    let seen = listen(&env, &bearer, "Bearer", &ch, 0, 0).await.ok(200);
    assert_eq!(seen["state"], "closed");
    assert_eq!(seen["messages"], json!([]));
    say(&env, &bearer, "Bearer", &ch, b"more")
        .await
        .err(409, "channel_closed");
    assert_eq!(env.sql_count("SELECT count(*) FROM pairing_messages"), 0);
    // After the deadline the channel is gone.
    env.clock.advance_secs(300);
    tomi_relay::maintenance::run_once(&env.state).await;
    assert_eq!(env.sql_count("SELECT count(*) FROM pairing_channels"), 0);
}

// ===== reads of a finished channel (§4.6) =============================================

async fn claim_token(env: &Env, channel: &str) -> String {
    claim(env, channel).await.ok(200)["pairing_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn hang_up(env: &Env, token: &str, scheme: &str, channel: &str) -> Resp {
    let auth = format!("{scheme} {token}");
    env.send(
        "DELETE",
        &format!("/v1/pairing/channels/{channel}"),
        &[("authorization", auth.as_str())],
        None,
    )
    .await
}

/// A108: once the channel is `completed` or `closed` nothing can arrive any more, so a read
/// answers at once, whatever `wait` it asks for. A channel still in progress keeps waiting.
#[tokio::test]
async fn a108_a_read_of_a_finished_channel_answers_at_once() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let a = &devs[0];
    let bearer = a.tok().to_owned();
    // The reads below ask for 10 seconds: far above the time an in-process answer takes.
    let at_once = Duration::from_secs(3);

    // `closed`: the messages are erased and each side learns the state without waiting.
    let ch = open_channel(&env, a).await;
    say(&env, &bearer, "Bearer", &ch, b"first").await.ok(201);
    let token = claim_token(&env, &ch).await;
    hang_up(&env, &token, "Pairing", &ch).await.ok(204);
    for (cred, scheme) in [(&bearer, "Bearer"), (&token, "Pairing")] {
        let started = Instant::now();
        let seen = listen(&env, cred, scheme, &ch, 0, 10).await.ok(200);
        assert!(
            started.elapsed() < at_once,
            "{scheme}: {:?}",
            started.elapsed()
        );
        assert_eq!(seen["state"], "closed");
        assert_eq!(seen["messages"], json!([]));
    }

    // `completed`: the newcomer still reads what the inviter left, then nothing, at once.
    let x = Dev::new("X");
    let ch = open_channel(&env, a).await;
    say(&env, &bearer, "Bearer", &ch, b"last words")
        .await
        .ok(201);
    let token = claim_token(&env, &ch).await;
    env.post(
        a,
        "/v1/devices",
        json!({
            "channel_id": ch, "key_epoch": 1, "name": "X", "platform": "android",
            "certificate": b64(&env.cert(&a.keys, &x.keys)),
        }),
    )
    .await
    .ok(201);
    for (cred, scheme, after, messages) in [
        (&token, "Pairing", 0, 1),
        (&token, "Pairing", 1, 0),
        (&bearer, "Bearer", 0, 0),
    ] {
        let started = Instant::now();
        let seen = listen(&env, cred, scheme, &ch, after, 10).await.ok(200);
        assert!(
            started.elapsed() < at_once,
            "{scheme} after {after}: {:?}",
            started.elapsed()
        );
        assert_eq!(seen["state"], "completed");
        assert_eq!(seen["messages"].as_array().unwrap().len(), messages);
    }

    // A channel that is still in progress makes the reader wait: the rule is about finished
    // channels only (an empty read of a `claimed` channel lasts what it asked for).
    let ch = open_channel(&env, a).await;
    claim(&env, &ch).await.ok(200);
    let started = Instant::now();
    let seen = listen(&env, &bearer, "Bearer", &ch, 0, 1).await.ok(200);
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(seen["state"], "claimed");
}

// ===== rate limits of the `Pairing` credential (§8.5) ==================================
//
// A `Pairing` request has no session, so the per-device limit (step 5) never applies to it
// by itself. The relay reads the channel named by the URL to tell a token that is the
// channel's from any other: the first is not counted in `rl_ip_public` (it is counted per
// channel, like a device), the others are. `tests/ops.rs` has the original flood test.

/// A request from the TCP peer `ip` (not a trusted proxy: the address the limits see).
async fn from_ip(
    env: &Env,
    ip: &str,
    (method, path): (&str, &str),
    auth: Option<&str>,
    body: Option<Value>,
) -> Resp {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(auth) = auth {
        b = b.header("authorization", auth);
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let peer: SocketAddr = format!("{ip}:7000").parse().unwrap();
    env.call_from(peer, req).await
}

/// How many requests with a bogus `Pairing` token the address every `env.send` comes from can
/// still make before it is limited: what is left of its budget in `rl_ip_public`.
async fn ip_budget_left(env: &Env, channel: &str) -> usize {
    let bogus = format!("Pairing tmp1.{}", b64(&[7u8; 24]));
    let path = format!("/v1/pairing/channels/{channel}/messages");
    let mut n = 0;
    while env
        .send("GET", &path, &[("authorization", bogus.as_str())], None)
        .await
        .status
        != 429
    {
        n += 1;
        assert!(n < 100, "the address is never limited");
    }
    n
}

/// A132: the token of a channel is not counted in the budget of the address. Two identical
/// relays; only the first sees the newcomer at work, and both addresses end with the same
/// budget left.
#[tokio::test]
async fn a132_a_valid_pairing_token_is_not_counted_in_the_ip_limit() {
    let busy = Env::with(|cfg, _| real_limits(cfg)).await;
    let idle = Env::with(|cfg, _| real_limits(cfg)).await;
    let mut channels = Vec::new();
    for env in [&busy, &idle] {
        let (devs, _) = env.space_of(1).await;
        let ch = open_channel(env, &devs[0]).await;
        let token = claim_token(env, &ch).await;
        channels.push((ch, token));
    }
    let (ch, token) = &channels[0];
    // 19 requests of the newcomer, from the address that already made a few requests of the
    // kind that are counted (the space, the login of the inviter, the claim): the burst of
    // `rl_ip_public` is 10. The reads (9) and the messages (8) are within their own limits.
    for i in 0..8u8 {
        say(&busy, token, "Pairing", ch, &[i + 1]).await.ok(201);
    }
    say(&busy, token, "Pairing", ch, b"ninth")
        .await
        .err(409, "channel_limit");
    for _ in 0..8 {
        listen(&busy, token, "Pairing", ch, 0, 0).await.ok(200);
    }
    hang_up(&busy, token, "Pairing", ch).await.ok(204);
    assert_eq!(
        listen(&busy, token, "Pairing", ch, 0, 0).await.ok(200)["state"],
        "closed"
    );
    // None of it came out of the budget of the address.
    let left_idle = ip_budget_left(&idle, &channels[1].0).await;
    let left_busy = ip_budget_left(&busy, ch).await;
    assert!((1..10).contains(&left_idle), "{left_idle}");
    assert_eq!(left_busy, left_idle);
}

/// A133: a `Pairing` token that is not the channel's is counted per IP, whatever the way it
/// is not: the flood is stopped after the burst, and until then the route answers as usual.
#[tokio::test]
async fn a133_every_kind_of_invalid_pairing_token_is_limited_per_ip() {
    let env = Env::with(|cfg, _| {
        real_limits(cfg);
        cfg.pairing_max_open = 5;
    })
    .await;
    let (devs, _) = env.space_of(1).await;
    let a = &devs[0];
    let claimed = open_channel(&env, a).await;
    let claimed_token = claim_token(&env, &claimed).await;
    let other = open_channel(&env, a).await;
    let other_token = claim_token(&env, &other).await;
    let unclaimed = open_channel(&env, a).await;
    let unknown = (1..=9999u32)
        .map(|n| n.to_string())
        .find(|id| *id != claimed && *id != other && *id != unclaimed)
        .unwrap();
    let well_formed = format!("tmp1.{}", b64(&[9u8; 24]));

    // (what is wrong, channel in the URL, the header, what the route answers meanwhile)
    let kinds = [
        (
            "a token nobody was given",
            &claimed,
            format!("Pairing {well_formed}"),
            (401, "unauthorized"),
        ),
        (
            "the token of another channel",
            &claimed,
            format!("Pairing {other_token}"),
            (401, "unauthorized"),
        ),
        (
            "a channel nobody claimed",
            &unclaimed,
            format!("Pairing {claimed_token}"),
            (401, "unauthorized"),
        ),
        (
            "a channel that does not exist",
            &unknown,
            format!("Pairing {claimed_token}"),
            (404, "not_found"),
        ),
        (
            "something that is not a pairing token",
            &claimed,
            "Pairing not-a-token".to_owned(),
            (401, "unauthorized"),
        ),
    ];
    let mut n = 0;
    for (what, channel, auth, expected) in &kinds {
        for (method, path, body) in channel_routes(channel) {
            // One address for each flood: they do not share a budget.
            n += 1;
            let ip = format!("198.51.100.{n}");
            let what = format!("{method} with {what}");
            flood(&env, &ip, (method, &path), auth, body, *expected, &what).await;
        }
    }

    // A channel past its deadline: its token is not valid any more, whatever the row still
    // says until the relay cleans it up.
    env.clock.advance_secs(121);
    for (method, path, body) in channel_routes(&claimed) {
        n += 1;
        let ip = format!("198.51.100.{n}");
        let auth = format!("Pairing {claimed_token}");
        let what = format!("{method} with the token of an expired channel");
        flood(
            &env,
            &ip,
            (method, &path),
            &auth,
            body,
            (404, "not_found"),
            &what,
        )
        .await;
    }
}

/// A133 without a space: the limit of the address (step 2) still comes before the check of the
/// space (step 3), as everywhere (§2.5), for a token that is not the channel's.
#[tokio::test]
async fn a133b_the_order_of_checks_holds_for_a_pairing_token() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let auth = format!("Pairing tmp1.{}", b64(&[9u8; 24]));
    let route = ("POST", "/v1/pairing/channels/1/messages");
    let body = Some(json!({ "body": "AAAA" }));
    let expected = (409, "space_not_initialized");
    flood(
        &env,
        "198.51.100.200",
        route,
        &auth,
        body,
        expected,
        "no space",
    )
    .await;
}

/// The three routes a `Pairing` token can be presented on, for one channel.
fn channel_routes(channel: &str) -> [(&'static str, String, Option<Value>); 3] {
    [
        (
            "POST",
            format!("/v1/pairing/channels/{channel}/messages"),
            Some(json!({ "body": "AAAA" })),
        ),
        (
            "GET",
            format!("/v1/pairing/channels/{channel}/messages"),
            None,
        ),
        ("DELETE", format!("/v1/pairing/channels/{channel}"), None),
    ]
}

/// A flood of one route from one address with one credential: the route answers as `expected`
/// while the burst of `rl_ip_public` lasts (10 requests), then the flood is stopped.
async fn flood(
    env: &Env,
    ip: &str,
    route: (&str, &str),
    auth: &str,
    body: Option<Value>,
    expected: (u16, &str),
    what: &str,
) {
    for i in 0..10 {
        let r = from_ip(env, ip, route, Some(auth), body.clone()).await;
        assert_eq!(
            (r.status, r.code()),
            (expected.0, expected.1.to_owned()),
            "{what}: request {i}"
        );
    }
    for _ in 0..2 {
        let r = from_ip(env, ip, route, Some(auth), body.clone()).await;
        assert_eq!(
            (r.status, r.code()),
            (429, "rate_limited".to_owned()),
            "{what}: the flood must be stopped"
        );
    }
}

/// A134: a bogus token pays for itself only. The newcomer that shares the address of a flood
/// of bogus tokens (a household, a mobile carrier) is still served with its own.
#[tokio::test]
async fn a134_a_flood_of_bogus_tokens_does_not_block_the_valid_one() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let (devs, _) = env.space_of(1).await;
    let ch = open_channel(&env, &devs[0]).await;
    let token = claim_token(&env, &ch).await;
    let ip = "198.51.100.77";
    let path = format!("/v1/pairing/channels/{ch}/messages");
    let bogus = format!("Pairing tmp1.{}", b64(&[9u8; 24]));
    let valid = format!("Pairing {token}");
    let body = || Some(json!({ "body": b64(b"hello") }));

    let mut served = 0;
    while from_ip(&env, ip, ("POST", &path), Some(&bogus), body())
        .await
        .status
        != 429
    {
        served += 1;
        assert!(served <= 10, "the flood is not stopped");
    }
    from_ip(&env, ip, ("POST", &path), Some(&bogus), body())
        .await
        .err(429, "rate_limited");

    // Same address, valid token: not limited.
    from_ip(&env, ip, ("POST", &path), Some(&valid), body())
        .await
        .ok(201);
    let read = format!("{path}?after=0&wait=0");
    from_ip(&env, ip, ("GET", &read), Some(&valid), None)
        .await
        .ok(200);
    hang_up_from(&env, ip, &token, &ch).await.ok(204);
    // And the bogus one is still refused.
    from_ip(&env, ip, ("POST", &path), Some(&bogus), body())
        .await
        .err(429, "rate_limited");
}

async fn hang_up_from(env: &Env, ip: &str, token: &str, channel: &str) -> Resp {
    let auth = format!("Pairing {token}");
    let path = format!("/v1/pairing/channels/{channel}");
    from_ip(env, ip, ("DELETE", &path), Some(&auth), None).await
}

/// A135: the token of a channel is still bounded, per channel like a device is per device:
/// `rl_device` counts every request of its holder, so refused writes (the 9th message) and
/// repeated closings cannot be a way to keep the single writer of the relay busy.
#[tokio::test]
async fn a135_a_valid_pairing_token_is_still_bounded() {
    let env = Env::with(|cfg, _| {
        real_limits(cfg);
        // A slow refill, so that the test does not depend on the time it takes.
        cfg.rl_device = Rate {
            per_minute: 1.0,
            burst: 20,
        };
    })
    .await;
    let (devs, _) = env.space_of(1).await;
    let a = &devs[0];
    let ch = open_channel(&env, a).await;
    let token = claim_token(&env, &ch).await;

    // 8 messages, then 12 refusals: the 20 requests of the burst, all above the 10 of the
    // address, none of them limited by it.
    for i in 0..8u8 {
        say(&env, &token, "Pairing", &ch, &[i + 1]).await.ok(201);
    }
    for _ in 0..12 {
        say(&env, &token, "Pairing", &ch, b"more")
            .await
            .err(409, "channel_limit");
    }
    // The 21st request is limited, whatever it is.
    say(&env, &token, "Pairing", &ch, b"more")
        .await
        .err(429, "rate_limited");
    hang_up(&env, &token, "Pairing", &ch)
        .await
        .err(429, "rate_limited");
    listen(&env, &token, "Pairing", &ch, 0, 0)
        .await
        .err(429, "rate_limited");
    // The inviter has the allowance of its own device, and another channel has its own.
    say(&env, a.tok(), "Bearer", &ch, b"inviter").await.ok(201);
    let other = open_channel(&env, a).await;
    let other_token = claim_token(&env, &other).await;
    say(&env, &other_token, "Pairing", &other, b"fresh")
        .await
        .ok(201);
    // The allowance refills with time (one request a minute here).
    env.clock.advance_secs(61);
    say(&env, &token, "Pairing", &ch, b"later")
        .await
        .err(409, "channel_limit");
}

/// A136: the reads of a channel are limited for each side of it (`rl_pairing_poll`), also when
/// the reader is the holder of a token that the address does not count.
#[tokio::test]
async fn a136_reads_are_limited_per_channel_and_side() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let (devs, _) = env.space_of(1).await;
    let a = &devs[0];
    let ch = open_channel(&env, a).await;
    let token = claim_token(&env, &ch).await;
    // 60 a minute, burst 10.
    for _ in 0..10 {
        listen(&env, &token, "Pairing", &ch, 0, 0).await.ok(200);
    }
    let limited = listen(&env, &token, "Pairing", &ch, 0, 0).await;
    limited.err(429, "rate_limited");
    assert!(limited.header("retry-after").is_some());
    // The other side of the same channel has its own allowance.
    listen(&env, a.tok(), "Bearer", &ch, 0, 0).await.ok(200);
}

// ===== a real session through the relay (P31) ==========================================

/// One end of a channel as the runtime sees it: its credential, and how far it has read.
struct End {
    scheme: &'static str,
    cred: String,
    /// The `side` the messages it reads come from.
    from: &'static str,
    after: i64,
}

impl End {
    fn inviter(session: &str) -> Self {
        Self {
            scheme: "Bearer",
            cred: session.to_owned(),
            from: "joiner",
            after: 0,
        }
    }

    fn joiner(pairing_token: &str) -> Self {
        Self {
            scheme: "Pairing",
            cred: pairing_token.to_owned(),
            from: "inviter",
            after: 0,
        }
    }

    /// Deposits a frame and returns the index the relay gave it.
    async fn put(&self, env: &Env, channel: &str, frame: &[u8]) -> i64 {
        assert!(
            frame.len() <= MAX_FRAME_LEN,
            "a frame is at most one message"
        );
        say(env, &self.cred, self.scheme, channel, frame)
            .await
            .ok(201)["index"]
            .as_i64()
            .unwrap()
    }

    /// The state of the channel, and the frames of the other side that came since the last
    /// read (no wait: the test moves the two ends by hand).
    async fn get(&mut self, env: &Env, channel: &str) -> (String, Vec<Vec<u8>>) {
        let seen = listen(env, &self.cred, self.scheme, channel, self.after, 0)
            .await
            .ok(200);
        let mut frames = Vec::new();
        for m in seen["messages"].as_array().unwrap() {
            assert_eq!(m["side"], self.from, "an end reads the other side only");
            let index = m["index"].as_i64().unwrap();
            assert!(index > self.after, "in order");
            self.after = index;
            frames.push(unb64(m["body"].as_str().unwrap()));
        }
        (seen["state"].as_str().unwrap().to_owned(), frames)
    }
}

/// P31 (`pairing-v1.md` §14.2): A100 to A107 with the frames of the real protocol, from
/// `tomi_core::pairing`, and with the production limits: a pairing between two devices of one
/// household (the same address) is never rate limited.
#[tokio::test]
async fn p31_a_real_pairing_session_goes_through_the_relay() {
    let env = Env::with(|cfg, _| real_limits(cfg)).await;
    let (devs, trust) = env.space_of(1).await;
    let a = &devs[0];
    let authority = "relay.test";
    // A frame of the protocol is exactly what the relay accepts as a message (A104).
    assert_eq!(
        MAX_FRAME_LEN as u64,
        env.state.cfg.pairing_max_message_bytes
    );

    // A2, A3 (A100): the channel, then the first frame, before the code is shown.
    let ch = open_channel(&env, a).await;
    let mut inviter = Inviter::start(
        InviterParams {
            relay_authority: authority.into(),
            channel_id: ch.clone(),
            device_name: "dev0".into(),
            platform: "windows".into(),
            history_offer: None,
        },
        &a.keys,
        &trust,
    )
    .unwrap();
    let mut end_a = End::inviter(a.tok());
    assert_eq!(end_a.put(&env, &ch, inviter.commit_frame()).await, 1);

    // A4, N1: the code is shown, and typed on the new device.
    let typed = inviter.code().unwrap().to_code_string();
    let mut joiner = Joiner::new(JoinerParams {
        relay_authority: authority.into(),
        code: PairingCode::parse(&typed).unwrap(),
        device_name: "Pixel".into(),
        platform: "android".into(),
    })
    .unwrap();

    // N2 (A101): one claim per channel.
    let token = claim_token(&env, &ch).await;
    claim(&env, &ch).await.err(409, "channel_claimed");
    let mut end_n = End::joiner(&token);

    // N3, N4 (A102): the newcomer reads what the inviter wrote before the claim.
    let (state, frames) = end_n.get(&env, &ch).await;
    assert_eq!((state.as_str(), frames.len()), ("claimed", 1));
    let step = joiner.receive(&frames[0]).unwrap();
    assert_eq!(step.event, JoinerEvent::Shared);
    assert_eq!(end_n.put(&env, &ch, &step.send.unwrap()).await, 2);
    // The newcomer asks again before A has answered, as a client does: nothing yet.
    assert!(end_n.get(&env, &ch).await.1.is_empty());

    // A5, A6
    let (_, frames) = end_a.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = inviter.receive(&frames[0]).unwrap();
    assert_eq!(step.event, InviterEvent::Revealed);
    assert_eq!(end_a.put(&env, &ch, &step.send.unwrap()).await, 3);
    assert!(end_a.get(&env, &ch).await.1.is_empty());

    // N5 to N7
    let (_, frames) = end_n.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = joiner.receive(&frames[0]).unwrap();
    assert_eq!(step.event, JoinerEvent::InviterHello);
    assert_eq!(end_n.put(&env, &ch, &step.send.unwrap()).await, 4);
    assert!(end_n.get(&env, &ch).await.1.is_empty());

    // A7: the two screens show the same verification code.
    let (_, frames) = end_a.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = inviter.receive(&frames[0]).unwrap();
    assert_eq!(step.event, InviterEvent::PeerHello);
    assert_eq!(
        inviter.sas().unwrap().digits(),
        joiner.sas().unwrap().digits()
    );

    // A8, N8, N9: both users confirm.
    inviter.confirm().unwrap();
    let confirm = joiner.confirm(false).unwrap();
    assert_eq!(end_n.put(&env, &ch, &confirm).await, 5);
    assert!(end_n.get(&env, &ch).await.1.is_empty());

    // A9, A10: the space key leaves A after both confirmations.
    let (_, frames) = end_a.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = inviter.receive(&frames[0]).unwrap();
    assert_eq!(step.event, InviterEvent::JoinerConfirmed);
    assert!(inviter.ready_to_provision());
    let space_key = SpaceKey::generate(1).unwrap();
    let now = u64::try_from(env.clock.now_ms()).unwrap();
    let provision = inviter.provision(&a.keys, &trust, &space_key, now).unwrap();
    assert_eq!(end_a.put(&env, &ch, &provision).await, 6);

    // N10, N11
    let (_, frames) = end_n.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = joiner.receive(&frames[0]).unwrap();
    assert_eq!(step.event, JoinerEvent::ProvisionVerified);
    assert_eq!(end_n.put(&env, &ch, &step.send.unwrap()).await, 7);

    // A11, A12 (A105): the inviter registers the device of the batch.
    let (_, frames) = end_a.get(&env, &ch).await;
    assert_eq!(frames.len(), 1);
    let step = inviter.receive(&frames[0]).unwrap();
    assert_eq!(step.event, InviterEvent::Acknowledged);
    let outcome = inviter.finish().unwrap();
    let registered = env
        .post(
            a,
            "/v1/devices",
            json!({
                "channel_id": ch,
                "key_epoch": outcome.key_epoch,
                "name": outcome.peer.name,
                "platform": outcome.peer.platform,
                "certificate": b64(&outcome.device_cert),
            }),
        )
        .await
        .ok(201);
    let added = env.events_of_type(a, "device_added").await;
    assert!(added
        .iter()
        .any(|e| e["data"]["device"]["device_id"] == registered["device_id"]));

    // N12, N13: the newcomer sees `completed` and provisions itself.
    assert_eq!(end_n.get(&env, &ch).await.0, "completed");
    let provisioned = joiner.take_provision().unwrap();
    // A106: the relay derived the identifier of the device from its key.
    assert_eq!(registered["device_id"], provisioned.keys.device_id());
    assert_eq!(provisioned.trust.space_id(), env.space_id);
    // A107: the token of the newcomer opens no other door.
    env.send(
        "GET",
        "/v1/events?after=0",
        &[("authorization", format!("Pairing {token}").as_str())],
        None,
    )
    .await
    .err(401, "unauthorized");
    // The newcomer opens a session with the keys it generated for the pairing.
    let mut x = Dev {
        id: provisioned.keys.device_id(),
        keys: provisioned.keys,
        name: "Pixel".into(),
        token: None,
    };
    env.login(&mut x, 1, 1).await;
    let members = env.get(&x, "/v1/devices").await.ok(200);
    assert_eq!(members["devices"].as_array().unwrap().len(), 2);

    // Blind: the seven frames are in the relay as they were deposited, and the words of the
    // code never reached it.
    let stored: Vec<Vec<u8>> = env
        .db()
        .read_sync(|tx| {
            let mut s = tx.prepare("SELECT body FROM pairing_messages ORDER BY idx")?;
            let rows = s
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(stored.len(), 7);
    assert_eq!(stored[5], provision);
    for word in typed.split('-').skip(1) {
        assert!(!word.is_empty());
        for body in &stored {
            assert!(
                !body.windows(word.len()).any(|w| w == word.as_bytes()),
                "a word of the code is in a frame"
            );
        }
    }
}
