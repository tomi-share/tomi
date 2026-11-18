//! Queries shared by several endpoints: the space row, devices, the event log, quota.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Map, Value};
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::error::ApiError;
use crate::gate::Dev;
use crate::wire::{b64_encode, ts_format};

/// The single `space` row.
#[derive(Debug, Clone)]
pub struct SpaceRow {
    pub space_id: [u8; 16],
    pub created_at: i64,
    pub key_epoch: u32,
    pub rotation_required: bool,
    pub retention_floor_seq: i64,
    pub last_seq: i64,
}

/// The space row, if the space exists.
pub fn space_row(conn: &Connection) -> Result<Option<SpaceRow>, ApiError> {
    let r = conn
        .query_row(
            "SELECT space_id, created_at, key_epoch, rotation_required, retention_floor_seq, last_seq
             FROM space WHERE id = 1",
            [],
            |r| {
                let id: Vec<u8> = r.get(0)?;
                Ok(SpaceRow {
                    space_id: <[u8; 16]>::try_from(id.as_slice()).unwrap_or([0; 16]),
                    created_at: r.get(1)?,
                    key_epoch: u32::try_from(r.get::<_, i64>(2)?).unwrap_or(u32::MAX),
                    rotation_required: r.get::<_, i64>(3)? != 0,
                    retention_floor_seq: r.get(4)?,
                    last_seq: r.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(r)
}

/// The space row of this space database, or `409 space_not_initialized` (a database without a
/// row is not a space: the registry never hands one out).
pub fn require_space(conn: &Connection) -> Result<SpaceRow, ApiError> {
    space_row(conn)?.ok_or_else(ApiError::space_not_initialized)
}

/// The caller of an authenticated request, judged **again inside the transaction that acts for it**.
///
/// The gate reads the session and the device before the body of the request is read, and a body
/// may take minutes to arrive (`api-v1.md` §8.5): a device revoked meanwhile (rotation, `DELETE
/// /v1/me`, `admin revoke`) would otherwise still complete a request it started before. Every
/// write transaction on behalf of a device therefore starts with this check: the session is
/// still there and not revoked, the device is not revoked, and it still holds the key the gate
/// saw. The answers are those of the gate for the same states (`lookup_session`): `403 revoked`
/// for a revoked session or device, `401 unauthorized` for a session that is gone or a key that
/// changed. Nothing here tells a caller more than the gate would have told it a moment later.
///
/// `expires_at` of the session is NOT checked here: a session that expires during a long upload
/// finishes its write, as the gate (which judged the expiry once, when the request arrived)
/// already lets it.
pub fn require_caller(conn: &Connection, dev: &Dev) -> Result<(), ApiError> {
    let row = conn
        .query_row(
            "SELECT s.device_id, s.revoked, d.revoked_at, d.sign_pk
             FROM sessions s JOIN devices d ON d.device_id = s.device_id
             WHERE s.token_hash = ?1",
            params![dev.token_hash.to_vec()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((device_id, session_revoked, revoked_at, sign_pk)) = row else {
        return Err(ApiError::unauthorized());
    };
    if session_revoked != 0 || revoked_at.is_some() {
        return Err(ApiError::revoked(revoked_at));
    }
    if device_id != dev.device_id || sign_pk.as_deref() != Some(dev.sign_pk.as_slice()) {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}

/// Whether `token_hash` is the hash of the pairing token the channel `channel_id` was claimed
/// with. The channel must exist and not be past its deadline, like everywhere else (a channel
/// that is `completed` or `closed` keeps its token until then). Read only, and the comparison
/// takes the same time whatever the bytes: this tells the holder of the token of a channel
/// from anybody else for the rate limits (`api-v1.md` §8.5); what the holder may do is for
/// the routes of the channel to decide.
pub fn pairing_token_matches(
    conn: &Connection,
    channel_id: &str,
    token_hash: &[u8; 32],
    now_ms: i64,
) -> Result<bool, ApiError> {
    let stored = conn
        .query_row(
            "SELECT token_hash FROM pairing_channels WHERE channel_id = ?1 AND expires_at > ?2",
            params![channel_id, now_ms],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten();
    Ok(stored.is_some_and(|s| bool::from(s.as_slice().ct_eq(token_hash.as_slice()))))
}

/// Assigns the next `seq` (§8.2): strictly above every `seq` ever given and above the
/// relay clock, so a restored backup never hands out a `seq` a client has already passed.
pub fn next_seq(tx: &Connection, now_ms: i64) -> Result<i64, ApiError> {
    let last: i64 = tx.query_row("SELECT last_seq FROM space WHERE id = 1", [], |r| r.get(0))?;
    let max_event: i64 =
        tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
    let seq = last.max(max_event).max(now_ms - 1) + 1;
    tx.execute("UPDATE space SET last_seq = ?1 WHERE id = 1", params![seq])?;
    Ok(seq)
}

/// Appends an event to the log and returns its `seq`. `data` is the JSON of the `data`
/// member (for an `item` event, without the envelope: it is read from `items`).
pub fn add_event(
    tx: &Connection,
    now_ms: i64,
    kind: &str,
    author: Option<&str>,
    item_id: Option<&[u8; 16]>,
    data: &Value,
) -> Result<i64, ApiError> {
    let seq = next_seq(tx, now_ms)?;
    insert_event(tx, seq, now_ms, kind, author, item_id, data)?;
    Ok(seq)
}

/// Appends an event under a `seq` already assigned by [`next_seq`].
pub fn insert_event(
    tx: &Connection,
    seq: i64,
    now_ms: i64,
    kind: &str,
    author: Option<&str>,
    item_id: Option<&[u8; 16]>,
    data: &Value,
) -> Result<(), ApiError> {
    tx.execute(
        "INSERT INTO events (seq, type, at, author, item_id, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            seq,
            kind,
            now_ms,
            author,
            item_id.map(|i| i.to_vec()),
            data.to_string()
        ],
    )?;
    Ok(())
}

/// A row of `devices`.
#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    pub sign_pk: Option<Vec<u8>>,
    pub kx_pk: Option<Vec<u8>>,
    pub certificate: Option<Vec<u8>>,
    pub issuer_device_id: String,
    pub joined_seq: i64,
    pub registered_at: i64,
    pub last_seen_at: Option<i64>,
    pub present_until: Option<i64>,
    pub format_min: Option<i64>,
    pub format_max: Option<i64>,
    pub app_version: Option<String>,
    pub cursor_hint: Option<i64>,
    pub revoked_at: Option<i64>,
    pub push: bool,
}

/// Column list matching [`DeviceRow::from_row`].
pub const DEVICE_COLS: &str =
    "device_id, name, platform, sign_pk, kx_pk, certificate, issuer_device_id, \
     joined_seq, registered_at, last_seen_at, present_until, format_min, format_max, app_version, \
     cursor_hint, revoked_at, \
     EXISTS(SELECT 1 FROM push_registrations p WHERE p.device_id = devices.device_id)";

impl DeviceRow {
    pub fn from_row(r: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            device_id: r.get(0)?,
            name: r.get(1)?,
            platform: r.get(2)?,
            sign_pk: r.get(3)?,
            kx_pk: r.get(4)?,
            certificate: r.get(5)?,
            issuer_device_id: r.get(6)?,
            joined_seq: r.get(7)?,
            registered_at: r.get(8)?,
            last_seen_at: r.get(9)?,
            present_until: r.get(10)?,
            format_min: r.get(11)?,
            format_max: r.get(12)?,
            app_version: r.get(13)?,
            cursor_hint: r.get(14)?,
            revoked_at: r.get(15)?,
            push: r.get::<_, i64>(16)? != 0,
        })
    }

    /// Member of the space: registered and not revoked.
    pub fn is_member(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Active (§0.2): a session was opened within `inactive_after_days`.
    pub fn is_active(&self, now_ms: i64, cfg: &Config) -> bool {
        let window =
            i64::try_from(cfg.inactive_after_days.saturating_mul(86_400_000)).unwrap_or(i64::MAX);
        self.is_member()
            && self
                .last_seen_at
                .is_some_and(|t| t >= now_ms.saturating_sub(window))
    }

    /// Present (§6.2): recent user activity reported.
    pub fn is_present(&self, now_ms: i64) -> bool {
        self.is_member() && self.present_until.is_some_and(|t| t > now_ms)
    }
}

/// A device by id (revoked ones included).
pub fn device_by_id(conn: &Connection, device_id: &str) -> Result<Option<DeviceRow>, ApiError> {
    let r = conn
        .query_row(
            &format!("SELECT {DEVICE_COLS} FROM devices WHERE device_id = ?1"),
            params![device_id],
            DeviceRow::from_row,
        )
        .optional()?;
    Ok(r)
}

/// A member by id (`None` for an unknown or revoked device).
pub fn member_by_id(conn: &Connection, device_id: &str) -> Result<Option<DeviceRow>, ApiError> {
    Ok(device_by_id(conn, device_id)?.filter(DeviceRow::is_member))
}

/// A member by Ed25519 key.
pub fn member_by_sign_pk(conn: &Connection, sign_pk: &[u8]) -> Result<Option<DeviceRow>, ApiError> {
    let r = conn
        .query_row(
            &format!("SELECT {DEVICE_COLS} FROM devices WHERE sign_pk = ?1 AND revoked_at IS NULL"),
            params![sign_pk],
            DeviceRow::from_row,
        )
        .optional()?;
    Ok(r)
}

/// All members, oldest registration first.
pub fn members(conn: &Connection) -> Result<Vec<DeviceRow>, ApiError> {
    let mut st = conn.prepare(&format!(
        "SELECT {DEVICE_COLS} FROM devices WHERE revoked_at IS NULL ORDER BY registered_at, rowid"
    ))?;
    let rows = st
        .query_map([], DeviceRow::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `DeviceRecord` of §4.3. `full = false` gives the reduced form of `device_added` events.
pub fn device_record(d: &DeviceRow, now_ms: i64, cfg: &Config, full: bool) -> Value {
    let mut m = Map::new();
    m.insert("device_id".into(), json!(d.device_id));
    m.insert("name".into(), json!(d.name));
    m.insert("platform".into(), json!(d.platform));
    m.insert(
        "sign_pk".into(),
        json!(d.sign_pk.as_deref().map(b64_encode)),
    );
    m.insert("kx_pk".into(), json!(d.kx_pk.as_deref().map(b64_encode)));
    m.insert(
        "certificate".into(),
        json!(d.certificate.as_deref().map(b64_encode)),
    );
    m.insert("issuer_device_id".into(), json!(d.issuer_device_id));
    m.insert("joined_seq".into(), json!(d.joined_seq));
    m.insert("registered_at".into(), json!(ts_format(d.registered_at)));
    if full {
        if let Some(t) = d.last_seen_at {
            // Rounded down to the minute (§4.3).
            m.insert(
                "last_seen_at".into(),
                json!(ts_format(t - t.rem_euclid(60_000))),
            );
        }
        m.insert("active".into(), json!(d.is_active(now_ms, cfg)));
        m.insert("present".into(), json!(d.is_present(now_ms)));
        m.insert("push".into(), json!(d.push));
        if let Some(v) = d.format_min {
            m.insert("format_min".into(), json!(v));
        }
        if let Some(v) = d.format_max {
            m.insert("format_max".into(), json!(v));
        }
        if let Some(v) = &d.app_version {
            m.insert("app_version".into(), json!(v));
        }
    }
    Value::Object(m)
}

/// `used_bytes` (§8.4): live envelopes, item-state envelopes still in the log, and the
/// declared size of every blob (complete or not, attached or not).
pub fn used_bytes(conn: &Connection) -> Result<u64, ApiError> {
    let items: i64 = conn.query_row(
        "SELECT COALESCE(SUM(envelope_size), 0) FROM items WHERE purged_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let states: i64 = conn.query_row(
        "SELECT COALESCE(SUM(envelope_size), 0) FROM item_states",
        [],
        |r| r.get(0),
    )?;
    let blobs: i64 =
        conn.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |r| r.get(0))?;
    Ok(u64::try_from(items + states + blobs).unwrap_or(0))
}

/// `Limits` object (§2.8 without `max_space_bytes`).
pub fn limits_json(cfg: &Config) -> Value {
    json!({
        "max_envelope_bytes": cfg.max_envelope_bytes,
        "max_blob_bytes": cfg.max_blob_bytes,
        "max_item_blob_bytes": cfg.max_item_blob_bytes,
        "max_blobs_per_item": cfg.max_blobs_per_item,
        "max_upload_part_bytes": cfg.max_upload_part_bytes,
        "max_ttl_secs": cfg.max_ttl_secs,
        "max_devices": cfg.max_devices,
        "max_transfer_bytes": cfg.max_transfer_bytes,
        "max_blobs_per_transfer": cfg.max_blobs_per_transfer,
        "transfer_ttl_secs": cfg.transfer_ttl_secs,
        "transfer_max_open": cfg.transfer_max_open,
        "max_item_state_bytes": cfg.max_item_state_bytes,
    })
}

/// Effective format of the space and the devices that limit it (§3.5).
pub fn effective_format(members: &[DeviceRow], now_ms: i64, cfg: &Config) -> (u32, Vec<String>) {
    let active: Vec<&DeviceRow> = members
        .iter()
        .filter(|d| d.is_active(now_ms, cfg) && d.format_max.is_some())
        .collect();
    let min = active
        .iter()
        .filter_map(|d| d.format_max)
        .min()
        .unwrap_or(1);
    let limited_by = active
        .iter()
        .filter(|d| d.format_max == Some(min))
        .map(|d| d.device_id.clone())
        .collect();
    (u32::try_from(min).unwrap_or(1), limited_by)
}

/// What a space may still take (§8.4): its own quota, if it has one, the room left in the total
/// of the relay, and the free disk. Built by `Inner::budget`; it never carries the usage of a
/// single other space, only their sum, and only to compute a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Sum of `used_bytes` of every other space.
    pub others_used: u64,
    /// Quota of this space (override of `admin quota`, else `max_space_bytes`), if any.
    pub space_max: Option<u64>,
    /// `max_relay_bytes`.
    pub relay_max: u64,
    /// `min_free_disk_bytes`.
    pub min_free_disk: u64,
    /// Free bytes on the filesystem of `data_dir`.
    pub disk_free: u64,
}

impl Budget {
    /// The ceiling announced to the space: the smaller of its quota and what it uses now plus
    /// the room left in the relay total. This is the `max_bytes` of `quota` and of `relay_full`.
    pub fn effective_max(&self, used: u64) -> u64 {
        let room = self
            .relay_max
            .saturating_sub(self.others_used.saturating_add(used));
        let by_relay = used.saturating_add(room);
        self.space_max.map_or(by_relay, |m| m.min(by_relay))
    }
}

/// `SpaceState` of §3.3.
pub fn space_state(
    conn: &Connection,
    cfg: &Config,
    now_ms: i64,
    budget: &Budget,
) -> Result<Value, ApiError> {
    let sp = require_space(conn)?;
    let members = members(conn)?;
    let (effective, limited_by) = effective_format(&members, now_ms, cfg);
    let used = used_bytes(conn)?;
    Ok(json!({
        "space_id": crate::wire::uuid_format(&sp.space_id),
        "created_at": ts_format(sp.created_at),
        "effective_format": effective,
        "effective_format_limited_by": limited_by,
        "key_epoch": sp.key_epoch,
        "rotation_required": sp.rotation_required,
        "head_seq": sp.last_seq,
        "server_time": ts_format(now_ms),
        "quota": { "used_bytes": used, "max_bytes": budget.effective_max(used) },
        "limits": limits_json(cfg),
        "features": FEATURES,
    }))
}

/// Additive capabilities announced in `hello` and in `SpaceState` (`api-v1.md` §4.2): a client
/// offers a feature only if its relay lists it. `attachment_tombstones`: `POST /v1/attachment-tombstones`.
pub const FEATURES: [&str; 1] = ["attachment_tombstones"];

/// `507 relay_full` unless `extra` more bytes fit in the quota of the space (if it has one)
/// and in the total of the relay (§8.4), and the disk keeps `min_free_disk_bytes` free. The
/// answer carries the usage of *this* space and the effective ceiling, never the usage of another.
///
/// Called with the reservation lock held (`Inner::reserve`): two spaces cannot reserve at the
/// same time, so the total is never exceeded by a race.
pub fn check_quota(conn: &Connection, budget: &Budget, extra: u64) -> Result<(), ApiError> {
    let used = used_bytes(conn)?;
    let max = budget.effective_max(used);
    if used.saturating_add(extra) > max {
        return Err(ApiError::relay_full(used, max));
    }
    if budget.disk_free.saturating_sub(extra) < budget.min_free_disk {
        return Err(ApiError::relay_full(used, max).detail("Not enough free disk space."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, Tx};
    use crate::error::Code;

    fn seed(db: &Db) {
        db.write_sync(|tx| {
            tx.execute(
                "INSERT INTO space (id, space_id, created_at, key_epoch) VALUES (1, x'00112233445566778899aabbccddeeff', 1, 1)",
                [],
            )?;
            Ok(Tx::Commit(()))
        })
        .unwrap();
    }

    #[test]
    fn seq_is_monotonic_and_follows_the_clock() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        seed(&db);
        let (a, b, c, d) = db
            .write_sync(|tx| {
                let a = add_event(tx, 1_000_000, "x", None, None, &json!({}))?;
                let b = add_event(tx, 1_000_000, "x", None, None, &json!({}))?;
                // The clock jumps ahead, then goes back: seq never decreases.
                let c = add_event(tx, 9_000_000, "x", None, None, &json!({}))?;
                let d = add_event(tx, 5, "x", None, None, &json!({}))?;
                Ok(Tx::Commit((a, b, c, d)))
            })
            .unwrap();
        assert_eq!(a, 1_000_000);
        assert_eq!(b, 1_000_001);
        assert_eq!(c, 9_000_000);
        assert_eq!(d, 9_000_001);
        let head = db.read_sync(|tx| Ok(require_space(tx)?.last_seq)).unwrap();
        assert_eq!(head, d);
    }

    fn budget(others_used: u64, space_max: Option<u64>) -> Budget {
        Budget {
            others_used,
            space_max,
            relay_max: 1_000,
            min_free_disk: 0,
            disk_free: u64::MAX,
        }
    }

    #[test]
    fn the_ceiling_is_the_smaller_of_the_space_quota_and_the_room_in_the_total() {
        // No quota of its own: what it uses plus what the others leave of the total.
        assert_eq!(budget(0, None).effective_max(100), 1_000);
        assert_eq!(budget(600, None).effective_max(100), 400);
        // A quota below that wins, a quota above it does not.
        assert_eq!(budget(600, Some(150)).effective_max(100), 150);
        assert_eq!(budget(600, Some(900)).effective_max(100), 400);
        // The total is already exceeded (the configuration was lowered): it can only shrink.
        assert_eq!(budget(2_000, None).effective_max(100), 100);
    }

    #[test]
    fn a_reservation_needs_the_space_quota_the_total_and_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        seed(&db);
        db.write_sync(|tx| {
            tx.execute(
                "INSERT INTO blobs (blob_id, owner, size, last_activity) VALUES (x'01', 'd', 100, 0)",
                [],
            )?;
            Ok(Tx::Commit(()))
        })
        .unwrap();
        let check = |b: Budget, extra: u64| db.read_sync(|tx| check_quota(tx, &b, extra));
        assert!(check(budget(0, None), 900).is_ok());
        let full = check(budget(0, None), 901).unwrap_err();
        assert_eq!(full.code, Code::RelayFull);
        // The total is shared: the others use 850, 50 are left.
        assert!(check(budget(850, None), 50).is_ok());
        assert!(check(budget(850, None), 51).is_err());
        // The quota of the space.
        assert!(check(budget(0, Some(120)), 20).is_ok());
        assert!(check(budget(0, Some(120)), 21).is_err());
        // The answer names this space's usage and the effective ceiling, not the others' usage.
        let e = check(budget(850, None), 51).unwrap_err();
        assert_eq!(e.ext["used_bytes"], 100);
        assert_eq!(e.ext["max_bytes"], 150);
        // The disk.
        let low = Budget {
            disk_free: 10,
            min_free_disk: 5,
            ..budget(0, None)
        };
        assert!(check(low, 5).is_ok());
        assert!(check(low, 6).is_err());
    }

    #[test]
    fn missing_space_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let e = db
            .read_sync(|tx| require_space(tx).map(|_| ()))
            .unwrap_err();
        assert_eq!(e.code, Code::SpaceNotInitialized);
    }
}
