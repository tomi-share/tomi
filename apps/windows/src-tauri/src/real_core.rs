//! The real core: the `runtime` of `tomi-core` (SQLite store, sync engine, outbox, history
//! transfer, HTTPS / WebSocket transport) behind the app's [`TomiCore`] trait.
//!
//! This file adapts, it never decides: no crypto, no protocol. What it owns is the
//! plumbing of a desktop host:
//!
//! * the wrapping key of the database, sealed by DPAPI ([`crate::wrapkey`]);
//! * the engine's blocking calls, moved off the async threads;
//! * the stream of item changes, rebuilt from the engine's "something changed" callback
//!   by comparing the feed with the last one the UI saw;
//! * the doorbell (WebSocket) thread and a slow safety-net synchronisation;
//! * the decrypted copies of attachments the user opened: named like the file (the core's
//!   cache is named after ids), kept in a directory of their own, wiped with the core's;
//! * the departure (`spec/api-v1.md` 7.9): the engine's own `leave_space` / `delete_space` /
//!   `continue_departure` / ..., mapped one to one. The engine erases on a positive proof of the
//!   relay and nowhere else; this file only runs the calls off the async threads, reads the
//!   outcome, and, once the engine says `Erased`, lets go of what the host itself holds in memory
//!   (the doorbell loop, the decrypted feed, the decrypted copies of files).
//!
//! Hidden items ("masqués") are never written in clear at rest. In memory, the bridge
//! keeps a snapshot of the feed to diff successive passes; it holds the decrypted items,
//! secrets included, for the life of the session (it could be pared down to redacted
//! copies of secrets, a defence-in-depth follow-up noted in the review). This is in-process
//! memory only, not storage; it may still reach the pagefile or a crash dump.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use tokio::sync::{broadcast, Notify};

use tomi_core::runtime::http::HttpTransport;
use tomi_core::runtime::{
    self, DispatchOutcome, DraftAttachment, DraftText, Engine, EngineConfig, ItemContentView,
    ItemOrigin, OutgoingDraft, RelayTransport, SpaceStatus, Store, SystemClock,
};
use tomi_core::{CoreError as Failure, SourceFormat as CoreFormat, Thumbnail};
use zeroize::Zeroizing;

use crate::core_api::{
    clean_device_name, CoreError, CoreEvent, CoreStatus, DepartureKind, DepartureOutcome,
    DepartureView, PairingOptions, PairingView, PendingReason, ProgressFn, RefusalReason,
    SendOutcome, SendProgress, SpaceState, TomiCore,
};
use crate::files;
use crate::inactivity::{inactive_days, INACTIVE_AFTER_DAYS, LAST_SEEN_IS_FROM_RELAY};
use crate::model::{
    now_ms, Attachment, Delivery, DeliveryState, Device, DeviceRef, DeviceState, Item, ItemDraft,
    ReadPolicy, SourceFormat, TextBody, Transfer, TransferDirection, TransferPhase,
    UnreadableReason,
};
use crate::pairing::{default_device_name, EnginePairing, Hooks, PairingHost, Timing};
use crate::wrapkey;

/// The relay a build may carry by default, fixed at compile time with the environment variable
/// `TOMI_DEFAULT_RELAY_URL` (the installer of the owner sets it in CI). There is none otherwise:
/// the relay then comes from a pairing link, a QR code, a creation link or the "Relais" field
/// (`spec/pairing-v1.md` §4.4, §10), and is shown in full before anything is kept. A debug build
/// may also override it at run time with `TOMI_RELAY_URL` to talk to a local relay.
pub const DEFAULT_RELAY_URL: Option<&str> = option_env!("TOMI_DEFAULT_RELAY_URL");

const DB_FILE: &str = "tomi.db";
const FILES_DIR: &str = "store";
const WRAP_KEY_FILE: &str = "wrap.key";
const OPEN_DIR: &str = "open";

/// Changes arriving together (an import, a sync) become one pass over the feed.
const BRIDGE_DEBOUNCE: Duration = Duration::from_millis(80);
const BRIDGE_MAX_PAUSE: Duration = Duration::from_secs(2);
/// Local housekeeping: items past their explicit expiration are erased from the disk.
const MAINTENANCE_EVERY: Duration = Duration::from_secs(60);
/// Safety net when the doorbell cannot connect (a proxy that blocks WebSockets, say).
const SYNC_EVERY: Duration = Duration::from_secs(300);
/// How often a gesture of the departure is watched for the moment its marker is written (see
/// [`Gestures`]): short, because it is a local read, and it only runs while a gesture does.
const MARKER_POLL: Duration = Duration::from_millis(40);

/// Where the core lives.
#[derive(Debug, Clone)]
pub struct RealCoreConfig {
    /// Private, per-user, not roaming (`%LOCALAPPDATA%\eu.shaffe.tomi`).
    pub data_dir: PathBuf,
    /// The relay of a PC that has none yet; `None` in a release build without a default.
    /// A PC that belongs to a space keeps the relay of its space, whatever this says.
    pub relay_url: Option<String>,
    pub app_version: String,
}

pub struct RealCore {
    /// `None` when the local store cannot be opened (see `unavailable`).
    engine: Option<Arc<Engine>>,
    unavailable: Option<String>,
    /// Decrypted copies of the attachments the user opened, named like the files.
    open_dir: PathBuf,
    tx: broadcast::Sender<CoreEvent>,
    changed: Arc<Notify>,
    /// The feed as the UI last saw it (`None` until the first pass, and again after an erasure).
    snapshot: Mutex<Option<HashMap<String, Item>>>,
    /// Stops the change stream bridge and the housekeeping, for good (the app quits). Never set
    /// by an erasure: a later pairing needs both. The doorbell has its own stop, per loop.
    stop: Arc<AtomicBool>,
    started: AtomicBool,
    /// The doorbell loop. Shared with the pairing hook that starts it once this PC is provisioned
    /// (again, after an erasure).
    doorbell: Arc<Doorbell>,
    /// The departure gestures in flight, for "Annuler" before the marker is written.
    gestures: Arc<Gestures>,
    /// A decrypted copy may be on disk (set by `attachment_path`).
    plaintext_dirty: AtomicBool,
    /// The pairing screens (`spec/pairing-v1.md`); `None` when the store cannot be opened.
    pairing: Option<Arc<PairingHost>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The doorbell loop (`Engine::run_doorbell`) of this PC, with a stop flag **of its own per loop**.
///
/// The old shared `stop` flag could not be used to stop the doorbell alone: setting it also stopped
/// the bridge and the housekeeping (which a later pairing needs), and an erased PC that pairs again
/// would have started a loop that is stopped before it begins. `stop` sets the flag of the running
/// loop and forgets it; the next `spawn` (the pairing hook, once this PC is provisioned again) makes
/// a new flag. A single loop at a time is guaranteed by the core (`doorbell_gate`): a new loop
/// waits for the old one to leave, and leaves at once if its flag is set meanwhile.
struct Doorbell {
    state: Mutex<DoorbellState>,
}

#[derive(Default)]
struct DoorbellState {
    /// The flag of the loop that was started and not stopped (`None`: no loop wanted).
    flag: Option<Arc<AtomicBool>>,
    /// The app quits: no loop starts any more.
    closed: bool,
}

impl Doorbell {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(DoorbellState::default()),
        })
    }

    /// Starts the loop of a provisioned engine, once (a second call while a loop runs does
    /// nothing).
    fn spawn(self: &Arc<Self>, engine: &Arc<Engine>) {
        if !engine.is_provisioned() {
            return;
        }
        let flag = {
            let mut state = lock(&self.state);
            if state.closed || state.flag.is_some() {
                return;
            }
            let flag = Arc::new(AtomicBool::new(false));
            state.flag = Some(flag.clone());
            flag
        };
        let (engine, me, run) = (engine.clone(), self.clone(), flag.clone());
        let spawned = std::thread::Builder::new()
            .name("tomi-doorbell".into())
            .spawn(move || {
                engine.run_doorbell(&run);
                me.ended(&run);
            });
        if spawned.is_err() {
            self.ended(&flag);
        }
    }

    /// A loop left (stopped, or the device is no longer provisioned): a later `spawn` may start
    /// another. A loop that was already replaced does not touch the state of its successor.
    fn ended(&self, flag: &Arc<AtomicBool>) {
        let mut state = lock(&self.state);
        if state.flag.as_ref().is_some_and(|f| Arc::ptr_eq(f, flag)) {
            state.flag = None;
        }
    }

    /// Stops the running loop with its own flag. A later `spawn` starts a new one.
    fn stop(&self) {
        if let Some(flag) = lock(&self.state).flag.take() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// The app quits: stops the loop and refuses to start another.
    fn shutdown(&self) {
        lock(&self.state).closed = true;
        self.stop();
    }

    #[cfg(test)]
    fn running(&self) -> bool {
        lock(&self.state).flag.is_some()
    }
}

/// The departure gestures ("Se déconnecter", "Supprimer l'espace") in flight, and an "Annuler" that
/// came before the engine wrote the marker.
///
/// The engine writes the marker (`requested` / `deleting`) in its first transaction, but only tells
/// the app when the attempt ends (its notification waits for the departure lock). Until the marker
/// exists the engine has nothing to cancel, so a plain `cancel_departure` would answer "nothing"
/// while the gesture goes on to send the request: an "Annuler" that did not work. Here the request
/// is kept (`cancel_asked`) and carried out the moment the marker shows up (the gesture's watcher,
/// [`Gestures::marker_written`]) or, if the gesture has not even called the engine yet, it never
/// does (`take_cancel`). Everything is decided under one lock, so a cancel and a marker that
/// appear together cannot miss each other.
#[derive(Default)]
struct Gestures {
    state: Mutex<GestureState>,
}

#[derive(Default)]
struct GestureState {
    /// Gestures that have been called and have not returned.
    in_flight: usize,
    /// "Annuler" came while a gesture had no marker yet.
    cancel_asked: bool,
    /// The marker has been seen (written) during the current gesture. From then on the marker
    /// can disappear without the gesture being over (the engine removes it at the end of the
    /// erasure, the gesture is still counted until its guard drops): "no marker" no longer means
    /// "not written yet", so `cancel` promises nothing and leaves the engine's answer standing.
    marker_seen: bool,
}

impl Gestures {
    fn begin(&self) {
        let mut state = lock(&self.state);
        state.in_flight += 1;
        // A request of an earlier gesture does not apply to this one.
        state.cancel_asked = false;
        state.marker_seen = false;
    }

    fn end(&self) {
        let mut state = lock(&self.state);
        state.in_flight = state.in_flight.saturating_sub(1);
        if state.in_flight == 0 {
            // A gesture that ended without ever writing a marker (it failed) leaves nothing to
            // cancel: the request does not wait for the next gesture.
            state.cancel_asked = false;
            state.marker_seen = false;
        }
    }

    /// The cancel asked before the engine was even called, spent: the gesture then does nothing.
    fn take_cancel(&self) -> bool {
        std::mem::take(&mut lock(&self.state).cancel_asked)
    }

    /// "Annuler". `true` only if the departure is cancelled now, or a gesture is on its way to
    /// write its marker and will be cancelled the moment it does. The erasure (`wiping`) is never
    /// cancellable: the engine answers `false` and nothing is promised.
    fn cancel(&self, engine: &Engine) -> Result<bool, Failure> {
        let mut state = lock(&self.state);
        if engine.cancel_departure()? {
            state.cancel_asked = false;
            return Ok(true);
        }
        if engine.departure().is_some() {
            // The marker exists and the engine refuses (the erasure): no longer "not written yet".
            state.marker_seen = true;
            return Ok(false);
        }
        // No marker. "Not written yet" only if it has never existed during this gesture: not
        // seen by the watcher, and the device still paired (a marker written and erased within
        // one poll leaves an unpaired device behind it, with the gesture still counted).
        if state.in_flight > 0 && !state.marker_seen && engine.is_provisioned() {
            state.cancel_asked = true;
            return Ok(true);
        }
        Ok(false)
    }

    /// The gesture's watcher: `true` once the marker exists (a request kept by [`Self::cancel`] is
    /// carried out first).
    fn marker_written(&self, engine: &Engine) -> bool {
        let mut state = lock(&self.state);
        if engine.departure().is_none() {
            return false;
        }
        state.marker_seen = true;
        if std::mem::take(&mut state.cancel_asked) {
            let _ = engine.cancel_departure();
        }
        true
    }
}

/// A gesture in flight. On drop: the watcher stops and the gesture is no longer counted.
struct GestureGuard {
    gestures: Arc<Gestures>,
    done: Arc<AtomicBool>,
}

impl GestureGuard {
    fn start(gestures: &Arc<Gestures>) -> Self {
        gestures.begin();
        Self {
            gestures: gestures.clone(),
            done: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for GestureGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        self.gestures.end();
    }
}

impl RealCore {
    /// Opens the local store. Never fails: when it cannot be opened (wrong Windows
    /// account, key file gone) the core reports it in [`TomiCore::status`] and every call
    /// answers with a storage error, so the app can explain instead of crashing. Nothing is
    /// ever deleted or overwritten in that case.
    pub fn open(cfg: &RealCoreConfig) -> Arc<Self> {
        match open_engine(cfg) {
            Ok((engine, relay)) => {
                Self::build(Some(Arc::new(engine)), Some(relay), None, &cfg.data_dir)
            }
            Err(code) => {
                // The code only: no path, no content.
                eprintln!("tomi: the local store cannot be opened ({code})");
                Self::build(None, None, Some(code), &cfg.data_dir)
            }
        }
    }

    /// Wraps an engine built by the caller (tests). Without the transport the engine talks
    /// through, the pairing cannot ask the relay whether a space exists.
    #[cfg(test)]
    pub fn with_engine(engine: Arc<Engine>, data_dir: &Path) -> Arc<Self> {
        Self::build(Some(engine), None, None, data_dir)
    }

    /// [`with_engine`](Self::with_engine), with the transport the engine talks through.
    #[cfg(test)]
    pub fn with_engine_and_relay(
        engine: Arc<Engine>,
        relay: Arc<dyn RelayTransport>,
        data_dir: &Path,
    ) -> Arc<Self> {
        Self::build(Some(engine), Some(relay), None, data_dir)
    }

    fn build(
        engine: Option<Arc<Engine>>,
        relay: Option<Arc<dyn RelayTransport>>,
        unavailable: Option<String>,
        data_dir: &Path,
    ) -> Arc<Self> {
        // Decrypted copies left by a crash or a kill do not outlive a restart (the core
        // does the same with its own cache when it opens).
        let open_dir = data_dir.join(OPEN_DIR);
        let _ = wipe_dir(&open_dir);
        let (tx, _) = broadcast::channel(256);
        let changed = Arc::new(Notify::new());
        if let Some(e) = &engine {
            let n = changed.clone();
            e.subscribe(Arc::new(move || n.notify_one()));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let doorbell = Doorbell::new();
        let pairing = engine.as_ref().map(|e| {
            let on_change = tx.clone();
            let (engine, doorbell, wake) = (e.clone(), doorbell.clone(), changed.clone());
            PairingHost::new(
                Arc::new(EnginePairing {
                    engine: e.clone(),
                    relay,
                }),
                Hooks {
                    changed: Arc::new(move || {
                        let _ = on_change.send(CoreEvent::PairingChanged);
                    }),
                    // A PC that just joined or created the space listens like any other
                    // (including one that was erased and pairs again: the doorbell has a new
                    // stop flag of its own), and the feed is read again.
                    provisioned: Arc::new(move || {
                        doorbell.spawn(&engine);
                        wake.notify_one();
                    }),
                },
                Timing::default(),
            )
        });
        Arc::new(Self {
            engine,
            unavailable,
            open_dir,
            tx,
            changed,
            snapshot: Mutex::new(None),
            stop,
            started: AtomicBool::new(false),
            doorbell,
            gestures: Arc::new(Gestures::default()),
            plaintext_dirty: AtomicBool::new(false),
            pairing,
        })
    }

    /// Starts the background work: the change stream, the housekeeping and the doorbell once
    /// the device is paired. Call once, from a Tauri async context.
    ///
    /// The settling of an interrupted pairing is NOT started here: `spec/api-v1.md` 7.9.4 wants
    /// the pending departure settled first, so the app calls [`TomiCore::resume_pairing`] once
    /// `LeaveFlow::resume_at_startup` is done.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) || self.engine.is_none() {
            return;
        }
        let core = self.clone();
        tauri::async_runtime::spawn(async move { core.bridge().await });
        let core = self.clone();
        tauri::async_runtime::spawn(async move { core.housekeeping().await });
        self.start_doorbell();
    }

    fn host(&self) -> Result<Arc<PairingHost>, CoreError> {
        self.pairing.clone().ok_or_else(|| {
            CoreError::Storage(
                self.unavailable
                    .clone()
                    .unwrap_or_else(|| "unavailable".into()),
            )
        })
    }

    fn engine_or_err(&self) -> Result<Arc<Engine>, CoreError> {
        self.engine.clone().ok_or_else(|| {
            CoreError::Storage(
                self.unavailable
                    .clone()
                    .unwrap_or_else(|| "unavailable".into()),
            )
        })
    }

    fn paired_engine(&self) -> Result<Arc<Engine>, CoreError> {
        let engine = self.engine_or_err()?;
        if engine.is_provisioned() {
            Ok(engine)
        } else {
            Err(CoreError::NotPaired)
        }
    }

