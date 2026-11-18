//! The new device (N): joins the space with a code or a link (`spec/pairing-v1.md` §7.3, §8.3,
//! §8.4, §9, §11.4).
//!
//! ```text
//! join_space()         N0 checks, N1 reading of the code, N2 claim
//! wait_for_inviter()   N3 commit, N4 share, N5 reveal, N6 joiner_hello, N7 the code to compare
//! confirm(history)     N8 the user compared, N9 joiner_confirm
//! wait_done()          N10 provision verified and sealed in the store, N11 joiner_ack,
//!                      N12 wait for "completed", N13 provisioning, session, first sync
//! cancel()             at any time: the channel is closed, the secrets and the batch wiped
//! ```
//!
//! After N11 the device is registered or not depending on the inviter alone, and the app may
//! stop at any moment: the verified batch is sealed in the store before the acknowledgement
//! leaves, and [`Engine::resume_pairing`] settles it at the next start.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use zeroize::Zeroizing;

use super::pending::Resumed;
use super::{
    guard, idle_pause, is_transient, Cursor, Link, PairingPeer, Role, LONG_POLL_SECS,
    STATE_POLL_SECS,
};
use crate::error::{CoreError, Result};
use crate::pairing::{
    HistoryAnnouncement, HistoryOffer, Joiner, JoinerEvent, JoinerParams, JoinerPhase, PairingCode,
};
use crate::runtime::engine::Engine;
use crate::runtime::relay::RelayHold;
use crate::runtime::transport::{ChannelState, RelayError};

/// Longest wait, in seconds, before the single retry of a claim refused with 429.
const CLAIM_RETRY_MAX_SECS: u64 = 5;

/// Where a joining is, for the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPhase {
    /// The channel is claimed; the inviter has not shown its side yet.
    AwaitInviter,
    /// The verification code and the inviter are available ([`JoinSession::prompt`]): compare,
    /// answer the history question if it is asked, then [`confirm`](JoinSession::confirm) or
    /// [`cancel`](JoinSession::cancel).
    Verify,
    /// Confirmed here; waiting for the inviter's user and the batch.
    Provisioning,
    /// The batch is verified and sealed; waiting for the inviter to register this device.
    Completing,
    /// This device is part of the space.
    Done,
    /// Cancelled, here or on the other device, or the channel expired.
    Cancelled,
    /// Failed (wrong code, refused batch...). Read the error of the call that failed.
    Failed,
}

/// What the user of the new device compares and decides on (N7, N8).
#[derive(Clone, PartialEq, Eq)]
pub struct JoinPrompt {
    /// The relay this device is joining through, as the pairing binds to it (`relay.example.org`,
    /// `host[:port]`). It may have come from a link or a QR code: the screen shows it in full next
    /// to the verification code, and the user confirms it with the code (`pairing-v1.md` §4.4).
    pub relay_authority: String,
    /// The verification code in groups of three digits (`"482 913"`). Secret.
    pub sas: String,
    /// The inviter, as it announced itself.
    pub inviter: PairingPeer,
    /// Devices of the space, the inviter included ("Your space has 3 devices").
    pub member_count: Option<u16>,
    /// What the inviter offers to transfer. When it is `Some`, the screen asks the single
    /// yes / no question and passes the answer to [`confirm`](JoinSession::confirm); when it is
    /// `None` the question is not asked.
    pub history: Option<HistoryOffer>,
}

impl std::fmt::Debug for JoinPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinPrompt")
            .field("relay_authority", &self.relay_authority)
            .field("sas", &"<redacted>")
            .field("inviter", &self.inviter)
            .field("member_count", &self.member_count)
            .field("history", &self.history)
            .finish()
    }
}

/// This device is provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinOutcome {
    pub device_id: String,
    pub space_id: String,
    /// The history transfer the inviter announced (the user said yes): it is imported when it
    /// arrives, without a question. See [`Engine::awaited_history`].
    pub history: Option<HistoryAnnouncement>,
}

struct Shared {
    machine: Joiner,
    /// The user of this device pressed "Join".
    confirmed: bool,
    done: Option<JoinOutcome>,
    failure: Option<CoreError>,
}

