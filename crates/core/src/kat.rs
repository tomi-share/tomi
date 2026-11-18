//! Known-answer-test hooks: deterministic variants of the randomized operations,
//! used only by `tests/spec_vectors.rs` and `examples/gen_spec_vectors.rs`.
//!
//! **Never call these from application code**: reusing a nonce or a CEK breaks the
//! confidentiality of every item concerned. Not part of the UniFFI surface.

use zeroize::Zeroizing;

use crate::crypto::{self, parse_uuid, to_array, SpaceKey, KEY_LEN, NONCE_LEN, NONCE_PREFIX_LEN};
use crate::device::{self, DeviceKeys, DevicePublic, RotationBundle, TrustStore};
use crate::envelope;
use crate::error::Result;
use crate::history::{self, HistoryKey, HistoryPart, HistoryPartEncryptor, HistoryTransfer};
use crate::item::Item;
use crate::pin;
use crate::stream::{self, ChunkEncryptor};

/// Raw XChaCha20-Poly1305 (ciphertext || tag).
pub fn xchacha20poly1305_seal(key: &[u8], nonce: &[u8], aad: &[u8], pt: &[u8]) -> Result<Vec<u8>> {
    crypto::aead_seal(&to_array(key, "key")?, &to_array(nonce, "nonce")?, aad, pt)
}

/// Raw XChaCha20-Poly1305 decryption.
pub fn xchacha20poly1305_open(key: &[u8], nonce: &[u8], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
    crypto::aead_open(&to_array(key, "key")?, &to_array(nonce, "nonce")?, aad, ct)
}

/// Raw HKDF-SHA256; an empty `salt` means "absent" (RFC 5869: HashLen zero bytes).
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>> {
    let salt = if salt.is_empty() { None } else { Some(salt) };
    Ok(crypto::hkdf_sha256(salt, ikm, info, len)?.to_vec())
}

/// X25519 public key of a secret.
pub fn x25519_public(secret: &[u8]) -> Result<Vec<u8>> {
    let s = x25519_dalek::StaticSecret::from(to_array::<32>(secret, "secret")?);
    Ok(x25519_dalek::PublicKey::from(&s).to_bytes().to_vec())
}

/// X25519 shared secret.
pub fn x25519_shared(secret: &[u8], public: &[u8]) -> Result<Vec<u8>> {
    let s = x25519_dalek::StaticSecret::from(to_array::<32>(secret, "secret")?);
    let p = x25519_dalek::PublicKey::from(to_array::<32>(public, "public")?);
    Ok(s.diffie_hellman(&p).to_bytes().to_vec())
}

/// Ed25519 signature with a 32-byte seed (RFC 8032, pure Ed25519).
pub fn ed25519_sign(seed: &[u8], message: &[u8]) -> Result<Vec<u8>> {
    let keys = DeviceKeys::from_secrets(seed, &[0u8; 32])?;
    Ok(keys.sign(message))
}

/// Strict Ed25519 verification as used by the format.
pub fn ed25519_verify(public: &[u8], message: &[u8], signature: &[u8]) -> Result<()> {
    device::verify_signature(&to_array(public, "public")?, message, signature)
}

/// `K_wrap` derived from a space key (spec §4.2).
pub fn cek_wrap_key(space_key: &SpaceKey) -> Result<Vec<u8>> {
    Ok(space_key.cek_wrap_key()?.to_vec())
}

/// `K_transport` (spec §7.3).
pub fn transport_key(
    shared: &[u8],
    ephemeral_public: &[u8],
    recipient_x25519: &[u8],
) -> Result<Vec<u8>> {
    Ok(device::transport_key(
        &to_array(shared, "shared")?,
        &to_array(ephemeral_public, "ephemeral")?,
        &to_array(recipient_x25519, "recipient")?,
    )?
    .to_vec())
}

pub fn aad_cek_wrap(format: u32, key_epoch: u32, item_id: &str) -> Result<Vec<u8>> {
    Ok(envelope::aad_cek_wrap(
        format,
        key_epoch,
        &parse_uuid(item_id, "item id")?,
    ))
}

pub fn aad_payload(format: u32, key_epoch: u32, item_id: &str) -> Result<Vec<u8>> {
    Ok(envelope::aad_payload(
        format,
        key_epoch,
        &parse_uuid(item_id, "item id")?,
    ))
}

pub fn aad_chunk(format: u32, item_id: &str, attachment_index: u32) -> Result<Vec<u8>> {
    Ok(stream::aad_chunk(
        format,
        &parse_uuid(item_id, "item id")?,
        attachment_index,
    ))
}

