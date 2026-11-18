package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.time.Duration
import java.time.Instant

/** The hint that suggests removing another device that has not shown up for longer than the relay's `inactive_after_days`. */
class DeviceInactivityTest {
    private val now = Instant.parse("2026-11-09T12:00:00Z")

    private fun other(seen: Instant?) = Device("d", "PC-Maison", DevicePlatform.Windows, seen, isThisDevice = false)

    private fun days(device: Device, thresholdDays: Long = 30): Long? =
        DeviceInactivity.inactiveDays(device, now, thresholdDays, lastSeenFromRelay = true)

    @Test
    fun exactlyAtTheThresholdIsStillActive() {
        assertNull(days(other(now.minus(Duration.ofDays(30)))))
    }

    @Test
    fun oneMillisecondPastTheThresholdIsInactive() {
        assertEquals(30L, days(other(now.minus(Duration.ofDays(30)).minusMillis(1))))
    }

    @Test
    fun countsWholeDays() {
        assertEquals(34L, days(other(now.minus(Duration.ofDays(34)))))
        assertEquals(34L, days(other(now.minus(Duration.ofDays(34)).minus(Duration.ofHours(12)))))
    }

    @Test
    fun aRecentDeviceIsActive() {
        assertNull(days(other(now)))
        assertNull(days(other(now.minus(Duration.ofDays(29)))))
    }

    @Test
    fun aDeviceThatNeverShowedUpIsNotReported() {
        assertNull(days(other(null)))
    }

    @Test
    fun thisDeviceIsNeverReported() {
        val me = Device("me", "Pixel 8", DevicePlatform.Android, now.minus(Duration.ofDays(400)), isThisDevice = true)
        assertNull(days(me))
        assertNull(days(Device("me", "Pixel 8", DevicePlatform.Android, null, isThisDevice = true)))
    }

    @Test
    fun aDateInTheFutureIsNotInactive() {
        assertNull(days(other(now.plus(Duration.ofDays(5)))))
    }

    @Test
    fun theThresholdIsTheOneGiven() {
        val device = other(now.minus(Duration.ofDays(10)))
        assertNull(days(device, thresholdDays = 10))
        assertEquals(10L, days(device, thresholdDays = 9))
    }

    @Test
    fun theDefaultThresholdIsTheRelaysThirtyDays() {
        assertEquals(30L, DeviceInactivity.INACTIVE_AFTER_DAYS)
    }

    @Test
    fun nothingIsReportedWhileTheDateIsNotTheRelaysLastSeen() {
        // The core does not carry the relay's `last_seen_at` yet: its date is the time of pairing, which says nothing of use.
        val old = other(now.minus(Duration.ofDays(400)))
        assertNull(DeviceInactivity.inactiveDays(old, now, lastSeenFromRelay = false))
    }
}
