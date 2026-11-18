package eu.shaffe.tomi.core

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.IOException

/** A failure of the core is a message for the user, never a crash; a cancellation always goes through. */
class SafeCoreTest {
    @Test
    fun `a call that works returns true and says nothing`() = runTest {
        var told = false
        val ok = safeCore(onFailure = { told = true }) { }
        assertTrue(ok)
        assertFalse(told)
    }

    @Test
    fun `any exception is swallowed, told once, and returns false`() = runTest {
        for (failure in listOf<Exception>(IOException("offline"), IllegalStateException("keystore"), RuntimeException("panic"))) {
            var told = 0
            val ok = safeCore(onFailure = { told++ }) { throw failure }
            assertFalse(ok)
            assertEquals(1, told)
        }
    }

    @Test
    fun `a failure of the core itself is swallowed too`() = runTest {
        var told = false
        val ok = safeCore(onFailure = { told = true }) { throw TomiCoreException("storage") }
        assertFalse(ok)
        assertTrue(told)
    }

    @Test
    fun `a cancellation is rethrown and not told`() = runTest {
        var told = false
        try {
            safeCore(onFailure = { told = true }) { throw CancellationException("cancelled") }
            fail("the cancellation must go through")
        } catch (_: CancellationException) {
            // expected
        }
        assertFalse(told)
    }
}
