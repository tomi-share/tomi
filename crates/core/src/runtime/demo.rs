//! Demo mode of the apps (feature `demo`, `RustTomiCore::open_demo`).
//!
//! The real [`Engine`] of this device runs on an in-memory database and talks to the
//! in-memory relay double of the tests ([`super::mock`]), shared with two simulated devices,
//! "PC-Bureau" and "PC-Maison", which run the real engine too. Nothing leaves the process;
//! the demo files under `files_dir` are erased when the demo is dropped (and at the next
//! opening, after a crash).
//!
//! The demo space holds the demo feed ([`super::demo_data`]: the demo feed of the Android app on
//! top, a week of every kind of item with heavy files, then about three years of history),
//! written by those devices through the normal protocol (encryption, relay log, receipts, pins),
//! so a demo behaves like the product (undo of a send, "after first read", delivery states,
//! deletion everywhere, attachments downloaded on demand...) and measures the apps on a feed of
//! several years. The relay double keeps the items as long as the history goes back (no 30-day
//! purge in a demo), and the demo files stay on disk only once (about 45 MB after the opening,
//! more as attachments are opened). PC-Bureau catches up every two
//! seconds (items sent from here become "delivered" to it); PC-Maison stays offline.
//!
//! Pairing is simulated (the demo relay is the in-memory double, already founded by PC-Bureau):
//! any code of 4 characters or more (or any QR payload) is accepted, the verification code is
//! always [`DEMO_VERIFICATION_CODE`], the confirmation waits a moment for PC-Bureau to confirm
//! too, and "import the history" means receiving the demo feed. Creating the space takes
//! [`DEMO_INSTALL_TOKEN`] (any other token is refused like a wrong one) and starts from an
//! empty feed.
//!
//! Adding a device from this one is NOT simulated: the invitation runs the real pairing protocol
//! on the relay double, and a new device of the demo, "Tablette", joins with the code shown
//! ([`DemoWorld::answer_invitation`]): it claims the code a moment later, says hello, confirms a
//! moment after the verification code appears, and becomes a member of the space.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use zeroize::Zeroizing;

use super::demo_data::{self, Author, DemoEntry, DemoFile, Scale};
use super::demo_files;
use super::engine::{Clock, Engine, EngineConfig, Provision, SystemClock};
use super::mock::MockRelay;
use super::store::{DeviceState, Store};
use super::transport::{DeviceRecord, PostItem, RelayError};
use super::{DeviceView, DispatchOutcome, DraftAttachment, DraftText, OutgoingDraft};
use crate::error::{CoreError, Result};
use crate::item::{Item, ItemText, ReadPolicy, SourceFormat, Thumbnail};
use crate::{
    new_item_id, new_space_id, relay_expires_at_ms, seal_item, DeviceKeys, DevicePublic, SpaceKey,
    TrustStore, DEFAULT_MAX_TTL_SECS, FORMAT_MAX,
};

/// What the feed of a demo joined with its history holds.
#[cfg(test)]
pub(crate) fn expected_feed() -> demo_data::FeedCounts {
    demo_data::counts(Scale::DEFAULT)
}

/// Verification code shown by a demo pairing (the Android demo uses the same).
pub const DEMO_VERIFICATION_CODE: &str = "482913";

/// Install token that the demo accepts to "create" the space (8 groups of 4 characters of the
/// relay's alphabet, as a real one). Any other token is refused with `install_token_invalid`.
pub const DEMO_INSTALL_TOKEN: &str = "2345-6789-abcd-efgh-jkmn-pqrs-tvwx-yz23";
/// Validity of an invitation code once claimed, as the relay gives it (`api-v1.md` §6).
pub const DEMO_PAIRING_EXPIRES_MS: i64 = 120_000;

const OFFICE_NAME: &str = "PC-Bureau";
/// The new device that answers the invitations of this device ("Tablette", "Tablette 2"...).
const GUEST_NAME: &str = "Tablette";
/// The guest claims the code once its user has typed it or scanned the QR code (none in the tests).
const GUEST_JOIN_DELAY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(4)
};
/// The guest confirms the codes a moment after they appear on both screens (none in the tests).
const GUEST_CONFIRM_DELAY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_millis(1500)
};
const HOME_NAME: &str = "PC-Maison";
const MIN_CODE_LENGTH: usize = 4;
const DEMO_DAY_MS: i64 = 86_400_000;
/// Store of this device in the past, which writes the items "sent from this phone".
const PAST_SELF_DIR: &str = "peers/past-self";
/// A pairing takes a moment, as with a real relay (none in the tests).
const PAIRING_LATENCY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_millis(800)
};
/// PC-Bureau confirms the codes a moment after this device (none in the tests).
const PEER_CONFIRM_DELAY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_millis(2500)
};
const PEER_SYNC_PERIOD: Duration = Duration::from_secs(2);
const STOP_POLL: Duration = Duration::from_millis(100);
/// A bundled image up to this size serves as its own thumbnail when no `.thumb.jpg` exists.
const MAX_THUMBNAIL_BYTES: u64 = 128 * 1024;
/// Bundled files above this size are ignored (the per-file limit of the apps).
const MAX_ASSET_BYTES: u64 = 25 * 1024 * 1024;

/// How to open the demo.
#[derive(Debug, Clone)]
pub struct DemoConfig {
    /// Private directory of the demo files (staged and downloaded attachments), absolute.
    /// Its `this`, `peers` and `staging` subdirectories are erased at every opening and when
    /// the demo is dropped.
    pub files_dir: PathBuf,
    /// Bundled demo files of the app (`demo_paysage.jpg`, `demo_main.rs`...), if any.
    pub assets_dir: Option<PathBuf>,
    /// Platform of this device (`ios`, `android`...).
    pub platform: String,
    pub app_version: String,
    /// Name of this device in the demo space.
    pub device_name: String,
    /// `true`: this device already belongs to the space, with the demo feed. `false`: it
    /// starts unpaired and joins through the simulated pairing.
    pub start_paired: bool,
}

/// What [`DemoWorld::simulate_incoming`] makes PC-Bureau send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoIncoming {
    Link,
    Text,
    RichText,
    Image,
    File,
    Secret,
    Unreadable,
}

/// How the relay of the demo answers the next departures of this device (debug menus of the apps),
/// so that the screens of "Sign out" can be tried in every case. The departure itself runs
/// the real code of the core against the relay double.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoDeparture {
    /// The relay answers normally (and comes back from the other modes).
    Normal,
    /// No network: the departure stays pending, retried.
    Offline,
    /// The relay fails once (`503`): pending, retried.
    ServerError,
    /// The other devices left: this one is the last, "Delete space" is offered.
    LastDevice,
    /// The relay lost its data (`409 space_not_initialized`): "Space not found", "Erase anyway"
    /// is allowed.
    Reset,
    /// A relay that cannot delete a space (`404` on `DELETE /v1/space`); this device is the last one,
    /// so that "Delete space" is reached.
    OldRelay,
}

