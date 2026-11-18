//! Pairing of a new device (`spec/pairing-v1.md`), pure part: no network.
//!
//! An existing member (the **inviter**, A) shows a short code and a QR code; the new device (the
//! **joiner**, N) reads it. Both run CPace (ristretto255, SHA-512) with the two secret words of
//! the code, compare a 6-digit verification code by eye, and A then seals the space key and the
//! member list for N. This module holds everything that does not touch the network:
//!
//! * [`PairingCode`]: generate, read (typed code, link, QR), show (§4);
//! * `cpace` (private): the CPace composition over `curve25519-dalek` (§3);
//! * frames and their TLV bodies (§5, §6);
//! * [`Inviter`] and [`Joiner`]: the two state machines (§7), fed with the frames read from the
//!   relay channel and the decisions of the users, returning the frames to deposit;
//! * the pairing batch (§8): built by A on a copy of its trust store, verified by N in the exact
//!   order of §8.3, giving a [`JoinerProvision`] ready for `Engine::provision`.
//!
//! The relay never sees any of it: it carries opaque frames.
//!
//! ## Secrets
//!
//! The code (its two words), `ya`, `yb`, `K`, `ISK`, the frame keys and the verification code
//! live only in zeroized buffers and in objects whose `Debug` hides them. No error message
//! contains a frame byte or a secret, and every failure of a wrong code is the same value,
//! [`CoreError::PairingFailed`].

use crate::error::CoreError;

mod batch;
mod code;
mod cpace;
mod entropy;
mod frame;
mod info;
mod inviter;
mod joiner;
mod session;
mod tlv;
mod wordlist;

#[doc(hidden)]
pub mod kat;
#[cfg(test)]
mod tests;

pub(crate) use code::check_relay_authority;
pub use code::{PairingCode, CODE_WORDS};
pub use frame::{FrameKind, MAX_FRAME_LEN, PAIR_VERSION};
pub use info::{HistoryAnnouncement, HistoryOffer, InviterInfo, PeerInfo, Sas};
pub use inviter::{
    Inviter, InviterEvent, InviterOutcome, InviterParams, InviterPhase, InviterStep,
};
pub use joiner::{Joiner, JoinerEvent, JoinerParams, JoinerPhase, JoinerProvision, JoinerStep};
pub use wordlist::{rank_of, word, WORD_COUNT};

/// Whether `s` is a device name as the pairing frames and the relay type it
/// (`api-v1.md` §2.3): the runtime checks the names it passes to the machines with the same rule.
#[cfg(feature = "runtime")]
pub(crate) fn is_valid_device_name(s: &str) -> bool {
    tlv::is_valid_name(s)
}

/// Whether `s` is a platform as the pairing frames and the relay type it (`^[a-z0-9-]{1,32}$`).
#[cfg(feature = "runtime")]
pub(crate) fn is_valid_platform_name(s: &str) -> bool {
    tlv::is_valid_platform(s)
}

/// What a machine reports for a failed step (`pairing-v1.md` §11.1, §11.5).
///
/// Every protocol failure is `pairing_failed`, so a wrong code, a forged engagement, an invalid
/// point, a bad key confirmation and a refused batch are indistinguishable. Kept as they are: an
/// unknown version or critical field (`pairing_unsupported`), a cancellation, and the
/// operational errors of the environment (randomness, internal state).
fn normalize(e: CoreError) -> CoreError {
    match e {
        CoreError::PairingUnsupported
        | CoreError::PairingCancelled
        | CoreError::Randomness
        | CoreError::Internal { .. } => e,
        _ => CoreError::PairingFailed,
    }
}
