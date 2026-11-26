//! What the apps do: send, delete, read, pin, list (`spec/pin-contract.md`, Kotlin
//! `TomiCore`). Everything here is local first; the network follows through the outbox.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::crypto::{new_blob_id, new_item_id};
use crate::envelope::seal_item;
use crate::error::{CoreError, Result};
use crate::format::{encrypted_attachment_len, CHUNK_CIPHERTEXT_SIZE, CHUNK_SIZE};
use crate::item::{Attachment, Item, ItemText, ReadPolicy, SourceFormat, Thumbnail};
use crate::runtime::engine::{jitter_i64, lock, Active, Blocked, Crypto, Engine};
use crate::runtime::outbox::StepErr;
use crate::runtime::store::{
    cache_file_name, encrypted_file_name, BlobUpload, DeviceState, FileRow, FileState, ItemOrigin,
    ItemRow, ItemStatus, NewOp, OpKind, SendStatus, CACHE_DIR,
};
use crate::runtime::transport::RelayError;
use crate::stream::{ChunkDecryptor, ChunkEncryptor};
use crate::{DevicePublic, UnreadableReason};

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

// The Debug impls of this file's content-carrying types never print content (title, text,
// url, file names, paths, thumbnails): a `{:?}` in a log must not leak a secret (AGENTS.md
// golden rule 3; same policy as `Item` in item.rs).

/// Text of a draft; `plain` is always required.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftText {
    pub plain: String,
    pub html: Option<String>,
    pub markdown: Option<String>,
    pub source_format: SourceFormat,
}

impl std::fmt::Debug for DraftText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftText")
            .field("source_format", &self.source_format)
            .field("has_html", &self.html.is_some())
            .field("has_markdown", &self.markdown.is_some())
            .finish_non_exhaustive()
    }
}

/// A file staged by the app; the core reads it before `enqueue` returns.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftAttachment {
    pub path: String,
    pub name: String,
    pub mime: String,
    pub thumbnail: Option<Thumbnail>,
}

impl std::fmt::Debug for DraftAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftAttachment")
            .field("has_thumbnail", &self.thumbnail.is_some())
            .finish_non_exhaustive()
    }
}

/// What the UI hands over to send.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingDraft {
    /// Item id chosen by the caller (UUID v4) so a retry is idempotent; generated if `None`.
    pub id: Option<String>,
    pub title: Option<String>,
    pub text: Option<DraftText>,
    pub url: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    pub secret: bool,
    pub expires_at_ms: Option<i64>,
    pub read_policy: ReadPolicy,
}

impl std::fmt::Debug for OutgoingDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutgoingDraft")
            .field("id", &self.id)
            .field("has_title", &self.title.is_some())
            .field("has_text", &self.text.is_some())
            .field("has_url", &self.url.is_some())
            .field("attachments", &self.attachments.len())
            .field("secret", &self.secret)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("read_policy", &self.read_policy)
            .finish()
    }
}

/// Result of [`Engine::dispatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The relay accepted the item.
    Sent,
    /// No network or a temporary relay error: retry later.
    RetryLater,
    /// The item no longer exists locally.
    NotFound,
    /// The relay refused it for good, or it expired before sending.
    Failed,
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentView {
    /// Position of the attachment in the payload, stable for the life of the item: what
    /// [`Engine::attachment_path`] and [`Engine::delete_attachment`] take. It is the index in
    /// `attachments` as long as no attachment was removed; a removed one is not listed, so
    /// the following ones keep their slot.
    pub slot: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub thumbnail: Option<Thumbnail>,
    /// Absolute path of the decrypted copy while it is in the cache (until the item goes
    /// away or [`Engine::purge_plaintext_cache`]). `None` otherwise: call
    /// [`Engine::attachment_path`], which decrypts the local encrypted copy on demand (or
    /// downloads it first).
    pub local_path: Option<String>,
    /// `false` when the file cannot be fetched any more (not transferred, purged).
    pub available: bool,
}

impl std::fmt::Debug for AttachmentView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentView")
            .field("slot", &self.slot)
            .field("size", &self.size)
            .field("has_thumbnail", &self.thumbnail.is_some())
            .field("has_local_path", &self.local_path.is_some())
            .field("available", &self.available)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum ItemContentView {
    Readable {
        title: Option<String>,
        text: Option<DraftText>,
        url: Option<String>,
        attachments: Vec<AttachmentView>,
        secret: bool,
    },
    Unreadable {
        reason: UnreadableReason,
    },
}

impl std::fmt::Debug for ItemContentView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemContentView::Readable {
                title,
                text,
                url,
                attachments,
                secret,
            } => f
                .debug_struct("Readable")
                .field("has_title", &title.is_some())
                .field("has_text", &text.is_some())
                .field("has_url", &url.is_some())
                .field("attachments", &attachments.len())
                .field("secret", secret)
                .finish(),
            ItemContentView::Unreadable { reason } => f
                .debug_struct("Unreadable")
                .field("reason", reason)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    Queued,
    Sent,
    Delivered,
    ExpiredBeforeDelivery,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryView {
    pub device_id: String,
    pub device_name: String,
    pub state: DeliveryState,
}

/// One item of the feed. A secret keeps its content here: the UI masks it until the user
/// reveals it (spec `format-v1.md` §5.4.5), and must never index or log it.
#[derive(Clone, PartialEq, Eq)]
pub struct ItemView {
    pub id: String,
    pub content: ItemContentView,
    pub source_device_id: String,
    pub source_device_name: String,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub read_policy: ReadPolicy,
    pub read: bool,
    pub origin: ItemOrigin,
    pub send_status: Option<SendStatus>,
    pub deliveries: Vec<DeliveryView>,
    pub pinned: bool,
    /// Time of the decision to pin, null when not pinned.
    pub pinned_at_ms: Option<i64>,
    /// Archived on all devices (`spec/archive-contract.md`). Pin and archive are exclusive at
    /// display: a pinned item is never reported archived, even if both registers say so.
    pub archived: bool,
    /// Time of the decision to archive, null when not archived (or shown pinned).
    pub archived_at_ms: Option<i64>,
}

// `content` is masked by its own Debug; device names are metadata, not content.
impl std::fmt::Debug for ItemView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemView")
            .field("id", &self.id)
            .field("content", &self.content)
            .field("source_device_id", &self.source_device_id)
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("read_policy", &self.read_policy)
            .field("read", &self.read)
            .field("origin", &self.origin)
            .field("send_status", &self.send_status)
            .field("deliveries", &self.deliveries.len())
            .field("pinned", &self.pinned)
            .field("pinned_at_ms", &self.pinned_at_ms)
            .field("archived", &self.archived)
            .field("archived_at_ms", &self.archived_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub state: DeviceState,
    pub last_seen_at_ms: Option<i64>,
    pub is_this_device: bool,
}

/// State of the space as the app shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceStatus {
    Active,
    Revoked,
    UpdateRequired,
    /// `NotFound`: the relay does not find the space or this device (`401` twice,
    /// `409 space_not_initialized`). Not a proof of anything: nothing is erased, the relay is
    /// asked again (`api-v1.md` §7.2, §7.9).
    NotFound,
    /// Blocked for another reason (format limited by another device).
    Blocked(String),
}

