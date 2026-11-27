package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.ScrollScope
import androidx.compose.runtime.MonotonicFrameClock
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.readableItem
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/**
 * The swipe to the RIGHT (pin / unpin) is the swipe to the left mirrored: same stages and thresholds, measured on the distance
 * whichever side the card is dragged to. What is ours is tested here (anchors per side, the side of an offset, the settle on a real
 * `AnchoredDraggableState`, the rules of the action); the gesture is Compose foundation code, validated on a device.
 */
class SwipeToPinTest {
    private val width = 1000f
    private val open = 100f // threshold = 580
    private val hysteresis = 30f
    private val now = Instant.parse("2026-11-05T10:00:00Z")
    private val source = DeviceRef("pc", "PC-Bureau")

    private val frames = object : MonotonicFrameClock {
        private var nanos = 0L
        override suspend fun <R> withFrameNanos(onFrame: (Long) -> R): R {
            nanos += 16_000_000L
            return onFrame(nanos)
        }
    }

    private fun stateWith(hasEnd: Boolean, hasStart: Boolean): AnchoredDraggableState<SwipeValue> =
        SwipeArchiveLogic.newState().also { it.updateAnchors(SwipeArchiveLogic.anchors(width, open, hasEnd, hasStart)) }

    /** Drags to [offset] (negative = left, positive = right) and lifts the finger, as the draggable node does. */
    private suspend fun dragAndRelease(state: AnchoredDraggableState<SwipeValue>, offset: Float, was: SwipeZone = SwipeZone.Closed) {
        state.anchoredDrag { dragTo(offset) }
        val fling = ArchiveSettleFling(
            state = state,
            targetFor = {
                val target = SwipeArchiveLogic.settleTarget(was, SwipeArchiveLogic.magnitude(state.offset), width, open, hysteresis)
                SwipeArchiveLogic.onSide(target, SwipeArchiveLogic.isStartSide(state.offset))
            },
            reduceMotion = true,
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
            with(fling) { scope.performFling(0f) }
        }
    }

    private fun item(pinned: Boolean = false, archived: Boolean = false): TomiItem = readableItem(
        id = "1", sourceDevice = source, createdAt = now, text = RichText("a"),
        pinned = pinned, pinnedAt = if (pinned) now else null, archived = archived, archivedAt = if (archived) now else null,
    )

    // ---- Anchors ------------------------------------------------------------------------------------------------------

    @Test
    fun bothSidesGiveFiveAnchorsMirroredAroundRest() {
        val anchors = SwipeArchiveLogic.anchors(1000f, 100f, hasEnd = true, hasStart = true)
        assertEquals(5, anchors.size)
        assertEquals(-100f, anchors.positionOf(SwipeValue.Open), 0f)
        assertEquals(-1000f, anchors.positionOf(SwipeValue.Out), 0f)
        assertEquals(100f, anchors.positionOf(SwipeValue.OpenStart), 0f)
        assertEquals(1000f, anchors.positionOf(SwipeValue.OutStart), 0f)
    }

    @Test
    fun aSideWithoutActionHasNoAnchorSoTheCardDoesNotMoveThatWay() {
        val startOnly = SwipeArchiveLogic.anchors(1000f, 100f, hasEnd = false, hasStart = true)
        assertEquals(3, startOnly.size)
        assertEquals(0f, startOnly.minPosition(), 0f)
        assertEquals(1000f, startOnly.maxPosition(), 0f)
        val endOnly = SwipeArchiveLogic.anchors(1000f, 100f) // what the archive swipe always had
        assertEquals(3, endOnly.size)
        assertEquals(0f, endOnly.maxPosition(), 0f)
    }

    @Test
    fun theStartOpenStageStaysUnderTheThresholdOnANarrowCardToo() {
        assertEquals(58f, SwipeArchiveLogic.anchors(100f, 88f, hasEnd = false, hasStart = true).positionOf(SwipeValue.OpenStart), 0.001f)
    }

    // ---- Side of an offset ----------------------------------------------------------------------------------------------

    @Test
    fun theSideAndTheDistanceFollowTheSignOfTheOffset() {
        assertEquals(250f, SwipeArchiveLogic.revealStart(250f), 0f)
        assertEquals(0f, SwipeArchiveLogic.revealStart(-250f), 0f)
        assertEquals(0f, SwipeArchiveLogic.revealStart(Float.NaN), 0f)
        assertEquals(250f, SwipeArchiveLogic.magnitude(250f), 0f)
        assertEquals(250f, SwipeArchiveLogic.magnitude(-250f), 0f)
        assertEquals(0f, SwipeArchiveLogic.magnitude(Float.NaN), 0f)
        assertTrue(SwipeArchiveLogic.isStartSide(40f))
        assertFalse(SwipeArchiveLogic.isStartSide(-40f))
        assertFalse(SwipeArchiveLogic.isStartSide(0f))
    }

