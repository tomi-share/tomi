//! The web administration panel and the logs (the counterpart of A01 and A120 of `logs.rs`): the
//! relay logs the route of every request, and for the panel that route is the constant
//! `admin-web`. Neither the secret path segment, nor the enrolment code, nor the session cookie,
//! nor a challenge, nor the credential id, nor an invitation token may appear in any line, even
//! under `RUST_LOG=trace`, whether the request succeeded or failed.
//!
//! Like `logs.rs`, this binary installs one global `tracing` subscriber with the relay's own
//! filter at `trace`, so the logs of every thread the relay uses are captured.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use common::authenticator::{Authenticator, Tweak};
use common::*;
use serde_json::{json, Value};
use tomi_relay::admin::{self, Cmd, WebAction};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;

const ORIGIN: &str = "https://relay.test";

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

fn logs() -> &'static Arc<Mutex<Vec<u8>>> {
    BUFFER.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        tracing_subscriber::fmt()
            .with_env_filter(tomi_relay::logging::filter(Some("trace")))
            .with_writer(Capture(buf.clone()))
            .with_ansi(false)
            .finish()
            .try_init()
            .expect("one global subscriber");
        buf
    })
}

fn mark(tag: &str) -> usize {
    let at = logs().lock().unwrap().len();
    tracing::warn!("test-marker {tag}");
    at
}

fn captured_after(at: usize) -> String {
    let buf = logs().lock().unwrap();
    String::from_utf8_lossy(&buf[at.min(buf.len())..]).into_owned()
}