/// Answer of a simulated pairing request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemoPairing {
    /// The user compares `verification_code` with the other device, then confirms.
    AwaitingVerification {
        peer: DeviceView,
        verification_code: String,
        history_item_count: u32,
        /// Bytes of the history (texts and files), an indication for the screen.
        history_bytes: u64,
        /// Devices of the space before this one joins.
        member_count: u16,
        /// Validity of the code, from now.
        expires_in_ms: i64,
        /// The relay of the pairing, as the screen shows it (the one of the link or the field,
        /// else the demo relay).
        relay_authority: String,
    },
    /// Code too short, or empty QR payload.
    InvalidCode,
}

/// Wall clock that the demo moves into the past to write its history, then back to now.
/// It only ever moves forward.
#[derive(Debug, Default)]
struct DemoClock {
    offset_ms: AtomicI64,
}

impl DemoClock {
    fn set(&self, at_ms: i64) {
        self.offset_ms
            .store(at_ms - SystemClock.now_ms(), Ordering::SeqCst);
    }

    fn reset(&self) {
        self.offset_ms.store(0, Ordering::SeqCst);
    }
}

impl Clock for DemoClock {
    fn now_ms(&self) -> i64 {
        SystemClock
            .now_ms()
            .saturating_add(self.offset_ms.load(Ordering::SeqCst))
    }
}

/// What this device needs to join, kept until it does.
struct JoinMaterial {
    ed25519_seed: Zeroizing<Vec<u8>>,
    x25519_secret: Zeroizing<Vec<u8>>,
    trust_json: String,
    device_name: String,
}

/// A demo item written to the relay, with what this device does with it once it joins.
struct Seeded {
    item_id: String,
    /// Position in the demo list (pins are decided in this order).
    order: usize,
    from: Author,
    read: bool,
    archived: bool,
    pinned: bool,
    unreadable: bool,
}

/// The demo space: the relay double, this device and the two simulated ones.
pub struct DemoWorld {
    clock: Arc<DemoClock>,
    this: Arc<Engine>,
    office: Arc<Engine>,
    office_id: String,
    home_id: String,
    office_view: DeviceView,
    space_key: Zeroizing<Vec<u8>>,
    join_material: Mutex<Option<JoinMaterial>>,
    /// Set once this device has fully joined (history included or skipped): the worker
    /// only syncs it from then on, so a "no history" answer is never overtaken by a sync.
    joined: Arc<AtomicBool>,
    seeded: Vec<Seeded>,
    /// Bytes written by the history (texts and files).
    history_bytes: AtomicU64,
    pairing_pending: AtomicBool,
    /// Bumped by every pairing request and every cancel: a confirmation waiting for PC-Bureau only goes on while the
    /// pairing it confirms is still the current one.
    pairing_generation: AtomicU64,
    /// How long PC-Bureau takes to confirm ([`PEER_CONFIRM_DELAY`]; the tests change it).
    peer_confirm_delay_ms: AtomicU64,
    root: PathBuf,
    staging: PathBuf,
    assets_dir: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    app_version: String,
    /// The guests that joined the space ("Tablette"...), kept for the lifetime of the demo; the others
    /// end with their thread.
    guests: Arc<Mutex<Vec<Arc<Engine>>>>,
    /// Guests created so far (each gets a store of its own).
    guest_attempts: AtomicU64,
    /// The devices `simulate_departure` removed from the relay (to make this one the last): only
    /// these are brought back, never a device the user removed.
    departure_revoked: Mutex<Vec<String>>,
    /// Kept for the lifetime of the demo (the engines hold transports onto it).
    _relay: MockRelay,
    _home: Arc<Engine>,
}

impl std::fmt::Debug for DemoWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoWorld")
            .field("items", &self.seeded.len())
            .finish_non_exhaustive()
    }
}

fn trust_for(space_id: &str, own: &DevicePublic, all: &[DevicePublic]) -> Result<TrustStore> {
    let mut trust = TrustStore::new(space_id, own, 1)?;
    for p in all.iter().filter(|p| *p != own) {
        trust.add_verified_peer(p)?;
    }
    Ok(trust)
}

fn copy_keys(k: &DeviceKeys) -> Result<DeviceKeys> {
    let seed = Zeroizing::new(k.ed25519_seed());
    let x = Zeroizing::new(k.x25519_secret());
    DeviceKeys::from_secrets(&seed, &x)
}

/// Subdirectories of `files_dir` owned by the demo.
const DEMO_DIRS: [&str; 3] = ["this", "peers", "staging"];

