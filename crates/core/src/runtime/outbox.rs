//! Persistent outgoing queue and blob upload (`api-v1.md` §7.7).
//!
//! Every outgoing operation is stored before any request, with everything needed to replay
//! it identically (ids drawn, envelope sealed, blobs encrypted on disk). Operations are
//! idempotent on the relay, so a replay after a cut connection has no side effect.

use std::io::{Read, Seek, SeekFrom};

use crate::envelope::{read_envelope_header, seal_item};
use crate::error::{CoreError, Result};
use crate::keyring::relay_expires_at_ms;
use crate::pin::{read_item_state_header, seal_item_state, OutgoingItemState, StateStatement};
use crate::runtime::engine::{lock, Engine};
use crate::runtime::store::{BlobUpload, OpKind, OpRow, SendStatus};
use crate::runtime::transport::{ApiProblem, PostItem, RelayError};

/// Result of one attempt at an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Done on the relay (or moot): remove from the queue.
    Done,
    /// Temporary failure: retry later, the queue keeps going.
    Defer { delay_ms: i64, reason: String },
    /// Permanent refusal: remove, mark the item failed.
    Fail(String),
    /// The relay could not be reached: stop the pass.
    Offline(String),
    /// The local epoch is behind: sync (rotation), then retry.
    NeedSync,
    /// The item expired before it could be sent.
    Expired,
}

/// Error inside a step: either the relay's, or a local one (file, database).
pub(crate) enum StepErr {
    Relay(RelayError),
    Local(CoreError),
}

impl From<RelayError> for StepErr {
    fn from(e: RelayError) -> Self {
        StepErr::Relay(e)
    }
}

impl From<CoreError> for StepErr {
    fn from(e: CoreError) -> Self {
        StepErr::Local(e)
    }
}

impl From<std::io::Error> for StepErr {
    fn from(e: std::io::Error) -> Self {
        StepErr::Local(e.into())
    }
}

/// Backoff for attempt `n` (1 s doubling up to 5 min) with 50 to 100 % jitter.
pub(crate) fn backoff_ms(attempts: u32) -> i64 {
    let base = 1000i64.saturating_mul(1i64 << attempts.min(9)).min(300_000);
    let jitter = crate::crypto::random_array::<2>()
        .map(u16::from_le_bytes)
        .unwrap_or(u16::MAX);
    base / 2 + (base / 2) * i64::from(jitter) / i64::from(u16::MAX)
}

/// Classifies a relay error for operations without a specific rule.
pub(crate) fn classify(e: RelayError, attempts: u32) -> Outcome {
    match e {
        RelayError::Network(r) => Outcome::Offline(r),
        RelayError::Api(p) => {
            let ApiProblem {
                code,
                retry_after_secs,
                status,
                ..
            } = *p;
            classify_api(code, retry_after_secs, status, attempts)
        }
    }
}

fn classify_api(
    code: String,
    retry_after_secs: Option<u64>,
    status: u16,
    attempts: u32,
) -> Outcome {
    match code.as_str() {
        "stale_key_epoch" => Outcome::NeedSync,
        "relay_full" => Outcome::Defer {
            delay_ms: 300_000,
            reason: code,
        },
        "rate_limited" => Outcome::Defer {
            delay_ms: i64::try_from(retry_after_secs.unwrap_or(1).max(1).saturating_mul(1000))
                .unwrap_or(i64::MAX / 2),
            reason: code,
        },
        _ if status >= 500 => Outcome::Defer {
            delay_ms: backoff_ms(attempts),
            reason: code,
        },
        _ => Outcome::Fail(code),
    }
}

fn step_outcome(e: StepErr, attempts: u32) -> Result<Outcome> {
    match e {
        StepErr::Relay(r) => Ok(classify(r, attempts)),
        StepErr::Local(l) => Err(l),
    }
}

/// `stale_key_epoch` answers after which an operation is given up (marked failed): the
/// catch-up sync did not bring the epoch the relay wants (refused rotation, unknown issuer,
/// buggy relay). Between two attempts the delay grows ([`backoff_ms`]).
pub(crate) const MAX_STALE_ATTEMPTS: u32 = 8;

