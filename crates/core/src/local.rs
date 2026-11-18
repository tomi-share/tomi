//! Device-local storage key `LSK` (spec §7.2, §13.3; decision D1).
//!
//! Once a received item is opened, the client re-seals its payload under a random key
//! that belongs to this device only (kept in the Android Keystore or with DPAPI, never
//! derived from a space key). Local storage thus no longer depends on the space key
//! epochs, which can be discarded after a rotation ([`crate::SpaceKeyRing`]).
//!
//! Record layout (local, not exchanged, not covered by the vectors):
//! `u8(version = 1) || nonce (24) || XC_Seal(LSK, nonce, AAD_local, payload JSON)` with
//! `AAD_local = "tomi/v1/aad/local-item" || 0x00 || u8(version) || uuid16(item_id)`.

use zeroize::Zeroizing;

use crate::crypto::{
    aead_open, aead_seal, labels, parse_uuid, random_array, random_key, to_array, Writer, KEY_LEN,
    NONCE_LEN,
};
use crate::error::{CoreError, Result};
use crate::format::TAG_LEN;
use crate::item::Item;

/// Version byte of a local record.
pub const LOCAL_RECORD_VERSION: u8 = 1;
/// Bytes added to the payload JSON by [`LocalKey::seal_item`].
pub const LOCAL_RECORD_OVERHEAD: usize = 1 + NONCE_LEN + TAG_LEN;

fn aad_local_item(version: u8, item_id: &[u8; 16]) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_LOCAL_ITEM);
    w.u8(version).bytes(item_id);
    w.finish()
}

/// Random key of this device's local item store. Zeroized on drop; `Debug` redacted.
#[derive(Clone)]
pub struct LocalKey {
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl LocalKey {
    /// Generated once per installation.
    pub fn generate() -> Result<Self> {
        Ok(Self { key: random_key()? })
    }

    /// Restores the key from secure storage.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            key: Zeroizing::new(to_array(bytes, "local key")?),
        })
    }

    /// Raw key bytes, only to persist them in secure storage. Never log them.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_vec()
    }

    /// Seals an opened item (title, secret flag and all content included).
    ///
    /// An item that comes from [`open_item`](Self::open_item), [`crate::open_item`] or a
    /// history import is sealed with its **original payload bytes**, so a field this build
    /// does not know survives (spec §5.4.4, §7.2). Fails with `invalid_input` if the item
    /// was edited since (see [`Item::raw_payload`]).
    pub fn seal_item(&self, item: &Item) -> Result<Vec<u8>> {
        self.seal_item_with_nonce(item, &random_array()?)
    }

    pub(crate) fn seal_item_with_nonce(
        &self,
        item: &Item,
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Vec<u8>> {
        let id = parse_uuid(&item.id, "item id")?;
        let json = Zeroizing::new(item.to_json()?);
        let ct = aead_seal(
            &self.key,
            nonce,
            &aad_local_item(LOCAL_RECORD_VERSION, &id),
            &json,
        )?;
        let mut out = Vec::with_capacity(LOCAL_RECORD_OVERHEAD + json.len());
        out.push(LOCAL_RECORD_VERSION);
        out.extend_from_slice(nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Opens a record stored under `item_id`.
    pub fn open_item(&self, record: &[u8], item_id: &str) -> Result<Item> {
        if record.len() < LOCAL_RECORD_OVERHEAD {
            return Err(CoreError::envelope("local record too short"));
        }
        if record[0] != LOCAL_RECORD_VERSION {
            return Err(CoreError::UnsupportedFormat {
                format: u32::from(record[0]),
                min: u32::from(LOCAL_RECORD_VERSION),
                max: u32::from(LOCAL_RECORD_VERSION),
            });
        }
        let id = parse_uuid(item_id, "item id")?;
        let nonce: [u8; NONCE_LEN] = to_array(&record[1..1 + NONCE_LEN], "nonce")?;
        let json = Zeroizing::new(aead_open(
            &self.key,
            &nonce,
            &aad_local_item(record[0], &id),
            &record[1 + NONCE_LEN..],
        )?);
        Item::from_json(&json, item_id)
    }
}

impl std::fmt::Debug for LocalKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocalKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ReadPolicy;

    #[test]
    fn roundtrip_and_binding() {
        let k = LocalKey::generate().unwrap();
        let item = Item {
            id: crate::new_item_id().unwrap(),
            source_device: "0123456789abcdef0123456789abcdef".into(),
            created_at_ms: 1,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            secret: true,
            title: Some("Code Wi-Fi".into()),
            text: None,
            url: None,
            attachments: vec![],
            raw_payload: None,
        };
        let rec = k.seal_item(&item).unwrap();
        assert_eq!(
            rec.len(),
            LOCAL_RECORD_OVERHEAD + item.to_json().unwrap().len()
        );
        assert!(!rec.windows(4).any(|w| w == b"Wi-F"));
        assert_eq!(k.open_item(&rec, &item.id).unwrap(), item);
        let other = crate::new_item_id().unwrap();
        assert_eq!(
            k.open_item(&rec, &other).unwrap_err(),
            CoreError::DecryptionFailed
        );
        let k2 = LocalKey::generate().unwrap();
        assert_eq!(
            k2.open_item(&rec, &item.id).unwrap_err(),
            CoreError::DecryptionFailed
        );
        assert_eq!(format!("{k:?}"), "LocalKey(<redacted>)");
    }

    #[test]
    fn unknown_fields_survive_local_reseal() {
        let k = LocalKey::generate().unwrap();
        let id = crate::new_item_id().unwrap();
        let payload = crate::item::testing::payload_with_unknown_fields(&id);
        // Received from a newer device: parsed, then re-sealed locally, then read back,
        // then re-sealed again (a second pass must not alter anything either).
        let received = Item::from_json(&payload, &id).unwrap();
        let rec = k.seal_item(&received).unwrap();
        let opened = k.open_item(&rec, &id).unwrap();
        assert_eq!(opened, received);
        assert_eq!(
            opened.raw_payload.as_ref().unwrap().as_bytes(),
            &payload[..]
        );
        let rec2 = k.seal_item(&opened).unwrap();
        assert_eq!(
            k.open_item(&rec2, &id)
                .unwrap()
                .raw_payload
                .unwrap()
                .as_bytes(),
            &payload[..]
        );
        // The record holds exactly the original bytes (same length as before).
        assert_eq!(rec.len(), LOCAL_RECORD_OVERHEAD + payload.len());
    }

    #[test]
    fn edited_item_is_not_sealed_with_a_stale_payload() {
        let k = LocalKey::generate().unwrap();
        let id = crate::new_item_id().unwrap();
        let payload = crate::item::testing::payload_with_unknown_fields(&id);
        let mut item = Item::from_json(&payload, &id).unwrap();
        item.secret = false;
        assert_eq!(k.seal_item(&item).unwrap_err().code(), "invalid_input");
        item.raw_payload = None;
        assert!(k.seal_item(&item).is_ok());
    }
}
