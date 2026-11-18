//! The inviter (A): an existing member that shows a code and adds a new device
//! (`spec/pairing-v1.md` §7.2, §8.1, §8.2, §9).
//!
//! ```text
//! start_invitation()   A1 sync and checks, A2 channel, A3 commit, A4 code and link
//! wait_for_peer()      A5 share, A6 reveal, A7 joiner_hello: the code to compare, the new device
//! confirm()            A8 the user pressed "Add" (the batch goes out once N confirmed too)
//! wait_done()          A9 joiner_confirm, A10 provision, A11 joiner_ack, A12 POST /v1/devices,
//!                      A13 the new device joins the trust store; the export is announced
//! cancel()             at any time: the channel is closed, the secrets of the session wiped
//! ```
//!
//! `poll` is the step all of them are made of. The lock that serialises the calls (`driver`) is
//! never needed by `cancel`, `phase` or `prompt`, which therefore answer at once even while a
//! long poll is running on another thread.

use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use super::history::push_export_intent;
use super::{
    guard, idle_pause, is_transient, retrying, Cursor, Link, PairingPeer, Role, LONG_POLL_SECS,
};
use crate::device::DevicePublic;
use crate::error::{CoreError, Result};
use crate::pairing::{
    HistoryAnnouncement, HistoryOffer, Inviter, InviterEvent, InviterOutcome, InviterParams,
    InviterPhase,
};
use crate::runtime::engine::{blocked_err, Blocked, Engine};
use crate::runtime::store::{DeviceRow, DeviceState, FileState, ItemStatus, SendStatus};
use crate::runtime::transport::{ApiProblem, ChannelState, DeviceRecord, NewDevice, RelayError};

/// Where an invitation is, for the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitePhase {
    /// The code is shown; the new device has not said hello yet.
    AwaitPeer,
    /// The verification code and the new device are available ([`InviteSession::prompt`]):
    /// compare, then [`confirm`](InviteSession::confirm) or [`cancel`](InviteSession::cancel).
    Verify,
    /// Both users confirmed: the batch is exchanged and the device registered.
    Provisioning,
    /// The new device is part of the space.
    Done,
    /// Cancelled, here or on the other device, or the channel expired.
    Cancelled,
    /// Failed (wrong code, refusal of the relay...). Read the error of the call that failed.
    Failed,
}

/// What the user of the inviter compares and decides on (A7).
#[derive(Clone, PartialEq, Eq)]
pub struct VerifyPrompt {
    /// The verification code in groups of three digits (`"482 913"`). Secret: show it, never
    /// log it, never ask the user to type it.
    pub sas: String,
    /// The new device, as it announced itself (authentic if the codes are the same).
    pub peer: PairingPeer,
}

impl std::fmt::Debug for VerifyPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyPrompt")
            .field("sas", &"<redacted>")
            .field("peer", &self.peer)
            .finish()
    }
}

/// The new device is registered and in the trust store of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteOutcome {
    pub device_id: String,
    pub name: String,
    pub platform: String,
    /// The history transfer announced to the new device. Its export is a task, recorded with
    /// the device, that the next [`sync`](Engine::sync) starts and runs (building and uploading
    /// the history can take a while, so `wait_done` does not wait for it): call `sync` once the
    /// screen says the device was added.
    pub history: Option<HistoryAnnouncement>,
}

/// The registration of the new device at the relay, after its acknowledgement (A12, A13).
struct Registration {
    outcome: InviterOutcome,
    /// An attempt ended without a usable answer: the relay may have registered the device.
    uncertain: bool,
    /// The relay registered the device (`201`, or found in the list after a lost answer).
    record: Option<DeviceRecord>,
}

struct Shared {
    machine: Inviter,
    /// The user of this device pressed "Add".
    confirmed: bool,
    /// A `POST /v1/devices` may have reached the relay (set before the first attempt). From
    /// then on, this device never closes the channel itself: a `completed` channel released the
    /// new device, and closing it (cancel, expiry, a late error) would turn `completed` into
    /// `closed`, which the new device reads as "give up" and drops its batch while the relay has
    /// registered it. The channel is left to the relay's own deadline instead.
    registering: bool,
    done: Option<InviteOutcome>,
    failure: Option<CoreError>,
}

struct Driver {
    cur: Cursor,
    registration: Option<Registration>,
}

