import XCTest
@testable import TomiKit

// Port of the UI parts of apps/android/app/src/test/java/eu/shaffe/tomi/core/PinTest.kt: the "Pinned" section built by the
// UI and the filtering inside the sections.
//
// Not ported (storage semantics of FakeTomiCore, now tested in the Rust core): pinSetsPinnedAndPinnedAt...,
// pinAndUnpinAreIdempotent..., pinningNeverReordersTheCoreFlow..., aHiddenAndAnUnreadableItemCanBePinned.

private let base = ISO8601DateFormatter().date(from: "2026-11-04T10:00:00Z")!
private let office = DeviceRef(id: "pc", name: "PC-Bureau")

final class PinTests: XCTestCase {
    func testTheSectionsPutPinnedItemsFirstMostRecentlyPinnedFirst() {
        func item(_ id: String, _ minutes: Double, pinnedAtMinutes: Double? = nil) -> TomiItem {
            TomiItem.readable(
                id: id, sourceDevice: office, createdAt: base.addingTimeInterval(minutes * 60), text: RichText(plain: id),
                pinned: pinnedAtMinutes != nil, pinnedAt: pinnedAtMinutes.map { base.addingTimeInterval($0 * 60) }
            )
        }
        // Newest first, as the core delivers them.
        let items = [item("new", 50), item("pinned-old", 10, pinnedAtMinutes: 5), item("mid", 30), item("pinned-new", 5, pinnedAtMinutes: 60)]
        let sections = FeedSections.split(items)
        XCTAssertEqual(sections.pinned.map(\.id), ["pinned-new", "pinned-old"])
        XCTAssertEqual(sections.recent.map(\.id), ["new", "mid"])
        XCTAssertFalse(sections.isEmpty)
        XCTAssertTrue(FeedSections.split([]).isEmpty)
    }

    func testTiesAndAMissingPinnedAtFallBackToTheCreationTime() {
        func pinned(_ id: String, created: Double, pinnedAt: Double?) -> TomiItem {
            TomiItem.readable(
                id: id, sourceDevice: office, createdAt: base.addingTimeInterval(created * 60), text: RichText(plain: id),
                pinned: true, pinnedAt: pinnedAt.map { base.addingTimeInterval($0 * 60) }
            )
        }
        let items = [
            pinned("tie-old", created: 1, pinnedAt: 10), pinned("none-a", created: 3, pinnedAt: nil),
            pinned("tie-new", created: 2, pinnedAt: 10), pinned("none-b", created: 4, pinnedAt: nil),
        ]
        // Same pinnedAt: newest creation first; no pinnedAt: after every dated pin, newest creation first.
        XCTAssertEqual(FeedSections.split(items).pinned.map(\.id), ["tie-new", "tie-old", "none-b", "none-a"])
        // Full tie: the order of the core is kept.
        let same = [pinned("x", created: 1, pinnedAt: 1), pinned("y", created: 1, pinnedAt: 1), pinned("z", created: 1, pinnedAt: 1)]
        XCTAssertEqual(FeedSections.split(same).pinned.map(\.id), ["x", "y", "z"])
    }

    func testFilteringAndSearchApplyInsideTheSections() {
        let link = TomiItem.readable(id: "l", sourceDevice: office, createdAt: base, url: "https://a.example", pinned: true, pinnedAt: base)
        let text = TomiItem.readable(id: "t", sourceDevice: office, createdAt: base, text: RichText(plain: "hello"))
        let sections = FeedSections.split(FeedFiltering.apply([link, text], .links, ""))
        XCTAssertEqual(sections.pinned.map(\.id), ["l"])
        XCTAssertTrue(sections.recent.isEmpty)
    }
}
