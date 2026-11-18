import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/pairing/InviteFlowTest.kt (the "Ajouter un
// appareil" flow, without the screen).

private let tablet = PairingPeer(name: "Tablette", platform: .android)

/// A scripted invitation: what the new device does, and whether the screen cancelled it.
private final class FakeInviteSession: InviteSession, @unchecked Sendable {
    let code: String? = "4821-crayon-renard"
    let link: String? = "https://relay.example.org/l/pair#4821-crayon-renard"
    let peer: Result<InvitePrompt, PairingFailure>
    let done: Result<InvitedDevice, PairingFailure>
    private(set) var cancelled = false
    private(set) var confirmed = false

    init(peer: Result<InvitePrompt, PairingFailure> = .success(InvitePrompt(peer: tablet, verificationCode: "482913")),
         done: Result<InvitedDevice, PairingFailure> = .success(InvitedDevice(deviceId: "d", name: "Tablette", platform: .android, historyAnnounced: true))) {
        self.peer = peer
        self.done = done
    }

    func expiresInMs() -> Int64 { 120_000 }
    func awaitPeer() async throws -> InvitePrompt { try peer.get() }
    func confirm() async throws { confirmed = true }
    func awaitDone() async throws -> InvitedDevice { try done.get() }
    func cancel() async { cancelled = true }
}

@MainActor
private final class InvitingCore: TomiCore {
    let state = CoreState()
    let isDemo = true
    var calls: [String] = []

    /// The sessions the next invitations get, in order; an error once they run out.
    var sessions: [FakeInviteSession] = []
    var startError: PairingFailure?

    func startInvitation() async throws -> any InviteSession {
        calls.append("startInvitation")
        if let startError { throw startError }
        guard !sessions.isEmpty else { throw PairingFailure.unexpected }
        return sessions.removeFirst()
    }

    func sync(pushToken: String?) async throws { calls.append("sync") }
    func pin(id: String) async throws { fatalError("unused") }
    func unpin(id: String) async throws { fatalError("unused") }
    func archive(id: String) async throws { fatalError("unused") }
    func unarchive(id: String) async throws { fatalError("unused") }

    func getItem(id: String) async throws -> TomiItem? { fatalError("unused") }
    func enqueue(_ draft: OutgoingDraft) async throws { fatalError("unused") }
    func dispatch(id: String) async throws -> DispatchResult { fatalError("unused") }
    func cancelIfQueued(id: String) async throws -> Bool { fatalError("unused") }
    func deleteLocal(id: String) async throws { fatalError("unused") }
    func deleteEverywhere(id: String) async throws { fatalError("unused") }
    func markRead(id: String) async throws { fatalError("unused") }
    func attachmentPath(itemId: String, slot: Int) async throws -> String? { fatalError("unused") }
    func deleteAttachment(itemId: String, slot: Int, everywhere: Bool) async throws -> AttachmentRemoval { fatalError("unused") }
    func supportsAttachmentRemoval() async -> Bool { fatalError("unused") }
    func renameDevice(id: String, name: String) async throws { fatalError("unused") }
    func revokeDevice(id: String) async throws { fatalError("unused") }
    func currentRelay() async -> String? { fatalError("unused") }
    func pairWithCode(code: String, relayUrl: String?) async throws -> PairingResult { fatalError("unused") }
    func confirmPairing(importHistory: Bool) async throws { fatalError("unused") }
    func cancelPairing() async { fatalError("unused") }
    func createSpace(installToken: String, deviceName: String, relayUrl: String?) async throws { fatalError("unused") }
    func resumePairing() async -> ResumeOutcome { fatalError("unused") }
    func simulateDeparture(_ scenario: DemoDepartureScenario) async { fatalError("unused") }
    func leaveSpace() async -> DepartureResult { fatalError("unused") }
    func deleteSpace() async -> DepartureResult { fatalError("unused") }
    func retryDeparture() async -> DepartureResult { fatalError("unused") }
    func continueDeparture() async -> DepartureResult { fatalError("unused") }
    func cancelDeparture() async -> Bool { fatalError("unused") }
    func canEraseWithoutProof() async -> Bool { fatalError("unused") }
    func eraseWithoutProof() async -> EraseResult { fatalError("unused") }
    func isValidDeviceName(_ name: String) -> Bool { fatalError("unused") }
    func simulateIncoming(_ kind: SimulatedIncoming) async throws { fatalError("unused") }
    func purgePlaintextCache() async throws { fatalError("unused") }
    func enterForeground() { fatalError("unused") }
    func enterBackground() { fatalError("unused") }
}

@MainActor
final class InviteFlowTests: XCTestCase {
    private var core: InvitingCore!
    private var states: [InviteState] = []
    private var flow: InviteFlow!
    private var running: Task<Void, Never>?

    override func setUp() async throws {
        core = InvitingCore()
        states = []
        flow = InviteFlow(core: core) { [unowned self] in states.append($0) }
    }

    private func start() {
        running = Task { await flow.run() }
    }

    private func settle() async {
        for _ in 0..<60 { await Task.yield() }
    }

    private var last: InviteState? { states.last }

    private func isVerify(_ state: InviteState?) -> Bool {
        if case .verify = state { return true }
        return false
    }

    private func notices() -> [String?] {
        states.compactMap { if case let .showCode(_, _, _, notice) = $0 { return notice } else { return nil } }
    }

    func testTheCodeThenTheVerificationThenTheDeviceIsAdded() async {
        let session = FakeInviteSession()
        core.sessions = [session]
        start()
        await settle()
        XCTAssertEqual(states.first, .starting)
        XCTAssertTrue(states.contains { if case .showCode(code: "4821-crayon-renard", _, _, nil) = $0 { return true } else { return false } })
        XCTAssertEqual(last, .verify(peer: tablet, verificationCode: "482913"))
        flow.add()
        await running?.value
        XCTAssertTrue(session.confirmed)
        XCTAssertEqual(last, .done(peer: tablet, historyAnnounced: true))
        XCTAssertEqual(core.calls, ["startInvitation", "sync"], "the export of the history starts at the next sync")
        XCTAssertFalse(session.cancelled)
    }

    func testAnExpiredCodeIsRenewedQuietlyAndAWrongCodeWithANotice() async {
        core.sessions = [
            FakeInviteSession(peer: .failure(.expired)),
            FakeInviteSession(peer: .failure(.wrongCode)),
            FakeInviteSession(),
        ]
        start()
        await settle()
        XCTAssertTrue(isVerify(last))
        XCTAssertEqual(notices(), [nil, nil, "invite_notice_failed"])
        flow.abort()
        await running?.value
    }

    func testRenewalsStopAfterFiveInARow() async {
        core.sessions = (0...InviteFlow.maxRenewals).map { _ in FakeInviteSession(peer: .failure(.cancelled)) }
        start()
        await running?.value
        XCTAssertEqual(last, .failed(.cancelled))
        XCTAssertEqual(core.calls.count, InviteFlow.maxRenewals + 1)
    }

    func testDifferentCodesCloseTheChannelAndMakeANewCode() async {
        let first = FakeInviteSession()
        core.sessions = [first, FakeInviteSession()]
        start()
        await settle()
        flow.refuse()
        await settle()
        XCTAssertTrue(first.cancelled)
        XCTAssertFalse(first.confirmed)
        XCTAssertEqual(notices().last, "invite_notice_refused")
        XCTAssertTrue(isVerify(last))
        flow.abort()
        await running?.value
    }

    func testLeavingWhileComparingClosesTheChannel() async {
        let session = FakeInviteSession()
        core.sessions = [session]
        start()
        await settle()
        let count = states.count
        flow.abort()
        await running?.value
        await settle()
        XCTAssertTrue(session.cancelled)
        XCTAssertFalse(session.confirmed)
        XCTAssertEqual(states.count, count, "nothing is shown after leaving")
    }

    func testAnAbortBeforeTheRunStartsIsKept() async {
        core.sessions = [FakeInviteSession()]
        flow.abort()
        await flow.run()
        XCTAssertTrue(core.calls.isEmpty, "no channel is opened for a screen already gone")
        XCTAssertTrue(states.isEmpty)
    }

    func testAnInvitationThatCannotStartFails() async {
        core.startError = .spaceFull
        start()
        await running?.value
        XCTAssertEqual(last, .failed(.spaceFull))
        XCTAssertEqual(PairingErrors.inviteMessageKey(.spaceFull), "pair_error_space_full")
        XCTAssertEqual(PairingErrors.inviteMessageKey(.anotherPairingOpen), "invite_error_busy")
    }

    func testAFailureWhileAddingIsShown() async {
        core.sessions = [FakeInviteSession(done: .failure(.cancelled))]
        start()
        await settle()
        flow.add()
        await running?.value
        XCTAssertEqual(last, .failed(.cancelled))
    }
}