fn remove_demo_dirs(root: &std::path::Path) -> Result<()> {
    for sub in DEMO_DIRS {
        let dir = root.join(sub);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    Ok(())
}

impl DemoWorld {
    /// Builds the demo space and writes its history. Blocks for a few seconds (thousands of
    /// encrypted items, tens of megabytes of files): call it off the main thread.
    pub fn open(config: DemoConfig) -> Result<Arc<Self>> {
        Self::open_scaled(config, Scale::DEFAULT)
    }

    fn open_scaled(config: DemoConfig, scale: Scale) -> Result<Arc<Self>> {
        let root = config.files_dir.clone();
        if !root.is_absolute() {
            return Err(CoreError::input(
                "the demo files directory must be absolute",
            ));
        }
        remove_demo_dirs(&root)?;
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging)?;

        let entries = demo_data::entries(scale);
        let oldest = entries.iter().map(|e| e.age_ms).max().unwrap_or(0);
        let now0 = SystemClock.now_ms();
        let clock = Arc::new(DemoClock::default());
        // Everything happens in the past, then the clock only moves forward.
        let start = now0 - oldest - 60_000;
        clock.set(start);

        let space_id = new_space_id()?;
        let space_key = SpaceKey::generate(1)?;
        let this_keys = DeviceKeys::generate()?;
        let office_keys = DeviceKeys::generate()?;
        let home_keys = DeviceKeys::generate()?;
        let publics = [this_keys.public(), office_keys.public(), home_keys.public()];
        let relay = MockRelay::new(&space_id, clock.clone());
        // The relay of the demo keeps its items for as long as the history goes back (a real
        // relay purges them after `max_ttl`, 30 days by default): the oldest items are still
        // there for this device to receive when it joins.
        let ttl_secs = u64::try_from((oldest + 60 * DEMO_DAY_MS) / 1000)
            .unwrap_or(0)
            .max(DEFAULT_MAX_TTL_SECS);
        relay.with(|s| s.limits.max_ttl_secs = ttl_secs);

        // PC-Bureau founded the space and vouches for every device. This device is
        // registered first so that it sees the registration (name, platform) of the others.
        let issued_at = u64::try_from(start).unwrap_or(0);
        for (keys, name, platform) in [
            (
                &this_keys,
                config.device_name.as_str(),
                config.platform.as_str(),
            ),
            (&office_keys, OFFICE_NAME, "windows"),
            (&home_keys, HOME_NAME, "windows"),
        ] {
            let public = keys.public();
            relay.add_device(DeviceRecord {
                device_id: keys.device_id(),
                name: name.into(),
                platform: platform.into(),
                sign_pk: public.ed25519.clone(),
                kx_pk: public.x25519.clone(),
                certificate: office_keys
                    .sign_device_cert(&space_id, &public, issued_at)?
                    .to_bytes()?,
                issuer_device_id: office_keys.device_id(),
                joined_seq: 0,
                registered_at_ms: start,
                last_seen_at_ms: None,
                active: None,
            });
        }

        let engine = |sub: &str, platform: &str| -> Result<Arc<Engine>> {
            let wrap = Zeroizing::new(crate::crypto::random_array::<32>()?);
            let store = Store::open_in_memory(&root.join(sub), &wrap[..])?;
            Ok(Arc::new(Engine::open(
                EngineConfig {
                    app_version: config.app_version.clone(),
                    platform: platform.into(),
                    auto_import: true,
                    default_relay_url: None,
                },
                store,
                relay.transport(),
                clock.clone(),
            )?))
        };
        let provision = |e: &Engine, keys: &DeviceKeys, name: &str| -> Result<()> {
            e.provision(Provision {
                keys: copy_keys(keys)?,
                trust: trust_for(&space_id, &keys.public(), &publics)?,
                space_key: SpaceKey::from_bytes(1, &Zeroizing::new(space_key.to_bytes()))?,
                device_name: name.into(),
            })
        };
        let office = engine("peers/office", "windows")?;
        provision(&office, &office_keys, OFFICE_NAME)?;
        let home = engine("peers/home", "windows")?;
        provision(&home, &home_keys, HOME_NAME)?;
        // This device in the past, for the items "sent from this phone"; it is dropped
        // after the history is written and this device later reads them from the relay.
        let past_self = engine(PAST_SELF_DIR, &config.platform)?;
        provision(&past_self, &this_keys, &config.device_name)?;
        let this = engine("this", &config.platform)?;

        let mut world = Self {
            clock: clock.clone(),
            this,
            office: office.clone(),
            office_id: office_keys.device_id(),
            home_id: home_keys.device_id(),
            office_view: DeviceView {
                id: office_keys.device_id(),
                name: OFFICE_NAME.into(),
                platform: "windows".into(),
                state: DeviceState::Verified,
                last_seen_at_ms: Some(now0 - 3 * 60_000),
                is_this_device: false,
            },
            space_key: Zeroizing::new(space_key.to_bytes()),
            join_material: Mutex::new(Some(JoinMaterial {
                ed25519_seed: Zeroizing::new(this_keys.ed25519_seed()),
                x25519_secret: Zeroizing::new(this_keys.x25519_secret()),
                trust_json: trust_for(&space_id, &this_keys.public(), &publics)?.to_json()?,
                device_name: config.device_name.clone(),
            })),
            joined: Arc::new(AtomicBool::new(false)),
            seeded: Vec::new(),
            history_bytes: AtomicU64::new(0),
            pairing_pending: AtomicBool::new(false),
            pairing_generation: AtomicU64::new(0),
            peer_confirm_delay_ms: AtomicU64::new(
                u64::try_from(PEER_CONFIRM_DELAY.as_millis()).unwrap_or(u64::MAX),
            ),
            root: root.clone(),
            staging,
            assets_dir: config.assets_dir.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            worker: Mutex::new(None),
            app_version: config.app_version.clone(),
            guests: Arc::new(Mutex::new(Vec::new())),
            guest_attempts: AtomicU64::new(0),
            departure_revoked: Mutex::new(Vec::new()),
            _relay: relay,
            _home: home.clone(),
        };

        // The history, oldest first, each item at its time in the past.
        let mut order: Vec<(usize, DemoEntry)> = entries.into_iter().enumerate().collect();
        order.sort_by_key(|(_, e)| std::cmp::Reverse(e.age_ms));
        let home_id = home_keys.device_id();
        let this_id = this_keys.device_id();
        for (i, entry) in order {
            clock.set(now0 - entry.age_ms);
            let (author, author_id) = match entry.from {
                Author::Office => (&office, &world.office_id),
                Author::Home => (&home, &home_id),
                Author::This => (&past_self, &this_id),
            };
            let item_id = if entry.unreadable {
                world.post_unreadable(author, author_id)?
            } else {
                world.send(author, &entry)?
            };
            if entry.home_catches_up {
                home.sync()?;
            }
            world.seeded.push(Seeded {
                item_id,
                order: i,
                from: entry.from,
                read: entry.read,
                archived: entry.archived,
                pinned: entry.pinned,
                unreadable: entry.unreadable,
            });
        }
        // PC-Bureau receives everything: its receipts make the items sent from here
        // "delivered" to it.
        office.sync()?;
        // Its encrypted copies of the files sent "from here" are of no use any more (this
        // device downloads them from the relay like any other): tens of megabytes less on disk.
        drop(past_self);
        let _ = std::fs::remove_dir_all(root.join(PAST_SELF_DIR));
        clock.reset();

        let world = Arc::new(world);
        if config.start_paired {
            world.join(true)?;
        }
        // The tests drive the devices themselves (and start the worker where they need it).
        if !cfg!(test) {
            world.start_worker()?;
        }
        Ok(world)
    }

    /// The engine of this device.
    pub fn engine(&self) -> Arc<Engine> {
        self.this.clone()
    }

    /// Simulated pairing with a short code: accepted from 4 characters.
    pub fn pair_with_code(&self, code: &str, relay_url: Option<&str>) -> Result<DemoPairing> {
        let authority = relay_url
            .map(crate::relay_url::relay_authority_of)
            .transpose()?;
        self.pair(code.trim().chars().count() >= MIN_CODE_LENGTH, authority)
    }

    /// Simulated pairing with a scanned QR payload: any non-empty payload is accepted.
    pub fn pair_with_qr(&self, payload: &str) -> Result<DemoPairing> {
        let authority = crate::relay_url::parse_pair_link(payload)
            .and_then(|l| l.relay_url)
            .map(|u| crate::relay_url::relay_authority_of(&u))
            .transpose()?;
        self.pair(!payload.trim().is_empty(), authority)
    }

    fn pair(&self, valid: bool, relay_authority: Option<String>) -> Result<DemoPairing> {
        std::thread::sleep(PAIRING_LATENCY);
        if self.this.is_provisioned() {
            return Err(CoreError::input("this device already belongs to a space"));
        }
        if !valid {
            return Ok(DemoPairing::InvalidCode);
        }
        self.pairing_generation.fetch_add(1, Ordering::SeqCst);
        self.pairing_pending.store(true, Ordering::SeqCst);
        Ok(DemoPairing::AwaitingVerification {
            peer: self.office_view.clone(),
            verification_code: DEMO_VERIFICATION_CODE.into(),
            history_item_count: u32::try_from(self.seeded.len()).unwrap_or(u32::MAX),
            history_bytes: self.history_bytes.load(Ordering::SeqCst),
            // PC-Bureau and PC-Maison.
            member_count: 2,
            expires_in_ms: DEMO_PAIRING_EXPIRES_MS,
            relay_authority: relay_authority.unwrap_or_else(|| {
                self.this
                    .relay_authority()
                    .unwrap_or_else(|_| "relay.example.org".into())
            }),
        })
    }

    /// The codes match: once PC-Bureau confirms too (a moment later), this device joins, with
    /// the demo feed or with an empty one. Without a pairing in progress (cancelled, or already
    /// confirmed) nothing happens. A [`cancel_pairing`](Self::cancel_pairing) from another
    /// thread during the wait ends it with `pairing_cancelled`, nothing joined.
    pub fn confirm_pairing(&self, import_history: bool) -> Result<()> {
        if !self.pairing_pending.load(Ordering::SeqCst) {
            return Ok(());
        }
        let generation = self.pairing_generation.load(Ordering::SeqCst);
        let current = || {
            self.pairing_pending.load(Ordering::SeqCst)
                && self.pairing_generation.load(Ordering::SeqCst) == generation
        };
        let mut waited = Duration::ZERO;
        while waited < self.peer_confirm_delay() {
            if !current() {
                return Err(CoreError::PairingCancelled);
            }
            std::thread::sleep(STOP_POLL);
            waited += STOP_POLL;
        }
        if !current() || !self.pairing_pending.swap(false, Ordering::SeqCst) {
            return Err(CoreError::PairingCancelled);
        }
        self.join(import_history)
    }

    /// Sets how the relay answers the next departures ([`DemoDeparture`]). Each mode first undoes
    /// what the previous one set (and only that). The faults are one-shot: "Transient error" fails
    /// the next departure request once, "Old relay" the next `DELETE /v1/space` once; choose the
    /// mode again for another try.
    pub fn simulate_departure(&self, mode: DemoDeparture) {
        let relay = &self._relay;
        relay.set_offline(false);
        relay.reinitialize();
        relay.clear_faults();
        let mut revoked = self
            .departure_revoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for id in revoked.drain(..) {
            relay.set_revoked(&id, false);
        }
        // "Old relay" only shows on "Delete space", which only the last device reaches.
        if matches!(mode, DemoDeparture::LastDevice | DemoDeparture::OldRelay) {
            let guests: Vec<String> = self
                .guests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter_map(|g| g.device_id().ok())
                .collect();
            for id in [self.office_id.clone(), self.home_id.clone()]
                .into_iter()
                .chain(guests)
            {
                relay.set_revoked(&id, true);
                revoked.push(id);
            }
        }
        drop(revoked);
        match mode {
            DemoDeparture::Normal | DemoDeparture::LastDevice => {}
            DemoDeparture::Offline => relay.set_offline(true),
            DemoDeparture::ServerError => {
                relay.fail_next("leave_space", RelayError::api(503, "unavailable"));
                relay.fail_next("delete_space", RelayError::api(503, "unavailable"));
            }
            DemoDeparture::Reset => relay.uninitialize(),
            DemoDeparture::OldRelay => {
                relay.fail_next("delete_space", RelayError::api(404, "not_found"));
            }
        }
    }

    /// A new device of the demo answers the invitation this device just started with `code`
    /// (the real protocol, on the relay double): on a thread of its own it claims the code, waits
    /// for the verification code, confirms (without the history) and joins. If this device cancels
    /// or refuses, the guest's calls fail and it simply stops. Each invitation gets its own guest
    /// ("Tablette", "Tablette 2"...).
    pub fn answer_invitation(&self, code: Zeroizing<String>) -> Result<()> {
        // Named after the guests that really joined: a refused or expired attempt leaves no "Tablette".
        let joined = self
            .guests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        let name = if joined == 0 {
            GUEST_NAME.to_owned()
        } else {
            format!("{GUEST_NAME} {}", joined + 1)
        };
        let attempt = self.guest_attempts.fetch_add(1, Ordering::SeqCst) + 1;
        let guest = self.guest_engine(&format!("peers/guest-{attempt}"))?;
        let guests = self.guests.clone();
        let stop = self.stop.clone();
        std::thread::Builder::new()
            .name("tomi-demo-guest".into())
            .spawn(move || {
                if !pause(&stop, GUEST_JOIN_DELAY) {
                    return;
                }
                let Ok(session) = guest.join_space(&code, &name) else {
                    return;
                };
                if session.wait_for_inviter().is_err() {
                    return;
                }
                if !pause(&stop, GUEST_CONFIRM_DELAY) {
                    session.cancel();
                    return;
                }
                if session.confirm(false).is_ok() && session.wait_done().is_ok() {
                    guests
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(guest);
                }
            })?;
        Ok(())
    }

    /// A device of the demo that is not in the space yet, on the relay double.
    fn guest_engine(&self, sub: &str) -> Result<Arc<Engine>> {
        let wrap = Zeroizing::new(crate::crypto::random_array::<32>()?);
        let store = Store::open_in_memory(&self.root.join(sub), &wrap[..])?;
        Ok(Arc::new(Engine::open(
            EngineConfig {
                app_version: self.app_version.clone(),
                platform: "android".into(),
                auto_import: true,
                default_relay_url: None,
            },
            store,
            self._relay.transport(),
            self.clock.clone(),
        )?))
    }

    fn peer_confirm_delay(&self) -> Duration {
        Duration::from_millis(self.peer_confirm_delay_ms.load(Ordering::SeqCst))
    }

    /// Creates the space from this device (the first member), with [`DEMO_INSTALL_TOKEN`]
    /// (dashes, blanks and case ignored): the feed starts empty. Another token is refused as
    /// a relay would (`403 install_token_invalid`), and an invalid name or a device already in
    /// the space with `InvalidInput`.
    pub fn create_space(&self, install_token: &str, device_name: &str) -> Result<()> {
        std::thread::sleep(PAIRING_LATENCY);
        let name = device_name.trim();
        if !crate::pairing::is_valid_device_name(name) {
            return Err(CoreError::input("invalid device name"));
        }
        if self.this.is_provisioned() {
            return Err(CoreError::input("this device already belongs to a space"));
        }
        let normalize = |t: &str| -> String {
            t.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .flat_map(char::to_lowercase)
                .collect()
        };
        // A simulated join left open is over: this device founds the space instead.
        self.cancel_pairing();
        if normalize(install_token) != normalize(DEMO_INSTALL_TOKEN) {
            return Err(CoreError::Relay {
                status: 403,
                code: "install_token_invalid".into(),
            });
        }
        self.join(false)?;
        // The demo registered this device under the configured name: the one typed here wins.
        let own = self
            .this
            .list_devices()?
            .into_iter()
            .find(|d| d.is_this_device);
        if let Some(own) = own.filter(|d| d.name != name) {
            self.this.rename_device(&own.id, name)?;
        }
        Ok(())
    }

    /// The codes differ or the user left: nothing is joined.
    pub fn cancel_pairing(&self) {
        self.pairing_generation.fetch_add(1, Ordering::SeqCst);
        self.pairing_pending.store(false, Ordering::SeqCst);
    }

    /// PC-Bureau sends an item of `kind` now, and this device receives it.
    pub fn simulate_incoming(&self, kind: DemoIncoming) -> Result<()> {
        if !self.this.is_provisioned() {
            return Err(CoreError::input("the demo device has not joined the space"));
        }
        let entry = demo_data::incoming(kind);
        if entry.unreadable {
            self.post_unreadable(&self.office, &self.office_id)?;
        } else {
            self.send(&self.office, &entry)?;
        }
        self.this.sync()?;
        Ok(())
    }

    fn join(&self, import_history: bool) -> Result<()> {
        let Some(m) = self
            .join_material
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return Ok(());
        };
        self.this.provision(Provision {
            keys: DeviceKeys::from_secrets(&m.ed25519_seed, &m.x25519_secret)?,
            trust: TrustStore::from_json(&m.trust_json)?,
            space_key: SpaceKey::from_bytes(1, &self.space_key)?,
            device_name: m.device_name,
        })?;
        if !import_history {
            // Start from an empty feed: skip the log, learn the devices.
            self.this.full_resync()?;
            let _ = self.this.take_resync_notice();
            self.joined.store(true, Ordering::SeqCst);
            return Ok(());
        }
        self.this.sync()?;
        let mut by_order: Vec<&Seeded> = self.seeded.iter().collect();
        by_order.sort_by_key(|s| s.order);
        for s in &by_order {
            if s.read && s.from != Author::This && !s.unreadable {
                self.this.mark_read(&s.item_id)?;
            }
            if s.archived {
                self.this.archive(&s.item_id)?;
            }
        }
        // Pins in list order, so the last one listed is the most recent decision.
        for s in by_order.iter().filter(|s| s.pinned) {
            std::thread::sleep(Duration::from_millis(2));
            self.this.pin(&s.item_id)?;
        }
        self.this.sync()?;
        self.joined.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Writes `entry` through `author` (staging its files) and sends it now.
    fn send(&self, author: &Engine, entry: &DemoEntry) -> Result<String> {
        let mut attachments = Vec::new();
        let mut staged = Vec::new();
        for file in &entry.files {
            let (name, mime, data, thumbnail) = self.materialize(file);
            self.count_bytes(data.len());
            let path = self.staging.join(format!("{}-{name}", new_item_id()?));
            std::fs::write(&path, &data)?;
            attachments.push(DraftAttachment {
                path: path.to_string_lossy().into_owned(),
                name,
                mime,
                thumbnail,
            });
            staged.push(path);
        }
        let text_len = entry.title.as_ref().map_or(0, |t| t.len())
            + entry.url.as_ref().map_or(0, |u| u.len())
            + entry.text.as_ref().map_or(0, |t| t.plain.len());
        self.count_bytes(text_len);
        let now = self.clock.now_ms();
        let draft = OutgoingDraft {
            id: None,
            title: entry.title.as_ref().map(|t| t.to_string()),
            text: entry.text.as_ref().map(|t| DraftText {
                plain: t.plain.to_string(),
                html: t.html.as_ref().map(|h| h.to_string()),
                markdown: t.markdown.as_ref().map(|m| m.to_string()),
                source_format: t.source_format,
            }),
            url: entry.url.as_ref().map(|u| u.to_string()),
            attachments,
            secret: entry.secret,
            expires_at_ms: entry.expires_after_ms.map(|d| now + d),
            read_policy: if entry.after_first_read {
                ReadPolicy::AfterFirstRead
            } else {
                ReadPolicy::UntilExpiry
            },
        };
        let queued = author.enqueue(draft);
        for path in staged {
            let _ = std::fs::remove_file(path);
        }
        let id = queued?;
        match author.dispatch(&id)? {
            DispatchOutcome::Sent => Ok(id),
            other => Err(CoreError::Internal {
                reason: format!("demo item {} not accepted: {other:?}", entry.key),
            }),
        }
    }

    fn count_bytes(&self, len: usize) {
        self.history_bytes
            .fetch_add(u64::try_from(len).unwrap_or(0), Ordering::SeqCst);
    }

    /// Posts, as `author`, an item sealed in a format this build does not know: every
    /// device keeps it as "update required" (DEV.md invariant 7).
    fn post_unreadable(&self, author: &Engine, author_id: &str) -> Result<String> {
        // The session first: it brings the limits of the relay (its retention).
        author.ensure_session()?;
        let key = SpaceKey::from_bytes(1, &self.space_key)?;
        let now = self.clock.now_ms();
        let item = Item {
            id: new_item_id()?,
            source_device: author_id.into(),
            created_at_ms: now,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            secret: false,
            title: None,
            text: Some(ItemText {
                plain: "Élément d'une version future de Tomi.".into(),
                html: None,
                markdown: None,
                source_format: SourceFormat::Plain,
            }),
            url: None,
            attachments: Vec::new(),
            raw_payload: None,
        };
        let mut envelope = seal_item(&item, &key)?;
        if let Some(format) = envelope.get_mut(..4) {
            format.copy_from_slice(&(FORMAT_MAX + 1).to_be_bytes());
        }
        let request = PostItem {
            item_id: item.id.clone(),
            expires_at_ms: relay_expires_at_ms(None, now, author.limits().max_ttl_secs),
            key_epoch: 1,
            envelope,
            blob_ids: Vec::new(),
        };
        author.authed(|| author.transport.post_item(&request))?;
        Ok(item.id)
    }

    /// Name, type, bytes and thumbnail of a demo file. A bundled file the app did not
    /// provide is replaced by a built-in stand-in (never an error).
    fn materialize(&self, file: &DemoFile) -> (String, String, Vec<u8>, Option<Thumbnail>) {
        match file {
            &DemoFile::Asset { name, mime } => {
                if let Some(data) = self.asset(name) {
                    let thumbnail = mime
                        .starts_with("image/")
                        .then(|| self.thumbnail(name, mime, &data))
                        .flatten();
                    return (name.into(), mime.into(), data, thumbnail);
                }
                if mime.starts_with("image/") {
                    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
                    let png = demo_data::FALLBACK_PNG.to_vec();
                    let thumbnail = Thumbnail {
                        mime: "image/png".into(),
                        data: png.clone(),
                    };
                    return (
                        format!("{stem}.png"),
                        "image/png".into(),
                        png,
                        Some(thumbnail),
                    );
                }
                let code = demo_data::fallback_code(name).unwrap_or("Démo Tomi\n");
                (name.into(), mime.into(), code.as_bytes().to_vec(), None)
            }
            DemoFile::Pdf { name, title } => (
                name.to_string(),
                "application/pdf".into(),
                demo_files::pdf(title),
                None,
            ),
            DemoFile::Text {
                name,
                mime,
                content,
            } => (
                name.to_string(),
                mime.to_string(),
                content.as_bytes().to_vec(),
                None,
            ),
            DemoFile::Generated { name, file } => {
                let g = file.generate(self.clock.now_ms());
                (name.to_string(), g.mime.into(), g.data, g.thumbnail)
            }
        }
    }

    /// `<stem>.thumb.jpg` next to the image if the app bundles one, else the image itself
    /// when it is small enough.
    fn thumbnail(&self, name: &str, mime: &str, data: &[u8]) -> Option<Thumbnail> {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        // A thumbnail travels inside the envelope (1 MiB at most): never a large file.
        if let Some(thumb) = self
            .asset(&format!("{stem}.thumb.jpg"))
            .filter(|t| u64::try_from(t.len()).unwrap_or(u64::MAX) <= MAX_THUMBNAIL_BYTES)
        {
            return Some(Thumbnail {
                mime: "image/jpeg".into(),
                data: thumb,
            });
        }
        (u64::try_from(data.len()).unwrap_or(u64::MAX) <= MAX_THUMBNAIL_BYTES).then(|| Thumbnail {
            mime: mime.into(),
            data: data.to_vec(),
        })
    }

    fn asset(&self, name: &str) -> Option<Vec<u8>> {
        if name.contains(['/', '\\']) || name.starts_with('.') {
            return None;
        }
        let path: PathBuf = self.assets_dir.as_deref().map(|d| d.join(name))?;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_ASSET_BYTES {
            return None;
        }
        std::fs::read(&path).ok()
    }

    /// PC-Bureau and this device catch up regularly (receipts, deliveries) until the demo
    /// is dropped. PC-Maison stays offline.
    fn start_worker(&self) -> Result<()> {
        let stop = self.stop.clone();
        let joined = self.joined.clone();
        let office = self.office.clone();
        let this = self.this.clone();
        let handle = std::thread::Builder::new()
            .name("tomi-demo".into())
            .spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let _ = office.sync();
                    if joined.load(Ordering::SeqCst) {
                        let _ = this.sync();
                    }
                    let mut waited = Duration::ZERO;
                    while waited < PEER_SYNC_PERIOD && !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(STOP_POLL);
                        waited += STOP_POLL;
                    }
                }
            })?;
        *self.worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        Ok(())
    }
}

