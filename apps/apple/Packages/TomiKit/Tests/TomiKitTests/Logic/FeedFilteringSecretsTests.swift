import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/feed/FeedFilteringSecretsTest.kt: the type filters must
// not class a hidden item by its content, it stays under "Masqués".

private let base = ISO8601DateFormatter().date(from: "2026-11-04T10:00:00Z")!
private let file = Attachment(name: "secret.txt", mime: "text/plain", size: 3)

private func item(_ id: String, secret: Bool, url: String? = nil, text: String? = nil, files: [Attachment] = []) -> TomiItem {
    TomiItem.readable(
        id: id, sourceDevice: DeviceRef(id: "pc", name: "PC"), createdAt: base,
        text: text.map { RichText(plain: $0) }, url: url, attachments: files, secret: secret
    )
}

final class FeedFilteringSecretsTests: XCTestCase {
    func testAHiddenItemIsNotClassedByItsContentUnderTheTypeFilters() {
        let hidden = item("s", secret: true, url: "https://example.org", text: "mot de passe", files: [file])
        XCTAssertFalse(FeedFiltering.matches(hidden, .links))
        XCTAssertFalse(FeedFiltering.matches(hidden, .text))
        XCTAssertFalse(FeedFiltering.matches(hidden, .files))
        XCTAssertTrue(FeedFiltering.matches(hidden, .secrets))
        XCTAssertTrue(FeedFiltering.matches(hidden, .all))
    }

    func testAVisibleItemStillMatchesItsTypeFilters() {
        let visible = item("v", secret: false, url: "https://example.org", text: "hello", files: [file])
        XCTAssertTrue(FeedFiltering.matches(visible, .links))
        XCTAssertTrue(FeedFiltering.matches(visible, .text))
        XCTAssertTrue(FeedFiltering.matches(visible, .files))
        XCTAssertFalse(FeedFiltering.matches(visible, .secrets))
    }

    func testTheLinksFilterListsOnlyVisibleLinks() {
        let items = [item("link", secret: false, url: "https://a.example"), item("hidden-link", secret: true, url: "https://b.example")]
        XCTAssertEqual(FeedFiltering.apply(items, .links, "").map(\.id), ["link"])
    }
}
