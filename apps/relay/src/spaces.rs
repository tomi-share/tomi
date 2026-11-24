//! The spaces a relay hosts and the routing that finds the space of a request
//! (`api-v1.md` §1.1, §10, annex A).
//!
//! # Layout
//!
//! ```text
//! <data_dir>/routing.db                          global: invitations, quota overrides, push health
//! <data_dir>/spaces/<space_id>/relay.db          one SQLite database per space
//! <data_dir>/spaces/<space_id>/blobs/<2 hex>/<blob_id hex>
//! ```
//!
//! A space is a [`Space`]: its identifier, its directory and its database. Nothing in a space
//! database says anything about another space, and a connection opened on one cannot see the
//! files of another. That is the isolation; this module only decides **which** space a request
//! talks to.
//!
//! # Routing
//!
//! The routing is an in-memory index, rebuilt from the space databases at every start, so it
//! cannot disagree with them after a crash or a restore:
//!
//! | Identifier in the request | Index |
//! |---|---|
//! | session token (`SHA-256`) | `sessions` |
//! | `device_id` | `devices` |
//! | `channel_id` | `channels` |
//! | `sign_pk`, active or revoked (uniqueness only) | `keys` |
//!
//! **The space database stays the authority.** The index only chooses which database to ask: a
//! stale entry makes the database answer "unknown", exactly as for a token that never existed,
//! and a missing entry gives the same "unknown". Neither can grant anything. The one thing the
//! index decides alone is uniqueness (`device_id`, `sign_pk` and `channel_id` are unique
//! relay-wide); every writer of those holds [`Spaces::lock_registry`] from the check to the
//! insertion in the index.
//!
//! # Pending spaces
//!
//! A space whose deletion is under way, or whose state is unknown until the next start (a
//! deletion that could not be proved or undone), is **pending** (`api-v1.md` §4.4.1): out of
//! `map`, still in `index`. Every identifier of it routes to [`Route::Pending`], answered `503
//! unavailable` (`1013` on a WebSocket), never like an unknown identifier: `401` and `409
//! space_not_initialized` must not come out of a state that may still turn back into a whole
//! space. A route is decided in one look under the three locks ([`Spaces::route_token`] and
//! siblings), so no request sees a space that is in neither `map` nor `pending`.
//!
//! # Lock order
//!
//! `pending`, then `index`, then `map`; a function may skip any of them but never takes one of
//! them while holding a later one. `unregister` (pending, map), `delete` step 6 (pending, index),
//! `reinstate` (index alone, then pending, map), `attach` (index alone), `is_empty` (pending,
//! map) and the `route_*` functions (pending, index, map) all keep it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use anyhow::{bail, Context};
use rusqlite::{params, OptionalExtension, Transaction};

use crate::config::Config;
use crate::db::{lock, Db, Tx};
use crate::error::{ApiError, Code};
use crate::store;
use crate::wire::{uuid_format, uuid_parse};

/// The 16 bytes of a `space_id`.
pub type SpaceId = [u8; 16];

/// Exit code of a relay that REFUSES TO START because of what its data holds or cannot hold
/// (`EX_CONFIG`, `api-v1.md` §10.1): a volume that is not mounted, a `spaces/` or space folder that
/// is a link, unreadable or not the right one, a database that cannot be read or belongs to another
/// space, a routing conflict. `infra/update.sh` and `infra/forge/deploy.sh` read it: with this code
/// they do NOT go back to the previous binary, which would ignore the same spaces and answer `401`
/// / `409 space_not_initialized` for a space that is alive. Any other failure to start exits `1`.
pub const EXIT_DATA_REFUSED: u8 = 78;

/// A refusal to start because of the data (see [`EXIT_DATA_REFUSED`]). Wraps the real error and
/// prints like it, so a message is unchanged; [`start_exit_code`] recognises it.
pub struct DataRefused(anyhow::Error);

impl std::fmt::Display for DataRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::fmt::Debug for DataRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::error::Error for DataRefused {}

/// Marks `e` as a refusal because of the data.
pub fn data_refused(e: impl Into<anyhow::Error>) -> anyhow::Error {
    anyhow::Error::new(DataRefused(e.into()))
}

/// The exit code of a relay (or of an `admin` command) whose start failed with `e`:
/// [`EXIT_DATA_REFUSED`] for a refusal because of the data, `1` otherwise.
pub fn start_exit_code(e: &anyhow::Error) -> u8 {
    if e.downcast_ref::<DataRefused>().is_some() {
        EXIT_DATA_REFUSED
    } else {
        1
    }
}

/// What a SQLite failure says, WITHOUT the path of the file (rusqlite puts it in some messages) and
/// so without the whole `space_id` it contains: its result code only.
fn sqlite_reason(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(f, _) => {
            format!("SQLite error {:?} (code {})", f.code, f.extended_code)
        }
        // Built by `Db::open*` for a schema version it does not know: no path in it.
        rusqlite::Error::InvalidParameterName(why) => why.clone(),
        _ => "the database cannot be read".to_owned(),
    }
}

/// `<data_dir>/routing.db`.
pub const ROUTING_FILE: &str = "routing.db";
/// `<data_dir>/spaces/`.
pub const SPACES_DIR: &str = "spaces";
/// `<data_dir>/spaces/<space_id>/relay.db`.
pub const SPACE_DB_FILE: &str = "relay.db";
/// Prefix of the directory a deleted space is renamed to, outside `spaces/`, before it is
/// removed (`<data_dir>/spaces.deleted-<space_id>-<random>`). One left by a crash is removed at
/// the next start.
const TRASH_PREFIX: &str = "spaces.deleted-";
/// `<data_dir>/spaces.trash/`: where a space deleted from the web panel waits to be restored or
/// purged (`<space_id>-<unix seconds>/`). Not `spaces.deleted-*`: the start never removes it.
pub const TRASH_DIR: &str = "spaces.trash";
/// How long a space stays in the trash.
pub const TRASH_TTL_MS: i64 = 7 * 86_400_000;

/// A space deleted from the web panel, as `space_trash` keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashRow {
    pub dir_name: String,
    pub space_id: String,
    pub deleted_at: i64,
    pub purge_after: i64,
    pub size_bytes: u64,
    /// `Some(q)`: the space had a quota of its own (`q` `None`: no limit).
    pub quota: Option<Option<u64>>,
}

/// Why a restore was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    /// The id is used by a space (or a pending one) again.
    IdTaken,
    /// A device, key, session or channel of the space belongs to another space now.
    Conflict(String),
    /// The folder is not in the trash any more.
    Missing,
    Failed(String),
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => total = total.saturating_add(dir_size(&e.path())),
            Ok(m) => total = total.saturating_add(m.len()),
            Err(_) => {}
        }
    }
    total
}

/// Flushes a directory to disk, so that a rename or a removal in it survives a crash. Only Unix
/// can open a directory to sync it; elsewhere this does nothing.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// The top message of an error of the deletion, for the logs. The messages built here never
/// hold a path nor a whole `space_id` (§9); the causes underneath (which may) are not shown.
fn short_error(e: &anyhow::Error) -> String {
    e.to_string()
}

/// `<data_dir>/spaces/<space_id>`.
pub fn space_dir(data_dir: &Path, id: &SpaceId) -> PathBuf {
    data_dir.join(SPACES_DIR).join(uuid_format(id))
}

/// What identifies, on the disk, the folder of a space and its database, as opposed to their
/// **path**: `(st_dev, st_ino)` of `spaces/<id>/` and of `spaces/<id>/relay.db`. Recorded when
/// the space is loaded, created or registered again; the presence check of a deletion
/// (`api-v1.md` §4.4.1, step 2) compares it with what the path leads to just before acting.
/// A volume unmounted lazily (`umount -l`) under a relay that runs, or a `data_dir` link pointed
/// elsewhere, can leave an **old copy** of the folder at the same path: the check would find
/// it, the rename and the `fsync` would succeed on the copy, and `routing.db` (still open on
/// the real volume) would record a receipt for a space that comes back with the mount. The
/// copy is another folder: its identity differs.
///
/// Unix only. Windows has no `st_ino` in the standard library (the file index needs
/// `GetFileInformationByHandle` on an open handle, or an unstable API): there the identity is
/// empty and every comparison holds, so the check there stays the one by path. Production runs
/// on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    #[cfg(unix)]
    dir: (u64, u64),
    #[cfg(unix)]
    db: (u64, u64),
}

impl Identity {
    /// The identity of a folder and of its database, from their metadata (read without following
    /// a symbolic link by the caller).
    fn of(dir: &std::fs::Metadata, db: &std::fs::Metadata) -> Identity {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Identity {
                dir: (dir.dev(), dir.ino()),
                db: (db.dev(), db.ino()),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (dir, db);
            Identity {}
        }
    }

    /// Reads the identity of `dir` and of `dir/relay.db`, without following a symbolic link.
    fn read(dir: &Path) -> std::io::Result<Identity> {
        let d = std::fs::symlink_metadata(dir)?;
        let f = std::fs::symlink_metadata(dir.join(SPACE_DB_FILE))?;
        Ok(Identity::of(&d, &f))
    }
}

