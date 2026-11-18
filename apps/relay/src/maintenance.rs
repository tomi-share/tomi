//! Periodic work: purge of delivered or expired items (`api-v1.md` §8.1), history transfer
//! expiry, clean-ups (§8.3), compaction of the log (§8.2). Each pass is a plain function so
//! the tests can run it at a chosen moment of the injected clock. Every pass runs on every
//! space, one after the other; the counters of the report add up.

use std::sync::Arc;
use std::time::Duration;

use rusqlite::{params, Connection};

use crate::blobstore;
use crate::error::ApiResult;
use crate::purge::{close_transfer, purge_item};
use crate::spaces::Space;
use crate::state::{AppState, Effects};
use crate::store::{self, members, DeviceRow};

const DAY_MS: i64 = 86_400_000;
/// Expired sessions are kept this long so that they still answer `session_expired`.
const SESSION_GRACE_MS: i64 = DAY_MS;
/// The quota warning is given at most once an hour.
const QUOTA_WARN_EVERY_MS: i64 = 3_600_000;

/// What a pass did (counters only, never identifiers: §9).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub items_purged: u64,
    pub bytes_freed: u64,
    pub transfers_closed: u64,
    pub blobs_collected: u64,
    pub channels_removed: u64,
    pub events_compacted: u64,
    /// Receipts of deletions of spaces older than `deletion_receipt_ttl_secs` (§4.4.2).
    pub receipts_expired: u64,
}

impl Report {
    fn add(&mut self, o: &Report) {
        self.items_purged += o.items_purged;
        self.bytes_freed += o.bytes_freed;
        self.transfers_closed += o.transfers_closed;
        self.blobs_collected += o.blobs_collected;
        self.channels_removed += o.channels_removed;
        self.events_compacted += o.events_compacted;
        self.receipts_expired += o.receipts_expired;
    }
}

