//! Item envelope v1: canonical AAD, binary layout, padding, seal/open (spec §5).

use zeroize::Zeroizing;

use crate::crypto::{
    aead_open, aead_seal, format_uuid, labels, parse_uuid, random_array, random_key, to_array,
    SpaceKey, Writer, KEY_LEN, NONCE_LEN,
};
use crate::error::{CoreError, Result};
use crate::format::{
    is_format_supported, FORMAT_MAX, FORMAT_MIN, MAX_PAYLOAD_LEN, PAYLOAD_PAD_BLOCK, TAG_LEN,
};
use crate::item::Item;

/// Offsets of the v1 envelope header (spec §5.2).
const OFF_EPOCH: usize = 4;
const OFF_ITEM_ID: usize = 8;
const OFF_WRAP_NONCE: usize = 24;
const OFF_WRAPPED_CEK: usize = 48;
const OFF_PAYLOAD_NONCE: usize = 96;
/// Length of the v1 header, i.e. offset of the payload ciphertext.
pub const ENVELOPE_HEADER_LEN: usize = 120;
const WRAPPED_CEK_LEN: usize = KEY_LEN + TAG_LEN;

/// Clear-text header fields of an envelope (the relay sees the same values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeHeader {
    pub format: u32,
    pub key_epoch: u32,
    pub item_id: String,
}

/// Result of [`open_item`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedItem {
    /// Format the item was written in.
    pub format: u32,
    /// Space key epoch that sealed it.
    pub key_epoch: u32,
    pub item: Item,
}

/// `AAD_wrap` (spec §5.1).
pub(crate) fn aad_cek_wrap(format: u32, key_epoch: u32, item_id: &[u8; 16]) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_CEK_WRAP);
    w.u32(format).u32(key_epoch).bytes(item_id);
    w.finish()
}

/// `AAD_payload` (spec §5.1).
pub(crate) fn aad_payload(format: u32, key_epoch: u32, item_id: &[u8; 16]) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_PAYLOAD);
    w.u32(format).u32(key_epoch).bytes(item_id);
    w.finish()
}

/// Appends `0x80 0x00*` up to the next multiple of 256 bytes (spec §5.3).
pub(crate) fn pad_payload(json: &[u8]) -> Vec<u8> {
    let padded_len = (json.len() / PAYLOAD_PAD_BLOCK + 1) * PAYLOAD_PAD_BLOCK;
    let mut out = Vec::with_capacity(padded_len);
    out.extend_from_slice(json);
    out.push(0x80);
    out.resize(padded_len, 0);
    out
}

/// Strict inverse of [`pad_payload`].
pub(crate) fn unpad_payload(padded: &[u8]) -> Result<&[u8]> {
    if padded.is_empty() || !padded.len().is_multiple_of(PAYLOAD_PAD_BLOCK) {
        return Err(CoreError::envelope("payload length not a multiple of 256"));
    }
    let marker = padded
        .iter()
        .rposition(|&b| b != 0)
        .ok_or_else(|| CoreError::envelope("missing padding marker"))?;
    if padded[marker] != 0x80 || padded.len() - marker > PAYLOAD_PAD_BLOCK {
        return Err(CoreError::envelope("invalid padding"));
    }
    Ok(&padded[..marker])
}

