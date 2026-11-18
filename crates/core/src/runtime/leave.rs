//! Leaving the space and deleting it (`api-v1.md` §7.9): the device quits (`DELETE /v1/me`,
//! §4.4) or, as the last device, deletes the whole space (`DELETE /v1/space`, §4.4.1), and only
//! then erases everything it holds.
//!
//! # Positive proof only (§7.9.1)
//!
//! Nothing is erased automatically except on one of these answers, received whole during the
//! attempt in progress:
//!
//! * `DELETE /v1/me` → `200` with an integer `seq` (checked by the transport);
//! * `403 revoked` from `POST /v1/auth/session`, `DELETE /v1/me` or `DELETE /v1/space`;
//! * `DELETE /v1/space` or `POST /v1/space/receipt` → `200 {"space_deleted": true}` (strict
//!   boolean, checked by the transport).
//!
//! Nothing else is a proof: not a `401` (even twice), not `409 space_not_initialized` (a relay
//! on an unmounted volume answers exactly that), not a `4004`, not any state kept locally
//! (`meta.blocked`, an earlier verdict). The session of an attempt is always fresh
//! ([`Engine::fresh_session`]): a persisted `revoked` is asked again before erasing.
//!
//! # Markers in `meta` (§7.9.2)
//!
//! * `leave_state`: `requested` (leaving), `deleting` (deleting the space), `wiping` (proof
//!   received, erasure running). `requested` / `deleting` are written, alone in their
//!   transaction, before the first network call; `wiping` is written by the erasing
//!   transaction itself, and only if `requested` / `deleting` is still there (a cancellation
//!   that won the race erases nothing).
//! * `departure_since`: time of the last gesture (decimal milliseconds). The automatic attempt
//!   ([`Engine::continue_departure`]) resends a request only within 15 minutes of it; later, or
//!   without it, it only verifies.
//! * `deletion_id`: 32 random bytes, drawn at the first "Delete space" and sent unchanged
//!   at every retry, so that a deletion whose `200` was lost is proven by the receipt. Kept by a
//!   cancellation, erased only by the erasure (or a new provisioning).
//!
//! In memory only: the last verdict of the relay (`Verdict`, in `Flags`) and why the last
//! attempt is pending. "Erase anyway" ([`Engine::erase_without_proof`], D4) is allowed only
//! if the last verdict, at most 10 minutes old, is "not found" or "relay too old".
//!
//! # Duration of an attempt
//!
//! Every call has its own limit: the challenge and the session 60 s each (connection 15 s,
//! `http.rs`), `DELETE /v1/me`, `DELETE /v1/space` and the receipt 20 s each (connection 10 s,
//! `DEPARTURE_TIMEOUTS`). A fresh session (T1) is a challenge then a session, and starts over
//! once on a `401`: at most 2 × (60 + 60) = 240 s. The worst case of an attempt adds up what the
//! tables chain, when every call runs into its limit:
//!
//! | Attempt | Chain | Worst case |
//! |---------|-------|-----------:|
//! | `leave_space`, `retry_departure` (Leave) | T1 240 + `DELETE /v1/me` 20 + (on `401`) T1 240 + `DELETE` 20 | 520 s |
//! | `delete_space` ending on `401` | receipt 20 + T1 240 + `DELETE /v1/space` 20 + T1 240 + `DELETE` 20 + receipt 20 | 560 s |
//! | `delete_space` ending on `409 not_last_device` | receipt 20 + T1 240 + `DELETE /v1/space` 20 + the 520 s of a departure | 800 s |
//! | `delete_space`: `401`, renewal, then `409 not_last_device` | receipt 20 + T1 240 + `DELETE` 20 + T1 240 + `DELETE` 20 + the 520 s of a departure | 1060 s |
//! | `continue_departure`, outside the window (verification) | (receipt 20 +) T1 240 (+ receipt 20) | 280 s |
//!
//! So an attempt may block for several minutes, never "a minute", and a caller that waits for
//! `departure_lock` adds the attempt in front of it. The erasure that follows a proof waits for
//! `sync_lock` first: a synchronisation already running (a file upload on a bad network, for
//! instance) adds its own duration, which this table does not bound; the erasure itself is
//! quick once that synchronisation is over. The marker is written before the first call, so
//! the departure is visible (and cancellable) during the whole wait.
//!
//! # Locks
//!
//! `departure_lock` serialises the attempts (and D4) from start to end; it does not block a
//! sync or a send. The three serialising locks (`sync`, `dispatch`, `transfer`) are taken only
//! by the erasure itself, never across a network call. [`Engine::cancel_departure`] takes none
//! of them: one short store transaction.
//!
//! # The erasure (§7.9.6)
//!
//! [`Engine::wipe_local`]: the decrypted cache first (choice of lot C5: it is emptied before the
//! guarded transaction; if that transaction then finds the departure cancelled, nothing is lost,
//! the cache is rebuilt on demand), then one transaction under `inner` that checks the marker
//! and erases every table, then the identity and the keys in memory, then the files and the
//! compaction, then the marker. A stop after the transaction is finished at the next opening
//! ([`Engine::finish_pending_wipe`]). The physical medium (flash) may keep freed blocks, and the
//! host's wrapping key is kept.

