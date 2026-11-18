import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/core/LeaveSpace.kt: leaving the space and deleting it, as the
// app sees it (spec/api-v1.md 7.9).
//
// The core erases this device only on a positive proof of the relay. Anything else leaves a pending departure that the
// app shows at all times (`CoreState.departure`), with "Réessayer" and "Annuler". The one way to erase without proof is
// "Effacer quand même" (`TomiCore.eraseWithoutProof`), behind strong biometrics, and only while the core allows it.

/// Which departure is pending.
public enum DepartureKind: Sendable, Hashable {
    /// "Se déconnecter" (`DELETE /v1/me`).
    case leave
    /// "Supprimer l'espace", the last device (`DELETE /v1/space`).
    case deleteSpace
}

/// Why a departure is still pending. Nothing was erased.
public enum PendingReason: Sendable, Hashable {
    /// Network error, timeout, unexpected answer. Retried.
    case network
    /// `5xx`, `429`. Retried.
    case server
    /// The relay does not find the space or this device: not a proof of anything, "Effacer quand même" may be offered.
    case notFound
    /// Verification only, more than 15 minutes after the gesture: the device is still a member.
    case notSent
}

/// Why the relay refused a departure for good. Nothing was erased and nothing is pending any more.
public enum RefusalReason: Sendable, Hashable {
    /// The only device of the space: "Supprimer l'espace" is offered.
    case lastDevice
    case updateRequired
    /// The relay cannot delete a space: "Effacer quand même" is allowed.
    case relayTooOld
    case unexpected
}

/// What one attempt of a departure gave.
public enum DepartureResult: Sendable, Hashable {
    /// The relay proved it and this device holds nothing of the space any more.
    case erased
    /// Nothing was erased; the departure stays pending, visible and cancellable.
    case pending(PendingReason)
    /// Nothing was erased; the departure is no longer pending.
    case refused(RefusalReason)
    /// Cancelled meanwhile: nothing was erased.
    case cancelled
    /// No departure pending (`continueDeparture`).
    case nothingPending
    /// The storage refused the erasure, or the call failed: nothing was erased.
    case failed

    /// Pending on the network or on the server: worth trying again by itself.
    public var isTransient: Bool {
        if case let .pending(reason) = self { return reason == .network || reason == .server }
        return false
    }
}

/// The departure pending on this device. `wiping`: the proof came and the erasure runs (no button, not cancellable).
/// `since`: time of the last gesture. `last`: why the last attempt of this process is pending, nil before the first.
public struct Departure: Sendable, Hashable {
    public let kind: DepartureKind
    public let wiping: Bool
    public let since: Date?
    public let last: PendingReason?

    public init(kind: DepartureKind, wiping: Bool, since: Date?, last: PendingReason?) {
        self.kind = kind
        self.wiping = wiping
        self.since = since
        self.last = last
    }
}

/// What "Effacer quand même" gave.
public enum EraseResult: Sendable, Hashable {
    case erased
    /// The core no longer allows it (the verdict expired or changed): nothing was erased.
    case notAllowed
    case failed
}

/// How the relay of the demo answers the next departures (debug menu of the demo core), so that every screen of
/// "Se déconnecter" can be tried. The departure itself runs the real code of the core.
public enum DemoDepartureScenario: Sendable, Hashable, CaseIterable {
    case normal, offline, serverError, lastDevice, reset, oldRelay
}

/// The strong biometric check of an irreversible action, as the screens get it.
public enum StrongAuthOutcome: Sendable, Hashable {
    case success
    /// Dismissed or failed: nothing happens.
    case cancelled
    /// No biometrics enrolled (or locked out): the action is impossible, the user is told.
    case unavailable
}
