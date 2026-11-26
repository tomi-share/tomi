//! Local persistence: one SQLite database per installation (`api-v1.md` §7.1, `format-v1.md` §7).
//!
//! * **Engine**: SQLite through `rusqlite` with the `bundled` feature (SQLite compiled from
//!   source, no system library). WAL journal, `synchronous = FULL` for the sync cursor.
//! * **Secrets**: device keys, the local key `LSK`, space keys and history keys are stored
//!   in the database *sealed* under a 32-byte wrapping key supplied by the host (Android
//!   Keystore / DPAPI). The database file alone reveals none of them.
//! * **Content**: readable items are stored re-sealed under `LSK` ([`crate::LocalKey`],
//!   spec §7.2), with their original payload bytes, so unknown fields survive. Only
//!   unreadable items keep their raw envelope. Columns in the clear are limited to what the
//!   feed needs without decrypting (ids, dates, flags); titles and text are never clear.
//! * **Attachments** (`format-v1.md` §7.2): at rest, a file is only ever kept *encrypted*
//!   (`files/{item}_{idx}.enc`, the blob as on the relay, under its `FK` from the sealed
//!   payload, or under the transfer key `FK'` sealed in `item_files` for an imported item).
//!   Decryption happens on demand into `files/cache/`, purged when the item goes away and
//!   by [`Engine::purge_plaintext_cache`](super::Engine::purge_plaintext_cache).
//! * **Metadata in the clear, accepted** (not content, but visible to whoever reads the
//!   file without the wrapping key): item / blob / event / transfer ids; dates (creation,
//!   reception, relay expiry, user expiry, pin and archive time, tombstones, receipts); the `secret`
//!   flag, the `read_policy`, read / kept / pinned / archived flags, send status, key epochs,
//!   attachment sizes and indexes; device names, platforms and public keys; the trust store
//!   (public keys only); the sync cursor; the raw bytes of an outgoing `PinStatement` or `ArchiveStatement`
//!   (`outbox.aux`: item id, decision, time, signer public key and signature); envelope /
//!   tombstone bytes awaiting sending (sealed, but their clear headers); diagnostics codes.
//!   Device keys, `LSK`, space keys, history keys and `FK'` are sealed under the wrapping
//!   key; titles, text, URLs, file names and file contents are sealed under `LSK` or `FK`.
//! * **Transactions** (`api-v1.md` §7.1): [`Store::tx`] runs a closure in one SQLite
//!   transaction; the engine applies an event and advances the cursor in the same one.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use zeroize::Zeroizing;

use crate::archive::{ArchiveBook, ArchiveState};
use crate::crypto::{
    aead_open, aead_seal, labels, random_array, to_array, Writer, KEY_LEN, NONCE_LEN,
};
use crate::error::{CoreError, Result};
use crate::keyring::{KeyRingRecord, Supersession};
use crate::pin::{PinBook, PinState};
use crate::runtime::transport::BlobRef;
use crate::UnreadableReason;

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Storage {
            reason: e.to_string(),
        }
    }
}

impl From<std::io::Error> for CoreError {
    fn from(e: std::io::Error) -> Self {
        CoreError::Storage {
            reason: format!("io: {}", e.kind()),
        }
    }
}

/// Current schema version (`PRAGMA user_version`).
pub const SCHEMA_VERSION: i64 = 4;

/// v1 -> v2: count of `stale_key_epoch` answers per operation (bounded resync).
const MIGRATE_V2: &str = "ALTER TABLE outbox ADD COLUMN stale_attempts INTEGER NOT NULL DEFAULT 0;";

/// v2 -> v3: the archive registry (format-v1 §8.9), the twin of `pins`. Additive: no existing
/// table or column changes.
const MIGRATE_V3: &str = "
CREATE TABLE archives (
  item_id TEXT PRIMARY KEY,
  archived INTEGER NOT NULL,
  at_ms INTEGER NOT NULL
) WITHOUT ROWID;
";

/// v3 -> v4: the registry of removed attachments (format-v1 §8.10), independent of the items
/// like `tombstones` and `pins`, and a new `outbox.kind` (`attachment_tombstone`). The `CHECK`
/// of `kind` cannot be altered in place: `outbox` is rebuilt (same columns, same `op_id`s, same
/// `AUTOINCREMENT` counter, same indexes). Nothing references `outbox` by a foreign key.
const MIGRATE_V4: &str = "
CREATE TABLE attachment_removals (
  item_id TEXT NOT NULL,
  idx INTEGER NOT NULL,
  blob_id TEXT NOT NULL,
  scope TEXT NOT NULL CHECK (scope IN ('local','everywhere')),
  at_ms INTEGER NOT NULL,
  recorded_at_ms INTEGER NOT NULL,
  PRIMARY KEY (item_id, idx)
) WITHOUT ROWID;

CREATE TABLE outbox_v4 (
  op_id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK (kind IN
    ('send_item','tombstone','ack','read','item_state','attachment_tombstone')),
  item_id TEXT,
  event_id TEXT,
  payload BLOB,
  aux BLOB,
  flag INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','done')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL,
  stale_attempts INTEGER NOT NULL DEFAULT 0
);
INSERT INTO outbox_v4 (op_id, kind, item_id, event_id, payload, aux, flag, status, attempts,
  next_attempt_at_ms, last_error, created_at_ms, stale_attempts)
  SELECT op_id, kind, item_id, event_id, payload, aux, flag, status, attempts,
  next_attempt_at_ms, last_error, created_at_ms, stale_attempts FROM outbox;
-- Keeps the counter of the old table (ids of finished operations are never reused).
UPDATE sqlite_sequence SET seq = MAX(seq,
  COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'outbox'), 0))
  WHERE name = 'outbox_v4';
INSERT INTO sqlite_sequence (name, seq)
  SELECT 'outbox_v4', seq FROM sqlite_sequence WHERE name = 'outbox'
  AND NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'outbox_v4');
DROP TABLE outbox;
ALTER TABLE outbox_v4 RENAME TO outbox;
CREATE INDEX outbox_due ON outbox (status, next_attempt_at_ms, op_id);
CREATE INDEX outbox_item ON outbox (item_id);
";

/// Name (relative to the files directory) of the encrypted copy of an attachment at rest.
pub(crate) fn encrypted_file_name(item_id: &str, idx: u32) -> String {
    format!("files/{item_id}_{idx}.enc")
}

/// Name (relative to the files directory) of the decrypted copy, made on demand.
pub(crate) fn cache_file_name(item_id: &str, idx: u32) -> String {
    format!("{CACHE_DIR}/{item_id}_{idx}")
}

/// Directory (relative to the files directory) of decrypted attachments.
pub(crate) const CACHE_DIR: &str = "files/cache";

/// Top-level sub-directories of the files directory (`files/cache` is inside `files`).
const FILE_DIRS: [&str; 5] = ["files", "outbox", "dl", "export", "import"];

const SCHEMA_V1: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;
-- Secrets sealed under the host wrapping key: 'device_keys', 'local_key'.
CREATE TABLE secrets (name TEXT PRIMARY KEY, record BLOB NOT NULL) WITHOUT ROWID;
-- Space key ring: key sealed (NULL once discarded) + rotation that superseded the epoch.
CREATE TABLE space_keys (
  epoch INTEGER PRIMARY KEY,
  key_sealed BLOB,
  superseded_seq INTEGER,
  superseded_applied_at_ms INTEGER
);
-- TrustStore.to_json (public data only).
CREATE TABLE trust (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);
-- Sync cursor (last applied seq of the relay log).
CREATE TABLE sync_state (id INTEGER PRIMARY KEY CHECK (id = 1), cursor INTEGER NOT NULL);
INSERT INTO sync_state (id, cursor) VALUES (1, 0);

CREATE TABLE items (
  item_id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,                    -- seq of the origin 'item' event (0 = not sent yet)
  origin TEXT NOT NULL CHECK (origin IN ('received','sent','imported')),
  sender_device_id TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  received_at_ms INTEGER NOT NULL,
  relay_expires_at_ms INTEGER,
  key_epoch INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN
    ('readable','update_required','key_pending','suspect_epoch','undecryptable')),
  sealed BLOB,                             -- LocalKey record (readable items)
  raw_envelope BLOB,                       -- original envelope (unreadable items only)
  payload_expires_at_ms INTEGER,           -- user expiration, local erasure
  secret INTEGER NOT NULL DEFAULT 0,
  read_policy TEXT NOT NULL DEFAULT 'until_expiry',
  is_read INTEGER NOT NULL DEFAULT 0,
  kept INTEGER NOT NULL DEFAULT 0,
  blobs_json TEXT NOT NULL DEFAULT '[]',   -- blobs announced by the relay for this item
  ring INTEGER NOT NULL DEFAULT 0,
  send_status TEXT CHECK (send_status IN ('queued','sent','expired_before_send','failed'))
);
CREATE INDEX items_feed ON items (created_at_ms DESC, seq DESC);
CREATE INDEX items_state ON items (state);

-- One row per attachment of an item. Imported items carry the fresh key FK' of the
-- transferred copy (sealed); relay items use the key inside their payload.
CREATE TABLE item_files (
  item_id TEXT NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  idx INTEGER NOT NULL,
  blob_id TEXT,
  size INTEGER NOT NULL,
  key_sealed BLOB,
  nonce_prefix BLOB,
  state TEXT NOT NULL CHECK (state IN ('remote','done','unavailable')),
  path TEXT,
  PRIMARY KEY (item_id, idx)
) WITHOUT ROWID;

CREATE TABLE receipts (
  item_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  received_at_ms INTEGER,
  read_at_ms INTEGER,
  PRIMARY KEY (item_id, device_id)
) WITHOUT ROWID;

-- Items deleted everywhere ('everywhere') or on this device ('local'); kept 45 days.
CREATE TABLE tombstones (
  item_id TEXT PRIMARY KEY,
  deleted_at_ms INTEGER NOT NULL,
  scope TEXT NOT NULL CHECK (scope IN ('local','everywhere')),
  recorded_at_ms INTEGER NOT NULL
) WITHOUT ROWID;

-- PinBook, independent of items (format-v1 §8.8.3).
CREATE TABLE pins (
  item_id TEXT PRIMARY KEY,
  pinned INTEGER NOT NULL,
  at_ms INTEGER NOT NULL
) WITHOUT ROWID;

-- Item-state envelopes kept raw until a key or an update makes them readable.
CREATE TABLE pending_states (
  event_id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  key_epoch INTEGER NOT NULL,
  envelope BLOB NOT NULL
) WITHOUT ROWID;

-- Persistent outgoing queue (api-v1 §7.7).
CREATE TABLE outbox (
  op_id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL CHECK (kind IN ('send_item','tombstone','ack','read','item_state')),
  item_id TEXT,
  event_id TEXT,
  payload BLOB,                            -- envelope / tombstone bytes
  aux BLOB,                                -- item_state: PinStatement or ArchiveStatement bytes
  flag INTEGER NOT NULL DEFAULT 0,         -- ack: release
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','done')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at_ms INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at_ms INTEGER NOT NULL
);
CREATE INDEX outbox_due ON outbox (status, next_attempt_at_ms, op_id);
CREATE INDEX outbox_item ON outbox (item_id);

