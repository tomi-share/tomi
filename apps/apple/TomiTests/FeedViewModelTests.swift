import Foundation
import TomiKit
import XCTest
@testable import Tomi

/// The feed model on the in-memory core of the previews: reveal and auto-hide of hidden items, the snapshot of an
/// "after first read" item, filters and sections, archive with undo, marking read.
@MainActor
final class FeedViewModelTests: XCTestCase {
    private var core: PreviewCore!
    private var feedback: FeedbackCenter!
    private var authOutcome = AuthOutcome.success
    private var authPrompts = 0

    private func makeModel(_ items: [TomiItem] = PreviewFixtures.all) -> FeedViewModel {
        core = PreviewCore(items: items, localDevice: PreviewFixtures.phone)
        feedback = FeedbackCenter()
        let suite = "FeedViewModelTests.\(UUID().uuidString)"
        addTeardownBlock { UserDefaults().removePersistentDomain(forName: suite) }
        return FeedViewModel(
            core: core,
            prefs: UserPrefs(defaults: UserDefaults(suiteName: suite)!),
            gate: BiometricGate { [unowned self] _ in
                authPrompts += 1
                return authOutcome
            },
            network: NetworkMonitor(),
            feedback: feedback
        )
    }

    /// The model runs its calls in tasks: wait until `condition` holds (or fail after one second).
    private func eventually(_ condition: () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
        for _ in 0..<200 {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(5))
        }
        XCTFail("condition never held", file: file, line: line)
    }

    // ---- Reveal ----------------------------------------------------------------------------------------------------------

    func testRevealingAHiddenItemAsksOnceThenMarksItRead() async {
        let model = makeModel()
        let secret = PreviewFixtures.secret
        model.reveal(secret)
        await eventually { model.isRevealed(secret) }
        XCTAssertEqual(authPrompts, 1)
        await eventually { core.calls.contains("markRead:secret") }
        // The 2-minute window: the next protected action does not ask again.
        model.hide(secret.id)
        model.reveal(secret)
        await eventually { model.isRevealed(secret) }
        XCTAssertEqual(authPrompts, 1)
    }

    func testAnAfterFirstReadItemStaysOnScreenWhileRevealedThenGoes() async {
        let model = makeModel()
        let once = PreviewFixtures.secretAfterRead
        model.reveal(once)
        await eventually { core.calls.contains("markRead:secret-once") }
        XCTAssertNil(core.state.items.first { $0.id == once.id }, "consumed in the core")
        XCTAssertNotNil(model.item(id: once.id), "the revealed snapshot stays visible")
        XCTAssertTrue(model.visibleItems.contains { $0.id == once.id })
        model.hide(once.id)
        XCTAssertNil(model.item(id: once.id))
    }

    func testHideAllMasksEverything() async {
        let model = makeModel()
        model.reveal(PreviewFixtures.secret)
        await eventually { model.isRevealed(PreviewFixtures.secret) }
        model.hideAll()
        XCTAssertTrue(model.revealedIds.isEmpty)
    }

    func testNothingIsRevealedWithoutAuthentication() async {
        let model = makeModel()
        authOutcome = .unavailable
        model.reveal(PreviewFixtures.secret)
        await eventually { feedback.current != nil }
        XCTAssertFalse(model.isRevealed(PreviewFixtures.secret))
        XCTAssertEqual(feedback.current?.text, L10n.text("toast_biometric_unavailable"))

        feedback.dismiss()
        authOutcome = .cancelled
        model.reveal(PreviewFixtures.secret)
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertFalse(model.isRevealed(PreviewFixtures.secret))
        XCTAssertNil(feedback.current, "a cancelled prompt says nothing")
        XCTAssertFalse(core.calls.contains("markRead:secret"))
    }

    // ---- Feed --------------------------------------------------------------------------------------------------------------

    func testSectionsAndFilters() {
        let model = makeModel()
        XCTAssertEqual(model.sections.pinned.map(\.id), ["pinned"])
        XCTAssertFalse(model.sections.recent.contains { $0.id == "archived" }, "archived items stay out of Récents")
        XCTAssertEqual(model.allItems.map(\.createdAt), model.allItems.map(\.createdAt).sorted(by: >), "newest first")

        model.filter = .archives
        XCTAssertEqual(model.visibleItems.map(\.id), ["archived"])
        model.filter = .secrets
        XCTAssertEqual(Set(model.visibleItems.map(\.id)), ["secret", "secret-once"])
        model.filter = .files
        XCTAssertEqual(Set(model.visibleItems.map(\.id)), ["image", "files"])
    }

    func testTheSearchIsCappedAndNeverMatchesHiddenContent() {
        let model = makeModel()
        model.setQuery(String(repeating: "a", count: 500))
        XCTAssertEqual(model.query.count, FeedViewModel.maxQuery)
        model.setQuery("correct-horse")
        XCTAssertTrue(model.visibleItems.isEmpty, "the content of a hidden item is not searchable")
        model.setQuery("wi-fi")
        XCTAssertTrue(model.visibleItems.isEmpty, "nor its title")
        model.filter = .secrets
        model.setQuery("bureau")
        XCTAssertEqual(model.visibleItems.map(\.id), ["secret"], "only the source device is")
    }

    // ---- Archive, pin, read --------------------------------------------------------------------------------------------

    func testArchivingOffersAnUndoThatRestoresThePin() async throws {
        let model = makeModel()
        model.applyArchiveAction(PreviewFixtures.pinned)
        await eventually { feedback.current?.actionTitle != nil }
        XCTAssertEqual(core.state.items.first { $0.id == "pinned" }?.archived, true)
        XCTAssertEqual(core.state.items.first { $0.id == "pinned" }?.pinned, false)
        XCTAssertFalse(core.calls.contains { $0.hasPrefix("markRead") }, "archiving never marks read")

        try XCTUnwrap(feedback.current?.action)()
        await eventually { core.state.items.first { $0.id == "pinned" }?.pinned == true }
        XCTAssertEqual(core.state.items.first { $0.id == "pinned" }?.archived, false)
    }

    func testArchivingAHiddenItemMasksItAgain() async {
        let model = makeModel()
        model.reveal(PreviewFixtures.secret)
        await eventually { model.isRevealed(PreviewFixtures.secret) }
        model.applyArchiveAction(PreviewFixtures.secret)
        XCTAssertFalse(model.isRevealed(PreviewFixtures.secret))
    }

    func testOpeningMarksReadOnlyIncomingReadableItems() async {
        let model = makeModel()
        model.onCardOpened(PreviewFixtures.plain)
        model.onCardOpened(PreviewFixtures.secret)
        model.onCardOpened(PreviewFixtures.outgoingQueued)
        model.onCardOpened(PreviewFixtures.unreadable)
        await eventually { core.calls.contains("markRead:plain") }
        try? await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(core.calls.filter { $0.hasPrefix("markRead") }, ["markRead:plain"])
    }

    func testItemsSeenOnScreenAreReadLikeATap() async {
        let model = makeModel()
        model.onItemsSeen([PreviewFixtures.plain, PreviewFixtures.secret, PreviewFixtures.outgoingQueued, PreviewFixtures.unreadable])
        await eventually { core.calls.contains("markRead:plain") }
        try? await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(core.calls.filter { $0.hasPrefix("markRead") }, ["markRead:plain"])
        let before = core.calls.count
        model.onItemsSeen([])
        try? await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(core.calls.count, before, "nothing seen, nothing read")
    }

    func testDeletingAnItemRemovesItAndSaysSo() async {
        let model = makeModel()
        model.deleteEverywhere(PreviewFixtures.link)
        await eventually { feedback.current != nil }
        XCTAssertNil(model.item(id: "link"))
        XCTAssertEqual(feedback.current?.text, L10n.text("snackbar_deleted_everywhere"))
    }

    // ---- Review fixes ------------------------------------------------------------------------------------------------------

    func testAHiddenItemsFileOpensTheViewerOnItsSharePage() async {
        let model = makeModel()
        let files = PreviewFixtures.files
        let hidden = TomiItem.readable(
            id: "hidden-files", sourceDevice: files.sourceDevice, createdAt: files.createdAt, attachments: files.attachments, secret: true
        )
        model.viewerRequest = ViewerRequest(itemId: "x", attachmentIndex: 0)
        model.openAttachment(hidden, index: 0)
        // One viewer at a time; the file viewer itself never draws a hidden item's file (`FileViewerScreen.load`).
        XCTAssertNil(model.viewerRequest)
        XCTAssertEqual(model.fileRequest, ViewerRequest(itemId: "hidden-files", attachmentIndex: 0))
    }

    func testDeliveryLabelsKeepADigitOnlyDeviceNameAsText() {
        // A device named "42" used to be formatted as a number for "%2$@" and crashed the feed.
        let expired = L10n.delivery(.expired(delivered: 1, devices: "42"))
        XCTAssertTrue(expired.contains("42"), expired)
        XCTAssertTrue(expired.contains("1"), expired)
        let partial = L10n.delivery(.deliveredPartial(delivered: 1, total: 3))
        XCTAssertTrue(partial.contains("1") && partial.contains("3"), partial)
        XCTAssertFalse(L10n.delivery(.received).isEmpty)
    }

    // ---- Image viewer and code screen ------------------------------------------------------------------------------------

    func testEachAttachmentRowOpensItsOwnScreen() {
        let model = makeModel()
        XCTAssertTrue(model.openAttachmentRow(PreviewFixtures.image, index: 0))
        XCTAssertEqual(model.viewerRequest, ViewerRequest(itemId: "image", attachmentIndex: 0))
        XCTAssertTrue(model.openAttachmentRow(PreviewFixtures.files, index: 1), "main.rs")
        XCTAssertEqual(model.codeRequest, ViewerRequest(itemId: "files", attachmentIndex: 1))
        XCTAssertFalse(model.openAttachmentRow(PreviewFixtures.files, index: 9))
    }

    func testAHiddenItemsOtherFileIsNeverDrawnBySystemViewers() {
        let model = makeModel()
        let files = PreviewFixtures.files
        let hidden = TomiItem.readable(
            id: "hidden-files", sourceDevice: files.sourceDevice, createdAt: files.createdAt, attachments: files.attachments, secret: true
        )
        // The file viewer opens on the page that offers to share it: neither Quick Look nor the player draw it.
        XCTAssertTrue(model.openAttachmentRow(hidden, index: 0), "a PDF of a hidden item")
        XCTAssertEqual(model.fileRequest, ViewerRequest(itemId: "hidden-files", attachmentIndex: 0))
        XCTAssertTrue(model.openAttachmentRow(hidden, index: 1), "its code file opens in Tomi, behind the gate")
    }

    func testTheViewersAskTheGateOnlyForAHiddenItem() async {
        let model = makeModel()
        let opened = await model.authorizeView(PreviewFixtures.image, file: false)
        XCTAssertTrue(opened)
        XCTAssertEqual(authPrompts, 0)
        authOutcome = .cancelled
        let refused = await model.authorizeView(PreviewFixtures.secret, file: true)
        XCTAssertFalse(refused)
        XCTAssertEqual(authPrompts, 1)
        authOutcome = .success
        let allowed = await model.authorizeView(PreviewFixtures.secret, file: true)
        XCTAssertTrue(allowed)
    }
}
