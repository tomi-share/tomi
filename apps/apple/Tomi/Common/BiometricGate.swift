import LocalAuthentication
import TomiKit

/// Face ID / Touch ID or the passcode before showing, copying or sharing a hidden item (Android
/// `BiometricGate`). A success stays valid 2 minutes on this device (`AuthWindow`); PR-7 adds the
/// invalidation on lock and the biometrics-only variant for revocation.
@MainActor
final class BiometricGate {
    let window = AuthWindow(nowMs: { MonotonicClock.nowMs() })
    /// A system prompt is up (the Mac popover does not close behind it).
    private(set) var isPrompting = false
    private let evaluate: @MainActor (String) async -> AuthOutcome
    private let evaluateStrong: @MainActor (String) async -> AuthOutcome

    /// `evaluate` and `evaluateStrong` replace the system prompts in tests and previews (a trailing closure is
    /// `evaluate`).
    init(
        evaluateStrong: (@MainActor (String) async -> AuthOutcome)? = nil,
        evaluate: (@MainActor (String) async -> AuthOutcome)? = nil
    ) {
        self.evaluate = evaluate ?? { await Self.system(reason: $0, policy: .deviceOwnerAuthentication) }
        #if os(macOS)
        // A Mac may have no Touch ID at all (Mac mini, a closed lid): the destructive actions would be impossible.
        // Biometrics alone when the Mac has them, else the password of the Mac (docs/macos-app.md, A7).
        self.evaluateStrong = evaluateStrong ?? { reason in
            let strong = await Self.system(reason: reason, policy: .deviceOwnerAuthenticationWithBiometrics)
            guard strong == .unavailable else { return strong }
            return await Self.system(reason: reason, policy: .deviceOwnerAuthentication)
        }
        #else
        self.evaluateStrong = evaluateStrong ?? { await Self.system(reason: $0, policy: .deviceOwnerAuthenticationWithBiometrics) }
        #endif
    }

    /// Face ID / Touch ID only, every time, for a destructive action (revoking a device): no passcode fallback, and
    /// neither the 2-minute window nor this success are remembered. `.unavailable` without enrolled biometrics.
    /// On the Mac, where biometrics may not exist or be locked out: then the password of the Mac (see `init`).
    func authenticateStrong(reason: String) async -> AuthOutcome {
        isPrompting = true
        defer { isPrompting = false }
        return await evaluateStrong(reason)
    }

    /// Face ID / Touch ID or the passcode, asked every time, for an action that opens the space to another device
    /// (adding one): the 2-minute window is neither used nor extended, so a phone left unlocked right after a reveal
    /// cannot be used to add a device without the owner.
    func authenticateFresh(reason: String) async -> AuthOutcome {
        // The Mac keeps its popover open behind the prompt only while it knows one is up.
        isPrompting = true
        defer { isPrompting = false }
        return await evaluate(reason)
    }

    func authenticate(reason: String) async -> AuthOutcome {
        if window.isValid { return .success }
        isPrompting = true
        defer { isPrompting = false }
        let outcome = await evaluate(reason)
        if case .success = outcome { window.markAuthenticated() }
        return outcome
    }

    private static func system(reason: String, policy: LAPolicy) async -> AuthOutcome {
        let context = LAContext()
        // Biometrics only: no "Saisir le code" button that would only cancel.
        if policy == .deviceOwnerAuthenticationWithBiometrics { context.localizedFallbackTitle = "" }
        var error: NSError?
        guard context.canEvaluatePolicy(policy, error: &error) else { return .unavailable }
        do {
            return try await context.evaluatePolicy(policy, localizedReason: reason) ? .success : .failed
        } catch let error as LAError where [.userCancel, .appCancel, .systemCancel, .userFallback].contains(error.code) {
            return .cancelled
        } catch let error as LAError where [.biometryLockout, .biometryNotEnrolled, .biometryNotAvailable].contains(error.code) {
            return .unavailable
        } catch {
            return .failed
        }
    }
}
