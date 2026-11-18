//! Pairing over the relay (`spec/pairing-v1.md` §7, §8, §9, §10, §11): the orchestration that
//! drives the pure machines of [`crate::pairing`] across the network.
//!
//! | role | entry point | handle | steps of the spec |
//! |------|-------------|--------|-------------------|
//! | founder (first device) | [`Engine::create_space`] | none | §10 |
//! | inviter (A) | [`Engine::start_invitation`] | [`InviteSession`] | A1 to A13 |
//! | new device (N) | [`Engine::join_space`] | [`JoinSession`] | N0 to N13 |
//! | after a stop of N | [`Engine::resume_pairing`] | none | §11.4 |
//!
//! ## How a session is driven
//!
//! A session is a handle (`Arc`), usable from several threads. Each side runs on the thread of
//! the app: [`InviteSession::wait_for_peer`] and [`JoinSession::wait_for_inviter`] block (long
//! polls of 25 s) until the verification code can be compared; `confirm` records the decision of
//! the user; `wait_done` blocks until the pairing is over. [`poll`](InviteSession::poll) is the
//! single step they are made of (one round trip to the relay at most, `wait_secs` = 0 never
//! blocks): an app that prefers to keep the thread can call it itself, and the tests interleave
//! both devices with it. `cancel` may be called at any time from another thread.
//!
//! The engine never spawns a thread. It never holds a lock of a session across a call to the
//! relay, except the one that serialises the calls driving the session.
//!
//! ## Safety properties kept here (and not in the machines)
//!
//! * the space key leaves the inviter only after **both** humans confirmed: the inviter's
//!   machine refuses to build the batch otherwise, and this module calls
//!   [`confirm`](InviteSession::confirm) only on the decision of the user;
//! * the new device stores the verified batch in the protected storage **before** its
//!   acknowledgement leaves, and provisions from that stored copy (§8.3, §11.4);
//! * the inviter learns the new device in its trust store **by adding** it to the current
//!   store, in the transaction of the device row, after `201` (never by replacing the store);
//! * a relay that duplicates, reorders, or replays frames cannot make a session do more than
//!   fail: every frame is read once, in order, and a repeated frame is dropped;
//! * nothing secret is logged: this module has no log, and its errors are stable codes.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::crypto::sha256;
use crate::error::{CoreError, Result};
use crate::runtime::engine::Engine;
use crate::runtime::transport::{ChannelState, PairingAuth, PairingSide, RelayError, TResult};

mod founder;
mod history;
mod invite;
mod join;
mod pending;
#[cfg(test)]
mod tests;

pub use history::AwaitedHistory;
pub(crate) use history::{set_awaited_history, take_awaited_history};
pub use invite::{InviteOutcome, InvitePhase, InviteSession, VerifyPrompt};
pub use join::{JoinOutcome, JoinPhase, JoinPrompt, JoinSession};
pub use pending::ResumeOutcome;
pub(crate) use pending::{FOUNDER_PENDING, JOINER_PENDING};

/// Longest wait of one long poll (`api-v1.md` §4.6).
pub(crate) const LONG_POLL_SECS: u32 = 25;
/// Longest wait of one read when what is awaited is a **change of state** and no frame ("completed",
/// N12). The relay wakes a long poll on a message, or on a state that changes *while it waits*: a
/// read that begins after the change sees nothing happen and sleeps through its whole wait. A
/// device that was held up between its acknowledgement and this read (a pause, a suspended app)
/// would stare at "Adding…" for 25 s, so the wait is kept short.
pub(super) const STATE_POLL_SECS: u32 = 5;
/// A client gives up 600 s after the start of a session, whatever the relay says, so that a
/// relay that never expires the channel cannot keep a session alive (`pairing-v1.md` §11.3).
const SESSION_LIMIT_MS: i64 = 600_000;
/// Attempts of one call that fails for a transient reason (no answer, `429`, `5xx`).
const MAX_ATTEMPTS: u32 = 4;

/// The other device, as it announced itself on the authenticated channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPeer {
    pub name: String,
    pub platform: String,
}