/// One invitation, from the code to the registered device. See the module documentation.
///
/// Call [`cancel`](InviteSession::cancel) when the screen closes: dropping the handle closes
/// nothing (no network call is made on a drop), and the relay then keeps the channel until its
/// deadline, which counts against the two channels it allows at once.
pub struct InviteSession {
    wire: Link,
    /// Key epoch when the invitation started: a rotation since would make the batch stale.
    key_epoch: u32,
    expires_at_ms: i64,
    shared: Mutex<Shared>,
    driver: Mutex<Driver>,
}

impl std::fmt::Debug for InviteSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InviteSession")
            .field("channel_id", &self.wire.channel_id)
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Starts an invitation (A1 to A4): synchronises, checks that the space can take another
    /// device, opens a channel, deposits the engagement of the PAKE, and returns the session
    /// whose [`code`](InviteSession::code) and [`link`](InviteSession::link) the screen shows.
    ///
    /// Errors: not provisioned or blocked; offline; `Relay { 409, "too_many_devices" }` when
    /// the space is full; `Relay { 409, "channel_limit" }` when a pairing is already open.
    /// Nothing is stored: a failure leaves the device as it was.
    pub fn start_invitation(self: &Arc<Self>) -> Result<Arc<InviteSession>> {
        let active = self.active()?;
        self.ensure_usable()?;
        // A1: the epoch and the members this batch is built on are the current ones.
        if self.sync()?.offline {
            return Err(CoreError::Network {
                reason: "offline".into(),
            });
        }
        self.ensure_usable()?;
        let trust = self.read_crypto(|c, _| Ok(c.trust.clone()))?;
        let members = u64::try_from(trust.members().len()).unwrap_or(u64::MAX);
        if members >= self.limits().max_devices {
            return Err(CoreError::Relay {
                status: 409,
                code: "too_many_devices".into(),
            });
        }
        let authority = self.relay_authority()?;
        let device_name = self.local_device()?.name;
        let history_offer = self.history_offer()?;
        // A2
        let channel = self
            .authed(|| self.transport.create_pairing_channel())
            .map_err(CoreError::from)?;
        let wire = Link::new(
            self.clone(),
            channel.channel_id.clone(),
            Role::Inviter,
            Link::limit_from(self),
        );
        // A3: the code is not shown before the engagement is deposited (§4.2).
        let machine = match Inviter::start(
            InviterParams {
                relay_authority: authority,
                channel_id: channel.channel_id,
                device_name,
                platform: self.cfg.platform.clone(),
                history_offer,
            },
            &active.keys,
            &trust,
        ) {
            Ok(m) => m,
            Err(e) => {
                wire.close();
                return Err(e);
            }
        };
        let mut cur = Cursor::default();
        cur.outbox.push_back(machine.commit_frame().to_vec());
        if let Err(e) = wire.flush(&mut cur) {
            wire.close();
            return Err(e);
        }
        Ok(Arc::new(InviteSession {
            wire,
            key_epoch: trust.key_epoch(),
            expires_at_ms: channel.expires_at_ms,
            shared: Mutex::new(Shared {
                machine,
                confirmed: false,
                registering: false,
                done: None,
                failure: None,
            }),
            driver: Mutex::new(Driver {
                cur,
                registration: None,
            }),
        }))
    }

    /// The device can talk to the relay (not revoked, not too old for the space; "not found"
    /// is asked again by the sync that follows) and is not leaving the space: no pairing
    /// channel during a pending departure (`api-v1.md` §7.9).
    fn ensure_usable(&self) -> Result<()> {
        if self.departure_pending() {
            return Err(CoreError::input("a departure is pending"));
        }
        match self.blocked() {
            None | Some(Blocked::NotFound) => Ok(()),
            Some(b) => Err(blocked_err(&format!("{b:?}"))),
        }
    }

    /// What this device can transfer to a new device (`pairing-v1.md` §9 step 1): the
    /// readable items that are not waiting in the outbox, and their approximate size. `None` if
    /// there is nothing to offer, or if the relay does not carry transfers.
    fn history_offer(&self) -> Result<Option<HistoryOffer>> {
        if self.limits().transfer_max_open.is_none() {
            return Ok(None);
        }
        let (items, bytes) = self.store.read(|db| {
            let mut items = 0u64;
            let mut bytes = 0u64;
            for row in db.items()? {
                let exportable = row.status == ItemStatus::Readable
                    && row.send_status.is_none_or(|s| s == SendStatus::Sent);
                if !exportable {
                    continue;
                }
                items += 1;
                bytes += row
                    .sealed
                    .as_ref()
                    .map_or(0, |s| u64::try_from(s.len()).unwrap_or(0));
                for f in db.files(&row.item_id)? {
                    if f.state == FileState::Done {
                        bytes += f.size;
                    }
                }
            }
            Ok((items, bytes))
        })?;
        Ok((items > 0).then_some(HistoryOffer { items, bytes }))
    }
}

