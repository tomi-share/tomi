//! Session identifiers, engagement and key derivation (`spec/pairing-v1.md` §3.3, §5.1, §5.2).

use zeroize::Zeroizing;

use super::cpace::{self, ISK_LEN};
use super::frame::PAIR_VERSION;
use super::info::Sas;
use crate::crypto::{hkdf_key, hkdf_sha256, labels, sha256, KEY_LEN};
use crate::error::{CoreError, Result};

/// `sid`: 16 random bytes drawn by the inviter, sent in clear in the `commit` frame.
pub(super) const SID_LEN: usize = 16;

/// `CI = lv_cat("tomi/v1/pair-ci", relay_authority, channel_id)`.
///
/// The authority is the relay the device retained (read from a link or a field, the user confirmed it
/// with the verification code, `pairing-v1.md` §4.4); `channel_id` is the canonical decimal form
/// (`pairing-v1.md` §3.3).
pub(super) fn channel_identifier(relay_authority: &str, channel_id: &str) -> Vec<u8> {
    cpace::lv_cat(&[
        labels::PAIR_CI,
        relay_authority.as_bytes(),
        channel_id.as_bytes(),
    ])
}

/// `AD = label || 0x00 || u8(PAIR_VERSION)` (`ADa` with the inviter label, `ADb` with the joiner
/// label). The roles are not transmitted: each side knows them. A different version changes the
/// associated data, hence `ISK`: a downgrade forced by the relay makes the session fail.
pub(super) fn associated_data(label: &[u8]) -> Vec<u8> {
    let mut ad = label.to_vec();
    ad.push(0);
    ad.push(PAIR_VERSION);
    ad
}

/// `ADa` (inviter).
pub(super) fn ad_inviter() -> Vec<u8> {
    associated_data(labels::PAIR_INVITER)
}

/// `ADb` (joiner).
pub(super) fn ad_joiner() -> Vec<u8> {
    associated_data(labels::PAIR_JOINER)
}

/// `commit = SHA-256("tomi/v1/pair-commit" || 0x00 || sid || Ya)` (§5.1).
pub(super) fn commitment(sid: &[u8; SID_LEN], ya: &[u8; cpace::POINT_LEN]) -> [u8; 32] {
    sha256(&[labels::PAIR_COMMIT, &[0u8], sid, ya])
}

/// The two directional frame keys, derived from `ISK` (§5.2).
pub(super) struct SessionKeys {
    /// `K_I`: seals the frames A -> N.
    pub(super) inviter: Zeroizing<[u8; KEY_LEN]>,
    /// `K_J`: seals the frames N -> A.
    pub(super) joiner: Zeroizing<[u8; KEY_LEN]>,
}

impl std::fmt::Debug for SessionKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKeys(<redacted>)")
    }
}

/// `K_I`, `K_J` and the SAS from `ISK`. `ISK` itself is not kept by the caller afterwards.
///
/// ```text
/// K_I = HKDF(∅, ISK, "tomi/v1/hkdf/pair-inviter", 32)
/// K_J = HKDF(∅, ISK, "tomi/v1/hkdf/pair-joiner", 32)
/// SAS = u64be(HKDF(∅, ISK, "tomi/v1/hkdf/pair-sas", 8)) mod 1 000 000
/// ```
pub(super) fn derive(isk: &[u8; ISK_LEN]) -> Result<(SessionKeys, Sas)> {
    let inviter = hkdf_key(None, isk, labels::HKDF_PAIR_INVITER)?;
    let joiner = hkdf_key(None, isk, labels::HKDF_PAIR_JOINER)?;
    let s8 = hkdf_sha256(None, isk, labels::HKDF_PAIR_SAS, 8)?;
    let mut s8_arr: Zeroizing<[u8; 8]> = Zeroizing::new([0u8; 8]);
    if s8.len() != 8 {
        return Err(CoreError::Internal {
            reason: "sas derivation length".into(),
        });
    }
    s8_arr.copy_from_slice(s8.as_slice());
    let value = u64::from_be_bytes(*s8_arr) % Sas::MODULUS;
    // `value` is below 10^6: it fits a u32.
    let sas = Sas::new(u32::try_from(value).map_err(|_| CoreError::Internal {
        reason: "sas range".into(),
    })?);
    Ok((SessionKeys { inviter, joiner }, sas))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_and_associated_data_match_the_spec_example() {
        // pairing-v1.md §3.3, relay_authority = relay.example.org, channel 4821.
        let ci = channel_identifier("relay.example.org", "4821");
        assert_eq!(
            hex::encode(&ci),
            "0f746f6d692f76312f706169722d6369\
             1172656c61792e6578616d706c652e6f7267\
             0434383231"
        );
        assert_eq!(ci.len(), 39);
        assert_eq!(
            hex::encode(ad_inviter()),
            "746f6d692f76312f706169722d696e76697465720001"
        );
        assert_eq!(ad_inviter().len(), 22);
        assert_eq!(
            hex::encode(ad_joiner()),
            "746f6d692f76312f706169722d6a6f696e65720001"
        );
        assert_eq!(ad_joiner().len(), 21);
    }

    #[test]
    fn commitment_is_a_68_byte_hash_input() {
        let sid = [7u8; 16];
        let ya = [9u8; 32];
        let mut m = b"tomi/v1/pair-commit".to_vec();
        m.push(0);
        m.extend_from_slice(&sid);
        m.extend_from_slice(&ya);
        assert_eq!(m.len(), 68);
        assert_eq!(commitment(&sid, &ya), sha256(&[&m]));
        assert_ne!(commitment(&sid, &ya), commitment(&[8u8; 16], &ya));
        assert_ne!(commitment(&sid, &ya), commitment(&sid, &[8u8; 32]));
    }

    #[test]
    fn derivation_separates_the_two_directions_and_the_sas() {
        let isk = [0x42u8; ISK_LEN];
        let (keys, sas) = derive(&isk).unwrap();
        assert_ne!(*keys.inviter, *keys.joiner);
        assert!(sas.value() < 1_000_000);
        let (again, sas2) = derive(&isk).unwrap();
        assert_eq!(*again.inviter, *keys.inviter);
        assert_eq!(sas2, sas);
        let (other, _) = derive(&[0x43u8; ISK_LEN]).unwrap();
        assert_ne!(*other.inviter, *keys.inviter);
        assert_eq!(format!("{keys:?}"), "SessionKeys(<redacted>)");
    }
}