/// Wait before asking again a relay that answered `404` to `POST /v1/attachment-tombstones`
/// (it does not know the route yet): 6 hours.
pub(crate) const UNKNOWN_ROUTE_RETRY_MS: i64 = 6 * 3_600_000;

/// Whether a staged upload file is the item's own encrypted copy at rest (kept after the
/// upload) rather than a temporary staging file.
pub(crate) fn is_item_file(path: &str) -> bool {
    path.starts_with("files/")
}

impl Engine {
    /// Sends what can be sent. Returns `true` if the relay could not be reached.
    pub fn drain_outbox(&self) -> Result<bool> {
        let now = self.clock.now_ms();
        let ops = self.store.read(|db| db.due_ops(now))?;
        let mut offline = false;
        for op in ops {
            let _serial = self.serial(&self.dispatch_lock);
            // Removed meanwhile (cancelled, item erased)?
            let Some(op) = self.store.read(|db| db.op(op.op_id))? else {
                continue;
            };
            if self.settle(&op)? {
                offline = true;
                break;
            }
        }
        if offline {
            self.notify();
        }
        Ok(offline)
    }

    /// Runs `op` and records its outcome. Returns `true` when offline.
    pub(crate) fn settle(&self, op: &OpRow) -> Result<bool> {
        let outcome = self.run_op(op)?;
        let now = self.clock.now_ms();
        let mut offline = false;
        match outcome {
            Outcome::Done => {}
            Outcome::Defer { delay_ms, reason } => {
                self.store
                    .tx(|db| db.defer_op(op.op_id, now + delay_ms, &reason))?;
            }
            Outcome::Offline(reason) => {
                self.store.tx(|db| db.defer_op(op.op_id, now, &reason))?;
                offline = true;
            }
            Outcome::NeedSync => {
                let stale = op.stale_attempts.saturating_add(1);
                if stale >= MAX_STALE_ATTEMPTS {
                    // Not resolvable by a sync: give up this one, the others go on.
                    self.drop_op(op, SendStatus::Failed, "stale_key_epoch")?;
                } else {
                    // One catch-up sync per operation and per `sync()`; after that, wait.
                    let first = lock(&self.resynced_ops)?.insert(op.op_id);
                    let next = if first {
                        self.dirty_again
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        now
                    } else {
                        now.saturating_add(backoff_ms(stale))
                    };
                    self.store.tx(|db| db.defer_op_stale(op.op_id, next))?;
                }
            }
            Outcome::Fail(code) => self.drop_op(op, SendStatus::Failed, &code)?,
            Outcome::Expired => self.drop_op(op, SendStatus::ExpiredBeforeSend, "expired")?,
        }
        self.notify();
        Ok(offline)
    }

    fn drop_op(&self, op: &OpRow, status: SendStatus, why: &str) -> Result<()> {
        let now = self.clock.now_ms();
        let staged = self.store.tx(|db| {
            if op.kind == OpKind::SendItem {
                if let Some(id) = &op.item_id {
                    db.set_send_status(id, status)?;
                }
            } else {
                db.diag(now, "outbox_dropped", &format!("{:?}: {why}", op.kind))?;
            }
            let uploads = db.blob_uploads("op", &op.op_id.to_string())?;
            db.delete_blob_uploads("op", &op.op_id.to_string())?;
            db.finish_op(op.op_id)?;
            Ok(uploads)
        })?;
        for b in staged {
            // Best effort: the relay purges orphans after 24 h anyway.
            let _ = self.transport.delete_blob(&b.blob_id);
            self.discard_staged(&b);
        }
        Ok(())
    }

    /// Removes a staging file once its upload is over, but never the item's own encrypted
    /// copy (erased with the item).
    pub(crate) fn discard_staged(&self, b: &BlobUpload) {
        if !is_item_file(&b.path) {
            let _ = std::fs::remove_file(self.store.file_path(&b.path));
        }
    }

    fn finish(&self, op: &OpRow) -> Result<()> {
        self.store.tx(|db| db.finish_op(op.op_id))
    }