/// One space: its database and its directory (blobs included).
pub struct Space {
    pub id: SpaceId,
    pub created_at: i64,
    /// `<data_dir>/spaces/<space_id>`; the argument of every [`crate::blobstore`] function.
    pub dir: PathBuf,
    pub db: Arc<Db>,
    /// The folder and the database this space was loaded from (see [`Identity`]).
    identity: Identity,
}

impl std::fmt::Debug for Space {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Space")
            .field("id", &self.short_id())
            .finish_non_exhaustive()
    }
}

impl Space {
    /// The `space_id` as lowercase hyphenated text.
    pub fn text_id(&self) -> String {
        uuid_format(&self.id)
    }

    /// The first 8 characters of the `space_id`: the only form of it that goes to the logs.
    pub fn short_id(&self) -> String {
        uuid_format(&self.id)[..8].to_owned()
    }
}

/// Why a prefix of a `space_id` designates no single space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Find {
    /// Fewer than 8 characters, or not hexadecimal.
    Unusable,
    NotFound,
    /// The matching spaces, as text.
    Ambiguous(Vec<String>),
}

/// What a space database says about the identifiers that route to it.
#[derive(Default)]
struct Entries {
    devices: Vec<String>,
    keys: Vec<Vec<u8>>,
    sessions: Vec<([u8; 32], i64)>,
    channels: Vec<String>,
}

fn read_entries(db: &Db) -> Result<Entries, ApiError> {
    db.read_sync(|tx| {
        let mut e = Entries::default();
        let mut st = tx.prepare("SELECT device_id, sign_pk FROM devices")?;
        let rows = st
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, pk) in rows {
            e.devices.push(id);
            e.keys.extend(pk);
        }
        let mut st = tx.prepare("SELECT sign_pk FROM revoked_sign_pks")?;
        e.keys.extend(
            st.query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        let mut st = tx.prepare("SELECT token_hash, expires_at FROM sessions")?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        e.sessions.extend(
            rows.into_iter()
                .filter_map(|(h, at)| <[u8; 32]>::try_from(h.as_slice()).ok().map(|h| (h, at))),
        );
        let mut st = tx.prepare("SELECT channel_id FROM pairing_channels")?;
        e.channels.extend(
            st.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        Ok(e)
    })
}

/// The lookup tables. Every value is the space the key belongs to.
#[derive(Default)]
struct Index {
    devices: HashMap<String, SpaceId>,
    keys: HashMap<Vec<u8>, SpaceId>,
    sessions: HashMap<[u8; 32], (SpaceId, i64)>,
    channels: HashMap<String, SpaceId>,
}

impl Index {
    /// Adds the entries of a space. Refuses (and adds nothing) if one of them already belongs to
    /// another space: that would make the routing ambiguous.
    fn attach(&mut self, space: SpaceId, e: &Entries) -> Result<(), String> {
        let foreign = |owner: Option<&SpaceId>| owner.is_some_and(|o| *o != space);
        if let Some(d) = e.devices.iter().find(|d| foreign(self.devices.get(*d))) {
            return Err(format!("device {} is in two spaces", &d[..d.len().min(8)]));
        }
        if e.keys.iter().any(|k| foreign(self.keys.get(k))) {
            return Err("a signing key is in two spaces".to_owned());
        }
        if e.sessions
            .iter()
            .any(|(h, _)| foreign(self.sessions.get(h).map(|v| &v.0)))
        {
            return Err("a session is in two spaces".to_owned());
        }
        if let Some(c) = e.channels.iter().find(|c| foreign(self.channels.get(*c))) {
            return Err(format!("pairing channel {c} is in two spaces"));
        }
        for d in &e.devices {
            self.devices.insert(d.clone(), space);
        }
        for k in &e.keys {
            self.keys.insert(k.clone(), space);
        }
        for (h, at) in &e.sessions {
            self.sessions.insert(*h, (space, *at));
        }
        for c in &e.channels {
            self.channels.insert(c.clone(), space);
        }
        Ok(())
    }

    fn detach(&mut self, space: &SpaceId) {
        self.devices.retain(|_, s| s != space);
        self.keys.retain(|_, s| s != space);
        self.sessions.retain(|_, (s, _)| s != space);
        self.channels.retain(|_, s| s != space);
    }
}

fn plain<T>(r: std::sync::LockResult<T>) -> T {
    r.unwrap_or_else(PoisonError::into_inner)
}

/// The registry lock, owned: it can move into a task that outlives the request (see
/// [`Spaces::lock_registry`]).
pub type RegistryGuard = tokio::sync::OwnedMutexGuard<()>;

/// The registry of the spaces of the relay.
pub struct Spaces {
    data_dir: PathBuf,
    routing: Arc<Db>,
    map: RwLock<BTreeMap<SpaceId, Arc<Space>>>,
    index: Mutex<Index>,
    /// Quota overrides of `admin quota` (`Some(None)`: explicitly no limit), mirrored in
    /// `routing.db`.
    overrides: Mutex<HashMap<SpaceId, Option<u64>>>,
    /// The default quota set from the web panel, mirrored in `routing.db` (`default_space_quota`):
    /// `None` if the panel never set one, `Some(None)` for "no limit per space", `Some(Some(n))`
    /// for `n` bytes. Read at every reservation, so a change applies at once, without a restart.
    default_quota: Mutex<Option<Option<u64>>>,
    /// Held from the check to the insertion in the index by everything that creates an
    /// identifier that must stay unique relay-wide: a space, a device, a pairing channel; and by
    /// `admin reset`.
    registry: Arc<tokio::sync::Mutex<()>>,
    /// Spaces that are out of `map` because a deletion is under way, or because it failed
    /// without deleting anything and the space could not be opened again. They are still in
    /// `index` (their identifiers stay taken). A token of one of them is answered `503
    /// unavailable`, and the relay does not count as empty while one is listed: none of that
    /// can be mistaken for "your space is gone". (Since §1.4 a client takes neither `401` nor
    /// `409 space_not_initialized` for a proof, but a `503` is the honest answer while the state
    /// is not known, and the relay must not count as empty, which would create a token.) Emptied
    /// by the end of the deletion, or by the next start.
    pending: Mutex<BTreeSet<SpaceId>>,
    /// Serialises the reservations of quota (see `Inner::reserve`).
    reserve: Mutex<()>,
    /// When each quota warning was last given (`None`: the total).
    warned: Mutex<HashMap<Option<SpaceId>, i64>>,
    /// Test hook: the steps of [`Spaces::destroy`] that fail (see [`Spaces::inject_destroy_fault`]).
    destroy_faults: Mutex<Vec<DestroyStep>>,
    /// Test hook: how many callers wait for the registry lock right now.
    registry_waiters: AtomicUsize,
    /// Test hook: the next deletion stops right after the unregistering (see
    /// [`Spaces::pause_next_deletion`]).
    deletion_pause: Mutex<Option<DeletionPause>>,
}

/// Where an identifier of a request leads (`api-v1.md` §1.4, §4.4.1).
#[derive(Debug, Clone)]
pub enum Route {
    /// A registered space.
    Live(Arc<Space>),
    /// A space out of `map` whose deletion is under way, or whose state is unknown until the
    /// next start: `503 unavailable`, never "unknown".
    Pending,
    /// No space knows it.
    Unknown,
}

/// What `destroy` did on the disk (step 3 of §4.4.1).
enum Removed {
    /// The directory was renamed out of `spaces/`; what is left to remove.
    Renamed { trash: PathBuf },
    /// The rename was refused and `relay.db` was removed in place.
    InPlace,
}

/// Why `destroy` did not give a provable deletion.
enum DestroyFail {
    /// Nothing was deleted (the rename and the in-place removal were both refused); the
    /// database is on the disk: the space can be registered again.
    NothingDeleted(anyhow::Error),
    /// The presence check of step 2 did not find the space (volume not mounted, folder moved,
    /// `relay.db` replaced): nothing was deleted, and there is nothing to open again.
    Absent(anyhow::Error),
    /// The positive act happened but could not be made durable (`fsync`).
    NotDurable(anyhow::Error),
    /// `ENOENT` after the presence check, a panic, an unexpected I/O error: unknown until the
    /// next start.
    Unknown(anyhow::Error),
}

/// Outcome of a whole deletion ([`Spaces::delete`]), as `DELETE /v1/space` and `admin reset`
/// report it (`api-v1.md` §4.4.1, *Failure*).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// Steps 2 to 6 succeeded (the receipt, if asked for, is written): the **only** outcome
    /// that may be answered `200 {"space_deleted": true}`.
    Deleted,
    /// Nothing was deleted and the space is registered again, whole: `500`.
    NotDeleted,
    /// The positive act happened but its `fsync` failed: `500`, no receipt, and the space
    /// stays pending (`503`) until the next start.
    NotDurable,
    /// Gone for good, but the transaction of `routing.db` (receipt, quota, invitation) failed:
    /// `500`, no receipt.
    DeletedWithoutReceipt,
    /// State unknown (presence check failed, `ENOENT`, panic, a space that cannot be opened
    /// again): the space stays pending until the next start, `503`.
    Unknown,
    /// The space was no longer registered when the task ran (not reachable while the caller
    /// holds the registry lock): `503`.
    NotRegistered,
}

