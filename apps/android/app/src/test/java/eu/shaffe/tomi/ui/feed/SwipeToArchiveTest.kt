package eu.shaffe.tomi.ui.feed

import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.ScrollScope
import androidx.compose.foundation.gestures.animateTo
import androidx.compose.runtime.MonotonicFrameClock
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Anchors, stages, settle and look of the two-step swipe to archive (closed, open, out). The gesture itself
 * (`Modifier.anchoredDraggable`, axis lock, conflict with the vertical scroll) is Compose foundation code and is validated on a
 * device; what is ours is tested here: the three anchors, the stages (open stage, 58 % threshold, hysteresis), the decision at the
 * release, the settle of a real `AnchoredDraggableState`, the haptic mapping, the animation styles, the single delivery.
 */
class SwipeToArchiveTest {
    private val width = 1000f
    private val open = 100f // the open stage (88 dp in the app); threshold = 580
    private val hysteresis = 30f

    /** Frames of 16 ms that come at once, so that animations run to the end in virtual time. */
    private val frames = object : MonotonicFrameClock {
        private var nanos = 0L
        override suspend fun <R> withFrameNanos(onFrame: (Long) -> R): R {
            nanos += 16_000_000L
            return onFrame(nanos)
        }
    }

    private fun stateOfWidth(w: Float = width): AnchoredDraggableState<SwipeValue> =
        SwipeArchiveLogic.newState().also { it.updateAnchors(SwipeArchiveLogic.anchors(w, open)) }

    private fun zone(was: SwipeZone, reveal: Float) = SwipeArchiveLogic.nextZone(was, reveal, width, open, hysteresis)

    private fun target(was: SwipeZone, reveal: Float) = SwipeArchiveLogic.settleTarget(was, reveal, width, open, hysteresis)

    /**
     * Drags the card to [reveal] px to the left and lifts the finger, as the draggable node does: the settle runs in the drag.
     * [was] is the stage the card was in when the finger was lifted (the app tracks it while dragging).
     */
    private suspend fun dragAndRelease(
        state: AnchoredDraggableState<SwipeValue>,
        reveal: Float,
        velocity: Float = 0f,
        reduceMotion: Boolean = false,
        was: SwipeZone = SwipeZone.Closed,
    ) {
        state.anchoredDrag { dragTo(-reveal) } // the finger is down and moves; the offset follows
        val fling = ArchiveSettleFling(
            state = state,
            targetFor = { target(was, SwipeArchiveLogic.reveal(state.offset)) },
            reduceMotion = reduceMotion,
        )
        state.anchoredDrag {
            val scope = object : ScrollScope {
                override fun scrollBy(pixels: Float): Float {
                    val target = (state.offset + pixels).coerceIn(state.anchors.minPosition(), state.anchors.maxPosition())
                    val consumed = target - state.offset
                    dragTo(target)
                    return consumed
                }
            }
            with(fling) { scope.performFling(velocity) }
        }
    }

    // ---- Anchors and threshold ---------------------------------------------------------------------------------------------

    @Test
    fun thereAreThreeAnchorsRestOpenAndFullyOutToTheLeftAndNothingOnTheRight() {
        val anchors = SwipeArchiveLogic.anchors(1000f, 100f)
        assertEquals(3, anchors.size)
        assertEquals(0f, anchors.positionOf(SwipeValue.Resting), 0f)
        assertEquals(-100f, anchors.positionOf(SwipeValue.Open), 0f)
        assertEquals(-1000f, anchors.positionOf(SwipeValue.Out), 0f)
        // No anchor beyond rest: a drag to the right can not move the card.
        assertEquals(0f, anchors.maxPosition(), 0f)
        assertEquals(-1000f, anchors.minPosition(), 0f)
    }

    @Test
    fun theOpenStageIsTheActionsZoneAndStaysUnderTheThreshold() {
        // 88 dp zone on a normal card: unchanged.
        assertEquals(88f, SwipeArchiveLogic.openOffset(88f, 400f), 0f)
        // On a very narrow card the open stage never goes past the arming threshold (stages stay ordered).
        assertEquals(0.58f * 100f, SwipeArchiveLogic.openOffset(88f, 100f), 0.001f)
        assertEquals(-0.58f * 100f, SwipeArchiveLogic.anchors(100f, 88f).positionOf(SwipeValue.Open), 0.001f)
    }