use std::sync::Arc;

use crate::error::{CoreError, Result};
use crate::runtime::engine::{lock, Active, Blocked, Engine, Verdict};
use crate::runtime::store::WipeGuard;
use crate::runtime::transport::{RelayError, TResult};

/// Key of the departure marker in `meta`.
pub(crate) const LEAVE_STATE: &str = "leave_state";
/// Key of the time of the last gesture, decimal milliseconds.
pub(crate) const DEPARTURE_SINCE: &str = "departure_since";
/// Key of the 32 raw bytes sent with `DELETE /v1/space`.
pub(crate) const DELETION_ID: &str = "deletion_id";
pub(crate) const REQUESTED: &[u8] = b"requested";
pub(crate) const DELETING: &[u8] = b"deleting";
pub(crate) const WIPING: &[u8] = b"wiping";

/// The automatic attempt resends a request only this long after the last gesture (§7.9.4).
const RESEND_WINDOW_MS: i64 = 15 * 60_000;
/// How long a verdict allows "Erase anyway" (§7.9.5).
const VERDICT_TTL_MS: i64 = 10 * 60_000;

/// Which departure is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartureKind {
    /// "Sign out": `DELETE /v1/me`.
    Leave,
    /// "Delete space" (last device): `DELETE /v1/space`.
    DeleteSpace,
}

/// Why a departure is still pending (nothing was erased).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingReason {
    /// Network error, timeout, truncated or unexpected body.
    Network,
    /// `5xx`, `503`, `429`.
    Server,
    /// `401` confirmed or `409 space_not_initialized`: "space not found". D4 may be offered.
    NotFound,
    /// Verification only (outside the 15-minute window): the device is still a member, the
    /// request had no effect.
    NotSent,
}

/// Why the relay refused a departure for good (the marker is removed, nothing was erased).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// `409 last_device`: the interface offers "Delete space" (D3).
    LastDevice,
    /// `update_required`, `format_unsupported`.
    UpdateRequired,
    /// `DELETE /v1/space` unknown to the relay (`404`, `405`). D4 may be offered.
    RelayTooOld,
    /// Any other `4xx`.
    Unexpected,
}

/// Result of a departure attempt. Every verdict of the relay is here; `Err` is kept for
/// `InvalidInput` (not provisioned, D4 not allowed, nothing to retry), `Storage` (the erasing
/// transaction failed: nothing was erased, the marker stays) and `Internal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartureOutcome {
    /// The relay proved it and everything was erased.
    Erased,
    /// Nothing was erased; the departure stays pending (visible, retried, cancellable).
    Pending(PendingReason),
    /// Nothing was erased; the departure is no longer pending.
    Refused(RefusalReason),
    /// Cancelled meanwhile: nothing was erased.
    Cancelled,
    /// No departure pending ([`Engine::continue_departure`] only).
    NothingPending,
}

/// The departure pending on this device, as the interface shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepartureView {
    /// `Leave` for a `wiping` marker (the kind is no longer known then).
    pub kind: DepartureKind,
    /// The proof came and the erasure runs (or a stop interrupted it): not cancellable.
    pub wiping: bool,
    /// `departure_since`; `None` for a marker written by an older version.
    pub since_ms: Option<i64>,
    /// Reason of the last attempt of this process, `None` before the first one.
    pub last: Option<PendingReason>,
}

