//! The departure of the demo core: a simulated relay that answers the way tables T1 to T4 of
//! `spec/api-v1.md` 7.9.3 describe, and the attempt logic of the rewritten engine
//! on top of it.
//!
//! It exists so that the window can be tried in the demo mode and so that every row of the
//! tables (proof, `last_device`, `not_last_device`, network, not found, relay too old, cancel
//! during an attempt, restart) can be tested. Nothing here is encrypted, sent or kept.
//!
//! The rules it keeps, which the real core keeps too:
//! * the marker (`requested` / `deleting`) is written before the first request and stays until
//!   a proof, a final refusal or "Annuler";
//! * only a proof erases: `Reply::Ok` on the right step, `Reply::Revoked`, or a receipt. A `401`
//!   or a `space_not_initialized` never does, whatever the persisted state;
//! * the erasure checks the marker again (an "Annuler" that came first wins);
//! * "Effacer quand même" is allowed only after a recent "not found" or "relay too old" verdict.

use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::Arc;
use std::sync::MutexGuard;

use super::FakeCore;
use crate::core_api::{
    CoreError, CoreEvent, DepartureKind, DepartureOutcome, DepartureView, PendingReason,
    RefusalReason, SpaceState,
};

/// A request is sent again automatically only this long after the last gesture (7.9.4).
const RESEND_WINDOW_MS: i64 = 15 * 60_000;
/// The verdict that allows "Effacer quand même" lives this long (7.9.5).
const VERDICT_TTL_MS: i64 = 10 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    Requested,
    Deleting,
    /// A proof was received and the erasure is under way. The fake erases at once, so this is
    /// only ever seen after a simulated interruption (`reopen`).
    Wiping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Member,
    NotFound,
    RelayTooOld,
    Transient,
}

/// What the simulated relay answers to one request: the rows of tables T1 to T4. Most of the
/// replies are only scripted by the tests.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// `200` that proves what the step asks (session opened, `seq`, `"space_deleted": true`,
    /// receipt found).
    Ok,
    /// `2xx` without the proof field, or with another value: never a proof.
    OkNoProof,
    /// `403 revoked`.
    Revoked,
    /// `409 last_device`.
    LastDevice,
    /// `409 not_last_device`.
    NotLastDevice,
    /// `401`, still `401` on a second fresh challenge.
    Unauthorized,
    /// `409 space_not_initialized`.
    SpaceNotInitialized,
    /// `409 update_required`, `409 format_unsupported`, `410 update_required`.
    UpdateRequired,
    /// `404` / `405`: no such route (relay too old), or no receipt (T4).
    NotFound404,
    /// `5xx`, `429`.
    Server,
    /// Network error, timeout, truncated body.
    Network,
    /// The request reached the relay and took effect, but the answer was lost.
    LostAfterEffect,
    /// Any other `4xx`.
    Unexpected4xx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `POST /v1/auth/challenge` + `POST /v1/auth/session` (T1).
    Session,
    /// `DELETE /v1/me` (T2).
    LeaveMe,
    /// `DELETE /v1/space` (T3).
    DeleteSpace,
    /// `POST /v1/space/receipt` (T4).
    Receipt,
}

/// What the simulated relay answers when nothing is scripted (the demo and the tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RelayMode {
    /// A healthy relay holding this space.
    #[default]
    Normal,
    /// No network: every request fails.
    Offline,
    /// The relay is up but failing (`503`).
    Server,
    /// `tomi-relay admin reset` was done by hand: the space is gone, no receipt exists.
    Reset,
    /// A relay without `DELETE /v1/space` and without the receipt route.
    OldRelay,
}

impl std::str::FromStr for RelayMode {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "normal" => Ok(Self::Normal),
            "offline" => Ok(Self::Offline),
            "server" => Ok(Self::Server),
            "reset" => Ok(Self::Reset),
            "oldRelay" => Ok(Self::OldRelay),
            _ => Err(()),
        }
    }
}

enum SessionProbe {
    Opened,
    Revoked,
    NotFound,
    UpdateRequired,
    Transient(PendingReason),
    Unexpected,
}

/// A stop for the next attempt, for the tests (see `FakeCore::hold_next_attempt`).
#[cfg(test)]
#[derive(Default)]
pub struct Hold {
    /// Notified by the attempt when it has got to the stop.
    pub entered: tokio::sync::Notify,
    /// Notified by the test to let the attempt go on.
    pub release: tokio::sync::Notify,
}

