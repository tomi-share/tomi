import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/feed/SeenTrackerTest.kt.
final class SeenTrackerTests: XCTestCase {
    func testAKeyIsDueOnlyAfterTheDwell() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["a"], now: 0)
        XCTAssertTrue(t.takeDue(now: 999).isEmpty)
        XCTAssertEqual(t.takeDue(now: 1_000), ["a"])
    }

    func testAKeyIsReportedOnceUntilItLeavesTheScreen() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["a"], now: 0)
        XCTAssertEqual(t.takeDue(now: 1_000), ["a"])
        XCTAssertTrue(t.takeDue(now: 5_000).isEmpty)
        XCTAssertNil(t.nextDelay(now: 5_000))
        t.update(onScreen: [], now: 6_000)
        t.update(onScreen: ["a"], now: 7_000)
        XCTAssertEqual(t.takeDue(now: 8_000), ["a"])
    }

    func testAQuickScrollPastReadsNothing() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["a"], now: 0)
        t.update(onScreen: [], now: 400)
        XCTAssertTrue(t.takeDue(now: 2_000).isEmpty)
        XCTAssertNil(t.nextDelay(now: 2_000))
    }

    func testEachKeyKeepsItsOwnStartWhenTheSetChanges() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["a"], now: 0)
        t.update(onScreen: ["a", "b"], now: 600)
        XCTAssertEqual(t.nextDelay(now: 600), 400)
        XCTAssertEqual(t.takeDue(now: 1_000), ["a"])
        XCTAssertEqual(t.nextDelay(now: 1_000), 600)
        XCTAssertEqual(t.takeDue(now: 1_600), ["b"])
    }

    func testResetForgetsTheTimeSpentWhileNotAttended() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["a"], now: 0)
        t.reset()
        t.update(onScreen: ["a"], now: 900)
        XCTAssertTrue(t.takeDue(now: 1_000).isEmpty)
        XCTAssertEqual(t.takeDue(now: 1_900), ["a"])
    }

    func testSeveralKeysDueTogetherComeOutSorted() {
        var t = SeenTracker(dwellMs: 1_000)
        t.update(onScreen: ["b", "c", "a"], now: 0)
        XCTAssertEqual(t.takeDue(now: 1_000), ["a", "b", "c"])
        XCTAssertTrue(t.takeDue(now: 1_000).isEmpty)
    }

    func testTheDefaultDwellIsOneAndAHalfSeconds() {
        var t = SeenTracker()
        t.update(onScreen: ["a"], now: 0)
        XCTAssertEqual(t.nextDelay(now: 0), 1_500)
        XCTAssertTrue(t.takeDue(now: 1_499).isEmpty)
        XCTAssertEqual(t.takeDue(now: 1_500), ["a"])
    }

    func testACardNeedsMostOfItselfOnScreen() {
        // Free zone 0..1000, card of 200.
        XCTAssertTrue(SeenTracker.visibleEnough(offset: 0, size: 200, top: 0, bottom: 1000))
        XCTAssertTrue(SeenTracker.visibleEnough(offset: 880, size: 200, top: 0, bottom: 1000)) // 120/200 = 60 %
        XCTAssertFalse(SeenTracker.visibleEnough(offset: 900, size: 200, top: 0, bottom: 1000)) // 50 %
        XCTAssertFalse(SeenTracker.visibleEnough(offset: -100, size: 200, top: 0, bottom: 1000)) // 50 %
        XCTAssertFalse(SeenTracker.visibleEnough(offset: 1200, size: 200, top: 0, bottom: 1000))
    }

    func testACardTallerThanTheZoneNeedsMostOfTheZone() {
        // Card of 1500 in a 1000 zone: 600 of it on screen is enough.
        XCTAssertTrue(SeenTracker.visibleEnough(offset: -900, size: 1_500, top: 0, bottom: 1_000))
        XCTAssertFalse(SeenTracker.visibleEnough(offset: -1_000, size: 1_500, top: 0, bottom: 1_000))
    }

    func testNothingIsVisibleInAnEmptyZoneOrForAnEmptyCard() {
        XCTAssertFalse(SeenTracker.visibleEnough(offset: 0, size: 200, top: 500, bottom: 500))
        XCTAssertFalse(SeenTracker.visibleEnough(offset: 0, size: 0, top: 0, bottom: 1_000))
    }
}
