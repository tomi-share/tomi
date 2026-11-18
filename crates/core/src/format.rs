//! Protocol version numbers and fixed parameters of format v1 (spec §3, §10).

/// Highest item `format` version this build can read and write.
pub const FORMAT_MAX: u32 = 1;
/// Lowest item `format` version this build can still read.
pub const FORMAT_MIN: u32 = 1;
/// REST/WebSocket API version spoken by this build (`/v1/...`).
pub const API_VERSION: u32 = 1;

/// Highest `key_epoch` of a space key: 2^31 − 1. The values `0x8000_0000..=0xFFFF_FFFF`
/// are **reserved** for future key identifiers that are not space key epochs (group or
/// recipient keys, spec §5.8.2). A v1 device never creates, signs, accepts or stores a
/// space key there; a received item or item state announcing such a value stays
/// `KeyPending` (it is never opened with a space key, never deleted).
pub const SPACE_KEY_EPOCH_MAX: u32 = 0x7fff_ffff;

/// Plaintext size of every attachment chunk except the last one (64 KiB).
pub const CHUNK_SIZE: usize = 65_536;
/// Poly1305 tag length appended to every AEAD ciphertext.
pub const TAG_LEN: usize = 16;
/// Ciphertext size of a full (non-last) attachment chunk.
pub const CHUNK_CIPHERTEXT_SIZE: usize = CHUNK_SIZE + TAG_LEN;
/// The padded payload plaintext length is a multiple of this block (spec §5.3).
pub const PAYLOAD_PAD_BLOCK: usize = 256;
/// Maximum padded payload plaintext length (1 MiB), checked on write and read.
pub const MAX_PAYLOAD_LEN: usize = 1_048_576;

/// `true` if this build can open an envelope of the given `format`.
pub fn is_format_supported(format: u32) -> bool {
    (FORMAT_MIN..=FORMAT_MAX).contains(&format)
}

/// Effective format of the space (PROJECT.md §6): the smallest `format_max` announced
/// by the active devices. Returns `None` if no device is active.
pub fn effective_format(active_format_max: &[u32]) -> Option<u32> {
    active_format_max.iter().copied().min()
}

/// Size in bytes of the encrypted blob of an attachment of `size` plaintext bytes
/// (spec §6.4). Lets the relay or a client check a blob length before downloading it.
pub fn encrypted_attachment_len(size: u64) -> u64 {
    let chunk = CHUNK_SIZE as u64;
    let chunks = if size == 0 { 1 } else { size.div_ceil(chunk) };
    size + chunks * TAG_LEN as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_range_is_coherent() {
        const { assert!(FORMAT_MIN <= FORMAT_MAX) };
        assert!(is_format_supported(1));
        assert!(!is_format_supported(0));
        assert!(!is_format_supported(FORMAT_MAX + 1));
    }

    #[test]
    fn effective_format_is_min() {
        assert_eq!(effective_format(&[3, 1, 2]), Some(1));
        assert_eq!(effective_format(&[]), None);
    }

    #[test]
    fn blob_len() {
        assert_eq!(encrypted_attachment_len(0), 16);
        assert_eq!(encrypted_attachment_len(65_536), 65_552);
        assert_eq!(encrypted_attachment_len(65_537), 65_537 + 32);
    }
}
