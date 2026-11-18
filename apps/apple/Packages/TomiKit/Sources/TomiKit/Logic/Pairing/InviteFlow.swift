import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/pairing/{InviteFlow,InviteViewModel}.kt (`InviteUiState`).

/// What the "Ajouter un appareil" screen shows.
public enum InviteState: Equatable, Sendable {
    /// The channel is being opened at the relay.
    case starting
    /// The code and the QR code are on screen until `deadline` (monotonic clock). `notice`: why the code was renewed
    /// (a text key), or nil.
    case showCode(code: String, link: String, deadline: ContinuousClock.Instant, notice: String?)
    /// The new device said hello: the user compares the verification code and decides.
    case verify(peer: PairingPeer, verificationCode: String)
    /// "Ajouter" was pressed: waiting for the person on the new device to confirm too.
    case adding(peer: PairingPeer)
    case done(peer: PairingPeer, historyAnnounced: Bool)
    case failed(PairingFailure)
}

/// The course of "Ajouter un appareil" (spec/pairing-v1.md 7.2, 4.2, 11): a code is shown and renewed, the new device
/// says hello, the user decides, the device is added. The screen runs it and shows the states it publishes.
///
/// The code is renewed by itself while nothing happens (the relay drops an unused code after about two minutes) and
/// when the other device gave up or typed a wrong code, up to `maxRenewals` times in a row: then the user decides (the
/// relay limits how many channels a device may open, and a screen left open must not keep opening them). A refusal
/// ("Ils sont différents") also makes a new code; it is the user's own decision and does not count.
///
/// One run per instance: once aborted, a flow stays aborted (an abort that comes before the run starts is kept), and a
/// new attempt takes a new flow.
@MainActor
public final class InviteFlow {
    public static let maxRenewals = 5

    private let core: any TomiCore
    private let publish: (InviteState) -> Void
    private let now: () -> ContinuousClock.Instant
    private var session: (any InviteSession)?
    private var decision: CheckedContinuation<Bool, Never>?
    private var renewals = 0
    private var aborted = false

    public init(
        core: any TomiCore,
        now: @escaping () -> ContinuousClock.Instant = { ContinuousClock.now },
        publish: @escaping (InviteState) -> Void
    ) {
        self.core = core
        self.now = now
        self.publish = publish
    }

    /// Runs until the device is added or something ends the invitation; returns then (the last state is published).
    public func run() async {
        var notice: String?
        while !aborted {
            publish(.starting)
            let invitation: any InviteSession
            do {
                invitation = try await core.startInvitation()
            } catch {
                if !aborted { publish(.failed(Self.reason(error))) }
                return
            }
            guard !aborted else {
                Self.discard(invitation)
                return
            }
            session = invitation
            guard let code = invitation.code, let link = invitation.link else {
                close()
                publish(.failed(.unexpected))
                return
            }
            publish(.showCode(code: code, link: link, deadline: now().advanced(by: .milliseconds(invitation.expiresInMs())), notice: notice))
            notice = nil

            let prompt: InvitePrompt
            do {
                prompt = try await invitation.awaitPeer()
            } catch {
                // The failed session released itself (or the user left and closed it).
                session = nil
                guard !aborted else { return }
                let reason = Self.reason(error)
                if reason == .expired || reason == .wrongCode || reason == .cancelled {
                    // The code ran out, or the other device gave up or typed a wrong code: a new code.
                    renewals += 1
                    if renewals > Self.maxRenewals {
                        publish(.failed(reason))
                        return
                    }
                    notice = reason == .expired ? nil : "invite_notice_failed"
                    continue
                }
                publish(.failed(reason))
                return
            }
            guard !aborted else { return }

            renewals = 0
            publish(.verify(peer: prompt.peer, verificationCode: prompt.verificationCode))
            let add = await withCheckedContinuation { decision = $0 }
            decision = nil
            if aborted {
                close()
                return
            }
            if !add {
                // "Ils sont différents": nothing was given to anyone. The channel is closed and a new code is made.
                close()
                notice = "invite_notice_refused"
                continue
            }

            publish(.adding(peer: prompt.peer))
            do {
                try await invitation.confirm()
                let added = try await invitation.awaitDone()
                session = nil
                guard !aborted else { return }
                publish(.done(peer: prompt.peer, historyAnnounced: added.historyAnnounced))
                // The export of the history the new device asked for starts at the next sync; offline, later.
                try? await core.sync(pushToken: nil)
            } catch {
                session = nil
                if !aborted { publish(.failed(Self.reason(error))) }
            }
            return
        }
    }

    /// "Ajouter": the codes are the same. This alone gives the new device nothing: both people must confirm.
    public func add() {
        decision?.resume(returning: true)
        decision = nil
    }

    /// "Ils sont différents", or back from the comparison.
    public func refuse() {
        decision?.resume(returning: false)
        decision = nil
    }

    /// The user leaves: nothing is waited for any more, no new code is made, and the open invitation is closed.
    public func abort() {
        aborted = true
        decision?.resume(returning: false)
        decision = nil
        close()
    }

    /// Closes the open invitation at the relay, without waiting.
    private func close() {
        guard let open = session else { return }
        session = nil
        Self.discard(open)
    }

    private static func discard(_ open: any InviteSession) {
        Task { await open.cancel() }
    }

    private static func reason(_ error: Error) -> PairingFailure {
        error as? PairingFailure ?? .unexpected
    }
}
