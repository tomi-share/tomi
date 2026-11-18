import Foundation
import XCTest
@testable import TomiKit

@MainActor
private final class FakeSender: ShareSending {
    var submitted: [(ParsedShare, Bool, ExpiryChoice)] = []
    var cancelled: [String] = []
    var cancelAnswer = true
    var failSubmit = false

    func submit(_ parsed: ParsedShare, secret: Bool, expiry: ExpiryChoice) async throws -> ShareSent {
        if failSubmit { throw CocoaError(.fileWriteUnknown) }
        submitted.append((parsed, secret, expiry))
        return ShareSent(
            id: "s\(submitted.count)", parsed: parsed, secret: secret, expiry: expiry, skippedFiles: 0,
            deadline: UndoDeadline(atElapsedMs: 3_000, windowMs: 3_000)
        )
    }

    func cancel(_ sent: ShareSent) async throws -> Bool {
        cancelled.append(sent.id)
        return cancelAnswer
    }
}

/// The share sheet (Android `ShareViewModelTest`): sent at once, a secret confirmed first, "Annuler", "Options".
@MainActor
final class ShareFlowTests: XCTestCase {
    private var sender: FakeSender!

    private func flow(paired: Bool = true) -> ShareFlow {
        sender = FakeSender()
        return ShareFlow(sender: sender, isPaired: paired)
    }

    func testNotPairedReadsNothing() async {
        let flow = flow(paired: false)
        var read = false
        await flow.start { read = true; return ShareInput(text: "x") }
        XCTAssertEqual(flow.state, .notPaired)
        XCTAssertFalse(read)
    }

    func testUnreadableDataReadsNothingAndComesBeforeThePairing() async {
        sender = FakeSender()
        for paired in [true, false] {
            let flow = ShareFlow(sender: sender, isPaired: paired, isUnreadable: true)
            var read = false
            await flow.start { read = true; return ShareInput(text: "x") }
            XCTAssertEqual(flow.state, .unreadable)
            XCTAssertFalse(read)
        }
    }

    func testALinkLeavesAtOnceWithTheDefaults() async {
        let flow = flow()
        await flow.start { ShareInput(text: "Un article https://example.org/a") }
        guard case let .sent(sent) = flow.state else { return XCTFail("\(flow.state)") }
        XCTAssertEqual(sent.parsed.url, "https://example.org/a")
        XCTAssertFalse(sent.secret)
        XCTAssertEqual(sent.expiry, .never)
        // Runs once.
        await flow.start { ShareInput(text: "autre") }
        XCTAssertEqual(sender.submitted.count, 1)
    }

    func testNothingUsableAndAFailedReadingEndQuietly() async {
        let empty = flow()
        await empty.start { ShareInput(text: "   ") }
        XCTAssertEqual(empty.state, .nothingToShare)
        let broken = flow()
        await broken.start { throw CocoaError(.fileReadUnknown) }
        XCTAssertEqual(broken.state, .failed)
    }

    func testASecretIsConfirmedFirstAndCanBeMadePlain() async {
        let flow = flow()
        await flow.start { ShareInput(text: "Xk9#mQ2$vL7!pR4&") }
        guard case let .confirm(draft, locked) = flow.state else { return XCTFail("\(flow.state)") }
        XCTAssertTrue(draft.secret)
        XCTAssertFalse(locked)
        XCTAssertTrue(flow.state.showsSecret)
        XCTAssertTrue(sender.submitted.isEmpty)
        flow.onExpiryChange(.afterFirstRead)
        flow.onSecretChange(false)
        guard case let .confirm(plain, _) = flow.state else { return XCTFail() }
        XCTAssertFalse(plain.secret)
        XCTAssertEqual(plain.expiry, .never, "after first read is for hidden content only")
        await flow.confirm()
        XCTAssertEqual(sender.submitted.first?.1, false)
    }

    func testMarkedSensitiveStaysHidden() async {
        let flow = flow()
        await flow.start { ShareInput(text: "code 1234", markedSensitive: true) }
        flow.onSecretChange(false)
        guard case let .confirm(draft, locked) = flow.state else { return XCTFail() }
        XCTAssertTrue(locked)
        XCTAssertTrue(draft.secret)
    }

    func testUndoClosesOrSaysItIsTooLate() async {
        let flow = flow()
        await flow.start { ShareInput(text: "https://example.org") }
        let closes = await flow.undo()
        XCTAssertTrue(closes)
        XCTAssertEqual(sender.cancelled, ["s1"])

        let late = self.flow()
        await late.start { ShareInput(text: "https://example.org") }
        sender.cancelAnswer = false
        let lateCloses = await late.undo()
        XCTAssertFalse(lateCloses)
        XCTAssertEqual(late.state, .tooLate)
    }

    func testOptionsCancelThenSendAgainWithTheNewSettings() async {
        let flow = flow()
        await flow.start { ShareInput(text: "https://example.org") }
        await flow.openOptions()
        guard case let .options(draft) = flow.state else { return XCTFail("\(flow.state)") }
        XCTAssertFalse(draft.secret)
        flow.onSecretChange(true)
        flow.onExpiryChange(.hour1)
        await flow.confirm()
        guard case let .sent(sent) = flow.state else { return XCTFail() }
        XCTAssertEqual(sent.id, "s2")
        XCTAssertTrue(sent.secret)
        XCTAssertEqual(sent.expiry, .hour1)
        XCTAssertEqual(sender.cancelled, ["s1"])
    }

    func testASubmitFailureShowsTheFailedCard() async {
        let flow = flow()
        sender.failSubmit = true
        await flow.start { ShareInput(text: "https://example.org") }
        XCTAssertEqual(flow.state, .failed)
    }
}
