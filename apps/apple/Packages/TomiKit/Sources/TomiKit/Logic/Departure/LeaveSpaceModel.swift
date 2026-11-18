import Foundation
import Observation

// Port of `LeaveSpaceViewModel`, `LeaveUiState`, `DepartureBannerState` and `bannerOf` in
// apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/LeaveSpaceDialog.kt.

/// "Se déconnecter de ton espace ?". `busy`: the attempt runs (progress, only "Annuler"). `error`: the relay refused for
/// good (a failure, never a departure). `failed`: the storage refused the erasure or the call failed; nothing was erased.
public struct ConfirmLeave: Equatable, Sendable {
    public var busy = false
    public var error: RefusalReason?
    public var failed = false

    public init(busy: Bool = false, error: RefusalReason? = nil, failed: Bool = false) {
        self.busy = busy
        self.error = error
        self.failed = failed
    }
}

/// "Supprimer ton espace ?", after the relay said this is the only device. `armed`: the button works 2 s after the dialog
/// shows. `authenticating`: the strong biometric prompt is up. `error == .relayTooOld` offers "Effacer quand même".
public struct ConfirmDelete: Equatable, Sendable {
    public var armed = false
    public var authenticating = false
    public var busy = false
    public var error: RefusalReason?
    public var failed = false
    public var biometricUnavailable = false

    public init(armed: Bool = false, authenticating: Bool = false, busy: Bool = false, error: RefusalReason? = nil,
                failed: Bool = false, biometricUnavailable: Bool = false) {
        self.armed = armed
        self.authenticating = authenticating
        self.busy = busy
        self.error = error
        self.failed = failed
        self.biometricUnavailable = biometricUnavailable
    }

    public var canConfirm: Bool { armed && !authenticating && !busy }
}

/// "Effacer quand même" (D4): erases this device without a proof of the relay. `checking`: Tomi asks the core (and, if
/// needed, the relay) whether it is allowed. `notAllowed`: it is not; `expired`: it was, but no longer after the biometric
/// check. The button needs the box (`checked`) and 2 s (`armed`). `waiting`: the biometric check passed but an attempt of
/// the departure still holds the core ("Annuler" leaves: nothing was asked yet).
public struct EraseWithoutProof: Equatable, Sendable {
    public var checked = false
    public var armed = false
    public var checking = false
    public var authenticating = false
    public var busy = false
    public var notAllowed = false
    public var expired = false
    public var failed = false
    public var biometricUnavailable = false
    public var waiting = false

    public init(checked: Bool = false, armed: Bool = false, checking: Bool = false, authenticating: Bool = false,
                busy: Bool = false, notAllowed: Bool = false, expired: Bool = false, failed: Bool = false,
                biometricUnavailable: Bool = false, waiting: Bool = false) {
        self.checked = checked
        self.armed = armed
        self.checking = checking
        self.authenticating = authenticating
        self.busy = busy
        self.notAllowed = notAllowed
        self.expired = expired
        self.failed = failed
        self.biometricUnavailable = biometricUnavailable
        self.waiting = waiting
    }

    public var canConfirm: Bool { checked && armed && !checking && !authenticating && !busy && !notAllowed }
}

/// Which dialog of the departure is open, and what it shows.
public enum LeaveDialog: Equatable, Sendable {
    case hidden
    case confirmLeave(ConfirmLeave)
    case confirmDelete(ConfirmDelete)
    case eraseWithoutProof(EraseWithoutProof)
}

/// The banner at the top of the feed: what the departure is doing, at all times.
public enum DepartureBanner: Equatable, Sendable {
    case none
    /// A departure waits for a proof of the relay. `running`: an attempt runs ("Réessayer" off, "Annuler" not).
    /// `authenticating`: the strong prompt of the "Réessayer" of a deletion is up.
    case pending(kind: DepartureKind, last: PendingReason?, running: Bool, authenticating: Bool)
    /// The proof came: the erasure runs. No button.
    case wiping
    /// Cancelled, but the attempt that had left keeps running to its end (it erases nothing).
    case cancelling
    /// No departure, and the relay does not find the space ("Espace introuvable").
    case spaceNotFound
    /// No departure, and another device removed this one from the space.
    case revoked

