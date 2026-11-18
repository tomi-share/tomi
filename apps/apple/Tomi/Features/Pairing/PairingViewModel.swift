import Foundation
import Observation
import TomiKit

/// Input (scan or code) → Working → Verify (6-digit code) → History (one yes / no, only when the space has some) →
/// Waiting (the other device confirms too) → Done.
enum PairingPhase: Equatable {
    case input, working, verify, history, waiting, done
}

/// State of "Rejoins ton espace" (Android `PairingViewModel`). Nothing joins the space before the user has compared the
/// codes and confirmed. Leaving is possible at every step, the wait for the other device included: the pairing is then
/// cancelled at the core, outside the life of the screen.
@MainActor
@Observable
final class PairingViewModel {
    private(set) var phase = PairingPhase.input
    /// Error to show (a text key).
    private(set) var error: String?
    private(set) var code = ""
    /// The typed-code step instead of the scan step ("Saisis le code d'invitation").
    var enteringCode = false
    /// The other device was reached; nothing is joined yet.
    private(set) var awaiting: PairingPrompt?
    /// Until when the code is valid, on the monotonic clock (an indication, never a cut-off).
    private(set) var validUntil: ContinuousClock.Instant?
    /// The single yes / no answer of the history step, "Oui" preselected.
    var importHistory = true
    private(set) var busy = false
    /// A pairing link that opened the app (code and relay, read by the core): only offered, never used by itself. A
    /// secret: the code is single-use and printed nowhere.
    private(set) var pendingLink: ParsedPairLink?
    /// What is in the "Relais" field. A link or a QR code that names a relay fills it; the user can correct it. The relay
    /// is not trusted: the verification screen shows it in full.
    private(set) var relayText = ""
    /// URL of the relay the device already has, nil when it has none (then the field is required). Read from the core
    /// while no pairing is under way.
    private(set) var deviceRelay: String?

    private let core: () -> (any TomiCore)?
    private let reader: any LinkReading
    private let clock = ContinuousClock()
    /// Bumped by every start and every abort: an answer that comes back after the user left is ignored.
    private var attempt = 0
    private var job: Task<Void, Never>?

    /// `core` is read at each step: the debug demo entry may replace it.
    init(core: @escaping () -> (any TomiCore)?, reader: any LinkReading = DeepLinkParser.reader) {
        self.core = core
        self.reader = reader
    }

    // MARK: Relay

    /// The relay the next join will use, from the field and the device: the usable choice, "missing" or "invalid".
    var relayChoice: RelayAddress.Choice {
        RelayAddress.choose(field: relayText, deviceRelay: deviceRelay, reader: reader)
    }

    /// The field is open and required: the device has no relay of its own.
    var relayFieldRequired: Bool { deviceRelay == nil }

    /// Authority of the relay the device already has, to name it above the folded field.
    var deviceRelayAuthority: String? { deviceRelay.flatMap(RelayAddress.authority(of:)) }

    /// Authority of the relay of the pending link, shown in full on the card of the link.
    var pendingLinkRelayAuthority: String? { pendingLink?.relayUrl.flatMap(RelayAddress.authority(of:)) }

    func onRelayChange(_ value: String) {
        relayText = String(value.prefix(RelayAddress.maxInput))
        error = nil
    }

    /// Reads the relay of the device (when the screen shows, and each time it is back on its first step).
    func loadDeviceRelay() async {
        guard let core = core() else { return }
        deviceRelay = await core.currentRelay()
    }

    private func reloadDeviceRelay() {
        Task { await loadDeviceRelay() }
    }

    // MARK: Input

    func onCodeChange(_ value: String) {
        code = String(value.prefix(PairingInput.maxCodeInput))
        error = nil
    }

    /// A pairing link opened the app: offered on the screen, joined only by "Rejoindre". Its relay (if it names one)
    /// fills the field, where the user can read it and correct it.
    func offerLink(relayUrl: String?, code: String) {
        pendingLink = ParsedPairLink(relayUrl: relayUrl, code: code)
        if let relayUrl { relayText = relayUrl }
    }