    /// Runs a blocking engine call off the async threads and maps its error.
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Engine) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, CoreError> {
        let engine = self.paired_engine()?;
        tokio::task::spawn_blocking(move || f(&engine))
            .await
            .map_err(|_| CoreError::Other("task".into()))?
            .map_err(map_failure)
    }

    /// [`call`](Self::call) for the departure calls that must also run on a device that is NOT
    /// paired (`continue_departure` only removes a leftover marker there; "Annuler"; the reads).
    /// Built on `engine_or_err`, never on `paired_engine`. Every error goes through
    /// `map_failure` (`LeaveFlow::confirm_erase` counts on `Rejected` for "the verdict is too old,
    /// nothing was erased", and on `Storage` for "failed, try again").
    async fn departure_call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Engine) -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, CoreError> {
        let engine = self.engine_or_err()?;
        tokio::task::spawn_blocking(move || f(&engine))
            .await
            .map_err(|_| CoreError::Other("task".into()))?
            .map_err(map_failure)
    }

    /// A gesture of the departure ("Se déconnecter", "Supprimer l'espace"): the engine writes
    /// the marker, then makes one attempt. Paired device only. While it runs, the marker is
    /// watched for (the engine tells the app only when the attempt ends): the windows hear of
    /// it at once, and an "Annuler" pressed before it existed is carried out then
    /// ([`Gestures`]). The outcome is read, never decided here.
    async fn gesture(
        &self,
        run: impl FnOnce(&Engine) -> Result<runtime::DepartureOutcome, Failure> + Send + 'static,
    ) -> Result<DepartureOutcome, CoreError> {
        let engine = self.paired_engine()?;
        let guard = GestureGuard::start(&self.gestures);
        {
            let (gestures, tx, done) = (self.gestures.clone(), self.tx.clone(), guard.done.clone());
            tokio::task::spawn_blocking(move || {
                while !done.load(Ordering::SeqCst) {
                    if gestures.marker_written(&engine) {
                        let _ = tx.send(CoreEvent::DepartureChanged);
                        return;
                    }
                    std::thread::sleep(MARKER_POLL);
                }
            });
        }
        let gestures = self.gestures.clone();
        let result = self
            .call(move |engine| {
                // "Annuler" came before the engine was called: nothing is written, nothing sent.
                if gestures.take_cancel() {
                    return Ok(runtime::DepartureOutcome::Cancelled);
                }
                run(engine)
            })
            .await;
        drop(guard);
        Ok(self.settled(map_outcome(result?)))
    }

    /// Reads the outcome of an attempt: after an `Erased`, lets go of what the host holds.
    fn settled(&self, outcome: DepartureOutcome) -> DepartureOutcome {
        if outcome == DepartureOutcome::Erased {
            self.after_erasure();
        }
        outcome
    }

    /// The engine erased this PC (a proof of the relay, or "Effacer quand même"): the host
    /// forgets what it holds itself.
    ///
    /// * NOT `self.stop`: that would stop the change stream bridge and the housekeeping, which a
    ///   later pairing needs.
    /// * The doorbell loop is stopped with its own flag and may start again (the pairing hook,
    ///   once this PC is provisioned again).
    /// * The decrypted feed is dropped, so that no decrypted item survives in memory.
    /// * The decrypted copies of files opened in other programs go (the engine only wipes its own
    ///   cache); a copy that is still held open stays on the list for the next purge.
    /// * A pairing invitation that was on screen is cancelled: its code (a secret) must not stay in
    ///   memory, nor stay valid at the relay, for a PC that no longer belongs to the space.
    fn after_erasure(&self) {
        self.doorbell.stop();
        self.pairing_cancel();
        *lock(&self.snapshot) = None;
        if wipe_dir(&self.open_dir) > 0 {
            self.plaintext_dirty.store(true, Ordering::SeqCst);
        }
        let _ = self.tx.send(CoreEvent::Changed);
    }

    // ------------------------------------------------------------ background

    /// The WebSocket doorbell on its own thread: it connects, catches up, syncs on every
    /// wake and reconnects with jitter (`Engine::run_doorbell`).
    fn start_doorbell(&self) {
        if let Some(engine) = &self.engine {
            self.doorbell.spawn(engine);
        }
    }

    /// Turns the engine's "something changed" into item events.
    async fn bridge(self: Arc<Self>) {
        // The first pass records the feed without announcing it: what was already there
        // at launch is not news.
        self.emit_changes().await;
        let mut pause = BRIDGE_DEBOUNCE;
        loop {
            self.changed.notified().await;
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(pause).await;
            let started = Instant::now();
            self.emit_changes().await;
            // A pass reads and decrypts the whole feed: during a storm of changes (a
            // history import) the pause grows with the cost of a pass, so that the
            // bridge never takes more than about a third of a core.
            pause = (started.elapsed() * 2).clamp(BRIDGE_DEBOUNCE, BRIDGE_MAX_PAUSE);
        }
    }

    async fn emit_changes(&self) {
        let items = match self.list_items().await {
            Ok(items) => items,
            Err(CoreError::NotPaired) => {
                // Erased (or not paired yet): no decrypted item may stay in memory, whichever pass
                // of the bridge was still running when the erasure happened.
                *lock(&self.snapshot) = None;
                return;
            }
            Err(_) => return,
        };
        for event in self.diff(items) {
            if let CoreEvent::ItemRemoved(id) = &event {
                // A deletion elsewhere, an expiry, a consumed item: no decrypted copy of
                // it may stay on this disk.
                self.remove_open_copies(id);
            }
            let _ = self.tx.send(event);
        }
        let _ = self.tx.send(CoreEvent::Changed);
    }

    /// What changed between the last feed the UI saw and `items`.
    fn diff(&self, items: Vec<Item>) -> Vec<CoreEvent> {
        let now: HashMap<String, Item> = items.into_iter().map(|i| (i.id.clone(), i)).collect();
        let mut snapshot = lock(&self.snapshot);
        let mut events = Vec::new();
        if let Some(before) = snapshot.as_ref() {
            for (id, item) in &now {
                match before.get(id) {
                    None => events.push(CoreEvent::ItemAdded(item.clone())),
                    Some(old) if old != item => events.push(CoreEvent::ItemUpdated(item.clone())),
                    Some(_) => {}
                }
            }
            for id in before.keys().filter(|id| !now.contains_key(*id)) {
                events.push(CoreEvent::ItemRemoved(id.clone()));
            }
        }
        *snapshot = Some(now);
        events
    }

    /// Erases what has passed its expiration from the disk, and synchronises now and then
    /// in case the doorbell is down.
    async fn housekeeping(self: Arc<Self>) {
        let mut tick = tokio::time::interval(MAINTENANCE_EVERY);
        let mut count = 0u32;
        let rounds_per_sync = (SYNC_EVERY.as_secs() / MAINTENANCE_EVERY.as_secs()).max(1) as u32;
        loop {
            tick.tick().await;
            if self.stop.load(Ordering::SeqCst) {
                return;
            }
            count = count.wrapping_add(1);
            // Both fail harmlessly when unpaired, offline or blocked.
            let _ = self.call(|e| e.maintenance()).await;
            // Local expiry erases items without the engine notifying: wake the bridge so the
            // UI drops the expired cards (and their clipboard clears fire) without waiting for
            // the next unrelated change.
            self.changed.notify_one();
            if count.is_multiple_of(rounds_per_sync) {
                let _ = self.call(|e| e.sync().map(|_| ())).await;
            }
        }
    }

    /// A decision taken here (delete everywhere, read, pin...) leaves through the core's outbox, and only a
    /// sync drains it. Without this call it would wait for the next wake of the doorbell or the 5-minute
    /// safety sync, and the other devices would not hear of it. Fire and forget: offline, the outbox keeps
    /// it and the next sync sends it.
    fn flush_outbox(&self) {
        if let Some(engine) = self.engine.clone() {
            tokio::task::spawn_blocking(move || {
                let _ = engine.sync();
            });
        }
    }

    // ------------------------------------------------------------ decrypted copies

    fn remove_open_copies(&self, item_id: &str) {
        if !is_plain_id(item_id) {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&self.open_dir) {
            let prefix = format!("{item_id}_");
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}

/// An item or transfer id as the core makes them: UUID characters only. Anything else
/// never becomes part of a path.
fn is_plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Stores the draft, then tries to push it. `true` when it was left in the outbox.
fn push(engine: &Engine, draft: OutgoingDraft, id: &str) -> Result<bool, CoreError> {
    // Stored locally first (the files are encrypted before this returns), so a failure
    // after this point never loses the item.
    engine.enqueue(draft).map_err(map_failure)?;
    match engine.dispatch(id) {
        Ok(DispatchOutcome::Sent) => Ok(false),
        // Offline or the relay is busy: the outbox keeps it and sends it with the next
        // synchronisation.
        Ok(DispatchOutcome::RetryLater) => Ok(true),
        Ok(DispatchOutcome::NotFound) => Err(CoreError::NotFound(id.to_string())),
        Ok(DispatchOutcome::Failed) => {
            // Refused for good: no junk item that can never go out.
            let _ = engine.delete_local(id);
            Err(CoreError::Rejected("send_failed".into()))
        }
        Err(e) => {
            // A call that failed leaves nothing behind (blocked, storage).
            let _ = engine.cancel_if_queued(id);
            Err(map_failure(e))
        }
    }
}

/// Opens the engine over the DPAPI-wrapped database, with the transport it talks through (the
/// pairing asks the relay whether a space exists, and binds the code to the relay authority).
/// The error is a machine code.
fn open_engine(cfg: &RealCoreConfig) -> Result<(Engine, Arc<dyn RelayTransport>), String> {
    std::fs::create_dir_all(&cfg.data_dir).map_err(|_| "storage".to_string())?;
    let db_path = cfg.data_dir.join(DB_FILE);
    // A database without its key must not get a new key: that would lock it for good. On an
    // access error we cannot tell, so we assume the database is there and refuse to create a
    // key (the safe side: never risk orphaning existing data).
    let db_absent = matches!(db_path.try_exists(), Ok(false));
    let wrap = wrapkey::load_or_create(&cfg.data_dir.join(WRAP_KEY_FILE), db_absent)
        .map_err(|e| e.code().to_string())?;
    let store = Store::open(&db_path, &cfg.data_dir.join(FILES_DIR), wrap.as_slice())
        .map_err(|e| e.code().to_string())?;
    // A default that is not a valid relay (a mistyped build variable) must not lock the app: the
    // PC then has no relay, like a release build, and asks for one.
    let relay_url = cfg
        .relay_url
        .as_deref()
        .filter(|u| HttpTransport::with_relay(Some(u)).is_ok());
    let transport: Arc<dyn RelayTransport> =
        Arc::new(HttpTransport::with_relay(relay_url).map_err(|e| e.code().to_string())?);
    let engine = Engine::open(
        EngineConfig {
            app_version: cfg.app_version.clone(),
            platform: "windows".into(),
            // The history of another device waits for a yes or a no (decision D2). The one
            // announced at pairing is accepted by the core without asking: the user already
            // answered.
            auto_import: false,
            default_relay_url: relay_url.map(str::to_owned),
        },
        store,
        transport.clone(),
        Arc::new(SystemClock),
    )
    .map_err(|e| e.code().to_string())?;
    Ok((engine, transport))
}

/// Removes everything inside `dir` (and recreates it). Returns how many entries could not
/// be removed (a file still open in another program).
fn wipe_dir(dir: &Path) -> usize {
    let mut left = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let removed = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if removed.is_err() {
                left += 1;
            }
        }
    }
    let _ = std::fs::create_dir_all(dir);
    left
}

// ---------------------------------------------------------------- mapping

pub(crate) fn map_failure(e: Failure) -> CoreError {
    match e {
        Failure::Storage { reason } => CoreError::Storage(reason),
        Failure::Network { .. } => CoreError::Offline,
        // The relay is busy or down: same answer as no network, the outbox retries.
        Failure::Relay { status, .. } if status == 429 || status >= 500 => CoreError::Offline,
        Failure::Relay { code, .. } => CoreError::Rejected(code),
        Failure::Blocked { reason } => CoreError::Blocked(reason),
        Failure::InvalidInput { reason } if reason.contains("not provisioned") => {
            CoreError::NotPaired
        }
        Failure::InvalidInput { reason } => CoreError::Rejected(reason),
        // A pin time beyond 2^53 - 1 ms is refused to sign: the clock is wrong.
        e @ Failure::MalformedStatement { .. } => CoreError::Rejected(e.code().to_string()),
        other => CoreError::Other(other.code().to_string()),
    }
}

/// A device id as the relay types it: 32 lowercase hexadecimal characters
/// (`spec/api-v1.md`). It is put in the path of a request, so nothing else gets through.
fn is_device_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// What a refused rename means for the user: the relay checks the name too (`422`), and a
/// device that is not a member answers `404`.
fn rename_refusal(e: CoreError) -> CoreError {
    match e {
        CoreError::Rejected(code) if code == "validation_failed" => {
            CoreError::Rejected("name_invalid".into())
        }
        CoreError::Rejected(code) if code == "not_found" => {
            CoreError::Rejected("unknown_device".into())
        }
        other => other,
    }
}

/// What a refused revocation means for the user. The core answers `unknown device <id>` when the
/// device is not (or no longer) a member: the id is dropped from the code, it has no use for the
/// window. `device_set_mismatch` and `epoch_conflict` come back when the devices or the key moved
/// twice in a row while the key was renewed.
fn revoke_refusal(e: CoreError) -> CoreError {
    match e {
        CoreError::Rejected(reason) if reason.starts_with("unknown device") => {
            CoreError::Rejected("unknown_device".into())
        }
        CoreError::Rejected(code) if code == "not_found" => {
            CoreError::Rejected("unknown_device".into())
        }
        other => other,
    }
}
fn map_policy(p: tomi_core::ReadPolicy) -> ReadPolicy {
    match p {
        tomi_core::ReadPolicy::UntilExpiry => ReadPolicy::Normal,
        tomi_core::ReadPolicy::AfterFirstRead => ReadPolicy::AfterFirstRead,
    }
}

fn map_format(f: CoreFormat) -> SourceFormat {
    match f {
        CoreFormat::Plain => SourceFormat::Plain,
        CoreFormat::Markdown => SourceFormat::Markdown,
        CoreFormat::Html => SourceFormat::Html,
    }
}

fn unmap_format(f: Option<SourceFormat>) -> CoreFormat {
    match f {
        Some(SourceFormat::Markdown) => CoreFormat::Markdown,
        Some(SourceFormat::Html) => CoreFormat::Html,
        _ => CoreFormat::Plain,
    }
}

fn map_delivery(s: runtime::DeliveryState) -> DeliveryState {
    match s {
        runtime::DeliveryState::Queued => DeliveryState::Queued,
        runtime::DeliveryState::Sent => DeliveryState::Sent,
        runtime::DeliveryState::Delivered => DeliveryState::Delivered,
        runtime::DeliveryState::ExpiredBeforeDelivery => DeliveryState::Expired,
        runtime::DeliveryState::Failed => DeliveryState::Failed,
    }
}

/// Raster formats a thumbnail may have. Anything else is dropped rather than put in an
/// `<img>` of the webview.
fn thumbnail_url(t: &Thumbnail) -> Option<String> {
    matches!(
        t.mime.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
    )
    .then(|| {
        format!(
            "data:{};base64,{}",
            t.mime,
            base64::engine::general_purpose::STANDARD.encode(&t.data)
        )
    })
}

/// The reverse of [`thumbnail_url`], for the `data:image/png;base64,...` URLs the app
/// itself builds from the files it sends.
fn thumbnail_from_url(url: &str) -> Option<Thumbnail> {
    let rest = url.strip_prefix("data:")?;
    let (mime, data) = rest.split_once(";base64,")?;
    if !mime.starts_with("image/") {
        return None;
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Thumbnail {
        mime: mime.to_string(),
        data,
    })
}

fn to_item(v: runtime::ItemView) -> Item {
    let outgoing = v.origin == ItemOrigin::Sent;
    let imported = v.origin == ItemOrigin::Imported;
    let unreadable_reason = match &v.content {
        ItemContentView::Readable { .. } => None,
        ItemContentView::Unreadable { reason } => Some(map_unreadable(reason)),
    };
    let (title, text, url, attachments, secret, unreadable, source_format) = match v.content {
        ItemContentView::Readable {
            title,
            text,
            url,
            attachments,
            secret,
        } => {
            let source_format = text.as_ref().map(|t| map_format(t.source_format));
            (
                title,
                text.map(|t| TextBody {
                    plain: t.plain,
                    html: t.html,
                    markdown: t.markdown,
                }),
                url,
                attachments
                    .into_iter()
                    .map(|a| Attachment {
                        slot: Some(a.slot),
                        thumbnail: a.thumbnail.as_ref().and_then(thumbnail_url),
                        name: a.name,
                        mime: a.mime,
                        size: a.size,
                        path: None,
                        available: a.available,
                    })
                    .collect(),
                secret,
                false,
                source_format,
            )
        }
        // Kept, never deleted for that (invariant 7).
        ItemContentView::Unreadable { .. } => (None, None, None, Vec::new(), false, true, None),
    };
    Item {
        id: v.id,
        title,
        text,
        url,
        attachments,
        secret,
        source_device: DeviceRef {
            id: v.source_device_id,
            name: v.source_device_name,
        },
        created_at: v.created_at_ms,
        expires_at: v.expires_at_ms,
        read_policy: map_policy(v.read_policy),
        read: v.read,
        outgoing,
        imported,
        delivery: v
            .deliveries
            .into_iter()
            .map(|d| Delivery {
                device: DeviceRef {
                    id: d.device_id,
                    name: d.device_name,
                },
                state: map_delivery(d.state),
            })
            .collect(),
        source_format,
        unreadable,
        unreadable_reason,
        pinned: v.pinned,
        pinned_at: v.pinned_at_ms,
        archived: v.archived,
        archived_at: v.archived_at_ms,
    }
}

fn to_device(d: runtime::DeviceView) -> Device {
    use tomi_core::runtime::DeviceState as Core;
    Device {
        id: d.id,
        name: d.name,
        platform: d.platform,
        last_seen: d.last_seen_at_ms,
        this_device: d.is_this_device,
        state: match d.state {
            Core::Verified => DeviceState::Verified,
            Core::Unverified => DeviceState::Unverified,
            Core::Revoked | Core::RevokedByRelay => DeviceState::Revoked,
        },
        // No hint while the core's date is not the relay's `last_seen_at` (see `inactivity.rs`).
        inactive_days: if LAST_SEEN_IS_FROM_RELAY {
            inactive_days(
                d.last_seen_at_ms,
                d.is_this_device,
                now_ms(),
                INACTIVE_AFTER_DAYS,
            )
        } else {
            None
        },
    }
}

fn to_outgoing(draft: ItemDraft, id: String) -> OutgoingDraft {
    OutgoingDraft {
        id: Some(id),
        title: draft.title,
        text: draft.text.map(|t| DraftText {
            plain: t.plain,
            html: t.html,
            markdown: t.markdown,
            source_format: unmap_format(draft.source_format),
        }),
        url: draft.url,
        attachments: draft
            .attachments
            .into_iter()
            .map(|a| DraftAttachment {
                path: a.path.unwrap_or_default(),
                thumbnail: a.thumbnail.as_deref().and_then(thumbnail_from_url),
                name: a.name,
                mime: a.mime,
            })
            .collect(),
        secret: draft.secret,
        expires_at_ms: draft.expires_at,
        read_policy: match draft.read_policy {
            ReadPolicy::Normal => tomi_core::ReadPolicy::UntilExpiry,
            ReadPolicy::AfterFirstRead => tomi_core::ReadPolicy::AfterFirstRead,
        },
    }
}