/// Waits `delay` unless the demo stops meanwhile: `false` when it stopped.
fn pause(stop: &AtomicBool, delay: Duration) -> bool {
    let mut waited = Duration::ZERO;
    while waited < delay {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(STOP_POLL);
        waited += STOP_POLL;
    }
    !stop.load(Ordering::SeqCst)
}

impl Drop for DemoWorld {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let handle = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(h) = handle {
            // The last reference can go away on the worker itself (a listener fired by its
            // sync): it cannot join itself, and it stops on its own once `stop` is set.
            if h.thread().id() != std::thread::current().id() {
                let _ = h.join();
            }
        }
        // Best effort: the next opening erases them anyway.
        let _ = remove_demo_dirs(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{DeliveryState, ItemContentView, ItemOrigin};
    use crate::UnreadableReason;
    use std::path::Path;

    /// The Android demo assets, used when the repository has them.
    fn android_assets() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/android/app/src/debug/assets/demo");
        p.is_dir().then_some(p)
    }

    fn config(dir: &Path, start_paired: bool, assets: Option<PathBuf>) -> DemoConfig {
        DemoConfig {
            files_dir: dir.to_owned(),
            assets_dir: assets,
            platform: "ios".into(),
            app_version: "0.1.0".into(),
            device_name: "iPhone".into(),
            start_paired,
        }
    }

