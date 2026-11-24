//! The web administration panel: WebAuthn verification, enrolment, sessions, the disabled panel
//! that looks like nothing, and one run from the enrolment to the creation of an invitation.
//!
//! The "browser" is `common::authenticator`: a real P-256 key that builds what a browser hands
//! over. What no test here can show is a real browser or phone with a real passkey.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use common::authenticator::{
    cbor_bytes, cbor_int, cbor_map, new_key, Authenticator, Tweak, FLAG_AT, FLAG_UP, FLAG_UV,
};
use common::*;
use serde_json::{json, Value};
use tomi_relay::admin::{self, Cmd, WebAction};
use tomi_relay::webauthn::{
    cbor_decode_exact, verify_assertion, verify_registration, Expected, Reject,
};
use tomi_relay::wire::b64_encode;

const ORIGIN: &str = "https://relay.test";
const RP_ID: &str = "relay.test";
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const COOKIE: &str = "__Host-tomi-admin";

// ===== helpers ==============================================================================

async fn panel() -> Env {
    Env::with(|cfg, _| cfg.admin_web = true).await
}

fn segment(env: &Env) -> String {
    env.state.admin_web.as_ref().unwrap().segment()
}

async fn web(env: &Env, action: WebAction) -> admin::Output {
    admin::run(&env.state, Cmd::Web { action }).await
}

async fn enroll_code(env: &Env) -> String {
    let out = web(env, WebAction::Enroll).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    out.stdout.trim().to_owned()
}

async fn get(env: &Env, seg: &str, path: &str, cookie: Option<&str>) -> Resp {
    let url = format!("/{seg}/{path}");
    let mut h: Vec<(&str, &str)> = Vec::new();
    if let Some(c) = cookie {
        h.push(("cookie", c));
    }
    env.send("GET", &url, &h, None).await
}

async fn post_with(
    env: &Env,
    seg: &str,
    path: &str,
    body: Value,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
) -> Resp {
    let url = format!("/{seg}/{path}");
    let mut h: Vec<(&str, &str)> = headers.to_vec();
    if let Some(c) = cookie {
        h.push(("cookie", c));
    }
    env.send("POST", &url, &h, Some(body)).await
}

/// A `POST` as the page sends it: the `Origin` of the relay and `X-Tomi-Admin: 1`.
async fn post(env: &Env, seg: &str, path: &str, body: Value, cookie: Option<&str>) -> Resp {
    post_with(
        env,
        seg,
        path,
        body,
        cookie,
        &[("origin", ORIGIN), ("x-tomi-admin", "1")],
    )
    .await
}

/// A step-up action as the page does it: the options (the challenge tied to the action), then the
/// assertion of the passkey, then the action. Returns the answer of the options if they are refused.
async fn act_with(
    env: &Env,
    seg: &str,
    action: Value,
    cookie: &str,
    auth: &mut Authenticator,
    headers: &[(&str, &str)],
) -> Resp {
    // Two sensitive requests per action: let the bucket of the address refill.
    env.clock.advance_secs(20);
    let mut h: Vec<(&str, &str)> = vec![("origin", ORIGIN), ("x-tomi-admin", "1")];
    h.extend_from_slice(headers);
    let opts = post_with(
        env,
        seg,
        "api/stepup/options",
        json!({ "action": action.clone() }),
        Some(cookie),
        &h,
    )
    .await;
    if opts.status != 200 {
        return opts;
    }
    let assertion = auth.get(&challenge_of(&opts.json()), &Tweak::default());
    post_with(
        env,
        seg,
        "api/actions",
        json!({ "action": action, "assertion": assertion.json() }),
        Some(cookie),
        &h,
    )
    .await
}

async fn act(env: &Env, seg: &str, action: Value, cookie: &str, auth: &mut Authenticator) -> Resp {
    act_with(env, seg, action, cookie, auth, &[]).await
}

async fn register_options(env: &Env, seg: &str, code: &str) -> Resp {
    post(
        env,
        seg,
        "api/register/options",
        json!({ "code": code }),
        None,
    )
    .await
}

fn challenge_of(options: &Value) -> Vec<u8> {
    unb64(options["publicKey"]["challenge"].as_str().unwrap())
}

/// Enrolment as the page does it; returns the answer of `register/finish`.
async fn register(env: &Env, seg: &str, auth: &Authenticator, code: &str) -> Resp {
    let options = register_options(env, seg, code).await.ok(200);
    let reg = auth.create(&challenge_of(&options), &Tweak::default());
    post(env, seg, "api/register/finish", reg.json(), None).await
}

async fn login_options(env: &Env, seg: &str) -> Resp {
    post(env, seg, "api/login/options", json!({}), None).await
}

fn cookie_of(resp: &Resp) -> String {
    let line = resp
        .headers
        .get("set-cookie")
        .expect("a Set-Cookie header")
        .to_str()
        .unwrap();
    line.split(';').next().unwrap().to_owned()
}

/// Sign-in as the page does it; returns the answer of `login/finish`.
async fn login_raw(env: &Env, seg: &str, auth: &mut Authenticator, t: &Tweak) -> Resp {
    let options = login_options(env, seg).await.ok(200);
    let assertion = auth.get(&challenge_of(&options), t);
    post(env, seg, "api/login/finish", assertion.json(), None).await
}

/// Enrolment then sign-in: the cookie header of a live session.
async fn signed_in(env: &Env, auth: &mut Authenticator) -> String {
    let seg = segment(env);
    let code = enroll_code(env).await;
    register(env, &seg, auth, &code).await.ok(204);
    let resp = login_raw(env, &seg, auth, &Tweak::default()).await;
    assert_eq!(resp.status, 204, "{}", String::from_utf8_lossy(&resp.body));
    cookie_of(&resp)
}

/// What an answer looks like once the request id (which differs on purpose) is set aside.
fn shape(r: &Resp) -> (u16, Vec<(String, String)>, Value) {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .filter(|(k, _)| k.as_str() != "x-request-id")
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
        .collect();
    headers.sort();
    let mut body = if r.body.is_empty() {
        Value::Null
    } else {
        r.json()
    };
    if let Some(o) = body.as_object_mut() {
        o.remove("request_id");
    }
    (r.status, headers, body)
}

async fn unknown_shape(env: &Env) -> (u16, Vec<(String, String)>, Value) {
    let r = env.send("GET", "/some/unknown/path", &[], None).await;
    assert_eq!(r.status, 404);
    shape(&r)
}

fn expected<'a>(challenge: &'a [u8]) -> Expected<'a> {
    Expected {
        origin: ORIGIN,
        rp_id: RP_ID,
        challenge,
    }
}

fn public_key_of(a: &Authenticator) -> Vec<u8> {
    a.key
        .verifying_key()
        .to_sec1_point(false)
        .as_bytes()
        .to_vec()
}

// ===== WebAuthn verification =================================================================

#[test]
fn a_valid_registration_is_accepted() {
    let auth = Authenticator::for_test_relay();
    let challenge = [7u8; 32];
    let reg = auth.create(&challenge, &Tweak::default());
    let got = verify_registration(
        &expected(&challenge),
        &reg.client_data_json,
        &reg.attestation_object,
    )
    .unwrap();
    assert_eq!(got.credential_id, auth.credential_id);
    assert_eq!(got.public_key, public_key_of(&auth));
    assert_eq!(got.public_key.len(), 65);
    assert_eq!(got.sign_count, 0);
}

#[test]
fn registrations_that_get_one_thing_wrong_are_refused() {
    let auth = Authenticator::for_test_relay();
    let challenge = [7u8; 32];
    let t = Tweak::default;
    let cases: Vec<(&str, Tweak, Reject)> = vec![
        (
            "wrong origin",
            Tweak {
                origin: Some("https://evil.test".into()),
                ..t()
            },
            Reject::Origin,
        ),
        (
            "http origin",
            Tweak {
                origin: Some("http://relay.test".into()),
                ..t()
            },
            Reject::Origin,
        ),
        (
            "origin with a port",
            Tweak {
                origin: Some("https://relay.test:444".into()),
                ..t()
            },
            Reject::Origin,
        ),
        (
            "wrong rpIdHash",
            Tweak {
                rp_id: Some("other.test".into()),
                ..t()
            },
            Reject::RpIdHash,
        ),
        (
            "wrong challenge",
            Tweak {
                challenge: Some(vec![8u8; 32]),
                ..t()
            },
            Reject::Challenge,
        ),
        (
            "wrong type",
            Tweak {
                kind: Some("webauthn.get"),
                ..t()
            },
            Reject::WrongType,
        ),
        (
            "user verification absent",
            Tweak {
                flags: Some(FLAG_UP | FLAG_AT),
                ..t()
            },
            Reject::UserVerification,
        ),
        (
            "user presence absent",
            Tweak {
                flags: Some(FLAG_UV | FLAG_AT),
                ..t()
            },
            Reject::UserPresence,
        ),
        (
            "no attested credential data",
            Tweak {
                flags: Some(FLAG_UP | FLAG_UV),
                ..t()
            },
            Reject::AuthData,
        ),
        (
            "backup state without backup eligibility",
            Tweak {
                flags: Some(FLAG_UP | FLAG_UV | FLAG_AT | 0x10),
                ..t()
            },
            Reject::AuthData,
        ),
        (
            "algorithm RS256",
            Tweak {
                alg: Some(-257),
                ..t()
            },
            Reject::Algorithm,
        ),
        (
            "algorithm EdDSA",
            Tweak {
                alg: Some(-8),
                ..t()
            },
            Reject::Algorithm,
        ),
        (
            "key type OKP",
            Tweak {
                kty: Some(1),
                ..t()
            },
            Reject::PublicKey,
        ),
        (
            "curve P-384",
            Tweak {
                crv: Some(2),
                ..t()
            },
            Reject::PublicKey,
        ),
        (
            "point off the curve",
            Tweak {
                off_curve: true,
                ..t()
            },
            Reject::PublicKey,
        ),
        (
            "format packed",
            Tweak {
                fmt: Some("packed"),
                ..t()
            },
            Reject::Attestation,
        ),
        (
            "non-empty attestation statement",
            Tweak {
                att_stmt_entry: true,
                ..t()
            },
            Reject::Attestation,
        ),
        (
            "cross-origin ceremony",
            Tweak {
                cross_origin: true,
                ..t()
            },
            Reject::CrossOrigin,
        ),
        (
            "credential id too short",
            Tweak {
                short_id: true,
                ..t()
            },
            Reject::AuthData,
        ),
        (
            "bytes after the key",
            Tweak {
                trailing: true,
                ..t()
            },
            Reject::AuthData,
        ),
    ];
    for (what, tweak, why) in cases {
        let reg = auth.create(&challenge, &tweak);
        let got = verify_registration(
            &expected(&challenge),
            &reg.client_data_json,
            &reg.attestation_object,
        );
        assert_eq!(got, Err(why), "{what}");
    }

    // Garbage in place of the two documents.
    let good = auth.create(&challenge, &Tweak::default());
    assert_eq!(
        verify_registration(&expected(&challenge), b"not json", &good.attestation_object),
        Err(Reject::ClientData)
    );
    assert_eq!(
        verify_registration(&expected(&challenge), &good.client_data_json, &[0xff, 0x00]),
        Err(Reject::Attestation)
    );
    // An attestation object with a fourth entry.
    let extra = cbor_map(vec![
        (b"\x63fmt".to_vec(), b"\x64none".to_vec()),
        (b"\x67attStmt".to_vec(), cbor_map(vec![])),
        (b"\x68authData".to_vec(), cbor_bytes(&[0u8; 40])),
        (b"\x61x".to_vec(), cbor_int(1)),
    ]);
    assert_eq!(
        verify_registration(&expected(&challenge), &good.client_data_json, &extra),
        Err(Reject::Attestation)
    );
}