fn to_transfer(t: runtime::TransferView) -> Transfer {
    let phase = match (t.role.as_str(), t.state.as_str()) {
        ("import", "received") => TransferPhase::Offered,
        (_, "done") => TransferPhase::Done,
        (_, "failed") => TransferPhase::Failed,
        (_, "cancelled") => TransferPhase::Cancelled,
        (_, "declined") => TransferPhase::Declined,
        _ => TransferPhase::Running,
    };
    Transfer {
        id: t.transfer_id,
        direction: if t.role == "export" {
            TransferDirection::Send
        } else {
            TransferDirection::Receive
        },
        peer: DeviceRef {
            id: t.peer_device_id,
            name: t.peer_name,
        },
        phase,
        items_total: t.items_total,
        items_done: t.items_done,
        files_total: t.files_total,
        files_done: t.files_done,
        error: t.error,
    }
}

fn map_space(s: SpaceStatus) -> SpaceState {
    match s {
        SpaceStatus::Active => SpaceState::Active,
        SpaceStatus::Revoked => SpaceState::Revoked,
        SpaceStatus::UpdateRequired => SpaceState::UpdateRequired,
        SpaceStatus::NotFound => SpaceState::NotFound,
        SpaceStatus::Blocked(_) => SpaceState::Blocked,
    }
}

// The types of the departure, one to one (no wildcard: a variant the engine adds must break the
// build here, not be guessed). Nothing is interpreted: what the relay said was already read by the
// engine, which erases only on a positive proof.

pub(crate) fn map_kind(k: runtime::DepartureKind) -> DepartureKind {
    match k {
        runtime::DepartureKind::Leave => DepartureKind::Leave,
        runtime::DepartureKind::DeleteSpace => DepartureKind::DeleteSpace,
    }
}

pub(crate) fn map_pending(r: runtime::PendingReason) -> PendingReason {
    match r {
        runtime::PendingReason::Network => PendingReason::Network,
        runtime::PendingReason::Server => PendingReason::Server,
        runtime::PendingReason::NotFound => PendingReason::NotFound,
        runtime::PendingReason::NotSent => PendingReason::NotSent,
    }
}

pub(crate) fn map_refusal(r: runtime::RefusalReason) -> RefusalReason {
    match r {
        runtime::RefusalReason::LastDevice => RefusalReason::LastDevice,
        runtime::RefusalReason::UpdateRequired => RefusalReason::UpdateRequired,
        runtime::RefusalReason::RelayTooOld => RefusalReason::RelayTooOld,
        runtime::RefusalReason::Unexpected => RefusalReason::Unexpected,
    }
}

pub(crate) fn map_outcome(o: runtime::DepartureOutcome) -> DepartureOutcome {
    match o {
        runtime::DepartureOutcome::Erased => DepartureOutcome::Erased,
        runtime::DepartureOutcome::Pending(reason) => DepartureOutcome::Pending {
            reason: map_pending(reason),
        },
        runtime::DepartureOutcome::Refused(reason) => DepartureOutcome::Refused {
            reason: map_refusal(reason),
        },
        runtime::DepartureOutcome::Cancelled => DepartureOutcome::Cancelled,
        runtime::DepartureOutcome::NothingPending => DepartureOutcome::NothingPending,
    }
}

pub(crate) fn map_view(v: runtime::DepartureView) -> DepartureView {
    DepartureView {
        kind: map_kind(v.kind),
        wiping: v.wiping,
        since_ms: v.since_ms,
        last: v.last.map(map_pending),
    }
}

/// The two sentences of the app: an update of Tomi will read the item, or it is unreadable for now.
fn map_unreadable(r: &tomi_core::UnreadableReason) -> UnreadableReason {
    match r {
        tomi_core::UnreadableReason::UpdateRequired => UnreadableReason::UpdateRequired,
        // A key that has not arrived yet, one that a rotation replaced, a failed decryption: the
        // item is kept and a synchronisation or an update may make it readable.
        tomi_core::UnreadableReason::KeyPending
        | tomi_core::UnreadableReason::SuspectEpoch
        | tomi_core::UnreadableReason::Undecryptable => UnreadableReason::Other,
    }
}

/// Copy of a decrypted attachment named like the file, so that "Ouvrir", the clipboard
/// and the drag-out hand over the right name and extension. Lives under `open_dir`, which
/// is wiped with the core's own cache.
fn named_copy(
    open_dir: &Path,
    item_id: &str,
    index: u32,
    name: &str,
    src: &Path,
) -> std::io::Result<PathBuf> {
    if !is_plain_id(item_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unexpected item id",
        ));
    }
    let dir = open_dir.join(format!("{item_id}_{index}"));
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(files::sanitize_file_name(name));
    let same_size = match (std::fs::metadata(&dest), std::fs::metadata(src)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if !same_size {
        // A stale entry (other size) goes first: a hard link never overwrites.
        match std::fs::remove_file(&dest) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // Read-only BEFORE the link (the two names share the attribute): the core hands back its
        // cache file without checking it, so an editor that saves in place (VS Code, Notepad)
        // must not be able to change it behind the core's back. It is refused and offers
        // "Enregistrer sous" instead.
        files::set_readonly(src, true)?;
        link_or_copy(src, &dest, |from, to| std::fs::hard_link(from, to))?;
    }
    Ok(dest)
}

/// The link itself is impossible here (another volume, a file system without hard links, no
/// right to link), as opposed to a failure that a copy would only hide (`AlreadyExists`, a
/// missing source, a full disk).
fn link_unavailable(e: &std::io::Error) -> bool {
    use std::io::ErrorKind as K;
    matches!(
        e.kind(),
        K::Unsupported | K::CrossesDevices | K::PermissionDenied
    ) || matches!(
        // ERROR_INVALID_FUNCTION (FAT/exFAT), ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED,
        // ERROR_TOO_MANY_LINKS.
        e.raw_os_error(),
        Some(1 | 17 | 50 | 1142)
    )
}

/// Gives `dest` the content of `src` without writing a second clear copy of it (rule 3): a hard
/// link to the core's cache file (same volume, under `%LOCALAPPDATA%`), so that the bytes are on
/// disk once, under two names. Both names are wiped (the cache by the core, `open\` by
/// `wipe_dir`): removing a link never removes the other. Only if the link is impossible
/// ([`link_unavailable`]) does it fall back to a real copy, which never overwrites. An
/// `AlreadyExists` (something took the name since the caller cleared it) is removed once and
/// retried, then comes out as an error instead of being overwritten.
fn link_or_copy(
    src: &Path,
    dest: &Path,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut result = link(src, dest);
    if matches!(&result, Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists) {
        std::fs::remove_file(dest)?;
        result = link(src, dest);
    }
    match result {
        Ok(()) => Ok(()),
        Err(e) if link_unavailable(&e) => copy_new(src, dest),
        Err(e) => Err(e),
    }
}