#[derive(Default)]
pub(super) struct Sim {
    marker: Option<Marker>,
    /// Which gesture the marker comes from (kept through `wiping`).
    kind: Option<DepartureKind>,
    /// `departure_since`.
    since_ms: Option<i64>,
    /// A `deletion_id` exists (kept by a cancel, removed by an erasure).
    deletion_id: bool,
    /// Reason of the last attempt of this process.
    last: Option<PendingReason>,
    verdict: Option<(Verdict, i64)>,
    space: Option<SpaceState>,
    pub(super) erased: bool,
    /// The departure is not offered (tests of the window with a core that cannot do it).
    unavailable: bool,
    // The simulated relay.
    mode: RelayMode,
    script: Vec<(Step, Reply)>,
    /// This PC was removed from the space at the relay (`DELETE /v1/me` took effect).
    left_relay: bool,
    /// The space was deleted at the relay (a receipt exists).
    space_deleted: bool,
    #[cfg(test)]
    hook: Option<Box<dyn FnOnce() + Send>>,
    /// Like the real core: an "Annuler" with no marker yet, while a gesture is on its way, is
    /// promised (and carried out when the marker appears).
    #[cfg(test)]
    promise_without_marker: bool,
}

impl Sim {
    pub(super) fn space_state(&self) -> SpaceState {
        self.space.unwrap_or(SpaceState::Active)
    }
}

impl FakeCore {
    fn sim(&self) -> MutexGuard<'_, Sim> {
        self.dep.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The clock of the core: the real one, plus what the tests moved it by.
    fn clock_ms(&self) -> i64 {
        crate::model::now_ms() + self.skew_ms.load(Ordering::SeqCst)
    }

    /// What the simulated relay does when nothing is scripted for the next request
    /// (`RelayMode`). The demo window offers it (`demo_departure`).
    pub fn simulate_relay(&self, mode: RelayMode) {
        self.sim().mode = mode;
    }

    /// The demo space has only this PC left: the next "Se déconnecter" is refused with
    /// `last_device`, which leads to "Supprimer l'espace".
    pub fn simulate_last_device(&self) {
        self.others_lock().clear();
        self.emit(CoreEvent::Changed);
    }

    // ---------------------------------------------------------------- the simulated relay

    /// The answer to one request, and its effect at the relay (a lost answer still has its
    /// effect).
    fn reply(&self, step: Step) -> Reply {
        let members = self.others_lock().len() + 1;
        let mut sim = self.sim();
        let scripted = sim
            .script
            .iter()
            .position(|(s, _)| *s == step)
            .map(|i| sim.script.remove(i).1);
        let reply = scripted.unwrap_or_else(|| default_reply(&sim, step, members));
        let took_effect = matches!(reply, Reply::Ok | Reply::LostAfterEffect);
        match step {
            Step::LeaveMe if took_effect => sim.left_relay = true,
            Step::DeleteSpace if took_effect => sim.space_deleted = true,
            _ => {}
        }
        reply
    }

    /// A test acts while a request is in flight (between its effect and its answer).
    fn in_flight(&self) {
        #[cfg(test)]
        {
            let hook = self.sim().hook.take();
            if let Some(hook) = hook {
                hook();
            }
        }
    }

    fn note_verdict(&self, verdict: Verdict) {
        let now = self.clock_ms();
        let mut sim = self.sim();
        sim.verdict = Some((verdict, now));
        match verdict {
            Verdict::NotFound => sim.space = Some(SpaceState::NotFound),
            Verdict::Member => {
                if sim.space == Some(SpaceState::NotFound) {
                    sim.space = None;
                }
            }
            Verdict::RelayTooOld | Verdict::Transient => {}
        }
    }

