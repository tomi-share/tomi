import XCTest
@testable import TomiKit

/// The notices about removed devices: the `via` the core sends, the text chosen, shown once, never printed with a name.
@MainActor
final class RelayNoticesTests: XCTestCase {
    private func notice(_ id: String, _ name: String = "PC-Bureau", via: RelayRevocation.Via = .administrator) -> RelayRevocation {
        RelayRevocation(deviceId: id, name: name, via: via)
    }

    func testTheViaOfTheCoreIsMapped() {
        XCTAssertEqual(RelayRevocation.Via(wire: "admin"), .administrator)
        XCTAssertEqual(RelayRevocation.Via(wire: "self"), .device)
        XCTAssertEqual(RelayRevocation.Via(wire: "something-new"), .unknown)
        XCTAssertEqual(RelayRevocation.Via(wire: ""), .unknown)
    }

    func testEachViaHasItsOwnText() {
        XCTAssertEqual(notice("a", via: .administrator).messageKey, "ios_relay_revoked_admin")
        XCTAssertEqual(notice("a", via: .device).messageKey, "ios_relay_revoked_self")
        XCTAssertEqual(notice("a", via: .unknown).messageKey, "ios_relay_revoked_other")
    }

    func testADeviceIsToldOnceWhateverTheOrder() {
        let first = notice("a")
        let second = notice("b", "Tablette")
        XCTAssertEqual(RelayNotices.merged([first], with: [first, second, second]), [first, second])
        XCTAssertEqual(RelayNotices.merged([], with: []), [])
        XCTAssertEqual(RelayNotices.merged([second], with: [first]), [second, first])
    }

    func testTheNoticesComeOutOfTheStateOnce() {
        let state = CoreState()
        XCTAssertTrue(state.takeRelayRevocations().isEmpty)
        state.addRelayRevocations([notice("a")])
        state.addRelayRevocations([notice("b", "Tablette", via: .device)])
        XCTAssertEqual(state.relayRevocations.map(\.deviceId), ["a", "b"])
        XCTAssertEqual(state.takeRelayRevocations().map(\.deviceId), ["a", "b"])
        XCTAssertTrue(state.relayRevocations.isEmpty)
        XCTAssertTrue(state.takeRelayRevocations().isEmpty, "once")
    }

    func testTheRotationAlertIsOffUntilTheCoreRaisesIt() {
        let state = CoreState()
        XCTAssertFalse(state.rotationStalled)
        state.rotationStalled = true
        XCTAssertTrue(state.rotationStalled)
    }

    func testTheNameOfADeviceIsNeverPrinted() {
        let printed = [String(describing: notice("a")), String(reflecting: notice("a"))]
        for text in printed {
            XCTAssertFalse(text.contains("PC-Bureau"), text)
            XCTAssertTrue(text.contains("redacted"), text)
        }
    }
}
