import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/send/UndoDeadline.kt.

/// End of the undo window, on a monotonic clock in milliseconds that keeps counting while the device sleeps and
/// ignores wall-clock changes (the app injects it, e.g. from `CLOCK_MONOTONIC_RAW`; tests inject a fake one).
///
/// The same value drives the banner (what the user sees) and the background job (when the item may leave), so the
/// "Undo" button is honest. The decisive guard stays `TomiCore.cancelIfQueued`.
public struct UndoDeadline: Sendable, Hashable {
    public let atElapsedMs: Int64
    public let windowMs: Int64

    public init(atElapsedMs: Int64, windowMs: Int64) {
        self.atElapsedMs = atElapsedMs
        self.windowMs = windowMs
    }

    /// Time left for the user to undo, never negative.
    public func remainingMs(_ nowElapsedMs: Int64) -> Int64 { max(atElapsedMs - nowElapsedMs, 0) }

    /// Progress bar value: 1 at the start, 0 at the deadline.
    public func fraction(_ nowElapsedMs: Int64) -> Float {
        if windowMs <= 0 { return 0 }
        return min(max(Float(remainingMs(nowElapsedMs)) / Float(windowMs), 0), 1)
    }

    public func isOver(_ nowElapsedMs: Int64) -> Bool { remainingMs(nowElapsedMs) == 0 }

    /// How long a background job must still wait before it may push the item. A value outside `1...windowMs` is
    /// ignored: it means the clock was reset by a reboot, and the window is long over.
    public func waitBeforeDispatchMs(_ nowElapsedMs: Int64) -> Int64 {
        let remaining = atElapsedMs - nowElapsedMs
        return remaining >= 1 && remaining <= windowMs ? remaining : 0
    }

    public static func startingAt(_ nowElapsedMs: Int64, windowMs: Int64) -> UndoDeadline {
        UndoDeadline(atElapsedMs: nowElapsedMs + windowMs, windowMs: windowMs)
    }

    /// Same, on the default monotonic clock (`MonotonicClock.nowMs`).
    public static func startingNow(windowMs: Int64) -> UndoDeadline {
        startingAt(MonotonicClock.nowMs(), windowMs: windowMs)
    }

    /// Same, reading the injected monotonic clock.
    public static func startingAt(clock: () -> Int64, windowMs: Int64) -> UndoDeadline {
        startingAt(clock(), windowMs: windowMs)
    }
}

/// Monotonic clock in milliseconds that keeps counting while the device sleeps and ignores wall-clock changes
/// (`CLOCK_MONOTONIC_RAW`; `Date` and `ProcessInfo.systemUptime` do not fit: one jumps, the other stops in sleep).
public enum MonotonicClock {
    public static func nowMs() -> Int64 {
        Int64(clamping: clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW) / 1_000_000)
    }
}
