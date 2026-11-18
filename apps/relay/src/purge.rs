//! Removals that several endpoints share: purge of an item (§8.1) and closing of a history
//! transfer (§4.13, §8.1). Both run inside the caller's transaction; the files are deleted
//! by the caller after the commit (`Effects::unlink_blobs`).

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

use crate::error::ApiResult;
use crate::state::{Effects, Target, WakeEvent};
use crate::store::{add_event, members};
use crate::wire::uuid_format;

/// Purges an item: envelope erased, blob files scheduled for deletion, `item` event removed
/// from the log, quota freed. The row stays (without envelope) for the idempotence of §8.2.
/// Returns the number of bytes freed (envelope and blobs), or `None` if the item is unknown or
/// already purged.
pub fn purge_item(
    tx: &Connection,
    now: i64,
    item_id: &[u8; 16],
    fx: &mut Effects,
) -> ApiResult<Option<u64>> {
    let state: Option<(Option<i64>, i64)> = tx
        .query_row(
            "SELECT purged_at, envelope_size FROM items WHERE item_id = ?1",
            params![item_id.to_vec()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((None, envelope_size)) = state else {
        return Ok(None);
    };
    let blob_bytes: i64 = tx.query_row(
        "SELECT COALESCE(SUM(size), 0) FROM blobs WHERE item_id = ?1",
        params![item_id.to_vec()],
        |r| r.get(0),
    )?;
    let mut st = tx.prepare("SELECT blob_id FROM blobs WHERE item_id = ?1")?;
    let ids: Vec<Vec<u8>> = st
        .query_map(params![item_id.to_vec()], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in ids {
        if let Ok(b) = <[u8; 16]>::try_from(id.as_slice()) {
            fx.unlink_blobs.push(b);
        }
    }
    tx.execute(
        "DELETE FROM blobs WHERE item_id = ?1",
        params![item_id.to_vec()],
    )?;
    tx.execute(
        "UPDATE items SET envelope = NULL, purged_at = ?2 WHERE item_id = ?1",
        params![item_id.to_vec(), now],
    )?;
    tx.execute(
        "DELETE FROM events WHERE item_id = ?1 AND type = 'item'",
        params![item_id.to_vec()],
    )?;
    tx.execute(
        "DELETE FROM item_rings WHERE item_id = ?1",
        params![item_id.to_vec()],
    )?;
    Ok(Some(u64::try_from(envelope_size + blob_bytes).unwrap_or(0)))
}

/// Closes an open history transfer: blobs deleted, `history_transfer_closed` appended.
/// `by` is `None` when the relay closes it itself (expiry, revocation). Returns `false` if
/// the transfer is unknown or already closed.
pub fn close_transfer(
    tx: &Connection,
    now: i64,
    transfer_id: &[u8; 16],
    by: Option<&str>,
    outcome: &str,
    fx: &mut Effects,
) -> ApiResult<bool> {
    let row: Option<(String, String, Option<i64>)> = tx
        .query_row(
            "SELECT from_device, to_device, closed_at FROM transfers WHERE transfer_id = ?1",
            params![transfer_id.to_vec()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((from, to, None)) = row else {
        return Ok(false);
    };
    let mut st = tx.prepare("SELECT blob_id FROM blobs WHERE transfer_id = ?1")?;
    let ids: Vec<Vec<u8>> = st
        .query_map(params![transfer_id.to_vec()], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in ids {
        if let Ok(b) = <[u8; 16]>::try_from(id.as_slice()) {
            fx.unlink_blobs.push(b);
        }
    }
    tx.execute(
        "DELETE FROM blobs WHERE transfer_id = ?1",
        params![transfer_id.to_vec()],
    )?;
    tx.execute(
        "UPDATE transfers SET closed_at = ?2, outcome = ?3 WHERE transfer_id = ?1",
        params![transfer_id.to_vec(), now, outcome],
    )?;
    let seq = add_event(
        tx,
        now,
        "history_transfer_closed",
        by,
        None,
        &json!({
            "transfer_id": uuid_format(transfer_id),
            "by_device_id": by,
            "outcome": outcome,
        }),
    )?;
    // Both parties hear about it (WebSocket only: no push for this event).
    let member_ids: Vec<String> = members(tx)?.into_iter().map(|m| m.device_id).collect();
    let targets: Vec<Target> = [from, to]
        .into_iter()
        .filter(|d| Some(d.as_str()) != by && member_ids.contains(d))
        .map(|d| Target::quiet(&d))
        .collect();
    fx.wakes.push(WakeEvent {
        seq,
        fcm: false,
        targets,
    });
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, Tx};

    #[test]
    fn purging_an_unknown_item_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).unwrap();
        let purged = db
            .write_sync(|tx| {
                let mut fx = Effects::default();
                Ok(Tx::Commit(purge_item(tx, 1, &[7; 16], &mut fx)?))
            })
            .unwrap();
        assert_eq!(purged, None);
    }
}
