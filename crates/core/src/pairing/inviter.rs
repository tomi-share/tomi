//! The inviter (A): an existing member that shows a code and adds a new device
//! (`spec/pairing-v1.md` §7.2).
//!
//! A pure state machine, with no network: the caller deposits the frames it returns and feeds
//! the frames it reads from the relay channel, in order. Steps A1 and A2 (synchronisation, the
//! channel) happen before [`Inviter::start`]; A12 and A13 (`POST /v1/devices`, committing the
//! staged trust store) happen after [`Inviter::finish`].
//!
//! ```text
//! start()               -> commit frame (A3), code (A4)        AwaitShare
//! receive(share)        -> reveal frame (A5, A6)               AwaitJoinerHello
//! receive(joiner_hello) -> SAS + peer to show (A7)             AwaitConfirmations
//! confirm()  +  receive(joiner_confirm)          (A8, A9, any order)
//! provision()           -> provision frame (A10)               AwaitAck
//! receive(joiner_ack)   -> done (A11)                          Done
//! finish()              -> InviterOutcome (A12, A13)
//! ```
//!
//! Frames arrive one at a time and in this order only (§7.4): any other frame, a second copy of
//! one, a bad size or a bad version is a failure, except frames after the last expected one,
//! which are ignored. Every failure wipes the secrets of the session.

use zeroize::Zeroizing;

use super::batch;
use super::code::{check_channel_id, check_relay_authority, PairingCode};
use super::cpace::{self, Role, Share, POINT_LEN};
use super::entropy::Entropy;
use super::frame::{self, FrameKind, COMMIT_FRAME_LEN, SHARE_FRAME_LEN, TAG_LEN};
use super::info::{HistoryAnnouncement, HistoryOffer, PeerInfo, Sas};
use super::session::{self, SessionKeys, SID_LEN};
use super::tlv::{is_valid_name, is_valid_platform, InviterHello, JoinerConfirm, JoinerHello};
use crate::crypto::SpaceKey;
use crate::device::{DeviceKeys, DevicePublic, TrustStore};
use crate::error::{CoreError, Result};

/// Parameters of an invitation.
#[derive(Debug, Clone)]
pub struct InviterParams {
    /// Authority (host, and port if not 443) of the relay URL the device retained, in
    /// lowercase ASCII (`relay.example.org`). Read from the transport: a relay named by a link or a
    /// field is shown to the user and confirmed first (`pairing-v1.md` §4.4).
    pub relay_authority: String,
    /// The channel the relay just opened (`POST /v1/pairing/channels`).
    pub channel_id: String,
    /// Name of this device, shown on the new device.
    pub device_name: String,
    /// Platform of this device (`windows`, `android`...).
    pub platform: String,
    /// What this device can transfer to the new device, if the relay allows transfers.
    pub history_offer: Option<HistoryOffer>,
}

/// Where the inviter is in the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviterPhase {
    /// The code is shown; waiting for `share` (A5).
    AwaitShare,
    /// Waiting for `joiner_hello` (A7).
    AwaitJoinerHello,
    /// Waiting for the user on A and for `joiner_confirm` (A8, A9).
    AwaitConfirmations,
    /// `provision` sent; waiting for `joiner_ack` (A11).
    AwaitAck,
    /// `joiner_ack` received: register the device (A12).
    Done,
    /// Cancelled by the user.
    Cancelled,
    /// Failed; the secrets are wiped.
    Failed,
}

/// What a received frame taught the inviter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviterEvent {
    /// `share` accepted, `reveal` to deposit (A6).
    Revealed,
    /// `joiner_hello` opened: show the SAS and the new device (A7).
    PeerHello,
    /// `joiner_confirm` received: the user on the new device accepted (A9).
    JoinerConfirmed,
    /// `joiner_ack` received: the new device verified and kept the batch (A11).
    Acknowledged,
    /// A frame after the last expected one: ignored.
    Ignored,
}

/// Result of [`Inviter::receive`].
#[derive(Debug)]
pub struct InviterStep {
    /// A frame to deposit in the channel, if any.
    pub send: Option<Vec<u8>>,
    pub event: InviterEvent,
}

