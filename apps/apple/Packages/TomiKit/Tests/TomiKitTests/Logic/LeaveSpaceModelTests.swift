import XCTest
@testable import TomiKit

// Ports of apps/android/app/src/test/java/eu/shaffe/tomi/ui/feed/LeaveSpaceViewModelTest.kt and
// core/DepartureRetrierTest.kt (the main cases).

@MainActor
private final class DepartingCore: TomiCore {
    let state = CoreState()
    let isDemo = true
    var calls: [String] = []
    /// What the next calls give, in order (`.failed` once they run out).
    var results: [DepartureResult] = []
    var eraseAllowed = false
    var eraseResult = EraseResult.erased
    /// When set, the next attempt waits until `release()`.
    var hold = false
    private var held: CheckedContinuation<Void, Never>?
    /// The attempt writes its marker only when it is released (the cancel that misses it).
    var markerOnRelease = false

    func release() {
        held?.resume()
        held = nil
    }

    private func run(_ name: String, kind: DepartureKind) async -> DepartureResult {
        calls.append(name)
        state.departureRunning = true
        if !markerOnRelease { state.departure = Departure(kind: kind, wiping: false, since: Date(), last: state.departure?.last) }
        if hold {
            hold = false
            await withCheckedContinuation { held = $0 }
        }
        if markerOnRelease {
            // As the core: the marker is written, then the request is on the network for a while.
            state.departure = Departure(kind: kind, wiping: false, since: Date(), last: nil)
            markerOnRelease = false
            for _ in 0..<20 { await Task.yield() }
        }
        let result = results.isEmpty ? DepartureResult.failed : results.removeFirst()
        // A cancellation that came meanwhile wins: nothing is erased.
        let cancelled = state.departure == nil
        switch result {
        case .erased where !cancelled:
            state.departure = nil
            state.isPaired = false
        case let .pending(reason) where !cancelled:
            state.departure = Departure(kind: kind, wiping: false, since: Date(), last: reason)
        case .refused, .cancelled, .nothingPending:
            state.departure = nil
        default:
            break
        }
        state.departureRunning = false
        return cancelled && result != .failed ? .cancelled : result
    }

    func leaveSpace() async -> DepartureResult { await run("leave", kind: .leave) }
    func deleteSpace() async -> DepartureResult { await run("delete", kind: .deleteSpace) }
    func retryDeparture() async -> DepartureResult { await run("retry", kind: state.departure?.kind ?? .leave) }
    func continueDeparture() async -> DepartureResult {
        guard let kind = state.departure?.kind else {
            calls.append("continue")
            return .nothingPending
        }
        return await run("continue", kind: kind)
    }
    func cancelDeparture() async -> Bool {
        calls.append("cancel")
        guard let departure = state.departure, !departure.wiping else { return false }
        state.departure = nil
        return true
    }
    func canEraseWithoutProof() async -> Bool {
        calls.append("canErase")
        return eraseAllowed
    }
    func eraseWithoutProof() async -> EraseResult {
        calls.append("erase")
        if eraseResult == .erased {
            state.departure = nil
            state.isPaired = false
        }
        return eraseResult
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
    func startInvitation() async throws -> any InviteSession { fatalError("unused") }
    func resumePairing() async -> ResumeOutcome { fatalError("unused") }
    func simulateDeparture(_ scenario: DemoDepartureScenario) async { fatalError("unused") }
    func isValidDeviceName(_ name: String) -> Bool { fatalError("unused") }
    func simulateIncoming(_ kind: SimulatedIncoming) async throws { fatalError("unused") }
    func purgePlaintextCache() async throws { fatalError("unused") }
    func enterForeground() { fatalError("unused") }
    func enterBackground() { fatalError("unused") }
}

@MainActor
private final class FakeFollowUp: DepartureFollowUp {
    var refusal: DepartureRefusal?
    var onRefusal: (() -> Void)?
    var kicks = 0
    func kick() { kicks += 1 }
    func consumeRefusal() { refusal = nil }
}

/// The arm delay is real but short; the polls only yield.
private let fastSleep: (Duration) async throws -> Void = { duration in
    if duration >= .seconds(1) {
        try await Task.sleep(for: .milliseconds(30))
    } else {
        await Task.yield()
    }
}

@MainActor
final class LeaveSpaceModelTests: XCTestCase {
    private var core: DepartingCore!
    private var followUp: FakeFollowUp!
    private var model: LeaveSpaceModel!
    private var events: [LeaveEvent] = []

    override func setUp() async throws {
        core = DepartingCore()
        core.state.isPaired = true
        followUp = FakeFollowUp()
        events = []
        model = LeaveSpaceModel(core: core, followUp: followUp, sleep: fastSleep)
        model.onEvent = { [unowned self] in events.append($0) }
    }

