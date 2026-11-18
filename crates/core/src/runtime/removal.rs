//! Removing ONE attachment of an item (`format-v1.md` §8.10, `api-v1.md` §4.8 bis).
//!
//! A removal empties the slot as if the attachment had never existed on this device: the
//! `item_files` row, the encrypted copy and the decrypted cache are erased, and the entry of
//! the local payload loses its name, type, size, key, nonce prefix and thumbnail
//! ([`Item::with_attachment_emptied`]). The slot keeps its place (the index is part of the
//! chunk AAD) and the registry `attachment_removals` remembers it, independently of the
//! item, so that a removal that arrives before the item, during a download or during a
//! history import is not lost.
//!
//! "Everywhere" also signs an [`AttachmentTombstone`] and queues it (`OpKind::AttachmentTombstone`),
//! replayed until the relay takes it. A relay that does not know the route answers `404`: the
//! operation then waits a long time and is never dropped.

use std::collections::BTreeSet;

use crate::attachment_tombstone::{AttachmentTombstone, ATTACHMENT_TOMBSTONE_AT_MS_MAX};
use crate::error::{CoreError, Result};
use crate::item::Item;
use crate::runtime::engine::{Active, Engine, FEATURES_KEY, FEATURE_ATTACHMENT_TOMBSTONES};
use crate::runtime::store::{
    Db, FileState, ItemOrigin, ItemRow, ItemStatus, NewOp, OpKind, SendStatus,
};

/// What [`Engine::delete_attachment`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentRemoval {
    /// The attachment is gone (or already was: the call is idempotent).
    AttachmentRemoved,
    /// It was the last thing in the item (no other attachment, no text, no URL): the whole
    /// item was deleted instead, with the same scope.
    ItemDeleted,
}

/// What [`remove_in_tx`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The slot was emptied now.
    Removed,
    /// Nothing to do on this device (unknown item, unreadable item, unknown or already removed
    /// slot). The registry was still updated when the call carries a statement.
    Nothing,
}

/// Slots of `item` that must not exist on this device: marked in its payload, recorded in the
/// registry for the same `blob_id`, or listed by the caller (`extra`: the `removed_attachments`
/// of a history index).
pub(crate) fn removed_slots(db: &Db<'_>, item: &Item, extra: &[u32]) -> Result<BTreeSet<u32>> {
    let mut out: BTreeSet<u32> = item.removed_slots().into_iter().collect();
    out.extend(extra.iter().copied());
    for r in db.attachment_removals(&item.id)? {
        if item
            .attachments
            .get(r.idx as usize)
            .is_some_and(|a| a.blob_id == r.blob_id)
        {
            out.insert(r.idx);
        }
    }
    out.retain(|s| (*s as usize) < item.attachments.len());
    Ok(out)
}

/// `item` with every slot of `slots` emptied (those already marked are left as they are).
pub(crate) fn emptied(item: &Item, slots: &BTreeSet<u32>) -> Result<Item> {
    let marked = item.removed_slots();
    let mut out = item.clone();
    for s in slots.iter().filter(|s| !marked.contains(s)) {
        if let Some(next) = out.with_attachment_emptied(*s)? {
            out = next;
        }
    }
    Ok(out)
}

/// Whether removing `slot` would leave nothing in the item: no other attachment left, no text,
/// no URL. The title is ignored (secret or not).
fn would_leave_nothing(item: &Item, slot: u32) -> bool {
    let removed = item.removed_slots();
    let others = (0..item.attachments.len())
        .filter_map(|i| u32::try_from(i).ok())
        .filter(|i| *i != slot && !removed.contains(i))
        .count();
    let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    let no_text = item
        .text
        .as_ref()
        .is_none_or(|t| t.plain.trim().is_empty() && blank(&t.html) && blank(&t.markdown));
    others == 0 && no_text && blank(&item.url)
}