    @Test
    fun theThresholdIs58PercentOfTheCardWidth() {
        assertEquals(580f, SwipeArchiveLogic.positionalThreshold(1000f), 0.001f)
        assertEquals(208.8f, SwipeArchiveLogic.positionalThreshold(360f), 0.001f)
        assertEquals(0.58f, SwipeArchiveLogic.THRESHOLD_FRACTION, 0f)
    }

    @Test
    fun theOpenStageMatchesTheLayoutOfThePill() {
        // The pill (48 dp) is centred in the 88 dp zone that Open reveals, and arrives completely before the zone is fully shown.
        assertEquals(88f, SwipeArchiveLogic.ACTIONS_WIDTH_DP, 0f)
        assertEquals(20f, (SwipeArchiveLogic.ACTIONS_WIDTH_DP - SwipeArchiveLogic.PILL_SIZE_DP) / 2f, 0f)
        assertEquals(1f, SwipeArchiveLogic.arrival(SwipeArchiveLogic.ACTIONS_WIDTH_DP, SwipeArchiveLogic.ACTIONS_WIDTH_DP), 0f)
        assertEquals(0f, SwipeArchiveLogic.stretch(SwipeArchiveLogic.ACTIONS_WIDTH_DP, SwipeArchiveLogic.ACTIONS_WIDTH_DP), 0f) // not stretched at Open
        assertTrue(SwipeArchiveLogic.PILL_SIZE_DP >= SwipeArchiveLogic.MIN_TOUCH_DP) // 48 dp touch target
    }

    @Test
    fun theRevealIsTheLeftwardDistanceAndNeverNegativeOrNaN() {
        assertEquals(0f, SwipeArchiveLogic.reveal(Float.NaN), 0f) // anchors not known yet
        assertEquals(0f, SwipeArchiveLogic.reveal(0f), 0f)
        assertEquals(250f, SwipeArchiveLogic.reveal(-250f), 0f)
        assertEquals(0f, SwipeArchiveLogic.reveal(40f), 0f) // rightwards: nothing
    }

    // ---- Stages (visual + haptic) and decision at the release --------------------------------------------------------------

    @Test
    fun theStagesAreClosedBelowTheOpenStageOpenUpToTheThresholdThenArmed() {
        assertEquals(SwipeZone.Closed, zone(SwipeZone.Closed, 0f))
        assertEquals(SwipeZone.Closed, zone(SwipeZone.Closed, 99f))
        assertEquals(SwipeZone.Open, zone(SwipeZone.Closed, 100f))
        assertEquals(SwipeZone.Open, zone(SwipeZone.Closed, 579f))
        assertEquals(SwipeZone.Armed, zone(SwipeZone.Closed, 580f))
        assertEquals(SwipeZone.Armed, zone(SwipeZone.Closed, 1000f))
        assertEquals(SwipeZone.Closed, SwipeArchiveLogic.nextZone(SwipeZone.Closed, 900f, 0f, open, hysteresis)) // unknown width
    }

    @Test
    fun theHysteresisAvoidsFlickerAroundEachStage() {
        // Armed: a few px back is still armed (no second tick), further back it is Open again, and it needs the threshold again.
        assertEquals(SwipeZone.Armed, zone(SwipeZone.Armed, 560f))
        assertEquals(SwipeZone.Armed, zone(SwipeZone.Armed, 550f))
        assertEquals(SwipeZone.Open, zone(SwipeZone.Armed, 549f))
        assertEquals(SwipeZone.Open, zone(SwipeZone.Open, 560f))
        // Open: stays open a little below the open stage (a finger that trembles does not close it), then closes.
        assertEquals(SwipeZone.Open, zone(SwipeZone.Open, 70f))
        assertEquals(SwipeZone.Closed, zone(SwipeZone.Open, 69f))
        assertEquals(SwipeZone.Closed, zone(SwipeZone.Closed, 90f)) // and needs the full stage again to open
        // From far out, back to closed in one step.
        assertEquals(SwipeZone.Closed, zone(SwipeZone.Armed, 10f))
    }

    @Test
    fun oneTickPerChangeOfStageAndNoneBetweenOpenAndArmed() {
        var current = SwipeZone.Closed
        val ticks = mutableListOf<SwipeZone>()
        // closed -> open (tick) -> lots of travel inside Open (no tick) -> armed (tick) -> a few px back (none) -> open (tick)
        for (reveal in listOf(50f, 100f, 150f, 300f, 570f, 585f, 595f, 570f, 555f, 540f, 400f, 80f, 60f)) {
            val next = zone(current, reveal)
            if (next != current) ticks += next
            current = next
        }
        assertEquals(listOf(SwipeZone.Open, SwipeZone.Armed, SwipeZone.Open, SwipeZone.Closed), ticks)
    }

