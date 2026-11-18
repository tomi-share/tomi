//! `api-v1.md` §11.12 logs: A01 (the install token is logged once, at the first start of a fresh
//! relay), the 90 % quota warnings (a space is named by the first 8 characters of its id) and A120
//! (nothing secret ever reaches the logs, even under `RUST_LOG=trace`).
//!
//! This test binary installs a single global `tracing` subscriber (so logs from every thread
//! the relay uses are captured) with the relay's own `RUST_LOG=trace` filter. The capture
//! buffer records a marker between the two scenarios so each asserts on its own slice.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

mod common;

use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use common::*;
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::util::SubscriberInitExt;

/// A `tracing` writer that appends every log line into a shared buffer.
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

/// Installs the global capturing subscriber once (worst case: the relay's filter under
/// `RUST_LOG=trace`, with the held-at-info directives of §9 still applied) and returns the
/// shared buffer.
///
/// Crucially it uses `try_init()`, which installs the `log` -> `tracing` bridge
/// (`LogTracer`). The only third-party crates of this build that can see a secret (tungstenite,
/// ureq, rustls) log through the `log` facade, not `tracing`; without the bridge their records
/// would be dropped before the filter and A120 would pass without testing anything. The bridge
/// also means the `HELD_AT_INFO` directives of §9 actually apply to those `log` targets. Two
/// probes in A120 prove both ends of this.
fn logs() -> &'static Arc<Mutex<Vec<u8>>> {
    BUFFER.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = Capture(buf.clone());
        tracing_subscriber::fmt()
            .with_env_filter(tomi_relay::logging::filter(Some("trace")))
            .with_writer(writer)
            .with_ansi(false)
            .finish()
            .try_init()
            .expect("one global subscriber with the log bridge");
        buf
    })
}

/// A line that lets a scenario read only the logs produced after it.
fn mark(tag: &str) -> usize {
    let at = logs().lock().unwrap().len();
    tracing::warn!("test-marker {tag}");
    at
}

fn captured_after(at: usize) -> String {
    let buf = logs().lock().unwrap();
    String::from_utf8_lossy(&buf[at.min(buf.len())..]).into_owned()
}

// ===== A01 ========================================================================

#[tokio::test]
async fn a01_the_install_token_is_logged_once_at_the_first_start_only() {
    let at = mark("a01-fresh");
    // A fresh relay (no space, no invitation): an invitation is made and its token logged once.
    let env = Env::new().await;
    let token = env.install_token.clone();
    assert!(
        !token.is_empty(),
        "an invitation must be made on a fresh relay"
    );
    assert!(
        tomi_relay::install::is_pending(env.state.spaces.routing(), &token).unwrap(),
        "the logged token designates a pending invitation"
    );
    // Scope to this relay's own token: the binary runs tests concurrently against one shared
    // buffer, so another test's relay may log its own (different) token in the same window.
    let first = captured_after(at);
    let lines: Vec<&str> = first
        .lines()
        .filter(|l| l.contains("install token:") && l.contains(&token))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "exactly one `install token:` line carries this token: {first}"
    );

    // A second start without a space logs no token: the relay keeps only its hash.
    let at2 = mark("a01-restart");
    let env2 = env.restart().await;
    assert_eq!(
        env2.install_token, token,
        "the invitation survives a restart"
    );
    assert!(tomi_relay::install::is_pending(env2.state.spaces.routing(), &token).unwrap());
    tomi_relay::app::announce_invitation(&env2.state).unwrap();
    let second = captured_after(at2);
    assert!(
        !second.contains(&token),
        "the second start must not log the token: {second}"
    );
}

#[tokio::test]
async fn a01b_no_token_is_logged_once_the_relay_has_a_space() {
    let env = Env::new().await;
    let token = env.install_token.clone();
    let mut founder = Dev::new("PC");
    env.create_space(&mut founder).await;
    let at = mark("a01b-restart");
    let env = env.restart().await;
    // A relay with a space makes no invitation and logs no token.
    assert_eq!(
        tomi_relay::app::announce_invitation(&env.state).unwrap(),
        None
    );
    let after = captured_after(at);
    assert!(!after.contains(&token), "{after}");
    // Its logs name a space by the first 8 characters of its id, never by the whole id.
    let all = captured_after(0);
    assert!(
        !all.contains(&env.space_id),
        "the full space id never reaches the logs"
    );
}
// ===== quota warnings =================================================================