    private func settle() async {
        for _ in 0..<40 { await Task.yield() }
    }

    private func armed() async {
        try? await Task.sleep(for: .milliseconds(120))
        await settle()
    }

    private var confirmDelete: ConfirmDelete? {
        if case let .confirmDelete(state) = model.dialog { return state }
        return nil
    }

    private var erase: EraseWithoutProof? {
        if case let .eraseWithoutProof(state) = model.dialog { return state }
        return nil
    }

    func testANetworkErrorLeavesTheBannerWithoutEraseAnywayAndStartsTheRetries() async {
        core.results = [.pending(.network)]
        XCTAssertTrue(model.canOfferLeave)
        model.open()
        model.confirmLeave()
        XCTAssertEqual(model.dialog, .confirmLeave(ConfirmLeave(busy: true)))
        await settle()
        XCTAssertEqual(model.dialog, .hidden)
        XCTAssertEqual(model.banner, .pending(kind: .leave, last: .network, running: false, authenticating: false))
        XCTAssertFalse(model.banner.offersEraseAnyway)
        XCTAssertEqual(followUp.kicks, 1)
        XCTAssertFalse(model.canOfferLeave, "the banner handles a pending departure")
    }

    func testNotFoundLeavesTheBannerWithEraseAnywayAndNoRetry() async {
        core.results = [.pending(.notFound)]
        model.open()
        model.confirmLeave()
        await settle()
        XCTAssertTrue(model.banner.offersEraseAnyway)
        XCTAssertEqual(followUp.kicks, 0)
    }

    func testTheLastDeviceIsAskedToDeleteTheSpaceWithAButtonArmedAfterTwoSeconds() async {
        core.results = [.refused(.lastDevice)]
        model.open()
        model.confirmLeave()
        await settle()
        XCTAssertEqual(confirmDelete?.armed, false)
        XCTAssertFalse(model.beginDelete(), "not before the delay")
        await armed()
        XCTAssertEqual(confirmDelete?.canConfirm, true)
    }

    func testWithoutStrongBiometricsNothingIsDeletedAndTheUserIsTold() async {
        core.results = [.refused(.lastDevice)]
        model.open()
        model.confirmLeave()
        await settle()
        await armed()
        XCTAssertTrue(model.beginDelete())
        XCTAssertEqual(confirmDelete?.authenticating, true)
        model.authEnded(unavailable: true)
        XCTAssertEqual(confirmDelete?.biometricUnavailable, true)
        XCTAssertEqual(core.calls, ["leave"], "nothing sent")
    }

    func testTheSpaceIsDeletedAfterTheStrongCheck() async {
        core.results = [.refused(.lastDevice), .erased]
        model.open()
        model.confirmLeave()
        await settle()
        await armed()
        XCTAssertTrue(model.beginDelete())
        model.deleteAuthenticated()
        XCTAssertEqual(confirmDelete?.busy, true)
        await settle()
        XCTAssertEqual(core.calls, ["leave", "delete"])
        XCTAssertEqual(model.dialog, .hidden)
        XCTAssertFalse(core.state.isPaired)
    }

    func testARelayTooOldOffersEraseAnyway() async {
        core.results = [.refused(.lastDevice), .refused(.relayTooOld)]
        model.open()
        model.confirmLeave()
        await settle()
        await armed()
        _ = model.beginDelete()
        model.deleteAuthenticated()
        await settle()
        XCTAssertEqual(confirmDelete?.error, .relayTooOld)
        core.eraseAllowed = true
        model.openErase()
        await settle()
        XCTAssertEqual(erase?.checking, false)
        XCTAssertEqual(erase?.notAllowed, false)
    }

    func testAnotherRefusalIsShownAsAFailureNeverAsADeparture() async {
        core.results = [.refused(.unexpected)]
        model.open()
        model.confirmLeave()
        await settle()
        XCTAssertEqual(model.dialog, .confirmLeave(ConfirmLeave(error: .unexpected)))
        XCTAssertTrue(core.state.isPaired)
    }

    func testDismissIsIgnoredWhileTheAttemptRunsAndAnnulerCancelsIt() async {
        core.results = [.pending(.network)]
        core.hold = true
        model.open()
        model.confirmLeave()
        await settle()
        model.dismiss()
        XCTAssertEqual(model.dialog, .confirmLeave(ConfirmLeave(busy: true)))
        model.cancelRunning()
        XCTAssertEqual(model.dialog, .hidden)
        await settle()
        XCTAssertEqual(events, [.cancelled(.leave)])
        XCTAssertEqual(model.banner, .cancelling, "the attempt that left runs to its end")
        core.release()
        await settle()
        XCTAssertEqual(model.banner, .none)
        XCTAssertTrue(core.state.isPaired, "nothing erased")
    }

