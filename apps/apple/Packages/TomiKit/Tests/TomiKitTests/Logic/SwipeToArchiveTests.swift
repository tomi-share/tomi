import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/feed/SwipeToArchiveTest.kt (logic parts).
//
// Not ported (Compose only, the gesture of the iOS app is not in this package): the cases that drive a real
// `AnchoredDraggableState` with a frame clock (`aNewStateStartsClosed`, `releasedBelowTheOpenStage...` and the following
// settle cases, `withReducedMotionTheSettleIsImmediate`, `aTapOnThePillAnimatesTheOpenCardOut...`). Their decision part
// (which anchor the card settles on for a given distance, whatever the velocity) is covered through `settleTarget`
// and `anchors`; the velocity cannot matter because `settleTarget` does not take one.
final class SwipeToArchiveTests: XCTestCase {
    private let width: Float = 1000
    private let openStage: Float = 100 // the open stage (88 pt in the app); threshold = 580
    private let hysteresis: Float = 30

    private func zone(_ was: SwipeZone, _ reveal: Float) -> SwipeZone {
        SwipeArchiveLogic.nextZone(was: was, reveal: reveal, width: width, openPx: openStage, hysteresis: hysteresis)
    }

    private func target(_ was: SwipeZone, _ reveal: Float) -> SwipeValue {
        SwipeArchiveLogic.settleTarget(was: was, reveal: reveal, width: width, openPx: openStage, hysteresis: hysteresis)
    }

    // ---- Anchors and threshold -----------------------------------------------------------------------------------------

    func testThereAreThreeAnchorsRestOpenAndFullyOutToTheLeftAndNothingOnTheRight() {
        let anchors = SwipeArchiveLogic.anchors(width: 1000, openPx: 100)
        XCTAssertEqual(anchors.count, 3)
        XCTAssertEqual(anchors.positionOf(.resting), 0)
        XCTAssertEqual(anchors.positionOf(.open), -100)
        XCTAssertEqual(anchors.positionOf(.out), -1000)
        // No anchor beyond rest: a drag to the right can not move the card.
        XCTAssertEqual(anchors.maxPosition, 0)
        XCTAssertEqual(anchors.minPosition, -1000)
    }

    func testTheOpenStageIsTheActionsZoneAndStaysUnderTheThreshold() {
        // 88 pt zone on a normal card: unchanged.
        XCTAssertEqual(SwipeArchiveLogic.openOffset(actionsWidth: 88, width: 400), 88)
        // On a very narrow card the open stage never goes past the arming threshold (stages stay ordered).
        XCTAssertEqual(SwipeArchiveLogic.openOffset(actionsWidth: 88, width: 100), 0.58 * 100, accuracy: 0.001)
        XCTAssertEqual(SwipeArchiveLogic.anchors(width: 100, openPx: 88).positionOf(.open), -0.58 * 100, accuracy: 0.001)
    }

    func testTheThresholdIs58PercentOfTheCardWidth() {
        XCTAssertEqual(SwipeArchiveLogic.positionalThreshold(1000), 580, accuracy: 0.001)
        XCTAssertEqual(SwipeArchiveLogic.positionalThreshold(360), 208.8, accuracy: 0.001)
        XCTAssertEqual(SwipeArchiveLogic.thresholdFraction, 0.58)
    }

    func testTheOpenStageMatchesTheLayoutOfThePill() {
        // The pill (48 pt) is centred in the 88 pt zone that Open reveals, and arrives completely before the zone is fully shown.
        XCTAssertEqual(SwipeArchiveLogic.actionsWidthDp, 88)
        XCTAssertEqual((SwipeArchiveLogic.actionsWidthDp - SwipeArchiveLogic.pillSizeDp) / 2, 20)
        XCTAssertEqual(SwipeArchiveLogic.arrival(reveal: SwipeArchiveLogic.actionsWidthDp, actionsWidth: SwipeArchiveLogic.actionsWidthDp), 1)
        // Not stretched at Open.
        XCTAssertEqual(SwipeArchiveLogic.stretch(reveal: SwipeArchiveLogic.actionsWidthDp, actionsWidth: SwipeArchiveLogic.actionsWidthDp), 0)
        XCTAssertGreaterThanOrEqual(SwipeArchiveLogic.pillSizeDp, SwipeArchiveLogic.minTouchDp) // 48 pt touch target
    }