#[tokio::test]
async fn a_space_above_90_percent_of_its_quota_is_warned_about_by_its_short_id() {
    let _ = mark("quota-space");
    let env = Env::with(|cfg, _| cfg.max_space_bytes = Some(10_000)).await;
    let other = env.another_space().await;
    let (devs, _) = env.space_of(1).await;
    let (others, _) = other.space_of(1).await;
    env.create_blob(&devs[0], &new_uuid(), 9_500).await.ok(201);
    other
        .create_blob(&others[0], &new_uuid(), 1_000)
        .await
        .ok(201);
    let at = mark("quota-space-pass");
    tomi_relay::maintenance::run_once(&env.state).await;
    let after = captured_after(at);
    let own = &env.space_id[..8];
    let lines: Vec<&str> = after
        .lines()
        .filter(|l| l.contains("90%") && l.contains(own))
        .collect();
    assert_eq!(lines.len(), 1, "one warning for the space: {after}");
    assert!(
        lines[0].contains("9500") && lines[0].contains("10000"),
        "{}",
        lines[0]
    );
    assert!(
        !after
            .lines()
            .any(|l| l.contains("90%") && l.contains(&other.space_id[..8])),
        "the other space is under its quota: no warning"
    );
    assert!(!after.contains(&env.space_id), "never the whole id");
    // At most once an hour.
    let at = mark("quota-space-again");
    tomi_relay::maintenance::run_once(&env.state).await;
    assert!(!captured_after(at)
        .lines()
        .any(|l| l.contains("90%") && l.contains(own)));
    env.clock.advance_secs(3601);
    let at = mark("quota-space-later");
    tomi_relay::maintenance::run_once(&env.state).await;
    assert!(captured_after(at)
        .lines()
        .any(|l| l.contains("90%") && l.contains(own)));
}

#[tokio::test]
async fn the_total_above_90_percent_is_warned_about_without_naming_a_space() {
    let _ = mark("quota-total");
    let env = Env::with(|cfg, _| cfg.max_relay_bytes = 10_000).await;
    let other = env.another_space().await;
    let (devs, _) = env.space_of(1).await;
    let (others, _) = other.space_of(1).await;
    env.create_blob(&devs[0], &new_uuid(), 6_000).await.ok(201);
    other
        .create_blob(&others[0], &new_uuid(), 3_500)
        .await
        .ok(201);
    let at = mark("quota-total-pass");
    tomi_relay::maintenance::run_once(&env.state).await;
    let after = captured_after(at);
    let mine: Vec<&str> = after
        .lines()
        .filter(|l| l.contains("90% of the relay total") && l.contains("9500"))
        .collect();
    assert_eq!(mine.len(), 1, "{after}");
    // Neither space is named: no space has a quota of its own here.
    assert!(!mine[0].contains(&env.space_id[..8]) && !mine[0].contains(&other.space_id[..8]));
}

// ===== A120 =======================================================================

