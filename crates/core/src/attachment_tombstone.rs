//! Signed removal of ONE attachment of an item (spec `format-v1.md` §8.10).
//!
//! The twin of [`crate::Tombstone`] one level down: it names an item, the `slot` (position
//! of the attachment in the payload at the time the item was sent) and the `blob_id` of that
//! attachment. The slot is never renumbered (the index is part of the chunk AAD, §6.3), so a
//! removed attachment keeps its place. Only the scope "everywhere" travels; a local removal
//! is never signed nor sent.
//!
//! Its signature label is its own: a signature made for an item tombstone can never be
//! replayed as the removal of an attachment, nor the other way round.

use crate::crypto::{format_uuid, labels, parse_uuid, to_array, Reader, Writer};
use crate::device::{verify_signature, DeviceKeys, SIGNATURE_LEN};
use crate::error::{CoreError, Result};
use crate::pin::PIN_AT_MS_MAX;

/// Upper bound of `deleted_at_ms` (2^53 - 1), so that it compares the same way in every
/// language that reads it as a double.
pub const ATTACHMENT_TOMBSTONE_AT_MS_MAX: u64 = PIN_AT_MS_MAX;

/// Wire length of an attachment tombstone: label (32) + `0x00` + space id + item id + blob
/// id + slot (u32) + time (u64) + signer + signature.
pub const ATTACHMENT_TOMBSTONE_LEN: usize =
    labels::SIG_ATTACHMENT_TOMBSTONE.len() + 1 + 16 + 16 + 16 + 4 + 8 + 32 + SIGNATURE_LEN;

/// `{ item_id, slot, blob_id, deleted_at }` signed by a member device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentTombstone {
    pub space_id: String,
    pub item_id: String,
    /// Blob id of the attachment in the payload the item was sent with.
    pub blob_id: String,
    /// Position of the attachment in `payload.attachments` (never renumbered).
    pub slot: u32,
    pub deleted_at_ms: u64,
    pub signer_ed25519: Vec<u8>,
    pub signature: Vec<u8>,
}

