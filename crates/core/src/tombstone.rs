//! Signed deletion tombstones (spec §8.5).

use crate::crypto::{format_uuid, labels, parse_uuid, to_array, Reader, Writer};
use crate::device::{verify_signature, DeviceKeys, SIGNATURE_LEN};
use crate::error::{CoreError, Result};

/// Scope of a deletion (PROJECT.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionScope {
    /// This device only. Never uploaded; a received local tombstone is ignored.
    Local,
    /// Everywhere: the relay purges the item and every device deletes it.
    Everywhere,
}

impl DeletionScope {
    fn code(self) -> u8 {
        match self {
            DeletionScope::Local => 1,
            DeletionScope::Everywhere => 2,
        }
    }
    fn from_code(c: u8) -> Result<Self> {
        match c {
            1 => Ok(DeletionScope::Local),
            2 => Ok(DeletionScope::Everywhere),
            _ => Err(CoreError::statement("unknown deletion scope")),
        }
    }
}

/// `{ item_id, deleted_at, scope }` signed by a member device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub space_id: String,
    pub item_id: String,
    pub deleted_at_ms: u64,
    pub scope: DeletionScope,
    pub signer_ed25519: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Wire length of a v1 tombstone.
pub const TOMBSTONE_LEN: usize = 21 + 1 + 16 + 16 + 8 + 1 + 32 + SIGNATURE_LEN;

impl Tombstone {
    fn tbs(&self) -> Result<Vec<u8>> {
        let sid = parse_uuid(&self.space_id, "space id")?;
        let iid = parse_uuid(&self.item_id, "item id")?;
        let signer: [u8; 32] = to_array(&self.signer_ed25519, "signer")?;
        let mut w = Writer::labeled(labels::SIG_TOMBSTONE);
        w.bytes(&sid)
            .bytes(&iid)
            .u64(self.deleted_at_ms)
            .u8(self.scope.code())
            .bytes(&signer);
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature` ([`TOMBSTONE_LEN`] bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.signature.len() != SIGNATURE_LEN {
            return Err(CoreError::input("signature: expected 64 bytes"));
        }
        let mut out = self.tbs()?;
        out.extend_from_slice(&self.signature);
        Ok(out)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != TOMBSTONE_LEN {
            return Err(CoreError::statement("tombstone: wrong length"));
        }
        let mut r = Reader::new(bytes);
        r.expect_label(labels::SIG_TOMBSTONE)?;
        let sid: [u8; 16] = r.array()?;
        let iid: [u8; 16] = r.array()?;
        let deleted_at_ms = r.u64()?;
        let scope = DeletionScope::from_code(r.u8()?)?;
        let signer: [u8; 32] = r.array()?;
        let sig = r.take(SIGNATURE_LEN)?;
        Ok(Self {
            space_id: format_uuid(&sid),
            item_id: format_uuid(&iid),
            deleted_at_ms,
            scope,
            signer_ed25519: signer.to_vec(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `signer_ed25519` only. Devices must use
    /// [`crate::TrustStore::verify_tombstone`]; the relay checks that the signer is a
    /// registered device itself.
    pub fn verify_signature(&self) -> Result<()> {
        let signer: [u8; 32] = to_array(&self.signer_ed25519, "signer")?;
        verify_signature(&signer, &self.tbs()?, &self.signature)
    }
}

impl DeviceKeys {
    /// Signs a tombstone for `item_id`.
    pub fn sign_tombstone(
        &self,
        space_id: &str,
        item_id: &str,
        deleted_at_ms: u64,
        scope: DeletionScope,
    ) -> Result<Tombstone> {
        let mut t = Tombstone {
            space_id: space_id.to_owned(),
            item_id: item_id.to_owned(),
            deleted_at_ms,
            scope,
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

    #[test]
    fn roundtrip_and_tamper() {
        let k = DeviceKeys::generate().unwrap();
        let t = k
            .sign_tombstone(
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222",
                5,
                DeletionScope::Everywhere,
            )
            .unwrap();
        let bytes = t.to_bytes().unwrap();
        assert_eq!(bytes.len(), TOMBSTONE_LEN);
        let parsed = Tombstone::from_bytes(&bytes).unwrap();
        parsed.verify_signature().unwrap();
        let mut forged = parsed.clone();
        forged.scope = DeletionScope::Local;
        assert_eq!(
            forged.verify_signature().unwrap_err(),
            CoreError::InvalidSignature
        );
    }
}