/// Locks `m`, recovering it if a panic poisoned it: the data guarded here is replaced whole.
pub(super) fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether an error leaves the session alive: the relay could not be reached or asked to wait,
/// or the local database failed. The caller can call again.
pub(super) fn is_transient(e: &CoreError) -> bool {
    match e {
        CoreError::Network { .. } | CoreError::Storage { .. } => true,
        CoreError::Relay { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

/// Delay before attempt `n + 1` of a call that failed: 0.5 s, 1 s, 2 s; a `Retry-After` of the
/// relay's `Retry-After` is honoured in full (never shortened: retrying before the time the
/// relay asked would only earn another `429`), bounded to the long-poll window so a hostile
/// value cannot stall the session, which keeps its own overall deadline.
fn retry_delay_ms(failed_attempts: u32, error: &RelayError) -> u64 {
    let base = 500u64 << failed_attempts.saturating_sub(1).min(4);
    match error {
        RelayError::Api(p) => p.retry_after_secs.map_or(base, |s| {
            s.saturating_mul(1000)
                .clamp(base, LONG_POLL_SECS as u64 * 1000)
        }),
        RelayError::Network(_) => base,
    }
}

/// Least time between two reads of the channel that brought nothing: a relay that answers a long
/// poll at once (a bug, or an attack on the battery) is not asked again in a loop. Kept at or
/// below `rl_pairing_poll` (60/min = one per second) so honest idle polling is never rate-limited.
const IDLE_FLOOR_MS: u64 = 1000;

/// Waits for what is left of [`IDLE_FLOOR_MS`] since `started`.
pub(super) fn idle_pause(started: Instant, cancelled: &AtomicBool) {
    let spent = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if spent < IDLE_FLOOR_MS {
        pause(IDLE_FLOOR_MS - spent, Some(cancelled));
    }
}

/// Waits `ms`, in slices so that a cancellation is noticed. Tests run on a manual clock and
/// never wait. Returns `false` if the session was cancelled meanwhile.
fn pause(ms: u64, cancelled: Option<&AtomicBool>) -> bool {
    if cfg!(test) {
        return !cancelled.is_some_and(|c| c.load(Ordering::SeqCst));
    }
    let mut left = ms;
    while left > 0 {
        if cancelled.is_some_and(|c| c.load(Ordering::SeqCst)) {
            return false;
        }
        let step = left.min(100);
        std::thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    true
}

/// Runs a relay call, repeating it a few times when it fails for a transient reason.
///
/// Only for calls that are safe to repeat: reads, and writes the relay answers the same way
/// twice (the deposit of a pairing frame can leave a duplicate if the first answer was lost;
/// [`Cursor`] drops duplicates on the reading side).
pub(super) fn retrying<T>(
    cancelled: Option<&AtomicBool>,
    mut call: impl FnMut() -> TResult<T>,
) -> TResult<T> {
    let mut failed = 0u32;
    loop {
        match call() {
            Err(e) if e.is_retryable() && failed + 1 < MAX_ATTEMPTS => {
                failed += 1;
                if !pause(retry_delay_ms(failed, &e), cancelled) {
                    return Err(e);
                }
            }
            other => return other,
        }
    }
}

/// How a session reaches the channel: as the inviter (its `Bearer` session) or as the new
/// device (the `pairing_token` of the claim, kept in memory only).
pub(super) enum Role {
    Inviter,
    Joiner(Zeroizing<String>),
}

/// What a session shares with the calls that stop it from another thread.
pub(super) struct Link {
    pub(super) engine: Arc<Engine>,
    pub(super) channel_id: String,
    pub(super) role: Role,
    /// Local limit of the session, on the engine clock (`pairing-v1.md` §11.3).
    pub(super) deadline_ms: i64,
    pub(super) cancelled: AtomicBool,
}

/// Reading position and frames to deposit: owned by whoever drives the session.
#[derive(Default)]
pub(super) struct Cursor {
    /// Index of the last message of the other side that was read.
    pub(super) after: u64,
    /// Digests of the frames already read: a frame deposited twice (the answer to the first
    /// deposit was lost, the deposit was repeated) is delivered once.
    seen: HashSet<[u8; 32]>,
    /// Frames produced by the machine, not yet accepted by the relay, oldest first.
    pub(super) outbox: VecDeque<Vec<u8>>,
}

/// What one reading of the channel brought.
pub(super) struct Pumped {
    pub(super) state: ChannelState,
    /// New frames of the other side, in order, without duplicates.
    pub(super) frames: Vec<Vec<u8>>,
}

impl Link {
    pub(super) fn new(
        engine: Arc<Engine>,
        channel_id: String,
        role: Role,
        deadline_ms: i64,
    ) -> Self {
        Self {
            engine,
            channel_id,
            role,
            deadline_ms,
            cancelled: AtomicBool::new(false),
        }
    }

    /// The local limit of a session that starts now.
    pub(super) fn limit_from(engine: &Engine) -> i64 {
        engine.clock.now_ms().saturating_add(SESSION_LIMIT_MS)
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Calls the relay as this side. The inviter renews its session on `session_expired`.
    fn call<T>(&self, mut f: impl FnMut(PairingAuth<'_>) -> TResult<T>) -> TResult<T> {
        match &self.role {
            Role::Inviter => self.engine.authed(|| f(PairingAuth::Inviter)),
            Role::Joiner(token) => f(PairingAuth::Joiner(token.as_str())),
        }
    }

    /// A call that is repeated while the relay does not answer.
    fn call_retrying<T>(&self, mut f: impl FnMut(PairingAuth<'_>) -> TResult<T>) -> TResult<T> {
        retrying(Some(&self.cancelled), || self.call(&mut f))
    }

    fn other_side(&self) -> PairingSide {
        match self.role {
            Role::Inviter => PairingSide::Joiner,
            Role::Joiner(_) => PairingSide::Inviter,
        }
    }

    /// Turns what the relay said about the channel into the error of the pairing
    /// (`pairing-v1.md` §11.5).
    pub(super) fn map_error(&self, e: RelayError) -> CoreError {
        if let RelayError::Api(p) = &e {
            match (p.status, p.code.as_str()) {
                // The channel is gone: expired, purged, or never ours.
                (404, _) => return CoreError::PairingExpired,
                // The token of the new device is not (or no longer) valid for this channel.
                (401, _) if matches!(self.role, Role::Joiner(_)) => {
                    return CoreError::PairingExpired
                }
                // The other side (or the user) closed it.
                (409, "channel_closed") => return CoreError::PairingCancelled,
                // We would exceed the frames of the protocol, or one is too large: not ours.
                (409, "channel_limit") | (413, _) => return CoreError::PairingFailed,
                _ => {}
            }
        }
        e.into()
    }

    /// Stops the session if it was cancelled, or ran past its local limit (the channel is
    /// then closed on the way: nobody will use it).
    pub(super) fn check_alive(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(CoreError::PairingCancelled);
        }
        if self.engine.clock.now_ms() >= self.deadline_ms {
            self.close();
            return Err(CoreError::PairingExpired);
        }
        Ok(())
    }

    /// Deposits the frames waiting in the outbox, oldest first.
    pub(super) fn flush(&self, cur: &mut Cursor) -> Result<()> {
        while let Some(frame) = cur.outbox.front() {
            self.call_retrying(|auth| {
                self.engine
                    .transport
                    .post_pairing_message(&self.channel_id, auth, frame)
            })
            .map_err(|e| self.map_error(e))?;
            cur.outbox.pop_front();
        }
        Ok(())
    }

    /// Deposits what waits, then reads the new frames of the other side, waiting up to
    /// `wait_secs` for one.
    ///
    /// Frames are returned in index order. A message that is not from the other side, that is
    /// not newer than the last one read, or that repeats a frame already read, is dropped: a
    /// relay can reorder and replay, it cannot make a machine read a frame twice.
    pub(super) fn pump(&self, cur: &mut Cursor, wait_secs: u32) -> Result<Pumped> {
        self.check_alive()?;
        self.flush(cur)?;
        self.check_alive()?;
        let mut wait = wait_secs.min(LONG_POLL_SECS);
        let after = cur.after;
        let got = self
            .call_retrying(|auth| {
                let r = self
                    .engine
                    .transport
                    .pairing_messages(&self.channel_id, auth, after, wait);
                if r.is_err() {
                    // Time goes by before the repeat, and the relay does not wake a long poll
                    // for a change of state that happened before it began (the channel may have
                    // been completed or closed meanwhile, see [`STATE_POLL_SECS`]): the repeat
                    // only looks, and the caller waits again from what it saw.
                    wait = 0;
                }
                r
            })
            .map_err(|e| self.map_error(e))?;
        self.check_alive()?;
        let mut messages = got.messages;
        messages.sort_by_key(|m| m.index);
        let want = self.other_side();
        let mut frames = Vec::new();
        for m in messages {
            if m.side != want || m.index <= cur.after {
                continue;
            }
            cur.after = m.index;
            if cur.seen.insert(sha256(&[&m.body])) {
                frames.push(m.body);
            }
        }
        Ok(Pumped {
            state: got.state,
            frames,
        })
    }

    /// Closes the channel at the relay, at best (`DELETE`): used after a failure, a refusal or a
    /// cancellation. A failure to do so changes nothing: the relay drops the channel at its
    /// deadline.
    pub(super) fn close(&self) {
        let _ = self.call(|auth| {
            self.engine
                .transport
                .close_pairing_channel(&self.channel_id, auth)
        });
    }
}

impl Engine {
    /// The relay authority the PAKE binds to, from the transport that talks to the relay
    /// (`pairing-v1.md` §3.3): the relay the device retained, which a link or a field may have
    /// named and the user confirmed. `RelayNotConfigured` when there is none.
    pub(crate) fn relay_authority(&self) -> Result<String> {
        self.transport
            .relay_authority()
            .ok_or(CoreError::RelayNotConfigured)
    }
}

/// A device name or a platform as `api-v1.md` §2.3 types them.
pub(super) fn check_device_name(name: &str) -> Result<String> {
    let name = name.trim();
    if crate::pairing::is_valid_device_name(name) {
        Ok(name.to_owned())
    } else {
        Err(CoreError::input(
            "invalid device name (1 to 64 characters, no control character)",
        ))
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn transient_errors_keep_the_session_alive_and_the_others_end_it() {
        assert!(is_transient(&CoreError::Network { reason: "x".into() }));
        assert!(is_transient(&CoreError::Storage { reason: "x".into() }));
        assert!(is_transient(&CoreError::Relay {
            status: 429,
            code: "rate_limited".into()
        }));
        assert!(is_transient(&CoreError::Relay {
            status: 503,
            code: "unavailable".into()
        }));
        for e in [
            CoreError::PairingFailed,
            CoreError::PairingExpired,
            CoreError::PairingCancelled,
            CoreError::PairingTaken,
            CoreError::PairingUnsupported,
            CoreError::Relay {
                status: 409,
                code: "epoch_conflict".into(),
            },
        ] {
            assert!(!is_transient(&e), "{e:?}");
        }
    }

    #[test]
    fn retry_delay_grows_and_honours_retry_after_within_bounds() {
        let net = RelayError::Network("x".into());
        assert_eq!(retry_delay_ms(1, &net), 500);
        assert_eq!(retry_delay_ms(2, &net), 1000);
        assert_eq!(retry_delay_ms(3, &net), 2000);
        // A modest Retry-After is honoured in full (never shortened below what the relay asked).
        let modest = RelayError::problem(crate::runtime::transport::ApiProblem {
            status: 429,
            code: "rate_limited".into(),
            retry_after_secs: Some(3),
            ..Default::default()
        });
        assert_eq!(retry_delay_ms(1, &modest), 3000, "honoured in full");
        // A large Retry-After is bounded to the long-poll window, not shortened to a few seconds
        // (the session keeps its own overall deadline).
        let large = RelayError::problem(crate::runtime::transport::ApiProblem {
            status: 429,
            code: "rate_limited".into(),
            retry_after_secs: Some(60),
            ..Default::default()
        });
        assert_eq!(
            retry_delay_ms(1, &large),
            LONG_POLL_SECS as u64 * 1000,
            "bounded to the long-poll window"
        );
    }

    #[test]
    fn a_device_name_is_trimmed_then_checked() {
        assert_eq!(check_device_name("  Pixel ").unwrap(), "Pixel");
        assert!(check_device_name("").is_err());
        assert!(check_device_name("   ").is_err());
        assert!(check_device_name("a\u{7}b").is_err());
        assert!(check_device_name(&"x".repeat(65)).is_err());
    }
}
