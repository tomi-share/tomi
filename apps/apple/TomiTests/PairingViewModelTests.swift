import Foundation
import TomiKit
import XCTest
@testable import Tomi

/// "Rejoins ton espace" (Android `PairingViewModel`): the code checked before the core, the relay settled before the core,
/// the verification, the single history question, the wait for the other device, leaving at every step, and the messages
/// of each failure.
@MainActor
final class PairingViewModelTests: XCTestCase {
    private var core: PreviewCore!
    private static let deviceRelay = "https://relay.example.org"

    /// A model on a core whose device has `relay` (nil: a build without a default relay), already read by the model.
    private func makeModel(relay: String? = PairingViewModelTests.deviceRelay) async -> PairingViewModel {
        core = PreviewCore(items: [], localDevice: PreviewFixtures.phone)
        core.state.isPaired = false
        core.relay = relay
        let model = PairingViewModel(core: { [unowned self] in core }, reader: FakeLinkReader())
        await model.loadDeviceRelay()
        return model
    }

    private func settle() async {
        for _ in 0..<40 { await Task.yield() }
    }

    private func prompt(history: Int, relayAuthority: String = "relay.example.org") -> PairingPrompt {
        PairingPrompt(
            peer: PairingPeer(name: "PC-Bureau", platform: .windows), verificationCode: "482913",
            historyItemCount: history, historyBytes: 4_000_000, memberCount: 2, expiresInMs: 120_000,
            relayAuthority: relayAuthority
        )
    }