impl InviteSession {
    /// The code to show, `4821-crayon-renard`; `None` once the session is over. Secret.
    pub fn code(&self) -> Option<String> {
        guard(&self.shared)
            .machine
            .code()
            .map(|c| c.to_code_string())
    }

    /// The link of the QR code, `https://<relay>/l/pair#<code>`; `None` once the session is
    /// over. Secret. The app draws the QR code from exactly this string.
    pub fn link(&self) -> Option<String> {
        let authority = self.wire.engine.relay_authority().ok()?;
        guard(&self.shared)
            .machine
            .code()
            .map(|c| c.link(&authority))
    }

    /// Identifier of the channel (not secret).
    pub fn channel_id(&self) -> &str {
        &self.wire.channel_id
    }

    /// When the relay drops the channel if nobody claimed it, on the relay clock
    /// (`Engine::server_now_ms` is the clock to compare it with). At the end the app starts a
    /// new invitation (§4.2).
    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    /// Where the invitation is.
    pub fn phase(&self) -> InvitePhase {
        let sh = guard(&self.shared);
        if sh.done.is_some() {
            return InvitePhase::Done;
        }
        if let Some(e) = &sh.failure {
            return if *e == CoreError::PairingCancelled {
                InvitePhase::Cancelled
            } else {
                InvitePhase::Failed
            };
        }
        match sh.machine.phase() {
            InviterPhase::AwaitShare | InviterPhase::AwaitJoinerHello => InvitePhase::AwaitPeer,
            InviterPhase::AwaitConfirmations if !sh.confirmed => InvitePhase::Verify,
            InviterPhase::AwaitConfirmations | InviterPhase::AwaitAck | InviterPhase::Done => {
                InvitePhase::Provisioning
            }
            InviterPhase::Cancelled => InvitePhase::Cancelled,
            InviterPhase::Failed => InvitePhase::Failed,
        }
    }

    /// The verification code and the new device, from the moment the new device said hello
    /// (A7) until the pairing is over.
    pub fn prompt(&self) -> Option<VerifyPrompt> {
        let sh = guard(&self.shared);
        let sas = sh.machine.sas()?;
        let peer = sh.machine.peer()?;
        Some(VerifyPrompt {
            sas: sas.grouped(),
            peer: PairingPeer {
                name: peer.name.clone(),
                platform: peer.platform.clone(),
            },
        })
    }

    /// Blocks until the new device said hello and the verification code can be compared
    /// (A5 to A7). Long polls of 25 s; returns at once if that is already the case.
    ///
    /// Errors end the invitation, except a transient one (no network): `PairingExpired` when
    /// the code ran out, `PairingCancelled` when the other device closed the channel (a wrong
    /// code looks like that from here), `PairingFailed`, `PairingUnsupported`. The app then
    /// starts a new invitation.
    pub fn wait_for_peer(&self) -> Result<VerifyPrompt> {
        loop {
            if let Some(p) = self.prompt() {
                return Ok(p);
            }
            self.ensure_running()?;
            self.wait_step()?;
        }
    }

    /// The user compared the codes and pressed "Add" (A8). Valid once the new device said
    /// hello; nothing is sent to the relay before the new device confirmed too, then the batch
    /// goes out at once. Follow with [`wait_done`](Self::wait_done).
    pub fn confirm(&self) -> Result<()> {
        let mut d = self.begin()?;
        {
            let mut sh = guard(&self.shared);
            if sh.confirmed {
                return Ok(());
            }
            sh.machine.confirm()?;
            sh.confirmed = true;
        }
        self.provision_if_ready(&mut d)
            .and_then(|()| self.wire.flush(&mut d.cur))
            .map_err(|e| self.settle(e))
    }

