import XCTest
@testable import TomiKit

// Port of the UndoDeadline parts of apps/android/app/src/test/java/eu/shaffe/tomi/send/UndoSendTest.kt.
//
// Not ported: the SendCoordinator / SendScheduling / FakeTomiCore cases (submission and job share the deadline, undo within
// the window, undo too late, undo racing with the dispatch, cancelIfQueued), which are not in this batch.
final class UndoSendTests: XCTestCase {
    func testDeadlineFractionAndRemainingTimeFollowTheRealTimeClock() {
        let deadline = UndoDeadline.startingAt(1_000, windowMs: 5_000)
        XCTAssertEqual(deadline.atElapsedMs, 6_000)
        XCTAssertEqual(deadline.fraction(1_000), 1)
        XCTAssertEqual(deadline.fraction(3_500), 0.5, accuracy: 0.0001)
        XCTAssertEqual(deadline.fraction(6_000), 0)
        XCTAssertEqual(deadline.fraction(9_000), 0)
        XCTAssertEqual(deadline.remainingMs(3_500), 2_500)
        XCTAssertEqual(deadline.remainingMs(9_000), 0)
        XCTAssertFalse(deadline.isOver(5_999))
        XCTAssertTrue(deadline.isOver(6_000))
    }

    func testTheBackgroundJobWaitsOutTheWindowButIgnoresAStaleClockAfterAReboot() {
        let deadline = UndoDeadline(atElapsedMs: 6_000, windowMs: 5_000)
        XCTAssertEqual(deadline.waitBeforeDispatchMs(4_000), 2_000)
        XCTAssertEqual(deadline.waitBeforeDispatchMs(6_000), 0)
        XCTAssertEqual(deadline.waitBeforeDispatchMs(60_000), 0)
        // The monotonic clock restarted from 0 after a reboot: the remaining time looks huge, so it is not waited for.
        let beforeReboot = UndoDeadline(atElapsedMs: 8_000_000, windowMs: 5_000)
        XCTAssertEqual(beforeReboot.waitBeforeDispatchMs(1_000), 0)
    }

    func testTheWindowBoundsAreInclusiveForTheWaitAndAnEmptyWindowNeverWaits() {
        let deadline = UndoDeadline(atElapsedMs: 6_000, windowMs: 5_000)
        XCTAssertEqual(deadline.waitBeforeDispatchMs(1_000), 5_000) // exactly the window
        XCTAssertEqual(deadline.waitBeforeDispatchMs(999), 0) // one more than the window: stale clock
        XCTAssertEqual(deadline.waitBeforeDispatchMs(5_999), 1)
        let empty = UndoDeadline.startingAt(1_000, windowMs: 0)
        XCTAssertEqual(empty.fraction(1_000), 0)
        XCTAssertEqual(empty.waitBeforeDispatchMs(1_000), 0)
        XCTAssertTrue(empty.isOver(1_000))
    }

    func testTheDeadlineStartsFromTheInjectedClock() {
        let clock = Clock()
        clock.now = 10_000
        let deadline = UndoDeadline.startingAt(clock: { clock.now }, windowMs: 5_000)
        XCTAssertEqual(deadline.atElapsedMs, 15_000)
        XCTAssertEqual(deadline.windowMs, 5_000)
        clock.now = 12_500
        XCTAssertEqual(deadline.remainingMs(clock.now), 2_500)
        XCTAssertFalse(deadline.isOver(clock.now))
    }
}