    fn run_op(&self, op: &OpRow) -> Result<Outcome> {
        match op.kind {
            OpKind::SendItem => self.run_send_item(op),
            OpKind::Tombstone => {
                let payload = op.payload.clone().unwrap_or_default();
                match self.authed(|| self.transport.post_tombstone(&payload)) {
                    Ok(_) => self.done(op),
                    Err(e) => Ok(classify(e, op.attempts)),
                }
            }
            OpKind::Ack => {
                let id = op.item_id.clone().unwrap_or_default();
                match self.authed(|| self.transport.ack_item(&id, op.flag)) {
                    Ok(()) => self.done(op),
                    // Unknown / not visible to this device: nothing to acknowledge.
                    Err(RelayError::Api(p)) if p.status == 404 => self.done(op),
                    Err(e) => Ok(classify(e, op.attempts)),
                }
            }
            OpKind::Read => {
                let id = op.item_id.clone().unwrap_or_default();
                match self.authed(|| self.transport.read_item(&id)) {
                    Ok(()) => self.done(op),
                    Err(RelayError::Api(p)) if p.status == 404 => self.done(op),
                    Err(e) => Ok(classify(e, op.attempts)),
                }
            }
            OpKind::ItemState => self.run_item_state(op),
            OpKind::AttachmentTombstone => {
                let payload = op.payload.clone().unwrap_or_default();
                match self.authed(|| self.transport.post_attachment_tombstone(&payload)) {
                    Ok(_) => self.done(op),
                    // A relay without the route: wait a long time, never drop the removal.
                    Err(RelayError::Api(p)) if p.status == 404 => Ok(Outcome::Defer {
                        delay_ms: UNKNOWN_ROUTE_RETRY_MS,
                        reason: "route_unknown".into(),
                    }),
                    Err(e) => Ok(classify(e, op.attempts)),
                }
            }
        }
    }

    fn done(&self, op: &OpRow) -> Result<Outcome> {
        self.finish(op)?;
        Ok(Outcome::Done)
    }

    fn run_send_item(&self, op: &OpRow) -> Result<Outcome> {
        let item_id = op.item_id.clone().unwrap_or_default();
        let Some(row) = self.store.read(|db| db.item(&item_id))? else {
            // The item was erased locally: nothing to send.
            self.finish(op)?;
            return Ok(Outcome::Done);
        };
        let server_now = self.server_now_ms();
        let limits = self.limits();
        let relay_exp =
            relay_expires_at_ms(row.payload_expires_at_ms, server_now, limits.max_ttl_secs);
        if relay_exp <= server_now {
            return Ok(Outcome::Expired);
        }
        // Re-seal under the current epoch if a rotation was applied since (same item id).
        let mut envelope = op.payload.clone().unwrap_or_default();
        let header = read_envelope_header(&envelope)?;
        let current = self.read_crypto(|c, _| Ok(c.ring.current_epoch()))?;
        if header.key_epoch != current {
            envelope = self.read_crypto(|c, a| {
                let sealed = row
                    .sealed
                    .as_ref()
                    .ok_or_else(|| CoreError::input("item has no local record"))?;
                let item = a.lsk.open_item(sealed, &item_id)?;
                seal_item(&item, c.ring.current()?)
            })?;
            let env = envelope.clone();
            self.store
                .tx(|db| db.update_op_payload(op.op_id, &env, None))?;
        }
        let uploads = self
            .store
            .read(|db| db.blob_uploads("op", &op.op_id.to_string()))?;
        for b in &uploads {
            if let Err(e) = self.upload_blob(b) {
                return step_outcome(e, op.attempts);
            }
        }
        let post = PostItem {
            item_id: item_id.clone(),
            expires_at_ms: relay_exp,
            key_epoch: read_envelope_header(&envelope)?.key_epoch,
            envelope,
            blob_ids: uploads.iter().map(|b| b.blob_id.clone()).collect(),
        };
        match self.authed(|| self.transport.post_item(&post)) {
            Ok(acc) => {
                self.store.tx(|db| {
                    db.set_send_status(&item_id, SendStatus::Sent)?;
                    db.set_item_seq(&item_id, acc.seq)?;
                    db.delete_blob_uploads("op", &op.op_id.to_string())?;
                    db.finish_op(op.op_id)
                })?;
                for b in &uploads {
                    self.discard_staged(b);
                }
                Ok(Outcome::Done)
            }
            Err(RelayError::Api(p)) if p.code == "already_expired" => Ok(Outcome::Expired),
            Err(e) => Ok(classify(e, op.attempts)),
        }
    }

