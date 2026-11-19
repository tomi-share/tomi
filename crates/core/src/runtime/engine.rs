//! The sync engine: session, event application, full resync (`api-v1.md` §7.1 to §7.5).
//!
//! `Engine` is the single object the apps talk to (through UniFFI). It owns the
//! [`Store`](super::store::Store), a [`RelayTransport`] and the in-memory copies of the
//! cryptographic state ([`TrustStore`], [`SpaceKeyRing`], [`PinBook`], [`ArchiveBook`]).
//!
//! Locking: `inner` (state) is taken before the database lock, never the other way
//! round, and never held across a network call. Closures run by [`Engine::with_crypto`]
//! must use the `Db` they receive and nothing else of the engine that locks.

use std::cell::RefCell;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use crate::archive::ArchiveBook;
use crate::device::DeviceKeys;
use crate::envelope::read_envelope_header;
use crate::error::{CoreError, Result};
use crate::item::{Item, ReadPolicy};
use crate::keyring::{ReceivedItem, SpaceKeyRing};
use crate::local::LocalKey;
use crate::pin::{read_item_state_header, ItemStateKind, PinBook, ReceivedItemState};
use crate::runtime::relay::RelayState;
use crate::runtime::store::{
    Db, DeviceRow, DeviceState, FileRow, FileState, ItemOrigin, ItemRow, ItemStatus, NewOp, OpKind,
    SendStatus, Store,
};
use crate::runtime::transport::{
    ClientInfo, Event, EventData, Limits, RelayError, RelayTransport, SessionRequest, Wake,
};
use crate::tombstone::{DeletionScope, Tombstone};
use crate::{
    DeviceCert, KeyRotation, SpaceKey, TrustStore, UnreadableReason, FORMAT_MAX, FORMAT_MIN,
};

/// Time source, replaceable in tests.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_ms(&self) -> i64;
}

/// Wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
    }
}

/// Static configuration of the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Sent in `client.app_version`.
    pub app_version: String,
    /// `windows`, `android` or `cli`.
    pub platform: String,
    /// Import an incoming history transfer as soon as it is announced. When `false` it
    /// waits for [`Engine::accept_import`] (the pairing screen asks the user first).
    pub auto_import: bool,
    /// The relay of a device that has none of its own yet: a default for development builds,
    /// `None` in a release build (the relay then comes from a link, a QR code or the "Relais"
    /// field). Never used to change the relay of a provisioned device, which keeps its own. When
    /// `None`, the relay the transport was made with is the default.
    pub default_relay_url: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").into(),
            platform: "cli".into(),
            auto_import: true,
            default_relay_url: None,
        }
    }
}

/// Everything needed to join (or found) a space; produced by pairing / space creation.
pub struct Provision {
    pub keys: DeviceKeys,
    pub trust: TrustStore,
    pub space_key: SpaceKey,
    /// Name shown for this device.
    pub device_name: String,
}

/// A member whose name and platform the pairing channel announced (the inviter, for the new
/// device).
pub(crate) struct KnownMember {
    pub ed25519: Vec<u8>,
    pub name: String,
    pub platform: String,
}

/// What pairing adds to [`Engine::provision`], applied in the same transaction.
#[derive(Default)]
pub(crate) struct ProvisionExtras {
    pub known_members: Vec<KnownMember>,
    /// The history transfer the inviter announced and this device accepts without asking.
    pub awaited_history: Option<crate::runtime::pairing::AwaitedHistory>,
    /// Sealed records to erase with the provisioning (the pending lot, the founder's material).
    pub drop_secrets: Vec<&'static str>,
}

/// Why the device cannot talk to the relay (`api-v1.md` §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The app is too old for the space.
    UpdateRequired { update_url: Option<String> },
    /// This app cannot write the space's format any more.
    FormatUnsupported { limited_by: Vec<String> },
    /// Revoked. Persisted, and still asked again before any erasure (`api-v1.md` §7.9.1).
    Revoked,
    /// Not found: `401` twice on fresh challenges, or `409 space_not_initialized`. The
    /// relay does not find the space or this device; it proves nothing (an unmounted volume
    /// answers the same, `api-v1.md` §1.4 rule 4). Never persisted, asked again at most once a
    /// minute.
    NotFound,
}

impl Blocked {
    fn persisted(&self) -> Option<&'static str> {
        match self {
            Blocked::Revoked => Some("revoked"),
            _ => None,
        }
    }
}

/// Last answer of the relay about this device's membership, in memory only
/// (`api-v1.md` §7.9.2): what "Erase anyway" (D4) is allowed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// A session was opened: the device is a member.
    Member,
    /// Not found.
    NotFound,
    /// `DELETE /v1/space` is unknown to the relay (`404`, `405`).
    RelayTooOld,
    /// Network error, timeout, `429`, `5xx`: nothing learned.
    Transient,
    /// Anything else (revoked, update required, unexpected refusal): never allows D4.
    Other,
}

/// How long "not found" holds before the relay is asked again (`api-v1.md` §7.2).
const NOT_FOUND_RETRY_MS: i64 = 60_000;

/// Longest wait of the doorbell between two questions to a relay that keeps saying "not found".
const NOT_FOUND_MAX_WAIT_MS: u64 = 1_800_000;

/// Upper bound of the wait of the doorbell after `streak` consecutive "not found" answers of
/// the relay (`streak >= 1`): 60 s, 120 s, 240 s... doubling, at most 30 minutes.
pub(crate) fn not_found_ceiling_ms(streak: u32) -> u64 {
    let base = NOT_FOUND_RETRY_MS.unsigned_abs();
    (base << streak.saturating_sub(1).min(5)).min(NOT_FOUND_MAX_WAIT_MS)
}

/// The wait itself: between 75 % and 100 % of [`not_found_ceiling_ms`] (never below the minute
/// `ensure_session` keeps between two checks, so that the next turn really asks the relay).
pub(crate) fn not_found_wait_ms(streak: u32) -> u64 {
    let ceiling = not_found_ceiling_ms(streak);
    (ceiling - jitter_ms(ceiling / 4)).max(NOT_FOUND_RETRY_MS.unsigned_abs())
}

/// What the doorbell does between two turns ([`Engine::doorbell_turn`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoorbellWait {
    /// How long to wait.
    pub ms: u64,
    /// The wait follows a "not found": it ends as soon as the state is no longer `NotFound`.
    pub after_not_found: bool,
}

/// Secrets and identity of the provisioned device.
pub(crate) struct Active {
    pub keys: DeviceKeys,
    pub lsk: LocalKey,
    pub space_id: String,
    pub device_id: String,
}

/// The cryptographic state that changes together with the database.
#[derive(Clone)]
pub(crate) struct Crypto {
    pub trust: TrustStore,
    pub ring: SpaceKeyRing,
    pub pins: PinBook,
    pub archives: ArchiveBook,
}

struct Session {
    expires_at_ms: i64,
}

pub(crate) struct Inner {
    pub active: Option<Arc<Active>>,
    pub crypto: Option<Crypto>,
    session: Option<Session>,
    pub limits: Limits,
    server_offset_ms: i64,
}

/// Flags that closures running under `inner` must be able to set: kept in their own lock.
#[derive(Default)]
pub(crate) struct Flags {
    pub blocked: Option<Blocked>,
    /// When `blocked` last became (or was confirmed) `NotFound`.
    pub not_found_at_ms: Option<i64>,
    /// "Not found" answers of the relay in a row, since the last session that opened. Drives the
    /// back-off of the doorbell ([`not_found_wait_ms`]); never persisted. It counts the answers
    /// of every caller (a `sync`, a request of the app, the doorbell), not only the ones the
    /// doorbell asked for: each of them is a question put to the relay that got no member in
    /// return, so the back-off may reach its ceiling sooner than the doorbell's own turns
    /// alone would take it. It starts over at 0 when a session opens
    /// ([`Engine::open_session`]) and when the identity is forgotten
    /// ([`Engine::forget_identity`]).
    pub not_found_streak: u32,
    pub rotation_required: bool,
    /// Engine clock time from which the devices' activity (`last_seen_at`) is read again from
    /// the relay ([`Engine::refresh_devices_if_due`]). 0 = at the first opportunity, which is
    /// the first sync of a process (or of a new identity: the flags start over then).
    pub devices_next_refresh_at_ms: i64,
    pub resync_notice: bool,
    /// Last verdict of the relay and its time (engine clock).
    pub verdict: Option<(Verdict, i64)>,
    /// Why the last departure attempt of this process is pending (`DepartureView::last`).
    pub departure_last: Option<crate::runtime::leave::PendingReason>,
    /// When the automatic rotation is due (`api-v1.md` §7.6), `None` when none is planned.
    pub rotation_due_at_ms: Option<i64>,
    /// Failed automatic rotations in a row (back-off).
    pub rotation_failures: u32,
}

/// What a [`Engine::sync`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Another sync was already running; it will take this request into account.
    pub already_running: bool,
    pub events_applied: u64,
    pub new_items: u64,
    pub cursor: u64,
    /// The cursor was reset by a full resync (`cursor_expired` / `cursor_ahead`).
    pub full_resync: bool,
    pub outbox_sent: u32,
    pub outbox_pending: u32,
    /// The relay could not be reached (the local state is unchanged).
    pub offline: bool,
    /// The relay asks a member to rotate the key (`rotation_required`). The engine does it by
    /// itself after a random delay of 0 to 30 s (`api-v1.md` §7.6), at the end of a later
    /// `sync` or from the doorbell loop: the app has nothing to call. Kept for compatibility.
    pub rotation_required: bool,
}

type Listener = Arc<dyn Fn() + Send + Sync>;

/// See the module documentation.
///
/// # Departure: duration and strong authentication
///
/// [`leave_space`](Self::leave_space), [`delete_space`](Self::delete_space),
/// [`retry_departure`](Self::retry_departure) and
/// [`continue_departure`](Self::continue_departure) **block** for the whole attempt, and an
/// attempt is far longer than a minute on a bad network: every call has its own time limit
/// and the attempt chains several of them. The worst case is 520 s (about 9 minutes) for
/// `leave_space`, and 1060 s (about 18 minutes) for `delete_space` when the relay answers
/// `not_last_device`; see the table in the module documentation of `runtime::leave`. A
/// caller that is waiting for the departure lock adds the time of the attempt in front of it.
/// Run them off the main thread, show [`departure`](Self::departure) meanwhile, and let
/// "Annuler" ([`cancel_departure`](Self::cancel_departure)) work at any time: it never waits.
///
/// [`erase_without_proof`](Self::erase_without_proof) destroys everything this device holds
/// without the relay's word, and the core cannot tell who asks. The application MUST put a
/// strong biometric check (same scope as revoking another device) in front of it, every
/// time, and an honest irreversible confirmation.
pub struct Engine {
    pub(crate) store: Store,
    pub(crate) transport: Arc<dyn RelayTransport>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) cfg: EngineConfig,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) flags: Mutex<Flags>,
    pub(crate) sync_lock: Mutex<()>,
    sync_dirty: AtomicBool,
    pub(crate) dispatch_lock: Mutex<()>,
    pub(crate) transfer_lock: Mutex<()>,
    pub(crate) dirty_again: AtomicBool,
    /// Operations that already had their catch-up sync in the current [`Engine::sync`].
    pub(crate) resynced_ops: Mutex<HashSet<i64>>,
    /// A notification was asked for while a serialising lock was held.
    notify_pending: AtomicBool,
    listeners: Mutex<Vec<Listener>>,
    /// A live joining session holds the sealed batch of this (new) device and waits for the
    /// inviter: [`Engine::resume_pairing`] leaves it alone (`runtime::pairing`).
    pub(crate) lot_held: AtomicBool,
    /// Serialises the departure attempts and the erasure without proof (`runtime::leave`).
    /// Global lock order: `departure_lock`, `sync_lock`, `dispatch_lock`, `transfer_lock`,
    /// `inner`, the store.
    pub(crate) departure_lock: Mutex<()>,
    /// A rotation runs (automatic, or a revocation by the user).
    pub(crate) rotating: AtomicBool,
    /// Held for the whole life of a doorbell loop ([`Engine::run_doorbell`]): at most one
    /// connection to the doorbell at a time (`api-v1.md` §5.5). Taken first, before any other
    /// lock of the engine, and never while another one is held.
    pub(crate) doorbell_gate: Mutex<()>,
    /// Loops of [`Engine::run_doorbell`] that wait for `doorbell_gate` (observed by the tests).
    pub(crate) doorbell_waiting: AtomicU32,
    /// `max_ttl_secs` of the relay's limits, readable while `inner` is locked (the opening of an
    /// item runs inside `with_crypto`, which holds it): kept in step with `Inner::limits`.
    max_ttl_secs: AtomicU64,
    /// The relay gate (`runtime::relay`): the pairings and creations holding the relay. While
    /// there is one, the relay does not change. Every decision about the relay (start of a
    /// session, `set_relay`, the end of a session, the settling) is taken **with this lock held**,
    /// so that a check and the switch it allows cannot be separated by another thread.
    /// Lock order: after the serialising locks, before `inner` and the store (the relay code
    /// reads `inner` and the store under it); nothing waits for another engine lock, and no call
    /// to the relay is made, while it is held.
    pub(crate) relay_gate: Mutex<RelayState>,
    /// The relay of a device that keeps none: `EngineConfig::default_relay_url`, else the one the
    /// transport was made with.
    pub(crate) relay_default: Option<String>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

