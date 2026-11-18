//! The pairing batch: built by the inviter, verified by the new device
//! (`spec/pairing-v1.md` §8).
//!
//! The batch adds **no** cryptographic construction: it reuses `DeviceCert`, `MemberList` and the
//! sealed space key of `format-v1.md` through the existing functions of the core
//! (`seal_space_key`, `DeviceKeys::sign_device_cert`, `TrustStore::{clone, add_verified_peer,
//! issue_member_list, accept_member_list}`, `DeviceCert::from_bytes`, `MemberList::from_bytes`,
//! `DeviceKeys::open_space_key`).

use zeroize::Zeroizing;

use super::entropy::Entropy;
use super::info::{HistoryAnnouncement, HistoryOffer};
use super::tlv::{HistoryWire, ProvisionBody};
use crate::crypto::{format_uuid, SpaceKey, NONCE_LEN};
use crate::device::{
    seal_space_key_raw, DeviceCert, DeviceKeys, DevicePublic, MemberList, TrustStore,
};
use crate::error::{CoreError, Result};

/// What the inviter produces at step A10.
pub(super) struct Built {
    /// The encoded `Provision` body, to seal in the `provision` frame.
    pub(super) body: Vec<u8>,
    /// The inviter's trust store with the new device added. It replaces the real store only
    /// after `POST /v1/devices` answered `201` (§8.1); until then nothing is committed.
    pub(super) staged: TrustStore,
    /// `DeviceCert(N)`, signed by the inviter (wire form), for `POST /v1/devices`.
    pub(super) device_cert: Vec<u8>,
    /// The history announcement, when the user of N asked for the history and A offered some.
    pub(super) history: Option<HistoryAnnouncement>,
}

/// A fresh UUID v4 from 16 random bytes (RFC 9562 §5.4), as `new_uuid` does.
fn uuid_v4(entropy: &mut Entropy) -> Result<[u8; 16]> {
    let mut b = entropy.array::<16>()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(b)
}

/// Builds the batch for `peer` (§8.1). `peer` comes from the authenticated PAKE channel (D38),
/// never from the relay.
///
/// Randomness is drawn from `entropy` in this order: sealing ephemeral secret (32 bytes), sealing
/// nonce (24 bytes), and, only when a history is announced, the transfer id (16 bytes).
/// `seal_space_key_raw` is the function behind `seal_space_key`, which draws the same two values
/// from the OS.
pub(super) fn build(
    trust: &TrustStore,
    keys: &DeviceKeys,
    space_key: &SpaceKey,
    peer: &DevicePublic,
    now_ms: u64,
    announce: Option<HistoryOffer>,
    entropy: &mut Entropy,
) -> Result<Built> {
    let space_id = trust.space_id();
    let space_id_bytes = trust.space_id_bytes();
    let key_epoch = trust.key_epoch();
    if space_key.epoch() != key_epoch {
        return Err(CoreError::input(
            "space key epoch differs from the trust store",
        ));
    }
    let mut ephemeral = Zeroizing::new([0u8; 32]);
    entropy.fill(ephemeral.as_mut())?;
    let nonce = entropy.array::<NONCE_LEN>()?;
    let sealed = seal_space_key_raw(peer, &space_id_bytes, space_key, &ephemeral, &nonce)?;
    let history = match announce {
        Some(offer) => Some(HistoryWire {
            transfer_id: uuid_v4(entropy)?,
            items: offer.items,
            bytes: offer.bytes,
        }),
        None => None,
    };
    let cert = keys.sign_device_cert(&space_id, peer, now_ms)?;
    let mut staged = trust.clone();
    staged.add_verified_peer(peer)?;
    let list = staged.issue_member_list(keys, now_ms)?;
    let device_cert = cert.to_bytes()?;
    let body = ProvisionBody {
        space_id: space_id_bytes,
        key_epoch,
        sealed,
        device_cert: device_cert.clone(),
        member_list: list.to_bytes()?,
        history,
    }
    .encode()?;
    Ok(Built {
        body,
        staged,
        device_cert,
        history: history.map(announcement),
    })
}

