package eu.shaffe.tomi.ui.common

import java.time.Duration
import java.time.Instant
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RemainingRefreshTest {
    private val now = Instant.parse("2026-11-17T12:00:00Z")

    @Test
    fun `nothing to refresh without expiry or once expired`() {
        assertNull(remainingRefreshDelay(null, now))
        assertNull(remainingRefreshDelay(now, now))
        assertNull(remainingRefreshDelay(now.minusSeconds(5), now))
    }

    @Test
    fun `last minute waits for the expiry itself`() {
        // 30 s left: displays "1 minute" until it is gone.
        assertEquals(Duration.ofSeconds(30), remainingRefreshDelay(now.plusSeconds(30), now)!!.minusMillis(1))
    }

    @Test
    fun `minutes refresh at the next minute boundary`() {
        // 5 min 20 s left shows "5 minutes" for 20 more seconds.
        assertEquals(Duration.ofSeconds(20), remainingRefreshDelay(now.plusSeconds(320), now)!!.minusMillis(1))
    }

    @Test
    fun `hours and days never wait longer than a minute`() {
        assertEquals(Duration.ofSeconds(60), remainingRefreshDelay(now.plus(Duration.ofHours(2).plusMinutes(30)), now))
        assertEquals(Duration.ofSeconds(60), remainingRefreshDelay(now.plus(Duration.ofDays(3).plusHours(5)), now))
    }

    @Test
    fun `never spins faster than once a second`() {
        // Right on a boundary: 2 h 0 min 0.2 s left.
        assertEquals(Duration.ofSeconds(1), remainingRefreshDelay(now.plus(Duration.ofHours(1)).plusMillis(200), now))
    }
}
