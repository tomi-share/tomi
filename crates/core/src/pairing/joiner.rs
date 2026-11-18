//! The joiner (N): a new device that enters the space with a code (`spec/pairing-v1.md` §7.3).
//!
//! A pure state machine, with no network: the caller deposits the frames it returns and feeds
//! the frames it reads from the relay channel, in order. Reading the code (N1), the `claim`
//! (N2) and the wait for `state = "completed"` (N12) happen outside.
//!
//! ```text
//! receive(commit)    -> share frame (N3, N4)                        AwaitReveal
//! receive(reveal)    -> joiner_hello frame, SAS + inviter (N5-N7)   AwaitUserConfirm
//! confirm(history)   -> joiner_confirm frame (N8, N9)               AwaitProvision
//! receive(provision) -> joiner_ack frame, batch verified (N10, N11) Done
//! take_provision()   -> keys, trust store, space key (N13)
//! ```
//!
//! Frames arrive one at a time and in this order only (§7.4), and a frame while the user is
//! comparing the codes is a violation like any other. Every failure wipes the session; the
//! error is `pairing_failed` whatever the cause (engagement, invalid point, key confirmation,
//! batch), so a wrong code is not an oracle.

use zeroize::{Zeroize, Zeroizing};

use super::batch;
use super::code::{check_relay_authority, PairingCode};
use super::cpace::{self, Role, Share, POINT_LEN};
use super::entropy::Entropy;
use super::frame::{self, FrameKind, COMMIT_FRAME_LEN, TAG_LEN};
use super::info::{HistoryAnnouncement, InviterInfo, PeerInfo, Sas};
use super::session::{self, SessionKeys, SID_LEN};
use super::tlv::{
    is_valid_name, is_valid_platform, InviterHello, JoinerConfirm, JoinerHello, ProvisionBody,
};
use crate::crypto::SpaceKey;
use crate::device::{DeviceKeys, DevicePublic, TrustStore};
use crate::error::{CoreError, Result};

/// Parameters of a joining session.
#[derive(Debug, Clone)]
pub struct JoinerParams {
    /// Authority of the relay the device retained (see [`super::InviterParams`]).
    pub relay_authority: String,
    /// The code, read locally (N1).
    pub code: PairingCode,
    /// Name of this device, shown on the inviter and used to register it.
    pub device_name: String,
    /// Platform of this device.
    pub platform: String,
}

/// Where the joiner is in the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinerPhase {
    /// Claimed the channel; waiting for `commit` (N3).
    AwaitCommit,
    /// `share` sent; waiting for `reveal` (N5).
    AwaitReveal,
    /// Hello sent; the user compares the codes (N7, N8).
    AwaitUserConfirm,
    /// `joiner_confirm` sent; waiting for `provision` (N10).
    AwaitProvision,
    /// Batch verified and acknowledged; waiting for the channel to complete (N12).
    Done,
    /// Cancelled by the user.
    Cancelled,
    /// Failed; the secrets are wiped.
    Failed,
}

/// What a received frame taught the joiner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinerEvent {
    /// `commit` accepted, `share` to deposit (N4).
    Shared,
    /// `reveal` verified and opened: show the SAS and the inviter (N7).
    InviterHello,
    /// `provision` verified: the batch is kept pending (N10), `joiner_ack` to deposit (N11).
    ProvisionVerified,
    /// A frame after the last expected one: ignored.
    Ignored,
}

/// Result of [`Joiner::receive`].
#[derive(Debug)]
pub struct JoinerStep {
    /// A frame to deposit in the channel, if any.
    pub send: Option<Vec<u8>>,
    pub event: JoinerEvent,
}

/// What `engine.provision` needs, plus what to tell the user (N13).
///
/// The space key and the device keys are secrets: keep them in the OS secure storage until the
/// channel completes (§8.3), then give them to the engine and erase the pending copy.
pub struct JoinerProvision {
    /// The identity this device generated for the pairing.
    pub keys: DeviceKeys,
    /// The trust store: this device, the inviter and the other members.
    pub trust: TrustStore,
    /// The current space key.
    pub space_key: SpaceKey,
    /// Name of this device.
    pub device_name: String,
    /// The inviter, as it announced itself.
    pub inviter: PeerInfo,
    /// The history transfer the inviter announced, if the user asked for it.
    pub history: Option<HistoryAnnouncement>,
}