/// Deletes the receipts of deletions of spaces (`routing.db`, §4.4.2) that are
/// `deletion_receipt_ttl_secs` old or more: the route answers `404` for them already.
async fn purge_receipts(st: &AppState, now: i64) -> u64 {
    let ttl_ms =
        i64::try_from(st.cfg.deletion_receipt_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let until = now.saturating_sub(ttl_ms);
    let r = st
        .spaces
        .routing()
        .write(move |tx| {
            Ok(tx.execute(
                "DELETE FROM space_deletions WHERE deleted_at <= ?1",
                params![until],
            )?)
        })
        .await;
    match r {
        Ok(n) => u64::try_from(n).unwrap_or(0),
        Err(e) => {
            tracing::warn!(
                "purge: expired receipts could not be removed ({})",
                e.code.as_str()
            );
            0
        }
    }
}

/// Is the item due for purge at `now`? (a) its deadline passed, (c) every active member that
/// was there before it has released it and the grace period has run.
/// `devices` are the members of the space (`members()`), read by the caller once for a pass.
fn due(
    tx: &Connection,
    st: &AppState,
    devices: &[DeviceRow],
    item_id: &[u8; 16],
    now: i64,
) -> ApiResult<bool> {
    let row: Option<(i64, String, i64, i64)> = tx
        .query_row(
            "SELECT seq, sender, created_at, expires_at FROM items WHERE item_id = ?1 AND purged_at IS NULL",
            params![item_id.to_vec()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((seq, sender, created_at, expires_at)) = row else {
        return Ok(false);
    };
    if expires_at <= now {
        return Ok(true);
    }
    let grace = i64::try_from(st.cfg.release_grace_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    // R: the active members, other than the sender, that were there before the item.
    let mut latest = created_at;
    for m in devices {
        if m.device_id == sender || !m.is_active(now, &st.cfg) || m.joined_seq >= seq {
            continue;
        }
        let released: Option<i64> = tx
            .query_row(
                "SELECT released_at FROM receipts WHERE item_id = ?1 AND device_id = ?2",
                params![item_id.to_vec(), m.device_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        match released {
            Some(t) => latest = latest.max(t),
            None => return Ok(false),
        }
    }
    Ok(now >= latest.saturating_add(grace))
}

/// The pass that runs every `purge_interval_secs`, on every space.
pub async fn run_once(st: &AppState) -> Report {
    let mut report = Report::default();
    let now = st.now();
    for space in st.spaces.list() {
        report.add(&purge_space(st, &space, now).await);
    }
    report.receipts_expired = purge_receipts(st, now).await;
    warn_if_nearly_full(st, now).await;
    if report.items_purged + report.transfers_closed + report.blobs_collected > 0 {
        tracing::info!(
            items = report.items_purged,
            bytes_freed = report.bytes_freed,
            transfers = report.transfers_closed,
            blobs = report.blobs_collected,
            "purge: pass done"
        );
    }
    report
}

async fn purge_space(st: &AppState, space: &Arc<Space>, now: i64) -> Report {
    let mut report = Report::default();

    // Items: whether each one is due is judged in a read (one snapshot, no write lock). Only the
    // items found due are checked again and purged, each in its own write transaction: a pass
    // that purges nothing writes nothing.
    let judge = st.clone();
    let ids: Vec<[u8; 16]> = match space
        .db
        .read(move |tx| {
            let mut s = tx.prepare("SELECT item_id FROM items WHERE purged_at IS NULL")?;
            let all: Vec<[u8; 16]> = s
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .filter_map(Result::ok)
                .filter_map(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
                .collect();
            let devices = members(tx)?;
            let mut due_ids = Vec::new();
            for id in all {
                // An item that cannot be judged here stays a candidate: it is judged again in
                // its own write transaction, where a failure is logged. One bad row must not
                // cost the whole pass.
                if due(tx, &judge, &devices, &id, now).unwrap_or(true) {
                    due_ids.push(id);
                }
            }
            Ok(due_ids)
        })
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                space = space.short_id(),
                "purge: the items due could not be listed ({})",
                e.code.as_str()
            );
            Vec::new()
        }
    };
    for id in ids {
        let stc = st.clone();
        let r = space
            .db
            .write(move |tx| {
                let mut fx = Effects::default();
                let devices = members(tx)?;
                if !due(tx, &stc, &devices, &id, now)? {
                    return Ok((None, fx));
                }
                let freed = purge_item(tx, now, &id, &mut fx)?;
                Ok((freed, fx))
            })
            .await;
        match r {
            Ok((freed, fx)) => {
                if let Some(bytes) = freed {
                    report.items_purged += 1;
                    report.bytes_freed += bytes;
                }
                st.apply(space, fx).await;
            }
            Err(e) => tracing::warn!(
                space = space.short_id(),
                "purge: an item could not be purged ({})",
                e.code.as_str()
            ),
        }
    }

    // History transfers past their deadline.
    let transfers: Vec<[u8; 16]> = space
        .db
        .read(move |tx| {
            let mut s = tx.prepare(
                "SELECT transfer_id FROM transfers WHERE closed_at IS NULL AND expires_at <= ?1",
            )?;
            let v = s
                .query_map(params![now], |r| r.get::<_, Vec<u8>>(0))?
                .filter_map(Result::ok)
                .filter_map(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
                .collect();
            Ok(v)
        })
        .await
        .unwrap_or_default();
    for id in transfers {
        let r = space
            .db
            .write(move |tx| {
                let mut fx = Effects::default();
                let closed = close_transfer(tx, now, &id, None, "expired", &mut fx)?;
                Ok((closed, fx))
            })
            .await;
        if let Ok((closed, fx)) = r {
            report.transfers_closed += u64::from(closed);
            st.apply(space, fx).await;
        }
    }

    // Challenges, old sessions, pairing channels, orphan blobs.
    let orphan_ttl =
        i64::try_from(st.cfg.blob_orphan_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let cleaned = space
        .db
        .write(move |tx| {
            tx.execute("DELETE FROM challenges WHERE expires_at <= ?1", params![now])?;
            tx.execute(
                "DELETE FROM sessions WHERE expires_at < ?1",
                params![now.saturating_sub(SESSION_GRACE_MS)],
            )?;
            let mut s = tx.prepare("SELECT channel_id FROM pairing_channels WHERE expires_at <= ?1")?;
            let channels: Vec<String> = s
                .query_map(params![now], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            tx.execute("DELETE FROM pairing_channels WHERE expires_at <= ?1", params![now])?;
            let mut s = tx.prepare(
                "SELECT blob_id FROM blobs WHERE item_id IS NULL AND transfer_id IS NULL AND last_activity <= ?1",
            )?;
            let orphans: Vec<Vec<u8>> = s
                .query_map(params![now.saturating_sub(orphan_ttl)], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for b in &orphans {
                tx.execute("DELETE FROM blobs WHERE blob_id = ?1", params![b])?;
            }
            Ok((channels, orphans))
        })
        .await;
    if let Ok((channels, orphans)) = cleaned {
        // The routing index forgets what the database just deleted.
        st.spaces
            .prune_sessions(now.saturating_sub(SESSION_GRACE_MS));
        st.spaces.remove_channels(&channels);
        report.channels_removed = channels.len() as u64;
        report.blobs_collected = orphans.len() as u64;
        let ids: Vec<[u8; 16]> = orphans
            .iter()
            .filter_map(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
            .collect();
        st.apply(
            space,
            Effects {
                unlink_blobs: ids,
                pairing: channels,
                ..Effects::default()
            },
        )
        .await;
    }
    report
}

/// WARN at most once an hour above 90 % of the quota of a space that has one, and above 90 %
/// of the total of the relay (§8.4). The log names the space by the first 8 characters of its
/// id and never carries the usage of another space.
async fn warn_if_nearly_full(st: &AppState, now: i64) {
    let mut total = 0u64;
    for space in st.spaces.list() {
        let Ok(used) = space.db.read(|tx| store::used_bytes(tx)).await else {
            continue;
        };
        total = total.saturating_add(used);
        let Some(max) = st.spaces.space_max(&space.id, &st.cfg) else {
            continue;
        };
        if used.saturating_mul(10) > max.saturating_mul(9)
            && st.spaces.may_warn(Some(space.id), now, QUOTA_WARN_EVERY_MS)
        {
            tracing::warn!(
                space = space.short_id(),
                used_bytes = used,
                max_bytes = max,
                "quota: more than 90% of the space quota is used"
            );
        }
    }
    let max = st.cfg.max_relay_bytes;
    if total.saturating_mul(10) > max.saturating_mul(9)
        && st.spaces.may_warn(None, now, QUOTA_WARN_EVERY_MS)
    {
        tracing::warn!(
            used_bytes = total,
            max_bytes = max,
            "quota: more than 90% of the relay total is used"
        );
    }
}

/// The hourly compaction of the log (§8.2), on every space: events other than `item` older than
/// `event_retention_days`, their idempotence rows, purged items, closed transfers.
pub async fn compact(st: &AppState) -> Report {
    let now = st.now();
    let cutoff = now.saturating_sub(
        i64::try_from(st.cfg.event_retention_days)
            .unwrap_or(i64::MAX)
            .saturating_mul(DAY_MS),
    );
    let mut report = Report::default();
    for space in st.spaces.list() {
        let r = space
            .db
            .write(move |tx| {
                let top: Option<i64> = tx.query_row(
                    "SELECT MAX(seq) FROM events WHERE type != 'item' AND at < ?1",
                    params![cutoff],
                    |r| r.get(0),
                )?;
                let mut removed = 0u64;
                if let Some(top) = top {
                    tx.execute(
                        "DELETE FROM item_states WHERE seq IN (SELECT seq FROM events WHERE type = 'item_state' AND at < ?1)",
                        params![cutoff],
                    )?;
                    tx.execute(
                        "DELETE FROM tombstones WHERE seq IN (SELECT seq FROM events WHERE type = 'tombstone' AND at < ?1)",
                        params![cutoff],
                    )?;
                    // Same retention as the tombstones, before the events they point at go.
                    tx.execute(
                        "DELETE FROM attachment_tombstones WHERE seq IN (SELECT seq FROM events WHERE type = 'attachment_tombstone' AND at < ?1)",
                        params![cutoff],
                    )?;
                    removed = u64::try_from(tx.execute(
                        "DELETE FROM events WHERE type != 'item' AND at < ?1",
                        params![cutoff],
                    )?)
                    .unwrap_or(0);
                    // The horizon only moves forward.
                    tx.execute(
                        "UPDATE space SET retention_floor_seq = MAX(retention_floor_seq, ?1) WHERE id = 1",
                        params![top],
                    )?;
                }
                tx.execute(
                    "DELETE FROM items WHERE purged_at IS NOT NULL AND purged_at < ?1",
                    params![cutoff],
                )?;
                tx.execute(
                    "DELETE FROM transfers WHERE closed_at IS NOT NULL AND closed_at < ?1",
                    params![cutoff],
                )?;
                Ok(removed)
            })
            .await;
        match r {
            Ok(n) => {
                report.events_compacted += n;
                if n > 0 {
                    tracing::info!(
                        space = space.short_id(),
                        events = n,
                        "compaction: old events removed"
                    );
                }
            }
            Err(e) => tracing::warn!(
                space = space.short_id(),
                "compaction failed ({})",
                e.code.as_str()
            ),
        }
    }
    report
}
/// Starts the periodic tasks: the purge pass every `purge_interval_secs`, the compaction
/// every hour. Blob files without a row were already removed at start-up (§8.3).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let every = Duration::from_secs(state.cfg.purge_interval_secs.max(1));
        let mut tick = tokio::time::interval(every);
        let mut last_compaction: Option<tokio::time::Instant> = None;
        loop {
            tick.tick().await;
            run_once(&state).await;
            if last_compaction.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                compact(&state).await;
                last_compaction = Some(tokio::time::Instant::now());
            }
        }
    });
}

/// Removes every blob file that has no row, in every space (tests).
pub fn sweep_orphan_files(st: &AppState) -> usize {
    let mut n = 0;
    for space in st.spaces.list() {
        let known: std::collections::HashSet<[u8; 16]> = space
            .db
            .read_sync(|tx| {
                let mut s = tx.prepare("SELECT blob_id FROM blobs")?;
                let v = s
                    .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                    .filter_map(Result::ok)
                    .filter_map(|b| <[u8; 16]>::try_from(b.as_slice()).ok())
                    .collect();
                Ok(v)
            })
            .unwrap_or_default();
        for id in blobstore::list(&space.dir) {
            if !known.contains(&id) {
                blobstore::remove(&space.dir, &id);
                n += 1;
            }
        }
    }
    n
}
