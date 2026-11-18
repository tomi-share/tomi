import XCTest
@testable import TomiKit

// Port of the pure parts of apps/android/app/src/test/java/eu/shaffe/tomi/core/ArchiveTest.kt: filtering of archived
// items, the exclusion of pin and archive in the UI, the swipe rules and the calls that reach the core contract.
//
// Not ported (storage semantics of FakeTomiCore, now tested in the Rust core): archiveSetsArchivedAnd...,
// archiveAndUnarchiveAreIdempotent..., archivingNeverReordersTheFlow..., anUnreadAfterFirstReadSecretSurvives...,
// aHiddenAndAnUnreadableItemCanBeArchived, archivingAPinnedItemUnpinsIt, pinningAnArchivedItemUnarchivesIt,
// noSequenceOfPinAndArchiveLeavesAnItemBothPinnedAndArchived, theDemoShipsArchivedItems..., filteringReactsToTheCoreFlow.
// The whole-cycle test, which ran against FakeTomiCore, is rewritten on plain values and a recording double.
// The HTML overflow test of this file is in HtmlSanitizerTests.

private let base = ISO8601DateFormatter().date(from: "2026-11-04T10:00:00Z")!
private let office = DeviceRef(id: "pc", name: "PC-Bureau")
private let localId = "local-device"
private let local = DeviceRef(id: localId, name: "iPhone")

private func plain(_ id: String) -> TomiItem {
    TomiItem.readable(id: id, sourceDevice: office, createdAt: base, text: RichText(plain: "text \(id)"))
}

private func item(_ id: String, archived: Bool = false, pinned: Bool = false, build: (String) -> TomiItem = plain) -> TomiItem {
    var result = build(id)
    result.archived = archived
    result.archivedAt = archived ? base : nil
    result.pinned = pinned
    result.pinnedAt = pinned ? base : nil
    return result
}

private func outgoing(_ id: String, _ states: DeliveryState...) -> TomiItem {
    TomiItem.readable(
        id: id, sourceDevice: local, createdAt: base, text: RichText(plain: id),
        deliveries: states.enumerated().map { DeviceDelivery(deviceId: "d\($0.offset)", deviceName: "D\($0.offset)", state: $0.element) }
    )
}

private func ids(_ items: [TomiItem]) -> [String] { items.map(\.id) }

/// Records the calls it receives; every other method of the contract is out of reach of the code under test.
@MainActor
private final class RecordingCore: TomiCore {
    let state = CoreState()
    let isDemo = true
    var calls: [String] = []

    func pin(id: String) async throws { calls.append("pin:\(id)") }
    func unpin(id: String) async throws { calls.append("unpin:\(id)") }
    func archive(id: String) async throws { calls.append("archive:\(id)") }
    func unarchive(id: String) async throws { calls.append("unarchive:\(id)") }

    func getItem(id: String) async throws -> TomiItem? { fatalError("unused") }
    func sync(pushToken: String?) async throws { fatalError("unused") }
    func enqueue(_ draft: OutgoingDraft) async throws { fatalError("unused") }
    func dispatch(id: String) async throws -> DispatchResult { fatalError("unused") }
    func cancelIfQueued(id: String) async throws -> Bool { fatalError("unused") }
    func deleteLocal(id: String) async throws { fatalError("unused") }
    func deleteEverywhere(id: String) async throws { fatalError("unused") }
    func markRead(id: String) async throws { fatalError("unused") }
    func attachmentPath(itemId: String, slot: Int) async throws -> String? { fatalError("unused") }
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
final class ArchiveTests: XCTestCase {
    // ---- Filtering -------------------------------------------------------------------------------------------------------

