import XCTest
@testable import TomiKit

// Port of apps/android/.../share/ImageActionsTest.kt: display, save and share of an image, direct for a normal item,
// behind the same biometric gate as "Afficher" for an item hidden by the sender, and never a decrypted file requested before that.
final class ImageActionsTests: XCTestCase {
    private let source = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = Date(timeIntervalSince1970: 1_793_872_800)
    private let png = Attachment(name: "a.png", mime: "image/png", size: 10)
    private let pdf = Attachment(name: "a.pdf", mime: "application/pdf", size: 10)

    private func normal() -> TomiItem { .readable(id: "n", sourceDevice: source, createdAt: now, attachments: [pdf, png]) }
    private func hidden() -> TomiItem { .readable(id: "s", sourceDevice: source, createdAt: now, attachments: [pdf, png], secret: true) }
    private func unreadable() -> TomiItem { TomiItem(id: "u", content: .unreadable(.updateRequired), sourceDevice: source, createdAt: now) }

    /// Counts gate prompts and answers with `outcome`.
    private final class Gate {
        let outcome: AuthOutcome
        var calls = 0
        init(_ outcome: AuthOutcome) { self.outcome = outcome }
        func authenticate() async -> AuthOutcome {
            calls += 1
            return outcome
        }
    }

    /// Core stand-in: remembers every file it was asked to decrypt.
    private final class Core {
        var asked: [String] = []
        var actions: ImageActions!
        init() {
            actions = ImageActions { [unowned self] itemId, index in
                asked.append("\(itemId)#\(index)")
                return "file:///tomi.test/\(itemId)/\(index)"
            }
        }
    }

    private final class Sink {
        var written: [String] = []
        var shared: [String] = []
        var writeResult = true
        var shareResult = true
        func write(_ uri: String, _ attachment: Attachment) async -> Bool {
            written.append(uri)
            return writeResult
        }

        func launch(_ uri: String, _ attachment: Attachment) async -> Bool {
            shared.append(uri)
            return shareResult
        }
    }

    // --- normal item: no gate ------------------------------------------------------------------------------------------

    func testANormalImageIsViewedSavedAndSharedWithoutAnyAuthentication() async {
        let gate = Gate(.failed)
        let core = Core()
        let sink = Sink()
        let view = await core.actions.authorizeView(normal(), authenticate: gate.authenticate)
        let save = await core.actions.save(normal(), attachmentIndex: 1, authenticate: gate.authenticate, write: sink.write)
        let share = await core.actions.share(normal(), attachmentIndex: 1, authenticate: gate.authenticate, launch: sink.launch)
        XCTAssertEqual([view, save, share], [.done, .done, .done])
        XCTAssertEqual(gate.calls, 0)
        XCTAssertEqual(sink.written, ["file:///tomi.test/n/1"])
        XCTAssertEqual(sink.shared, ["file:///tomi.test/n/1"])
    }

    // --- hidden item: gate first ---------------------------------------------------------------------------------------

    func testAHiddenImageAsksTheGateToBeViewedSavedAndShared() async {
        let gate = Gate(.success)
        let core = Core()
        let sink = Sink()
        let view = await core.actions.authorizeView(hidden(), authenticate: gate.authenticate)
        let save = await core.actions.save(hidden(), attachmentIndex: 1, authenticate: gate.authenticate, write: sink.write)
        let share = await core.actions.share(hidden(), attachmentIndex: 1, authenticate: gate.authenticate, launch: sink.launch)
        XCTAssertEqual([view, save, share], [.done, .done, .done])
        XCTAssertEqual(gate.calls, 3)
        XCTAssertEqual(sink.written.count, 1)
        XCTAssertEqual(sink.shared.count, 1)
    }

    func testAHiddenImageIsNeverDecryptedWrittenOrSharedWithoutASuccessfulAuthentication() async {
        for refusal in [AuthOutcome.cancelled, .failed, .unavailable] {
            let gate = Gate(refusal)
            let core = Core()
            let sink = Sink()
            let expected: ImageActionResult = refusal == .unavailable ? .biometricUnavailable : .notAuthenticated

            let view = await core.actions.authorizeView(hidden(), authenticate: gate.authenticate)
            let save = await core.actions.save(hidden(), attachmentIndex: 1, authenticate: gate.authenticate, write: sink.write)
            let share = await core.actions.share(hidden(), attachmentIndex: 1, authenticate: gate.authenticate, launch: sink.launch)
            XCTAssertEqual(view, expected, "\(refusal)")
            XCTAssertEqual(save, expected, "\(refusal)")
            XCTAssertEqual(share, expected, "\(refusal)")

            XCTAssertTrue(core.asked.isEmpty, "no file may be decrypted for a refused user (\(refusal))")
            XCTAssertTrue(sink.written.isEmpty, "nothing may reach the photo library (\(refusal))")
            XCTAssertTrue(sink.shared.isEmpty, "nothing may reach a share sheet (\(refusal))")
        }
    }

