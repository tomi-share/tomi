//! Test harness: a real relay (temporary data directory, injectable clock, recording push
//! sender) served in-process, plus helpers that build real signed objects with `tomi-core`.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_arguments
)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use tower::ServiceExt;

pub mod authenticator;
pub mod rsa_key;

#[allow(unused_imports)]
pub use tomi_core::{
    DeletionScope, DeviceKeys, HistoryKey, HistoryPart, KeyRotation, PinBook, RotationRecipient,
    SpaceKey, TrustStore, SEALED_SPACE_KEY_LEN,
};
pub use tomi_relay::app::{open, Options, PushChoice};
pub use tomi_relay::clock::{Clock, ManualClock};
#[allow(unused_imports)]
pub use tomi_relay::config::Rate;
pub use tomi_relay::config::{Config, Timing};
pub use tomi_relay::disk::FixedDisk;
pub use tomi_relay::push::{PushError, PushSender, PushTarget, Urgency};
pub use tomi_relay::state::AppState;

/// 2026-11-05T00:00:00.000Z.
pub const T0: i64 = 1_793_836_800_000;
pub const DAY_MS: i64 = 86_400_000;

pub fn b64(data: &[u8]) -> String {
    B64.encode(data)
}

pub fn unb64(s: &str) -> Vec<u8> {
    B64.decode(s).unwrap()
}

pub fn ts(ms: i64) -> String {
    tomi_relay::wire::ts_format(ms)
}

pub fn uuid_text(bytes: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*bytes).hyphenated().to_string()
}

pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

pub fn uuid_bytes(s: &str) -> [u8; 16] {
    *uuid::Uuid::parse_str(s).unwrap().as_bytes()
}

/// An answer of the relay.
#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", String::from_utf8_lossy(&self.body)))
    }
    pub fn code(&self) -> String {
        self.json()["code"].as_str().unwrap_or("").to_owned()
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }
    #[track_caller]
    pub fn ok(self, status: u16) -> Value {
        assert_eq!(
            self.status,
            status,
            "unexpected answer: {}",
            String::from_utf8_lossy(&self.body)
        );
        if self.body.is_empty() {
            Value::Null
        } else {
            self.json()
        }
    }
    #[track_caller]
    pub fn err(&self, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.code()),
            (status, code.to_owned()),
            "unexpected answer: {}",
            String::from_utf8_lossy(&self.body)
        );
    }
}

/// Records every wake-up the relay asks the push sender to send.
#[derive(Default)]
pub struct RecordingPush {
    pub calls: Mutex<Vec<(String, Urgency)>>,
    pub script: Mutex<VecDeque<Result<(), PushError>>>,
}

impl RecordingPush {
    pub fn calls(&self) -> Vec<(String, Urgency)> {
        self.calls.lock().unwrap().clone()
    }
    pub fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    pub fn clear(&self) {
        self.calls.lock().unwrap().clear();
    }
    pub fn push_result(&self, r: Result<(), PushError>) {
        self.script.lock().unwrap().push_back(r);
    }
}

#[async_trait::async_trait]
impl PushSender for RecordingPush {
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError> {
        self.calls
            .lock()
            .unwrap()
            .push((target.token.clone(), urgency));
        self.script.lock().unwrap().pop_front().unwrap_or(Ok(()))
    }
}

/// A device of the tests: its real keys and its current session.
pub struct Dev {
    pub keys: DeviceKeys,
    pub id: String,
    pub name: String,
    pub token: Option<String>,
}

impl Dev {
    pub fn new(name: &str) -> Self {
        let keys = DeviceKeys::generate().unwrap();
        Self {
            id: keys.device_id(),
            keys,
            name: name.to_owned(),
            token: None,
        }
    }
    pub fn pk(&self) -> Vec<u8> {
        self.keys.sign_pk()
    }
    pub fn tok(&self) -> &str {
        self.token.as_deref().expect("device is not logged in")
    }
}

/// Another handle on the same device (same keys, same session): what a restart of the app is.
pub fn clone_dev(d: &Dev) -> Dev {
    Dev {
        keys: DeviceKeys::from_secrets(&d.keys.ed25519_seed(), &d.keys.x25519_secret()).unwrap(),
        id: d.id.clone(),
        name: d.name.clone(),
        token: d.token.clone(),
    }
}