/// What a fresh session said (T1 of §7.9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionProbe {
    Opened,
    Revoked,
    NotFound,
    UpdateRequired,
    Transient(PendingReason),
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Within 15 minutes of a gesture: the request may be (re)sent.
    Send,
    /// Later: receipt and fresh session only, never a `DELETE`.
    VerifyOnly,
}

/// What a step of an attempt decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Go on with the next table.
    Continue,
    /// A proof came: erase, if the marker is still there.
    Erase,
    Pending(PendingReason),
    Refused(RefusalReason),
    Cancelled,
}

/// The answer to a call of T2 / T3, after at most one session renewal on `401`.
enum Renewed {
    Answer(TResult<()>),
    /// The renewal (a fresh session, T1) stopped the attempt.
    Session(Step),
    /// `401` again after a fresh session.
    Unrecognised,
}

impl Engine {
    // -----------------------------------------------------------------------
    // Gestures and automatic attempts
    // -----------------------------------------------------------------------

    /// Gesture "Sign out": writes `requested` and `departure_since`, then one attempt
    /// (T1, T2 of `api-v1.md` §7.9.3).
    ///
    /// **Blocks** for the whole attempt: up to 520 s (about 9 minutes) on a network that
    /// answers nothing in time (see "Duration of an attempt" in the module documentation), not
    /// "a minute", plus the wait for the departure lock if another attempt is running. Call it
    /// off the main thread; the pending departure is visible meanwhile
    /// ([`departure`](Self::departure)) and [`cancel_departure`](Self::cancel_departure) does
    /// not wait for it.
    pub fn leave_space(&self) -> Result<DepartureOutcome> {
        self.gesture(DepartureKind::Leave)
    }

    /// Gesture "Delete space" (after `Refused(LastDevice)` and its confirmation): writes
    /// `deleting`, `departure_since` and `deletion_id` if absent, then one attempt (T4, T1, T3).
    /// **Blocks** for the whole attempt: up to 1060 s (about 18 minutes) in the worst case, see
    /// [`leave_space`](Self::leave_space).
    pub fn delete_space(&self) -> Result<DepartureOutcome> {
        self.gesture(DepartureKind::DeleteSpace)
    }

    /// Gesture "Retry": refreshes `departure_since`, then one attempt of the pending kind
    /// (the request is sent again). `InvalidInput` if no departure is pending.
    pub fn retry_departure(&self) -> Result<DepartureOutcome> {
        self.active()?;
        let _departure = self.serial(&self.departure_lock);
        let active = self.active()?;
        let now = self.clock.now_ms();
        let kind = self.live_tx(&active, |db| {
            let kind = match db.meta_get(LEAVE_STATE)?.as_deref() {
                Some(REQUESTED) => DepartureKind::Leave,
                Some(DELETING) => DepartureKind::DeleteSpace,
                _ => return Err(CoreError::input("no departure is pending")),
            };
            db.meta_set(DEPARTURE_SINCE, now.to_string().as_bytes())?;
            Ok(kind)
        })?;
        self.notify();
        self.attempt(&active, kind, Mode::Send)
    }

    /// Automatic attempt (start-up, back-off, network back): one attempt if the last gesture
    /// is less than 15 minutes old, verification only otherwise (§7.9.4: receipt, then a fresh
    /// session; never a `DELETE`). `NothingPending` when no departure is pending, including on a
    /// device that is not provisioned (a leftover marker is removed). Call it at start-up,
    /// before `resume_pairing`.
    pub fn continue_departure(&self) -> Result<DepartureOutcome> {
        let _departure = self.serial(&self.departure_lock);
        let marker = self.store.read(|db| db.meta_get(LEAVE_STATE))?;
        let Ok(active) = self.active() else {
            if matches!(marker.as_deref(), Some(REQUESTED | DELETING)) {
                self.clear_departure()?;
            }
            return Ok(DepartureOutcome::NothingPending);
        };
        let kind = match marker.as_deref() {
            Some(REQUESTED) => DepartureKind::Leave,
            Some(DELETING) => DepartureKind::DeleteSpace,
            _ => return Ok(DepartureOutcome::NothingPending),
        };
        let now = self.clock.now_ms();
        let mode = match self.departure_since()? {
            Some(t) if now >= t && now - t < RESEND_WINDOW_MS => Mode::Send,
            // Old, from the future (clock moved back), or written by an older version.
            _ => Mode::VerifyOnly,
        };
        self.attempt(&active, kind, mode)
    }

