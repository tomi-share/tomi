package eu.shaffe.tomi

import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.security.GeneralSecurityException

/** The opening of the core, off the main thread, with a way out when the data cannot be read */
@OptIn(ExperimentalCoroutinesApi::class)
class CoreStartupTest {
    private class Recorder {
        val events = mutableListOf<String>()
        var failOpen: Exception? = null
        var failWipe: Exception? = null
        var opened = 0

        fun open(): String {
            events += "open"
            opened++
            failOpen?.let { throw it }
            return "core#$opened"
        }
    }

    private fun TestScope.startup(r: Recorder) = CoreStartup(
        scope = backgroundScope,
        io = UnconfinedTestDispatcher(testScheduler),
        main = UnconfinedTestDispatcher(testScheduler),
        open = r::open,
        onReady = { r.events += "ready:$it" },
        wipe = {
            r.events += "wipe"
            r.failWipe?.let { throw it }
        },
    )

    @Test
    fun `opens the core, tells the main thread, then is ready`() = runTest {
        val r = Recorder()
        val startup = startup(r)
        assertEquals(StartupState.Opening, startup.state.value)

        startup.begin()

        assertEquals(StartupState.Ready("core#1"), startup.state.value)
        assertEquals(listOf("open", "ready:core#1"), r.events)
        assertEquals("core#1", startup.await())
    }

    @Test
    fun `a core that cannot be opened is unreadable, not a crash`() = runTest {
        for (failure in listOf<Exception>(GeneralSecurityException("keystore"), IllegalStateException("wrap key"), IOException("sqlite"))) {
            val r = Recorder().apply { failOpen = failure }
            val startup = startup(r)

            startup.begin()

            assertEquals(StartupState.Unreadable(resetFailed = false), startup.state.value)
            assertEquals(listOf("open"), r.events)
            assertEquals(null, startup.await())
        }
    }

    @Test
    fun `retry opens again and can succeed`() = runTest {
        val r = Recorder().apply { failOpen = IllegalStateException("keystore not ready") }
        val startup = startup(r)
        startup.begin()

        r.failOpen = null
        startup.retry()

        assertEquals(StartupState.Ready("core#2"), startup.state.value)
    }

    @Test
    fun `reset erases the data first, then opens a fresh core`() = runTest {
        val r = Recorder().apply { failOpen = IllegalStateException("database unreadable") }
        val startup = startup(r)
        startup.begin()

        r.failOpen = null
        startup.reset()

        assertEquals(listOf("open", "wipe", "open", "ready:core#2"), r.events)
        assertEquals(StartupState.Ready("core#2"), startup.state.value)
    }

    @Test
    fun `a reset that cannot erase says so and does not open anything`() = runTest {
        val r = Recorder().apply { failOpen = IllegalStateException("database unreadable") }
        val startup = startup(r)
        startup.begin()

        r.failWipe = IOException("locked")
        startup.reset()

        assertEquals(StartupState.Unreadable(resetFailed = true), startup.state.value)
        assertEquals(listOf("open", "wipe"), r.events)
    }

    /** A scope like the application's, but that records what would crash the process (no handler = uncaught exception = crash). */
    private fun TestScope.recordingScope(crashes: MutableList<Throwable>) = CoroutineScope(
        SupervisorJob() + UnconfinedTestDispatcher(testScheduler) + CoroutineExceptionHandler { _, e -> crashes += e },
    )

    private fun TestScope.startup(r: Recorder, scope: CoroutineScope, onReady: (String) -> Unit) = CoreStartup(
        scope = scope,
        io = UnconfinedTestDispatcher(testScheduler),
        main = UnconfinedTestDispatcher(testScheduler),
        open = r::open,
        onReady = onReady,
        wipe = { r.events += "wipe" },
    )

    @Test
    fun `a failure after the core is open crashes, it is not unreadable (audit A-L1)`() = runTest {
        val crashes = mutableListOf<Throwable>()
        val r = Recorder()
        val bug = IllegalStateException("container constructor")
        val startup = CoreStartup(
            scope = recordingScope(crashes),
            io = UnconfinedTestDispatcher(testScheduler),
            main = UnconfinedTestDispatcher(testScheduler),
            open = { r.open().also { throw StartupBug(bug) } },
            onReady = { r.events += "ready:$it" },
            wipe = { r.events += "wipe" },
        )

        startup.begin()

        // Not Unreadable: "Réessayer" would open a second core on the database of the one that is still open.
        assertEquals(StartupState.Opening, startup.state.value)
        assertEquals(1, crashes.size)
        assertTrue(crashes.single() is StartupBug)
        assertEquals(bug, crashes.single().cause)
        startup.retry()
        startup.reset()
        assertEquals(listOf("open"), r.events)
    }

    @Test
    fun `a failure of onReady crashes, it is not unreadable (audit A-L1)`() = runTest {
        val crashes = mutableListOf<Throwable>()
        val r = Recorder()
        val startup = startup(r, recordingScope(crashes)) { throw IllegalStateException("start() failed") }

        startup.begin()

        assertEquals(StartupState.Opening, startup.state.value)
        assertEquals(1, crashes.size)
        assertTrue(crashes.single() is IllegalStateException)
        // Neither a retry nor a reset can run next to the core that is open.
        startup.retry()
        startup.reset()
        assertEquals(listOf("open"), r.events)
    }

    @Test
    fun `reset and retry do nothing unless the data is unreadable`() = runTest {
        val r = Recorder()
        val startup = startup(r)
        startup.begin()

        startup.reset()
        startup.retry()

        assertEquals(listOf("open", "ready:core#1"), r.events)
        assertTrue(startup.state.value is StartupState.Ready)
    }
}
