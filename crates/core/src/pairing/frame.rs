//! Pairing frames: header, sealing and opening (`spec/pairing-v1.md` §5.4, §6.1).
//!
//! ```text
//! frame    = u8(PAIR_VERSION) || u8(type) || payload
//! nonce(n) = zero_bytes(20) || u32be(n)
//! AAD(t)   = "tomi/v1/aad/pair" || 0x00 || u8(PAIR_VERSION) || u8(t) || sid
//! box      = XC_Seal(K_dir, nonce(n), AAD(t), body)
//! ```
//!
//! The nonces are deterministic, which is an assumed exception to `format-v1.md` §2.2: `K_I` and
//! `K_J` belong to one session (fresh `ya`, `yb`) and each seals at most three frames of distinct
//! types, so no `(key, nonce)` pair is ever used twice. The mapping from frame type to key and
//! counter is fixed here and is the only place where a counter appears.

use zeroize::Zeroizing;

use super::cpace::POINT_LEN;
use super::session::{SessionKeys, SID_LEN};
use crate::crypto::{aead_open, aead_seal, labels, KEY_LEN, NONCE_LEN};
use crate::error::{CoreError, Result};

/// Version of the pairing protocol (`PAIR_VERSION`).
pub const PAIR_VERSION: u8 = 1;
/// Largest frame the relay carries (`pairing_max_message_bytes`).
pub const MAX_FRAME_LEN: usize = 8192;
/// Version byte and type byte.
pub(super) const HEADER_LEN: usize = 2;
/// Poly1305 tag of a sealed body.
pub(super) const TAG_LEN: usize = 16;
/// `commit` frame: header, `sid` (16), `commit` (32).
pub(super) const COMMIT_FRAME_LEN: usize = HEADER_LEN + SID_LEN + 32;
/// `share` frame: header, `Yb` (32).
pub(super) const SHARE_FRAME_LEN: usize = HEADER_LEN + POINT_LEN;

/// Length of `AAD(t)`.
const AAD_LEN: usize = labels::AAD_PAIR.len() + 1 + 1 + 1 + SID_LEN;

/// The seven frame types of version 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// A -> N: `sid`, engagement on `Ya`.
    Commit = 0x01,
    /// N -> A: `Yb`.
    Share = 0x02,
    /// A -> N: `Ya` and the sealed `InviterHello`.
    Reveal = 0x03,
    /// N -> A: the sealed `JoinerHello`.
    JoinerHello = 0x04,
    /// N -> A: the sealed answer of the user on N (history yes / no).
    JoinerConfirm = 0x05,
    /// A -> N: the sealed pairing batch.
    Provision = 0x06,
    /// N -> A: the sealed acknowledgement of the batch.
    JoinerAck = 0x07,
}

/// Which session key seals a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    /// `K_I`, A -> N.
    FromInviter,
    /// `K_J`, N -> A.
    FromJoiner,
}

impl FrameKind {
    /// The kind of a type byte; `None` for the reserved types (a v1 reader fails on them).
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0x01 => Self::Commit,
            0x02 => Self::Share,
            0x03 => Self::Reveal,
            0x04 => Self::JoinerHello,
            0x05 => Self::JoinerConfirm,
            0x06 => Self::Provision,
            0x07 => Self::JoinerAck,
            _ => return None,
        })
    }

    /// The type byte.
    pub fn byte(self) -> u8 {
        self as u8
    }

    /// Key and counter `n` of a sealed frame (`pairing-v1.md` §5.4); `None` for the two
    /// frames that travel in clear.
    pub(super) fn sealing(self) -> Option<(Direction, u32)> {
        match self {
            Self::Commit | Self::Share => None,
            Self::Reveal => Some((Direction::FromInviter, 0)),
            Self::Provision => Some((Direction::FromInviter, 1)),
            Self::JoinerHello => Some((Direction::FromJoiner, 0)),
            Self::JoinerConfirm => Some((Direction::FromJoiner, 1)),
            Self::JoinerAck => Some((Direction::FromJoiner, 2)),
        }
    }
}

