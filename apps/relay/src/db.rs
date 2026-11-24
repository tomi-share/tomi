//! Server-side SQLite (`api-v1.md` §10, annex A): WAL, `foreign_keys = ON`,
//! `synchronous = FULL`, **one writer** (a connection behind a mutex) and a few read-only
//! connections. Blobs live on disk, never in the database.
//!
//! There are two kinds of database: one `relay.db` **per space** ([`SCHEMA`], one space row,
//! `WHERE id = 1` is true inside each of them) and one global `routing.db` ([`ROUTING_SCHEMA`]:
//! pending invitations, per-space quota overrides, push health). Each has its own
//! `user_version`.
//!
//! This is a server database, unrelated to the client store of `tomi-core`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::ApiError;

/// Version of a space database, stored in `PRAGMA user_version`.
///
/// Two shapes share version 1: a database migrated from the one-space relay 0.1.1 keeps its
/// `push_health` / `push_events` tables, a new one does not have them (the push bookkeeping lives
/// in `routing.db`). Nothing reads them; the next schema version should `DROP TABLE IF EXISTS` both.
pub const SCHEMA_VERSION: i64 = 1;

/// Version of `routing.db`, stored in its own `PRAGMA user_version`.
pub const ROUTING_SCHEMA_VERSION: i64 = 1;

/// Number of read-only connections.
const READERS: usize = 3;