impl AttachmentTombstone {
    fn tbs(&self) -> Result<Vec<u8>> {
        if self.deleted_at_ms > ATTACHMENT_TOMBSTONE_AT_MS_MAX {
            return Err(CoreError::statement("deleted_at above 2^53 - 1"));
        }
        let sid = parse_uuid(&self.space_id, "space id")?;
        let iid = parse_uuid(&self.item_id, "item id")?;
        let bid = parse_uuid(&self.blob_id, "blob id")?;
        let signer: [u8; 32] = to_array(&self.signer_ed25519, "signer")?;
        let mut w = Writer::labeled(labels::SIG_ATTACHMENT_TOMBSTONE);
        w.bytes(&sid)
            .bytes(&iid)
            .bytes(&bid)
            .u32(self.slot)
            .u64(self.deleted_at_ms)
            .bytes(&signer);
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature` ([`ATTACHMENT_TOMBSTONE_LEN`] bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.signature.len() != SIGNATURE_LEN {
            return Err(CoreError::input("signature: expected 64 bytes"));
        }
        let mut out = self.tbs()?;
        out.extend_from_slice(&self.signature);
        Ok(out)
    }

    /// Parses the wire form (does not verify the signature). Exact length required.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ATTACHMENT_TOMBSTONE_LEN {
            return Err(CoreError::statement("attachment tombstone: wrong length"));
        }
        let mut r = Reader::new(bytes);
        r.expect_label(labels::SIG_ATTACHMENT_TOMBSTONE)?;
        let sid: [u8; 16] = r.array()?;
        let iid: [u8; 16] = r.array()?;
        let bid: [u8; 16] = r.array()?;
        let slot = r.u32()?;
        let deleted_at_ms = r.u64()?;
        if deleted_at_ms > ATTACHMENT_TOMBSTONE_AT_MS_MAX {
            return Err(CoreError::statement("deleted_at above 2^53 - 1"));
        }
        let signer: [u8; 32] = r.array()?;
        let sig = r.take(SIGNATURE_LEN)?;
        Ok(Self {
            space_id: format_uuid(&sid),
            item_id: format_uuid(&iid),
            blob_id: format_uuid(&bid),
            slot,
            deleted_at_ms,
            signer_ed25519: signer.to_vec(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `signer_ed25519` only. Devices must use
    /// [`crate::TrustStore::verify_attachment_tombstone`]; the relay checks that the signer
    /// is a registered device itself.
    pub fn verify_signature(&self) -> Result<()> {
        let signer: [u8; 32] = to_array(&self.signer_ed25519, "signer")?;
        verify_signature(&signer, &self.tbs()?, &self.signature)
    }
}

impl DeviceKeys {
    /// Signs the removal of attachment `slot` (blob `blob_id`) of `item_id`.
    pub fn sign_attachment_tombstone(
        &self,
        space_id: &str,
        item_id: &str,
        blob_id: &str,
        slot: u32,
        deleted_at_ms: u64,
    ) -> Result<AttachmentTombstone> {
        let mut t = AttachmentTombstone {
            space_id: space_id.to_owned(),
            item_id: item_id.to_owned(),
            blob_id: blob_id.to_owned(),
            slot,
            deleted_at_ms,
            signer_ed25519: self.ed_public().to_vec(),
            signature: Vec::new(),
        };
        t.signature = self.sign(&t.tbs()?);
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tombstone::{DeletionScope, Tombstone};

    const SPACE: &str = "11111111-1111-4111-8111-111111111111";
    const ITEM: &str = "22222222-2222-4222-8222-222222222222";
    const BLOB: &str = "33333333-3333-4333-8333-333333333333";

    fn signed() -> (DeviceKeys, AttachmentTombstone) {
        let k = DeviceKeys::generate().unwrap();
        let t = k
            .sign_attachment_tombstone(SPACE, ITEM, BLOB, 2, 5)
            .unwrap();
        (k, t)
    }

    #[test]
    fn length_is_189_bytes() {
        assert_eq!(ATTACHMENT_TOMBSTONE_LEN, 189);
    }

    #[test]
    fn roundtrip_and_tamper() {
        let (_k, t) = signed();
        let bytes = t.to_bytes().unwrap();
        assert_eq!(bytes.len(), ATTACHMENT_TOMBSTONE_LEN);
        let parsed = AttachmentTombstone::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, t);
        parsed.verify_signature().unwrap();
        for forged in [
            AttachmentTombstone {
                slot: 3,
                ..parsed.clone()
            },
            AttachmentTombstone {
                deleted_at_ms: 6,
                ..parsed.clone()
            },
            AttachmentTombstone {
                blob_id: "44444444-4444-4444-8444-444444444444".into(),
                ..parsed.clone()
            },
            AttachmentTombstone {
                item_id: "44444444-4444-4444-8444-444444444444".into(),
                ..parsed.clone()
            },
        ] {
            assert_eq!(
                forged.verify_signature().unwrap_err(),
                CoreError::InvalidSignature
            );
        }
    }

    #[test]
    fn exact_length_and_time_bound() {
        let (k, t) = signed();
        let mut bytes = t.to_bytes().unwrap();
        bytes.push(0);
        assert!(AttachmentTombstone::from_bytes(&bytes).is_err());
        bytes.truncate(ATTACHMENT_TOMBSTONE_LEN - 1);
        assert!(AttachmentTombstone::from_bytes(&bytes).is_err());
        assert!(k
            .sign_attachment_tombstone(SPACE, ITEM, BLOB, 0, 1 << 53)
            .is_err());
        k.sign_attachment_tombstone(SPACE, ITEM, BLOB, 0, (1 << 53) - 1)
            .unwrap();
    }

    #[test]
    fn an_item_tombstone_is_not_an_attachment_removal() {
        let k = DeviceKeys::generate().unwrap();
        let item = k
            .sign_tombstone(SPACE, ITEM, 5, DeletionScope::Everywhere)
            .unwrap();
        assert!(AttachmentTombstone::from_bytes(&item.to_bytes().unwrap()).is_err());
        let (_k, att) = signed();
        assert!(Tombstone::from_bytes(&att.to_bytes().unwrap()).is_err());
    }
}
