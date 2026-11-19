//! Tomi core.
//!
//! Single implementation of the crypto, the item envelope format, sync and local
//! storage, shared by the Windows client, the CLI and (via UniFFI) the Android client.
//! The normative specification is `spec/format-v1.md`; the frozen test vectors in
//! `spec/vectors/v1/` are checked by `tests/spec_vectors.rs`.
//!
//! Public API overview:
//! * space key: [`SpaceKey`], [`new_space_id`];
//! * items: [`Item`], [`seal_item`], [`read_envelope_header`], [`open_item`];
//! * attachments: [`ChunkEncryptor`], [`ChunkDecryptor`];
//! * devices and trust: [`DeviceKeys`], [`DevicePublic`], [`TrustStore`],
//!   [`DeviceCert`], [`MemberList`], [`KeyRotation`], [`seal_space_key`];
//! * deletion: [`Tombstone`], [`DeletionScope`];
//! * received items and old keys: [`SpaceKeyRing`], [`ReceivedItem`],
//!   [`UnreadableReason`], [`relay_expires_at_ms`];
//! * local storage: [`LocalKey`];
//! * pinned items (shared, encrypted, last-writer-wins): [`PinBook`], [`PinState`],
//!   [`PinStatement`], [`seal_item_state`], [`open_item_state`], [`local_retention`];
//! * archived items (same mechanism, own statement kind): [`ArchiveBook`], [`ArchiveState`],
//!   [`ArchiveStatement`];
//! * history transfer: [`HistoryKey`], [`HistoryTransfer`], [`HistoryPartEncryptor`],
//!   [`HistoryPartDecryptor`], [`HistoryIndexWriter`], [`HistoryIndexReader`];
//! * relay authentication: [`DeviceKeys::sign_relay_auth`], [`verify_relay_auth`];
//! * pairing a new device (code, QR, CPace session, batch), pure part without network:
//!   [`pairing`] (`spec/pairing-v1.md`).
//!
//! Cargo features (all on by default; the relay should use `default-features = false`):
//! * `runtime`: SQLite store, sync engine, outbox, history transfer orchestration
//!   (`runtime` module, tested against an in-memory relay double);
//! * `http`: real HTTPS / WebSocket transport (`runtime::http`);
//! * `ffi`: UniFFI bindings (`RustTomiCore`), see `src/ffi.rs` for the generation steps.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod archive;
mod attachment_tombstone;
mod crypto;
mod device;
mod envelope;
mod error;
mod format;
mod history;
mod item;
mod keyring;
mod local;
pub mod pairing;
mod pin;
mod relay_auth;
mod relay_url;
mod stream;
mod tombstone;

#[doc(hidden)]
pub mod kat;