fn announcement(h: HistoryWire) -> HistoryAnnouncement {
    HistoryAnnouncement {
        transfer_id: format_uuid(&h.transfer_id),
        items: h.items,
        bytes: h.bytes,
    }
}

/// What the new device gets from a verified batch.
pub(super) struct Verified {
    pub(super) trust: TrustStore,
    pub(super) space_key: SpaceKey,
    pub(super) history: Option<HistoryAnnouncement>,
}

/// Verifies a batch, in the exact order of §8.3. Any failure refuses the whole batch.
///
/// The functions of the core keep their own error (`invalid_signature`, `key_conflict`,
/// `decryption_failed`, ...); the checks that bind the batch to this pairing are
/// `pairing_failed`. The pairing machine turns every failure into `pairing_failed`.
///
/// * `own`: the keys this device generated for the pairing;
/// * `inviter`: the public keys the inviter announced in `InviterHello`.
pub(super) fn verify(
    own: &DeviceKeys,
    inviter: &DevicePublic,
    body: &ProvisionBody,
) -> Result<Verified> {
    let own_public = own.public();
    let space_id = format_uuid(&body.space_id);

    // 2. Device certificate: for this space, about me, issued by the inviter, not self-signed.
    let cert = DeviceCert::from_bytes(&body.device_cert)?;
    if cert.space_id != space_id
        || cert.subject != own_public
        || cert.issuer_ed25519 != inviter.ed25519
        || cert.is_self_signed()
    {
        return Err(CoreError::PairingFailed);
    }
    cert.verify_signature()?;

    // 3. Member list: this space and epoch, issued by the inviter, and it lists the inviter.
    //    (An entry for the inviter with another X25519 key, or for this device with another
    //    key than its own, is `key_conflict` at step 5.)
    let list = MemberList::from_bytes(&body.member_list)?;
    if list.space_id != space_id
        || list.key_epoch != body.key_epoch
        || list.issuer_ed25519 != inviter.ed25519
        || !list.members.iter().any(|m| m.ed25519 == inviter.ed25519)
    {
        return Err(CoreError::PairingFailed);
    }

    // 4. The space key, sealed for my X25519 key and bound to the space, the epoch and my
    //    Ed25519 key.
    let space_key = own.open_space_key(&space_id, body.key_epoch, &body.sealed)?;

    // 5. My trust store: me, the inviter (verified face to face by the PAKE), then the list.
    let mut trust = TrustStore::new(&space_id, &own_public, body.key_epoch)?;
    trust.add_verified_peer(inviter)?;
    trust.accept_member_list(&list)?;

    Ok(Verified {
        trust,
        space_key,
        history: body.history.map(announcement),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::new_space_id;
    use crate::pairing::frame::MAX_FRAME_LEN;

    struct World {
        a: DeviceKeys,
        c: DeviceKeys,
        n: DeviceKeys,
        space_id: String,
        key: SpaceKey,
        trust_a: TrustStore,
    }

    fn world() -> World {
        let a = DeviceKeys::generate().unwrap();
        let c = DeviceKeys::generate().unwrap();
        let n = DeviceKeys::generate().unwrap();
        let space_id = new_space_id().unwrap();
        let key = SpaceKey::generate(1).unwrap();
        let mut trust_a = TrustStore::new(&space_id, &a.public(), 1).unwrap();
        trust_a.add_verified_peer(&c.public()).unwrap();
        World {
            a,
            c,
            n,
            space_id,
            key,
            trust_a,
        }
    }

    fn build_for(w: &World, announce: Option<HistoryOffer>) -> Built {
        build(
            &w.trust_a,
            &w.a,
            &w.key,
            &w.n.public(),
            1_000,
            announce,
            &mut Entropy::os(),
        )
        .unwrap()
    }

    #[test]
    fn built_batch_verifies_and_gives_the_new_device_the_space() {
        let w = world();
        let built = build_for(&w, None);
        assert!(built.history.is_none());
        let body = ProvisionBody::decode(&built.body).unwrap();
        let v = verify(&w.n, &w.a.public(), &body).unwrap();
        assert_eq!(v.space_key.to_bytes(), w.key.to_bytes());
        assert_eq!(v.space_key.epoch(), 1);
        assert_eq!(v.trust.space_id(), w.space_id);
        for d in [&w.a, &w.c, &w.n] {
            assert!(v.trust.is_member(&d.public().ed25519));
        }
        assert_eq!(v.trust.members().len(), 3);
        assert_eq!(v.trust.key_epoch(), 1);
        // The inviter's own store is untouched; the staged copy has the new device.
        assert!(!w.trust_a.is_member(&w.n.public().ed25519));
        assert!(built.staged.is_member(&w.n.public().ed25519));
        assert_eq!(built.staged.members().len(), 3);
        // The certificate is the one a third member accepts.
        let cert = DeviceCert::from_bytes(&built.device_cert).unwrap();
        assert_eq!(cert.subject, w.n.public());
        let mut trust_c = TrustStore::new(&w.space_id, &w.c.public(), 1).unwrap();
        trust_c.add_verified_peer(&w.a.public()).unwrap();
        trust_c.accept_device_cert(&cert).unwrap();
        assert!(trust_c.is_member(&w.n.public().ed25519));
    }

    #[test]
    fn history_is_announced_with_a_fresh_uuid_v4() {
        let w = world();
        let offer = HistoryOffer {
            items: 812,
            bytes: 356_515_840,
        };
        let built = build_for(&w, Some(offer));
        let h = built.history.clone().unwrap();
        assert_eq!((h.items, h.bytes), (812, 356_515_840));
        let parsed = uuid::Uuid::parse_str(&h.transfer_id).unwrap();
        assert_eq!(parsed.get_version_num(), 4);
        let again = build_for(&w, Some(offer)).history.unwrap();
        assert_ne!(again.transfer_id, h.transfer_id);
        let body = ProvisionBody::decode(&built.body).unwrap();
        let v = verify(&w.n, &w.a.public(), &body).unwrap();
        assert_eq!(v.history, Some(h));
    }

    #[test]
    fn epoch_mismatch_is_refused_before_anything_is_sealed() {
        let w = world();
        let key2 = SpaceKey::generate(2).unwrap();
        let err = build(
            &w.trust_a,
            &w.a,
            &key2,
            &w.n.public(),
            1,
            None,
            &mut Entropy::os(),
        )
        .err()
        .unwrap();
        assert_eq!(err.code(), "invalid_input");
    }

    #[test]
    fn an_issuer_outside_the_trust_store_cannot_issue() {
        // The inviter must be a trusted member to issue a member list.
        let w = world();
        let stranger = DeviceKeys::generate().unwrap();
        let err = build(
            &w.trust_a,
            &stranger,
            &w.key,
            &w.n.public(),
            1,
            None,
            &mut Entropy::os(),
        )
        .err()
        .unwrap();
        assert_eq!(err.code(), "untrusted_signer");
    }

    #[test]
    fn sixty_four_bytes_per_member_bound_the_frame() {
        // Members of the list = existing members + the new device (pairing-v1.md §6.6).
        let a = DeviceKeys::generate().unwrap();
        let n = DeviceKeys::generate().unwrap();
        let space_id = new_space_id().unwrap();
        let key = SpaceKey::generate(1).unwrap();
        let mut trust = TrustStore::new(&space_id, &a.public(), 1).unwrap();
        let mut sizes = Vec::new();
        for existing in 1..=119usize {
            while trust.members().len() < existing {
                trust
                    .add_verified_peer(&DeviceKeys::generate().unwrap().public())
                    .unwrap();
            }
            if existing == 118 || existing == 119 {
                let built = build(
                    &trust,
                    &a,
                    &key,
                    &n.public(),
                    1,
                    Some(HistoryOffer { items: 1, bytes: 1 }),
                    &mut Entropy::os(),
                )
                .unwrap();
                sizes.push((existing + 1, 2 + 16 + built.body.len()));
            }
        }
        // 119 members in the list: 552 + 64 * 119 = 8168 bytes; 120 members: 8232.
        assert_eq!(sizes[0], (119, 552 + 64 * 119));
        assert!(sizes[0].1 <= MAX_FRAME_LEN);
        assert_eq!(sizes[1], (120, 552 + 64 * 120));
        assert!(sizes[1].1 > MAX_FRAME_LEN);
    }
}