/// The relay under test, and one space of it (`space_id`). `another_space` gives a second `Env`
/// on the same relay for the tests of isolation.
pub struct Env {
    pub dir: Arc<tempfile::TempDir>,
    pub state: AppState,
    pub clock: ManualClock,
    pub disk: Arc<FixedDisk>,
    pub push: Arc<RecordingPush>,
    pub router: Router,
    pub space_id: String,
    pub install_token: String,
}

/// The production rate limits of §8.5. `Config::for_tests` puts every limit out of the way;
/// the tests that are about a limit (or about not being limited) put the real ones back.
pub fn real_limits(cfg: &mut Config) {
    let d = Config::default();
    cfg.rl_ip_public = d.rl_ip_public;
    cfg.rl_ip_space_create = d.rl_ip_space_create;
    cfg.rl_ip_auth = d.rl_ip_auth;
    cfg.rl_ip_claim = d.rl_ip_claim;
    cfg.rl_global_claim = d.rl_global_claim;
    cfg.rl_ip_ws = d.rl_ip_ws;
    cfg.rl_ip_static = d.rl_ip_static;
    cfg.rl_device = d.rl_device;
    cfg.rl_device_items = d.rl_device_items;
    cfg.rl_device_tombstones = d.rl_device_tombstones;
    cfg.rl_device_item_states = d.rl_device_item_states;
    cfg.rl_device_presence = d.rl_device_presence;
    cfg.rl_device_pairing = d.rl_device_pairing;
    cfg.rl_device_transfers = d.rl_device_transfers;
    cfg.rl_device_blob_parts = d.rl_device_blob_parts;
    cfg.rl_pairing_poll = d.rl_pairing_poll;
}

/// Reduced real-time constants (the production values are asserted by a unit test).
pub fn fast_timing() -> Timing {
    Timing {
        ws_auth_timeout: Duration::from_millis(400),
        ws_ping_interval: Duration::from_millis(300),
        ws_pong_timeout: Duration::from_millis(300),
        push_min_interval: Duration::from_millis(500),
        max_long_poll: Duration::from_secs(25),
    }
}

impl Env {
    pub async fn new() -> Self {
        Self::with(|_, _| {}).await
    }

    pub async fn with(tweak: impl FnOnce(&mut Config, &mut Options)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::in_dir(dir, tweak).await
    }

    pub async fn in_dir(
        dir: tempfile::TempDir,
        tweak: impl FnOnce(&mut Config, &mut Options),
    ) -> Self {
        let clock = ManualClock::new(T0);
        let disk = Arc::new(FixedDisk::new(500 * 1024 * 1024 * 1024));
        let push = Arc::new(RecordingPush::default());
        let mut cfg = Config::for_tests(dir.path());
        let mut opts = Options {
            clock: Arc::new(clock.clone()),
            disk: disk.clone(),
            timing: fast_timing(),
            push: PushChoice::Custom(push.clone()),
            background: false,
            hold_lock: false,
            read_only: false,
            cleanup: true,
            fcm_http: None,
            fcm_base: None,
        };
        tweak(&mut cfg, &mut opts);
        let state = open(cfg, opts).await.unwrap();
        let router = tomi_relay::routes::router(state.clone());
        // A fresh relay has one pending invitation, whose token `tomi-relay` logs at start.
        let install_token = tomi_relay::app::announce_invitation(&state)
            .unwrap()
            .unwrap_or_default();
        Self {
            dir: Arc::new(dir),
            state,
            clock,
            disk,
            push,
            router,
            space_id: new_uuid(),
            install_token,
        }
    }

    /// A second relay process on the same data directory (restart).
    pub async fn restart(self) -> Self {
        let Env {
            dir,
            clock,
            disk,
            push,
            space_id,
            install_token,
            ..
        } = self;
        let mut cfg = Config::for_tests(dir.path());
        cfg.data_dir = dir.path().to_path_buf();
        let opts = Options {
            clock: Arc::new(clock.clone()),
            disk: disk.clone(),
            timing: fast_timing(),
            push: PushChoice::Custom(push.clone()),
            background: false,
            hold_lock: false,
            read_only: false,
            cleanup: true,
            fcm_http: None,
            fcm_base: None,
        };
        let state = open(cfg, opts).await.unwrap();
        let router = tomi_relay::routes::router(state.clone());
        Env {
            dir,
            state,
            clock,
            disk,
            push,
            router,
            space_id,
            // The invitation is still pending (it is stored by hash); the token is the same.
            install_token,
        }
    }