    /// "Annuler": removes `requested` / `deleting` at once, without waiting for an attempt in
    /// progress (whose proof will then erase nothing). Refused in `wiping` (`Ok(false)`).
    /// `deletion_id` is kept. `Ok(true)` if a departure was cancelled.
    pub fn cancel_departure(&self) -> Result<bool> {
        let cancelled = self
            .store
            .tx(|db| match db.meta_get(LEAVE_STATE)?.as_deref() {
                Some(REQUESTED | DELETING) => {
                    db.meta_delete(LEAVE_STATE)?;
                    db.meta_delete(DEPARTURE_SINCE)?;
                    Ok(true)
                }
                _ => Ok(false),
            })?;
        if cancelled {
            if let Ok(mut f) = lock(&self.flags) {
                f.departure_last = None;
            }
            self.notify();
        }
        Ok(cancelled)
    }

    /// The departure pending on this device, `None` when none.
    pub fn departure(&self) -> Option<DepartureView> {
        let (marker, since) = self
            .store
            .read(|db| {
                Ok((
                    db.meta_get(LEAVE_STATE)?,
                    db.meta_get_string(DEPARTURE_SINCE)?,
                ))
            })
            .ok()?;
        let (kind, wiping) = match marker.as_deref()? {
            REQUESTED => (DepartureKind::Leave, false),
            DELETING => (DepartureKind::DeleteSpace, false),
            WIPING => (DepartureKind::Leave, true),
            _ => return None,
        };
        let last = if wiping {
            None
        } else {
            lock(&self.flags).ok().and_then(|f| f.departure_last)
        };
        Some(DepartureView {
            kind,
            wiping,
            since_ms: since.and_then(|s| s.parse().ok()),
            last,
        })
    }

    /// Whether "Erase anyway" (D4) is allowed now: the last verdict of the relay, obtained
    /// by this process at most 10 minutes ago, is "not found" or "relay too old" (§7.9.5).
    /// Never after a mere network error, timeout, `5xx` or `429`.
    pub fn can_erase_without_proof(&self) -> bool {
        if !self.is_provisioned() {
            return false;
        }
        let now = self.clock.now_ms();
        lock(&self.flags)
            .ok()
            .and_then(|f| f.verdict)
            .is_some_and(|(v, at)| {
                matches!(v, Verdict::NotFound | Verdict::RelayTooOld)
                    && now >= at
                    && now - at <= VERDICT_TTL_MS
            })
    }

    /// "Erase anyway" (D4): the same erasure, **without any call to the relay**. The
    /// device does not leave anything at the relay: if the space still exists, it stays a
    /// member there that nobody can reach. `InvalidInput` unless
    /// [`can_erase_without_proof`](Self::can_erase_without_proof); `Storage`: nothing erased.
    ///
    /// **The caller must have made the user pass a strong biometric check first**, every time
    /// (same scope as revoking another device), followed by an honest irreversible
    /// confirmation: this call destroys all the data of the device on the word of whoever calls
    /// it, and the core has no way to check who that is. It waits for the departure lock, so
    /// it may wait for an attempt in progress (see "Duration of an attempt" in the module
    /// documentation); the erasure itself is quick.
    pub fn erase_without_proof(&self) -> Result<()> {
        self.active()?;
        let _departure = self.serial(&self.departure_lock);
        let active = self.active()?;
        if !self.can_erase_without_proof() {
            return Err(CoreError::input(
                "erasing without proof needs a recent answer of the relay",
            ));
        }
        if self.wipe_local(&active, WipeGuard::Unconditional)? {
            Ok(())
        } else {
            Err(CoreError::input("device is not provisioned"))
        }
    }

    /// Whether `requested` or `deleting` is pending.
    pub(crate) fn departure_pending(&self) -> bool {
        self.store
            .read(|db| db.meta_get(LEAVE_STATE))
            .ok()
            .flatten()
            .is_some_and(|m| m == REQUESTED || m == DELETING)
    }

