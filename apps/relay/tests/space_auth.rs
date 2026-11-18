//! `api-v1.md` §11.1 (space and installation token) and §11.2 (authentication).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::*;
use serde_json::json;
use tomi_relay::admin::{self, Cmd};

// ===== 11.1 =======================================================================

#[tokio::test]
async fn a02_wrong_install_token_creates_nothing() {
    let env = Env::new().await;
    let founder = Dev::new("PC");
    env.create_space_raw(&founder, "aaaa-bbbb-cccc-dddd-eeee-ffff-gggg-hhhh")
        .await
        .err(403, "install_token_invalid");
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], false);
    assert!(
        tomi_relay::install::is_pending(env.state.spaces.routing(), &env.install_token).unwrap(),
        "a wrong token does not consume the invitation"
    );
    assert!(env.state.spaces.is_empty(), "no space, no directory");
    assert!(!env.data_dir().join("spaces").join(&env.space_id).exists());
}

#[tokio::test]
async fn a03_creation_with_the_right_token() {
    let env = Env::new().await;
    let mut founder = Dev::new("PC-Bureau");
    let token = env.install_token.clone();
    assert!(!token.is_empty());
    let created = env.create_space_raw(&founder, &token).await.ok(201);
    assert_eq!(created["space_id"], env.space_id);
    assert_eq!(created["device_id"], founder.id);
    assert_eq!(created["key_epoch"], 1);
    assert_eq!(created["created_at"], ts(T0));
    env.login(&mut founder, 1, 1).await;
    let events = env.all_events(&founder).await;
    assert_eq!(events.len(), 1, "a single event");
    assert_eq!(events[0]["type"], "device_added");
    assert_eq!(events[0]["seq"], created["head_seq"]);
    assert_eq!(events[0]["seq"].as_i64().unwrap(), env.head_seq().await);
    assert_eq!(events[0]["data"]["device"]["device_id"], founder.id);
    assert!(
        !tomi_relay::install::is_pending(env.state.spaces.routing(), &token).unwrap(),
        "the invitation is consumed by the creation"
    );
    assert!(!env.data_dir().join("install-token").exists());
    // One database per space, and a directory for its blobs.
    let dir = env.data_dir().join("spaces").join(&env.space_id);
    assert!(dir.join("relay.db").is_file());
    assert!(env.data_dir().join("routing.db").is_file());
    assert_eq!(env.space().dir, dir);
}

#[tokio::test]
async fn a04_a_token_serves_once_and_a_space_id_is_never_created_twice() {
    let env = Env::new().await;
    let mut founder = Dev::new("PC");
    let old_token = env.install_token.clone();
    env.create_space(&mut founder).await;
    let other = Dev::new("Other");
    // The invitation was consumed with the creation: the token has done its job, whatever
    // space_id comes with it (a founder whose 201 was lost asks again and must hear that).
    env.create_space_raw(&other, &old_token)
        .await
        .err(409, "space_exists");
    env.for_space(&new_uuid())
        .create_space_raw(&other, &old_token)
        .await
        .err(409, "space_exists");
    assert!(!tomi_relay::install::is_pending(env.state.spaces.routing(), &old_token).unwrap());
    assert_eq!(env.state.spaces.list().len(), 1, "no second space");
    // A fresh invitation does not create a space that exists: 409, and it stays valid.
    let again = env.another_space().await;
    let fresh = again.install_token.clone();
    env.create_space_raw(&other, &fresh)
        .await
        .err(409, "space_exists");
    assert!(tomi_relay::install::is_pending(env.state.spaces.routing(), &fresh).unwrap());
    // Without a valid token nobody learns that a space_id exists: the token is judged first.
    env.create_space_raw(&other, "aaaa-bbbb-cccc-dddd-eeee-ffff-gggg-hhhh")
        .await
        .err(403, "install_token_invalid");
}