    /// A second space on the same relay (same state, router, clock, disk and push): a new
    /// `space_id` and an invitation of its own, created like `admin invite --new` does.
    pub async fn another_space(&self) -> Env {
        let out = tomi_relay::admin::run(
            &self.state,
            tomi_relay::admin::Cmd::Invite {
                rotate: false,
                new: true,
            },
        )
        .await;
        assert_eq!(out.exit, 0, "{}", out.stderr);
        Env {
            dir: self.dir.clone(),
            state: self.state.clone(),
            clock: self.clock.clone(),
            disk: self.disk.clone(),
            push: self.push.clone(),
            router: self.router.clone(),
            space_id: new_uuid(),
            install_token: out.stdout.trim().to_owned(),
        }
    }

    /// The same relay seen as another of its spaces (one that `another_space` made, before or
    /// after a restart): this env's state, router, clock, disk and push, that `space_id`.
    pub fn for_space(&self, space_id: &str) -> Env {
        Env {
            dir: self.dir.clone(),
            state: self.state.clone(),
            clock: self.clock.clone(),
            disk: self.disk.clone(),
            push: self.push.clone(),
            router: self.router.clone(),
            space_id: space_id.to_owned(),
            install_token: String::new(),
        }
    }

    /// The database of this env's space, once the space exists.
    pub fn space_db(&self) -> Option<Arc<tomi_relay::db::Db>> {
        self.state
            .spaces
            .get(&uuid_bytes(&self.space_id))
            .map(|s| s.db.clone())
    }

    /// The database of this env's space (panics if it was not created).
    pub fn db(&self) -> Arc<tomi_relay::db::Db> {
        self.space_db().expect("the space does not exist")
    }

