import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/feed/FeedSearchDisplayTest.kt and CardFooterLogicTest.kt.

private let office = DeviceRef(id: "pc", name: "PC-Bureau")

private func item(_ id: String, secret: Bool = false) -> TomiItem {
    TomiItem.readable(id: id, sourceDevice: office, createdAt: Date(timeIntervalSince1970: 1_800_000_000),
                      text: RichText(plain: id), secret: secret)
}

final class FeedSearchDisplayTests: XCTestCase {
    func testCounterIsHiddenWithoutQueryAndWithTheAllFilter() {
        XCTAssertFalse(FeedSearchDisplay.showsCounter(query: "", filter: .all))
        XCTAssertFalse(FeedSearchDisplay.showsCounter(query: "   ", filter: .all))
    }

    func testCounterIsShownForAQueryOrAFilter() {
        XCTAssertTrue(FeedSearchDisplay.showsCounter(query: "wifi", filter: .all))
        XCTAssertTrue(FeedSearchDisplay.showsCounter(query: "", filter: .links))
        XCTAssertTrue(FeedSearchDisplay.showsCounter(query: "", filter: .archives))
        XCTAssertTrue(FeedSearchDisplay.showsCounter(query: "wifi", filter: .secrets))
    }

    func testMaskedNoteNeedsAQueryAndAHiddenItemAmongTheResults() {
        let plain = item("a")
        let hidden = item("b", secret: true)
        XCTAssertTrue(FeedSearchDisplay.showsMaskedNote(query: "pc", results: [plain, hidden]))
        XCTAssertFalse(FeedSearchDisplay.showsMaskedNote(query: "pc", results: [plain]))
        XCTAssertFalse(FeedSearchDisplay.showsMaskedNote(query: "pc", results: []))
        // No query: no search, so no note even with a hidden item in the list (a filter alone).
        XCTAssertFalse(FeedSearchDisplay.showsMaskedNote(query: "", results: [hidden]))
        XCTAssertFalse(FeedSearchDisplay.showsMaskedNote(query: "  ", results: [hidden]))
    }

    func testResultsAreFlatOnlyForARealQuery() {
        XCTAssertTrue(FeedSearchDisplay.isFlat(query: "wifi"))
        XCTAssertFalse(FeedSearchDisplay.isFlat(query: ""))
        XCTAssertFalse(FeedSearchDisplay.isFlat(query: " "))
    }
}

final class CardFooterLogicTests: XCTestCase {
    func testFullDeliveryIsLeftOutOfTheFeedCardsButKeptOnTheDetail() {
        let complete = DeliveryStatus.delivered(delivered: 2, total: 2)
        XCTAssertTrue(CardFooterLogic.isFullyDelivered(complete))
        XCTAssertFalse(CardFooterLogic.showsDeliveryStatus(complete, expanded: false))
        XCTAssertTrue(CardFooterLogic.showsDeliveryStatus(complete, expanded: true))
    }

    func testOtherStatesStayVisibleEverywhere() {
        let states: [DeliveryStatus] = [
            .delivered(delivered: 1, total: 2), .queued, .sent, .failed,
            .expiredBeforeDelivery(delivered: 1, deviceNames: ["PC-Maison"]),
        ]
        for status in states {
            XCTAssertFalse(CardFooterLogic.isFullyDelivered(status), "\(status)")
            XCTAssertTrue(CardFooterLogic.showsDeliveryStatus(status, expanded: false), "\(status) feed")
            XCTAssertTrue(CardFooterLogic.showsDeliveryStatus(status, expanded: true), "\(status) detail")
        }
    }

    func testAReceivedItemHasNoDeliveryLine() {
        XCTAssertFalse(CardFooterLogic.showsDeliveryStatus(.received, expanded: false))
        XCTAssertFalse(CardFooterLogic.showsDeliveryStatus(.received, expanded: true))
    }
}