/// Locks `m`, recovering it if a panic poisoned it. The data the engine guards with a
/// mutex is either replaced whole (`Inner`, `Flags`) or unit (`()` serialising locks), so a
/// panic elsewhere never leaves it half-updated; refusing to work until the next start would
/// be worse.
pub(crate) fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    Ok(m.lock().unwrap_or_else(PoisonError::into_inner))
}

thread_local! {
    /// Engines (by address) whose serialising lock (`sync`, `dispatch`, `transfer`) this
    /// thread holds. Their notifications wait until the last of these locks is released,
    /// so that a listener never runs under an engine lock.
    static HOLDING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Guard of a serialising lock taken through [`Engine::serial`]. On drop it releases the
/// lock first, then delivers the notifications deferred meanwhile.
pub(crate) struct Serial<'a> {
    engine: &'a Engine,
    guard: Option<MutexGuard<'a, ()>>,
}

impl Drop for Serial<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        let me = self.engine.addr();
        let still_held = HOLDING.with(|h| {
            let mut h = h.borrow_mut();
            if let Some(pos) = h.iter().rposition(|a| *a == me) {
                h.remove(pos);
            }
            h.contains(&me)
        });
        if !still_held && self.engine.notify_pending.swap(false, Ordering::SeqCst) {
            self.engine.fire_listeners();
        }
    }
}

/// `meta` key: the decoder version (`Engine::decoder_version`) at the last replay of the
/// unreadable items and kept item states.
pub(crate) const REPLAYED_VERSION_KEY: &str = "replayed_decoder_version";
/// How long a removal of an attachment whose item is not here is remembered (`format-v1.md`
/// §8.10): the registry of removals is separate from the tombstones.
const ATTACHMENT_REMOVAL_RETENTION_MS: i64 = 45 * 86_400_000;
const KEYS_SECRET: &str = "device_keys";
const LSK_SECRET: &str = "local_key";
const SESSION_RENEW_MARGIN_MS: i64 = 300_000;

/// `meta` key: present while an automatic rotation after a revocation keeps failing with
/// `device_set_mismatch` (the relay lists a member this device's trust store does not know).
pub(crate) const ROTATION_STALLED_KEY: &str = "rotation_stalled";

/// `meta` key of the capabilities announced by the relay (JSON list of strings).
pub(crate) const FEATURES_KEY: &str = "relay_features";

/// Capability of the relay: `POST /v1/attachment-tombstones` (`api-v1.md` §4.8 bis).
pub(crate) const FEATURE_ATTACHMENT_TOMBSTONES: &str = "attachment_tombstones";
/// Upper bound of fetch / drain rounds in one [`Engine::sync`]: what is left (a concurrent
/// request, a re-issued operation) is done by the next call, so `sync` always returns.
const MAX_SYNC_ROUNDS: u32 = 8;
/// How often the activity of the devices (`last_seen_at`, `GET /v1/devices`) is read again
/// while the device is in use (a sync or the doorbell loop is running). The relay writes
/// `last_seen_at` at most once a minute and rounds it down to the minute (`api-v1.md` §4.3,
/// §7.2): while the device is in use, the value shown is at most about this + 1 minute (relay
/// write throttle) + 1 minute (rounding) old, so under 7 minutes.
const DEVICES_REFRESH_EVERY_MS: i64 = 5 * 60_000;
/// Pause before trying again after a refresh of the devices that failed (offline, 5xx).
const DEVICES_REFRESH_RETRY_MS: i64 = 60_000;
/// Key of `meta`: the `last_seen_at_ms` values of the `devices` table come from the relay only.
/// Earlier builds wrote the time of pairing there (not an activity): absent, they are dropped
/// once at the next opening.
pub(crate) const LAST_SEEN_FROM_RELAY: &str = "devices_last_seen_from_relay";

impl Engine {
    /// Opens the engine on `store`. If the store already holds a provisioned device the
    /// state is loaded; otherwise [`provision`](Self::provision) must be called.
    pub fn open(
        cfg: EngineConfig,
        store: Store,
        transport: Arc<dyn RelayTransport>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let relay_default = cfg
            .default_relay_url
            .clone()
            .or_else(|| transport.relay_url());
        let engine = Self {
            store,
            transport,
            clock,
            cfg,
            inner: Mutex::new(Inner {
                active: None,
                crypto: None,
                session: None,
                limits: Limits::default(),
                server_offset_ms: 0,
            }),
            flags: Mutex::new(Flags::default()),
            sync_lock: Mutex::new(()),
            sync_dirty: AtomicBool::new(false),
            dispatch_lock: Mutex::new(()),
            transfer_lock: Mutex::new(()),
            dirty_again: AtomicBool::new(false),
            resynced_ops: Mutex::new(HashSet::new()),
            notify_pending: AtomicBool::new(false),
            listeners: Mutex::new(Vec::new()),
            lot_held: AtomicBool::new(false),
            departure_lock: Mutex::new(()),
            rotating: AtomicBool::new(false),
            doorbell_gate: Mutex::new(()),
            doorbell_waiting: AtomicU32::new(0),
            max_ttl_secs: AtomicU64::new(crate::DEFAULT_MAX_TTL_SECS),
            relay_gate: Mutex::new(RelayState::default()),
            relay_default,
        };
        engine.load()?;
        engine.load_relay();
        // `space_deleted`, written by an earlier version, is not a proof of anything
        // (`api-v1.md` §7.2): ignored by `load`, and removed here.
        engine.store.tx(|db| {
            if db.meta_get("blocked")?.as_deref() == Some(b"space_deleted".as_slice()) {
                db.meta_delete("blocked")?;
            }
            // The time of pairing that earlier builds stored as "last seen" was not an
            // activity: forgotten once, then only the relay's own word is stored.
            // Only a provisioned database: an empty one stays empty (an erased device keeps
            // no trace), and `provision` writes the marker itself.
            if db.meta_get("space_id")?.is_some() && db.meta_get(LAST_SEEN_FROM_RELAY)?.is_none() {
                db.clear_devices_last_seen()?;
                db.meta_set(LAST_SEEN_FROM_RELAY, b"1")?;
            }
            Ok(())
        })?;
        // An erasure (leaving the space) that a stop interrupted is finished first. A failure
        // must not prevent the start: it is retried at the next opening (and before any new
        // pairing, provision_with).
        let _ = engine.finish_pending_wipe();
        // Decrypted copies left by a crash or a kill, and plaintext files of older builds.
        // A copy still open by another program (Windows) must not prevent the start: it is
        // retried at the next purge.
        let _ = engine.purge_plaintext_cache();
        engine.sweep_legacy_plaintext()?;
        Ok(engine)
    }