    @Test
    fun theStagesOfTheLeftAreTheOnesOfTheRightOnTheStartSide() {
        assertEquals(SwipeValue.OpenStart, SwipeArchiveLogic.onSide(SwipeValue.Open, start = true))
        assertEquals(SwipeValue.OutStart, SwipeArchiveLogic.onSide(SwipeValue.Out, start = true))
        assertEquals(SwipeValue.Resting, SwipeArchiveLogic.onSide(SwipeValue.Resting, start = true))
        assertEquals(SwipeValue.Open, SwipeArchiveLogic.onSide(SwipeValue.Open, start = false))
        assertEquals(SwipeValue.Out, SwipeArchiveLogic.onSide(SwipeValue.Out, start = false))
    }

    @Test
    fun theTapsOfAnOpenCardExistOnBothSides() {
        assertTrue(SwipeArchiveLogic.isOpenForTaps(SwipeValue.OpenStart, exiting = false))
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.OpenStart, exiting = true))
        assertFalse(SwipeArchiveLogic.isOpenForTaps(SwipeValue.OutStart, exiting = false))
    }

    // ---- Settle on a real state, to the right -----------------------------------------------------------------------------

    @Test
    fun releasedBelowTheOpenStageToTheRightTheCardCloses() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = true, hasStart = true)
            dragAndRelease(state, 40f)
            assertEquals(SwipeValue.Resting, state.settledValue)
            assertEquals(0f, state.offset, 0f)
        }
    }

    @Test
    fun releasedBetweenTheOpenStageAndTheThresholdToTheRightTheCardStaysOpen() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = true, hasStart = true)
            dragAndRelease(state, 300f, was = SwipeZone.Open)
            assertEquals(SwipeValue.OpenStart, state.settledValue)
            assertEquals(100f, state.offset, 0f)
        }
    }

    @Test
    fun releasedPastTheThresholdToTheRightTheStateSettlesOnOutStart() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = true, hasStart = true)
            dragAndRelease(state, 700f, was = SwipeZone.Armed)
            assertEquals(SwipeValue.OutStart, state.settledValue)
            assertEquals(1000f, state.offset, 0f)
        }
    }

    @Test
    fun theLeftStillSettlesOnOutWhenTheCardHasBothSides() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = true, hasStart = true)
            dragAndRelease(state, -700f, was = SwipeZone.Armed)
            assertEquals(SwipeValue.Out, state.settledValue)
            assertEquals(-1000f, state.offset, 0f)
        }
    }

    @Test
    fun aDragToTheRightDoesNotMoveACardWithoutAPinAction() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = true, hasStart = false)
            state.dispatchRawDelta(300f)
            assertEquals(0f, state.offset, 0f)
        }
    }

    @Test
    fun aDragToTheLeftDoesNotMoveACardThatOnlyPins() = runTest {
        withContext(frames) {
            val state = stateWith(hasEnd = false, hasStart = true)
            state.dispatchRawDelta(-300f)
            assertEquals(0f, state.offset, 0f)
        }
    }

    @Test
    fun aDecisionOnEachSideIsDeliveredOnceByItsOwnDelivery() {
        val archive = PendingDelivery<ArchiveAction>()
        val pin = PendingDelivery<PinAction>()
        archive.arm(ArchiveAction.Archive)
        pin.arm(PinAction.Pin)
        val seen = mutableListOf<Any>()
        assertTrue(archive.deliver { seen += it })
        assertTrue(pin.deliver { seen += it })
        assertFalse(archive.deliver { seen += it })
        assertFalse(pin.deliver { seen += it })
        assertEquals(listOf<Any>(ArchiveAction.Archive, PinAction.Pin), seen)
    }

    // ---- The action of the right ----------------------------------------------------------------------------------------

    @Test
    fun anUnpinnedCardPinsAndAPinnedOneUnpins() {
        assertEquals(PinAction.Pin, ArchiveSwipe.pinSwipeAction(item()))
        assertEquals(PinAction.Unpin, ArchiveSwipe.pinSwipeAction(item(pinned = true)))
        assertEquals(PinAction.Unpin, PinAction.Pin.inverse)
        assertEquals(PinAction.Pin, PinAction.Unpin.inverse)
    }

    @Test
    fun anArchivedCardHasNoPinSwipeBecauseUndoCouldNotBringItBack() {
        assertNull(ArchiveSwipe.pinSwipeAction(item(archived = true)))
    }

    @Test
    fun aPinnedCardSwipesToTheRightOnlyAndAnUnpinnedOneBothWays() {
        val local = "me"
        val pinned = item(pinned = true)
        assertNull(ArchiveSwipe.swipeAction(pinned, local)) // no archive by swipe on a pinned card
        assertEquals(PinAction.Unpin, ArchiveSwipe.pinSwipeAction(pinned))
        val plain = item()
        assertEquals(ArchiveAction.Archive, ArchiveSwipe.swipeAction(plain, local))
        assertEquals(PinAction.Pin, ArchiveSwipe.pinSwipeAction(plain))
    }
}
