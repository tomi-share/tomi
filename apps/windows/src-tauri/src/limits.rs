//! Size limits: 25 Mo per file, 100 Mo per item.
//! "Mo" is computed in MiB (1 Mo = 1 048 576 octets).

use serde::Serialize;

pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_ITEM_BYTES: u64 = 100 * 1024 * 1024;
/// The relay refuses more than 32 blobs in one item (`max_blobs_per_item`, spec/api-v1.md
/// section 2.8); the core checks it again against what the relay announces.
pub const MAX_FILES_PER_ITEM: usize = 32;

/// Maximum *padded* payload plaintext of one item: everything that is not a file body
/// (title, url, text in all its flavours, attachment metadata and thumbnails) shares this
/// budget. Mirrors `MAX_PAYLOAD_LEN` of `crates/core`
/// (`format.rs`, spec/format-v1.md section 5.3). Not defined here: only aligned.
pub const MAX_PAYLOAD_BYTES: usize = 1_048_576;
/// The payload is padded to a multiple of this block, with at least one marker byte
/// (`PAYLOAD_PAD_BLOCK` in `crates/core`).
pub const PAYLOAD_PAD_BLOCK: usize = 256;

/// Longest retention the relay accepts: `max_ttl_secs` = 2 592 000 s = 30 days
/// (spec/api-v1.md section 2.8). A fixed duration chosen by the user never exceeds it.
pub const MAX_TTL_MS: i64 = 2_592_000 * 1000;

/// True when a serialized payload of `json_len` bytes still fits once padded.
pub fn payload_fits(json_len: usize) -> bool {
    (json_len / PAYLOAD_PAD_BLOCK + 1) * PAYLOAD_PAD_BLOCK <= MAX_PAYLOAD_BYTES
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCandidate {
    pub name: String,
    pub size: u64,
    pub is_file: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RejectReason {
    /// Larger than 25 Mo.
    TooLarge,
    /// Accepting it would push the item above 100 Mo.
    ItemTotalExceeded,
    /// Folders and other non-files.
    NotAFile,
    /// More than the allowed number of files in one item.
    TooMany,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rejection {
    pub name: String,
    pub size: u64,
    pub reason: RejectReason,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FileCheck {
    /// Indices (into the input slice) of the accepted candidates, in order.
    pub accepted: Vec<usize>,
    pub rejected: Vec<Rejection>,
}

/// Checks candidates one by one: a refused file does not affect the others.
/// `existing_total` / `existing_count` are what the draft already contains.
pub fn check_files(
    existing_total: u64,
    existing_count: usize,
    candidates: &[FileCandidate],
) -> FileCheck {
    let mut out = FileCheck::default();
    let mut total = existing_total;
    let mut count = existing_count;
    for (i, c) in candidates.iter().enumerate() {
        let reason = if !c.is_file {
            Some(RejectReason::NotAFile)
        } else if c.size > MAX_FILE_BYTES {
            Some(RejectReason::TooLarge)
        } else if count >= MAX_FILES_PER_ITEM {
            Some(RejectReason::TooMany)
        } else if total + c.size > MAX_ITEM_BYTES {
            Some(RejectReason::ItemTotalExceeded)
        } else {
            None
        };
        match reason {
            Some(reason) => out.rejected.push(Rejection {
                name: c.name.clone(),
                size: c.size,
                reason,
            }),
            None => {
                total += c.size;
                count += 1;
                out.accepted.push(i);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn file(name: &str, size: u64) -> FileCandidate {
        FileCandidate {
            name: name.into(),
            size,
            is_file: true,
        }
    }

    #[test]
    fn exactly_25_mo_is_accepted_and_one_byte_more_is_refused() {
        let r = check_files(0, 0, &[file("a", 25 * MB), file("b", 25 * MB + 1)]);
        assert_eq!(r.accepted, vec![0]);
        assert_eq!(r.rejected.len(), 1);
        assert_eq!(r.rejected[0].name, "b");
        assert_eq!(r.rejected[0].reason, RejectReason::TooLarge);
    }

    #[test]
    fn oversized_file_does_not_affect_the_others() {
        let r = check_files(
            0,
            0,
            &[file("a", MB), file("big", 31 * MB), file("c", 2 * MB)],
        );
        assert_eq!(r.accepted, vec![0, 2]);
        assert_eq!(r.rejected.len(), 1);
        assert_eq!(r.rejected[0].size, 31 * MB);
    }

    #[test]
    fn item_total_is_capped_at_100_mo() {
        let files: Vec<_> = (0..5).map(|i| file(&format!("f{i}"), 25 * MB)).collect();
        let r = check_files(0, 0, &files);
        assert_eq!(r.accepted, vec![0, 1, 2, 3]);
        assert_eq!(r.rejected[0].reason, RejectReason::ItemTotalExceeded);
    }

    #[test]
    fn existing_draft_content_counts_towards_the_item_total() {
        let r = check_files(90 * MB, 3, &[file("a", 11 * MB), file("b", 10 * MB)]);
        assert_eq!(r.accepted, vec![1]);
        assert_eq!(r.rejected[0].reason, RejectReason::ItemTotalExceeded);
    }

    #[test]
    fn folders_are_refused() {
        let r = check_files(
            0,
            0,
            &[FileCandidate {
                name: "dir".into(),
                size: 0,
                is_file: false,
            }],
        );
        assert!(r.accepted.is_empty());
        assert_eq!(r.rejected[0].reason, RejectReason::NotAFile);
    }

    #[test]
    fn payload_budget_matches_the_padded_limit() {
        // 1 MiB of JSON would pad to 1 MiB + 256: refused. One byte less fits exactly.
        assert!(payload_fits(MAX_PAYLOAD_BYTES - 1));
        assert!(!payload_fits(MAX_PAYLOAD_BYTES));
        assert!(payload_fits(0));
    }

    #[test]
    fn max_ttl_is_thirty_days() {
        assert_eq!(MAX_TTL_MS, 30 * 24 * 3_600 * 1000);
    }

    #[test]
    fn file_count_is_bounded() {
        let r = check_files(0, MAX_FILES_PER_ITEM, &[file("a", 1)]);
        assert_eq!(r.rejected[0].reason, RejectReason::TooMany);
    }
}
