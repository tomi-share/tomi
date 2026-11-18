//! The relay of the device (`spec/pairing-v1.md` §4.4, §10).
//!
//! A device talks to **one** relay: the one of its space. Until the device is provisioned the
//! relay is not settled: it may come from the build (a default for development, none in a release
//! build), from a link or a QR code, or from a field the user filled. Whatever the source, a relay
//! that was *read* is not trustworthy by itself (whoever shows the QR code chooses it): the
//! application shows it to the user, and **nothing about it is kept for good before the pairing
//! or the creation succeeds**. What is written earlier is the *pending* relay
//! (`meta.pending_relay_url`), in the transaction that seals a record only a stop could lose
//! (the verified batch of a pairing, the keys of a founder); it is forgotten when that record is.
//!
//! | what | where |
//! |------|-------|
//! | the relay of a provisioned device | `meta.relay_url`, written in the transaction of the provisioning; never changes afterwards |
//! | a relay chosen with [`Engine::set_relay`] | `meta.relay_url` (a device that is not provisioned; or, once, a provisioned device that has none: see below) |
//! | the relay of a pairing or a creation under way | the transport only (in memory) until the pairing seals its batch (new device) or the founder seals its keys; then `meta.pending_relay_url` as well, so that a stop does not lose the relay the pending record belongs to |
//!
//! At opening the relay is, in this order: the one of the pending record, `meta.relay_url`, the
//! default of the build ([`EngineConfig::default_relay_url`], or the one the transport was made
//! with), or none. A pairing or a creation that fails, is cancelled or expires puts the relay
//! back; a provisioned device never changes relay.
//!
//! **One exception, once:** a device provisioned *before* the relay was kept (`meta.relay_url`
//! absent) and opened by a build that has no default relay has none: it could neither sync nor
//! ring the doorbell, and nothing else could give it one. [`Engine::set_relay`] then accepts one
//! relay, which becomes its own for good (the next call is refused like for any provisioned
//! device).
//!
//! ## The gate
//!
//! Every decision about the relay is taken with [`Engine::relay_gate`] held: the start of a
//! session (the checks and the switch of the transport it allows, and the count), the end of a
//! session and the settling that follows, `set_relay`, and the passage of a founder to the relay
//! of its pending keys ([`Engine::exclusive_relay`]). Two sessions at the same relay may overlap;
//! one that needs another relay, or `set_relay`, finds the gate taken by a session and fails
//! with `InvalidInput`. A session is held by a [`RelayHold`], which gives it back on **every**
//! path (an error, a drop, a panic).
//!
//! ## What is not closed
//!
//! The relay a session talks to is a property of the shared transport, not of each request: a
//! call that was started by a thread of the app *while* the session ends (`cancel()` from another
//! thread) can leave after the transport went back to the relay the device keeps, and carry the
//! `pairing_token` of the session there. That relay is the one the device keeps (its own, or none),
//! or, if another session at another relay began right after, the relay that session was started
//! for (one the user typed or scanned). The token only opens a channel that is being closed. Closing it would take a
//! relay per request in the transport, which is out of the scope of this lot.

use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{MutexGuard, PoisonError};

use crate::error::{CoreError, Result};
use crate::relay_url::{check_base_url, check_relay_url};
use crate::runtime::engine::Engine;
use crate::runtime::pairing::{FOUNDER_PENDING, JOINER_PENDING};
use crate::runtime::store::Db;

/// `meta` key: the relay of the device (normalised base URL).
pub(crate) const RELAY_URL_KEY: &str = "relay_url";
/// `meta` key: the relay of the pending record (a sealed batch, or the keys of a founder).
pub(crate) const PENDING_RELAY_KEY: &str = "pending_relay_url";

/// What the relay gate guards.
#[derive(Debug, Default)]
pub(crate) struct RelayState {
    /// Pairings and creations holding the relay: while there is one, the relay does not change.
    sessions: u32,
    /// A founder moves the transport to the relay of its pending keys and back
    /// ([`Engine::exclusive_relay`]): no other decision about the relay is taken meanwhile.
    exclusive: bool,
}

