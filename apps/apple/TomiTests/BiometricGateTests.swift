import TomiKit
import XCTest
@testable import Tomi

/// Adding a device asks the owner every time; a reveal's 2-minute window does not stand for it.
@MainActor
final class BiometricGateTests: XCTestCase {
    func testAddingADeviceAsksEveryTimeEvenInsideTheTwoMinuteWindow() async {
        var prompts = 0
        let gate = BiometricGate(evaluate: { _ in
            prompts += 1
            return .success
        })
        let first = await gate.authenticate(reason: "reveal")
        XCTAssertEqual(first, .success)
        let second = await gate.authenticate(reason: "reveal")
        XCTAssertEqual(second, .success)
        XCTAssertEqual(prompts, 1, "the window covers a second reveal")
        let fresh = await gate.authenticateFresh(reason: "add")
        XCTAssertEqual(fresh, .success)
        let again = await gate.authenticateFresh(reason: "add")
        XCTAssertEqual(again, .success)
        XCTAssertEqual(prompts, 3, "a fresh check is never served by the window")
    }

    func testAFreshCheckThatFailsOpensNothing() async {
        let gate = BiometricGate(evaluate: { _ in .failed })
        let outcome = await gate.authenticateFresh(reason: "add")
        XCTAssertEqual(outcome, .failed)
        XCTAssertFalse(gate.window.isValid)
    }
}