#[test]
fn a_valid_assertion_is_accepted_and_the_counter_moves() {
    let mut auth = Authenticator::for_test_relay();
    auth.sign_count = 4;
    let challenge = [3u8; 32];
    let a = auth.get(&challenge, &Tweak::default());
    let count = verify_assertion(
        &expected(&challenge),
        &public_key_of(&auth),
        4,
        &a.client_data_json,
        &a.authenticator_data,
        &a.signature,
    )
    .unwrap();
    assert_eq!(count, 5);
}

#[test]
fn assertions_that_get_one_thing_wrong_are_refused() {
    let mut auth = Authenticator::for_test_relay();
    auth.sign_count = 4;
    let challenge = [3u8; 32];
    let t = Tweak::default;
    let cases: Vec<(&str, Tweak, Reject)> = vec![
        (
            "wrong origin",
            Tweak {
                origin: Some("https://evil.test".into()),
                ..t()
            },
            Reject::Origin,
        ),
        (
            "wrong rpIdHash",
            Tweak {
                rp_id: Some("other.test".into()),
                ..t()
            },
            Reject::RpIdHash,
        ),
        (
            "wrong challenge",
            Tweak {
                challenge: Some(vec![4u8; 32]),
                ..t()
            },
            Reject::Challenge,
        ),
        (
            "wrong type",
            Tweak {
                kind: Some("webauthn.create"),
                ..t()
            },
            Reject::WrongType,
        ),
        (
            "user verification absent",
            Tweak {
                flags: Some(FLAG_UP),
                ..t()
            },
            Reject::UserVerification,
        ),
        (
            "user presence absent",
            Tweak {
                flags: Some(FLAG_UV),
                ..t()
            },
            Reject::UserPresence,
        ),
        (
            "a flipped bit in the signature",
            Tweak {
                bad_signature: true,
                ..t()
            },
            Reject::Signature,
        ),
        (
            "a signature that is not DER",
            Tweak {
                garbage_signature: true,
                ..t()
            },
            Reject::Signature,
        ),
        (
            "a signature over other client data",
            Tweak {
                sign_other_client_data: true,
                ..t()
            },
            Reject::Signature,
        ),
        (
            "a signature by another key",
            Tweak {
                sign_with: Some(new_key()),
                ..t()
            },
            Reject::Signature,
        ),
        (
            "a counter that stays",
            Tweak {
                count: Some(4),
                ..t()
            },
            Reject::SignCount,
        ),
        (
            "a counter that goes back",
            Tweak {
                count: Some(2),
                ..t()
            },
            Reject::SignCount,
        ),
        (
            "a counter that drops to zero",
            Tweak {
                count: Some(0),
                ..t()
            },
            Reject::SignCount,
        ),
    ];
    for (what, tweak, why) in cases {
        let a = auth.get(&challenge, &tweak);
        let got = verify_assertion(
            &expected(&challenge),
            &public_key_of(&auth),
            4,
            &a.client_data_json,
            &a.authenticator_data,
            &a.signature,
        );
        assert_eq!(got, Err(why), "{what}");
    }
    // A key that is not a point.
    let a = auth.get(&challenge, &Tweak::default());
    assert_eq!(
        verify_assertion(
            &expected(&challenge),
            &[4u8; 65],
            4,
            &a.client_data_json,
            &a.authenticator_data,
            &a.signature,
        ),
        Err(Reject::PublicKey)
    );
    // Authenticator data that is too short.
    assert_eq!(
        verify_assertion(
            &expected(&challenge),
            &public_key_of(&auth),
            4,
            &a.client_data_json,
            &a.authenticator_data[..30],
            &a.signature,
        ),
        Err(Reject::AuthData)
    );
}

#[test]
fn an_authenticator_without_a_counter_is_accepted_but_never_a_regression() {
    let mut auth = Authenticator::for_test_relay();
    let challenge = [3u8; 32];
    let a = auth.get(
        &challenge,
        &Tweak {
            count: Some(0),
            ..Tweak::default()
        },
    );
    let got = verify_assertion(
        &expected(&challenge),
        &public_key_of(&auth),
        0,
        &a.client_data_json,
        &a.authenticator_data,
        &a.signature,
    );
    assert_eq!(got, Ok(0), "two zeros: no counter on this authenticator");
}

#[test]
fn the_cbor_decoder_agrees_with_the_encoder_of_the_tests() {
    let auth = Authenticator::for_test_relay();
    let key = auth.cose_key(&Tweak::default());
    assert!(cbor_decode_exact(&key).is_some());
}

// ===== enrolment =============================================================================

#[tokio::test]
async fn the_enrolment_needs_a_code_that_the_server_made() {
    let env = panel().await;
    let seg = segment(&env);
    let auth = Authenticator::for_test_relay();
    // No code was ever made: every code is wrong, and nothing says why.
    register_options(&env, &seg, "abcd-efgh-jkmn")
        .await
        .err(401, "unauthorized");
    // A missing code is a malformed request.
    post(&env, &seg, "api/register/options", json!({}), None)
        .await
        .err(400, "bad_request");
    // Without a challenge, a registration is nothing.
    let reg = auth.create(&[1u8; 32], &Tweak::default());
    post(&env, &seg, "api/register/finish", reg.json(), None)
        .await
        .err(401, "unauthorized");
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state["enrolled"], false);
}

#[tokio::test]
async fn the_code_is_only_kept_as_a_hash_and_is_shown_once() {
    let env = panel().await;
    let out = web(&env, WebAction::Enroll).await;
    assert_eq!(out.exit, 0);
    let code = out.stdout.trim().to_owned();
    assert_eq!(code.len(), 14, "{code}");
    assert!(out.stderr.contains("15 minutes"), "{}", out.stderr);
    let stored: Vec<u8> = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT code_hash FROM admin_web_enrollment", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert!(!String::from_utf8_lossy(&stored).contains(&code));
    // Nothing else tells it: `admin web` shows the URL and the state, not the code.
    let shown = web(&env, WebAction::Show).await;
    assert!(!shown.stdout.contains(&code) && !shown.stderr.contains(&code));
    assert!(shown.stdout.contains("pending"), "{}", shown.stdout);
}

#[tokio::test]
async fn five_wrong_codes_invalidate_the_code() {
    let env = panel().await;
    let seg = segment(&env);
    let code = enroll_code(&env).await;
    for _ in 0..4 {
        register_options(&env, &seg, "0000-0000-0000")
            .await
            .err(401, "unauthorized");
    }
    // The right code still works after four mistakes (and costs nothing).
    register_options(&env, &seg, &code).await.ok(200);
    // The fifth mistake kills the code: the right one no longer works.
    register_options(&env, &seg, "0000-0000-0000")
        .await
        .err(401, "unauthorized");
    register_options(&env, &seg, &code)
        .await
        .err(401, "unauthorized");
    let row: i64 = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT count(*) FROM admin_web_enrollment", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(row, 0);
    // A new code starts again with five attempts.
    let again = enroll_code(&env).await;
    assert_ne!(again, code);
    register_options(&env, &seg, &again).await.ok(200);
}

#[tokio::test]
async fn an_expired_code_is_refused() {
    let env = panel().await;
    let seg = segment(&env);
    let code = enroll_code(&env).await;
    env.clock.advance_ms(15 * 60_000 - 1);
    register_options(&env, &seg, &code).await.ok(200);
    env.clock.advance_ms(1);
    register_options(&env, &seg, &code)
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn a_code_that_expires_between_the_two_steps_registers_nothing() {
    let env = panel().await;
    let seg = segment(&env);
    let auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    let options = register_options(&env, &seg, &code).await.ok(200);
    env.clock.advance_ms(15 * 60_000);
    // The challenge itself expired after 5 minutes, and so did the code.
    let reg = auth.create(&challenge_of(&options), &Tweak::default());
    post(&env, &seg, "api/register/finish", reg.json(), None)
        .await
        .err(401, "unauthorized");
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state["enrolled"], false);
}

#[tokio::test]
async fn the_enrolment_is_closed_for_good_by_the_first_key() {
    let env = panel().await;
    let seg = segment(&env);
    let auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    let options = register_options(&env, &seg, &code).await.ok(200);
    let reg = auth.create(&challenge_of(&options), &Tweak::default());
    post(&env, &seg, "api/register/finish", reg.json(), None)
        .await
        .ok(204);

    // The endpoints of the enrolment are closed while no code is pending: the 404 of an unknown
    // resource.
    let other = Authenticator::for_test_relay();
    register_options(&env, &seg, &code)
        .await
        .err(404, "not_found");
    let reg = other.create(&[1u8; 32], &Tweak::default());
    post(&env, &seg, "api/register/finish", reg.json(), None)
        .await
        .err(404, "not_found");
    // The code was used up; a new one (made on the server) opens the enrolment of a second key.
    let out = web(&env, WebAction::Enroll).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let second = Authenticator::for_test_relay();
    register(&env, &seg, &second, out.stdout.trim())
        .await
        .ok(204);
    let keys: i64 = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT count(*) FROM admin_web_credentials", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(keys, 2);
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state["enrolled"], true);
}

#[tokio::test]
async fn only_what_the_brief_allows_is_stored_about_the_key() {
    let env = panel().await;
    let seg = segment(&env);
    let auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);
    let columns: Vec<String> = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            let mut st =
                tx.prepare("SELECT name FROM pragma_table_info('admin_web_credential')")?;
            let rows = st
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(
        columns,
        [
            "id",
            "credential_id",
            "public_key",
            "sign_count",
            "created_at"
        ]
    );
    let (id, pk, count): (Vec<u8>, Vec<u8>, i64) = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(tx.query_row(
                "SELECT credential_id, public_key, sign_count FROM admin_web_credential",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(id, auth.credential_id);
    assert_eq!(pk, public_key_of(&auth));
    assert_eq!(count, 0);
    // The enrolment code is gone with the registration.
    let rows: i64 = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT count(*) FROM admin_web_enrollment", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn reset_removes_the_key_and_the_sessions_and_reopens_the_enrolment() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);

    // Refused without the confirmation, or with a wrong one: nothing changes.
    let none = web(&env, WebAction::Reset { confirm: None }).await;
    assert_eq!(none.exit, 2);
    let wrong = web(
        &env,
        WebAction::Reset {
            confirm: Some("00000000".into()),
        },
    )
    .await;
    assert_eq!(wrong.exit, 2);
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    assert!(admin::needs_exclusive_access(&Cmd::Web {
        action: WebAction::Reset { confirm: None }
    }));

    let done = web(
        &env,
        WebAction::Reset {
            confirm: Some(seg[..8].to_owned()),
        },
    )
    .await;
    assert_eq!(done.exit, 0, "{}", done.stderr);

    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state["enrolled"], false);
    login_options(&env, &seg).await.err(404, "not_found");

    // A new code is needed, and then a new key (the old one is not the key any more).
    let mut second = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &second, &code).await.ok(204);
    env.clock.advance_secs(120);
    let refused = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(refused.status, 401);
    env.clock.advance_secs(120);
    let ok = login_raw(&env, &seg, &mut second, &Tweak::default()).await;
    assert_eq!(ok.status, 204);
}

// ===== sign-in ===============================================================================

#[tokio::test]
async fn the_cookie_has_the_attributes_of_a_host_cookie() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);
    let resp = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(resp.status, 204);
    let line = resp
        .headers
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut parts = line.split("; ");
    let first = parts.next().unwrap();
    let (name, token) = first.split_once('=').unwrap();
    assert_eq!(name, COOKIE);
    assert_eq!(token.len(), 43, "256 bits in base64url");
    let attrs: Vec<&str> = parts.collect();
    for needed in ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"] {
        assert!(attrs.contains(&needed), "{needed} in {line}");
    }
    assert!(
        !attrs
            .iter()
            .any(|a| a.to_ascii_lowercase().starts_with("domain")),
        "a __Host- cookie has no Domain: {line}"
    );
    assert_eq!(resp.headers.get_all("set-cookie").iter().count(), 1);
}