/// What the inviter needs to register the new device and to start the history export
/// (A12, A13).
#[derive(Debug)]
pub struct InviterOutcome {
    /// The new device's verified public keys, built into `member_list`/`device_cert`. After
    /// `POST /v1/devices` → `201`, apply it by adding this peer to the **current** trust store in
    /// the same transaction as the device row: `current.add_verified_peer(&outcome.peer.public)`.
    /// Do NOT replace the live store with this value — it was built from a read of the store and
    /// would drop any peer learned since. Drop it entirely if the registration did not succeed.
    pub staged_trust: TrustStore,
    /// The new device: name and platform for `POST /v1/devices`, keys for the local row.
    pub peer: PeerInfo,
    /// `DeviceCert(N)`, wire form, for `POST /v1/devices` (`certificate`, base64).
    pub device_cert: Vec<u8>,
    /// Space key epoch of the batch, for `POST /v1/devices` (`key_epoch`).
    pub key_epoch: u32,
    /// The announced history transfer: export with exactly this `transfer_id` after the `201`.
    pub history: Option<HistoryAnnouncement>,
}

enum State {
    AwaitShare {
        share: Share,
    },
    AwaitJoinerHello {
        keys: SessionKeys,
    },
    AwaitConfirmations {
        keys: SessionKeys,
        joiner: JoinerHello,
        user_confirmed: bool,
        joiner_confirm: Option<JoinerConfirm>,
    },
    AwaitAck {
        keys: SessionKeys,
        outcome: Box<InviterOutcome>,
    },
    Done {
        outcome: Option<Box<InviterOutcome>>,
    },
    Cancelled,
    Failed,
}

impl State {
    fn phase(&self) -> InviterPhase {
        match self {
            State::AwaitShare { .. } => InviterPhase::AwaitShare,
            State::AwaitJoinerHello { .. } => InviterPhase::AwaitJoinerHello,
            State::AwaitConfirmations { .. } => InviterPhase::AwaitConfirmations,
            State::AwaitAck { .. } => InviterPhase::AwaitAck,
            State::Done { .. } => InviterPhase::Done,
            State::Cancelled => InviterPhase::Cancelled,
            State::Failed => InviterPhase::Failed,
        }
    }
}

/// The inviter's side of a pairing session.
pub struct Inviter {
    state: State,
    /// No frame received yet (an unknown version there is `pairing_unsupported`).
    first: bool,
    sid: [u8; SID_LEN],
    ada: Vec<u8>,
    adb: Vec<u8>,
    hello: InviterHello,
    own_public: DevicePublic,
    /// Snapshot of the trust store at the start of the session (public data).
    trust: TrustStore,
    entropy: Entropy,
    code: Option<PairingCode>,
    commit_frame: Vec<u8>,
    sas: Option<Sas>,
    peer: Option<PeerInfo>,
}

impl Inviter {
    /// Starts an invitation (A3): draws the code, `sid` and the CPace share, and builds the
    /// `commit` frame. `keys` and `trust` are those of this device, which must be a member of the
    /// space; they are read here and not kept (the batch needs them again in
    /// [`provision`](Self::provision)).
    ///
    /// The code must not be shown before the `commit` frame is deposited (§4.2).
    pub fn start(params: InviterParams, keys: &DeviceKeys, trust: &TrustStore) -> Result<Self> {
        Self::start_with(params, keys, trust, Entropy::os())
    }

    /// Entropy draw order: rank 1 (2 bytes), rank 2 (2 bytes), `sid` (16), `ya` (32); then, in
    /// [`provision`](Self::provision), the sealing ephemeral secret (32), the sealing nonce (24)
    /// and, when a history is announced, the transfer id (16).
    pub(super) fn start_with(
        params: InviterParams,
        keys: &DeviceKeys,
        trust: &TrustStore,
        mut entropy: Entropy,
    ) -> Result<Self> {
        check_relay_authority(&params.relay_authority)?;
        check_channel_id(&params.channel_id)?;
        if !is_valid_name(&params.device_name) || !is_valid_platform(&params.platform) {
            return Err(CoreError::input("invalid device name or platform"));
        }
        let own_public = keys.public();
        if !trust.is_member(&own_public.ed25519) || trust.is_revoked(&own_public.ed25519) {
            return Err(CoreError::input("this device is not a member of the space"));
        }
        let (ed_pk, x_pk) = own_public.arrays()?;
        let member_count = u16::try_from(trust.members().len()).ok();
        let hello = InviterHello {
            ed_pk,
            x_pk,
            name: params.device_name,
            platform: params.platform,
            member_count,
            history_offer: params.history_offer.filter(|o| o.items > 0),
        };

        let code = PairingCode::generate_with(&params.channel_id, &mut entropy)?;
        let sid = entropy.array::<SID_LEN>()?;
        let ci = session::channel_identifier(&params.relay_authority, &params.channel_id);
        let g = cpace::generator(code.prs().as_slice(), &ci, &sid);
        let share = Share::sample(&g, &mut entropy)?;
        let commit = session::commitment(&sid, share.public());
        let mut payload = Vec::with_capacity(SID_LEN + commit.len());
        payload.extend_from_slice(&sid);
        payload.extend_from_slice(&commit);
        let commit_frame = frame::encode(FrameKind::Commit, &payload)?;
        debug_assert_eq!(commit_frame.len(), COMMIT_FRAME_LEN);

        Ok(Self {
            state: State::AwaitShare { share },
            first: true,
            sid,
            ada: session::ad_inviter(),
            adb: session::ad_joiner(),
            hello,
            own_public,
            trust: trust.clone(),
            entropy,
            code: Some(code),
            commit_frame,
            sas: None,
            peer: None,
        })
    }