    fn addr(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// Takes a serialising lock (`dispatch_lock`, `transfer_lock`): notifications raised
    /// while it is held are delivered after it is released.
    pub(crate) fn serial<'a>(&'a self, m: &'a Mutex<()>) -> Serial<'a> {
        let guard = m.lock().unwrap_or_else(PoisonError::into_inner);
        self.hold(guard)
    }

    /// Non-blocking [`serial`](Self::serial); `None` when another thread holds the lock.
    pub(crate) fn try_serial<'a>(&'a self, m: &'a Mutex<()>) -> Option<Serial<'a>> {
        let guard = match m.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some(self.hold(guard))
    }

    fn hold<'a>(&'a self, guard: MutexGuard<'a, ()>) -> Serial<'a> {
        HOLDING.with(|h| h.borrow_mut().push(self.addr()));
        Serial {
            engine: self,
            guard: Some(guard),
        }
    }

    fn load(&self) -> Result<()> {
        let loaded = self.store.read(|db| {
            let Some(space_id) = db.meta_get_string("space_id")? else {
                return Ok(None);
            };
            let keys_bytes = db
                .secret_get(KEYS_SECRET)?
                .ok_or_else(|| CoreError::input("device keys missing"))?;
            if keys_bytes.len() != 64 {
                return Err(CoreError::input("device keys record"));
            }
            let keys = DeviceKeys::from_secrets(&keys_bytes[..32], &keys_bytes[32..])?;
            let lsk = LocalKey::from_bytes(
                &db.secret_get(LSK_SECRET)?
                    .ok_or_else(|| CoreError::input("local key missing"))?,
            )?;
            let trust = TrustStore::from_json(
                &db.trust_json()?
                    .ok_or_else(|| CoreError::input("trust store missing"))?,
            )?;
            let ring = SpaceKeyRing::from_records(&db.keyring_records()?)?;
            let pins = db.pin_book()?;
            let archives = db.archive_book()?;
            // Only `revoked` is kept on disk; anything else (`space_deleted` of an earlier
            // version) is ignored.
            let blocked = match db.meta_get_string("blocked")?.as_deref() {
                Some("revoked") => Some(Blocked::Revoked),
                _ => None,
            };
            let limits = db
                .meta_get("limits")?
                .and_then(|b| serde_json::from_slice::<Limits>(&b).ok())
                .unwrap_or_default();
            let device_id = keys.device_id();
            Ok(Some((
                Active {
                    keys,
                    lsk,
                    space_id,
                    device_id,
                },
                Crypto {
                    trust,
                    ring,
                    pins,
                    archives,
                },
                blocked,
                limits,
            )))
        })?;
        if let Some((active, crypto, blocked, limits)) = loaded {
            let mut inner = lock(&self.inner)?;
            inner.active = Some(Arc::new(active));
            inner.crypto = Some(crypto);
            self.max_ttl_secs
                .store(limits.max_ttl_secs, Ordering::SeqCst);
            inner.limits = limits;
            drop(inner);
            lock(&self.flags)?.blocked = blocked;
        }
        Ok(())
    }

    /// Stores the identity and keys obtained by pairing or space creation, once.
    pub fn provision(&self, p: Provision) -> Result<()> {
        self.provision_with(p, ProvisionExtras::default())
    }

    /// [`provision`](Self::provision), plus what pairing hands over in the same transaction
    /// (`pairing-v1.md` §8.4): the names of the members learned on the pairing channel, the
    /// announced history transfer to accept, and the pending records to erase. Either all of it
    /// is stored, or nothing: a device is never half provisioned with its pending lot gone.
    pub(crate) fn provision_with(&self, p: Provision, extras: ProvisionExtras) -> Result<()> {
        if self.is_provisioned() {
            return Err(CoreError::input("device already provisioned"));
        }
        // Files of a previous space that an interrupted erasure left must go before the new
        // space writes its own (a later start would otherwise erase them).
        self.finish_pending_wipe()?;
        let space_id = p.trust.space_id();
        let ring = SpaceKeyRing::new(p.space_key);
        let lsk = LocalKey::generate()?;
        let device_id = p.keys.device_id();
        let mut secret = p.keys.ed25519_seed();
        secret.extend_from_slice(&p.keys.x25519_secret());
        let secret = zeroize::Zeroizing::new(secret);
        self.store.tx(|db| {
            db.secret_put(KEYS_SECRET, &secret)?;
            db.secret_put(LSK_SECRET, &lsk.to_bytes())?;
            db.set_trust_json(&p.trust.to_json()?)?;
            db.set_keyring_records(&ring.to_records())?;
            db.meta_set("device_name", p.device_name.as_bytes())?;
            // The relay of the space, for good (`runtime::relay`): from now on it never changes.
            if let Some(url) = self.transport.relay_url() {
                db.meta_set(crate::runtime::relay::RELAY_URL_KEY, url.as_bytes())?;
            }
            db.meta_delete(crate::runtime::relay::PENDING_RELAY_KEY)?;
            for m in p.trust.members() {
                let id = m.device_id()?;
                let known = extras.known_members.iter().find(|k| k.ed25519 == m.ed25519);
                let (name, platform) = if id == device_id {
                    (p.device_name.clone(), "unknown".to_owned())
                } else if let Some(k) = known {
                    (k.name.clone(), k.platform.clone())
                } else {
                    ("Appareil".to_owned(), "unknown".to_owned())
                };
                db.put_device(&DeviceRow {
                    device_id: id,
                    name,
                    platform,
                    ed25519: Some(m.ed25519.clone()),
                    x25519: Some(m.x25519.clone()),
                    state: DeviceState::Verified,
                    joined_seq: 0,
                    // The time of pairing is not an activity of that device: unknown until
                    // the relay says (`refresh_devices`, from the first sync).
                    last_seen_at_ms: None,
                })?;
            }
            if let Some(h) = &extras.awaited_history {
                crate::runtime::pairing::set_awaited_history(db, h)?;
            }
            for name in &extras.drop_secrets {
                db.secret_delete(name)?;
            }
            // A device that joins a space after leaving another one starts unblocked.
            db.meta_delete("blocked")?;
            db.meta_delete(crate::runtime::leave::LEAVE_STATE)?;
            db.meta_delete(crate::runtime::leave::DEPARTURE_SINCE)?;
            db.meta_delete(crate::runtime::leave::DELETION_ID)?;
            // From now on the activity of the devices comes from the relay only.
            db.meta_set(LAST_SEEN_FROM_RELAY, b"1")?;
            // Written last: its presence means "provisioned".
            db.meta_set("space_id", space_id.as_bytes())
        })?;
        *lock(&self.flags)? = Flags::default();
        {
            let mut inner = lock(&self.inner)?;
            // A session of an earlier identity (opened while it was being erased) must not
            // serve the new one.
            inner.session = None;
            inner.server_offset_ms = 0;
            // The token of an earlier identity must not be used for this one, and it is dropped
            // BEFORE the new identity becomes live, under `inner`, so that a token opened for
            // the new identity can never be erased by it. (The joining side of a pairing passes
            // its `Pairing` token apart.)
            self.transport.set_session_token(None);
            inner.crypto = Some(Crypto {
                trust: p.trust,
                ring,
                pins: PinBook::new(),
                archives: ArchiveBook::new(),
            });
            inner.active = Some(Arc::new(Active {
                keys: p.keys,
                lsk,
                space_id,
                device_id,
            }));
        }
        self.notify();
        Ok(())
    }

    pub fn is_provisioned(&self) -> bool {
        lock(&self.inner)
            .map(|i| i.active.is_some())
            .unwrap_or(false)
    }

    pub(crate) fn active(&self) -> Result<Arc<Active>> {
        lock(&self.inner)?
            .active
            .clone()
            .ok_or_else(|| CoreError::input("device is not provisioned"))
    }

    /// Id of this device.
    pub fn device_id(&self) -> Result<String> {
        Ok(self.active()?.device_id.clone())
    }

    /// Id of the space.
    pub fn space_id(&self) -> Result<String> {
        Ok(self.active()?.space_id.clone())
    }

    /// Why the relay cannot be used, if so.
    pub fn blocked(&self) -> Option<Blocked> {
        lock(&self.flags).ok().and_then(|f| f.blocked.clone())
    }

    /// Records why the relay cannot be used, for the identity `active` the answer was received
    /// for. Does nothing unless `active` is still the live identity: a late `4003` or `403` of
    /// the doorbell or of a request in flight, after the device left the space and erased
    /// everything (or joined another one), must write nothing, neither on disk nor in memory,
    /// and above all not block the identity that replaced it. Done under inner, so that it
    /// cannot interleave with the erasure ([forget_identity](Self::forget_identity)).
    pub(crate) fn set_blocked(&self, active: &Arc<Active>, b: Blocked) {
        {
            let Ok(inner) = lock(&self.inner) else {
                return;
            };
            if !inner
                .active
                .as_ref()
                .is_some_and(|now| Arc::ptr_eq(now, active))
            {
                return;
            }
            if let Some(v) = b.persisted() {
                let _ = self.store.tx(|db| {
                    if db.meta_get("space_id")?.is_some() {
                        db.meta_set("blocked", v.as_bytes())?;
                    }
                    Ok(())
                });
            }
            if let Ok(mut f) = lock(&self.flags) {
                f.blocked = Some(b);
            }
        }
        self.notify();
    }

    /// Not found (`401` twice, `409 space_not_initialized`): in memory only, never a proof.
    /// The session is dropped so that the next check starts from a fresh challenge. Does
    /// nothing unless `active`, the identity the answer was received for, is still the live one
    /// (see [`set_blocked`](Self::set_blocked)). Counts one more answer in
    /// [`Flags::not_found_streak`], whoever asked (the doorbell, a `sync`, a request of the app).
    pub(crate) fn set_not_found(&self, active: &Arc<Active>) {
        let now = self.clock.now_ms();
        {
            let Ok(mut inner) = lock(&self.inner) else {
                return;
            };
            if !inner
                .active
                .as_ref()
                .is_some_and(|live| Arc::ptr_eq(live, active))
            {
                return;
            }
            inner.session = None;
            if let Ok(mut f) = lock(&self.flags) {
                f.blocked = Some(Blocked::NotFound);
                f.not_found_at_ms = Some(now);
                f.not_found_streak = f.not_found_streak.saturating_add(1);
                f.verdict = Some((Verdict::NotFound, now));
            }
        }
        self.transport.set_session_token(None);
        self.notify();
    }

    /// Records the last verdict of the relay (`api-v1.md` §7.9.2).
    pub(crate) fn set_verdict(&self, v: Verdict) {
        let now = self.clock.now_ms();
        if let Ok(mut f) = lock(&self.flags) {
            f.verdict = Some((v, now));
        }
    }

    /// Runs `f` in one store transaction only if `active` is still the live identity: `inner`
    /// is held for the whole transaction (the order of [`with_crypto`](Self::with_crypto)), so
    /// that no write can land after the erasure (`api-v1.md` §7.9.6). Otherwise
    /// `InvalidInput("device is not provisioned")` and nothing is written.
    pub(crate) fn live_tx<R>(
        &self,
        active: &Arc<Active>,
        f: impl FnOnce(&Db<'_>) -> Result<R>,
    ) -> Result<R> {
        let inner = lock(&self.inner)?;
        if !inner
            .active
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, active))
        {
            return Err(CoreError::input("device is not provisioned"));
        }
        let out = self.store.tx(f);
        drop(inner);
        out
    }

    /// Forgets the identity, the keys, the session and the flags, right after the erasing
    /// transaction of [Engine::leave_space]. Called with inner locked. The secrets are
    /// zeroized when their last owner drops (DeviceKeys, LocalKey, SpaceKey zeroize on
    /// drop); a call in flight that still holds the Arc<Active> frees them when it ends.
    pub(crate) fn forget_identity(&self, inner: &mut Inner) {
        inner.active = None;
        inner.crypto = None;
        inner.session = None;
        self.max_ttl_secs
            .store(Limits::default().max_ttl_secs, Ordering::SeqCst);
        inner.limits = Limits::default();
        inner.server_offset_ms = 0;
        if let Ok(mut f) = lock(&self.flags) {
            *f = Flags::default();
        }
        if let Ok(mut ops) = lock(&self.resynced_ops) {
            ops.clear();
        }
        self.sync_dirty.store(false, Ordering::SeqCst);
        self.dirty_again.store(false, Ordering::SeqCst);
    }

    /// Limits announced by the relay (defaults until the first session).
    pub fn limits(&self) -> Limits {
        lock(&self.inner)
            .map(|i| i.limits.clone())
            .unwrap_or_default()
    }

    /// Estimated relay clock (`api-v1.md` §4.8, "estimated server time").
    pub fn server_now_ms(&self) -> i64 {
        let offset = lock(&self.inner).map(|i| i.server_offset_ms).unwrap_or(0);
        self.clock.now_ms().saturating_add(offset)
    }

