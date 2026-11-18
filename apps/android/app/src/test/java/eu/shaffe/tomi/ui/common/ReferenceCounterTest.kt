package eu.shaffe.tomi.ui.common

import org.junit.Assert.assertEquals
import org.junit.Test

class ReferenceCounterTest {
    private val events = mutableListOf<String>()
    private val counter = ReferenceCounter<String>(
        onFirst = { events += "set:$it" },
        onLast = { events += "clear:$it" },
    )

    @Test
    fun theFlagIsSetByTheFirstUserAndKeptUntilTheLastOneLeaves() {
        // Feed shows a secret, then the compose screen needs the flag too.
        counter.acquire("window")
        counter.acquire("window")
        assertEquals(listOf("set:window"), events)

        // The feed hides its secret: the compose screen still needs the flag.
        counter.release("window")
        assertEquals(listOf("set:window"), events)
        assertEquals(1, counter.count("window"))

        counter.release("window")
        assertEquals(listOf("set:window", "clear:window"), events)
        assertEquals(0, counter.count("window"))
    }

    @Test
    fun windowsAreCountedIndependently() {
        counter.acquire("main")
        counter.acquire("share")
        counter.release("main")
        assertEquals(listOf("set:main", "set:share", "clear:main"), events)
        assertEquals(1, counter.count("share"))
    }

    @Test
    fun anUnbalancedReleaseDoesNothingAndDoesNotBreakTheNextAcquire() {
        counter.release("window")
        assertEquals(emptyList<String>(), events)

        counter.acquire("window")
        counter.release("window")
        counter.release("window")
        counter.acquire("window")
        assertEquals(listOf("set:window", "clear:window", "set:window"), events)
    }
}