    /// "Effacer quand même" in the banner: only after "not found", never after a network or server error.
    public var offersEraseAnyway: Bool {
        if case let .pending(_, last, running, authenticating) = self { return last == .notFound && !running && !authenticating }
        return false
    }

    /// No marker and an attempt runs while no dialog is open: an attempt whose departure was just cancelled, still going
    /// to its end. Derived from the core, so that it survives a new model.
    public static func of(departure: Departure?, running: Bool, authenticating: Bool, dialogHidden: Bool,
                          spaceState: SpaceState) -> DepartureBanner {
        if departure?.wiping == true { return .wiping }
        if let departure {
            return .pending(kind: departure.kind, last: departure.last, running: running, authenticating: authenticating)
        }
        if running && dialogHidden { return .cancelling }
        if spaceState == .notFound { return .spaceNotFound }
        if spaceState == .revoked { return .revoked }
        return .none
    }
}

/// One-line confirmations of the departure, shown by the feed.
public enum LeaveEvent: Equatable, Sendable {
    case cancelled(DepartureKind)
    /// A "Réessayer" from the banner failed in the storage: nothing was erased.
    case failed
    /// The "Réessayer" of a deletion needs strong biometrics and the phone has none: nothing was sent.
    case biometricUnavailable
}

/// The departure of this device (spec/api-v1.md 7.9): the dialogs, the banner and their
/// buttons. The core erases only on a proof of the relay; everything else leaves a pending departure that the banner
/// shows with "Réessayer" and "Annuler", and that `followUp` retries by itself. The one way to erase without proof is
/// "Effacer quand même": a box, 2 s, a fresh look at whether the core allows it, strong biometrics (asked by the screen,
/// which then calls `…Authenticated` or `authEnded`), then the call. Once erased the device is no longer paired and the
/// app goes back to the welcome screen.
@MainActor
@Observable
public final class LeaveSpaceModel {
    /// The confirm button of an irreversible dialog works only this long after the dialog shows.
    public nonisolated static let armDelay: Duration = .seconds(2)

    public private(set) var dialog = LeaveDialog.hidden

    /// Where the "Réessayer" of the banner is: a second tap does nothing until it is idle again.
    private enum RetryPhase { case idle, authenticating, running }
    private var retryPhase = RetryPhase.idle

    @ObservationIgnored public var onEvent: ((LeaveEvent) -> Void)?
    @ObservationIgnored private let core: any TomiCore
    @ObservationIgnored private let followUp: any DepartureFollowUp
    @ObservationIgnored private let armDelay: Duration
    @ObservationIgnored private let sleep: (Duration) async throws -> Void
    /// Changes whenever the user closes or replaces a dialog: a result or a timer of the previous one is dropped.
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var armTask: Task<Void, Never>?