-- Encrypted blobs staged on disk for upload (items and history transfers).
CREATE TABLE blob_uploads (
  ord INTEGER PRIMARY KEY AUTOINCREMENT,
  blob_id TEXT NOT NULL UNIQUE,
  owner_kind TEXT NOT NULL CHECK (owner_kind IN ('op','transfer')),
  owner_id TEXT NOT NULL,
  size INTEGER NOT NULL,
  path TEXT NOT NULL
);
CREATE INDEX blob_uploads_owner ON blob_uploads (owner_kind, owner_id);

CREATE TABLE devices (
  device_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  platform TEXT NOT NULL,
  ed25519 BLOB,
  x25519 BLOB,
  state TEXT NOT NULL CHECK (state IN ('verified','unverified','revoked','revoked_by_relay')),
  joined_seq INTEGER NOT NULL DEFAULT 0,
  last_seen_at_ms INTEGER
) WITHOUT ROWID;

-- Resumable history transfer tasks, both directions (api-v1 §7.8).
CREATE TABLE transfers (
  transfer_id TEXT PRIMARY KEY,
  role TEXT NOT NULL CHECK (role IN ('export','import')),
  peer_device_id TEXT NOT NULL,
  state TEXT NOT NULL,
  as_of_seq INTEGER NOT NULL DEFAULT 0,
  history_key_sealed BLOB,
  temp_key_sealed BLOB,                    -- export: key of the local temporary index
  manifest BLOB,
  index_path TEXT,
  index_len INTEGER NOT NULL DEFAULT 0,
  items_total INTEGER NOT NULL DEFAULT 0,
  items_done INTEGER NOT NULL DEFAULT 0,
  skipped_unreadable INTEGER NOT NULL DEFAULT 0,
  files_total INTEGER NOT NULL DEFAULT 0,
  files_done INTEGER NOT NULL DEFAULT 0,
  outcome TEXT,
  error TEXT,
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL
) WITHOUT ROWID;

-- Events ignored or refused (no content), for support.
CREATE TABLE diagnostics (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at_ms INTEGER NOT NULL,
  kind TEXT NOT NULL,
  detail TEXT NOT NULL
);
";

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// What the erasure checks in its own transaction ([`Store::wipe_everything`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WipeGuard {
    /// A departure: `meta.leave_state` must still be `requested` or `deleting` (a cancellation
    /// that won the race erases nothing).
    DepartureMarker,
    /// "Erase anyway" (D4): no marker to check.
    Unconditional,
}

/// How an item came to this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemOrigin {
    Received,
    Sent,
    Imported,
}

impl ItemOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ItemOrigin::Received => "received",
            ItemOrigin::Sent => "sent",
            ItemOrigin::Imported => "imported",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "received" => ItemOrigin::Received,
            "sent" => ItemOrigin::Sent,
            "imported" => ItemOrigin::Imported,
            _ => return Err(corrupt("item origin")),
        })
    }
}

/// Readable or unreadable ("illisible"), never deleted for that reason (invariant 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Readable,
    Unreadable(UnreadableReason),
}

impl ItemStatus {
    fn as_str(self) -> &'static str {
        match self {
            ItemStatus::Readable => "readable",
            ItemStatus::Unreadable(r) => match r {
                UnreadableReason::UpdateRequired => "update_required",
                UnreadableReason::KeyPending => "key_pending",
                UnreadableReason::SuspectEpoch => "suspect_epoch",
                UnreadableReason::Undecryptable => "undecryptable",
            },
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "readable" => ItemStatus::Readable,
            "update_required" => ItemStatus::Unreadable(UnreadableReason::UpdateRequired),
            "key_pending" => ItemStatus::Unreadable(UnreadableReason::KeyPending),
            "suspect_epoch" => ItemStatus::Unreadable(UnreadableReason::SuspectEpoch),
            "undecryptable" => ItemStatus::Unreadable(UnreadableReason::Undecryptable),
            _ => return Err(corrupt("item state")),
        })
    }
}

/// Delivery state of an item written by this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    Queued,
    Sent,
    ExpiredBeforeSend,
    Failed,
}

impl SendStatus {
    fn as_str(self) -> &'static str {
        match self {
            SendStatus::Queued => "queued",
            SendStatus::Sent => "sent",
            SendStatus::ExpiredBeforeSend => "expired_before_send",
            SendStatus::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "queued" => SendStatus::Queued,
            "sent" => SendStatus::Sent,
            "expired_before_send" => SendStatus::ExpiredBeforeSend,
            "failed" => SendStatus::Failed,
            _ => return Err(corrupt("send status")),
        })
    }
}

/// A row of `items`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    pub item_id: String,
    pub seq: u64,
    pub origin: ItemOrigin,
    pub sender_device_id: String,
    pub created_at_ms: i64,
    pub received_at_ms: i64,
    pub relay_expires_at_ms: Option<i64>,
    pub key_epoch: u32,
    pub status: ItemStatus,
    /// `LocalKey` record, readable items only.
    pub sealed: Option<Vec<u8>>,
    /// Original envelope, unreadable items only.
    pub raw_envelope: Option<Vec<u8>>,
    pub payload_expires_at_ms: Option<i64>,
    pub secret: bool,
    /// `after_first_read` | `until_expiry` (`ReadPolicy` wire names).
    pub read_policy: String,
    pub read: bool,
    pub kept: bool,
    pub blobs: Vec<BlobRef>,
    pub ring: bool,
    pub send_status: Option<SendStatus>,
}

/// State of the local copy of one attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// Not downloaded (still on the relay or in a transfer).
    Remote,
    /// Decrypted file on disk.
    Done,
    /// Not available anywhere (not transferred, or gone).
    Unavailable,
}

impl FileState {
    fn as_str(self) -> &'static str {
        match self {
            FileState::Remote => "remote",
            FileState::Done => "done",
            FileState::Unavailable => "unavailable",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "remote" => FileState::Remote,
            "done" => FileState::Done,
            "unavailable" => FileState::Unavailable,
            _ => return Err(corrupt("file state")),
        })
    }
}

/// A row of `item_files`. `key`/`nonce_prefix` override the attachment of the payload
/// (imported items only).
#[derive(Clone, PartialEq, Eq)]
pub struct FileRow {
    pub item_id: String,
    pub idx: u32,
    pub blob_id: Option<String>,
    pub size: u64,
    pub key: Option<Vec<u8>>,
    pub nonce_prefix: Option<Vec<u8>>,
    pub state: FileState,
    /// Path relative to the files directory.
    pub path: Option<String>,
}

// Never prints the file key (AGENTS.md golden rule 3).
impl std::fmt::Debug for FileRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileRow")
            .field("item_id", &self.item_id)
            .field("idx", &self.idx)
            .field("blob_id", &self.blob_id)
            .field("size", &self.size)
            .field("has_key", &self.key.is_some())
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// A row of `attachment_removals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRemovalRow {
    pub item_id: String,
    pub idx: u32,
    /// Blob id of the attachment in the payload the item was sent with.
    pub blob_id: String,
    pub everywhere: bool,
}

/// Kind of outgoing operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    SendItem,
    Tombstone,
    Ack,
    Read,
    ItemState,
    /// Signed removal of one attachment (format-v1 §8.10); `payload` = wire bytes. Carries no
    /// `item_id`: it survives the erasure of the item it is about.
    AttachmentTombstone,
}

impl OpKind {
    fn as_str(self) -> &'static str {
        match self {
            OpKind::SendItem => "send_item",
            OpKind::Tombstone => "tombstone",
            OpKind::Ack => "ack",
            OpKind::Read => "read",
            OpKind::ItemState => "item_state",
            OpKind::AttachmentTombstone => "attachment_tombstone",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "send_item" => OpKind::SendItem,
            "tombstone" => OpKind::Tombstone,
            "ack" => OpKind::Ack,
            "read" => OpKind::Read,
            "item_state" => OpKind::ItemState,
            "attachment_tombstone" => OpKind::AttachmentTombstone,
            _ => return Err(corrupt("outbox kind")),
        })
    }
}

/// An operation to insert into the outbox.
#[derive(Debug, Clone)]
pub struct NewOp {
    pub kind: OpKind,
    pub item_id: Option<String>,
    pub event_id: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub aux: Option<Vec<u8>>,
    pub flag: bool,
}

impl NewOp {
    pub fn new(kind: OpKind) -> Self {
        Self {
            kind,
            item_id: None,
            event_id: None,
            payload: None,
            aux: None,
            flag: false,
        }
    }
}

/// `(device_id, received_at_ms, read_at_ms)`.
pub type ReceiptRow = (String, Option<i64>, Option<i64>);

/// `(event_id, seq, key_epoch, envelope)` of a kept item-state envelope.
pub type PendingState = (String, u64, u32, Vec<u8>);

/// Raw columns of an `outbox` row.
type RawOp = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    bool,
    u32,
    i64,
    Option<String>,
    u32,
);

/// A row of `outbox`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRow {
    pub op_id: i64,
    pub kind: OpKind,
    pub item_id: Option<String>,
    pub event_id: Option<String>,
    pub payload: Option<Vec<u8>>,
    pub aux: Option<Vec<u8>>,
    pub flag: bool,
    pub attempts: u32,
    pub next_attempt_at_ms: i64,
    pub last_error: Option<String>,
    /// `stale_key_epoch` answers received for this operation.
    pub stale_attempts: u32,
}

/// A staged blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobUpload {
    pub blob_id: String,
    pub size: u64,
    /// Relative to the files directory.
    pub path: String,
}

/// State of a known device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Verified,
    Unverified,
    Revoked,
    RevokedByRelay,
}

impl DeviceState {
    fn as_str(self) -> &'static str {
        match self {
            DeviceState::Verified => "verified",
            DeviceState::Unverified => "unverified",
            DeviceState::Revoked => "revoked",
            DeviceState::RevokedByRelay => "revoked_by_relay",
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "verified" => DeviceState::Verified,
            "unverified" => DeviceState::Unverified,
            "revoked" => DeviceState::Revoked,
            "revoked_by_relay" => DeviceState::RevokedByRelay,
            _ => return Err(corrupt("device state")),
        })
    }
}

/// A row of `devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub ed25519: Option<Vec<u8>>,
    pub x25519: Option<Vec<u8>>,
    pub state: DeviceState,
    pub joined_seq: u64,
    pub last_seen_at_ms: Option<i64>,
}