#[tokio::test]
async fn a05_two_simultaneous_creations_one_wins() {
    let env = Env::new().await;
    let (a, b) = (Dev::new("A"), Dev::new("B"));
    let token = env.install_token.clone();
    let (ra, rb) = tokio::join!(
        env.create_space_raw(&a, &token),
        env.create_space_raw(&b, &token)
    );
    let mut statuses = [(ra.status, ra.code()), (rb.status, rb.code())];
    statuses.sort();
    assert_eq!(statuses[0].0, 201);
    // The loser finds the invitation consumed: the space exists.
    assert_eq!(statuses[1], (409, "space_exists".to_owned()));
    assert_eq!(env.sql_count("SELECT count(*) FROM devices"), 1);
    assert_eq!(env.state.spaces.list().len(), 1);
}

/// `pairing-v1.md` §10.2 and P30: the answer to `POST /v1/space` is lost, the founder asks
/// again with the same request and hears `409 space_exists`, then proves with its keys that the
/// space is its own.
#[tokio::test]
async fn a05b_a_lost_201_is_recovered_by_asking_again() {
    let env = Env::new().await;
    let founder = Dev::new("PC");
    let token = env.install_token.clone();
    // The first request created the space; its answer never arrived.
    let _lost = env.create_space_raw(&founder, &token).await.ok(201);
    // The same request again: not "wrong token".
    env.create_space_raw(&founder, &token)
        .await
        .err(409, "space_exists");
    // The founder's challenge/response with its pending keys succeeds.
    let v = env.login_raw(&founder, 1, 1).await.ok(200);
    assert_eq!(v["space"]["space_id"], env.space_id);
    // The operator rotating the invitations does not take that proof away.
    let out = admin::run(
        &env.state,
        Cmd::Invite {
            rotate: true,
            new: false,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    env.create_space_raw(&founder, &token)
        .await
        .err(409, "space_exists");
    assert_eq!(env.state.spaces.list().len(), 1);
    // It survives a restart of the relay (it is in routing.db).
    let env = env.restart().await;
    env.create_space_raw(&founder, &token)
        .await
        .err(409, "space_exists");
    env.login_raw(&founder, 1, 1).await.ok(200);
}

/// Two devices with the same token: the second hears `space_exists`, and its challenge/response
/// fails with `401`, so it knows another device created the space and offers "Rejoindre".
#[tokio::test]
async fn a05c_the_second_device_with_the_same_token_is_not_a_founder() {
    let env = Env::new().await;
    let (first, second) = (Dev::new("first"), Dev::new("second"));
    let token = env.install_token.clone();
    env.create_space_raw(&first, &token).await.ok(201);
    let second_env = env.for_space(&new_uuid());
    second_env
        .create_space_raw(&second, &token)
        .await
        .err(409, "space_exists");
    let ch = second_env.challenge(&second.id).await;
    assert_eq!(
        ch.status, 200,
        "an unknown device gets a challenge as any other"
    );
    second_env
        .login_raw(&second, 1, 1)
        .await
        .err(401, "unauthorized");
    assert_eq!(env.state.spaces.list().len(), 1);
    assert!(env.state.spaces.by_device(&second.id).is_none());
}

/// `admin reset` of a space forgets the invitation that created it (its token is a wrong token
/// again) and only that one.
#[tokio::test]
async fn a05d_reset_forgets_the_consumed_invitation_of_that_space_only() {
    let a = Env::new().await;
    let b = a.another_space().await;
    let (mut fa, mut fb) = (Dev::new("A"), Dev::new("B"));
    let (token_a, token_b) = (a.install_token.clone(), b.install_token.clone());
    a.create_space(&mut fa).await;
    b.create_space(&mut fb).await;
    a.create_space_raw(&fa, &token_a)
        .await
        .err(409, "space_exists");
    b.create_space_raw(&fb, &token_b)
        .await
        .err(409, "space_exists");

    let out = admin::run(
        &a.state,
        Cmd::Reset {
            confirm: Some(a.space_id[..8].to_owned()),
            space: Some(a.space_id[..8].to_owned()),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    a.create_space_raw(&fa, &token_a)
        .await
        .err(403, "install_token_invalid");
    b.create_space_raw(&fb, &token_b)
        .await
        .err(409, "space_exists");

    // Resetting the last space replaces the pending invitations and forgets its own too.
    let out = admin::run(
        &b.state,
        Cmd::Reset {
            confirm: Some(b.space_id[..8].to_owned()),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let fresh = out.stdout.trim().to_owned();
    b.create_space_raw(&fb, &token_b)
        .await
        .err(403, "install_token_invalid");
    b.create_space_raw(&fb, &fresh).await.ok(201);
}

#[tokio::test]
async fn a06_a_bad_certificate_is_refused_and_the_token_stays_valid() {
    let env = Env::new().await;
    let founder = Dev::new("PC");
    let token = env.install_token.clone();
    let body = |cert: Vec<u8>, space_id: &str| {
        json!({
            "install_token": token, "space_id": space_id, "name": "PC", "platform": "windows",
            "certificate": b64(&cert),
        })
    };
    // Wrong signature.
    let mut bad_sig = env.cert(&founder.keys, &founder.keys);
    let last = bad_sig.len() - 1;
    bad_sig[last] ^= 1;
    let r = env
        .send("POST", "/v1/space", &[], Some(body(bad_sig, &env.space_id)))
        .await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/certificate");
    // Not self-signed: issued by another key.
    let other = Dev::new("Other");
    let r = env
        .send(
            "POST",
            "/v1/space",
            &[],
            Some(body(env.cert(&other.keys, &founder.keys), &env.space_id)),
        )
        .await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/certificate");
    // Another space_id in the certificate.
    let foreign = new_uuid();
    let cert = founder
        .keys
        .sign_device_cert(&foreign, &founder.keys.public(), T0 as u64)
        .unwrap()
        .to_bytes()
        .unwrap();
    let r = env
        .send("POST", "/v1/space", &[], Some(body(cert, &env.space_id)))
        .await;
    r.err(422, "validation_failed");
    assert_eq!(r.json()["errors"][0]["field"], "/certificate");
    // Garbage bytes.
    let r = env
        .send(
            "POST",
            "/v1/space",
            &[],
            Some(body(vec![1, 2, 3], &env.space_id)),
        )
        .await;
    r.err(422, "validation_failed");
    // The invitation is still valid, and nothing was left on the disk.
    assert!(tomi_relay::install::is_pending(env.state.spaces.routing(), &token).unwrap());
    assert!(env.state.spaces.is_empty());
    assert!(!env.data_dir().join("spaces").join(&env.space_id).exists());
    env.create_space_raw(&founder, &token).await.ok(201);
}

#[tokio::test]
async fn a07_no_space_yet() {
    let env = Env::new().await;
    let d = Dev::new("PC");
    env.challenge(&d.id).await.err(409, "space_not_initialized");
    let hello = env.send("GET", "/v1/hello", &[], None).await.ok(200);
    assert_eq!(hello["space_initialized"], false);
    assert_eq!(hello["service"], "tomi-relay");
    assert_eq!(hello["api_versions"], json!([1]));
    assert_eq!(hello["limits"]["max_envelope_bytes"], 1_114_112);
    // The other routes agree.
    env.bearer("GET", "/v1/space", "tms1.whatever", None)
        .await
        .err(409, "space_not_initialized");
}

// ===== 11.2 =======================================================================

#[tokio::test]
async fn a10_a_member_signs_the_challenge() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let v = env.login_raw(&devs[0], 1, 1).await.ok(200);
    let token = v["session_token"].as_str().unwrap();
    assert!(token.starts_with("tms1."), "{token}");
    assert_eq!(unb64(&token["tms1.".len()..]).len(), 32);
    assert_eq!(v["expires_at"], ts(T0 + 3_600_000));
    assert_eq!(v["device_id"], devs[0].id);
    assert_eq!(v["space"]["space_id"], env.space_id);
    assert_eq!(v["space"]["key_epoch"], 1);
    assert_eq!(v["space"]["rotation_required"], false);
    assert_eq!(v["space"]["effective_format"], 1);
    // No quota of its own by default: the room left in the total of the relay (1 GiB).
    assert_eq!(v["space"]["quota"]["max_bytes"], 1_073_741_824u64);
    assert_eq!(v["space"]["limits"]["max_devices"], 20);
    // Only the hash of the token is stored.
    let stored = env.sql_count(&format!(
        "SELECT count(*) FROM sessions WHERE token_hash = x'{}'",
        hex::encode(tomi_relay::wire::sha256(token.as_bytes()))
    ));
    assert_eq!(stored, 1);
    assert_eq!(
        env.sql_count(&format!(
            "SELECT count(*) FROM sessions WHERE hex(token_hash) = '{}'",
            hex::encode_upper(token.as_bytes())
        )),
        0
    );
}

#[tokio::test]
async fn a11_a_challenge_serves_once() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    // Used with success, presented again.
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let body = env.session_body(d, &challenge, 1, 1, "windows");
    env.send("POST", "/v1/auth/session", &[], Some(body.clone()))
        .await
        .ok(200);
    env.send("POST", "/v1/auth/session", &[], Some(body))
        .await
        .err(401, "unauthorized");
    // Used with a wrong signature, then presented with the right one.
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let mut bad = env.session_body(d, &challenge, 1, 1, "windows");
    bad["signature"] = json!(b64(&[7u8; 64]));
    env.send("POST", "/v1/auth/session", &[], Some(bad))
        .await
        .err(401, "unauthorized");
    let good = env.session_body(d, &challenge, 1, 1, "windows");
    env.send("POST", "/v1/auth/session", &[], Some(good))
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn a12_a_challenge_lives_sixty_seconds() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = &devs[0];
    let ch = env.challenge(&d.id).await.ok(200);
    assert_eq!(ch["expires_at"], ts(T0 + 60_000));
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    env.clock.advance_secs(61);
    env.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(env.session_body(d, &challenge, 1, 1, "windows")),
    )
    .await
    .err(401, "unauthorized");
}

#[tokio::test]
async fn a13_wrong_signatures_are_refused() {
    use ed25519_dalek::Signer;
    let env = Env::new().await;
    let (devs, _) = env.space_of(2).await;
    let d = &devs[0];
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let space = uuid_bytes(&env.space_id);

    // Without the label `tomi/v1/sig/relay-auth\0`.
    let seed: [u8; 32] = d.keys.ed25519_seed().try_into().unwrap();
    let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
    let mut no_label = Vec::new();
    no_label.extend_from_slice(&space);
    no_label.extend_from_slice(&d.pk());
    no_label.extend_from_slice(&challenge);
    let mut body = env.session_body(d, &challenge, 1, 1, "windows");
    body["signature"] = json!(b64(&sk.sign(&no_label).to_bytes()));
    env.send("POST", "/v1/auth/session", &[], Some(body))
        .await
        .err(401, "unauthorized");

    // With the key of another device.
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let mut body = env.session_body(d, &challenge, 1, 1, "windows");
    body["signature"] = json!(b64(&devs[1]
        .keys
        .sign_relay_auth(&env.space_id, &challenge)
        .unwrap()));
    env.send("POST", "/v1/auth/session", &[], Some(body))
        .await
        .err(401, "unauthorized");

    // On another space_id.
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let mut body = env.session_body(d, &challenge, 1, 1, "windows");
    body["signature"] = json!(b64(&d
        .keys
        .sign_relay_auth(&new_uuid(), &challenge)
        .unwrap()));
    env.send("POST", "/v1/auth/session", &[], Some(body))
        .await
        .err(401, "unauthorized");

    // A signature of another statement (a tombstone) cannot be replayed as a proof.
    let ch = env.challenge(&d.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    let t = d
        .keys
        .sign_tombstone(&env.space_id, &new_uuid(), 1, DeletionScope::Everywhere)
        .unwrap();
    let mut body = env.session_body(d, &challenge, 1, 1, "windows");
    body["signature"] = json!(b64(&t.signature));
    env.send("POST", "/v1/auth/session", &[], Some(body))
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn a14_an_unknown_device_cannot_be_told_from_a_known_one() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let stranger = Dev::new("Stranger");
    let known = env.challenge(&devs[0].id).await;
    let unknown = env.challenge(&stranger.id).await;
    assert_eq!((known.status, unknown.status), (200, 200));
    let (k, u) = (known.json(), unknown.json());
    let keys = |v: &serde_json::Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&k), keys(&u));
    let challenge = unb64(u["challenge"].as_str().unwrap());
    env.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(env.session_body(&stranger, &challenge, 1, 1, "windows")),
    )
    .await
    .err(401, "unauthorized");
}

#[tokio::test]
async fn a15_an_expired_session_says_so() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    env.get(&devs[0], "/v1/space").await.ok(200);
    env.clock.advance_secs(3601);
    env.get(&devs[0], "/v1/space")
        .await
        .err(401, "session_expired");
}