    fn gesture(&self, kind: DepartureKind) -> Result<DepartureOutcome> {
        self.active()?;
        let _departure = self.serial(&self.departure_lock);
        // Erased while this call waited for the previous attempt.
        let active = self.active()?;
        let now = self.clock.now_ms();
        self.live_tx(&active, |db| {
            if db.meta_get(LEAVE_STATE)?.as_deref() == Some(WIPING) {
                return Err(CoreError::input("an erasure is running"));
            }
            let marker = match kind {
                DepartureKind::Leave => REQUESTED,
                DepartureKind::DeleteSpace => DELETING,
            };
            db.meta_set(LEAVE_STATE, marker)?;
            db.meta_set(DEPARTURE_SINCE, now.to_string().as_bytes())?;
            if kind == DepartureKind::DeleteSpace && !has_deletion_id(db)? {
                db.meta_set(DELETION_ID, &crate::crypto::random_array::<32>()?)?;
            }
            Ok(())
        })?;
        self.notify();
        // A gesture always sends the request.
        self.attempt(&active, kind, Mode::Send)
    }

    // -----------------------------------------------------------------------
    // One attempt (§7.9.3)
    // -----------------------------------------------------------------------

    /// One attempt; the caller holds `departure_lock`.
    fn attempt(
        &self,
        active: &Arc<Active>,
        kind: DepartureKind,
        mode: Mode,
    ) -> Result<DepartureOutcome> {
        let step = match kind {
            DepartureKind::Leave => self.attempt_leave(active, mode),
            DepartureKind::DeleteSpace => self.attempt_delete(active, mode)?,
        };
        self.conclude(active, step)
    }

    /// `requested`: T1, then T2.
    fn attempt_leave(&self, active: &Arc<Active>, mode: Mode) -> Step {
        match self.t1(active) {
            Step::Continue => {}
            other => return other,
        }
        if mode == Mode::VerifyOnly {
            return Step::Pending(PendingReason::NotSent);
        }
        match self.with_renewal(active, || self.transport.leave_space()) {
            Renewed::Answer(Ok(())) => {
                // The device is out. If the erasure finds the departure cancelled meanwhile,
                // it stays revoked with its data readable (§7.9.2).
                self.set_verdict(Verdict::Other);
                self.set_blocked(active, Blocked::Revoked);
                Step::Erase
            }
            Renewed::Answer(Err(e)) => self.classify(active, &e).unwrap_or_else(|| match &e {
                RelayError::Api(p) if p.status == 409 && p.code == "last_device" => {
                    Step::Refused(RefusalReason::LastDevice)
                }
                _ => Step::Refused(RefusalReason::Unexpected),
            }),
            Renewed::Session(step) => step,
            Renewed::Unrecognised => {
                self.set_not_found(active);
                Step::Pending(PendingReason::NotFound)
            }
        }
    }