/// Every secret value, as text and as the hexadecimal of that text, and raw bytes in base64url
/// and hexadecimal.
#[derive(Default)]
struct Secrets {
    values: Vec<(&'static str, String)>,
}

impl Secrets {
    fn add(&mut self, what: &'static str, value: &str) {
        if !value.is_empty() {
            self.values.push((what, value.to_owned()));
            self.values.push((what, hex::encode(value.as_bytes())));
        }
    }
    fn add_bytes(&mut self, what: &'static str, raw: &[u8]) {
        self.values.push((what, b64(raw)));
        self.values.push((what, hex::encode(raw)));
    }
}

async fn get(env: &Env, seg: &str, path: &str, cookie: &str) -> Resp {
    env.send(
        "GET",
        &format!("/{seg}/{path}"),
        &[("cookie", cookie)],
        None,
    )
    .await
}

async fn post(env: &Env, seg: &str, path: &str, body: Value, cookie: Option<&str>) -> Resp {
    let url = format!("/{seg}/{path}");
    let mut h: Vec<(&str, &str)> = vec![("origin", ORIGIN), ("x-tomi-admin", "1")];
    if let Some(c) = cookie {
        h.push(("cookie", c));
    }
    env.send("POST", &url, &h, Some(body)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_secret_of_the_web_panel_reaches_the_logs_under_trace() {
    let at = mark("admin-web");
    let env = Env::with(|cfg, _| cfg.admin_web = true).await;
    let mut secrets = Secrets::default();
    let seg = env.state.admin_web.as_ref().unwrap().segment();
    secrets.add("secret path segment", &seg);

    // The enrolment code, from the command that shows it once.
    let out = admin::run(
        &env.state,
        Cmd::Web {
            action: WebAction::Enroll,
        },
    )
    .await;
    let code = out.stdout.trim().to_owned();
    assert_eq!(out.exit, 0);
    secrets.add("enrolment code", &code);
    secrets.add("enrolment code (no dashes)", &code.replace('-', ""));

    // Refused attempts first: a wrong code, a wrong path, a request from another origin.
    post(
        &env,
        &seg,
        "api/register/options",
        json!({ "code": "0000-0000-0000" }),
        None,
    )
    .await
    .err(401, "unauthorized");
    let wrong_path = format!("{}x", &seg[..seg.len() - 1]);
    env.send("GET", &format!("/{wrong_path}/api/state"), &[], None)
        .await
        .err(404, "not_found");
    env.send(
        "POST",
        &format!("/{seg}/api/login/options"),
        &[("origin", "https://evil.test"), ("x-tomi-admin", "1")],
        Some(json!({})),
    )
    .await
    .err(403, "forbidden");

    // Enrolment.
    let mut auth = Authenticator::for_test_relay();
    secrets.add_bytes("credential id", &auth.credential_id);
    let options = post(
        &env,
        &seg,
        "api/register/options",
        json!({ "code": code }),
        None,
    )
    .await
    .ok(200);
    let challenge = unb64(options["publicKey"]["challenge"].as_str().unwrap());
    secrets.add_bytes("registration challenge", &challenge);
    let reg = auth.create(&challenge, &Tweak::default());
    post(&env, &seg, "api/register/finish", reg.json(), None)
        .await
        .ok(204);

    // A refused sign-in (bad signature), then a good one.
    env.clock.advance_secs(120);
    for tweak in [
        Tweak {
            bad_signature: true,
            ..Tweak::default()
        },
        Tweak::default(),
    ] {
        let options = post(&env, &seg, "api/login/options", json!({}), None)
            .await
            .ok(200);
        let challenge = unb64(options["publicKey"]["challenge"].as_str().unwrap());
        secrets.add_bytes("login challenge", &challenge);
        let a = auth.get(&challenge, &tweak);
        secrets.add_bytes("signature", &a.signature);
        let r = post(&env, &seg, "api/login/finish", a.json(), None).await;
        if tweak.bad_signature {
            assert_eq!(r.status, 401);
        } else {
            assert_eq!(r.status, 204);
            let set_cookie = r.headers.get("set-cookie").unwrap().to_str().unwrap();
            let pair = set_cookie.split(';').next().unwrap().to_owned();
            let token = pair.split_once('=').unwrap().1.to_owned();
            secrets.add("session cookie", &pair);
            secrets.add("session token", &token);

            // The dashboard, an invitation, a sign-out.
            get(&env, &seg, "api/status", &pair).await.ok(200);
            get(&env, &seg, "api/spaces", &pair).await.ok(200);
            get(&env, &seg, "api/about", &pair).await.ok(200);
            get(&env, &seg, "api/journal", &pair).await.ok(200);
            let created = post(
                &env,
                &seg,
                "api/invitations",
                json!({ "mode": "new" }),
                Some(&pair),
            )
            .await
            .ok(200);
            secrets.add("invitation token", created["token"].as_str().unwrap());
            post(&env, &seg, "api/logout", json!({}), Some(&pair))
                .await
                .ok(204);
            get(&env, &seg, "api/status", &pair)
                .await
                .err(401, "unauthorized");
        }
    }

    // Rotating the path and the reset also leave nothing behind.
    let rotated = admin::run(
        &env.state,
        Cmd::Web {
            action: WebAction::RotatePath,
        },
    )
    .await;
    let new_seg = rotated
        .stdout
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    secrets.add("rotated path segment", &new_seg);
    let page = env
        .send("GET", &format!("/{new_seg}/app.js"), &[], None)
        .await;
    assert_eq!(page.status, 200);
    env.send("GET", &format!("/{seg}/app.js"), &[], None)
        .await
        .err(404, "not_found");
    env.settle(100).await;

    let after = captured_after(at);

    // Not vacuous: the requests of the panel were logged, under their constant label, and the
    // events of the panel were logged too.
    let panel_lines: Vec<&str> = after
        .lines()
        .filter(|l| l.contains("route=admin-web"))
        .collect();
    assert!(
        panel_lines.len() >= 10,
        "the requests of the panel are logged under `admin-web`: {} line(s)",
        panel_lines.len()
    );
    assert!(
        after.contains("admin web: the administrator signed in"),
        "{after}"
    );
    assert!(after.contains("admin web: sign-in refused"), "{after}");
    assert!(after.contains("admin web: enrolment refused"), "{after}");

    for (what, value) in &secrets.values {
        let hits: Vec<&str> = after
            .lines()
            .filter(|l| !l.contains("test-marker"))
            .filter(|l| l.contains(value.as_str()))
            .collect();
        assert!(
            hits.is_empty(),
            "a {what} leaked into the logs ({} line(s)); first: {}",
            hits.len(),
            hits.first().copied().unwrap_or_default()
        );
    }
}
