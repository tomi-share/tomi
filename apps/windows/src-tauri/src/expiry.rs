//! Expiration choices and defaults.
//!
//! Owner decision: nothing expires by default, secrets included. The choices offered at
//! capture are "Jamais" (default for every kind of item), "Après lecture" and a
//! duration: 1 h, 1 day, 7 days, 30 days (no 5 minutes). "Après lecture" is only offered
//! for a hidden ("masqué") item: for anything else, opening the card marks the item read,
//! which would erase it as soon as it is seen. Same rule as Android (`ExpiryPolicy.kt`).
//!
//! `expires_at` here is the *user* expiration carried by the payload (local erasure on
//! the devices). `None` means "never". The relay retention (`max_ttl_secs`, 30 days,
//! `limits::MAX_TTL_MS`) is computed by the core when it uploads; durations offered here
//! never exceed it.

use serde::{Deserialize, Serialize};

use crate::limits::MAX_TTL_MS;
use crate::model::ReadPolicy;

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpiryChoice {
    Never,
    AfterFirstRead,
    Hour1,
    Day1,
    Day7,
    Day30,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpiryRules {
    pub options: Vec<ExpiryChoice>,
    pub default: ExpiryChoice,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllExpiryRules {
    pub normal: ExpiryRules,
    pub secret: ExpiryRules,
    /// A hidden item with at least one file: no "Après lecture".
    pub secret_with_files: ExpiryRules,
}

/// Default for every kind of item, secrets included.
pub fn default_choice() -> ExpiryChoice {
    ExpiryChoice::Never
}

/// The choices offered, in display order, for a hidden item or for any other.
///
/// "Après lecture" is offered for a hidden item WITHOUT files only (same rule as Android):
/// a read-once item with files would be erased everywhere by its first read, files included,
/// before the recipient could open or save them.
pub fn options_for(secret: bool, has_files: bool) -> Vec<ExpiryChoice> {
    use ExpiryChoice::*;
    let mut v = vec![Never];
    if secret && !has_files {
        v.push(AfterFirstRead);
    }
    v.extend([Hour1, Day1, Day7, Day30]);
    v
}

pub fn is_allowed(choice: ExpiryChoice, secret: bool, has_files: bool) -> bool {
    options_for(secret, has_files).contains(&choice)
}

fn rules(secret: bool, has_files: bool) -> ExpiryRules {
    ExpiryRules {
        options: options_for(secret, has_files),
        default: default_choice(),
    }
}

/// The front end keeps one entry per kind: the list is not the same for a hidden item, nor
/// for a hidden item that carries files.
pub fn all_rules() -> AllExpiryRules {
    AllExpiryRules {
        normal: rules(false, false),
        secret: rules(true, false),
        secret_with_files: rules(true, true),
    }
}

/// Resolves a choice into `(expires_at, read_policy)`.
///
/// - A choice that is not offered for this kind of item (the front end is not trusted)
///   falls back to "Jamais": no expiration, never a hidden stricter one.
/// - "Jamais" and "Après lecture" carry no date: "after first read" is a read policy,
///   not a deadline (the relay keeps an undelivered item at most 30 days anyway).
/// - A duration is capped at the relay maximum.
pub fn resolve(
    choice: ExpiryChoice,
    secret: bool,
    has_files: bool,
    now_ms: i64,
) -> (Option<i64>, ReadPolicy) {
    let choice = if is_allowed(choice, secret, has_files) {
        choice
    } else {
        default_choice()
    };
    let after = |ms: i64| Some(now_ms + ms.min(MAX_TTL_MS));
    match choice {
        ExpiryChoice::Never => (None, ReadPolicy::Normal),
        ExpiryChoice::AfterFirstRead => (None, ReadPolicy::AfterFirstRead),
        ExpiryChoice::Hour1 => (after(HOUR_MS), ReadPolicy::Normal),
        ExpiryChoice::Day1 => (after(DAY_MS), ReadPolicy::Normal),
        ExpiryChoice::Day7 => (after(7 * DAY_MS), ReadPolicy::Normal),
        ExpiryChoice::Day30 => (after(30 * DAY_MS), ReadPolicy::Normal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    #[test]
    fn nothing_expires_by_default_secrets_included() {
        assert_eq!(default_choice(), ExpiryChoice::Never);
        let rules = all_rules();
        assert_eq!(rules.normal.default, ExpiryChoice::Never);
        assert_eq!(rules.secret.default, ExpiryChoice::Never);
        for secret in [false, true] {
            assert_eq!(
                resolve(default_choice(), secret, false, NOW),
                (None, ReadPolicy::Normal),
                "secret = {secret}"
            );
        }
    }

    #[test]
    fn after_first_read_is_only_offered_for_a_hidden_item() {
        let rules = all_rules();
        assert!(!rules.normal.options.contains(&ExpiryChoice::AfterFirstRead));
        assert!(rules.secret.options.contains(&ExpiryChoice::AfterFirstRead));
    }

    #[test]
    fn after_first_read_is_not_offered_for_a_hidden_item_with_files() {
        let rules = all_rules();
        assert!(!rules
            .secret_with_files
            .options
            .contains(&ExpiryChoice::AfterFirstRead));
        assert_eq!(rules.secret_with_files.default, ExpiryChoice::Never);
        // The front end is not trusted: the choice is not honoured either.
        assert_eq!(
            resolve(ExpiryChoice::AfterFirstRead, true, true, NOW),
            (None, ReadPolicy::Normal)
        );
        assert_eq!(
            resolve(ExpiryChoice::Hour1, true, true, NOW),
            (Some(NOW + HOUR_MS), ReadPolicy::Normal)
        );
    }

    #[test]
    fn never_is_offered_first_for_every_kind() {
        let rules = all_rules();
        for o in [&rules.normal.options, &rules.secret.options] {
            assert_eq!(o.first(), Some(&ExpiryChoice::Never));
        }
    }

    #[test]
    fn after_first_read_is_an_explicit_option_never_the_default() {
        let rules = all_rules();
        assert_ne!(rules.normal.default, ExpiryChoice::AfterFirstRead);
        assert_ne!(rules.secret.default, ExpiryChoice::AfterFirstRead);
    }

    #[test]
    fn after_first_read_sets_the_policy_without_an_arbitrary_deadline() {
        assert_eq!(
            resolve(ExpiryChoice::AfterFirstRead, true, false, NOW),
            (None, ReadPolicy::AfterFirstRead)
        );
    }

    #[test]
    fn a_choice_that_is_not_offered_falls_back_to_never() {
        // The front end is not trusted: "Après lecture" on a plain item is not honoured.
        assert_eq!(
            resolve(ExpiryChoice::AfterFirstRead, false, false, NOW),
            (None, ReadPolicy::Normal)
        );
    }

    #[test]
    fn the_offered_lists_are_the_chosen_ones() {
        use ExpiryChoice::*;
        assert_eq!(
            options_for(false, false),
            vec![Never, Hour1, Day1, Day7, Day30]
        );
        assert_eq!(
            options_for(true, false),
            vec![Never, AfterFirstRead, Hour1, Day1, Day7, Day30]
        );
    }

    #[test]
    fn durations_match_the_spec_and_stay_within_the_relay_cap() {
        use ExpiryChoice::*;
        let ms = |c| resolve(c, false, false, 0).0.unwrap();
        assert_eq!(ms(Hour1), 3_600_000);
        assert_eq!(ms(Day1), 86_400_000);
        assert_eq!(ms(Day7), 7 * 86_400_000);
        assert_eq!(ms(Day30), 30 * 86_400_000);
        assert!(ms(Day30) <= MAX_TTL_MS);
    }

    #[test]
    fn old_five_minute_choice_is_gone() {
        assert!(serde_json::from_str::<ExpiryChoice>("\"min5\"").is_err());
    }
}