/// Schema of a space database (annex A) plus the columns the idempotence rules need
/// (`items.blob_set_hash`, `blobs.pos`, `rotations.revoked_ids`). The FCM bookkeeping of
/// `admin status` is global: it lives in `routing.db`.
pub const SCHEMA: &str = r"
CREATE TABLE space (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  space_id            BLOB NOT NULL,
  created_at          INTEGER NOT NULL,
  key_epoch           INTEGER NOT NULL,
  rotation_required   INTEGER NOT NULL DEFAULT 0,
  retention_floor_seq INTEGER NOT NULL DEFAULT 0,
  last_seq            INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE devices (
  device_id        TEXT PRIMARY KEY,
  name             TEXT NOT NULL,
  platform         TEXT NOT NULL,
  sign_pk          BLOB UNIQUE,
  kx_pk            BLOB,
  certificate      BLOB,
  issuer_device_id TEXT NOT NULL,
  joined_seq       INTEGER NOT NULL,
  registered_at    INTEGER NOT NULL,
  last_seen_at     INTEGER,
  present_until    INTEGER,
  format_min       INTEGER,
  format_max       INTEGER,
  app_version      TEXT,
  cursor_hint      INTEGER,
  revoked_at       INTEGER
);
CREATE TABLE revoked_sign_pks (sign_pk BLOB PRIMARY KEY);

CREATE TABLE sessions (
  token_hash   BLOB PRIMARY KEY,
  device_id    TEXT NOT NULL REFERENCES devices(device_id),
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  revoked      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sessions_device ON sessions(device_id);

CREATE TABLE challenges (
  challenge    BLOB PRIMARY KEY,
  device_id    TEXT NOT NULL,
  expires_at   INTEGER NOT NULL
);
CREATE INDEX challenges_device ON challenges(device_id);

CREATE TABLE events (
  seq          INTEGER PRIMARY KEY,
  type         TEXT NOT NULL,
  at           INTEGER NOT NULL,
  author       TEXT,
  item_id      BLOB,
  data         TEXT NOT NULL
);
CREATE INDEX events_item ON events(item_id);
CREATE INDEX events_at ON events(at);

CREATE TABLE items (
  item_id        BLOB PRIMARY KEY,
  seq            INTEGER NOT NULL,
  sender         TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  expires_at     INTEGER NOT NULL,
  key_epoch      INTEGER NOT NULL,
  envelope       BLOB,
  envelope_size  INTEGER NOT NULL,
  envelope_hash  BLOB NOT NULL,
  blob_set_hash  BLOB NOT NULL,
  purged_at      INTEGER
);
CREATE INDEX items_expires ON items(expires_at) WHERE purged_at IS NULL;
CREATE INDEX items_purged ON items(purged_at) WHERE purged_at IS NOT NULL;

CREATE TABLE item_rings (
  item_id   BLOB NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  device_id TEXT NOT NULL,
  ring      INTEGER NOT NULL,
  PRIMARY KEY (item_id, device_id)
);

CREATE TABLE receipts (
  item_id      BLOB NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  device_id    TEXT NOT NULL,
  received_at  INTEGER NOT NULL,
  read_at      INTEGER,
  released_at  INTEGER,
  PRIMARY KEY (item_id, device_id)
);

CREATE TABLE tombstones (
  item_id      BLOB PRIMARY KEY,
  seq          INTEGER NOT NULL,
  device_id    TEXT NOT NULL,
  deleted_at   INTEGER NOT NULL,
  statement    BLOB NOT NULL
);

CREATE TABLE item_states (
  event_id       BLOB PRIMARY KEY,
  seq            INTEGER NOT NULL,
  device_id      TEXT NOT NULL,
  key_epoch      INTEGER NOT NULL,
  envelope_size  INTEGER NOT NULL,
  envelope_hash  BLOB NOT NULL
);

CREATE TABLE blobs (
  blob_id        BLOB PRIMARY KEY,
  owner          TEXT NOT NULL,
  size           INTEGER NOT NULL,
  upload_offset  INTEGER NOT NULL DEFAULT 0,
  complete       INTEGER NOT NULL DEFAULT 0,
  item_id        BLOB REFERENCES items(item_id),
  transfer_id    BLOB REFERENCES transfers(transfer_id),
  pos            INTEGER NOT NULL DEFAULT 0,
  last_activity  INTEGER NOT NULL,
  CHECK (item_id IS NULL OR transfer_id IS NULL)
);
CREATE INDEX blobs_item ON blobs(item_id);
CREATE INDEX blobs_transfer ON blobs(transfer_id);
CREATE INDEX blobs_orphan ON blobs(last_activity) WHERE item_id IS NULL AND transfer_id IS NULL;

CREATE TABLE transfers (
  transfer_id   BLOB PRIMARY KEY,
  from_device   TEXT NOT NULL,
  to_device     TEXT NOT NULL,
  manifest      BLOB NOT NULL,
  blob_set_hash BLOB NOT NULL,
  as_of_seq     INTEGER NOT NULL,
  seq           INTEGER NOT NULL,
  created_at    INTEGER NOT NULL,
  expires_at    INTEGER NOT NULL,
  closed_at     INTEGER,
  outcome       TEXT
);
CREATE INDEX transfers_open ON transfers(expires_at) WHERE closed_at IS NULL;

CREATE TABLE rotations (
  key_epoch    INTEGER PRIMARY KEY,
  device_id    TEXT NOT NULL,
  created_at   INTEGER NOT NULL,
  seq          INTEGER NOT NULL,
  statement    BLOB NOT NULL,
  revoked_ids  TEXT NOT NULL
);

CREATE TABLE pairing_channels (
  channel_id   TEXT PRIMARY KEY,
  creator      TEXT NOT NULL,
  state        TEXT NOT NULL,
  token_hash   BLOB,
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL
);
CREATE TABLE pairing_messages (
  channel_id   TEXT NOT NULL REFERENCES pairing_channels(channel_id) ON DELETE CASCADE,
  idx          INTEGER NOT NULL,
  side         TEXT NOT NULL,
  body         BLOB NOT NULL,
  PRIMARY KEY (channel_id, idx)
);

CREATE TABLE push_registrations (
  device_id    TEXT PRIMARY KEY REFERENCES devices(device_id),
  provider     TEXT NOT NULL,
  token        TEXT NOT NULL UNIQUE,
  updated_at   INTEGER NOT NULL
);
";

/// Schema of `routing.db`: what is global to the relay and cannot be derived from the space
/// databases. The lookup tables (device, key, session, channel -> space) are not stored: they are
/// rebuilt from the space databases at every start (`spaces.rs`).
///
/// An invitation is pending while `consumed_at` is NULL. Once `POST /v1/space` has used it, the
/// row stays (with the date and the `space_id` it created) so that a founder whose `201` was lost
/// is told `space_exists` when it retries (`api-v1.md` §3.2). `admin reset` of that space deletes
/// the row.
pub const ROUTING_SCHEMA: &str = r"
CREATE TABLE invitations (
  token_hash   BLOB PRIMARY KEY,
  created_at   INTEGER NOT NULL,
  consumed_at  INTEGER,
  space_id     TEXT
);

CREATE TABLE space_quotas (
  space_id     TEXT PRIMARY KEY,
  max_bytes    INTEGER
);

CREATE TABLE push_health (
  id              INTEGER PRIMARY KEY CHECK (id = 1),
  last_success_at INTEGER,
  last_error_at   INTEGER,
  last_error      TEXT,
  state           TEXT NOT NULL
);

CREATE TABLE push_events (
  at    INTEGER NOT NULL,
  kind  TEXT NOT NULL
);
CREATE INDEX push_events_at ON push_events(at);
";

/// The tables of the web administration panel (`admin_web.rs`), in `routing.db`. An **additive**
/// migration: every statement is `CREATE TABLE IF NOT EXISTS`, run at every open of a writable
/// `routing.db`, and `user_version` stays at 1. A relay of the previous version opens such a
/// database unchanged (it ignores the tables), and the panel reads an older database that lacks
/// them as "nothing recorded". Each table holds at most one row.
///
/// * `admin_web_path`: the secret path segment (128 random bits, base64url). Not a credential:
///   the passkey is. It is kept in clear because `tomi-relay admin web` must show the URL.
/// * `admin_web_credential`: the one passkey (credential id, raw public key, signature counter).
/// * `admin_web_enrollment`: the one-time enrolment code, as a SHA-256 only, with its deadline and
///   the attempts left.
pub const ADMIN_WEB_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS admin_web_path (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  segment     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_web_credential (
  id             INTEGER PRIMARY KEY CHECK (id = 1),
  credential_id  BLOB NOT NULL,
  public_key     BLOB NOT NULL,
  sign_count     INTEGER NOT NULL,
  created_at     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_web_enrollment (
  id             INTEGER PRIMARY KEY CHECK (id = 1),
  code_hash      BLOB NOT NULL,
  expires_at     INTEGER NOT NULL,
  attempts_left  INTEGER NOT NULL
);
";

/// The actions of the web panel (`admin_web.rs`, `admin_actions.rs`, `trash.rs`), in `routing.db`.
/// An **additive** migration like [`ADMIN_WEB_SCHEMA`] (`CREATE ... IF NOT EXISTS`, `user_version`
/// stays at 1; a relay of the previous version ignores these tables).
///
/// * `admin_web_credentials`: the passkeys, up to 5. `num` is an `AUTOINCREMENT`: a number is
///   never reused, even after a key was removed or `admin web reset`, so that "key 2" in the audit
///   log always designates the same key. `backup_eligible` is the BE flag the authenticator
///   declared at enrolment (no attestation: a declaration, not a proof). The first key also stays
///   in `admin_web_credential` (see `sync_legacy` in `admin_web.rs`) so that the previous binary
///   still starts. A database that only knows `admin_web_credential` gets its key copied here once.
/// * `admin_web_policy`: at most one row; `critical_requires_device_bound` (set by the CLI only).
/// * `admin_web_audit`: what was done through the panel or about its access: a date, a fixed word,
///   the 8-character id of the target and the number of the passkey. Never a name, an address, a
///   token or a value of a request. Bounded by `admin_web::AUDIT_MAX_ROWS` and `AUDIT_MAX_AGE_MS`.
/// * `space_trash`: the spaces deleted from the panel, kept 7 days in `spaces.trash/<dir_name>/`
///   (renamed, not copied) so that the CLI can restore them. `quota_set`/`quota_bytes` keep the
///   quota of the space itself (`quota_bytes` NULL: no limit).
pub const ADMIN_ACTIONS_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS admin_web_credentials (
  num              INTEGER PRIMARY KEY AUTOINCREMENT,
  credential_id    BLOB NOT NULL UNIQUE,
  public_key       BLOB NOT NULL,
  sign_count       INTEGER NOT NULL,
  backup_eligible  INTEGER NOT NULL DEFAULT 1,
  created_at       INTEGER NOT NULL,
  last_used_at     INTEGER
);

INSERT INTO admin_web_credentials (credential_id, public_key, sign_count, backup_eligible, created_at)
SELECT credential_id, public_key, sign_count, 1, created_at FROM admin_web_credential
WHERE NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'admin_web_credentials');

CREATE TABLE IF NOT EXISTS admin_web_policy (
  id                             INTEGER PRIMARY KEY CHECK (id = 1),
  critical_requires_device_bound INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_web_audit (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  at      INTEGER NOT NULL,
  kind    TEXT NOT NULL,
  target  TEXT,
  key_num INTEGER
);

CREATE TABLE IF NOT EXISTS space_trash (
  dir_name     TEXT PRIMARY KEY,
  space_id     TEXT NOT NULL,
  deleted_at   INTEGER NOT NULL,
  purge_after  INTEGER NOT NULL,
  size_bytes   INTEGER NOT NULL,
  quota_set    INTEGER NOT NULL DEFAULT 0,
  quota_bytes  INTEGER
);
";

/// The receipts of the deletions of spaces (`api-v1.md` §4.4.2, §10.2), in `routing.db`. An
/// **additive** migration like [`ADMIN_WEB_SCHEMA`]: `user_version` stays at 1, and a database
/// opened read-only that lacks the table is simply one without receipts. A row holds
/// `SHA-256(deletion_id)` and the time of the deletion, nothing else (no `space_id`, no device);
/// the maintenance deletes it after `deletion_receipt_ttl_secs`.
pub const DELETIONS_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS space_deletions (
  receipt_hash  BLOB PRIMARY KEY,
  deleted_at    INTEGER NOT NULL
);
";

/// The default quota of a space, set from the web panel (`admin_web.rs`), in `routing.db`. An
/// **additive** migration like [`ADMIN_WEB_SCHEMA`]: `CREATE TABLE IF NOT EXISTS`, `user_version`
/// stays at 1, a relay of the previous version opens such a database unchanged (it ignores the
/// table), and a database opened read-only that lacks it is simply one where the panel never set
/// a default. At most one row; **no row** means "nothing was set from the panel" (the
/// configuration, then the built-in default, apply). `max_bytes = 0` means "no limit per space".
/// It is a number of bytes and a date, nothing about a space.
pub const DEFAULT_QUOTA_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS default_space_quota (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  max_bytes   INTEGER NOT NULL CHECK (max_bytes >= 0),
  updated_at  INTEGER NOT NULL
);
";

/// The removals of one attachment (`api-v1.md` §4.8 bis, §10.2), in each space database. An
/// **additive** migration like [`ADMIN_WEB_SCHEMA`]: `CREATE TABLE IF NOT EXISTS`, run at every
/// writable open of a space database, `user_version` stays at 1. A relay of the previous version
/// opens such a database unchanged (it ignores the table). Like `tombstones`, a row survives the
/// purge of its item (the purge of an item must not make the next replay a second event) and
/// goes with the compaction of its event (`event_retention_days`).
pub const ATTACHMENT_TOMBSTONES_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS attachment_tombstones (
  item_id      BLOB NOT NULL,
  slot         INTEGER NOT NULL,
  blob_id      BLOB NOT NULL,
  seq          INTEGER NOT NULL,
  device_id    TEXT NOT NULL,
  deleted_at   INTEGER NOT NULL,
  statement    BLOB NOT NULL,
  PRIMARY KEY (item_id, slot)
) WITHOUT ROWID;
";

/// Poison-tolerant lock: a panicking request must not take the relay down with it.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a write closure decides: commit and return a value, or commit and then fail
/// (the challenge of `POST /v1/auth/session` is consumed whatever the result, §3.3).
pub enum Tx<R> {
    Commit(R),
    CommitThenFail(ApiError),
}

/// The database handle. After [`Db::close`] every access fails with `503 unavailable`.
pub struct Db {
    writer: Mutex<Option<Connection>>,
    readers: Vec<Mutex<Option<Connection>>>,
    next_reader: AtomicUsize,
    fault: AtomicBool,
    path: PathBuf,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

impl Db {
    /// Opens (and creates) the database of a space at `path`.
    pub fn open(path: &Path) -> Result<Arc<Db>, rusqlite::Error> {
        Self::open_with(
            path,
            SCHEMA,
            SCHEMA_VERSION,
            &[ATTACHMENT_TOMBSTONES_SCHEMA],
            true,
        )
    }

    /// Opens the database of a space that is expected to be on the disk already, for writing:
    /// **never creates the file** (`SQLITE_OPEN_CREATE` is not given), so a file that vanished
    /// since it was looked at is an error, not a new empty database.
    pub fn open_existing(path: &Path) -> Result<Arc<Db>, rusqlite::Error> {
        Self::open_with(
            path,
            SCHEMA,
            SCHEMA_VERSION,
            &[ATTACHMENT_TOMBSTONES_SCHEMA],
            false,
        )
    }

    /// Opens (and creates) `routing.db` at `path`.
    pub fn open_routing(path: &Path) -> Result<Arc<Db>, rusqlite::Error> {
        Self::open_with(
            path,
            ROUTING_SCHEMA,
            ROUTING_SCHEMA_VERSION,
            &[
                ADMIN_WEB_SCHEMA,
                ADMIN_ACTIONS_SCHEMA,
                DELETIONS_SCHEMA,
                DEFAULT_QUOTA_SCHEMA,
            ],
            true,
        )
    }

    /// Opens an existing space database for reading only: no writer, no `journal_mode` change,
    /// no schema creation (a command that holds no lock must write nothing). `None` when the file
    /// holds no schema yet (`user_version` 0). Every write on the handle fails with `503`.
    pub fn open_read_only(path: &Path, expected: i64) -> Result<Option<Arc<Db>>, rusqlite::Error> {
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let r = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure(&r)?;
            readers.push(Mutex::new(Some(r)));
        }
        let version: i64 = {
            let guard = lock(&readers[0]);
            let conn = guard.as_ref().ok_or(rusqlite::Error::InvalidQuery)?;
            conn.query_row("PRAGMA user_version", [], |r| r.get(0))?
        };
        if version == 0 {
            return Ok(None);
        }
        if version != expected {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {version} is not supported (expected {expected})"
            )));
        }
        Ok(Some(Arc::new(Db {
            writer: Mutex::new(None),
            readers,
            next_reader: AtomicUsize::new(0),
            fault: AtomicBool::new(false),
            path: path.to_path_buf(),
        })))
    }

    /// `routing.db` for a command that holds no lock: read-only if the file exists and has a
    /// schema, otherwise an empty in-memory routing database (nothing is created on disk).
    pub fn open_routing_read_only(path: &Path) -> Result<Arc<Db>, rusqlite::Error> {
        if path.is_file() {
            if let Some(db) = Self::open_read_only(path, ROUTING_SCHEMA_VERSION)? {
                return Ok(db);
            }
        }
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let r = Connection::open_in_memory()?;
            r.execute_batch(ROUTING_SCHEMA)?;
            r.execute_batch(ADMIN_WEB_SCHEMA)?;
            r.execute_batch(ADMIN_ACTIONS_SCHEMA)?;
            r.execute_batch(DELETIONS_SCHEMA)?;
            r.execute_batch(DEFAULT_QUOTA_SCHEMA)?;
            readers.push(Mutex::new(Some(r)));
        }
        Ok(Arc::new(Db {
            writer: Mutex::new(None),
            readers,
            next_reader: AtomicUsize::new(0),
            fault: AtomicBool::new(false),
            path: path.to_path_buf(),
        }))
    }

    /// `additive` is applied at every open, after the version check: only `CREATE ... IF NOT EXISTS`
    /// statements (see [`ADMIN_WEB_SCHEMA`], [`DELETIONS_SCHEMA`], [`DEFAULT_QUOTA_SCHEMA`],
    /// [`ATTACHMENT_TOMBSTONES_SCHEMA`]). `create`: whether a missing
    /// file may be created (see [`Db::open_existing`]).
    fn open_with(
        path: &Path,
        schema: &str,
        expected: i64,
        additive: &[&str],
        create: bool,
    ) -> Result<Arc<Db>, rusqlite::Error> {
        let mut flags = OpenFlags::default();
        if !create {
            flags.remove(OpenFlags::SQLITE_OPEN_CREATE);
        }
        let writer = Connection::open_with_flags(path, flags)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "foreign_keys", "ON")?;
        writer.pragma_update(None, "synchronous", "FULL")?;
        let version: i64 = writer.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            writer.execute_batch(schema)?;
            writer.pragma_update(None, "user_version", expected)?;
        } else if version != expected {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "database schema version {version} is not supported (expected {expected})"
            )));
        }
        for batch in additive {
            writer.execute_batch(batch)?;
        }
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let r = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            configure(&r)?;
            readers.push(Mutex::new(Some(r)));
        }
        Ok(Arc::new(Db {
            writer: Mutex::new(Some(writer)),
            readers,
            next_reader: AtomicUsize::new(0),
            fault: AtomicBool::new(false),
            path: path.to_path_buf(),
        }))
    }

    /// Path of the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Closes every connection (waiting for the statements in flight): the files can then be
    /// moved or deleted (`admin reset`), which Windows refuses while they are open. Idempotent.
    /// A closed handle answers `503 unavailable`.
    pub fn close(&self) {
        *lock(&self.writer) = None;
        for r in &self.readers {
            *lock(r) = None;
        }
    }

    /// Test hook: makes every access fail with `503 unavailable` (a broken database).
    pub fn inject_fault(&self, on: bool) {
        self.fault.store(on, Ordering::SeqCst);
    }

    fn check_fault(&self) -> Result<(), ApiError> {
        if self.fault.load(Ordering::SeqCst) {
            Err(ApiError::unavailable())
        } else {
            Ok(())
        }
    }

    /// Runs `f` in an `IMMEDIATE` transaction on the single writer (blocking).
    pub fn write_sync<R>(
        &self,
        f: impl FnOnce(&Transaction) -> Result<Tx<R>, ApiError>,
    ) -> Result<R, ApiError> {
        self.check_fault()?;
        let mut guard = lock(&self.writer);
        let conn = guard.as_mut().ok_or_else(ApiError::unavailable)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match f(&tx)? {
            Tx::Commit(v) => {
                tx.commit()?;
                Ok(v)
            }
            Tx::CommitThenFail(e) => {
                tx.commit()?;
                Err(e)
            }
        }
    }

    /// Async wrapper of [`Db::write_sync`] for closures that return a plain value.
    pub async fn write<R, F>(self: &Arc<Self>, f: F) -> Result<R, ApiError>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction) -> Result<R, ApiError> + Send + 'static,
    {
        self.write_with(move |tx| f(tx).map(Tx::Commit)).await
    }

    /// Async write whose closure may ask for "commit, then fail".
    pub async fn write_with<R, F>(self: &Arc<Self>, f: F) -> Result<R, ApiError>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction) -> Result<Tx<R>, ApiError> + Send + 'static,
    {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || db.write_sync(f))
            .await
            .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    }

    /// Runs `f` in a read transaction (one consistent snapshot) on a read-only connection.
    pub fn read_sync<R>(
        &self,
        f: impl FnOnce(&Transaction) -> Result<R, ApiError>,
    ) -> Result<R, ApiError> {
        self.check_fault()?;
        let i = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        let mut guard = lock(&self.readers[i]);
        let conn = guard.as_mut().ok_or_else(ApiError::unavailable)?;
        let tx = conn.transaction()?;
        f(&tx)
    }

    /// Async wrapper of [`Db::read_sync`].
    pub async fn read<R, F>(self: &Arc<Self>, f: F) -> Result<R, ApiError>
    where
        R: Send + 'static,
        F: FnOnce(&Transaction) -> Result<R, ApiError> + Send + 'static,
    {
        let db = Arc::clone(self);
        tokio::task::spawn_blocking(move || db.read_sync(f))
            .await
            .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_applies_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        {
            let db = Db::open(&p).unwrap();
            db.write_sync(|tx| {
                tx.execute(
                    "INSERT INTO space (id, space_id, created_at, key_epoch) VALUES (1, x'00', 1, 1)",
                    [],
                )?;
                Ok(Tx::Commit(()))
            })
            .unwrap();
        }
        let db = Db::open(&p).unwrap();
        let n: i64 = db
            .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM space", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 1);
        let mode: String = db
            .read_sync(|tx| Ok(tx.query_row("PRAGMA journal_mode", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn rollback_on_error_and_commit_then_fail() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let r: Result<(), _> = db.write_sync(|tx| {
            tx.execute(
                "INSERT INTO challenges (challenge, device_id, expires_at) VALUES (x'01', 'a', 1)",
                [],
            )?;
            Err(ApiError::not_found())
        });
        assert!(r.is_err());
        let r: Result<(), _> = db.write_sync(|tx| {
            tx.execute(
                "INSERT INTO challenges (challenge, device_id, expires_at) VALUES (x'02', 'b', 2)",
                [],
            )?;
            Ok(Tx::CommitThenFail(ApiError::unauthorized()))
        });
        assert!(r.is_err());
        let n: i64 = db
            .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM challenges", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 1, "the first write rolled back, the second committed");
    }

    #[test]
    fn routing_database_has_its_own_schema_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("routing.db");
        {
            let db = Db::open_routing(&p).unwrap();
            db.write_sync(|tx| {
                tx.execute(
                    "INSERT INTO invitations (token_hash, created_at) VALUES (x'01', 5)",
                    [],
                )?;
                Ok(Tx::Commit(()))
            })
            .unwrap();
            db.close();
        }
        let db = Db::open_routing(&p).unwrap();
        // A routing database has no space in it.
        assert!(db
            .read_sync(|tx| Ok(
                tx.query_row("SELECT count(*) FROM space", [], |r| r.get::<_, i64>(0))?
            ))
            .is_err());
        let n: i64 = db
            .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM invitations", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 1);
        let v: i64 = db
            .read_sync(|tx| Ok(tx.query_row("PRAGMA user_version", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(v, ROUTING_SCHEMA_VERSION);
    }

    #[test]
    fn a_read_only_open_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // No file: an empty routing database in memory, and nothing appears on disk.
        let p = dir.path().join("routing.db");
        let db = Db::open_routing_read_only(&p).unwrap();
        let n: i64 = db
            .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM invitations", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(
            db.write_sync(|_| Ok(Tx::Commit(()))).unwrap_err().code,
            crate::error::Code::Unavailable
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // An existing database is read, not modified.
        {
            let rw = Db::open_routing(&p).unwrap();
            rw.write_sync(|tx| {
                tx.execute(
                    "INSERT INTO invitations (token_hash, created_at) VALUES (x'01', 5)",
                    [],
                )?;
                Ok(Tx::Commit(()))
            })
            .unwrap();
            rw.close();
        }
        let before = std::fs::read(&p).unwrap();
        let ro = Db::open_routing_read_only(&p).unwrap();
        let n: i64 = ro
            .read_sync(|tx| Ok(tx.query_row("SELECT count(*) FROM invitations", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 1);
        assert!(ro.write_sync(|_| Ok(Tx::Commit(()))).is_err());
        ro.close();
        assert_eq!(std::fs::read(&p).unwrap(), before);

        // A file with no schema is `None`, and is left blank.
        let blank = dir.path().join("blank.db");
        Connection::open(&blank).unwrap();
        assert!(Db::open_read_only(&blank, SCHEMA_VERSION)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_closed_database_is_unavailable_and_its_file_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        let db = Db::open(&p).unwrap();
        db.close();
        db.close();
        assert_eq!(
            db.read_sync(|_| Ok(())).unwrap_err().code,
            crate::error::Code::Unavailable
        );
        assert_eq!(
            db.write_sync(|_| Ok(Tx::Commit(()))).unwrap_err().code,
            crate::error::Code::Unavailable
        );
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn fault_injection_returns_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        db.inject_fault(true);
        let e = db.read_sync(|_| Ok(())).unwrap_err();
        assert_eq!(e.code, crate::error::Code::Unavailable);
    }
}
