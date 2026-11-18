//! Error type of the crate.
//!
//! Flat enum with simple payloads so that it maps 1:1 to a UniFFI error later.
//! Messages never contain plaintext, keys or other secret material.

/// Every fallible operation of `tomi-core` returns this error.
///
/// Rule for clients (spec §9.3): whatever the variant, an item that fails to open is
/// **kept** and displayed as unreadable ("illisible"); it is never deleted or skipped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// The envelope announces a `format` this build cannot read.
    #[error("unsupported format {format} (this build reads {min}..={max})")]
    UnsupportedFormat { format: u32, min: u32, max: u32 },
    /// The binary envelope is structurally invalid (length, padding...).
    #[error("malformed envelope: {reason}")]
    MalformedEnvelope { reason: String },
    /// The decrypted payload is not a valid v1 item (bad JSON, missing field...).
    #[error("malformed payload: {reason}")]
    MalformedPayload { reason: String },
    /// A signed statement (device certificate, rotation, tombstone...) is malformed.
    #[error("malformed statement: {reason}")]
    MalformedStatement { reason: String },
    /// AEAD authentication failed: wrong key, wrong AAD, or tampered ciphertext.
    #[error("decryption failed")]
    DecryptionFailed,
    /// The envelope was sealed under another space key epoch than the key provided.
    #[error("key epoch mismatch: envelope {envelope}, key {key}")]
    KeyEpochMismatch { envelope: u32, key: u32 },
    /// The item id inside the envelope or payload differs from the expected one.
    #[error("item id mismatch")]
    ItemIdMismatch,
    /// A caller-provided argument is invalid (length, encoding...).
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    /// An Ed25519 signature does not verify.
    #[error("invalid signature")]
    InvalidSignature,
    /// The statement belongs to another space.
    #[error("space mismatch")]
    SpaceMismatch,
    /// The signer is not a trusted member of the space.
    #[error("untrusted signer")]
    UntrustedSigner,
    /// The device concerned has been revoked.
    #[error("revoked device")]
    RevokedDevice,
    /// A known device is presented with a different X25519 key.
    #[error("conflicting key for a known device")]
    KeyConflict,
    /// A key rotation does not follow the current epoch.
    #[error("key epoch out of order: expected {expected}, got {got}")]
    EpochOutOfOrder { expected: u32, got: u32 },
    /// This device is neither a recipient nor revoked in a key rotation.
    #[error("this device is not a recipient of the key rotation")]
    NotARecipient,
    /// The attachment stream ended before its last chunk.
    #[error("attachment stream truncated")]
    StreamTruncated,
    /// Data was supplied after the last chunk of an attachment stream.
    #[error("attachment stream has trailing data")]
    StreamTrailingData,
    /// The attachment stream object was already finalized.
    #[error("attachment stream already finalized")]
    StreamFinalized,
    /// A history transfer index ended without its final `end` line.
    #[error("history transfer index is incomplete")]
    HistoryIncomplete,
    /// The event id inside an item-state envelope differs from the announced one.
    #[error("event id mismatch")]
    EventIdMismatch,
    /// The operating system random generator failed.
    #[error("random generator unavailable")]
    Randomness,
    /// Unexpected internal state (e.g. poisoned lock).
    #[error("internal error: {reason}")]
    Internal { reason: String },
    /// The local database or the local files failed (runtime layer).
    #[error("storage error: {reason}")]
    Storage { reason: String },
    /// The relay could not be reached or answered garbage (runtime layer). Retry later.
    #[error("network error: {reason}")]
    Network { reason: String },
    /// The relay refused a request with a stable API code (`api-v1.md` §2.5).
    #[error("relay error {status}: {code}")]
    Relay { status: u16, code: String },
    /// The device cannot talk to the relay until the user acts (revoked, app too old...).
    #[error("device blocked: {reason}")]
    Blocked { reason: String },
    /// The pairing session failed: wrong code, unexpected or malformed frame, refused batch
    /// (`pairing-v1.md` §11.1, §11.5). Deliberately without detail: the causes of a wrong
    /// code must stay indistinguishable for the user and in the logs.
    #[error("pairing failed")]
    PairingFailed,
    /// The pairing code or its channel expired (`pairing-v1.md` §11.3).
    #[error("pairing expired")]
    PairingExpired,
    /// Another device already claimed this pairing code.
    #[error("pairing code already used")]
    PairingTaken,
    /// The other device, or the user, cancelled the pairing.
    #[error("pairing cancelled")]
    PairingCancelled,
    /// The other device speaks a pairing version, or uses a critical field, this build does
    /// not know (`pairing-v1.md` §6.2, §7.4).
    #[error("pairing unsupported: update Tomi on both devices")]
    PairingUnsupported,
    /// No relay is configured: the app was built without a default relay and the user has not
    /// given one yet (a link, a QR code or the "Relais" field). Nothing was sent.
    #[error("no relay configured")]
    RelayNotConfigured,
}

impl CoreError {
    /// Stable machine-readable code, used by the test vectors (`expected_error`)
    /// and usable by clients for localisation.
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::UnsupportedFormat { .. } => "unsupported_format",
            CoreError::MalformedEnvelope { .. } => "malformed_envelope",
            CoreError::MalformedPayload { .. } => "malformed_payload",
            CoreError::MalformedStatement { .. } => "malformed_statement",
            CoreError::DecryptionFailed => "decryption_failed",
            CoreError::KeyEpochMismatch { .. } => "key_epoch_mismatch",
            CoreError::ItemIdMismatch => "item_id_mismatch",
            CoreError::InvalidInput { .. } => "invalid_input",
            CoreError::InvalidSignature => "invalid_signature",
            CoreError::SpaceMismatch => "space_mismatch",
            CoreError::UntrustedSigner => "untrusted_signer",
            CoreError::RevokedDevice => "revoked_device",
            CoreError::KeyConflict => "key_conflict",
            CoreError::EpochOutOfOrder { .. } => "epoch_out_of_order",
            CoreError::NotARecipient => "not_a_recipient",
            CoreError::StreamTruncated => "stream_truncated",
            CoreError::StreamTrailingData => "stream_trailing_data",
            CoreError::StreamFinalized => "stream_finalized",
            CoreError::HistoryIncomplete => "history_incomplete",
            CoreError::EventIdMismatch => "event_id_mismatch",
            CoreError::Randomness => "randomness",
            CoreError::Internal { .. } => "internal",
            CoreError::Storage { .. } => "storage",
            CoreError::Network { .. } => "network",
            CoreError::Relay { .. } => "relay",
            CoreError::Blocked { .. } => "blocked",
            CoreError::PairingFailed => "pairing_failed",
            CoreError::PairingExpired => "pairing_expired",
            CoreError::PairingTaken => "pairing_taken",
            CoreError::PairingCancelled => "pairing_cancelled",
            CoreError::PairingUnsupported => "pairing_unsupported",
            CoreError::RelayNotConfigured => "relay_not_configured",
        }
    }

    pub(crate) fn input(reason: impl Into<String>) -> Self {
        CoreError::InvalidInput {
            reason: reason.into(),
        }
    }

    pub(crate) fn envelope(reason: impl Into<String>) -> Self {
        CoreError::MalformedEnvelope {
            reason: reason.into(),
        }
    }

    pub(crate) fn payload(reason: impl Into<String>) -> Self {
        CoreError::MalformedPayload {
            reason: reason.into(),
        }
    }

    pub(crate) fn statement(reason: impl Into<String>) -> Self {
        CoreError::MalformedStatement {
            reason: reason.into(),
        }
    }
}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, CoreError>;