#[cfg(feature = "ffi")]
mod ffi;
#[cfg(feature = "runtime")]
pub mod runtime;
#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub use archive::{
    ArchiveBook, ArchiveState, ArchiveStatement, ARCHIVE_AT_MS_MAX, ARCHIVE_STATEMENT_LEN,
};
pub use attachment_tombstone::{
    AttachmentTombstone, ATTACHMENT_TOMBSTONE_AT_MS_MAX, ATTACHMENT_TOMBSTONE_LEN,
};
pub use crypto::{
    labels, new_blob_id, new_item_id, new_space_id, SpaceKey, KEY_LEN, NONCE_LEN, NONCE_PREFIX_LEN,
};
pub use device::{
    device_id_from_ed25519, seal_space_key, DeviceCert, DeviceKeys, DevicePublic, KeyRotation,
    MemberList, RotationBundle, RotationOutcome, RotationRecipient, TrustStore,
    MAX_STATEMENT_ENTRIES, PUBLIC_KEY_LEN, SEALED_SPACE_KEY_LEN, SIGNATURE_LEN,
};
pub use envelope::{
    open_item, read_envelope_header, seal_item, EnvelopeHeader, OpenedItem, ENVELOPE_HEADER_LEN,
};
pub use error::{CoreError, Result};
pub use format::{
    effective_format, encrypted_attachment_len, is_format_supported, API_VERSION,
    CHUNK_CIPHERTEXT_SIZE, CHUNK_SIZE, FORMAT_MAX, FORMAT_MIN, MAX_PAYLOAD_LEN, PAYLOAD_PAD_BLOCK,
    SPACE_KEY_EPOCH_MAX, TAG_LEN,
};
pub use history::{
    history_part_capacity, history_part_count, history_transfer_len, HistoryDevice, HistoryEntry,
    HistoryHeader, HistoryIndexReader, HistoryIndexWriter, HistoryItem, HistoryKey, HistoryPart,
    HistoryPartDecryptor, HistoryPartEncryptor, HistorySummary, HistoryTransfer, TransferredFile,
    HISTORY_INDEX_VERSION, HISTORY_PART_ENTRY_LEN, HISTORY_SEALED_KEY_LEN,
    HISTORY_TRANSFER_BASE_LEN, MAX_HISTORY_LINE_BYTES, MAX_HISTORY_PARTS,
};
pub use item::{Attachment, Item, ItemText, RawPayload, ReadPolicy, SourceFormat, Thumbnail};
pub use keyring::{
    item_max_age_ms, relay_expires_at_ms, space_key_retention_ms, tombstone_retention_ms,
    KeyRingRecord, ReceivedItem, SpaceKeyRing, Supersession, UnreadableReason,
    DEFAULT_MAX_TTL_SECS, RELAY_EXPIRY_MARGIN_MS, SPACE_KEY_GRACE_MS,
};
pub use local::{LocalKey, LOCAL_RECORD_OVERHEAD, LOCAL_RECORD_VERSION};
pub use pin::{
    local_retention, open_item_state, read_item_state_header, seal_item_state, ItemStateHeader,
    ItemStateKind, ItemStateWire, LocalRetention, OpenedItemState, OutgoingItemState, PinBook,
    PinState, PinStatement, ReceivedItemState, StateStatement, ITEM_STATE_ENVELOPE_LEN,
    ITEM_STATE_HEADER_LEN, MAX_ITEM_STATE_PADDED_LEN, PIN_AT_MS_MAX, PIN_STATEMENT_LEN,
};
pub use relay_auth::{
    relay_auth_message, verify_relay_auth, RELAY_AUTH_MESSAGE_LEN, RELAY_CHALLENGE_LEN,
};
pub use relay_url::{
    is_install_token_shape, parse_pair_link, parse_setup_link, PairLink, SetupLink,
};
pub use stream::{ChunkDecryptor, ChunkEncryptor, FinalizedAttachment};
pub use tombstone::{DeletionScope, Tombstone, TOMBSTONE_LEN};

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item() -> Item {
        Item {
            id: new_item_id().unwrap(),
            source_device: "0123456789abcdef0123456789abcdef".into(),
            created_at_ms: 1_793_836_800_000,
            expires_at_ms: Some(1_793_840_400_000),
            read_policy: ReadPolicy::AfterFirstRead,
            secret: true,
            title: None,
            text: Some(ItemText {
                plain: "hunter2".into(),
                html: None,
                markdown: None,
                source_format: SourceFormat::Plain,
            }),
            url: None,
            attachments: vec![],
            raw_payload: None,
        }
    }

    #[test]
    fn seal_open_roundtrip() {
        let key = SpaceKey::generate(3).unwrap();
        let item = sample_item();
        let env = seal_item(&item, &key).unwrap();
        let header = read_envelope_header(&env).unwrap();
        assert_eq!(header.key_epoch, 3);
        assert_eq!(header.item_id, item.id);
        let opened = open_item(&env, &item.id, &key).unwrap();
        assert_eq!(opened.item, item);
        assert_eq!(opened.format, FORMAT_MAX);
    }

    #[test]
    fn unknown_fields_survive_a_new_envelope() {
        // Re-sealing a received item under a space key (e.g. after `stale_key_epoch`)
        // carries its original payload too.
        let key = SpaceKey::generate(2).unwrap();
        let id = new_item_id().unwrap();
        let payload = item::testing::payload_with_unknown_fields(&id);
        let received = open_item(
            &kat::seal_envelope(
                &key,
                &id,
                &[5u8; KEY_LEN],
                &[6u8; NONCE_LEN],
                &[7u8; NONCE_LEN],
                &payload,
            )
            .unwrap(),
            &id,
            &key,
        )
        .unwrap()
        .item;
        let again = open_item(&seal_item(&received, &key).unwrap(), &id, &key)
            .unwrap()
            .item;
        assert_eq!(again.raw_payload.unwrap().as_bytes(), &payload[..]);
    }

    #[test]
    fn wrong_key_or_id_fails() {
        let key = SpaceKey::generate(1).unwrap();
        let other = SpaceKey::generate(1).unwrap();
        let item = sample_item();
        let env = seal_item(&item, &key).unwrap();
        assert_eq!(
            open_item(&env, &item.id, &other).unwrap_err(),
            CoreError::DecryptionFailed
        );
        assert_eq!(
            open_item(&env, &new_item_id().unwrap(), &key).unwrap_err(),
            CoreError::ItemIdMismatch
        );
    }
}
