package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.random.Random

/**
 * The automatic attempts of a pending departure (spec/api-v1.md 7.9.4): the series after a gesture follows its delays and stops as soon as
 * an attempt is not pending on the network or the server; afterwards, at most one attempt a minute when the network or the foreground
 * comes back; one series at a time; the start-up attempt; a refusal is kept for the screens. Virtual time: deterministic.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class DepartureRetrierTest {
    private val fakeScope = CoroutineScope(Job())

    @After
    fun tearDown() {
        (fakeScope.coroutineContext[Job])?.cancel()
    }

    /** No jitter: the delays are exactly the listed ones. */
    private object NoJitter : Random() {
        override fun nextBits(bitCount: Int): Int = 0
    }

    /** A core whose `continueDeparture` gives the [results] in order (then [after]) and records when it was called. */
    private class CountingCore(inner: FakeTomiCore, private val clock: () -> Long) : TomiCore by inner {
        val departureFlow = MutableStateFlow<Departure?>(Departure(DepartureKind.Leave, false, 0L, null))
        val runningFlow = MutableStateFlow(false)
        override val departure: StateFlow<Departure?> = departureFlow
        override val departureRunning: StateFlow<Boolean> = runningFlow
        val results = ArrayDeque<DepartureResult>()
        var after: DepartureResult = DepartureResult.Pending(PendingReason.Network)
        val calledAt = mutableListOf<Long>()

        override suspend fun continueDeparture(): DepartureResult {
            calledAt += clock()
            return results.removeFirstOrNull() ?: after
        }
    }

    private class Setup(val core: CountingCore, val online: MutableStateFlow<Boolean>, val foreground: MutableStateFlow<Boolean>, val retrier: DepartureRetrier)

    private fun TestScope.setup(): Setup {
        val core = CountingCore(FakeTomiCore(fakeScope, startPaired = true)) { testScheduler.currentTime }
        val online = MutableStateFlow(true)
        val foreground = MutableStateFlow(true)
        val retrier = DepartureRetrier(core, backgroundScope, online, foreground, { testScheduler.currentTime }, NoJitter)
        retrier.start()
        runCurrent()
        return Setup(core, online, foreground, retrier)
    }

    private val delays = DepartureRetrier.DEPARTURE_RETRY_DELAYS_MS

    /** Far beyond a whole series. `advanceUntilIdle` would not do: it ignores the work of `backgroundScope`, where the retrier runs. */
    private fun TestScope.runAll() {
        advanceTimeBy(ONE_HOUR_MS)
        runCurrent()
    }

    private companion object {
        const val ONE_HOUR_MS = 3_600_000L
    }

    /** The times of the attempts of a whole series, from 0. */
    private fun seriesTimes(count: Int = delays.size): List<Long> = delays.runningReduce { a, b -> a + b }.take(count)

    @Test
    fun theSeriesFollowsTheDelaysWhileItStaysPendingOnTheNetworkThenStops() = runTest {
        val s = setup()
        s.core.after = DepartureResult.Pending(PendingReason.Server)

        s.retrier.kick()
        advanceTimeBy(delays.first() - 1)
        assertEquals("nothing before the first delay", emptyList<Long>(), s.core.calledAt)
        runAll()

        assertEquals(seriesTimes(), s.core.calledAt)
    }

    @Test
    fun theSeriesStopsAtTheFirstAttemptThatIsNotTransient() = runTest {
        val enders = listOf(
            DepartureResult.Erased,
            DepartureResult.Refused(RefusalReason.Unexpected),
            DepartureResult.Pending(PendingReason.NotFound),
            DepartureResult.Pending(PendingReason.NotSent),
            DepartureResult.Cancelled,
            DepartureResult.NothingPending,
            DepartureResult.Failed,
        )
        for (end in enders) {
            val s = setup()
            s.core.results += DepartureResult.Pending(PendingReason.Network)
            s.core.results += end
            val start = testScheduler.currentTime

            s.retrier.kick()
            runAll()

            assertEquals(end.toString(), seriesTimes(2), s.core.calledAt.map { it - start })
        }
    }

    @Test
    fun theSeriesStopsWhenTheDepartureIsGone() = runTest {
        val s = setup()
        s.retrier.kick()
        advanceTimeBy(delays.first() + 1)
        assertEquals(1, s.core.calledAt.size)

        s.core.departureFlow.value = null
        runAll()

        assertEquals(1, s.core.calledAt.size)
    }

    @Test
    fun oneSeriesAtATime() = runTest {
        val s = setup()
        s.core.after = DepartureResult.Pending(PendingReason.Network)
        s.retrier.kick()
        advanceTimeBy(1_000)
        s.retrier.kick()
        runAll()

        // The second kick replaced the first series: its times start at 1 s.
        assertEquals(seriesTimes().map { it + 1_000 }, s.core.calledAt)
    }

    @Test
    fun afterTheSeriesTheNetworkComingBackGivesAtMostOneAttemptAMinute() = runTest {
        val s = setup()
        s.online.value = false
        runCurrent()
        s.online.value = true
        runCurrent()
        assertEquals("the network came back: one attempt", 1, s.core.calledAt.size)

        // It flaps: nothing more within the minute, the foreground included.
        s.online.value = false
        runCurrent()
        s.online.value = true
        s.foreground.value = false
        runCurrent()
        s.foreground.value = true
        runCurrent()
        assertEquals(1, s.core.calledAt.size)

        advanceTimeBy(DepartureRetrier.COME_BACK_MIN_GAP_MS)
        s.foreground.value = false
        runCurrent()
        s.foreground.value = true
        runCurrent()
        assertEquals("a minute later, the foreground: one more", 2, s.core.calledAt.size)
    }

    @Test
    fun nothingComesBackWithoutAPendingDepartureOrWhileOneRuns() = runTest {
        val s = setup()
        s.core.runningFlow.value = true
        s.online.value = false
        runCurrent()
        s.online.value = true
        runCurrent()
        assertEquals(0, s.core.calledAt.size)

        s.core.runningFlow.value = false
        s.core.departureFlow.value = Departure(DepartureKind.Leave, wiping = true, sinceMs = 0L, last = null)
        runCurrent()
        s.online.value = false
        runCurrent()
        s.online.value = true
        runCurrent()
        assertEquals("not during the erasure", 0, s.core.calledAt.size)
    }

    @Test
    fun atStartUpOneAttemptThenTheSeriesIfItIsTransient() = runTest {
        val transient = setup()
        transient.core.results += DepartureResult.Pending(PendingReason.Network)
        transient.core.results += DepartureResult.Erased
        assertEquals(DepartureResult.Pending(PendingReason.Network), transient.retrier.settleAtStart())
        runAll()
        assertEquals(listOf(0L, delays.first()), transient.core.calledAt)

        val settled = setup()
        settled.core.results += DepartureResult.Pending(PendingReason.NotSent)
        settled.retrier.settleAtStart()
        runAll()
        assertEquals("verification only: shown, not retried", 1, settled.core.calledAt.size)
    }

    @Test
    fun aRefusalOfAnAutomaticAttemptIsKeptForTheScreens() = runTest {
        val s = setup()
        s.core.departureFlow.value = Departure(DepartureKind.DeleteSpace, false, 0L, null)
        s.core.results += DepartureResult.Refused(RefusalReason.RelayTooOld)

        s.retrier.settleAtStart()

        assertEquals(DepartureRefusal(DepartureKind.DeleteSpace, RefusalReason.RelayTooOld), s.retrier.refusal.value)
        s.retrier.consumeRefusal()
        assertNull(s.retrier.refusal.value)
    }

    @Test
    fun theJitterAddsAtMostTwentyPercent() = runTest {
        val core = CountingCore(FakeTomiCore(fakeScope, startPaired = true)) { testScheduler.currentTime }
        core.results += DepartureResult.Erased
        val alwaysMax = object : Random() {
            override fun nextBits(bitCount: Int): Int = (1 shl bitCount) - 1
        }
        val retrier = DepartureRetrier(core, backgroundScope, MutableStateFlow(true), MutableStateFlow(true), { testScheduler.currentTime }, alwaysMax)

        retrier.kick()
        runAll()

        val at = core.calledAt.single()
        assertTrue("first attempt at $at ms", at > delays.first() && at <= delays.first() * 12 / 10)
    }
}