    func joinWithPendingLink() {
        guard let link = pendingLink else { return }
        join(code: link.code)
    }

    /// A scanned QR code must be a pairing link, read by the core; anything else is refused before the core is called.
    func onQRScanned(_ raw: String) {
        guard let link = PairingInput.linkFromQR(raw, reader: reader) else {
            error = "pair_error_qr_unknown"
            return
        }
        if let relay = link.relayUrl { relayText = relay }
        join(code: link.code)
    }

    func joinWithCode() {
        guard PairingInput.isValidCode(code) else {
            error = "pair_error_code_format"
            return
        }
        join(code: code.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// The relay is settled before the core is called: the field (a link or a QR code filled it), else the device's own.
    /// None at all, or a text that is not an address, stops here with the error and the code on the typing step, where
    /// the field is.
    private func join(code: String) {
        guard phase == .input else { return }
        let choice = relayChoice
        self.code = code
        if let key = choice.errorKey {
            error = key
            pendingLink = nil
            enteringCode = true
            return
        }
        let relay = choice.urlToPass
        start { try await $0.pairWithCode(code: code, relayUrl: relay) }
    }

    // MARK: Verify, history, waiting

    /// "Les codes sont identiques": the history question if the space has any, otherwise the confirmation.
    func codesMatch() {
        guard let awaiting, !busy else { return }
        if awaiting.historyItemCount > 0 {
            phase = .history
        } else {
            finish(importHistory: false)
        }
    }

    /// "Ils sont différents": the pairing is cancelled, nothing is joined.
    func codesDiffer() {
        abort(error: "pair_verify_mismatch")
    }

    func confirmHistory() {
        finish(importHistory: importHistory)
    }

    /// Back or "Annuler" from any step after the input (working, verify, history, waiting): the pairing is cancelled.
    func leave() {
        abort(error: nil)
    }

    /// The device became paired while the screen was on its first step (a pairing interrupted by a stop was settled
    /// at start-up): nothing left to do here.
    func pairedChanged(_ paired: Bool) {
        guard paired && phase == .input else { return }
        // The link and the code are single-use secrets: nothing left to use them for.
        pendingLink = nil
        code = ""
        relayText = ""
        phase = .done
    }

    /// Milliseconds left on the code at `now`, 0 once past (and without a code).
    func remainingMs(at now: ContinuousClock.Instant) -> Int64 {
        guard let validUntil, validUntil > now else { return 0 }
        let left = now.duration(to: validUntil).components
        return left.seconds * 1000 + left.attoseconds / 1_000_000_000_000_000
    }

    /// Debug builds: the whole simulated pairing in one go (demo code, codes accepted, history kept) on the demo core,
    /// so a tester reaches the feed without a QR code. Stays on Working until the screen goes away.
    @discardableResult
    func enterDemo(open: () async -> (any TomiCore)?) async -> Bool {
        guard phase == .input else { return false }
        phase = .working
        error = nil
        guard let core = await open() else {
            phase = .input
            error = "pair_error_network"
            return false
        }
        do {
            guard case .awaitingVerification = try await core.pairWithCode(code: Self.demoCode, relayUrl: nil) else {
                phase = .input
                error = "pair_error_unexpected"
                return false
            }
            try await core.confirmPairing(importHistory: true)
            pendingLink = nil
            return true
        } catch {
            phase = .input
            self.error = "pair_error_unexpected"
            return false
        }
    }

    /// The discreet demo entry of the join screen: offered only by a real core built with the demo, in no space.
    var canEnterDemo: Bool { core()?.canEnterDemo ?? false }

    /// Enters the demo space of the core (no pairing, no network): the feed shows the demo data at once. Stays on
    /// Working until the screen goes away.
    @discardableResult
    func enterBundledDemo() async -> Bool {
        guard phase == .input, let core = core(), core.canEnterDemo else { return false }
        phase = .working
        error = nil
        do {
            try await core.enterDemo()
            pendingPayload = nil
            return true
        } catch {
            phase = .input
            self.error = "pair_error_unexpected"
            return false
        }
    }

    /// Back to the first step, for the next time the screen shows (after a revocation).
    func reset() {
        // A pairing left half-way must not stay open at the relay.
        let leftHalfway = phase != .input && phase != .done
        attempt += 1
        job?.cancel()
        phase = .input
        error = nil
        code = ""
        enteringCode = false
        awaiting = nil
        validUntil = nil
        busy = false
        pendingLink = nil
        relayText = ""
        if leftHalfway, let core = core() {
            Task {
                await core.cancelPairing()
                await loadDeviceRelay()
            }
        } else {
            reloadDeviceRelay()
        }
    }

    static let demoCode = "7-crayon-lune"

    // MARK: Private

    private func start(_ pair: @escaping (any TomiCore) async throws -> PairingResult) {
        guard phase == .input, let core = core() else { return }
        phase = .working
        error = nil
        attempt += 1
        let current = attempt
        job = Task {
            let result: PairingResult
            do {
                result = try await pair(core)
            } catch {
                result = .failure(.unexpected)
            }
            guard current == attempt else {
                // The user left meanwhile: a pairing the core opened after the cancel must not stay open.
                if case .awaitingVerification = result { await core.cancelPairing() }
                return
            }
            handle(result)
        }
    }

    private func handle(_ result: PairingResult) {
        switch result {
        case let .awaitingVerification(prompt):
            // The code and the link are single-use secrets: gone once claimed.
            pendingLink = nil
            code = ""
            awaiting = prompt
            validUntil = clock.now.advanced(by: .milliseconds(prompt.expiresInMs))
            importHistory = true
            phase = .verify
        case let .failure(reason):
            phase = .input
            error = PairingErrors.joinMessageKey(reason)
            // A link is worth trying again only after a network failure; a code after a typo, a network failure or a
            // relay to correct (then on the typing step, where the "Relais" field is).
            if reason != .network { pendingLink = nil }
            let relayProblem = reason == .relayNotConfigured || reason == .invalidRelay
            if reason != .invalidCode && reason != .network && !relayProblem { code = "" }
            if relayProblem && !code.isEmpty { enteringCode = true }
            reloadDeviceRelay()
        case .alreadyPaired:
            pendingLink = nil
            code = ""
            relayText = ""
            phase = .done
        }
    }

    /// Deposits the confirmation, then waits for the other device to confirm too.
    private func finish(importHistory: Bool) {
        guard !busy, let core = core() else { return }
        busy = true
        phase = .waiting
        attempt += 1
        let current = attempt
        job = Task {
            var failure: PairingFailure?
            do {
                try await core.confirmPairing(importHistory: importHistory)
            } catch let reason as PairingFailure {
                failure = reason
            } catch {
                failure = .unexpected
            }
            guard current == attempt else { return }
            busy = false
            awaiting = nil
            validUntil = nil
            if let failure {
                phase = .input
                error = PairingErrors.joinMessageKey(failure)
                // The session must not stay open at the relay; the relay it pointed at is back once it is closed.
                Task {
                    await core.cancelPairing()
                    await loadDeviceRelay()
                }
            } else if core.state.isPaired {
                relayText = ""
                phase = .done
            } else {
                // A confirmation without a pairing in progress does nothing.
                phase = .input
                error = "pair_error_unexpected"
            }
        }
    }

    private func abort(error: String?) {
        attempt += 1
        job?.cancel()
        job = nil
        awaiting = nil
        validUntil = nil
        busy = false
        phase = .input
        self.error = error
        guard let core = core() else { return }
        // Outside the screen: the pairing is cancelled even if the screen goes away; the relay the session pointed at is
        // back once it is (read after the cancel).
        Task {
            await core.cancelPairing()
            await loadDeviceRelay()
        }
    }
}