/// Low-level seal with explicit randomness. Public randomized API: [`seal_item`].
pub(crate) fn seal_raw(
    space_key: &SpaceKey,
    item_id: &[u8; 16],
    cek: &[u8; KEY_LEN],
    wrap_nonce: &[u8; NONCE_LEN],
    payload_nonce: &[u8; NONCE_LEN],
    json: &[u8],
) -> Result<Vec<u8>> {
    let format = FORMAT_MAX;
    let epoch = space_key.epoch();
    let padded = Zeroizing::new(pad_payload(json));
    if padded.len() > MAX_PAYLOAD_LEN {
        return Err(CoreError::input("payload exceeds 1 MiB"));
    }
    let wrap_key = space_key.cek_wrap_key()?;
    let wrapped = aead_seal(
        &wrap_key,
        wrap_nonce,
        &aad_cek_wrap(format, epoch, item_id),
        cek,
    )?;
    let ct = aead_seal(
        cek,
        payload_nonce,
        &aad_payload(format, epoch, item_id),
        &padded,
    )?;
    let mut out = Vec::with_capacity(ENVELOPE_HEADER_LEN + ct.len());
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&epoch.to_be_bytes());
    out.extend_from_slice(item_id);
    out.extend_from_slice(wrap_nonce);
    out.extend_from_slice(&wrapped);
    out.extend_from_slice(payload_nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Encrypts `item` into a v1 envelope under `space_key` (fresh random CEK and nonces).
///
/// The envelope is written in format [`FORMAT_MAX`]; when a future build supports
/// several formats, this function will take the space's effective format.
pub fn seal_item(item: &Item, space_key: &SpaceKey) -> Result<Vec<u8>> {
    let item_id = parse_uuid(&item.id, "item id")?;
    let json = Zeroizing::new(item.to_json()?);
    let cek = random_key()?;
    let wrap_nonce: [u8; NONCE_LEN] = random_array()?;
    let payload_nonce: [u8; NONCE_LEN] = random_array()?;
    seal_raw(
        space_key,
        &item_id,
        &cek,
        &wrap_nonce,
        &payload_nonce,
        &json,
    )
}

/// Reads the clear header. Fails with `UnsupportedFormat` for an unknown format,
/// in which case the client keeps the item and shows it as unreadable.
pub fn read_envelope_header(envelope: &[u8]) -> Result<EnvelopeHeader> {
    let format_bytes: [u8; 4] = envelope
        .get(..4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| CoreError::envelope("shorter than 4 bytes"))?;
    let format = u32::from_be_bytes(format_bytes);
    if !is_format_supported(format) {
        return Err(CoreError::UnsupportedFormat {
            format,
            min: FORMAT_MIN,
            max: FORMAT_MAX,
        });
    }
    let min_len = ENVELOPE_HEADER_LEN + PAYLOAD_PAD_BLOCK + TAG_LEN;
    if envelope.len() < min_len {
        return Err(CoreError::envelope("too short"));
    }
    if !(envelope.len() - ENVELOPE_HEADER_LEN - TAG_LEN).is_multiple_of(PAYLOAD_PAD_BLOCK) {
        return Err(CoreError::envelope("payload length not a multiple of 256"));
    }
    if envelope.len() - ENVELOPE_HEADER_LEN - TAG_LEN > MAX_PAYLOAD_LEN {
        return Err(CoreError::envelope("payload exceeds 1 MiB"));
    }
    let key_epoch = u32::from_be_bytes(to_array(&envelope[OFF_EPOCH..OFF_ITEM_ID], "key_epoch")?);
    let item_id: [u8; 16] = to_array(&envelope[OFF_ITEM_ID..OFF_WRAP_NONCE], "item_id")?;
    Ok(EnvelopeHeader {
        format,
        key_epoch,
        item_id: format_uuid(&item_id),
    })
}

/// Decrypts a v1 envelope.
///
/// * `expected_item_id`: the id under which the relay listed the envelope.
/// * `space_key`: the key whose epoch matches [`EnvelopeHeader::key_epoch`].
///
/// Never delete an item because this fails (DEV.md invariant 7).
pub fn open_item(
    envelope: &[u8],
    expected_item_id: &str,
    space_key: &SpaceKey,
) -> Result<OpenedItem> {
    let header = read_envelope_header(envelope)?;
    if header.item_id != expected_item_id {
        return Err(CoreError::ItemIdMismatch);
    }
    if header.key_epoch != space_key.epoch() {
        return Err(CoreError::KeyEpochMismatch {
            envelope: header.key_epoch,
            key: space_key.epoch(),
        });
    }
    let item_id: [u8; 16] = to_array(&envelope[OFF_ITEM_ID..OFF_WRAP_NONCE], "item_id")?;
    let wrap_nonce: [u8; NONCE_LEN] =
        to_array(&envelope[OFF_WRAP_NONCE..OFF_WRAPPED_CEK], "wrap nonce")?;
    let wrapped = &envelope[OFF_WRAPPED_CEK..OFF_WRAPPED_CEK + WRAPPED_CEK_LEN];
    let payload_nonce: [u8; NONCE_LEN] = to_array(
        &envelope[OFF_PAYLOAD_NONCE..ENVELOPE_HEADER_LEN],
        "payload nonce",
    )?;
    let ct = &envelope[ENVELOPE_HEADER_LEN..];

    let wrap_key = space_key.cek_wrap_key()?;
    let cek = Zeroizing::new(aead_open(
        &wrap_key,
        &wrap_nonce,
        &aad_cek_wrap(header.format, header.key_epoch, &item_id),
        wrapped,
    )?);
    let cek: Zeroizing<[u8; KEY_LEN]> =
        Zeroizing::new(to_array(&cek, "cek").map_err(|_| CoreError::DecryptionFailed)?);
    let padded = Zeroizing::new(aead_open(
        &cek,
        &payload_nonce,
        &aad_payload(header.format, header.key_epoch, &item_id),
        ct,
    )?);
    let json = unpad_payload(&padded)?;
    let item = Item::from_json(json, &header.item_id)?;
    Ok(OpenedItem {
        format: header.format,
        key_epoch: header.key_epoch,
        item,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding() {
        assert_eq!(pad_payload(b"").len(), 256);
        assert_eq!(pad_payload(&[1u8; 255]).len(), 256);
        assert_eq!(pad_payload(&[1u8; 256]).len(), 512);
        let p = pad_payload(b"{}");
        assert_eq!(unpad_payload(&p).unwrap(), b"{}");
        let mut bad = p.clone();
        bad[2] = 0x81;
        assert!(unpad_payload(&bad).is_err());
        // Over-long padding (a whole extra zero block) is rejected.
        let mut long = p.clone();
        long.extend_from_slice(&[0u8; 256]);
        assert!(unpad_payload(&long).is_err());
    }

    #[test]
    fn unknown_format_is_reported() {
        let mut env = vec![0u8; 400];
        env[3] = 2;
        assert_eq!(
            read_envelope_header(&env).unwrap_err().code(),
            "unsupported_format"
        );
    }
}