    func testACancellationThatMissesTheMarkerOfARunningAttemptWaitsForItAndCancelsAgain() async {
        core.results = [.erased]
        core.hold = true
        core.markerOnRelease = true
        model.open()
        model.confirmLeave()
        await settle()
        model.cancelRunning()
        await settle()
        XCTAssertGreaterThanOrEqual(core.calls.filter { $0 == "cancel" }.count, 1)
        core.release()
        await settle()
        XCTAssertGreaterThanOrEqual(core.calls.filter { $0 == "cancel" }.count, 2)
        XCTAssertEqual(events, [.cancelled(.leave)])
        XCTAssertTrue(core.state.isPaired, "the second cancel won: nothing erased")
    }

    func testRetryingADeletionAsksForStrongBiometricsEveryTimeAndSendsNothingBeforeThem() async {
        core.state.departure = Departure(kind: .deleteSpace, wiping: false, since: nil, last: .network)
        core.results = [.pending(.network)]
        XCTAssertTrue(model.beginRetry())
        XCTAssertEqual(model.banner, .pending(kind: .deleteSpace, last: .network, running: false, authenticating: true))
        XCTAssertTrue(core.calls.isEmpty)
        model.retryAuthenticated()
        await settle()
        XCTAssertEqual(core.calls, ["retry"])
        XCTAssertEqual(followUp.kicks, 1)
        XCTAssertTrue(model.beginRetry(), "asked again the next time")
        model.retryAuthEnded(unavailable: true)
        XCTAssertEqual(events, [.biometricUnavailable])
    }

    func testRetryingALeaveIsSentAtOnce() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .server)
        core.results = [.erased]
        XCTAssertFalse(model.beginRetry())
        await settle()
        XCTAssertEqual(core.calls, ["retry"])
        XCTAssertFalse(core.state.isPaired)
    }

    func testAnAutomaticRefusalWaitsWhileADialogIsOpenAndIsShownWhenItCloses() {
        model.open()
        followUp.refusal = DepartureRefusal(kind: .leave, reason: .unexpected)
        followUp.onRefusal?()
        XCTAssertEqual(model.dialog, .confirmLeave(ConfirmLeave()))
        model.dismiss()
        XCTAssertEqual(model.dialog, .confirmLeave(ConfirmLeave(error: .unexpected)))
        XCTAssertNil(followUp.refusal)
    }

    func testTheBannerShowsARevokedDeviceAndAMissingSpace() {
        core.state.spaceState = .revoked
        XCTAssertEqual(model.banner, .revoked)
        core.state.spaceState = .notFound
        XCTAssertEqual(model.banner, .spaceNotFound)
        core.state.departure = Departure(kind: .leave, wiping: true, since: nil, last: nil)
        XCTAssertEqual(model.banner, .wiping)
    }

    func testEraseAnywayNeedsTheBoxAndTwoSecondsAndReadsTheCoreAgainBeforeThePrompt() async {
        core.eraseAllowed = true
        model.openErase()
        await settle()
        XCTAssertEqual(erase?.canConfirm, false)
        await armed()
        XCTAssertEqual(erase?.canConfirm, false, "the box is not checked")
        model.setEraseChecked(true)
        XCTAssertEqual(erase?.canConfirm, true)
        core.eraseAllowed = false
        let prompt = await model.beginErase()
        XCTAssertFalse(prompt, "the core no longer allows it: no prompt")
        XCTAssertEqual(erase?.notAllowed, true)
    }

    func testEraseAnywayErasesAfterTheStrongCheck() async {
        core.eraseAllowed = true
        model.openErase()
        await armed()
        model.setEraseChecked(true)
        let prompt = await model.beginErase()
        XCTAssertTrue(prompt)
        model.eraseAuthenticated()
        await settle()
        XCTAssertEqual(core.calls.last, "erase")
        XCTAssertEqual(model.dialog, .hidden)
        XCTAssertFalse(core.state.isPaired)
    }

    func testARefusalOfTheCoreAfterTheBiometricCheckErasesNothingAndSaysWhy() async {
        core.eraseAllowed = true
        core.eraseResult = .notAllowed
        model.openErase()
        await armed()
        model.setEraseChecked(true)
        _ = await model.beginErase()
        model.eraseAuthenticated()
        await settle()
        XCTAssertEqual(erase?.expired, true)
        XCTAssertEqual(erase?.notAllowed, true)
        XCTAssertTrue(core.state.isPaired)
    }

    func testEraseAnywayAsksTheRelayWhenNotAllowedYet() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        core.results = [.pending(.notFound)]
        model.openErase()
        await settle()
        XCTAssertEqual(core.calls, ["canErase", "continue", "canErase"])
        XCTAssertEqual(erase?.notAllowed, true, "still not allowed by the core")
    }

    func testEraseAnywayWaitsWhileAnAttemptRuns() async {
        core.eraseAllowed = true
        model.openErase()
        await armed()
        model.setEraseChecked(true)
        _ = await model.beginErase()
        core.state.departureRunning = true
        model.eraseAuthenticated()
        await settle()
        XCTAssertEqual(erase?.waiting, true)
        XCTAssertFalse(core.calls.contains("erase"))
        core.state.departureRunning = false
        try? await Task.sleep(for: .milliseconds(20))
        await settle()
        XCTAssertEqual(core.calls.last, "erase")
    }
}

