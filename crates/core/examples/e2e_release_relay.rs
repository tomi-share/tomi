//! End-to-end test of the RELEASE binary of the relay (`target/release/tomi-relay`), driven by
//! the real client (`tomi_core::runtime`: `Engine`, `HttpTransport`, founder / invite / join).
//!
//! It proves that several people each have their own isolated space on one relay:
//!
//! 1. start the binary on a free loopback port and a temporary `data_dir`; `/healthz`,
//!    `/v1/hello` (`space_initialized = false`, `relay_version = 0.2.0`);
//! 2. `tomi-relay admin invite --new` twice: two install tokens;
//! 3. person A creates a space (token 1), person B creates another (token 2);
//! 4. a second device is paired for A (real pairing, two threads, codes compared), an item
//!    with an attachment goes A1 -> A2 and is received and decrypted;
//! 5. same for B (with a secret item);
//! 6. isolation: B sees nothing of A (feed, event log, devices, blobs, pairing channel) and
//!    the other way round; nothing of the plaintext reaches the relay's disk or logs;
//! 7. the relay is killed and started again on the same data: everything is still there and
//!    keeps working, a third device of A imports the history.
//!
//! Everything happens on 127.0.0.1 under `<temp>/tomi-e2e-run`, which is removed at the end
//! (set `TOMI_E2E_KEEP=1` to keep it). The production relay is never contacted.
//!
//! Usage (the release binary must already be built, this example is not):
//!
//! ```text
//! cargo run -p tomi-core --example e2e_release_relay
//! ```
//!
//! `TOMI_E2E_RELAY_BIN` overrides the path of the binary. The example must be built in the
//! default (dev) profile: the HTTP transport only accepts plain `http://127.0.0.1` there.
//! The exit code is 0 when every check is OK, 1 otherwise.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tomi_core::runtime::http::HttpTransport;
use tomi_core::runtime::transport::{EventData, PairingAuth, RelayError, RelayTransport};
use tomi_core::runtime::{
    DeliveryState, DispatchOutcome, DraftAttachment, DraftText, Engine, EngineConfig,
    InviteOutcome, ItemContentView, ItemOrigin, ItemView, JoinOutcome, OutgoingDraft, Store,
    SystemClock,
};
use tomi_core::{CoreError, ReadPolicy, SourceFormat};

type R<T> = Result<T, Box<dyn Error>>;

/// Wrapping key of the local stores (the OS secure storage in the apps).
const WRAP: [u8; 32] = [7u8; 32];
const EXPECTED_VERSION: &str = "0.2.0";

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    steps: Vec<(String, bool)>,
    failures: Vec<String>,
}

impl Report {
    fn step(&mut self, title: &str) {
        println!("\n== {title}");
        self.steps.push((title.to_owned(), true));
    }

    fn check(&mut self, ok: bool, what: &str, detail: impl Display) {
        println!("  [{}] {what}: {detail}", if ok { "OK" } else { "KO" });
        if !ok {
            if let Some(last) = self.steps.last_mut() {
                last.1 = false;
            }
            self.failures.push(format!("{what}: {detail}"));
        }
    }

    fn info(&self, text: impl Display) {
        println!("  [..] {text}");
    }

    fn probes(&mut self, probes: Vec<Probe>) {
        for p in probes {
            self.check(p.ok, &p.what, p.detail);
        }
    }
}

/// The outcome of an attempt that is expected to be refused.
struct Probe {
    what: String,
    ok: bool,
    detail: String,
}

/// An attempt on an identifier of another space: it must be refused (`401`, `403` or `404`).
fn denied<T>(what: impl Into<String>, r: Result<T, RelayError>) -> Probe {
    let what = what.into();
    match r {
        Err(RelayError::Api(p)) if matches!(p.status, 401 | 403 | 404) => Probe {
            what,
            ok: true,
            detail: format!("refused {} {}", p.status, p.code),
        },
        Err(e) => Probe {
            what,
            ok: false,
            detail: format!("unexpected error: {e}"),
        },
        Ok(_) => Probe {
            what,
            ok: false,
            detail: "ACCEPTED, it should have been refused".into(),
        },
    }
}

// ---------------------------------------------------------------------------
// The relay process
// ---------------------------------------------------------------------------

struct Relay {
    exe: PathBuf,
    port: u16,
    data_dir: PathBuf,
    logs: PathBuf,
    child: Option<Child>,
    runs: u32,
}

impl Relay {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The command with a clean `TOMI_RELAY_*` environment: nothing inherited can point it at
    /// another data directory or address.
    fn command(&self) -> Command {
        let mut c = Command::new(&self.exe);
        for (k, _) in std::env::vars() {
            if k.starts_with("TOMI_RELAY_") || k == "RUST_LOG" {
                c.env_remove(k);
            }
        }
        c.env("TOMI_RELAY_LISTEN", format!("127.0.0.1:{}", self.port))
            .env("TOMI_RELAY_DATA_DIR", &self.data_dir)
            .env("TOMI_RELAY_LOG_FORMAT", "text")
            .env("TOMI_RELAY_PUBLIC_URL", self.url())
            // Every device here comes from 127.0.0.1, so they share the per-IP buckets that
            // production spreads over many addresses. `rl_ip_public` (burst 10) is spent by
            // each `/v1/space`, `/v1/auth/challenge`, `/v1/auth/session` and pairing claim,
            // `rl_ip_auth` (burst 10) by each challenge and session: the default values answer
            // `429` to the 11th such call of the run. Only these two are raised; the others
            // (per device, pairing, claims, space creation) keep their defaults.
            .env("TOMI_RELAY_RL_IP_PUBLIC", "6000,2000")
            .env("TOMI_RELAY_RL_IP_AUTH", "6000,2000")
            .stdin(Stdio::null());
        c
    }