    @Test
    fun theReleaseDecisionFollowsTheStageAndIgnoresTheVelocity() {
        // Below the open stage: closed.
        assertEquals(SwipeValue.Resting, target(SwipeZone.Closed, 0f))
        assertEquals(SwipeValue.Resting, target(SwipeZone.Closed, 99f))
        // From the open stage to 58 %: open.
        assertEquals(SwipeValue.Open, target(SwipeZone.Closed, 100f))
        assertEquals(SwipeValue.Open, target(SwipeZone.Closed, 350f))
        assertEquals(SwipeValue.Open, target(SwipeZone.Closed, 579f))
        // From 58 %: out (the action).
        assertEquals(SwipeValue.Out, target(SwipeZone.Closed, 580f))
        assertEquals(SwipeValue.Out, target(SwipeZone.Closed, 900f))
        // What the user saw (stage) is what happens, hysteresis included.
        assertEquals(SwipeValue.Out, target(SwipeZone.Armed, 560f))
        assertEquals(SwipeValue.Open, target(SwipeZone.Armed, 300f))
        assertEquals(SwipeValue.Open, target(SwipeZone.Open, 80f))
        assertEquals(SwipeValue.Resting, target(SwipeZone.Open, 60f)) // a drag to the right closes it
        assertEquals(SwipeValue.Resting, target(SwipeZone.Armed, 10f))
    }

    // ---- Settle of a real AnchoredDraggableState ---------------------------------------------------------------------------

    @Test
    fun aNewStateStartsClosed() {
        // Undo / a card that comes back is a new composition with a new state: closed, nothing restored.
        val state = stateOfWidth()
        assertEquals(SwipeValue.Resting, state.currentValue)
        assertEquals(SwipeValue.Resting, state.settledValue)
        assertEquals(0f, state.requireOffset(), 0f)
    }

    @Test
    fun releasedBelowTheOpenStageTheCardClosesAndNothingHappens() = runTest {
        withContext(frames) {
            val state = stateOfWidth()
            dragAndRelease(state, reveal = 99f)
            assertEquals(SwipeValue.Resting, state.settledValue)
            assertEquals(0f, state.requireOffset(), 0.001f)
        }
    }

    @Test
    fun releasedBetweenTheOpenStageAndTheThresholdTheCardStaysOpenOnItsAnchor() = runTest {
        withContext(frames) {
            for (reveal in listOf(100f, 250f, 579f)) {
                val state = stateOfWidth()
                dragAndRelease(state, reveal = reveal)
                assertEquals("reveal $reveal", SwipeValue.Open, state.settledValue)
                assertEquals("reveal $reveal", -open, state.requireOffset(), 0.001f) // pill fully revealed, settled, not stretched
            }
        }
    }

    @Test
    fun releasedPastTheThresholdTheStateSettlesOnOut() = runTest {
        withContext(frames) {
            val state = stateOfWidth()
            dragAndRelease(state, reveal = 620f)
            assertEquals(SwipeValue.Out, state.settledValue)
            assertEquals(-width, state.requireOffset(), 0.001f)
        }
    }

    @Test
    fun aVelocityNeverShortCircuitsTheStages() = runTest {
        withContext(frames) {
            // A fast flick to the left, released between the open stage and the threshold: open, not out.
            val flickLeft = stateOfWidth()
            dragAndRelease(flickLeft, reveal = 300f, velocity = -5000f)
            assertEquals(SwipeValue.Open, flickLeft.settledValue)
            assertEquals(-open, flickLeft.requireOffset(), 0.001f)
            // A fast flick to the right from the same place: still open (decision by distance).
            val flickRight = stateOfWidth()
            dragAndRelease(flickRight, reveal = 300f, velocity = 5000f)
            assertEquals(SwipeValue.Open, flickRight.settledValue)
            // A fast flick far from the open stage does not open anything.
            val short = stateOfWidth()
            dragAndRelease(short, reveal = 50f, velocity = -4000f)
            assertEquals(SwipeValue.Resting, short.settledValue)
            assertEquals(0f, short.requireOffset(), 0.001f)
        }
    }

