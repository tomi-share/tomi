//! History transfer tasks: export and import (`api-v1.md` §7.8, `format-v1.md` §8.7).
//!
//! Both sides are persistent state machines in the `transfers` table, advanced by
//! [`Engine::step_transfers`] (called at the end of every sync and after a user action),
//! so that a crash, a cut connection or a restart resumes where it stopped:
//!
//! ```text
//! export:  building -> uploading -> posted -> done | failed | cancelled
//! import:  received -> (accepted) -> verifying -> downloading -> importing -> files
//!          -> finishing -> done | failed | declined
//! ```
//!
//! * `building` is restartable from scratch (nothing is on the relay yet): the index is
//!   written to a temporary file encrypted with a throw-away key, cut into encrypted
//!   parts, and every attached file present locally is re-encrypted under a fresh key.
//! * `uploading` resumes every blob from the relay's `upload_offset`.
//! * `downloading` resumes every part with `Range`; `importing` re-reads the parts from
//!   the start and is idempotent (an item already present is skipped, its pin and archive merged).

use std::io::Write;

use zeroize::Zeroizing;

use crate::crypto::{new_blob_id, new_uuid};
use crate::error::{CoreError, Result};
use crate::format::CHUNK_CIPHERTEXT_SIZE;
use crate::history::{
    HistoryDevice, HistoryEntry, HistoryHeader, HistoryIndexReader, HistoryIndexWriter,
    HistoryItem, HistoryPart, HistoryPartDecryptor, HistoryPartEncryptor, HistoryTransfer,
    TransferredFile,
};
use crate::item::Attachment;
use crate::runtime::actions::{local_attachment, read_full};
use crate::runtime::engine::{fill_readable, is_final_state, Engine};
use crate::runtime::outbox::StepErr;
use crate::runtime::store::{
    BlobUpload, Db, DeviceRow, DeviceState, FileRow, FileState, ItemOrigin, ItemRow, ItemStatus,
    TransferRow,
};
use crate::runtime::transport::{PostTransfer, RelayError};
use crate::stream::{ChunkDecryptor, ChunkEncryptor};
use crate::{
    encrypted_attachment_len, history_part_capacity, history_part_count, HistoryKey,
    UnreadableReason,
};

/// A transfer task as the apps show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferView {
    pub transfer_id: String,
    /// `export` or `import`.
    pub role: String,
    pub peer_device_id: String,
    pub peer_name: String,
    pub state: String,
    pub items_total: u64,
    pub items_done: u64,
    pub files_total: u64,
    pub files_done: u64,
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub finished: bool,
}

const TEMP_INDEX_ATTACHMENT: u32 = u32::MAX;

/// Records the export of the whole feed to `to_device_id` under `transfer_id`, as a task in the
/// `building` state. The recipient must be a verified member and the log must have caught up
/// with its arrival (`cursor >= joined_seq`): the snapshot is taken at the cursor.
pub(crate) fn new_export_row(
    db: &Db<'_>,
    to_device_id: &str,
    transfer_id: &str,
    now_ms: i64,
) -> Result<()> {
    let peer = db
        .device(to_device_id)?
        .filter(|d| d.state == DeviceState::Verified)
        .ok_or_else(|| CoreError::input("recipient is not a verified member"))?;
    if peer.joined_seq > db.cursor()? {
        return Err(CoreError::input(
            "not synchronised up to the recipient's arrival",
        ));
    }
    let mut t = TransferRow::new(transfer_id, "export", to_device_id, "building", now_ms);
    t.as_of_seq = db.cursor()?;
    db.put_transfer(&t)
}

impl Engine {
    /// Tasks, oldest first.
    pub fn transfers(&self) -> Result<Vec<TransferView>> {
        self.store.read(|db| {
            let devices = db.devices()?;
            Ok(db
                .transfers()?
                .into_iter()
                .map(|t| TransferView {
                    peer_name: devices
                        .iter()
                        .find(|d| d.device_id == t.peer_device_id)
                        .map_or_else(|| "Appareil".into(), |d| d.name.clone()),
                    finished: is_final_state(&t.state),
                    transfer_id: t.transfer_id,
                    role: t.role,
                    peer_device_id: t.peer_device_id,
                    state: t.state,
                    items_total: t.items_total,
                    items_done: t.items_done,
                    files_total: t.files_total,
                    files_done: t.files_done,
                    outcome: t.outcome,
                    error: t.error,
                })
                .collect())
        })
    }

