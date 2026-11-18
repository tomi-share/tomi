import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/send/ThumbnailSizeTest.kt, plus the budget of
// `OutgoingDraft.toFfi` (core/RealCoreMapping.kt).
final class ThumbnailSizeTests: XCTestCase {
    func testATallScreenshotIsCutToItsCentreAndKeepsItsWidth() {
        let plan = ThumbnailSize.plan(width: 1080, height: 2340)
        XCTAssertEqual(plan.cropHeight, 1440)
        XCTAssertEqual(plan.top, (2340 - 1440) / 2)
        XCTAssertTrue(plan.size(width: 1080) == (600, 800))
    }

    func testALandscapePhotoIsOnlyScaledDown() {
        let plan = ThumbnailSize.plan(width: 4000, height: 3000)
        XCTAssertEqual(plan.top, 0)
        XCTAssertEqual(plan.cropHeight, 3000)
        XCTAssertTrue(plan.size(width: 4000) == (600, 450))
    }

    func testASmallImageIsLeftAsItIs() {
        XCTAssertTrue(ThumbnailSize.plan(width: 100, height: 50).isIdentity(width: 100, height: 50))
    }

    func testAThinImageKeepsAtLeastOnePixel() {
        XCTAssertTrue(ThumbnailSize.plan(width: 10_000, height: 3).size(width: 10_000) == (600, 1))
    }

    func testAHiddenItemCarriesNoThumbnail() {
        XCTAssertEqual(ThumbnailBudget.apply([Data([1]), nil], secret: true), [nil, nil])
    }

    func testThumbnailsStayUnderTheBudgetOfTheItem() {
        let big = Data(count: 150 * 1024)
        let small = Data(count: 10 * 1024)
        // 150 + 150 = 300 Ko; a third 150 Ko does not fit (84 Ko left), a 10 Ko one still does.
        let kept = ThumbnailBudget.apply([big, big, big, small], secret: false)
        XCTAssertEqual(kept.map { $0?.count }, [big.count, big.count, nil, small.count])
    }
}