    /// `deleting`: T4 (receipt), T1, then T3 (and T4 again after `401` / `409
    /// space_not_initialized`).
    fn attempt_delete(&self, active: &Arc<Active>, mode: Mode) -> Result<Step> {
        let id = self.deletion_id(active)?;
        if self.receipt_proves(&id) {
            return Ok(self.space_deleted(active));
        }
        let not_found = || {
            // The space may be gone and its `200` lost: the receipt says so (T3, `401` row).
            if self.receipt_proves(&id) {
                self.space_deleted(active)
            } else {
                Step::Pending(PendingReason::NotFound)
            }
        };
        match self.t1(active) {
            Step::Continue => {}
            // Not in the table T1 itself, but only more proof-seeking: the receipt of the start
            // may have met a passing failure (choice of lot C, reported).
            Step::Pending(PendingReason::NotFound) => return Ok(not_found()),
            other => return Ok(other),
        }
        if mode == Mode::VerifyOnly {
            return Ok(Step::Pending(PendingReason::NotSent));
        }
        Ok(
            match self.with_renewal(active, || self.transport.delete_space(&id)) {
                Renewed::Answer(Ok(())) => self.space_deleted(active),
                Renewed::Answer(Err(RelayError::Api(p)))
                    if p.status == 409 && p.code == "not_last_device" =>
                {
                    // Not the last device after all: back to an ordinary departure, in the same
                    // transaction, then one attempt T1 + T2 in this same call.
                    let switched = self.live_tx(active, |db| {
                        if db.meta_get(LEAVE_STATE)?.as_deref() != Some(DELETING) {
                            return Ok(false);
                        }
                        db.meta_set(LEAVE_STATE, REQUESTED)?;
                        Ok(true)
                    })?;
                    if !switched {
                        return Ok(Step::Cancelled);
                    }
                    self.notify();
                    self.attempt_leave(active, Mode::Send)
                }
                Renewed::Answer(Err(RelayError::Api(p))) if p.code == "space_not_initialized" => {
                    self.set_not_found(active);
                    not_found()
                }
                Renewed::Answer(Err(RelayError::Api(p))) if p.status == 404 || p.status == 405 => {
                    self.set_verdict(Verdict::RelayTooOld);
                    Step::Refused(RefusalReason::RelayTooOld)
                }
                Renewed::Answer(Err(e)) => self
                    .classify(active, &e)
                    .unwrap_or(Step::Refused(RefusalReason::Unexpected)),
                Renewed::Session(Step::Pending(PendingReason::NotFound))
                | Renewed::Unrecognised => {
                    self.set_not_found(active);
                    not_found()
                }
                Renewed::Session(step) => step,
            },
        )
    }

    /// T1: a fresh session, the state kept locally being ignored.
    fn t1(&self, active: &Arc<Active>) -> Step {
        match self.fresh_session(active) {
            SessionProbe::Opened => Step::Continue,
            SessionProbe::Revoked => Step::Erase,
            SessionProbe::NotFound => Step::Pending(PendingReason::NotFound),
            SessionProbe::UpdateRequired => Step::Refused(RefusalReason::UpdateRequired),
            SessionProbe::Transient(r) => Step::Pending(r),
            SessionProbe::Unexpected => Step::Refused(RefusalReason::Unexpected),
        }
    }

    /// Calls `f`; on a `401`, opens a fresh session (T1) and calls it once more.
    fn with_renewal(&self, active: &Arc<Active>, f: impl Fn() -> TResult<()>) -> Renewed {
        match f() {
            Err(RelayError::Api(p)) if p.status == 401 => match self.t1(active) {
                Step::Continue => match f() {
                    Err(RelayError::Api(p)) if p.status == 401 => Renewed::Unrecognised,
                    other => Renewed::Answer(other),
                },
                step => Renewed::Session(step),
            },
            other => Renewed::Answer(other),
        }
    }

    /// The rows T2 and T3 share; `None` for the rows proper to each (last device, not last
    /// device, relay too old, another `4xx`).
    fn classify(&self, active: &Arc<Active>, e: &RelayError) -> Option<Step> {
        match e {
            RelayError::Network(_) => {
                // Includes a `2xx` without its proof (`seq`, `space_deleted`), a truncated body.
                self.set_verdict(Verdict::Transient);
                Some(Step::Pending(PendingReason::Network))
            }
            RelayError::Api(p) if p.status == 429 || p.status >= 500 => {
                self.set_verdict(Verdict::Transient);
                Some(Step::Pending(PendingReason::Server))
            }
            RelayError::Api(p) if p.status == 403 && p.code == "revoked" => {
                // A proof of departure (Q3: also during "Delete space"). If the erasure
                // then finds the departure cancelled, the device stays revoked, data readable.
                self.set_verdict(Verdict::Other);
                self.set_blocked(active, Blocked::Revoked);
                Some(Step::Erase)
            }
            RelayError::Api(p) if p.status == 409 && p.code == "space_not_initialized" => {
                self.set_not_found(active);
                Some(Step::Pending(PendingReason::NotFound))
            }
            RelayError::Api(p)
                if ((p.status == 409 || p.status == 410) && p.code == "update_required")
                    || (p.status == 409 && p.code == "format_unsupported") =>
            {
                self.set_verdict(Verdict::Other);
                Some(Step::Refused(RefusalReason::UpdateRequired))
            }
            RelayError::Api(_) => {
                self.set_verdict(Verdict::Other);
                None
            }
        }
    }