    fn run_item_state(&self, op: &OpRow) -> Result<Outcome> {
        let (Some(event_id), Some(envelope), Some(aux)) =
            (op.event_id.clone(), op.payload.clone(), op.aux.clone())
        else {
            self.finish(op)?;
            return Ok(Outcome::Done);
        };
        let statement = StateStatement::from_bytes(&aux)?;
        let mut envelope = envelope;
        let mut key_epoch = read_item_state_header(&envelope)?.key_epoch;
        let current = self.read_crypto(|c, _| Ok(c.ring.current_epoch()))?;
        if key_epoch != current {
            // Same declaration, same event id, sealed under the current key.
            envelope =
                self.read_crypto(|c, _| seal_item_state(&statement, &event_id, c.ring.current()?))?;
            key_epoch = current;
            let env = envelope.clone();
            self.store
                .tx(|db| db.update_op_payload(op.op_id, &env, None))?;
        }
        match self.authed(|| {
            self.transport
                .post_item_state(&event_id, key_epoch, &envelope)
        }) {
            Ok(_) => self.done(op),
            Err(RelayError::Api(p)) if p.code == "event_id_conflict" => {
                if op.flag {
                    // Second conflict for the same decision: a bug, not an abandoned state.
                    return Ok(Outcome::Fail("event_id_conflict".into()));
                }
                let out = OutgoingItemState {
                    event_id: event_id.clone(),
                    key_epoch,
                    envelope: envelope.clone(),
                    statement,
                };
                let again = self.read_crypto(|c, _| out.reissue(c.ring.current()?))?;
                self.store.tx(|db| {
                    db.update_op_payload(op.op_id, &again.envelope, Some(&again.event_id))?;
                    db.defer_op(op.op_id, 0, "event_id_conflict")?;
                    db.mark_op_reissued(op.op_id)
                })?;
                self.dirty_again
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Outcome::Done)
            }
            Err(e) => Ok(classify(e, op.attempts)),
        }
    }

    /// Uploads a staged blob, resuming from the relay's `upload_offset` (`api-v1.md` §4.9).
    pub(crate) fn upload_blob(&self, b: &BlobUpload) -> std::result::Result<(), StepErr> {
        let mut st = self.authed(|| self.transport.create_blob(&b.blob_id, b.size))?;
        if st.complete {
            return Ok(());
        }
        let part_max = self.limits().max_upload_part_bytes.max(1);
        let mut file = std::fs::File::open(self.store.file_path(&b.path))?;
        let mut resyncs = 0;
        while st.upload_offset < b.size {
            let len = part_max.min(b.size - st.upload_offset);
            let mut buf =
                vec![0u8; usize::try_from(len).map_err(|_| CoreError::input("part too large"))?];
            file.seek(SeekFrom::Start(st.upload_offset))?;
            file.read_exact(&mut buf)?;
            let offset = st.upload_offset;
            match self.authed(|| self.transport.put_blob_part(&b.blob_id, offset, &buf)) {
                // The relay must move forward, and not past the end: otherwise this loop
                // would never end (or would claim a complete blob that is not).
                Ok(next) if next > offset && next <= b.size => st.upload_offset = next,
                Ok(_) => {
                    return Err(RelayError::api(502, "upload_offset_not_advanced").into());
                }
                Err(RelayError::Api(p)) if p.code == "upload_offset_mismatch" && resyncs < 3 => {
                    resyncs += 1;
                    st = self.authed(|| self.transport.blob_status(&b.blob_id))?;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