    fn start(&mut self) -> R<()> {
        self.runs += 1;
        let out = fs::File::create(self.logs.join(format!("relay-{}.out.log", self.runs)))?;
        let err = fs::File::create(self.logs.join(format!("relay-{}.err.log", self.runs)))?;
        let child = self.command().stdout(out).stderr(err).spawn()?;
        self.child = Some(child);
        let started = Instant::now();
        loop {
            if let Some(child) = self.child.as_mut() {
                if let Some(status) = child.try_wait()? {
                    return Err(format!("the relay exited at start: {status}").into());
                }
            }
            if matches!(http_get(self.port, "/healthz"), Ok((200, _))) {
                return Ok(());
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err("the relay does not answer /healthz after 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Kills the process (Windows has no gentle signal for a console-less child).
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// `tomi-relay admin <args>`: exit code, stdout, stderr.
    fn admin(&self, args: &[&str]) -> R<(i32, String, String)> {
        let out = self
            .command()
            .arg("admin")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    fn all_logs(&self) -> String {
        let mut text = String::new();
        if let Ok(rd) = fs::read_dir(&self.logs) {
            for e in rd.flatten() {
                text.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
            }
        }
        text
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop();
    }
}

fn free_port() -> R<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Minimal `GET` over HTTP/1.0 (no chunking, no keep-alive): status and body.
fn http_get(port: u16, path: &str) -> R<(u16, String)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    write!(s, "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n")?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").ok_or("malformed HTTP answer")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("malformed status line")?;
    Ok((status, body.to_owned()))
}

fn release_binary() -> R<PathBuf> {
    if let Ok(p) = std::env::var("TOMI_E2E_RELAY_BIN") {
        return Ok(PathBuf::from(p));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let exe = root
        .join("target")
        .join("release")
        .join(format!("tomi-relay{}", std::env::consts::EXE_SUFFIX));
    Ok(exe.canonicalize()?)
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

struct Dev {
    label: String,
    engine: Arc<Engine>,
    /// The transport of the engine: after a `sync` it holds the session of the device, so a
    /// probe made with it is made as that device.
    transport: Arc<HttpTransport>,
    dir: PathBuf,
    platform: String,
}

impl Dev {
    fn open(root: &Path, label: &str, platform: &str, url: &str) -> R<Dev> {
        let dir = root.join(label);
        let store = Store::open(&dir.join("tomi.db"), &dir.join("files"), &WRAP)?;
        let transport = Arc::new(HttpTransport::new(url)?);
        let engine = Engine::open(
            EngineConfig {
                platform: platform.into(),
                ..EngineConfig::default()
            },
            store,
            transport.clone(),
            Arc::new(SystemClock),
        )?;
        Ok(Dev {
            label: label.to_owned(),
            engine: Arc::new(engine),
            transport,
            dir,
            platform: platform.to_owned(),
        })
    }

    /// The app is closed and opened again on the same files.
    fn reopen(self, url: &str) -> R<Dev> {
        let Dev {
            label,
            engine,
            dir,
            platform,
            ..
        } = self;
        drop(engine);
        let root = dir.parent().ok_or("no parent directory")?.to_owned();
        Dev::open(&root, &label, &platform, url)
    }

    fn id(&self) -> R<String> {
        Ok(self.engine.device_id()?)
    }

    fn space_id(&self) -> R<String> {
        Ok(self.engine.space_id()?)
    }
}

/// Pairs `joiner` through `inviter` with the real blocking calls on two threads. The two
/// "users" compare the verification codes: pairing goes on only if they are equal. `during`
/// runs while the channel is open (the isolation probes use it).
fn pair_with(
    inviter: &Dev,
    joiner: &Dev,
    name: &str,
    history: bool,
    during: impl FnOnce(&str) -> Vec<Probe>,
) -> R<(InviteOutcome, JoinOutcome, Vec<Probe>)> {
    let inv = inviter.engine.start_invitation()?;
    let code = inv.code().ok_or("the invitation has no code")?;
    let probes = during(inv.channel_id());
    for p in &probes {
        println!("  [..] probe on the open channel: {}: {}", p.what, p.detail);
    }
    // Each thread reports the verification code it shows, or why it stopped.
    let (events_tx, events_rx) = mpsc::channel::<(&'static str, Result<String, String>)>();
    let (go_a_tx, go_a_rx) = mpsc::channel::<()>();
    let (go_n_tx, go_n_rx) = mpsc::channel::<()>();
    let engine = joiner.engine.clone();
    let (invited, joined) = std::thread::scope(|scope| {
        let inv_ref = &inv;
        let tx = events_tx.clone();
        let a = scope.spawn(move || -> Result<InviteOutcome, String> {
            let prompt = inv_ref.wait_for_peer().map_err(|e| {
                let _ = tx.send(("inviter", Err(e.to_string())));
                e.to_string()
            })?;
            tx.send(("inviter", Ok(prompt.sas)))
                .map_err(|e| e.to_string())?;
            go_a_rx.recv().map_err(|_| "cancelled".to_owned())?;
            inv_ref.confirm().map_err(|e| e.to_string())?;
            inv_ref.wait_done().map_err(|e| e.to_string())
        });
        let code_ref = &code;
        let tx = events_tx;
        let n = scope.spawn(move || -> Result<JoinOutcome, String> {
            let fail = |e: CoreError| {
                let _ = tx.send(("new device", Err(e.to_string())));
                e.to_string()
            };
            let js = engine.join_space(code_ref, name).map_err(fail)?;
            let prompt = js.wait_for_inviter().map_err(fail)?;
            tx.send(("new device", Ok(prompt.sas)))
                .map_err(|e| e.to_string())?;
            go_n_rx.recv().map_err(|_| "cancelled".to_owned())?;
            js.confirm(history).map_err(|e| e.to_string())?;
            js.wait_done().map_err(|e| e.to_string())
        });
        // The two users compare the codes, on this thread.
        let mut shown = Vec::new();
        let mut broken = false;
        while shown.len() < 2 && !broken {
            match events_rx.recv_timeout(Duration::from_secs(90)) {
                Ok((_, Ok(sas))) => shown.push(sas),
                _ => broken = true,
            }
        }
        if !broken && shown[0] == shown[1] {
            let _ = go_a_tx.send(());
            let _ = go_n_tx.send(());
        } else {
            // Stop the side that still waits.
            inv.cancel();
            drop((go_a_tx, go_n_tx));
        }
        (a.join(), n.join())
    });
    let invited = invited.map_err(|_| "the inviter thread panicked");
    let joined = joined.map_err(|_| "the joiner thread panicked");
    let (invited, joined) = match (invited?, joined?) {
        (Ok(i), Ok(j)) => (i, j),
        (i, j) => {
            return Err(format!(
                "pairing failed (inviter: {}; new device: {})",
                i.err().unwrap_or_else(|| "ok".into()),
                j.err().unwrap_or_else(|| "ok".into())
            )
            .into())
        }
    };
    Ok((invited, joined, probes))
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

fn draft(title: &str, plain: &str, secret: bool, file: Option<(&Path, &str)>) -> OutgoingDraft {
    OutgoingDraft {
        id: None,
        title: Some(title.to_owned()),
        text: Some(DraftText {
            plain: plain.to_owned(),
            html: None,
            markdown: None,
            source_format: SourceFormat::Plain,
        }),
        url: None,
        attachments: file
            .map(|(path, name)| DraftAttachment {
                path: path.display().to_string(),
                name: name.to_owned(),
                mime: "application/octet-stream".into(),
                thumbnail: None,
            })
            .into_iter()
            .collect(),
        secret,
        expires_at_ms: None,
        read_policy: ReadPolicy::UntilExpiry,
    }
}

/// Enqueues and sends; the relay must have accepted it.
fn send(from: &Dev, d: OutgoingDraft) -> R<String> {
    let id = from.engine.enqueue(d)?;
    match from.engine.dispatch(&id)? {
        DispatchOutcome::Sent => Ok(id),
        other => Err(format!("{}: dispatch gave {other:?}", from.label).into()),
    }
}

/// Synchronises `dev` until it has the item (or `secs` pass).
fn wait_item(dev: &Dev, id: &str, secs: u64) -> R<Option<ItemView>> {
    let started = Instant::now();
    loop {
        dev.engine.sync()?;
        if let Some(v) = dev.engine.get_item(id)? {
            return Ok(Some(v));
        }
        if started.elapsed() > Duration::from_secs(secs) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Whether `sender` learns that `receiver_id` has the item.
fn wait_delivered(sender: &Dev, id: &str, receiver_id: &str, secs: u64) -> R<bool> {
    let started = Instant::now();
    loop {
        sender.engine.sync()?;
        if let Some(v) = sender.engine.get_item(id)? {
            if v.deliveries
                .iter()
                .any(|d| d.device_id == receiver_id && d.state == DeliveryState::Delivered)
            {
                return Ok(true);
            }
        }
        if started.elapsed() > Duration::from_secs(secs) {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn readable(v: &ItemView) -> Option<(Option<String>, bool, usize)> {
    match &v.content {
        ItemContentView::Readable {
            text,
            secret,
            attachments,
            ..
        } => Some((
            text.as_ref().map(|t| t.plain.clone()),
            *secret,
            attachments.len(),
        )),
        ItemContentView::Unreadable { .. } => None,
    }
}

fn feed_ids(dev: &Dev) -> R<BTreeSet<String>> {
    Ok(dev.engine.list_items()?.into_iter().map(|v| v.id).collect())
}

fn feed_texts(dev: &Dev) -> R<Vec<String>> {
    Ok(dev
        .engine
        .list_items()?
        .iter()
        .filter_map(|v| readable(v).and_then(|r| r.0))
        .collect())
}

/// What the relay's event log shows to the session of `dev`.
#[derive(Default)]
struct LogView {
    items: BTreeSet<String>,
    blobs: BTreeSet<String>,
    devices: BTreeSet<String>,
}

fn log_view(dev: &Dev) -> R<LogView> {
    let mut view = LogView::default();
    let mut after = 0;
    loop {
        let page = dev
            .transport
            .events(after, 200)
            .map_err(|e| format!("{}: events: {e}", dev.label))?;
        for ev in page.events {
            match ev.data {
                EventData::Item {
                    item_id,
                    sender_device_id,
                    blobs,
                    ..
                } => {
                    view.items.insert(item_id);
                    view.devices.insert(sender_device_id);
                    view.blobs.extend(blobs.into_iter().map(|b| b.blob_id));
                }
                EventData::Ack {
                    item_id, device_id, ..
                }
                | EventData::Read {
                    item_id, device_id, ..
                } => {
                    view.items.insert(item_id);
                    view.devices.insert(device_id);
                }
                EventData::DeviceAdded { device } => {
                    view.devices.insert(device.device_id);
                }
                EventData::DeviceUpdated { device_id, .. }
                | EventData::DeviceRevoked { device_id, .. } => {
                    view.devices.insert(device_id);
                }
                _ => {}
            }
        }
        if !page.has_more {
            return Ok(view);
        }
        after = page.next_after;
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                files_under(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

/// Path of a blob file of the relay (`spaces/<space_id>/blobs/<2 hex>/<hex>`).
fn blob_file(data_dir: &Path, space_id: &str, blob_id: &str) -> PathBuf {
    let hex = blob_id.replace('-', "").to_ascii_lowercase();
    data_dir
        .join("spaces")
        .join(space_id)
        .join("blobs")
        .join(&hex[..2.min(hex.len())])
        .join(hex)
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// The scenario
// ---------------------------------------------------------------------------

/// Isolation probes of `intruder` (a device of one space) on the identifiers of `victim`
/// (a device of another space). `label` names the direction in the report.
fn cross_space_probes(
    label: &str,
    intruder: &Dev,
    victim: &Dev,
    victim_item: &str,
    victim_blob: &str,
) -> R<Vec<Probe>> {
    let t = &intruder.transport;
    let victim_id = victim.id()?;
    let mut p = Vec::new();
    p.push(denied(
        format!("{label}: blob_status of a foreign blob"),
        t.blob_status(victim_blob),
    ));
    p.push(denied(
        format!("{label}: download of a foreign blob"),
        t.get_blob_range(victim_blob, 0, 16),
    ));
    p.push(denied(
        format!("{label}: deletion of a foreign blob"),
        t.delete_blob(victim_blob),
    ));
    p.push(denied(
        format!("{label}: ack of a foreign item"),
        t.ack_item(victim_item, false),
    ));
    p.push(denied(
        format!("{label}: read receipt of a foreign item"),
        t.read_item(victim_item),
    ));
    p.push(denied(
        format!("{label}: rename of a foreign device"),
        t.rename_device(&victim_id, "pwned"),
    ));
    Ok(p)
}

fn run(report: &mut Report, root: &Path) -> R<()> {
    let exe = release_binary().map_err(|e| {
        format!(
            "release binary not found ({e}); build it with `cargo build --release -p tomi-relay`"
        )
    })?;
    let data_dir = root.join("relay-data");
    let logs = root.join("relay-logs");
    fs::create_dir_all(&logs)?;
    fs::create_dir_all(root.join("payloads"))?;
    let port = free_port()?;
    let mut relay = Relay {
        exe: exe.clone(),
        port,
        data_dir: data_dir.clone(),
        logs,
        child: None,
        runs: 0,
    };
    let url = relay.url();
    let nonce = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let marker_a = format!("MARQUEUR-A-{nonce}");
    let marker_b = format!("MARQUEUR-B-{nonce}");
    let secret_b = format!("SECRET-B-{nonce}");
    let file_a = root.join("payloads").join("a.bin");
    let file_b = root.join("payloads").join("b.bin");
    let bytes_a = format!("FICHIER-A-{nonce}|").repeat(2000).into_bytes();
    let bytes_b = format!("FICHIER-B-{nonce}|").repeat(3000).into_bytes();
    fs::write(&file_a, &bytes_a)?;
    fs::write(&file_b, &bytes_b)?;

    // ------------------------------------------------------------------ 1
    report.step("1. Start the release binary, /healthz, /v1/hello");
    let version = Command::new(&exe).arg("--version").output()?;
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    report.check(
        version.ends_with(EXPECTED_VERSION),
        "binary",
        format!("{} ({version})", exe.display()),
    );
    report.info(format!(
        "listening on 127.0.0.1:{port}, data_dir {}",
        data_dir.display()
    ));
    relay.start()?;
    let (status, body) = http_get(port, "/healthz")?;
    report.check(
        status == 200 && json_of(&body)["status"] == "ok",
        "GET /healthz",
        format!("{status} {body}"),
    );
    let (status, body) = http_get(port, "/v1/hello")?;
    let hello = json_of(&body);
    report.check(
        status == 200
            && hello["relay_version"] == EXPECTED_VERSION
            && hello["space_initialized"] == false,
        "GET /v1/hello",
        format!(
            "{status} relay_version={} space_initialized={}",
            hello["relay_version"], hello["space_initialized"]
        ),
    );
    let logs_now = relay.all_logs();
    report.check(
        logs_now.contains("install token"),
        "first start logs the install token (documented exception)",
        "line present (value not shown)",
    );

    // ------------------------------------------------------------------ 2
    report.step("2. tomi-relay admin invite --new (two tokens)");
    let live = (
        relay.admin(&["invite", "--new"])?,
        relay.admin(&["invite", "--new"])?,
    );
    let tokens: Vec<String> = if live.0 .0 == 0 && live.1 .0 == 0 {
        report.info("the commands worked on the running relay");
        vec![live.0 .1.trim().to_owned(), live.1 .1.trim().to_owned()]
    } else {
        report.info(format!(
            "on the running relay: exit {}, \"{}\" (this platform needs the relay stopped to change state)",
            live.0 .0,
            live.0 .2.trim()
        ));
        relay.stop();
        let first = relay.admin(&["invite", "--new"])?;
        let second = relay.admin(&["invite", "--new"])?;
        report.check(
            first.0 == 0 && second.0 == 0,
            "admin invite --new on the stopped relay",
            format!("exit {} / {} {}", first.0, second.0, first.2.trim()),
        );
        let (_, list, _) = relay.admin(&["invite"])?;
        let pending = list
            .lines()
            .filter(|l| l.starts_with("invitation "))
            .count();
        report.check(
            pending >= 2,
            "pending invitations listed",
            format!("{pending} (one from the first start, two created now)"),
        );
        relay.start()?;
        vec![first.1.trim().to_owned(), second.1.trim().to_owned()]
    };
    let (token_a, token_b) = (tokens[0].clone(), tokens[1].clone());
    report.check(
        token_a.len() == 39 && token_b.len() == 39 && token_a != token_b,
        "two distinct tokens",
        format!(
            "lengths {} and {} (values not shown)",
            token_a.len(),
            token_b.len()
        ),
    );

    // ------------------------------------------------------------------ 3
    report.step("3. A and B each create their space");
    let a1 = Dev::open(root, "a1", "windows", &url)?;
    let b1 = Dev::open(root, "b1", "windows", &url)?;
    a1.engine.create_space(&token_a, "A-PC")?;
    b1.engine.create_space(&token_b, "B-PC")?;
    let (space_a, space_b) = (a1.space_id()?, b1.space_id()?);
    report.check(
        a1.engine.is_provisioned() && b1.engine.is_provisioned(),
        "both founders provisioned",
        format!("A {space_a}, B {space_b}"),
    );
    report.check(space_a != space_b, "distinct space ids", "");
    let (_, body) = http_get(port, "/v1/hello")?;
    report.check(
        json_of(&body)["space_initialized"] == true,
        "hello: space_initialized",
        "true",
    );
    let (code, spaces, _) = relay.admin(&["spaces", "--json"])?;
    let spaces = json_of(&spaces);
    let rows = spaces["spaces"].as_array().cloned().unwrap_or_default();
    report.check(
        code == 0 && rows.len() == 2 && rows.iter().all(|r| r["members"] == 1),
        "admin spaces: 2 spaces, 1 member each",
        format!("{} space(s)", rows.len()),
    );
    // A single-use token: replaying it, or a made-up one, creates nothing.
    let intruder = Dev::open(root, "intruder", "windows", &url)?;
    let replay = intruder.engine.create_space(&token_a, "Intrus");
    report.check(
        matches!(&replay, Err(CoreError::Relay { status: 409, code }) if code == "space_exists"),
        "replaying the consumed token of A",
        format!("{replay:?}"),
    );
    let bogus = intruder
        .engine
        .create_space("aaaa-bbbb-cccc-dddd-eeee-ffff-gggg-hhhh", "Intrus");
    report.check(
        matches!(&bogus, Err(CoreError::Relay { status: 403, .. })),
        "a made-up token",
        format!("{bogus:?}"),
    );

    // ------------------------------------------------------------------ 4
    report.step("4. Person A: pair a 2nd device, send A1 -> A2");
    let a2 = Dev::open(root, "a2", "android", &url)?;
    let (inv, joined, probes_b_on_a_channel) = pair_with(&a1, &a2, "A-Pixel", false, |channel| {
        vec![
            denied(
                "B reads the messages of A's pairing channel",
                b1.transport
                    .pairing_messages(channel, PairingAuth::Inviter, 0, 0),
            ),
            denied(
                "B closes A's pairing channel",
                b1.transport
                    .close_pairing_channel(channel, PairingAuth::Inviter),
            ),
        ]
    })?;
    report.check(
        a2.engine.is_provisioned()
            && inv.device_id == joined.device_id
            && joined.space_id == space_a,
        "A2 paired into the space of A",
        format!("device {}", joined.device_id),
    );
    let item_a = send(
        &a1,
        draft("Titre A", &marker_a, false, Some((&file_a, "a.bin"))),
    )?;
    let got = wait_item(&a2, &item_a, 20)?;
    let got_ok = got.as_ref().and_then(readable);
    report.check(
        got_ok
            .as_ref()
            .is_some_and(|r| r.0.as_deref() == Some(marker_a.as_str()) && !r.1 && r.2 == 1),
        "A2 received and decrypted the text of A1",
        got_ok.map_or("not received".to_owned(), |r| {
            format!("{} attachment(s)", r.2)
        }),
    );
    report.check(
        got.as_ref().is_some_and(|v| {
            v.origin == ItemOrigin::Received && v.source_device_id == a1.id().unwrap_or_default()
        }),
        "origin Received, source = A1",
        "",
    );
    let path = a2.engine.attachment_path(&item_a, 0)?;
    let file_ok = path
        .as_ref()
        .and_then(|p| fs::read(p).ok())
        .is_some_and(|b| b == bytes_a);
    report.check(
        file_ok,
        "A2 decrypted the attachment (byte for byte)",
        format!("{} bytes", bytes_a.len()),
    );
    report.check(
        wait_delivered(&a1, &item_a, &a2.id()?, 20)?,
        "A1 sees the item as delivered to A2",
        "",
    );

    // ------------------------------------------------------------------ 5
    report.step("5. Person B: pair a 2nd device, send B1 -> B2 (text + secret)");
    let b2 = Dev::open(root, "b2", "android", &url)?;
    let (inv, joined, probes_a_on_b_channel) = pair_with(&b1, &b2, "B-Pixel", false, |channel| {
        vec![
            denied(
                "A reads the messages of B's pairing channel",
                a1.transport
                    .pairing_messages(channel, PairingAuth::Inviter, 0, 0),
            ),
            denied(
                "A closes B's pairing channel",
                a1.transport
                    .close_pairing_channel(channel, PairingAuth::Inviter),
            ),
        ]
    })?;
    report.check(
        b2.engine.is_provisioned()
            && inv.device_id == joined.device_id
            && joined.space_id == space_b,
        "B2 paired into the space of B",
        format!("device {}", joined.device_id),
    );
    let item_b = send(
        &b1,
        draft("Titre B", &marker_b, false, Some((&file_b, "b.bin"))),
    )?;
    let item_b_secret = send(&b1, draft("Titre secret B", &secret_b, true, None))?;
    let got = wait_item(&b2, &item_b, 20)?;
    let got_ok = got.as_ref().and_then(readable);
    report.check(
        got_ok
            .as_ref()
            .is_some_and(|r| r.0.as_deref() == Some(marker_b.as_str()) && !r.1 && r.2 == 1),
        "B2 received and decrypted the text of B1",
        got_ok.map_or("not received".to_owned(), |r| {
            format!("{} attachment(s)", r.2)
        }),
    );
    let path = b2.engine.attachment_path(&item_b, 0)?;
    report.check(
        path.as_ref()
            .and_then(|p| fs::read(p).ok())
            .is_some_and(|b| b == bytes_b),
        "B2 decrypted the attachment (byte for byte)",
        format!("{} bytes", bytes_b.len()),
    );
    let got = wait_item(&b2, &item_b_secret, 20)?;
    let got_ok = got.as_ref().and_then(readable);
    report.check(
        got_ok
            .as_ref()
            .is_some_and(|r| r.0.as_deref() == Some(secret_b.as_str()) && r.1),
        "B2 received the secret item, flagged secret",
        "",
    );
    report.check(
        wait_delivered(&b1, &item_b, &b2.id()?, 20)?,
        "B1 sees the item as delivered to B2",
        "",
    );

    // ------------------------------------------------------------------ 6
    report.step("6. Isolation between the two spaces");
    report.probes(probes_b_on_a_channel);
    report.probes(probes_a_on_b_channel);
    a1.engine.sync()?;
    b1.engine.sync()?;
    let (ids_a, ids_b) = (feed_ids(&a2)?, feed_ids(&b2)?);
    report.check(
        ids_a.contains(&item_a) && ids_a.is_disjoint(&ids_b),
        "feeds are disjoint (A2 / B2)",
        format!("A2 {} item(s), B2 {} item(s)", ids_a.len(), ids_b.len()),
    );
    let (texts_a, texts_b) = (feed_texts(&a1)?, feed_texts(&b1)?);
    report.check(
        !texts_b.iter().any(|t| t.contains("MARQUEUR-A"))
            && !texts_a
                .iter()
                .any(|t| t.contains("MARQUEUR-B") || t.contains("SECRET-B")),
        "no plaintext of one person in the feed of the other",
        format!("A1 {} item(s), B1 {} item(s)", texts_a.len(), texts_b.len()),
    );
    let (view_a, view_b) = (log_view(&a1)?, log_view(&b1)?);
    let (devs_a, devs_b) = (
        [a1.id()?, a2.id()?].into_iter().collect::<BTreeSet<_>>(),
        [b1.id()?, b2.id()?].into_iter().collect::<BTreeSet<_>>(),
    );
    report.check(
        view_a.items == BTreeSet::from([item_a.clone()])
            && view_b.items == BTreeSet::from([item_b.clone(), item_b_secret.clone()]),
        "event log of each session holds only its own items",
        format!(
            "A {} item(s), B {} item(s)",
            view_a.items.len(),
            view_b.items.len()
        ),
    );
    report.check(
        view_a.devices.is_subset(&devs_a) && view_b.devices.is_subset(&devs_b),
        "event log of each session names only its own devices",
        format!(
            "A {} device(s), B {} device(s)",
            view_a.devices.len(),
            view_b.devices.len()
        ),
    );
    let list_a: BTreeSet<_> = a1
        .transport
        .devices()?
        .into_iter()
        .map(|d| d.device_id)
        .collect();
    let list_b: BTreeSet<_> = b1
        .transport
        .devices()?
        .into_iter()
        .map(|d| d.device_id)
        .collect();
    report.check(
        list_a == devs_a && list_b == devs_b,
        "GET /v1/devices lists only the devices of the space",
        format!("A {}, B {}", list_a.len(), list_b.len()),
    );
    let (blob_a, blob_b) = (
        view_a.blobs.iter().next().cloned().ok_or("A has no blob")?,
        view_b.blobs.iter().next().cloned().ok_or("B has no blob")?,
    );
    report.probes(cross_space_probes("B on A", &b1, &a1, &item_a, &blob_a)?);
    report.probes(cross_space_probes("A on B", &a1, &b1, &item_b, &blob_b)?);
    // The refused attempts changed nothing.
    let still = a1.transport.blob_status(&blob_a);
    report.check(
        still.as_ref().is_ok_and(|s| s.complete),
        "A's blob survived B's deletion attempt",
        format!("{:?}", still.map(|s| s.size)),
    );
    a1.engine.sync()?;
    let names: Vec<String> = a1
        .engine
        .list_devices()?
        .into_iter()
        .map(|d| d.name)
        .collect();
    report.check(
        !names.iter().any(|n| n == "pwned"),
        "A's device names unchanged after B's rename attempt",
        names.join(", "),
    );
    // Files on the relay's disk: one directory per space, the blobs in the right one.
    let on_disk = |space: &str, blob: &str| blob_file(&data_dir, space, blob).is_file();
    report.check(
        on_disk(&space_a, &blob_a)
            && on_disk(&space_b, &blob_b)
            && !on_disk(&space_a, &blob_b)
            && !on_disk(&space_b, &blob_a),
        "blob files live in their own space directory only",
        format!(
            "spaces/{}/blobs, spaces/{}/blobs",
            &space_a[..8],
            &space_b[..8]
        ),
    );
    let (code, devs_json, _) = relay.admin(&["devices", "--space", &space_a[..8], "--json"])?;
    let listed = json_of(&devs_json)["devices"]
        .as_array()
        .map_or(0, Vec::len);
    report.check(
        code == 0
            && listed == 2
            && !devs_json.contains(&b1.id()?)
            && !devs_json.contains(&b2.id()?),
        "admin devices --space A: 2 devices, none of B",
        format!("{listed} device(s)"),
    );
    // Blind relay: no plaintext on its disk or in its logs.
    let mut files = Vec::new();
    files_under(&data_dir, &mut files);
    let secrets = [
        marker_a.as_str(),
        marker_b.as_str(),
        secret_b.as_str(),
        "FICHIER-A-",
        "FICHIER-B-",
        "Titre A",
        "Titre B",
        "Titre secret B",
    ];
    let mut leaks = Vec::new();
    let mut unreadable = Vec::new();
    for f in &files {
        match fs::read(f) {
            Ok(bytes) => {
                for s in secrets {
                    if contains(&bytes, s.as_bytes()) {
                        leaks.push(format!("{} in {}", s, f.display()));
                    }
                }
            }
            Err(_) => unreadable.push(
                f.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            ),
        }
    }
    report.check(
        // `relay.lock` is held open by the running relay (Windows refuses to read it).
        leaks.is_empty() && unreadable.iter().all(|n| n == "relay.lock"),
        "no plaintext (text, title, secret, file) in the relay's data_dir",
        format!(
            "{} file(s) scanned, unreadable (locked): {unreadable:?}, leaks: {leaks:?}",
            files.len()
        ),
    );
    let logs_text = relay.all_logs();
    let log_leaks: Vec<&str> = secrets
        .iter()
        .copied()
        .chain([token_a.as_str(), token_b.as_str()])
        .filter(|s| logs_text.contains(s))
        .map(|s| {
            if s.len() == 39 {
                "an invitation token"
            } else {
                s
            }
        })
        .collect();
    report.check(
        log_leaks.is_empty(),
        "no plaintext or invitation token in the relay's logs",
        format!("{} byte(s) of logs, leaks: {log_leaks:?}", logs_text.len()),
    );

    // ------------------------------------------------------------------ 7
    report.step("7. Kill the relay, start it again on the same data");
    relay.stop();
    let down = http_get(port, "/healthz").is_err();
    report.check(down, "the process is stopped", "no answer on the port");
    relay.start()?;
    let (status, body) = http_get(port, "/healthz")?;
    report.check(
        status == 200,
        "GET /healthz after restart",
        format!("{status} {body}"),
    );
    let (_, body) = http_get(port, "/v1/hello")?;
    let hello = json_of(&body);
    report.check(
        hello["space_initialized"] == true && hello["relay_version"] == EXPECTED_VERSION,
        "hello after restart: space_initialized, version",
        format!("{} {}", hello["space_initialized"], hello["relay_version"]),
    );
    let (code, spaces, _) = relay.admin(&["spaces", "--json"])?;
    let rows = json_of(&spaces)["spaces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: BTreeSet<String> = rows
        .iter()
        .filter_map(|r| r["space_id"].as_str().map(str::to_owned))
        .collect();
    report.check(
        code == 0
            && rows.len() == 2
            && rows.iter().all(|r| r["members"] == 2)
            && ids == BTreeSet::from([space_a.clone(), space_b.clone()]),
        "admin spaces: the same 2 spaces, 2 members each",
        format!("{} space(s)", rows.len()),
    );
    // The apps are closed and opened again: everything is on their disk.
    let a1 = a1.reopen(&url)?;
    let b1 = b1.reopen(&url)?;
    report.check(
        a1.engine.is_provisioned()
            && b1.engine.is_provisioned()
            && feed_ids(&a1)?.contains(&item_a),
        "devices reopen provisioned with their feed",
        "A1, B1",
    );
    // The relay's log still has the old items and blobs, per space.
    a1.engine.sync()?;
    b1.engine.sync()?;
    let (view_a2, view_b2) = (log_view(&a1)?, log_view(&b1)?);
    report.check(
        view_a2.items == view_a.items && view_b2.items == view_b.items,
        "the event log of each space is intact",
        format!(
            "A {} item(s), B {} item(s)",
            view_a2.items.len(),
            view_b2.items.len()
        ),
    );
    let status = a1.transport.blob_status(&blob_a);
    let bytes = a1.transport.get_blob_range(&blob_a, 0, 32);
    report.check(
        status.as_ref().is_ok_and(|s| s.complete) && bytes.as_ref().is_ok_and(|b| b.len() == 32),
        "A's blob is still served after restart",
        format!("{:?}", status.map(|s| s.size)),
    );
    // Sending still works in both directions, in both spaces.
    let item_a2 = send(
        &a2,
        draft("Retour A", &format!("{marker_a}-retour"), false, None),
    )?;
    let item_b2 = send(
        &b2,
        draft("Retour B", &format!("{marker_b}-retour"), false, None),
    )?;
    let back_a = wait_item(&a1, &item_a2, 20)?.as_ref().and_then(readable);
    let back_b = wait_item(&b1, &item_b2, 20)?.as_ref().and_then(readable);
    report.check(
        back_a.is_some_and(|r| r.0.as_deref() == Some(format!("{marker_a}-retour").as_str()))
            && back_b
                .is_some_and(|r| r.0.as_deref() == Some(format!("{marker_b}-retour").as_str())),
        "A2 -> A1 and B2 -> B1 work after restart",
        "",
    );
    report.check(
        !feed_ids(&a1)?.contains(&item_b2) && !feed_ids(&b1)?.contains(&item_a2),
        "still no crossing after restart",
        "",
    );
    // A third device of A joins after the restart and imports the history.
    let a3 = Dev::open(root, "a3", "android", &url)?;
    let (_, joined, _) = pair_with(&a1, &a3, "A-Tablette", true, |_| Vec::new())?;
    report.check(
        joined.space_id == space_a && joined.history.is_some(),
        "A3 paired after restart, history transfer announced",
        format!("{:?}", joined.history.is_some()),
    );
    let started = Instant::now();
    let mut imported = false;
    while started.elapsed() < Duration::from_secs(60) {
        a1.engine.sync()?;
        a3.engine.sync()?;
        let ids = feed_ids(&a3)?;
        if ids.contains(&item_a) && ids.contains(&item_a2) {
            imported = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    report.check(
        imported,
        "A3 imported the history of A (the items sent before it existed)",
        format!("{} item(s) in its feed", feed_ids(&a3)?.len()),
    );
    let ids_a3 = feed_ids(&a3)?;
    report.check(
        ids_a3.is_disjoint(&feed_ids(&b1)?),
        "A3 has nothing of B",
        "",
    );
    let leftover = relay.all_logs();
    report.check(
        !secrets.iter().any(|s| leftover.contains(s)),
        "relay logs still free of plaintext after the whole run",
        format!("{} byte(s)", leftover.len()),
    );
    relay.stop();
    Ok(())
}

fn main() -> ExitCode {
    let root = std::env::temp_dir().join("tomi-e2e-run");
    if root.exists() {
        if let Err(e) = fs::remove_dir_all(&root) {
            eprintln!("cannot clear {}: {e}", root.display());
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = fs::create_dir_all(&root) {
        eprintln!("cannot create {}: {e}", root.display());
        return ExitCode::FAILURE;
    }
    let mut report = Report::default();
    let result = run(&mut report, &root);
    let relay_logs_tail = fs::read_dir(root.join("relay-logs"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    if std::env::var("TOMI_E2E_KEEP").is_ok() {
        println!("\nkept: {}", root.display());
    } else if let Err(e) = fs::remove_dir_all(&root) {
        eprintln!("cannot remove {}: {e}", root.display());
    }
    println!("\n== Summary ({relay_logs_tail} relay log file(s) were written)");
    for (title, ok) in &report.steps {
        println!("  [{}] {title}", if *ok { "OK" } else { "KO" });
    }
    if let Err(e) = &result {
        println!("  [KO] the scenario stopped: {e}");
    }
    for f in &report.failures {
        println!("  failed check: {f}");
    }
    if result.is_ok() && report.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