    func testTheRevealIsTheLeftwardDistanceAndNeverNegativeOrNaN() {
        XCTAssertEqual(SwipeArchiveLogic.reveal(.nan), 0) // anchors not known yet
        XCTAssertEqual(SwipeArchiveLogic.reveal(0), 0)
        XCTAssertEqual(SwipeArchiveLogic.reveal(-250), 250)
        XCTAssertEqual(SwipeArchiveLogic.reveal(40), 0) // rightwards: nothing
    }

    // ---- Stages (visual + haptic) and decision at the release ----------------------------------------------------------

    func testTheStagesAreClosedBelowTheOpenStageOpenUpToTheThresholdThenArmed() {
        XCTAssertEqual(zone(.closed, 0), .closed)
        XCTAssertEqual(zone(.closed, 99), .closed)
        XCTAssertEqual(zone(.closed, 100), .open)
        XCTAssertEqual(zone(.closed, 579), .open)
        XCTAssertEqual(zone(.closed, 580), .armed)
        XCTAssertEqual(zone(.closed, 1000), .armed)
        XCTAssertEqual(SwipeArchiveLogic.nextZone(was: .closed, reveal: 900, width: 0, openPx: openStage, hysteresis: hysteresis), .closed) // unknown width
    }

    func testTheHysteresisAvoidsFlickerAroundEachStage() {
        // Armed: a few pt back is still armed (no second tick), further back it is Open again, and it needs the threshold again.
        XCTAssertEqual(zone(.armed, 560), .armed)
        XCTAssertEqual(zone(.armed, 550), .armed)
        XCTAssertEqual(zone(.armed, 549), .open)
        XCTAssertEqual(zone(.open, 560), .open)
        // Open: stays open a little below the open stage (a finger that trembles does not close it), then closes.
        XCTAssertEqual(zone(.open, 70), .open)
        XCTAssertEqual(zone(.open, 69), .closed)
        XCTAssertEqual(zone(.closed, 90), .closed) // and needs the full stage again to open
        // From far out, back to closed in one step.
        XCTAssertEqual(zone(.armed, 10), .closed)
    }

    func testOneTickPerChangeOfStageAndNoneBetweenOpenAndArmed() {
        var current = SwipeZone.closed
        var ticks: [SwipeZone] = []
        // closed -> open (tick) -> lots of travel inside Open (no tick) -> armed (tick) -> a few pt back (none) -> open (tick)
        for reveal in [50, 100, 150, 300, 570, 585, 595, 570, 555, 540, 400, 80, 60] as [Float] {
            let next = zone(current, reveal)
            if next != current { ticks.append(next) }
            current = next
        }
        XCTAssertEqual(ticks, [.open, .armed, .open, .closed])
    }

    func testTheReleaseDecisionFollowsTheStageAndIgnoresTheVelocity() {
        // Below the open stage: closed.
        XCTAssertEqual(target(.closed, 0), .resting)
        XCTAssertEqual(target(.closed, 99), .resting)
        // From the open stage to 58 %: open.
        XCTAssertEqual(target(.closed, 100), .open)
        XCTAssertEqual(target(.closed, 350), .open)
        XCTAssertEqual(target(.closed, 579), .open)
        // From 58 %: out (the action).
        XCTAssertEqual(target(.closed, 580), .out)
        XCTAssertEqual(target(.closed, 900), .out)
        // What the user saw (stage) is what happens, hysteresis included.
        XCTAssertEqual(target(.armed, 560), .out)
        XCTAssertEqual(target(.armed, 300), .open)
        XCTAssertEqual(target(.open, 80), .open)
        XCTAssertEqual(target(.open, 60), .resting) // a drag to the right closes it
        XCTAssertEqual(target(.armed, 10), .resting)
    }