#[tokio::test]
async fn a_challenge_works_once_and_for_five_minutes() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);

    // Replayed: the same answer a second time.
    let options = login_options(&env, &seg).await.ok(200);
    let assertion = auth.get(&challenge_of(&options), &Tweak::default());
    post(&env, &seg, "api/login/finish", assertion.json(), None)
        .await
        .ok(204);
    post(&env, &seg, "api/login/finish", assertion.json(), None)
        .await
        .err(401, "unauthorized");

    // Never issued.
    let invented = auth.get(&[5u8; 32], &Tweak::default());
    post(&env, &seg, "api/login/finish", invented.json(), None)
        .await
        .err(401, "unauthorized");

    // Expired, to the millisecond.
    let options = login_options(&env, &seg).await.ok(200);
    let assertion = auth.get(&challenge_of(&options), &Tweak::default());
    env.clock.advance_ms(5 * 60_000);
    post(&env, &seg, "api/login/finish", assertion.json(), None)
        .await
        .err(401, "unauthorized");
    let options = login_options(&env, &seg).await.ok(200);
    let assertion = auth.get(&challenge_of(&options), &Tweak::default());
    env.clock.advance_ms(5 * 60_000 - 1);
    post(&env, &seg, "api/login/finish", assertion.json(), None)
        .await
        .ok(204);

    // A credential id that is not the registered one.
    let options = login_options(&env, &seg).await.ok(200);
    let mut wrong_id = auth.get(&challenge_of(&options), &Tweak::default());
    wrong_id.id = vec![1; 32];
    post(&env, &seg, "api/login/finish", wrong_id.json(), None)
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn a_signature_counter_that_does_not_increase_is_refused() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);
    assert_eq!(
        login_raw(&env, &seg, &mut auth, &Tweak::default())
            .await
            .status,
        204
    );
    // Count 1 again: a clone of the key, or a replay.
    let stuck = Tweak {
        count: Some(1),
        ..Tweak::default()
    };
    assert_eq!(login_raw(&env, &seg, &mut auth, &stuck).await.status, 401);
    // The authenticator carries on from its own counter (3 now).
    assert_eq!(
        login_raw(&env, &seg, &mut auth, &Tweak::default())
            .await
            .status,
        204
    );
    let stored: i64 = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT sign_count FROM admin_web_credentials", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(stored, 3);
}

#[tokio::test]
async fn a_bad_sign_in_gets_the_same_answer_whatever_the_cause() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);
    let mut shapes = Vec::new();
    for tweak in [
        Tweak {
            bad_signature: true,
            ..Tweak::default()
        },
        Tweak {
            origin: Some("https://evil.test".into()),
            ..Tweak::default()
        },
        Tweak {
            rp_id: Some("other.test".into()),
            ..Tweak::default()
        },
        Tweak {
            flags: Some(FLAG_UP),
            ..Tweak::default()
        },
        Tweak {
            challenge: Some(vec![1; 32]),
            ..Tweak::default()
        },
    ] {
        // Each try takes two sensitive requests: let the shared bucket refill.
        env.clock.advance_secs(60);
        let r = login_raw(&env, &seg, &mut auth, &tweak).await;
        assert_eq!(r.status, 401);
        assert!(r.headers.get("set-cookie").is_none());
        shapes.push(shape(&r));
    }
    assert!(shapes.windows(2).all(|w| w[0] == w[1]), "{shapes:?}");
}

// ===== sessions ==============================================================================

#[tokio::test]
async fn a_session_ends_after_30_minutes_without_a_request() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let web_state = env.state.admin_web.as_ref().unwrap();
    assert_eq!(web_state.session_count(env.clock.now_ms()), 1);

    env.clock.advance_ms(30 * 60_000 - 1);
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    // That request counted as activity.
    env.clock.advance_ms(30 * 60_000 - 1);
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    env.clock.advance_ms(30 * 60_000);
    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
    assert_eq!(web_state.session_count(env.clock.now_ms()), 0);
    let state = get(&env, &seg, "api/state", Some(&cookie)).await.ok(200);
    assert_eq!(state["authenticated"], false);
}

#[tokio::test]
async fn a_session_ends_after_8_hours_whatever_the_activity() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    // A request every 20 minutes keeps it alive from the point of view of inactivity...
    for _ in 0..23 {
        env.clock.advance_ms(20 * 60_000);
        get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    }
    // ...7 h 40 min in; at 8 h exactly it is over.
    env.clock.advance_ms(20 * 60_000 - 1);
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    env.clock.advance_ms(1);
    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn only_the_exact_cookie_is_a_session() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let token = cookie.split_once('=').unwrap().1.to_owned();

    get(&env, &seg, "api/status", None)
        .await
        .err(401, "unauthorized");
    // Without the `__Host-` prefix it is another cookie.
    for bad in [
        format!("tomi-admin={token}"),
        format!("__Host-tomi-admin-x={token}"),
        format!("{COOKIE}=x{token}"),
        format!("{COOKIE}={}", &token[1..]),
        format!("{COOKIE}="),
        format!("{COOKIE}=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        format!("{COOKIE}= {token}"),
    ] {
        get(&env, &seg, "api/status", Some(&bad))
            .await
            .err(401, "unauthorized");
    }
    // Among other cookies it is found.
    let mixed = format!("theme=dark; {cookie}; other=1");
    get(&env, &seg, "api/status", Some(&mixed)).await.ok(200);

    // Signing out ends it for good.
    let out = post(&env, &seg, "api/logout", json!({}), Some(&cookie)).await;
    assert_eq!(out.status, 204);
    let gone = out.headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(
        gone.contains("Max-Age=0") && gone.starts_with(COOKIE),
        "{gone}"
    );
    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
}

#[tokio::test]
async fn a_post_needs_the_origin_of_the_relay_and_the_custom_header() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let body = json!({ "action": { "type": "create_invitation", "mode": "new" } });
    let attempts: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no Origin, no header", vec![]),
        ("no Origin", vec![("x-tomi-admin", "1")]),
        ("no header", vec![("origin", ORIGIN)]),
        (
            "another origin",
            vec![("origin", "https://evil.test"), ("x-tomi-admin", "1")],
        ),
        (
            "the origin with a path",
            vec![("origin", "https://relay.test/"), ("x-tomi-admin", "1")],
        ),
        (
            "http origin",
            vec![("origin", "http://relay.test"), ("x-tomi-admin", "1")],
        ),
        (
            "null origin",
            vec![("origin", "null"), ("x-tomi-admin", "1")],
        ),
        (
            "a wrong header",
            vec![("origin", ORIGIN), ("x-tomi-admin", "0")],
        ),
    ];
    for (what, headers) in attempts {
        for path in ["api/stepup/options", "api/actions"] {
            let r = post_with(&env, &seg, path, body.clone(), Some(&cookie), &headers).await;
            assert_eq!(
                (r.status, r.code()),
                (403, "forbidden".to_owned()),
                "{what} {path}"
            );
        }
    }
    // The same on the other writes.
    for path in [
        "api/register/options",
        "api/register/finish",
        "api/login/options",
        "api/login/finish",
        "api/logout",
        "api/spaces/quota",
        "api/invitations/cancel",
        "api/sessions/close-all",
    ] {
        let r = post_with(&env, &seg, path, json!({}), Some(&cookie), &[]).await;
        assert_eq!(r.status, 403, "{path}");
    }
    // Nothing was created by the refused ones.
    let list = get(&env, &seg, "api/invitations", Some(&cookie))
        .await
        .ok(200);
    assert_eq!(
        list["invitations"].as_array().unwrap().len(),
        1,
        "only the install one"
    );
    act(&env, &seg, body["action"].clone(), &cookie, &mut auth)
        .await
        .ok(200);
}

// ===== the default quota of a space ===========================================================