    /// The deletion of the space is proven: erase. If the erasure finds the departure cancelled
    /// meanwhile, the device stays not found, with its data readable (§7.9.2).
    fn space_deleted(&self, active: &Arc<Active>) -> Step {
        self.set_not_found(active);
        Step::Erase
    }

    /// T4: only `200 {"space_deleted": true}` counts; any other answer is ignored.
    fn receipt_proves(&self, id: &[u8; 32]) -> bool {
        self.transport.space_deletion_receipt(id).is_ok()
    }

    /// A fresh session (`api-v1.md` §7.9.3 T1): the session and the state kept locally
    /// (`Revoked`, `NotFound`) are ignored, a new challenge is signed, and a `401` is retried
    /// once with another challenge. Updates `blocked` (`Revoked` persisted, `NotFound` not) and
    /// the verdict.
    pub(crate) fn fresh_session(&self, active: &Arc<Active>) -> SessionProbe {
        self.drop_session();
        for attempt in 0..2 {
            let p = match self.open_session(active) {
                Ok(()) => return SessionProbe::Opened,
                Err(RelayError::Network(_)) => {
                    self.set_verdict(Verdict::Transient);
                    return SessionProbe::Transient(PendingReason::Network);
                }
                Err(RelayError::Api(p)) => p,
            };
            match (p.status, p.code.as_str()) {
                (401, _) if attempt == 0 => {}
                (401, _) | (409, "space_not_initialized") => {
                    self.set_not_found(active);
                    return SessionProbe::NotFound;
                }
                (403, "revoked") => {
                    self.set_verdict(Verdict::Other);
                    self.set_blocked(active, Blocked::Revoked);
                    return SessionProbe::Revoked;
                }
                (409 | 410, "update_required") => {
                    self.set_verdict(Verdict::Other);
                    self.set_blocked(
                        active,
                        Blocked::UpdateRequired {
                            update_url: p.update_url.clone(),
                        },
                    );
                    return SessionProbe::UpdateRequired;
                }
                (409, "format_unsupported") => {
                    self.set_verdict(Verdict::Other);
                    self.set_blocked(
                        active,
                        Blocked::FormatUnsupported {
                            limited_by: p.limited_by.clone(),
                        },
                    );
                    return SessionProbe::UpdateRequired;
                }
                (s, _) if s == 429 || s >= 500 => {
                    self.set_verdict(Verdict::Transient);
                    return SessionProbe::Transient(PendingReason::Server);
                }
                _ => {
                    self.set_verdict(Verdict::Other);
                    return SessionProbe::Unexpected;
                }
            }
        }
        SessionProbe::NotFound
    }

    /// Ends an attempt: erases on a proof (if the marker is still there), drops the marker on a
    /// refusal, reports a cancellation that happened meanwhile.
    fn conclude(&self, active: &Arc<Active>, step: Step) -> Result<DepartureOutcome> {
        let outcome = match step {
            Step::Erase => {
                if self.wipe_local(active, WipeGuard::DepartureMarker)? {
                    DepartureOutcome::Erased
                } else {
                    DepartureOutcome::Cancelled
                }
            }
            _ if !self.departure_pending() => DepartureOutcome::Cancelled,
            Step::Refused(r) => {
                self.live_tx(active, |db| {
                    db.meta_delete(LEAVE_STATE)?;
                    db.meta_delete(DEPARTURE_SINCE)
                })?;
                DepartureOutcome::Refused(r)
            }
            Step::Pending(r) => DepartureOutcome::Pending(r),
            Step::Cancelled => DepartureOutcome::Cancelled,
            // Every path ends on another step; nothing proven, nothing refused.
            Step::Continue => DepartureOutcome::Pending(PendingReason::Network),
        };
        if let Ok(mut f) = lock(&self.flags) {
            f.departure_last = match outcome {
                DepartureOutcome::Pending(r) => Some(r),
                _ => None,
            };
        }
        self.notify();
        Ok(outcome)
    }

    // -----------------------------------------------------------------------
    // The erasure (§7.9.6)
    // -----------------------------------------------------------------------