/// The normalised form of a relay URL, `None` if it is not one.
fn normalised(url: &str) -> Option<String> {
    check_base_url(url).ok().map(|b| b.url)
}

fn authority_of(url: &str) -> Option<String> {
    check_base_url(url).ok().map(|b| b.relay_authority())
}

/// A pairing or a creation holding the relay of the device (`engine` is `&Engine` or
/// `Arc<Engine>`). The hold ends with [`release`](Self::release) or when the value is dropped,
/// whichever comes first, exactly once.
pub(crate) struct RelayHold<E: Deref<Target = Engine>> {
    engine: E,
    held: AtomicBool,
}

impl<E: Deref<Target = Engine>> RelayHold<E> {
    /// Starts a session at a relay: see [`Engine::begin_relay_session`].
    pub(crate) fn begin(
        engine: E,
        explicit: Option<&str>,
        from_link: Option<&str>,
    ) -> Result<Self> {
        engine.begin_relay_session(explicit, from_link)?;
        Ok(Self {
            engine,
            held: AtomicBool::new(true),
        })
    }

    /// The session is over, whatever the way: the device goes back to the relay it keeps unless
    /// it is provisioned, a sealed record needs this one, or another session holds it.
    /// Idempotent.
    pub(crate) fn release(&self) {
        if self.held.swap(false, Ordering::SeqCst) {
            self.engine.end_relay_session();
        }
    }
}

impl<E: Deref<Target = Engine>> Drop for RelayHold<E> {
    fn drop(&mut self) {
        self.release();
    }
}

/// Proof that no other decision about the relay is taken: the holder may move the transport.
/// Given back on drop.
pub(crate) struct ExclusiveRelay<'a>(&'a Engine);

impl Drop for ExclusiveRelay<'_> {
    fn drop(&mut self) {
        self.0.gate().exclusive = false;
    }
}

impl Engine {
    /// Takes the relay gate (a panic that poisoned it left a counter that is still right).
    fn gate(&self) -> MutexGuard<'_, RelayState> {
        self.relay_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The relay this device talks to now (`https://host[:port][/prefix]`): the one of its space
    /// once provisioned; before, the one a pairing or a creation is using, the one chosen with
    /// [`set_relay`](Self::set_relay), or the default of the build. `None`: not configured, every
    /// call to the relay then fails (`RelayNotConfigured` for a pairing or a creation).
    pub fn relay_url(&self) -> Option<String> {
        self.transport.relay_url()
    }

    /// Chooses the relay of a device that is not provisioned (the "Relais" field), and keeps it.
    ///
    /// `InvalidInput` for a URL that is not a valid relay (`https` only, no user information,
    /// query or fragment, a valid port written with digits only, an ASCII host), for a
    /// provisioned device (its relay is the one of its space and never changes), and while a
    /// pairing or a creation is under way or pending ([`resume_pairing`](Self::resume_pairing)
    /// settles a pending one first).
    ///
    /// **Exception, once:** a provisioned device that keeps no relay (it was provisioned before
    /// the relay was kept, and this build has no default) accepts one; it is then the relay of
    /// its space for good.
    pub fn set_relay(&self, url: &str) -> Result<()> {
        let base = check_relay_url(url)?;
        // Held for the whole change: no session starts, none ends, meanwhile.
        let state = self.gate();
        if state.sessions > 0 || state.exclusive {
            return Err(CoreError::input("a pairing is in progress on this device"));
        }
        if self.is_provisioned() {
            let kept = self.store.read(|db| db.meta_get_string(RELAY_URL_KEY))?;
            if kept.is_some() {
                return Err(CoreError::input(
                    "this device belongs to a space: its relay cannot change",
                ));
            }
            // Provisioned before the relay was kept: this relay becomes its own, once.
        } else if self.has_pending_pairing()? {
            return Err(CoreError::input(
                "a pairing is pending on this device: settle it first",
            ));
        }
        let before = self.transport.relay_url();
        self.point_transport_at(Some(&base.url))?;
        let kept = self
            .store
            .tx(|db| db.meta_set(RELAY_URL_KEY, base.url.as_bytes()));
        if kept.is_err() {
            // What is not kept is not used.
            let _ = self.point_transport_at(before.as_deref());
        }
        drop(state);
        kept
    }