#[cfg(feature = "runtime")]
impl JoinerProvision {
    /// The value to give to `Engine::provision`, and the announced history transfer to accept
    /// when its `history_transfer` event arrives.
    pub fn into_provision(self) -> (crate::runtime::Provision, Option<HistoryAnnouncement>) {
        (
            crate::runtime::Provision {
                keys: self.keys,
                trust: self.trust,
                space_key: self.space_key,
                device_name: self.device_name,
            },
            self.history,
        )
    }
}

impl std::fmt::Debug for JoinerProvision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinerProvision")
            .field("device_id", &self.keys.device_id())
            .field("space_id", &self.trust.space_id())
            .field("key_epoch", &self.space_key.epoch())
            .field("history", &self.history)
            .finish_non_exhaustive()
    }
}

enum State {
    AwaitCommit,
    AwaitReveal {
        sid: [u8; SID_LEN],
        commit: [u8; 32],
        share: Share,
    },
    AwaitUserConfirm {
        keys: SessionKeys,
        sid: [u8; SID_LEN],
    },
    AwaitProvision {
        keys: SessionKeys,
        sid: [u8; SID_LEN],
    },
    Done {
        provision: Option<Box<JoinerProvision>>,
    },
    Cancelled,
    Failed,
}

impl State {
    fn phase(&self) -> JoinerPhase {
        match self {
            State::AwaitCommit => JoinerPhase::AwaitCommit,
            State::AwaitReveal { .. } => JoinerPhase::AwaitReveal,
            State::AwaitUserConfirm { .. } => JoinerPhase::AwaitUserConfirm,
            State::AwaitProvision { .. } => JoinerPhase::AwaitProvision,
            State::Done { .. } => JoinerPhase::Done,
            State::Cancelled => JoinerPhase::Cancelled,
            State::Failed => JoinerPhase::Failed,
        }
    }
}

/// The joiner's side of a pairing session.
pub struct Joiner {
    state: State,
    /// No frame received yet (an unknown version there is `pairing_unsupported`).
    first: bool,
    prs: Zeroizing<[u8; 4]>,
    ci: Vec<u8>,
    ada: Vec<u8>,
    adb: Vec<u8>,
    name: String,
    platform: String,
    /// Generated for this pairing; moved into the [`JoinerProvision`] at the end.
    keys: Option<DeviceKeys>,
    entropy: Entropy,
    sas: Option<Sas>,
    inviter: Option<InviterInfo>,
}

impl Joiner {
    /// Starts joining with the code read by the user (N1) and fresh device keys (N6).
    pub fn new(params: JoinerParams) -> Result<Self> {
        Self::new_with(params, DeviceKeys::generate()?, Entropy::os())
    }

    /// Entropy draw order: `yb` (32 bytes), when `commit` is received.
    pub(super) fn new_with(
        params: JoinerParams,
        keys: DeviceKeys,
        entropy: Entropy,
    ) -> Result<Self> {
        check_relay_authority(&params.relay_authority)?;
        if !is_valid_name(&params.device_name) || !is_valid_platform(&params.platform) {
            return Err(CoreError::input("invalid device name or platform"));
        }
        Ok(Self {
            state: State::AwaitCommit,
            first: true,
            prs: params.code.prs(),
            ci: session::channel_identifier(&params.relay_authority, params.code.channel_id()),
            ada: session::ad_inviter(),
            adb: session::ad_joiner(),
            name: params.device_name,
            platform: params.platform,
            keys: Some(keys),
            entropy,
            sas: None,
            inviter: None,
        })
    }

    /// The phase of the sequence.
    pub fn phase(&self) -> JoinerPhase {
        self.state.phase()
    }

    /// The verification code, once the inviter's `reveal` was verified (N7).
    pub fn sas(&self) -> Option<&Sas> {
        self.sas.as_ref()
    }

    /// The inviter, as it announced itself (N7): name, platform, number of devices, history offer.
    pub fn inviter(&self) -> Option<&InviterInfo> {
        self.inviter.as_ref()
    }

    /// Handles the next frame read from the channel (the inviter's messages, in index order).
    ///
    /// On error the session is over: wipe and close the channel. The error is `pairing_failed`
    /// whatever the cause, except `pairing_unsupported` (unknown version in the first frame, or
    /// an unknown critical field) and operational errors.
    pub fn receive(&mut self, frame: &[u8]) -> Result<JoinerStep> {
        let first = std::mem::replace(&mut self.first, false);
        let state = std::mem::replace(&mut self.state, State::Failed);
        match self.receive_in(state, frame, first) {
            Ok(step) => Ok(step),
            Err(e) => {
                if self.state.phase() != JoinerPhase::Cancelled {
                    self.state = State::Failed;
                }
                self.wipe();
                Err(super::normalize(e))
            }
        }
    }