    // Decision part of the Kotlin settle cases: the target, and the position it maps to on the anchors.

    func testReleasedBetweenTheOpenStageAndTheThresholdTheCardStaysOpenOnItsAnchor() {
        let anchors = SwipeArchiveLogic.anchors(width: width, openPx: openStage)
        for reveal: Float in [100, 250, 579] {
            XCTAssertEqual(target(.closed, reveal), .open, "reveal \(reveal)")
            XCTAssertEqual(anchors.positionOf(target(.closed, reveal)), -openStage, "reveal \(reveal)") // pill fully revealed, not stretched
        }
    }

    func testReleasedPastTheThresholdTheCardSettlesOnOut() {
        let anchors = SwipeArchiveLogic.anchors(width: width, openPx: openStage)
        XCTAssertEqual(target(.closed, 620), .out)
        XCTAssertEqual(anchors.positionOf(.out), -width)
    }

    func testAReleaseJustAroundEachStageDecidesOnTheStage() {
        XCTAssertEqual(target(.closed, 101), .open)
        XCTAssertEqual(target(.closed, 99), .resting)
        XCTAssertEqual(target(.closed, 581), .out)
        XCTAssertEqual(target(.closed, 579), .open)
    }

    func testFromOpenADragToTheRightClosesAndADragPastTheThresholdArchives() {
        XCTAssertEqual(target(.closed, 200), .open)
        // Same card, the finger comes back to the right (the app was in the Open stage): closed.
        XCTAssertEqual(target(.open, 40), .resting)
        // Open again, then dragged to the left past 58 %: out.
        XCTAssertEqual(target(.closed, 200), .open)
        XCTAssertEqual(target(.open, 700), .out)
    }

    // ---- Taps on an open card ------------------------------------------------------------------------------------------

    func testTheTapsOfAnOpenCardOnlyExistWhenSettledOpenAndNotLeaving() {
        XCTAssertTrue(SwipeArchiveLogic.isOpenForTaps(settled: .open, exiting: false))
        XCTAssertFalse(SwipeArchiveLogic.isOpenForTaps(settled: .open, exiting: true)) // the pill was tapped: no second tap
        XCTAssertFalse(SwipeArchiveLogic.isOpenForTaps(settled: .resting, exiting: false)) // closed card: the body opens the item
        XCTAssertFalse(SwipeArchiveLogic.isOpenForTaps(settled: .out, exiting: false))
    }

    func testTheTapOnThePillDeliversTheActionOnlyOnce() {
        // The pill tap and the full swipe share the path: arm at the settle on Out, deliver after the collapse, and the
        // disposal of the row right after (it left the list) must not deliver a second time.
        let delivery = PendingDelivery()
        var got: [ArchiveAction] = []
        delivery.arm(.archive)
        XCTAssertTrue(delivery.deliver { got.append($0) }) // after the collapse
        XCTAssertFalse(delivery.deliver { got.append($0) }) // onDispose
        // A double tap re-arms nothing: the card is already leaving.
        XCTAssertFalse(SwipeArchiveLogic.isOpenForTaps(settled: .open, exiting: true))
        XCTAssertEqual(got, [.archive])
    }

    // ---- Single delivery of the action ---------------------------------------------------------------------------------

    func testTheDecidedActionIsDeliveredExactlyOnce() {
        let delivery = PendingDelivery()
        var got: [ArchiveAction] = []
        XCTAssertFalse(delivery.deliver { got.append($0) }) // nothing decided: nothing delivered (disposal of a card never swiped)
        delivery.arm(.archive)
        XCTAssertTrue(delivery.deliver { got.append($0) })
        XCTAssertFalse(delivery.deliver { got.append($0) }) // the disposal after the exit does not deliver again
        XCTAssertEqual(got, [.archive])
    }

    func testANewDecisionAfterARestoreIsDeliveredAgain() {
        let delivery = PendingDelivery()
        var got: [ArchiveAction] = []
        delivery.arm(.archive)
        delivery.deliver { got.append($0) }
        delivery.arm(.unarchive)
        delivery.deliver { got.append($0) }
        XCTAssertEqual(got, [.archive, .unarchive])
    }

