//! The history transfer announced at pairing (`spec/pairing-v1.md` §9).
//!
//! Two small records of the `meta` table link the two devices across the pairing:
//!
//! * on the **new device**, the announced transfer ([`AwaitedHistory`]): the user already said
//!   yes, so when the `history_transfer` event with that `transfer_id` arrives, the import runs
//!   without asking, even when the engine otherwise waits for `accept_import` (step 5);
//! * on the **inviter**, the export to start ([`ExportIntent`]): written in the transaction that
//!   adds the new device, and turned into an export task by the next `sync` once the log has
//!   caught up with the device's arrival (step 4). An export is long, and `confirm` must not
//!   wait for it; a crash or a cut network between the `201` and the task loses nothing.
//!
//! Both hold identifiers and counters only (the same kind of metadata as the `transfers` table).

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::runtime::engine::Engine;
use crate::runtime::store::{Db, DeviceState};
use crate::runtime::transfer::new_export_row;

const AWAITED_KEY: &str = "awaited_history";
const EXPORTS_KEY: &str = "pairing_exports";
/// Default `transfer_ttl_secs` (`api-v1.md` §2.8), when the relay does not announce one.
const DEFAULT_TRANSFER_TTL_SECS: u64 = 604_800;

/// A history transfer the inviter announced to this (new) device, not yet received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitedHistory {
    /// The transfer the inviter will export with: the import of exactly this id is accepted.
    pub transfer_id: String,
    /// Device id of the inviter, the only sender accepted for it.
    pub from_device_id: String,
    /// Items, from the inviter's estimate (display only).
    pub items: u64,
    /// Bytes, from the inviter's estimate (display only).
    pub bytes: u64,
    /// When the pairing completed, on this device's clock.
    pub announced_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ExportIntent {
    peer_device_id: String,
    transfer_id: String,
}

fn json_err(e: &serde_json::Error) -> CoreError {
    CoreError::Internal {
        reason: format!("json: {e}"),
    }
}

pub(crate) fn set_awaited_history(db: &Db<'_>, h: &AwaitedHistory) -> Result<()> {
    db.meta_set(
        AWAITED_KEY,
        &serde_json::to_vec(h).map_err(|e| json_err(&e))?,
    )
}

fn get_awaited_history(db: &Db<'_>) -> Result<Option<AwaitedHistory>> {
    // An unreadable record is as good as none: the transfer then follows the usual rule.
    Ok(db
        .meta_get(AWAITED_KEY)?
        .and_then(|b| serde_json::from_slice(&b).ok()))
}

/// Whether `transfer_id` from `from_device_id` is the announced transfer; if so the
/// announcement is consumed (the task now exists).
pub(crate) fn take_awaited_history(
    db: &Db<'_>,
    transfer_id: &str,
    from_device_id: &str,
) -> Result<bool> {
    let Some(h) = get_awaited_history(db)? else {
        return Ok(false);
    };
    if h.transfer_id == transfer_id && h.from_device_id == from_device_id {
        db.meta_delete(AWAITED_KEY)?;
        return Ok(true);
    }
    Ok(false)
}

fn read_intents(db: &Db<'_>) -> Result<Vec<ExportIntent>> {
    Ok(db
        .meta_get(EXPORTS_KEY)?
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default())
}

fn write_intents(db: &Db<'_>, intents: &[ExportIntent]) -> Result<()> {
    if intents.is_empty() {
        return db.meta_delete(EXPORTS_KEY);
    }
    db.meta_set(
        EXPORTS_KEY,
        &serde_json::to_vec(intents).map_err(|e| json_err(&e))?,
    )
}

/// Records that the history must be exported to `peer_device_id` with exactly `transfer_id`
/// (the id announced to the new device). Run in the transaction that adds the device.
pub(super) fn push_export_intent(
    db: &Db<'_>,
    peer_device_id: &str,
    transfer_id: &str,
) -> Result<()> {
    let mut intents = read_intents(db)?;
    let intent = ExportIntent {
        peer_device_id: peer_device_id.to_owned(),
        transfer_id: transfer_id.to_owned(),
    };
    if !intents.contains(&intent) {
        intents.push(intent);
    }
    write_intents(db, &intents)
}

impl Engine {
    /// The history transfer announced to this device at pairing and not received yet, if any
    /// ("History being prepared on PC-Bureau"). It is forgotten once the transfer exists as
    /// a task (see [`transfers`](Self::transfers)), or when the relay would have dropped it.
    pub fn awaited_history(&self) -> Result<Option<AwaitedHistory>> {
        let now = self.clock.now_ms();
        let ttl_ms = i64::try_from(
            self.limits()
                .transfer_ttl_secs
                .unwrap_or(DEFAULT_TRANSFER_TTL_SECS)
                .saturating_mul(1000),
        )
        .unwrap_or(i64::MAX);
        self.store.tx(|db| {
            let Some(h) = get_awaited_history(db)? else {
                return Ok(None);
            };
            if now.saturating_sub(h.announced_at_ms) > ttl_ms {
                db.meta_delete(AWAITED_KEY)?;
                return Ok(None);
            }
            Ok(Some(h))
        })
    }

    /// Turns the exports announced at pairing into tasks, for the recipients the log has caught
    /// up with. Runs at the start of [`step_transfers`](Self::step_transfers), i.e. after every
    /// sync. An intent whose recipient is no longer a verified member, or whose relay does not
    /// carry transfers, is dropped.
    pub(crate) fn promote_announced_exports(&self) -> Result<()> {
        if self.store.read(|db| db.meta_get(EXPORTS_KEY))?.is_none() {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let supported = self.limits().transfer_max_open.is_some();
        self.store.tx(|db| {
            let mut waiting = Vec::new();
            for intent in read_intents(db)? {
                let verified = db
                    .device(&intent.peer_device_id)?
                    .filter(|d| d.state == DeviceState::Verified);
                let Some(peer) = verified.filter(|_| supported) else {
                    continue;
                };
                if db.transfer(&intent.transfer_id)?.is_some() {
                    continue;
                }
                if peer.joined_seq <= db.cursor()? {
                    new_export_row(db, &intent.peer_device_id, &intent.transfer_id, now)?;
                } else {
                    waiting.push(intent);
                }
            }
            write_intents(db, &waiting)
        })
    }
}