    /// T1: a fresh session, whatever the persisted state says.
    fn session(&self) -> SessionProbe {
        match self.reply(Step::Session) {
            Reply::Ok => {
                self.note_verdict(Verdict::Member);
                SessionProbe::Opened
            }
            Reply::Revoked => {
                self.note_verdict(Verdict::Member);
                SessionProbe::Revoked
            }
            Reply::Unauthorized | Reply::SpaceNotInitialized => {
                self.note_verdict(Verdict::NotFound);
                SessionProbe::NotFound
            }
            Reply::UpdateRequired => SessionProbe::UpdateRequired,
            Reply::Server => {
                self.note_verdict(Verdict::Transient);
                SessionProbe::Transient(PendingReason::Server)
            }
            Reply::Network | Reply::LostAfterEffect | Reply::OkNoProof => {
                self.note_verdict(Verdict::Transient);
                SessionProbe::Transient(PendingReason::Network)
            }
            Reply::LastDevice
            | Reply::NotLastDevice
            | Reply::NotFound404
            | Reply::Unexpected4xx => SessionProbe::Unexpected,
        }
    }

    // ---------------------------------------------------------------- the attempt

    fn drop_marker(&self) {
        let mut sim = self.sim();
        if matches!(sim.marker, Some(Marker::Requested | Marker::Deleting)) {
            sim.marker = None;
        }
    }

    fn refuse(&self, reason: RefusalReason) -> DepartureOutcome {
        self.drop_marker();
        DepartureOutcome::Refused { reason }
    }

    fn pending(&self, reason: PendingReason) -> DepartureOutcome {
        DepartureOutcome::Pending { reason }
    }

    /// The erasure, guarded by the marker (C5): `Cancelled` if "Annuler" came first. `proof`
    /// is the space state to show when it did (the proof itself says what became of the space).
    fn wipe(&self, if_cancelled: SpaceState) -> DepartureOutcome {
        {
            let mut sim = self.sim();
            if !matches!(sim.marker, Some(Marker::Requested | Marker::Deleting)) {
                sim.space = Some(if_cancelled);
                drop(sim);
                self.emit(CoreEvent::Changed);
                return DepartureOutcome::Cancelled;
            }
            sim.marker = Some(Marker::Wiping);
        }
        self.finish_wipe();
        DepartureOutcome::Erased
    }

    /// Steps 3 to 6 of 7.9.6: everything held is forgotten, files included.
    fn finish_wipe(&self) {
        let gone: Vec<String> = {
            let mut items = self.lock();
            items.drain(..).map(|i| i.id).collect()
        };
        let _ = std::fs::remove_dir_all(&self.dir);
        self.others_lock().clear();
        {
            let mut sim = self.sim();
            sim.marker = None;
            sim.since_ms = None;
            sim.deletion_id = false;
            sim.last = None;
            sim.verdict = None;
            sim.space = None;
            sim.erased = true;
        }
        for id in gone {
            self.emit(CoreEvent::ItemRemoved(id));
        }
        self.emit(CoreEvent::Changed);
    }

    /// One attempt of the pending departure. `resend`: the request itself may be sent (a
    /// gesture, or an automatic attempt inside the 15 minutes); otherwise it only verifies.
    fn attempt(&self, resend: bool) -> DepartureOutcome {
        let marker = self.sim().marker;
        let outcome = match marker {
            Some(Marker::Requested) => self.attempt_requested(resend),
            Some(Marker::Deleting) => self.attempt_deleting(resend),
            Some(Marker::Wiping) => {
                self.finish_wipe();
                DepartureOutcome::Erased
            }
            None => DepartureOutcome::NothingPending,
        };
        if let DepartureOutcome::Pending { reason } = outcome {
            self.sim().last = Some(reason);
        }
        self.emit(CoreEvent::Changed);
        outcome
    }

    /// `requested`: T1 then T2.
    fn attempt_requested(&self, resend: bool) -> DepartureOutcome {
        match self.session() {
            SessionProbe::Revoked => return self.wipe(SpaceState::Revoked),
            SessionProbe::NotFound => return self.pending(PendingReason::NotFound),
            SessionProbe::UpdateRequired => return self.refuse(RefusalReason::UpdateRequired),
            SessionProbe::Transient(r) => return self.pending(r),
            SessionProbe::Unexpected => return self.refuse(RefusalReason::Unexpected),
            SessionProbe::Opened => {}
        }
        if !resend {
            return self.pending(PendingReason::NotSent);
        }
        let reply = self.reply(Step::LeaveMe);
        self.in_flight();
        match reply {
            Reply::Ok | Reply::Revoked => self.wipe(SpaceState::Revoked),
            Reply::OkNoProof | Reply::Network | Reply::LostAfterEffect => {
                self.pending(PendingReason::Network)
            }
            Reply::LastDevice => self.refuse(RefusalReason::LastDevice),
            Reply::Unauthorized | Reply::SpaceNotInitialized => {
                self.note_verdict(Verdict::NotFound);
                self.pending(PendingReason::NotFound)
            }
            Reply::UpdateRequired => self.refuse(RefusalReason::UpdateRequired),
            Reply::Server => self.pending(PendingReason::Server),
            Reply::NotLastDevice | Reply::NotFound404 | Reply::Unexpected4xx => {
                self.refuse(RefusalReason::Unexpected)
            }
        }
    }