    // ---- Look: arrival, stretch, armed swell, timings --------------------------------------------------------------------

    func testThePillArrivesProgressivelyBetweenTheEntryRange() {
        let actions: Float = 88
        XCTAssertEqual(SwipeArchiveLogic.arrival(reveal: 0, actionsWidth: actions), 0)
        XCTAssertEqual(SwipeArchiveLogic.arrival(reveal: actions * SwipeArchiveLogic.arrivalStart, actionsWidth: actions), 0, accuracy: 0.0001)
        XCTAssertEqual(SwipeArchiveLogic.arrival(reveal: actions * SwipeArchiveLogic.arrivalEnd, actionsWidth: actions), 1, accuracy: 0.0001)
        XCTAssertEqual(SwipeArchiveLogic.arrival(reveal: 500, actionsWidth: actions), 1)
        let mid = SwipeArchiveLogic.arrival(reveal: actions * 0.5, actionsWidth: actions)
        XCTAssertTrue(mid > 0 && mid < 1)
    }

    func testTheWholePillZoomsFrom78PercentToFullAndNeverSwells() {
        XCTAssertEqual(SwipeArchiveLogic.pillScale(0), 0.78, accuracy: 0.0001)
        XCTAssertEqual(SwipeArchiveLogic.pillScale(1), 1, accuracy: 0.0001)
        XCTAssertEqual(SwipeArchiveLogic.pillScale(0.5), 0.89, accuracy: 0.0001)
        // Fully arrived, the scale stays 1 whatever the reveal: no swell once armed.
        let actions: Float = 88
        for reveal: Float in [actions, 300, 580, 1000] {
            XCTAssertEqual(SwipeArchiveLogic.pillScale(SwipeArchiveLogic.arrival(reveal: reveal, actionsWidth: actions)), 1)
        }
    }

    // ---- Haptic tick (pure rule; the feedback generator itself lives on the app side) ------------------------------------

    func testOnlyEnteringTheArmedZoneVibrates() {
        XCTAssertTrue(SwipeArchiveLogic.vibratesWhenEntering(.armed))
        XCTAssertFalse(SwipeArchiveLogic.vibratesWhenEntering(.open)) // the pill opens / back to open from armed
        XCTAssertFalse(SwipeArchiveLogic.vibratesWhenEntering(.closed)) // closing by a drag
    }

    func testAFullGestureVibratesOnceAtTheCrossingOf58PercentAndNowhereElse() {
        var current = SwipeZone.closed
        var ticks = 0
        var stagesSeen: [SwipeZone] = []
        // closed -> open (silent) -> armed (tick) -> a few pt back, inside the hysteresis (none) -> open (silent) -> closed (silent)
        for reveal in [50, 100, 300, 585, 570, 540, 60] as [Float] {
            let next = zone(current, reveal)
            if next != current {
                stagesSeen.append(next)
                if SwipeArchiveLogic.vibratesWhenEntering(next) { ticks += 1 }
            }
            current = next
        }
        XCTAssertEqual(stagesSeen, [.open, .armed, .open, .closed])
        XCTAssertEqual(ticks, 1)
    }

    func testWobblingAroundTheThresholdVibratesOnlyOnceThanksToTheHysteresis() {
        var current = SwipeZone.open
        var ticks = 0
        let threshold = width * SwipeArchiveLogic.thresholdFraction
        // Cross, then jitter within the hysteresis band: a single tick.
        for reveal in [threshold - 2, threshold + 1, threshold - 3, threshold + 2, threshold - hysteresis + 1, threshold + 5] {
            let next = zone(current, reveal)
            if next != current && SwipeArchiveLogic.vibratesWhenEntering(next) { ticks += 1 }
            current = next
        }
        XCTAssertEqual(ticks, 1)
        // Going all the way back below the band and crossing again is a new crossing: a second tick.
        for reveal in [threshold - hysteresis - 5, threshold + 1] {
            let next = zone(current, reveal)
            if next != current && SwipeArchiveLogic.vibratesWhenEntering(next) { ticks += 1 }
            current = next
        }
        XCTAssertEqual(ticks, 2)
    }