    fn receive_in(&mut self, state: State, frame: &[u8], first: bool) -> Result<JoinerStep> {
        match state {
            State::Done { provision } => {
                self.state = State::Done { provision };
                return Ok(JoinerStep {
                    send: None,
                    event: JoinerEvent::Ignored,
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
            (State::AwaitCommit, FrameKind::Commit) => self.on_commit(frame.len(), payload),
            (State::AwaitReveal { sid, commit, share }, FrameKind::Reveal) => {
                self.on_reveal(sid, commit, share, payload)
            }
            (State::AwaitProvision { keys, sid }, FrameKind::Provision) => {
                self.on_provision(keys, sid, payload)
            }
            // Includes any frame while the user compares the codes (`AwaitUserConfirm`).
            _ => Err(CoreError::PairingFailed),
        }
    }

    /// N3, N4: `commit` received; derive the generator with its `sid` and answer with `Yb`.
    fn on_commit(&mut self, frame_len: usize, payload: &[u8]) -> Result<JoinerStep> {
        if frame_len != COMMIT_FRAME_LEN {
            return Err(CoreError::PairingFailed);
        }
        let (sid, commit) = payload.split_at(SID_LEN);
        let sid: [u8; SID_LEN] = sid.try_into().map_err(|_| CoreError::PairingFailed)?;
        let commit: [u8; 32] = commit.try_into().map_err(|_| CoreError::PairingFailed)?;
        let g = cpace::generator(self.prs.as_slice(), &self.ci, &sid);
        // The password is not needed any more.
        self.prs.zeroize();
        let share = Share::sample(&g, &mut self.entropy)?;
        let reply = frame::encode(FrameKind::Share, share.public())?;
        self.state = State::AwaitReveal { sid, commit, share };
        Ok(JoinerStep {
            send: Some(reply),
            event: JoinerEvent::Shared,
        })
    }

    /// N5, N6: `reveal` received. Everything that can go wrong here is "wrong code" (§11.1).
    fn on_reveal(
        &mut self,
        sid: [u8; SID_LEN],
        commit: [u8; 32],
        share: Share,
        payload: &[u8],
    ) -> Result<JoinerStep> {
        if payload.len() < POINT_LEN + TAG_LEN {
            return Err(CoreError::PairingFailed);
        }
        let (ya, boxed) = payload.split_at(POINT_LEN);
        let ya: [u8; POINT_LEN] = ya.try_into().map_err(|_| CoreError::PairingFailed)?;
        // The engagement: the inviter reveals the `Ya` it committed to, not another one.
        if session::commitment(&sid, &ya) != commit {
            return Err(CoreError::PairingFailed);
        }
        // decode(Ya) failing or K = I aborts inside `finish`.
        let isk = share.finish(Role::Responder, &ya, &sid, &self.ada, &self.adb)?;
        let (keys, sas) = session::derive(&isk)?;
        // The first box is the key confirmation: it opens only if the inviter holds the same ISK.
        let body = frame::open_body(&keys, FrameKind::Reveal, &sid, boxed)?;
        let hello = InviterHello::decode(&body)?;
        let own = self.own_keys()?;
        let inviter_public = DevicePublic {
            ed25519: hello.ed_pk.to_vec(),
            x25519: hello.x_pk.to_vec(),
        };
        inviter_public.arrays()?;
        // The inviter is somebody else.
        let mine = own.public();
        if inviter_public.ed25519 == mine.ed25519 || inviter_public.x25519 == mine.x25519 {
            return Err(CoreError::PairingFailed);
        }
        let reply_hello = JoinerHello {
            ed_pk: own.ed_public(),
            x_pk: own.x_public(),
            name: self.name.clone(),
            platform: self.platform.clone(),
        };
        let reply_body = Zeroizing::new(reply_hello.encode()?);
        let boxed = frame::seal_body(&keys, FrameKind::JoinerHello, &sid, &reply_body)?;
        let reply = frame::encode(FrameKind::JoinerHello, &boxed)?;
        self.inviter = Some(InviterInfo {
            peer: PeerInfo {
                name: hello.name,
                platform: hello.platform,
                public: inviter_public,
            },
            member_count: hello.member_count,
            history_offer: hello.history_offer,
        });
        self.sas = Some(sas);
        self.state = State::AwaitUserConfirm { keys, sid };
        Ok(JoinerStep {
            send: Some(reply),
            event: JoinerEvent::InviterHello,
        })
    }

    /// N10, N11: `provision` received; verify the batch (§8.3) and acknowledge it.
    fn on_provision(
        &mut self,
        keys: SessionKeys,
        sid: [u8; SID_LEN],
        payload: &[u8],
    ) -> Result<JoinerStep> {
        let inviter = self
            .inviter
            .as_ref()
            .map(|i| i.peer.clone())
            .ok_or(CoreError::PairingFailed)?;
        let body = frame::open_body(&keys, FrameKind::Provision, &sid, payload)?;
        let body = ProvisionBody::decode(&body)?;
        let own = self.keys.take().ok_or(CoreError::PairingFailed)?;
        let verified = batch::verify(&own, &inviter.public, &body)?;
        let ack = frame::seal_body(&keys, FrameKind::JoinerAck, &sid, &[])?;
        let reply = frame::encode(FrameKind::JoinerAck, &ack)?;
        let provision = JoinerProvision {
            keys: own,
            trust: verified.trust,
            space_key: verified.space_key,
            device_name: self.name.clone(),
            inviter,
            history: verified.history,
        };
        self.sas = None;
        self.state = State::Done {
            provision: Some(Box::new(provision)),
        };
        Ok(JoinerStep {
            send: Some(reply),
            event: JoinerEvent::ProvisionVerified,
        })
    }

    fn own_keys(&self) -> Result<&DeviceKeys> {
        self.keys.as_ref().ok_or(CoreError::PairingFailed)
    }

    /// The user on this device confirmed that both screens show the same verification code
    /// (N8) and, if the inviter offered its history, answered the single yes / no question.
    /// Returns the `joiner_confirm` frame to deposit (N9).
    ///
    /// `import_history` is ignored (sent as "no") when the inviter offered nothing: the question
    /// is not asked then.
    pub fn confirm(&mut self, import_history: bool) -> Result<Vec<u8>> {
        if !matches!(self.state, State::AwaitUserConfirm { .. }) {
            return Err(CoreError::input("no inviter waiting for confirmation"));
        }
        let offered = self
            .inviter
            .as_ref()
            .and_then(|i| i.history_offer)
            .is_some_and(|o| o.items > 0);
        let history = import_history && offered;
        let State::AwaitUserConfirm { keys, sid } =
            std::mem::replace(&mut self.state, State::Failed)
        else {
            return Err(CoreError::PairingFailed);
        };
        match confirm_frame(&keys, &sid, history) {
            Ok(frame) => {
                self.state = State::AwaitProvision { keys, sid };
                Ok(frame)
            }
            Err(e) => {
                self.wipe();
                Err(super::normalize(e))
            }
        }
    }

    /// Takes the verified batch once the channel is completed (N13). Until then it stays
    /// pending inside the machine: persist it in the OS secure storage if the app may stop
    /// between N11 and N13 (§8.3, §11.4).
    pub fn take_provision(&mut self) -> Result<JoinerProvision> {
        let provision = match &mut self.state {
            State::Done { provision } => provision
                .take()
                .map(|p| *p)
                .ok_or_else(|| CoreError::input("provision already taken"))?,
            _ => return Err(CoreError::input("no verified batch")),
        };
        self.wipe();
        Ok(provision)
    }

    /// The user refused ("they are different", "Cancel", back): wipes the session and the
    /// pending batch. Close the channel afterwards.
    pub fn cancel(&mut self) {
        self.state = State::Cancelled;
        self.keys = None;
        self.wipe();
    }

    /// Erases what the session must not outlive.
    fn wipe(&mut self) {
        self.prs.zeroize();
        self.sas = None;
        self.inviter = None;
        self.keys = None;
    }
}

/// The `joiner_confirm` frame (N9). Its size does not depend on the answer.
fn confirm_frame(keys: &SessionKeys, sid: &[u8; SID_LEN], history: bool) -> Result<Vec<u8>> {
    let body = JoinerConfirm { history }.encode()?;
    let boxed = frame::seal_body(keys, FrameKind::JoinerConfirm, sid, &body)?;
    frame::encode(FrameKind::JoinerConfirm, &boxed)
}

impl std::fmt::Debug for Joiner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Joiner")
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}