/// A row of `transfers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRow {
    pub transfer_id: String,
    /// `export` or `import`.
    pub role: String,
    pub peer_device_id: String,
    pub state: String,
    pub as_of_seq: u64,
    pub history_key_sealed: Option<Vec<u8>>,
    pub temp_key_sealed: Option<Vec<u8>>,
    pub manifest: Option<Vec<u8>>,
    pub index_path: Option<String>,
    pub index_len: u64,
    pub items_total: u64,
    pub items_done: u64,
    pub skipped_unreadable: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl TransferRow {
    pub fn new(transfer_id: &str, role: &str, peer: &str, state: &str, now_ms: i64) -> Self {
        Self {
            transfer_id: transfer_id.to_owned(),
            role: role.to_owned(),
            peer_device_id: peer.to_owned(),
            state: state.to_owned(),
            as_of_seq: 0,
            history_key_sealed: None,
            temp_key_sealed: None,
            manifest: None,
            index_path: None,
            index_len: 0,
            items_total: 0,
            items_done: 0,
            skipped_unreadable: 0,
            files_total: 0,
            files_done: 0,
            outcome: None,
            error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }
}

fn corrupt(what: &str) -> CoreError {
    CoreError::Storage {
        reason: format!("corrupt value: {what}"),
    }
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The database plus the directory of staged / downloaded files.
pub struct Store {
    conn: Mutex<Connection>,
    files_dir: PathBuf,
    wrap: Zeroizing<[u8; KEY_LEN]>,
    /// When the write-ahead log was last truncated ([`Store::truncate_wal_if_due`]).
    last_wal_truncate: Mutex<Option<Instant>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("files_dir", &self.files_dir)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (creating and migrating if needed) the database at `db_path`. `wrap_key` is the
    /// 32-byte key from the OS secure storage that seals the secrets of the database.
    pub fn open(db_path: &Path, files_dir: &Path, wrap_key: &[u8]) -> Result<Self> {
        let wrap = Zeroizing::new(to_array::<KEY_LEN>(wrap_key, "wrapping key")?);
        Self::prepare_dirs(files_dir)?;
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        Self::from_connection(Connection::open(db_path)?, files_dir, wrap)
    }

    /// In-memory database (tests).
    pub fn open_in_memory(files_dir: &Path, wrap_key: &[u8]) -> Result<Self> {
        let wrap = Zeroizing::new(to_array::<KEY_LEN>(wrap_key, "wrapping key")?);
        Self::prepare_dirs(files_dir)?;
        Self::from_connection(Connection::open_in_memory()?, files_dir, wrap)
    }

    fn prepare_dirs(files_dir: &Path) -> Result<()> {
        for sub in ["files", CACHE_DIR, "outbox", "dl", "export", "import"] {
            std::fs::create_dir_all(files_dir.join(sub))?;
        }
        Ok(())
    }

    fn from_connection(
        conn: Connection,
        files_dir: &Path,
        wrap: Zeroizing<[u8; KEY_LEN]>,
    ) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        // For the whole life of the connection: a deleted or consumed record (an "after first
        // read" secret, an expired item) must not stay readable in a free page until something
        // happens to overwrite it. Cheap here: the records are small and sealed.
        conn.pragma_update(None, "secure_delete", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(&format!(
                    "BEGIN;{SCHEMA_V1}{MIGRATE_V2}{MIGRATE_V3}{MIGRATE_V4}PRAGMA user_version = {SCHEMA_VERSION};COMMIT;"
                ))?;
            }
            1 => {
                conn.execute_batch(&format!(
                    "BEGIN;{MIGRATE_V2}{MIGRATE_V3}{MIGRATE_V4}PRAGMA user_version = {SCHEMA_VERSION};COMMIT;"
                ))?;
            }
            2 => {
                conn.execute_batch(&format!(
                    "BEGIN;{MIGRATE_V3}{MIGRATE_V4}PRAGMA user_version = {SCHEMA_VERSION};COMMIT;"
                ))?;
            }
            3 => {
                conn.execute_batch(&format!(
                    "BEGIN;{MIGRATE_V4}PRAGMA user_version = {SCHEMA_VERSION};COMMIT;"
                ))?;
            }
            v if v == SCHEMA_VERSION => {}
            v => {
                return Err(CoreError::Storage {
                    reason: format!(
                        "database schema {v} is newer than this build ({SCHEMA_VERSION})"
                    ),
                })
            }
        }
        Ok(Self {
            conn: Mutex::new(conn),
            files_dir: files_dir.to_owned(),
            wrap,
            last_wal_truncate: Mutex::new(None),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        // A panic while the lock was held cannot leave a half-applied change: a single
        // statement is atomic, and a transaction rolls back when its guard is dropped.
        Ok(self
            .conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Directory of staged blobs and downloaded files.
    pub fn files_dir(&self) -> &Path {
        &self.files_dir
    }

    /// Runs `f` in one transaction: committed if it returns `Ok`, rolled back otherwise.
    pub fn tx<R>(&self, f: impl FnOnce(&Db<'_>) -> Result<R>) -> Result<R> {
        let mut guard = self.lock()?;
        let tx = guard.transaction()?;
        let out = {
            let db = Db {
                conn: &tx,
                wrap: &self.wrap,
            };
            f(&db)?
        };
        tx.commit()?;
        Ok(out)
    }

    /// Runs `f` outside any explicit transaction (reads, single statements).
    pub fn read<R>(&self, f: impl FnOnce(&Db<'_>) -> Result<R>) -> Result<R> {
        let guard = self.lock()?;
        f(&Db {
            conn: &guard,
            wrap: &self.wrap,
        })
    }

    /// Absolute path of a file stored under the files directory.
    pub fn file_path(&self, relative: &str) -> PathBuf {
        self.files_dir.join(relative)
    }

    /// Erases every row of every table (the schema stays), with `secure_delete` on, and
    /// records `meta.leave_state = "wiping"` in the same transaction. After the commit the
    /// device is no longer provisioned (no `space_id`, no `secrets`), and there is never a
    /// state of erased keys without the marker. The list of tables is read from the schema,
    /// not written down, so that a table added later is erased too. If anything fails the
    /// transaction is rolled back and nothing was erased.
    ///
    /// With [`WipeGuard::DepartureMarker`] the transaction first checks that
    /// `meta.leave_state` is still `requested` or `deleting` (`api-v1.md` §7.9.6 step 2): if
    /// the departure was cancelled meanwhile, nothing is erased and the result is `Ok(false)`.
    /// `Ok(true)`: everything was erased.
    pub(crate) fn wipe_everything(&self, guard: WipeGuard) -> Result<bool> {
        let mut conn = self.lock()?;
        // `secure_delete` stays on for the whole life of the connection (`from_connection`);
        // set again here so that this erasure never depends on it.
        conn.pragma_update(None, "secure_delete", true)?;
        Self::wipe_tables(&mut conn, guard)
    }

    fn wipe_tables(conn: &mut Connection, guard: WipeGuard) -> Result<bool> {
        let tx = conn.transaction()?;
        if guard == WipeGuard::DepartureMarker {
            let marker: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT value FROM meta WHERE key = ?1",
                    params![crate::runtime::leave::LEAVE_STATE],
                    |r| r.get(0),
                )
                .optional()?;
            let pending = marker.as_deref().is_some_and(|m| {
                m == crate::runtime::leave::REQUESTED || m == crate::runtime::leave::DELETING
            });
            if !pending {
                // Dropping `tx` rolls it back: nothing was written.
                return Ok(false);
            }
        }
        // A future foreign key without `ON DELETE CASCADE` must not make the erasure fail
        // (after the relay was told): the constraints are checked at commit, when all tables
        // are empty. The pragma resets itself at the end of the transaction.
        tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let tables: Vec<String> = {
            let mut st = tx.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for name in &tables {
            // Names come from the schema; quoted all the same.
            tx.execute(
                &format!("DELETE FROM \"{}\"", name.replace('"', "\"\"")),
                [],
            )?;
        }
        // The single row `cursor()` expects.
        tx.execute("INSERT INTO sync_state (id, cursor) VALUES (1, 0)", [])?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            params![
                crate::runtime::leave::LEAVE_STATE,
                crate::runtime::leave::WIPING
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Truncates the WAL and rebuilds the file so that erased pages are no longer in the
    /// database files (`wal_checkpoint(TRUNCATE)`, `VACUUM`, `wal_checkpoint(TRUNCATE)`).
    /// The physical medium (flash) may still hold freed blocks: this does not overwrite them.
    /// Outside any transaction. Fails if a checkpoint could not complete (another connection
    /// busy).
    pub(crate) fn compact(&self) -> Result<()> {
        let guard = self.lock()?;
        Self::checkpoint(&guard)?;
        guard.execute_batch("VACUUM")?;
        Self::checkpoint(&guard)
    }

    /// Truncates the write-ahead log, where the freed records of an earlier transaction would
    /// otherwise stay readable until the next checkpoint recycles it. Best effort and cheap:
    /// at most once every 5 minutes, unless `force` (something was just deleted); a checkpoint
    /// that cannot complete (a reader is busy) is simply tried again later.
    pub(crate) fn truncate_wal_if_due(&self, force: bool) {
        const EVERY: Duration = Duration::from_secs(300);
        let due = {
            let mut last = self
                .last_wal_truncate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let due = force || last.is_none_or(|t| t.elapsed() >= EVERY);
            if due {
                *last = Some(Instant::now());
            }
            due
        };
        if !due {
            return;
        }
        if let Ok(guard) = self.lock() {
            let _ = guard.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                r.get::<_, i64>(0)
            });
        }
    }

    /// Whether `secure_delete` is on for the connection (tests).
    #[cfg(test)]
    pub(crate) fn secure_delete_on(&self) -> bool {
        self.lock().is_ok_and(|g| {
            g.pragma_query_value(None, "secure_delete", |r| r.get::<_, i64>(0))
                .is_ok_and(|v| v == 1)
        })
    }

    fn checkpoint(conn: &Connection) -> Result<()> {
        let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        if busy != 0 {
            return Err(CoreError::Storage {
                reason: "the write-ahead log could not be truncated".into(),
            });
        }
        Ok(())
    }

    /// Removes the sub-directories of the files directory and creates them again, empty.
    /// Never touches anything outside `files_dir`. Returns `Err(Storage)` if something could
    /// not be removed (still open elsewhere, on Windows); the rest is removed anyway.
    pub(crate) fn wipe_files(&self) -> Result<()> {
        let mut failed = 0u32;
        for sub in FILE_DIRS {
            match std::fs::remove_dir_all(self.files_dir.join(sub)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => failed += 1,
            }
        }
        Self::prepare_dirs(&self.files_dir)?;
        if failed > 0 {
            return Err(CoreError::Storage {
                reason: format!("{failed} directories could not be removed"),
            });
        }
        Ok(())
    }

    /// Runs raw SQL (tests: a trigger that makes the erasing transaction fail).
    #[cfg(test)]
    pub(crate) fn exec_for_test(&self, sql: &str) {
        self.lock().unwrap().execute_batch(sql).unwrap();
    }

    /// Number of rows of every table (tests).
    #[cfg(test)]
    pub(crate) fn row_counts(&self) -> Vec<(String, i64)> {
        let guard = self.lock().unwrap();
        let names: Vec<String> = {
            let mut st = guard
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap();
            let rows = st.query_map([], |r| r.get::<_, String>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        names
            .into_iter()
            .map(|n| {
                let c: i64 = guard
                    .query_row(&format!("SELECT count(*) FROM \"{n}\""), [], |r| r.get(0))
                    .unwrap();
                (n, c)
            })
            .collect()
    }
}

/// Operations on the database; usable inside or outside a transaction.
pub struct Db<'a> {
    conn: &'a Connection,
    wrap: &'a [u8; KEY_LEN],
}

impl Db<'_> {
    // --- secrets ------------------------------------------------------------

    /// Seals `plaintext` under the host wrapping key, bound to `name`.
    pub fn seal_secret(&self, name: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce: [u8; NONCE_LEN] = random_array()?;
        let ct = aead_seal(self.wrap, &nonce, &secret_aad(name), plaintext)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Opens a record made by [`seal_secret`](Self::seal_secret).
    pub fn open_secret(&self, name: &str, record: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if record.len() < NONCE_LEN {
            return Err(CoreError::DecryptionFailed);
        }
        let nonce: [u8; NONCE_LEN] = to_array(&record[..NONCE_LEN], "nonce")?;
        Ok(Zeroizing::new(aead_open(
            self.wrap,
            &nonce,
            &secret_aad(name),
            &record[NONCE_LEN..],
        )?))
    }

    pub fn secret_get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let rec: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT record FROM secrets WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?;
        rec.map(|r| self.open_secret(name, &r)).transpose()
    }

    pub fn secret_put(&self, name: &str, plaintext: &[u8]) -> Result<()> {
        let rec = self.seal_secret(name, plaintext)?;
        self.conn.execute(
            "INSERT INTO secrets (name, record) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET record = excluded.record",
            params![name, rec],
        )?;
        Ok(())
    }

    /// Whether a sealed record is stored under `name` (without opening it).
    pub fn secret_exists(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM secrets WHERE name = ?1", [name], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Erases the record stored under `name`, if any.
    pub fn secret_delete(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM secrets WHERE name = ?1", [name])?;
        Ok(())
    }

    // --- meta ---------------------------------------------------------------

    pub fn meta_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_get_string(&self, key: &str) -> Result<Option<String>> {
        Ok(self.meta_get(key)?.and_then(|v| String::from_utf8(v).ok()))
    }

    pub fn meta_set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_delete(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    // --- cursor -------------------------------------------------------------

    pub fn cursor(&self) -> Result<u64> {
        Ok(u(self.conn.query_row(
            "SELECT cursor FROM sync_state WHERE id = 1",
            [],
            |r| r.get(0),
        )?))
    }

    pub fn set_cursor(&self, cursor: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_state SET cursor = ?1 WHERE id = 1",
            [i(cursor)],
        )?;
        Ok(())
    }

    // --- trust store --------------------------------------------------------

    pub fn trust_json(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT json FROM trust WHERE id = 1", [], |r| r.get(0))
            .optional()?)
    }

    pub fn set_trust_json(&self, json: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO trust (id, json) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            [json],
        )?;
        Ok(())
    }

    // --- space key ring -----------------------------------------------------

    pub fn keyring_records(&self) -> Result<Vec<KeyRingRecord>> {
        let mut st = self.conn.prepare(
            "SELECT epoch, key_sealed, superseded_seq, superseded_applied_at_ms
             FROM space_keys ORDER BY epoch",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<Vec<u8>>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (epoch, sealed, seq, applied) = row?;
            let epoch = u32::try_from(epoch).map_err(|_| corrupt("epoch"))?;
            let key = match sealed {
                Some(s) => Some(
                    self.open_secret(&format!("space_key:{epoch}"), &s)?
                        .to_vec(),
                ),
                None => None,
            };
            let superseded = match (seq, applied) {
                (Some(s), Some(a)) => Some(Supersession {
                    rotation_seq: u(s),
                    applied_at_ms: u(a),
                }),
                _ => None,
            };
            out.push(KeyRingRecord {
                epoch,
                key,
                superseded,
            });
        }
        Ok(out)
    }

    /// Replaces the stored ring by `records`.
    pub fn set_keyring_records(&self, records: &[KeyRingRecord]) -> Result<()> {
        self.conn.execute("DELETE FROM space_keys", [])?;
        for r in records {
            let sealed = match &r.key {
                Some(k) => Some(self.seal_secret(&format!("space_key:{}", r.epoch), k)?),
                None => None,
            };
            self.conn.execute(
                "INSERT INTO space_keys
                 (epoch, key_sealed, superseded_seq, superseded_applied_at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    r.epoch,
                    sealed,
                    r.superseded.map(|s| i(s.rotation_seq)),
                    r.superseded.map(|s| i(s.applied_at_ms))
                ],
            )?;
        }
        Ok(())
    }

    // --- items --------------------------------------------------------------

    const ITEM_COLS: &'static str = "item_id, seq, origin, sender_device_id, created_at_ms,
        received_at_ms, relay_expires_at_ms, key_epoch, state, sealed, raw_envelope,
        payload_expires_at_ms, secret, read_policy, is_read, kept, blobs_json, ring, send_status";

    fn item_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawItemRow> {
        Ok(RawItemRow {
            item_id: r.get(0)?,
            seq: r.get(1)?,
            origin: r.get(2)?,
            sender: r.get(3)?,
            created: r.get(4)?,
            received: r.get(5)?,
            relay_exp: r.get(6)?,
            epoch: r.get(7)?,
            state: r.get(8)?,
            sealed: r.get(9)?,
            raw: r.get(10)?,
            payload_exp: r.get(11)?,
            secret: r.get(12)?,
            policy: r.get(13)?,
            read: r.get(14)?,
            kept: r.get(15)?,
            blobs: r.get(16)?,
            ring: r.get(17)?,
            send: r.get(18)?,
        })
    }

    pub fn item(&self, item_id: &str) -> Result<Option<ItemRow>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {} FROM items WHERE item_id = ?1", Self::ITEM_COLS),
                [item_id],
                Self::item_from_row,
            )
            .optional()?;
        raw.map(RawItemRow::into_row).transpose()
    }

    /// Number of items (cheap: no row is decoded).
    pub fn item_count(&self) -> Result<u64> {
        Ok(u(self.conn.query_row(
            "SELECT count(*) FROM items",
            [],
            |r| r.get(0),
        )?))
    }

    pub fn item_exists(&self, item_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM items WHERE item_id = ?1", [item_id], |_| {
                Ok(())
            })
            .optional()?
            .is_some())
    }

    /// All items, newest first (`created_at`, then `seq`).
    pub fn items(&self) -> Result<Vec<ItemRow>> {
        self.items_where("1 = 1")
    }

    pub fn items_in_state(&self, status: ItemStatus) -> Result<Vec<ItemRow>> {
        self.items_where(&format!("state = '{}'", status.as_str()))
    }

    fn items_where(&self, cond: &str) -> Result<Vec<ItemRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM items WHERE {cond} ORDER BY created_at_ms DESC, seq DESC, item_id",
            Self::ITEM_COLS
        ))?;
        let rows = st.query_map([], Self::item_from_row)?;
        rows.map(|r| r?.into_row()).collect()
    }

    /// Insert or replace an item row (the files of the item are untouched).
    pub fn put_item(&self, it: &ItemRow) -> Result<()> {
        let blobs = serde_json::to_string(&it.blobs).map_err(|_| CoreError::Internal {
            reason: "blob list serialization".into(),
        })?;
        self.conn.execute(
            &format!(
                "INSERT INTO items ({}) VALUES
                 (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
                 ON CONFLICT(item_id) DO UPDATE SET
                 seq=excluded.seq, origin=excluded.origin, sender_device_id=excluded.sender_device_id,
                 created_at_ms=excluded.created_at_ms, received_at_ms=excluded.received_at_ms,
                 relay_expires_at_ms=excluded.relay_expires_at_ms, key_epoch=excluded.key_epoch,
                 state=excluded.state, sealed=excluded.sealed, raw_envelope=excluded.raw_envelope,
                 payload_expires_at_ms=excluded.payload_expires_at_ms, secret=excluded.secret,
                 read_policy=excluded.read_policy, is_read=excluded.is_read, kept=excluded.kept,
                 blobs_json=excluded.blobs_json, ring=excluded.ring, send_status=excluded.send_status",
                Self::ITEM_COLS
            ),
            params![
                it.item_id,
                i(it.seq),
                it.origin.as_str(),
                it.sender_device_id,
                it.created_at_ms,
                it.received_at_ms,
                it.relay_expires_at_ms,
                it.key_epoch,
                it.status.as_str(),
                it.sealed,
                it.raw_envelope,
                it.payload_expires_at_ms,
                it.secret,
                it.read_policy,
                it.read,
                it.kept,
                blobs,
                it.ring,
                it.send_status.map(SendStatus::as_str),
            ],
        )?;
        Ok(())
    }

    pub fn set_item_read(&self, item_id: &str, read: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET is_read = ?2 WHERE item_id = ?1",
            params![item_id, read],
        )?;
        Ok(())
    }

    pub fn set_send_status(&self, item_id: &str, status: SendStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET send_status = ?2 WHERE item_id = ?1",
            params![item_id, status.as_str()],
        )?;
        Ok(())
    }

    pub fn set_item_seq(&self, item_id: &str, seq: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET seq = ?2 WHERE item_id = ?1 AND seq = 0",
            params![item_id, i(seq)],
        )?;
        Ok(())
    }

    /// Whether anything waits for a newer version to be read: an item in `update_required`
    /// or an item state kept raw. Two `LIMIT 1` probes, nothing is loaded.
    pub fn has_update_required_work(&self) -> Result<bool> {
        let probe = |sql: &str| -> Result<bool> {
            Ok(self
                .conn
                .query_row(sql, [], |_| Ok(()))
                .optional()?
                .is_some())
        };
        Ok(
            probe("SELECT 1 FROM items WHERE state = 'update_required' LIMIT 1")?
                || probe("SELECT 1 FROM pending_states LIMIT 1")?,
        )
    }

    /// Epochs of the raw envelopes still held (needed by `discard_expired`).
    pub fn raw_envelope_epochs(&self) -> Result<Vec<u32>> {
        let mut st = self.conn.prepare(
            "SELECT DISTINCT key_epoch FROM items
             WHERE raw_envelope IS NOT NULL AND state IN ('key_pending','update_required')",
        )?;
        let rows = st.query_map([], |r| r.get::<_, u32>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Deletes the item, its files rows and its receipts. It does NOT delete its queued
    /// operations (`outbox`): see [`Self::delete_ops_for_item`]. Returns the paths
    /// (relative) of the files to erase: the encrypted copies and the decrypted cache.
    pub fn delete_item(&self, item_id: &str) -> Result<Vec<String>> {
        let mut paths = Vec::new();
        for f in self.files(item_id)? {
            paths.push(cache_file_name(item_id, f.idx));
            if let Some(p) = f.path {
                paths.push(p);
            }
        }
        self.conn
            .execute("DELETE FROM item_files WHERE item_id = ?1", [item_id])?;
        self.conn
            .execute("DELETE FROM receipts WHERE item_id = ?1", [item_id])?;
        self.conn.execute(
            "DELETE FROM attachment_removals WHERE item_id = ?1",
            [item_id],
        )?;
        self.conn
            .execute("DELETE FROM items WHERE item_id = ?1", [item_id])?;
        Ok(paths)
    }

    // --- files --------------------------------------------------------------

    pub fn put_file(&self, f: &FileRow) -> Result<()> {
        let key = match &f.key {
            Some(k) => Some(self.seal_secret(&format!("file_key:{}:{}", f.item_id, f.idx), k)?),
            None => None,
        };
        self.conn.execute(
            "INSERT INTO item_files (item_id, idx, blob_id, size, key_sealed, nonce_prefix, state, path)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(item_id, idx) DO UPDATE SET blob_id=excluded.blob_id, size=excluded.size,
             key_sealed=excluded.key_sealed, nonce_prefix=excluded.nonce_prefix,
             state=excluded.state, path=excluded.path",
            params![
                f.item_id,
                f.idx,
                f.blob_id,
                i(f.size),
                key,
                f.nonce_prefix,
                f.state.as_str(),
                f.path
            ],
        )?;
        Ok(())
    }

    /// Deletes the row of attachment `idx`. Returns the relative paths of the files to erase
    /// after the commit: the decrypted cache and the encrypted copy.
    pub fn delete_file(&self, item_id: &str, idx: u32) -> Result<Vec<String>> {
        let mut paths = vec![cache_file_name(item_id, idx)];
        let path: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT path FROM item_files WHERE item_id = ?1 AND idx = ?2",
                params![item_id, idx],
                |r| r.get(0),
            )
            .optional()?;
        paths.extend(path.flatten());
        self.conn.execute(
            "DELETE FROM item_files WHERE item_id = ?1 AND idx = ?2",
            params![item_id, idx],
        )?;
        Ok(paths)
    }

    pub fn files(&self, item_id: &str) -> Result<Vec<FileRow>> {
        let mut st = self.conn.prepare(
            "SELECT item_id, idx, blob_id, size, key_sealed, nonce_prefix, state, path
             FROM item_files WHERE item_id = ?1 ORDER BY idx",
        )?;
        let rows = st.query_map([item_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<Vec<u8>>>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (item_id, idx, blob_id, size, key, prefix, state, path) = row?;
            let key = match key {
                Some(k) => Some(
                    self.open_secret(&format!("file_key:{item_id}:{idx}"), &k)?
                        .to_vec(),
                ),
                None => None,
            };
            out.push(FileRow {
                item_id,
                idx,
                blob_id,
                size: u(size),
                key,
                nonce_prefix: prefix,
                state: FileState::parse(&state)?,
                path,
            });
        }
        Ok(out)
    }

    // --- receipts -----------------------------------------------------------

    pub fn receipt_received(&self, item_id: &str, device_id: &str, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO receipts (item_id, device_id, received_at_ms) VALUES (?1,?2,?3)
             ON CONFLICT(item_id, device_id) DO UPDATE SET
             received_at_ms = COALESCE(receipts.received_at_ms, excluded.received_at_ms)",
            params![item_id, device_id, at_ms],
        )?;
        Ok(())
    }

    pub fn receipt_read(&self, item_id: &str, device_id: &str, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO receipts (item_id, device_id, read_at_ms) VALUES (?1,?2,?3)
             ON CONFLICT(item_id, device_id) DO UPDATE SET
             read_at_ms = COALESCE(receipts.read_at_ms, excluded.read_at_ms)",
            params![item_id, device_id, at_ms],
        )?;
        Ok(())
    }

    /// `(device_id, received_at_ms, read_at_ms)` per device.
    pub fn receipts(&self, item_id: &str) -> Result<Vec<ReceiptRow>> {
        let mut st = self.conn.prepare(
            "SELECT device_id, received_at_ms, read_at_ms FROM receipts
             WHERE item_id = ?1 ORDER BY device_id",
        )?;
        let rows = st.query_map([item_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // --- tombstones ---------------------------------------------------------

    pub fn put_tombstone(
        &self,
        item_id: &str,
        deleted_at_ms: i64,
        everywhere: bool,
        now_ms: i64,
    ) -> Result<()> {
        // An 'everywhere' record is never downgraded to 'local'.
        self.conn.execute(
            "INSERT INTO tombstones (item_id, deleted_at_ms, scope, recorded_at_ms)
             VALUES (?1,?2,?3,?4)
             ON CONFLICT(item_id) DO UPDATE SET
             scope = CASE WHEN excluded.scope = 'everywhere' THEN 'everywhere' ELSE tombstones.scope END",
            params![item_id, deleted_at_ms, if everywhere { "everywhere" } else { "local" }, now_ms],
        )?;
        Ok(())
    }

    pub fn is_tombstoned(&self, item_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tombstones WHERE item_id = ?1",
                [item_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Forgets tombstones recorded before `before_ms` (45 days, spec §7.1). Returns the count.
    pub fn prune_tombstones(&self, before_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM tombstones WHERE recorded_at_ms < ?1",
            [before_ms],
        )?)
    }

    // --- attachment removals (format-v1 §8.10) --------------------------------

    /// Records the removal of attachment `idx` of `item_id`. Independent of the item: it may
    /// be recorded before the item exists. A removal "everywhere" is never downgraded to
    /// "local". Returns `true` when the row is new (or was upgraded to "everywhere").
    pub fn put_attachment_removal(
        &self,
        item_id: &str,
        idx: u32,
        blob_id: &str,
        everywhere: bool,
        at_ms: i64,
        now_ms: i64,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT INTO attachment_removals (item_id, idx, blob_id, scope, at_ms, recorded_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(item_id, idx) DO UPDATE SET scope = 'everywhere'
             WHERE excluded.scope = 'everywhere' AND attachment_removals.scope = 'local'",
            params![
                item_id,
                idx,
                blob_id,
                if everywhere { "everywhere" } else { "local" },
                at_ms,
                now_ms
            ],
        )?;
        Ok(changed > 0)
    }

    /// The removal recorded for attachment `idx` of the item, if any.
    pub fn attachment_removal(
        &self,
        item_id: &str,
        idx: u32,
    ) -> Result<Option<AttachmentRemovalRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT item_id, idx, blob_id, scope FROM attachment_removals
                 WHERE item_id = ?1 AND idx = ?2",
                params![item_id, idx],
                |r| {
                    Ok(AttachmentRemovalRow {
                        item_id: r.get(0)?,
                        idx: r.get(1)?,
                        blob_id: r.get(2)?,
                        everywhere: r.get::<_, String>(3)? == "everywhere",
                    })
                },
            )
            .optional()?)
    }

    /// Every removal recorded for the item, by slot.
    pub fn attachment_removals(&self, item_id: &str) -> Result<Vec<AttachmentRemovalRow>> {
        let mut st = self.conn.prepare(
            "SELECT item_id, idx, blob_id, scope FROM attachment_removals
             WHERE item_id = ?1 ORDER BY idx",
        )?;
        let rows = st.query_map([item_id], |r| {
            Ok(AttachmentRemovalRow {
                item_id: r.get(0)?,
                idx: r.get(1)?,
                blob_id: r.get(2)?,
                everywhere: r.get::<_, String>(3)? == "everywhere",
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Forgets the removals recorded before `before_ms` whose item is not here (45 days,
    /// like the tombstones). Those of an item that exists stay as long as it does.
    pub fn prune_attachment_removals(&self, before_ms: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM attachment_removals WHERE recorded_at_ms < ?1
             AND item_id NOT IN (SELECT item_id FROM items)",
            [before_ms],
        )?)
    }

    // --- pins ---------------------------------------------------------------

    pub fn pin_book(&self) -> Result<PinBook> {
        let mut book = PinBook::new();
        let mut st = self
            .conn
            .prepare("SELECT item_id, pinned, at_ms FROM pins")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, pinned, at) = row?;
            book.merge(
                &id,
                PinState {
                    pinned,
                    at_ms: u(at),
                },
            )?;
        }
        Ok(book)
    }

    pub fn put_pin(&self, item_id: &str, state: PinState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO pins (item_id, pinned, at_ms) VALUES (?1,?2,?3)
             ON CONFLICT(item_id) DO UPDATE SET pinned = excluded.pinned, at_ms = excluded.at_ms",
            params![item_id, state.pinned, i(state.at_ms)],
        )?;
        Ok(())
    }

    pub fn delete_pin(&self, item_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM pins WHERE item_id = ?1", [item_id])?;
        Ok(())
    }

    // --- archives (twin of pins) --------------------------------------------

    pub fn archive_book(&self) -> Result<ArchiveBook> {
        let mut book = ArchiveBook::new();
        let mut st = self
            .conn
            .prepare("SELECT item_id, archived, at_ms FROM archives")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, archived, at) = row?;
            book.merge(
                &id,
                ArchiveState {
                    archived,
                    at_ms: u(at),
                },
            )?;
        }
        Ok(book)
    }

    pub fn put_archive(&self, item_id: &str, state: ArchiveState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO archives (item_id, archived, at_ms) VALUES (?1,?2,?3)
             ON CONFLICT(item_id) DO UPDATE SET archived = excluded.archived, at_ms = excluded.at_ms",
            params![item_id, state.archived, i(state.at_ms)],
        )?;
        Ok(())
    }

    pub fn delete_archive(&self, item_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM archives WHERE item_id = ?1", [item_id])?;
        Ok(())
    }

    // --- pending item states ------------------------------------------------

    pub fn put_pending_state(
        &self,
        event_id: &str,
        seq: u64,
        key_epoch: u32,
        envelope: &[u8],
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO pending_states (event_id, seq, key_epoch, envelope)
             VALUES (?1,?2,?3,?4)",
            params![event_id, i(seq), key_epoch, envelope],
        )?;
        Ok(())
    }

    /// `(event_id, seq, key_epoch, envelope)` by ascending `seq`.
    pub fn pending_states(&self) -> Result<Vec<PendingState>> {
        let mut st = self.conn.prepare(
            "SELECT event_id, seq, key_epoch, envelope FROM pending_states ORDER BY seq",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, u32>(2)?,
                r.get(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (a, b, c, d) = row?;
            out.push((a, u(b), c, d));
        }
        Ok(out)
    }

    pub fn delete_pending_state(&self, event_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM pending_states WHERE event_id = ?1", [event_id])?;
        Ok(())
    }

    // --- outbox -------------------------------------------------------------

    pub fn enqueue_op(&self, op: &NewOp, now_ms: i64) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO outbox (kind, item_id, event_id, payload, aux, flag, created_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                op.kind.as_str(),
                op.item_id,
                op.event_id,
                op.payload,
                op.aux,
                op.flag,
                now_ms
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn op_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawOp> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
        ))
    }

    const OP_COLS: &'static str = "op_id, kind, item_id, event_id, payload, aux, flag, attempts,
        next_attempt_at_ms, last_error, stale_attempts";

    fn ops_where(&self, cond: &str, args: impl rusqlite::Params) -> Result<Vec<OpRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM outbox WHERE status = 'pending' AND {cond} ORDER BY op_id",
            Self::OP_COLS
        ))?;
        let rows = st.query_map(args, Self::op_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let (
                op_id,
                kind,
                item_id,
                event_id,
                payload,
                aux,
                flag,
                attempts,
                next,
                last_error,
                stale_attempts,
            ) = row?;
            out.push(OpRow {
                op_id,
                kind: OpKind::parse(&kind)?,
                item_id,
                event_id,
                payload,
                aux,
                flag,
                attempts,
                next_attempt_at_ms: next,
                last_error,
                stale_attempts,
            });
        }
        Ok(out)
    }

    /// Pending operations whose retry time has come, in insertion order.
    pub fn due_ops(&self, now_ms: i64) -> Result<Vec<OpRow>> {
        self.ops_where("next_attempt_at_ms <= ?1", [now_ms])
    }

    pub fn pending_ops(&self) -> Result<Vec<OpRow>> {
        self.ops_where("1 = 1", [])
    }

    pub fn ops_for_item(&self, item_id: &str) -> Result<Vec<OpRow>> {
        self.ops_where("item_id = ?1", [item_id])
    }

    pub fn op(&self, op_id: i64) -> Result<Option<OpRow>> {
        Ok(self.ops_where("op_id = ?1", [op_id])?.into_iter().next())
    }

    pub fn finish_op(&self, op_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM outbox WHERE op_id = ?1", [op_id])?;
        Ok(())
    }

    pub fn finish_ops_by_event(&self, event_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM outbox WHERE event_id = ?1 AND kind = 'item_state'",
            [event_id],
        )?;
        Ok(())
    }

    pub fn delete_ops_for_item(&self, item_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM outbox WHERE item_id = ?1", [item_id])?;
        Ok(())
    }

    /// Marks an `item_state` operation whose `event_id` was already re-issued once.
    pub fn mark_op_reissued(&self, op_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE outbox SET flag = 1 WHERE op_id = ?1", [op_id])?;
        Ok(())
    }

    pub fn defer_op(&self, op_id: i64, next_attempt_at_ms: i64, error: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, next_attempt_at_ms = ?2, last_error = ?3
             WHERE op_id = ?1",
            params![op_id, next_attempt_at_ms, error],
        )?;
        Ok(())
    }

    /// [`defer_op`](Self::defer_op) after a `stale_key_epoch` answer: also counts it.
    pub fn defer_op_stale(&self, op_id: i64, next_attempt_at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1, stale_attempts = stale_attempts + 1,
             next_attempt_at_ms = ?2, last_error = 'stale_key_epoch' WHERE op_id = ?1",
            params![op_id, next_attempt_at_ms],
        )?;
        Ok(())
    }

    /// Files rows in state `done` (every item), for housekeeping.
    pub fn done_files(&self) -> Result<Vec<FileRow>> {
        let ids: Vec<String> = {
            let mut st = self
                .conn
                .prepare("SELECT DISTINCT item_id FROM item_files WHERE state = 'done'")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        let mut out = Vec::new();
        for id in ids {
            out.extend(
                self.files(&id)?
                    .into_iter()
                    .filter(|f| f.state == FileState::Done),
            );
        }
        Ok(out)
    }

    pub fn update_op_payload(
        &self,
        op_id: i64,
        payload: &[u8],
        event_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET payload = ?2, event_id = COALESCE(?3, event_id) WHERE op_id = ?1",
            params![op_id, payload, event_id],
        )?;
        Ok(())
    }

    // --- staged blobs -------------------------------------------------------

    pub fn put_blob_upload(&self, owner_kind: &str, owner_id: &str, b: &BlobUpload) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blob_uploads (blob_id, owner_kind, owner_id, size, path)
             VALUES (?1,?2,?3,?4,?5)",
            params![b.blob_id, owner_kind, owner_id, i(b.size), b.path],
        )?;
        Ok(())
    }

    pub fn blob_uploads(&self, owner_kind: &str, owner_id: &str) -> Result<Vec<BlobUpload>> {
        let mut st = self.conn.prepare(
            "SELECT blob_id, size, path FROM blob_uploads
             WHERE owner_kind = ?1 AND owner_id = ?2 ORDER BY ord",
        )?;
        let rows = st.query_map(params![owner_kind, owner_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (blob_id, size, path) = row?;
            out.push(BlobUpload {
                blob_id,
                size: u(size),
                path,
            });
        }
        Ok(out)
    }

    pub fn delete_blob_uploads(&self, owner_kind: &str, owner_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM blob_uploads WHERE owner_kind = ?1 AND owner_id = ?2",
            params![owner_kind, owner_id],
        )?;
        Ok(())
    }

    // --- devices ------------------------------------------------------------

    pub fn put_device(&self, d: &DeviceRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO devices (device_id, name, platform, ed25519, x25519, state, joined_seq, last_seen_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(device_id) DO UPDATE SET name=excluded.name, platform=excluded.platform,
             ed25519=COALESCE(excluded.ed25519, devices.ed25519),
             x25519=COALESCE(excluded.x25519, devices.x25519),
             -- A revoked device never comes back as a member through a record the relay served
             -- (a replayed `device_added`, a list): ids are never reused, so it would only be
             -- a relay putting back a device the user removed.
             state=CASE WHEN devices.state IN ('revoked','revoked_by_relay')
                         AND excluded.state IN ('verified','unverified')
                        THEN devices.state ELSE excluded.state END,
             joined_seq=excluded.joined_seq,
             last_seen_at_ms=COALESCE(excluded.last_seen_at_ms, devices.last_seen_at_ms)",
            params![d.device_id, d.name, d.platform, d.ed25519, d.x25519, d.state.as_str(), i(d.joined_seq), d.last_seen_at_ms],
        )?;
        Ok(())
    }

    fn device_rows(&self, cond: &str, args: impl rusqlite::Params) -> Result<Vec<DeviceRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT device_id, name, platform, ed25519, x25519, state, joined_seq, last_seen_at_ms
             FROM devices WHERE {cond} ORDER BY joined_seq, device_id"
        ))?;
        let rows = st.query_map(args, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (device_id, name, platform, ed25519, x25519, state, joined, last_seen) = row?;
            out.push(DeviceRow {
                device_id,
                name,
                platform,
                ed25519,
                x25519,
                state: DeviceState::parse(&state)?,
                joined_seq: u(joined),
                last_seen_at_ms: last_seen,
            });
        }
        Ok(out)
    }

    pub fn devices(&self) -> Result<Vec<DeviceRow>> {
        self.device_rows("1 = 1", [])
    }

    pub fn device(&self, device_id: &str) -> Result<Option<DeviceRow>> {
        Ok(self
            .device_rows("device_id = ?1", [device_id])?
            .into_iter()
            .next())
    }

    pub fn set_device_state(&self, device_id: &str, state: DeviceState) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET state = ?2 WHERE device_id = ?1",
            params![device_id, state.as_str()],
        )?;
        Ok(())
    }

    /// Replaces the last activity of a KNOWN device, `None` included (no `COALESCE`, unlike
    /// [`put_device`](Self::put_device)): the value the relay reports is the truth. Never
    /// creates a row and never touches anything else (state, keys, name).
    pub fn set_device_last_seen(
        &self,
        device_id: &str,
        last_seen_at_ms: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET last_seen_at_ms = ?2 WHERE device_id = ?1",
            params![device_id, last_seen_at_ms],
        )?;
        Ok(())
    }

    /// Forgets the last activity of every device. Used once to drop the values that earlier
    /// builds wrote (the time of pairing, which was not an activity).
    pub fn clear_devices_last_seen(&self) -> Result<()> {
        self.conn
            .execute("UPDATE devices SET last_seen_at_ms = NULL", [])?;
        Ok(())
    }

    pub fn set_device_name(&self, device_id: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET name = ?2 WHERE device_id = ?1",
            params![device_id, name],
        )?;
        Ok(())
    }

    /// Sets the name and the platform of a device (`refresh_devices`: the member of a space that
    /// the pairing did not name).
    pub fn set_device_identity(&self, device_id: &str, name: &str, platform: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET name = ?2, platform = ?3 WHERE device_id = ?1",
            params![device_id, name, platform],
        )?;
        Ok(())
    }

    // --- transfers ----------------------------------------------------------

    pub fn put_transfer(&self, t: &TransferRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO transfers (transfer_id, role, peer_device_id, state, as_of_seq,
             history_key_sealed, temp_key_sealed, manifest, index_path, index_len, items_total,
             items_done, skipped_unreadable, files_total, files_done, outcome, error,
             created_at_ms, updated_at_ms)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                t.transfer_id,
                t.role,
                t.peer_device_id,
                t.state,
                i(t.as_of_seq),
                t.history_key_sealed,
                t.temp_key_sealed,
                t.manifest,
                t.index_path,
                i(t.index_len),
                i(t.items_total),
                i(t.items_done),
                i(t.skipped_unreadable),
                i(t.files_total),
                i(t.files_done),
                t.outcome,
                t.error,
                t.created_at_ms,
                t.updated_at_ms
            ],
        )?;
        Ok(())
    }

    fn transfer_rows(&self, cond: &str, args: impl rusqlite::Params) -> Result<Vec<TransferRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT transfer_id, role, peer_device_id, state, as_of_seq, history_key_sealed,
             temp_key_sealed, manifest, index_path, index_len, items_total, items_done,
             skipped_unreadable, files_total, files_done, outcome, error, created_at_ms, updated_at_ms
             FROM transfers WHERE {cond} ORDER BY created_at_ms, transfer_id"
        ))?;
        let rows = st.query_map(args, |r| {
            Ok(TransferRow {
                transfer_id: r.get(0)?,
                role: r.get(1)?,
                peer_device_id: r.get(2)?,
                state: r.get(3)?,
                as_of_seq: u(r.get(4)?),
                history_key_sealed: r.get(5)?,
                temp_key_sealed: r.get(6)?,
                manifest: r.get(7)?,
                index_path: r.get(8)?,
                index_len: u(r.get(9)?),
                items_total: u(r.get(10)?),
                items_done: u(r.get(11)?),
                skipped_unreadable: u(r.get(12)?),
                files_total: u(r.get(13)?),
                files_done: u(r.get(14)?),
                outcome: r.get(15)?,
                error: r.get(16)?,
                created_at_ms: r.get(17)?,
                updated_at_ms: r.get(18)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn transfer(&self, transfer_id: &str) -> Result<Option<TransferRow>> {
        Ok(self
            .transfer_rows("transfer_id = ?1", [transfer_id])?
            .into_iter()
            .next())
    }

    pub fn transfers(&self) -> Result<Vec<TransferRow>> {
        self.transfer_rows("1 = 1", [])
    }

    pub fn delete_transfer(&self, transfer_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM transfers WHERE transfer_id = ?1",
            [transfer_id],
        )?;
        Ok(())
    }

    // --- diagnostics --------------------------------------------------------

    /// Records an ignored or refused event. `detail` must never contain content.
    pub fn diag(&self, at_ms: i64, kind: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO diagnostics (at_ms, kind, detail) VALUES (?1,?2,?3)",
            params![at_ms, kind, detail],
        )?;
        // Keep the log small.
        self.conn.execute(
            "DELETE FROM diagnostics WHERE id <= (SELECT MAX(id) FROM diagnostics) - 500",
            [],
        )?;
        Ok(())
    }

    /// Raw statement, tests only (fault injection).
    #[cfg(test)]
    pub(crate) fn conn_exec(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    /// The sealed record of a secret exactly as stored (nonce and ciphertext), tests only.
    #[cfg(test)]
    pub(crate) fn raw_secret_record(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT record FROM secrets WHERE name = ?1", [name], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// The sealed space keys and the sealed bodies of the secret items exactly as stored,
    /// labelled `space_key` / `secret_item`, tests only.
    #[cfg(test)]
    pub(crate) fn raw_sealed_records(&self) -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut out = Vec::new();
        let mut st = self
            .conn
            .prepare("SELECT key_sealed FROM space_keys WHERE key_sealed IS NOT NULL")?;
        for r in st.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            out.push(("space_key", r?));
        }
        let mut st = self
            .conn
            .prepare("SELECT sealed FROM items WHERE secret = 1 AND sealed IS NOT NULL")?;
        for r in st.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            out.push(("secret_item", r?));
        }
        Ok(out)
    }

    pub fn diagnostics(&self) -> Result<Vec<(i64, String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT at_ms, kind, detail FROM diagnostics ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn secret_aad(name: &str) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_LOCAL_SECRET);
    w.bytes(name.as_bytes());
    w.finish()
}

