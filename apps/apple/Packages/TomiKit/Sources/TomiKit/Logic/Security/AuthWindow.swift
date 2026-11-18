import Foundation

// Port of apps/android/.../security/AuthWindow.kt (and AuthOutcome of BiometricGate.kt).

/// Result of a biometric / passcode check.
public enum AuthOutcome: Sendable, Hashable {
    case success
    case cancelled
    /// No suitable biometrics or passcode is set up: the protected action stays locked.
    case unavailable
    case failed
}

/// Remembers for `windowMs` that the user just passed a biometric check on this device
/// (the validation is valid for 2 minutes). `nowMs` is a monotonic
/// clock in milliseconds (it must keep counting while the device sleeps).
public final class AuthWindow: @unchecked Sendable {
    public static let defaultWindowMs: Int64 = 2 * 60 * 1000

    private let nowMs: @Sendable () -> Int64
    private let windowMs: Int64
    private let lock = NSLock()
    private var validUntilMs = Int64.min

    public init(nowMs: @escaping @Sendable () -> Int64, windowMs: Int64 = AuthWindow.defaultWindowMs) {
        self.nowMs = nowMs
        self.windowMs = windowMs
    }

    public func markAuthenticated() {
        lock.withLock { validUntilMs = nowMs() + windowMs }
    }

    public var isValid: Bool {
        lock.withLock { nowMs() < validUntilMs }
    }

    public func invalidate() {
        lock.withLock { validUntilMs = .min }
    }
}

/// Decides whether the clipboard must be cleared after a hidden item was copied. The content is
/// never read: only a marker of the clipboard state is compared with the one recorded when the
/// item was put there (Android: the clip timestamp; iOS: `UIPasteboard.changeCount`). 0 means
/// "unknown"; nil means "not readable now".
public enum ClipboardClearPolicy {
    public static let clearDelayMs: Int64 = 30_000

    /// Timed job (30 s after the copy). Unknown on either side: clear, the content may be a secret.
    public static func shouldClear(expected: Int64, current: Int64?) -> Bool {
        guard expected != 0, let current else { return true }
        return expected == current
    }

    public enum ForegroundAction: Sendable, Hashable {
        /// The 30 s are not over: the timed job will do it.
        case wait
        /// The clipboard still holds our item and is overdue: clear it.
        case clear
        /// Not provably ours any more: stop tracking, leave it alone.
        case forget
        /// Not readable yet: keep tracking, try again at the next foreground.
        case retry
    }

    /// Check when the app comes back on screen. Unlike `shouldClear` it never clears a clipboard it
    /// cannot prove to hold our item.
    public static func onForeground(expected: Int64, current: Int64?, nowMs: Int64, deadlineMs: Int64) -> ForegroundAction {
        if nowMs < deadlineMs { return .wait }
        guard let current else { return .retry }
        if expected != 0 && current == expected { return .clear }
        return .forget
    }
}