    /// The phase of the sequence.
    pub fn phase(&self) -> InviterPhase {
        self.state.phase()
    }

    /// The code to show (A4); `None` once the session ended. Secret.
    pub fn code(&self) -> Option<&PairingCode> {
        self.code.as_ref()
    }

    /// The first frame to deposit in the channel (A3), before the code is shown.
    pub fn commit_frame(&self) -> &[u8] {
        &self.commit_frame
    }

    /// The verification code, once the new device said hello (A7).
    pub fn sas(&self) -> Option<&Sas> {
        self.sas.as_ref()
    }

    /// The new device, as it announced itself (A7). Its keys are authentic if the user confirms
    /// that both screens show the same verification code.
    pub fn peer(&self) -> Option<&PeerInfo> {
        self.peer.as_ref()
    }

    /// Handles the next frame read from the channel (the other side's messages, in index order).
    ///
    /// On error the session is over: wipe, close the channel, create a new one. The error is
    /// `pairing_failed` whatever the cause, except `pairing_unsupported` (unknown version in the
    /// first frame, or an unknown critical field) and operational errors.
    pub fn receive(&mut self, frame: &[u8]) -> Result<InviterStep> {
        let first = std::mem::replace(&mut self.first, false);
        let state = std::mem::replace(&mut self.state, State::Failed);
        match self.receive_in(state, frame, first) {
            Ok(step) => Ok(step),
            Err(e) => {
                if self.state.phase() != InviterPhase::Cancelled {
                    self.state = State::Failed;
                }
                self.wipe();
                Err(super::normalize(e))
            }
        }
    }

    fn receive_in(&mut self, state: State, frame: &[u8], first: bool) -> Result<InviterStep> {
        match state {
            State::Done { outcome } => {
                self.state = State::Done { outcome };
                return Ok(InviterStep {
                    send: None,
                    event: InviterEvent::Ignored,
                });
            }
            State::Cancelled => {
                self.state = State::Cancelled;
                return Err(CoreError::PairingCancelled);
            }
            State::Failed => return Err(CoreError::PairingFailed),
            _ => {}
        }
        let (kind, payload) = frame::split(frame, first)?;
        let kind = FrameKind::from_byte(kind).ok_or(CoreError::PairingFailed)?;
        match (state, kind) {
            (State::AwaitShare { share }, FrameKind::Share) => {
                self.on_share(share, frame.len(), payload)
            }
            (State::AwaitJoinerHello { keys }, FrameKind::JoinerHello) => {
                self.on_joiner_hello(keys, payload)
            }
            (
                State::AwaitConfirmations {
                    keys,
                    joiner,
                    user_confirmed,
                    joiner_confirm: None,
                },
                FrameKind::JoinerConfirm,
            ) => self.on_joiner_confirm(keys, joiner, user_confirmed, payload),
            (State::AwaitAck { keys, outcome }, FrameKind::JoinerAck) => {
                self.on_ack(keys, outcome, payload)
            }
            // A frame of another type, or a second copy of one already received.
            _ => Err(CoreError::PairingFailed),
        }
    }

    /// A5, A6: `Yb` received; derive the keys and reveal `Ya` with `InviterHello`.
    fn on_share(&mut self, share: Share, frame_len: usize, payload: &[u8]) -> Result<InviterStep> {
        if frame_len != SHARE_FRAME_LEN {
            return Err(CoreError::PairingFailed);
        }
        let yb: [u8; POINT_LEN] = payload.try_into().map_err(|_| CoreError::PairingFailed)?;
        // decode(Yb) failing or K = I aborts here, as a wrong code does later.
        let isk = share.finish(Role::Initiator, &yb, &self.sid, &self.ada, &self.adb)?;
        let (keys, sas) = session::derive(&isk)?;
        let body = Zeroizing::new(self.hello.encode()?);
        let boxed = frame::seal_body(&keys, FrameKind::Reveal, &self.sid, &body)?;
        let mut reveal = Vec::with_capacity(POINT_LEN + boxed.len());
        reveal.extend_from_slice(share.public());
        reveal.extend_from_slice(&boxed);
        let reveal = frame::encode(FrameKind::Reveal, &reveal)?;
        self.sas = Some(sas);
        self.state = State::AwaitJoinerHello { keys };
        Ok(InviterStep {
            send: Some(reveal),
            event: InviterEvent::Revealed,
        })
    }