/// Test hook of [`Spaces::pause_next_deletion`]: the deletion adds a permit to `reached` once
/// the space is pending, then waits for a permit of `resume`.
#[doc(hidden)]
#[derive(Clone)]
pub struct DeletionPause {
    pub reached: Arc<tokio::sync::Semaphore>,
    pub resume: Arc<tokio::sync::Semaphore>,
}

/// A step of [`Spaces::destroy`] that a test can make fail (a filesystem refusal is not
/// reproducible the same way on every platform).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestroyStep {
    /// The presence check: the space looks absent (volume not mounted).
    Presence,
    /// Renaming the directory out of `spaces/`.
    Rename,
    /// Removing `relay.db` in place (the fallback when the rename is refused).
    RemoveDatabase,
    /// Syncing the modified directories.
    SyncDir,
    /// The transaction of `routing.db`: receipt, quota override, invitation.
    ForgetRouting,
    /// Removing what is left on the disk (renamed folder, or debris of an in-place removal).
    RemoveDirectory,
    /// Opening again the database of a space that nothing could be deleted from.
    Reopen,
}

/// Counts a caller as waiting for the registry lock for as long as it lives (also if the
/// waiting future is dropped).
struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Spaces {
    /// Opens `routing.db`, migrates the layout that preceded the multi-space relay if need be
    /// (`migrate.rs`), opens every space and builds the index. Refuses to start if two spaces
    /// share an identifier that must be unique. For a process that holds `relay.lock`.
    pub fn open(data_dir: &Path) -> anyhow::Result<Spaces> {
        Self::open_as(data_dir, false)
    }

    /// Opens what exists for reading and writes **nothing**: no migration, no `routing.db`
    /// created, no row deleted, no `journal_mode` change, no writer connection (every write
    /// fails with `503`). For the `admin` commands that only read and hold no lock. The older
    /// layout is refused with a message that says to start the relay (it migrates).
    pub fn open_read_only(data_dir: &Path) -> anyhow::Result<Spaces> {
        Self::open_as(data_dir, true)
    }

    /// Every failure of this function is a refusal to start because of the data
    /// ([`EXIT_DATA_REFUSED`]), except the intact old layout seen by a process that does not
    /// migrate ([`crate::migrate::LegacyLayout`]): a directory that is fine, only older.
    fn open_as(data_dir: &Path, read_only: bool) -> anyhow::Result<Spaces> {
        Self::load(data_dir, read_only).map_err(|e| {
            if e.chain().any(|c| c.is::<crate::migrate::LegacyLayout>()) {
                e
            } else {
                data_refused(e)
            }
        })
    }

    fn load(data_dir: &Path, read_only: bool) -> anyhow::Result<Spaces> {
        crate::migrate::run(data_dir, !read_only).map_err(|e| {
            if read_only {
                e
            } else {
                e.context("cannot migrate the data directory")
            }
        })?;
        let routing = if read_only {
            Db::open_routing_read_only(&data_dir.join(ROUTING_FILE))
        } else {
            Db::open_routing(&data_dir.join(ROUTING_FILE))
        }
        .map_err(|e| anyhow::anyhow!("cannot open routing.db: {}", sqlite_reason(&e)))?;
        let spaces = Spaces {
            data_dir: data_dir.to_path_buf(),
            routing,
            map: RwLock::new(BTreeMap::new()),
            index: Mutex::new(Index::default()),
            overrides: Mutex::new(HashMap::new()),
            default_quota: Mutex::new(None),
            registry: Arc::new(tokio::sync::Mutex::new(())),
            pending: Mutex::new(BTreeSet::new()),
            reserve: Mutex::new(()),
            warned: Mutex::new(HashMap::new()),
            destroy_faults: Mutex::new(Vec::new()),
            registry_waiters: AtomicUsize::new(0),
            deletion_pause: Mutex::new(None),
        };

        let root = data_dir.join(SPACES_DIR);
        // The folder of a space and its database are never loaded through a symbolic link as
        // their last component: the presence check of a deletion does not follow one either
        // (§4.4.1, step 2), so a space mounted that way could never be deleted and would go
        // "pending" at every try. Skipping it would be worse: every device of the whole space
        // would be answered "no such space" (`401`, `409 space_not_initialized`), and a relay
        // left without any space would create an installation token. So the start is REFUSED,
        // like for any space that cannot be loaded faithfully (§10.1). Links higher up
        // (`data_dir`, `spaces/`) are fine: the identity of the folder is what counts.
        //
        // Only "does not exist" (`NotFound`) means "no space". Any other failure to look at what
        // is on the disk (a refused read after a restore without `chown`, an I/O error, a
        // `spaces/` link whose target is not there because the volume is not mounted) is NOT
        // "no space": the start is refused for the same reason as above. No path and no whole
        // `space_id` in these messages.
        let load = match std::fs::symlink_metadata(&root) {
            Ok(_) => match std::fs::metadata(&root) {
                Ok(m) if m.is_dir() => true,
                Ok(_) => bail!(
                    "the spaces folder is not a folder: refusing to start, because the devices of every space would be told that their space does not exist"
                ),
                Err(e) => bail!(
                    "the spaces folder cannot be read ({}; a link to a volume that is not mounted?): refusing to start, because the devices of every space would be told that their space does not exist",
                    e.kind()
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => bail!(
                "the spaces folder cannot be looked at ({}): refusing to start, because the devices of every space would be told that their space does not exist",
                e.kind()
            ),
        };
        if load {
            let mut names: Vec<(String, PathBuf)> = Vec::new();
            for e in std::fs::read_dir(&root).context("cannot read the spaces folder")? {
                let e = e.context("cannot read an entry of the spaces folder")?;
                let name = e.file_name().to_string_lossy().into_owned();
                let kind = e
                    .file_type()
                    .context("cannot read the type of an entry of the spaces folder")?;
                if kind.is_symlink() {
                    if uuid_parse(&name).is_some() {
                        bail!(
                            "the space folder {} is a symbolic link: refusing to start, because its devices would be told that their space does not exist. Replace the link by the real folder (move the folder itself into spaces/)",
                            &name[..8]
                        );
                    }
                    tracing::warn!("spaces: a symbolic link that is not a space id; ignored");
                } else if kind.is_dir() {
                    names.push((name, e.path()));
                }
            }
            names.sort();
            for (name, dir) in names {
                let Some(id) = uuid_parse(&name) else {
                    tracing::warn!("spaces: the directory {name:?} is not a space id; ignored");
                    continue;
                };
                let file = dir.join(SPACE_DB_FILE);
                match std::fs::symlink_metadata(&file) {
                    Ok(f) if f.file_type().is_symlink() => bail!(
                        "the database of space {} is a symbolic link: refusing to start, because its devices would be told that their space does not exist. Replace the link by the real relay.db file",
                        &name[..8]
                    ),
                    Ok(f) if f.is_file() => {}
                    // Not a file (a folder, ...): not a space database, as for a missing one.
                    Ok(_) => {
                        tracing::warn!("spaces: {} has no database; ignored", &name[..8]);
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        tracing::warn!("spaces: {} has no database; ignored", &name[..8]);
                        continue;
                    }
                    // Refused (`EACCES`), I/O error, ...: the database may well be there.
                    Err(e) => {
                        return Err(anyhow::Error::new(e).context(format!(
                            "cannot look at the database of space {}: refusing to start, because its devices would be told that their space does not exist",
                            &name[..8]
                        )))
                    }
                }
                let opened = if read_only {
                    Db::open_read_only(&file, crate::db::SCHEMA_VERSION)
                } else {
                    Db::open(&file).map(Some)
                }
                .map_err(|e| {
                    anyhow::anyhow!(
                        "cannot open the database of space {}: {}; refusing to start, because its devices would be told that their space does not exist",
                        &name[..8],
                        sqlite_reason(&e)
                    )
                })?;
                let Some(db) = opened else {
                    tracing::warn!("spaces: {} was never created; ignored", &name[..8]);
                    continue;
                };
                let row = db
                    .read_sync(|tx| store::space_row(tx))
                    .map_err(|e| anyhow::anyhow!("cannot read space {}: {e}", &name[..8]))?;
                let Some(row) = row else {
                    tracing::warn!("spaces: {} was never created; ignored", &name[..8]);
                    db.close();
                    continue;
                };
                if row.space_id != id {
                    bail!(
                        "the directory {} holds the database of another space: refusing to start",
                        &name[..8]
                    );
                }
                // Read once the database is open and the space row read, as `create` does: the
                // identity must be the one of what was actually opened. What is on the disk now
                // must still be a plain folder and a plain file, not a link put there meanwhile.
                let identity = match std::fs::symlink_metadata(&dir)
                    .and_then(|d| std::fs::symlink_metadata(&file).map(|f| (d, f)))
                {
                    Ok((d, f)) if d.is_dir() && f.is_file() => Identity::of(&d, &f),
                    Ok(_) => bail!(
                        "the folder or the database of space {} changed while it was loaded (a link?): refusing to start",
                        &name[..8]
                    ),
                    Err(e) => {
                        return Err(anyhow::Error::new(e)
                            .context(format!("cannot read the identity of space {}", &name[..8])))
                    }
                };
                let space = Arc::new(Space {
                    id,
                    created_at: row.created_at,
                    dir,
                    db,
                    identity,
                });
                spaces
                    .attach(&space)
                    .map_err(|why| anyhow::anyhow!("routing conflict: {why}; refusing to start"))?;
                plain(spaces.map.write()).insert(id, space);
            }
        }
        spaces.load_overrides()?;
        spaces.load_default_quota()?;
        spaces.count_orphan_invitations()?;
        if !read_only {
            spaces.remove_trash();
        }
        Ok(spaces)
    }

    /// Counts the consumed invitations whose space the start did not find, and only warns:
    /// they are **kept** (`api-v1.md` §10.1). A start cannot tell a deleted space from a volume
    /// that is not mounted or a folder moved by hand, and such a row is harmless (its token
    /// answers `space_exists`, which creates nothing). A deletion that succeeds forgets the
    /// invitation in the transaction of its receipt (§4.4.1, step 5). No identifier is logged.
    fn count_orphan_invitations(&self) -> anyhow::Result<()> {
        let ids: Vec<String> = self
            .routing
            .read_sync(|tx| {
                let mut st = tx.prepare(
                    "SELECT space_id FROM invitations
                     WHERE consumed_at IS NOT NULL AND space_id IS NOT NULL",
                )?;
                let v = st
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .map_err(|e| anyhow::anyhow!("cannot read the invitations: {e}"))?;
        // A `space_id` that is not a UUID was not written by this relay: not counted.
        let orphans = ids
            .iter()
            .filter_map(|t| uuid_parse(t))
            .filter(|id| self.get(id).is_none())
            .count();
        if orphans > 0 {
            tracing::warn!(
                count = orphans,
                "start-up: consumed invitations name spaces that were not found (kept: a deleted space and an unmounted volume look the same)"
            );
        }
        Ok(())
    }

    /// Removes the directories of spaces deleted by a process that stopped before it finished.
    fn remove_trash(&self) {
        let Ok(rd) = std::fs::read_dir(&self.data_dir) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(TRASH_PREFIX) && e.path().is_dir() {
                match std::fs::remove_dir_all(e.path()) {
                    Ok(()) => tracing::info!("start-up: a deleted space was removed for good"),
                    Err(err) => {
                        tracing::warn!("start-up: cannot remove a deleted space: {}", err.kind());
                    }
                }
            }
        }
    }

    fn attach(&self, space: &Space) -> Result<(), String> {
        let entries = read_entries(&space.db).map_err(|e| format!("{}: {e}", space.short_id()))?;
        plain(self.index.lock())
            .attach(space.id, &entries)
            .map_err(|why| format!("{why} (space {})", space.short_id()))
    }

    /// Reads the quota overrides into memory. The rows of spaces the start did not find stay in
    /// `routing.db` (see [`Spaces::count_orphan_invitations`] for why) and out of memory, with a
    /// warning that gives their number only.
    fn load_overrides(&self) -> anyhow::Result<()> {
        let rows: Vec<(String, Option<i64>)> = self
            .routing
            .read_sync(|tx| {
                let mut st = tx.prepare("SELECT space_id, max_bytes FROM space_quotas")?;
                let v = st
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .map_err(|e| anyhow::anyhow!("cannot read the quotas: {e}"))?;
        let mut keep = HashMap::new();
        let mut dangling = 0usize;
        for (text, max) in rows {
            match uuid_parse(&text).filter(|id| self.get(id).is_some()) {
                Some(id) => {
                    keep.insert(id, max.map(|m| u64::try_from(m).unwrap_or(0)));
                }
                None => dangling += 1,
            }
        }
        if dangling > 0 {
            tracing::warn!(
                count = dangling,
                "start-up: quota overrides name spaces that were not found (kept)"
            );
        }
        *plain(self.overrides.lock()) = keep;
        Ok(())
    }

    /// Reads the default quota of the web panel from `routing.db`. An older database opened
    /// read-only may lack the table: nothing was set.
    fn load_default_quota(&self) -> anyhow::Result<()> {
        let stored: Option<i64> = self
            .routing
            .read_sync(|tx| {
                let table: i64 = tx.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'default_space_quota'",
                    [],
                    |r| r.get(0),
                )?;
                if table == 0 {
                    return Ok(None);
                }
                Ok(tx
                    .query_row(
                        "SELECT max_bytes FROM default_space_quota WHERE id = 1",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?)
            })
            .map_err(|e| anyhow::anyhow!("cannot read the default quota: {e}"))?;
        *plain(self.default_quota.lock()) = stored.map(|b| match u64::try_from(b) {
            Ok(0) | Err(_) => None,
            Ok(n) => Some(n),
        });
        Ok(())
    }

    // ----- access -----------------------------------------------------------------------

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// `routing.db`.
    pub fn routing(&self) -> &Arc<Db> {
        &self.routing
    }

    /// Whether the relay hosts no space at all (the only case of `space_not_initialized`). A
    /// space whose deletion is under way or failed (see `pending`) still counts: it may come
    /// back whole.
    pub fn is_empty(&self) -> bool {
        // `pending` is locked while a space moves between it and `map` (always in this order),
        // so this never looks at the two while a space is in neither.
        let pending = plain(self.pending.lock());
        pending.is_empty() && plain(self.map.read()).is_empty()
    }

    /// Whether the space is pending (deletion under way, or state unknown until the next start).
    pub fn is_pending(&self, id: &SpaceId) -> bool {
        plain(self.pending.lock()).contains(id)
    }

    /// How many spaces are pending (`admin status`, §8.7).
    pub fn pending_count(&self) -> usize {
        plain(self.pending.lock()).len()
    }

    /// Decides a route from the owner the index gives, with `pending` (and `index`, if the
    /// caller looked in it) still locked: the three locks are held together, in their order.
    fn decide(&self, pending: &BTreeSet<SpaceId>, owner: Option<SpaceId>) -> Route {
        let Some(id) = owner else {
            return Route::Unknown;
        };
        if pending.contains(&id) {
            return Route::Pending;
        }
        // A stale entry of the index (a space that no longer exists) is an unknown identifier.
        plain(self.map.read())
            .get(&id)
            .cloned()
            .map_or(Route::Unknown, Route::Live)
    }

    /// Where a session token (`SHA-256` of the token) leads, in one look (`pending`, `index`,
    /// `map`).
    pub fn route_token(&self, token_hash: &[u8; 32]) -> Route {
        let pending = plain(self.pending.lock());
        let ix = plain(self.index.lock());
        let owner = ix.sessions.get(token_hash).map(|(s, _)| *s);
        self.decide(&pending, owner)
    }

    /// Where a `device_id` leads (revoked devices included), in one look.
    pub fn route_device(&self, device_id: &str) -> Route {
        let pending = plain(self.pending.lock());
        let ix = plain(self.index.lock());
        let owner = ix.devices.get(device_id).copied();
        self.decide(&pending, owner)
    }

    /// Where a pairing channel number leads, in one look.
    pub fn route_channel(&self, channel_id: &str) -> Route {
        let pending = plain(self.pending.lock());
        let ix = plain(self.index.lock());
        let owner = ix.channels.get(channel_id).copied();
        self.decide(&pending, owner)
    }

    /// The state of a space by its identifier, in one look (`pending`, `map`).
    pub fn route_space(&self, id: &SpaceId) -> Route {
        let pending = plain(self.pending.lock());
        self.decide(&pending, Some(*id))
    }

    /// The spaces, ordered by identifier.
    pub fn list(&self) -> Vec<Arc<Space>> {
        plain(self.map.read()).values().cloned().collect()
    }

    pub fn get(&self, id: &SpaceId) -> Option<Arc<Space>> {
        plain(self.map.read()).get(id).cloned()
    }

    /// The space whose identifier starts with `prefix` (at least 8 characters).
    pub fn find(&self, prefix: &str) -> Result<Arc<Space>, Find> {
        let p = prefix.trim().to_ascii_lowercase();
        if p.len() < 8 || !p.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err(Find::Unusable);
        }
        let hits: Vec<Arc<Space>> = self
            .list()
            .into_iter()
            .filter(|s| s.text_id().starts_with(&p))
            .collect();
        match hits.as_slice() {
            [] => Err(Find::NotFound),
            [one] => Ok(one.clone()),
            many => Err(Find::Ambiguous(many.iter().map(|s| s.text_id()).collect())),
        }
    }

    /// The registered space of a device, if any (push, tests). It cannot tell a pending space
    /// from an unknown one: a request uses [`Spaces::route_device`].
    pub fn by_device(&self, device_id: &str) -> Option<Arc<Space>> {
        match self.route_device(device_id) {
            Route::Live(s) => Some(s),
            Route::Pending | Route::Unknown => None,
        }
    }

    /// The registered space of a pairing channel, if any. Reserved for tests: it cannot tell a
    /// pending space from an unknown one, so a request uses [`Spaces::route_channel`].
    #[doc(hidden)]
    pub fn by_channel(&self, channel_id: &str) -> Option<Arc<Space>> {
        match self.route_channel(channel_id) {
            Route::Live(s) => Some(s),
            Route::Pending | Route::Unknown => None,
        }
    }

    /// Whether the signing key is known to some space, active or revoked.
    pub fn key_known(&self, sign_pk: &[u8]) -> bool {
        plain(self.index.lock()).keys.contains_key(sign_pk)
    }

    /// Whether the device is known to some space (revoked ones included).
    pub fn device_known(&self, device_id: &str) -> bool {
        plain(self.index.lock()).devices.contains_key(device_id)
    }

    /// Whether the pairing channel number is used by some space (even by a closed channel that
    /// has not expired yet).
    pub fn channel_taken(&self, channel_id: &str) -> bool {
        plain(self.index.lock()).channels.contains_key(channel_id)
    }

    /// The devices of a space, revoked ones included.
    pub fn devices_of(&self, id: &SpaceId) -> Vec<String> {
        plain(self.index.lock())
            .devices
            .iter()
            .filter(|(_, s)| *s == id)
            .map(|(d, _)| d.clone())
            .collect()
    }

    // ----- index maintenance ------------------------------------------------------------

    /// To call once the device is committed, with the registry locked.
    pub fn add_device(&self, device_id: &str, sign_pk: &[u8], space: SpaceId) {
        let mut ix = plain(self.index.lock());
        ix.devices.insert(device_id.to_owned(), space);
        ix.keys.insert(sign_pk.to_vec(), space);
    }

    /// To call once the session is committed, before the token is given out.
    pub fn add_session(&self, token_hash: [u8; 32], space: SpaceId, expires_at: i64) {
        plain(self.index.lock())
            .sessions
            .insert(token_hash, (space, expires_at));
    }

    pub fn remove_session(&self, token_hash: &[u8; 32]) {
        plain(self.index.lock()).sessions.remove(token_hash);
    }

    /// Forgets the sessions that expired before `before_ms` (the maintenance deletes their rows
    /// at the same moment).
    pub fn prune_sessions(&self, before_ms: i64) {
        plain(self.index.lock())
            .sessions
            .retain(|_, (_, expires_at)| *expires_at >= before_ms);
    }

    /// To call once the channel is committed, with the registry locked.
    pub fn add_channel(&self, channel_id: &str, space: SpaceId) {
        plain(self.index.lock())
            .channels
            .insert(channel_id.to_owned(), space);
    }

    pub fn remove_channels(&self, ids: &[String]) {
        let mut ix = plain(self.index.lock());
        for id in ids {
            ix.channels.remove(id);
        }
    }

    // ----- creation and removal ---------------------------------------------------------

    /// The registry lock (see the field). The guard is owned, so that a deletion can carry it
    /// into a task of its own: a client that hangs up drops the request, not the task, and the
    /// lock stays held until the deletion has finished.
    pub async fn lock_registry(&self) -> RegistryGuard {
        let waiting = Waiting::new(&self.registry_waiters);
        let guard = Arc::clone(&self.registry).lock_owned().await;
        drop(waiting);
        guard
    }

    /// Test hook: how many callers of [`Spaces::lock_registry`] wait for the lock right now. The
    /// lock is fair (first come, first served), so a test that holds it and waits for this
    /// counter to grow decides the order in which the waiting requests run.
    #[doc(hidden)]
    pub fn registry_waiters(&self) -> usize {
        self.registry_waiters.load(Ordering::SeqCst)
    }

    /// Test hook: makes these steps of [`Spaces::destroy`] fail (an empty list: none). Not
    /// persistent: a relay that starts again knows nothing of it.
    #[doc(hidden)]
    pub fn inject_destroy_fault(&self, steps: &[DestroyStep]) {
        *lock(&self.destroy_faults) = steps.to_vec();
    }

    /// Test hook: the next deletion ([`crate::state::Inner::delete_space`]) stops once the space
    /// is pending, before anything is deleted, and goes on when the test adds a permit to
    /// `resume`. It holds the registry lock meanwhile, as a real one does.
    #[doc(hidden)]
    pub fn pause_next_deletion(&self) -> DeletionPause {
        let pause = DeletionPause {
            reached: Arc::new(tokio::sync::Semaphore::new(0)),
            resume: Arc::new(tokio::sync::Semaphore::new(0)),
        };
        *lock(&self.deletion_pause) = Some(pause.clone());
        pause
    }

    /// The pause of [`Spaces::pause_next_deletion`], taken by the deletion that meets it.
    pub(crate) fn take_deletion_pause(&self) -> Option<DeletionPause> {
        lock(&self.deletion_pause).take()
    }

    fn destroy_fault(&self, step: DestroyStep) -> std::io::Result<()> {
        if lock(&self.destroy_faults).contains(&step) {
            Err(std::io::Error::other("injected fault"))
        } else {
            Ok(())
        }
    }

    /// Creates the directory and the database of a new space, runs `build` in its first
    /// transaction (the space row, the founder, the first event) and registers it. Nothing is
    /// left behind if any step fails. Blocking; the registry lock must be held.
    pub fn create<R>(
        &self,
        id: SpaceId,
        build: impl FnOnce(&Transaction) -> Result<R, ApiError>,
    ) -> Result<(Arc<Space>, R), ApiError> {
        match self.route_space(&id) {
            Route::Live(_) => {
                return Err(ApiError::new(Code::SpaceExists).detail("This space already exists."))
            }
            // Its folder may come back whole at the next start: never touched (§4.4.1).
            Route::Pending => return Err(ApiError::pending()),
            Route::Unknown => {}
        }
        let dir = space_dir(&self.data_dir, &id);
        self.clear_unloaded_folder(&dir)?;
        std::fs::create_dir_all(&dir)?;
        let abandon = |db: Option<&Arc<Db>>| {
            if let Some(db) = db {
                db.close();
            }
            let _ = std::fs::remove_dir_all(&dir);
        };
        let db = match Db::open(&dir.join(SPACE_DB_FILE)) {
            Ok(db) => db,
            Err(e) => {
                abandon(None);
                return Err(e.into());
            }
        };
        let built = db
            .write_sync(|tx| build(tx).map(Tx::Commit))
            .and_then(|r| Ok((r, db.read_sync(|tx| store::require_space(tx))?)));
        let (value, row) = match built {
            Ok(v) => v,
            Err(e) => {
                abandon(Some(&db));
                return Err(e);
            }
        };
        let identity = match Identity::read(&dir) {
            Ok(i) => i,
            Err(e) => {
                abandon(Some(&db));
                return Err(e.into());
            }
        };
        let space = Arc::new(Space {
            id,
            created_at: row.created_at,
            dir: dir.clone(),
            db: db.clone(),
            identity,
        });
        if let Err(why) = self.attach(&space) {
            abandon(Some(&db));
            return Err(ApiError::internal(format!("routing conflict: {why}")));
        }
        plain(self.map.write()).insert(id, space.clone());
        Ok((space, value))
    }

    /// Step 1 of §4.4.1: unregisters a space. No request reaches it any more, and nobody can log
    /// in; it is `pending` (answered `503`, the relay is not empty because of it) until
    /// [`Spaces::delete`] settles the outcome. Its identifiers stay in the index, its database
    /// is still open and its WebSockets stay open. For the holder of the registry lock.
    pub fn unregister(&self, id: &SpaceId) -> Option<Arc<Space>> {
        let mut pending = plain(self.pending.lock());
        let space = plain(self.map.write()).remove(id);
        if space.is_some() {
            pending.insert(*id);
        }
        space
    }

    /// Steps 2 to 7 of §4.4.1 on a space that [`Spaces::unregister`] made pending. Blocking;
    /// the caller holds the registry lock. `receipt` is the `deletion_id` of the client, if any
    /// (its `SHA-256` is what is stored).
    ///
    /// **Fail-closed.** [`DeleteOutcome::Deleted`] comes out of one path only: the positive act
    /// on the disk (rename, or removal of `relay.db`) **succeeded**, was made durable, and the
    /// transaction of `routing.db` (receipt, quota, invitation) committed. A missing file is
    /// never a success. Every other outcome is answered `500` or `503`, never `200`, and none is
    /// a proof for a client. Most leave the space whole and registered again, or pending (`503`)
    /// until the next start, which brings it back if it is on the disk. The exception is
    /// [`DeleteOutcome::DeletedWithoutReceipt`]: the space IS gone for good (the positive act
    /// happened) but the receipt could not be written, so the answer is `500`, the client keeps
    /// its keys and retries or uses "delete anyway" (§4.4.2, §7.9); the quota or invitation rows
    /// stay as orphans (§10.1).
    pub fn delete(&self, space: &Space, receipt: Option<[u8; 32]>, now_ms: i64) -> DeleteOutcome {
        self.delete_keep(space, receipt, now_ms, false)
    }

    /// [`Spaces::delete`], or with `keep` the deletion of the web panel: the folder is renamed
    /// into `spaces.trash/` (no byte copied, no in-place removal as a fallback) and recorded in
    /// `space_trash` in the same transaction of `routing.db` as the receipt; the space is gone for
    /// every device exactly as for a plain deletion.
    pub fn delete_keep(
        &self,
        space: &Space,
        receipt: Option<[u8; 32]>,
        now_ms: i64,
        keep: bool,
    ) -> DeleteOutcome {
        let keep_dir = keep.then(|| {
            self.data_dir
                .join(TRASH_DIR)
                .join(format!("{}-{}", space.text_id(), now_ms / 1000))
        });
        let destroyed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.destroy(space, keep_dir.as_deref())
        }))
        .unwrap_or_else(|_| {
            Err(DestroyFail::Unknown(anyhow::anyhow!(
                "the deletion of the space panicked"
            )))
        });
        let removed = match destroyed {
            Ok(removed) => removed,
            Err(DestroyFail::NothingDeleted(e)) => {
                tracing::warn!(
                    space = space.short_id(),
                    "a space could not be deleted, nothing was deleted: {}",
                    short_error(&e)
                );
                return match self.reinstate(space) {
                    Ok(()) => {
                        tracing::warn!(space = space.short_id(), "the space is registered again");
                        DeleteOutcome::NotDeleted
                    }
                    Err(e) => {
                        tracing::error!(
                            space = space.short_id(),
                            "the space cannot be opened again, it is unavailable until the next start: {}",
                            short_error(&e)
                        );
                        DeleteOutcome::Unknown
                    }
                };
            }
            Err(DestroyFail::Absent(e)) => {
                tracing::error!(
                    space = space.short_id(),
                    "a space to delete is not on the disk (volume not mounted?), nothing was deleted; it is unavailable until the next start: {}",
                    short_error(&e)
                );
                return DeleteOutcome::Unknown;
            }
            Err(DestroyFail::NotDurable(e)) => {
                tracing::error!(
                    space = space.short_id(),
                    "a space was deleted but the deletion could not be made durable; it is unavailable until the next start: {}",
                    short_error(&e)
                );
                return DeleteOutcome::NotDurable;
            }
            Err(DestroyFail::Unknown(e)) => {
                tracing::error!(
                    space = space.short_id(),
                    "the deletion of a space ended in an unknown state; it is unavailable until the next start: {}",
                    short_error(&e)
                );
                return DeleteOutcome::Unknown;
            }
        };

        // 5. The space cannot come back: one transaction of routing.db forgets its quota and
        // the invitation it consumed, and records the receipt.
        let trash_row = match (&removed, keep) {
            (Removed::Renamed { trash }, true) => Some(TrashRow {
                dir_name: trash
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                space_id: space.text_id(),
                deleted_at: now_ms,
                purge_after: now_ms.saturating_add(TRASH_TTL_MS),
                size_bytes: dir_size(trash),
                quota: self.quota_override(&space.id),
            }),
            _ => None,
        };
        let receipted = self.forget_in_routing(space, receipt, now_ms, trash_row.as_ref());
        if let Err(why) = &receipted {
            tracing::error!(
                space = space.short_id(),
                "a space was deleted but routing.db could not record it (no receipt; its quota and invitation stay): {why}"
            );
        }

        // 6. Its identifiers are free again.
        {
            let mut pending = plain(self.pending.lock());
            plain(self.index.lock()).detach(&space.id);
            pending.remove(&space.id);
        }
        plain(self.overrides.lock()).remove(&space.id);

        // 7. What is left on the disk, at best (a space kept in the trash stays where it is).
        if !keep {
            self.remove_leftovers(space, &removed);
        }

        if receipted.is_ok() {
            DeleteOutcome::Deleted
        } else {
            DeleteOutcome::DeletedWithoutReceipt
        }
    }

    /// Step 5 of §4.4.1: one transaction of `routing.db`.
    fn forget_in_routing(
        &self,
        space: &Space,
        receipt: Option<[u8; 32]>,
        now_ms: i64,
        trash: Option<&TrashRow>,
    ) -> Result<(), String> {
        self.destroy_fault(DestroyStep::ForgetRouting)
            .map_err(|e| e.kind().to_string())?;
        let text = space.text_id();
        let hash = receipt.map(|r| crate::wire::sha256(&r));
        self.routing
            .write_sync(|tx| {
                tx.execute(
                    "DELETE FROM space_quotas WHERE space_id = ?1",
                    params![text],
                )?;
                crate::install::forget_space(tx, &text)?;
                if let Some(h) = hash {
                    tx.execute(
                        "INSERT OR IGNORE INTO space_deletions (receipt_hash, deleted_at) VALUES (?1, ?2)",
                        params![h.to_vec(), now_ms],
                    )?;
                }
                if let Some(t) = trash {
                    tx.execute(
                        "INSERT OR REPLACE INTO space_trash
                         (dir_name, space_id, deleted_at, purge_after, size_bytes, quota_set, quota_bytes)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            t.dir_name,
                            t.space_id,
                            t.deleted_at,
                            t.purge_after,
                            i64::try_from(t.size_bytes).unwrap_or(i64::MAX),
                            i64::from(t.quota.is_some()),
                            t.quota.flatten().and_then(|q| i64::try_from(q).ok()),
                        ],
                    )?;
                }
                Ok(Tx::Commit(()))
            })
            .map_err(|e| e.code.as_str().to_owned())
    }

    /// Step 7 of §4.4.1: removes the renamed folder, or what an in-place removal left in
    /// `spaces/<id>/`. A failure is a warning, never a failure of the call: a renamed folder
    /// that resists is removed at the next start, and the debris of an in-place removal has no
    /// `relay.db` and is skipped at start (it is cleared if this `space_id` is created again).
    fn remove_leftovers(&self, space: &Space, removed: &Removed) {
        let doomed = match removed {
            Removed::Renamed { trash } => trash.as_path(),
            Removed::InPlace => space.dir.as_path(),
        };
        let gone = self
            .destroy_fault(DestroyStep::RemoveDirectory)
            .and_then(|()| std::fs::remove_dir_all(doomed));
        if let Err(e) = gone {
            tracing::warn!(
                space = space.short_id(),
                "a deleted space left files on the disk ({}); they are never loaded as a space",
                e.kind()
            );
        }
    }

    /// Registers again a space that nothing was deleted from, as [`Spaces::open`] does.
    fn reinstate(&self, old: &Space) -> anyhow::Result<()> {
        self.destroy_fault(DestroyStep::Reopen)?;
        let file = old.dir.join(SPACE_DB_FILE);
        // Only an existing database is opened again, and by identity: the folder must be the one
        // the space was loaded from, not a copy that took its place (see [`Identity`]).
        let identity = match std::fs::symlink_metadata(&old.dir)
            .and_then(|d| std::fs::symlink_metadata(&file).map(|f| (d, f)))
        {
            Ok((d, f)) if d.is_dir() && f.is_file() => Identity::of(&d, &f),
            _ => bail!("the database is no longer on the disk"),
        };
        if identity != old.identity {
            bail!("the space folder is not the one that was loaded");
        }
        // Without `SQLITE_OPEN_CREATE`: a file that vanishes right now is an error, never a new
        // empty database.
        let db = Db::open_existing(&file).context("cannot open the database again")?;
        let space = db
            .read_sync(|tx| store::space_row(tx))
            .map_err(|e| anyhow::anyhow!("cannot read the space: {e}"))
            .and_then(|row| match row {
                Some(row) if row.space_id == old.id => Ok(Arc::new(Space {
                    id: old.id,
                    created_at: row.created_at,
                    dir: old.dir.clone(),
                    db: db.clone(),
                    identity,
                })),
                _ => bail!("the database is not the one of the space"),
            })
            .and_then(|space| {
                self.attach(&space)
                    .map_err(|why| anyhow::anyhow!("routing conflict: {why}"))?;
                Ok(space)
            });
        let space = match space {
            Ok(s) => s,
            Err(e) => {
                db.close();
                return Err(e);
            }
        };
        let mut pending = plain(self.pending.lock());
        plain(self.map.write()).insert(old.id, space);
        pending.remove(&old.id);
        Ok(())
    }

    /// Steps 2 to 4 of §4.4.1: closes the database of a pending space, checks that it is on the
    /// disk, does the **positive act** and makes it durable. Blocking; called by
    /// [`Spaces::delete`], which does steps 5 to 7.
    ///
    /// 2. presence, read just before acting and without following a symbolic link:
    ///    `spaces/<id>/` is a directory and `spaces/<id>/relay.db` a regular file, **and they
    ///    are the very ones the space was loaded from** (same device and inode, [`Identity`];
    ///    Unix). Otherwise (volume not mounted, lazily unmounted with an old copy underneath,
    ///    wrong `data_dir`, folder moved by hand) nothing is touched;
    /// 3. the point of no return: the folder is **renamed** out of `spaces/` (a blob upload that
    ///    still holds the space cannot put a file back under `spaces/<id>/`: the directory is
    ///    gone, and `blobstore::ensure_parent` does not recreate it). If the rename is refused
    ///    (a file still open on Windows), `relay.db` is removed in place, then its journal
    ///    files: a start skips a folder without `relay.db`. **Only the success of one of these
    ///    two system calls counts**: `ENOENT` is never a success, it is an unknown state;
    /// 4. the directories whose entries changed are synced (Unix), so that a crash right after
    ///    cannot bring the folder back while `routing.db` has recorded the deletion.
    fn destroy(&self, space: &Space, keep_dir: Option<&Path>) -> Result<Removed, DestroyFail> {
        use std::io::ErrorKind::NotFound;
        space.db.close();

        // 2. Presence.
        self.destroy_fault(DestroyStep::Presence)
            .map_err(|e| DestroyFail::Absent(anyhow::anyhow!("presence check: {}", e.kind())))?;
        let file = space.dir.join(SPACE_DB_FILE);
        let looked = std::fs::symlink_metadata(&space.dir)
            .map(|dir| (dir, std::fs::symlink_metadata(&file)));
        match looked {
            Ok((dir, Ok(db))) if dir.is_dir() && db.is_file() => {
                // By identity, not by path alone: whatever the path leads to must be the folder
                // and the database this space was loaded from (see [`Identity`]).
                if Identity::of(&dir, &db) != space.identity {
                    return Err(DestroyFail::Absent(anyhow::anyhow!(
                        "the space folder is not the one that was loaded (volume unmounted or replaced?)"
                    )));
                }
            }
            Ok((dir, _)) if !dir.is_dir() => {
                return Err(DestroyFail::Absent(anyhow::anyhow!(
                    "the space folder is not a directory"
                )))
            }
            Ok((_, Ok(_))) => {
                return Err(DestroyFail::Absent(anyhow::anyhow!(
                    "the space database is not a regular file"
                )))
            }
            Ok((_, Err(e))) | Err(e) if e.kind() == NotFound => {
                return Err(DestroyFail::Absent(anyhow::anyhow!(
                    "the space folder or its database is missing"
                )))
            }
            Ok((_, Err(e))) | Err(e) => {
                return Err(DestroyFail::Unknown(anyhow::anyhow!(
                    "presence check: {}",
                    e.kind()
                )))
            }
        }

        // 3. The positive act.
        let text = space.text_id();
        let random = crate::wire::random_bytes::<4>()
            .map(hex::encode)
            .unwrap_or_default();
        let trash = match keep_dir {
            Some(dir) => dir.to_path_buf(),
            None => self.data_dir.join(format!("{TRASH_PREFIX}{text}-{random}")),
        };
        let renamed = self.destroy_fault(DestroyStep::Rename).and_then(|()| {
            if let Some(parent) = trash.parent().filter(|_| keep_dir.is_some()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&space.dir, &trash)
        });
        let removed = match renamed {
            Ok(()) => Removed::Renamed { trash },
            Err(e) if e.kind() == NotFound => {
                return Err(DestroyFail::Unknown(anyhow::anyhow!(
                    "the space folder vanished during the rename"
                )))
            }
            // A space kept in the trash is renamed or not deleted: no in-place removal.
            Err(refused) if keep_dir.is_some() => {
                return Err(DestroyFail::NothingDeleted(anyhow::anyhow!(
                    "rename into the trash refused ({})",
                    refused.kind()
                )))
            }
            Err(refused) => {
                let in_place = self
                    .destroy_fault(DestroyStep::RemoveDatabase)
                    .and_then(|()| std::fs::remove_file(&file));
                match in_place {
                    Ok(()) => {
                        for suffix in ["-wal", "-shm"] {
                            let _ = std::fs::remove_file(
                                space.dir.join(format!("{SPACE_DB_FILE}{suffix}")),
                            );
                        }
                        Removed::InPlace
                    }
                    Err(e) if e.kind() == NotFound => {
                        return Err(DestroyFail::Unknown(anyhow::anyhow!(
                            "the space database vanished during its removal"
                        )))
                    }
                    Err(e) => {
                        return Err(DestroyFail::NothingDeleted(anyhow::anyhow!(
                            "rename refused ({}), in-place removal refused ({})",
                            refused.kind(),
                            e.kind()
                        )))
                    }
                }
            }
        };

        // 4. Durability.
        let changed: Vec<PathBuf> = match &removed {
            Removed::Renamed { trash } => {
                let mut v = vec![self.data_dir.clone(), self.data_dir.join(SPACES_DIR)];
                if keep_dir.is_some() {
                    v.extend(trash.parent().map(Path::to_path_buf));
                }
                v
            }
            Removed::InPlace => vec![space.dir.clone()],
        };
        for dir in &changed {
            self.destroy_fault(DestroyStep::SyncDir)
                .and_then(|()| sync_dir(dir))
                .map_err(|e| DestroyFail::NotDurable(anyhow::anyhow!("fsync: {}", e.kind())))?;
        }
        Ok(removed)
    }

    /// Brings a space of the trash back (`admin trash restore`): the folder goes back to
    /// `spaces/<id>/`, the space is loaded and registered, which rebuilds its routing (devices,
    /// signing keys, sessions and channels are read from its database), and its own quota is given
    /// back. Refused, nothing changed, if the id is taken or if something of the space belongs to
    /// another space now. Blocking; the caller holds the registry lock and deletes the row of
    /// `space_trash`.
    pub fn restore_from_trash(&self, row: &TrashRow) -> Result<Arc<Space>, RestoreError> {
        let fail = |e: &dyn std::fmt::Display| RestoreError::Failed(e.to_string());
        let id = uuid_parse(&row.space_id).ok_or(RestoreError::Missing)?;
        if !matches!(self.route_space(&id), Route::Unknown) {
            return Err(RestoreError::IdTaken);
        }
        let src = self.data_dir.join(TRASH_DIR).join(&row.dir_name);
        if !src.join(SPACE_DB_FILE).is_file() {
            return Err(RestoreError::Missing);
        }
        let dst = space_dir(&self.data_dir, &id);
        self.clear_unloaded_folder(&dst).map_err(|e| match e.code {
            Code::SpaceExists => RestoreError::IdTaken,
            _ => RestoreError::Failed(e.code.as_str().to_owned()),
        })?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| fail(&e.kind()))?;
        }
        std::fs::rename(&src, &dst).map_err(|e| fail(&e.kind()))?;
        let back = |db: Option<&Arc<Db>>| {
            if let Some(db) = db {
                db.close();
            }
            let _ = std::fs::rename(&dst, &src);
        };
        let file = dst.join(SPACE_DB_FILE);
        let db = match Db::open_existing(&file) {
            Ok(db) => db,
            Err(e) => {
                back(None);
                return Err(fail(&sqlite_reason(&e)));
            }
        };
        let space_row = match db.read_sync(|tx| store::space_row(tx)) {
            Ok(Some(r)) if r.space_id == id => r,
            _ => {
                back(Some(&db));
                return Err(RestoreError::Missing);
            }
        };
        let identity = match Identity::read(&dst) {
            Ok(i) => i,
            Err(e) => {
                back(Some(&db));
                return Err(fail(&e.kind()));
            }
        };
        let space = Arc::new(Space {
            id,
            created_at: space_row.created_at,
            dir: dst.clone(),
            db: db.clone(),
            identity,
        });
        if let Err(why) = self.attach(&space) {
            back(Some(&db));
            return Err(RestoreError::Conflict(why));
        }
        plain(self.map.write()).insert(id, space.clone());
        if let Some(q) = row.quota {
            if let Err(e) = self.set_quota_override(&id, Some(q)) {
                tracing::warn!(
                    "restore: the quota of the space could not be given back ({})",
                    e.code.as_str()
                );
            }
        }
        for d in [dst.parent(), src.parent()].into_iter().flatten() {
            let _ = sync_dir(d);
        }
        Ok(space)
    }

    /// Before a space is created in `dir`, which no registered or pending space owns: a folder
    /// left there is the debris of a creation that was interrupted, or of an in-place removal,
    /// and is removed. **Unless** it holds a `relay.db` with a `space` row: that is a space the
    /// start did not load (restored by hand, say), and nothing is touched (`409 space_exists`).
    /// A database that cannot be read is not touched either (`500`).
    fn clear_unloaded_folder(&self, dir: &Path) -> Result<(), ApiError> {
        let file = dir.join(SPACE_DB_FILE);
        match std::fs::symlink_metadata(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(_) => {
                let db = Db::open_read_only(&file, crate::db::SCHEMA_VERSION)
                    .map_err(|_| ApiError::internal("an unloaded space database cannot be read"))?;
                if let Some(db) = db {
                    let row = db.read_sync(|tx| store::space_row(tx));
                    db.close();
                    match row {
                        Ok(None) => {}
                        Ok(Some(_)) => {
                            return Err(ApiError::new(Code::SpaceExists)
                                .detail("This space already exists."))
                        }
                        Err(_) => {
                            return Err(ApiError::internal(
                                "an unloaded space database cannot be read",
                            ))
                        }
                    }
                }
            }
        }
        match std::fs::remove_dir_all(dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Closes every database (shutdown, tests that move the files).
    pub fn close_all(&self) {
        for s in self.list() {
            s.db.close();
        }
        self.routing.close();
    }

    // ----- quota ------------------------------------------------------------------------

    /// The override of `admin quota`: `None` if the space has none, `Some(None)` if it was
    /// explicitly set to no limit.
    pub fn quota_override(&self, id: &SpaceId) -> Option<Option<u64>> {
        plain(self.overrides.lock()).get(id).copied()
    }

    /// The quota of a space: its override (`admin quota`), else the default quota
    /// ([`Spaces::default_space_quota`]). `None`: no limit of its own.
    pub fn space_max(&self, id: &SpaceId, cfg: &Config) -> Option<u64> {
        self.quota_override(id)
            .unwrap_or_else(|| self.default_space_quota(cfg).0)
    }

    /// Whether the quota of a space is its own (an override of `admin quota`, `none` included) or
    /// the default of the relay.
    pub fn quota_source(&self, id: &SpaceId) -> &'static str {
        if self.quota_override(id).is_some() {
            "space"
        } else {
            "default"
        }
    }

    /// What the web panel set as the default quota: `None` if nothing, `Some(None)` for "no limit
    /// per space".
    pub fn panel_default_quota(&self) -> Option<Option<u64>> {
        *plain(self.default_quota.lock())
    }

    /// The default quota of a space that has none of its own, and where it comes from: what the
    /// web panel set (`"panel"`), else `max_space_bytes` (`"config"`), else no limit (`"none"`).
    /// `None` bytes: no limit per space (also what the panel's `0` means, with source `"panel"`).
    pub fn default_space_quota(&self, cfg: &Config) -> (Option<u64>, &'static str) {
        match self.panel_default_quota() {
            Some(v) => (v, "panel"),
            None => (
                cfg.configured_space_quota(),
                cfg.configured_space_quota_source(),
            ),
        }
    }

    /// Sets (`Some`) or removes (`None`: back to the configuration, else no limit) the
    /// default quota of the web panel. `Some(None)` is "no limit per space". Persisted first, then
    /// applied: the next reservation of any space without an override of its own sees it. The
    /// caller validated the number (`admin::parse_default_quota`).
    pub fn set_default_quota(&self, value: Option<Option<u64>>, now: i64) -> Result<(), ApiError> {
        let stored = value.map(|v| v.map_or(0, |b| i64::try_from(b).unwrap_or(i64::MAX)));
        // Held across the write so that two changes cannot land in the database in one order and
        // in memory in the other.
        let mut current = plain(self.default_quota.lock());
        self.routing.write_sync(|tx| {
            match stored {
                None => {
                    tx.execute("DELETE FROM default_space_quota WHERE id = 1", [])?;
                }
                Some(max) => {
                    tx.execute(
                        "INSERT INTO default_space_quota (id, max_bytes, updated_at) VALUES (1, ?1, ?2)
                         ON CONFLICT(id) DO UPDATE SET max_bytes = ?1, updated_at = ?2",
                        params![max, now],
                    )?;
                }
            }
            Ok(Tx::Commit(()))
        })?;
        *current = value;
        Ok(())
    }

    /// Sets (`Some`) or removes (`None`) the override of a space. `Some(None)` is "no limit".
    pub fn set_quota_override(
        &self,
        id: &SpaceId,
        value: Option<Option<u64>>,
    ) -> Result<(), ApiError> {
        let text = uuid_format(id);
        let stored = value.map(|v| v.map(|b| i64::try_from(b).unwrap_or(i64::MAX)));
        self.routing.write_sync(|tx| {
            match stored {
                None => {
                    tx.execute(
                        "DELETE FROM space_quotas WHERE space_id = ?1",
                        params![text],
                    )?;
                }
                Some(max) => {
                    tx.execute(
                        "INSERT INTO space_quotas (space_id, max_bytes) VALUES (?1, ?2)
                         ON CONFLICT(space_id) DO UPDATE SET max_bytes = ?2",
                        params![text, max],
                    )?;
                }
            }
            Ok(Tx::Commit(()))
        })?;
        let mut o = plain(self.overrides.lock());
        match value {
            None => {
                o.remove(id);
            }
            Some(v) => {
                o.insert(*id, v);
            }
        }
        Ok(())
    }

    /// `used_bytes` of every space but `except`, summed. Never called from inside a database
    /// closure: it takes the read connections of the other spaces one at a time.
    pub fn others_used(&self, except: Option<&SpaceId>) -> Result<u64, ApiError> {
        let mut total = 0u64;
        for s in self.list() {
            if Some(&s.id) == except {
                continue;
            }
            total = total.saturating_add(s.db.read_sync(|tx| store::used_bytes(tx))?);
        }
        Ok(total)
    }

    /// The guard of the reservations of quota.
    pub(crate) fn serialize_reservations(&self) -> std::sync::MutexGuard<'_, ()> {
        lock(&self.reserve)
    }

    /// Whether a quota warning for `key` may be given now (at most once per `every_ms`).
    pub fn may_warn(&self, key: Option<SpaceId>, now_ms: i64, every_ms: i64) -> bool {
        let mut w = plain(self.warned.lock());
        match w.get(&key) {
            Some(last) if now_ms.saturating_sub(*last) < every_ms => false,
            _ => {
                w.insert(key, now_ms);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> SpaceId {
        [n; 16]
    }

    fn found(devices: &[&str], keys: &[&[u8]]) -> Entries {
        Entries {
            devices: devices.iter().map(|d| (*d).to_owned()).collect(),
            keys: keys.iter().map(|k| k.to_vec()).collect(),
            ..Entries::default()
        }
    }

    #[test]
    fn an_identifier_in_two_spaces_is_refused_and_adds_nothing() {
        let mut ix = Index::default();
        ix.attach(id(1), &found(&["d1"], &[b"k1"])).unwrap();
        // Same space again: fine (idempotent).
        ix.attach(id(1), &found(&["d1"], &[b"k1"])).unwrap();
        let err = ix
            .attach(id(2), &found(&["d2", "d1"], &[b"k2"]))
            .unwrap_err();
        assert!(err.contains("two spaces"), "{err}");
        assert!(!ix.devices.contains_key("d2"), "nothing was added");
        assert!(!ix.keys.contains_key(b"k2".as_slice()));
        let err = ix.attach(id(3), &found(&["d3"], &[b"k1"])).unwrap_err();
        assert!(err.contains("key"), "{err}");
        ix.detach(&id(1));
        ix.attach(id(2), &found(&["d1"], &[b"k1"])).unwrap();
    }

    #[test]
    fn prefixes_need_eight_characters() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::open(dir.path()).unwrap();
        assert!(spaces.is_empty());
        assert_eq!(spaces.find("abc").err(), Some(Find::Unusable));
        assert_eq!(spaces.find("zzzzzzzz").err(), Some(Find::Unusable));
        assert_eq!(spaces.find("abcdef01").err(), Some(Find::NotFound));
    }

    #[test]
    fn quota_overrides_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::for_tests(dir.path());
        let spaces = Spaces::open(dir.path()).unwrap();
        let (space, ()) = spaces
            .create(id(7), |tx| {
                tx.execute(
                    "INSERT INTO space (id, space_id, created_at, key_epoch) VALUES (1, ?1, 5, 1)",
                    params![id(7).to_vec()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(spaces.space_max(&space.id, &cfg), None);
        spaces
            .set_quota_override(&space.id, Some(Some(99)))
            .unwrap();
        assert_eq!(spaces.space_max(&space.id, &cfg), Some(99));
        spaces.close_all();
        drop(spaces);

        let spaces = Spaces::open(dir.path()).unwrap();
        assert_eq!(spaces.len_for_tests(), 1);
        assert_eq!(spaces.space_max(&id(7), &cfg), Some(99));
        // "No limit" is an override too: it beats a default from the configuration.
        let with_default = Config {
            max_space_bytes: Some(10),
            ..cfg
        };
        spaces.set_quota_override(&id(7), Some(None)).unwrap();
        assert_eq!(spaces.space_max(&id(7), &with_default), None);
        spaces.set_quota_override(&id(7), None).unwrap();
        assert_eq!(spaces.space_max(&id(7), &with_default), Some(10));
        spaces.close_all();
    }

    #[test]
    fn a_creation_that_fails_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::open(dir.path()).unwrap();
        let err = spaces
            .create(id(9), |_| Err::<(), _>(ApiError::not_found()))
            .unwrap_err();
        assert_eq!(err.code, Code::NotFound);
        assert!(spaces.is_empty());
        assert!(!space_dir(dir.path(), &id(9)).exists());
    }

    #[test]
    fn may_warn_is_throttled_per_key() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Spaces::open(dir.path()).unwrap();
        assert!(spaces.may_warn(None, 1_000, 100));
        assert!(!spaces.may_warn(None, 1_050, 100));
        assert!(spaces.may_warn(Some(id(1)), 1_050, 100), "another key");
        assert!(spaces.may_warn(None, 1_100, 100));
    }

    impl Spaces {
        fn len_for_tests(&self) -> usize {
            self.list().len()
        }
    }
}