#[tokio::test]
async fn the_default_quota_is_read_and_changed_from_the_panel_behind_the_same_guards() {
    let env = Env::with(|cfg, _| {
        cfg.admin_web = true;
        cfg.max_space_bytes = None; // nothing configured: no limit per space
    })
    .await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(1).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let short = &env.space_id[..8];

    // Reading needs a session, like every other read.
    get(&env, &seg, "api/quota", None)
        .await
        .err(401, "unauthorized");
    let v = get(&env, &seg, "api/quota", Some(&cookie)).await.ok(200);
    assert!(
        v["default"]["bytes"].is_null(),
        "nothing set: no limit per space"
    );
    assert_eq!(v["default"]["source"], "none");
    assert!(v["default"].get("builtin_bytes").is_none());
    assert_eq!(v["default"]["min_bytes"], 1_048_576);
    assert_eq!(v["default"]["max_bytes"], 9_007_199_254_740_991u64);
    assert_eq!(v["default"]["relay_max_bytes"], 1_073_741_824u64);

    // Writing needs the Origin of the relay, the custom header and a session.
    let body = json!({ "default_bytes": 5_242_880 });
    let attempts: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no Origin, no header", vec![]),
        ("no Origin", vec![("x-tomi-admin", "1")]),
        ("no header", vec![("origin", ORIGIN)]),
        (
            "another origin",
            vec![("origin", "https://evil.test"), ("x-tomi-admin", "1")],
        ),
        (
            "null origin",
            vec![("origin", "null"), ("x-tomi-admin", "1")],
        ),
    ];
    for (what, headers) in attempts {
        let r = post_with(
            &env,
            &seg,
            "api/quota",
            body.clone(),
            Some(&cookie),
            &headers,
        )
        .await;
        assert_eq!(
            (r.status, r.code()),
            (403, "forbidden".to_owned()),
            "{what}"
        );
    }
    post(&env, &seg, "api/quota", body.clone(), None)
        .await
        .err(401, "unauthorized");
    assert_eq!(
        env.state.spaces.panel_default_quota(),
        None,
        "nothing was set"
    );

    // The page's own request: applied at once, to the space that has no quota of its own.
    let set = post(&env, &seg, "api/quota", body, Some(&cookie))
        .await
        .ok(200);
    assert_eq!(set["default"]["bytes"], 5_242_880);
    assert_eq!(set["default"]["source"], "panel");
    let spaces = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    assert_eq!(spaces["spaces"][0]["max_bytes"], 5_242_880);
    assert_eq!(spaces["spaces"][0]["quota_source"], "default");
    assert_eq!(spaces["spaces"][0]["id"], short);
    let status = get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    assert_eq!(status["default_space_quota"]["bytes"], 5_242_880);

    // A quota of the space itself (the CLI) stays above the default.
    let out = admin::run(
        &env.state,
        Cmd::Quota {
            space: Some(short.to_owned()),
            value: "9000000".into(),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let spaces = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    assert_eq!(spaces["spaces"][0]["max_bytes"], 9_000_000);
    assert_eq!(spaces["spaces"][0]["quota_source"], "space");

    // 0 is no limit per space; null goes back to what the configuration says (here nothing: no limit).
    let none = post(
        &env,
        &seg,
        "api/quota",
        json!({ "default_bytes": 0 }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(none["default"]["bytes"], Value::Null);
    assert_eq!(none["default"]["source"], "panel");
    let back = post(
        &env,
        &seg,
        "api/quota",
        json!({ "default_bytes": null }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert!(
        back["default"]["bytes"].is_null(),
        "back to nothing: no limit"
    );
    assert_eq!(back["default"]["source"], "none");
    assert_eq!(env.state.spaces.panel_default_quota(), None);
}

#[tokio::test]
async fn the_default_quota_refuses_what_is_not_a_sensible_number() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    for (what, body) in [
        ("missing", json!({})),
        ("text", json!({ "default_bytes": "5000000" })),
        ("a fraction", json!({ "default_bytes": 5_000_000.5 })),
        ("negative", json!({ "default_bytes": -1 })),
        ("below the minimum", json!({ "default_bytes": 1 })),
        (
            "just below the minimum",
            json!({ "default_bytes": 1_048_575 }),
        ),
        (
            "above 2^53 - 1",
            json!({ "default_bytes": 9_007_199_254_740_992u64 }),
        ),
        ("a boolean", json!({ "default_bytes": true })),
        ("a list", json!({ "default_bytes": [5_000_000] })),
    ] {
        let r = post(&env, &seg, "api/quota", body, Some(&cookie)).await;
        assert_eq!(
            (r.status, r.code()),
            (400, "bad_request".to_owned()),
            "{what}"
        );
        assert!(
            !r.json().to_string().contains("5000000"),
            "{what}: the value is not echoed"
        );
    }
    // Not JSON at all.
    let r = env
        .send(
            "POST",
            &format!("/{seg}/api/quota"),
            &[
                ("origin", ORIGIN),
                ("x-tomi-admin", "1"),
                ("cookie", cookie.as_str()),
                ("content-type", "application/json"),
            ],
            None,
        )
        .await;
    assert_eq!(r.status, 400);
    // The bounds themselves are accepted.
    for ok in [1_048_576u64, 9_007_199_254_740_991u64, 0] {
        post(
            &env,
            &seg,
            "api/quota",
            json!({ "default_bytes": ok }),
            Some(&cookie),
        )
        .await
        .ok(200);
    }
    // Nothing else of the panel moved: no other route accepts a quota.
    let r = post(
        &env,
        &seg,
        "api/quotas",
        json!({ "default_bytes": 5_000_000 }),
        Some(&cookie),
    )
    .await;
    assert_eq!(r.status, 404);
}

#[tokio::test]
async fn the_default_quota_of_the_panel_survives_a_restart_and_the_page_offers_it() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    post(
        &env,
        &seg,
        "api/quota",
        json!({ "default_bytes": 3_145_728 }),
        Some(&cookie),
    )
    .await
    .ok(200);
    // The page carries the controls (no inline code: the script is the one that is served).
    let js = String::from_utf8_lossy(&get(&env, &seg, "app.js", None).await.body).into_owned();
    let html = String::from_utf8_lossy(&get(&env, &seg, "", None).await.body).into_owned();
    for needle in ["api/quota", "default_bytes"] {
        assert!(js.contains(needle), "{needle}");
    }
    for id in [
        "quota-form",
        "quota-mib",
        "quota-save",
        "quota-unlimited",
        "quota-reset",
    ] {
        assert!(html.contains(&format!("id=\"{id}\"")), "{id}");
    }
    let env = env.restart().await;
    assert_eq!(
        env.state.spaces.default_space_quota(&env.state.cfg),
        (Some(3_145_728), "panel")
    );
}

#[tokio::test]
async fn sensitive_requests_are_limited_for_everyone() {
    let env = panel().await;
    let seg = segment(&env);
    let _ = enroll_code(&env).await;
    for _ in 0..10 {
        register_options(&env, &seg, "0000-0000-0000")
            .await
            .err(401, "unauthorized");
        // Keep the code alive for the whole loop.
        let _ = enroll_code(&env).await;
    }
    let r = register_options(&env, &seg, "0000-0000-0000").await;
    r.err(429, "rate_limited");
    assert!(r.headers.get("retry-after").is_some());
    env.clock.advance_secs(60);
    register_options(&env, &seg, "0000-0000-0000")
        .await
        .err(401, "unauthorized");
}

// ===== off, and the wrong path ================================================================

#[tokio::test]
async fn a_panel_that_is_off_answers_like_an_unknown_resource() {
    let env = Env::new().await;
    assert!(env.state.admin_web.is_none());
    let unknown = unknown_shape(&env).await;
    // Even a path that was a segment once, in a database that remembers it.
    let seg = "AAAAAAAAAAAAAAAAAAAAAA";
    env.state
        .spaces
        .routing()
        .write_sync(|tx| {
            tx.execute(
                "INSERT INTO admin_web_path (id, segment, created_at) VALUES (1, ?1, 0)",
                [seg],
            )?;
            Ok(tomi_relay::db::Tx::Commit(()))
        })
        .unwrap();
    for (method, path) in [
        ("GET", format!("/{seg}")),
        ("GET", format!("/{seg}/")),
        ("GET", format!("/{seg}/app.js")),
        ("GET", format!("/{seg}/app.css")),
        ("GET", format!("/{seg}/api/state")),
        ("GET", format!("/{seg}/api/status")),
        ("POST", format!("/{seg}/api/login/options")),
        ("POST", format!("/{seg}/api/register/options")),
        ("DELETE", format!("/{seg}/")),
        ("GET", "/admin".to_owned()),
        ("GET", "/admin/".to_owned()),
    ] {
        let r = env
            .send(
                method,
                &path,
                &[("origin", ORIGIN), ("x-tomi-admin", "1")],
                (method == "POST").then(|| json!({})),
            )
            .await;
        assert_eq!(shape(&r), unknown, "{method} {path}");
    }
    // The command says the panel is off.
    let out = web(&env, WebAction::Show).await;
    assert_eq!(out.exit, 1);
    assert!(out.stderr.contains("admin_web"));
}

#[tokio::test]
async fn a_wrong_path_answers_like_an_unknown_resource_even_when_the_panel_is_on() {
    let env = panel().await;
    let seg = segment(&env);
    assert_eq!(seg.len(), 22);
    let unknown = unknown_shape(&env).await;
    let mut flipped = seg.clone().into_bytes();
    flipped[21] = if flipped[21] == b'A' { b'B' } else { b'A' };
    let flipped = String::from_utf8(flipped).unwrap();
    for wrong in [
        flipped,
        seg[..21].to_owned(),
        format!("{seg}x"),
        "admin".to_owned(),
    ] {
        for (method, tail) in [
            ("GET", ""),
            ("GET", "/"),
            ("GET", "/app.js"),
            ("GET", "/api/state"),
            ("GET", "/api/status"),
            ("POST", "/api/login/options"),
            ("POST", "/api/register/options"),
        ] {
            let path = format!("/{wrong}{tail}");
            let r = env
                .send(
                    method,
                    &path,
                    &[("origin", ORIGIN), ("x-tomi-admin", "1")],
                    (method == "POST").then(|| json!({})),
                )
                .await;
            assert_eq!(shape(&r), unknown, "{method} {path}");
        }
    }
    // And the real one is not that.
    assert_ne!(get(&env, &seg, "", None).await.status, 404);
}

// ===== the panel itself ======================================================================

#[tokio::test]
async fn every_answer_of_the_panel_carries_the_security_headers() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let mut answers = vec![
        get(&env, &seg, "", None).await,
        get(&env, &seg, "app.js", None).await,
        get(&env, &seg, "app.css", None).await,
        get(&env, &seg, "api/state", None).await,
        get(&env, &seg, "api/status", None).await,
        get(&env, &seg, "api/status", Some(&cookie)).await,
        get(&env, &seg, "api/nothing", Some(&cookie)).await,
        post_with(&env, &seg, "api/actions", json!({}), Some(&cookie), &[]).await,
        post(
            &env,
            &seg,
            "api/register/options",
            json!({"code": "x"}),
            None,
        )
        .await,
        env.send("GET", &format!("/{seg}"), &[], None).await,
    ];
    answers.push(post(&env, &seg, "api/logout", json!({}), Some(&cookie)).await);
    let statuses: Vec<u16> = answers.iter().map(|r| r.status).collect();
    assert_eq!(
        statuses,
        [200, 200, 200, 200, 401, 200, 404, 403, 404, 308, 204]
    );
    for r in &answers {
        assert_eq!(r.header("content-security-policy").as_deref(), Some(CSP));
        assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
        assert_eq!(r.header("referrer-policy").as_deref(), Some("no-referrer"));
        assert_eq!(
            r.header("x-content-type-options").as_deref(),
            Some("nosniff")
        );
    }
    // The redirect only adds the slash.
    assert_eq!(answers[9].header("location"), Some(format!("/{seg}/")));
    // Types.
    assert!(answers[0]
        .header("content-type")
        .unwrap()
        .starts_with("text/html"));
    assert!(answers[1]
        .header("content-type")
        .unwrap()
        .starts_with("text/javascript"));
    assert!(answers[2]
        .header("content-type")
        .unwrap()
        .starts_with("text/css"));
}

#[tokio::test]
async fn the_page_has_no_inline_code_no_third_party_and_is_in_french() {
    let env = panel().await;
    let seg = segment(&env);
    let html = String::from_utf8(get(&env, &seg, "", None).await.body.to_vec()).unwrap();
    let js = String::from_utf8(get(&env, &seg, "app.js", None).await.body.to_vec()).unwrap();
    let css = String::from_utf8(get(&env, &seg, "app.css", None).await.body.to_vec()).unwrap();
    assert!(html.contains("<html lang=\"fr\">"));
    // Every script is a file of ours; no style block or attribute; no inline handler.
    for tag in html.split("<script").skip(1) {
        let head = tag.split('>').next().unwrap();
        assert!(head.contains("src=\"app.js\""), "{head}");
    }
    assert!(!html.contains("<style") && !html.contains(" style="));
    for handler in [
        " onclick=",
        " onload=",
        " onerror=",
        " onsubmit=",
        "javascript:",
    ] {
        assert!(!html.contains(handler), "{handler}");
    }
    for (name, text) in [("html", &html), ("js", &js), ("css", &css)] {
        for forbidden in [
            "http://",
            "https://",
            "//cdn",
            "@import",
            "url(",
            "eval(",
            "innerHTML",
        ] {
            assert!(!text.contains(forbidden), "{forbidden} in {name}");
        }
    }
    // Every control has a label; the form is the one of the enrolment.
    assert!(html.contains("<label for=\"code\">"));
    assert!(html.contains("Se connecter avec ma clé d’accès"));
    assert!(html.contains("Créer ma clé d’accès"));
    // The page asks for ES256, no attestation, user verification required.
    assert!(
        js.contains("navigator.credentials.create") && js.contains("navigator.credentials.get")
    );
}

#[tokio::test]
async fn registration_options_ask_for_es256_none_and_user_verification() {
    let env = panel().await;
    let seg = segment(&env);
    let code = enroll_code(&env).await;
    let o = register_options(&env, &seg, &code).await.ok(200);
    let pk = &o["publicKey"];
    assert_eq!(pk["rp"]["id"], RP_ID);
    assert_eq!(pk["attestation"], "none");
    assert_eq!(pk["authenticatorSelection"]["userVerification"], "required");
    assert_eq!(
        pk["pubKeyCredParams"],
        json!([{ "type": "public-key", "alg": -7 }])
    );
    assert_eq!(unb64(pk["challenge"].as_str().unwrap()).len(), 32);
    // Two requests, two challenges.
    let again = register_options(&env, &seg, &code).await.ok(200);
    assert_ne!(again["publicKey"]["challenge"], pk["challenge"]);
}

// ===== the path ==============================================================================

#[tokio::test]
async fn the_path_is_128_random_bits_and_survives_a_restart_and_a_rotation_replaces_it() {
    let env = panel().await;
    let seg = segment(&env);
    assert_eq!(seg.len(), 22);
    assert_eq!(unb64(&seg).len(), 16);
    // A second open of the same database finds the same segment.
    let again =
        tomi_relay::admin_web::AdminWeb::open(&env.state.cfg, env.state.spaces.routing(), 0)
            .unwrap();
    assert_eq!(again.segment(), seg);

    let shown = web(&env, WebAction::Show).await;
    assert_eq!(shown.exit, 0);
    assert_eq!(
        shown.stdout.lines().next().unwrap(),
        format!("https://relay.test/{seg}/")
    );

    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);

    let unknown = unknown_shape(&env).await;
    let rotated = web(&env, WebAction::RotatePath).await;
    assert_eq!(rotated.exit, 0, "{}", rotated.stderr);
    let new_seg = rotated
        .stdout
        .trim()
        .strip_prefix("https://relay.test/")
        .and_then(|s| s.strip_suffix('/'))
        .unwrap()
        .to_owned();
    assert_ne!(new_seg, seg);
    assert_eq!(segment(&env), new_seg);
    // The old path is nothing, the new one works, and the sessions are gone.
    assert_eq!(shape(&get(&env, &seg, "", None).await), unknown);
    assert_eq!(
        shape(&get(&env, &seg, "api/status", Some(&cookie)).await),
        unknown
    );
    assert_eq!(get(&env, &new_seg, "", None).await.status, 200);
    get(&env, &new_seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
    // The passkey is still there: signing in again works at the new path.
    let ok = login_raw(&env, &new_seg, &mut auth, &Tweak::default()).await;
    assert_eq!(ok.status, 204);
}

#[tokio::test]
async fn the_tables_are_added_to_an_existing_routing_database_without_a_version_bump() {
    let dir = tempfile::tempdir().unwrap();
    {
        // A routing.db as the previous relay made it.
        let conn = rusqlite::Connection::open(dir.path().join("routing.db")).unwrap();
        conn.execute_batch(tomi_relay::db::ROUTING_SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO invitations (token_hash, created_at) VALUES (zeroblob(32), 5)",
            [],
        )
        .unwrap();
    }
    let env = Env::in_dir(dir, |cfg, _| cfg.admin_web = true).await;
    let (version, invitations, tables): (i64, i64, i64) = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok((
                tx.query_row("PRAGMA user_version", [], |r| r.get(0))?,
                tx.query_row("SELECT count(*) FROM invitations", [], |r| r.get(0))?,
                tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name LIKE 'admin_web_%'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(version, 1);
    assert_eq!(invitations, 1, "what was there is untouched");
    assert_eq!(tables, 6);
    assert_eq!(segment(&env).len(), 22);
}

#[tokio::test]
async fn a_command_that_only_looks_never_creates_the_panel_tables() {
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = rusqlite::Connection::open(dir.path().join("routing.db")).unwrap();
        conn.execute_batch(tomi_relay::db::ROUTING_SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let before = std::fs::read(dir.path().join("routing.db")).unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.admin_web = true;
    let state = open(
        cfg,
        Options {
            push: PushChoice::Disabled,
            background: false,
            hold_lock: false,
            read_only: true,
            cleanup: false,
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert!(
        state.admin_web.is_none(),
        "a read-only relay never serves the panel"
    );
    let out = admin::run(
        &state,
        Cmd::Web {
            action: WebAction::Show,
        },
    )
    .await;
    assert_eq!(out.exit, 1, "{}", out.stderr);
    assert!(out.stderr.contains("no path yet"));
    assert_eq!(
        std::fs::read(dir.path().join("routing.db")).unwrap(),
        before
    );
}

// ===== the whole way =========================================================================

#[tokio::test]
async fn from_the_enrolment_to_an_invitation() {
    let env = panel().await;
    let seg = segment(&env);
    let (devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();

    // (a) not enrolled: the page shows the code field.
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state, json!({ "enrolled": false, "authenticated": false }));
    let code = enroll_code(&env).await;
    register(&env, &seg, &auth, &code).await.ok(204);

    // (b) enrolled: a sign-in.
    let state = get(&env, &seg, "api/state", None).await.ok(200);
    assert_eq!(state, json!({ "enrolled": true, "authenticated": false }));
    let resp = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(resp.status, 204);
    let cookie = cookie_of(&resp);

    // (c) the dashboard.
    let status = get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    assert_eq!(status["relay_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["spaces"], 1);
    assert_eq!(status["invitations_pending"], 0);
    assert!(status["disk_free_bytes"].is_u64());
    assert_eq!(status["fcm"]["state"], "disabled");
    // The same function as `admin status --json`.
    let cli = admin::run(
        &env.state,
        Cmd::Status {
            json: true,
            space: None,
        },
    )
    .await;
    assert_eq!(serde_json::from_str::<Value>(&cli.stdout).unwrap(), status);

    let spaces = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    let rows = spaces["spaces"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let short = rows[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(short, env.space_id[..8]);
    assert_eq!(rows[0]["members"], 2);
    assert!(rows[0].get("used_bytes").is_some() && rows[0].get("max_bytes").is_some());

    let devices = get(
        &env,
        &seg,
        &format!("api/devices?space={short}"),
        Some(&cookie),
    )
    .await
    .ok(200);
    let list = devices["devices"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let mut keys: Vec<&str> = list[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "active",
            "app_version",
            "fingerprint",
            "id",
            "last_seen_at",
            "name",
            "platform",
            "present",
            "push",
            "registered_at"
        ],
        "only what the panel shows: no device_id, cursor or format range"
    );
    let names: Vec<&str> = list.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert!(names.contains(&devs[0].name.as_str()));
    // Unknown, ambiguous or missing space.
    get(&env, &seg, "api/devices?space=ffffffff", Some(&cookie))
        .await
        .err(404, "not_found");
    get(&env, &seg, "api/devices?space=xyz", Some(&cookie))
        .await
        .err(400, "bad_request");
    get(&env, &seg, "api/devices", Some(&cookie))
        .await
        .err(400, "bad_request");

    // Invitations: listed without their token, created with it shown once.
    let list = get(&env, &seg, "api/invitations", Some(&cookie))
        .await
        .ok(200);
    assert_eq!(list["invitations"], json!([]));
    let created = act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200);
    let token = created["token"].as_str().unwrap().to_owned();
    // The link that creates a space: the origin of `public_url`, the token in the fragment.
    assert_eq!(
        created["link"].as_str().unwrap(),
        format!("{ORIGIN}/create#{token}")
    );
    assert_eq!(created["invitations"].as_array().unwrap().len(), 1);
    let list = get(&env, &seg, "api/invitations", Some(&cookie))
        .await
        .ok(200);
    assert_eq!(list["invitations"].as_array().unwrap().len(), 1);
    assert!(
        !list.to_string().contains(&token),
        "the token is shown once"
    );
    // The token is a real one: it creates a second space.
    let other = env.for_space(&new_uuid());
    let founder = Dev::new("PC-2");
    other.create_space_raw(&founder, &token).await.ok(201);
    let status = get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    assert_eq!(
        (
            status["spaces"].as_u64(),
            status["invitations_pending"].as_u64()
        ),
        (Some(2), Some(0))
    );

    // Rotation replaces the pending ones.
    let first = act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200)["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "rotate" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200);
    assert_ne!(second["token"].as_str().unwrap(), first);
    assert_eq!(second["invitations"].as_array().unwrap().len(), 1);
    assert!(!tomi_relay::install::is_pending(env.state.spaces.routing(), &first).unwrap());
    act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "wat" }),
        &cookie,
        &mut auth,
    )
    .await
    .err(400, "bad_request");

    // Nothing of a space leaks: not its whole id, not a key, not a device certificate.
    let mut everything = String::new();
    for path in ["api/status", "api/spaces", "api/invitations"] {
        everything.push_str(&String::from_utf8_lossy(
            &get(&env, &seg, path, Some(&cookie)).await.body,
        ));
    }
    everything.push_str(&String::from_utf8_lossy(
        &get(
            &env,
            &seg,
            &format!("api/devices?space={short}"),
            Some(&cookie),
        )
        .await
        .body,
    ));
    assert!(!everything.contains(&env.space_id));
    for d in &devs {
        assert!(!everything.contains(&b64(&d.pk())) && !everything.contains(&hex::encode(d.pk())));
        assert!(!everything.contains(d.tok()));
    }

    // The panel offers no revoke, no reset, and no quota of one space (only the default one,
    // tested apart).
    for (method, path) in [
        ("POST", "api/revoke"),
        ("POST", "api/reset"),
        ("POST", "api/quota/space"),
        ("DELETE", "api/devices"),
        ("POST", "api/devices"),
        ("PUT", "api/quota"),
    ] {
        let r = env
            .send(
                method,
                &format!("/{seg}/{path}"),
                &[
                    ("origin", ORIGIN),
                    ("x-tomi-admin", "1"),
                    ("cookie", cookie.as_str()),
                ],
                Some(json!({})),
            )
            .await;
        assert_eq!(r.status, 404, "{method} {path}");
    }
    assert_eq!(devs.len(), 2);
    let after = get(
        &env,
        &seg,
        &format!("api/devices?space={short}"),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(after["devices"].as_array().unwrap().len(), 2);

    // Signing out.
    post(&env, &seg, "api/logout", json!({}), Some(&cookie))
        .await
        .ok(204);
    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
}

// ===== what was added for the operator (additive) ============================================

#[tokio::test]
async fn the_spaces_carry_counts_dates_and_a_pending_key_change_but_nothing_secret() {
    let env = panel().await;
    let seg = segment(&env);
    let (devs, _) = env.space_of(3).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;

    // One item with one blob, so the counts are not all zero.
    let blob = env.upload_blob(&devs[0], &vec![7u8; 5000], 5000).await;
    let (_, r) = env.send_item(&devs[0], &[&blob]).await;
    assert_eq!(r.status / 100, 2, "{}", String::from_utf8_lossy(&r.body));

    let rows = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    let row = &rows["spaces"][0];
    assert_eq!(row["members"], 3);
    assert_eq!(row["items"], 1);
    assert_eq!(row["blobs"], 1);
    assert!(row["events"].as_i64().unwrap() > 0);
    assert_eq!(row["head_seq"], env.head_seq().await);
    assert!(row["retention_floor_seq"].is_i64());
    assert_eq!(row["rotation_required"], false);
    assert!(row["present"].is_u64());
    // The last activity is the latest `last_seen_at` of a member.
    let cli_devices = admin::run(
        &env.state,
        Cmd::Devices {
            json: true,
            space: None,
        },
    )
    .await;
    let cli_devices: Value = serde_json::from_str(&cli_devices.stdout).unwrap();
    let latest = cli_devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["last_seen_at"].as_str())
        .max()
        .unwrap()
        .to_owned();
    assert_eq!(row["last_activity_at"], latest.as_str());
    // Every device row says when it joined.
    assert!(cli_devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .all(|d| d["registered_at"].is_string()));

    // The same figures as `admin spaces --json` (which has the whole id; the panel the short one).
    let cli = admin::run(&env.state, Cmd::Spaces { json: true }).await;
    let cli: Value = serde_json::from_str(&cli.stdout).unwrap();
    for key in [
        "members",
        "active",
        "present",
        "last_activity_at",
        "used_bytes",
        "items",
        "blobs",
        "events",
        "head_seq",
        "retention_floor_seq",
        "rotation_required",
    ] {
        assert_eq!(cli["spaces"][0][key], row[key], "{key}");
    }

    // A revocation from the server: the panel shows the key change that is now due.
    let out = admin::run(
        &env.state,
        Cmd::Revoke {
            target: devs[2].id.clone(),
            space: None,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let rows = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    assert_eq!(rows["spaces"][0]["rotation_required"], true);
    assert_eq!(rows["spaces"][0]["members"], 2);

    // Still nothing of a space that the relay would not already show: no whole id, no device id,
    // no key, no token.
    let body = String::from_utf8_lossy(&get(&env, &seg, "api/spaces", Some(&cookie)).await.body)
        .into_owned();
    assert!(!body.contains(&env.space_id));
    for d in &devs {
        assert!(!body.contains(&d.id) && !body.contains(d.tok()));
        assert!(!body.contains(&b64(&d.pk())) && !body.contains(&hex::encode(d.pk())));
    }
}

#[tokio::test]
async fn about_gives_the_uptime_the_end_of_the_session_and_the_limits() {
    let env = panel().await;
    let seg = segment(&env);
    let started = env.state.admin_web.as_ref().unwrap().started_ms();
    // A session is needed, like for every read of the dashboard.
    get(&env, &seg, "api/about", None)
        .await
        .err(401, "unauthorized");

    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let signed_at = env.clock.now_ms();
    env.clock.advance_ms(90_000);
    let now = env.clock.now_ms();
    let r = get(&env, &seg, "api/about", Some(&cookie)).await;
    assert_eq!(r.header("content-security-policy").as_deref(), Some(CSP));
    assert_eq!(r.header("cache-control").as_deref(), Some("no-store"));
    let a = r.ok(200);
    assert_eq!(a["relay_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(a["started_at"], ts(started));
    assert_eq!(a["server_time"], ts(now));
    assert_eq!(a["uptime_secs"], (now - started) / 1000);
    assert_eq!(a["session"]["idle_ttl_secs"], 1800);
    assert_eq!(a["session"]["absolute_ttl_secs"], 8 * 3600);
    assert_eq!(a["session"]["idle_ends_at"], ts(now + 30 * 60_000));
    assert_eq!(
        a["session"]["absolute_ends_at"],
        ts(signed_at + 8 * 3_600_000)
    );
    assert_eq!(a["session"]["ends_at"], ts(now + 30 * 60_000));
    assert_eq!(a["push_configured"], false);
    let cfg = &env.state.cfg;
    let l = &a["limits"];
    assert_eq!(l["max_relay_bytes"], cfg.max_relay_bytes);
    assert_eq!(l["max_devices"], cfg.max_devices);
    assert_eq!(l["event_retention_days"], cfg.event_retention_days);
    assert_eq!(l["inactive_after_days"], cfg.inactive_after_days);
    assert_eq!(l["max_envelope_bytes"], cfg.max_envelope_bytes);
    assert_eq!(l["max_blob_bytes"], cfg.max_blob_bytes);
    assert_eq!(l["min_free_disk_bytes"], cfg.min_free_disk_bytes);
    assert_eq!(
        l["max_space_bytes"],
        json!(cfg.configured_space_quota()),
        "null when the configuration sets no quota per space"
    );

    // Reading it counts as activity, like any request of the dashboard.
    env.clock.advance_ms(29 * 60_000);
    get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    env.clock.advance_ms(29 * 60_000);
    let a = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(
        a["uptime_secs"],
        (env.clock.now_ms() - started) / 1000,
        "the uptime moves with the clock"
    );
    // Near the 8 hours, the end of the session is the hard one.
    for _ in 0..16 {
        env.clock.advance_ms(25 * 60_000);
        get(&env, &seg, "api/status", Some(&cookie)).await.ok(200);
    }
    let a = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(a["session"]["ends_at"], ts(signed_at + 8 * 3_600_000));

    // No path, no URL, no file: numbers and dates.
    let text = a.to_string();
    assert!(!text.contains(&seg));
    assert!(!text.contains("relay.test"));
    assert!(!text.contains(&env.data_dir().display().to_string()));
}

#[tokio::test]
async fn the_journal_says_what_was_done_with_fixed_words_and_nothing_from_a_request() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(1).await;
    let mut auth = Authenticator::for_test_relay();

    // Two wrong codes (folded into one line), the key, one refused sign-in, one sign-in.
    let code = enroll_code(&env).await;
    register_options(&env, &seg, "AAAA-BBBB-CCCC")
        .await
        .err(401, "unauthorized");
    register_options(&env, &seg, "AAAA-BBBB-DDDD")
        .await
        .err(401, "unauthorized");
    register(&env, &seg, &auth, &code).await.ok(204);
    let bad = Tweak {
        bad_signature: true,
        ..Tweak::default()
    };
    assert_eq!(login_raw(&env, &seg, &mut auth, &bad).await.status, 401);
    let resp = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(resp.status, 204);
    let cookie = cookie_of(&resp);
    get(&env, &seg, "api/journal", None)
        .await
        .err(401, "unauthorized");

    // Invitations and the default quota.
    let first = act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200)["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "rotate" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200)["token"]
        .as_str()
        .unwrap()
        .to_owned();
    for value in [json!(5_242_880), json!(0), Value::Null] {
        post(
            &env,
            &seg,
            "api/quota",
            json!({ "default_bytes": value }),
            Some(&cookie),
        )
        .await
        .ok(200);
    }
    // A refused value is not an action.
    post(
        &env,
        &seg,
        "api/quota",
        json!({ "default_bytes": 12 }),
        Some(&cookie),
    )
    .await
    .err(400, "bad_request");

    let r = get(&env, &seg, "api/journal", Some(&cookie)).await;
    let body = String::from_utf8_lossy(&r.body).into_owned();
    let j = r.ok(200);
    assert_eq!(j["max_entries"], 100);
    assert_eq!(
        j["since"],
        ts(env.state.admin_web.as_ref().unwrap().started_ms())
    );
    let kinds: Vec<&str> = j["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "default_quota_config",
            "default_quota_unlimited",
            "default_quota_set",
            "invitations_replaced",
            "invitation_created",
            "signed_in",
            "sign_in_refused",
            "passkey_registered",
            "enrollment_refused",
        ],
        "newest first"
    );
    let entries = j["entries"].as_array().unwrap();
    assert_eq!(entries[2]["bytes"], 5_242_880);
    assert_eq!(entries[8]["count"], 2, "two refusals in a row, one line");
    assert!(entries.iter().all(|e| e["at"].is_string()
        && e["first_at"].is_string()
        && e["count"].as_u64().unwrap() >= 1));
    // Only the quota change carries a value, and it is a number of bytes.
    assert!(entries
        .iter()
        .enumerate()
        .all(|(i, e)| (i == 2) == e.get("bytes").is_some()));

    // Nothing that came with a request, nothing secret, nothing about a space.
    let cookie_value = cookie.split_once('=').unwrap().1;
    for secret in [
        first.as_str(),
        second.as_str(),
        code.as_str(),
        "AAAA-BBBB",
        seg.as_str(),
        cookie_value,
        env.space_id.as_str(),
        &env.space_id[..8],
        &b64_encode(&auth.credential_id),
        "127.0.0.1",
    ] {
        assert!(!body.contains(secret), "{secret}");
    }

    // A sign-out, then a new path from the server: both are lines, read after a new sign-in.
    post(&env, &seg, "api/logout", json!({}), Some(&cookie))
        .await
        .ok(204);
    assert_eq!(web(&env, WebAction::RotatePath).await.exit, 0);
    let seg = segment(&env);
    let resp = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(resp.status, 204);
    let cookie = cookie_of(&resp);
    let j = get(&env, &seg, "api/journal", Some(&cookie)).await.ok(200);
    let kinds: Vec<&str> = j["entries"]
        .as_array()
        .unwrap()
        .iter()
        .take(3)
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["signed_in", "path_rotated", "signed_out"]);
}

#[tokio::test]
async fn the_journal_is_bounded_and_a_flood_of_refusals_is_one_line() {
    use tomi_relay::admin_web::{JournalKind, JOURNAL_LEN};
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let web_state = env.state.admin_web.as_ref().unwrap();
    let now = env.clock.now_ms();
    for _ in 0..500 {
        web_state.record(JournalKind::SignInRefused, None, now);
    }
    let j = get(&env, &seg, "api/journal", Some(&cookie)).await.ok(200);
    assert_eq!(j["entries"][0]["kind"], "sign_in_refused");
    assert_eq!(j["entries"][0]["count"], 500);
    assert_eq!(
        j["entries"][1]["kind"], "signed_in",
        "the flood did not push the rest out"
    );
    // Lines that differ each take a line, and the oldest make room.
    for i in 0..(JOURNAL_LEN + 30) {
        let kind = if i % 2 == 0 {
            JournalKind::InvitationCreated
        } else {
            JournalKind::SignedIn
        };
        web_state.record(kind, None, now);
    }
    let j = get(&env, &seg, "api/journal", Some(&cookie)).await.ok(200);
    assert_eq!(j["entries"].as_array().unwrap().len(), JOURNAL_LEN);
}

#[tokio::test]
async fn the_page_offers_the_new_views_without_inline_code_or_a_secret_in_storage() {
    let env = panel().await;
    let seg = segment(&env);
    let html = String::from_utf8(get(&env, &seg, "", None).await.body.to_vec()).unwrap();
    let js = String::from_utf8(get(&env, &seg, "app.js", None).await.body.to_vec()).unwrap();
    let css = String::from_utf8(get(&env, &seg, "app.css", None).await.body.to_vec()).unwrap();
    for id in [
        "refresh",
        "alerts",
        "theme",
        "session-info",
        "devices-close",
        "space-commands",
        "journal-list",
        "about-facts",
        "limits-facts",
        "diagnostic",
        "diagnostic-copy",
        "confirm-dialog",
    ] {
        assert!(html.contains(&format!("id=\"{id}\"")), "{id}");
    }
    for needle in ["api/about", "api/journal", "showModal"] {
        assert!(js.contains(needle), "{needle}");
    }
    // A confirmation is a dialog of the page, not a browser prompt (kept only as a fallback).
    assert_eq!(js.matches("window.confirm(").count(), 1);
    // The theme is the only thing the page keeps in the browser; the token never is.
    assert_eq!(js.matches("localStorage.setItem(").count(), 1);
    assert!(js.contains("localStorage.setItem(THEME_KEY"));
    assert!(!js.contains("sessionStorage") && !js.contains("indexedDB"));
    assert!(!js.contains("document.cookie"));
    // The dialog does not submit a form (form-action 'none').
    assert!(!html.contains("method=\"dialog\""));
    // Both themes, with a manual choice that wins over the system.
    assert!(css.contains(":root[data-theme=\"dark\"]"));
    assert!(css.contains(":root:not([data-theme=\"light\"])"));
}

#[test]
fn the_unit_helpers_of_the_tests_are_in_use() {
    // `b64_encode` and `cbor_int` are the encoders the tests above lean on.
    assert_eq!(b64_encode(&[1, 2, 3, 4]), "AQIDBA");
    assert_eq!(cbor_int(-7), vec![0x26]);
}

#[tokio::test]
async fn the_creation_link_follows_public_url_never_the_host_header_and_is_shown_once() {
    let env = panel().await;
    let seg = segment(&env);
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    // The request claims another host in every way a proxy or a client could: the link does not move.
    let created = act_with(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
        &[
            ("host", "evil.example"),
            ("x-forwarded-host", "evil.example"),
            ("forwarded", "host=evil.example"),
        ],
    )
    .await
    .ok(200);
    let token = created["token"].as_str().unwrap();
    let link = created["link"].as_str().unwrap();
    assert_eq!(link, format!("{ORIGIN}/create#{token}"));
    assert_eq!(
        link,
        tomi_relay::admin_web::creation_link("https://relay.test", token)
    );
    assert!(!link.contains("evil"));
    // Shown once: neither the list, nor the status, nor the journal carries the token or the link.
    for path in ["api/invitations", "api/status", "api/journal", "api/about"] {
        let text = get(&env, &seg, path, Some(&cookie))
            .await
            .ok(200)
            .to_string();
        assert!(!text.contains(token), "{path}");
        assert!(!text.contains("/create#"), "{path}");
    }
    // The page offers the bare token, the copiable link and the QR code of the link.
    let html = String::from_utf8(get(&env, &seg, "", None).await.body.to_vec()).unwrap();
    let js = String::from_utf8(get(&env, &seg, "app.js", None).await.body.to_vec()).unwrap();
    for id in [
        "id=\"token\"",
        "id=\"token-link\"",
        "id=\"token-link-copy\"",
    ] {
        assert!(html.contains(id), "{id}");
    }
    assert!(js.contains("showQr(link || r.data.token)"));
    assert!(!js.contains("location.host") && !js.contains("location.origin"));
}

// ===== the actions: step-up, ceilings, trash ==================================================

fn revoke_action(space: &str, ids: &[&str]) -> Value {
    json!({ "type": "revoke_devices", "space_id": space, "device_ids": ids })
}

async fn device_ids(env: &Env, seg: &str, cookie: &str) -> (String, Vec<String>) {
    let space = env.space_id[..8].to_owned();
    let list = get(
        env,
        seg,
        &format!("api/devices?space={space}"),
        Some(cookie),
    )
    .await
    .ok(200);
    let ids = list["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap().to_owned())
        .collect();
    (space, ids)
}

fn member_count(env: &Env) -> i64 {
    env.sql_count("SELECT count(*) FROM devices WHERE revoked_at IS NULL")
}

async fn signed_in_again(env: &Env, seg: &str, auth: &mut Authenticator) -> String {
    let r = login_raw(env, seg, auth, &Tweak::default()).await;
    assert_eq!(r.status, 204);
    cookie_of(&r)
}

/// Options for `action`, then an assertion signed over them: what a page holds just before it sends
/// `api/actions`.
async fn prepared(
    env: &Env,
    seg: &str,
    action: &Value,
    cookie: &str,
    auth: &mut Authenticator,
) -> Value {
    env.clock.advance_secs(20);
    let opts = post(
        env,
        seg,
        "api/stepup/options",
        json!({ "action": action }),
        Some(cookie),
    )
    .await
    .ok(200);
    let a = auth.get(&challenge_of(&opts), &Tweak::default());
    a.json()
}

#[tokio::test]
async fn a_stolen_session_without_the_passkey_can_neither_remove_nor_delete_nor_invite() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let (space, ids) = device_ids(&env, &seg, &cookie).await;
    let before = member_count(&env);

    let attempts = [
        revoke_action(&space, &[&ids[0]]),
        json!({ "type": "delete_space", "space_id": space, "confirm": space }),
        json!({ "type": "create_invitation", "mode": "new" }),
    ];
    let forged = auth.get(&[1u8; 32], &Tweak::default());
    for action in attempts {
        env.clock.advance_secs(20);
        // No assertion at all, a forged one, and the old route: none does anything.
        post(
            &env,
            &seg,
            "api/actions",
            json!({ "action": action }),
            Some(&cookie),
        )
        .await
        .err(400, "bad_request");
        env.clock.advance_secs(20);
        post(
            &env,
            &seg,
            "api/actions",
            json!({ "action": action, "assertion": forged.json() }),
            Some(&cookie),
        )
        .await
        .err(401, "unauthorized");
    }
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/invitations",
        json!({ "mode": "new" }),
        Some(&cookie),
    )
    .await
    .err(404, "not_found");
    assert_eq!(member_count(&env), before);
    assert_eq!(env.state.spaces.list().len(), 1);
    let list = get(&env, &seg, "api/invitations", Some(&cookie))
        .await
        .ok(200);
    assert_eq!(list["invitations"], json!([]));
}

#[tokio::test]
async fn a_device_is_removed_with_one_passkey_gesture_and_the_summary_comes_from_the_server() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(3).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let (space, ids) = device_ids(&env, &seg, &cookie).await;
    let action = revoke_action(&space, &[&ids[0]]);

    env.clock.advance_secs(20);
    let opts = post(
        &env,
        &seg,
        "api/stepup/options",
        json!({ "action": action }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(opts["publicKey"]["timeout"], 120_000);
    assert_eq!(opts["publicKey"]["userVerification"], "required");
    assert_eq!(opts["summary"]["members_after"], 2);
    assert_eq!(opts["summary"]["devices"][0]["id"], ids[0]);
    assert!(opts["summary"]["devices"][0]["name"].is_string());
    let a = auth.get(&challenge_of(&opts), &Tweak::default());
    env.clock.advance_secs(20);
    let done = post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": action, "assertion": a.json() }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(done["members"], 2);
    assert_eq!(done["rotation_required"], true);
    assert_eq!(member_count(&env), 2);
    let spaces = get(&env, &seg, "api/spaces", Some(&cookie)).await.ok(200);
    assert_eq!(spaces["spaces"][0]["rotation_required"], true);
    // The audit log has the short id of the device and the number of the key, nothing else.
    let journal = get(&env, &seg, "api/journal", Some(&cookie)).await.ok(200);
    let line = journal["audit"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "device_removed")
        .expect("an audit line");
    assert_eq!(line["target"], ids[0]);
    assert_eq!(line["key"], 1);
    assert!(!journal.to_string().contains(&env.space_id));

    // The two others cannot both go: a space keeps a member (the last one is the deletion).
    let rest: Vec<&str> = ids[1..].iter().map(String::as_str).collect();
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/stepup/options",
        json!({ "action": revoke_action(&space, &rest) }),
        Some(&cookie),
    )
    .await
    .err(409, "device_set_mismatch");
}

#[tokio::test]
async fn a_challenge_replayed_or_made_for_another_target_session_or_time_is_refused() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(3).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let (space, ids) = device_ids(&env, &seg, &cookie).await;
    let a0 = revoke_action(&space, &[&ids[0]]);
    let a1 = revoke_action(&space, &[&ids[1]]);

    // (1) Made for device 0, presented for device 1: refused, and the challenge is burnt.
    let assertion = prepared(&env, &seg, &a0, &cookie, &mut auth).await;
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a1, "assertion": assertion.clone() }),
        Some(&cookie),
    )
    .await
    .err(401, "unauthorized");
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a0, "assertion": assertion }),
        Some(&cookie),
    )
    .await
    .err(401, "unauthorized");
    assert_eq!(member_count(&env), 3, "nothing was removed");

    // (2) Replay: the first use works, the second does not.
    let assertion = prepared(&env, &seg, &a0, &cookie, &mut auth).await;
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a0, "assertion": assertion.clone() }),
        Some(&cookie),
    )
    .await
    .ok(200);
    env.clock.advance_secs(20);
    let again = post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a0, "assertion": assertion }),
        Some(&cookie),
    )
    .await;
    assert_eq!(again.status, 401);
    assert_eq!(member_count(&env), 2);

    // (3) A challenge of another session.
    let other_cookie = signed_in_again(&env, &seg, &mut auth).await;
    let assertion = prepared(&env, &seg, &a1, &cookie, &mut auth).await;
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a1, "assertion": assertion }),
        Some(&other_cookie),
    )
    .await
    .err(401, "unauthorized");
    assert_eq!(member_count(&env), 2);

    // (4) Expired: more than 2 minutes. Told apart from a refusal, and not counted.
    let assertion = prepared(&env, &seg, &a1, &cookie, &mut auth).await;
    env.clock.advance_secs(125);
    let late = post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": a1, "assertion": assertion }),
        Some(&cookie),
    )
    .await;
    assert_eq!(late.status, 401);
    assert_eq!(late.json()["reason"], "expired");
    assert_eq!(member_count(&env), 2);
}