    /// Blocks until the new device is registered (A9 to A13). Call after
    /// [`confirm`](Self::confirm).
    pub fn wait_done(&self) -> Result<InviteOutcome> {
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
    /// for a frame), feeds the machine, and goes as far as it can: the batch when both users
    /// confirmed, the registration when the batch is acknowledged. At most one round trip to
    /// the relay for the channel, plus the registration.
    ///
    /// A transient error (no network, `429`, `5xx`) leaves the session as it was: call again.
    /// Any other error ends it.
    pub fn poll(&self, wait_secs: u32) -> Result<InvitePhase> {
        self.step(wait_secs).map(|(phase, _)| phase)
    }

    /// [`poll`](Self::poll), and whether anything happened (a frame came, the device was
    /// registered).
    fn step(&self, wait_secs: u32) -> Result<(InvitePhase, bool)> {
        let mut d = self.begin()?;
        match self.advance(&mut d, wait_secs) {
            Ok(progressed) => Ok((self.phase(), progressed)),
            Err(e) => Err(self.settle(e)),
        }
    }

    /// The user refused (the codes differ, "Cancel", back) or left the screen. Closes the
    /// channel at the relay and wipes the secrets of the session. Nothing is registered unless
    /// the new device already is.
    pub fn cancel(&self) {
        self.wire
            .cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let registering = {
            let mut sh = guard(&self.shared);
            if sh.done.is_some() || sh.failure.is_some() {
                return;
            }
            sh.failure = Some(CoreError::PairingCancelled);
            sh.machine.cancel();
            sh.registering
        };
        // Once a registration may have reached the relay, do not close the channel: that would
        // strand an already-registered new device (it reads `closed` and drops its batch). Let
        // the relay drop the channel at its deadline.
        if !registering {
            self.wire.close();
        }
    }

    // -----------------------------------------------------------------------

    fn has_confirmed(&self) -> bool {
        let sh = guard(&self.shared);
        sh.done.is_some() || sh.confirmed
    }

    /// The error of a finished session, if it did not succeed.
    fn ensure_running(&self) -> Result<()> {
        let sh = guard(&self.shared);
        match (&sh.failure, &sh.done) {
            (Some(e), _) => Err(e.clone()),
            (None, Some(_)) => Err(CoreError::input("the pairing is already over")),
            (None, None) => Ok(()),
        }
    }

    /// Takes the right to drive the session: only one call at a time reads the channel.
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

    /// Ends the session on an error that is not transient (closing the channel, wiping the
    /// secrets), and returns it. A transient error leaves everything as it is.
    fn settle(&self, e: CoreError) -> CoreError {
        if is_transient(&e) {
            return e;
        }
        let registering = {
            let mut sh = guard(&self.shared);
            if sh.failure.is_none() && sh.done.is_none() {
                sh.failure = Some(e.clone());
            }
            sh.machine.cancel();
            sh.registering
        };
        // An expired or closed channel is already gone at the relay; any other end closes it,
        // UNLESS a registration may have reached the relay: closing would then strand an
        // already-registered new device. The relay drops the channel at its deadline instead.
        if !registering && !matches!(e, CoreError::PairingExpired | CoreError::PairingCancelled) {
            self.wire.close();
        }
        e
    }

    /// Returns whether something happened (a frame was read, the device was registered).
    fn advance(&self, d: &mut Driver, wait_secs: u32) -> Result<bool> {
        if guard(&self.shared).done.is_some() {
            return Ok(false);
        }
        // The batch was acknowledged: what is left is the registration (A12, A13).
        if d.registration.is_some() {
            self.register(d)?;
            return Ok(true);
        }
        let pumped = self.wire.pump(&mut d.cur, wait_secs)?;
        let progressed = !pumped.frames.is_empty();
        let mut acknowledged = false;
        for frame in &pumped.frames {
            if self.wire.is_cancelled() {
                return Err(CoreError::PairingCancelled);
            }
            let step = guard(&self.shared).machine.receive(frame)?;
            if let Some(send) = step.send {
                d.cur.outbox.push_back(send);
            }
            if step.event == InviterEvent::Acknowledged {
                acknowledged = true;
            }
        }
        match pumped.state {
            // The new device left (or the relay says so).
            ChannelState::Closed if !acknowledged => return Err(CoreError::PairingCancelled),
            // Only this device completes a channel, and it has not yet.
            ChannelState::Completed => return Err(CoreError::PairingFailed),
            _ => {}
        }
        // The acknowledgement is taken out of the machine at once: from here the registration
        // is a task of this session, whatever else fails in this call (the acknowledgement is
        // read once, and will not come again).
        if acknowledged {
            let outcome = guard(&self.shared).machine.finish()?;
            d.registration = Some(Registration {
                outcome,
                uncertain: false,
                record: None,
            });
        }
        self.provision_if_ready(d)?;
        self.wire.flush(&mut d.cur)?;
        if d.registration.is_some() {
            self.register(d)?;
        }
        Ok(progressed)
    }

    /// A10: builds the batch once both users confirmed, from the trust store **as it is now**.
    fn provision_if_ready(&self, d: &mut Driver) -> Result<()> {
        if !guard(&self.shared).machine.ready_to_provision() {
            return Ok(());
        }
        let engine = &self.wire.engine;
        let active = engine.active()?;
        let (trust, space_key) =
            engine.read_crypto(|c, _| Ok((c.trust.clone(), c.ring.current()?.clone())))?;
        // A rotation since the start of the invitation (a revocation, for instance) makes the
        // batch stale: the relay would refuse it with `epoch_conflict`; do not even send it.
        if trust.key_epoch() != self.key_epoch {
            return Err(CoreError::Relay {
                status: 409,
                code: "epoch_conflict".into(),
            });
        }
        let now = u64::try_from(engine.clock.now_ms()).unwrap_or(0);
        let frame = guard(&self.shared)
            .machine
            .provision(&active.keys, &trust, &space_key, now)?;
        d.cur.outbox.push_back(frame);
        Ok(())
    }

    /// A12 and A13: registers the new device at the relay, then adds it to this device's trust
    /// store and device list. Safe to call again after a transient error.
    fn register(&self, d: &mut Driver) -> Result<()> {
        let engine = &self.wire.engine;
        let Some(reg) = d.registration.as_mut() else {
            return Ok(());
        };
        if reg.record.is_none() {
            if self.wire.is_cancelled() {
                return Err(CoreError::PairingCancelled);
            }
            if !reg.uncertain {
                self.check_membership_unchanged(reg)?;
            }
            let posted = self.post_device(reg);
            // From the moment the registration MAY have reached the relay — a success, or an
            // attempt with no usable answer (`uncertain`) — this device must not close the
            // channel: a `completed` channel released N, and closing it would strand an
            // already-registered N (it reads `closed` and drops its batch). A definitive refusal
            // (`too_many_devices`, `device_exists` for another device, `channel_limit`) leaves N
            // unregistered, so closing the channel to free N quickly is correct.
            if posted.is_ok() || reg.uncertain {
                guard(&self.shared).registering = true;
            }
            reg.record = Some(posted?);
        }
        let Some(record) = reg.record.clone() else {
            return Ok(());
        };
        let peer = reg.outcome.peer.clone();
        let history = reg.outcome.history.clone();
        let device_id = peer.public.device_id()?;
        // A13, one transaction: the peer is **added** to the trust store as it is now (a member
        // learned since the batch was built stays; the store staged for the batch is never
        // swapped in), with its device row and, if a history was announced, the export to run.
        engine.with_crypto(|c, db, _| {
            c.trust.add_verified_peer(&peer.public)?;
            db.put_device(&DeviceRow {
                device_id: device_id.clone(),
                name: peer.name.clone(),
                platform: peer.platform.clone(),
                ed25519: Some(peer.public.ed25519.clone()),
                x25519: Some(peer.public.x25519.clone()),
                state: DeviceState::Verified,
                joined_seq: record.joined_seq,
                last_seen_at_ms: None,
            })?;
            if let Some(h) = &history {
                push_export_intent(db, &device_id, &h.transfer_id)?;
            }
            Engine::persist_crypto(c, db)
        })?;
        engine.notify();
        guard(&self.shared).done = Some(InviteOutcome {
            device_id,
            name: peer.name,
            platform: peer.platform,
            history,
        });
        d.registration = None;
        Ok(())
    }

    /// Before the device is registered: the space is the one the batch was built for.
    ///
    /// The new device learns the members from the list in its batch, and reads the log only from
    /// its own arrival on. A member this device learned since the batch was built (another
    /// pairing completed and a sync applied it) is missing from that list, and the new device
    /// would never hear of it: the registration is not made, and the user starts again, which
    /// builds a batch from the store as it is then. A rotation since the start (a revocation)
    /// is the same case, and the relay would refuse the epoch anyway.
    fn check_membership_unchanged(&self, reg: &Registration) -> Result<()> {
        let (members, epoch) = self
            .wire
            .engine
            .read_crypto(|c, _| Ok((c.trust.members(), c.trust.key_epoch())))?;
        if epoch != reg.outcome.key_epoch {
            return Err(CoreError::Relay {
                status: 409,
                code: "epoch_conflict".into(),
            });
        }
        let in_batch = reg.outcome.staged_trust.members();
        if members.iter().any(|m| !in_batch.contains(m)) {
            return Err(CoreError::PairingFailed);
        }
        Ok(())
    }

    /// `POST /v1/devices` (`pairing-v1.md` §8.2).
    fn post_device(&self, reg: &mut Registration) -> Result<DeviceRecord> {
        let engine = &self.wire.engine;
        let request = NewDevice {
            channel_id: self.wire.channel_id.clone(),
            key_epoch: reg.outcome.key_epoch,
            name: reg.outcome.peer.name.clone(),
            platform: reg.outcome.peer.platform.clone(),
            certificate: reg.outcome.device_cert.clone(),
        };
        // An attempt that got no usable answer (no answer at all, or a server error) may have
        // been processed: the next answer then speaks of a channel already completed. A `429`
        // is an answer: the call was not processed.
        let mut unanswered = false;
        let result = retrying(Some(&self.wire.cancelled), || {
            let r = engine.authed(|| engine.transport.post_device(&request));
            if r.as_ref().err().is_some_and(no_usable_answer) {
                unanswered = true;
            }
            r
        });
        if unanswered {
            reg.uncertain = true;
        }
        match result {
            Ok(record) => Ok(record),
            Err(e) if e.is_retryable() => Err(e.into()),
            Err(RelayError::Api(p)) => {
                // A replay after a lost answer meets a channel already completed or a device
                // already known (`409`), or a channel purged since (`404`): the device is
                // registered if the relay lists it. Checking `GET /v1/devices` on `404` keeps
                // the announced history export instead of losing it to `PairingExpired`.
                if reg.uncertain
                    && (p.status == 404
                        || (p.status == 409
                            && matches!(p.code.as_str(), "channel_closed" | "device_exists")))
                {
                    if let Some(record) = self.find_registered(&reg.outcome.peer.public)? {
                        return Ok(record);
                    }
                }
                if p.code == "epoch_conflict" {
                    // A rotation passed since A1: learn it before the user starts again.
                    let _ = engine.sync();
                }
                Err(register_error(&p))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The record of the new device in `GET /v1/devices`, if the relay lists it.
    fn find_registered(&self, peer: &DevicePublic) -> Result<Option<DeviceRecord>> {
        let engine = &self.wire.engine;
        let id = peer.device_id()?;
        let list = retrying(Some(&self.wire.cancelled), || {
            engine.authed(|| engine.transport.devices())
        })
        .map_err(CoreError::from)?;
        Ok(list
            .into_iter()
            .find(|d| d.device_id == id && d.sign_pk == peer.ed25519))
    }
}

/// The request may have been processed although the caller learned nothing.
fn no_usable_answer(e: &RelayError) -> bool {
    match e {
        RelayError::Network(_) => true,
        RelayError::Api(p) => p.status >= 500,
    }
}

/// What a refusal of `POST /v1/devices` means for the pairing (`pairing-v1.md` §8.2, §11.5).
fn register_error(p: &ApiProblem) -> CoreError {
    match (p.status, p.code.as_str()) {
        // The channel is gone (expired, or not ours).
        (404, _) => CoreError::PairingExpired,
        // The new device closed it.
        (409, "channel_closed") => CoreError::PairingCancelled,
        // `epoch_conflict`, `too_many_devices`, `device_exists`, `422`...: the app says why.
        _ => CoreError::Relay {
            status: p.status,
            code: p.code.clone(),
        },
    }
}
