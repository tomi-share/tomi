import TomiKit
import XCTest

// Ports of apps/android/app/src/test/java/eu/shaffe/tomi/ui/pairing/{InstallTokenQrTest,PairingUiLogicTest}.kt and
// core/{DeviceNamesTest,PairingErrorsTest}.kt.

final class InstallTokenQrTests: XCTestCase {
    private let token = "y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"

    func testATokenIsAcceptedAsItIsScanned() {
        XCTAssertEqual(InstallTokenQr.parse(token), token)
        XCTAssertEqual(InstallTokenQr.parse("  \(token) \n"), token)
        XCTAssertEqual(InstallTokenQr.parse(token.uppercased()), token.uppercased())
        let bare = token.replacingOccurrences(of: "-", with: "")
        XCTAssertEqual(InstallTokenQr.parse(bare), bare)
    }

    func testAPairingLinkIsNotAToken() {
        XCTAssertNil(InstallTokenQr.parse("tomi-share://pair#4821-crayon-renard"))
        XCTAssertNil(InstallTokenQr.parse("https://relay.example.org/l/pair#4821-crayon-renard"))
        XCTAssertNil(InstallTokenQr.parse("4821-crayon-renard"))
    }

    func testAnythingElseIsRefused() {
        XCTAssertNil(InstallTokenQr.parse(""))
        XCTAssertNil(InstallTokenQr.parse("   "))
        XCTAssertNil(InstallTokenQr.parse("https://example.org/"))
        XCTAssertNil(InstallTokenQr.parse("y8d6-69p1"))
        XCTAssertNil(InstallTokenQr.parse("\(token)-y8d6-69p1-7b5p"))
        XCTAssertNil(InstallTokenQr.parse("i8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5n47"))
        XCTAssertNil(InstallTokenQr.parse("y8d6-69p1-7b5p-2azs-tpc9-ksx2-ywky-5nou"))
        XCTAssertNil(InstallTokenQr.parse(String(repeating: "x", count: 65)))
    }
}

final class DeviceNamesTests: XCTestCase {
    func testBlanksAndControlCharactersBecomeOneSpace() {
        XCTAssertEqual(DeviceNames.clean("  iPhone \t 17\u{0007}of   Alice\n"), "iPhone 17 of Alice")
    }

    func testNothingLeftMeansNoName() {
        XCTAssertNil(DeviceNames.clean(nil))
        XCTAssertNil(DeviceNames.clean(""))
        XCTAssertNil(DeviceNames.clean(" \t\n\u{0007} "))
    }

    func testALongNameIsCutAtSixtyFourCharacters() {
        XCTAssertEqual(DeviceNames.clean(String(repeating: "a", count: 80))?.unicodeScalars.count, 64)
        let emoji = DeviceNames.clean(String(repeating: "a", count: 63) + String(repeating: "📱", count: 5))
        XCTAssertEqual(emoji?.unicodeScalars.count, 64)
        XCTAssertEqual(emoji?.unicodeScalars.last, "📱".unicodeScalars.first)
        XCTAssertEqual(DeviceNames.clean(String(repeating: "a", count: 63) + " b"), String(repeating: "a", count: 63))
    }

    func testTheFirstCandidateTheCoreAcceptsWins() {
        let names: [String?] = [nil, "Mon téléphone", "iPhone17,1"]
        XCTAssertEqual(DeviceNames.choose(names) { $0 != "Mon téléphone" }, "iPhone17,1")
        XCTAssertEqual(DeviceNames.choose(names) { _ in true }, "Mon téléphone")
        XCTAssertEqual(DeviceNames.choose(["x", nil]) { _ in false }, DeviceNames.fallback)
        XCTAssertEqual(DeviceNames.choose([]) { _ in true }, DeviceNames.fallback)
    }
}

final class PairingUiLogicTests: XCTestCase {
    func testTheCountdownRoundsUpAndNeverReadsZeroWhileTimeIsLeft() {
        XCTAssertEqual(Countdown.format(remainingMs: 120_000), "2:00")
        XCTAssertEqual(Countdown.format(remainingMs: 108_001), "1:49")
        XCTAssertEqual(Countdown.format(remainingMs: 1), "0:01")
        XCTAssertEqual(Countdown.format(remainingMs: 0), "0:00")
        XCTAssertEqual(Countdown.format(remainingMs: -5_000), "0:00")
    }