    public init(
        core: any TomiCore, followUp: any DepartureFollowUp, armDelay: Duration = LeaveSpaceModel.armDelay,
        sleep: @escaping (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    ) {
        self.core = core
        self.followUp = followUp
        self.armDelay = armDelay
        self.sleep = sleep
        // A refusal met by an automatic attempt (start-up, back-off) is shown as soon as no dialog is open.
        followUp.onRefusal = { [weak self] in self?.showPendingRefusal() }
        showPendingRefusal()
    }

    /// An attempt (or an erasure) runs in the core.
    public var attemptRunning: Bool { core.state.departureRunning }

    public var banner: DepartureBanner {
        DepartureBanner.of(
            departure: core.state.departure, running: core.state.departureRunning || retryPhase == .running,
            authenticating: retryPhase == .authenticating, dialogHidden: dialog == .hidden, spaceState: core.state.spaceState
        )
    }

    /// "Se déconnecter" belongs in the menu only when nothing is pending nor running (the banner handles the rest).
    public var canOfferLeave: Bool { core.state.departure == nil && !core.state.departureRunning }

    // MARK: "Se déconnecter"

    /// "Se déconnecter" in the menu, or in the banner of a revoked device.
    public func open() {
        if dialog == .hidden { replace(.confirmLeave(ConfirmLeave())) }
    }

    /// Back, tap outside, "Annuler" of an idle dialog. Ignored while a call runs ("Annuler" is then `cancelRunning`).
    public func dismiss() {
        let busy: Bool
        switch dialog {
        case .hidden: return
        case let .confirmLeave(current): busy = current.busy
        case let .confirmDelete(current): busy = current.busy || current.authenticating
        // Waiting for an attempt to end after the biometric check: nothing was asked of the core yet, "Annuler" leaves.
        case let .eraseWithoutProof(current): busy = (current.busy || current.authenticating) && !current.waiting
        }
        if !busy { replace(.hidden) }
    }

    /// "Se déconnecter" in the dialog (also the retry after a refusal or a failure). One attempt at a time.
    public func confirmLeave() {
        guard case let .confirmLeave(current) = dialog, !current.busy else { return }
        dialog = .confirmLeave(ConfirmLeave(busy: true))
        attempt(.leave) { await $0.leaveSpace() }
    }

    /// "Annuler" while an attempt of the dialog runs: immediate, never erases; the attempt goes on and erases nothing.
    public func cancelRunning() {
        let kind: DepartureKind
        switch dialog {
        case let .confirmLeave(current) where current.busy: kind = .leave
        case let .confirmDelete(current) where current.busy: kind = .deleteSpace
        default: return
        }
        replace(.hidden)
        cancel(kind)
    }

    // MARK: "Supprimer l'espace" (the only device)

    /// "Supprimer l'espace": true when the screen must now ask for strong biometrics, then call `deleteAuthenticated`
    /// or `authEnded`.
    public func beginDelete() -> Bool {
        guard case var .confirmDelete(current) = dialog, current.canConfirm else { return false }
        current.authenticating = true
        current.biometricUnavailable = false
        current.failed = false
        dialog = .confirmDelete(current)
        return true
    }

    /// The strong biometric check of "Supprimer l'espace" passed.
    public func deleteAuthenticated() {
        guard case let .confirmDelete(current) = dialog, current.authenticating else { return }
        dialog = .confirmDelete(ConfirmDelete(armed: true, busy: true))
        attempt(.deleteSpace) { await $0.deleteSpace() }
    }

    // MARK: "Effacer quand même"

    /// Opens "Effacer quand même" (from the banner, or from "Supprimer ton espace ?" when the relay is too old) and
    /// checks whether it is allowed.
    public func openErase() {
        switch dialog {
        case .hidden: break
        case let .confirmDelete(current): if current.busy || current.authenticating { return }
        default: return
        }
        let gen = replace(.eraseWithoutProof(EraseWithoutProof(checking: true)))
        arm(gen)
        checkErase(gen)
    }

    /// "Réessayer" in "Effacer quand même" after it was not allowed: asks again.
    public func recheckErase() {
        guard case var .eraseWithoutProof(current) = dialog, !current.checking, !current.busy, !current.authenticating else { return }
        current.checking = true
        current.notAllowed = false
        current.expired = false
        current.failed = false
        dialog = .eraseWithoutProof(current)
        checkErase(generation)
    }

    public func setEraseChecked(_ checked: Bool) {
        guard case var .eraseWithoutProof(current) = dialog, !current.busy, !current.authenticating else { return }
        current.checked = checked
        dialog = .eraseWithoutProof(current)
    }

    /// "Effacer ce téléphone": reads again whether the core allows it, right before the biometric prompt. True when the
    /// screen must now ask for strong biometrics, then call `eraseAuthenticated` or `authEnded`.
    public func beginErase() async -> Bool {
        guard case var .eraseWithoutProof(current) = dialog, current.canConfirm, !core.state.departureRunning else { return false }
        let gen = generation
        current.authenticating = true
        current.biometricUnavailable = false
        current.failed = false
        current.expired = false
        dialog = .eraseWithoutProof(current)
        let allowed = await core.canEraseWithoutProof()
        guard gen == generation else { return false }
        if !allowed {
            updateErase { $0.authenticating = false; $0.notAllowed = true }
            return false
        }
        return true
    }

    /// The strong biometric check of "Effacer quand même" passed.
    public func eraseAuthenticated() {
        guard case var .eraseWithoutProof(current) = dialog, current.authenticating else { return }
        let gen = generation
        current.authenticating = false
        current.busy = true
        dialog = .eraseWithoutProof(current)
        Task {
            // An attempt of the departure started meanwhile holds the core: waits for it, with an "Annuler" that leaves
            // (the core is not asked yet, so nothing is erased). The attempt may last minutes on a bad network.
            if core.state.departureRunning {
                updateErase { $0.waiting = true }
                await waitUntil { !self.core.state.departureRunning || gen != self.generation }
                guard gen == generation else { return }
                updateErase { $0.waiting = false }
            }
            let result = await core.eraseWithoutProof()
            guard gen == generation else { return }
            switch result {
            case .erased: replace(.hidden)
            // The verdict expired or was replaced while the user confirmed: nothing was erased, say so.
            case .notAllowed: updateErase { $0.busy = false; $0.notAllowed = true; $0.expired = true }
            case .failed: updateErase { $0.busy = false; $0.failed = true }
            }
        }
    }

    /// The biometric prompt ended without success (`unavailable`: no strong biometrics enrolled). Nothing happens.
    public func authEnded(unavailable: Bool) {
        switch dialog {
        case var .confirmDelete(current) where current.authenticating:
            current.authenticating = false
            current.biometricUnavailable = unavailable
            dialog = .confirmDelete(current)
        case var .eraseWithoutProof(current) where current.authenticating:
            current.authenticating = false
            current.biometricUnavailable = unavailable
            dialog = .eraseWithoutProof(current)
        default:
            break
        }
    }

    // MARK: The banner

    /// "Réessayer": a gesture, the request is sent again. Off while an attempt runs. The retry of a "Leave" is sent at once
    /// (false). The retry of a deletion deletes the space on the relay: true when the screen must now ask for strong
    /// biometrics, then call `retryAuthenticated` or `retryAuthEnded`. The automatic attempts need no biometrics: they send
    /// again only within 15 minutes of the gesture, and only verify after that.
    public func beginRetry() -> Bool {
        guard let departure = core.state.departure, !departure.wiping, retryPhase == .idle, !core.state.departureRunning else {
            return false
        }
        if departure.kind == .deleteSpace {
            retryPhase = .authenticating
            return true
        }
        runRetry(departure.kind)
        return false
    }

    /// The strong biometric check of the "Réessayer" of a deletion passed: sent again if that deletion is still pending.
    public func retryAuthenticated() {
        guard retryPhase == .authenticating else { return }
        // The prompt took its time: the departure may have been cancelled, replaced, or an attempt may have started.
        guard let departure = core.state.departure, departure.kind == .deleteSpace, !departure.wiping,
              !core.state.departureRunning else {
            retryPhase = .idle
            return
        }
        runRetry(departure.kind)
    }

    /// The biometric prompt of the "Réessayer" ended without success. Nothing is sent.
    public func retryAuthEnded(unavailable: Bool) {
        guard retryPhase == .authenticating else { return }
        retryPhase = .idle
        if unavailable { onEvent?(.biometricUnavailable) }
    }

    /// "Annuler" in the banner: always available but during the erasure, immediate.
    public func cancelFromBanner() {
        guard let departure = core.state.departure, !departure.wiping else { return }
        cancel(departure.kind)
    }

    // MARK: Internals

    private func runRetry(_ kind: DepartureKind) {
        retryPhase = .running
        Task {
            defer { retryPhase = .idle }
            let result = await core.retryDeparture()
            if result.isTransient {
                followUp.kick()
            } else if case let .refused(reason) = result {
                if dialog == .hidden { showRefusal(DepartureRefusal(kind: kind, reason: reason)) }
            } else if result == .failed, core.state.departure != nil {
                // Not after a cancellation: the departure is gone, the failure of the cancelled attempt is not news.
                onEvent?(.failed)
            }
        }
    }

    private func cancel(_ fallbackKind: DepartureKind) {
        let kind = core.state.departure?.kind ?? fallbackKind
        Task {
            var cancelled = await core.cancelDeparture()
            // The attempt that runs may not have written its marker yet: there was nothing to cancel, and it would go on
            // to erase on a proof. Cancels again, every 40 ms (a short call that never waits for the attempt), until it
            // takes or the attempt is over.
            while !cancelled && core.state.departureRunning {
                do { try await sleep(.milliseconds(40)) } catch { break }
                cancelled = await core.cancelDeparture()
            }
            if cancelled { onEvent?(.cancelled(kind)) }
        }
    }

    /// Runs one attempt for the open dialog and shows what it gave, unless the user closed or replaced the dialog.
    private func attempt(_ kind: DepartureKind, _ call: @escaping (any TomiCore) async -> DepartureResult) {
        let gen = generation
        let core = self.core
        Task {
            let result = await call(core)
            if result.isTransient { followUp.kick() }
            guard gen == generation else { return }
            switch result {
            // Erased: the app goes back to the welcome screen. Pending: the banner takes over (and the retries).
            case .erased, .pending, .cancelled, .nothingPending:
                replace(.hidden)
            case let .refused(reason):
                showRefusal(DepartureRefusal(kind: kind, reason: reason))
            case .failed:
                switch kind {
                case .leave: replace(.confirmLeave(ConfirmLeave(failed: true)))
                case .deleteSpace: replaceArmed(ConfirmDelete(failed: true))
                }
            }
        }
    }

    /// A refusal of the relay, as a dialog: never shown as a departure that happened.
    private func showRefusal(_ refusal: DepartureRefusal) {
        if refusal.reason == .lastDevice {
            // The only device: "Supprimer ton espace ?" (again, after a "not the last device" detour).
            replaceArmed(ConfirmDelete())
        } else if refusal.kind == .deleteSpace {
            replaceArmed(ConfirmDelete(error: refusal.reason))
        } else {
            replace(.confirmLeave(ConfirmLeave(error: refusal.reason)))
        }
    }

    /// The refusal an automatic attempt met, once no dialog is open.
    private func showPendingRefusal() {
        guard dialog == .hidden, let refusal = followUp.refusal else { return }
        followUp.consumeRefusal()
        showRefusal(refusal)
    }

    private func checkErase(_ gen: Int) {
        Task {
            var allowed = await core.canEraseWithoutProof()
            if !allowed {
                // The verdict may be missing or old: ask the relay again, then read again. With a pending departure this is
                // the automatic attempt; without one, a sync (which opens a session).
                var refusal: DepartureRefusal?
                if let kind = core.state.departure?.kind {
                    let result = await core.continueDeparture()
                    if result.isTransient { followUp.kick() }
                    // "Relay too old" is what allows this very dialog: the read below says so. Any other refusal ends it.
                    if case let .refused(reason) = result, reason != .relayTooOld {
                        refusal = DepartureRefusal(kind: kind, reason: reason)
                    }
                } else {
                    // Offline or refused: not allowed, which is what the read below says.
                    try? await core.sync(pushToken: nil)
                }
                if let refusal {
                    if gen == generation { showRefusal(refusal) }
                    return
                }
                allowed = await core.canEraseWithoutProof()
            }
            if gen == generation { updateErase { $0.checking = false; $0.notAllowed = !allowed } }
        }
    }

    private func updateErase(_ change: (inout EraseWithoutProof) -> Void) {
        guard case var .eraseWithoutProof(current) = dialog else { return }
        change(&current)
        dialog = .eraseWithoutProof(current)
    }

    /// Shows `next` in place of the current dialog; what was started for the previous one is dropped.
    @discardableResult
    private func replace(_ next: LeaveDialog) -> Int {
        armTask?.cancel()
        armTask = nil
        dialog = next
        generation += 1
        if next == .hidden { showPendingRefusal() }
        return generation
    }

    /// Shows a confirmation whose button is armed after `armDelay`.
    private func replaceArmed(_ next: ConfirmDelete) {
        var next = next
        next.armed = false
        arm(replace(.confirmDelete(next)))
    }

    private func arm(_ gen: Int) {
        armTask?.cancel()
        let delay = armDelay
        armTask = Task {
            do { try await sleep(delay) } catch { return }
            guard gen == generation else { return }
            switch dialog {
            case var .confirmDelete(current):
                current.armed = true
                dialog = .confirmDelete(current)
            case var .eraseWithoutProof(current):
                current.armed = true
                dialog = .eraseWithoutProof(current)
            default:
                break
            }
        }
    }

    /// Waits until `condition` holds, reading the core again every 100 ms (the core state is observed on this actor).
    private func waitUntil(_ condition: @escaping () -> Bool) async {
        while !condition() {
            do { try await sleep(.milliseconds(100)) } catch { return }
        }
    }
}