    /// Registers a callback run after every change visible in the feed, the devices or the
    /// state. It is never called with an engine lock held, and a panic inside it is caught
    /// (the engine stays usable). It should still return quickly.
    pub fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut l) = lock(&self.listeners) {
            l.push(listener);
        }
    }

    /// Tells the listeners, now or, under a serialising lock, once it is released.
    pub(crate) fn notify(&self) {
        let me = self.addr();
        if HOLDING.with(|h| h.borrow().contains(&me)) {
            self.notify_pending.store(true, Ordering::SeqCst);
            return;
        }
        self.fire_listeners();
    }

    fn fire_listeners(&self) {
        let ls: Vec<Listener> = lock(&self.listeners).map(|l| l.clone()).unwrap_or_default();
        for l in ls {
            // A failing callback (an exception of the app) must not unwind into the engine.
            let _ = catch_unwind(AssertUnwindSafe(|| l()));
        }
    }

    /// Runs `f` on a working copy of the crypto state inside one database transaction. If
    /// `f` succeeds the transaction commits and the copy replaces the live state; otherwise
    /// neither changes.
    pub(crate) fn with_crypto<R>(
        &self,
        f: impl FnOnce(&mut Crypto, &Db<'_>, &Active) -> Result<R>,
    ) -> Result<R> {
        let mut inner = lock(&self.inner)?;
        let active = inner
            .active
            .clone()
            .ok_or_else(|| CoreError::input("device is not provisioned"))?;
        let mut work = inner
            .crypto
            .clone()
            .ok_or_else(|| CoreError::input("device is not provisioned"))?;
        let out = self.store.tx(|db| f(&mut work, db, &active))?;
        inner.crypto = Some(work);
        Ok(out)
    }

    /// Read-only access to the live crypto state.
    pub(crate) fn read_crypto<R>(
        &self,
        f: impl FnOnce(&Crypto, &Active) -> Result<R>,
    ) -> Result<R> {
        let inner = lock(&self.inner)?;
        let active = inner
            .active
            .clone()
            .ok_or_else(|| CoreError::input("device is not provisioned"))?;
        let crypto = inner
            .crypto
            .as_ref()
            .ok_or_else(|| CoreError::input("device is not provisioned"))?;
        f(crypto, &active)
    }

    pub(crate) fn persist_crypto(c: &Crypto, db: &Db<'_>) -> Result<()> {
        db.set_trust_json(&c.trust.to_json()?)?;
        db.set_keyring_records(&c.ring.to_records())
    }

    // -----------------------------------------------------------------------
    // Session (api-v1 §3.3, §7.2 ensure_session)
    // -----------------------------------------------------------------------

    /// Makes sure a session token valid for more than 5 minutes is installed.
    ///
    /// `NotFound` is not a final state: the relay is asked again once a minute has passed
    /// since it last said so; before that the call fails at once, without network.
    pub fn ensure_session(&self) -> Result<()> {
        let active = self.active()?;
        if let Some(b) = self.blocked() {
            let retry = b == Blocked::NotFound && {
                let since = lock(&self.flags)?.not_found_at_ms;
                since.is_none_or(|t| self.clock.now_ms().saturating_sub(t) >= NOT_FOUND_RETRY_MS)
            };
            if !retry {
                return Err(CoreError::Blocked {
                    reason: format!("{b:?}"),
                });
            }
        }
        {
            let inner = lock(&self.inner)?;
            if let Some(s) = &inner.session {
                let now = self.clock.now_ms().saturating_add(inner.server_offset_ms);
                if s.expires_at_ms - now > SESSION_RENEW_MARGIN_MS {
                    return Ok(());
                }
            }
        }
        let mut attempt = 0;
        loop {
            let result = self.open_session(&active);
            match result {
                Ok(()) => return Ok(()),
                Err(RelayError::Api(p)) => match p.code.as_str() {
                    "unauthorized" if attempt == 0 => {
                        attempt += 1;
                    }
                    "unauthorized" => {
                        self.set_not_found(&active);
                        return Err(blocked_err("NotFound"));
                    }
                    "update_required" => {
                        self.set_verdict(Verdict::Other);
                        self.set_blocked(
                            &active,
                            Blocked::UpdateRequired {
                                update_url: p.update_url.clone(),
                            },
                        );
                        return Err(blocked_err("update required"));
                    }
                    "format_unsupported" => {
                        self.set_verdict(Verdict::Other);
                        self.set_blocked(
                            &active,
                            Blocked::FormatUnsupported {
                                limited_by: p.limited_by.clone(),
                            },
                        );
                        return Err(blocked_err("format unsupported"));
                    }
                    "revoked" => {
                        self.set_verdict(Verdict::Other);
                        self.set_blocked(&active, Blocked::Revoked);
                        return Err(blocked_err("revoked"));
                    }
                    "space_not_initialized" => {
                        self.set_not_found(&active);
                        return Err(blocked_err("NotFound"));
                    }
                    _ => {
                        if p.status == 429 || p.status >= 500 {
                            self.set_verdict(Verdict::Transient);
                        }
                        return Err(RelayError::Api(p).into());
                    }
                },
                Err(e) => {
                    self.set_verdict(Verdict::Transient);
                    return Err(e.into());
                }
            }
        }
    }

    /// `active` is the identity the challenge is signed with; the session is installed only if it
    /// is still the live one when the answer arrives.
    pub(crate) fn open_session(&self, active: &Arc<Active>) -> std::result::Result<(), RelayError> {
        self.transport.set_session_token(None);
        let c = self
            .transport
            .auth_challenge(&active.device_id)
            .map_err(challenge_error)?;
        let signature = active
            .keys
            .sign_relay_auth(&active.space_id, &c.challenge)
            .map_err(|e| RelayError::Network(format!("cannot sign challenge: {e}")))?;
        let grant = self.transport.auth_session(&SessionRequest {
            device_id: active.device_id.clone(),
            challenge: c.challenge,
            signature,
            client: ClientInfo {
                format_min: FORMAT_MIN,
                format_max: FORMAT_MAX,
                app_version: self.cfg.app_version.clone(),
                platform: self.cfg.platform.clone(),
            },
        })?;
        // Installed under `inner`, like the erasure: if the identity this session was opened
        // for was forgotten (or replaced) meanwhile, nothing is installed and nothing is
        // written, so that a revoked token never reaches a later space.
        let mut inner = lock(&self.inner).map_err(|e| RelayError::Network(e.to_string()))?;
        if !inner
            .active
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, active))
        {
            return Err(RelayError::Network(
                "the device changed during the session request".into(),
            ));
        }
        self.transport.set_session_token(Some(grant.token));
        inner.session = Some(Session {
            expires_at_ms: grant.expires_at_ms,
        });
        inner.server_offset_ms = grant.space.server_time_ms - self.clock.now_ms();
        self.max_ttl_secs
            .store(grant.space.limits.max_ttl_secs, Ordering::SeqCst);
        inner.limits = grant.space.limits.clone();
        if let Ok(mut f) = lock(&self.flags) {
            f.rotation_required = grant.space.rotation_required;
            if grant.space.rotation_required {
                // Drawn once; a due date already drawn is not pushed back (§7.6).
                let now = self.clock.now_ms();
                f.rotation_due_at_ms
                    .get_or_insert_with(|| now.saturating_add(jitter_i64(30_000)));
            } else {
                // Nothing to rotate (any more): no due date, and no failure left to back off on.
                f.rotation_due_at_ms = None;
                f.rotation_failures = 0;
            }
            f.blocked = None;
            f.not_found_at_ms = None;
            f.not_found_streak = 0;
            f.verdict = Some((Verdict::Member, self.clock.now_ms()));
        }
        // Capabilities of the relay, kept for `supports_attachment_removal` (no engine lock).
        if let Ok(json) = serde_json::to_vec(&grant.space.features) {
            let _ = self.store.tx(|db| db.meta_set(FEATURES_KEY, &json));
        }
        if let Ok(json) = serde_json::to_vec(&grant.space.limits) {
            let _ = self.store.tx(|db| {
                db.meta_set("limits", &json)?;
                db.meta_delete("blocked")
            });
        }
        Ok(())
    }

    pub(crate) fn drop_session(&self) {
        if let Ok(mut inner) = lock(&self.inner) {
            inner.session = None;
        }
        self.transport.set_session_token(None);
    }

    /// Calls `f`, renewing the session once on `session_expired` / `unauthorized`.
    pub(crate) fn authed<T>(
        &self,
        mut f: impl FnMut() -> std::result::Result<T, RelayError>,
    ) -> std::result::Result<T, RelayError> {
        // The identity the request is made for: a late `403` / `409` must not mark the one
        // that replaced it (erased, then joined again) meanwhile.
        let identity = self.active().ok();
        match f() {
            Err(RelayError::Api(p)) if p.code == "session_expired" || p.code == "unauthorized" => {
                self.drop_session();
                self.ensure_session().map_err(|e| match e {
                    CoreError::Relay { status, code } => RelayError::api(status, &code),
                    other => RelayError::Network(other.to_string()),
                })?;
                // The answer after the renewal is read like the first one.
                self.note_answer(identity.as_ref(), f())
            }
            first => self.note_answer(identity.as_ref(), first),
        }
    }

    /// What an answer of a route B says about the device, for `identity` (the one the request
    /// was made for): `403 revoked` marks it `Revoked`, `409 space_not_initialized` marks it
    /// `NotFound` in memory only (never a proof, §1.4 rule 4). Nothing is ever erased here. The
    /// answer is handed back as it is.
    fn note_answer<T>(
        &self,
        identity: Option<&Arc<Active>>,
        answer: std::result::Result<T, RelayError>,
    ) -> std::result::Result<T, RelayError> {
        if let (Err(RelayError::Api(p)), Some(a)) = (&answer, identity) {
            match p.code.as_str() {
                "revoked" => self.set_blocked(a, Blocked::Revoked),
                "space_not_initialized" => self.set_not_found(a),
                _ => {}
            }
        }
        answer
    }

    // -----------------------------------------------------------------------
    // Sync loop (api-v1 §7.2)
    // -----------------------------------------------------------------------

    /// Fetches and applies the relay log, drains the outbox, advances transfers. Only one
    /// run at a time: a concurrent call marks the running one as dirty and returns. Then,
    /// outside the sync lock, runs the automatic rotation if it is due (`api-v1.md` §7.6).
    pub fn sync(&self) -> Result<SyncReport> {
        let r = self.sync_serial();
        self.run_due_rotation();
        r
    }

    fn sync_serial(&self) -> Result<SyncReport> {
        self.active()?;
        let Some(_guard) = self.try_serial(&self.sync_lock) else {
            self.sync_dirty.store(true, Ordering::SeqCst);
            return Ok(SyncReport {
                already_running: true,
                ..SyncReport::default()
            });
        };
        // At most one catch-up per operation and per call (`stale_key_epoch`, outbox.rs).
        lock(&self.resynced_ops)?.clear();
        let mut report = SyncReport::default();
        for _round in 0..MAX_SYNC_ROUNDS {
            self.sync_dirty.store(false, Ordering::SeqCst);
            self.dirty_again.store(false, Ordering::SeqCst);
            if let Err(e) = self.sync_once(&mut report) {
                return match e {
                    CoreError::Network { .. } => {
                        report.offline = true;
                        report.outbox_pending = self.pending_count();
                        Ok(report)
                    }
                    other => Err(other),
                };
            }
            if !self.sync_dirty.load(Ordering::SeqCst) && !self.dirty_again.load(Ordering::SeqCst) {
                break;
            }
        }
        report.cursor = self.store.read(|db| db.cursor())?;
        report.outbox_pending = self.pending_count();
        report.rotation_required = lock(&self.flags)?.rotation_required;
        self.notify();
        Ok(report)
    }

    fn pending_count(&self) -> u32 {
        self.store
            .read(|db| db.pending_ops())
            .map(|v| u32::try_from(v.len()).unwrap_or(u32::MAX))
            .unwrap_or(0)
    }

    fn sync_once(&self, report: &mut SyncReport) -> Result<()> {
        self.ensure_session()?;
        self.retry_update_required_if_updated()?;
        loop {
            let after = self.store.read(|db| db.cursor())?;
            let page = match self.authed(|| self.transport.events(after, 200)) {
                Ok(p) => p,
                Err(RelayError::Api(p))
                    if p.code == "cursor_expired" || p.code == "cursor_ahead" =>
                {
                    self.full_resync()?;
                    report.full_resync = true;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let items_before = self.store.read(|db| db.item_count())?;
            for ev in &page.events {
                if self.blocked() == Some(Blocked::Revoked) {
                    return Err(blocked_err("revoked"));
                }
                self.apply_event(ev)?;
                report.events_applied += 1;
            }
            let items_after = self.store.read(|db| db.item_count())?;
            report.new_items += items_after.saturating_sub(items_before);
            // The relay may skip compacted seqs: move to next_after.
            let progressed = self.store.tx(|db| {
                if page.next_after > db.cursor()? {
                    db.set_cursor(page.next_after)?;
                }
                Ok(db.cursor()? > after)
            })?;
            self.notify();
            if !page.has_more {
                break;
            }
            if !progressed {
                // `has_more` without any progress of the cursor would ask for the same page
                // forever (a buggy or hostile relay): stop here, the next sync tries again.
                self.store
                    .tx(|db| self.diag(db, "events_no_progress", &format!("after {after}")))?;
                break;
            }
        }
        let sent_before = self.pending_count();
        if self.drain_outbox()? {
            report.offline = true;
        }
        report.outbox_sent += sent_before.saturating_sub(self.pending_count());
        self.maintenance()?;
        self.step_transfers_quiet();
        self.refresh_devices_if_due();
        Ok(())
    }

    /// Reads the activity of the devices (`last_seen_at` of `GET /v1/devices`) again, and
    /// stores it, whatever the schedule. Only the activity of the devices already known is
    /// written: members, keys, states and names come from the log and from the trust store,
    /// never from this list (a device the log has not announced yet is left to its
    /// `device_added`). A relay that does not report the activity (no `active` in the record)
    /// changes nothing. Meant for the apps that open the list of devices and want it fresh;
    /// [`sync`](Self::sync) and the doorbell loop do it by themselves every few minutes.
    pub fn refresh_devices(&self) -> Result<()> {
        self.active()?;
        let devices = self.authed(|| self.transport.devices())?;
        let changed = self.store.tx(|db| store_relay_activity(db, &devices))?;
        let now = self.clock.now_ms();
        if let Ok(mut f) = lock(&self.flags) {
            f.devices_next_refresh_at_ms = now.saturating_add(DEVICES_REFRESH_EVERY_MS);
        }
        if changed {
            self.notify();
        }
        Ok(())
    }

    /// [`refresh_devices`](Self::refresh_devices) when the schedule says so, and nothing is
    /// blocked. Best effort: a failure (offline, relay error) is not one of the caller's, it is
    /// tried again in a minute.
    pub(crate) fn refresh_devices_if_due(&self) {
        let now = self.clock.now_ms();
        let due = lock(&self.flags)
            .map(|f| f.blocked.is_none() && now >= f.devices_next_refresh_at_ms)
            .unwrap_or(false);
        if !due || !self.is_provisioned() {
            return;
        }
        if self.refresh_devices().is_err() {
            if let Ok(mut f) = lock(&self.flags) {
                f.devices_next_refresh_at_ms = now.saturating_add(DEVICES_REFRESH_RETRY_MS);
            }
        }
    }

    /// `wake` from the WebSocket doorbell (§5.2): sync when the log grew past the cursor.
    pub fn handle_wake(&self, wake: Wake) -> Result<Option<SyncReport>> {
        let cursor = self.store.read(|db| db.cursor())?;
        if wake.seq > cursor {
            return self.sync().map(Some);
        }
        Ok(None)
    }

    /// Loop over a doorbell connection until it closes; returns the close code so the
    /// caller can apply [`Reconnect::for_close`](super::transport::Reconnect::for_close).
    pub fn run_wake_source(
        &self,
        source: &mut dyn super::transport::WakeSource,
        stop: &AtomicBool,
    ) -> super::transport::WakeClosed {
        // A catch-up right after `ready` (§5.1 step 4).
        let _ = self.sync();
        loop {
            // Stopped, or the device left the space meanwhile (nothing to sync any more).
            if stop.load(Ordering::SeqCst) || !self.is_provisioned() {
                return super::transport::WakeClosed { code: None };
            }
            match source.next() {
                Ok(Some(w)) => {
                    let _ = self.handle_wake(w);
                }
                Ok(None) => {}
                Err(closed) => return closed,
            }
            // The doorbell wakes up at least every 5 s: a due rotation does not wait for the
            // next `wake` (§7.6).
            self.run_due_rotation();
            // Nothing else reads the devices while the log stays quiet.
            self.refresh_devices_if_due();
        }
    }

    /// Keeps a doorbell connection open until `stop` is set (`api-v1.md` §5): connect,
    /// catch up, sync on every `wake`, and reconnect according to the close code with the
    /// full-jitter backoff of §5.5. Returns when `stop` is set or the device may no longer
    /// connect: revoked (`4003`, `403 revoked`), replaced (`4008`), or, after a `4004`, a fresh
    /// session that does not find the space. A `NotFound` met anywhere else (a sync, a start-up
    /// while the relay's volume is not mounted yet) does not end the loop: the relay is asked
    /// again with a fresh session so that the doorbell comes back by itself when the relay
    /// does (`api-v1.md` §5.4, §7.2). The wait doubles with every consecutive `NotFound` (60 s,
    /// 120 s, 240 s... at most 30 minutes, with jitter) and starts over as soon as a session
    /// opens; nothing is ever erased on this path. A `4004` followed by an unexpected answer
    /// (a `403` of a proxy, a `422`) counts as a transient failure: the loop goes on. Blocks:
    /// run it on a background thread.
    ///
    /// For the apps: a doorbell that has been told "not found" for a long time asks rarely (up
    /// to every 30 minutes) and keeps running. To bring it back sooner, when the app comes
    /// back to the foreground or the network returns, set the old loop's `stop` and start a
    /// new call with a new flag (see below: no need to wait for the old call to return); the
    /// first question is asked at once if the last answer is a minute old. The old loop
    /// returns once the catch-up sync it may be running is over. A wait that follows a "not found" also ends by itself, within 100 ms, as
    /// soon as the state is no longer "not found" (a session opened by a `sync` of the app, a
    /// device that left): the loop does not sleep on through an answer that is already
    /// obsolete.
    ///
    /// **Never two loops at the same time** (`api-v1.md` §5.5): the engine holds a gate for the
    /// whole life of a loop. A second call, while another loop runs, **does not return at once
    /// and does not connect**: it waits (it looks at the gate every 50 ms) until the first loop
    /// has returned, then runs in its turn. It gives up and returns without having connected
    /// if its own `stop` is set meanwhile. It does not stop the first loop: that is up to the
    /// caller, through the first loop's own `stop` flag. Each call must therefore have its own
    /// `stop` flag, and "stop then start again" is `stop.store(true)` on the old flag, then a
    /// call with a new flag, with no need to wait for the old call to return.
    pub fn run_doorbell(&self, stop: &AtomicBool) {
        let Some(_gate) = self.enter_doorbell(stop) else {
            return;
        };
        let mut attempt: u32 = 0;
        while !stop.load(Ordering::SeqCst) {
            let Some(wait) = self.doorbell_turn(stop, &mut attempt) else {
                return;
            };
            self.doorbell_pause(stop, &wait, &mut |ms| {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            });
        }
    }

    /// Takes `doorbell_gate`, waiting for the loop that holds it. `None` if `stop` is set
    /// before it is free.
    fn enter_doorbell(&self, stop: &AtomicBool) -> Option<MutexGuard<'_, ()>> {
        /// Counts the loops that wait for the gate, until this value is dropped.
        struct Waiting<'a>(&'a AtomicU32);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let mut waiting: Option<Waiting<'_>> = None;
        loop {
            match self.doorbell_gate.try_lock() {
                Ok(g) => return Some(g),
                Err(TryLockError::Poisoned(p)) => return Some(p.into_inner()),
                Err(TryLockError::WouldBlock) => {}
            }
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            if waiting.is_none() {
                self.doorbell_waiting.fetch_add(1, Ordering::SeqCst);
                waiting = Some(Waiting(&self.doorbell_waiting));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// The wait between two turns of the doorbell, in slices of 100 ms so that `stop` is
    /// honoured quickly; `sleep(ms)` waits for one slice. After a "not found" the wait also
    /// ends as soon as the state is no longer "not found": a session opened elsewhere, or the
    /// device erased. Checked before every slice, so a session opened between the end of the
    /// turn and the start of the wait is not missed.
    pub(crate) fn doorbell_pause(
        &self,
        stop: &AtomicBool,
        wait: &DoorbellWait,
        sleep: &mut dyn FnMut(u64),
    ) {
        let mut left = wait.ms;
        while left > 0 && !stop.load(Ordering::SeqCst) {
            if wait.after_not_found && self.blocked() != Some(Blocked::NotFound) {
                return;
            }
            let step = left.min(100);
            sleep(step);
            left -= step;
        }
    }

    /// [`doorbell_turn`](Self::doorbell_turn) reduced to the length of the wait.
    #[cfg(test)]
    pub(crate) fn doorbell_round(&self, stop: &AtomicBool, attempt: &mut u32) -> Option<u64> {
        self.doorbell_turn(stop, attempt).map(|w| w.ms)
    }

    /// One turn of [`run_doorbell`](Self::run_doorbell): session, connection, and everything
    /// until the connection closes. `None`: the doorbell ends for good. `Some(wait)`: wait
    /// that long ([`doorbell_pause`](Self::doorbell_pause)), then take another turn (`attempt`
    /// is the back-off counter of §5.5).
    pub(crate) fn doorbell_turn(
        &self,
        stop: &AtomicBool,
        attempt: &mut u32,
    ) -> Option<DoorbellWait> {
        use super::transport::{reconnect_ceiling_ms, Reconnect};
        let plain = |ms: u64| DoorbellWait {
            ms,
            after_not_found: false,
        };
        // Left the space (or never provisioned): there is nothing to ring for.
        let active = self.active().ok()?;
        let connected = self.ensure_session().and_then(|()| {
            self.transport.open_wake().map_err(|e| {
                if e.code() == Some("space_not_initialized") {
                    // Not found, in memory only (§7.2): asked again below, after a minute.
                    self.set_not_found(&active);
                }
                CoreError::from(e)
            })
        });
        let backoff = |attempt: &mut u32| {
            *attempt = attempt.saturating_add(1);
            jitter_ms(reconnect_ceiling_ms(*attempt - 1))
        };
        match connected {
            Ok(mut source) => {
                let started = std::time::Instant::now();
                let closed = self.run_wake_source(source.as_mut(), stop);
                if started.elapsed() >= std::time::Duration::from_secs(60) {
                    *attempt = 0;
                }
                match Reconnect::for_close(closed.code) {
                    Reconnect::Stop => {
                        if closed.code == Some(4003) {
                            self.set_blocked(&active, Blocked::Revoked);
                        }
                        None
                    }
                    Reconnect::Probe => {
                        if self.still_member_after_space_reset() {
                            Some(plain(backoff(attempt)))
                        } else {
                            None
                        }
                    }
                    Reconnect::RenewSession => {
                        self.drop_session();
                        Some(plain(0))
                    }
                    Reconnect::AfterRestart => Some(plain(1000 + jitter_ms(14_000))),
                    Reconnect::Backoff => Some(plain(backoff(attempt))),
                }
            }
            // Revoked: nothing to ring for until the app starts the doorbell again (the state
            // is asked again then).
            Err(_) if self.blocked() == Some(Blocked::Revoked) => None,
            // The relay does not find the space or the device. That proves nothing and may be
            // temporary (a volume not mounted yet): not a reason to give up, nor to erase
            // anything. The wait doubles with every consecutive answer (60 s, 120 s... at most
            // 30 minutes, with jitter) and starts over once a session opens, so that a space
            // that is really gone costs a few requests an hour instead of one every minute.
            // The wait ends early if the state stops being "not found" meanwhile.
            Err(_) if self.blocked() == Some(Blocked::NotFound) => {
                let streak = lock(&self.flags).map_or(1, |f| f.not_found_streak);
                Some(DoorbellWait {
                    ms: not_found_wait_ms(streak),
                    after_not_found: true,
                })
            }
            Err(_) => Some(plain(backoff(attempt))),
        }
    }

    /// After a `4004` (`space_reset`) of the doorbell, which proves nothing (`api-v1.md` §5.4,
    /// §7.9.1): a fresh session says whether to go on. `true` if the device is still a member,
    /// or if the relay could not be reached or answered something unexpected (a `4xx` of a
    /// proxy, a `422`): try again with the normal delay, as for a transient failure; `false` if
    /// it is revoked, not found or too old (the doorbell stops; nothing is erased).
    pub(crate) fn still_member_after_space_reset(&self) -> bool {
        use crate::runtime::leave::SessionProbe;
        let Ok(active) = self.active() else {
            return false;
        };
        matches!(
            self.fresh_session(&active),
            SessionProbe::Opened | SessionProbe::Transient(_) | SessionProbe::Unexpected
        )
    }

    // -----------------------------------------------------------------------
    // Event application (api-v1 §7.3)
    // -----------------------------------------------------------------------

    pub(crate) fn apply_event(&self, ev: &Event) -> Result<()> {
        let mut erased = Vec::new();
        self.with_crypto(|c, db, a| {
            // An honest relay serves strictly increasing `seq` numbers, all above the cursor
            // (`api-v1.md` §2.7, §7.3). An event at or below it is a replay or an injection:
            // the epoch ordering rule (`format-v1.md` §12) compares the `seq` of an item with
            // that of the rotation, so it must never be applied with a `seq` the relay picked
            // out of order.
            if ev.seq <= db.cursor()? {
                return self.diag(db, "event_seq_not_increasing", &format!("seq {}", ev.seq));
            }
            self.apply_in_tx(c, db, a, ev, &mut erased)?;
            db.set_cursor(ev.seq)?;
            Ok(())
        })?;
        // Only once the transaction has committed: were it to fail, the rows would stay and
        // the files must stay with them.
        for p in erased {
            let _ = std::fs::remove_file(self.store.file_path(&p));
        }
        Ok(())
    }

    pub(crate) fn diag(&self, db: &Db<'_>, kind: &str, detail: &str) -> Result<()> {
        db.diag(self.clock.now_ms(), kind, detail)
    }

    /// Applies one event inside the caller's transaction. Files that a tombstone makes
    /// obsolete are pushed to `erased` (relative paths): the caller removes them after the
    /// commit, never here, since a rollback must leave them in place.
    fn apply_in_tx(
        &self,
        c: &mut Crypto,
        db: &Db<'_>,
        a: &Active,
        ev: &Event,
        erased: &mut Vec<String>,
    ) -> Result<()> {
        let now = self.clock.now_ms();
        match &ev.data {
            EventData::Item {
                item_id,
                sender_device_id,
                created_at_ms,
                expires_at_ms,
                key_epoch,
                envelope,
                blobs,
                ring,
            } => {
                let mine = *sender_device_id == a.device_id;
                if db.is_tombstoned(item_id)? {
                    return Ok(());
                }
                if let Some(existing) = db.item(item_id)? {
                    // Idempotent. For my own item this is the relay's confirmation.
                    if mine && existing.origin == ItemOrigin::Sent {
                        db.set_item_seq(item_id, ev.seq)?;
                        db.set_send_status(item_id, SendStatus::Sent)?;
                    }
                    return Ok(());
                }
                let mut row = ItemRow {
                    item_id: item_id.clone(),
                    seq: ev.seq,
                    origin: if mine {
                        ItemOrigin::Sent
                    } else {
                        ItemOrigin::Received
                    },
                    sender_device_id: sender_device_id.clone(),
                    created_at_ms: *created_at_ms,
                    received_at_ms: now,
                    relay_expires_at_ms: Some(*expires_at_ms),
                    // The clear header of the envelope is authoritative (it is bound to the
                    // ciphertext); the relay's copy only stands in when the header cannot be
                    // read, and the item is unreadable then anyway.
                    key_epoch: read_envelope_header(envelope).map_or(*key_epoch, |h| h.key_epoch),
                    status: ItemStatus::Unreadable(UnreadableReason::Undecryptable),
                    sealed: None,
                    raw_envelope: Some(envelope.clone()),
                    payload_expires_at_ms: None,
                    secret: false,
                    read_policy: "until_expiry".into(),
                    read: mine,
                    kept: false,
                    blobs: blobs.clone(),
                    ring: *ring,
                    send_status: mine.then_some(SendStatus::Sent),
                };
                let opened = self.open_into_row(c, db, a, &mut row)?;
                db.put_item(&row)?;
                if let Some(item) = &opened {
                    store_files(db, a, &mut row, item, None)?;
                }
                if !mine {
                    // Ack now; release at once when there is nothing to fetch later.
                    let mut op = NewOp::new(OpKind::Ack);
                    op.item_id = Some(item_id.clone());
                    // An opened item releases at once when no file is left to fetch (no attachment, or
                    // all of them already removed); an unopened one, when it announces no blob.
                    op.flag = if opened.is_some() {
                        db.files(item_id)?
                            .iter()
                            .all(|f| f.state != FileState::Remote)
                    } else {
                        row.blobs.is_empty()
                    };
                    db.enqueue_op(&op, now)?;
                }
                Ok(())
            }
            EventData::Ack {
                item_id,
                device_id,
                received_at_ms,
            } => db.receipt_received(item_id, device_id, *received_at_ms),
            EventData::Read {
                item_id,
                device_id,
                read_at_ms,
            } => db.receipt_read(item_id, device_id, *read_at_ms),
            EventData::Tombstone {
                item_id, tombstone, ..
            } => {
                let verified = Tombstone::from_bytes(tombstone).and_then(|t| {
                    if t.item_id != *item_id || t.scope != DeletionScope::Everywhere {
                        return Err(CoreError::statement("tombstone does not match its event"));
                    }
                    c.trust.verify_tombstone(&t)?;
                    Ok(t)
                });
                match verified {
                    Ok(_) => {
                        erased.extend(Self::erase_item_everywhere(c, db, item_id, now)?);
                        Ok(())
                    }
                    Err(e) => self.diag(db, "tombstone_refused", e.code()),
                }
            }
            EventData::AttachmentTombstone { statement, .. } => {
                self.apply_attachment_tombstone_event(c, db, a, statement, erased)
            }
            EventData::ItemState {
                event_id,
                device_id,
                key_epoch,
                envelope,
            } => {
                if *device_id == a.device_id {
                    return db.finish_ops_by_event(event_id);
                }
                match c.ring.open_received_state(envelope, event_id, ev.seq) {
                    ReceivedItemState::Opened(opened) => {
                        if self.apply_opened_state(c, db, &opened.state)? {
                            Ok(())
                        } else {
                            // A state of a type this version does not know (spec §8.8.2,
                            // step 6): keep the raw envelope and replay it after an update.
                            db.put_pending_state(event_id, ev.seq, opened.key_epoch, envelope)
                        }
                    }
                    ReceivedItemState::Unreadable(
                        UnreadableReason::KeyPending | UnreadableReason::UpdateRequired,
                    ) => {
                        let epoch =
                            read_item_state_header(envelope).map_or(*key_epoch, |h| h.key_epoch);
                        db.put_pending_state(event_id, ev.seq, epoch, envelope)
                    }
                    ReceivedItemState::Unreadable(r) => {
                        self.diag(db, "item_state_unreadable", r.code())
                    }
                }
            }
            EventData::Rotation {
                key_epoch,
                rotation,
                ..
            } => {
                let parsed = KeyRotation::from_bytes(rotation).and_then(|r| {
                    if r.new_epoch <= c.trust.key_epoch() {
                        // Already applied (a rotation this device made itself, or a full
                        // resync): the position in the log is now known for sure.
                        if c.ring
                            .tighten_supersession(r.new_epoch.saturating_sub(1), ev.seq)
                        {
                            Self::persist_crypto(c, db)?;
                        }
                        return Ok(None);
                    }
                    c.trust.apply_rotation(&r, &a.keys).map(Some)
                });
                let parsed = match parsed {
                    Ok(None) => return Ok(()),
                    Ok(Some(o)) => Ok(o),
                    Err(e) => Err(e),
                };
                match parsed {
                    Ok(outcome) => {
                        let revoked_ids = outcome_revoked(&c.trust, db)?;
                        for id in revoked_ids {
                            db.set_device_state(&id, DeviceState::Revoked)?;
                        }
                        if outcome.self_revoked {
                            // A verified signed rotation lists this device as revoked: proof a
                            // relay cannot forge. The space keys are erased (`format-v1.md`
                            // §8.3 step 6); the local content, re-sealed under `LSK`, stays.
                            c.ring.erase_keys();
                            db.meta_set("blocked", b"revoked")?;
                            lock(&self.flags)
                                .map(|mut f| f.blocked = Some(Blocked::Revoked))
                                .ok();
                        } else if let Some(key) = outcome.new_space_key {
                            // A rotation now exists: whatever kept ours from completing is over.
                            db.meta_delete(ROTATION_STALLED_KEY)?;
                            c.ring.add_rotation_key(
                                key,
                                ev.seq,
                                u64::try_from(now).unwrap_or(0),
                            )?;
                            self.retry_key_pending(c, db, a)?;
                        }
                        Self::persist_crypto(c, db)
                    }
                    Err(e) => self.diag(
                        db,
                        "rotation_refused",
                        &format!("epoch {key_epoch}: {}", e.code()),
                    ),
                }
            }
            EventData::DeviceAdded { device } => {
                let cert = DeviceCert::from_bytes(&device.certificate);
                let state = match cert {
                    Ok(cert) if cert.is_self_signed() => {
                        if c.trust.is_member(&device.sign_pk) {
                            DeviceState::Verified
                        } else {
                            DeviceState::Unverified
                        }
                    }
                    Ok(cert) => match c.trust.accept_device_cert(&cert) {
                        Ok(()) => {
                            Self::persist_crypto(c, db)?;
                            DeviceState::Verified
                        }
                        Err(e) => {
                            self.diag(db, "device_cert_refused", e.code())?;
                            DeviceState::Unverified
                        }
                    },
                    Err(e) => {
                        self.diag(db, "device_cert_malformed", e.code())?;
                        DeviceState::Unverified
                    }
                };
                self.upsert_device(db, device, state)
            }
            EventData::DeviceUpdated { device_id, name } => db.set_device_name(device_id, name),
            EventData::DeviceRevoked { device_id, via } => {
                if via == "rotation" {
                    return Ok(());
                }
                if *device_id == a.device_id {
                    db.meta_set("blocked", b"revoked")?;
                    lock(&self.flags)
                        .map(|mut f| f.blocked = Some(Blocked::Revoked))
                        .ok();
                    return Ok(());
                }
                // The event is unsigned and its fields are the relay's, so nothing it names is
                // stored as it came (`api-v1.md` §7.6). Only the two values the relay API
                // defines are acted on, and only on a device this one already knows and has not
                // revoked yet; anything else changes nothing.
                if !RELAY_REVOCATION_VIAS.contains(&via.as_str()) {
                    self.diag(db, "device_revoked_ignored", "unknown via")?;
                    return Ok(());
                }
                let Some(row) = db.device(device_id)? else {
                    self.diag(db, "device_revoked_ignored", "unknown device")?;
                    return Ok(());
                };
                if matches!(
                    row.state,
                    DeviceState::Revoked | DeviceState::RevokedByRelay
                ) {
                    return Ok(());
                }
                db.set_device_state(device_id, DeviceState::RevokedByRelay)?;
                // The relay alone removed this device: the user is told (never a tap to give,
                // `take_relay_revocations`), whatever the honest rotation that follows. A
                // device that was never verified was never a member: nothing to tell.
                if row.state == DeviceState::Verified {
                    record_relay_revocation(db, device_id, via)?;
                }
                if let Ok(mut f) = lock(&self.flags) {
                    f.rotation_required = true;
                    // The automatic rotation (§7.6): drawn once, never pushed back.
                    f.rotation_due_at_ms
                        .get_or_insert_with(|| now.saturating_add(jitter_i64(30_000)));
                }
                Ok(())
            }
            EventData::HistoryTransfer {
                transfer_id,
                from_device_id,
                to_device_id,
                ..
            } => {
                if *to_device_id == a.device_id && db.transfer(transfer_id)?.is_none() {
                    // The transfer the inviter announced at pairing: the user already said yes
                    // (`pairing-v1.md` §9 step 5). Any other follows the usual rule.
                    let announced = crate::runtime::pairing::take_awaited_history(
                        db,
                        transfer_id,
                        from_device_id,
                    )?;
                    db.put_transfer(&crate::runtime::store::TransferRow::new(
                        transfer_id,
                        "import",
                        from_device_id,
                        if announced { "accepted" } else { "received" },
                        now,
                    ))?;
                }
                Ok(())
            }
            EventData::HistoryTransferClosed {
                transfer_id,
                outcome,
            } => {
                if let Some(mut t) = db.transfer(transfer_id)? {
                    t.updated_at_ms = now;
                    t.outcome = Some(outcome.clone());
                    if t.role == "export" && t.state == "posted" {
                        t.state = if outcome == "imported" {
                            "done"
                        } else {
                            "failed"
                        }
                        .into();
                        db.put_transfer(&t)?;
                        // The relay dropped the blobs: free the staging area.
                        db.delete_blob_uploads("transfer", transfer_id)?;
                        let _ = std::fs::remove_dir_all(
                            self.store.file_path(&format!("export/{transfer_id}")),
                        );
                    } else if t.role == "import" && !is_final_state(&t.state) {
                        t.state = "failed".into();
                        t.error = Some(format!("closed by the relay: {outcome}"));
                        db.put_transfer(&t)?;
                    }
                }
                Ok(())
            }
            EventData::Unknown { kind }
                if kind.starts_with(super::transport::MALFORMED_EVENT_PREFIX) =>
            {
                self.diag(db, "event_malformed", kind)
            }
            EventData::Unknown { .. } => Ok(()),
        }
    }

    fn upsert_device(
        &self,
        db: &Db<'_>,
        d: &crate::runtime::transport::DeviceRecord,
        state: DeviceState,
    ) -> Result<()> {
        db.put_device(&DeviceRow {
            device_id: d.device_id.clone(),
            name: d.name.clone(),
            platform: d.platform.clone(),
            ed25519: Some(d.sign_pk.clone()),
            x25519: Some(d.kx_pk.clone()),
            state,
            joined_seq: d.joined_seq,
            // The activity is stored by `store_relay_activity` (authoritative list only): a
            // `device_added` snapshot is older than what is stored, and `put_device` keeps it.
            last_seen_at_ms: None,
        })
    }

    /// Decodes `row.raw_envelope` into `row` (readable: re-sealed under the local key and the
    /// raw envelope dropped; unreadable: raw envelope kept, never deleted).
    pub(crate) fn open_into_row(
        &self,
        c: &Crypto,
        db: &Db<'_>,
        a: &Active,
        row: &mut ItemRow,
    ) -> Result<Option<Item>> {
        let Some(envelope) = row.raw_envelope.clone() else {
            return Ok(None);
        };
        match c.ring.open_received(&envelope, &row.item_id, row.seq) {
            ReceivedItem::Readable(opened) => {
                // Age, by the authenticated `created_at`, at the time the item reached this
                // device: an honest relay cannot hold an item longer than `max_ttl`, so a much
                // older one is a deleted or consumed item served again after its tombstone was
                // pruned. Kept raw, never shown, never retried.
                let oldest = row.received_at_ms.saturating_sub(crate::item_max_age_ms(
                    self.max_ttl_secs.load(Ordering::SeqCst),
                ));
                if opened.item.created_at_ms < oldest {
                    row.status = ItemStatus::Unreadable(UnreadableReason::SuspectEpoch);
                    self.diag(db, "item_too_old", &row.item_id)?;
                    return Ok(None);
                }
                fill_readable(row, &opened.item, a)?;
                Ok(Some(opened.item))
            }
            ReceivedItem::Unreadable(reason) => {
                row.status = ItemStatus::Unreadable(reason);
                Ok(None)
            }
        }
    }

    /// Erases an item deleted everywhere: row, files rows, queued operations, pin; records
    /// the tombstone. Returns the relative paths of the files to remove after the commit.
    ///
    /// Item states kept raw in `pending_states` are not touched: they are sealed, and one
    /// of a type this version does not know does not say which item it is about, so it
    /// cannot be matched here. Replayed after an update, a state of an erased item is
    /// ignored (tombstone) and dropped.
    pub(crate) fn erase_item_everywhere(
        c: &mut Crypto,
        db: &Db<'_>,
        item_id: &str,
        now: i64,
    ) -> Result<Vec<String>> {
        let mut paths = db.delete_item(item_id)?;
        paths.extend(Self::drop_item_ops(db, item_id)?);
        db.put_tombstone(item_id, now, true, now)?;
        c.pins.forget(item_id);
        db.delete_pin(item_id)?;
        c.archives.forget(item_id);
        db.delete_archive(item_id)?;
        Ok(paths)
    }

    /// Deletes the queued operations of an item and the rows of the blobs staged for them.
    /// Returns the relative paths of the staged files (`files/{item}_{idx}.enc`, the
    /// encrypted copy at rest that doubles as the blob to upload): the caller removes them
    /// after the commit, like the other files of the item.
    ///
    /// Unlike `remove_item_locally` (actions.rs), no `delete_blob` goes to the relay for a
    /// part that was already uploaded: the relay purges orphan blobs within 24 h.
    fn drop_item_ops(db: &Db<'_>, item_id: &str) -> Result<Vec<String>> {
        let mut staged = Vec::new();
        for op in db.ops_for_item(item_id)? {
            let owner = op.op_id.to_string();
            staged.extend(db.blob_uploads("op", &owner)?.into_iter().map(|b| b.path));
            db.delete_blob_uploads("op", &owner)?;
        }
        db.delete_ops_for_item(item_id)?;
        Ok(staged)
    }

    /// Applies an opened state. Returns `false` only for a state of an unknown type (kept
    /// raw by the caller to be replayed after an update, spec §8.8.2 step 6); `true` when
    /// the event is consumed (applied, refused or ignored).
    fn apply_opened_state(
        &self,
        c: &mut Crypto,
        db: &Db<'_>,
        state: &ItemStateKind,
    ) -> Result<bool> {
        match state {
            ItemStateKind::Pin(p) => {
                if db.is_tombstoned(&p.item_id)? {
                    return Ok(true);
                }
                match c.pins.apply_statement(&c.trust, p) {
                    Ok(_) => {
                        if let Some(s) = c.pins.state(&p.item_id) {
                            db.put_pin(&p.item_id, s)?;
                        }
                    }
                    Err(e) => self.diag(db, "pin_refused", e.code())?,
                }
                Ok(true)
            }
            ItemStateKind::Archive(s) => {
                if db.is_tombstoned(&s.item_id)? {
                    return Ok(true);
                }
                match c.archives.apply_statement(&c.trust, s) {
                    Ok(_) => {
                        if let Some(st) = c.archives.state(&s.item_id) {
                            db.put_archive(&s.item_id, st)?;
                        }
                    }
                    Err(e) => self.diag(db, "archive_refused", e.code())?,
                }
                Ok(true)
            }
            ItemStateKind::Unknown => Ok(false),
        }
    }

    /// After a rotation: retry the `KeyPending` items and item states.
    pub(crate) fn retry_key_pending(&self, c: &mut Crypto, db: &Db<'_>, a: &Active) -> Result<()> {
        for mut row in db.items_in_state(ItemStatus::Unreadable(UnreadableReason::KeyPending))? {
            let opened = self.open_into_row(c, db, a, &mut row)?;
            if let Some(item) = opened {
                db.put_item(&row)?;
                store_files(db, a, &mut row, &item, Some(self.clock.now_ms()))?;
            } else if row.status != ItemStatus::Unreadable(UnreadableReason::KeyPending) {
                db.put_item(&row)?;
            }
        }
        self.replay_pending_states(c, db)
    }

    /// Replays the item states kept raw (unreadable format, or a type unknown to this
    /// version), oldest `seq` first. Those still unreadable or unknown stay in place.
    fn replay_pending_states(&self, c: &mut Crypto, db: &Db<'_>) -> Result<()> {
        for (event_id, seq, _epoch, envelope) in db.pending_states()? {
            match c.ring.open_received_state(&envelope, &event_id, seq) {
                ReceivedItemState::Opened(opened) => {
                    if self.apply_opened_state(c, db, &opened.state)? {
                        db.delete_pending_state(&event_id)?;
                    }
                }
                ReceivedItemState::Unreadable(
                    UnreadableReason::KeyPending | UnreadableReason::UpdateRequired,
                ) => {}
                ReceivedItemState::Unreadable(_) => db.delete_pending_state(&event_id)?,
            }
        }
        Ok(())
    }

    /// What identifies the code that reads the data: the same value means the same decoder,
    /// so replaying what it could not read would change nothing.
    pub(crate) fn decoder_version() -> String {
        format!("{}+f{}", env!("CARGO_PKG_VERSION"), FORMAT_MAX)
    }

    /// Called at every sync: replays what an older version could not read (spec §8.8.2,
    /// "after each update"), but only when the version of the core changed since the last
    /// replay. Between two updates nothing can have become readable, so a device holding
    /// an unknown state neither opens a write transaction nor decrypts everything again at
    /// each sync. [`Self::retry_update_required`] is the explicit, unconditional replay.
    fn retry_update_required_if_updated(&self) -> Result<()> {
        let current = Self::decoder_version();
        let seen = self
            .store
            .read(|db| db.meta_get_string(REPLAYED_VERSION_KEY))?;
        if seen.as_deref() == Some(current.as_str()) {
            return Ok(());
        }
        self.retry_update_required()?;
        // Recorded afterwards: a failed replay (error above) is attempted again.
        self.store
            .tx(|db| db.meta_set(REPLAYED_VERSION_KEY, current.as_bytes()))
    }

    /// Re-decodes `UpdateRequired` items and item states (§7.4), unconditionally.
    pub(crate) fn retry_update_required(&self) -> Result<()> {
        if !self.store.read(|db| db.has_update_required_work())? {
            return Ok(());
        }
        self.with_crypto(|c, db, a| {
            for mut row in
                db.items_in_state(ItemStatus::Unreadable(UnreadableReason::UpdateRequired))?
            {
                let opened = self.open_into_row(c, db, a, &mut row)?;
                if row.status != ItemStatus::Unreadable(UnreadableReason::UpdateRequired) {
                    db.put_item(&row)?;
                    if let Some(item) = opened {
                        store_files(db, a, &mut row, &item, Some(self.clock.now_ms()))?;
                    }
                }
            }
            // Item states kept raw: unreadable format, or a type unknown to the previous
            // version (spec §8.8.2 step 6). Anything this version now knows is applied.
            self.replay_pending_states(c, db)
        })
    }

    // -----------------------------------------------------------------------
    // Full resync (api-v1 §7.5)
    // -----------------------------------------------------------------------

    /// Rebuilds keys and devices from the relay and resets the cursor to its head.
    pub fn full_resync(&self) -> Result<()> {
        let space = self.authed(|| self.transport.space())?;
        let keys = self.authed(|| self.transport.keys())?;
        let devices = self.authed(|| self.transport.devices())?;
        let now = self.clock.now_ms();
        self.with_crypto(|c, db, a| {
            let mut rotations = keys.rotations.clone();
            rotations.sort_by_key(|r| r.key_epoch);
            for r in rotations {
                if r.key_epoch <= c.trust.key_epoch() {
                    continue;
                }
                let applied = KeyRotation::from_bytes(&r.rotation)
                    .and_then(|rot| c.trust.apply_rotation(&rot, &a.keys));
                match applied {
                    Ok(outcome) => {
                        if outcome.self_revoked {
                            // Verified signed proof of the revocation: erase the space keys.
                            c.ring.erase_keys();
                            db.meta_set("blocked", b"revoked")?;
                            lock(&self.flags)
                                .map(|mut f| f.blocked = Some(Blocked::Revoked))
                                .ok();
                            break;
                        }
                        if let Some(key) = outcome.new_space_key {
                            // Nothing the log serves from now on can sit below the head: a
                            // `seq` above it (a relay's lie) must not widen the window.
                            c.ring.add_rotation_key(
                                key,
                                r.seq.min(space.head_seq),
                                u64::try_from(now).unwrap_or(0),
                            )?;
                            self.retry_key_pending(c, db, a)?;
                        }
                    }
                    Err(e) => self.diag(db, "rotation_refused", e.code())?,
                }
            }
            let known: Vec<String> = devices.iter().map(|d| d.device_id.clone()).collect();
            for d in &devices {
                let state = match DeviceCert::from_bytes(&d.certificate) {
                    Ok(cert) if cert.is_self_signed() && c.trust.is_member(&d.sign_pk) => {
                        DeviceState::Verified
                    }
                    Ok(cert) if !cert.is_self_signed() => match c.trust.accept_device_cert(&cert) {
                        Ok(()) => DeviceState::Verified,
                        Err(_) if c.trust.is_member(&d.sign_pk) => DeviceState::Verified,
                        Err(e) => {
                            self.diag(db, "device_cert_refused", e.code())?;
                            DeviceState::Unverified
                        }
                    },
                    _ => DeviceState::Unverified,
                };
                self.upsert_device(db, d, state)?;
            }
            store_relay_activity(db, &devices)?;
            // A verified device that is missing from the relay's list is NOT marked as revoked by
            // the relay: that mark makes the honest devices sign a rotation that revokes the
            // device for good (`format-v1.md` §8.4), and the absence from a list is only the
            // relay's word, which it can make up at will. Only an explicit `device_revoked`
            // event does (`apply_in_tx`), and a signed rotation of a member does it for real.
            let missing: Vec<String> = db
                .devices()?
                .iter()
                .filter(|r| r.state == DeviceState::Verified && !known.contains(&r.device_id))
                .map(|r| r.device_id.clone())
                .collect();
            if !missing.is_empty() {
                self.diag(db, "devices_missing_from_list", &missing.len().to_string())?;
            }
            // Exception, only when the relay itself says a rotation is owed (the condition of
            // the automatic rotation, `api-v1.md` §7.6): the rotation must list exactly the
            // members the relay keeps, so a device it no longer lists can never be sealed for,
            // and keeping it as a member would make the rotation fail with `device_set_mismatch`
            // for good (it happens when the device left or was removed while this one was away
            // for longer than the log is kept, and nobody rotated meanwhile). Such a device is
            // then treated like an unsigned `device_revoked`: marked as removed by the relay
            // (excluded from the rotation, reported to the user). That gives the relay nothing
            // it did not have (it can refuse service to any device); no key is erased. The list
            // must look sane: an empty one, or one that does not even contain this device, is
            // a failure of the relay, not "everyone was removed".
            if space.rotation_required && !known.is_empty() && known.contains(&a.device_id) {
                let mut removed = false;
                for id in missing.iter().filter(|id| **id != a.device_id) {
                    db.set_device_state(id, DeviceState::RevokedByRelay)?;
                    record_relay_revocation(db, id, "admin")?;
                    removed = true;
                }
                if removed {
                    self.diag(db, "devices_missing_removed", "rotation owed")?;
                    if let Ok(mut f) = lock(&self.flags) {
                        f.rotation_required = true;
                        f.rotation_due_at_ms
                            .get_or_insert_with(|| now.saturating_add(jitter_i64(30_000)));
                    }
                }
            }
            // The cursor never goes back, except when the relay's log itself went back (a
            // restore from an older backup, `cursor_ahead`): there is no other way to read what
            // it will publish next. The `seq` numbers can no longer be compared with the
            // rotations then, so every superseded epoch becomes final: an item sealed under
            // one of them that is first seen from now on is suspect, whatever its `seq`
            // (a relay that rewinds its log gains nothing against the epoch ordering rule).
            let cursor = db.cursor()?;
            if space.head_seq < cursor {
                c.ring.freeze_supersessions();
                self.diag(
                    db,
                    "relay_log_went_back",
                    &format!("cursor {cursor}, head {}", space.head_seq),
                )?;
            }
            db.set_cursor(space.head_seq)?;
            // Also persists the ring (frozen supersessions included).
            Self::persist_crypto(c, db)
        })?;
        if let Ok(mut f) = lock(&self.flags) {
            f.resync_notice = true;
        }
        Ok(())
    }

    /// Whether the removal of a device is **not finished**: the automatic rotation that must
    /// follow a revocation (`api-v1.md` §7.6) failed with `device_set_mismatch` (the relay lists a
    /// member that this device's trust store does not know yet), and is tried again every hour.
    /// Until it succeeds, the revoked device may still hold the current space key: the app shows a
    /// persistent warning. Stored (it survives a restart); cleared by any rotation that succeeds or
    /// that another member applied.
    pub fn rotation_stalled(&self) -> bool {
        self.store
            .read(|db| db.meta_get(ROTATION_STALLED_KEY))
            .ok()
            .flatten()
            .is_some()
    }

    /// Sets or clears the persisted `rotation_stalled` flag. Returns whether it changed.
    pub(crate) fn set_rotation_stalled(&self, stalled: bool) -> bool {
        let was = self.rotation_stalled();
        if was == stalled {
            return false;
        }
        let _ = self.store.tx(|db| {
            if stalled {
                db.meta_set(ROTATION_STALLED_KEY, b"1")
            } else {
                db.meta_delete(ROTATION_STALLED_KEY)
            }
        });
        true
    }

    /// The devices the relay removed on its own authority (an unsigned `device_revoked` of the
    /// log: `admin`, or a departure it reports), not yet shown; clears the list. The honest
    /// devices then sign the rotation that makes the removal real (`api-v1.md` §7.6) without
    /// asking anyone, so the app should tell the user plainly: "X was removed by the relay's
    /// administrator". Persisted until taken, so a restart loses nothing.
    pub fn take_relay_revocations(&self) -> Result<Vec<RelayRevocation>> {
        self.store.tx(|db| {
            let text = db
                .meta_get_string(RELAY_REVOCATIONS_KEY)?
                .unwrap_or_default();
            if text.is_empty() {
                return Ok(Vec::new());
            }
            db.meta_delete(RELAY_REVOCATIONS_KEY)?;
            let mut out = Vec::new();
            for line in text.lines() {
                // Stored lines are re-checked: only a known device and one of the two `via`
                // values ever reach the apps.
                let Some((id, via)) = line.rsplit_once(':') else {
                    continue;
                };
                let Some(via) = RELAY_REVOCATION_VIAS.iter().find(|v| **v == via) else {
                    continue;
                };
                let Some(row) = db.device(id)? else {
                    continue;
                };
                out.push(RelayRevocation {
                    device_id: id.to_owned(),
                    name: display_name(&row.name, id),
                    via: (*via).to_owned(),
                });
            }
            Ok(out)
        })
    }

    /// Whether the "older items could not be recovered" notice is pending; clears it.
    pub fn take_resync_notice(&self) -> bool {
        lock(&self.flags)
            .map(|mut f| std::mem::take(&mut f.resync_notice))
            .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Local housekeeping (api-v1 §7.3, "periodic task")
    // -----------------------------------------------------------------------

    /// Erases items past their explicit expiration (pinned or not), forgets old
    /// tombstones, discards old space keys.
    pub fn maintenance(&self) -> Result<()> {
        let now = self.clock.now_ms();
        let max_ttl = self.limits().max_ttl_secs;
        let mut expired_any = false;
        let erased = self.with_crypto(|c, db, _a| {
            let mut erased = Vec::new();
            for row in db.items()? {
                let pinned = c.pins.is_pinned(&row.item_id);
                if crate::local_retention(
                    row.payload_expires_at_ms,
                    pinned,
                    row.received_at_ms,
                    now,
                    None,
                ) == crate::LocalRetention::EraseExpired
                {
                    expired_any = true;
                    erased.extend(db.delete_item(&row.item_id)?);
                    erased.extend(Self::drop_item_ops(db, &row.item_id)?);
                    db.put_tombstone(&row.item_id, now, false, now)?;
                    c.pins.forget(&row.item_id);
                    db.delete_pin(&row.item_id)?;
                    c.archives.forget(&row.item_id);
                    db.delete_archive(&row.item_id)?;
                }
            }
            db.prune_tombstones(now - crate::tombstone_retention_ms(max_ttl))?;
            db.prune_attachment_removals(now - ATTACHMENT_REMOVAL_RETENTION_MS)?;
            let needed = db.raw_envelope_epochs()?;
            let dropped = c.ring.discard_expired(
                u64::try_from(now).unwrap_or(0),
                crate::space_key_retention_ms(max_ttl),
                &needed,
            );
            if !dropped.is_empty() {
                db.set_keyring_records(&c.ring.to_records())?;
            }
            Ok(erased)
        })?;
        for p in erased {
            let _ = std::fs::remove_file(self.store.file_path(&p));
        }
        // What was just erased must not linger in the write-ahead log.
        self.store.truncate_wal_if_due(expired_any);
        Ok(())
    }
}

/// `meta` key: devices the relay removed on its own authority (`device_revoked` with `via` other
/// than `rotation`), one `device_id:via` per line, until the app has shown them.
pub(crate) const RELAY_REVOCATIONS_KEY: &str = "relay_revocations";

/// A device the relay removed on its own authority (an administrator, or a departure the relay
/// reports): the log says so, but no member signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayRevocation {
    pub device_id: String,
    /// Name of the device as this device knows it (its id when unknown).
    pub name: String,
    /// `admin` (the relay's administrator) or `self` (the device left on its own, as far as the
    /// relay says).
    pub via: String,
}

/// The `via` values of an unsigned `device_revoked` that start a relay revocation (`rotation` is
/// the signed one, already applied by the rotation itself).
const RELAY_REVOCATION_VIAS: [&str; 2] = ["admin", "self"];

/// At most this many notices wait for the app; the oldest are dropped.
const RELAY_REVOCATIONS_MAX: usize = 32;

/// Longest device name (in characters) handed to the apps in a notice.
const NOTICE_NAME_MAX_CHARS: usize = 64;

/// A device name made safe to show in a notice: control characters (new lines included) become
/// spaces, the length is bounded, and an empty name falls back to the id.
fn display_name(name: &str, id: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let out: String = cleaned.trim().chars().take(NOTICE_NAME_MAX_CHARS).collect();
    if out.is_empty() {
        id.chars().take(NOTICE_NAME_MAX_CHARS).collect()
    } else {
        out
    }
}

/// Remembers that the relay removed the (known, validated) device `device_id` on its own
/// authority, until the app has shown it. One entry per device, bounded: the oldest is dropped.
pub(crate) fn record_relay_revocation(db: &Db<'_>, device_id: &str, via: &str) -> Result<()> {
    debug_assert!(RELAY_REVOCATION_VIAS.contains(&via));
    let text = db
        .meta_get_string(RELAY_REVOCATIONS_KEY)?
        .unwrap_or_default();
    let mut lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    let prefix = format!("{device_id}:");
    if lines.iter().any(|l| l.starts_with(&prefix)) {
        return Ok(());
    }
    let line = format!("{device_id}:{via}");
    lines.push(&line);
    let skip = lines.len().saturating_sub(RELAY_REVOCATIONS_MAX);
    let mut out = String::new();
    for l in &lines[skip..] {
        out.push_str(l);
        out.push('\n');
    }
    db.meta_set(RELAY_REVOCATIONS_KEY, out.as_bytes())
}

/// Stores the activity (`last_seen_at`) the relay reports for the devices already known.
/// `true` when a value changed. A record without `active` comes from a relay that does not
/// report the activity: it says nothing (not even "never connected"), nothing is written.
/// With `active`, an absent `last_seen_at` is the truth ("never connected") and replaces any
/// older value.
fn store_relay_activity(
    db: &Db<'_>,
    devices: &[crate::runtime::transport::DeviceRecord],
) -> Result<bool> {
    let mut changed = false;
    for d in devices.iter().filter(|d| d.active.is_some()) {
        let Some(row) = db.device(&d.device_id)? else {
            continue;
        };
        if row.last_seen_at_ms != d.last_seen_at_ms {
            db.set_device_last_seen(&d.device_id, d.last_seen_at_ms)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// [`jitter_ms`] for a delay added to a time of the engine clock.
pub(crate) fn jitter_i64(max_ms: i64) -> i64 {
    i64::try_from(jitter_ms(u64::try_from(max_ms).unwrap_or(0))).unwrap_or(0)
}

/// Uniform random delay in `[0, max_ms]`.
pub(crate) fn jitter_ms(max_ms: u64) -> u64 {
    let r = crate::crypto::random_array::<4>().map_or(u32::MAX, u32::from_le_bytes);
    max_ms * u64::from(r) / u64::from(u32::MAX)
}

/// What the engine makes of an error of `POST /v1/auth/challenge`. `403 revoked` is known to
/// `api-v1.md` §7.9.1 from `POST /v1/auth/session` and from the routes B only: from the
/// challenge route it is not a proof of anything (nor a reason to block the device), so it
/// becomes an unexpected answer. Everything else is left as it is.
fn challenge_error(e: RelayError) -> RelayError {
    match e {
        RelayError::Api(p) if p.code == "revoked" => RelayError::api(p.status, "unexpected_answer"),
        other => other,
    }
}

pub(crate) fn blocked_err(reason: &str) -> CoreError {
    CoreError::Blocked {
        reason: reason.into(),
    }
}

pub(crate) fn is_final_state(state: &str) -> bool {
    matches!(state, "done" | "failed" | "cancelled" | "declined")
}

/// Device ids that the trust store now lists as revoked.
fn outcome_revoked(trust: &TrustStore, db: &Db<'_>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for d in db.devices()? {
        if let Some(ed) = &d.ed25519 {
            if trust.is_revoked(ed) && d.state != DeviceState::Revoked {
                out.push(d.device_id);
            }
        }
    }
    Ok(out)
}

/// Fills the readable fields of `row` from an opened item and re-seals it under `LSK`.
pub(crate) fn fill_readable(row: &mut ItemRow, item: &Item, a: &Active) -> Result<()> {
    row.sealed = Some(a.lsk.seal_item(item)?);
    row.raw_envelope = None;
    row.status = ItemStatus::Readable;
    row.secret = item.secret;
    row.payload_expires_at_ms = item.expires_at_ms;
    row.read_policy = match item.read_policy {
        ReadPolicy::UntilExpiry => "until_expiry",
        ReadPolicy::AfterFirstRead => "after_first_read",
    }
    .into();
    Ok(())
}

/// Creates the `item_files` rows of a readable item (attachments not yet downloaded), except
/// for the slots that were removed: marked in the payload, or recorded in the registry before
/// the item could be opened (`format-v1.md` §8.10). For those the local payload is emptied
/// and re-sealed, the relay's blob list forgets them, and `row` is written again.
///
/// Returns whether a received item, some of whose slots were removed, has nothing left to
/// fetch: the relay may then purge it (`release`). With `release_at` (the clock) that release is
/// queued here; the arrival of an item passes `None` and folds it into its own ack.
pub(crate) fn store_files(
    db: &Db<'_>,
    a: &Active,
    row: &mut ItemRow,
    item: &Item,
    release_at: Option<i64>,
) -> Result<bool> {
    let removed = crate::runtime::removal::removed_slots(db, item, &[])?;
    let blob_ids: Vec<&str> = removed
        .iter()
        .filter_map(|s| item.attachments.get(*s as usize))
        .map(|att| att.blob_id.as_str())
        .collect();
    let listed = row.blobs.len();
    row.blobs
        .retain(|b| !blob_ids.contains(&b.blob_id.as_str()));
    if removed.iter().any(|s| !item.removed_slots().contains(s)) {
        let next = crate::runtime::removal::emptied(item, &removed)?;
        row.sealed = Some(a.lsk.seal_item(&next)?);
        db.put_item(row)?;
    } else if row.blobs.len() != listed {
        db.put_item(row)?;
    }
    for (idx, att) in item.attachments.iter().enumerate() {
        let idx = u32::try_from(idx).map_err(|_| CoreError::input("too many attachments"))?;
        if removed.contains(&idx) || db.files(&item.id)?.iter().any(|f| f.idx == idx) {
            continue;
        }
        db.put_file(&FileRow {
            item_id: item.id.clone(),
            idx,
            blob_id: Some(att.blob_id.clone()),
            size: att.size,
            key: None,
            nonce_prefix: None,
            state: FileState::Remote,
            path: None,
        })?;
    }
    let nothing_to_fetch = row.origin == ItemOrigin::Received
        && !removed.is_empty()
        && db
            .files(&item.id)?
            .iter()
            .all(|f| f.state != FileState::Remote);
    if let (true, Some(now)) = (nothing_to_fetch, release_at) {
        crate::runtime::removal::queue_release(db, &item.id, now)?;
    }
    Ok(nothing_to_fetch)
}