    /// A7: open `joiner_hello`. A failure to open is a wrong code (§11.1).
    fn on_joiner_hello(&mut self, keys: SessionKeys, payload: &[u8]) -> Result<InviterStep> {
        let body = frame::open_body(&keys, FrameKind::JoinerHello, &self.sid, payload)?;
        let joiner = JoinerHello::decode(&body)?;
        let public = DevicePublic {
            ed25519: joiner.ed_pk.to_vec(),
            x25519: joiner.x_pk.to_vec(),
        };
        // The keys must decode; a device already in the space, or removed from it, must reinstall.
        public.arrays()?;
        if self.trust.is_member(&public.ed25519) || self.trust.is_revoked(&public.ed25519) {
            return Err(CoreError::PairingFailed);
        }
        self.peer = Some(PeerInfo {
            name: joiner.name.clone(),
            platform: joiner.platform.clone(),
            public,
        });
        self.state = State::AwaitConfirmations {
            keys,
            joiner,
            user_confirmed: false,
            joiner_confirm: None,
        };
        Ok(InviterStep {
            send: None,
            event: InviterEvent::PeerHello,
        })
    }

    /// A9: the user on the new device accepted, with or without the history.
    fn on_joiner_confirm(
        &mut self,
        keys: SessionKeys,
        joiner: JoinerHello,
        user_confirmed: bool,
        payload: &[u8],
    ) -> Result<InviterStep> {
        let body = frame::open_body(&keys, FrameKind::JoinerConfirm, &self.sid, payload)?;
        let confirm = JoinerConfirm::decode(&body)?;
        self.state = State::AwaitConfirmations {
            keys,
            joiner,
            user_confirmed,
            joiner_confirm: Some(confirm),
        };
        Ok(InviterStep {
            send: None,
            event: InviterEvent::JoinerConfirmed,
        })
    }

    /// A11: `joiner_ack` is an empty body: only its authentication tag matters.
    fn on_ack(
        &mut self,
        keys: SessionKeys,
        outcome: Box<InviterOutcome>,
        payload: &[u8],
    ) -> Result<InviterStep> {
        if payload.len() != TAG_LEN {
            return Err(CoreError::PairingFailed);
        }
        let body = frame::open_body(&keys, FrameKind::JoinerAck, &self.sid, payload)?;
        if !body.is_empty() {
            return Err(CoreError::PairingFailed);
        }
        // The session keys are not needed any more.
        drop(keys);
        self.state = State::Done {
            outcome: Some(outcome),
        };
        Ok(InviterStep {
            send: None,
            event: InviterEvent::Acknowledged,
        })
    }

    /// The user on this device confirmed that both screens show the same verification code and
    /// pressed "Add" (A8). Only valid once the new device said hello.
    pub fn confirm(&mut self) -> Result<()> {
        match &mut self.state {
            State::AwaitConfirmations { user_confirmed, .. } => {
                *user_confirmed = true;
                Ok(())
            }
            _ => Err(CoreError::input("no new device waiting for confirmation")),
        }
    }

    /// `true` when both the user on this device and the user on the new device confirmed:
    /// [`provision`](Self::provision) may be called (A10).
    pub fn ready_to_provision(&self) -> bool {
        matches!(
            &self.state,
            State::AwaitConfirmations {
                user_confirmed: true,
                joiner_confirm: Some(_),
                ..
            }
        )
    }