    /// T4: the receipt of a deletion whose answer may have been lost.
    fn receipt_proves_deletion(&self) -> bool {
        self.reply(Step::Receipt) == Reply::Ok
    }

    /// `deleting`: T4, T1, T3 (and T4 again after a `401` / `space_not_initialized`).
    fn attempt_deleting(&self, resend: bool) -> DepartureOutcome {
        if self.receipt_proves_deletion() {
            return self.wipe(SpaceState::NotFound);
        }
        match self.session() {
            SessionProbe::Revoked => return self.wipe(SpaceState::Revoked),
            SessionProbe::NotFound => return self.pending(PendingReason::NotFound),
            SessionProbe::UpdateRequired => return self.refuse(RefusalReason::UpdateRequired),
            SessionProbe::Transient(r) => return self.pending(r),
            SessionProbe::Unexpected => return self.refuse(RefusalReason::Unexpected),
            SessionProbe::Opened => {}
        }
        if !resend {
            return self.pending(PendingReason::NotSent);
        }
        let reply = self.reply(Step::DeleteSpace);
        self.in_flight();
        match reply {
            Reply::Ok => self.wipe(SpaceState::NotFound),
            Reply::Revoked => self.wipe(SpaceState::Revoked),
            Reply::OkNoProof | Reply::Network | Reply::LostAfterEffect => {
                self.pending(PendingReason::Network)
            }
            Reply::NotLastDevice => {
                // Not the last device after all: back to a normal departure, with one attempt.
                {
                    let mut sim = self.sim();
                    if sim.marker == Some(Marker::Deleting) {
                        sim.marker = Some(Marker::Requested);
                        sim.kind = Some(DepartureKind::Leave);
                    }
                }
                self.attempt_requested(true)
            }
            Reply::Unauthorized | Reply::SpaceNotInitialized => {
                if self.receipt_proves_deletion() {
                    self.wipe(SpaceState::NotFound)
                } else {
                    self.note_verdict(Verdict::NotFound);
                    self.pending(PendingReason::NotFound)
                }
            }
            Reply::NotFound404 => {
                self.note_verdict(Verdict::RelayTooOld);
                self.refuse(RefusalReason::RelayTooOld)
            }
            Reply::UpdateRequired => self.refuse(RefusalReason::UpdateRequired),
            Reply::Server => self.pending(PendingReason::Server),
            Reply::LastDevice | Reply::Unexpected4xx => self.refuse(RefusalReason::Unexpected),
        }
    }

    fn gate(&self) -> Result<(), CoreError> {
        if self.sim().unavailable {
            Err(CoreError::DepartureUnavailable)
        } else {
            Ok(())
        }
    }

