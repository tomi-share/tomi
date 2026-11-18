//! Signature of the relay authentication challenge (`api-v1.md` §3.6).
//!
//! ```text
//! msg = "tomi/v1/sig/relay-auth" || 0x00 || space_id (16) || sign_pk (32) || challenge (32)
//! signature = Ed25519(sign_sk, msg)        // 103 bytes signed, 64 bytes of signature
//! ```
//!
//! Not part of the frozen vectors (the API spec defines it, `format-v1.md` does not); it has
//! its own unit tests below.

use crate::crypto::{labels, parse_uuid, to_array, Writer};
use crate::device::{verify_signature, DeviceKeys, PUBLIC_KEY_LEN};
use crate::error::Result;

/// Length of the challenge sent by the relay.
pub const RELAY_CHALLENGE_LEN: usize = 32;
/// Length of the signed message.
pub const RELAY_AUTH_MESSAGE_LEN: usize = 22 + 1 + 16 + PUBLIC_KEY_LEN + RELAY_CHALLENGE_LEN;

/// The exact bytes signed by a device to answer `challenge` (also used by the relay and by
/// the test double to verify).
pub fn relay_auth_message(space_id: &str, sign_pk: &[u8], challenge: &[u8]) -> Result<Vec<u8>> {
    let sid = parse_uuid(space_id, "space id")?;
    let pk: [u8; PUBLIC_KEY_LEN] = to_array(sign_pk, "sign_pk")?;
    let ch: [u8; RELAY_CHALLENGE_LEN] = to_array(challenge, "challenge")?;
    let mut w = Writer::labeled(labels::SIG_RELAY_AUTH_RESERVED);
    w.bytes(&sid).bytes(&pk).bytes(&ch);
    Ok(w.finish())
}

/// Checks a challenge answer (relay side; strict Ed25519).
pub fn verify_relay_auth(
    space_id: &str,
    sign_pk: &[u8],
    challenge: &[u8],
    signature: &[u8],
) -> Result<()> {
    let msg = relay_auth_message(space_id, sign_pk, challenge)?;
    let pk: [u8; PUBLIC_KEY_LEN] = to_array(sign_pk, "sign_pk")?;
    verify_signature(&pk, &msg, signature)
}

impl DeviceKeys {
    /// Answers the relay challenge of `POST /v1/auth/challenge` (64-byte signature, to send
    /// as `signature` in `POST /v1/auth/session`).
    pub fn sign_relay_auth(&self, space_id: &str, challenge: &[u8]) -> Result<Vec<u8>> {
        let msg = relay_auth_message(space_id, &self.ed_public(), challenge)?;
        Ok(self.sign(&msg))
    }

    /// Ed25519 public key (`sign_pk` of the API).
    pub fn sign_pk(&self) -> Vec<u8> {
        self.ed_public().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CoreError;

    const SPACE: &str = "6b5167fa-6c43-43da-a770-bb43fad022e2";

    #[test]
    fn message_layout_is_the_spec_one() {
        let keys = DeviceKeys::generate().unwrap();
        let challenge = [7u8; 32];
        let msg = relay_auth_message(SPACE, &keys.sign_pk(), &challenge).unwrap();
        assert_eq!(msg.len(), RELAY_AUTH_MESSAGE_LEN);
        assert_eq!(msg.len(), 103);
        assert_eq!(&msg[..22], b"tomi/v1/sig/relay-auth");
        assert_eq!(msg[22], 0);
        assert_eq!(&msg[23..39], &parse_uuid(SPACE, "x").unwrap());
        assert_eq!(&msg[39..71], &keys.sign_pk()[..]);
        assert_eq!(&msg[71..], &challenge);
    }

    #[test]
    fn sign_and_verify() {
        let keys = DeviceKeys::generate().unwrap();
        let challenge = [9u8; 32];
        let sig = keys.sign_relay_auth(SPACE, &challenge).unwrap();
        assert_eq!(sig.len(), 64);
        verify_relay_auth(SPACE, &keys.sign_pk(), &challenge, &sig).unwrap();
        // Bound to the challenge, the space and the key.
        assert_eq!(
            verify_relay_auth(SPACE, &keys.sign_pk(), &[8u8; 32], &sig).unwrap_err(),
            CoreError::InvalidSignature
        );
        let other_space = "00000000-0000-4000-8000-000000000000";
        assert!(verify_relay_auth(other_space, &keys.sign_pk(), &challenge, &sig).is_err());
        let other = DeviceKeys::generate().unwrap();
        assert!(verify_relay_auth(SPACE, &other.sign_pk(), &challenge, &sig).is_err());
        // A malformed challenge is an input error, not a panic.
        assert!(keys.sign_relay_auth(SPACE, &[1u8; 31]).is_err());
    }
}