    /// Points the transport at `url` (`None`: no relay) unless it already is. The caller holds
    /// the relay gate (or is the only one that can: opening).
    pub(crate) fn point_transport_at(&self, url: Option<&str>) -> Result<()> {
        let want = url.and_then(normalised);
        if self.transport.relay_url() == want {
            return Ok(());
        }
        self.transport
            .set_relay(want.as_deref())
            .map_err(Into::into)
    }

    /// The relay a device falls back to: the one it kept, else the default of the build.
    fn base_relay(&self) -> Option<String> {
        self.store
            .read(|db| db.meta_get_string(RELAY_URL_KEY))
            .ok()
            .flatten()
            .or_else(|| self.relay_default.clone())
    }

    /// Opening: decides the relay (see the module documentation). Never fails the opening: a
    /// relay that cannot be used leaves the device without one.
    pub(crate) fn load_relay(&self) {
        let read = self.store.read(|db| {
            Ok((
                db.meta_get_string(RELAY_URL_KEY)?,
                db.meta_get_string(PENDING_RELAY_KEY)?,
                db.secret_exists(JOINER_PENDING)? || db.secret_exists(FOUNDER_PENDING)?,
            ))
        });
        let Ok((kept, pending, has_pending)) = read else {
            return;
        };
        let provisioned = self.is_provisioned();
        let chosen = if !provisioned && has_pending {
            pending.or_else(|| kept.clone())
        } else {
            kept.clone()
        }
        .or_else(|| self.relay_default.clone());
        // A device provisioned before the relay was kept (it was the build's): that relay is its
        // own from now on, whatever the build says later. Without a default it has none, and
        // `set_relay` gives it one, once.
        if provisioned && kept.is_none() {
            if let Some(url) = chosen.as_deref().and_then(normalised) {
                let _ = self
                    .store
                    .tx(|db| db.meta_set(RELAY_URL_KEY, url.as_bytes()));
            }
        }
        if self.point_transport_at(chosen.as_deref()).is_err() {
            // Not usable (a URL a newer build refuses): no relay rather than the wrong one.
            let _ = self.transport.set_relay(None);
        }
    }

    /// Puts the relay back to the one the device keeps (or the default of the build), unless a
    /// session holds the relay (it puts it back itself when it ends).
    pub(crate) fn restore_base_relay(&self) {
        let state = self.gate();
        if state.sessions == 0 && !state.exclusive {
            let _ = self.point_transport_at(self.base_relay().as_deref());
        }
    }

    /// Writes the relay of the pairing that is sealing its pending record, in the transaction of
    /// that record.
    pub(crate) fn stage_pending_relay(&self, db: &Db<'_>) -> Result<()> {
        match self.transport.relay_url() {
            Some(url) => db.meta_set(PENDING_RELAY_KEY, url.as_bytes()),
            None => db.meta_delete(PENDING_RELAY_KEY),
        }
    }