    fn titled<'a>(
        items: &'a [crate::runtime::ItemView],
        title: &str,
    ) -> &'a crate::runtime::ItemView {
        items
            .iter()
            .find(|v| matches!(&v.content, ItemContentView::Readable { title: Some(t), .. } if t == title))
            .unwrap_or_else(|| panic!("no item titled {title}"))
    }

    #[test]
    fn a_paired_demo_holds_the_android_demo_feed() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, android_assets())).unwrap();
        let e = world.engine();
        assert!(e.is_provisioned());
        let items = e.list_items().unwrap();
        let expected = expected_feed();
        assert_eq!(items.len(), expected.items);
        assert_eq!(
            items.iter().filter(|v| v.archived).count(),
            expected.archived
        );
        assert_eq!(items.iter().filter(|v| v.pinned).count(), expected.pinned);
        assert!(
            !e.take_resync_notice(),
            "no 'older items lost' notice in a demo"
        );

        // Newest first, with the times of the history.
        assert!(items
            .windows(2)
            .all(|w| w[0].created_at_ms >= w[1].created_at_ms));
        let link = titled(&items, "Material Design 3 dans Compose");
        assert_eq!(link.id, items[0].id);
        assert!(!link.read);
        assert!(titled(&items, "Wi-Fi invités").pinned);
        let devices = e.list_devices().unwrap();
        assert_eq!(devices.len(), 3);
        assert!(devices
            .iter()
            .any(|d| d.name == "PC-Bureau" && d.platform == "windows"));
        assert!(devices
            .iter()
            .any(|d| d.is_this_device && d.name == "iPhone"));

        // Sent from here: delivered to PC-Bureau; PC-Maison caught up only for the link.
        let sent_link = titled(&items, "Itinéraire du week-end");
        assert_eq!(sent_link.origin, ItemOrigin::Sent);
        assert!(sent_link
            .deliveries
            .iter()
            .all(|d| d.state == DeliveryState::Delivered));
        let sent_secret = items
            .iter()
            .filter(|v| v.origin == ItemOrigin::Sent && v.read_policy == ReadPolicy::AfterFirstRead)
            .max_by_key(|v| v.created_at_ms)
            .unwrap();
        let states: Vec<_> = sent_secret
            .deliveries
            .iter()
            .map(|d| (d.device_name.as_str(), d.state))
            .collect();
        assert!(states.contains(&("PC-Bureau", DeliveryState::Delivered)));
        assert!(states.contains(&("PC-Maison", DeliveryState::Sent)));

        // The unreadable one is kept as "update required".
        assert!(items.iter().any(|v| matches!(
            v.content,
            ItemContentView::Unreadable {
                reason: UnreadableReason::UpdateRequired
            }
        )));
        // The archived "after first read" secret was not consumed.
        let archived_secret = titled(&items, "Code du coffre (démo)");
        assert!(!archived_secret.read);

        // Attachments come down from the relay double and decrypt on demand.
        let photo = titled(&items, "Photos du week-end");
        let path = e.attachment_path(&photo.id, 1).unwrap().unwrap();
        assert!(std::fs::metadata(path).unwrap().len() > 0);
    }

    fn dir_size(path: &Path) -> u64 {
        std::fs::read_dir(path).map_or(0, |entries| {
            entries
                .flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
    }

    /// The full public demo, as the apps open it: time to write the history, time to join it,
    /// size of the feed and of the demo directory. A measure, not a check of the build:
    /// `cargo test --release -p tomi-core --features demo -- --ignored --nocapture full_demo`.
    #[test]
    #[ignore = "measure, in release"]
    fn full_demo_opening_time_and_disk() {
        let mb = |b: u64| b as f64 / 1_048_576.0;
        let dir = tempfile::tempdir().unwrap();
        let t = std::time::Instant::now();
        let world =
            DemoWorld::open_scaled(config(dir.path(), false, android_assets()), Scale::Full)
                .unwrap();
        let written = t.elapsed();
        let t = std::time::Instant::now();
        world.pair_with_code("7-crayon-lune", None).unwrap();
        world.confirm_pairing(true).unwrap();
        let joined = t.elapsed();
        let e = world.engine();
        let t = std::time::Instant::now();
        let items = e.list_items().unwrap();
        let listed = t.elapsed();
        let expected = demo_data::counts(Scale::Full);
        assert_eq!(items.len(), expected.items);
        assert_eq!(
            items.iter().filter(|v| v.archived).count(),
            expected.archived
        );
        assert_eq!(items.iter().filter(|v| v.pinned).count(), expected.pinned);
        let disk = dir_size(dir.path());
        println!(
            "full demo: {} items ({} archived, {} pinned), {:.1} MB of history",
            items.len(),
            expected.archived,
            expected.pinned,
            mb(world.history_bytes.load(Ordering::SeqCst)),
        );
        println!(
            "opening: {:.2} s = {:.2} s to write the history + {:.2} s to join it; list_items {} ms",
            (written + joined).as_secs_f64(),
            written.as_secs_f64(),
            joined.as_secs_f64(),
            listed.as_millis(),
        );
        println!("demo directory after opening: {:.1} MB", mb(disk));
        // Every attachment of the last week opened once (downloaded and decrypted here).
        let t = std::time::Instant::now();
        let week_start = SystemClock.now_ms() - 7 * DEMO_DAY_MS;
        let (mut opened, mut heavy, mut heavy_bytes, mut all_bytes) = (0, 0, 0, 0);
        for v in items.iter().filter(|v| v.created_at_ms >= week_start) {
            if let ItemContentView::Readable { attachments, .. } = &v.content {
                for a in attachments {
                    all_bytes += a.size;
                    if a.size >= 1_000_000 {
                        heavy += 1;
                        heavy_bytes += a.size;
                    }
                    if e.attachment_path(&v.id, a.slot).unwrap().is_some() {
                        opened += 1;
                    }
                }
            }
        }
        println!(
            "files of the week: {:.1} MB in all, {heavy} of 1 MB or more ({:.1} MB)",
            mb(all_bytes),
            mb(heavy_bytes)
        );
        println!(
            "{opened} attachments of the week opened in {:.2} s; demo directory then: {:.1} MB",
            t.elapsed().as_secs_f64(),
            mb(dir_size(dir.path()))
        );
    }

    #[test]
    fn the_simulated_pairing_follows_the_android_contract() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        let e = world.engine();
        assert!(!e.is_provisioned());
        assert_eq!(
            world.pair_with_code("abc", None).unwrap(),
            DemoPairing::InvalidCode
        );
        assert_eq!(world.pair_with_qr("  ").unwrap(), DemoPairing::InvalidCode);
        // Cancelled: a later confirmation is ignored.
        assert!(matches!(
            world.pair_with_code("7-crayon-lune", None).unwrap(),
            DemoPairing::AwaitingVerification { .. }
        ));
        world.cancel_pairing();
        world.confirm_pairing(true).unwrap();
        assert!(!e.is_provisioned());

        match world.pair_with_code("4821-crayon-renard", None).unwrap() {
            DemoPairing::AwaitingVerification {
                peer,
                verification_code,
                history_item_count,
                history_bytes,
                member_count,
                expires_in_ms,
                relay_authority,
            } => {
                assert_eq!(relay_authority, "relay.example.org");
                assert_eq!(verification_code, DEMO_VERIFICATION_CODE);
                assert_eq!(peer.name, "PC-Bureau");
                assert_eq!(peer.platform, "windows");
                assert_eq!(history_item_count as usize, expected_feed().items);
                assert!(history_bytes > 0);
                assert_eq!(member_count, 2);
                assert_eq!(expires_in_ms, DEMO_PAIRING_EXPIRES_MS);
            }
            other => panic!("unexpected {other:?}"),
        }
        world.confirm_pairing(false).unwrap();
        assert!(e.is_provisioned());
        assert!(e.list_items().unwrap().is_empty(), "no history asked");
        assert_eq!(e.list_devices().unwrap().len(), 3);
        assert!(!e.take_resync_notice());
        assert!(
            world.pair_with_code("4821-crayon-renard", None).is_err(),
            "already joined"
        );

        // Then items arrive as usual, including one of a future format.
        world.simulate_incoming(DemoIncoming::Link).unwrap();
        world.simulate_incoming(DemoIncoming::Unreadable).unwrap();
        let items = e.list_items().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|v| matches!(
            v.content,
            ItemContentView::Unreadable {
                reason: UnreadableReason::UpdateRequired
            }
        )));
        assert_eq!(
            titled(&items, "Documentation Android").source_device_name,
            "PC-Bureau"
        );
    }

    #[test]
    fn a_cancel_before_the_confirmation_leaves_nothing_to_confirm() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        world.pair_with_code("7-crayon-lune", None).unwrap();
        world.cancel_pairing();
        world.confirm_pairing(true).unwrap();
        assert!(!world.engine().is_provisioned());
    }

    #[test]
    fn a_cancel_during_the_wait_for_pc_bureau_joins_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        world.peer_confirm_delay_ms.store(2_000, Ordering::SeqCst);
        world.pair_with_code("7-crayon-lune", None).unwrap();
        let waiting = {
            let world = world.clone();
            std::thread::spawn(move || world.confirm_pairing(true))
        };
        std::thread::sleep(Duration::from_millis(300));
        world.cancel_pairing();
        // A new pairing right away must not be joined by the confirmation of the old one.
        world.pair_with_code("7-crayon-lune", None).unwrap();
        assert!(matches!(
            waiting.join().unwrap(),
            Err(CoreError::PairingCancelled)
        ));
        assert!(!world.engine().is_provisioned());
        world.peer_confirm_delay_ms.store(0, Ordering::SeqCst);
        world.confirm_pairing(false).unwrap();
        assert!(world.engine().is_provisioned());
    }

    #[test]
    fn the_space_is_created_with_the_demo_token_only() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        let e = world.engine();
        assert!(matches!(
            world.create_space("aaaa-bbbb-cccc-dddd-eeee-ffff-gggg-hhhh", "Test phone"),
            Err(CoreError::Relay { status: 403, ref code }) if code == "install_token_invalid"
        ));
        assert!(matches!(
            world.create_space(DEMO_INSTALL_TOKEN, " "),
            Err(CoreError::InvalidInput { .. })
        ));
        assert!(!e.is_provisioned());
        // Dashes, blanks and case do not matter, as for a real token.
        let typed = DEMO_INSTALL_TOKEN.replace('-', " ").to_uppercase();
        world.create_space(&typed, "Test phone").unwrap();
        assert!(e.is_provisioned());
        assert!(e.list_items().unwrap().is_empty(), "a new space is empty");
        assert!(e
            .list_devices()
            .unwrap()
            .iter()
            .any(|d| d.is_this_device && d.name == "Test phone"));
        assert!(matches!(
            world.create_space(DEMO_INSTALL_TOKEN, "Test phone"),
            Err(CoreError::InvalidInput { .. })
        ));
    }

    #[test]
    fn a_tablet_answers_the_invitation_with_the_real_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, None)).unwrap();
        let e = world.engine();
        let invitation = e.start_invitation().unwrap();
        world
            .answer_invitation(Zeroizing::new(invitation.code().unwrap()))
            .unwrap();
        let prompt = invitation.wait_for_peer().unwrap();
        assert_eq!(prompt.peer.name, "Tablette");
        assert_eq!(prompt.peer.platform, "android");
        assert_eq!(prompt.sas.len(), 7, "six digits in two groups");
        invitation.confirm().unwrap();
        let added = invitation.wait_done().unwrap();
        assert_eq!(added.name, "Tablette");
        e.sync().unwrap();
        assert!(e
            .list_devices()
            .unwrap()
            .iter()
            .any(|d| d.name == "Tablette" && !d.is_this_device));
    }

    #[test]
    fn a_cancelled_invitation_adds_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, None)).unwrap();
        let e = world.engine();
        let invitation = e.start_invitation().unwrap();
        world
            .answer_invitation(Zeroizing::new(invitation.code().unwrap()))
            .unwrap();
        invitation.wait_for_peer().unwrap();
        invitation.cancel();
        // The guest gives up by itself; the space keeps its three devices.
        std::thread::sleep(Duration::from_millis(300));
        e.sync().unwrap();
        assert_eq!(e.list_devices().unwrap().len(), 3);
        // The next guest is still the first "Tablette": the cancelled attempt joined nothing.
        let next = e.start_invitation().unwrap();
        world
            .answer_invitation(Zeroizing::new(next.code().unwrap()))
            .unwrap();
        assert_eq!(next.wait_for_peer().unwrap().peer.name, "Tablette");
        next.cancel();
    }

    #[test]
    fn the_departure_scenarios_of_the_demo() {
        use crate::runtime::leave::{
            DepartureOutcome as D, PendingReason as P, RefusalReason as R,
        };
        let paired = |dir: &Path| DemoWorld::open(config(dir, true, None)).unwrap();

        // Offline: pending, then erased with the proof once the network is back.
        let dir = tempfile::tempdir().unwrap();
        let world = paired(dir.path());
        let e = world.engine();
        world.simulate_departure(DemoDeparture::Offline);
        assert_eq!(e.leave_space().unwrap(), D::Pending(P::Network));
        assert!(e.departure().is_some());
        world.simulate_departure(DemoDeparture::Normal);
        assert_eq!(e.continue_departure().unwrap(), D::Erased);
        assert!(!e.is_provisioned());

        // The relay fails once: pending on the server.
        let dir = tempfile::tempdir().unwrap();
        let world = paired(dir.path());
        world.simulate_departure(DemoDeparture::ServerError);
        assert_eq!(world.engine().leave_space().unwrap(), D::Pending(P::Server));

        // The last device: refused, then the space is deleted.
        let dir = tempfile::tempdir().unwrap();
        let world = paired(dir.path());
        let e = world.engine();
        world.simulate_departure(DemoDeparture::LastDevice);
        assert_eq!(e.leave_space().unwrap(), D::Refused(R::LastDevice));
        assert!(e.departure().is_none(), "a refusal leaves nothing pending");
        assert_eq!(e.delete_space().unwrap(), D::Erased);

        // A relay that lost its data: not found, and erasing without proof is allowed.
        let dir = tempfile::tempdir().unwrap();
        let world = paired(dir.path());
        let e = world.engine();
        world.simulate_departure(DemoDeparture::Reset);
        assert_eq!(e.leave_space().unwrap(), D::Pending(P::NotFound));
        assert!(e.can_erase_without_proof());
        e.erase_without_proof().unwrap();
        assert!(!e.is_provisioned());

        // A relay too old to delete a space.
        let dir = tempfile::tempdir().unwrap();
        let world = paired(dir.path());
        let e = world.engine();
        world.simulate_departure(DemoDeparture::OldRelay);
        assert_eq!(e.leave_space().unwrap(), D::Refused(R::LastDevice));
        assert_eq!(e.delete_space().unwrap(), D::Refused(R::RelayTooOld));
        assert!(e.can_erase_without_proof());
    }

    #[test]
    fn no_history_stays_empty_while_the_worker_runs() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        world.start_worker().unwrap();
        world.pair_with_code("7-crayon-lune", None).unwrap();
        world.confirm_pairing(false).unwrap();
        std::thread::sleep(Duration::from_millis(2500));
        assert!(world.engine().list_items().unwrap().is_empty());
    }

    #[test]
    fn the_demo_refuses_a_relative_directory_and_cleans_up_when_dropped() {
        assert!(DemoWorld::open(config(Path::new("demo"), true, None)).is_err());
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, None)).unwrap();
        assert!(dir.path().join("this").exists());
        drop(world);
        assert!(DEMO_DIRS.iter().all(|d| !dir.path().join(d).exists()));
    }

    #[test]
    fn importing_the_history_at_pairing_gives_the_demo_feed() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), false, None)).unwrap();
        assert!(
            world.simulate_incoming(DemoIncoming::Text).is_err(),
            "not joined yet"
        );
        world
            .pair_with_qr("https://relay.example.org/l/pair#4821-crayon-renard")
            .unwrap();
        world.confirm_pairing(true).unwrap();
        let e = world.engine();
        assert_eq!(e.list_items().unwrap().len(), expected_feed().items);
        // Without the bundled images, a stand-in is attached (never an empty item).
        let items = e.list_items().unwrap();
        match &titled(&items, "Photos du week-end").content {
            ItemContentView::Readable { attachments, .. } => {
                assert_eq!(attachments.len(), 3);
                assert!(attachments
                    .iter()
                    .all(|a| a.mime == "image/png" && a.thumbnail.is_some()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_send_from_the_demo_device_is_delivered_to_pc_bureau() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, None)).unwrap();
        let e = world.engine();
        let id = e
            .enqueue(OutgoingDraft {
                id: None,
                title: None,
                text: Some(DraftText {
                    plain: "depuis la démo".into(),
                    html: None,
                    markdown: None,
                    source_format: SourceFormat::Plain,
                }),
                url: None,
                attachments: Vec::new(),
                secret: false,
                expires_at_ms: None,
                read_policy: ReadPolicy::UntilExpiry,
            })
            .unwrap();
        // Undo still works before the dispatch.
        assert!(e.cancel_if_queued(&id).unwrap());
        let id = e
            .enqueue(OutgoingDraft {
                id: None,
                title: Some("Note".into()),
                text: None,
                url: Some("https://example.org/".into()),
                attachments: Vec::new(),
                secret: false,
                expires_at_ms: None,
                read_policy: ReadPolicy::UntilExpiry,
            })
            .unwrap();
        assert_eq!(e.dispatch(&id).unwrap(), DispatchOutcome::Sent);
        world.office.sync().unwrap();
        e.sync().unwrap();
        let v = e.get_item(&id).unwrap().unwrap();
        let states: Vec<_> = v
            .deliveries
            .iter()
            .map(|d| (d.device_name.as_str(), d.state))
            .collect();
        assert!(states.contains(&("PC-Bureau", DeliveryState::Delivered)));
        assert!(states.contains(&("PC-Maison", DeliveryState::Sent)));
    }

    #[test]
    fn the_worker_delivers_in_the_background_and_stops_with_the_demo() {
        let dir = tempfile::tempdir().unwrap();
        let world = DemoWorld::open(config(dir.path(), true, None)).unwrap();
        let e = world.engine();
        let id = e
            .enqueue(OutgoingDraft {
                id: None,
                title: Some("En arrière-plan".into()),
                text: None,
                url: Some("https://example.org/fond".into()),
                attachments: Vec::new(),
                secret: false,
                expires_at_ms: None,
                read_policy: ReadPolicy::UntilExpiry,
            })
            .unwrap();
        assert_eq!(e.dispatch(&id).unwrap(), DispatchOutcome::Sent);
        world.start_worker().unwrap();
        let delivered = || {
            e.get_item(&id)
                .unwrap()
                .unwrap()
                .deliveries
                .iter()
                .any(|d| d.device_name == "PC-Bureau" && d.state == DeliveryState::Delivered)
        };
        let mut waited = Duration::ZERO;
        while !delivered() && waited < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
            waited += Duration::from_millis(100);
        }
        assert!(delivered(), "PC-Bureau caught up within a few seconds");
        drop(world); // joins the worker; must not hang
    }
}