    func testWhatIsClearlyNotACodeNeverReachesTheCore() async {
        let model = await makeModel()
        model.onCodeChange("   ")
        model.joinWithCode()
        XCTAssertEqual(model.error, "pair_error_code_format")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testAnUnknownQRCodeIsRefused() async {
        let model = await makeModel()
        model.onQRScanned("https://example.com")
        XCTAssertEqual(model.error, "pair_error_qr_unknown")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testCodesThenHistoryThenWaitingThenDone() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 23))
        model.onCodeChange(" 7-crayon-lune ")
        model.joinWithCode()
        XCTAssertEqual(model.phase, .working)
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:7-crayon-lune"], "the relay of the device is the core's own: none is passed")
        XCTAssertEqual(model.phase, .verify)
        XCTAssertEqual(model.code, "", "the code is single-use: gone once claimed")
        XCTAssertGreaterThan(model.remainingMs(at: ContinuousClock.now), 110_000)
        model.codesMatch()
        XCTAssertEqual(model.phase, .history)
        XCTAssertTrue(model.importHistory, "« Oui » is preselected")
        model.importHistory = false
        let gate = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        core.confirmGate = gate
        model.confirmHistory()
        XCTAssertEqual(model.phase, .waiting)
        await settle()
        XCTAssertEqual(core.calls.last, "confirmPairing:false")
        XCTAssertEqual(model.phase, .waiting, "the other device has not confirmed yet")
        gate.cancel()
        await gate.value
        await settle()
        XCTAssertEqual(model.phase, .done)
    }

    func testWithoutHistoryTheConfirmationGoesStraightOn() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 0))
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        model.codesMatch()
        await settle()
        XCTAssertEqual(core.calls.last, "confirmPairing:false")
        XCTAssertEqual(model.phase, .done)
    }

    func testDifferentCodesCancelThePairing() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 3))
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        model.codesDiffer()
        await settle()
        XCTAssertEqual(model.phase, .input)
        XCTAssertEqual(model.error, "pair_verify_mismatch")
        XCTAssertNil(model.awaiting)
        XCTAssertEqual(core.calls.last, "cancelPairing")
    }

    func testLeavingDuringTheWaitCancelsAndIgnoresTheLateAnswer() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 0))
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        let gate = Task<Void, Never> { try? await Task.sleep(for: .seconds(60)) }
        core.confirmGate = gate
        model.codesMatch()
        await settle()
        XCTAssertEqual(model.phase, .waiting)
        model.leave()
        XCTAssertEqual(model.phase, .input)
        XCTAssertNil(model.error)
        await settle()
        XCTAssertEqual(core.calls.last, "cancelPairing")
        // The confirmation comes back afterwards: ignored, the screen stays where the user went.
        gate.cancel()
        await gate.value
        await settle()
        XCTAssertEqual(model.phase, .input)
    }

    func testLeavingWhileConnectingIgnoresTheAnswer() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 0))
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        model.leave()
        await settle()
        XCTAssertEqual(model.phase, .input)
        XCTAssertNil(model.awaiting)
        // The pairing the core opened after the cancel is cancelled too.
        XCTAssertEqual(core.calls.filter { $0 == "cancelPairing" }.count, 2)
        XCTAssertEqual(core.calls.last, "cancelPairing")
    }

    func testEachFailureHasItsMessageAndKeepsTheCodeOnlyWhenWorthIt() async {
        let model = await makeModel()
        for (failure, key, keepsCode) in [
            (PairingFailure.invalidCode, "pair_error_invalid", true), (.network, "pair_error_network", true),
            (.wrongCode, "pair_error_wrong_code", false), (.expired, "pair_error_expired", false),
            (.taken, "pair_error_taken", false), (.interrupted, "pair_error_interrupted", false),
            (.cancelled, "pair_error_cancelled", false), (.spaceFull, "pair_error_space_full", false),
            (.unsupported, "ios_pair_error_unsupported", false),
            // A relay to correct keeps the code, on the typing step where the field is.
            (.relayNotConfigured, "pair_error_relay_required", true), (.invalidRelay, "pair_error_relay_invalid", true),
        ] {
            core.pairingResult = .failure(failure)
            model.reset()
            model.onCodeChange("7-crayon-lune")
            model.joinWithCode()
            await settle()
            XCTAssertEqual(model.phase, .input)
            XCTAssertEqual(model.error, key, "\(failure)")
            XCTAssertEqual(model.code.isEmpty, !keepsCode, "\(failure)")
            let relayProblem = failure == .relayNotConfigured || failure == .invalidRelay
            XCTAssertEqual(model.enteringCode, relayProblem, "\(failure)")
        }
    }

    func testAFailedConfirmationShowsItsReasonAndCancels() async {
        let model = await makeModel()
        core.pairingResult = .awaitingVerification(prompt(history: 0))
        core.confirmError = PairingFailure.cancelled
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        model.codesMatch()
        await settle()
        XCTAssertEqual(model.phase, .input)
        XCTAssertEqual(model.error, "pair_error_cancelled")
        XCTAssertEqual(core.calls.last, "cancelPairing")
    }

    func testAnOfferedLinkIsKeptOnlyAfterANetworkFailure() async {
        let model = await makeModel()
        model.offerLink(relayUrl: "https://relay.example.org", code: "4821-crayon-renard")
        XCTAssertTrue(core.calls.isEmpty, "a link is never followed by itself")
        core.pairingResult = .failure(.network)
        model.joinWithPendingLink()
        await settle()
        XCTAssertNotNil(model.pendingLink)
        core.pairingResult = .failure(.expired)
        model.joinWithPendingLink()
        await settle()
        XCTAssertNil(model.pendingLink)
    }

    func testAlreadyPairedEndsTheScreen() async {
        let model = await makeModel()
        core.pairingResult = .alreadyPaired
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        XCTAssertEqual(model.phase, .done)
    }

    func testAPairingSettledAtStartUpEndsTheFirstStep() async {
        let model = await makeModel()
        model.offerLink(relayUrl: "https://relay.example.org", code: "4821-crayon-renard")
        model.onCodeChange("7-crayon-lune")
        model.pairedChanged(true)
        XCTAssertEqual(model.phase, .done)
        XCTAssertNil(model.pendingLink, "a single-use secret, nothing left to use it for")
        XCTAssertEqual(model.code, "")
        XCTAssertEqual(model.relayText, "")
    }

    func testTheDemoEntryJoinsTheDemoInOneStep() async {
        let model = await makeModel()
        core.canEnterDemo = true
        XCTAssertTrue(model.canEnterDemo)
        let entered = await model.enterBundledDemo()
        XCTAssertTrue(entered)
        XCTAssertEqual(core.calls, ["enterDemo"])
        XCTAssertTrue(core.state.isPaired)
    }

    func testAFailedDemoEntryComesBackToTheInput() async {
        let model = await makeModel()
        core.canEnterDemo = true
        core.enterDemoError = TomiCoreError(.internal, reason: "x")
        let entered = await model.enterBundledDemo()
        XCTAssertFalse(entered)
        XCTAssertEqual(model.phase, .input)
        XCTAssertEqual(model.error, "pair_error_unexpected")
    }

    // MARK: The relay (spec/pairing-v1.md 4.4 rules 6 and 7)

    func testTheFieldIsRequiredOnlyWhenTheDeviceHasNoRelay() async {
        let withRelay = await makeModel()
        XCTAssertFalse(withRelay.relayFieldRequired)
        XCTAssertEqual(withRelay.deviceRelayAuthority, "relay.example.org")
        let without = await makeModel(relay: nil)
        XCTAssertTrue(without.relayFieldRequired)
        XCTAssertNil(without.deviceRelayAuthority)
    }

    func testWithoutAnyRelayTheCodeStopsOnTheTypingStepAndNothingReachesTheCore() async {
        let model = await makeModel(relay: nil)
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        XCTAssertEqual(model.error, "pair_error_relay_required")
        XCTAssertTrue(model.enteringCode, "the field is there")
        XCTAssertEqual(model.code, "7-crayon-lune", "the code is kept for the retry")
        XCTAssertEqual(model.phase, .input)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testTheRelayOfTheFieldIsPassedWithTheCode() async {
        let model = await makeModel(relay: nil)
        core.pairingResult = .awaitingVerification(prompt(history: 0, relayAuthority: "other.example.net:8443"))
        model.onRelayChange("other.example.net:8443")
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:7-crayon-lune@https://other.example.net:8443"])
        // The verification screen gets the authority the core reports, in full.
        XCTAssertEqual(model.awaiting?.relayAuthority, "other.example.net:8443")
    }

    func testAnInvalidRelayIsRefusedBeforeTheCore() async {
        for device in [nil, Self.deviceRelay] as [String?] {
            let model = await makeModel(relay: device)
            model.onRelayChange("http://relay.example.org")
            model.onCodeChange("7-crayon-lune")
            model.joinWithCode()
            XCTAssertEqual(model.error, "pair_error_relay_invalid")
            XCTAssertTrue(core.calls.isEmpty)
        }
    }

    func testTheFieldWinsOverTheRelayOfTheDevice() async {
        let model = await makeModel()
        model.onRelayChange("other.example.net")
        model.onCodeChange("7-crayon-lune")
        model.joinWithCode()
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:7-crayon-lune@https://other.example.net"])
    }

    func testALinkOfAnotherRelayFillsTheFieldAndIsShownInFullBeforeJoining() async {
        let model = await makeModel()
        model.offerLink(relayUrl: "https://other.example.net:8443", code: "4821-crayon-renard")
        XCTAssertEqual(model.relayText, "https://other.example.net:8443")
        XCTAssertEqual(model.pendingLinkRelayAuthority, "other.example.net:8443")
        XCTAssertTrue(core.calls.isEmpty, "a link never starts the pairing by itself")
        model.joinWithPendingLink()
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:4821-crayon-renard@https://other.example.net:8443"])
    }

    func testALinkWithoutRelayUsesTheRelayOfTheDeviceOrAsksForOne() async {
        let withRelay = await makeModel()
        withRelay.offerLink(relayUrl: nil, code: "4821-crayon-renard")
        XCTAssertNil(withRelay.pendingLinkRelayAuthority)
        withRelay.joinWithPendingLink()
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:4821-crayon-renard"])

        let without = await makeModel(relay: nil)
        without.offerLink(relayUrl: nil, code: "4821-crayon-renard")
        without.joinWithPendingLink()
        XCTAssertEqual(without.error, "pair_error_relay_required")
        XCTAssertTrue(without.enteringCode)
        XCTAssertEqual(without.code, "4821-crayon-renard")
        XCTAssertNil(without.pendingLink)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testAScannedLinkNamesItsRelayEvenOnADeviceWithoutOne() async {
        let model = await makeModel(relay: nil)
        model.onQRScanned(" https://Relay.Example.org/l/pair#4821-crayon-renard \n")
        XCTAssertEqual(model.phase, .working)
        await settle()
        XCTAssertEqual(core.calls, ["pairWithCode:4821-crayon-renard@https://relay.example.org"])
    }

    func testAScannedFallbackLinkWithoutAnyRelayMovesToTheTypingStep() async {
        let model = await makeModel(relay: nil)
        model.onQRScanned("tomi-share://pair#4821-crayon-renard")
        XCTAssertEqual(model.error, "pair_error_relay_required")
        XCTAssertTrue(model.enteringCode)
        XCTAssertEqual(model.code, "4821-crayon-renard")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testTheRelayIsForgottenWhenTheScreenIsLeft() async {
        let model = await makeModel(relay: nil)
        model.onRelayChange("other.example.net")
        model.reset()
        XCTAssertEqual(model.relayText, "")
    }
}

/// "Crée ton espace" (Android `CreateSpaceViewModel`).
@MainActor
final class CreateSpaceViewModelTests: XCTestCase {
    private var core: PreviewCore!
    private static let deviceRelay = "https://relay.example.org"

    /// A model on a core whose device has `relay` (nil: no default relay), already read by the model.
    private func makeModel(name: String? = "Alice's iPhone", relay: String? = CreateSpaceViewModelTests.deviceRelay) async -> CreateSpaceViewModel {
        core = PreviewCore(items: [], localDevice: PreviewFixtures.phone)
        core.state.isPaired = false
        core.relay = relay
        let model = CreateSpaceViewModel(core: core, suggestedName: name, reader: FakeLinkReader())
        await model.loadDeviceRelay()
        return model
    }

    private let token = "y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"

    func testTheNameOfThePhoneIsSuggestedOrIPhone() async {
        let named = await makeModel()
        XCTAssertEqual(named.deviceName, "Alice's iPhone")
        let blank = await makeModel(name: " \n ")
        XCTAssertEqual(blank.deviceName, DeviceNames.fallback)
    }

    func testAScannedTokenFillsTheFieldAndSubmitsNothing() async {
        let model = await makeModel()
        model.onQRScanned("https://example.org")
        XCTAssertEqual(model.error, "create_error_qr")
        model.onQRScanned(" \(token) ")
        XCTAssertEqual(model.token, token)
        XCTAssertNil(model.error)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testTheNameAndTheTokenAreCheckedFirst() async {
        let model = await makeModel()
        model.onDeviceNameChange("   ")
        await model.submit()
        XCTAssertEqual(model.error, "create_error_name")
        model.onDeviceNameChange("iPhone")
        await model.submit()
        XCTAssertEqual(model.error, "create_error_token_empty")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testTheSpaceIsCreatedAndTheTokenDropped() async {
        let model = await makeModel()
        model.onTokenChange(token)
        await model.submit()
        XCTAssertEqual(core.calls, ["createSpace:Alice's iPhone"], "the relay of the device is the core's own: none is passed")
        XCTAssertTrue(model.done)
        XCTAssertEqual(model.token, "")
    }

    func testAnExistingSpaceOffersToJoinIt() async {
        let model = await makeModel()
        model.onTokenChange(token)
        core.createError = PairingFailure.spaceExists
        await model.submit()
        XCTAssertEqual(model.error, "create_error_space_exists")
        XCTAssertTrue(model.spaceExists)
        model.onTokenChange(token + "x")
        XCTAssertFalse(model.spaceExists, "a new token clears it")
        core.createError = PairingFailure.installTokenInvalid
        await model.submit()
        XCTAssertEqual(model.error, "create_error_token")
        XCTAssertFalse(model.done)
    }

    func testTheNameIsCutAtSixtyFourUnits() async {
        let model = await makeModel()
        model.onDeviceNameChange(String(repeating: "é", count: 70))
        XCTAssertEqual(model.deviceName.utf16.count, 64)
        model.onDeviceNameChange(String(repeating: "a", count: 63) + "📱")
        XCTAssertEqual(model.deviceName, String(repeating: "a", count: 63), "never half an emoji")
    }

    // MARK: The relay (spec/pairing-v1.md 10.3)

    func testACreationLinkFillsTheTokenAndTheRelayAndSubmitsNothing() async {
        let model = await makeModel(relay: nil)
        model.onQRScanned("https://relay.example.org/create#\(token)")
        XCTAssertEqual(model.token, token)
        XCTAssertEqual(model.relayText, "https://relay.example.org")
        XCTAssertTrue(model.relayFromLink)
        XCTAssertEqual(model.relayChoice.authority, "relay.example.org", "shown in full before the token leaves")
        XCTAssertNil(model.error)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testACreationLinkPastedInTheTokenFieldDoesTheSame() async {
        let model = await makeModel(relay: nil)
        model.onTokenChange("https://relay.example.org:8443/create/\(token)")
        XCTAssertEqual(model.token, token)
        XCTAssertEqual(model.relayText, "https://relay.example.org:8443")
        XCTAssertEqual(model.relayChoice.authority, "relay.example.org:8443")
    }

    func testTheRelayOfTheLinkIsPassedWithTheToken() async {
        let model = await makeModel(relay: nil)
        model.offerLink(relayUrl: "https://relay.example.org", token: token)
        await model.submit()
        XCTAssertEqual(core.calls, ["createSpace:Alice's iPhone@https://relay.example.org"])
        XCTAssertTrue(model.done)
        XCTAssertEqual(model.token, "")
        XCTAssertEqual(model.relayText, "")
    }

    func testTheRelayOfTheFieldIsPassedWithTheToken() async {
        let model = await makeModel(relay: nil)
        model.onTokenChange(token)
        model.onRelayChange("relay.example.org")
        XCTAssertFalse(model.relayFromLink)
        await model.submit()
        XCTAssertEqual(core.calls, ["createSpace:Alice's iPhone@https://relay.example.org"])
    }

    func testTheRetainedRelayIsTheDevicesWhenTheFieldIsEmpty() async {
        let model = await makeModel()
        XCTAssertEqual(model.relayChoice.authority, "relay.example.org")
        XCTAssertFalse(model.relayFieldRequired)
        model.onRelayChange("other.example.net")
        XCTAssertEqual(model.relayChoice.authority, "other.example.net")
    }

    func testWithoutAnyRelayNothingIsSent() async {
        let model = await makeModel(relay: nil)
        XCTAssertTrue(model.relayFieldRequired)
        XCTAssertNil(model.relayChoice.authority)
        model.onTokenChange(token)
        await model.submit()
        XCTAssertEqual(model.error, "pair_error_relay_required")
        XCTAssertTrue(core.calls.isEmpty)
        XCTAssertEqual(model.token, token, "kept for the retry")
    }

    func testAnInvalidRelayIsRefusedBeforeTheToken() async {
        let model = await makeModel()
        model.onTokenChange(token)
        model.onRelayChange("http://relay.example.org")
        await model.submit()
        XCTAssertEqual(model.error, "pair_error_relay_invalid")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testARelayTheCoreRefusesShowsItsMessage() async {
        let model = await makeModel(relay: nil)
        model.onTokenChange(token)
        model.onRelayChange("relay.example.org")
        core.createError = PairingFailure.invalidRelay
        await model.submit()
        XCTAssertEqual(model.error, "pair_error_relay_invalid")
        XCTAssertFalse(model.done)
    }
}
