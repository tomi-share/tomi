import XCTest
@testable import TomiKit

// Port of apps/android/.../share/ItemSharingTest.kt: system share of an item, direct for a normal item, behind the
// biometric gate for a hidden ("Masqué") one.
final class ItemSharingTests: XCTestCase {
    private let office = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_793_786_400)
    private let password = "Hx7!pQ2-demo-zR9"

    private func normal() -> TomiItem {
        .readable(id: "n", sourceDevice: office, createdAt: now, title: "Doc", text: RichText(plain: "bonjour"), url: "https://a.example")
    }

    private func hidden() -> TomiItem {
        .readable(id: "s", sourceDevice: office, createdAt: now, title: "Wi-Fi invités", text: RichText(plain: password), secret: true)
    }

    /// Counts how many times the gate is asked and answers with `outcome`.
    private final class Gate {
        let outcome: AuthOutcome
        var calls = 0
        init(_ outcome: AuthOutcome) { self.outcome = outcome }
        func authenticate() async -> AuthOutcome {
            calls += 1
            return outcome
        }
    }

    func testANormalItemIsSharedDirectlyWithoutAnyAuthentication() async {
        let gate = Gate(.failed)
        let outcome = await ItemSharing.prepare(normal(), authenticate: gate.authenticate)
        XCTAssertEqual(gate.calls, 0)
        guard case let .ready(payload) = outcome else { return XCTFail("expected ready") }
        XCTAssertEqual(payload.text, "bonjour\nhttps://a.example")
        XCTAssertEqual(payload.subject, "Doc")
    }

    func testAHiddenItemIsSharedOnlyAfterASuccessfulAuthentication() async {
        let gate = Gate(.success)
        let outcome = await ItemSharing.prepare(hidden(), authenticate: gate.authenticate)
        XCTAssertEqual(gate.calls, 1)
        guard case let .ready(payload) = outcome else { return XCTFail("expected ready") }
        XCTAssertEqual(payload.text, password)
    }

    func testAHiddenItemNeverProducesAPayloadWhenAuthenticationIsCancelledFailedOrUnavailable() async {
        for failure in [AuthOutcome.cancelled, .failed] {
            let gate = Gate(failure)
            let outcome = await ItemSharing.prepare(hidden(), authenticate: gate.authenticate)
            XCTAssertEqual(outcome, .notAuthenticated)
            XCTAssertEqual(gate.calls, 1)
        }
        let unavailable = Gate(.unavailable)
        let outcome = await ItemSharing.prepare(hidden(), authenticate: unavailable.authenticate)
        XCTAssertEqual(outcome, .biometricUnavailable)
    }

    func testTheContentOfAHiddenItemIsNotReadBeforeTheGateAnswers() async {
        // The gate is asked before anything is built, and what the caller gets back for a refusal never carries the content.
        var askedBeforeAnyPayload = false
        let outcome = await ItemSharing.prepare(hidden()) {
            askedBeforeAnyPayload = true
            return .cancelled
        }
        XCTAssertTrue(askedBeforeAnyPayload)
        if case .ready = outcome { XCTFail("must not be ready") }
    }

    func testAPayloadNeverPrintsItsContent() {
        let payload = SharePayload(text: password, subject: "Wi-Fi invités")
        for printed in [String(describing: payload), String(reflecting: payload), String(describing: ShareOutcome.ready(payload))] {
            XCTAssertFalse(printed.contains(password), printed)
            XCTAssertFalse(printed.contains("Wi-Fi"), printed)
        }
    }

    func testUnreadableItemsAndFileOnlyItemsHaveNothingToShareAndAskNoAuthentication() async {
        let unreadable = TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: office, createdAt: now)
        let filesOnly = TomiItem.readable(
            id: "f", sourceDevice: office, createdAt: now,
            attachments: [Attachment(name: "a.pdf", mime: "application/pdf", size: 1)], secret: true
        )
        let gate = Gate(.success)
        let first = await ItemSharing.prepare(unreadable, authenticate: gate.authenticate)
        let second = await ItemSharing.prepare(filesOnly, authenticate: gate.authenticate)
        XCTAssertEqual(first, .nothingToShare)
        XCTAssertEqual(second, .nothingToShare)
        XCTAssertEqual(gate.calls, 0)
        XCTAssertFalse(ItemSharing.canShare(unreadable))
        XCTAssertFalse(ItemSharing.canShare(filesOnly))
        XCTAssertTrue(ItemSharing.canShare(hidden()))
    }

    func testATitleOnlyItemSharesItsTitleAsText() throws {
        let payload = try XCTUnwrap(ItemSharing.payload(for: .readable(id: "t", sourceDevice: office, createdAt: now, title: "Rappel")))
        XCTAssertEqual(payload.text, "Rappel")
        XCTAssertNil(payload.subject)
    }
}
