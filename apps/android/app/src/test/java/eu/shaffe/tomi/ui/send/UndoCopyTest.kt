package eu.shaffe.tomi.ui.send

import eu.shaffe.tomi.send.UndoDeadline
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The copy of the sent text kept for "Undo" drops itself at the deadline, by itself, without any screen. */
@OptIn(ExperimentalCoroutinesApi::class)
class UndoCopyTest {

    private fun TestScope.copy() = UndoCopy<String>(backgroundScope) { testScheduler.currentTime }

    @Test
    fun theCopyIsKeptUntilTheDeadlineThenDroppedAndTheBannerIsToldToGo() = runTest {
        val kept = copy()
        var expired = 0
        kept.keep("mot de passe", UndoDeadline.startingAt(testScheduler.currentTime, 3_000L)) { expired++ }
        assertEquals("mot de passe", kept.value)

        advanceTimeBy(2_999L)
        runCurrent()
        assertEquals("mot de passe", kept.value)
        assertEquals(0, expired)

        advanceTimeBy(1L)
        runCurrent()
        assertNull(kept.value)
        assertEquals(1, expired)
    }

    @Test
    fun clearDropsTheCopyAtOnceAndTheDeadlineDoesNothingMore() = runTest {
        val kept = copy()
        var expired = 0
        kept.keep("secret", UndoDeadline.startingAt(testScheduler.currentTime, 3_000L)) { expired++ }
        kept.clear()
        assertNull(kept.value)

        advanceTimeBy(10_000L)
        runCurrent()
        assertEquals(0, expired)
    }

    @Test
    fun aNewCopyReplacesTheOldOneAndTheOldDeadlineCannotDropIt() = runTest {
        val kept = copy()
        kept.keep("premier", UndoDeadline.startingAt(testScheduler.currentTime, 3_000L))
        advanceTimeBy(2_000L)
        kept.keep("second", UndoDeadline.startingAt(testScheduler.currentTime, 3_000L))

        advanceTimeBy(1_500L) // the first deadline has passed, the second has not
        runCurrent()
        assertEquals("second", kept.value)

        advanceTimeBy(1_500L)
        runCurrent()
        assertNull(kept.value)
    }

    @Test
    fun onAnImmediateDispatcherADeadlineAlreadyOverCallsOnExpiredBeforeKeepReturns() = runTest {
        // viewModelScope runs on Dispatchers.Main.immediate: delay(0) does not suspend. The caller must therefore have set
        // its own state (the banner) BEFORE calling keep, or the late write would undo what onExpired just cleared.
        val kept = UndoCopy<String>(CoroutineScope(UnconfinedTestDispatcher(testScheduler))) { testScheduler.currentTime }
        var banner: String? = "sent" // set first, as SendViewModel.send() does
        kept.keep("tard", UndoDeadline(atElapsedMs = 0L, windowMs = 3_000L)) { banner = null }
        assertNull(kept.value)
        assertNull(banner)
    }

    @Test
    fun aDeadlineAlreadyOverDropsTheCopyRightAway() = runTest {
        val kept = copy()
        kept.keep("tard", UndoDeadline(atElapsedMs = 0L, windowMs = 3_000L))
        runCurrent()
        assertNull(kept.value)
    }
}
