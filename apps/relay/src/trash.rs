//! The trash of the relay: the spaces deleted from the web panel wait there 7 days.
//!
//! A space deleted from the panel is **gone at once for every device** (`4004`, the id is free),
//! exactly as with `admin reset` or `DELETE /v1/space`; only its folder (database and encrypted
//! blobs, which the relay can read no better than before) is kept, renamed into
//! `<data_dir>/spaces.trash/<space_id>-<unix seconds>/` (see `Spaces::delete_keep`) and recorded in
//! `space_trash`. It is **not** a delay: restoring and purging are `tomi-relay admin trash ...`,
//! on the machine, never from the web. The maintenance purges what is past its 7 days.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::admin::Output;
use crate::admin_audit::{self, kind};
use crate::error::ApiError;
use crate::spaces::{RestoreError, TrashRow, TRASH_DIR};
use crate::state::AppState;
use crate::wire::ts_format;

/// What `tomi-relay admin trash ...` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrashAction {
    /// Lists the trash.
    List,
    /// Brings a space back.
    Restore,
    /// Deletes a space of the trash for good (the 8 characters of its id to confirm).
    Purge,
}

/// The rows of `space_trash`, newest first. An older database that lacks the table has none.
pub fn rows(conn: &Connection) -> Result<Vec<TrashRow>, ApiError> {
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'space_trash'",
        [],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare(
        "SELECT dir_name, space_id, deleted_at, purge_after, size_bytes, quota_set, quota_bytes
         FROM space_trash ORDER BY deleted_at DESC",
    )?;
    let v = st
        .query_map([], |r| {
            let set: i64 = r.get(5)?;
            let bytes: Option<i64> = r.get(6)?;
            Ok(TrashRow {
                dir_name: r.get(0)?,
                space_id: r.get(1)?,
                deleted_at: r.get(2)?,
                purge_after: r.get(3)?,
                size_bytes: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                quota: (set != 0).then(|| bytes.and_then(|b| u64::try_from(b).ok())),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// A row as the panel and `--json` show it: the short id only for the panel (`full: false`).
pub fn row_json(r: &TrashRow, full: bool) -> Value {
    json!({
        "space_id": if full { r.space_id.clone() } else { r.space_id.chars().take(8).collect() },
        "deleted_at": ts_format(r.deleted_at),
        "purge_after": ts_format(r.purge_after),
        "size_bytes": r.size_bytes,
    })
}

fn fail(e: &ApiError) -> Output {
    Output::fail(1, format!("error: {}", e.code.as_str()))
}

/// The row a command designates: `--space` (at least 8 hexadecimal characters), or the only one.
fn pick(all: &[TrashRow], requested: Option<&str>) -> Result<TrashRow, Output> {
    let listing = |all: &[TrashRow]| {
        all.iter()
            .map(|r| r.space_id.clone())
            .collect::<Vec<_>>()
            .join("\n")
    };
    match requested {
        Some(p) => {
            let p = p.trim().to_ascii_lowercase();
            if p.len() < 8 || !p.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
                return Err(Output::fail(
                    2,
                    "give a space id, or a prefix of at least 8 hexadecimal characters \
                     (see `tomi-relay admin trash`)",
                ));
            }
            let hits: Vec<&TrashRow> = all.iter().filter(|r| r.space_id.starts_with(&p)).collect();
            let mut ids: Vec<&str> = hits.iter().map(|r| r.space_id.as_str()).collect();
            ids.dedup();
            match (hits.first(), ids.len()) {
                (None, _) => Err(Output::fail(1, "no such space in the trash")),
                // Newest first: the first row of a single id is its latest deletion.
                (Some(r), 1) => Ok((*r).clone()),
                _ => Err(Output::fail(
                    2,
                    format!("the prefix is ambiguous:\n{}", listing(all)),
                )),
            }
        }
        None => match all {
            [] => Err(Output::fail(1, "the trash is empty")),
            [one] => Ok(one.clone()),
            _ => Err(Output::fail(
                2,
                format!(
                    "the trash holds several spaces: say which one with --space <id or \
                     8-character prefix>\n{}",
                    listing(all)
                ),
            )),
        },
    }
}

fn remove_folder(st: &AppState, dir_name: &str) -> std::io::Result<()> {
    let dir = st.spaces.data_dir().join(TRASH_DIR).join(dir_name);
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

async fn forget_row(st: &AppState, dir_name: String) -> Result<(), ApiError> {
    st.spaces
        .routing()
        .write(move |tx| {
            tx.execute(
                "DELETE FROM space_trash WHERE dir_name = ?1",
                params![dir_name],
            )?;
            Ok(())
        })
        .await
}

/// `tomi-relay admin trash [--json]`, `... restore [--space <id>]`,
/// `... purge [--space <id>] [--confirm <8 characters>]`.
pub async fn command(
    st: &AppState,
    action: TrashAction,
    space: Option<&str>,
    confirm: Option<&str>,
    as_json: bool,
) -> Output {
    let all = match st.spaces.routing().read(|tx| rows(tx)).await {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    match action {
        TrashAction::List => {
            if as_json {
                let list: Vec<Value> = all.iter().map(|r| row_json(r, true)).collect();
                return Output::ok(format!("{}\n", json!({ "trash": list })));
            }
            if all.is_empty() {
                return Output::ok("the trash is empty\n");
            }
            let mut out = String::new();
            for r in &all {
                out.push_str(&format!(
                    "{}  deleted {}  purged after {}  size {} bytes\n",
                    r.space_id,
                    ts_format(r.deleted_at),
                    ts_format(r.purge_after),
                    r.size_bytes
                ));
            }
            out.push_str("`tomi-relay admin trash restore --space <id>` brings one back, `purge` deletes it for good\n");
            Output::ok(out)
        }
        TrashAction::Restore => {
            let row = match pick(&all, space) {
                Ok(r) => r,
                Err(out) => return out,
            };
            let registry = st.spaces.lock_registry().await;
            let (spaces, r2) = (st.spaces.clone(), row.clone());
            let done = tokio::task::spawn_blocking(move || spaces.restore_from_trash(&r2))
                .await
                .unwrap_or_else(|_| Err(RestoreError::Failed("task failed".into())));
            drop(registry);
            let restored = match done {
                Ok(s) => s,
                Err(RestoreError::IdTaken) => {
                    return Output::fail(
                        2,
                        "this space id is used again: nothing was restored (purge the entry to drop it)",
                    )
                }
                Err(RestoreError::Conflict(_)) => {
                    return Output::fail(
                        2,
                        "a device or a key of this space has founded or joined another space since: nothing was restored",
                    )
                }
                Err(RestoreError::Missing) => {
                    return Output::fail(
                        1,
                        "the folder of this space is not in the trash any more: `purge` removes the entry",
                    )
                }
                Err(RestoreError::Failed(why)) => {
                    return Output::fail(1, format!("the space could not be restored ({why})"))
                }
            };
            if let Err(e) = forget_row(st, row.dir_name.clone()).await {
                tracing::warn!(
                    "trash: the row of a restored space stays ({})",
                    e.code.as_str()
                );
            }
            admin_audit::record(st, kind::TRASH_RESTORED, Some(restored.short_id()), None).await;
            tracing::warn!(
                space = restored.short_id(),
                "a space was restored from the trash"
            );
            Output::ok(format!(
                "space {} restored: its routing is rebuilt; open Tomi on each device of the space, they resume at their next network action\n",
                restored.short_id()
            ))
        }
        TrashAction::Purge => {
            let row = match pick(&all, space) {
                Ok(r) => r,
                Err(out) => return out,
            };
            let short: String = row.space_id.chars().take(8).collect();
            let given = confirm.map(|c| c.trim().to_ascii_lowercase());
            let ok = given
                .as_deref()
                .is_some_and(|g| g.len() >= 8 && (g == row.space_id || g == short));
            if !ok {
                return Output::fail(
                    2,
                    format!(
                        "confirmation needed: retype the first 8 characters of the space id ({short}); \
                         this deletes the space for good"
                    ),
                );
            }
            if let Err(e) = remove_folder(st, &row.dir_name) {
                return Output::fail(1, format!("the folder could not be removed ({})", e.kind()));
            }
            if let Err(e) = forget_row(st, row.dir_name.clone()).await {
                return fail(&e);
            }
            admin_audit::record(st, kind::TRASH_PURGED, Some(short.clone()), None).await;
            tracing::warn!(space = short, "a space of the trash was purged");
            Output::ok(format!("space {short} purged for good\n"))
        }
    }
}

/// The maintenance: purges what is past its 7 days. Returns how many spaces went.
pub async fn purge_expired(st: &AppState, now: i64) -> u64 {
    let due: Vec<TrashRow> = match st.spaces.routing().read(|tx| rows(tx)).await {
        Ok(v) => v.into_iter().filter(|r| r.purge_after <= now).collect(),
        Err(_) => return 0,
    };
    let mut n = 0;
    for r in due {
        if remove_folder(st, &r.dir_name).is_ok()
            && forget_row(st, r.dir_name.clone()).await.is_ok()
        {
            admin_audit::record(
                st,
                kind::TRASH_EXPIRED,
                Some(r.space_id.chars().take(8).collect()),
                None,
            )
            .await;
            n += 1;
        }
    }
    n
}