/// A real copy that refuses to replace anything (`create_new`).
fn copy_new(src: &Path, dest: &Path) -> std::io::Result<()> {
    let mut from = std::fs::File::open(src)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    if let Err(e) = std::io::copy(&mut from, &mut to) {
        drop(to);
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    Ok(())
}

// ---------------------------------------------------------------- the trait

#[async_trait]
impl TomiCore for RealCore {
    fn status(&self) -> CoreStatus {
        match &self.engine {
            Some(e) => CoreStatus {
                paired: e.is_provisioned(),
                space: map_space(e.space_status()),
                unavailable: None,
            },
            None => CoreStatus {
                paired: false,
                space: SpaceState::Active,
                unavailable: self.unavailable.clone(),
            },
        }
    }

    fn this_device(&self) -> Device {
        self.engine
            .as_ref()
            .and_then(|e| e.local_device().ok())
            .map(to_device)
            .unwrap_or_else(|| Device {
                id: String::new(),
                name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Ce PC".to_string()),
                platform: "windows".into(),
                last_seen: None,
                this_device: true,
                state: DeviceState::Verified,
                inactive_days: None,
            })
    }

    async fn send_item(
        &self,
        draft: ItemDraft,
        progress: ProgressFn,
    ) -> Result<SendOutcome, CoreError> {
        let engine = self.paired_engine()?;
        match engine.space_status() {
            SpaceStatus::Active => {}
            SpaceStatus::Revoked => return Err(CoreError::Blocked("revoked".into())),
            SpaceStatus::UpdateRequired => {
                return Err(CoreError::Blocked("update_required".into()))
            }
            SpaceStatus::NotFound => return Err(CoreError::Blocked("not_found".into())),
            SpaceStatus::Blocked(why) => return Err(CoreError::Blocked(why)),
        }
        let item_id = draft
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let total = draft.total_attachment_bytes();
        let outgoing = to_outgoing(draft, item_id.clone());
        // The core reports no byte count while it encrypts and uploads: the bar goes
        // from nothing to everything.
        let report = move |sent: u64| {
            if total > 0 {
                progress(SendProgress {
                    sent_bytes: sent,
                    total_bytes: total,
                });
            }
        };

        report(0);
        let id = item_id.clone();
        let queued = tokio::task::spawn_blocking(move || push(&engine, outgoing, &id))
            .await
            .map_err(|_| CoreError::Other("task".into()))??;
        report(total);
        Ok(SendOutcome { item_id, queued })
    }

    async fn list_items(&self) -> Result<Vec<Item>, CoreError> {
        let views = self.call(|e| e.list_items()).await?;
        Ok(views.into_iter().map(to_item).collect())
    }

    async fn get_item(&self, id: &str) -> Result<Item, CoreError> {
        let wanted = id.to_string();
        match self.call(move |e| e.get_item(&wanted)).await? {
            Some(v) => Ok(to_item(v)),
            None => Err(CoreError::NotFound(id.to_string())),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.tx.subscribe()
    }

    async fn delete_local(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.delete_local(&wanted)).await?;
        self.remove_open_copies(id);
        self.flush_outbox();
        Ok(())
    }

    async fn delete_everywhere(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.delete_everywhere(&wanted)).await?;
        self.remove_open_copies(id);
        self.flush_outbox();
        Ok(())
    }

    async fn mark_read(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.mark_read(&wanted)).await?;
        self.flush_outbox();
        Ok(())
    }

    async fn pin(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.pin(&wanted)).await?;
        self.flush_outbox();
        Ok(())
    }

    async fn unpin(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.unpin(&wanted)).await?;
        self.flush_outbox();
        Ok(())
    }

    async fn archive(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.archive(&wanted)).await?;
        self.flush_outbox();
        Ok(())
    }

    async fn unarchive(&self, id: &str) -> Result<(), CoreError> {
        let wanted = id.to_string();
        self.call(move |e| e.unarchive(&wanted)).await?;
        self.flush_outbox();
        Ok(())
    }

    async fn rename_device(&self, id: &str, name: &str) -> Result<(), CoreError> {
        let name =
            clean_device_name(name).ok_or_else(|| CoreError::Rejected("name_invalid".into()))?;
        // The id goes into the path of the request: only the shape of a device id gets through.
        if !is_device_id(id) {
            return Err(CoreError::Rejected("unknown_device".into()));
        }
        let wanted = id.to_string();
        // The rename goes straight to the relay and is then written in the local database,
        // which tells the windows (`Changed`): it does not pass through the outbox, so there
        // is nothing to flush.
        self.call(move |e| e.rename_device(&wanted, &name))
            .await
            .map_err(rename_refusal)
    }

    async fn revoke_device(&self, id: &str) -> Result<(), CoreError> {
        let engine = self.paired_engine()?;
        // Never this PC: the core would renew the key without it and cut it off from its own
        // space. Checked on the engine's own idea of "this device" before anything else; when
        // that cannot be read, nothing is revoked.
        let this = engine.local_device().map_err(map_failure)?;
        if this.id.eq_ignore_ascii_case(id) {
            return Err(CoreError::Rejected("revoke_self".into()));
        }
        // The id goes into the path of the request: only the shape of a device id gets through.
        if !is_device_id(id) {
            return Err(CoreError::Rejected("unknown_device".into()));
        }
        let wanted = id.to_string();
        // Synchronises, then posts the rotation that renews the space key without the device
        // (retried once by the core when the key or the device list moved). All of it is
        // network calls made now: the outbox is not involved, and the synchronisation the core
        // starts with has already sent whatever it held.
        tokio::task::spawn_blocking(move || engine.revoke_device(&wanted))
            .await
            .map_err(|_| CoreError::Other("task".into()))?
            .map_err(map_failure)
            .map_err(revoke_refusal)
    }

    async fn devices(&self) -> Result<Vec<Device>, CoreError> {
        let views = self.call(|e| e.list_devices()).await?;
        let mut devices: Vec<Device> = views
            .into_iter()
            .map(to_device)
            // A revoked device is gone from the space: it is not offered anything.
            .filter(|d| d.state != DeviceState::Revoked)
            .collect();
        devices.sort_by(|a, b| {
            b.this_device
                .cmp(&a.this_device)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(devices)
    }

    async fn attachment_path(&self, id: &str, slot: usize) -> Result<PathBuf, CoreError> {
        let index = u32::try_from(slot).map_err(|_| CoreError::NotFound(id.to_string()))?;
        let wanted = id.to_string();
        let open_dir = self.open_dir.clone();
        // From here a decrypted copy may exist: the janitor has something to do.
        self.plaintext_dirty.store(true, Ordering::SeqCst);
        let path = self
            .call(move |e| {
                let Some(view) = e.get_item(&wanted)? else {
                    return Ok(None);
                };
                let name = match &view.content {
                    ItemContentView::Readable { attachments, .. } => {
                        // By slot: the list no longer holds the removed attachments.
                        attachments
                            .iter()
                            .find(|a| a.slot == index)
                            .map(|a| a.name.clone())
                    }
                    ItemContentView::Unreadable { .. } => None,
                };
                let Some(name) = name else {
                    return Ok(None);
                };
                // Decrypts into the core's cache, downloading first if needed.
                let Some(cached) = e.attachment_path(&wanted, index)? else {
                    return Ok(None);
                };
                Ok(Some(named_copy(&open_dir, &wanted, index, &name, &cached)?))
            })
            .await?;
        path.ok_or_else(|| CoreError::NotFound(format!("{id}#{index}")))
    }

    async fn transfers(&self) -> Result<Vec<Transfer>, CoreError> {
        let views = self.call(|e| e.transfers()).await?;
        Ok(views.into_iter().map(to_transfer).collect())
    }

    async fn send_history(&self, device_id: &str) -> Result<(), CoreError> {
        let to = device_id.to_string();
        self.call(move |e| e.start_export(&to).map(|_| ())).await
    }

    async fn answer_history(&self, transfer_id: &str, accept: bool) -> Result<(), CoreError> {
        let id = transfer_id.to_string();
        self.call(move |e| {
            if accept {
                e.accept_import(&id)
            } else {
                e.decline_import(&id)
            }
        })
        .await
    }

    async fn cancel_transfer(&self, transfer_id: &str) -> Result<(), CoreError> {
        let id = transfer_id.to_string();
        self.call(move |e| e.cancel_transfer(&id)).await
    }

    // The pairing runs in `PairingHost`, which maps the errors of the core itself. The calls
    // that reach the relay block, so they run off the async threads.

    async fn pairing_options(&self) -> Result<PairingOptions, CoreError> {
        let host = self.host()?;
        let name = default_device_name();
        tokio::task::spawn_blocking(move || host.options(name))
            .await
            .map_err(|_| CoreError::Other("task".into()))
    }

    fn pairing_view(&self) -> PairingView {
        self.pairing
            .as_ref()
            .map_or(PairingView::None, |host| host.view())
    }

    async fn pairing_create_space_at(
        &self,
        install_token: &str,
        device_name: Option<&str>,
        relay_url: Option<&str>,
    ) -> Result<(), CoreError> {
        let host = self.host()?;
        let relay = relay_url.map(str::to_owned);
        // The install token is a secret: wiped from memory once used, never stored or logged.
        let token = Zeroizing::new(install_token.to_owned());
        let name = device_name.map_or_else(default_device_name, str::to_owned);
        tokio::task::spawn_blocking(move || host.create_space_at(&token, &name, relay.as_deref()))
            .await
            .map_err(|_| CoreError::Other("task".into()))?
    }

    async fn pairing_invite(&self) -> Result<PairingView, CoreError> {
        let host = self.host()?;
        tokio::task::spawn_blocking(move || host.start_invite())
            .await
            .map_err(|_| CoreError::Other("task".into()))?
    }

    async fn pairing_join_at(
        &self,
        code_or_link: &str,
        device_name: Option<&str>,
        relay_url: Option<&str>,
    ) -> Result<PairingView, CoreError> {
        let host = self.host()?;
        let relay = relay_url.map(str::to_owned);
        // The code is a secret too.
        let input = Zeroizing::new(code_or_link.to_owned());
        let name = device_name.map_or_else(default_device_name, str::to_owned);
        tokio::task::spawn_blocking(move || host.start_join_at(&input, &name, relay.as_deref()))
            .await
            .map_err(|_| CoreError::Other("task".into()))?
    }

    fn pairing_confirm(&self, import_history: bool) -> Result<(), CoreError> {
        self.host()?.confirm(import_history)
    }

    fn pairing_cancel(&self) {
        if let Some(host) = &self.pairing {
            // The screen is clear at once; the channel is closed on a short thread.
            let _ = host.cancel();
        }
    }

    fn pairing_leave(&self) {
        if let Some(host) = &self.pairing {
            let _ = host.leave();
        }
    }

    fn pairing_dismiss(&self) {
        if let Some(host) = &self.pairing {
            host.dismiss();
        }
    }

    async fn sync(&self) -> Result<(), CoreError> {
        // Offline is a report, not an error: the outbox keeps what could not leave.
        self.call(|e| e.sync().map(|_| ())).await
    }

    async fn purge_plaintext_cache(&self) -> Result<(), CoreError> {
        if !self.plaintext_dirty.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let engine = self.engine_or_err()?;
        let open_dir = self.open_dir.clone();
        let result = tokio::task::spawn_blocking(move || {
            let core = engine.purge_plaintext_cache();
            let own = wipe_dir(&open_dir);
            (core, own)
        })
        .await
        .map_err(|_| CoreError::Other("task".into()))?;
        match result {
            (Ok(()), 0) => Ok(()),
            // A file is still open in another program: try again later.
            (core, _) => {
                self.plaintext_dirty.store(true, Ordering::SeqCst);
                core.map_err(map_failure)?;
                Err(CoreError::Storage("decrypted file in use".into()))
            }
        }
    }

    // ---- departure ----------------------------------------------------------------------
    //
    // The engine's own calls, mapped one to one. The
    // engine erases ONLY on a positive proof of the relay (`spec/api-v1.md` 7.9.1) or, for
    // `erase_without_proof`, after the strong verification that `LeaveFlow` asks first; nothing
    // here interprets a relay answer or erases by hand (`Engine::leave_space` is never called
    // around its outcome).
    //
    // * `leave_space` / `delete_space` / `retry_departure` need a paired device (`self.call`);
    //   `continue_departure`, `cancel_departure`, `erase_without_proof` and the reads go through
    //   `departure_call` (built on `engine_or_err`): `continue_departure` also runs on a device
    //   that is not paired, where it only removes a leftover `requested` / `deleting` marker. A
    //   `wiping` interrupted by a stop is finished when the engine is opened, not by these calls.
    // * After an `Erased` (also from `erase_without_proof`): `after_erasure`.
    // * "Annuler" before the marker is written: `Gestures`.

    fn departure_available(&self) -> bool {
        self.engine.is_some()
    }

    async fn leave_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.gesture(|e| e.leave_space()).await
    }

    async fn delete_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.gesture(|e| e.delete_space()).await
    }

    async fn retry_departure(&self) -> Result<DepartureOutcome, CoreError> {
        let outcome = self.call(|e| e.retry_departure()).await?;
        Ok(self.settled(map_outcome(outcome)))
    }

    async fn continue_departure(&self) -> Result<DepartureOutcome, CoreError> {
        let outcome = self.departure_call(|e| e.continue_departure()).await?;
        Ok(self.settled(map_outcome(outcome)))
    }

    async fn cancel_departure(&self) -> Result<bool, CoreError> {
        let gestures = self.gestures.clone();
        self.departure_call(move |e| gestures.cancel(e)).await
    }

    fn departure(&self) -> Option<DepartureView> {
        self.engine.as_ref()?.departure().map(map_view)
    }

    fn can_erase_without_proof(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|e| e.can_erase_without_proof())
    }

    async fn erase_without_proof(&self) -> Result<(), CoreError> {
        self.departure_call(|e| e.erase_without_proof()).await?;
        self.after_erasure();
        Ok(())
    }

    fn resume_pairing(&self) {
        // A pairing that a stop interrupted between the acknowledgement of the batch and the
        // provisioning (`pairing-v1.md` §11.4), or a space whose creation lost its answer
        // (§10.2): settled in the background, the relay asked again if it does not answer.
        if let Some(host) = &self.pairing {
            host.spawn_resume();
        }
    }

    fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.doorbell.shutdown();
        self.changed.notify_one();
        // A pairing in progress is cancelled (its channel closed if the relay answers within a
        // moment), so that no code stays valid for a window that is gone.
        if let Some(host) = &self.pairing {
            host.shutdown();
        }
        if let Some(engine) = &self.engine {
            let _ = engine.purge_plaintext_cache();
        }
        let _ = wipe_dir(&self.open_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomi_core::runtime::transport::*;
    use tomi_core::runtime::{Clock, Provision};
    use tomi_core::{DeviceKeys, SpaceKey, TrustStore};

    // ------------------------------------------------------------ the relay, stubbed

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Relay {
        /// Nothing answers.
        Offline,
        /// Answers the session and accepts items.
        Accepts,
        /// Answers the session and refuses items for good.
        Refuses,
        /// Answers the session, accepts items and lets a device leave (`DELETE /v1/me` answers
        /// `200`: the proof of a departure).
        Departs,
        /// Does not recognise the device (`401` on the session, twice): "espace introuvable".
        Lost,
        /// Nothing answers, but only after 300 ms: a gesture whose first request hangs.
        Slow,
    }

    impl Relay {
        fn down(self) -> bool {
            matches!(self, Relay::Offline | Relay::Slow)
        }
    }

    /// Just enough relay for `dispatch`: the session and `POST /v1/items`. Everything
    /// else is unreachable, which is what an offline device sees.
    struct StubRelay(Relay, Mutex<Option<String>>);

    impl StubRelay {
        /// A relay the device already has.
        fn new(relay: Relay) -> Self {
            Self(relay, Mutex::new(Some("https://relay.example.org".into())))
        }
        /// A device with no relay at all (a release build without a default).
        fn without_relay(relay: Relay) -> Self {
            Self(relay, Mutex::new(None))
        }
    }

    fn off<T>() -> TResult<T> {
        Err(RelayError::Network("offline".into()))
    }

    impl RelayTransport for StubRelay {
        fn set_session_token(&self, _token: Option<String>) {}
        fn relay_authority(&self) -> Option<String> {
            self.relay_url()
                .map(|u| u.trim_start_matches("https://").to_owned())
        }
        fn relay_url(&self) -> Option<String> {
            self.1.lock().unwrap().clone()
        }
        fn set_relay(&self, url: Option<&str>) -> TResult<()> {
            *self.1.lock().unwrap() = url.map(str::to_owned);
            Ok(())
        }
        fn hello(&self) -> TResult<Hello> {
            if self.0.down() {
                return off();
            }
            // A relay that answers and holds no space yet.
            Ok(Hello {
                server_time_ms: SystemClock.now_ms(),
                space_initialized: false,
                limits: None,
                features: Vec::new(),
            })
        }
        fn auth_challenge(&self, _device_id: &str) -> TResult<Challenge> {
            if self.0 == Relay::Slow {
                std::thread::sleep(Duration::from_millis(300));
            }
            if self.0.down() {
                return off();
            }
            Ok(Challenge {
                challenge: vec![0u8; 32],
                expires_at_ms: i64::MAX,
            })
        }
        fn leave_space(&self) -> TResult<()> {
            match self.0 {
                Relay::Departs => Ok(()),
                Relay::Offline | Relay::Slow => off(),
                // The only device of its space.
                _ => Err(RelayError::api(409, "last_device")),
            }
        }
        fn auth_session(&self, request: &SessionRequest) -> TResult<SessionGrant> {
            if self.0.down() {
                return off();
            }
            if self.0 == Relay::Lost {
                return Err(RelayError::api(401, "unauthorized"));
            }
            let now = SystemClock.now_ms();
            Ok(SessionGrant {
                token: "tms1.test".into(),
                expires_at_ms: now + 3_600_000,
                device_id: request.device_id.clone(),
                space: SpaceInfo {
                    space_id: "00000000-0000-4000-8000-000000000000".into(),
                    effective_format: 1,
                    key_epoch: 1,
                    rotation_required: false,
                    head_seq: 0,
                    server_time_ms: now,
                    quota_used_bytes: 0,
                    quota_max_bytes: 1 << 31,
                    limits: Limits::default(),
                    features: Vec::new(),
                },
            })
        }
        fn delete_session(&self) -> TResult<()> {
            off()
        }
        fn space(&self) -> TResult<SpaceInfo> {
            off()
        }
        fn keys(&self) -> TResult<KeysInfo> {
            off()
        }
        fn devices(&self) -> TResult<Vec<DeviceRecord>> {
            off()
        }
        fn rename_device(&self, _device_id: &str, _name: &str) -> TResult<()> {
            off()
        }
        fn post_rotation(&self, _rotation: &[u8]) -> TResult<u64> {
            off()
        }
        fn events(&self, _after: u64, _limit: u32) -> TResult<EventPage> {
            off()
        }
        fn post_item(&self, _item: &PostItem) -> TResult<Accepted> {
            match self.0 {
                Relay::Accepts | Relay::Departs => Ok(Accepted {
                    seq: 1,
                    replayed: false,
                }),
                Relay::Refuses => Err(RelayError::api(422, "validation_failed")),
                Relay::Offline | Relay::Slow | Relay::Lost => off(),
            }
        }
        fn ack_item(&self, _item_id: &str, _release: bool) -> TResult<()> {
            off()
        }
        fn read_item(&self, _item_id: &str) -> TResult<()> {
            off()
        }
        fn post_tombstone(&self, _tombstone: &[u8]) -> TResult<Accepted> {
            off()
        }
        fn post_item_state(
            &self,
            _event_id: &str,
            _key_epoch: u32,
            _envelope: &[u8],
        ) -> TResult<Accepted> {
            off()
        }
        fn create_blob(&self, _blob_id: &str, _size: u64) -> TResult<BlobStatus> {
            off()
        }
        fn blob_status(&self, _blob_id: &str) -> TResult<BlobStatus> {
            off()
        }
        fn put_blob_part(&self, _blob_id: &str, _offset: u64, _data: &[u8]) -> TResult<u64> {
            off()
        }
        fn get_blob_range(&self, _blob_id: &str, _offset: u64, _len: u64) -> TResult<Vec<u8>> {
            off()
        }
        fn delete_blob(&self, _blob_id: &str) -> TResult<()> {
            off()
        }
        fn post_transfer(&self, _transfer: &PostTransfer) -> TResult<Accepted> {
            off()
        }
        fn get_transfer(&self, _transfer_id: &str) -> TResult<TransferInfo> {
            off()
        }
        fn delete_transfer(&self, _transfer_id: &str, _outcome: &str) -> TResult<()> {
            off()
        }
        fn post_space(&self, _space: &NewSpace) -> TResult<CreatedSpace> {
            match self.0 {
                Relay::Offline | Relay::Slow => off(),
                // The relay of the tests knows no install token.
                _ => Err(RelayError::api(403, "install_token_invalid")),
            }
        }
    }

    // ------------------------------------------------------------ helpers

    fn engine_in(dir: &Path, relay: Relay) -> Engine {
        let store = Store::open(&dir.join(DB_FILE), &dir.join(FILES_DIR), &[7u8; 32]).unwrap();
        Engine::open(
            EngineConfig {
                app_version: "0.0.0-test".into(),
                platform: "windows".into(),
                auto_import: false,
                default_relay_url: None,
            },
            store,
            Arc::new(StubRelay::new(relay)),
            Arc::new(SystemClock),
        )
        .unwrap()
    }

    fn provision(engine: &Engine) {
        let keys = DeviceKeys::generate().unwrap();
        let space_id = tomi_core::new_space_id().unwrap();
        let trust = TrustStore::new(&space_id, &keys.public(), 1).unwrap();
        engine
            .provision(Provision {
                keys,
                trust,
                space_key: SpaceKey::generate(1).unwrap(),
                device_name: "PC-Test".into(),
            })
            .unwrap();
    }

    /// A paired core over a real SQLite store in `dir`.
    fn paired(dir: &Path, relay: Relay) -> Arc<RealCore> {
        let engine = engine_in(dir, relay);
        provision(&engine);
        RealCore::with_engine(Arc::new(engine), dir)
    }

    fn noop() -> ProgressFn {
        Arc::new(|_| {})
    }

    fn text_draft(text: &str) -> ItemDraft {
        ItemDraft {
            id: None,
            title: None,
            text: Some(TextBody {
                plain: text.into(),
                html: None,
                markdown: None,
            }),
            url: None,
            attachments: vec![],
            secret: false,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            source_format: Some(SourceFormat::Plain),
        }
    }

    fn view(content: ItemContentView, origin: ItemOrigin) -> runtime::ItemView {
        runtime::ItemView {
            id: "11111111-1111-4111-8111-111111111111".into(),
            content,
            source_device_id: "dev".into(),
            source_device_name: "Pixel".into(),
            created_at_ms: 10,
            expires_at_ms: None,
            read_policy: tomi_core::ReadPolicy::UntilExpiry,
            read: false,
            origin,
            send_status: None,
            deliveries: vec![],
            pinned: false,
            pinned_at_ms: None,
            archived: false,
            archived_at_ms: None,
        }
    }

    fn readable(title: Option<&str>, plain: Option<&str>, secret: bool) -> ItemContentView {
        ItemContentView::Readable {
            title: title.map(str::to_string),
            text: plain.map(|p| DraftText {
                plain: p.into(),
                html: None,
                markdown: None,
                source_format: CoreFormat::Plain,
            }),
            url: None,
            attachments: vec![],
            secret,
        }
    }

    /// Every file under `dir` that holds `needle` as plain bytes.
    fn files_holding(dir: &Path, needle: &[u8]) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    if bytes.windows(needle.len()).any(|w| w == needle) {
                        found.push(path);
                    }
                }
            }
        }
        found
    }

    // ------------------------------------------------------------ mapping

    #[test]
    fn a_received_item_keeps_its_content_and_its_origin() {
        let mut v = view(
            readable(Some("Courses"), Some("du pain"), false),
            ItemOrigin::Received,
        );
        v.expires_at_ms = Some(99);
        v.read_policy = tomi_core::ReadPolicy::AfterFirstRead;
        let item = to_item(v);
        assert_eq!(item.title.as_deref(), Some("Courses"));
        assert_eq!(item.text.as_ref().unwrap().plain, "du pain");
        assert_eq!(item.source_format, Some(SourceFormat::Plain));
        assert_eq!(item.source_device.name, "Pixel");
        assert_eq!(item.expires_at, Some(99));
        assert_eq!(item.read_policy, ReadPolicy::AfterFirstRead);
        assert!(!item.outgoing && !item.imported && !item.unreadable && !item.secret);
    }

    #[test]
    fn a_hidden_item_stays_hidden_and_the_feed_view_redacts_it() {
        let item = to_item(view(
            readable(Some("Banque"), Some("Tr0ub4dor&3"), true),
            ItemOrigin::Received,
        ));
        assert!(item.secret);
        let json = serde_json::to_string(&crate::view::ItemView::from_item(&item)).unwrap();
        assert!(
            !json.contains("Tr0ub4dor") && !json.contains("Banque"),
            "{json}"
        );
    }

    #[test]
    fn the_pin_of_the_core_reaches_the_item() {
        let mut v = view(readable(None, Some("a"), false), ItemOrigin::Received);
        v.pinned = true;
        v.pinned_at_ms = Some((1 << 53) - 1);
        let item = to_item(v);
        assert!(item.pinned);
        assert_eq!(item.pinned_at, Some((1 << 53) - 1));
        let plain = to_item(view(readable(None, Some("a"), false), ItemOrigin::Received));
        assert!(!plain.pinned && plain.pinned_at.is_none());
    }

    #[test]
    fn the_archive_of_the_core_reaches_the_item() {
        let mut v = view(readable(None, Some("a"), false), ItemOrigin::Received);
        v.archived = true;
        v.archived_at_ms = Some((1 << 53) - 1);
        let item = to_item(v);
        assert!(item.archived && !item.pinned);
        assert_eq!(item.archived_at, Some((1 << 53) - 1));
        assert!(item.is_shown_archived());
        let plain = to_item(view(readable(None, Some("a"), false), ItemOrigin::Received));
        assert!(!plain.archived && plain.archived_at.is_none());
    }

    #[test]
    fn sent_and_imported_items_are_told_apart() {
        let sent = to_item(view(readable(None, Some("a"), false), ItemOrigin::Sent));
        assert!(sent.outgoing && !sent.imported);
        let imported = to_item(view(readable(None, Some("a"), false), ItemOrigin::Imported));
        assert!(!imported.outgoing && imported.imported);
    }

    #[test]
    fn deliveries_follow_the_core_states() {
        use runtime::DeliveryState as Core;
        let mut v = view(readable(None, Some("a"), false), ItemOrigin::Sent);
        v.deliveries = [
            Core::Queued,
            Core::Sent,
            Core::Delivered,
            Core::ExpiredBeforeDelivery,
            Core::Failed,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, state)| runtime::DeliveryView {
            device_id: format!("d{i}"),
            device_name: format!("Appareil {i}"),
            state,
        })
        .collect();
        let states: Vec<_> = to_item(v).delivery.into_iter().map(|d| d.state).collect();
        assert_eq!(
            states,
            vec![
                DeliveryState::Queued,
                DeliveryState::Sent,
                DeliveryState::Delivered,
                DeliveryState::Expired,
                DeliveryState::Failed
            ]
        );
    }

    #[test]
    fn an_unreadable_item_is_kept_and_flagged() {
        let item = to_item(view(
            ItemContentView::Unreadable {
                reason: tomi_core::UnreadableReason::UpdateRequired,
            },
            ItemOrigin::Received,
        ));
        assert!(item.unreadable);
        assert!(item.text.is_none() && item.title.is_none() && item.attachments.is_empty());
        assert_eq!(
            item.unreadable_reason,
            Some(UnreadableReason::UpdateRequired)
        );
    }

    #[test]
    fn an_unreadable_item_says_why_in_the_two_sentences_of_the_app() {
        for (core_reason, expected) in [
            (
                tomi_core::UnreadableReason::UpdateRequired,
                UnreadableReason::UpdateRequired,
            ),
            // A key not here yet, one that a rotation replaced, a failed decryption: kept, and a
            // synchronisation or an update may make them readable.
            (
                tomi_core::UnreadableReason::KeyPending,
                UnreadableReason::Other,
            ),
            (
                tomi_core::UnreadableReason::SuspectEpoch,
                UnreadableReason::Other,
            ),
            (
                tomi_core::UnreadableReason::Undecryptable,
                UnreadableReason::Other,
            ),
        ] {
            let item = to_item(view(
                ItemContentView::Unreadable {
                    reason: core_reason,
                },
                ItemOrigin::Received,
            ));
            assert!(item.unreadable);
            assert_eq!(item.unreadable_reason, Some(expected), "{core_reason:?}");
        }
        // A readable item has no reason.
        let item = to_item(view(readable(None, Some("a"), false), ItemOrigin::Received));
        assert!(!item.unreadable && item.unreadable_reason.is_none());
    }

    // ------------------------------------------------------------ the departure, mapped

    #[test]
    fn the_departure_types_of_the_engine_map_one_to_one() {
        use runtime::{
            DepartureKind as K, DepartureOutcome as O, PendingReason as P, RefusalReason as R,
        };
        assert_eq!(map_kind(K::Leave), DepartureKind::Leave);
        assert_eq!(map_kind(K::DeleteSpace), DepartureKind::DeleteSpace);
        for (core, app) in [
            (P::Network, PendingReason::Network),
            (P::Server, PendingReason::Server),
            (P::NotFound, PendingReason::NotFound),
            (P::NotSent, PendingReason::NotSent),
        ] {
            assert_eq!(map_pending(core), app);
            assert_eq!(
                map_outcome(O::Pending(core)),
                DepartureOutcome::Pending { reason: app }
            );
        }
        for (core, app) in [
            (R::LastDevice, RefusalReason::LastDevice),
            (R::UpdateRequired, RefusalReason::UpdateRequired),
            (R::RelayTooOld, RefusalReason::RelayTooOld),
            (R::Unexpected, RefusalReason::Unexpected),
        ] {
            assert_eq!(map_refusal(core), app);
            assert_eq!(
                map_outcome(O::Refused(core)),
                DepartureOutcome::Refused { reason: app }
            );
        }
        assert_eq!(map_outcome(O::Erased), DepartureOutcome::Erased);
        assert_eq!(map_outcome(O::Cancelled), DepartureOutcome::Cancelled);
        assert_eq!(
            map_outcome(O::NothingPending),
            DepartureOutcome::NothingPending
        );

        let view = map_view(runtime::DepartureView {
            kind: K::DeleteSpace,
            wiping: true,
            since_ms: Some(42),
            last: Some(P::Server),
        });
        assert_eq!(
            view,
            DepartureView {
                kind: DepartureKind::DeleteSpace,
                wiping: true,
                since_ms: Some(42),
                last: Some(PendingReason::Server),
            }
        );
        let bare = map_view(runtime::DepartureView {
            kind: K::Leave,
            wiping: false,
            since_ms: None,
            last: None,
        });
        assert!(bare.since_ms.is_none() && bare.last.is_none() && !bare.wiping);
    }

    #[test]
    fn the_failures_of_a_departure_keep_the_meaning_the_flow_counts_on() {
        // `LeaveFlow::confirm_erase`: `Rejected` = "the verdict is too old, nothing was erased",
        // `Storage` = "failed, try again".
        assert_eq!(
            map_failure(Failure::InvalidInput {
                reason: "erasing without proof needs a recent answer of the relay".into()
            }),
            CoreError::Rejected("erasing without proof needs a recent answer of the relay".into())
        );
        assert_eq!(
            map_failure(Failure::Storage {
                reason: "disk".into()
            }),
            CoreError::Storage("disk".into())
        );
        // A device that is not paired.
        assert_eq!(
            map_failure(Failure::InvalidInput {
                reason: "device is not provisioned".into()
            }),
            CoreError::NotPaired
        );
    }

    #[test]
    fn only_raster_thumbnails_reach_the_webview() {
        let thumb = |mime: &str| Thumbnail {
            mime: mime.into(),
            data: vec![1, 2, 3],
        };
        assert_eq!(
            thumbnail_url(&thumb("image/png")).as_deref(),
            Some("data:image/png;base64,AQID")
        );
        for bad in ["image/svg+xml", "text/html", "application/pdf", ""] {
            assert!(thumbnail_url(&thumb(bad)).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_thumbnail_url_made_by_the_app_goes_back_to_bytes() {
        let t = thumbnail_from_url("data:image/png;base64,AQID").unwrap();
        assert_eq!(
            (t.mime.as_str(), t.data.as_slice()),
            ("image/png", &[1u8, 2, 3][..])
        );
        for bad in [
            "",
            "image/png;base64,AQID",
            "data:text/html;base64,AQID",
            "data:image/png,AQID",
            "data:image/png;base64,***",
        ] {
            assert!(thumbnail_from_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_draft_becomes_an_outgoing_draft_with_the_id_of_the_job() {
        let mut d = text_draft("salut");
        d.title = Some("Titre".into());
        d.secret = true;
        d.expires_at = Some(5);
        d.read_policy = ReadPolicy::AfterFirstRead;
        d.source_format = Some(SourceFormat::Markdown);
        d.attachments = vec![Attachment {
            name: "a.png".into(),
            mime: "image/png".into(),
            size: 3,
            path: Some("C:\\x\\a.png".into()),
            thumbnail: Some("data:image/png;base64,AQID".into()),
            available: true,
            slot: None,
        }];
        let o = to_outgoing(d, "job-id".into());
        assert_eq!(o.id.as_deref(), Some("job-id"));
        assert_eq!(o.title.as_deref(), Some("Titre"));
        assert!(o.secret);
        assert_eq!(o.expires_at_ms, Some(5));
        assert_eq!(o.read_policy, tomi_core::ReadPolicy::AfterFirstRead);
        let t = o.text.unwrap();
        assert_eq!(
            (t.plain.as_str(), t.source_format),
            ("salut", CoreFormat::Markdown)
        );
        assert_eq!(o.attachments[0].path, "C:\\x\\a.png");
        assert_eq!(
            o.attachments[0].thumbnail.as_ref().unwrap().data,
            vec![1, 2, 3]
        );
    }

    #[test]
    fn errors_of_the_core_become_errors_of_the_app() {
        let case = |f: Failure| map_failure(f);
        assert_eq!(
            case(Failure::Network { reason: "x".into() }),
            CoreError::Offline
        );
        assert_eq!(
            case(Failure::Relay {
                status: 503,
                code: "unavailable".into()
            }),
            CoreError::Offline
        );
        assert_eq!(
            case(Failure::Relay {
                status: 429,
                code: "rate_limited".into()
            }),
            CoreError::Offline
        );
        assert_eq!(
            case(Failure::Relay {
                status: 422,
                code: "validation_failed".into()
            }),
            CoreError::Rejected("validation_failed".into())
        );
        assert_eq!(
            case(Failure::InvalidInput {
                reason: "device is not provisioned".into()
            }),
            CoreError::NotPaired
        );
        assert_eq!(
            case(Failure::Blocked {
                reason: "Revoked".into()
            }),
            CoreError::Blocked("Revoked".into())
        );
        assert_eq!(
            case(Failure::MalformedStatement {
                reason: "at_ms".into()
            }),
            CoreError::Rejected("malformed_statement".into())
        );
        assert_eq!(
            case(Failure::DecryptionFailed),
            CoreError::Other("decryption_failed".into())
        );
        // And the app's own error codes: the clock of a pin that cannot be signed.
        assert_eq!(
            crate::error::AppError::from(CoreError::Rejected("malformed_statement".into())).code,
            "error.clock"
        );
    }

    #[test]
    fn transfers_are_offered_run_or_end() {
        let t = |role: &str, state: &str| {
            to_transfer(runtime::TransferView {
                transfer_id: "t".into(),
                role: role.into(),
                peer_device_id: "d".into(),
                peer_name: "Pixel".into(),
                state: state.into(),
                items_total: 10,
                items_done: 4,
                files_total: 2,
                files_done: 1,
                outcome: None,
                error: None,
                finished: false,
            })
        };
        assert_eq!(t("import", "received").phase, TransferPhase::Offered);
        assert_eq!(
            t("import", "received").direction,
            TransferDirection::Receive
        );
        assert_eq!(t("export", "building").phase, TransferPhase::Running);
        assert_eq!(t("export", "posted").direction, TransferDirection::Send);
        for state in [
            "accepted",
            "verifying",
            "downloading",
            "importing",
            "files",
            "finishing",
        ] {
            assert_eq!(t("import", state).phase, TransferPhase::Running, "{state}");
        }
        assert_eq!(t("import", "done").phase, TransferPhase::Done);
        assert_eq!(t("export", "failed").phase, TransferPhase::Failed);
        assert_eq!(t("import", "declined").phase, TransferPhase::Declined);
        assert_eq!(t("export", "cancelled").phase, TransferPhase::Cancelled);
        assert_eq!(t("import", "received").items_done, 4);
    }

    // ------------------------------------------------------------ files on disk

    #[test]
    fn a_decrypted_copy_is_named_like_the_file_and_never_leaves_the_open_folder() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open");
        let src = dir.path().join("0123_0");
        std::fs::write(&src, b"contenu").unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let copy = named_copy(&open, id, 0, "../../Mon fichier?.pdf", &src).unwrap();
        assert!(copy.starts_with(&open));
        assert_eq!(copy.file_name().unwrap(), "Mon fichier_.pdf");
        assert_eq!(std::fs::read(&copy).unwrap(), b"contenu");
        // A second call does not copy again but hands back the same path.
        assert_eq!(
            named_copy(&open, id, 0, "../../Mon fichier?.pdf", &src).unwrap(),
            copy
        );
        // The bytes are on disk once (hard link), so a write in place through the opened name
        // would change the core's cache file: both names are read-only, and an editor that saves
        // in place is refused (it offers "Enregistrer sous").
        assert!(std::fs::metadata(&copy).unwrap().permissions().readonly());
        assert!(std::fs::metadata(&src).unwrap().permissions().readonly());
        assert!(std::fs::OpenOptions::new()
            .append(true)
            .open(&copy)
            .is_err());
        assert!(std::fs::write(&copy, b"modifie").is_err());
        assert_eq!(std::fs::read(&src).unwrap(), b"contenu");
        // A stale entry of another size is replaced, not left behind (removing a read-only file
        // must work: it is what the purge does).
        std::fs::remove_file(&src).unwrap();
        std::fs::write(&src, b"autre contenu").unwrap();
        assert_eq!(
            named_copy(&open, id, 0, "../../Mon fichier?.pdf", &src).unwrap(),
            copy
        );
        assert_eq!(std::fs::read(&copy).unwrap(), b"autre contenu");
        assert!(std::fs::metadata(&copy).unwrap().permissions().readonly());
        // An id that is not an id never becomes part of a path.
        assert!(named_copy(&open, "..\\..", 0, "a.pdf", &src).is_err());
        assert!(named_copy(&open, "", 0, "a.pdf", &src).is_err());
    }

    #[test]
    fn both_names_of_an_opened_file_are_wiped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let open = dir.path().join("open");
        std::fs::create_dir_all(&cache).unwrap();
        let src = cache.join("0123_0");
        std::fs::write(&src, b"CONTENU-LIEN-9c1a").unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let copy = named_copy(&open, id, 0, "a.pdf", &src).unwrap();
        // Removing one name leaves the other, so each wipe has to take its own.
        assert_eq!(wipe_dir(&open), 0);
        assert!(!copy.exists());
        assert_eq!(
            files_holding(dir.path(), b"CONTENU-LIEN-9c1a"),
            std::slice::from_ref(&src)
        );
        std::fs::remove_file(&src).unwrap();
        assert!(files_holding(dir.path(), b"CONTENU-LIEN-9c1a").is_empty());
    }

    #[test]
    fn a_copy_is_made_only_when_the_link_cannot_be() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::write(&src, b"contenu").unwrap();
        // The link refuses (another volume, no hard links): a real copy takes over.
        let dest = dir.path().join("dest");
        link_or_copy(&src, &dest, |_, _| {
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"contenu");
        // Nothing to copy either: the error comes out.
        let missing = dir.path().join("missing");
        assert!(link_or_copy(&missing, &dir.path().join("d2"), |_, _| Err(
            std::io::Error::from(std::io::ErrorKind::Unsupported)
        ))
        .is_err());
        // No right to link, another volume: also a copy.
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::CrossesDevices,
        ] {
            let d = dir.path().join(format!("d-{kind:?}"));
            link_or_copy(&src, &d, |_, _| Err(std::io::Error::from(kind))).unwrap();
            assert_eq!(std::fs::read(&d).unwrap(), b"contenu");
        }
        // Any other failure is not hidden by a copy.
        let other = dir.path().join("other");
        assert!(link_or_copy(&src, &other, |_, _| Err(std::io::Error::other("x"))).is_err());
        assert!(!other.exists());
    }

    #[test]
    fn a_name_taken_in_the_meantime_is_retried_once_and_never_overwritten_by_a_copy() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::write(&src, b"contenu").unwrap();
        let exists = || std::io::Error::from(std::io::ErrorKind::AlreadyExists);

        // Something took the name: it goes, the link is made again.
        let taken = dir.path().join("taken");
        std::fs::write(&taken, b"intrus").unwrap();
        link_or_copy(&src, &taken, |from, to| {
            if std::fs::metadata(to).is_ok() {
                Err(exists())
            } else {
                std::fs::hard_link(from, to)
            }
        })
        .unwrap();
        assert_eq!(std::fs::read(&taken).unwrap(), b"contenu");

        // Taken again right after: an error, not a copy over it.
        let again = dir.path().join("again");
        std::fs::write(&again, b"intrus").unwrap();
        assert!(link_or_copy(&src, &again, |_, _| Err(exists())).is_err());
        assert!(!again.exists());
    }

    #[test]
    fn the_real_link_is_refused_on_an_existing_name() {
        // What `hard_link` really returns when the name exists, and that it is not read as
        // "link impossible" (which would end in a copy over the existing file).
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        std::fs::write(&src, b"a").unwrap();
        std::fs::write(&dest, b"b").unwrap();
        let e = std::fs::hard_link(&src, &dest).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(!link_unavailable(&e));
    }

    #[test]
    fn read_only_files_can_be_removed_the_way_the_core_and_the_host_wipe_them() {
        // The core purges its cache with `remove_file` / `remove_dir_all`, the host `open\` with
        // `wipe_dir`: none may be stopped by FILE_ATTRIBUTE_READONLY.
        let dir = tempfile::tempdir().unwrap();
        let ro = |p: &Path| files::set_readonly(p, true).unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, b"x").unwrap();
        ro(&a);
        std::fs::remove_file(&a).unwrap();
        assert!(!a.exists());

        let tree = dir.path().join("cache");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("f"), b"x").unwrap();
        std::fs::write(tree.join("sub").join("g"), b"y").unwrap();
        ro(&tree.join("f"));
        ro(&tree.join("sub").join("g"));
        assert_eq!(wipe_dir(&tree), 0);
        assert_eq!(std::fs::read_dir(&tree).unwrap().count(), 0);

        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::fs::write(tree.join("sub").join("g"), b"y").unwrap();
        ro(&tree.join("sub").join("g"));
        std::fs::remove_dir_all(&tree).unwrap();
        assert!(!tree.exists());
    }

    #[test]
    fn wiping_a_folder_empties_it_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let open = dir.path().join("open");
        std::fs::create_dir_all(open.join("a_0")).unwrap();
        std::fs::write(open.join("a_0").join("f.txt"), b"x").unwrap();
        std::fs::write(open.join("g.txt"), b"y").unwrap();
        assert_eq!(wipe_dir(&open), 0);
        assert!(open.is_dir());
        assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0);
    }

    // ------------------------------------------------------------ the change stream

    #[tokio::test]
    async fn the_feed_is_announced_by_what_changed_since_the_last_pass() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let mut rx = core.subscribe();

        // The first pass records the feed: what is already there is not news.
        core.emit_changes().await;
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Changed)));

        let out = core.send_item(text_draft("un"), noop()).await.unwrap();
        core.emit_changes().await;
        assert!(
            matches!(rx.try_recv(), Ok(CoreEvent::ItemAdded(i)) if i.id == out.item_id && i.outgoing)
        );
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Changed)));

        // Nothing changed: only the "something changed" tick.
        core.emit_changes().await;
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Changed)));
        assert!(rx.try_recv().is_err());

        // A pin re-emits the item with the pin up to date.
        core.pin(&out.item_id).await.unwrap();
        core.emit_changes().await;
        assert!(
            matches!(rx.try_recv(), Ok(CoreEvent::ItemUpdated(i)) if i.pinned && i.pinned_at.is_some())
        );
        let _ = rx.try_recv();

        core.delete_local(&out.item_id).await.unwrap();
        core.emit_changes().await;
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::ItemRemoved(id)) if id == out.item_id));
    }

    // ------------------------------------------------------------ the engine, end to end

    #[tokio::test]
    async fn an_unpaired_core_says_so_and_refuses_everything() {
        let dir = tempfile::tempdir().unwrap();
        let core =
            RealCore::with_engine(Arc::new(engine_in(dir.path(), Relay::Offline)), dir.path());
        let status = core.status();
        assert!(!status.paired && status.unavailable.is_none());
        assert_eq!(status.space, SpaceState::Active);
        assert_eq!(core.list_items().await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(core.pin("x").await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(core.archive("x").await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(core.unarchive("x").await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(
            core.send_item(text_draft("a"), noop()).await.unwrap_err(),
            CoreError::NotPaired
        );
        assert!(core.transfers().await.is_err());
        assert!(!core.this_device().name.is_empty());
    }

    #[tokio::test]
    async fn a_store_that_cannot_be_opened_is_reported_and_nothing_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let core = RealCore::build(None, None, Some("wrap_key_missing".into()), dir.path());
        let status = core.status();
        assert_eq!(status.unavailable.as_deref(), Some("wrap_key_missing"));
        assert!(!status.paired);
        assert_eq!(
            core.list_items().await.unwrap_err(),
            CoreError::Storage("wrap_key_missing".into())
        );
        // `start` has nothing to start and must not panic.
        core.start();
    }

    #[tokio::test]
    async fn offline_the_item_is_kept_queued_and_can_be_pinned_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        assert!(core.status().paired);

        let out = core.send_item(text_draft("bonjour"), noop()).await.unwrap();
        assert!(out.queued, "no network: the outbox keeps it");
        let items = core.list_items().await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].outgoing && !items[0].pinned);
        assert_eq!(items[0].id, out.item_id);
        assert_eq!(items[0].text.as_ref().unwrap().plain, "bonjour");

        // Pin: local effect at once, idempotent, no network needed.
        core.pin(&out.item_id).await.unwrap();
        let pinned = core.get_item(&out.item_id).await.unwrap();
        assert!(pinned.pinned);
        let at = pinned.pinned_at.expect("time of the decision");
        core.pin(&out.item_id).await.unwrap();
        assert_eq!(
            core.get_item(&out.item_id).await.unwrap().pinned_at,
            Some(at)
        );
        core.unpin(&out.item_id).await.unwrap();
        let unpinned = core.get_item(&out.item_id).await.unwrap();
        assert!(!unpinned.pinned && unpinned.pinned_at.is_none());
        // An unknown id changes nothing and is not an error.
        core.pin("22222222-2222-4222-8222-222222222222")
            .await
            .unwrap();
        core.unpin("22222222-2222-4222-8222-222222222222")
            .await
            .unwrap();

        core.delete_local(&out.item_id).await.unwrap();
        assert!(core.list_items().await.unwrap().is_empty());
        assert!(matches!(
            core.get_item(&out.item_id).await,
            Err(CoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn offline_an_item_can_be_archived_and_unarchived_without_being_read() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let out = core.send_item(text_draft("range"), noop()).await.unwrap();
        let before = core.get_item(&out.item_id).await.unwrap();
        assert!(!before.archived && before.archived_at.is_none());

        // Local effect at once, idempotent, no network needed.
        core.archive(&out.item_id).await.unwrap();
        let archived = core.get_item(&out.item_id).await.unwrap();
        assert!(archived.archived && !archived.pinned);
        let at = archived.archived_at.expect("time of the decision");
        assert_eq!(archived.read, before.read, "archiving never marks read");
        core.archive(&out.item_id).await.unwrap();
        assert_eq!(
            core.get_item(&out.item_id).await.unwrap().archived_at,
            Some(at)
        );

        // Pin and archive exclude each other: pinning unarchives, archiving unpins.
        core.pin(&out.item_id).await.unwrap();
        let pinned = core.get_item(&out.item_id).await.unwrap();
        assert!(pinned.pinned && !pinned.archived && pinned.archived_at.is_none());
        core.archive(&out.item_id).await.unwrap();
        let again = core.get_item(&out.item_id).await.unwrap();
        assert!(again.archived && !again.pinned && again.pinned_at.is_none());

        core.unarchive(&out.item_id).await.unwrap();
        let back = core.get_item(&out.item_id).await.unwrap();
        assert!(!back.archived && back.archived_at.is_none());
        core.unarchive(&out.item_id).await.unwrap();

        // An unknown id changes nothing and is not an error.
        core.archive("22222222-2222-4222-8222-222222222222")
            .await
            .unwrap();
        core.unarchive("22222222-2222-4222-8222-222222222222")
            .await
            .unwrap();
        assert_eq!(core.list_items().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_item_the_relay_accepts_is_not_reported_as_queued() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Accepts);
        let out = core.send_item(text_draft("parti"), noop()).await.unwrap();
        assert!(!out.queued);
        assert_eq!(core.list_items().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_item_the_relay_refuses_for_good_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Refuses);
        let err = core
            .send_item(text_draft("refusé"), noop())
            .await
            .unwrap_err();
        assert_eq!(err, CoreError::Rejected("send_failed".into()));
        assert!(
            core.list_items().await.unwrap().is_empty(),
            "no junk item that can never go out"
        );
        // The job is retried under a new id (see `SendQueue::retry`): that works.
        let mut again = text_draft("refusé");
        again.id = Some(uuid::Uuid::new_v4().to_string());
        assert!(core.send_item(again, noop()).await.is_err());
    }

    #[tokio::test]
    async fn the_id_of_the_job_is_the_id_of_the_item_and_a_retry_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let mut d = text_draft("une fois");
        let id = uuid::Uuid::new_v4().to_string();
        d.id = Some(id.clone());
        let first = core.send_item(d.clone(), noop()).await.unwrap();
        assert_eq!(first.item_id, id);
        let second = core.send_item(d, noop()).await.unwrap();
        assert_eq!(second.item_id, id);
        assert_eq!(core.list_items().await.unwrap().len(), 1, "no duplicate");
    }

    #[tokio::test]
    async fn the_device_is_listed_first_with_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let devices = core.devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].this_device);
        assert_eq!(devices[0].name, "PC-Test");
        assert_eq!(devices[0].state, DeviceState::Verified);
        assert_eq!(core.this_device().name, "PC-Test");
    }

    #[tokio::test]
    async fn nothing_hidden_is_written_in_clear_on_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("releve.txt");
        std::fs::write(&file, b"MARQUEUR-FICHIER-5d1e").unwrap();

        let mut d = text_draft("MARQUEUR-TEXTE-9a3c");
        d.secret = true;
        d.title = Some("MARQUEUR-TITRE-77b0".into());
        d.url = Some("https://MARQUEUR-LIEN-41f2.example/".into());
        d.attachments = vec![Attachment {
            name: "MARQUEUR-NOM-c08d.txt".into(),
            mime: "text/plain".into(),
            size: 21,
            path: Some(file.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        }];
        let out = core.send_item(d, noop()).await.unwrap();
        core.pin(&out.item_id).await.unwrap();

        for marker in [
            "MARQUEUR-TEXTE-9a3c",
            "MARQUEUR-TITRE-77b0",
            "MARQUEUR-LIEN-41f2",
            "MARQUEUR-NOM-c08d",
            "MARQUEUR-FICHIER-5d1e",
        ] {
            let holders = files_holding(dir.path(), marker.as_bytes());
            assert!(holders.is_empty(), "{marker} in the clear in {holders:?}");
        }

        // The core still hands it back, hidden, to whoever asks the app.
        let item = core.get_item(&out.item_id).await.unwrap();
        assert!(item.secret);
        assert_eq!(item.title.as_deref(), Some("MARQUEUR-TITRE-77b0"));
        assert_eq!(item.attachments[0].name, "MARQUEUR-NOM-c08d.txt");
    }

    #[tokio::test]
    async fn an_opened_attachment_is_decrypted_on_demand_then_wiped() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("notes.txt");
        std::fs::write(&file, b"CONTENU-DU-FICHIER-e4b7").unwrap();
        let mut d = text_draft("avec un fichier");
        d.attachments = vec![Attachment {
            name: "notes.txt".into(),
            mime: "text/plain".into(),
            size: 23,
            path: Some(file.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        }];
        let out = core.send_item(d, noop()).await.unwrap();
        assert!(files_holding(dir.path(), b"CONTENU-DU-FICHIER-e4b7").is_empty());

        let path = core.attachment_path(&out.item_id, 0).await.unwrap();
        assert_eq!(path.file_name().unwrap(), "notes.txt");
        assert_eq!(std::fs::read(&path).unwrap(), b"CONTENU-DU-FICHIER-e4b7");
        assert!(path.starts_with(dir.path().join(OPEN_DIR)));
        // Read-only (the core's cache file shares the attribute): the REAL core purge, the
        // deletion of an item and the shutdown below still have to erase it.
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());

        core.purge_plaintext_cache().await.unwrap();
        assert!(files_holding(dir.path(), b"CONTENU-DU-FICHIER-e4b7").is_empty());
        assert!(!path.exists());

        // And again on demand.
        let again = core.attachment_path(&out.item_id, 0).await.unwrap();
        assert_eq!(std::fs::read(&again).unwrap(), b"CONTENU-DU-FICHIER-e4b7");
        // Deleting the item takes its decrypted copy with it.
        core.delete_local(&out.item_id).await.unwrap();
        assert!(files_holding(dir.path(), b"CONTENU-DU-FICHIER-e4b7").is_empty());
        assert!(matches!(
            core.attachment_path(&out.item_id, 0).await,
            Err(CoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn the_purge_does_nothing_when_nothing_was_decrypted() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let mut rx = core.subscribe();
        core.purge_plaintext_cache().await.unwrap();
        // The core is not even disturbed: no change to announce.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn shutting_down_erases_the_decrypted_copies() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("a.txt");
        std::fs::write(&file, b"CONTENU-FINAL-12ab").unwrap();
        let mut d = text_draft("x");
        d.attachments = vec![Attachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            size: 18,
            path: Some(file.to_string_lossy().into_owned()),
            thumbnail: None,
            available: true,
            slot: None,
        }];
        let out = core.send_item(d, noop()).await.unwrap();
        core.attachment_path(&out.item_id, 0).await.unwrap();
        assert!(!files_holding(dir.path(), b"CONTENU-FINAL-12ab").is_empty());
        core.shutdown();
        assert!(files_holding(dir.path(), b"CONTENU-FINAL-12ab").is_empty());
    }

    #[tokio::test]
    async fn history_calls_need_a_paired_device_and_a_known_peer() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        assert!(core.transfers().await.unwrap().is_empty());
        // Offline (or not supported by the relay): an error, not a hang, nothing started.
        assert!(core
            .send_history("0123456789abcdef0123456789abcdef")
            .await
            .is_err());
        assert!(core.transfers().await.unwrap().is_empty());
        assert!(core.answer_history("no-such-transfer", true).await.is_err());
        core.cancel_transfer("no-such-transfer").await.unwrap();
    }

    #[tokio::test]
    async fn syncing_offline_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        core.sync().await.unwrap();
    }

    // ------------------------------------------------------------ pairing, over the real engine

    /// A core over a real SQLite store and a transport that also answers `hello`.
    fn core_over(dir: &Path, relay: Relay, paired: bool) -> Arc<RealCore> {
        core_with(dir, Arc::new(StubRelay::new(relay)), paired)
    }

    fn core_with(dir: &Path, stub: Arc<StubRelay>, paired: bool) -> Arc<RealCore> {
        let store = Store::open(&dir.join(DB_FILE), &dir.join(FILES_DIR), &[7u8; 32]).unwrap();
        let engine = Engine::open(
            EngineConfig {
                app_version: "0.0.0-test".into(),
                platform: "windows".into(),
                auto_import: false,
                default_relay_url: None,
            },
            store,
            stub.clone(),
            Arc::new(SystemClock),
        )
        .unwrap();
        if paired {
            provision(&engine);
        }
        RealCore::with_engine_and_relay(Arc::new(engine), stub, dir)
    }

    fn pairing_code(r: Result<impl std::fmt::Debug, CoreError>) -> String {
        match r {
            Err(CoreError::Pairing(code)) => code,
            other => format!("not a pairing error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unpaired_pc_reads_from_the_relay_whether_it_answers() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, false);
        assert_eq!(core.pairing_view(), PairingView::None);
        let o = core.pairing_options().await.unwrap();
        assert!(!o.paired && !o.pending && o.resumed.is_none());
        assert_eq!(o.space_initialized, Some(false), "the relay holds no space");
        assert!(!o.device_name.is_empty());

        // A relay that does not answer is not an error: both ways stay offered, with a note.
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Offline, false);
        assert_eq!(
            core.pairing_options().await.unwrap().space_initialized,
            None
        );
    }

    #[tokio::test]
    async fn a_paired_pc_offers_to_invite_and_never_to_create_or_join() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, true);
        let o = core.pairing_options().await.unwrap();
        assert!(o.paired && o.space_initialized.is_none());
        assert_eq!(
            pairing_code(core.pairing_join("4821-crayon-renard", None).await),
            "already_paired"
        );
        assert_eq!(
            pairing_code(core.pairing_create_space("tok", None).await),
            "already_paired"
        );
        // Offline, no invitation can start: the core says so before opening any channel.
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Offline, true);
        assert_eq!(core.pairing_invite().await.unwrap_err(), CoreError::Offline);
        assert_eq!(core.pairing_view(), PairingView::None);
    }

    #[tokio::test]
    async fn an_unpaired_pc_cannot_invite() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, false);
        assert_eq!(
            core.pairing_invite().await.unwrap_err(),
            CoreError::NotPaired
        );
    }

    #[tokio::test]
    async fn a_code_that_is_not_one_is_refused_before_anything_is_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, false);
        for bad in ["", "garbage", "4821-crayon", "4821-crayon-lune"] {
            assert_eq!(
                pairing_code(core.pairing_join(bad, None).await),
                "code_invalid",
                "{bad:?}"
            );
        }
        for bad in [
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/l/pair?4821-crayon-renard",
        ] {
            assert_eq!(
                pairing_code(core.pairing_join(bad, None).await),
                "link_invalid",
                "{bad:?}"
            );
        }
        assert_eq!(core.pairing_view(), PairingView::None);
    }

    const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";

    /// A release build has no relay: it comes from a link, a QR code, a creation link or the
    /// "Relais" field, and without any of them the screen is told to ask for one. The core is asked
    /// for real, so this also keeps the host in step with the reasons the core gives.
    #[tokio::test]
    async fn a_pc_without_a_relay_asks_for_one_and_judges_the_one_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_with(
            dir.path(),
            Arc::new(StubRelay::without_relay(Relay::Accepts)),
            false,
        );
        let o = core.pairing_options().await.unwrap();
        assert_eq!(o.relay_authority, None);
        // No relay: neither the code, nor a link of the fallback form, nor a token is enough.
        for code in ["4821-crayon-renard", "tomi-share://pair#4821-crayon-renard"] {
            assert_eq!(
                pairing_code(core.pairing_join(code, Some("PC")).await),
                "relay_not_configured"
            );
        }
        assert_eq!(
            pairing_code(core.pairing_create_space(TOKEN, Some("PC")).await),
            "relay_not_configured"
        );
        assert_eq!(
            pairing_code(
                core.pairing_join_at("4821-crayon-renard", Some("PC"), Some("   "))
                    .await
            ),
            "relay_not_configured"
        );
        // A relay that is not one.
        for bad in [
            "ftp://relay.example.org",
            "relay.example.org",
            "https://a b",
            "https://x@y.org",
        ] {
            assert_eq!(
                pairing_code(
                    core.pairing_join_at("4821-crayon-renard", Some("PC"), Some(bad))
                        .await
                ),
                "relay_invalid",
                "{bad:?}"
            );
            assert_eq!(
                pairing_code(
                    core.pairing_create_space_at(TOKEN, Some("PC"), Some(bad))
                        .await
                ),
                "relay_invalid",
                "{bad:?}"
            );
        }
        // A link and a field that name two relays.
        assert_eq!(
            pairing_code(
                core.pairing_join_at(
                    "https://relay.example.org/l/pair#4821-crayon-renard",
                    Some("PC"),
                    Some("https://other.example.net")
                )
                .await
            ),
            "relay_mismatch"
        );
        assert_eq!(
            pairing_code(
                core.pairing_create_space_at(
                    &format!("https://relay.example.org/create#{TOKEN}"),
                    Some("PC"),
                    Some("https://other.example.net")
                )
                .await
            ),
            "relay_mismatch"
        );
        // Not a creation link.
        assert_eq!(
            pairing_code(
                core.pairing_create_space("https://relay.example.org/create#abc", Some("PC"))
                    .await
            ),
            "setup_link_invalid"
        );
        // Nothing was kept by all this: still no relay, nothing pending, no pairing.
        let o = core.pairing_options().await.unwrap();
        assert!(o.relay_authority.is_none() && !o.pending);
        assert_eq!(core.pairing_view(), PairingView::None);
    }

    /// The relay of a link or of the field is accepted (it will be shown in full at the
    /// verification); here the stub refuses the install token, which proves the creation went as
    /// far as the relay without a relay of its own.
    #[tokio::test]
    async fn a_relay_read_from_a_link_or_a_field_is_used_for_the_creation() {
        for (input, field) in [
            (TOKEN.to_owned(), Some("https://relay.example.org")),
            (format!("https://relay.example.org/create#{TOKEN}"), None),
            (format!("https://relay.example.org/create/{TOKEN}"), None),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let core = core_with(
                dir.path(),
                Arc::new(StubRelay::without_relay(Relay::Accepts)),
                false,
            );
            assert_eq!(
                pairing_code(
                    core.pairing_create_space_at(&input, Some("PC"), field)
                        .await
                ),
                "token_invalid"
            );
        }
    }

    /// The core is asked for real: the host tells the refusals of `create_space` apart by the
    /// reasons the core gives, and this keeps the two in step.
    #[tokio::test]
    async fn the_host_tells_the_refusals_of_the_core_by_the_reasons_it_gives() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, false);
        assert_eq!(
            pairing_code(core.pairing_create_space("", Some("PC")).await),
            "token_invalid"
        );
        assert_eq!(
            pairing_code(
                core.pairing_create_space(&"x".repeat(500), Some("PC"))
                    .await
            ),
            "token_invalid"
        );
        for bad_name in ["", "   ", "a\u{7}b", &"x".repeat(65)] {
            assert_eq!(
                pairing_code(core.pairing_create_space("tok", Some(bad_name)).await),
                "name_invalid",
                "{bad_name:?}"
            );
        }
        // The relay of this test refuses every install token. The refusal reaches the user
        // as such, the PC stays unpaired, and the keys it sealed before asking stay for the
        // next try with the right token (`pairing-v1.md` §10.2).
        assert!(!core.pairing_options().await.unwrap().pending);
        assert_eq!(
            pairing_code(core.pairing_create_space("tok", Some("PC-Test")).await),
            "token_invalid"
        );
        assert!(!core.status().paired);
        assert!(core.pairing_options().await.unwrap().pending);
        assert_eq!(core.pairing_view(), PairingView::None);
        // The way is free: nothing is left running.
        assert_eq!(
            pairing_code(core.pairing_create_space("tok", Some("PC-Test")).await),
            "token_invalid"
        );
    }

    #[tokio::test]
    async fn the_confirmation_of_a_pairing_that_is_not_there_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Accepts, true);
        assert_eq!(
            core.pairing_confirm(true).unwrap_err(),
            CoreError::Pairing("no_session".into())
        );
        // Cancelling and dismissing nothing is harmless.
        core.pairing_cancel();
        core.pairing_dismiss();
        core.shutdown();
    }

    // ------------------------------------------------------------ the departure, over the engine

    /// A decrypted feed in memory, a decrypted copy of a file on disk, a doorbell loop: what the
    /// host holds itself and must let go of when the engine erases.
    fn host_holds_things(core: &RealCore, dir: &Path) {
        let item = to_item(view(
            readable(Some("t"), Some("SECRET-EN-MEMOIRE"), true),
            ItemOrigin::Received,
        ));
        *lock(&core.snapshot) = Some(HashMap::from([(item.id.clone(), item)]));
        std::fs::create_dir_all(dir.join(OPEN_DIR).join("a_0")).unwrap();
        std::fs::write(dir.join(OPEN_DIR).join("a_0").join("f.txt"), b"x").unwrap();
        core.doorbell.spawn(core.engine.as_ref().unwrap());
        assert!(core.doorbell.running());
    }

    #[tokio::test]
    async fn the_departure_is_available_with_an_engine_and_not_without() {
        let dir = tempfile::tempdir().unwrap();
        assert!(paired(dir.path(), Relay::Offline).departure_available());
        // Unpaired: still available (a leftover marker is settled there).
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Offline, false);
        assert!(core.departure_available());
        // No store: nothing to depart from.
        let dir = tempfile::tempdir().unwrap();
        let core = RealCore::build(None, None, Some("wrap_key_missing".into()), dir.path());
        assert!(!core.departure_available());
        assert!(core.departure().is_none() && !core.can_erase_without_proof());
        assert_eq!(
            core.continue_departure().await.unwrap_err(),
            CoreError::Storage("wrap_key_missing".into())
        );
        assert_eq!(
            core.leave_space().await.unwrap_err(),
            CoreError::Storage("wrap_key_missing".into())
        );
        assert_eq!(
            core.cancel_departure().await.unwrap_err(),
            CoreError::Storage("wrap_key_missing".into())
        );
    }

    #[tokio::test]
    async fn leaving_on_the_proof_of_the_relay_erases_and_the_host_lets_go_of_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Departs);
        host_holds_things(&core, dir.path());
        let engine = core.engine.clone().unwrap();
        let old_flag = lock(&core.doorbell.state).flag.clone().unwrap();
        let mut rx = core.subscribe();

        assert_eq!(core.leave_space().await.unwrap(), DepartureOutcome::Erased);

        // The engine erased (on the relay's `200`).
        assert!(!core.status().paired);
        assert!(core.departure().is_none());
        assert!(!engine.is_provisioned());
        assert!(core.list_items().await.is_err());
        // The host let go: the decrypted feed, the decrypted copies, the doorbell loop (by its
        // own flag, not the one of the bridge and the housekeeping).
        assert!(
            lock(&core.snapshot).is_none(),
            "no decrypted item in memory"
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join(OPEN_DIR))
                .unwrap()
                .count(),
            0
        );
        // (Its flag is set unless the loop had already left by itself: it leaves at its next turn
        // when the device is no longer provisioned.)
        assert!(!core.doorbell.running());
        assert!(
            !core.stop.load(Ordering::SeqCst),
            "the bridge and the housekeeping go on: a later pairing needs them"
        );
        let mut told = false;
        while let Ok(event) = rx.try_recv() {
            told |= matches!(event, CoreEvent::Changed);
        }
        assert!(told, "the windows are told to read the state again");

        // A bridge pass that was still running cannot bring a decrypted item back.
        *lock(&core.snapshot) = Some(HashMap::new());
        core.emit_changes().await;
        assert!(lock(&core.snapshot).is_none());

        // Paired again (what the pairing hook does): a new doorbell loop, with a new flag.
        provision(&engine);
        core.doorbell.spawn(&engine);
        assert!(core.doorbell.running());
        let new_flag = lock(&core.doorbell.state).flag.clone().unwrap();
        assert!(!Arc::ptr_eq(&old_flag, &new_flag));
        assert!(!new_flag.load(Ordering::SeqCst));
        core.shutdown();
        assert!(
            new_flag.load(Ordering::SeqCst),
            "quitting stops the loop too"
        );
        assert!(!core.doorbell.running());
    }

    #[tokio::test]
    async fn without_a_proof_nothing_is_erased_and_the_departure_can_be_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        host_holds_things(&core, dir.path());
        core.send_item(text_draft("a garder"), noop())
            .await
            .unwrap();

        // The network is down: the departure waits, visible, nothing erased.
        assert_eq!(
            core.leave_space().await.unwrap(),
            DepartureOutcome::Pending {
                reason: PendingReason::Network
            }
        );
        assert!(core.status().paired);
        let view = core.departure().expect("the marker stays");
        assert_eq!(view.kind, DepartureKind::Leave);
        assert!(!view.wiping && view.since_ms.is_some());
        assert_eq!(view.last, Some(PendingReason::Network));
        assert_eq!(
            core.list_items().await.unwrap().len(),
            1,
            "the feed is intact"
        );
        assert!(lock(&core.snapshot).is_some());
        assert!(core.doorbell.running());
        // A network error is never a reason to erase without proof.
        assert!(!core.can_erase_without_proof());
        assert!(matches!(
            core.erase_without_proof().await,
            Err(CoreError::Rejected(_))
        ));
        assert!(core.status().paired && core.doorbell.running());
        // The automatic attempt does the same, and retries do too.
        assert_eq!(
            core.continue_departure().await.unwrap(),
            DepartureOutcome::Pending {
                reason: PendingReason::Network
            }
        );
        assert_eq!(
            core.retry_departure().await.unwrap(),
            DepartureOutcome::Pending {
                reason: PendingReason::Network
            }
        );

        // "Annuler" removes the marker at once; nothing was sent for it to erase.
        assert!(core.cancel_departure().await.unwrap());
        assert!(core.departure().is_none());
        assert!(core.status().paired);
        assert!(
            !core.cancel_departure().await.unwrap(),
            "nothing left to cancel"
        );
        assert_eq!(
            core.continue_departure().await.unwrap(),
            DepartureOutcome::NothingPending
        );
        assert!(matches!(
            core.retry_departure().await,
            Err(CoreError::Rejected(_))
        ));
    }

    #[tokio::test]
    async fn the_last_device_is_refused_for_good_and_nothing_is_erased() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Accepts);
        assert_eq!(
            core.leave_space().await.unwrap(),
            DepartureOutcome::Refused {
                reason: RefusalReason::LastDevice
            }
        );
        assert!(core.status().paired);
        assert!(core.departure().is_none(), "a refusal removes the marker");
    }

    #[tokio::test]
    async fn a_space_the_relay_cannot_find_allows_erasing_without_proof_and_the_host_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Lost);
        host_holds_things(&core, dir.path());
        assert!(!core.can_erase_without_proof(), "no verdict yet");

        // `401` twice: "espace introuvable". Not a proof: nothing is erased.
        assert_eq!(
            core.leave_space().await.unwrap(),
            DepartureOutcome::Pending {
                reason: PendingReason::NotFound
            }
        );
        assert!(core.status().paired && core.departure().is_some());
        assert!(core.can_erase_without_proof());

        // "Effacer quand même": the same erasure, the same host clean-up.
        core.erase_without_proof().await.unwrap();
        assert!(!core.status().paired);
        assert!(core.departure().is_none());
        assert!(lock(&core.snapshot).is_none());
        assert_eq!(
            std::fs::read_dir(dir.path().join(OPEN_DIR))
                .unwrap()
                .count(),
            0
        );
        assert!(!core.doorbell.running());
        assert!(!core.stop.load(Ordering::SeqCst));
        assert!(!core.can_erase_without_proof());
    }

    #[tokio::test]
    async fn an_unpaired_device_has_nothing_to_leave_but_settles_what_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let core = core_over(dir.path(), Relay::Departs, false);
        assert!(core.departure().is_none() && !core.can_erase_without_proof());
        // Not through `paired_engine`: the answer is the engine's, not "not paired".
        assert_eq!(
            core.continue_departure().await.unwrap(),
            DepartureOutcome::NothingPending
        );
        assert!(!core.cancel_departure().await.unwrap());
        // The gestures need a paired device.
        assert_eq!(core.leave_space().await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(core.delete_space().await.unwrap_err(), CoreError::NotPaired);
        assert_eq!(
            core.retry_departure().await.unwrap_err(),
            CoreError::NotPaired
        );
        assert_eq!(core.gestures_in_flight(), 0);
        assert!(matches!(
            core.erase_without_proof().await,
            Err(CoreError::NotPaired | CoreError::Rejected(_))
        ));
    }

    impl RealCore {
        fn gestures_in_flight(&self) -> usize {
            lock(&self.gestures.state).in_flight
        }
    }

    #[tokio::test]
    async fn an_annuler_before_the_marker_exists_is_carried_out_when_it_appears() {
        let dir = tempfile::tempdir().unwrap();
        // The first request hangs for 300 ms, long enough to cancel while it does.
        let core = paired(dir.path(), Relay::Slow);
        let mut rx = core.subscribe();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let gesture = {
            let core = core.clone();
            tokio::spawn(async move {
                // The engine is called only when released, so the marker does not exist yet.
                core.gesture(move |e| {
                    go_rx.recv().unwrap();
                    e.leave_space()
                })
                .await
            })
        };
        until("the gesture is in flight", || {
            core.gestures_in_flight() == 1
        })
        .await;
        assert!(core.departure().is_none(), "no marker yet");

        // The core's own cancel would have nothing to cancel: here it is kept, and promised.
        assert!(core.cancel_departure().await.unwrap());
        assert!(core.departure().is_none());
        go_tx.send(()).unwrap();

        // The marker appears and the windows are told; the request kept is carried out at once.
        until("the marker is gone again", || {
            // Seen at least once, then cancelled.
            core.departure().is_none() && !lock(&core.gestures.state).cancel_asked
        })
        .await;
        let outcome = gesture.await.unwrap().unwrap();
        assert_eq!(outcome, DepartureOutcome::Cancelled);
        assert!(core.status().paired, "nothing was erased");
        assert!(core.departure().is_none());
        let mut told = false;
        while let Ok(event) = rx.try_recv() {
            told |= matches!(event, CoreEvent::DepartureChanged);
        }
        assert!(told, "the windows were told that the marker appeared");
        // And it does not hang on to the next gesture.
        assert_eq!(core.gestures_in_flight(), 0);
        assert!(!lock(&core.gestures.state).cancel_asked);
    }

    #[tokio::test]
    async fn a_marker_that_appears_without_an_annuler_is_told_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Slow);
        let mut rx = core.subscribe();
        let outcome = core.leave_space().await.unwrap();
        assert_eq!(
            outcome,
            DepartureOutcome::Pending {
                reason: PendingReason::Network
            }
        );
        assert!(core.departure().is_some());
        let mut told = false;
        while let Ok(event) = rx.try_recv() {
            told |= matches!(event, CoreEvent::DepartureChanged);
        }
        assert!(
            told,
            "the banner hears of the marker as soon as it is written"
        );
    }

    #[test]
    fn a_cancel_asked_before_the_engine_is_called_makes_the_gesture_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let engine = core.engine.clone().unwrap();
        let gestures = Gestures::default();

        // No gesture, no marker: nothing to cancel, and nothing is promised.
        assert!(!gestures.cancel(&engine).unwrap());
        assert!(!gestures.take_cancel());

        // A gesture on its way, no marker yet: promised, and spent by the gesture.
        gestures.begin();
        assert!(gestures.cancel(&engine).unwrap());
        assert!(gestures.take_cancel());
        assert!(!gestures.take_cancel(), "spent");

        // A gesture that ends without ever writing a marker leaves no request behind.
        assert!(gestures.cancel(&engine).unwrap());
        gestures.end();
        assert!(!gestures.take_cancel());

        // A request of an earlier gesture does not apply to the next one.
        gestures.begin();
        assert!(gestures.cancel(&engine).unwrap());
        gestures.begin();
        assert!(!gestures.take_cancel());
        gestures.end();
        gestures.end();
    }

    #[test]
    fn an_annuler_promises_nothing_once_the_marker_has_been_seen() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let engine = core.engine.clone().unwrap();
        let gestures = Gestures::default();

        // A gesture whose marker was written, and seen by the watcher.
        gestures.begin();
        engine.leave_space().unwrap();
        assert!(engine.departure().is_some());
        assert!(gestures.marker_written(&engine));
        // The marker goes (here: removed by the engine) while the gesture is still counted:
        // "no marker" no longer means "not written yet".
        assert!(engine.cancel_departure().unwrap());
        assert!(engine.departure().is_none());
        assert!(
            !gestures.cancel(&engine).unwrap(),
            "the engine's answer stands"
        );
        assert!(!gestures.take_cancel(), "and nothing is kept for later");
        gestures.end();

        // The next gesture starts afresh.
        gestures.begin();
        assert!(gestures.cancel(&engine).unwrap());
        gestures.end();
    }

    #[test]
    fn an_annuler_after_an_erasure_the_watcher_never_saw_promises_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Departs);
        let engine = core.engine.clone().unwrap();
        let gestures = Gestures::default();

        // The marker is written and the proof received before the watcher's next poll: it saw
        // nothing, the gesture is still counted, but the device is already erased.
        gestures.begin();
        assert_eq!(
            map_outcome(engine.leave_space().unwrap()),
            DepartureOutcome::Erased
        );
        assert!(!engine.is_provisioned() && engine.departure().is_none());
        assert!(!gestures.cancel(&engine).unwrap());
        assert!(!gestures.take_cancel());
        gestures.end();
    }

    #[tokio::test]
    async fn the_doorbell_has_a_flag_of_its_own_per_loop() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let engine = core.engine.clone().unwrap();
        let doorbell = Doorbell::new();

        // An engine that is not provisioned has nothing to ring for.
        let other = tempfile::tempdir().unwrap();
        doorbell.spawn(&Arc::new(engine_in(other.path(), Relay::Offline)));
        assert!(!doorbell.running());

        doorbell.spawn(&engine);
        assert!(doorbell.running());
        let first = lock(&doorbell.state).flag.clone().unwrap();
        doorbell.spawn(&engine);
        assert!(
            Arc::ptr_eq(&first, &lock(&doorbell.state).flag.clone().unwrap()),
            "one loop"
        );

        doorbell.stop();
        assert!(first.load(Ordering::SeqCst) && !doorbell.running());
        doorbell.spawn(&engine);
        let second = lock(&doorbell.state).flag.clone().unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(
            !second.load(Ordering::SeqCst),
            "the new loop is not stopped by the old flag"
        );

        // The old loop leaving does not touch the state of its successor.
        doorbell.ended(&first);
        assert!(doorbell.running());

        doorbell.shutdown();
        assert!(second.load(Ordering::SeqCst) && !doorbell.running());
        doorbell.spawn(&engine);
        assert!(!doorbell.running(), "quitting: no loop starts any more");
    }

    // ------------------------------------------------------------ a real relay (opt-in)

    /// A relay process on a free local port, with its own data directory and a file of
    /// configuration that lifts the limits per address and per device on codes, claims and
    /// sessions (three cores on one address ask for several in a row; the relay's own tests
    /// cover the limits). Stopped on drop.
    struct LocalRelay {
        child: std::process::Child,
        url: String,
        token: String,
        binary: String,
        config: std::path::PathBuf,
        log: std::path::PathBuf,
        _data: tempfile::TempDir,
    }

    impl LocalRelay {
        /// A new installation token, as the operator gets it with `admin invite --new`
        /// (the first one is single-use and gone once a space is created).
        /// On Windows `admin` has no socket to a running relay: the relay is stopped for the
        /// command, then started again on the same port and data.
        fn invite(&mut self) -> String {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let out = std::process::Command::new(&self.binary)
                .args(["admin", "invite", "--new", "--config"])
                .arg(&self.config)
                .output()
                .expect("admin invite runs");
            assert!(out.status.success(), "{out:?}");
            let token = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .find(|w| w.len() == 39 && w.matches('-').count() == 7)
                .unwrap_or_else(|| panic!("no token in {out:?}"))
                .to_owned();
            let log = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.log)
                .unwrap();
            let mut command = std::process::Command::new(&self.binary);
            command
                .arg("--config")
                .arg(&self.config)
                .stdout(log.try_clone().unwrap())
                .stderr(log);
            for (name, _) in std::env::vars() {
                if name.starts_with("TOMI_RELAY_") {
                    command.env_remove(name);
                }
            }
            self.child = command.spawn().expect("the relay restarts");
            let addr = self.url.trim_start_matches("http://").to_owned();
            let end = Instant::now() + Duration::from_secs(30);
            while Instant::now() < end && std::net::TcpStream::connect(&addr).is_err() {
                std::thread::sleep(Duration::from_millis(100));
            }
            token
        }

        fn start(binary: &str) -> Self {
            let data = tempfile::tempdir().unwrap();
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let config = data.path().join("config.toml");
            let lifted = "{ per_minute = 6000.0, burst = 1000 }";
            std::fs::write(
                &config,
                format!(
                    "listen = \"127.0.0.1:{port}\"\n\
                     data_dir = '{}'\n\
                     rl_device_pairing = {lifted}\n\
                     rl_ip_claim = {lifted}\n\
                     rl_global_claim = {lifted}\n\
                     rl_ip_public = {lifted}\n\
                     rl_ip_auth = {lifted}\n",
                    data.path().join("relay").display()
                ),
            )
            .unwrap();
            std::fs::create_dir_all(data.path().join("relay")).unwrap();
            let log = std::fs::File::create(data.path().join("relay.log")).unwrap();
            let mut command = std::process::Command::new(binary);
            command
                .arg("--config")
                .arg(&config)
                .stdout(log.try_clone().unwrap())
                .stderr(log);
            // The relay refuses a `TOMI_RELAY_*` variable it does not know (a typo must not run
            // with a default): none of ours reaches it.
            for (name, _) in std::env::vars() {
                if name.starts_with("TOMI_RELAY_") {
                    command.env_remove(name);
                }
            }
            let mut child = command.spawn().expect("the relay starts");
            // The multi-space relay no longer keeps the token in a file: it logs it once at
            // start (`install token: xxxx-...`).
            let log_path = data.path().join("relay.log");
            let find_token = || -> Option<String> {
                let log = std::fs::read_to_string(&log_path).ok()?;
                let rest = log.split("install token: ").nth(1)?;
                let token: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                (!token.is_empty()).then_some(token)
            };
            let end = Instant::now() + Duration::from_secs(30);
            let mut token = None;
            while Instant::now() < end {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    token = find_token();
                    if token.is_some() {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let Some(token) = token else {
                let _ = child.kill();
                let _ = child.wait();
                let log = std::fs::read_to_string(&log_path);
                panic!("the relay logged no install token; its log: {log:?}")
            };
            Self {
                child,
                url: format!("http://127.0.0.1:{port}"),
                token,
                binary: binary.to_owned(),
                config,
                log: log_path,
                _data: data,
            }
        }
    }

    impl Drop for LocalRelay {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// A core over a real store and the HTTP transport to `url`, as `RealCore::open` builds it
    /// (but with a fixed wrapping key: no DPAPI in a test).
    fn http_core(dir: &Path, url: &str, platform: &str) -> Arc<RealCore> {
        let store = Store::open(&dir.join(DB_FILE), &dir.join(FILES_DIR), &[7u8; 32]).unwrap();
        let transport: Arc<dyn RelayTransport> = Arc::new(HttpTransport::new(url).unwrap());
        let engine = Engine::open(
            EngineConfig {
                app_version: "0.0.0-e2e".into(),
                platform: platform.into(),
                auto_import: false,
                default_relay_url: None,
            },
            store,
            transport.clone(),
            Arc::new(SystemClock),
        )
        .unwrap();
        RealCore::with_engine_and_relay(Arc::new(engine), transport, dir)
    }

    fn stage_of(core: &RealCore) -> Option<crate::core_api::PairingStage> {
        core.pairing_view().session().map(|s| s.stage)
    }

    fn sas_of(core: &RealCore) -> String {
        let view = core.pairing_view();
        view.session()
            .and_then(|s| s.sas.as_ref())
            .map(|s| s.expose().to_owned())
            .unwrap_or_default()
    }

    async fn until(what: &str, mut cond: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(90);
        while Instant::now() < end {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    /// The whole of `spec/pairing-v1.md` through the app's own layers, against a real relay:
    /// creation of the space, an invitation, a joining with the history, a refusal, a wrong
    /// code. Opt-in:
    /// `TOMI_E2E_RELAY_BIN=<path to tomi-relay.exe> cargo test -- --ignored pairing_over_a_real_relay`.
    #[tokio::test]
    #[ignore = "needs a built tomi-relay in TOMI_E2E_RELAY_BIN"]
    async fn pairing_over_a_real_relay() {
        use crate::core_api::PairingStage::{Cancelled, Done, Failed, Verify, Waiting};
        let Ok(binary) = std::env::var("TOMI_E2E_RELAY_BIN") else {
            eprintln!("TOMI_E2E_RELAY_BIN is not set: skipped");
            return;
        };
        let relay = LocalRelay::start(&binary);
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let a = http_core(dirs[0].path(), &relay.url, "windows");
        let b = http_core(dirs[1].path(), &relay.url, "android");
        let c = http_core(dirs[2].path(), &relay.url, "windows");

        // §10: the first device creates the space with the install token of the relay.
        assert_eq!(
            a.pairing_options().await.unwrap().space_initialized,
            Some(false)
        );
        assert_eq!(
            pairing_code(a.pairing_create_space("not-the-token", Some("PC-A")).await),
            "token_invalid"
        );
        assert!(!a.status().paired && a.pairing_options().await.unwrap().pending);
        a.pairing_create_space(&relay.token, Some("PC-A"))
            .await
            .unwrap();
        assert!(a.status().paired && !a.pairing_options().await.unwrap().pending);
        a.send_item(text_draft("avant l'appairage"), noop())
            .await
            .unwrap();

        // The relay now holds a space. Another PC may still try to create one: this relay has a
        // single space, so it is refused with `space_exists`.
        let o = b.pairing_options().await.unwrap();
        assert_eq!((o.paired, o.space_initialized), (false, Some(true)));
        assert_eq!(
            pairing_code(b.pairing_create_space(&relay.token, Some("Pixel-B")).await),
            "space_exists"
        );

        // §7: A shows a code, a link and a QR code; B types the code.
        let PairingView::Invite(shown) = a.pairing_invite().await.unwrap() else {
            panic!("an invitation")
        };
        let code = shown.code.as_ref().unwrap().expose().to_owned();
        let link = shown.link.as_ref().unwrap().expose().to_owned();
        assert!(link.ends_with(&format!("/l/pair#{code}")), "{link}");
        assert!(shown.qr.is_some() && shown.expires_at.is_some());
        b.pairing_join(&code, Some("Pixel-B")).await.unwrap();

        // Both show the verification code, and it is the same one.
        until("the verification on both", || {
            stage_of(&a) == Some(Verify) && stage_of(&b) == Some(Verify)
        })
        .await;
        let (on_a, on_b) = (sas_of(&a), sas_of(&b));
        assert_eq!(on_a, on_b);
        assert_eq!(on_a.len(), 7, "{{three digits}} {{three digits}}");
        let PairingView::Join(joining) = b.pairing_view() else {
            panic!("a joining")
        };
        assert_eq!(joining.peer.as_ref().unwrap().name, "PC-A");
        assert_eq!(joining.history.as_ref().map(|h| h.items), Some(1));
        let PairingView::Invite(inviting) = a.pairing_view() else {
            panic!("an invitation")
        };
        assert_eq!(inviting.peer.as_ref().unwrap().name, "Pixel-B");
        assert!(inviting.code.is_none() && inviting.qr.is_none());

        // Both users agree: the batch goes, the device is registered and provisioned.
        a.pairing_confirm(true).unwrap();
        b.pairing_confirm(true).unwrap();
        until("the end on both", || {
            stage_of(&a) == Some(Done) && stage_of(&b) == Some(Done)
        })
        .await;
        assert!(b.status().paired);
        let PairingView::Invite(done_a) = a.pairing_view() else {
            panic!()
        };
        assert!(done_a.sas.is_none() && done_a.code.is_none() && done_a.history.is_some());

        // Each knows the other.
        let names =
            |devices: Vec<Device>| -> Vec<String> { devices.into_iter().map(|d| d.name).collect() };
        assert_eq!(names(a.devices().await.unwrap()), ["PC-A", "Pixel-B"]);
        assert_eq!(names(b.devices().await.unwrap()), ["Pixel-B", "PC-A"]);

        // The history that B said yes to arrives without a question, and B is a real member:
        // what it sends reaches A.
        item_arrives(&b, &a, "avant l'appairage").await;
        b.send_item(text_draft("après l'appairage"), noop())
            .await
            .unwrap();
        item_arrives(&a, &b, "après l'appairage").await;
        a.pairing_dismiss();
        b.pairing_dismiss();

        // A new device compares the codes and refuses: A is told, and nothing is registered.
        let PairingView::Invite(second) = a.pairing_invite().await.unwrap() else {
            panic!("an invitation")
        };
        c.pairing_join(second.code.as_ref().unwrap().expose(), Some("PC-C"))
            .await
            .unwrap();
        until("the verification again", || {
            stage_of(&a) == Some(Verify) && stage_of(&c) == Some(Verify)
        })
        .await;
        c.pairing_cancel();
        until("A told of the refusal", || stage_of(&a) == Some(Cancelled)).await;
        let PairingView::Invite(refused) = a.pairing_view() else {
            panic!()
        };
        assert_eq!(refused.error.unwrap().code, "error.pairing_cancelled");
        assert!(!c.status().paired);
        assert_eq!(a.devices().await.unwrap().len(), 2);
        a.pairing_dismiss();

        // A wrong code (the right channel, other words): the new device is told "code
        // incorrect", the inviter gets a fresh code on its own and says so.
        let PairingView::Invite(third) = a.pairing_invite().await.unwrap() else {
            panic!("an invitation")
        };
        let first_code = third.code.as_ref().unwrap().expose().to_owned();
        let channel = first_code.split('-').next().unwrap();
        c.pairing_join(&format!("{channel}-abaisser-abaisser"), Some("PC-C"))
            .await
            .unwrap();
        until("the wrong code told", || stage_of(&c) == Some(Failed)).await;
        let PairingView::Join(wrong) = c.pairing_view() else {
            panic!()
        };
        assert_eq!(wrong.error.unwrap().code, "error.pairing_failed");
        until("a fresh code on A", || {
            a.pairing_view()
                .session()
                .is_some_and(|s| s.stage == Waiting && s.renewed)
        })
        .await;
        let PairingView::Invite(fresh) = a.pairing_view() else {
            panic!()
        };
        assert_ne!(fresh.code.as_ref().unwrap().expose(), first_code);
        a.pairing_cancel();
        c.pairing_dismiss();
        // The relay allows two open channels at once: the one that was just cancelled is closed
        // on a thread of its own.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The third device joins with the link of the QR code, says no to the history, and
        // confirms *before* the inviter: the order of the two decisions does not matter.
        let PairingView::Invite(fourth) = a.pairing_invite().await.unwrap() else {
            panic!("an invitation")
        };
        let link = fourth.link.as_ref().unwrap().expose().to_owned();
        c.pairing_join(&link, Some("PC-C")).await.unwrap();
        until("the verification a third time", || {
            stage_of(&a) == Some(Verify) && stage_of(&c) == Some(Verify)
        })
        .await;
        assert_eq!(sas_of(&a), sas_of(&c));
        c.pairing_confirm(false).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(stage_of(&a), Some(Verify), "A has not decided yet");
        a.pairing_confirm(true).unwrap();
        until("the end for the third device", || {
            stage_of(&a) == Some(Done) && stage_of(&c) == Some(Done)
        })
        .await;
        assert!(c.status().paired);
        let PairingView::Invite(done_c) = a.pairing_view() else {
            panic!()
        };
        assert!(done_c.history.is_none(), "no history was asked for");
        assert_eq!(a.devices().await.unwrap().len(), 3);
        assert_eq!(c.devices().await.unwrap().len(), 3);
        // "No" means an empty feed: what was sent before does not come.
        let _ = a.sync().await;
        let _ = c.sync().await;
        assert!(c.list_items().await.unwrap().is_empty());

        for core in [&a, &b, &c] {
            core.shutdown();
        }
    }

    /// The departure of `spec/api-v1.md` §7.9 against a real relay, through the app's own layers
    /// (`TomiCore`): a device leaves (erased on the proof of the relay), the last one is refused,
    /// deletes the space, and the relay starts again from nothing. Opt-in like
    /// `pairing_over_a_real_relay`.
    #[tokio::test]
    #[ignore = "needs a built tomi-relay in TOMI_E2E_RELAY_BIN"]
    async fn departure_over_a_real_relay() {
        use crate::core_api::PairingStage::Done;
        use crate::core_api::{DepartureOutcome, RefusalReason};
        let Ok(binary) = std::env::var("TOMI_E2E_RELAY_BIN") else {
            eprintln!("TOMI_E2E_RELAY_BIN is not set: skipped");
            return;
        };
        let mut relay = LocalRelay::start(&binary);
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let a = http_core(dirs[0].path(), &relay.url, "windows");
        let b = http_core(dirs[1].path(), &relay.url, "android");

        a.pairing_create_space(&relay.token, Some("PC-A"))
            .await
            .unwrap();
        a.send_item(text_draft("avant le depart"), noop())
            .await
            .unwrap();
        let PairingView::Invite(shown) = a.pairing_invite().await.unwrap() else {
            panic!("an invitation")
        };
        b.pairing_join(shown.code.as_ref().unwrap().expose(), Some("Pixel-B"))
            .await
            .unwrap();
        until("the verification on both", || {
            stage_of(&a) == Some(crate::core_api::PairingStage::Verify)
                && stage_of(&b) == Some(crate::core_api::PairingStage::Verify)
        })
        .await;
        a.pairing_confirm(true).unwrap();
        b.pairing_confirm(true).unwrap();
        until("the end on both", || {
            stage_of(&a) == Some(Done) && stage_of(&b) == Some(Done)
        })
        .await;
        a.pairing_dismiss();
        b.pairing_dismiss();
        assert_eq!(a.devices().await.unwrap().len(), 2);

        // B is not the last device: it leaves, on the proof of the relay, and forgets all.
        assert_eq!(b.leave_space().await.unwrap(), DepartureOutcome::Erased);
        assert!(!b.status().paired, "B was erased");
        assert!(b.departure().is_none());
        let _ = a.sync().await;
        assert_eq!(names_of(&a).await, ["PC-A"], "A sees B gone");

        // A is the last one: the relay refuses, nothing is erased.
        assert_eq!(
            a.leave_space().await.unwrap(),
            DepartureOutcome::Refused {
                reason: RefusalReason::LastDevice
            }
        );
        assert!(a.status().paired);
        assert!(a.departure().is_none());
        assert_eq!(a.list_items().await.unwrap().len(), 1, "the feed is intact");

        // "Supprimer ton espace": the space is deleted and A is erased.
        assert_eq!(a.delete_space().await.unwrap(), DepartureOutcome::Erased);
        assert!(!a.status().paired);

        // The relay holds nothing any more: a new space can be created on a new device.
        let c = http_core(dirs[2].path(), &relay.url, "windows");
        assert_eq!(
            c.pairing_options().await.unwrap().space_initialized,
            Some(false)
        );
        // The first invitation was consumed by the old space: it does not open a new one.
        assert_eq!(
            pairing_code(c.pairing_create_space(&relay.token, Some("PC-C")).await),
            "token_invalid"
        );
        c.pairing_create_space(&relay.invite(), Some("PC-C"))
            .await
            .unwrap();
        assert!(c.status().paired);
        assert!(
            c.list_items().await.unwrap().is_empty(),
            "nothing of the old space"
        );
        for core in [&a, &b, &c] {
            core.shutdown();
        }
    }

    async fn names_of(core: &RealCore) -> Vec<String> {
        core.devices()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect()
    }

    /// Waits until `text` is in the feed of `core`, synchronising both ends meanwhile (a
    /// doorbell may be late).
    async fn item_arrives(core: &RealCore, other: &RealCore, text: &str) {
        let end = Instant::now() + Duration::from_secs(90);
        loop {
            let _ = other.sync().await;
            let _ = core.sync().await;
            let items = core.list_items().await.unwrap();
            if items
                .iter()
                .any(|i| i.text.as_ref().is_some_and(|t| t.plain == text))
            {
                return;
            }
            assert!(Instant::now() < end, "timed out waiting for {text:?}");
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    #[tokio::test]
    async fn a_store_that_cannot_be_opened_has_no_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let core = RealCore::build(None, None, Some("wrap_key_missing".into()), dir.path());
        assert_eq!(core.pairing_view(), PairingView::None);
        let storage = CoreError::Storage("wrap_key_missing".into());
        assert_eq!(core.pairing_options().await.unwrap_err(), storage);
        assert_eq!(core.pairing_invite().await.unwrap_err(), storage);
        assert_eq!(
            core.pairing_join("4821-crayon-renard", None)
                .await
                .unwrap_err(),
            storage
        );
        assert_eq!(
            core.pairing_create_space("tok", None).await.unwrap_err(),
            storage
        );
        assert_eq!(core.pairing_confirm(true).unwrap_err(), storage);
        core.pairing_cancel();
        core.pairing_dismiss();
    }

    // ------------------------------------------------------------ device management

    /// Another device's id, as the relay types it (32 lowercase hexadecimal characters).
    const OTHER_DEVICE: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn this_pc_is_never_revoked() {
        // Even with a relay that answers: the guard comes before any network call.
        for relay in [Relay::Offline, Relay::Accepts] {
            let dir = tempfile::tempdir().unwrap();
            let core = paired(dir.path(), relay);
            let this = core.this_device().id;
            assert_eq!(
                this.len(),
                32,
                "the core's id of this PC has the shape of a device id"
            );
            assert_eq!(
                core.revoke_device(&this).await.unwrap_err(),
                CoreError::Rejected("revoke_self".into())
            );
            assert_eq!(
                core.revoke_device(&this.to_uppercase()).await.unwrap_err(),
                CoreError::Rejected("revoke_self".into())
            );
            // Still a member, still listed.
            assert!(core.devices().await.unwrap().iter().any(|d| d.this_device));
        }
    }

    #[tokio::test]
    async fn revoking_needs_a_paired_pc_and_a_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        for bad in [
            "",
            "../x",
            "not-an-id",
            "0123",
            "0123456789ABCDEF0123456789ABCDEF",
        ] {
            assert_eq!(
                core.revoke_device(bad).await.unwrap_err(),
                CoreError::Rejected("unknown_device".into()),
                "{bad}"
            );
            assert_eq!(
                core.rename_device(bad, "Nom").await.unwrap_err(),
                CoreError::Rejected("unknown_device".into()),
                "{bad}"
            );
        }
        let other_dir = tempfile::tempdir().unwrap();
        let unpaired = RealCore::with_engine(
            Arc::new(engine_in(other_dir.path(), Relay::Offline)),
            dir.path(),
        );
        assert_eq!(
            unpaired.revoke_device(OTHER_DEVICE).await.unwrap_err(),
            CoreError::NotPaired
        );
        assert_eq!(
            unpaired
                .rename_device(OTHER_DEVICE, "Nom")
                .await
                .unwrap_err(),
            CoreError::NotPaired
        );
    }

    #[tokio::test]
    async fn renaming_checks_the_name_before_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        let this = core.this_device().id;
        for bad in ["", "   ", "\t", "a\nb", &"x".repeat(65)] {
            assert_eq!(
                core.rename_device(&this, bad).await.unwrap_err(),
                CoreError::Rejected("name_invalid".into()),
                "{bad:?}"
            );
        }
        // A valid name is not refused for its shape: offline, the answer is "Offline".
        assert_eq!(
            core.rename_device(&this, "  PC du bureau  ")
                .await
                .unwrap_err(),
            CoreError::Offline
        );
        assert_eq!(
            core.rename_device(&this, &"é".repeat(64))
                .await
                .unwrap_err(),
            CoreError::Offline
        );
        assert_eq!(
            core.this_device().name,
            "PC-Test",
            "nothing changed locally"
        );
    }

    #[tokio::test]
    async fn revoking_another_device_offline_is_offline_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let core = paired(dir.path(), Relay::Offline);
        assert_eq!(
            core.revoke_device(OTHER_DEVICE).await.unwrap_err(),
            CoreError::Offline
        );
        assert_eq!(core.devices().await.unwrap().len(), 1);
    }

    #[test]
    fn refusals_of_device_management_have_their_own_codes() {
        use crate::error::AppError;
        let rejected = |code: &str| CoreError::Rejected(code.into());
        // The relay refuses a name the core let through; a device that is not a member.
        assert_eq!(
            rename_refusal(rejected("validation_failed")),
            rejected("name_invalid")
        );
        assert_eq!(
            rename_refusal(rejected("not_found")),
            rejected("unknown_device")
        );
        assert_eq!(rename_refusal(CoreError::Offline), CoreError::Offline);
        // The core's own answer for a device that is not (or no longer) a member.
        assert_eq!(
            revoke_refusal(map_failure(Failure::InvalidInput {
                reason: format!("unknown device {OTHER_DEVICE}")
            })),
            rejected("unknown_device")
        );
        // Raised by the core after its single retry.
        let mismatch = map_failure(Failure::Relay {
            status: 409,
            code: "device_set_mismatch".into(),
        });
        assert_eq!(
            revoke_refusal(mismatch.clone()),
            rejected("device_set_mismatch")
        );
        assert_eq!(AppError::from(mismatch).code, "error.devices_changed");
        assert_eq!(
            AppError::from(rejected("epoch_conflict")).code,
            "error.devices_changed"
        );
        assert_eq!(
            AppError::from(rejected("name_invalid")).code,
            "error.device_name_invalid"
        );
        assert_eq!(
            AppError::from(rejected("unknown_device")).code,
            "error.unknown_device"
        );
        assert_eq!(
            AppError::from(rejected("revoke_self")).code,
            "error.revoke_self"
        );
        assert_eq!(AppError::from(CoreError::Offline).code, "error.offline");
        // Nothing of the id reaches the code the window sees.
        assert!(
            !AppError::from(revoke_refusal(map_failure(Failure::InvalidInput {
                reason: format!("unknown device {OTHER_DEVICE}")
            })))
            .code
            .contains(OTHER_DEVICE)
        );
    }
}