/// `header || payload`, checked against the size limit of the relay.
pub(super) fn encode(kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>> {
    let len = HEADER_LEN + payload.len();
    if len > MAX_FRAME_LEN {
        return Err(CoreError::PairingFailed);
    }
    let mut out = Vec::with_capacity(len);
    out.push(PAIR_VERSION);
    out.push(kind.byte());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Splits a received frame into its type byte and payload.
///
/// `first` is true for the first frame this side receives: an unknown version there is
/// `pairing_unsupported` ("update Tomi"); in a later frame it is a protocol violation
/// (`pairing-v1.md` §7.4).
pub(super) fn split(frame: &[u8], first: bool) -> Result<(u8, &[u8])> {
    let [version, kind, payload @ ..] = frame else {
        return Err(CoreError::PairingFailed);
    };
    if frame.len() > MAX_FRAME_LEN {
        return Err(CoreError::PairingFailed);
    }
    if *version != PAIR_VERSION {
        return Err(if first {
            CoreError::PairingUnsupported
        } else {
            CoreError::PairingFailed
        });
    }
    Ok((*kind, payload))
}

/// `AAD(t)`: binds a sealed body to its type, to the version and to the session.
pub(super) fn aad(kind: FrameKind, sid: &[u8; SID_LEN]) -> [u8; AAD_LEN] {
    let mut out = [0u8; AAD_LEN];
    let label = labels::AAD_PAIR;
    out[..label.len()].copy_from_slice(label);
    // out[label.len()] stays 0x00.
    out[label.len() + 1] = PAIR_VERSION;
    out[label.len() + 2] = kind.byte();
    out[label.len() + 3..].copy_from_slice(sid);
    out
}

/// `nonce(n) = zero_bytes(20) || u32be(n)`.
pub(super) fn nonce(n: u32) -> [u8; NONCE_LEN] {
    let mut out = [0u8; NONCE_LEN];
    out[NONCE_LEN - 4..].copy_from_slice(&n.to_be_bytes());
    out
}

fn key_for(keys: &SessionKeys, direction: Direction) -> &[u8; KEY_LEN] {
    match direction {
        Direction::FromInviter => &keys.inviter,
        Direction::FromJoiner => &keys.joiner,
    }
}

/// `box = XC_Seal(K_dir, nonce(n), AAD(t), body)` for a sealed frame type.
pub(super) fn seal_body(
    keys: &SessionKeys,
    kind: FrameKind,
    sid: &[u8; SID_LEN],
    body: &[u8],
) -> Result<Vec<u8>> {
    let (direction, n) = kind.sealing().ok_or(CoreError::PairingFailed)?;
    aead_seal(key_for(keys, direction), &nonce(n), &aad(kind, sid), body)
        .map_err(|_| CoreError::PairingFailed)
}

/// Opens a `box`. Any failure is `pairing_failed`: after the PAKE, this is the key confirmation,
/// and a failure means the other side does not hold the same `ISK` (a wrong code, `pairing-v1.md`
/// §5.4, §11.1).
pub(super) fn open_body(
    keys: &SessionKeys,
    kind: FrameKind,
    sid: &[u8; SID_LEN],
    boxed: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let (direction, n) = kind.sealing().ok_or(CoreError::PairingFailed)?;
    if boxed.len() < TAG_LEN {
        return Err(CoreError::PairingFailed);
    }
    aead_open(key_for(keys, direction), &nonce(n), &aad(kind, sid), boxed)
        .map(Zeroizing::new)
        .map_err(|_| CoreError::PairingFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> SessionKeys {
        SessionKeys {
            inviter: Zeroizing::new([1u8; KEY_LEN]),
            joiner: Zeroizing::new([2u8; KEY_LEN]),
        }
    }

    fn sid() -> [u8; SID_LEN] {
        std::array::from_fn(|i| i as u8)
    }

    #[test]
    fn aad_matches_the_spec_example() {
        // pairing-v1.md §5.4: AAD(0x03) with sid = 000102...0f.
        let a = aad(FrameKind::Reveal, &sid());
        assert_eq!(a.len(), 35);
        let want = concat!(
            "746f6d692f76312f6161642f70616972", // "tomi/v1/aad/pair"
            "00",                               // separator
            "01",                               // PAIR_VERSION
            "03",                               // type: reveal
            "000102030405060708090a0b0c0d0e0f", // sid
        );
        assert_eq!(hex::encode(a), want);
    }

    #[test]
    fn nonce_is_twenty_zero_bytes_then_the_counter() {
        assert_eq!(hex::encode(nonce(0)), "00".repeat(24));
        assert_eq!(
            hex::encode(nonce(2)),
            format!("{}00000002", "00".repeat(20))
        );
    }

    #[test]
    fn every_sealed_type_has_its_own_key_and_counter() {
        use Direction::*;
        let all = [
            (FrameKind::Reveal, Some((FromInviter, 0))),
            (FrameKind::Provision, Some((FromInviter, 1))),
            (FrameKind::JoinerHello, Some((FromJoiner, 0))),
            (FrameKind::JoinerConfirm, Some((FromJoiner, 1))),
            (FrameKind::JoinerAck, Some((FromJoiner, 2))),
            (FrameKind::Commit, None),
            (FrameKind::Share, None),
        ];
        let mut used = std::collections::BTreeSet::new();
        for (kind, want) in all {
            assert_eq!(kind.sealing(), want);
            assert_eq!(FrameKind::from_byte(kind.byte()), Some(kind));
            if let Some((d, n)) = want {
                // No (key, nonce) pair is used by two frame types.
                assert!(used.insert((d == FromInviter, n)));
            }
        }
        assert_eq!(FrameKind::from_byte(0x00), None);
        assert_eq!(FrameKind::from_byte(0x08), None);
    }

    #[test]
    fn seal_open_round_trip_and_bindings() {
        let k = keys();
        let s = sid();
        let boxed = seal_body(&k, FrameKind::JoinerHello, &s, b"hello").unwrap();
        assert_eq!(boxed.len(), 5 + TAG_LEN);
        let opened = open_body(&k, FrameKind::JoinerHello, &s, &boxed).unwrap();
        assert_eq!(opened.as_slice(), b"hello");
        // Another type (even with the same key and counter), another session, another
        // direction, a short box: all fail the same way.
        let other_sid = [9u8; SID_LEN];
        for result in [
            open_body(&k, FrameKind::JoinerConfirm, &s, &boxed),
            open_body(&k, FrameKind::JoinerHello, &other_sid, &boxed),
            open_body(&k, FrameKind::Reveal, &s, &boxed),
            open_body(&k, FrameKind::JoinerHello, &s, &boxed[..10]),
            open_body(&k, FrameKind::Commit, &s, &boxed),
        ] {
            assert_eq!(result.unwrap_err(), CoreError::PairingFailed);
        }
        // A flipped bit anywhere fails.
        for i in 0..boxed.len() {
            let mut bad = boxed.clone();
            bad[i] ^= 1;
            assert!(open_body(&k, FrameKind::JoinerHello, &s, &bad).is_err());
        }
        assert!(seal_body(&k, FrameKind::Commit, &s, b"x").is_err());
    }

    #[test]
    fn empty_body_seals_to_the_tag_only() {
        let k = keys();
        let boxed = seal_body(&k, FrameKind::JoinerAck, &sid(), b"").unwrap();
        assert_eq!(boxed.len(), TAG_LEN);
        assert!(open_body(&k, FrameKind::JoinerAck, &sid(), &boxed)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn split_checks_size_and_version() {
        let ok = encode(FrameKind::Share, &[7u8; 32]).unwrap();
        assert_eq!(ok.len(), SHARE_FRAME_LEN);
        let (kind, payload) = split(&ok, true).unwrap();
        assert_eq!(kind, 0x02);
        assert_eq!(payload, &[7u8; 32]);
        assert_eq!(split(&[], true).unwrap_err(), CoreError::PairingFailed);
        assert_eq!(split(&[1], false).unwrap_err(), CoreError::PairingFailed);
        // Header only is a frame of 2 bytes (the minimum of §6.1).
        assert_eq!(split(&[1, 7], false).unwrap(), (7, &[][..]));
        let v2 = [2u8, 2, 0];
        assert_eq!(split(&v2, true).unwrap_err(), CoreError::PairingUnsupported);
        assert_eq!(split(&v2, false).unwrap_err(), CoreError::PairingFailed);
        let mut huge = vec![1u8, 2];
        huge.resize(MAX_FRAME_LEN, 0);
        assert!(split(&huge, false).is_ok());
        huge.push(0);
        assert_eq!(split(&huge, false).unwrap_err(), CoreError::PairingFailed);
        assert_eq!(
            encode(FrameKind::Provision, &vec![0u8; MAX_FRAME_LEN - 1]).unwrap_err(),
            CoreError::PairingFailed
        );
        assert!(encode(FrameKind::Provision, &vec![0u8; MAX_FRAME_LEN - 2]).is_ok());
    }
}