    @Test
    fun aReleaseJustAroundEachStageDecidesOnTheStage() = runTest {
        withContext(frames) {
            val pastOpen = stateOfWidth()
            dragAndRelease(pastOpen, reveal = 101f)
            assertEquals(SwipeValue.Open, pastOpen.settledValue)
            val beforeOpen = stateOfWidth()
            dragAndRelease(beforeOpen, reveal = 99f)
            assertEquals(SwipeValue.Resting, beforeOpen.settledValue)
            val pastThreshold = stateOfWidth()
            dragAndRelease(pastThreshold, reveal = 581f)
            assertEquals(SwipeValue.Out, pastThreshold.settledValue)
            val beforeThreshold = stateOfWidth()
            dragAndRelease(beforeThreshold, reveal = 579f)
            assertEquals(SwipeValue.Open, beforeThreshold.settledValue)
        }
    }

    @Test
    fun fromOpenADragToTheRightClosesAndADragPastTheThresholdArchives() = runTest {
        withContext(frames) {
            val state = stateOfWidth()
            dragAndRelease(state, reveal = 200f)
            assertEquals(SwipeValue.Open, state.settledValue)
            // Same card, the finger comes back to the right (the app was in the Open stage): closed.
            dragAndRelease(state, reveal = 40f, was = SwipeZone.Open)
            assertEquals(SwipeValue.Resting, state.settledValue)
            assertEquals(0f, state.requireOffset(), 0.001f)
            // Open again, then dragged to the left past 58 %: out.
            dragAndRelease(state, reveal = 200f)
            assertEquals(SwipeValue.Open, state.settledValue)
            dragAndRelease(state, reveal = 700f, was = SwipeZone.Open)
            assertEquals(SwipeValue.Out, state.settledValue)
        }
    }

    @Test
    fun withReducedMotionTheSettleIsImmediate() = runTest {
        // No frame clock at all: any animation would fail. The jump needs none.
        val out = stateOfWidth()
        dragAndRelease(out, reveal = 700f, reduceMotion = true)
        assertEquals(SwipeValue.Out, out.settledValue)
        val opened = stateOfWidth()
        dragAndRelease(opened, reveal = 300f, reduceMotion = true)
        assertEquals(SwipeValue.Open, opened.settledValue)
        assertEquals(-open, opened.requireOffset(), 0.001f)
        val back = stateOfWidth()
        dragAndRelease(back, reveal = 50f, reduceMotion = true)
        assertEquals(SwipeValue.Resting, back.settledValue)
    }

    @Test
    fun aTapOnThePillAnimatesTheOpenCardOutAndATapOnTheBodyClosesIt() = runTest {
        withContext(frames) {
            val committed = stateOfWidth()
            dragAndRelease(committed, reveal = 200f)
            assertEquals(SwipeValue.Open, committed.settledValue)
            committed.animateTo(SwipeValue.Out, tween(SwipeArchiveLogic.STRETCH_MS, easing = EaseOutCubic)) // the commit of a pill tap
            assertEquals(SwipeValue.Out, committed.settledValue)

            val closed = stateOfWidth()
            dragAndRelease(closed, reveal = 200f)
            closed.animateTo(SwipeValue.Resting, spring(SwipeArchiveLogic.RETURN_DAMPING_RATIO, SwipeArchiveLogic.RETURN_STIFFNESS)) // body tap
            assertEquals(SwipeValue.Resting, closed.settledValue)
            assertEquals(0f, closed.requireOffset(), 0.001f)
        }
    }

    // ---- Taps on an open card ----------------------------------------------------------------------------------------------