pub fn aad_key_transport(
    space_id: &str,
    key_epoch: u32,
    recipient_ed25519: &[u8],
) -> Result<Vec<u8>> {
    Ok(device::aad_key_transport(
        &parse_uuid(space_id, "space id")?,
        key_epoch,
        &to_array(recipient_ed25519, "recipient")?,
    ))
}

/// STREAM nonce of chunk `counter`.
pub fn chunk_nonce(nonce_prefix: &[u8], counter: u32, last: bool) -> Result<Vec<u8>> {
    Ok(stream::chunk_nonce(&to_array(nonce_prefix, "prefix")?, counter, last).to_vec())
}

/// Payload padding (spec §5.3).
pub fn pad_payload(json: &[u8]) -> Vec<u8> {
    envelope::pad_payload(json)
}

/// Seals given JSON bytes with explicit CEK and nonces (format 1).
pub fn seal_envelope(
    space_key: &SpaceKey,
    item_id: &str,
    cek: &[u8],
    wrap_nonce: &[u8],
    payload_nonce: &[u8],
    json: &[u8],
) -> Result<Vec<u8>> {
    envelope::seal_raw(
        space_key,
        &parse_uuid(item_id, "item id")?,
        &to_array::<KEY_LEN>(cek, "cek")?,
        &to_array::<NONCE_LEN>(wrap_nonce, "wrap nonce")?,
        &to_array::<NONCE_LEN>(payload_nonce, "payload nonce")?,
        json,
    )
}

/// Parses a payload JSON as a reader would after decryption.
pub fn parse_payload(json: &[u8], expected_item_id: &str) -> Result<Item> {
    Item::from_json(json, expected_item_id)
}

/// Chunk encryptor with an explicit file key and nonce prefix.
pub fn chunk_encryptor(
    item_id: &str,
    attachment_index: u32,
    file_key: &[u8],
    nonce_prefix: &[u8],
) -> Result<ChunkEncryptor> {
    ChunkEncryptor::with_params(
        item_id,
        attachment_index,
        Zeroizing::new(to_array::<KEY_LEN>(file_key, "file key")?),
        to_array::<NONCE_PREFIX_LEN>(nonce_prefix, "nonce prefix")?,
    )
}

/// Encrypts one chunk with an arbitrary counter / flag (to build negative vectors).
pub fn encrypt_chunk(
    file_key: &[u8],
    nonce_prefix: &[u8],
    item_id: &str,
    attachment_index: u32,
    counter: u32,
    last: bool,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    stream::encrypt_chunk_raw(
        &to_array(file_key, "file key")?,
        &to_array(nonce_prefix, "prefix")?,
        &stream::aad_chunk(
            crate::FORMAT_MAX,
            &parse_uuid(item_id, "item id")?,
            attachment_index,
        ),
        counter,
        last,
        plaintext,
    )
}

