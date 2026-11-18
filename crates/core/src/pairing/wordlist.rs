//! The 2048-word list of the pairing code (`spec/pairing-v1.md` Annex B).
//!
//! French BIP-39 list in its **folded form**: NFKD, combining marks (Mn) removed, lowercase,
//! so only `a`-`z` remain. The words have 5 to 8 letters and are pairwise distinct on their
//! first 4 letters. The rank of a word is its line number, counted from 0. The text is
//! embedded unchanged and a test checks its SHA-256.

use std::sync::OnceLock;

/// Number of words in the list (11 bits of secret per word).
pub const WORD_COUNT: usize = 2048;

/// Minimum number of letters that identify a word by its prefix (§4.4 rule 4).
pub(crate) const PREFIX_MIN_LEN: usize = 4;

/// The embedded folded list: 2048 lines, each ended by `\n`, 16 001 bytes (Annex B).
pub(crate) const WORDLIST_TXT: &str = include_str!("wordlist_fr.txt");

/// SHA-256 of [`WORDLIST_TXT`], from Annex B (checked by the tests).
#[cfg(test)]
pub(crate) const WORDLIST_SHA256: &str =
    "cf8db448ad5ca0b68c3c8f88b51458f20362a5939fef236fac64a303f1485e87";

fn words() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| WORDLIST_TXT.lines().collect())
}

/// The word of a rank (`0..2048`), in its folded form (lowercase ASCII, no accent).
pub fn word(rank: u16) -> Option<&'static str> {
    words().get(usize::from(rank)).copied()
}

/// The rank of a folded word, if it is in the list.
pub fn rank_of(word: &str) -> Option<u16> {
    // The folded list is sorted bytewise: a test checks it.
    words()
        .binary_search(&word)
        .ok()
        .and_then(|i| u16::try_from(i).ok())
}

/// The rank of the only word that starts with `prefix`, if there is exactly one and the prefix
/// has at least [`PREFIX_MIN_LEN`] letters.
pub(crate) fn rank_of_unique_prefix(prefix: &str) -> Option<u16> {
    if prefix.len() < PREFIX_MIN_LEN {
        return None;
    }
    let mut found = None;
    for (i, w) in words().iter().enumerate() {
        if w.starts_with(prefix) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found.and_then(|i| u16::try_from(i).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sha256;

    #[test]
    fn embedded_list_has_the_annexe_b_digest() {
        assert_eq!(WORDLIST_TXT.len(), 16_001);
        assert_eq!(
            hex::encode(sha256(&[WORDLIST_TXT.as_bytes()])),
            WORDLIST_SHA256
        );
        assert!(WORDLIST_TXT.ends_with('\n'));
        assert!(!WORDLIST_TXT.contains('\r'));
    }

    #[test]
    fn p03_words_are_distinct_folded_and_distinct_on_four_letters() {
        let all = words();
        assert_eq!(all.len(), WORD_COUNT);
        let mut prefixes = std::collections::BTreeSet::new();
        let mut distinct = std::collections::BTreeSet::new();
        for w in all {
            assert!((5..=8).contains(&w.len()), "{w}");
            assert!(w.bytes().all(|b| b.is_ascii_lowercase()), "{w}");
            assert!(distinct.insert(*w), "duplicate {w}");
            assert!(
                prefixes.insert(&w[..4]),
                "4-letter prefix of {w} not unique"
            );
        }
        // `rank_of` relies on the bytewise order.
        assert!(all.windows(2).all(|p| p[0] < p[1]));
    }

    #[test]
    fn known_ranks() {
        for (rank, w) in [
            (0u16, "abaisser"),
            (465, "crayon"),
            (616, "ecole"),
            (642, "eleve"),
            (1633, "renard"),
            (1891, "tomate"),
            (2047, "zoologie"),
        ] {
            assert_eq!(word(rank), Some(w));
            assert_eq!(rank_of(w), Some(rank));
        }
        assert_eq!(word(2048), None);
        assert_eq!(rank_of("lune"), None);
        assert_eq!(rank_of("école"), None, "only the folded form is listed");
    }

    #[test]
    fn unique_prefix() {
        assert_eq!(rank_of_unique_prefix("cray"), Some(465));
        assert_eq!(rank_of_unique_prefix("rena"), Some(1633));
        assert_eq!(rank_of_unique_prefix("crayon"), Some(465));
        assert_eq!(rank_of_unique_prefix("cra"), None, "below 4 letters");
        assert_eq!(rank_of_unique_prefix("zzzz"), None);
        assert_eq!(rank_of_unique_prefix(""), None);
    }
}