    /// Starts a pairing or a creation at a relay (new device, founder), before anything is
    /// consumed. `explicit` is the relay the user typed (a full URL), `from_link` the origin read
    /// in a link or a QR code (`https://<authority>`); when both are given they must name the
    /// same authority. Reached through [`RelayHold::begin`], which gives the session back on
    /// every path.
    ///
    /// The relay is, in this order: `explicit`; the origin of the link, unless it is the one
    /// already used (then its prefix, if any, stays); the current relay. A device that has none
    /// answers `RelayNotConfigured`. If another relay is wanted the transport is pointed at it
    /// now, for the duration of the session: [`end_relay_session`](Self::end_relay_session) puts
    /// it back unless the device was provisioned or sealed a pending record. A candidate that is
    /// not a valid relay is `InvalidInput`, with nothing switched.
    ///
    /// Everything is decided **under the relay gate**, and the session is counted only when no
    /// step can fail any more: a refusal leaves the transport and the count as they were. Several
    /// sessions at the same relay may overlap (one sealed batch at a time is checked when it is
    /// sealed); one that needs another relay is `InvalidInput` while another is active. A device
    /// that became provisioned meanwhile is refused (`InvalidInput`).
    fn begin_relay_session(&self, explicit: Option<&str>, from_link: Option<&str>) -> Result<()> {
        let explicit = explicit
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(check_relay_url)
            .transpose()?;
        let link = from_link.map(check_relay_url).transpose()?;
        if let (Some(e), Some(l)) = (&explicit, &link) {
            if e.relay_authority() != l.relay_authority() {
                return Err(CoreError::input(
                    "the link and the relay entered name different relays",
                ));
            }
        }
        let mut state = self.gate();
        if state.exclusive {
            return Err(CoreError::input(
                "another pairing is in progress on this device",
            ));
        }
        // Checked again under the gate: the caller's check may be stale (a session of another
        // thread provisioned the device since), and a provisioned device never changes relay.
        if self.is_provisioned() {
            return Err(CoreError::input("device already provisioned"));
        }
        let current = self.transport.relay_url();
        let target = match (&explicit, &link) {
            (Some(e), _) => Some(e.url.clone()),
            (None, Some(l)) => {
                let same = current.as_deref().and_then(authority_of).as_deref()
                    == Some(l.relay_authority().as_str());
                (!same).then(|| l.url.clone())
            }
            (None, None) => None,
        };
        if target.is_none() && current.is_none() {
            return Err(CoreError::RelayNotConfigured);
        }
        let switches = target
            .as_deref()
            .is_some_and(|t| Some(t) != current.as_deref());
        if switches {
            if state.sessions > 0 {
                return Err(CoreError::input(
                    "another pairing is in progress on this device",
                ));
            }
            // A sealed batch that is still there belongs to the relay the transport points at (a
            // session of another thread that just ended, `resume_pairing` having found it held):
            // moving the transport would have it probed at the wrong relay and dropped. The
            // founder's keys are not concerned, `drop_founder_of_another_relay` handles them.
            if self
                .store
                .read(|db| db.secret_exists(JOINER_PENDING))
                .unwrap_or(true)
            {
                return Err(CoreError::input(
                    "a pairing is pending on this device: settle it first",
                ));
            }
        }
        if let Some(url) = target {
            self.point_transport_at(Some(&url))?;
        }
        state.sessions += 1;
        Ok(())
    }

    /// The pairing or the creation started by [`begin_relay_session`](Self::begin_relay_session)
    /// is over, whatever the way (through [`RelayHold::release`]).
    fn end_relay_session(&self) {
        let mut state = self.gate();
        state.sessions = state.sessions.saturating_sub(1);
        self.settle_with(&state);
    }

    /// A device that is not provisioned, with nothing pending and no session under way, goes
    /// back to the relay it keeps; its pending relay is forgotten. Idempotent.
    pub(crate) fn settle_relay(&self) {
        let state = self.gate();
        self.settle_with(&state);
    }

    /// [`settle_relay`](Self::settle_relay), with the gate held (`state`).
    fn settle_with(&self, state: &RelayState) {
        if state.sessions > 0 || state.exclusive || self.is_provisioned() {
            return;
        }
        if self.has_pending_pairing().unwrap_or(true) {
            return;
        }
        let _ = self.store.tx(|db| db.meta_delete(PENDING_RELAY_KEY));
        let _ = self.point_transport_at(self.base_relay().as_deref());
    }

    /// Sessions holding the relay (tests).
    #[cfg(test)]
    pub(crate) fn relay_sessions(&self) -> u32 {
        self.gate().sessions
    }

    /// For a creation that holds one session (its own): takes the relay for itself, to move the
    /// transport to another relay and back (the probe of the keys of an earlier attempt). It
    /// fails with `InvalidInput`, nothing moved, if another session is active, or another
    /// decision about the relay is being taken. While the value lives every other start of a
    /// session and `set_relay` is refused, and nothing settles the relay.
    pub(crate) fn exclusive_relay(&self) -> Result<ExclusiveRelay<'_>> {
        let mut state = self.gate();
        if state.exclusive || state.sessions > 1 {
            return Err(CoreError::input(
                "another pairing is in progress on this device",
            ));
        }
        state.exclusive = true;
        Ok(ExclusiveRelay(self))
    }
}