/// Raw SQLite values of an `items` row.
struct RawItemRow {
    item_id: String,
    seq: i64,
    origin: String,
    sender: String,
    created: i64,
    received: i64,
    relay_exp: Option<i64>,
    epoch: u32,
    state: String,
    sealed: Option<Vec<u8>>,
    raw: Option<Vec<u8>>,
    payload_exp: Option<i64>,
    secret: bool,
    policy: String,
    read: bool,
    kept: bool,
    blobs: String,
    ring: bool,
    send: Option<String>,
}

impl RawItemRow {
    fn into_row(self) -> Result<ItemRow> {
        Ok(ItemRow {
            item_id: self.item_id,
            seq: u(self.seq),
            origin: ItemOrigin::parse(&self.origin)?,
            sender_device_id: self.sender,
            created_at_ms: self.created,
            received_at_ms: self.received,
            relay_expires_at_ms: self.relay_exp,
            key_epoch: self.epoch,
            status: ItemStatus::parse(&self.state)?,
            sealed: self.sealed,
            raw_envelope: self.raw,
            payload_expires_at_ms: self.payload_exp,
            secret: self.secret,
            read_policy: self.policy,
            read: self.read,
            kept: self.kept,
            blobs: serde_json::from_str(&self.blobs).map_err(|_| corrupt("blob list"))?,
            ring: self.ring,
            send_status: self.send.as_deref().map(SendStatus::parse).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_of_a_file_row_never_prints_the_key() {
        let row = FileRow {
            item_id: "a".into(),
            idx: 0,
            blob_id: None,
            size: 3,
            key: Some(vec![0xAB; 32]),
            nonce_prefix: Some(vec![0xCD; 19]),
            state: FileState::Done,
            path: Some("a_0".into()),
        };
        for dbg in [format!("{row:?}"), format!("{row:#?}")] {
            for n in ["171", "ab", "AB", "205", "cd", "CD"] {
                assert!(!dbg.contains(n), "Debug leaks {n:?}: {dbg}");
            }
            assert!(dbg.contains("has_key: true"), "{dbg}");
        }
    }

    #[test]
    fn secure_delete_stays_on_and_the_wal_is_truncated() {
        let (dir, s) = store();
        assert!(s.secure_delete_on(), "on from the start");
        s.tx(|d| {
            for i in 0..200 {
                d.meta_set(&format!("k{i}"), &[0x5A; 512])?;
            }
            Ok(())
        })
        .unwrap();
        let wal = dir.path().join("tomi.db-wal");
        assert!(std::fs::metadata(&wal).map_or(0, |m| m.len()) > 0);
        s.tx(|d| d.meta_set("k0", b"x")).unwrap();
        s.truncate_wal_if_due(true);
        assert_eq!(std::fs::metadata(&wal).map_or(0, |m| m.len()), 0);
        // A wipe leaves it on for the rest of the connection's life.
        s.wipe_everything(WipeGuard::Unconditional).unwrap();
        assert!(s.secure_delete_on());
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(
            &dir.path().join("tomi.db"),
            &dir.path().join("files"),
            &[7u8; 32],
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn a_record_from_the_relay_never_puts_a_revoked_device_back_among_the_members() {
        let (_dir, s) = store();
        let row = |state| DeviceRow {
            device_id: "d".into(),
            name: "Pixel".into(),
            platform: "android".into(),
            ed25519: Some(vec![1; 32]),
            x25519: Some(vec![2; 32]),
            state,
            joined_seq: 3,
            last_seen_at_ms: None,
        };
        let state_of = |s: &Store| s.read(|db| Ok(db.devices()?[0].state)).unwrap();
        s.tx(|db| db.put_device(&row(DeviceState::Verified)))
            .unwrap();
        s.tx(|db| db.put_device(&row(DeviceState::Revoked)))
            .unwrap();
        for back in [DeviceState::Verified, DeviceState::Unverified] {
            s.tx(|db| db.put_device(&row(back))).unwrap();
            assert_eq!(state_of(&s), DeviceState::Revoked);
        }
        s.tx(|db| db.put_device(&row(DeviceState::RevokedByRelay)))
            .unwrap();
        assert_eq!(state_of(&s), DeviceState::RevokedByRelay);
        s.tx(|db| db.put_device(&row(DeviceState::Verified)))
            .unwrap();
        assert_eq!(state_of(&s), DeviceState::RevokedByRelay);
    }

    pub(crate) fn sample_item(id: &str, created: i64, seq: u64) -> ItemRow {
        ItemRow {
            item_id: id.into(),
            seq,
            origin: ItemOrigin::Received,
            sender_device_id: "0123456789abcdef0123456789abcdef".into(),
            created_at_ms: created,
            received_at_ms: created,
            relay_expires_at_ms: Some(created + 1000),
            key_epoch: 1,
            status: ItemStatus::Readable,
            sealed: Some(vec![1, 2, 3]),
            raw_envelope: None,
            payload_expires_at_ms: None,
            secret: false,
            read_policy: "until_expiry".into(),
            read: false,
            kept: false,
            blobs: vec![BlobRef {
                blob_id: "b".into(),
                size: 5,
            }],
            ring: false,
            send_status: None,
        }
    }

    #[test]
    fn schema_is_created_once_and_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("a.db");
        let files = dir.path().join("f");
        {
            let s = Store::open(&db, &files, &[1u8; 32]).unwrap();
            s.tx(|d| d.set_cursor(41)).unwrap();
        }
        let s = Store::open(&db, &files, &[1u8; 32]).unwrap();
        assert_eq!(s.read(|d| d.cursor()).unwrap(), 41);
        let v: i64 = s
            .read(|d| {
                Ok(d.conn
                    .pragma_query_value(None, "user_version", |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn a_version_1_database_is_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("v1.db");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(&format!("BEGIN;{SCHEMA_V1}PRAGMA user_version = 1;COMMIT;"))
                .unwrap();
            c.execute(
                "INSERT INTO outbox (kind, created_at_ms) VALUES ('ack', 1)",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&db, &dir.path().join("f"), &[1u8; 32]).unwrap();
        let ops = s.read(|d| d.pending_ops()).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].stale_attempts, 0);
        s.tx(|d| d.defer_op_stale(ops[0].op_id, 7)).unwrap();
        let op = s.read(|d| d.op(ops[0].op_id)).unwrap().unwrap();
        assert_eq!((op.attempts, op.stale_attempts), (1, 1));
        assert_eq!(op.last_error.as_deref(), Some("stale_key_epoch"));
    }

    #[test]
    fn a_version_2_database_gets_the_archive_table_and_keeps_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("v2.db");
        let id = "00000000-0000-4000-8000-0000000000bb";
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(&format!(
                "BEGIN;{SCHEMA_V1}{MIGRATE_V2}PRAGMA user_version = 2;COMMIT;"
            ))
            .unwrap();
            c.execute(
                "INSERT INTO pins (item_id, pinned, at_ms) VALUES (?1,1,5)",
                [id],
            )
            .unwrap();
        }
        let s = Store::open(&db, &dir.path().join("f"), &[1u8; 32]).unwrap();
        assert!(s.read(|d| d.archive_book()).unwrap().state(id).is_none());
        assert!(s.read(|d| d.pin_book()).unwrap().is_pinned(id));
        let state = ArchiveState {
            archived: true,
            at_ms: 9,
        };
        s.tx(|d| d.put_archive(id, state)).unwrap();
        assert_eq!(s.read(|d| d.archive_book()).unwrap().state(id), Some(state));
        // Replaced in place, then forgotten.
        let later = ArchiveState {
            archived: false,
            at_ms: 10,
        };
        s.tx(|d| d.put_archive(id, later)).unwrap();
        assert_eq!(s.read(|d| d.archive_book()).unwrap().state(id), Some(later));
        s.tx(|d| d.delete_archive(id)).unwrap();
        assert!(s.read(|d| d.archive_book()).unwrap().state(id).is_none());
        let v: i64 = s
            .read(|d| {
                Ok(d.conn
                    .pragma_query_value(None, "user_version", |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn a_version_3_database_is_rebuilt_to_v4_with_its_pending_operations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("v3.db");
        {
            let c = Connection::open(&db).unwrap();
            c.execute_batch(&format!(
                "BEGIN;{SCHEMA_V1}{MIGRATE_V2}{MIGRATE_V3}PRAGMA user_version = 3;COMMIT;"
            ))
            .unwrap();
            // Pending operations of several kinds, one retried, and a finished one (its
            // id must not be reused after the rebuild).
            c.execute_batch(
                "INSERT INTO outbox (kind, item_id, payload, created_at_ms, attempts, last_error, stale_attempts)
                   VALUES ('tombstone', '00000000-0000-4000-8000-000000000001', x'0102', 5, 2, 'rate_limited', 1);
                 INSERT INTO outbox (kind, item_id, flag, created_at_ms) VALUES ('ack', 'it-2', 1, 6);
                 INSERT INTO outbox (kind, event_id, payload, aux, created_at_ms)
                   VALUES ('item_state', 'ev-1', x'0a', x'0b', 8);
                 INSERT INTO outbox (kind, item_id, status, created_at_ms) VALUES ('read', 'x', 'done', 7);
                 DELETE FROM outbox WHERE kind = 'read';
                 INSERT INTO pins (item_id, pinned, at_ms) VALUES ('00000000-0000-4000-8000-000000000001', 1, 9);",
            )
            .unwrap();
        }
        let s = Store::open(&db, &dir.path().join("f"), &[1u8; 32]).unwrap();
        let ops = s.read(|d| d.pending_ops()).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].kind, OpKind::Tombstone);
        assert_eq!(ops[0].payload.as_deref(), Some(&[1u8, 2][..]));
        assert_eq!(
            (
                ops[0].attempts,
                ops[0].stale_attempts,
                ops[0].last_error.as_deref()
            ),
            (2, 1, Some("rate_limited"))
        );
        assert_eq!((ops[1].kind, ops[1].flag), (OpKind::Ack, true));
        assert_eq!(ops[2].aux.as_deref(), Some(&[0x0bu8][..]));
        // The data around it is untouched.
        assert!(s
            .read(|d| d.pin_book())
            .unwrap()
            .is_pinned("00000000-0000-4000-8000-000000000001"));
        // The new kind is accepted, and the counter did not go back.
        let mut op = NewOp::new(OpKind::AttachmentTombstone);
        op.payload = Some(vec![7]);
        let id = s.tx(|d| d.enqueue_op(&op, 10)).unwrap();
        assert!(
            id > ops[2].op_id + 1,
            "the id of the deleted operation is not reused"
        );
        // The registry exists; the indexes were recreated.
        s.tx(|d| {
            d.put_attachment_removal("00000000-0000-4000-8000-000000000001", 0, "b", true, 1, 1)
        })
        .unwrap();
        let names: Vec<String> = s
            .read(|d| {
                let mut st = d.conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'outbox'",
                )?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .unwrap();
        assert!(
            names.contains(&"outbox_due".to_owned()) && names.contains(&"outbox_item".to_owned())
        );
        let v: i64 = s
            .read(|d| {
                Ok(d.conn
                    .pragma_query_value(None, "user_version", |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(v, 4);
        // A database that is already v4 reopens as it is.
        drop(s);
        let s = Store::open(&db, &dir.path().join("f"), &[1u8; 32]).unwrap();
        assert_eq!(s.read(|d| d.pending_ops()).unwrap().len(), 4);
    }

    #[test]
    fn a_version_1_and_a_version_2_database_reach_v4_too() {
        for (v, base) in [(1, ""), (2, MIGRATE_V2)] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("old.db");
            {
                let c = Connection::open(&db).unwrap();
                c.execute_batch(&format!(
                    "BEGIN;{SCHEMA_V1}{base}PRAGMA user_version = {v};COMMIT;"
                ))
                .unwrap();
                c.execute(
                    "INSERT INTO outbox (kind, created_at_ms) VALUES ('ack', 1)",
                    [],
                )
                .unwrap();
            }
            let s = Store::open(&db, &dir.path().join("f"), &[1u8; 32]).unwrap();
            assert_eq!(s.read(|d| d.pending_ops()).unwrap().len(), 1);
            let mut op = NewOp::new(OpKind::AttachmentTombstone);
            op.payload = Some(vec![1]);
            s.tx(|d| d.enqueue_op(&op, 2)).unwrap();
            assert_eq!(s.read(|d| d.pending_ops()).unwrap().len(), 2);
        }
    }

    #[test]
    fn the_attachment_removal_registry_never_downgrades_and_prunes_orphans_only() {
        let s = Store::open_in_memory(&tempfile::tempdir().unwrap().path().join("f"), &[1u8; 32])
            .unwrap();
        s.tx(|d| {
            assert!(d.put_attachment_removal("a", 0, "b", false, 1, 100)?);
            assert!(
                d.put_attachment_removal("a", 0, "b", true, 2, 200)?,
                "upgraded"
            );
            assert!(
                !d.put_attachment_removal("a", 0, "b", false, 3, 300)?,
                "not downgraded"
            );
            Ok(())
        })
        .unwrap();
        let r = s.read(|d| d.attachment_removal("a", 0)).unwrap().unwrap();
        assert!(r.everywhere);
        // The first record date stays (an upgrade does not renew the 45 days).
        assert_eq!(s.tx(|d| d.prune_attachment_removals(50)).unwrap(), 0);
        assert_eq!(s.tx(|d| d.prune_attachment_removals(150)).unwrap(), 1);
    }

    #[test]
    fn secrets_are_sealed_and_bound_to_wrap_key_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("a.db");
        let files = dir.path().join("f");
        let s = Store::open(&db, &files, &[1u8; 32]).unwrap();
        s.tx(|d| d.secret_put("local_key", b"zzqq secret bytes"))
            .unwrap();
        assert_eq!(
            &**s.read(|d| d.secret_get("local_key")).unwrap().unwrap(),
            b"zzqq secret bytes"
        );
        // Not in clear in the file.
        drop(s);
        let raw: Vec<u8> = std::fs::read(&db).unwrap_or_default();
        let wal: Vec<u8> = std::fs::read(dir.path().join("a.db-wal")).unwrap_or_default();
        assert!(!raw.windows(5).any(|w| w == b"zzqq "));
        assert!(!wal.windows(5).any(|w| w == b"zzqq "));
        // Wrong wrapping key: refused.
        let s2 = Store::open(&db, &files, &[2u8; 32]).unwrap();
        assert_eq!(
            s2.read(|d| d.secret_get("local_key")).unwrap_err(),
            CoreError::DecryptionFailed
        );
        // Bound to the name: a record moved to another name does not open.
        let s3 = Store::open(&db, &files, &[1u8; 32]).unwrap();
        let moved = s3
            .tx(|d| {
                let rec = d.seal_secret("a", b"x")?;
                Ok(d.open_secret("b", &rec))
            })
            .unwrap();
        assert_eq!(moved.unwrap_err(), CoreError::DecryptionFailed);
    }

    #[test]
    fn transaction_rolls_back_on_error() {
        let (_d, s) = store();
        let r: Result<()> = s.tx(|d| {
            d.set_cursor(99)?;
            d.put_item(&sample_item("a", 1, 1))?;
            Err(CoreError::input("boom"))
        });
        assert!(r.is_err());
        assert_eq!(s.read(|d| d.cursor()).unwrap(), 0);
        assert!(s.read(|d| d.items()).unwrap().is_empty());
    }

    #[test]
    fn items_roundtrip_and_feed_order() {
        let (_d, s) = store();
        s.tx(|d| {
            d.put_item(&sample_item("b", 20, 2))?;
            d.put_item(&sample_item("a", 10, 1))?;
            d.put_item(&sample_item("c", 20, 3))?;
            Ok(())
        })
        .unwrap();
        let ids: Vec<String> = s
            .read(|d| d.items())
            .unwrap()
            .into_iter()
            .map(|i| i.item_id)
            .collect();
        assert_eq!(ids, ["c", "b", "a"]);
        let a = s.read(|d| d.item("a")).unwrap().unwrap();
        assert_eq!(a, sample_item("a", 10, 1));
        // Unreadable states are kept with their raw envelope.
        let mut u = sample_item("u", 5, 4);
        u.status = ItemStatus::Unreadable(UnreadableReason::KeyPending);
        u.sealed = None;
        u.raw_envelope = Some(vec![9; 10]);
        s.tx(|d| d.put_item(&u)).unwrap();
        assert_eq!(s.read(|d| d.items_in_state(u.status)).unwrap(), vec![u]);
    }

    #[test]
    fn deleting_an_item_removes_files_and_receipts() {
        let (_d, s) = store();
        let paths = s
            .tx(|d| {
                d.put_item(&sample_item("a", 1, 1))?;
                d.put_file(&FileRow {
                    item_id: "a".into(),
                    idx: 0,
                    blob_id: Some("b".into()),
                    size: 3,
                    key: Some(vec![5; 32]),
                    nonce_prefix: Some(vec![6; 19]),
                    state: FileState::Done,
                    path: Some("a_0".into()),
                })?;
                d.receipt_received("a", "dev", 5)?;
                d.delete_item("a")
            })
            .unwrap();
        // The encrypted copy and the decrypted cache.
        assert_eq!(paths, ["files/cache/a_0", "a_0"]);
        assert!(s.read(|d| d.files("a")).unwrap().is_empty());
        assert!(s.read(|d| d.receipts("a")).unwrap().is_empty());
    }

    #[test]
    fn file_key_is_sealed_in_the_database() {
        let (_d, s) = store();
        s.tx(|d| {
            d.put_item(&sample_item("a", 1, 1))?;
            d.put_file(&FileRow {
                item_id: "a".into(),
                idx: 1,
                blob_id: None,
                size: 3,
                key: Some(vec![0xAB; 32]),
                nonce_prefix: None,
                state: FileState::Remote,
                path: None,
            })
        })
        .unwrap();
        let f = s.read(|d| d.files("a")).unwrap();
        assert_eq!(f[0].key.as_deref(), Some(&[0xAB; 32][..]));
        let sealed: Vec<u8> = s
            .read(|d| {
                Ok(d.conn
                    .query_row("SELECT key_sealed FROM item_files", [], |r| r.get(0))?)
            })
            .unwrap();
        assert!(!sealed.windows(4).any(|w| w == [0xAB; 4]));
    }

    #[test]
    fn keyring_and_pins_and_trust_roundtrip() {
        let (_d, s) = store();
        let recs = vec![
            KeyRingRecord {
                epoch: 1,
                key: None,
                superseded: Some(Supersession {
                    rotation_seq: 9,
                    applied_at_ms: 100,
                }),
            },
            KeyRingRecord {
                epoch: 2,
                key: Some(vec![3; 32]),
                superseded: None,
            },
        ];
        s.tx(|d| {
            d.set_keyring_records(&recs)?;
            d.put_pin(
                "00000000-0000-4000-8000-0000000000aa",
                PinState {
                    pinned: true,
                    at_ms: 77,
                },
            )?;
            d.set_trust_json("{\"x\":1}")
        })
        .unwrap();
        assert_eq!(s.read(|d| d.keyring_records()).unwrap(), recs);
        let book = s.read(|d| d.pin_book()).unwrap();
        assert_eq!(
            book.state("00000000-0000-4000-8000-0000000000aa"),
            Some(PinState {
                pinned: true,
                at_ms: 77
            })
        );
        assert_eq!(
            s.read(|d| d.trust_json()).unwrap().as_deref(),
            Some("{\"x\":1}")
        );
    }

    #[test]
    fn outbox_keeps_insertion_order_and_defers() {
        let (_d, s) = store();
        let ids = s
            .tx(|d| {
                let mut ids = vec![];
                for k in [OpKind::SendItem, OpKind::Ack, OpKind::Tombstone] {
                    let mut op = NewOp::new(k);
                    op.item_id = Some("x".into());
                    ids.push(d.enqueue_op(&op, 5)?);
                }
                Ok(ids)
            })
            .unwrap();
        s.tx(|d| d.defer_op(ids[0], 1000, "offline")).unwrap();
        let due = s.read(|d| d.due_ops(10)).unwrap();
        assert_eq!(
            due.iter().map(|o| o.kind).collect::<Vec<_>>(),
            [OpKind::Ack, OpKind::Tombstone]
        );
        let all = s.read(|d| d.pending_ops()).unwrap();
        assert_eq!(all[0].attempts, 1);
        assert_eq!(all[0].last_error.as_deref(), Some("offline"));
        s.tx(|d| d.finish_op(ids[1])).unwrap();
        assert_eq!(s.read(|d| d.pending_ops()).unwrap().len(), 2);
    }

    #[test]
    fn tombstone_scope_is_never_downgraded() {
        let (_d, s) = store();
        s.tx(|d| {
            d.put_tombstone("a", 1, true, 100)?;
            d.put_tombstone("a", 2, false, 200)
        })
        .unwrap();
        let scope: String = s
            .read(|d| {
                Ok(d.conn
                    .query_row("SELECT scope FROM tombstones", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(scope, "everywhere");
        assert!(s.read(|d| d.is_tombstoned("a")).unwrap());
        assert_eq!(s.tx(|d| d.prune_tombstones(150)).unwrap(), 1);
        assert!(!s.read(|d| d.is_tombstoned("a")).unwrap());
    }
}