#[tokio::test]
async fn a16_the_query_string_is_never_read_for_authentication() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let r = env
        .send(
            "GET",
            &format!("/v1/space?token={}", devs[0].tok()),
            &[],
            None,
        )
        .await;
    r.err(401, "unauthorized");
    let r = env
        .send(
            "GET",
            &format!("/v1/events?after=0&access_token={}", devs[0].tok()),
            &[],
            None,
        )
        .await;
    r.err(401, "unauthorized");
}

#[tokio::test]
async fn a17_at_most_five_sessions_per_device() {
    let env = Env::new().await;
    let (mut devs, _) = env.space_of(1).await;
    let d = &mut devs[0];
    let mut tokens = vec![d.token.clone().unwrap()];
    for _ in 0..5 {
        env.login(d, 1, 1).await;
        tokens.push(d.token.clone().unwrap());
    }
    assert_eq!(tokens.len(), 6);
    env.bearer("GET", "/v1/space", &tokens[0], None)
        .await
        .err(401, "unauthorized");
    for t in &tokens[1..] {
        env.bearer("GET", "/v1/space", t, None).await.ok(200);
    }
}

#[tokio::test]
async fn a18_logout_invalidates_the_token() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    env.delete(&devs[0], "/v1/auth/session").await.ok(204);
    env.get(&devs[0], "/v1/space")
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn every_answer_carries_a_request_id_and_no_cors_header() {
    let env = Env::new().await;
    let ok = env.send("GET", "/v1/hello", &[], None).await;
    let id = ok.header("x-request-id").unwrap();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert!(ok
        .headers
        .keys()
        .all(|k| !k.as_str().starts_with("access-control-")));
    let err = env.send("GET", "/v1/nothing-here", &[], None).await;
    err.err(404, "not_found");
    assert_eq!(
        err.json()["request_id"],
        err.header("x-request-id").unwrap()
    );
    assert_eq!(
        err.header("content-type").as_deref(),
        Some("application/problem+json")
    );
    let method = env.send("DELETE", "/v1/hello", &[], None).await;
    method.err(405, "method_not_allowed");
    assert!(method.header("allow").unwrap().contains("GET"));
}