    func testArchivedItemsAreAbsentFromEveryFilterButArchives() {
        let link = item("link", archived: true) { TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, url: "https://a.example") }
        let text = item("text", archived: true)
        let file = item("file", archived: true) {
            TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, attachments: [Attachment(name: "a.pdf", mime: "application/pdf", size: 1)])
        }
        let secret = item("secret", archived: true) {
            TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, text: RichText(plain: "s"), secret: true)
        }
        let live = plain("live")
        let all = [link, text, file, secret, live]

        for filter in [FeedFilter.all, .links, .text, .files, .secrets] {
            XCTAssertTrue(FeedFiltering.apply(all, filter, "").allSatisfy { !$0.archived }, "\(filter)")
        }
        XCTAssertEqual(ids(FeedFiltering.apply(all, .all, "")), ["live"])
        XCTAssertEqual(ids(FeedFiltering.apply(all, .archives, "")), ["link", "text", "file", "secret"])
    }

    func testTheArchivesFilterOnlyShowsArchivedItems() {
        let all = [plain("a"), item("b", archived: true), plain("c")]
        XCTAssertEqual(ids(FeedFiltering.apply(all, .archives, "")), ["b"])
        XCTAssertTrue(FeedFiltering.apply([plain("a")], .archives, "").isEmpty)
        // The search works inside Archives, the hidden-item rule too.
        XCTAssertEqual(ids(FeedFiltering.apply(all, .archives, "text b")), ["b"])
        XCTAssertTrue(FeedFiltering.apply(all, .archives, "text a").isEmpty)
        let secret = item("s", archived: true) {
            TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, title: "Wi-Fi", text: RichText(plain: "hunter2"), secret: true)
        }
        XCTAssertTrue(FeedFiltering.apply([secret], .archives, "hunter2").isEmpty)
        XCTAssertEqual(ids(FeedFiltering.apply([secret], .archives, "bureau")), ["s"])
    }

    func testAnArchivedItemStaysSearchableFromAllButOnlyWhileSearching() {
        let archived = item("old", archived: true) { TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, title: "Recette tarte") }
        let live = TomiItem.readable(id: "live", sourceDevice: office, createdAt: base, title: "Recette soupe")
        let all = [live, archived]
        // No query: archived items stay out of "All".
        XCTAssertEqual(ids(FeedFiltering.apply(all, .all, "")), ["live"])
        XCTAssertEqual(ids(FeedFiltering.apply(all, .all, "  ")), ["live"])
        // With a query: found, archive badge on the card.
        XCTAssertEqual(ids(FeedFiltering.apply(all, .all, "recette")), ["live", "old"])
        XCTAssertEqual(ids(FeedFiltering.apply(all, .all, "tarte")), ["old"])
        // A typed filter never lists archived items, query or not.
        let link = item("lnk", archived: true) { TomiItem.readable(id: $0, sourceDevice: office, createdAt: base, url: "https://recette.example") }
        XCTAssertTrue(FeedFiltering.apply([link], .links, "recette").isEmpty)
    }

    func testAnItemThatIsPinnedAndArchivedIsShownAsPinned() {
        // A core never reports both; if devices converge on both, the pin wins (docs section 2.3).
        let both = item("both", archived: true, pinned: true)
        XCTAssertFalse(both.isShownAsArchived)
        XCTAssertEqual(ids(FeedFiltering.apply([both], .all, "")), ["both"])
        XCTAssertTrue(FeedFiltering.apply([both], .archives, "").isEmpty)
        XCTAssertEqual(ids(FeedSections.split(FeedFiltering.apply([both], .all, "")).pinned), ["both"])
    }

    func testArchivedItemsLeaveBothSectionsOfTheFeed() {
        let items = [plain("new"), item("arch", archived: true), item("pin", pinned: true), plain("old")]
        let sections = FeedSections.split(FeedFiltering.apply(items, .all, ""))
        XCTAssertEqual(ids(sections.pinned), ["pin"])
        XCTAssertEqual(ids(sections.recent), ["new", "old"])
    }

    // ---- Swipe dispatch --------------------------------------------------------------------------------------------------

    func testTheSwipeArchivesARecentCardAndUnarchivesAnArchivedOne() {
        XCTAssertEqual(ArchiveSwipe.swipeAction(plain("a"), localDeviceId: localId), .archive)
        XCTAssertEqual(ArchiveSwipe.swipeAction(item("b", archived: true), localDeviceId: localId), .unarchive)
        XCTAssertEqual(ArchiveSwipe.swipeAction(outgoing("sent", .delivered, .sent), localDeviceId: localId), .archive)
    }

    func testAPinnedCardDoesNotSwipe() {
        XCTAssertNil(ArchiveSwipe.swipeAction(item("p", pinned: true), localDeviceId: localId))
        // The menu still offers "Archive" for it (archiving unpins).
        XCTAssertTrue(ArchiveSwipe.canArchive(item("p", pinned: true), localDeviceId: localId))
        XCTAssertEqual(ArchiveSwipe.menuAction(item("p", pinned: true)), .archive)
    }

    func testAQueuedOrFailedOutgoingItemCannotBeArchived() {
        let queued = outgoing("q", .queued, .queued)
        let failed = outgoing("f", .delivered, .failed)
        XCTAssertNil(ArchiveSwipe.swipeAction(queued, localDeviceId: localId))
        XCTAssertNil(ArchiveSwipe.swipeAction(failed, localDeviceId: localId))
        XCTAssertFalse(ArchiveSwipe.canArchive(queued, localDeviceId: localId))
        XCTAssertFalse(ArchiveSwipe.canArchive(failed, localDeviceId: localId))
        // Someone else's queued delivery does not concern this device.
        XCTAssertTrue(ArchiveSwipe.canArchive(queued, localDeviceId: "another-device"))
    }

    func testTheSwipeActionReachesTheCoreContractAndNothingElse() async throws {
        let core = RecordingCore()
        let before = plain("demo-text-1")

        let action = try XCTUnwrap(ArchiveSwipe.swipeAction(before, localDeviceId: localId))
        try await ArchiveSwipe.perform(core, action, itemId: before.id)
        XCTAssertEqual(core.calls, ["archive:demo-text-1"])

        let archived = item("demo-text-1", archived: true)
        let back = try XCTUnwrap(ArchiveSwipe.swipeAction(archived, localDeviceId: localId))
        XCTAssertEqual(back, .unarchive)
        try await ArchiveSwipe.perform(core, back, itemId: "demo-text-1")
        XCTAssertEqual(core.calls, ["archive:demo-text-1", "unarchive:demo-text-1"])
    }

    func testUndoingAnArchiveRestoresThePinThatItRemoved() async throws {
        let core = RecordingCore()
        try await ArchiveSwipe.undo(core, .archive, itemId: "demo-text-1", wasPinned: true)
        // Un-archived first, then pinned again (pinning an archived item would also unarchive it).
        XCTAssertEqual(core.calls, ["unarchive:demo-text-1", "pin:demo-text-1"])
    }

    func testUndoingAnArchiveOfAnUnpinnedItemJustUnarchives() async throws {
        let core = RecordingCore()
        try await ArchiveSwipe.undo(core, .archive, itemId: "demo-text-1", wasPinned: false)
        XCTAssertEqual(core.calls, ["unarchive:demo-text-1"])
    }

    func testUndoingAnUnarchiveArchivesAgain() async throws {
        let core = RecordingCore()
        try await ArchiveSwipe.undo(core, .unarchive, itemId: "demo-archived-link", wasPinned: false)
        XCTAssertEqual(core.calls, ["archive:demo-archived-link"])
        XCTAssertEqual(ArchiveAction.archive.inverse, .unarchive)
        XCTAssertEqual(ArchiveAction.unarchive.inverse, .archive)
    }

    func testUndoingAnUnarchiveNeverTouchesThePin() async throws {
        let core = RecordingCore()
        try await ArchiveSwipe.undo(core, .unarchive, itemId: "x", wasPinned: true)
        XCTAssertEqual(core.calls, ["archive:x"])
    }

    func testTheWholeCycleSwipeBannerUndoBringsTheItemBackToRecent() async throws {
        // The core is a recording double, so its effect is applied by hand to the item, as a real core would report it.
        let core = RecordingCore()
        var current = plain("demo-text-1")
        func recentIds() -> [String] { ids(FeedSections.split(FeedFiltering.apply([current], .all, "")).recent) }
        XCTAssertEqual(recentIds(), ["demo-text-1"])

        // Swipe (released past the threshold): the core archives, the card leaves Recent.
        let action = try XCTUnwrap(ArchiveSwipe.swipeAction(current, localDeviceId: localId))
        try await ArchiveSwipe.perform(core, action, itemId: current.id)
        let before = current
        current = item(current.id, archived: true)
        XCTAssertTrue(recentIds().isEmpty)

        // "Undo" of the banner: the inverse call, the card is back in Recent, not archived.
        try await ArchiveSwipe.undo(core, action, itemId: before.id, wasPinned: before.pinned)
        current = before
        XCTAssertEqual(recentIds(), ["demo-text-1"])
        XCTAssertEqual(core.calls, ["archive:demo-text-1", "unarchive:demo-text-1"])
        // And the swipe offered on it is "Archive" again (no stale state).
        XCTAssertEqual(ArchiveSwipe.swipeAction(current, localDeviceId: localId), .archive)
    }

    func testUndoingAnArchiveFromTheMenuOfAPinnedItemRepinsIt() async throws {
        let core = RecordingCore()
        let pinned = item("demo-text-1", pinned: true)
        try await ArchiveSwipe.perform(core, ArchiveSwipe.menuAction(pinned), itemId: pinned.id)
        try await ArchiveSwipe.undo(core, .archive, itemId: pinned.id, wasPinned: pinned.pinned)
        XCTAssertEqual(core.calls, ["archive:demo-text-1", "unarchive:demo-text-1", "pin:demo-text-1"])
        // Back in the Pinned section as the core reports it again.
        let sections = FeedSections.split(FeedFiltering.apply([pinned], .all, ""))
        XCTAssertEqual(ids(sections.pinned), ["demo-text-1"])
        XCTAssertTrue(sections.recent.isEmpty)
    }
}