    /// Sends the whole feed (secrets and files included) to `to_device_id`, a verified
    /// member. Synchronises first (`cursor >= joined_seq` of the recipient). Returns the
    /// transfer id; progress and completion are visible through [`transfers`](Self::transfers).
    pub fn start_export(&self, to_device_id: &str) -> Result<String> {
        let active = self.active()?;
        let me = active.device_id.clone();
        if to_device_id == me {
            return Err(CoreError::input("cannot transfer history to oneself"));
        }
        if self.limits().transfer_max_open.is_none() {
            return Err(CoreError::input(
                "the relay does not support history transfer",
            ));
        }
        let report = self.sync()?;
        if report.offline {
            return Err(CoreError::Network {
                reason: "offline".into(),
            });
        }
        let now = self.clock.now_ms();
        let tid = new_uuid()?;
        // Erased during the sync: nothing is written into the emptied store.
        self.live_tx(&active, |db| new_export_row(db, to_device_id, &tid, now))?;
        self.step_transfers()?;
        Ok(tid)
    }

    /// Lets a waiting incoming transfer run (when `auto_import` is off).
    pub fn accept_import(&self, transfer_id: &str) -> Result<()> {
        let active = self.active()?;
        self.live_tx(&active, |db| {
            let mut t = db
                .transfer(transfer_id)?
                .filter(|t| t.role == "import" && t.state == "received")
                .ok_or_else(|| CoreError::input("no such incoming transfer"))?;
            t.state = "accepted".into();
            t.updated_at_ms = self.clock.now_ms();
            db.put_transfer(&t)
        })?;
        self.step_transfers()
    }

    /// Refuses an incoming transfer (the relay is told `declined`).
    pub fn decline_import(&self, transfer_id: &str) -> Result<()> {
        self.close_transfer(transfer_id, "declined", "declined")
    }

    /// Cancels a running export or import.
    pub fn cancel_transfer(&self, transfer_id: &str) -> Result<()> {
        self.close_transfer(transfer_id, "cancelled", "cancelled")
    }

    fn close_transfer(&self, transfer_id: &str, state: &str, outcome: &str) -> Result<()> {
        let _g = self.serial(&self.transfer_lock);
        let Some(mut t) = self.store.read(|db| db.transfer(transfer_id))? else {
            return Ok(());
        };
        if is_final_state(&t.state) {
            return Ok(());
        }
        if t.state != "received" || t.role == "import" {
            let _ = self.authed(|| self.transport.delete_transfer(transfer_id, outcome));
        }
        t.state = state.into();
        t.outcome = Some(outcome.into());
        t.updated_at_ms = self.clock.now_ms();
        self.finish_transfer_row(&t)?;
        self.notify();
        Ok(())
    }

    /// Persists a final state and erases the staging area and the history key.
    fn finish_transfer_row(&self, t: &TransferRow) -> Result<()> {
        let mut t = t.clone();
        t.history_key_sealed = None;
        t.temp_key_sealed = None;
        self.store.tx(|db| {
            db.delete_blob_uploads("transfer", &t.transfer_id)?;
            db.put_transfer(&t)
        })?;
        for sub in ["export", "import"] {
            let _ =
                std::fs::remove_dir_all(self.store.file_path(&format!("{sub}/{}", t.transfer_id)));
        }
        Ok(())
    }

    fn fail_transfer(&self, t: &mut TransferRow, code: &str, tell_relay: bool) -> Result<()> {
        if tell_relay {
            let _ = self.authed(|| self.transport.delete_transfer(&t.transfer_id, "failed"));
        }
        t.state = "failed".into();
        t.error = Some(code.into());
        t.updated_at_ms = self.clock.now_ms();
        self.finish_transfer_row(t)
    }

    pub(crate) fn step_transfers_quiet(&self) {
        if self.step_transfers().is_ok() {
            self.notify();
        }
    }

