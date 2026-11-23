import Foundation

/// Which other devices have not shown up for a long time, so that the Devices screen can suggest removing them
/// (Android `DeviceInactivity.kt`).
///
/// An app that is uninstalled does not tell the relay: the device stays a member until another device revokes it,
/// counts in `max_devices` and keeps receiving every key rotation. Tomi can only know that a device has not shown up,
/// never that it is gone, and removing one is always the user's decision (the usual revocation, with its confirmation
/// and its strong authentication). Nothing here removes anything.
///
/// The rule is the relay's own (`spec/api-v1.md` §0.2): a device is active while its `last_seen_at` is not older than
/// `inactive_after_days`; it is inactive only when it is strictly older.
public enum DeviceInactivity {
    /// `inactive_after_days` of the relay (`spec/api-v1.md` §0.2, §8.1, default 30). The relay does not announce its
    /// value to the apps, so the default is used. A relay configured with another threshold would make the hint early
    /// or late, never wrong about what it says.
    public static let inactiveAfterDays = 30

    /// Whether `Device.lastActivity` is the `last_seen_at` of the relay. It is: the core stores the `last_seen_at`
    /// that `GET /v1/devices` reports and no longer the time of pairing, which was not an activity. Keep it `true`
    /// only while that holds: with the pairing time, the hint would tell the user that a device in daily use "has not
    /// shown up for 40 days".
    public static let lastSeenIsFromRelay = true

    /// Whole days since `device.lastActivity` when `device` counts as inactive, nil otherwise.
    ///
    /// - this device is never reported (it is running);
    /// - a device that never showed up (no date) is not reported: there is no date to count from, and a device just
    ///   paired that has not opened the app yet is normal;
    /// - exactly `thresholdDays` is still active (the relay's `last_seen_at >= now - threshold`);
    /// - a date in the future (a skewed clock) is not inactive.
    public static func inactiveDays(
        _ device: Device, now: Date, thresholdDays: Int = inactiveAfterDays, lastSeenFromRelay: Bool = lastSeenIsFromRelay
    ) -> Int? {
        guard lastSeenFromRelay, !device.isThisDevice, let seen = device.lastActivity else { return nil }
        let elapsedMs = Int64((now.timeIntervalSince(seen) * 1000).rounded())
        let dayMs: Int64 = 24 * 60 * 60 * 1000
        guard elapsedMs > Int64(thresholdDays) * dayMs else { return nil }
        return Int(elapsedMs / dayMs)
    }
}