struct Driver {
    cur: Cursor,
    /// The verified batch is sealed in the store (N10).
    stored: bool,
}

/// One joining, from the code to the provisioned device. See the module documentation.
///
/// Call [`cancel`](JoinSession::cancel) when the screen closes before the end: dropping the
/// handle closes nothing (no network call is made on a drop). A verified batch that was sealed
/// stays sealed for [`Engine::resume_pairing`] to settle.
pub struct JoinSession {
    wire: Link,
    expires_at_ms: i64,
    /// Authority of the relay of this session (what the screen shows).
    relay_authority: String,
    /// This session holds the relay until it ends, or is dropped (the guard gives it back).
    relay: RelayHold<Arc<Engine>>,
    shared: Mutex<Shared>,
    driver: Mutex<Driver>,
    /// This session sealed the batch of the device and owns it until it ends: a resumption of
    /// the engine leaves it alone meanwhile ([`Engine::resume_pairing`]).
    holds_lot: AtomicBool,
}

impl Drop for JoinSession {
    /// An abandoned session gives its batch back: it stays sealed, for a resumption to settle.
    fn drop(&mut self) {
        self.release();
        self.end_relay();
    }
}

impl std::fmt::Debug for JoinSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinSession")
            .field("channel_id", &self.wire.channel_id)
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Joins the space of the inviter that shows `code_or_link` (N0 to N2): a typed code
    /// (`4821-crayon-renard`) or a link (QR code, App Link, `tomi-share://`). `device_name` is the
    /// name this device will have in the space.
    ///
    /// The code is read locally first: a typo costs nothing and consumes nothing
    /// (`InvalidInput`). The channel is claimed last: `PairingExpired` for a code that ran out,
    /// `PairingTaken` for a code another device already used. An interrupted pairing of this
    /// device is settled before ([`resume_pairing`](Self::resume_pairing)).
    pub fn join_space(
        self: &Arc<Self>,
        code_or_link: &str,
        device_name: &str,
    ) -> Result<Arc<JoinSession>> {
        self.join_space_at(code_or_link, device_name, None)
    }

    /// [`join_space`](Self::join_space) at a relay the user may have named: `relay_url` is the
    /// relay typed next to the code (`https://host[:port][/prefix]`), absent when the device keeps
    /// its relay or the link names one. A link (`https://<relay>/l/pair#<code>`) carries the relay
    /// of the inviter; the fallback `tomi-share://pair#<code>` carries none.
    ///
    /// **A relay that was read is not trusted** (`pairing-v1.md` §4.4): if it is not the one the
    /// device already uses, the transport is pointed at it for this session only, and
    /// [`JoinSession::relay_authority`] / [`JoinPrompt::relay_authority`] give the authority the
    /// screen must show with the verification code. Nothing is stored before the device is
    /// provisioned (or the batch sealed, which only happens after the user confirmed); a session
    /// that fails, is cancelled or expires puts the relay back. A candidate that is not a valid
    /// relay, or a code or link that cannot be read, is `InvalidInput`, with nothing switched and
    /// nothing consumed; no relay at all is `RelayNotConfigured`. A device that is provisioned is
    /// refused whatever the relay (`InvalidInput`).
    pub fn join_space_at(
        self: &Arc<Self>,
        code_or_link: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<Arc<JoinSession>> {
        // N0: this device is not part of a space, and a lot left by a stop is settled first.
        self.resume_pairing()?;
        if self.is_provisioned() {
            return Err(CoreError::input("device already provisioned"));
        }
        let name = super::check_device_name(device_name)?;
        // N1: refused here, before anything is consumed or switched.
        let (code, link_relay) = if code_or_link.contains("://") {
            let link = crate::relay_url::parse_pair_link(code_or_link)
                .ok_or_else(|| CoreError::input("not a Tomi pairing link"))?;
            (PairingCode::parse(&link.code)?, link.relay_url)
        } else {
            (PairingCode::parse(code_or_link)?, None)
        };
        // The relay is held from here by a guard that gives it back on every path: an error of
        // `start_joining`, a panic, or, once it is the session's, the end or the drop of that.
        let hold = RelayHold::begin(self.clone(), relay_url, link_relay.as_deref())?;
        self.start_joining(code, name, hold)
    }

    /// N1 (keys) and N2 (claim), at the relay the session holds (`hold`, which the session takes
    /// over; dropped, so given back, on an error).
    fn start_joining(
        self: &Arc<Self>,
        code: PairingCode,
        name: String,
        hold: RelayHold<Arc<Engine>>,
    ) -> Result<Arc<JoinSession>> {
        let authority = self.relay_authority()?;
        let channel_id = code.channel_id().to_owned();
        // The keys of this device are generated now (N6), and the names checked, before the
        // claim: a refusal after it would waste the code.
        let machine = Joiner::new(JoinerParams {
            relay_authority: authority.clone(),
            code,
            device_name: name,
            platform: self.cfg.platform.clone(),
        })?;
        // N2
        // One retry on 429 (a rate limit shared by several people behind the same IP), after
        // the delay the relay asks for. If it persists the user gets `rate_limited`, which the
        // apps show as "too many attempts, try again in a few minutes". The code is not consumed
        // by a refused claim.
        let claimed = match self.transport.claim_pairing_channel(&channel_id) {
            Err(RelayError::Api(p)) if p.status == 429 => {
                let wait = p
                    .retry_after_secs
                    .unwrap_or(1)
                    .clamp(1, CLAIM_RETRY_MAX_SECS);
                std::thread::sleep(std::time::Duration::from_secs(wait));
                self.transport.claim_pairing_channel(&channel_id)
            }
            other => other,
        };
        let claim = match claimed {
            Ok(c) => c,
            Err(RelayError::Api(p)) => {
                return Err(match (p.status, p.code.as_str()) {
                    (404, _) | (409, "channel_closed") => CoreError::PairingExpired,
                    (409, "channel_claimed") => CoreError::PairingTaken,
                    _ => CoreError::Relay {
                        status: p.status,
                        code: p.code.clone(),
                    },
                })
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Arc::new(JoinSession {
            wire: Link::new(
                self.clone(),
                channel_id,
                Role::Joiner(Zeroizing::new(claim.pairing_token)),
                Link::limit_from(self),
            ),
            expires_at_ms: claim.expires_at_ms,
            relay_authority: authority,
            relay: hold,
            shared: Mutex::new(Shared {
                machine,
                confirmed: false,
                done: None,
                failure: None,
            }),
            driver: Mutex::new(Driver {
                cur: Cursor::default(),
                stored: false,
            }),
            holds_lot: AtomicBool::new(false),
        }))
    }
}

impl JoinSession {
    /// The relay of this session, as the pairing binds to it (`host[:port]`): what the screen
    /// shows in full with the verification code.
    pub fn relay_authority(&self) -> &str {
        &self.relay_authority
    }

    /// Identifier of the channel (not secret).
    pub fn channel_id(&self) -> &str {
        &self.wire.channel_id
    }

    /// When the relay drops the channel, on the relay clock: 120 s after the claim.
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    /// Where the joining is.
    pub fn phase(&self) -> JoinPhase {
        let sh = guard(&self.shared);
        if sh.done.is_some() {
            return JoinPhase::Done;
        }
        if let Some(e) = &sh.failure {
            return if *e == CoreError::PairingCancelled {
                JoinPhase::Cancelled
            } else {
                JoinPhase::Failed
            };
        }
        match sh.machine.phase() {
            JoinerPhase::AwaitCommit | JoinerPhase::AwaitReveal => JoinPhase::AwaitInviter,
            JoinerPhase::AwaitUserConfirm => JoinPhase::Verify,
            JoinerPhase::AwaitProvision => JoinPhase::Provisioning,
            JoinerPhase::Done => JoinPhase::Completing,
            JoinerPhase::Cancelled => JoinPhase::Cancelled,
            JoinerPhase::Failed => JoinPhase::Failed,
        }
    }

    /// The verification code, the inviter and the history offer, from the moment the inviter's
    /// `reveal` was verified (N7) until the batch is verified.
    pub fn prompt(&self) -> Option<JoinPrompt> {
        let sh = guard(&self.shared);
        let sas = sh.machine.sas()?;
        let inviter = sh.machine.inviter()?;
        Some(JoinPrompt {
            relay_authority: self.relay_authority.clone(),
            sas: sas.grouped(),
            inviter: PairingPeer {
                name: inviter.peer.name.clone(),
                platform: inviter.peer.platform.clone(),
            },
            member_count: inviter.member_count,
            history: inviter.history_offer.filter(|o| o.items > 0),
        })
    }

    /// Blocks until the inviter's side is verified and the codes can be compared (N3 to N7).
    /// Long polls of 25 s; returns at once if that is already the case.
    ///
    /// A wrong code ends here with `PairingFailed` (the channel is closed on the way; the user
    /// asks for a new code on the other device). Also `PairingCancelled`, `PairingExpired`,
    /// `PairingUnsupported`; a transient error (no network) leaves the session alive.
    pub fn wait_for_inviter(&self) -> Result<JoinPrompt> {
        loop {
            if let Some(p) = self.prompt() {
                return Ok(p);
            }
            self.ensure_running()?;
            self.wait_step()?;
        }
    }

    /// The user compared the codes and pressed "Join" (N8, N9), with the answer to the history
    /// question: `import_history` is ignored (and sent as "no") when the inviter offered
    /// nothing. Deposits `joiner_confirm`; follow with [`wait_done`](Self::wait_done).
    pub fn confirm(&self, import_history: bool) -> Result<()> {
        let mut d = self.begin()?;
        let confirmed = {
            let mut sh = guard(&self.shared);
            if sh.confirmed {
                return Ok(());
            }
            match sh.machine.confirm(import_history) {
                Ok(frame) => {
                    sh.confirmed = true;
                    Ok(frame)
                }
                // Not the moment (a user error, nothing to end): the session goes on.
                Err(e @ CoreError::InvalidInput { .. }) => return Err(e),
                Err(e) => Err(e),
            }
        };
        match confirmed {
            Ok(frame) => d.cur.outbox.push_back(frame),
            Err(e) => return Err(self.settle(e)),
        }
        self.wire.flush(&mut d.cur).map_err(|e| self.settle(e))
    }

    /// Blocks until this device is provisioned (N10 to N13): the batch is verified and sealed,
    /// acknowledged, the inviter registers the device, the device is provisioned, its session
    /// opened and its first sync done. Call after [`confirm`](Self::confirm).
    ///
    /// `PairingExpired` also covers a channel that was purged before this device read
    /// "completed" and whose inviter did not register it (`pairing-v1.md` §11.4): the batch
    /// is erased. `PairingCancelled` when the inviter closed the channel.
    pub fn wait_done(&self) -> Result<JoinOutcome> {
        loop {
            if let Some(o) = guard(&self.shared).done.clone() {
                return Ok(o);
            }
            if !self.has_confirmed() {
                return Err(CoreError::input("confirm the verification code first"));
            }
            self.wait_step()?;
        }
    }

    /// One long poll of a blocking wait. A relay that answers at once with nothing new is not
    /// asked again at once.
    fn wait_step(&self) -> Result<()> {
        let started = Instant::now();
        let (_, progressed) = self.step(LONG_POLL_SECS)?;
        if !progressed {
            idle_pause(started, &self.wire.cancelled);
        }
        Ok(())
    }

    /// One step: deposits what waits, reads the channel (waiting up to `wait_secs`, 0 to 25,
    /// for a frame) and feeds the machine; once the batch is sealed, waits for "completed" and
    /// provisions. A transient error leaves the session as it was; any other ends it.
    pub fn poll(&self, wait_secs: u32) -> Result<JoinPhase> {
        self.step(wait_secs).map(|(phase, _)| phase)
    }

    /// [`poll`](Self::poll), and whether anything happened (a frame came, the device was
    /// provisioned).
    fn step(&self, wait_secs: u32) -> Result<(JoinPhase, bool)> {
        let mut d = self.begin()?;
        match self.advance(&mut d, wait_secs) {
            Ok(progressed) => Ok((self.phase(), progressed)),
            Err(e) => Err(self.settle(e)),
        }
    }

    /// The user refused ("they are different", "Cancel", back) or left the screen: closes the
    /// channel, wipes the session and erases the batch if one is sealed. If the inviter had
    /// already registered this device, the user removes it from the device list.
    pub fn cancel(&self) {
        self.wire.cancelled.store(true, Ordering::SeqCst);
        {
            let mut sh = guard(&self.shared);
            if sh.done.is_some() || sh.failure.is_some() {
                return;
            }
            sh.failure = Some(CoreError::PairingCancelled);
            sh.machine.cancel();
        }
        // The batch of *this* session goes; one sealed by another session is not ours to erase.
        if self.holds_lot.load(Ordering::SeqCst) {
            let _ = self.wire.engine.discard_lot();
        }
        self.release();
        self.wire.close();
        self.end_relay();
    }

    /// Gives the sealed batch back to the engine: the session ended or was abandoned.
    fn release(&self) {
        if self.holds_lot.swap(false, Ordering::SeqCst) {
            self.wire.engine.release_lot();
        }
    }

    /// The session no longer needs its relay: the device goes back to the one it keeps, unless it
    /// is provisioned or a sealed batch still needs this one. After the last call to the relay.
    fn end_relay(&self) {
        self.relay.release();
    }

    // -----------------------------------------------------------------------

    fn has_confirmed(&self) -> bool {
        let sh = guard(&self.shared);
        sh.done.is_some() || sh.confirmed
    }

    fn ensure_running(&self) -> Result<()> {
        let sh = guard(&self.shared);
        match (&sh.failure, &sh.done) {
            (Some(e), _) => Err(e.clone()),
            (None, Some(_)) => Err(CoreError::input("the pairing is already over")),
            (None, None) => Ok(()),
        }
    }

    fn begin(&self) -> Result<MutexGuard<'_, Driver>> {
        if let Some(e) = &guard(&self.shared).failure {
            return Err(e.clone());
        }
        match self.driver.try_lock() {
            Ok(g) => Ok(g),
            Err(TryLockError::Poisoned(p)) => Ok(p.into_inner()),
            Err(TryLockError::WouldBlock) => Err(CoreError::input(
                "another call is running on this pairing session",
            )),
        }
    }

    /// Ends the session on an error that is not transient, and returns it.
    ///
    /// A sealed batch is **kept**: whether the inviter registered this device is not known
    /// here, and [`Engine::resume_pairing`] decides it with the relay. The batch is erased by
    /// a cancellation, by a channel the inviter closed, and by a relay that does not know the
    /// device.
    fn settle(&self, e: CoreError) -> CoreError {
        if is_transient(&e) {
            return e;
        }
        {
            let mut sh = guard(&self.shared);
            if sh.failure.is_none() && sh.done.is_none() {
                sh.failure = Some(e.clone());
            }
            sh.machine.cancel();
        }
        self.release();
        if !matches!(e, CoreError::PairingExpired | CoreError::PairingCancelled) {
            self.wire.close();
        }
        self.end_relay();
        e
    }

    /// Returns whether something happened (a frame was read, the device was provisioned).
    fn advance(&self, d: &mut Driver, wait_secs: u32) -> Result<bool> {
        if guard(&self.shared).done.is_some() {
            return Ok(false);
        }
        if d.stored {
            return self.await_completion(d, wait_secs);
        }
        let pumped = self.wire.pump(&mut d.cur, wait_secs)?;
        let progressed = !pumped.frames.is_empty();
        for frame in &pumped.frames {
            if self.wire.is_cancelled() {
                return Err(CoreError::PairingCancelled);
            }
            let step = guard(&self.shared).machine.receive(frame)?;
            if let Some(send) = step.send {
                d.cur.outbox.push_back(send);
            }
            if step.event == JoinerEvent::ProvisionVerified {
                // N10: the batch is sealed in the store **before** the acknowledgement (in the
                // outbox, deposited below) lets the inviter register this device.
                let lot = guard(&self.shared).machine.take_provision()?;
                // Sealing is not optional, and not a "try again": the batch has left the
                // machine, and the acknowledgement waits in the outbox. A failure to seal it
                // ends the pairing here (the acknowledgement never leaves), even when it is a
                // storage error that would be transient anywhere else.
                self.wire.engine.save_lot(&lot).map_err(|e| {
                    if is_transient(&e) {
                        CoreError::Internal {
                            reason: "the pairing batch could not be sealed".into(),
                        }
                    } else {
                        e
                    }
                })?;
                self.holds_lot.store(true, Ordering::SeqCst);
                d.stored = true;
            }
        }
        match pumped.state {
            ChannelState::Closed => {
                if d.stored {
                    self.wire.engine.discard_lot()?;
                }
                return Err(CoreError::PairingCancelled);
            }
            // Only the inviter completes a channel, and not before this device acknowledged.
            ChannelState::Completed => return Err(CoreError::PairingFailed),
            ChannelState::Open | ChannelState::Claimed => {}
        }
        // Send the acknowledgement. Once the batch is stored, a `channel_closed` here does NOT
        // mean "cancelled": the inviter may have read a first copy of the ack (whose response
        // was lost), registered this device, and completed the channel, so the replay meets a
        // completed/closed channel. Read the state to tell `completed` (register) from a genuine
        // `closed` (§7.3 N12) instead of concluding "cancelled" and stranding a provisioned N.
        match self.wire.flush(&mut d.cur) {
            Ok(()) => Ok(progressed),
            Err(CoreError::PairingCancelled) if d.stored => self.resolve_after_ack(d),
            Err(e) => Err(e),
        }
    }

    /// The acknowledgement send met a closed channel after the batch was stored: read the
    /// channel state once (`wait=0`) to decide (R2 of the lot 2 review).
    fn resolve_after_ack(&self, d: &mut Driver) -> Result<bool> {
        match self.wire.pump(&mut d.cur, 0) {
            Ok(p) => match p.state {
                // The inviter registered this device and completed the channel.
                ChannelState::Completed => self.provision(),
                // A genuine give-up: drop the batch (§7.3 N12).
                ChannelState::Closed => {
                    self.wire.engine.discard_lot()?;
                    Err(CoreError::PairingCancelled)
                }
                // Still open/claimed: the ack really could not be delivered.
                ChannelState::Open | ChannelState::Claimed => Err(CoreError::PairingCancelled),
            },
            // The channel is gone: ask the relay whether this device was registered (§11.4).
            Err(CoreError::PairingExpired) => self.settle_unknown(),
            Err(e) => Err(e),
        }
    }

    /// N12: the acknowledgement is out; wait for the inviter to register this device. What is
    /// awaited is a change of state, so the wait is short (see [`STATE_POLL_SECS`]).
    fn await_completion(&self, d: &mut Driver, wait_secs: u32) -> Result<bool> {
        let pumped = match self.wire.pump(&mut d.cur, wait_secs.min(STATE_POLL_SECS)) {
            Ok(p) => p,
            // The channel is gone, or the session ran out, before "completed" was read: the
            // relay tells whether the inviter registered this device (`pairing-v1.md` §11.4).
            Err(CoreError::PairingExpired) => return self.settle_unknown(),
            Err(e) => return Err(e),
        };
        match pumped.state {
            ChannelState::Completed => self.provision(),
            ChannelState::Closed => {
                // The inviter gave up: the batch goes.
                self.wire.engine.discard_lot()?;
                Err(CoreError::PairingCancelled)
            }
            ChannelState::Open | ChannelState::Claimed => Ok(false),
        }
    }

    /// N13, from the sealed copy.
    fn provision(&self) -> Result<bool> {
        let outcome = self.wire.engine.complete_pending()?;
        guard(&self.shared).done = Some(outcome);
        self.release();
        self.end_relay();
        Ok(true)
    }

    /// The channel is gone and "completed" was not read: the relay says whether the inviter
    /// registered this device (`pairing-v1.md` §11.4). This session owns the batch, so it
    /// settles it itself, which a resumption of the engine would not do under it.
    fn settle_unknown(&self) -> Result<bool> {
        match self.wire.engine.resolve_lot()? {
            Resumed::Provisioned(outcome) => {
                guard(&self.shared).done = Some(outcome);
                self.release();
                self.end_relay();
                Ok(true)
            }
            Resumed::Discarded | Resumed::Nothing => Err(CoreError::PairingExpired),
        }
    }
}