    /// Advances every unfinished task as far as possible. A task that cannot go further
    /// now (offline, relay limit) is left as is and retried at the next call.
    pub fn step_transfers(&self) -> Result<()> {
        let Some(_g) = self.try_serial(&self.transfer_lock) else {
            return Ok(());
        };
        // Exports announced at pairing become tasks as soon as the log has caught up with the
        // new device (`pairing-v1.md` §9 step 4).
        self.promote_announced_exports()?;
        let rows = self.store.read(|db| db.transfers())?;
        for t in rows.into_iter().filter(|t| !is_final_state(&t.state)) {
            let id = t.transfer_id.clone();
            let result = if t.role == "export" {
                self.step_export(t)
            } else {
                self.step_import(t)
            };
            match result {
                Ok(()) => {}
                // Not reachable now: keep the task, try again later.
                Err(CoreError::Network { .. } | CoreError::Blocked { .. }) => {}
                Err(e) => {
                    // A local failure ends this task, not the others.
                    if let Some(mut t) = self.store.read(|db| db.transfer(&id))? {
                        if !is_final_state(&t.state) {
                            self.fail_transfer(&mut t, e.code(), true)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn step_transfer_relay<T>(&self, r: std::result::Result<T, StepErr>) -> Result<T> {
        match r {
            Ok(v) => Ok(v),
            Err(StepErr::Relay(e)) => Err(e.into()),
            Err(StepErr::Local(e)) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Export
    // -----------------------------------------------------------------------

    fn step_export(&self, mut t: TransferRow) -> Result<()> {
        loop {
            match t.state.as_str() {
                "building" => {
                    self.export_build(&mut t)?;
                }
                "uploading" => {
                    self.ensure_session()?;
                    let uploads = self
                        .store
                        .read(|db| db.blob_uploads("transfer", &t.transfer_id))?;
                    for b in &uploads {
                        match self.upload_blob(b) {
                            Ok(()) => {}
                            Err(StepErr::Relay(RelayError::Api(p)))
                                if p.status < 500 && p.status != 429 =>
                            {
                                return self.fail_transfer(&mut t, &p.code, false);
                            }
                            Err(e) => return self.step_transfer_relay(Err(e)),
                        }
                    }
                    let post = PostTransfer {
                        transfer_id: t.transfer_id.clone(),
                        manifest: t.manifest.clone().unwrap_or_default(),
                        blob_ids: uploads.iter().map(|b| b.blob_id.clone()).collect(),
                    };
                    match self.authed(|| self.transport.post_transfer(&post)) {
                        Ok(_) => {
                            t.state = "posted".into();
                            t.updated_at_ms = self.clock.now_ms();
                            self.store.tx(|db| db.put_transfer(&t))?;
                        }
                        // Too many open transfers to that device: wait.
                        Err(RelayError::Api(p)) if p.code == "transfer_limit" => return Ok(()),
                        Err(RelayError::Api(p)) if p.status < 500 && p.status != 429 => {
                            let code = if p.code == "payload_too_large" {
                                "too_large"
                            } else {
                                &p.code
                            };
                            return self.fail_transfer(&mut t, code, false);
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                // Waiting for `history_transfer_closed`; the files can go.
                _ => return Ok(()),
            }
            self.notify();
        }
    }

    fn export_build(&self, t: &mut TransferRow) -> Result<()> {
        let a = self.active()?;
        let limits = self.limits();
        let tid = t.transfer_id.clone();
        let dir_rel = format!("export/{tid}");
        let dir = self.store.file_path(&dir_rel);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        let hk = HistoryKey::generate()?;
        let (rows, devices, pins, archives, recipient_ed, skipped) = self.read_crypto(|c, _| {
            self.store.read(|db| {
                let mut rows: Vec<ItemRow> = Vec::new();
                let mut skipped = 0u64;
                for r in db.items()? {
                    match r.status {
                        ItemStatus::Readable
                            if r.send_status
                                .is_none_or(|s| s == crate::runtime::store::SendStatus::Sent) =>
                        {
                            rows.push(r);
                        }
                        // Still in the outbox: it will reach the recipient through the log.
                        ItemStatus::Readable => {}
                        ItemStatus::Unreadable(_) => skipped += 1,
                    }
                }
                rows.sort_by_key(|r| (r.seq, r.created_at_ms));
                let devices = db.devices()?;
                let peer = c
                    .trust
                    .members()
                    .into_iter()
                    .find(|m| m.device_id().is_ok_and(|id| id == t.peer_device_id))
                    .ok_or_else(|| CoreError::input("recipient is not a trusted member"))?;
                Ok((
                    rows,
                    devices,
                    c.pins.clone(),
                    c.archives.clone(),
                    peer.ed25519,
                    skipped,
                ))
            })
        })?;

        // 1. Index, written through a temporary encryption (no plaintext index on disk).
        let header = HistoryHeader {
            space_id: a.space_id.clone(),
            transfer_id: tid.clone(),
            as_of_seq: t.as_of_seq,
            source_device: a.device_id.clone(),
        };
        let mut writer = HistoryIndexWriter::new(&header)?;
        let temp = ChunkEncryptor::new(&tid, TEMP_INDEX_ATTACHMENT)?;
        let temp_path = dir.join("index.tmp");
        let mut temp_file = std::fs::File::create(&temp_path)?;
        let mut push = |line: Vec<u8>| -> Result<()> {
            let line = Zeroizing::new(line);
            temp_file.write_all(&temp.update(&line)?)?;
            Ok(())
        };
        push(writer.header_line()?)?;
        for d in &devices {
            push(writer.device_line(&HistoryDevice {
                device_id: d.device_id.clone(),
                name: d.name.clone(),
                platform: d.platform.clone(),
                revoked: matches!(d.state, DeviceState::Revoked | DeviceState::RevokedByRelay),
            })?)?;
        }
        let mut file_uploads: Vec<BlobUpload> = Vec::new();
        let mut items_total = 0u64;
        let mut files_total = 0u64;
        for row in &rows {
            let Some(sealed) = &row.sealed else { continue };
            let item = a.lsk.open_item(sealed, &row.item_id)?;
            let local_files = self.store.read(|db| db.files(&row.item_id))?;
            // Removed attachments are exported as removed, whatever the scope (spec §8.10.5).
            let removed = self
                .store
                .read(|db| crate::runtime::removal::removed_slots(db, &item, &[]))?;
            let mut files = Vec::with_capacity(item.attachments.len());
            for (i, att) in item.attachments.iter().enumerate() {
                let idx = u32::try_from(i).map_err(|_| CoreError::input("too many attachments"))?;
                if removed.contains(&idx) {
                    files.push(None);
                    continue;
                }
                let src = local_files
                    .iter()
                    .find(|f| f.idx == idx && f.state == FileState::Done)
                    .and_then(|f| f.path.as_ref().map(|p| (f, self.store.file_path(p))))
                    .filter(|(_, p)| p.exists());
                files.push(match src {
                    None => None,
                    Some((file, src)) => {
                        let blob_id = new_blob_id()?;
                        let rel = format!("{dir_rel}/{blob_id}");
                        let fin = reencrypt_file(
                            &src,
                            &local_attachment(att, file),
                            &self.store.file_path(&rel),
                            &row.item_id,
                            idx,
                        )?;
                        file_uploads.push(BlobUpload {
                            blob_id: blob_id.clone(),
                            size: encrypted_attachment_len(fin.size),
                            path: rel,
                        });
                        files_total += 1;
                        Some(TransferredFile {
                            blob_id,
                            file_key: fin.file_key.to_vec(),
                            nonce_prefix: fin.nonce_prefix,
                        })
                    }
                });
            }
            let entry = HistoryItem {
                item,
                origin_seq: row.seq,
                received_at_ms: row.received_at_ms,
                read: row.read,
                kept: row.kept,
                files,
                removed_attachments: removed.iter().copied().collect(),
                pin: pins.state(&row.item_id),
                archive: archives.state(&row.item_id),
            };
            push(writer.item_line(&entry)?)?;
            items_total += 1;
        }
        push(writer.end_line(skipped)?)?;
        let fin = temp.finalize()?;
        temp_file.write_all(&fin.ciphertext)?;
        temp_file.sync_all()?;
        drop(temp_file);

        // 2. Cut into encrypted parts.
        let n = history_part_count(fin.size, limits.max_blob_bytes)?;
        let cap = history_part_capacity(limits.max_blob_bytes)?;
        let mut parts = PartSink {
            hk: &hk,
            tid: &tid,
            n,
            cap,
            dir_rel: &dir_rel,
            store: &self.store,
            current: None,
            out: Vec::new(),
        };
        let reader_att = Attachment {
            blob_id: tid.clone(),
            name: String::new(),
            mime: String::new(),
            size: fin.size,
            file_key: fin.file_key.to_vec(),
            nonce_prefix: fin.nonce_prefix.clone(),
            thumbnail: None,
        };
        let dec = ChunkDecryptor::new(&tid, TEMP_INDEX_ATTACHMENT, &reader_att)?;
        let mut src = std::fs::File::open(&temp_path)?;
        let mut buf = vec![0u8; CHUNK_CIPHERTEXT_SIZE];
        loop {
            let k = read_full(&mut src, &mut buf)?;
            if k == 0 {
                break;
            }
            let plain = Zeroizing::new(dec.update(&buf[..k])?);
            parts.push(&plain)?;
        }
        dec.finalize()?;
        let part_sink = parts.finish()?;
        drop(src);
        let _ = std::fs::remove_file(&temp_path);
        if u32::try_from(part_sink.len()) != Ok(n) {
            return Err(CoreError::Internal {
                reason: "history part count mismatch".into(),
            });
        }

        // 3. Signed manifest.
        let manifest = self.read_crypto(|c, a| {
            let history_parts: Vec<HistoryPart> =
                part_sink.iter().map(|(p, _)| p.clone()).collect();
            c.trust.create_history_transfer(
                &a.keys,
                &recipient_ed,
                &tid,
                t.as_of_seq,
                u64::try_from(self.clock.now_ms()).unwrap_or(0),
                &hk,
                &history_parts,
            )
        })?;
        t.manifest = Some(manifest.to_bytes()?);
        t.index_len = fin.size;
        t.items_total = items_total;
        t.skipped_unreadable = skipped;
        t.files_total = files_total;
        t.state = "uploading".into();
        t.updated_at_ms = self.clock.now_ms();
        self.store.tx(|db| {
            db.delete_blob_uploads("transfer", &tid)?;
            for (_, b) in &part_sink {
                db.put_blob_upload("transfer", &tid, b)?;
            }
            for b in &file_uploads {
                db.put_blob_upload("transfer", &tid, b)?;
            }
            db.put_transfer(t)
        })
    }

    // -----------------------------------------------------------------------
    // Import
    // -----------------------------------------------------------------------

    fn step_import(&self, mut t: TransferRow) -> Result<()> {
        loop {
            match t.state.as_str() {
                "received" if !self.cfg.auto_import => return Ok(()),
                "received" | "accepted" => {
                    t.state = "verifying".into();
                    self.store.tx(|db| db.put_transfer(&t))?;
                }
                "verifying" => {
                    self.ensure_session()?;
                    let info = match self.authed(|| self.transport.get_transfer(&t.transfer_id)) {
                        Ok(i) => i,
                        Err(RelayError::Api(p)) if p.status == 404 => {
                            return self.fail_transfer(&mut t, "expired", false);
                        }
                        Err(e) => return Err(e.into()),
                    };
                    let verified = HistoryTransfer::from_bytes(&info.manifest).and_then(|m| {
                        if m.transfer_id != t.transfer_id {
                            return Err(CoreError::statement("manifest of another transfer"));
                        }
                        let hk =
                            self.read_crypto(|c, a| c.trust.verify_history_transfer(&m, &a.keys))?;
                        Ok((m, hk))
                    });
                    let (m, hk) = match verified {
                        Ok(v) => v,
                        Err(e) => return self.fail_transfer(&mut t, e.code(), true),
                    };
                    t.manifest = Some(info.manifest.clone());
                    t.as_of_seq = m.as_of_seq;
                    t.state = "downloading".into();
                    t.updated_at_ms = self.clock.now_ms();
                    let sealed = self.store.read(|db| {
                        db.seal_secret(&format!("history_key:{}", t.transfer_id), &hk.to_bytes())
                    })?;
                    t.history_key_sealed = Some(sealed);
                    self.store.tx(|db| db.put_transfer(&t))?;
                }
                "downloading" => {
                    let m = self.manifest_of(&t)?;
                    let info = match self.authed(|| self.transport.get_transfer(&t.transfer_id)) {
                        Ok(i) => i,
                        Err(RelayError::Api(p)) if p.status == 404 => {
                            return self.fail_transfer(&mut t, "expired", false);
                        }
                        Err(e) => return Err(e.into()),
                    };
                    let max_part = self.limits().max_blob_bytes;
                    for (i, part) in m.parts.iter().enumerate() {
                        let Some(b) = info.blobs.iter().find(|b| b.blob_id == part.blob_id) else {
                            return self.fail_transfer(&mut t, "part_missing", true);
                        };
                        // The size comes from the relay: never download more than a blob
                        // may hold.
                        if b.size > max_part {
                            return self.fail_transfer(&mut t, "part_too_large", true);
                        }
                        let path = self
                            .store
                            .file_path(&format!("import/{}/part-{i}", t.transfer_id));
                        self.step_transfer_relay(self.fetch_blob(&b.blob_id, b.size, &path))?;
                    }
                    t.state = "importing".into();
                    self.store.tx(|db| db.put_transfer(&t))?;
                }
                "importing" => {
                    let m = self.manifest_of(&t)?;
                    let hk = self.history_key_of(&t)?;
                    if let Err(e) = self.import_index(&mut t, &m, &hk) {
                        return self.fail_transfer(&mut t, e.code(), true);
                    }
                    t.state = "files".into();
                    t.updated_at_ms = self.clock.now_ms();
                    self.store.tx(|db| db.put_transfer(&t))?;
                    let _ = std::fs::remove_dir_all(
                        self.store.file_path(&format!("import/{}", t.transfer_id)),
                    );
                }
                "files" => {
                    self.import_files(&mut t)?;
                    t.state = "finishing".into();
                    self.store.tx(|db| db.put_transfer(&t))?;
                }
                "finishing" => {
                    self.ensure_session()?;
                    match self.authed(|| self.transport.delete_transfer(&t.transfer_id, "imported"))
                    {
                        Ok(()) => {}
                        Err(RelayError::Api(p)) if p.status == 404 => {}
                        Err(e) => return Err(e.into()),
                    }
                    t.state = "done".into();
                    t.outcome = Some("imported".into());
                    t.updated_at_ms = self.clock.now_ms();
                    self.finish_transfer_row(&t)?;
                    self.notify();
                    return Ok(());
                }
                _ => return Ok(()),
            }
            self.notify();
        }
    }

    fn manifest_of(&self, t: &TransferRow) -> Result<HistoryTransfer> {
        HistoryTransfer::from_bytes(
            t.manifest
                .as_deref()
                .ok_or_else(|| CoreError::input("transfer without manifest"))?,
        )
    }

    fn history_key_of(&self, t: &TransferRow) -> Result<HistoryKey> {
        let sealed = t
            .history_key_sealed
            .as_deref()
            .ok_or_else(|| CoreError::input("transfer without key"))?;
        let bytes = self
            .store
            .read(|db| db.open_secret(&format!("history_key:{}", t.transfer_id), sealed))?;
        HistoryKey::from_bytes(&bytes)
    }

    /// Decrypts the staged parts in order and imports the entries (idempotent).
    fn import_index(
        &self,
        t: &mut TransferRow,
        m: &HistoryTransfer,
        hk: &HistoryKey,
    ) -> Result<()> {
        let mut reader = HistoryIndexReader::new(m)?;
        let n = u32::try_from(m.parts.len()).map_err(|_| CoreError::input("too many parts"))?;
        for p in 0..n {
            let dec = HistoryPartDecryptor::new(hk, m, p)?;
            let path = self
                .store
                .file_path(&format!("import/{}/part-{p}", t.transfer_id));
            let mut f = std::fs::File::open(path)?;
            let mut buf = vec![0u8; CHUNK_CIPHERTEXT_SIZE];
            loop {
                let k = read_full(&mut f, &mut buf)?;
                if k == 0 {
                    break;
                }
                let plain = Zeroizing::new(dec.update(&buf[..k])?);
                let entries = reader.feed(&plain)?;
                self.import_entries(t, entries)?;
            }
            let tail = Zeroizing::new(dec.finalize()?);
            let entries = reader.feed(&tail)?;
            self.import_entries(t, entries)?;
        }
        let summary = reader.finish()?;
        t.items_total = summary.items;
        t.skipped_unreadable = summary.skipped_unreadable;
        Ok(())
    }

    fn import_entries(&self, t: &mut TransferRow, entries: Vec<HistoryEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let tid = t.transfer_id.clone();
        let done = self.with_crypto(|c, db, a| {
            let mut done = 0u64;
            for e in entries {
                match e {
                    HistoryEntry::Device(d) => {
                        if db.device(&d.device_id)?.is_none() {
                            db.put_device(&DeviceRow {
                                device_id: d.device_id,
                                name: d.name,
                                platform: d.platform,
                                ed25519: None,
                                x25519: None,
                                state: if d.revoked {
                                    DeviceState::Revoked
                                } else {
                                    DeviceState::Unverified
                                },
                                joined_seq: 0,
                                last_seen_at_ms: None,
                            })?;
                        }
                    }
                    HistoryEntry::Item(h) => {
                        let id = h.item.id.clone();
                        if db.is_tombstoned(&id)? {
                            continue;
                        }
                        // The pin converges even when the item is already here.
                        if let Some(pin) = h.pin {
                            if c.pins.merge(&id, pin)? {
                                if let Some(s) = c.pins.state(&id) {
                                    db.put_pin(&id, s)?;
                                }
                            }
                        }
                        // So does the archive state (spec §8.9.4).
                        if let Some(archive) = h.archive {
                            if c.archives.merge(&id, archive)? {
                                if let Some(s) = c.archives.state(&id) {
                                    db.put_archive(&id, s)?;
                                }
                            }
                        }
                        if db.item_exists(&id)? {
                            continue;
                        }
                        let mut row = ItemRow {
                            item_id: id.clone(),
                            seq: h.origin_seq,
                            origin: ItemOrigin::Imported,
                            sender_device_id: h.item.source_device.clone(),
                            created_at_ms: h.item.created_at_ms,
                            received_at_ms: h.received_at_ms,
                            relay_expires_at_ms: None,
                            key_epoch: 0,
                            status: ItemStatus::Unreadable(UnreadableReason::Undecryptable),
                            sealed: None,
                            raw_envelope: None,
                            payload_expires_at_ms: None,
                            secret: false,
                            read_policy: "until_expiry".into(),
                            read: h.read,
                            kept: h.kept,
                            blobs: Vec::new(),
                            ring: false,
                            send_status: None,
                        };
                        // A removed attachment is emptied before the item is sealed: it never
                        // existed here (spec §8.10.5).
                        let removed = crate::runtime::removal::removed_slots(
                            db,
                            &h.item,
                            &h.removed_attachments,
                        )?;
                        let imported = crate::runtime::removal::emptied(&h.item, &removed)?;
                        fill_readable(&mut row, &imported, a)?;
                        db.put_item(&row)?;
                        for (i, att) in imported.attachments.iter().enumerate() {
                            let idx =
                                u32::try_from(i).map_err(|_| CoreError::input("attachments"))?;
                            if removed.contains(&idx) {
                                continue;
                            }
                            let file = match h.files.get(i).and_then(Option::as_ref) {
                                Some(f) => FileRow {
                                    item_id: id.clone(),
                                    idx,
                                    blob_id: Some(f.blob_id.clone()),
                                    size: att.size,
                                    key: Some(f.file_key.clone()),
                                    nonce_prefix: Some(f.nonce_prefix.clone()),
                                    state: FileState::Remote,
                                    path: None,
                                },
                                None => FileRow {
                                    item_id: id.clone(),
                                    idx,
                                    blob_id: None,
                                    size: att.size,
                                    key: None,
                                    nonce_prefix: None,
                                    state: FileState::Unavailable,
                                    path: None,
                                },
                            };
                            db.put_file(&file)?;
                        }
                        done += 1;
                    }
                }
            }
            if let Some(mut row) = db.transfer(&tid)? {
                row.items_done += done;
                row.updated_at_ms = self.clock.now_ms();
                db.put_transfer(&row)?;
            }
            Ok(done)
        })?;
        t.items_done += done;
        self.notify();
        Ok(())
    }

    /// Downloads the files of the imported items while the transfer still exists.
    fn import_files(&self, t: &mut TransferRow) -> Result<()> {
        let a = self.active()?;
        let pending: Vec<(ItemRow, FileRow)> = self.store.read(|db| {
            let mut out = Vec::new();
            for row in db.items()? {
                if row.origin != ItemOrigin::Imported || row.status != ItemStatus::Readable {
                    continue;
                }
                for f in db.files(&row.item_id)? {
                    if f.state == FileState::Remote && f.key.is_some() {
                        out.push((row.clone(), f));
                    }
                }
            }
            Ok(out)
        })?;
        t.files_total = t.files_done + u64::try_from(pending.len()).unwrap_or(0);
        for (row, file) in pending {
            let Some(sealed) = &row.sealed else { continue };
            let item = a.lsk.open_item(sealed, &row.item_id)?;
            let Some(base) = item.attachments.get(file.idx as usize) else {
                continue;
            };
            let att = Attachment {
                blob_id: file.blob_id.clone().unwrap_or_default(),
                file_key: file.key.clone().unwrap_or_default(),
                nonce_prefix: file.nonce_prefix.clone().unwrap_or_default(),
                ..base.clone()
            };
            // Kept encrypted under FK' (sealed in `item_files`), decrypted on demand.
            let result = self.download_encrypted(&row.item_id, file.idx, &att);
            let new_state = match result {
                Ok(rel) => FileRow {
                    state: FileState::Done,
                    path: Some(rel),
                    ..file.clone()
                },
                // Purged / unknown on the relay, or corrupted: shown as unavailable.
                Err(StepErr::Relay(RelayError::Api(p))) if p.status == 404 => FileRow {
                    state: FileState::Unavailable,
                    ..file.clone()
                },
                Err(StepErr::Local(CoreError::DecryptionFailed | CoreError::StreamTruncated)) => {
                    FileRow {
                        state: FileState::Unavailable,
                        ..file.clone()
                    }
                }
                Err(StepErr::Relay(e)) => return Err(e.into()),
                Err(StepErr::Local(e)) => return Err(e),
            };
            t.files_done += 1;
            let kept = self.store.tx(|db| {
                // Removed while it downloaded: the row stays gone, the file goes.
                let gone = db.item_exists(&row.item_id)?
                    && db.attachment_removal(&row.item_id, file.idx)?.is_some();
                if !gone {
                    db.put_file(&new_state)?;
                }
                db.put_transfer(t)?;
                Ok(!gone)
            })?;
            if !kept {
                if let Some(p) = &new_state.path {
                    let _ = std::fs::remove_file(self.store.file_path(p));
                }
            }
            self.notify();
        }
        Ok(())
    }
}

/// Re-encrypts the local encrypted copy `src` (opened with `src_att`) under a fresh key for
/// the transfer (`api-v1.md` §7.8). The plaintext only exists in memory, chunk by chunk.
fn reencrypt_file(
    src: &std::path::Path,
    src_att: &Attachment,
    dest: &std::path::Path,
    item_id: &str,
    idx: u32,
) -> Result<crate::stream::FinalizedAttachment> {
    let dec = ChunkDecryptor::new(item_id, idx, src_att)?;
    let enc = ChunkEncryptor::new(item_id, idx)?;
    let mut input = std::fs::File::open(src)?;
    let mut out = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; CHUNK_CIPHERTEXT_SIZE];
    loop {
        let n = read_full(&mut input, &mut buf)?;
        if n == 0 {
            break;
        }
        let plain = Zeroizing::new(dec.update(&buf[..n])?);
        out.write_all(&enc.update(&plain)?)?;
    }
    dec.finalize()?;
    let fin = enc.finalize()?;
    out.write_all(&fin.ciphertext)?;
    out.sync_all()?;
    Ok(fin)
}

/// Writes the plaintext index as encrypted parts of at most `cap` bytes each.
struct PartSink<'a> {
    hk: &'a HistoryKey,
    tid: &'a str,
    n: u32,
    cap: u64,
    dir_rel: &'a str,
    store: &'a crate::runtime::store::Store,
    current: Option<(HistoryPartEncryptor, std::fs::File, String, String, u64)>,
    out: Vec<(HistoryPart, BlobUpload)>,
}

impl PartSink<'_> {
    fn open(&mut self) -> Result<()> {
        let idx = u32::try_from(self.out.len()).map_err(|_| CoreError::input("parts"))?;
        let enc = HistoryPartEncryptor::new(self.hk, self.tid, idx, self.n)?;
        let blob_id = new_blob_id()?;
        let rel = format!("{}/{blob_id}", self.dir_rel);
        let file = std::fs::File::create(self.store.file_path(&rel))?;
        self.current = Some((enc, file, blob_id, rel, 0));
        Ok(())
    }

    fn push(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            if self.current.is_none() {
                self.open()?;
            }
            let cap = self.cap;
            let Some((enc, file, _, _, written)) = self.current.as_mut() else {
                break;
            };
            let take =
                usize::try_from((cap - *written).min(data.len() as u64)).unwrap_or(data.len());
            file.write_all(&enc.update(&data[..take])?)?;
            *written += take as u64;
            data = &data[take..];
            if *written == cap && !data.is_empty() {
                self.close()?;
            }
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some((enc, mut file, blob_id, rel, _)) = self.current.take() {
            file.write_all(&enc.finalize()?)?;
            file.sync_all()?;
            let size = file.metadata()?.len();
            self.out.push((
                HistoryPart {
                    blob_id: blob_id.clone(),
                    nonce_prefix: enc.nonce_prefix(),
                },
                BlobUpload {
                    blob_id,
                    size,
                    path: rel,
                },
            ));
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<(HistoryPart, BlobUpload)>> {
        if self.current.is_none() && self.out.is_empty() {
            self.open()?;
        }
        self.close()?;
        Ok(self.out)
    }
}
