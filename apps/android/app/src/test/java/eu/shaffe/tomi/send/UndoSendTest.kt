package eu.shaffe.tomi.send

import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.DispatchResult
import eu.shaffe.tomi.core.FakeTomiCore
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.share.ExpiryChoice
import eu.shaffe.tomi.share.ParsedShare
import eu.shaffe.tomi.share.SharedStream
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset

@OptIn(ExperimentalCoroutinesApi::class)
class UndoSendTest {

    private class RecordingScheduler : SendScheduling {
        val scheduled = mutableListOf<Pair<String, UndoDeadline>>()
        val cancelled = mutableListOf<String>()
        override fun schedule(itemId: String, deadline: UndoDeadline) {
            scheduled += itemId to deadline
        }
        override fun ensureScheduled(itemId: String) = Unit
        override fun cancel(itemId: String) {
            cancelled += itemId
        }
    }

    private object NoFiles : AttachmentStaging {
        override suspend fun stage(draftId: String, streams: List<SharedStream>) = StagedAttachments(emptyList(), 0)
        override fun cleanup(draftId: String) = Unit
    }

    private val parsed = ParsedShare(title = null, text = RichText("bonjour"), url = null, streams = emptyList(), looksSecret = false)

    private class Setup(val core: FakeTomiCore, val scheduler: RecordingScheduler, val coordinator: SendCoordinator)

    private fun TestScope.setup(): Setup {
        val core = FakeTomiCore(backgroundScope, dispatchLatencyMs = 400, deliveryLatencyMs = 2_000)
        val scheduler = RecordingScheduler()
        val elapsed = longArrayOf(10_000L)
        val coordinator = SendCoordinator(
            core = core,
            scheduler = scheduler,
            staging = NoFiles,
            clock = Clock.fixed(Instant.parse("2026-11-04T10:00:00Z"), ZoneOffset.UTC),
            elapsedRealtimeMs = { elapsed[0] },
            newId = { "item-1" },
        )
        return Setup(core, scheduler, coordinator)
    }

    @Test
    fun theSubmissionAndTheBackgroundJobShareTheSameRealTimeDeadline() = runTest {
        val s = setup()
        val submission = s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)

        assertEquals(10_000L + SendCoordinator.UNDO_WINDOW_MS, submission.deadline.atElapsedMs)
        val (scheduledId, scheduledDeadline) = s.scheduler.scheduled.single()
        assertEquals("item-1", scheduledId)
        assertEquals(submission.deadline.atElapsedMs, scheduledDeadline.atElapsedMs)
        assertEquals(submission.deadline.windowMs, scheduledDeadline.windowMs)
    }

    @Test
    fun undoWithinTheWindowRemovesTheItemAndStopsTheJob() = runTest {
        val s = setup()
        val submission = s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)
        assertNotNull(s.core.getItem("item-1"))

        assertEquals(CancelOutcome.Cancelled, s.coordinator.cancel(submission))

        assertNull(s.core.getItem("item-1"))
        assertEquals(listOf("item-1"), s.scheduler.cancelled)
    }

    @Test
    fun undoAfterTheItemLeftSaysTooLateAndDeletesNothing() = runTest {
        val s = setup()
        val submission = s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)
        // The background job runs: the relay accepts the item.
        assertEquals(DispatchResult.Sent, s.core.dispatch("item-1"))

        assertEquals(CancelOutcome.TooLate, s.coordinator.cancel(submission))

        val item = s.core.getItem("item-1")
        assertNotNull("an item that already left must stay in the feed", item)
        assertTrue(item!!.deliveries.none { it.state == DeliveryState.Queued })
        assertTrue("the job must not be touched when it is too late", s.scheduler.cancelled.isEmpty())
    }

    @Test
    fun undoRacingWithTheDispatchHasExactlyOneWinner() = runTest {
        val s = setup()
        val submission = s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)

        // The dispatch is in flight (waiting for the relay) when the user taps Undo.
        val dispatch = async { s.core.dispatch("item-1") }
        runCurrent()
        assertEquals(CancelOutcome.Cancelled, s.coordinator.cancel(submission))
        advanceUntilIdle()

        assertEquals("the dispatch must find the item gone", DispatchResult.NotFound, dispatch.await())
        assertNull(s.core.getItem("item-1"))
    }

    @Test
    fun whenTheDispatchWinsTheRaceUndoIsTooLate() = runTest {
        val s = setup()
        val submission = s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)

        val dispatch = async { s.core.dispatch("item-1") }
        advanceTimeBy(500) // past the 400 ms latency: the relay accepted the item
        runCurrent()
        assertEquals(DispatchResult.Sent, dispatch.await())

        assertEquals(CancelOutcome.TooLate, s.coordinator.cancel(submission))
        assertNotNull(s.core.getItem("item-1"))
    }

    @Test
    fun cancelIfQueuedRefusesUnknownIdsAndIsNotRepeatable() = runTest {
        val s = setup()
        assertFalse(s.core.cancelIfQueued("nope"))

        s.coordinator.submit(parsed, secret = false, expiry = ExpiryChoice.Day7)
        assertTrue(s.core.cancelIfQueued("item-1"))
        // Second call: the item is already gone.
        assertFalse(s.core.cancelIfQueued("item-1"))
        assertTrue(s.core.items.first().none { it.id == "item-1" })
    }

    // --- deadline arithmetic --------------------------------------------------------------------------------------

    @Test
    fun deadlineFractionAndRemainingTimeFollowTheRealTimeClock() {
        val deadline = UndoDeadline.startingAt(nowElapsedMs = 1_000, windowMs = 5_000)
        assertEquals(6_000L, deadline.atElapsedMs)
        assertEquals(1f, deadline.fraction(1_000), 0f)
        assertEquals(0.5f, deadline.fraction(3_500), 0.0001f)
        assertEquals(0f, deadline.fraction(6_000), 0f)
        assertEquals(0f, deadline.fraction(9_000), 0f)
        assertEquals(2_500L, deadline.remainingMs(3_500))
        assertEquals(0L, deadline.remainingMs(9_000))
        assertFalse(deadline.isOver(5_999))
        assertTrue(deadline.isOver(6_000))
    }

    @Test
    fun theBackgroundJobWaitsOutTheWindowButIgnoresAStaleClockAfterAReboot() {
        val deadline = UndoDeadline(atElapsedMs = 6_000, windowMs = 5_000)
        assertEquals(2_000L, deadline.waitBeforeDispatchMs(4_000))
        assertEquals(0L, deadline.waitBeforeDispatchMs(6_000))
        assertEquals(0L, deadline.waitBeforeDispatchMs(60_000))
        // elapsedRealtime restarted from 0 after a reboot: the remaining time looks huge, so it is not waited for.
        val beforeReboot = UndoDeadline(atElapsedMs = 8_000_000, windowMs = 5_000)
        assertEquals(0L, beforeReboot.waitBeforeDispatchMs(1_000))
    }
}