/// Every secret value the relay handled, in the encodings an attacker could grep for.
#[derive(Default)]
struct Secrets {
    values: Vec<(&'static str, String)>,
}

impl Secrets {
    /// A textual secret (a token). Searched as-is and as the hex of its bytes (A120 asks for
    /// both the base64url and the hexadecimal forms; a token is already text/base64url).
    fn add(&mut self, what: &'static str, value: &str) {
        if !value.is_empty() {
            self.values.push((what, value.to_owned()));
            self.values.push((what, hex::encode(value.as_bytes())));
        }
    }
    /// A raw secret, checked both as base64url and as hex.
    fn add_bytes(&mut self, what: &'static str, raw: &[u8]) {
        if !raw.is_empty() {
            self.values.push((what, b64(raw)));
            self.values.push((what, hex::encode(raw)));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a120_no_secret_reaches_the_logs_under_trace() {
    let at = mark("a120");
    let mut secrets = Secrets::default();

    // Probe both ends of the `log` -> `tracing` bridge (R2). The crates that could leak a
    // secret log through `log` at a `tungstenite::…` / `ureq` / `rustls` target; §9 holds them
    // at INFO. So a `log::trace!` on a held target MUST be dropped, and a `log::trace!` on a
    // free target MUST pass. Without the bridge installed (try_init), the "held" probe would
    // also be absent and the whole secret scan would be vacuous.
    log::trace!(target: "tungstenite::protocol", "A120-HELD-PROBE must-not-appear");
    log::trace!(target: "a120_probe", "A120-ON-PROBE must-appear");

    let env = Env::new().await;
    secrets.add("install token", &env.install_token);

    // Space, sessions, challenge, signature.
    let (devs, _) = env.space_of(2).await;
    let a = &devs[0];
    let b = &devs[1];
    secrets.add("session token A", a.tok());
    secrets.add("session token B", b.tok());

    // A raw challenge and its signature (the signed answer of §3.6).
    let ch = env.challenge(&a.id).await.ok(200);
    let challenge = unb64(ch["challenge"].as_str().unwrap());
    secrets.add_bytes("challenge", &challenge);
    let sig = a.keys.sign_relay_auth(&env.space_id, &challenge).unwrap();
    secrets.add_bytes("challenge signature", &sig);
    env.send(
        "POST",
        "/v1/auth/session",
        &[],
        Some(env.session_body(a, &challenge, 1, 1, "windows")),
    )
    .await
    .ok(200);

    // A pairing channel: the pairing token and an opaque PAKE message body.
    let channel = env.post(a, "/v1/pairing/channels", json!({})).await.ok(201);
    let channel_id = channel["channel_id"].as_str().unwrap().to_owned();
    let claim = env
        .send(
            "POST",
            &format!("/v1/pairing/channels/{channel_id}/claim"),
            &[],
            Some(json!({})),
        )
        .await
        .ok(200);
    secrets.add("pairing token", claim["pairing_token"].as_str().unwrap());
    let pake = vec![0x9cu8; 48];
    secrets.add_bytes("pairing message", &pake);
    env.post(
        a,
        &format!("/v1/pairing/channels/{channel_id}/messages"),
        json!({ "body": b64(&pake) }),
    )
    .await
    .ok(201);

    // An FCM push token.
    let fcm_token = "fcm-registration-ZZQQ11secrettoken";
    secrets.add("fcm token", fcm_token);
    env.put(
        a,
        "/v1/me/push",
        json!({ "provider": "fcm", "token": fcm_token }),
    )
    .await
    .ok(204);

    // An item envelope and a blob part body.
    let blob_bytes = vec![0x5au8; 4096];
    secrets.add_bytes("blob part", &blob_bytes);
    let blob_id = env.upload_blob(a, &blob_bytes, 1024).await;
    let item_id = new_uuid();
    let epoch = env.key_epoch().await;
    let envelope = Env::envelope(&uuid_bytes(&item_id), epoch, 512);
    secrets.add_bytes("item envelope", &envelope);
    env.post(
        a,
        "/v1/items",
        env.item_body(&item_id, epoch, 7 * DAY_MS, &envelope, &[&blob_id]),
    )
    .await
    .ok(201);

    // An item-state (pin) envelope: opaque to the relay, carries only a random event_id.
    let event_id = new_uuid();
    let mut state_env = vec![0x3bu8; 320];
    state_env[..4].copy_from_slice(&1u32.to_be_bytes());
    state_env[4..8].copy_from_slice(&epoch.to_be_bytes());
    state_env[8..24].copy_from_slice(&uuid_bytes(&event_id));
    secrets.add_bytes("item-state envelope", &state_env);
    env.post(
        a,
        "/v1/item-states",
        json!({ "event_id": event_id, "key_epoch": epoch, "envelope": b64(&state_env) }),
    )
    .await
    .ok(201);

    // A signed tombstone (a statement the relay verifies but never reads into clear).
    let tomb = env.tombstone(&a.keys, &item_id, DeletionScope::Everywhere);
    secrets.add_bytes("tombstone statement", &tomb);
    env.post(a, "/v1/tombstones", json!({ "tombstone": b64(&tomb) }))
        .await
        .ok(201);

    // Response bodies that carry opaque secrets back to the client: the event log (envelopes)
    // and a blob download (the part bytes). Reading them must not log their content.
    let _ = env.events(b, 0, 100).await;
    // A fresh, unattached blob so the download actually streams part bytes back.
    let dl_bytes = vec![0x7eu8; 2048];
    secrets.add_bytes("downloaded blob", &dl_bytes);
    let dl_id = env.upload_blob(a, &dl_bytes, 1024).await;
    let dl = env.download(a, &dl_id, None).await;
    assert_eq!(dl.status, 200, "blob download"); // binary body, not JSON: don't call .ok()
    let _ = env
        .get(
            a,
            &format!("/v1/pairing/channels/{channel_id}/messages?after=0&wait=0"),
        )
        .await;

    // An invalid Bearer credential: the rejected token must not be echoed to the logs.
    let bogus = "tms1.bogus-ZZQ11-not-a-real-session-token";
    secrets.add("invalid bearer", bogus);
    env.bearer("GET", "/v1/space", bogus, None)
        .await
        .err(401, "unauthorized");

    // The WebSocket doorbell over a real socket: the first message carries the session token,
    // and the protocol crate traces received frames at TRACE (held at INFO by §9).
    let (addr, stop, handle) = serve(&env).await;
    let mut ws = Ws::connect(addr).await;
    let ready = ws.auth(b.tok()).await;
    assert_eq!(ready["type"], "ready", "{ready}");
    env.send_item(a, &[]).await.1.ok(201);
    let _ = ws.recv_json(1000).await; // let the wake flow through
    drop(ws);
    let _ = stop.send(());
    let _ = handle.await;
    env.settle(100).await;

    let logs = captured_after(at);

    // The bridge is live and the filter is at trace: the free `log::trace!` probe appears...
    assert!(
        logs.contains("A120-ON-PROBE"),
        "the log->tracing bridge must be installed and trace-level active, else the scan is vacuous"
    );
    // ...and the held `tungstenite::…` probe is dropped (§9 holds those crates at INFO).
    assert!(
        !logs.contains("A120-HELD-PROBE"),
        "a `log` record on a held target leaked: HELD_AT_INFO is not doing its job"
    );

    // Not a single secret value may appear anywhere in the logs, except the install token,
    // which is allowed only on its documented `install token:` line.
    for (what, value) in &secrets.values {
        let mut hits: Vec<&str> = logs
            .lines()
            .filter(|l| !l.contains("test-marker"))
            .filter(|l| l.contains(value.as_str()))
            .collect();
        if *what == "install token" {
            hits.retain(|l| !l.contains("install token:"));
        }
        assert!(
            hits.is_empty(),
            "a {what} leaked into the logs ({} line(s)); first: {}",
            hits.len(),
            hits.first().copied().unwrap_or_default()
        );
    }

    // Sanity: the capture actually recorded the start-up logs, so a silent empty buffer cannot
    // make the test pass by accident.
    assert!(
        logs.contains("install token:"),
        "the capture recorded the start-up logs"
    );
}

// ===== the link that creates a space ==================================================

#[tokio::test]
async fn a_token_in_the_path_of_a_create_link_never_reaches_the_logs() {
    let at = mark("create-link");
    let env = Env::new().await;
    let secret = "zq7createlinktokenmarker9f3a1c";
    for path in [format!("/create/{secret}"), format!("/create/{secret}/x")] {
        let r = env.send("GET", &path, &[], None).await;
        assert_eq!(r.status, 200, "{path}");
    }
    // The access log names the route template, never the path of the request.
    let after = captured_after(at);
    assert!(
        after.contains("/create/{*path}"),
        "the request is logged by its route: {after}"
    );
    assert!(
        !after.contains(secret),
        "the token of the path must not be logged: {after}"
    );
}