    func testEveryFailureHasAJoinAndACreateMessage() {
        for reason in PairingFailure.allCases {
            XCTAssertFalse(PairingErrors.joinMessageKey(reason).isEmpty)
            XCTAssertFalse(PairingErrors.createMessageKey(reason).isEmpty)
        }
        XCTAssertEqual(PairingErrors.joinMessageKey(.wrongCode), "pair_error_wrong_code")
        XCTAssertEqual(PairingErrors.joinMessageKey(.spaceExists), "pair_error_unexpected")
        XCTAssertEqual(PairingErrors.createMessageKey(.installTokenInvalid), "create_error_token")
        XCTAssertEqual(PairingErrors.createMessageKey(.spaceExists), "create_error_space_exists")
        XCTAssertEqual(PairingErrors.createMessageKey(.interrupted), "pair_error_network")
    }

    func testNoRelayAndAnInvalidRelayHaveTheirOwnMessages() {
        for reason in [PairingFailure.relayNotConfigured, .invalidRelay] {
            let key = reason == .relayNotConfigured ? "pair_error_relay_required" : "pair_error_relay_invalid"
            XCTAssertEqual(PairingErrors.joinMessageKey(reason), key)
            XCTAssertEqual(PairingErrors.createMessageKey(reason), key)
            XCTAssertEqual(PairingErrors.inviteMessageKey(reason), "invite_error_failed")
        }
    }
}

final class PairingErrorsTests: XCTestCase {
    private func failure(_ kind: TomiCoreError.Kind, _ reason: String = "") -> PairingFailure {
        PairingErrors.failure(of: TomiCoreError(kind, reason: reason))
    }

    func testTheStableCodesOfThePairing() {
        XCTAssertEqual(failure(.crypto(code: "pairing_failed")), .wrongCode)
        XCTAssertEqual(failure(.crypto(code: "pairing_expired")), .expired)
        XCTAssertEqual(failure(.crypto(code: "pairing_taken")), .taken)
        XCTAssertEqual(failure(.crypto(code: "pairing_cancelled")), .cancelled)
        XCTAssertEqual(failure(.crypto(code: "pairing_unsupported")), .updateRequired)
        XCTAssertEqual(failure(.crypto(code: "relay_not_configured")), .relayNotConfigured)
        XCTAssertEqual(failure(.crypto(code: "decrypt")), .unexpected)
    }

    func testTheRefusalsOfTheRelay() {
        XCTAssertEqual(failure(.relay(status: 429, code: "rate_limited")), .network)
        XCTAssertEqual(failure(.relay(status: 503, code: "unavailable")), .network)
        XCTAssertEqual(failure(.relay(status: 409, code: "too_many_devices")), .spaceFull)
        XCTAssertEqual(failure(.relay(status: 409, code: "channel_limit")), .anotherPairingOpen)
        XCTAssertEqual(failure(.relay(status: 403, code: "install_token_invalid")), .installTokenInvalid)
        XCTAssertEqual(failure(.relay(status: 409, code: "space_exists")), .spaceExists)
        XCTAssertEqual(failure(.relay(status: 426, code: "update_required")), .updateRequired)
        XCTAssertEqual(failure(.relay(status: 400, code: "format_unsupported")), .updateRequired)
        XCTAssertEqual(failure(.relay(status: 400, code: "bad_request")), .unexpected)
    }

    func testTheOtherErrors() {
        XCTAssertEqual(failure(.network), .network)
        XCTAssertEqual(failure(.blocked, "UpdateRequired { min: 2 }"), .updateRequired)
        XCTAssertEqual(failure(.blocked, "Revoked"), .blocked)
        XCTAssertEqual(failure(.invalidInput), .unexpected)
        XCTAssertEqual(failure(.storage), .unexpected)
        XCTAssertEqual(failure(.internal), .unexpected)
    }

    func testOnlyNetworkStorageAndBusyRelaysAreTransient() {
        XCTAssertTrue(PairingErrors.isTransient(TomiCoreError(.network)))
        XCTAssertTrue(PairingErrors.isTransient(TomiCoreError(.storage)))
        XCTAssertTrue(PairingErrors.isTransient(TomiCoreError(.relay(status: 502, code: "x"))))
        XCTAssertFalse(PairingErrors.isTransient(TomiCoreError(.relay(status: 409, code: "space_exists"))))
        XCTAssertFalse(PairingErrors.isTransient(TomiCoreError(.crypto(code: "pairing_failed"))))
    }
}