/// Seals a space key with an explicit ephemeral secret and nonce.
pub fn seal_space_key(
    recipient: &DevicePublic,
    space_id: &str,
    space_key: &SpaceKey,
    ephemeral_secret: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>> {
    device::seal_space_key_raw(
        recipient,
        &parse_uuid(space_id, "space id")?,
        space_key,
        &to_array(ephemeral_secret, "ephemeral secret")?,
        &to_array(nonce, "nonce")?,
    )
}

/// Rotation with explicit new key and per-recipient `(ephemeral_secret, nonce)`, in
/// ascending order of recipient Ed25519 key.
pub fn create_rotation(
    store: &TrustStore,
    issuer: &DeviceKeys,
    revoke: &[Vec<u8>],
    issued_at_ms: u64,
    new_space_key: SpaceKey,
    randomness: &[(Vec<u8>, Vec<u8>)],
) -> Result<RotationBundle> {
    let r = randomness
        .iter()
        .map(|(e, n)| {
            Ok((
                Zeroizing::new(to_array::<32>(e, "ephemeral")?),
                to_array::<NONCE_LEN>(n, "nonce")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    store.create_rotation_raw(issuer, revoke, issued_at_ms, new_space_key, &r)
}

/// `AAD` of a sealed history key (spec §7.4).
pub fn aad_history_transport(
    space_id: &str,
    transfer_id: &str,
    recipient_ed25519: &[u8],
) -> Result<Vec<u8>> {
    Ok(history::aad_history_transport(
        &parse_uuid(space_id, "space id")?,
        &parse_uuid(transfer_id, "transfer id")?,
        &to_array(recipient_ed25519, "recipient")?,
    ))
}

/// Transport key of a sealed history key (spec §7.4).
pub fn history_transport_key(
    shared: &[u8],
    ephemeral_public: &[u8],
    recipient_x25519: &[u8],
) -> Result<Vec<u8>> {
    Ok(history::history_transport_key(
        &to_array(shared, "shared")?,
        &to_array(ephemeral_public, "ephemeral")?,
        &to_array(recipient_x25519, "recipient")?,
    )?
    .to_vec())
}

/// `AAD_hist` of a history index part (spec §8.7.2).
pub fn aad_history_chunk(
    format: u32,
    transfer_id: &str,
    part_index: u32,
    n_parts: u32,
) -> Result<Vec<u8>> {
    Ok(history::aad_history_chunk(
        format,
        &parse_uuid(transfer_id, "transfer id")?,
        part_index,
        n_parts,
    ))
}

/// History manifest with an explicit ephemeral secret and nonce for the sealed key.
#[allow(clippy::too_many_arguments)]
pub fn create_history_transfer(
    store: &TrustStore,
    issuer: &DeviceKeys,
    recipient_ed25519: &[u8],
    transfer_id: &str,
    as_of_seq: u64,
    created_at_ms: u64,
    history_key: &HistoryKey,
    parts: &[HistoryPart],
    ephemeral_secret: &[u8],
    nonce: &[u8],
) -> Result<HistoryTransfer> {
    store.create_history_transfer_raw(
        issuer,
        recipient_ed25519,
        transfer_id,
        as_of_seq,
        created_at_ms,
        history_key,
        parts,
        &to_array(ephemeral_secret, "ephemeral secret")?,
        &to_array(nonce, "nonce")?,
    )
}

/// History part encryptor with an explicit nonce prefix.
pub fn history_part_encryptor(
    history_key: &HistoryKey,
    transfer_id: &str,
    part_index: u32,
    n_parts: u32,
    nonce_prefix: &[u8],
) -> Result<HistoryPartEncryptor> {
    HistoryPartEncryptor::with_prefix(
        history_key,
        transfer_id,
        part_index,
        n_parts,
        to_array(nonce_prefix, "nonce prefix")?,
    )
}

/// `K_state` derived from a space key (spec §8.8.2).
pub fn item_state_key(space_key: &SpaceKey) -> Result<Vec<u8>> {
    Ok(space_key.item_state_key()?.to_vec())
}

/// `AAD_state` of an item-state envelope (spec §8.8.2).
pub fn aad_item_state(format: u32, key_epoch: u32, event_id: &str) -> Result<Vec<u8>> {
    Ok(pin::aad_item_state(
        format,
        key_epoch,
        &parse_uuid(event_id, "event id")?,
    ))
}

/// Seals arbitrary inner bytes (a pin statement, or anything for negative vectors) as
/// an item-state envelope with an explicit nonce.
pub fn seal_item_state(
    inner: &[u8],
    event_id: &str,
    space_key: &SpaceKey,
    nonce: &[u8],
) -> Result<Vec<u8>> {
    pin::seal_item_state_raw(
        inner,
        &parse_uuid(event_id, "event id")?,
        space_key,
        &to_array::<NONCE_LEN>(nonce, "nonce")?,
    )
}

/// Normalised JSON view of an [`Item`] used by the vectors' `expected_item` field
/// (binary fields in lowercase hex, absent optionals as `null`).
pub fn item_to_value(item: &Item) -> serde_json::Value {
    use crate::item::{ReadPolicy, SourceFormat};
    use serde_json::json;
    let rp = match item.read_policy {
        ReadPolicy::UntilExpiry => "until_expiry",
        ReadPolicy::AfterFirstRead => "after_first_read",
    };
    let text = item.text.as_ref().map(|t| {
        json!({
            "plain": t.plain,
            "html": t.html,
            "markdown": t.markdown,
            "source_format": match t.source_format {
                SourceFormat::Plain => "plain",
                SourceFormat::Markdown => "markdown",
                SourceFormat::Html => "html",
            },
        })
    });
    let attachments: Vec<serde_json::Value> = item
        .attachments
        .iter()
        .map(|a| {
            json!({
                "blob_id": a.blob_id,
                "name": a.name,
                "mime": a.mime,
                "size": a.size,
                "file_key": hex::encode(&a.file_key),
                "nonce_prefix": hex::encode(&a.nonce_prefix),
                "thumbnail": a.thumbnail.as_ref().map(|t| json!({
                    "mime": t.mime,
                    "data": hex::encode(&t.data),
                })),
            })
        })
        .collect();
    json!({
        "id": item.id,
        "source_device": item.source_device,
        "created_at_ms": item.created_at_ms,
        "expires_at_ms": item.expires_at_ms,
        "read_policy": rp,
        "secret": item.secret,
        "title": item.title,
        "text": text,
        "url": item.url,
        "attachments": attachments,
    })
}