#[tokio::test]
async fn five_refused_step_ups_close_the_session_but_not_the_passkey() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let (space, ids) = device_ids(&env, &seg, &cookie).await;
    let action = revoke_action(&space, &[&ids[0]]);
    let forged = auth.get(&[2u8; 32], &Tweak::default());

    for n in 1..=5u64 {
        env.clock.advance_secs(20);
        let r = post(
            &env,
            &seg,
            "api/actions",
            json!({ "action": action, "assertion": forged.json() }),
            Some(&cookie),
        )
        .await;
        assert_eq!(r.status, 401, "refusal {n}");
        let body = r.json();
        assert_eq!(body["refusals"], n);
        assert_eq!(body["session_closed"] == json!(true), n == 5);
    }
    // The session is gone; the key still signs in.
    env.clock.advance_secs(20);
    get(&env, &seg, "api/status", Some(&cookie))
        .await
        .err(401, "unauthorized");
    let again = login_raw(&env, &seg, &mut auth, &Tweak::default()).await;
    assert_eq!(again.status, 204);
    assert_eq!(member_count(&env), 2);
}

#[tokio::test]
async fn the_ceilings_count_the_successful_actions_only() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let (space, ids) = device_ids(&env, &seg, &cookie).await;

    // A refused action (the last member would go): nothing is counted.
    let both: Vec<&str> = ids.iter().map(String::as_str).collect();
    act(&env, &seg, revoke_action(&space, &both), &cookie, &mut auth)
        .await
        .err(409, "device_set_mismatch");
    let about = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(about["ceilings"]["device_removals"]["used"], 0);

    // 5 invitations an hour, then the sixth is refused before any challenge, with the command.
    for _ in 0..5 {
        act(
            &env,
            &seg,
            json!({ "type": "create_invitation", "mode": "new" }),
            &cookie,
            &mut auth,
        )
        .await
        .ok(200);
    }
    env.clock.advance_secs(20);
    let sixth = post(
        &env,
        &seg,
        "api/stepup/options",
        json!({ "action": { "type": "create_invitation", "mode": "new" } }),
        Some(&cookie),
    )
    .await;
    assert_eq!(sixth.status, 429);
    let body = sixth.json();
    assert_eq!(body["reason"], "ceiling");
    assert_eq!(body["cli"], "tomi-relay admin invite --new");
    let about = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(about["ceilings"]["invitations"]["used"], 5);
    assert_eq!(about["ceilings"]["invitations"]["max"], 5);
    assert_eq!(about["ceilings"]["space_deletions"]["max"], 2);
    assert_eq!(about["ceilings"]["device_removals"]["max"], 10);
    // An hour later it is allowed again.
    env.clock.advance_secs(3601);
    let cookie = signed_in_again(&env, &seg, &mut auth).await;
    act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200);
}