/// Removes attachment `slot` from the local copy, inside the caller's transaction. Files to
/// erase after the commit are pushed to `erased`.
///
/// `expected_blob`: the `blob_id` of a received statement, checked against the payload
/// (`Err(malformed_statement)` on a mismatch); `None` for a removal decided on this device.
#[allow(clippy::too_many_arguments)]
pub(crate) fn remove_in_tx(
    db: &Db<'_>,
    a: &Active,
    item_id: &str,
    slot: u32,
    expected_blob: Option<&str>,
    everywhere: bool,
    at_ms: i64,
    now: i64,
    erased: &mut Vec<String>,
) -> Result<Applied> {
    let readable = db
        .item(item_id)?
        .filter(|r| r.status == ItemStatus::Readable && r.sealed.is_some());
    let Some(mut row) = readable else {
        // Not here yet, or not readable yet: the registry applies it when the item is opened.
        if let Some(blob) = expected_blob {
            db.put_attachment_removal(item_id, slot, blob, everywhere, at_ms, now)?;
        }
        return Ok(Applied::Nothing);
    };
    let item = a
        .lsk
        .open_item(row.sealed.as_deref().unwrap_or_default(), item_id)?;
    let Some(att) = item.attachments.get(slot as usize) else {
        return Ok(Applied::Nothing);
    };
    if expected_blob.is_some_and(|b| b != att.blob_id) {
        return Err(CoreError::statement(
            "attachment tombstone does not match the item",
        ));
    }
    db.put_attachment_removal(item_id, slot, &att.blob_id, everywhere, at_ms, now)?;
    if item.removed_slots().contains(&slot) {
        return Ok(Applied::Nothing);
    }
    let was_remote = db
        .files(item_id)?
        .iter()
        .any(|f| f.idx == slot && f.state == FileState::Remote);
    erased.extend(db.delete_file(item_id, slot)?);
    if let Some(next) = item.with_attachment_emptied(slot)? {
        row.sealed = Some(a.lsk.seal_item(&next)?);
        // The relay's list of blobs keeps neither the id nor the encrypted size of the slot.
        row.blobs.retain(|b| b.blob_id != att.blob_id);
        db.put_item(&row)?;
    }
    release_if_done(db, &row, was_remote, now)?;
    Ok(Applied::Removed)
}

/// After a removal on a received item: when nothing is left to fetch, the relay may purge it.
fn release_if_done(db: &Db<'_>, row: &ItemRow, was_remote: bool, now: i64) -> Result<()> {
    if !was_remote || row.origin != ItemOrigin::Received {
        return Ok(());
    }
    if db
        .files(&row.item_id)?
        .iter()
        .any(|f| f.state == FileState::Remote)
    {
        return Ok(());
    }
    queue_release(db, &row.item_id, now)
}

/// Queues the `release` ack of a received item: nothing is left to fetch from the relay.
pub(crate) fn queue_release(db: &Db<'_>, item_id: &str, now: i64) -> Result<()> {
    let mut op = NewOp::new(OpKind::Ack);
    op.item_id = Some(item_id.to_owned());
    op.flag = true;
    db.enqueue_op(&op, now).map(|_| ())
}

impl Engine {
    /// Whether the relay takes the removal of an attachment "everywhere": its capability
    /// `attachment_tombstones`, read at the last session. `false` until a session has told.
    /// The apps offer "everywhere" only then; "from this device" is always possible.
    pub fn supports_attachment_removal(&self) -> bool {
        self.store
            .read(|db| db.meta_get(FEATURES_KEY))
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .is_some_and(|f| f.iter().any(|s| s == FEATURE_ATTACHMENT_TOMBSTONES))
    }

    /// Removes attachment `slot` of an item (the `slot` of [`crate::runtime::AttachmentView`]).
    ///
    /// * unknown item, unreadable item, unknown or already removed slot: no effect, `Ok`
    ///   (idempotent); the attachment is never downloaded;
    /// * an item still waiting to be sent refuses the removal, whatever the scope
    ///   (`InvalidInput`: its envelope and its upload would carry the attachment all the same);
    /// * `everywhere = false`: this device only, no network;
    /// * `everywhere = true`: signs the removal, applies it here and queues it;
    /// * if nothing is left in the item (no other attachment, no text, no URL; the title is
    ///   ignored), the item itself is deleted with the same scope: `ItemDeleted`.
    ///
    /// No biometry: like deleting an item.
    pub fn delete_attachment(
        &self,
        item_id: &str,
        slot: u32,
        everywhere: bool,
    ) -> Result<AttachmentRemoval> {
        {
            let _serial = self.serial(&self.dispatch_lock);
            if self.remove_attachment_locked(item_id, slot, everywhere)? == Locked::Done {
                return Ok(AttachmentRemoval::AttachmentRemoved);
            }
        }
        // Nothing else left in the item. The lock is released: deleting an item takes it again.
        if everywhere {
            self.delete_everywhere(item_id)?;
        } else {
            self.delete_local(item_id)?;
        }
        Ok(AttachmentRemoval::ItemDeleted)
    }