    @Test
    fun theTapsOfAnOpenCardOnlyExistWhenSettledOpenAndNotLeaving() {
        assertTrue(SwipeArchiveLogic.isOpenForTaps(SwipeValue.Open, exiting = false))
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.Open, exiting = true)) // the pill was tapped: no second tap
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.Resting, exiting = false)) // closed card: the body opens the item
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.Out, exiting = false))
    }

    @Test
    fun theTapOnThePillDeliversTheActionOnlyOnce() {
        // The pill tap and the full swipe share the path: arm at the settle on Out, deliver after the collapse, and the
        // disposal of the row right after (it left the list) must not deliver a second time.
        val delivery = PendingDelivery<ArchiveAction>()
        val got = mutableListOf<ArchiveAction>()
        delivery.arm(ArchiveAction.Archive)
        assertTrue(delivery.deliver { got += it }) // after the collapse
        assertFalse(delivery.deliver { got += it }) // onDispose
        // A double tap re-arms nothing: the card is already leaving.
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.Open, exiting = true))
        assertEquals(listOf(ArchiveAction.Archive), got)
    }

    // ---- Single delivery of the action -------------------------------------------------------------------------------------

    @Test
    fun theDecidedActionIsDeliveredExactlyOnce() {
        val delivery = PendingDelivery<ArchiveAction>()
        val got = mutableListOf<ArchiveAction>()
        assertFalse(delivery.deliver { got += it }) // nothing decided: nothing delivered (e.g. disposal of a card never swiped)
        delivery.arm(ArchiveAction.Archive)
        assertTrue(delivery.deliver { got += it })
        assertFalse(delivery.deliver { got += it }) // the disposal after the exit does not deliver again
        assertEquals(listOf(ArchiveAction.Archive), got)
    }

    @Test
    fun aNewDecisionAfterARestoreIsDeliveredAgain() {
        val delivery = PendingDelivery<ArchiveAction>()
        val got = mutableListOf<ArchiveAction>()
        delivery.arm(ArchiveAction.Archive)
        delivery.deliver { got += it }
        delivery.arm(ArchiveAction.Unarchive)
        delivery.deliver { got += it }
        assertEquals(listOf(ArchiveAction.Archive, ArchiveAction.Unarchive), got)
    }

    // ---- Look: arrival, stretch, armed swell, timings ----------------------------------------------------------------------

    @Test
    fun thePillArrivesProgressivelyBetweenTheEntryRange() {
        val actions = 88f
        assertEquals(0f, SwipeArchiveLogic.arrival(0f, actions), 0f)
        assertEquals(0f, SwipeArchiveLogic.arrival(actions * SwipeArchiveLogic.ARRIVAL_START, actions), 0.0001f)
        assertEquals(1f, SwipeArchiveLogic.arrival(actions * SwipeArchiveLogic.ARRIVAL_END, actions), 0.0001f)
        assertEquals(1f, SwipeArchiveLogic.arrival(500f, actions), 0f)
        val mid = SwipeArchiveLogic.arrival(actions * 0.5f, actions)
        assertTrue(mid > 0f && mid < 1f)
    }

    @Test
    fun theWholePillZoomsFrom78PercentToFullAndNeverSwells() {
        assertEquals(0.78f, SwipeArchiveLogic.pillScale(0f), 0.0001f)
        assertEquals(1f, SwipeArchiveLogic.pillScale(1f), 0.0001f)
        assertEquals(0.89f, SwipeArchiveLogic.pillScale(0.5f), 0.0001f)
        // Fully arrived, the scale stays 1 whatever the reveal: no swell once armed.
        val actions = 88f
        for (reveal in listOf(actions, 300f, 580f, 1000f)) {
            assertEquals(1f, SwipeArchiveLogic.pillScale(SwipeArchiveLogic.arrival(reveal, actions)), 0f)
        }
    }

    // ---- Haptic tick (pure rule; the Vibrator effect itself lives on the View / Context side) -----------------------------------

    @Test
    fun onlyEnteringTheArmedZoneVibrates() {
        assertTrue(SwipeArchiveLogic.vibratesWhenEntering(SwipeZone.Armed))
        assertFalse(SwipeArchiveLogic.vibratesWhenEntering(SwipeZone.Open)) // the pill opens / back to open from armed
        assertFalse(SwipeArchiveLogic.vibratesWhenEntering(SwipeZone.Closed)) // closing by a drag
    }

    @Test
    fun aFullGestureVibratesOnceAtTheCrossingOf58PercentAndNowhereElse() {
        var current = SwipeZone.Closed
        var ticks = 0
        val stagesSeen = mutableListOf<SwipeZone>()
        // closed -> open (silent) -> armed (tick) -> a few px back, inside the hysteresis (none) -> open (silent) -> closed (silent)
        for (reveal in listOf(50f, 100f, 300f, 585f, 570f, 540f, 60f)) {
            val next = zone(current, reveal)
            if (next != current) {
                stagesSeen += next
                if (SwipeArchiveLogic.vibratesWhenEntering(next)) ticks++
            }
            current = next
        }
        assertEquals(listOf(SwipeZone.Open, SwipeZone.Armed, SwipeZone.Open, SwipeZone.Closed), stagesSeen)
        assertEquals(1, ticks)
    }

    @Test
    fun wobblingAroundTheThresholdVibratesOnlyOnceThanksToTheHysteresis() {
        var current = SwipeZone.Open
        var ticks = 0
        val threshold = width * SwipeArchiveLogic.THRESHOLD_FRACTION
        // Cross, then jitter within the hysteresis band: a single tick.
        for (reveal in listOf(threshold - 2f, threshold + 1f, threshold - 3f, threshold + 2f, threshold - hysteresis + 1f, threshold + 5f)) {
            val next = zone(current, reveal)
            if (next != current && SwipeArchiveLogic.vibratesWhenEntering(next)) ticks++
            current = next
        }
        assertEquals(1, ticks)
        // Going all the way back below the band and crossing again is a new crossing: a second tick.
        for (reveal in listOf(threshold - hysteresis - 5f, threshold + 1f)) {
            val next = zone(current, reveal)
            if (next != current && SwipeArchiveLogic.vibratesWhenEntering(next)) ticks++
            current = next
        }
        assertEquals(2, ticks)
    }
    // ---- Icon: centred in the pill up to 58 %, takes off to the left from there, stuck to the left edge from 75 % ---------------

    private val pillPx = 48f // PILL_SIZE_DP, in the units of the test
    private val iconPx = 24f // the icon (24 dp), never resized

    // Distance of the icon to its place at the open stage, ON SCREEN, positive = to the right: centre of the stretching pill
    // (the pill grows to the left, its centre moves left by stretch / 2) + the offset of the icon inside the pill.
    private fun iconScreenX(reveal: Float, actions: Float, width: Float): Float =
        -SwipeArchiveLogic.stretch(reveal, actions) / 2f + SwipeArchiveLogic.iconOffsetInPill(reveal, actions, width)

    private fun offset(reveal: Float, actions: Float = 88f, width: Float = 1000f) =
        SwipeArchiveLogic.iconOffsetInPill(reveal, actions, width)

    @Test
    fun thePillStretchesFromTheOpenStageAndNotFromTheThreshold() {
        val actions = 88f
        assertEquals(0f, SwipeArchiveLogic.stretch(0f, actions), 0f)
        assertEquals(0f, SwipeArchiveLogic.stretch(60f, actions), 0f)
        assertEquals(0f, SwipeArchiveLogic.stretch(88f, actions), 0f)
        assertEquals(12f, SwipeArchiveLogic.stretch(100f, actions), 0.001f)
        assertEquals(412f, SwipeArchiveLogic.stretch(500f, actions), 0.001f)
        assertEquals(912f, SwipeArchiveLogic.stretch(1000f, actions), 0.001f)
    }

    @Test
    fun theIconIsStrictlyCentredInThePillUpToThe58PercentThreshold() {
        val actions = 88f
        val threshold = width * SwipeArchiveLogic.THRESHOLD_FRACTION // 580
        // Rest, open stage, just past it, 50 % of the width, just under 58 %, and exactly 58 %: always the centre of the pill.
        for (reveal in listOf(0f, 30f, 60f, actions, actions + 0.01f, 100f, 250f, 334f, width / 2f, 570f, threshold - 0.001f, threshold)) {
            assertEquals("offset at $reveal", 0f, offset(reveal), 0f)
        }
        // On screen it simply follows the centre of the pill, which stretches to the left (no jump at Open).
        assertEquals(0f, iconScreenX(actions + 0.01f, actions, width), 0.01f)
        for (reveal in listOf(150f, 300f, threshold)) {
            assertEquals(-SwipeArchiveLogic.stretch(reveal, actions) / 2f, iconScreenX(reveal, actions, width), 0.001f)
        }
    }
    // Distance (px) between the left edge of the pill and the left edge of the icon: >= 0 means the icon is inside the pill.
    private fun iconLeftMargin(reveal: Float, actions: Float, w: Float): Float {
        val pillWidth = pillPx + SwipeArchiveLogic.stretch(reveal, actions)
        return pillWidth / 2f + SwipeArchiveLogic.iconOffsetInPill(reveal, actions, w) - iconPx / 2f
    }

    @Test
    fun theIconReachesTheLeftEdgeAt75PercentAndNotOnlyAtTheFullWidth() {
        assertEquals(0.75f, SwipeArchiveLogic.REACH_FRACTION, 0f)
        assertTrue(SwipeArchiveLogic.REACH_FRACTION > SwipeArchiveLogic.THRESHOLD_FRACTION)
        assertTrue(SwipeArchiveLogic.REACH_FRACTION < 1f)
    }

    @Test
    fun pastThe58PercentTheIconOvershootsTheCentreToTheLeftMoreAndMoreAndFasterUpTo75Percent() {
        val actions = 88f
        val threshold = width * SwipeArchiveLogic.THRESHOLD_FRACTION // 580
        val reach = width * SwipeArchiveLogic.REACH_FRACTION // 750
        var previous = offset(threshold)
        assertEquals(0f, previous, 0f)
        for (reveal in listOf(threshold + 1f, 600f, 650f, 700f, reach - 1f, reach)) {
            val inside = offset(reveal)
            assertTrue("reveal $reveal: $inside < 0 (left of the centre)", inside < 0f)
            assertTrue("reveal $reveal: |$inside| grows strictly", inside < previous)
            previous = inside
            // The icon never leaves the pill.
            assertTrue("reveal $reveal: margin ${iconLeftMargin(reveal, actions, width)}", iconLeftMargin(reveal, actions, width) >= -0.001f)
        }
        // At 75 %: stuck to the left edge, the centre of the icon half a pill from the left edge of the pill.
        val stretchAtReach = SwipeArchiveLogic.stretch(reach, actions)
        assertEquals(-stretchAtReach / 2f, offset(reach), 0.001f)
        assertEquals(pillPx / 2f, (pillPx + stretchAtReach) / 2f + offset(reach), 0.001f)
        // Almost stuck a little before, still moving: not at the edge yet at 70 %.
        assertTrue(iconLeftMargin(700f, actions, width) > iconLeftMargin(reach, actions, width) + 1f)
        // Faster than before the threshold: slope (px of offset per px of reveal), measured 10 px each side of it.
        val slopeBefore = (offset(threshold) - offset(threshold - 10f)) / 10f
        val slopeAfter = (offset(threshold + 10f) - offset(threshold)) / 10f
        assertEquals("no slope before (centred)", 0f, slopeBefore, 0f)
        assertTrue("slope after $slopeAfter is steep", slopeAfter < -1f)
    }

    @Test
    fun from75PercentToTheFullWidthTheIconStaysStuckToTheLeftEdgeOfTheGrowingPill() {
        val actions = 88f
        val reach = width * SwipeArchiveLogic.REACH_FRACTION
        for (reveal in listOf(reach, 850f, 900f, 950f, 999f, width, width + 50f)) {
            val stretch = SwipeArchiveLogic.stretch(reveal, actions)
            // Follows the left edge of the pill: always half a pill from it, never outside.
            assertEquals("reveal $reveal", -stretch / 2f, offset(reveal), 0.001f)
            assertEquals("reveal $reveal", pillPx / 2f, (pillPx + stretch) / 2f + offset(reveal), 0.001f)
            assertTrue("reveal $reveal", iconLeftMargin(reveal, actions, width) >= -0.001f)
        }
        // At the full width: the old target, the centre of the semicircle at the left end of the final pill.
        assertEquals(-SwipeArchiveLogic.stretch(width, actions) / 2f, offset(width), 0.001f)
        // On screen the icon keeps going left with the edge of the pill (no stop, no jump back).
        var previousX = iconScreenX(reach, actions, width)
        for (reveal in listOf(850f, 900f, 950f, width)) {
            val x = iconScreenX(reveal, actions, width)
            assertTrue("reveal $reveal: $x < $previousX", x < previousX)
            previousX = x
        }
    }

    @Test
    fun thePositionOfTheIconHasNoJumpAtThe58PercentThreshold() {
        val actions = 88f
        val threshold = width * SwipeArchiveLogic.THRESHOLD_FRACTION
        val atThreshold = iconScreenX(threshold, actions, width)
        for (eps in listOf(1f, 0.1f, 0.001f)) {
            // Screen speed: -0.5 px per px of reveal just before the threshold (the pill centre), about -2.5 just after (the pill centre
            // -0.5, the icon -1.95 inside it): the speed jumps (the icon takes off), the position does not.
            assertEquals("before by $eps", atThreshold, iconScreenX(threshold - eps, actions, width), 0.6f * eps)
            assertEquals("after by $eps", atThreshold, iconScreenX(threshold + eps, actions, width), 2.6f * eps)
        }
        assertEquals(0f, offset(threshold), 0f)
        assertEquals(offset(threshold), offset(threshold + 0.001f), 0.002f)
        assertEquals(offset(threshold), offset(threshold - 0.001f), 0.002f)
    }

    @Test
    fun thePositionOfTheIconHasNoJumpAt75Percent() {
        val actions = 88f
        val reach = width * SwipeArchiveLogic.REACH_FRACTION
        val atReach = iconScreenX(reach, actions, width)
        for (eps in listOf(1f, 0.1f, 0.001f)) {
            // Screen speed about -2.5 just before, -1 just after (the icon follows the edge of the pill): finite slopes, no jump.
            assertEquals("before by $eps", atReach, iconScreenX(reach - eps, actions, width), 2.6f * eps)
            assertEquals("after by $eps", atReach, iconScreenX(reach + eps, actions, width), 1.1f * eps)
        }
        assertEquals(offset(reach), offset(reach + 0.001f), 0.002f)
        assertEquals(offset(reach), offset(reach - 0.001f), 0.002f)
    }

    @Test
    fun theIconNeverLeavesThePillFromTheOpenStageToBeyondTheFullWidth() {
        val actions = 88f
        var reveal = 0f
        while (reveal <= width + 100f) {
            assertTrue("reveal $reveal", iconLeftMargin(reveal, actions, width) >= -0.001f)
            reveal += 3f
        }
    }

    @Test
    fun theIconBehavesTheSameOnAPhoneSizedCard() {
        val width = 360f // threshold 208.8, reach 270
        val actions = 88f
        val threshold = width * SwipeArchiveLogic.THRESHOLD_FRACTION
        val reach = width * SwipeArchiveLogic.REACH_FRACTION
        assertEquals(0f, offset(actions, actions, width), 0f)
        assertEquals(0f, offset(threshold, actions, width), 0f)
        assertEquals(0f, offset(150f, actions, width), 0f)
        assertEquals(0f, offset(200f, actions, width), 0f)
        assertTrue(offset(260f, actions, width) < 0f)
        assertTrue(offset(280f, actions, width) < offset(260f, actions, width))
        assertEquals(-SwipeArchiveLogic.stretch(reach, actions) / 2f, offset(reach, actions, width), 0.001f)
        assertEquals(-SwipeArchiveLogic.stretch(320f, actions) / 2f, offset(320f, actions, width), 0.001f)
        assertEquals(-SwipeArchiveLogic.stretch(width, actions) / 2f, offset(width, actions, width), 0.001f)
        for (reveal in listOf(150f, threshold, 250f, reach, 330f, width)) {
            assertTrue("reveal $reveal", iconLeftMargin(reveal, actions, width) >= -0.001f)
        }
    }

    @Test
    fun theIconMovementIsSafeOnAnUnmeasuredOrVeryNarrowCard() {
        assertEquals(0f, offset(500f, 88f, 0f), 0f)
        assertEquals(0f, offset(0f, 88f, 0f), 0f)
        // Narrow card: threshold (58) and reach (75) are below the open stage (88): no stretch, no offset in between, no NaN,
        // stuck to the left edge beyond.
        assertEquals(0f, offset(70f, 88f, 100f), 0f)
        assertEquals(0f, offset(88f, 88f, 100f), 0f)
        assertEquals(-SwipeArchiveLogic.stretch(100f, 88f) / 2f, offset(100f, 88f, 100f), 0.001f)
        assertFalse(offset(95f, 88f, 100f).isNaN())
        for (reveal in listOf(0f, 58f, 82f, 88f, 90f, 100f, 150f)) {
            assertTrue("reveal $reveal", iconLeftMargin(reveal, 88f, 100f) >= -0.001f)
        }
    }
    @Test
    fun theExitHasNoAnimationWithReducedMotion() {
        assertEquals(0 to 0, SwipeArchiveLogic.exitTimings(reduceMotion = true))
        assertEquals(220 to 180, SwipeArchiveLogic.exitTimings(reduceMotion = false))
    }

    @Test
    fun theReturnSpringMatchesTheReference() {
        // T3: damping 26, stiffness 330 (mass 1) -> damping ratio about 0.716.
        assertEquals(0.716f, SwipeArchiveLogic.RETURN_DAMPING_RATIO, 0.001f)
        assertEquals(330f, SwipeArchiveLogic.RETURN_STIFFNESS, 0f)
    }
}