    // ---- Icon: centred in the pill up to 58 %, takes off to the left from there, stuck to the left edge from 75 % -------

    private let pillPx: Float = 48 // pillSizeDp, in the units of the test
    private let iconPx: Float = 24 // the icon (24 pt), never resized

    // Distance of the icon to its place at the open stage, ON SCREEN, positive = to the right: centre of the stretching pill
    // (the pill grows to the left, its centre moves left by stretch / 2) + the offset of the icon inside the pill.
    private func iconScreenX(_ reveal: Float, _ actions: Float, _ width: Float) -> Float {
        -SwipeArchiveLogic.stretch(reveal: reveal, actionsWidth: actions) / 2
            + SwipeArchiveLogic.iconOffsetInPill(reveal: reveal, actionsWidth: actions, width: width)
    }

    private func offset(_ reveal: Float, _ actions: Float = 88, _ width: Float = 1000) -> Float {
        SwipeArchiveLogic.iconOffsetInPill(reveal: reveal, actionsWidth: actions, width: width)
    }

    private func stretch(_ reveal: Float, _ actions: Float) -> Float {
        SwipeArchiveLogic.stretch(reveal: reveal, actionsWidth: actions)
    }

    // Distance between the left edge of the pill and the left edge of the icon: >= 0 means the icon is inside the pill.
    private func iconLeftMargin(_ reveal: Float, _ actions: Float, _ w: Float) -> Float {
        let pillWidth = pillPx + stretch(reveal, actions)
        return pillWidth / 2 + SwipeArchiveLogic.iconOffsetInPill(reveal: reveal, actionsWidth: actions, width: w) - iconPx / 2
    }

    func testThePillStretchesFromTheOpenStageAndNotFromTheThreshold() {
        let actions: Float = 88
        XCTAssertEqual(stretch(0, actions), 0)
        XCTAssertEqual(stretch(60, actions), 0)
        XCTAssertEqual(stretch(88, actions), 0)
        XCTAssertEqual(stretch(100, actions), 12, accuracy: 0.001)
        XCTAssertEqual(stretch(500, actions), 412, accuracy: 0.001)
        XCTAssertEqual(stretch(1000, actions), 912, accuracy: 0.001)
    }

    func testTheIconIsStrictlyCentredInThePillUpToThe58PercentThreshold() {
        let actions: Float = 88
        let threshold = width * SwipeArchiveLogic.thresholdFraction // 580
        // Rest, open stage, just past it, 50 % of the width, just under 58 %, and exactly 58 %: always the centre of the pill.
        for reveal in [0, 30, 60, actions, actions + 0.01, 100, 250, 334, width / 2, 570, threshold - 0.001, threshold] as [Float] {
            XCTAssertEqual(offset(reveal), 0, "offset at \(reveal)")
        }
        // On screen it simply follows the centre of the pill, which stretches to the left (no jump at Open).
        XCTAssertEqual(iconScreenX(actions + 0.01, actions, width), 0, accuracy: 0.01)
        for reveal in [150, 300, threshold] as [Float] {
            XCTAssertEqual(iconScreenX(reveal, actions, width), -stretch(reveal, actions) / 2, accuracy: 0.001)
        }
    }

    func testTheIconReachesTheLeftEdgeAt75PercentAndNotOnlyAtTheFullWidth() {
        XCTAssertEqual(SwipeArchiveLogic.reachFraction, 0.75)
        XCTAssertGreaterThan(SwipeArchiveLogic.reachFraction, SwipeArchiveLogic.thresholdFraction)
        XCTAssertLessThan(SwipeArchiveLogic.reachFraction, 1)
    }