    fn remove_attachment_locked(
        &self,
        item_id: &str,
        slot: u32,
        everywhere: bool,
    ) -> Result<Locked> {
        let now = self.clock.now_ms();
        let mut erased = Vec::new();
        let outcome = self.with_crypto(|_c, db, a| {
            let Some(row) = db
                .item(item_id)?
                .filter(|r| r.status == ItemStatus::Readable && r.sealed.is_some())
            else {
                return Ok(Locked::Done);
            };
            let item = a
                .lsk
                .open_item(row.sealed.as_deref().unwrap_or_default(), item_id)?;
            let Some(att) = item.attachments.get(slot as usize) else {
                return Ok(Locked::Done);
            };
            if item.removed_slots().contains(&slot) {
                return Ok(Locked::Done);
            }
            if would_leave_nothing(&item, slot) {
                return Ok(Locked::LeavesNothing);
            }
            // The item is still to be sent: its envelope (re-sealed from the local record after a
            // rotation) and its upload would carry the attachment all the same, whatever the scope.
            if row.send_status == Some(SendStatus::Queued) {
                return Err(CoreError::input(
                    "the item has not been sent yet: remove the attachment once it is sent",
                ));
            }
            let statement = if everywhere {
                Some(
                    a.keys.sign_attachment_tombstone(
                        &a.space_id,
                        item_id,
                        &att.blob_id,
                        slot,
                        u64::try_from(now)
                            .unwrap_or(0)
                            .min(ATTACHMENT_TOMBSTONE_AT_MS_MAX),
                    )?,
                )
            } else {
                None
            };
            remove_in_tx(
                db,
                a,
                item_id,
                slot,
                None,
                everywhere,
                now,
                now,
                &mut erased,
            )?;
            if let Some(t) = statement {
                // No `item_id` on the operation: it survives the erasure of the item.
                let mut op = NewOp::new(OpKind::AttachmentTombstone);
                op.payload = Some(t.to_bytes()?);
                db.enqueue_op(&op, now)?;
            }
            Ok(Locked::Done)
        });
        let outcome = outcome?;
        for p in erased {
            let _ = std::fs::remove_file(self.store.file_path(&p));
        }
        if outcome == Locked::Done {
            self.notify();
        }
        Ok(outcome)
    }

    /// Applies a removal received in the log (`apply_in_tx`). A statement that does not
    /// verify, or does not match the item, is a diagnostic: it never blocks the sync.
    pub(crate) fn apply_attachment_tombstone_event(
        &self,
        c: &crate::runtime::engine::Crypto,
        db: &Db<'_>,
        a: &Active,
        statement: &[u8],
        erased: &mut Vec<String>,
    ) -> Result<()> {
        let now = self.clock.now_ms();
        let verified = AttachmentTombstone::from_bytes(statement).and_then(|t| {
            c.trust.verify_attachment_tombstone(&t)?;
            Ok(t)
        });
        let t = match verified {
            Ok(t) => t,
            Err(e) => return self.diag(db, "attachment_tombstone_refused", e.code()),
        };
        if db.is_tombstoned(&t.item_id)? {
            return Ok(());
        }
        let at = i64::try_from(t.deleted_at_ms).unwrap_or(i64::MAX);
        match remove_in_tx(
            db,
            a,
            &t.item_id,
            t.slot,
            Some(&t.blob_id),
            true,
            at,
            now,
            erased,
        ) {
            Ok(_) => Ok(()),
            Err(e @ CoreError::MalformedStatement { .. }) => {
                self.diag(db, "attachment_tombstone_refused", e.code())
            }
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Locked {
    Done,
    LeavesNothing,
}
