//! Revocation of a device (`api-v1.md` §3.4): the effects common to the three paths
//! (rotation, voluntary leave, administrator), in the caller's transaction.

use rusqlite::{params, Connection};
use serde_json::json;

use crate::error::ApiResult;
use crate::purge::close_transfer;
use crate::state::{Effects, Target, WakeEvent};
use crate::store::{add_event, device_by_id, members};
use crate::wire::ts_format;

/// How the device is revoked.
#[derive(Debug, Clone, Copy)]
pub enum Via<'a> {
    /// (a) by a `KeyRotation` signed by `by`; the `rotation` event is at `rotation_seq`.
    Rotation { by: &'a str, rotation_seq: i64 },
    /// (b) the device leaves by itself.
    SelfLeave { by: &'a str },
    /// (c) the administrator, over SSH.
    Admin,
}

impl Via<'_> {
    fn name(&self) -> &'static str {
        match self {
            Via::Rotation { .. } => "rotation",
            Via::SelfLeave { .. } => "self",
            Via::Admin => "admin",
        }
    }
    fn by(&self) -> Option<&str> {
        match self {
            Via::Rotation { by, .. } | Via::SelfLeave { by } => Some(by),
            Via::Admin => None,
        }
    }
}

/// Revokes one member. Returns the `seq` of its `device_revoked` event, or `None` if the
/// device is not a member (unknown or already revoked).
pub fn revoke_device(
    tx: &Connection,
    now: i64,
    device_id: &str,
    via: Via<'_>,
    fx: &mut Effects,
) -> ApiResult<Option<i64>> {
    let Some(dev) = device_by_id(tx, device_id)?.filter(|d| d.is_member()) else {
        return Ok(None);
    };
    // The key, the certificate and the presence go; the key stays in the list of forbidden keys.
    if let Some(pk) = &dev.sign_pk {
        tx.execute(
            "INSERT OR IGNORE INTO revoked_sign_pks (sign_pk) VALUES (?1)",
            params![pk],
        )?;
    }
    tx.execute(
        "UPDATE devices SET sign_pk = NULL, kx_pk = NULL, certificate = NULL, present_until = NULL, revoked_at = ?2
         WHERE device_id = ?1",
        params![device_id, now],
    )?;
    tx.execute(
        "DELETE FROM push_registrations WHERE device_id = ?1",
        params![device_id],
    )?;
    // Sessions are kept until they expire so that they answer 403 revoked, not 401.
    tx.execute(
        "UPDATE sessions SET revoked = 1 WHERE device_id = ?1",
        params![device_id],
    )?;

    // Pairing channels created by the device are closed.
    let mut st = tx.prepare(
        "SELECT channel_id FROM pairing_channels WHERE creator = ?1 AND state IN ('open', 'claimed')",
    )?;
    let channels: Vec<String> = st
        .query_map(params![device_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for ch in &channels {
        tx.execute(
            "UPDATE pairing_channels SET state = 'closed' WHERE channel_id = ?1",
            params![ch],
        )?;
        tx.execute(
            "DELETE FROM pairing_messages WHERE channel_id = ?1",
            params![ch],
        )?;
    }
    fx.pairing.extend(channels);

    // History transfers it sends or receives are purged (§8.1).
    let mut st = tx.prepare(
        "SELECT transfer_id FROM transfers WHERE closed_at IS NULL AND (from_device = ?1 OR to_device = ?1)",
    )?;
    let transfers: Vec<Vec<u8>> = st
        .query_map(params![device_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for t in transfers {
        if let Ok(id) = <[u8; 16]>::try_from(t.as_slice()) {
            close_transfer(tx, now, &id, None, "revoked", fx)?;
        }
    }

    let mut data = json!({
        "device_id": device_id,
        "revoked_at": ts_format(now),
        "via": via.name(),
        "revoked_by": via.by(),
    });
    if let Via::Rotation { rotation_seq, .. } = via {
        data["rotation_seq"] = json!(rotation_seq);
    }
    let seq = add_event(tx, now, "device_revoked", via.by(), None, &data)?;
    fx.close_ws.push(device_id.to_owned());
    let targets = members(tx)?
        .into_iter()
        .filter(|m| Some(m.device_id.as_str()) != via.by())
        .map(|m| Target::quiet(&m.device_id))
        .collect();
    fx.wakes.push(WakeEvent {
        seq,
        fcm: true,
        targets,
    });
    Ok(Some(seq))
}