    fn begin(&self, marker: Marker) -> Result<(), CoreError> {
        self.gate()?;
        let now = self.clock_ms();
        let mut sim = self.sim();
        if sim.erased {
            return Err(CoreError::NotPaired);
        }
        if sim.marker.is_some() {
            return Err(CoreError::Rejected("invalid_input".into()));
        }
        sim.marker = Some(marker);
        sim.kind = Some(if marker == Marker::Deleting {
            DepartureKind::DeleteSpace
        } else {
            DepartureKind::Leave
        });
        sim.since_ms = Some(now);
        sim.last = None;
        if marker == Marker::Deleting {
            sim.deletion_id = true;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- the trait, here

    /// Where a long attempt of the real core waits for the network: after the marker is written,
    /// before the relay answers. Only a test can stop the attempt here (`hold_next_attempt`).
    async fn hold(&self) {
        #[cfg(test)]
        {
            let hold = self.held.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(hold) = hold {
                hold.entered.notify_one();
                hold.release.notified().await;
            }
        }
    }

    pub(super) fn dep_available(&self) -> bool {
        !self.sim().unavailable
    }

    /// Demo only: after an erasure, pairing again (create or join) gives this PC a fresh demo
    /// space, as the real pairing gives a new one. Nothing of the erased space comes back.
    /// `false` if this PC was not erased.
    pub(super) fn dep_rejoin(&self) -> bool {
        {
            let mut sim = self.sim();
            if !sim.erased {
                return false;
            }
            // Everything the departure remembers is gone with the erasure; only the simulated
            // relay (mode, script, what it did) stays, as a relay would.
            sim.erased = false;
            sim.marker = None;
            sim.kind = None;
            sim.since_ms = None;
            sim.deletion_id = false;
            sim.last = None;
            sim.verdict = None;
            sim.space = None;
            sim.left_relay = false;
            sim.space_deleted = false;
        }
        *self.others_lock() = super::demo_others();
        self.emit(CoreEvent::Changed);
        true
    }

    pub(crate) async fn dep_leave_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.begin(Marker::Requested)?;
        self.hold().await;
        Ok(self.attempt(true))
    }

    pub(crate) async fn dep_delete_space(&self) -> Result<DepartureOutcome, CoreError> {
        self.begin(Marker::Deleting)?;
        self.hold().await;
        Ok(self.attempt(true))
    }

    pub(crate) async fn dep_retry(&self) -> Result<DepartureOutcome, CoreError> {
        self.gate()?;
        let now = self.clock_ms();
        {
            let mut sim = self.sim();
            if sim.marker.is_none() {
                return Err(CoreError::Rejected("invalid_input".into()));
            }
            sim.since_ms = Some(now);
        }
        self.hold().await;
        Ok(self.attempt(true))
    }

    pub(crate) async fn dep_continue(&self) -> Result<DepartureOutcome, CoreError> {
        self.gate()?;
        let now = self.clock_ms();
        let resend = {
            let sim = self.sim();
            if sim.marker.is_none() {
                return Ok(DepartureOutcome::NothingPending);
            }
            sim.since_ms
                .is_some_and(|since| (0..RESEND_WINDOW_MS).contains(&(now - since)))
        };
        self.hold().await;
        Ok(self.attempt(resend))
    }

    pub(crate) fn dep_cancel(&self) -> Result<bool, CoreError> {
        self.gate()?;
        let cancelled = {
            let mut sim = self.sim();
            if matches!(sim.marker, Some(Marker::Requested | Marker::Deleting)) {
                sim.marker = None;
                true
            } else {
                #[cfg(test)]
                {
                    sim.marker.is_none() && sim.promise_without_marker
                }
                #[cfg(not(test))]
                {
                    false
                }
            }
        };
        if cancelled {
            self.emit(CoreEvent::Changed);
        }
        Ok(cancelled)
    }

    pub(crate) fn dep_view(&self) -> Option<DepartureView> {
        let sim = self.sim();
        let marker = sim.marker?;
        Some(DepartureView {
            kind: sim.kind.unwrap_or(DepartureKind::Leave),
            wiping: marker == Marker::Wiping,
            since_ms: sim.since_ms,
            last: sim.last,
        })
    }

    pub(crate) fn dep_can_erase_without_proof(&self) -> bool {
        let now = self.clock_ms();
        let sim = self.sim();
        !sim.erased
            && matches!(
                sim.verdict,
                Some((Verdict::NotFound | Verdict::RelayTooOld, at)) if (0..=VERDICT_TTL_MS).contains(&(now - at))
            )
    }

    pub(crate) fn dep_erase_without_proof(&self) -> Result<(), CoreError> {
        self.gate()?;
        if !self.dep_can_erase_without_proof() {
            return Err(CoreError::Rejected("invalid_input".into()));
        }
        // No relay call, no marker needed: the same erasure, unconditionally.
        self.finish_wipe();
        Ok(())
    }

    // ---------------------------------------------------------------- test hooks

    /// The next attempt (leave, delete, retry, continue) stops after its marker is written, as an
    /// attempt does while the network is bad, until the test releases it. `entered` is notified
    /// when the attempt has got there.
    #[cfg(test)]
    pub fn hold_next_attempt(&self) -> Arc<Hold> {
        let hold = Arc::new(Hold::default());
        *self.held.lock().unwrap_or_else(|p| p.into_inner()) = Some(hold.clone());
        hold
    }

    /// The core cannot do a departure (the real core before lot C).
    #[cfg(test)]
    pub fn simulate_unavailable(&self) {
        self.sim().unavailable = true;
    }

    /// "Annuler" is promised while no marker exists yet (the real core, during a gesture).
    #[cfg(test)]
    pub fn promise_cancel_without_marker(&self) {
        self.sim().promise_without_marker = true;
    }

    /// The next request of `step` gets `reply` instead of what the mode would answer.
    #[cfg(test)]
    pub fn script(&self, step: Step, reply: Reply) {
        self.sim().script.push((step, reply));
    }

    /// Time passes for the core (the 15-minute window, the 10-minute verdict).
    #[cfg(test)]
    pub fn advance_clock(&self, ms: i64) {
        self.skew_ms.fetch_add(ms, Ordering::SeqCst);
    }

    /// `f` runs while the next `DELETE` is in flight (after its effect, before its answer).
    #[cfg(test)]
    pub fn during_next_request(&self, f: impl FnOnce() + Send + 'static) {
        self.sim().hook = Some(Box::new(f));
    }

    /// The engine is opened again: the in-memory verdict and last reason are gone, an
    /// interrupted erasure is finished, a marker on a PC that is no longer paired is removed.
    #[cfg(test)]
    pub fn reopen(&self) {
        let (wiping, erased) = {
            let mut sim = self.sim();
            sim.verdict = None;
            sim.last = None;
            (sim.marker == Some(Marker::Wiping), sim.erased)
        };
        if wiping {
            self.finish_wipe();
        } else if erased {
            self.sim().marker = None;
        }
    }

    /// An interruption right after the erasure transaction: the marker says `wiping`.
    #[cfg(test)]
    pub fn interrupt_in_wiping(&self) {
        self.sim().marker = Some(Marker::Wiping);
    }

    #[cfg(test)]
    pub fn has_deletion_id(&self) -> bool {
        self.sim().deletion_id
    }

    /// Whether the relay was asked to remove this PC / delete the space (a request had an
    /// effect there).
    #[cfg(test)]
    pub fn relay_state(&self) -> (bool, bool) {
        let sim = self.sim();
        (sim.left_relay, sim.space_deleted)
    }

    #[cfg(test)]
    pub fn pending_marker(&self) -> Option<&'static str> {
        match self.sim().marker {
            None => None,
            Some(Marker::Requested) => Some("requested"),
            Some(Marker::Deleting) => Some("deleting"),
            Some(Marker::Wiping) => Some("wiping"),
        }
    }

