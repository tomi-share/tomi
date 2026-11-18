//! Which other devices have not shown up for a long time, so that the Devices screen can
//! suggest removing them.
//!
//! An app that is uninstalled does not tell the relay: the device stays a member until another
//! device revokes it, counts in `max_devices` and keeps receiving every key rotation. Tomi can
//! only know that a device has not shown up, never that it is gone, and removing one is always
//! the user's decision (the usual revocation, with its confirmation). Nothing here removes
//! anything.
//!
//! The rule is the relay's own (`api-v1.md` §0.2): a device is active while its `last_seen_at`
//! is not older than `inactive_after_days`; it is inactive only when it is strictly older.

/// `inactive_after_days` of the relay (`api-v1.md` §0.2, §8.1, default 30).
///
/// The relay does not announce its value to the apps (the core hands over neither the
/// threshold nor the `active` flag of the roster), so the default is used. A relay configured
/// with another threshold would make the hint early or late, never wrong about what it says.
pub const INACTIVE_AFTER_DAYS: i64 = 30;

/// Whether the date the core gives for a device is the `last_seen_at` of the relay.
///
/// It is: `tomi-core` stores the `last_seen_at` that `GET /v1/devices` reports (read again every
/// few minutes while the device is in use) and no longer the time of pairing, which was not an
/// activity. Keep it `true` only while that holds: with the pairing time, the hint would tell the
/// user that a device in daily use "has not shown up for 40 days".
pub const LAST_SEEN_IS_FROM_RELAY: bool = true;

const DAY_MS: i64 = 86_400_000;

/// Whole days since `last_seen_ms` when the device counts as inactive, `None` otherwise.
///
/// * this device is never reported (it is running);
/// * a device that never showed up (`None`) is not reported: there is no date to count from, and
///   a device just paired that has not opened the app yet is normal;
/// * exactly `threshold_days` is still active (the relay's `last_seen_at >= now - threshold`);
/// * a date in the future (a skewed clock) is not inactive.
pub fn inactive_days(
    last_seen_ms: Option<i64>,
    this_device: bool,
    now_ms: i64,
    threshold_days: i64,
) -> Option<u32> {
    if this_device {
        return None;
    }
    let elapsed = now_ms.saturating_sub(last_seen_ms?);
    if elapsed <= threshold_days.saturating_mul(DAY_MS) {
        return None;
    }
    Some(u32::try_from(elapsed / DAY_MS).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn days_ago(d: i64) -> Option<i64> {
        Some(NOW - d * DAY_MS)
    }

    #[test]
    fn exactly_at_the_threshold_is_still_active() {
        assert_eq!(inactive_days(days_ago(30), false, NOW, 30), None);
    }

    #[test]
    fn one_millisecond_past_the_threshold_is_inactive() {
        let seen = Some(NOW - 30 * DAY_MS - 1);
        assert_eq!(inactive_days(seen, false, NOW, 30), Some(30));
    }

    #[test]
    fn counts_whole_days() {
        assert_eq!(inactive_days(days_ago(34), false, NOW, 30), Some(34));
        let seen = Some(NOW - 34 * DAY_MS - DAY_MS / 2);
        assert_eq!(inactive_days(seen, false, NOW, 30), Some(34));
    }

    #[test]
    fn a_recent_device_is_active() {
        assert_eq!(inactive_days(days_ago(0), false, NOW, 30), None);
        assert_eq!(inactive_days(days_ago(29), false, NOW, 30), None);
    }

    #[test]
    fn a_device_that_never_showed_up_is_not_reported() {
        assert_eq!(inactive_days(None, false, NOW, 30), None);
    }

    #[test]
    fn this_device_is_never_reported() {
        assert_eq!(inactive_days(days_ago(400), true, NOW, 30), None);
        assert_eq!(inactive_days(None, true, NOW, 30), None);
    }

    #[test]
    fn a_date_in_the_future_is_not_inactive() {
        assert_eq!(inactive_days(Some(NOW + 5 * DAY_MS), false, NOW, 30), None);
    }

    #[test]
    fn extreme_values_do_not_overflow() {
        assert_eq!(
            inactive_days(Some(i64::MIN), false, i64::MAX, 30),
            Some(u32::MAX)
        );
        assert_eq!(inactive_days(Some(0), false, i64::MIN, 30), None);
        assert_eq!(inactive_days(days_ago(400), false, NOW, i64::MAX), None);
    }

    #[test]
    fn the_default_threshold_is_the_relays() {
        assert_eq!(INACTIVE_AFTER_DAYS, 30);
    }
}