fn policy_str(p: ReadPolicy) -> &'static str {
    match p {
        ReadPolicy::UntilExpiry => "until_expiry",
        ReadPolicy::AfterFirstRead => "after_first_read",
    }
}

impl Engine {
    // -----------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------

    /// Feed, newest first. Items past their explicit expiration are not returned.
    pub fn list_items(&self) -> Result<Vec<ItemView>> {
        let now = self.clock.now_ms();
        let server_now = self.server_now_ms();
        self.read_crypto(|c, a| {
            self.store.read(|db| {
                let mut out = Vec::new();
                for row in db.items()? {
                    if row.payload_expires_at_ms.is_some_and(|t| now >= t) {
                        continue;
                    }
                    out.push(self.view_of(c, a, db, &row, server_now)?);
                }
                Ok(out)
            })
        })
    }

    pub fn get_item(&self, item_id: &str) -> Result<Option<ItemView>> {
        let server_now = self.server_now_ms();
        self.read_crypto(|c, a| {
            self.store.read(|db| match db.item(item_id)? {
                Some(row) => Ok(Some(self.view_of(c, a, db, &row, server_now)?)),
                None => Ok(None),
            })
        })
    }

    fn view_of(
        &self,
        c: &Crypto,
        a: &Active,
        db: &crate::runtime::store::Db<'_>,
        row: &ItemRow,
        server_now: i64,
    ) -> Result<ItemView> {
        let devices = db.devices()?;
        let name_of = |id: &str| {
            devices
                .iter()
                .find(|d| d.device_id == id)
                .map_or_else(|| "Appareil".to_owned(), |d| d.name.clone())
        };
        let (content, expires_at_ms, read_policy) = match (row.status, &row.sealed) {
            (ItemStatus::Readable, Some(sealed)) => {
                let item = a.lsk.open_item(sealed, &row.item_id)?;
                let files = db.files(&row.item_id)?;
                let removed = item.removed_slots();
                let attachments = item
                    .attachments
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !removed.iter().any(|s| *s as usize == *i))
                    .map(|(i, att)| {
                        let f = files.iter().find(|f| usize::try_from(f.idx) == Ok(i));
                        let local = f
                            .filter(|f| f.state == FileState::Done)
                            .map(|f| self.store.file_path(&cache_file_name(&row.item_id, f.idx)))
                            .filter(|p| p.exists())
                            .map(|p| p.to_string_lossy().into_owned());
                        AttachmentView {
                            slot: u32::try_from(i).unwrap_or(u32::MAX),
                            name: att.name.clone(),
                            mime: att.mime.clone(),
                            size: att.size,
                            thumbnail: att.thumbnail.clone(),
                            local_path: local,
                            available: f.is_none_or(|f| f.state != FileState::Unavailable),
                        }
                    })
                    .collect();
                (
                    ItemContentView::Readable {
                        title: item.title.clone(),
                        text: item.text.as_ref().map(|t| DraftText {
                            plain: t.plain.clone(),
                            html: t.html.clone(),
                            markdown: t.markdown.clone(),
                            source_format: t.source_format,
                        }),
                        url: item.url.clone(),
                        attachments,
                        secret: item.secret,
                    },
                    item.expires_at_ms,
                    item.read_policy,
                )
            }
            (ItemStatus::Unreadable(reason), _) => (
                ItemContentView::Unreadable { reason },
                None,
                ReadPolicy::UntilExpiry,
            ),
            _ => {
                return Err(CoreError::Storage {
                    reason: "readable item without local record".into(),
                })
            }
        };
        let deliveries = if row.origin == ItemOrigin::Sent {
            let receipts = db.receipts(&row.item_id)?;
            devices
                .iter()
                .filter(|d| d.device_id != a.device_id && d.state == DeviceState::Verified)
                .map(|d| {
                    let delivered = receipts
                        .iter()
                        .any(|(id, rcv, _)| *id == d.device_id && rcv.is_some());
                    let state = match row.send_status {
                        Some(SendStatus::Queued) | None => DeliveryState::Queued,
                        Some(SendStatus::Failed) => DeliveryState::Failed,
                        Some(SendStatus::ExpiredBeforeSend) => DeliveryState::ExpiredBeforeDelivery,
                        Some(SendStatus::Sent) if delivered => DeliveryState::Delivered,
                        Some(SendStatus::Sent)
                            if row.relay_expires_at_ms.is_some_and(|t| server_now > t) =>
                        {
                            DeliveryState::ExpiredBeforeDelivery
                        }
                        Some(SendStatus::Sent) => DeliveryState::Sent,
                    };
                    DeliveryView {
                        device_id: d.device_id.clone(),
                        device_name: d.name.clone(),
                        state,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let pin = c.pins.state(&row.item_id).filter(|p| p.pinned);
        // A pin wins over an archive when both are set (concurrent decisions, spec §8.9.3).
        let archive = c
            .archives
            .state(&row.item_id)
            .filter(|a| a.archived && pin.is_none());
        Ok(ItemView {
            id: row.item_id.clone(),
            content,
            source_device_id: row.sender_device_id.clone(),
            source_device_name: name_of(&row.sender_device_id),
            created_at_ms: row.created_at_ms,
            expires_at_ms,
            read_policy,
            read: row.read,
            origin: row.origin,
            send_status: row.send_status,
            deliveries,
            pinned: pin.is_some(),
            pinned_at_ms: pin.map(|p| i64::try_from(p.at_ms).unwrap_or(i64::MAX)),
            archived: archive.is_some(),
            archived_at_ms: archive.map(|a| i64::try_from(a.at_ms).unwrap_or(i64::MAX)),
        })
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceView>> {
        let me = self.device_id()?;
        self.store.read(|db| {
            Ok(db
                .devices()?
                .into_iter()
                .map(|d| DeviceView {
                    is_this_device: d.device_id == me,
                    id: d.device_id,
                    name: d.name,
                    platform: d.platform,
                    state: d.state,
                    last_seen_at_ms: d.last_seen_at_ms,
                })
                .collect())
        })
    }

    pub fn local_device(&self) -> Result<DeviceView> {
        let me = self.device_id()?;
        self.list_devices()?
            .into_iter()
            .find(|d| d.id == me)
            .ok_or_else(|| CoreError::Internal {
                reason: "local device missing".into(),
            })
    }

    pub fn space_status(&self) -> SpaceStatus {
        match self.blocked() {
            None => SpaceStatus::Active,
            Some(Blocked::NotFound) => SpaceStatus::NotFound,
            Some(Blocked::Revoked) => SpaceStatus::Revoked,
            Some(Blocked::UpdateRequired { .. }) => SpaceStatus::UpdateRequired,
            Some(other) => SpaceStatus::Blocked(format!("{other:?}")),
        }
    }

    // -----------------------------------------------------------------------
    // Sending
    // -----------------------------------------------------------------------

    /// Stores the draft in the feed as queued and in the outbox, ready to be sent by
    /// [`dispatch`](Self::dispatch) / [`sync`](Self::sync). Attachments are read and
    /// encrypted before returning; the encrypted file is both the upload and the sender's
    /// own copy at rest (no plain copy is kept, `format-v1.md` §7.2).
    /// Idempotent for a given `draft.id`.
    pub fn enqueue(&self, draft: OutgoingDraft) -> Result<String> {
        let a = self.active()?;
        let id = match &draft.id {
            Some(id) => id.clone(),
            None => new_item_id()?,
        };
        crate::crypto::parse_uuid(&id, "item id")?;
        if self.store.read(|db| db.item_exists(&id))?
            || self.store.read(|db| db.is_tombstoned(&id))?
        {
            return Ok(id);
        }
        let limits = self.limits();
        let now = self.clock.now_ms();
        let mut attachments = Vec::new();
        let mut staged: Vec<BlobUpload> = Vec::new();
        let cleanup = |staged: &[BlobUpload], store: &crate::runtime::store::Store| {
            for b in staged {
                let _ = std::fs::remove_file(store.file_path(&b.path));
            }
        };
        for (idx, d) in draft.attachments.iter().enumerate() {
            let idx = u32::try_from(idx).map_err(|_| CoreError::input("too many attachments"))?;
            match self.stage_attachment(&id, idx, d, limits.max_blob_bytes) {
                Ok((att, up)) => {
                    attachments.push(att);
                    staged.push(up);
                }
                Err(e) => {
                    cleanup(&staged, &self.store);
                    return Err(e);
                }
            }
        }
        if u64::try_from(attachments.len()).unwrap_or(u64::MAX) > limits.max_blobs_per_item {
            cleanup(&staged, &self.store);
            return Err(CoreError::input("too many attachments for one item"));
        }
        let item = Item {
            id: id.clone(),
            source_device: a.device_id.clone(),
            created_at_ms: now,
            expires_at_ms: draft.expires_at_ms,
            read_policy: draft.read_policy,
            secret: draft.secret,
            title: draft.title.clone(),
            text: draft.text.as_ref().map(|t| ItemText {
                plain: t.plain.clone(),
                html: t.html.clone(),
                markdown: t.markdown.clone(),
                source_format: t.source_format,
            }),
            url: draft.url.clone(),
            attachments,
            raw_payload: None,
        };
        let result = self.with_crypto(|c, db, a| {
            let envelope = seal_item(&item, c.ring.current()?)?;
            if u64::try_from(envelope.len()).unwrap_or(u64::MAX) > limits.max_envelope_bytes {
                return Err(CoreError::input("item too large for the relay"));
            }
            let blobs = staged
                .iter()
                .map(|b| crate::runtime::transport::BlobRef {
                    blob_id: b.blob_id.clone(),
                    size: b.size,
                })
                .collect();
            let mut row = ItemRow {
                item_id: id.clone(),
                seq: 0,
                origin: ItemOrigin::Sent,
                sender_device_id: a.device_id.clone(),
                created_at_ms: now,
                received_at_ms: now,
                relay_expires_at_ms: None,
                key_epoch: c.ring.current_epoch(),
                status: ItemStatus::Readable,
                sealed: None,
                raw_envelope: None,
                payload_expires_at_ms: None,
                secret: false,
                read_policy: "until_expiry".into(),
                read: true,
                kept: false,
                blobs,
                ring: false,
                send_status: Some(SendStatus::Queued),
            };
            crate::runtime::engine::fill_readable(&mut row, &item, a)?;
            db.put_item(&row)?;
            for (idx, (att, b)) in item.attachments.iter().zip(&staged).enumerate() {
                db.put_file(&FileRow {
                    item_id: id.clone(),
                    idx: u32::try_from(idx).map_err(|_| CoreError::input("attachments"))?,
                    blob_id: Some(att.blob_id.clone()),
                    size: att.size,
                    // The key is the one of the sealed payload.
                    key: None,
                    nonce_prefix: None,
                    state: FileState::Done,
                    path: Some(b.path.clone()),
                })?;
            }
            let mut op = NewOp::new(OpKind::SendItem);
            op.item_id = Some(id.clone());
            op.payload = Some(envelope);
            let op_id = db.enqueue_op(&op, now)?;
            for b in &staged {
                db.put_blob_upload("op", &op_id.to_string(), b)?;
            }
            Ok(())
        });
        if let Err(e) = result {
            cleanup(&staged, &self.store);
            return Err(e);
        }
        self.notify();
        Ok(id)
    }

    /// Encrypts `d.path` into the item's encrypted copy at rest, which is also the blob to
    /// upload (the plaintext is only ever in memory, one chunk at a time).
    fn stage_attachment(
        &self,
        item_id: &str,
        idx: u32,
        d: &DraftAttachment,
        max_blob_bytes: u64,
    ) -> Result<(Attachment, BlobUpload)> {
        let enc = ChunkEncryptor::new(item_id, idx)?;
        let blob_id = new_blob_id()?;
        let rel = encrypted_file_name(item_id, idx);
        let dest = self.store.file_path(&rel);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut src = std::fs::File::open(&d.path)?;
        // Refuse before encrypting: a huge file must not be written to disk first and refused
        // afterwards. The check on the real size below stays (the file can still grow).
        if encrypted_attachment_len(src.metadata()?.len()) > max_blob_bytes {
            return Err(CoreError::input("attachment larger than the relay limit"));
        }
        let mut out = std::fs::File::create(&dest)?;
        let mut buf = zeroize::Zeroizing::new(vec![0u8; CHUNK_SIZE]);
        let written: Result<crate::stream::FinalizedAttachment> = (|| {
            let mut read_total: u64 = 0;
            loop {
                let n = read_full(&mut src, &mut buf)?;
                if n == 0 {
                    break;
                }
                // The file grew since the check above: stop at once, do not encrypt it all.
                read_total += n as u64;
                if encrypted_attachment_len(read_total) > max_blob_bytes {
                    return Err(CoreError::input("attachment larger than the relay limit"));
                }
                out.write_all(&enc.update(&buf[..n])?)?;
            }
            let fin = enc.finalize()?;
            out.write_all(&fin.ciphertext)?;
            out.sync_all()?;
            Ok(fin)
        })();
        let fin = match written {
            Ok(f) => f,
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&dest);
                return Err(e);
            }
        };
        let size = fin.size;
        let ct_len = encrypted_attachment_len(size);
        if ct_len > max_blob_bytes {
            drop(out);
            let _ = std::fs::remove_file(&dest);
            return Err(CoreError::input("attachment larger than the relay limit"));
        }
        Ok((
            Attachment {
                blob_id: blob_id.clone(),
                name: d.name.clone(),
                mime: d.mime.clone(),
                size,
                file_key: fin.file_key.to_vec(),
                nonce_prefix: fin.nonce_prefix,
                thumbnail: d.thumbnail.clone(),
            },
            BlobUpload {
                blob_id,
                size: ct_len,
                path: rel,
            },
        ))
    }

    /// Tries to send one queued item now. Safe to repeat.
    pub fn dispatch(&self, item_id: &str) -> Result<DispatchOutcome> {
        for _round in 0..2 {
            {
                let _serial = self.serial(&self.dispatch_lock);
                let op = self
                    .store
                    .read(|db| db.ops_for_item(item_id))?
                    .into_iter()
                    .find(|o| o.kind == OpKind::SendItem);
                let Some(op) = op else {
                    return Ok(match self.store.read(|db| db.item(item_id))? {
                        Some(row) if row.origin == ItemOrigin::Sent => match row.send_status {
                            Some(SendStatus::Failed | SendStatus::ExpiredBeforeSend) => {
                                DispatchOutcome::Failed
                            }
                            _ => DispatchOutcome::Sent,
                        },
                        _ => DispatchOutcome::NotFound,
                    });
                };
                match self.ensure_session() {
                    Ok(()) => {}
                    Err(CoreError::Network { .. }) => return Ok(DispatchOutcome::RetryLater),
                    Err(CoreError::Relay { status, .. }) if status >= 500 || status == 429 => {
                        return Ok(DispatchOutcome::RetryLater)
                    }
                    Err(e) => return Err(e),
                }
                self.settle(&op)?;
            }
            if self
                .dirty_again
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                // Stale epoch: catch up with the log (rotation), then try once more.
                self.sync()?;
                continue;
            }
            break;
        }
        Ok(match self.store.read(|db| db.item(item_id))? {
            None => DispatchOutcome::NotFound,
            Some(row) => match row.send_status {
                Some(SendStatus::Sent) => DispatchOutcome::Sent,
                Some(SendStatus::Failed | SendStatus::ExpiredBeforeSend) => DispatchOutcome::Failed,
                _ => DispatchOutcome::RetryLater,
            },
        })
    }

    /// Cancels an item that has not left the device (the "Undo" of the send banner). Atomic
    /// with [`dispatch`](Self::dispatch): never erases an item the relay already accepted.
    pub fn cancel_if_queued(&self, item_id: &str) -> Result<bool> {
        let _serial = self.serial(&self.dispatch_lock);
        let Some(row) = self.store.read(|db| db.item(item_id))? else {
            return Ok(false);
        };
        let has_op = self
            .store
            .read(|db| db.ops_for_item(item_id))?
            .iter()
            .any(|o| o.kind == OpKind::SendItem);
        if row.send_status != Some(SendStatus::Queued) || !has_op {
            return Ok(false);
        }
        self.remove_item_locally(item_id, false)?;
        self.notify();
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Deleting and reading
    // -----------------------------------------------------------------------

    /// Removes the item from this device only (a queued item is cancelled).
    pub fn delete_local(&self, item_id: &str) -> Result<()> {
        let _serial = self.serial(&self.dispatch_lock);
        self.remove_item_locally(item_id, true)?;
        self.notify();
        Ok(())
    }

    /// Deletes the local copy, its queued operations and staged blobs, its pin and archive;
    /// remembers the id so a replayed event does not bring it back.
    fn remove_item_locally(&self, item_id: &str, release_on_relay: bool) -> Result<()> {
        let now = self.clock.now_ms();
        let (files, staged) = self.with_crypto(|c, db, _a| {
            let row = db.item(item_id)?;
            let mut staged = Vec::new();
            for op in db.ops_for_item(item_id)? {
                staged.extend(db.blob_uploads("op", &op.op_id.to_string())?);
                db.delete_blob_uploads("op", &op.op_id.to_string())?;
            }
            db.delete_ops_for_item(item_id)?;
            let files = db.delete_item(item_id)?;
            db.put_tombstone(item_id, now, false, now)?;
            c.pins.forget(item_id);
            db.delete_pin(item_id)?;
            c.archives.forget(item_id);
            db.delete_archive(item_id)?;
            if let Some(row) = row {
                if release_on_relay && row.origin == ItemOrigin::Received {
                    let mut op = NewOp::new(OpKind::Ack);
                    op.item_id = Some(item_id.to_owned());
                    op.flag = true;
                    db.enqueue_op(&op, now)?;
                }
            }
            Ok((files, staged))
        })?;
        for p in files {
            let _ = std::fs::remove_file(self.store.file_path(&p));
        }
        for b in staged {
            let _ = self.transport.delete_blob(&b.blob_id);
            let _ = std::fs::remove_file(self.store.file_path(&b.path));
        }
        self.store.truncate_wal_if_due(true);
        Ok(())
    }

    /// Deletes everywhere: signed tombstone, local erasure now, relay purge and erasure on
    /// the other devices when the operation reaches the relay (replayed until it does).
    pub fn delete_everywhere(&self, item_id: &str) -> Result<()> {
        let _serial = self.serial(&self.dispatch_lock);
        let queued = self
            .store
            .read(|db| db.item(item_id))?
            .is_some_and(|r| r.send_status == Some(SendStatus::Queued));
        if queued {
            // Never left the device: a local removal is enough.
            self.remove_item_locally(item_id, false)?;
            self.notify();
            return Ok(());
        }
        let now = self.clock.now_ms();
        let staged = self.with_crypto(|c, db, a| {
            let t = a.keys.sign_tombstone(
                &a.space_id,
                item_id,
                u64::try_from(now).unwrap_or(0),
                crate::DeletionScope::Everywhere,
            )?;
            let files = Self::erase_item_everywhere(c, db, item_id, now)?;
            let mut op = NewOp::new(OpKind::Tombstone);
            op.item_id = Some(item_id.to_owned());
            op.payload = Some(t.to_bytes()?);
            db.enqueue_op(&op, now)?;
            Ok(files)
        })?;
        for p in staged {
            let _ = std::fs::remove_file(self.store.file_path(&p));
        }
        self.store.truncate_wal_if_due(true);
        self.notify();
        Ok(())
    }

    /// Marks the item read on all devices; consumes an `after_first_read` item (erased
    /// everywhere). An imported item is only marked read here: it never sends an ack, a
    /// read receipt or a release (`api-v1.md` §7.8). Does nothing for an unreadable item or
    /// an item sent from here.
    pub fn mark_read(&self, item_id: &str) -> Result<()> {
        // The identity this call is made for: the writes below land only if it is still the
        // live one (no write after an erasure, `api-v1.md` §7.9.6).
        let a = self.active()?;
        let Some(row) = self.store.read(|db| db.item(item_id))? else {
            return Ok(());
        };
        if row.status != ItemStatus::Readable || row.origin == ItemOrigin::Sent {
            return Ok(());
        }
        if row.read_policy == policy_str(ReadPolicy::AfterFirstRead) {
            return self.delete_everywhere(item_id);
        }
        if row.read {
            return Ok(());
        }
        if row.origin == ItemOrigin::Imported {
            self.live_tx(&a, |db| db.set_item_read(item_id, true))?;
            self.notify();
            return Ok(());
        }
        let now = self.clock.now_ms();
        self.live_tx(&a, |db| {
            db.set_item_read(item_id, true)?;
            let mut op = NewOp::new(OpKind::Read);
            op.item_id = Some(item_id.to_owned());
            db.enqueue_op(&op, now)?;
            Ok(())
        })?;
        self.notify();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Pin (spec/pin-contract.md)
    // -----------------------------------------------------------------------

    /// Pins the item on all devices. Idempotent; unknown id: no effect; immediate local
    /// effect; the decision leaves through the outbox, so the network never fails it.
    /// Errors (`spec/pin-contract.md` §2): storage, `InvalidInput` (device not
    /// provisioned) and the refusal to sign (`malformed_statement` for a time beyond
    /// 2^53 - 1 ms, this device no longer a trusted signer).
    pub fn pin(&self, item_id: &str) -> Result<()> {
        self.set_pinned(item_id, true)
    }

    pub fn unpin(&self, item_id: &str) -> Result<()> {
        self.set_pinned(item_id, false)
    }

    fn set_pinned(&self, item_id: &str, pinned: bool) -> Result<()> {
        let now = self.clock.now_ms();
        let changed = self.with_crypto(|c, db, a| {
            if !db.item_exists(item_id)? {
                return Ok(false);
            }
            // Pin and archive are exclusive: pinning an archived item unarchives it first
            // (`spec/archive-contract.md` §2). Both decisions commit together.
            let mut changed = false;
            if pinned {
                changed |= Self::queue_archive(c, db, a, item_id, false, now)?;
            }
            changed |= Self::queue_pin(c, db, a, item_id, pinned, now)?;
            Ok(changed)
        })?;
        if changed {
            self.notify();
        }
        Ok(())
    }

    /// Signs a pin decision, applies it locally and queues it. `false` if already in state.
    fn queue_pin(
        c: &mut Crypto,
        db: &crate::runtime::store::Db<'_>,
        a: &Active,
        item_id: &str,
        pinned: bool,
        now: i64,
    ) -> Result<bool> {
        let out = c.pins.set_pinned(
            &a.keys,
            &c.trust,
            c.ring.current()?,
            item_id,
            pinned,
            u64::try_from(now).unwrap_or(0),
        )?;
        let Some(out) = out else {
            return Ok(false);
        };
        if let Some(s) = c.pins.state(item_id) {
            db.put_pin(item_id, s)?;
        }
        Self::enqueue_item_state(db, item_id, &out, now)?;
        Ok(true)
    }

    fn enqueue_item_state(
        db: &crate::runtime::store::Db<'_>,
        item_id: &str,
        out: &crate::OutgoingItemState,
        now: i64,
    ) -> Result<()> {
        let mut op = NewOp::new(OpKind::ItemState);
        op.item_id = Some(item_id.to_owned());
        op.event_id = Some(out.event_id.clone());
        op.payload = Some(out.envelope.clone());
        op.aux = Some(crate::ItemStateWire::wire_bytes(&out.statement)?);
        db.enqueue_op(&op, now)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Archive (spec/archive-contract.md)
    // -----------------------------------------------------------------------

    /// Archives the item on all devices: it leaves "Recents" and stays readable in
    /// "Archives". Idempotent; unknown id: no effect; immediate local effect; the decision
    /// leaves through the outbox, so the network never fails it.
    ///
    /// Archiving **never** marks the item read, never queues an ack / read / release and
    /// downloads nothing (so a "read once" secret is never consumed by it), and changes
    /// neither the relay purge nor an expiration. A pinned item is unpinned first (an
    /// unpin, then the archive; one local transaction). Errors as [`pin`](Self::pin).
    pub fn archive(&self, item_id: &str) -> Result<()> {
        self.set_archived(item_id, true)
    }

    pub fn unarchive(&self, item_id: &str) -> Result<()> {
        self.set_archived(item_id, false)
    }

    fn set_archived(&self, item_id: &str, archived: bool) -> Result<()> {
        let now = self.clock.now_ms();
        let changed = self.with_crypto(|c, db, a| {
            if !db.item_exists(item_id)? {
                return Ok(false);
            }
            let mut changed = false;
            if archived {
                changed |= Self::queue_pin(c, db, a, item_id, false, now)?;
            }
            changed |= Self::queue_archive(c, db, a, item_id, archived, now)?;
            Ok(changed)
        })?;
        if changed {
            self.notify();
        }
        Ok(())
    }

    /// Signs an archive decision, applies it locally and queues it. `false` if already in
    /// state.
    fn queue_archive(
        c: &mut Crypto,
        db: &crate::runtime::store::Db<'_>,
        a: &Active,
        item_id: &str,
        archived: bool,
        now: i64,
    ) -> Result<bool> {
        let out = c.archives.set_archived(
            &a.keys,
            &c.trust,
            c.ring.current()?,
            item_id,
            archived,
            u64::try_from(now).unwrap_or(0),
        )?;
        let Some(out) = out else {
            return Ok(false);
        };
        if let Some(s) = c.archives.state(item_id) {
            db.put_archive(item_id, s)?;
        }
        Self::enqueue_item_state(db, item_id, &out, now)?;
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Devices
    // -----------------------------------------------------------------------

    /// Renames a device on the relay and locally (needs the network).
    pub fn rename_device(&self, device_id: &str, name: &str) -> Result<()> {
        let a = self.active()?;
        self.ensure_session()?;
        self.authed(|| self.transport.rename_device(device_id, name))?;
        // Erased while the request was in flight: nothing is written into the emptied store.
        self.live_tx(&a, |db| db.set_device_name(device_id, name))?;
        self.notify();
        Ok(())
    }

    /// Registers the push token of this device with the relay (`PUT /v1/me/push`), so that it is
    /// woken when something arrives while no doorbell is open. `provider` is the push service
    /// (`"fcm"`, `"apns"`, `"apns-sandbox"`). Needs the network and a space; nothing is kept
    /// locally (the app gives the token again when it changes, and at each start).
    pub fn register_push(&self, provider: &str, token: &str) -> Result<()> {
        self.active()?;
        self.ensure_session()?;
        self.authed(|| self.transport.register_push(provider, token))?;
        Ok(())
    }

    /// Takes the push registration of this device back (`DELETE /v1/me/push`).
    pub fn unregister_push(&self) -> Result<()> {
        self.active()?;
        self.ensure_session()?;
        self.authed(|| self.transport.unregister_push())?;
        Ok(())
    }

    /// Revokes `target` and rotates the space key (`api-v1.md` §7.6). Needs the network.
    pub fn revoke_device(&self, target: &str) -> Result<()> {
        let _rotating = RotatingGuard::take(self);
        self.rotate_revoking(Some(target)).map(|_| ())
    }

    /// Rotation after a revocation by the relay (`self` / `admin`), if still required. The
    /// engine runs it by itself when it is due ([`sync`](Self::sync), doorbell loop); kept for
    /// the apps that still call it.
    pub fn maybe_rotate(&self) -> Result<bool> {
        let _rotating = RotatingGuard::take(self);
        self.maybe_rotate_now()
    }

    fn maybe_rotate_now(&self) -> Result<bool> {
        self.ensure_session()?;
        let space = self.authed(|| self.transport.space())?;
        if !space.rotation_required {
            // Another member did it.
            self.rotation_settled();
            return Ok(false);
        }
        // `false`: another member rotated first while this one was trying; nothing is left to do.
        let posted = self.rotate_revoking(None)?;
        self.rotation_settled();
        Ok(posted)
    }

    /// No rotation is required any more: the flag and the due date are cleared.
    fn rotation_settled(&self) {
        if let Ok(mut f) = lock(&self.flags) {
            f.rotation_required = false;
            f.rotation_due_at_ms = None;
            f.rotation_failures = 0;
        }
        self.set_rotation_stalled(false);
    }

    /// The automatic rotation (`api-v1.md` §7.6), when it is due: called at the end of every
    /// [`sync`](Self::sync) and from the doorbell loop. Nothing on a device that is not
    /// provisioned, blocked (revoked, not found, update required), with a departure pending,
    /// or when a rotation already runs.
    pub(crate) fn run_due_rotation(&self) {
        let now = self.clock.now_ms();
        let due = match lock(&self.flags) {
            Ok(f) => {
                f.rotation_required
                    && f.blocked.is_none()
                    && f.rotation_due_at_ms.is_some_and(|t| now > t)
            }
            Err(_) => false,
        };
        if !due || !self.is_provisioned() || self.departure_pending() {
            return;
        }
        let Some(_rotating) = RotatingGuard::try_take(self) else {
            return;
        };
        match self.maybe_rotate_now() {
            Ok(_) => {}
            Err(CoreError::Relay { code, .. }) if code == "device_set_mismatch" => {
                // A member the trust store does not know: never sealed for (§7.6). Try again in
                // an hour.
                // The alert of §7.6: a persisted flag (`rotation_stalled`, cleared by the next
                // rotation that succeeds or that another member applies) the apps show as a
                // banner, since until then the revoked device may still hold the space key.
                self.set_rotation_stalled(true);
                if let Ok(mut f) = lock(&self.flags) {
                    f.rotation_due_at_ms = Some(now.saturating_add(3_600_000));
                }
                self.notify();
            }
            Err(_) => {
                if let Ok(mut f) = lock(&self.flags) {
                    f.rotation_failures = f.rotation_failures.saturating_add(1);
                    let ceiling = (30_000i64 << f.rotation_failures.min(5)).min(900_000);
                    f.rotation_due_at_ms = Some(now.saturating_add(jitter_i64(ceiling)));
                }
            }
        }
    }

    /// Posts a rotation that revokes `target` (with `None`: the members the relay revoked
    /// itself), `api-v1.md` §7.6. `Ok(true)`: this call posted it. `Ok(false)`: after an
    /// `epoch_conflict` or a `device_set_mismatch` another member had done it meanwhile, so
    /// nothing is posted again (a second rotation would only change the key for nothing).
    fn rotate_revoking(&self, target: Option<&str>) -> Result<bool> {
        // A device that is leaving rotates nothing (§7.9).
        if self.departure_pending() {
            return Err(CoreError::input("a departure is pending"));
        }
        for attempt in 0..2 {
            if self.sync()?.offline {
                return Err(CoreError::Network {
                    reason: "offline".into(),
                });
            }
            if attempt > 0 && target.is_none() {
                // The first try failed because the relay moved on after our last look: ask
                // again whether a rotation is still owed. The synchronisation above turned the
                // devices the other member revoked into `Revoked`, so the list of targets alone
                // would be empty and post a useless second rotation.
                let space = self.authed(|| self.transport.space())?;
                if !space.rotation_required {
                    return Ok(false);
                }
            }
            let now = u64::try_from(self.clock.now_ms()).unwrap_or(0);
            let bundle = self.read_crypto(|c, a| {
                let members = c.trust.members();
                let relay_revoked: Vec<String> = self.store.read(|db| {
                    Ok(db
                        .devices()?
                        .into_iter()
                        .filter(|d| d.state == DeviceState::RevokedByRelay)
                        .map(|d| d.device_id)
                        .collect())
                })?;
                let revoke: Vec<Vec<u8>> = members
                    .iter()
                    .filter(|m: &&DevicePublic| {
                        m.device_id().is_ok_and(|id| {
                            Some(id.as_str()) == target || relay_revoked.contains(&id)
                        })
                    })
                    .map(|m| m.ed25519.clone())
                    .collect();
                if let Some(t) = target {
                    if revoke.is_empty() {
                        if attempt > 0 {
                            // Revoked by the rotation that won the conflict: done.
                            return Ok(None);
                        }
                        return Err(CoreError::input(format!("unknown device {t}")));
                    }
                }
                c.trust.create_rotation(&a.keys, &revoke, now).map(Some)
            })?;
            let Some(bundle) = bundle else {
                return Ok(false);
            };
            let bytes = bundle.rotation.to_bytes()?;
            // The position the relay answers is its own word, and so is the position at which it
            // later serves the event: neither bounds what an honest old-epoch item can be. What
            // this device has seen before posting does: the event is numbered above it. Items of
            // the old epoch above it are judged suspect, which can only hide an item that another
            // device sealed in the round trip of this very post.
            let cursor_at_post = self.store.read(|db| db.cursor())?;
            match self.authed(|| self.transport.post_rotation(&bytes)) {
                Ok(_answered_seq) => {
                    self.with_crypto(|c, db, a| {
                        let out = c.trust.apply_rotation(&bundle.rotation, &a.keys)?;
                        if let Some(key) = out.new_space_key {
                            c.ring.add_rotation_key(key, cursor_at_post, now)?;
                        }
                        for d in db.devices()? {
                            if d.ed25519.as_ref().is_some_and(|k| c.trust.is_revoked(k)) {
                                db.set_device_state(&d.device_id, DeviceState::Revoked)?;
                            }
                        }
                        Self::persist_crypto(c, db)
                    })?;
                    self.set_rotation_stalled(false);
                    self.notify();
                    return Ok(true);
                }
                Err(RelayError::Api(p))
                    if (p.code == "epoch_conflict" || p.code == "device_set_mismatch")
                        && attempt == 0 => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(CoreError::Relay {
            status: 409,
            code: "device_set_mismatch".into(),
        })
    }

    // -----------------------------------------------------------------------
    // Files (`format-v1.md` §7.2: encrypted at rest, decrypted on demand)
    // -----------------------------------------------------------------------

    /// Absolute path of the decrypted attachment. The local encrypted copy is decrypted into
    /// the cache (`files/cache/`), after downloading it if needed. `None` if the file is not
    /// available (not transferred, purged by the relay). The decrypted copy lives until the
    /// item goes away or [`purge_plaintext_cache`](Self::purge_plaintext_cache).
    pub fn attachment_path(&self, item_id: &str, index: u32) -> Result<Option<PathBuf>> {
        // `index` is the `slot` of the attachment (its position in the payload, stable).

        let a = self.active()?;
        let Some(row) = self.store.read(|db| db.item(item_id))? else {
            return Ok(None);
        };
        let Some(sealed) = row.sealed.as_ref() else {
            return Ok(None);
        };
        let Some(file) = self
            .store
            .read(|db| db.files(item_id))?
            .into_iter()
            .find(|f| f.idx == index)
        else {
            return Ok(None);
        };
        if file.state == FileState::Unavailable {
            return Ok(None);
        }
        let item = a.lsk.open_item(sealed, item_id)?;
        let base = item
            .attachments
            .get(index as usize)
            .ok_or_else(|| CoreError::input("no such attachment"))?;
        let base_blob = base.blob_id.clone();
        let att = local_attachment(base, &file);
        if file.state == FileState::Done {
            if let Some(p) = &file.path {
                let enc = self.store.file_path(p);
                if enc.exists() {
                    let path = self.decrypt_to_cache(item_id, index, &att, &enc)?;
                    return Ok(self.unless_removed(item_id, index, &base_blob, path));
                }
            }
        }
        match self.fetch_remote_file(&a, &row, &file, &att, &base_blob)? {
            Some(rel) => {
                let path = self.decrypt_to_cache(item_id, index, &att, &self.store.file_path(&rel));
                self.notify();
                path.map(|p| self.unless_removed(item_id, index, &base_blob, p))
            }
            None => Ok(None),
        }
    }

    /// Downloads the encrypted copy of a file that is not on this device yet (`Remote`, or `Done`
    /// with its copy gone) and records it as `Done`. Returns the relative path of the copy, or
    /// `None` when the file is not available: purged by the relay (the row becomes `Unavailable`)
    /// or removed meanwhile (its row stays gone). Network errors are returned, nothing is recorded.
    fn fetch_remote_file(
        &self,
        a: &Arc<Active>,
        row: &ItemRow,
        file: &FileRow,
        att: &Attachment,
        base_blob: &str,
    ) -> Result<Option<String>> {
        let item_id = row.item_id.as_str();
        let index = file.idx;
        self.ensure_session()?;
        match self.download_encrypted(item_id, index, att) {
            Ok(rel) => {
                // Erased while downloading: nothing is written into the emptied store.
                // Removed while downloading: neither is the file, and its row stays gone.
                let release = self.live_tx(a, |db| {
                    if db
                        .attachment_removal(item_id, index)?
                        .is_some_and(|r| r.blob_id == base_blob)
                    {
                        // The slot must not exist here: no stale `Remote` row either.
                        db.delete_file(item_id, index)?;
                        return Ok(None);
                    }
                    db.put_file(&FileRow {
                        state: FileState::Done,
                        path: Some(rel.clone()),
                        ..file.clone()
                    })?;
                    let all = db.files(item_id)?;
                    Ok(Some(
                        row.origin == ItemOrigin::Received
                            && all.iter().all(|f| f.state != FileState::Remote),
                    ))
                })?;
                let Some(release) = release else {
                    let _ = std::fs::remove_file(self.store.file_path(&rel));
                    self.notify();
                    return Ok(None);
                };
                if release {
                    // Everything wanted is here: the relay may purge (`release`).
                    self.live_tx(a, |db| {
                        let mut op = NewOp::new(OpKind::Ack);
                        op.item_id = Some(item_id.to_owned());
                        op.flag = true;
                        db.enqueue_op(&op, self.clock.now_ms()).map(|_| ())
                    })?;
                }
                Ok(Some(rel))
            }
            Err(StepErr::Relay(RelayError::Api(p))) if p.status == 404 => {
                self.live_tx(a, |db| {
                    db.put_file(&FileRow {
                        state: FileState::Unavailable,
                        ..file.clone()
                    })
                })?;
                self.notify();
                Ok(None)
            }
            Err(StepErr::Relay(e)) => Err(e.into()),
            Err(StepErr::Local(e)) => Err(e),
        }
    }

    /// The local encrypted copy of a file of an item, for the export of the history: the copy
    /// on this device, or — when the file was never opened here and is still on the relay — the
    /// one downloaded now, so that the new device does not get it as "unavailable". `None` when
    /// the file is not available anywhere. Never decrypts into the cache.
    pub(crate) fn local_encrypted_copy(
        &self,
        row: &ItemRow,
        item: &Item,
        idx: u32,
    ) -> Result<Option<(FileRow, PathBuf)>> {
        let Some(file) = self
            .store
            .read(|db| db.files(&row.item_id))?
            .into_iter()
            .find(|f| f.idx == idx)
        else {
            return Ok(None);
        };
        if file.state == FileState::Unavailable {
            return Ok(None);
        }
        if file.state == FileState::Done {
            if let Some(p) = &file.path {
                let path = self.store.file_path(p);
                if path.exists() {
                    return Ok(Some((file, path)));
                }
            }
        }
        let Some(base) = item.attachments.get(idx as usize) else {
            return Ok(None);
        };
        let a = self.active()?;
        let att = local_attachment(base, &file);
        let Some(rel) = self.fetch_remote_file(&a, row, &file, &att, &base.blob_id)? else {
            return Ok(None);
        };
        let done = FileRow {
            state: FileState::Done,
            path: Some(rel.clone()),
            ..file
        };
        Ok(Some((done, self.store.file_path(&rel))))
    }

    /// `Some(path)`, unless the attachment was removed meanwhile (a concurrent removal): the
    /// decrypted copy just made is then erased too.
    fn unless_removed(
        &self,
        item_id: &str,
        index: u32,
        blob_id: &str,
        path: PathBuf,
    ) -> Option<PathBuf> {
        let removed = self
            .store
            .read(|db| db.attachment_removal(item_id, index))
            .ok()
            .flatten()
            .is_some_and(|r| r.blob_id == blob_id);
        if removed {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(path)
    }

    /// Erases every decrypted attachment (`files/cache/`). The app calls it when it goes to
    /// the background (Android `onStop`, window hidden on Windows); the engine also does at
    /// start. The encrypted copies stay: [`attachment_path`](Self::attachment_path)
    /// decrypts again on demand. Fails if a file could not be removed (still open by another
    /// program on Windows); everything else is removed anyway.
    pub fn purge_plaintext_cache(&self) -> Result<()> {
        let dir = self.store.file_path(CACHE_DIR);
        let mut left = 0u32;
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    let removed = if path.is_dir() {
                        std::fs::remove_dir_all(&path)
                    } else {
                        std::fs::remove_file(&path)
                    };
                    if removed.is_err() {
                        left += 1;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        std::fs::create_dir_all(&dir)?;
        // `local_path` of the views changed.
        self.notify();
        if left > 0 {
            return Err(CoreError::Storage {
                reason: format!("{left} decrypted file(s) still in use"),
            });
        }
        Ok(())
    }

    /// Plain files kept at rest by older builds (`files/{item}_{idx}`, no `.enc`): erased,
    /// and marked to be fetched again.
    pub(crate) fn sweep_legacy_plaintext(&self) -> Result<()> {
        let legacy: Vec<FileRow> = self
            .store
            .read(|db| db.done_files())?
            .into_iter()
            .filter(|f| f.path.as_deref().is_some_and(|p| !p.ends_with(".enc")))
            .collect();
        for f in legacy {
            if let Some(p) = &f.path {
                let _ = std::fs::remove_file(self.store.file_path(p));
            }
            self.store.tx(|db| {
                db.put_file(&FileRow {
                    state: FileState::Remote,
                    path: None,
                    ..f.clone()
                })
            })?;
        }
        Ok(())
    }

    /// Downloads `total` bytes of a blob into `staging`, resuming from what the file already
    /// holds (`Range`, `api-v1.md` §4.9).
    pub(crate) fn fetch_blob(
        &self,
        blob_id: &str,
        total: u64,
        staging: &Path,
    ) -> std::result::Result<(), StepErr> {
        if let Some(dir) = staging.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = self.limits().max_upload_part_bytes.max(1);
        let mut have = std::fs::metadata(staging).map(|m| m.len()).unwrap_or(0);
        if have > total {
            std::fs::remove_file(staging)?;
            have = 0;
        }
        if have < total {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(staging)?;
            while have < total {
                let len = part.min(total - have);
                let data = self.authed(|| self.transport.get_blob_range(blob_id, have, len))?;
                if data.is_empty() {
                    return Err(RelayError::Network("empty range".into()).into());
                }
                // More than asked: the relay or a proxy ignored `Range`. Nothing of this
                // answer is written, so the staged bytes stay valid for the resume.
                if u64::try_from(data.len()).unwrap_or(u64::MAX) > len {
                    return Err(
                        RelayError::Network("range answer longer than requested".into()).into(),
                    );
                }
                f.write_all(&data)?;
                have += u64::try_from(data.len()).unwrap_or(0);
            }
            f.sync_all()?;
        }
        Ok(())
    }

    /// Downloads the blob of `att` (resumable, `Range`), authenticates it whole, and keeps it
    /// *encrypted* as the item's copy at rest. Returns its path relative to the files
    /// directory. Used for relay items and for imported transfers.
    pub(crate) fn download_encrypted(
        &self,
        item_id: &str,
        index: u32,
        att: &Attachment,
    ) -> std::result::Result<String, StepErr> {
        let total = encrypted_attachment_len(att.size);
        let staging = self.store.file_path(&format!("dl/{}.part", att.blob_id));
        self.fetch_blob(&att.blob_id, total, &staging)?;
        if let Err(e) = decrypt_file(&staging, None, item_id, index, att) {
            // Tampered or truncated: do not keep the bytes.
            let _ = std::fs::remove_file(&staging);
            return Err(e.into());
        }
        let rel = encrypted_file_name(item_id, index);
        std::fs::rename(&staging, self.store.file_path(&rel))?;
        Ok(rel)
    }

    /// Decrypts the encrypted copy `enc` into the cache, unless already there, and returns
    /// the path of the decrypted file. Written to a temporary name, exposed only once
    /// complete and authentic (`format-v1.md` §6.5).
    fn decrypt_to_cache(
        &self,
        item_id: &str,
        index: u32,
        att: &Attachment,
        enc: &Path,
    ) -> Result<PathBuf> {
        let dest = self.store.file_path(&cache_file_name(item_id, index));
        if dest.exists() {
            return Ok(dest);
        }
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tag = hex::encode(crate::crypto::random_array::<8>()?);
        let tmp = dest.with_file_name(format!("{item_id}_{index}.{tag}.tmp"));
        let written = std::fs::File::create(&tmp)
            .map_err(CoreError::from)
            .and_then(|mut out| {
                decrypt_file(enc, Some(&mut out), item_id, index, att)?;
                out.sync_all()?;
                Ok(())
            });
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::remove_file(&tmp);
            // Another call made it meanwhile.
            if !dest.exists() {
                return Err(e.into());
            }
        }
        // The device left the space while the copy was being written: the erasure may have
        // passed before this file existed, so it must not survive.
        if !self.is_provisioned() {
            let _ = std::fs::remove_file(&dest);
            return Err(CoreError::input("device is not provisioned"));
        }
        Ok(dest)
    }
}

/// Marks a rotation as running (`Engine::rotating`) for its lifetime, so that the automatic
/// rotation never starts a second one from inside a revocation's `sync`.
struct RotatingGuard<'a> {
    engine: &'a Engine,
    owned: bool,
}

impl<'a> RotatingGuard<'a> {
    /// `None` if a rotation already runs.
    fn try_take(engine: &'a Engine) -> Option<Self> {
        engine
            .rotating
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .ok()
            .map(|_| Self {
                engine,
                owned: true,
            })
    }

    /// For a gesture of the user: it goes on even if an automatic rotation runs (the relay
    /// orders the two with `epoch_conflict`).
    fn take(engine: &'a Engine) -> Self {
        Self::try_take(engine).unwrap_or(Self {
            engine,
            owned: false,
        })
    }
}

impl Drop for RotatingGuard<'_> {
    fn drop(&mut self) {
        if self.owned {
            self.engine
                .rotating
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// What opens the local encrypted copy: the transfer key `FK'` (and its blob) of an imported
/// item, else the attachment of the payload (`FK`).
pub(crate) fn local_attachment(base: &Attachment, file: &FileRow) -> Attachment {
    match (&file.key, &file.nonce_prefix, &file.blob_id) {
        (Some(k), Some(p), Some(b)) => Attachment {
            blob_id: b.clone(),
            file_key: k.clone(),
            nonce_prefix: p.clone(),
            ..base.clone()
        },
        _ => base.clone(),
    }
}

/// Decrypts `src` (one encrypted attachment) into `out`, or only authenticates it when `out`
/// is `None`. Fails unless the whole file is authentic and complete.
pub(crate) fn decrypt_file(
    src: &Path,
    mut out: Option<&mut std::fs::File>,
    item_id: &str,
    index: u32,
    att: &Attachment,
) -> Result<()> {
    let dec = ChunkDecryptor::new(item_id, index, att)?;
    let mut input = std::fs::File::open(src)?;
    let mut buf = vec![0u8; CHUNK_CIPHERTEXT_SIZE];
    loop {
        let n = read_full(&mut input, &mut buf)?;
        if n == 0 {
            break;
        }
        let plain = zeroize::Zeroizing::new(dec.update(&buf[..n])?);
        if let Some(o) = out.as_deref_mut() {
            o.write_all(&plain)?;
        }
    }
    dec.finalize()
}

/// Reads until `buf` is full or the end of the file.
pub(crate) fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

#[cfg(test)]
mod debug_tests {
    use super::*;

    const NEEDLES: [&str; 8] = [
        "SECRET-TITLE",
        "SECRET-TEXT",
        "SECRET-HTML",
        "SECRET-MD",
        "https://secret.example",
        "SECRET-FILE.pdf",
        "/secret/path",
        // The thumbnail bytes of b"SECRET-TEXT" as a `Vec<u8>` Debug would print them.
        "83, 69, 67, 82, 69, 84",
    ];

    fn assert_clean(dbg: &str) {
        for n in NEEDLES {
            assert!(!dbg.contains(n), "Debug leaks {n:?}: {dbg}");
        }
    }

    fn text() -> DraftText {
        DraftText {
            plain: "SECRET-TEXT".into(),
            html: Some("SECRET-HTML".into()),
            markdown: Some("SECRET-MD".into()),
            source_format: SourceFormat::Plain,
        }
    }

    #[test]
    fn debug_of_content_views_and_drafts_never_prints_content() {
        let attachment = AttachmentView {
            slot: 0,
            name: "SECRET-FILE.pdf".into(),
            mime: "application/pdf".into(),
            size: 12,
            thumbnail: Some(Thumbnail {
                mime: "image/png".into(),
                data: b"SECRET-TEXT".to_vec(),
            }),
            local_path: Some("/secret/path".into()),
            available: true,
        };
        let content = ItemContentView::Readable {
            title: Some("SECRET-TITLE".into()),
            text: Some(text()),
            url: Some("https://secret.example".into()),
            attachments: vec![attachment.clone()],
            secret: true,
        };
        let view = ItemView {
            id: "item-1".into(),
            content: content.clone(),
            source_device_id: "dev-1".into(),
            source_device_name: "Phone".into(),
            created_at_ms: 1,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            read: false,
            origin: ItemOrigin::Received,
            send_status: None,
            deliveries: vec![],
            pinned: false,
            pinned_at_ms: None,
            archived: false,
            archived_at_ms: None,
        };
        let draft = OutgoingDraft {
            id: None,
            title: Some("SECRET-TITLE".into()),
            text: Some(text()),
            url: Some("https://secret.example".into()),
            attachments: vec![DraftAttachment {
                path: "/secret/path".into(),
                name: "SECRET-FILE.pdf".into(),
                mime: "application/pdf".into(),
                thumbnail: None,
            }],
            secret: true,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
        };
        for dbg in [
            format!("{view:?}"),
            format!("{view:#?}"),
            format!("{content:?}"),
            format!("{attachment:?}"),
            format!("{:?}", text()),
            format!("{draft:?}"),
            format!("{:?}", draft.attachments[0]),
        ] {
            assert_clean(&dbg);
        }
        // The non-sensitive fields stay useful.
        let dbg = format!("{view:?}");
        assert!(
            dbg.contains("item-1") && dbg.contains("secret: true"),
            "{dbg}"
        );
    }
}