    func testPastThe58PercentTheIconOvershootsTheCentreToTheLeftMoreAndMoreAndFasterUpTo75Percent() {
        let actions: Float = 88
        let threshold = width * SwipeArchiveLogic.thresholdFraction // 580
        let reach = width * SwipeArchiveLogic.reachFraction // 750
        var previous = offset(threshold)
        XCTAssertEqual(previous, 0)
        for reveal in [threshold + 1, 600, 650, 700, reach - 1, reach] {
            let inside = offset(reveal)
            XCTAssertLessThan(inside, 0, "reveal \(reveal): left of the centre")
            XCTAssertLessThan(inside, previous, "reveal \(reveal): |\(inside)| grows strictly")
            previous = inside
            // The icon never leaves the pill.
            XCTAssertGreaterThanOrEqual(iconLeftMargin(reveal, actions, width), -0.001, "reveal \(reveal)")
        }
        // At 75 %: stuck to the left edge, the centre of the icon half a pill from the left edge of the pill.
        let stretchAtReach = stretch(reach, actions)
        XCTAssertEqual(offset(reach), -stretchAtReach / 2, accuracy: 0.001)
        XCTAssertEqual((pillPx + stretchAtReach) / 2 + offset(reach), pillPx / 2, accuracy: 0.001)
        // Almost stuck a little before, still moving: not at the edge yet at 70 %.
        XCTAssertGreaterThan(iconLeftMargin(700, actions, width), iconLeftMargin(reach, actions, width) + 1)
        // Faster than before the threshold: slope (pt of offset per pt of reveal), measured 10 pt each side of it.
        let slopeBefore = (offset(threshold) - offset(threshold - 10)) / 10
        let slopeAfter = (offset(threshold + 10) - offset(threshold)) / 10
        XCTAssertEqual(slopeBefore, 0, "no slope before (centred)")
        XCTAssertLessThan(slopeAfter, -1, "slope after \(slopeAfter) is steep")
    }

    func testFrom75PercentToTheFullWidthTheIconStaysStuckToTheLeftEdgeOfTheGrowingPill() {
        let actions: Float = 88
        let reach = width * SwipeArchiveLogic.reachFraction
        for reveal in [reach, 850, 900, 950, 999, width, width + 50] {
            let s = stretch(reveal, actions)
            // Follows the left edge of the pill: always half a pill from it, never outside.
            XCTAssertEqual(offset(reveal), -s / 2, accuracy: 0.001, "reveal \(reveal)")
            XCTAssertEqual((pillPx + s) / 2 + offset(reveal), pillPx / 2, accuracy: 0.001, "reveal \(reveal)")
            XCTAssertGreaterThanOrEqual(iconLeftMargin(reveal, actions, width), -0.001, "reveal \(reveal)")
        }
        // At the full width: the old target, the centre of the semicircle at the left end of the final pill.
        XCTAssertEqual(offset(width), -stretch(width, actions) / 2, accuracy: 0.001)
        // On screen the icon keeps going left with the edge of the pill (no stop, no jump back).
        var previousX = iconScreenX(reach, actions, width)
        for reveal in [850, 900, 950, width] as [Float] {
            let x = iconScreenX(reveal, actions, width)
            XCTAssertLessThan(x, previousX, "reveal \(reveal)")
            previousX = x
        }
    }

    func testThePositionOfTheIconHasNoJumpAtThe58PercentThreshold() {
        let actions: Float = 88
        let threshold = width * SwipeArchiveLogic.thresholdFraction
        let atThreshold = iconScreenX(threshold, actions, width)
        for eps: Float in [1, 0.1, 0.001] {
            // Screen speed: -0.5 pt per pt of reveal just before the threshold (the pill centre), about -2.5 just after: the
            // speed jumps (the icon takes off), the position does not.
            XCTAssertEqual(iconScreenX(threshold - eps, actions, width), atThreshold, accuracy: 0.6 * eps, "before by \(eps)")
            XCTAssertEqual(iconScreenX(threshold + eps, actions, width), atThreshold, accuracy: 2.6 * eps, "after by \(eps)")
        }
        XCTAssertEqual(offset(threshold), 0)
        XCTAssertEqual(offset(threshold + 0.001), offset(threshold), accuracy: 0.002)
        XCTAssertEqual(offset(threshold - 0.001), offset(threshold), accuracy: 0.002)
    }

