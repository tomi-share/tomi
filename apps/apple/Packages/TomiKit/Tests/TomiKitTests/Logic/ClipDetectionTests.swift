import XCTest
@testable import TomiKit

// Port of the clipping cases of apps/android/.../ui/feed/CardTapTest.kt (they test ui/render/ClipDetection.kt).
final class ClipDetectionTests: XCTestCase {
    func testContentTallerThanItsViewIsClipped() {
        // 220 pt at density 2.75 = 605 px visible; 900 px of content.
        XCTAssertTrue(ClipDetection.isClipped(contentHeight: 900, visibleHeight: 605))
    }

    func testContentThatFitsOrIsWithinRoundingIsNotClipped() {
        XCTAssertFalse(ClipDetection.isClipped(contentHeight: 300, visibleHeight: 300))
        XCTAssertFalse(ClipDetection.isClipped(contentHeight: 100, visibleHeight: 300))
        XCTAssertFalse(ClipDetection.isClipped(contentHeight: 302, visibleHeight: 300))
        XCTAssertTrue(ClipDetection.isClipped(contentHeight: 303, visibleHeight: 300))
    }

    func testAViewNotLaidOutYetIsNeverReportedAsClipped() {
        XCTAssertFalse(ClipDetection.isClipped(contentHeight: 900, visibleHeight: 0))
    }
}
