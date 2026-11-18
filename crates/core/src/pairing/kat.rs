//! Known-answer-test hooks of the pairing module, used only by the tests and by
//! `examples/gen_pairing_vectors.rs`.
//!
//! **Never call these from application code**: they fix the randomness of a session (so two
//! sessions can share `sid`, `ya`, `yb`) or expose `K`, which must never leave the CPace layer.
//! Not part of the UniFFI surface.

use sha2::{Digest, Sha512};

use super::cpace::{self, Role, Share};
use super::entropy::Entropy;
use super::inviter::{Inviter, InviterParams};
use super::joiner::{Joiner, JoinerParams};
use crate::device::{DeviceKeys, TrustStore};
use crate::error::{CoreError, Result};

/// Starts an invitation with scripted randomness. One chunk per draw, in this order: rank 1
/// (2 bytes, big endian, 11 bits used), rank 2 (2 bytes), `sid` (16), `ya` (32); then, when the
/// batch is built, the sealing ephemeral secret (32), the sealing nonce (24) and, if a history is
/// announced, the transfer id (16).
pub fn inviter_start(
    params: InviterParams,
    keys: &DeviceKeys,
    trust: &TrustStore,
    script: Vec<Vec<u8>>,
) -> Result<Inviter> {
    Inviter::start_with(params, keys, trust, Entropy::scripted(script))
}

/// Starts a joining session with explicit device keys and scripted randomness: one chunk, `yb`
/// (32 bytes).
pub fn joiner_new(params: JoinerParams, keys: DeviceKeys, script: Vec<Vec<u8>>) -> Result<Joiner> {
    Joiner::new_with(params, keys, Entropy::scripted(script))
}

/// Every value of one CPace run, `K` included (appendix B.3 of the draft lists them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpaceTrace {
    /// `generator_string(DSI, PRS, CI, sid, 128)`.
    pub generator_string: Vec<u8>,
    /// `SHA-512(generator_string)`.
    pub generator_hash: Vec<u8>,
    /// `encode(g)`.
    pub generator: [u8; 32],
    /// `Ya = encode(ya * g)`.
    pub ya_public: [u8; 32],
    /// `Yb = encode(yb * g)`.
    pub yb_public: [u8; 32],
    /// `K = scalar_mult_vfy(ya, Yb)`.
    pub k_initiator: [u8; 32],
    /// `K = scalar_mult_vfy(yb, Ya)`.
    pub k_responder: [u8; 32],
    /// `ISK` computed by the initiator.
    pub isk_initiator: Vec<u8>,
    /// `ISK` computed by the responder.
    pub isk_responder: Vec<u8>,
}

/// One complete CPace run with explicit scalars (little endian, below 2^252) and associated data.
pub fn cpace_trace(
    prs: &[u8],
    ci: &[u8],
    sid: &[u8],
    ada: &[u8],
    adb: &[u8],
    ya: &[u8; 32],
    yb: &[u8; 32],
) -> Result<CpaceTrace> {
    let generator_string = cpace::generator_string(prs, ci, sid);
    let generator_hash = Sha512::digest(&generator_string).to_vec();
    let g = cpace::generator(prs, ci, sid);
    let a = Share::from_scalar_bytes(&g, ya)?;
    let b = Share::from_scalar_bytes(&g, yb)?;
    let k_initiator = a.shared_point(b.public()).ok_or(CoreError::PairingFailed)?;
    let k_responder = b.shared_point(a.public()).ok_or(CoreError::PairingFailed)?;
    let isk_initiator = a.finish(Role::Initiator, b.public(), sid, ada, adb)?;
    let isk_responder = b.finish(Role::Responder, a.public(), sid, ada, adb)?;
    Ok(CpaceTrace {
        generator_string,
        generator_hash,
        generator: g.compress().to_bytes(),
        ya_public: *a.public(),
        yb_public: *b.public(),
        k_initiator: *k_initiator,
        k_responder: *k_responder,
        isk_initiator: isk_initiator.to_vec(),
        isk_responder: isk_responder.to_vec(),
    })
}

/// `G.scalar_mult_vfy(y, X)` with the scalar as 32 little-endian bytes: `I` (32 zero bytes) if
/// `X` is not a canonical encoding.
pub fn scalar_mult_vfy(y: &[u8; 32], x: &[u8; 32]) -> [u8; 32] {
    cpace::scalar_mult_vfy(
        &curve25519_dalek::scalar::Scalar::from_bytes_mod_order(*y),
        x,
    )
}

/// `prepend_len(data)` (draft A.1.1).
pub fn prepend_len(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    cpace::prepend_len(&mut out, data);
    out
}

/// `lv_cat(parts...)` (draft A.1.3).
pub fn lv_cat(parts: &[&[u8]]) -> Vec<u8> {
    cpace::lv_cat(parts)
}