    func testThePositionOfTheIconHasNoJumpAt75Percent() {
        let actions: Float = 88
        let reach = width * SwipeArchiveLogic.reachFraction
        let atReach = iconScreenX(reach, actions, width)
        for eps: Float in [1, 0.1, 0.001] {
            // Screen speed about -2.5 just before, -1 just after (the icon follows the edge of the pill): finite slopes, no jump.
            XCTAssertEqual(iconScreenX(reach - eps, actions, width), atReach, accuracy: 2.6 * eps, "before by \(eps)")
            XCTAssertEqual(iconScreenX(reach + eps, actions, width), atReach, accuracy: 1.1 * eps, "after by \(eps)")
        }
        XCTAssertEqual(offset(reach + 0.001), offset(reach), accuracy: 0.002)
        XCTAssertEqual(offset(reach - 0.001), offset(reach), accuracy: 0.002)
    }

    func testTheIconNeverLeavesThePillFromTheOpenStageToBeyondTheFullWidth() {
        let actions: Float = 88
        var reveal: Float = 0
        while reveal <= width + 100 {
            XCTAssertGreaterThanOrEqual(iconLeftMargin(reveal, actions, width), -0.001, "reveal \(reveal)")
            reveal += 3
        }
    }

    func testTheIconBehavesTheSameOnAPhoneSizedCard() {
        let width: Float = 360 // threshold 208.8, reach 270
        let actions: Float = 88
        let threshold = width * SwipeArchiveLogic.thresholdFraction
        let reach = width * SwipeArchiveLogic.reachFraction
        XCTAssertEqual(offset(actions, actions, width), 0)
        XCTAssertEqual(offset(threshold, actions, width), 0)
        XCTAssertEqual(offset(150, actions, width), 0)
        XCTAssertEqual(offset(200, actions, width), 0)
        XCTAssertLessThan(offset(260, actions, width), 0)
        XCTAssertLessThan(offset(280, actions, width), offset(260, actions, width))
        XCTAssertEqual(offset(reach, actions, width), -stretch(reach, actions) / 2, accuracy: 0.001)
        XCTAssertEqual(offset(320, actions, width), -stretch(320, actions) / 2, accuracy: 0.001)
        XCTAssertEqual(offset(width, actions, width), -stretch(width, actions) / 2, accuracy: 0.001)
        for reveal in [150, threshold, 250, reach, 330, width] {
            XCTAssertGreaterThanOrEqual(iconLeftMargin(reveal, actions, width), -0.001, "reveal \(reveal)")
        }
    }

    func testTheIconMovementIsSafeOnAnUnmeasuredOrVeryNarrowCard() {
        XCTAssertEqual(offset(500, 88, 0), 0)
        XCTAssertEqual(offset(0, 88, 0), 0)
        // Narrow card: threshold (58) and reach (75) are below the open stage (88): no stretch, no offset in between, no NaN,
        // stuck to the left edge beyond.
        XCTAssertEqual(offset(70, 88, 100), 0)
        XCTAssertEqual(offset(88, 88, 100), 0)
        XCTAssertEqual(offset(100, 88, 100), -stretch(100, 88) / 2, accuracy: 0.001)
        XCTAssertFalse(offset(95, 88, 100).isNaN)
        for reveal: Float in [0, 58, 82, 88, 90, 100, 150] {
            XCTAssertGreaterThanOrEqual(iconLeftMargin(reveal, 88, 100), -0.001, "reveal \(reveal)")
        }
    }

    func testTheExitHasNoAnimationWithReducedMotion() {
        XCTAssertTrue(SwipeArchiveLogic.exitTimings(reduceMotion: true) == (0, 0))
        XCTAssertTrue(SwipeArchiveLogic.exitTimings(reduceMotion: false) == (220, 180))
    }

    func testTheReturnSpringMatchesTheReference() {
        // T3: damping 26, stiffness 330 (mass 1) -> damping ratio about 0.716.
        XCTAssertEqual(SwipeArchiveLogic.returnDampingRatio, 0.716, accuracy: 0.001)
        XCTAssertEqual(SwipeArchiveLogic.returnStiffness, 330)
    }
}