// ===== the answers of /v1/* are never cached nor sniffed =====================================

#[tokio::test]
async fn every_answer_of_the_api_carries_no_store_and_nosniff_the_token_one_included() {
    let env = Env::new().await;
    let (devs, _) = env.space_of(1).await;
    let d = clone_dev(&devs[0]);
    // The answer that carries the session token.
    let login = env.login_raw(&d, 1, 1).await;
    assert_eq!(login.status, 200);
    let answers = vec![
        ("session", login),
        ("hello", env.send("GET", "/v1/hello", &[], None).await),
        ("space", env.get(&devs[0], "/v1/space").await),
        ("an error", env.send("GET", "/v1/space", &[], None).await),
        (
            "an unknown path of the API",
            env.send("GET", "/v1/nope", &[], None).await,
        ),
        (
            "a method not allowed",
            env.send("DELETE", "/v1/hello", &[], None).await,
        ),
        (
            "a blob",
            env.get(&devs[0], &format!("/v1/blobs/{}", new_uuid()))
                .await,
        ),
    ];
    for (what, r) in &answers {
        assert_eq!(
            r.header("cache-control").as_deref(),
            Some("no-store"),
            "{what}"
        );
        assert_eq!(
            r.header("x-content-type-options").as_deref(),
            Some("nosniff"),
            "{what}"
        );
    }
    // What is not the API keeps its own caching rules.
    let landing = env.send("GET", "/", &[], None).await;
    assert_eq!(
        landing.header("cache-control").as_deref(),
        Some("public, max-age=3600")
    );
}
