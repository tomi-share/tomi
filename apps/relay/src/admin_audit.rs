//! The persistent audit log of the web panel (`admin_web_audit` in `routing.db`).
//!
//! It records what was done through the panel and what concerns its access: sign-ins, refused
//! step-ups, the actions that destroy or create a right, the passkeys added or removed, the path
//! changed, the trash restored or purged. A line is a date, a **fixed word**, the 8-character id
//! of the target (a space or a device: the only exception to "nothing about a space", bounded to
//! what the panel already shows) and the number of the passkey. Never a device name, an address,
//! a token, a code or a value of a request. Bounded by age ([`MAX_AGE_MS`]); the lines of the
//! ordinary kinds are also bounded by number ([`MAX_ROWS`]), but the lines of the
//! **destructive or rights-changing kinds** ([`PROTECTED`]) are only ever purged by age: what a
//! session alone can write at will (a quota, a sign-in) can never push them out.

use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};

use crate::error::ApiError;
use crate::state::AppState;
use crate::wire::ts_format;

/// How many lines are kept.
pub const MAX_ROWS: i64 = 500;
/// How many lines [`list`] returns at most (the protected ones are not bounded by number, but by
/// the ceilings of the actions that write them).
pub const LIST_ROWS: i64 = 2000;
/// How long a line is kept (90 days).
pub const MAX_AGE_MS: i64 = 90 * 86_400_000;

/// The words of the log (fixed: nothing that came from a request is ever a word).
pub mod kind {
    pub const SIGNED_IN: &str = "signed_in";
    pub const STEPUP_REFUSED: &str = "stepup_refused";
    pub const SESSION_CLOSED_BY_REFUSALS: &str = "session_closed_by_refusals";
    pub const SESSIONS_CLOSED: &str = "sessions_closed";
    pub const DEVICE_REMOVED: &str = "device_removed";
    pub const SPACE_DELETED: &str = "space_deleted";
    pub const INVITATION_CREATED: &str = "invitation_created";
    pub const INVITATIONS_REPLACED: &str = "invitations_replaced";
    pub const INVITATIONS_CANCELLED: &str = "invitations_cancelled";
    pub const QUOTA_SET: &str = "quota_set";
    pub const DEFAULT_QUOTA_SET: &str = "default_quota_set";
    pub const PASSKEY_ADDED: &str = "passkey_added";
    pub const PASSKEY_REMOVED: &str = "passkey_removed";
    pub const PASSKEY_RESET: &str = "passkey_reset";
    pub const PATH_ROTATED: &str = "path_rotated";
    pub const POLICY_CHANGED: &str = "policy_changed";
    pub const TRASH_RESTORED: &str = "trash_restored";
    pub const TRASH_PURGED: &str = "trash_purged";
    pub const TRASH_EXPIRED: &str = "trash_expired";
}

/// The kinds that are never purged by number: they record what destroys data or changes who can
/// get in, which is what the owner needs after a compromise.
pub const PROTECTED: [&str; 12] = [
    kind::PATH_ROTATED,
    kind::DEVICE_REMOVED,
    kind::SPACE_DELETED,
    kind::INVITATION_CREATED,
    kind::INVITATIONS_REPLACED,
    kind::PASSKEY_ADDED,
    kind::PASSKEY_REMOVED,
    kind::PASSKEY_RESET,
    kind::POLICY_CHANGED,
    kind::TRASH_RESTORED,
    kind::TRASH_PURGED,
    kind::TRASH_EXPIRED,
];

fn short(target: &str) -> String {
    target.chars().take(8).collect()
}

/// Adds a line in the transaction of the action it describes, and trims the log.
pub fn record_tx(
    tx: &Transaction,
    now: i64,
    kind: &str,
    target: Option<&str>,
    key: Option<i64>,
) -> Result<(), ApiError> {
    tx.execute(
        "INSERT INTO admin_web_audit (at, kind, target, key_num) VALUES (?1, ?2, ?3, ?4)",
        params![now, kind, target.map(short), key],
    )?;
    let protected = PROTECTED
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    tx.execute(
        &format!(
            "DELETE FROM admin_web_audit
             WHERE at < ?1
                OR id IN (SELECT id FROM admin_web_audit WHERE kind NOT IN ({protected})
                          ORDER BY id DESC LIMIT -1 OFFSET ?2)"
        ),
        params![now - MAX_AGE_MS, MAX_ROWS],
    )?;
    Ok(())
}

/// Adds a line on its own. A failure is logged, never raised: the log must not be a way to make
/// an action fail after it was done.
pub async fn record(st: &AppState, kind: &'static str, target: Option<String>, key: Option<i64>) {
    let now = st.now();
    let done = st
        .spaces
        .routing()
        .write(move |tx| record_tx(tx, now, kind, target.as_deref(), key))
        .await;
    if let Err(e) = done {
        tracing::warn!(
            "admin audit: a line could not be written ({})",
            e.code.as_str()
        );
    }
}

/// The log, newest first, as the panel shows it.
pub fn list(conn: &Connection) -> Result<Vec<Value>, ApiError> {
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'admin_web_audit'",
        [],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare(
        "SELECT at, kind, target, key_num FROM admin_web_audit ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = st
        .query_map(params![LIST_ROWS], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(at, kind, target, key)| {
            let mut v = json!({ "at": ts_format(at), "kind": kind });
            if let Some(t) = target {
                v["target"] = json!(t);
            }
            if let Some(k) = key {
                v["key"] = json!(k);
            }
            v
        })
        .collect())
}