    #[cfg(test)]
    pub fn scripted_left(&self) -> usize {
        self.sim().script.len()
    }
}

/// What a healthy or broken relay answers when nothing is scripted.
fn default_reply(sim: &Sim, step: Step, members: usize) -> Reply {
    match sim.mode {
        RelayMode::Offline => return Reply::Network,
        RelayMode::Server => return Reply::Server,
        RelayMode::Reset => {
            return match step {
                Step::Session => Reply::Unauthorized,
                Step::LeaveMe | Step::DeleteSpace => Reply::SpaceNotInitialized,
                Step::Receipt => Reply::NotFound404,
            }
        }
        RelayMode::OldRelay if matches!(step, Step::DeleteSpace | Step::Receipt) => {
            return Reply::NotFound404
        }
        RelayMode::OldRelay | RelayMode::Normal => {}
    }
    if sim.space_deleted {
        return match step {
            Step::Session => Reply::Unauthorized,
            Step::LeaveMe | Step::DeleteSpace => Reply::SpaceNotInitialized,
            Step::Receipt => Reply::Ok,
        };
    }
    if sim.left_relay {
        return match step {
            Step::Receipt => Reply::NotFound404,
            Step::Session | Step::LeaveMe | Step::DeleteSpace => Reply::Revoked,
        };
    }
    match step {
        Step::Session => Reply::Ok,
        Step::LeaveMe if members <= 1 => Reply::LastDevice,
        Step::LeaveMe => Reply::Ok,
        Step::DeleteSpace if members <= 1 => Reply::Ok,
        Step::DeleteSpace => Reply::NotLastDevice,
        Step::Receipt => Reply::NotFound404,
    }
}

#[cfg(test)]
mod tests;