    /// The erasure. `Ok(false)`: the guard failed (departure cancelled meanwhile), nothing was
    /// erased. `Err(Storage)`: the transaction failed, nothing was erased, the marker stays and
    /// nothing is forgotten in memory. `InvalidInput`: `active` is no longer the live identity.
    pub(crate) fn wipe_local(&self, active: &Arc<Active>, guard: WipeGuard) -> Result<bool> {
        // 0. A running sync, send or transfer ends first; the next ones wait for the erasure.
        let _sync = self.serial(&self.sync_lock);
        let _dispatch = self.serial(&self.dispatch_lock);
        let _transfer = self.serial(&self.transfer_lock);
        if !self.is_live(active) {
            return Err(CoreError::input("device is not provisioned"));
        }
        // 1. The decrypted copies (before the guard: see the module header).
        let _ = self.purge_plaintext_cache();
        // 2. and 3. One transaction under `inner` (no `live_tx` can interleave), then the
        // identity and the keys in memory.
        {
            let mut inner = lock(&self.inner)?;
            if !inner
                .active
                .as_ref()
                .is_some_and(|now| Arc::ptr_eq(now, active))
            {
                return Err(CoreError::input("device is not provisioned"));
            }
            if !self.store.wipe_everything(guard)? {
                return Ok(false);
            }
            self.forget_identity(&mut inner);
        }
        self.transport.set_session_token(None);
        // The relay of the space is forgotten with the rest: back to the default of the build.
        self.restore_base_relay();
        // 4. to 6. A failure here is not the user's problem: the marker stays `wiping` and the
        // next opening finishes the files and the compaction.
        if self.store.wipe_files().is_ok() && self.store.compact().is_ok() {
            let _ = self.store.tx(|db| db.meta_delete(LEAVE_STATE));
        }
        self.notify();
        Ok(true)
    }

    /// Finishes an erasure whose files or compaction a stop interrupted
    /// (`leave_state = wiping`), and drops a `requested` / `deleting` marker left on a device
    /// that is not provisioned. Nothing is erased on a provisioned device: a `wiping` marker
    /// found there is a leftover, and is only removed.
    ///
    /// The compaction is mandatory (it removes the sealed records from the database files);
    /// the files are best effort when this runs before a new provisioning (a file still open
    /// elsewhere, on Windows, must not prevent a new pairing): the marker then stays until the
    /// next successful attempt, or until the new provisioning removes it.
    pub(crate) fn finish_pending_wipe(&self) -> Result<()> {
        let marker = self.store.read(|db| db.meta_get(LEAVE_STATE))?;
        match marker.as_deref() {
            Some(WIPING) if self.is_provisioned() => self.clear_departure(),
            Some(WIPING) => {
                let files = self.store.wipe_files();
                self.store.compact()?;
                if files.is_ok() {
                    self.clear_departure()?;
                }
                Ok(())
            }
            Some(_) if !self.is_provisioned() => self.clear_departure(),
            _ => Ok(()),
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn is_live(&self, active: &Arc<Active>) -> bool {
        lock(&self.inner).is_ok_and(|i| i.active.as_ref().is_some_and(|a| Arc::ptr_eq(a, active)))
    }

    fn departure_since(&self) -> Result<Option<i64>> {
        Ok(self
            .store
            .read(|db| db.meta_get_string(DEPARTURE_SINCE))?
            .and_then(|s| s.parse().ok()))
    }

    /// The `deletion_id` of this device, drawn now if a `deleting` marker came without one.
    fn deletion_id(&self, active: &Arc<Active>) -> Result<[u8; 32]> {
        self.live_tx(active, |db| {
            if let Some(id) = db
                .meta_get(DELETION_ID)?
                .and_then(|v| <[u8; 32]>::try_from(v).ok())
            {
                return Ok(id);
            }
            let id = crate::crypto::random_array::<32>()?;
            db.meta_set(DELETION_ID, &id)?;
            Ok(id)
        })
    }

    fn clear_departure(&self) -> Result<()> {
        self.store.tx(|db| {
            db.meta_delete(LEAVE_STATE)?;
            db.meta_delete(DEPARTURE_SINCE)
        })
    }
}

fn has_deletion_id(db: &crate::runtime::store::Db<'_>) -> Result<bool> {
    Ok(db.meta_get(DELETION_ID)?.is_some_and(|v| v.len() == 32))
}