    /// Builds the batch and returns the `provision` frame to deposit (A10).
    ///
    /// Valid only when [`ready_to_provision`](Self::ready_to_provision) is true: the space key
    /// leaves this device only after both humans confirmed. `keys` and `space_key` must be those
    /// of this device at the current epoch (`InvalidInput` otherwise, the session stays usable).
    /// Everything else that fails (a space too large for one frame, 120 members or more, for
    /// instance) ends the session with `pairing_failed` before anything is sent.
    ///
    /// `trust` is the inviter's trust store **read now** (at A10), not the snapshot taken at
    /// `start` (A3): a member added by another pairing between A3 and A10 must be in N's
    /// `MemberList`, or N would never learn of it (its cursor starts at `joined_seq(N)`). It must
    /// be the same space and epoch as the snapshot and still list this device as a non-revoked
    /// member (a revocation moves the epoch on, so this also rejects a stale epoch). The
    /// inviter's real store is not touched: the result is in [`InviterOutcome::staged_trust`],
    /// which the caller applies **by adding the new peer to the current store** (not by
    /// replacing it), in the same transaction as the device row, after `POST /v1/devices` → 201.
    pub fn provision(
        &mut self,
        keys: &DeviceKeys,
        trust: &TrustStore,
        space_key: &SpaceKey,
        now_ms: u64,
    ) -> Result<Vec<u8>> {
        if !self.ready_to_provision() {
            return Err(CoreError::input(
                "both users must confirm before the space key is sent",
            ));
        }
        if keys.public() != self.own_public {
            return Err(CoreError::input("keys are not those of the invitation"));
        }
        // The fresh store must be the same space, owned by this device, at the invitation's
        // epoch. A rotation (which carries any revocation) moves the epoch, so this rejects a
        // store whose membership changed under a revocation: the caller then restarts (A2).
        if trust.space_id_bytes() != self.trust.space_id_bytes() {
            return Err(CoreError::input("trust store is of another space"));
        }
        if trust.key_epoch() != self.trust.key_epoch()
            || space_key.epoch() != self.trust.key_epoch()
        {
            return Err(CoreError::input(
                "space key or trust epoch differs from the invitation",
            ));
        }
        let own_ed = &self.own_public.ed25519;
        if !trust.is_member(own_ed) || trust.is_revoked(own_ed) {
            return Err(CoreError::input(
                "this device is no longer a member of the space",
            ));
        }
        let State::AwaitConfirmations {
            keys: session_keys,
            joiner,
            joiner_confirm: Some(confirm),
            ..
        } = std::mem::replace(&mut self.state, State::Failed)
        else {
            return Err(CoreError::PairingFailed);
        };
        let result = self.build_provision(
            keys,
            trust,
            space_key,
            now_ms,
            session_keys,
            &joiner,
            confirm,
        );
        if result.is_err() {
            self.wipe();
        }
        result.map_err(super::normalize)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_provision(
        &mut self,
        keys: &DeviceKeys,
        trust: &TrustStore,
        space_key: &SpaceKey,
        now_ms: u64,
        session_keys: SessionKeys,
        joiner: &JoinerHello,
        confirm: JoinerConfirm,
    ) -> Result<Vec<u8>> {
        let peer = DevicePublic {
            ed25519: joiner.ed_pk.to_vec(),
            x25519: joiner.x_pk.to_vec(),
        };
        // The history is announced only if the user of N asked for it and A offered some.
        let announce = self.hello.history_offer.filter(|_| confirm.history);
        let built = batch::build(
            trust,
            keys,
            space_key,
            &peer,
            now_ms,
            announce,
            &mut self.entropy,
        )?;
        let boxed = frame::seal_body(&session_keys, FrameKind::Provision, &self.sid, &built.body)?;
        // The size limit of a frame (8192 bytes) is checked here: a space with 120 members or
        // more fails before anything is deposited, and is never split.
        let provision = frame::encode(FrameKind::Provision, &boxed)?;
        let peer_info = PeerInfo {
            name: joiner.name.clone(),
            platform: joiner.platform.clone(),
            public: peer,
        };
        let outcome = InviterOutcome {
            key_epoch: built.staged.key_epoch(),
            staged_trust: built.staged,
            peer: peer_info,
            device_cert: built.device_cert,
            history: built.history,
        };
        self.state = State::AwaitAck {
            keys: session_keys,
            outcome: Box::new(outcome),
        };
        Ok(provision)
    }

    /// Takes the result once `joiner_ack` was received (A12, A13). The code and the verification
    /// code are erased: the session is over.
    pub fn finish(&mut self) -> Result<InviterOutcome> {
        let outcome = match &mut self.state {
            State::Done { outcome } => outcome
                .take()
                .map(|o| *o)
                .ok_or_else(|| CoreError::input("outcome already taken"))?,
            _ => return Err(CoreError::input("the new device has not acknowledged yet")),
        };
        self.wipe();
        Ok(outcome)
    }

    /// The user refused (codes differ, "Cancel", back) or left the screen: wipes the session.
    /// Close the channel afterwards (`DELETE /v1/pairing/channels/{channel_id}`).
    pub fn cancel(&mut self) {
        self.state = State::Cancelled;
        self.wipe();
    }

    /// Erases what the session must not outlive.
    fn wipe(&mut self) {
        self.code = None;
        self.sas = None;
        self.peer = None;
    }
}

impl std::fmt::Debug for Inviter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inviter")
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}