@MainActor
final class DepartureRetrierTests: XCTestCase {
    private var core: DepartingCore!
    private var slept: [Duration] = []

    override func setUp() async throws {
        core = DepartingCore()
        core.state.isPaired = true
        slept = []
    }

    private func retrier(random: Double = 0, now: @escaping () -> ContinuousClock.Instant = { ContinuousClock.now }) -> DepartureRetrier {
        DepartureRetrier(core: core, now: now, random: { random }) { [unowned self] duration in
            slept.append(duration)
            await Task.yield()
        }
    }

    private func settle() async {
        for _ in 0..<80 { await Task.yield() }
    }

    func testTheSeriesFollowsTheDelaysAndStopsAtTheFirstResultThatIsNotTransient() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        core.results = [.pending(.network), .pending(.server), .erased]
        let retrier = retrier()
        retrier.kick()
        await settle()
        XCTAssertEqual(slept, [.seconds(2), .seconds(5), .seconds(15)])
        XCTAssertEqual(core.calls, ["continue", "continue", "continue"])
        XCTAssertFalse(retrier.seriesRunning)
    }

    func testTheSeriesStopsWhenTheDepartureIsGone() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        let retrier = retrier()
        retrier.kick()
        core.state.departure = nil
        await settle()
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testTheJitterAddsAtMostTwentyPercent() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        core.results = [.erased]
        let retrier = retrier(random: 0.999)
        retrier.kick()
        await settle()
        XCTAssertGreaterThan(slept[0], .seconds(2.39))
        XCTAssertLessThanOrEqual(slept[0], .seconds(2.4))
    }

    func testAtStartOneAttemptThenTheSeriesIfTransient() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: nil)
        core.results = [.pending(.network), .erased]
        let retrier = retrier()
        let first = await retrier.settleAtStart()
        XCTAssertEqual(first, .pending(.network))
        await settle()
        XCTAssertEqual(core.calls, ["continue", "continue"])
    }

    func testAnAutomaticRefusalIsKeptForTheScreens() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: nil)
        core.results = [.refused(.lastDevice)]
        let retrier = retrier()
        var told = false
        retrier.onRefusal = { told = true }
        await retrier.settleAtStart()
        XCTAssertEqual(retrier.refusal, DepartureRefusal(kind: .leave, reason: .lastDevice))
        XCTAssertTrue(told)
        retrier.consumeRefusal()
        XCTAssertNil(retrier.refusal)
    }

    func testComingBackTriesAtMostOnceAMinute() async {
        var now = ContinuousClock.now
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        core.results = [.pending(.network), .pending(.network), .pending(.network)]
        let retrier = retrier(now: { now })
        retrier.comeBack()
        await settle()
        retrier.comeBack()
        await settle()
        XCTAssertEqual(core.calls.count, 1)
        now = now.advanced(by: .seconds(61))
        retrier.comeBack()
        await settle()
        XCTAssertEqual(core.calls.count, 2)
    }

    func testStopEndsTheSeriesAndForgetsTheRefusal() async {
        core.state.departure = Departure(kind: .leave, wiping: false, since: nil, last: .network)
        let retrier = DepartureRetrier(core: core) { _ in try await Task.sleep(for: .seconds(60)) }
        retrier.kick()
        XCTAssertTrue(retrier.seriesRunning)
        retrier.stop()
        await settle()
        XCTAssertFalse(retrier.seriesRunning)
        XCTAssertNil(retrier.refusal)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testNothingIsTriedWithoutAPendingDeparture() async {
        let retrier = retrier()
        retrier.comeBack()
        await settle()
        XCTAssertTrue(core.calls.isEmpty)
    }
}