#[test]
fn the_ceiling_of_the_deletion_of_a_space_is_two_a_day_and_a_failure_gives_its_place_back() {
    use tomi_relay::admin_actions::{Ceilings, Kind};
    let c = Ceilings::default();
    let t = 1_000_000;
    let first = c.reserve(Kind::SpaceDeletion, 1, t).unwrap();
    c.reserve(Kind::SpaceDeletion, 1, t + 10).unwrap();
    assert!(c.reserve(Kind::SpaceDeletion, 1, t + 20).is_err());
    // The failed one is given back; the third then fits.
    c.release(first);
    c.reserve(Kind::SpaceDeletion, 1, t + 30).unwrap();
    // 24 hours later the window is empty.
    let day = 24 * 3_600_000;
    assert!(c.check(Kind::SpaceDeletion, 2, t + day + 40).is_ok());
    // Ten removals an hour, counted by device.
    assert!(c.reserve(Kind::DeviceRemoval, 10, t).is_ok());
    let wait = c.reserve(Kind::DeviceRemoval, 1, t + 1000).unwrap_err();
    assert!(wait > 3000 && wait <= 3600, "{wait}");
}

#[tokio::test]
async fn deleting_a_space_asks_its_eight_characters_and_goes_to_the_trash() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let space = env.space_id[..8].to_owned();

    // A wrong or missing confirmation: no challenge.
    for confirm in [json!("00000000"), json!(null), json!(&space[..4])] {
        act(
            &env,
            &seg,
            json!({ "type": "delete_space", "space_id": space, "confirm": confirm }),
            &cookie,
            &mut auth,
        )
        .await
        .err(400, "bad_request");
    }
    assert_eq!(env.state.spaces.list().len(), 1);

    let done = act(
        &env,
        &seg,
        json!({ "type": "delete_space", "space_id": space, "confirm": space.to_uppercase() }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200);
    assert_eq!(done["trash"]["days"], 7);
    assert_eq!(
        done["restore"],
        format!("tomi-relay admin trash restore --space {space}")
    );
    assert!(env.state.spaces.is_empty());
    let trash = get(&env, &seg, "api/trash", Some(&cookie)).await.ok(200);
    assert_eq!(trash["trash"][0]["space_id"], space);
    // The trash is shown, never restored or purged from the web.
    for path in ["api/trash/restore", "api/trash/purge"] {
        post(&env, &seg, path, json!({}), Some(&cookie))
            .await
            .err(404, "not_found");
    }
    let journal = get(&env, &seg, "api/journal", Some(&cookie)).await.ok(200);
    assert!(journal["audit"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["kind"] == "space_deleted" && l["target"] == space));
    // And the CLI brings it back.
    let out = admin::run(
        &env.state,
        Cmd::Trash {
            action: tomi_relay::trash::TrashAction::Restore,
            space: Some(space.clone()),
            confirm: None,
            json: false,
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(env.state.spaces.list().len(), 1);
}

#[tokio::test]
async fn the_quota_of_a_space_the_invitations_and_the_sessions_need_the_session_only() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    let mut auth = Authenticator::for_test_relay();
    let cookie = signed_in(&env, &mut auth).await;
    let space = env.space_id[..8].to_owned();

    // A quota above the usage is applied at once; `null` goes back to the default.
    let set = post(
        &env,
        &seg,
        "api/spaces/quota",
        json!({ "space_id": space, "value": 50_000_000 }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(set["max_bytes"], 50_000_000);
    assert_eq!(set["quota_source"], "space");
    let unset = post(
        &env,
        &seg,
        "api/spaces/quota",
        json!({ "space_id": space, "value": null }),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(unset["quota_source"], "default");
    post(
        &env,
        &seg,
        "api/spaces/quota",
        json!({ "space_id": space, "value": 12 }),
        Some(&cookie),
    )
    .await
    .err(400, "bad_request");
    post(
        &env,
        &seg,
        "api/spaces/quota",
        json!({ "space_id": "ffffffff", "value": null }),
        Some(&cookie),
    )
    .await
    .err(409, "device_set_mismatch");

    // Invitations: cancelling only removes a right.
    act(
        &env,
        &seg,
        json!({ "type": "create_invitation", "mode": "new" }),
        &cookie,
        &mut auth,
    )
    .await
    .ok(200);
    let cancelled = post(
        &env,
        &seg,
        "api/invitations/cancel",
        json!({}),
        Some(&cookie),
    )
    .await
    .ok(200);
    assert_eq!(cancelled["cancelled"], 1);
    assert_eq!(cancelled["invitations"], json!([]));

    // Other sessions, then "close all".
    let about = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(about["sessions"]["others"], 0);
    let other = signed_in_again(&env, &seg, &mut auth).await;
    let about = get(&env, &seg, "api/about", Some(&cookie)).await.ok(200);
    assert_eq!(about["sessions"]["others"], 1);
    let closed = post(
        &env,
        &seg,
        "api/sessions/close-all",
        json!({}),
        Some(&cookie),
    )
    .await;
    assert_eq!(closed.status, 204);
    for c in [&cookie, &other] {
        get(&env, &seg, "api/status", Some(c))
            .await
            .err(401, "unauthorized");
    }
    // Without a session none of it works.
    post(&env, &seg, "api/invitations/cancel", json!({}), None)
        .await
        .err(401, "unauthorized");
}

// ===== the CLI side: several passkeys, the policy, the invitations ============================

const FLAG_BE: u8 = 0x08;

async fn register_flags(env: &Env, seg: &str, auth: &Authenticator, flags: Option<u8>) {
    env.clock.advance_secs(60);
    let code = enroll_code(env).await;
    let options = register_options(env, seg, &code).await.ok(200);
    let tweak = Tweak {
        flags,
        ..Tweak::default()
    };
    let reg = auth.create(&challenge_of(&options), &tweak);
    post(env, seg, "api/register/finish", reg.json(), None)
        .await
        .ok(204);
}

#[tokio::test]
async fn up_to_five_passkeys_with_numbers_that_are_never_reused() {
    let env = panel().await;
    let seg = segment(&env);
    let mut keys: Vec<Authenticator> = (0..5).map(|_| Authenticator::for_test_relay()).collect();
    for k in &keys {
        register_flags(&env, &seg, k, None).await;
    }
    // A sixth is refused before any code is made.
    let out = web(&env, WebAction::Enroll).await;
    assert_eq!(out.exit, 2, "{}", out.stderr);

    let listing = web(&env, WebAction::Keys { json: true }).await;
    let v: Value = serde_json::from_str(&listing.stdout).unwrap();
    let numbers: Vec<i64> = v["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, [1, 2, 3, 4, 5]);
    assert_eq!(v["keys"][0]["device_bound"], true);
    assert_eq!(v["policy"]["space_deletion"], "any-key");

    // Each key signs in; removing key 2 closes the sessions it opened, and no other one.
    let c2 = signed_in_again(&env, &seg, &mut keys[1]).await;
    let c3 = signed_in_again(&env, &seg, &mut keys[2]).await;
    let out = web(&env, WebAction::KeysRemove { number: 2 }).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    get(&env, &seg, "api/status", Some(&c2))
        .await
        .err(401, "unauthorized");
    get(&env, &seg, "api/status", Some(&c3)).await.ok(200);
    // The removed key no longer signs in; the number 2 is not given again.
    let r = login_raw(&env, &seg, &mut keys[1], &Tweak::default()).await;
    assert_eq!(r.status, 401);
    let extra = Authenticator::for_test_relay();
    register_flags(&env, &seg, &extra, None).await;
    let listing = web(&env, WebAction::Keys { json: true }).await;
    let v: Value = serde_json::from_str(&listing.stdout).unwrap();
    let numbers: Vec<i64> = v["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, [1, 3, 4, 5, 6]);
    // The panel lists them, generated labels only, and marks the key of this session.
    let panel_keys = get(&env, &seg, "api/keys", Some(&c3)).await.ok(200);
    assert_eq!(panel_keys["keys"][1]["number"], 3);
    assert_eq!(panel_keys["keys"][1]["this_session"], true);
    assert_eq!(panel_keys["keys"][0]["this_session"], false);
    assert_eq!(panel_keys["keys"][0]["label"], "Clé 1 · liée à l'appareil");
    // Keys are enrolled and removed with the CLI only.
    for path in ["api/keys/remove", "api/keys/add", "api/policy"] {
        post(&env, &seg, path, json!({}), Some(&c3))
            .await
            .err(404, "not_found");
    }
}

#[tokio::test]
async fn the_last_passkey_cannot_be_removed_and_the_previous_binary_still_finds_a_key() {
    let env = panel().await;
    let seg = segment(&env);
    let a = Authenticator::for_test_relay();
    register_flags(&env, &seg, &a, None).await;
    let out = web(&env, WebAction::KeysRemove { number: 1 }).await;
    assert_eq!(out.exit, 2, "{}", out.stderr);
    assert!(out.stderr.contains("reset"), "{}", out.stderr);
    assert_eq!(web(&env, WebAction::KeysRemove { number: 9 }).await.exit, 1);

    let b = Authenticator::for_test_relay();
    register_flags(&env, &seg, &b, None).await;
    assert_eq!(web(&env, WebAction::KeysRemove { number: 1 }).await.exit, 0);
    // The single-key table of the previous version holds the key that is left.
    let legacy: Vec<u8> = env
        .state
        .spaces
        .routing()
        .read_sync(|tx| {
            Ok(
                tx.query_row("SELECT credential_id FROM admin_web_credential", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(legacy, b.credential_id);
    // A reset removes everything and the numbers still go on.
    let code = web(&env, WebAction::Show).await;
    let segment8 = code
        .stdout
        .lines()
        .next()
        .unwrap()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()[..8]
        .to_owned();
    let out = web(
        &env,
        WebAction::Reset {
            confirm: Some(segment8),
        },
    )
    .await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    let c = Authenticator::for_test_relay();
    register_flags(&env, &seg, &c, None).await;
    let v: Value =
        serde_json::from_str(&web(&env, WebAction::Keys { json: true }).await.stdout).unwrap();
    assert_eq!(v["keys"][0]["number"], 3);
}

#[tokio::test]
async fn the_device_key_policy_is_refused_without_such_a_key_and_then_binds_the_deletion() {
    let env = panel().await;
    let seg = segment(&env);
    let (_devs, _) = env.space_of(2).await;
    // Key 1 is synchronisable (BE set), key 2 is bound to its device.
    let mut synced = Authenticator::for_test_relay();
    register_flags(
        &env,
        &seg,
        &synced,
        Some(FLAG_UP | FLAG_UV | FLAG_AT | FLAG_BE),
    )
    .await;
    let mut bound = Authenticator::for_test_relay();

    let policy = |v: Option<&str>| WebAction::Policy {
        space_deletion: v.map(str::to_owned),
    };
    assert_eq!(
        web(&env, policy(None)).await.stdout,
        "space deletion: any-key\n"
    );
    let refused = web(&env, policy(Some("device-key"))).await;
    assert_eq!(refused.exit, 2, "{}", refused.stderr);
    assert_eq!(web(&env, policy(Some("sometimes"))).await.exit, 2);

    register_flags(&env, &seg, &bound, None).await;
    assert_eq!(web(&env, policy(Some("device-key"))).await.exit, 0);
    assert_eq!(
        web(&env, policy(None)).await.stdout,
        "space deletion: device-key\n"
    );
    // The last bound key cannot go while the policy asks for one.
    assert_eq!(web(&env, WebAction::KeysRemove { number: 2 }).await.exit, 2);

    let cookie = signed_in_again(&env, &seg, &mut synced).await;
    let space = env.space_id[..8].to_owned();
    let del = json!({ "type": "delete_space", "space_id": space, "confirm": space });
    // The options only offer the bound key, and an assertion of the synced one is refused.
    env.clock.advance_secs(20);
    let opts = post(
        &env,
        &seg,
        "api/stepup/options",
        json!({ "action": del }),
        Some(&cookie),
    )
    .await
    .ok(200);
    let allowed = opts["publicKey"]["allowCredentials"].as_array().unwrap();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0]["id"], b64_encode(&bound.credential_id));
    let a = synced.get(&challenge_of(&opts), &Tweak::default());
    env.clock.advance_secs(20);
    post(
        &env,
        &seg,
        "api/actions",
        json!({ "action": del, "assertion": a.json() }),
        Some(&cookie),
    )
    .await
    .err(401, "unauthorized");
    assert_eq!(env.state.spaces.list().len(), 1);
    // The bound key does it (other actions stay open to any key).
    act(&env, &seg, del, &cookie, &mut bound).await.ok(200);
    assert!(env.state.spaces.is_empty());
}

#[tokio::test]
async fn the_invitations_are_invalidated_from_the_cli_without_making_another() {
    let env = panel().await;
    let new = |rotate, new, cancel| Cmd::Invite {
        rotate,
        new,
        cancel,
    };
    assert_eq!(
        admin::run(&env.state, new(false, true, false)).await.exit,
        0
    );
    assert_eq!(
        admin::run(&env.state, new(false, true, false)).await.exit,
        0
    );
    let pending = |env: &Env| {
        tomi_relay::install::list(env.state.spaces.routing())
            .unwrap()
            .len()
    };
    let before = pending(&env);
    assert!(before >= 2);
    let out = admin::run(&env.state, new(false, false, true)).await;
    assert_eq!(out.exit, 0, "{}", out.stderr);
    assert_eq!(pending(&env), 0, "nothing is created");
    assert!(admin::needs_exclusive_access(&new(false, false, true)));
    // --cancel does not mix with the other two.
    assert_eq!(admin::run(&env.state, new(true, false, true)).await.exit, 2);
}