    /// The space of this env (panics if it was not created).
    pub fn space(&self) -> Arc<tomi_relay::spaces::Space> {
        self.state
            .spaces
            .get(&uuid_bytes(&self.space_id))
            .expect("the space does not exist")
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    // ----- raw HTTP -------------------------------------------------------------------

    pub async fn call(&self, req: Request<Body>) -> Resp {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let (parts, body) = resp.into_parts();
        let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        Resp {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> Resp {
        let mut b = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some(v) => b
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&v).unwrap()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        self.call(req).await
    }

    pub async fn bearer(&self, method: &str, path: &str, token: &str, body: Option<Value>) -> Resp {
        let auth = format!("Bearer {token}");
        self.send(method, path, &[("authorization", auth.as_str())], body)
            .await
    }

    pub async fn get(&self, d: &Dev, path: &str) -> Resp {
        self.bearer("GET", path, d.tok(), None).await
    }

    pub async fn post(&self, d: &Dev, path: &str, body: Value) -> Resp {
        self.bearer("POST", path, d.tok(), Some(body)).await
    }

    pub async fn put(&self, d: &Dev, path: &str, body: Value) -> Resp {
        self.bearer("PUT", path, d.tok(), Some(body)).await
    }

    pub async fn delete(&self, d: &Dev, path: &str) -> Resp {
        self.bearer("DELETE", path, d.tok(), None).await
    }

    pub async fn call_from(&self, peer: SocketAddr, mut req: Request<Body>) -> Resp {
        req.extensions_mut().insert(ConnectInfo(peer));
        self.call(req).await
    }

    // ----- space and sessions ----------------------------------------------------------

    pub fn cert(&self, issuer: &DeviceKeys, subject: &DeviceKeys) -> Vec<u8> {
        issuer
            .sign_device_cert(
                &self.space_id,
                &subject.public(),
                self.clock.now_ms() as u64,
            )
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    pub async fn create_space_raw(&self, founder: &Dev, token: &str) -> Resp {
        self.send(
            "POST",
            "/v1/space",
            &[],
            Some(json!({
                "install_token": token,
                "space_id": self.space_id,
                "name": founder.name,
                "platform": "windows",
                "certificate": b64(&self.cert(&founder.keys, &founder.keys)),
            })),
        )
        .await
    }

    /// Creates the space with `founder` and logs it in.
    pub async fn create_space(&self, founder: &mut Dev) {
        let token = self.install_token.clone();
        self.create_space_raw(founder, &token).await.ok(201);
        self.login(founder, 1, 1).await;
    }

    pub async fn challenge(&self, device_id: &str) -> Resp {
        self.send(
            "POST",
            "/v1/auth/challenge",
            &[],
            Some(json!({ "device_id": device_id })),
        )
        .await
    }

    pub fn session_body(
        &self,
        d: &Dev,
        challenge: &[u8],
        fmin: u32,
        fmax: u32,
        platform: &str,
    ) -> Value {
        let sig = d.keys.sign_relay_auth(&self.space_id, challenge).unwrap();
        json!({
            "device_id": d.id,
            "challenge": b64(challenge),
            "signature": b64(&sig),
            "client": { "format_min": fmin, "format_max": fmax, "app_version": "0.1.0", "platform": platform },
        })
    }

    /// Challenge + session; returns the raw answer of `POST /v1/auth/session`.
    pub async fn login_raw(&self, d: &Dev, fmin: u32, fmax: u32) -> Resp {
        let ch = self.challenge(&d.id).await.ok(200);
        let challenge = unb64(ch["challenge"].as_str().unwrap());
        self.send(
            "POST",
            "/v1/auth/session",
            &[],
            Some(self.session_body(d, &challenge, fmin, fmax, "windows")),
        )
        .await
    }

    pub async fn login(&self, d: &mut Dev, fmin: u32, fmax: u32) -> Value {
        let v = self.login_raw(d, fmin, fmax).await.ok(200);
        d.token = Some(v["session_token"].as_str().unwrap().to_owned());
        v
    }

    // ----- pairing ---------------------------------------------------------------------

    /// Channel + claim + registration of `newcomer` by `inviter`; returns the answer of
    /// `POST /v1/devices` (the newcomer is not logged in).
    pub async fn pair_raw(&self, inviter: &Dev, newcomer: &Dev, key_epoch: u32) -> Resp {
        let ch = self
            .post(inviter, "/v1/pairing/channels", json!({}))
            .await
            .ok(201);
        let channel_id = ch["channel_id"].as_str().unwrap().to_owned();
        self.send(
            "POST",
            &format!("/v1/pairing/channels/{channel_id}/claim"),
            &[],
            Some(json!({})),
        )
        .await
        .ok(200);
        self.post(
            inviter,
            "/v1/devices",
            json!({
                "channel_id": channel_id,
                "key_epoch": key_epoch,
                "name": newcomer.name,
                "platform": "android",
                "certificate": b64(&self.cert(&inviter.keys, &newcomer.keys)),
            }),
        )
        .await
    }

    /// Registers `newcomer` through `inviter` and logs it in.
    pub async fn add_device(&self, inviter: &Dev, newcomer: &mut Dev) {
        let epoch = self.key_epoch().await;
        self.pair_raw(inviter, newcomer, epoch).await.ok(201);
        self.login(newcomer, 1, 1).await;
    }

    /// A space of `n` logged-in devices and the trust store of the founder.
    pub async fn space_of(&self, n: usize) -> (Vec<Dev>, TrustStore) {
        let mut devs: Vec<Dev> = (0..n).map(|i| Dev::new(&format!("dev{i}"))).collect();
        self.create_space(&mut devs[0]).await;
        let mut ts = TrustStore::new(&self.space_id, &devs[0].keys.public(), 1).unwrap();
        for i in 1..n {
            let (head, tail) = devs.split_at_mut(i);
            self.add_device(&head[0], &mut tail[0]).await;
            ts.add_verified_peer(&tail[0].keys.public()).unwrap();
        }
        (devs, ts)
    }

    pub async fn key_epoch(&self) -> u32 {
        let Some(db) = self.space_db() else {
            return 1;
        };
        db.read(|tx| Ok(tomi_relay::store::require_space(tx)?.key_epoch))
            .await
            .unwrap_or(1)
    }

    pub async fn head_seq(&self) -> i64 {
        self.db()
            .read(|tx| Ok(tomi_relay::store::require_space(tx)?.last_seq))
            .await
            .unwrap()
    }

    // ----- items and events ------------------------------------------------------------

    /// An envelope with a format-1 header (`format || key_epoch || item_id`) and `len`
    /// bytes in total. The relay never reads further.
    pub fn envelope(item_id: &[u8; 16], key_epoch: u32, len: usize) -> Vec<u8> {
        let mut e = vec![0xa5u8; len.max(24)];
        e[..4].copy_from_slice(&1u32.to_be_bytes());
        e[4..8].copy_from_slice(&key_epoch.to_be_bytes());
        e[8..24].copy_from_slice(item_id);
        e
    }

    pub fn item_body(
        &self,
        item_id: &str,
        key_epoch: u32,
        expires_in_ms: i64,
        env: &[u8],
        blobs: &[&str],
    ) -> Value {
        json!({
            "item_id": item_id,
            "expires_at": ts(self.clock.now_ms() + expires_in_ms),
            "key_epoch": key_epoch,
            "envelope": b64(env),
            "blob_ids": blobs,
        })
    }

    /// Sends a small valid item and returns `(item_id, answer)`.
    pub async fn send_item(&self, d: &Dev, blobs: &[&str]) -> (String, Resp) {
        let id = new_uuid();
        let epoch = self.key_epoch().await;
        let env = Self::envelope(&uuid_bytes(&id), epoch, 300);
        let r = self
            .post(
                d,
                "/v1/items",
                self.item_body(&id, epoch, 7 * DAY_MS, &env, blobs),
            )
            .await;
        (id, r)
    }

    pub async fn events(&self, d: &Dev, after: i64, limit: u32) -> Value {
        self.get(d, &format!("/v1/events?after={after}&limit={limit}"))
            .await
            .ok(200)
    }

    pub async fn all_events(&self, d: &Dev) -> Vec<Value> {
        let mut after = 0i64;
        let mut out = Vec::new();
        loop {
            let page = self.events(d, after, 100).await;
            out.extend(page["events"].as_array().unwrap().iter().cloned());
            after = page["next_after"].as_i64().unwrap();
            if !page["has_more"].as_bool().unwrap() {
                return out;
            }
        }
    }

    pub async fn events_of_type(&self, d: &Dev, kind: &str) -> Vec<Value> {
        self.all_events(d)
            .await
            .into_iter()
            .filter(|e| e["type"] == kind)
            .collect()
    }

    /// Runs one SQL query on the database of the space and returns the integer it selects.
    pub fn sql_count(&self, sql: &str) -> i64 {
        let sql = sql.to_owned();
        self.db()
            .read_sync(move |tx| Ok(tx.query_row(&sql, [], |r| r.get::<_, i64>(0))?))
            .unwrap()
    }

    // ----- a request whose body is slow ---------------------------------------------------

    /// A request by `d` whose body arrives in two parts: the first at once, the second when
    /// `release` is fired (or dropped). The gate judges the caller before the body is read, so
    /// whatever happens to the device between the two parts happens "after the access control
    /// and before the transaction".
    ///
    /// The returned [`Slow`] tells, without any sleep, when the relay has consumed the first part
    /// and asks for the second one (`Slow::body_started`): from there the handler is past the gate
    /// and every check it does before reading the body, and is waiting on the rest of it.
    pub fn slow_request(
        &self,
        method: &str,
        path: &str,
        d: &Dev,
        headers: &[(&str, String)],
        bytes: Vec<u8>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Slow {
        use futures_util::StreamExt as _;
        let (first, second) = bytes.split_at(bytes.len() / 2);
        let (first, second) = (
            Bytes::copy_from_slice(first),
            Bytes::copy_from_slice(second),
        );
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) })
            .chain(futures_util::stream::once(async move {
                // Polled only when the consumer of the body has taken the first part.
                let _ = reached_tx.send(());
                let _ = release.await;
                Ok::<_, std::io::Error>(second)
            }));
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", d.tok()))
            .header("content-length", bytes.len());
        for (name, value) in headers {
            req = req.header(*name, value.as_str());
        }
        let req = req.body(Body::from_stream(stream)).unwrap();
        let router = self.router.clone();
        let task = tokio::spawn(async move {
            let resp = router.oneshot(req).await.unwrap();
            let (parts, body) = resp.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            Resp {
                status: parts.status.as_u16(),
                headers: parts.headers,
                body,
            }
        });
        Slow { reached, task }
    }

    /// A `POST` of a JSON body by `d` whose body arrives slowly (see [`Env::slow_request`]).
    pub fn post_slow(
        &self,
        d: &Dev,
        path: &str,
        body: Value,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Slow {
        let bytes = serde_json::to_vec(&body).unwrap();
        let headers = [("content-type", "application/json".to_owned())];
        self.slow_request("POST", path, d, &headers, bytes, release)
    }

    /// A `PATCH` of a part of a blob by `d` whose body arrives slowly: the long case, the one a
    /// real upload is (see [`Env::slow_request`]). The first half of the part is written to the
    /// file before the relay asks for the second.
    pub fn patch_blob_slow(
        &self,
        d: &Dev,
        blob_id: &str,
        offset: u64,
        data: Vec<u8>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) -> Slow {
        let headers = [
            ("content-type", "application/octet-stream".to_owned()),
            ("upload-offset", offset.to_string()),
        ];
        self.slow_request(
            "PATCH",
            &format!("/v1/blobs/{blob_id}"),
            d,
            &headers,
            data,
            release,
        )
    }

    // ----- blobs -----------------------------------------------------------------------

    pub async fn create_blob(&self, d: &Dev, blob_id: &str, size: u64) -> Resp {
        self.post(d, "/v1/blobs", json!({ "blob_id": blob_id, "size": size }))
            .await
    }

    pub async fn patch_blob(&self, d: &Dev, blob_id: &str, offset: u64, data: Vec<u8>) -> Resp {
        let auth = format!("Bearer {}", d.tok());
        let req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/blobs/{blob_id}"))
            .header("authorization", auth)
            .header("content-type", "application/octet-stream")
            .header("content-length", data.len().to_string())
            .header("upload-offset", offset.to_string())
            .body(Body::from(data))
            .unwrap();
        self.call(req).await
    }

    /// Creates a blob of `data.len()` bytes and uploads it in parts of `part` bytes.
    pub async fn upload_blob(&self, d: &Dev, data: &[u8], part: usize) -> String {
        let id = new_uuid();
        self.create_blob(d, &id, data.len() as u64).await.ok(201);
        let mut off = 0usize;
        while off < data.len() {
            let end = (off + part).min(data.len());
            self.patch_blob(d, &id, off as u64, data[off..end].to_vec())
                .await
                .ok(204);
            off = end;
        }
        id
    }

    pub async fn download(&self, d: &Dev, blob_id: &str, range: Option<&str>) -> Resp {
        let auth = format!("Bearer {}", d.tok());
        let mut b = Request::builder()
            .method("GET")
            .uri(format!("/v1/blobs/{blob_id}"))
            .header("authorization", auth);
        if let Some(r) = range {
            b = b.header("range", r);
        }
        self.call(b.body(Body::empty()).unwrap()).await
    }

    // ----- statements -------------------------------------------------------------------

    pub fn tombstone(&self, signer: &DeviceKeys, item_id: &str, scope: DeletionScope) -> Vec<u8> {
        signer
            .sign_tombstone(&self.space_id, item_id, self.clock.now_ms() as u64, scope)
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    pub async fn post_tombstone(&self, d: &Dev, item_id: &str) -> Resp {
        let t = self.tombstone(&d.keys, item_id, DeletionScope::Everywhere);
        self.post(d, "/v1/tombstones", json!({ "tombstone": b64(&t) }))
            .await
    }

    /// Wire bytes of the removal of attachment `slot` (blob `blob_id`) of `item_id`.
    pub fn attachment_tombstone(
        &self,
        signer: &DeviceKeys,
        item_id: &str,
        blob_id: &str,
        slot: u32,
    ) -> Vec<u8> {
        signer
            .sign_attachment_tombstone(
                &self.space_id,
                item_id,
                blob_id,
                slot,
                self.clock.now_ms() as u64,
            )
            .unwrap()
            .to_bytes()
            .unwrap()
    }

    pub async fn post_attachment_tombstone(
        &self,
        d: &Dev,
        item_id: &str,
        blob_id: &str,
        slot: u32,
    ) -> Resp {
        let t = self.attachment_tombstone(&d.keys, item_id, blob_id, slot);
        self.post(
            d,
            "/v1/attachment-tombstones",
            json!({ "attachment_tombstone": b64(&t) }),
        )
        .await
    }

    /// Time to let the relay run in the background (push dispatcher, WebSocket tasks).
    pub async fn settle(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// A request in flight whose body is not finished (see [`Env::slow_request`]).
pub struct Slow {
    reached: tokio::sync::oneshot::Receiver<()>,
    task: tokio::task::JoinHandle<Resp>,
}

impl Slow {
    /// Waits until the relay has read the first part of the body and asks for the rest: the
    /// request has passed the gate. Fails the test instead of hanging if that never happens.
    pub async fn body_started(&mut self) {
        tokio::time::timeout(Duration::from_secs(10), &mut self.reached)
            .await
            .expect("the relay never asked for the rest of the body")
            .expect("the request ended before its body was read");
    }

    /// The answer, once the body has been released.
    pub async fn answer(self) -> Resp {
        self.task.await.unwrap()
    }
}

/// Serves `env` on an ephemeral TCP port (WebSocket tests). Returns the address and a
/// handle that stops the server.
pub async fn serve(
    env: &Env,
) -> (
    SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let state = env.state.clone();
    let handle = tokio::spawn(async move {
        let _ = tomi_relay::app::serve(state, listener, async {
            let _ = stop_rx.await;
        })
        .await;
    });
    (addr, stop_tx, handle)
}

// ===== statements the honest client never produces =====================================

/// A `KeyRotation` signed by `issuer` over any recipients and revoked list (the sealed keys
/// are filler: the relay cannot open them). `tomi-core` refuses to build such objects.
pub fn forge_rotation(
    env: &Env,
    issuer: &Dev,
    new_epoch: u32,
    revoked: &[Vec<u8>],
    recipients: &[Vec<u8>],
) -> Vec<u8> {
    use ed25519_dalek::Signer;
    let mut revoked = revoked.to_vec();
    revoked.sort();
    revoked.dedup();
    let mut keys = recipients.to_vec();
    keys.sort();
    keys.dedup();
    let mut r = KeyRotation {
        space_id: env.space_id.clone(),
        new_epoch,
        issued_at_ms: env.clock.now_ms() as u64,
        issuer_ed25519: issuer.pk(),
        revoked,
        recipients: keys
            .into_iter()
            .map(|ed25519| RotationRecipient {
                ed25519,
                sealed_key: vec![7u8; SEALED_SPACE_KEY_LEN],
            })
            .collect(),
        signature: Vec::new(),
    };
    let seed: [u8; 32] = issuer.keys.ed25519_seed().try_into().unwrap();
    r.signature = ed25519_dalek::SigningKey::from_bytes(&seed)
        .sign(&r.signed_bytes().unwrap())
        .to_bytes()
        .to_vec();
    r.to_bytes().unwrap()
}

impl Env {
    pub async fn post_rotation(&self, d: &Dev, statement: &[u8]) -> Resp {
        self.post(d, "/v1/rotations", json!({ "rotation": b64(statement) }))
            .await
    }

    /// A rotation built by `tomi-core` from the trust store, revoking `revoke`.
    pub fn rotation_for(
        &self,
        ts: &TrustStore,
        issuer: &Dev,
        revoke: &[&Dev],
    ) -> (Vec<u8>, KeyRotation) {
        let pks: Vec<Vec<u8>> = revoke.iter().map(|d| d.pk()).collect();
        let bundle = ts
            .create_rotation(&issuer.keys, &pks, self.clock.now_ms() as u64)
            .unwrap();
        (bundle.rotation.to_bytes().unwrap(), bundle.rotation)
    }
}

// ===== WebSocket client ==================================================================

pub use tokio_tungstenite::tungstenite::Message as WsMessage;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct Ws {
    pub s: WsStream,
}

/// A socket read in the background that waits for a close frame (see [`Ws::watch_close`]).
pub struct CloseWatch {
    task: tokio::task::JoinHandle<Option<u16>>,
}

impl CloseWatch {
    /// The code of the close frame, waiting at most `ms` milliseconds from now; `None` if the
    /// socket ended without one, or if nothing came in time (the reading stops then).
    pub async fn code(mut self, ms: u64) -> Option<u16> {
        match tokio::time::timeout(Duration::from_millis(ms), &mut self.task).await {
            Ok(Ok(code)) => code,
            Ok(Err(_)) => None,
            Err(_) => {
                self.task.abort();
                None
            }
        }
    }

    /// `true` if the socket stays open (no close frame, no end of the stream) for `ms`
    /// milliseconds, the relay's pings being answered meanwhile.
    pub async fn still_open(mut self, ms: u64) -> bool {
        match tokio::time::timeout(Duration::from_millis(ms), &mut self.task).await {
            Err(_) => {
                self.task.abort();
                true
            }
            Ok(_) => false,
        }
    }
}

impl Ws {
    pub async fn connect(addr: SocketAddr) -> Ws {
        Self::connect_path(addr, "/v1/ws").await
    }

    pub async fn connect_path(addr: SocketAddr, path: &str) -> Ws {
        let (s, _) = tokio_tungstenite::connect_async(format!("ws://{addr}{path}"))
            .await
            .unwrap();
        Ws { s }
    }

    pub async fn send_json(&mut self, v: Value) {
        use futures_util::SinkExt;
        self.s
            .send(WsMessage::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    /// The next frame, or `None` after `ms` milliseconds.
    pub async fn recv(&mut self, ms: u64) -> Option<WsMessage> {
        use futures_util::StreamExt;
        match tokio::time::timeout(Duration::from_millis(ms), self.s.next()).await {
            Ok(Some(Ok(m))) => Some(m),
            _ => None,
        }
    }

    /// The next JSON text message (pings and pongs skipped), or `None` on timeout or close.
    pub async fn recv_json(&mut self, ms: u64) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv(left.as_millis() as u64 + 1).await? {
                WsMessage::Text(t) => return serde_json::from_str(t.as_str()).ok(),
                WsMessage::Ping(_) | WsMessage::Pong(_) => {}
                _ => return None,
            }
            if left.is_zero() {
                return None;
            }
        }
    }

    /// Waits for a close frame and returns its code.
    pub async fn recv_close(&mut self, ms: u64) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv(left.as_millis() as u64 + 1).await? {
                WsMessage::Close(Some(f)) => return Some(u16::from(f.code)),
                WsMessage::Close(None) => return Some(1005),
                _ => {}
            }
            if left.is_zero() {
                return None;
            }
        }
    }

    /// Reads this socket in the background FROM NOW ON and keeps the code of the close frame the
    /// relay sends. A test client that nobody reads does not answer the pings of the relay (every
    /// 300 ms, dropped 300 ms later in the tests): a socket left unread between its opening and
    /// the close the test expects is cut by the relay before that close, and `recv_close` then
    /// finds nothing. Use this right after opening, whatever the test does meanwhile, then
    /// [`CloseWatch::code`] where the close is expected.
    pub fn watch_close(self) -> CloseWatch {
        let task = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut ws = self;
            loop {
                match ws.s.next().await {
                    Some(Ok(WsMessage::Close(Some(f)))) => return Some(u16::from(f.code)),
                    Some(Ok(WsMessage::Close(None))) => return Some(1005),
                    Some(Ok(_)) => {}
                    // The stream ended without a close frame.
                    Some(Err(_)) | None => return None,
                }
            }
        });
        CloseWatch { task }
    }

    pub async fn auth(&mut self, token: &str) -> Value {
        self.send_json(json!({ "type": "auth", "token": token }))
            .await;
        self.recv_json(2000).await.expect("ready")
    }

    /// Connects and authenticates `d`.
    pub async fn open(addr: SocketAddr, d: &Dev) -> Ws {
        let mut ws = Ws::connect(addr).await;
        let ready = ws.auth(d.tok()).await;
        assert_eq!(ready["type"], "ready", "{ready}");
        ws
    }
}