    func testTheFileOfAHiddenImageIsRequestedAfterTheGateAnswers() async {
        var order: [String] = []
        let actions = ImageActions { _, _ in
            order.append("decrypt")
            return "file:///tomi.test/s/1"
        }
        _ = await actions.save(
            hidden(), attachmentIndex: 1,
            authenticate: {
                order.append("gate")
                return .success
            },
            write: { _, _ in
                order.append("write")
                return true
            }
        )
        XCTAssertEqual(order, ["gate", "decrypt", "write"])
    }

    // --- failures ------------------------------------------------------------------------------------------------------

    func testAnAttachmentThatIsNotAnImageIsUnavailableAndAsksNothing() async {
        let gate = Gate(.success)
        let core = Core()
        let sink = Sink()
        // Index 0 is the PDF, 5 does not exist.
        let save = await core.actions.save(hidden(), attachmentIndex: 0, authenticate: gate.authenticate, write: sink.write)
        let share = await core.actions.share(hidden(), attachmentIndex: 5, authenticate: gate.authenticate, launch: sink.launch)
        XCTAssertEqual(save, .unavailable)
        XCTAssertEqual(share, .unavailable)
        XCTAssertEqual(gate.calls, 0)
        XCTAssertTrue(core.asked.isEmpty)
        XCTAssertTrue(sink.written.isEmpty)
    }

    func testAFileTheCoreCannotProvideIsUnavailable() async {
        let actions = ImageActions { _, _ in nil }
        let sink = Sink()
        let result = await actions.save(normal(), attachmentIndex: 1, authenticate: Gate(.success).authenticate, write: sink.write)
        XCTAssertEqual(result, .unavailable)
        XCTAssertTrue(sink.written.isEmpty)
    }

    func testAFailedWriteAndANoAppShareAreReported() async {
        let core = Core()
        let sink = Sink()
        sink.writeResult = false
        sink.shareResult = false
        let gate = Gate(.success)
        let save = await core.actions.save(normal(), attachmentIndex: 1, authenticate: gate.authenticate, write: sink.write)
        let share = await core.actions.share(normal(), attachmentIndex: 1, authenticate: gate.authenticate, launch: sink.launch)
        XCTAssertEqual(save, .failed)
        XCTAssertEqual(share, .unavailable)
    }

    func testAnUnreadableItemHasNoImage() async {
        let gate = Gate(.success)
        let result = await Core().actions.authorizeView(unreadable(), authenticate: gate.authenticate)
        XCTAssertEqual(result, .unavailable)
        XCTAssertEqual(gate.calls, 0)
    }

    // --- file production enforces the gate (not only the screens) --------------------------------------------------------

    func testAHiddenFileIsNotDecryptedWhenTheAuthorizationWindowIsClosed() async {
        let core = Core()
        let file = await core.actions.fileForView(hidden(), attachmentIndex: 1) { false }
        XCTAssertNil(file)
        XCTAssertTrue(core.asked.isEmpty, "nothing may be decrypted")
    }

    func testAHiddenFileIsServedWhileTheAuthorizationWindowIsOpen() async {
        let core = Core()
        let file = await core.actions.fileForView(hidden(), attachmentIndex: 1) { true }
        XCTAssertEqual(file, "file:///tomi.test/s/1")
        XCTAssertEqual(core.asked, ["s#1"])
    }

    func testANormalFileNeverNeedsTheAuthorizationWindow() async {
        let core = Core()
        let file = await core.actions.fileForView(normal(), attachmentIndex: 1) { false }
        XCTAssertEqual(file, "file:///tomi.test/n/1")
    }

    func testAnUnknownUnreadableOrOutOfRangeItemGivesNoFileAndNoDecryption() async {
        let core = Core()
        let unknown = await core.actions.fileForView(nil, attachmentIndex: 0) { true }
        let unreadableFile = await core.actions.fileForView(unreadable(), attachmentIndex: 0) { true }
        let outOfRange = await core.actions.fileForView(normal(), attachmentIndex: 9) { true }
        let negative = await core.actions.fileForView(hidden(), attachmentIndex: -1) { true }
        XCTAssertNil(unknown)
        XCTAssertNil(unreadableFile)
        XCTAssertNil(outOfRange)
        XCTAssertNil(negative)
        XCTAssertTrue(core.asked.isEmpty)
    }
}
