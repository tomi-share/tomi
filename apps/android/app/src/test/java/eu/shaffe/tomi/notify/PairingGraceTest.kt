package eu.shaffe.tomi.notify

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The history imported right after the pairing is not announced as a burst of notifications */
class PairingGraceTest {
    private var now = 1_000L
    private val grace = PairingGrace(nowMs = { now }, graceMs = 60_000)

    @Test
    fun aDeviceAlreadyPairedAtStartNeverHasAGracePeriod() {
        assertFalse(grace.isImporting(paired = true))
        now += 1
        assertFalse(grace.isImporting(paired = true))
    }

    @Test
    fun aDeviceThatJoinsWhileRunningIsImportingDuringTheGraceThenNot() {
        assertFalse(grace.isImporting(paired = false))
        assertTrue(grace.isImporting(paired = true))
        now += 59_000
        assertTrue(grace.isImporting(paired = true))
        now += 2_000
        assertFalse("a real item that arrives later notifies again", grace.isImporting(paired = true))
        now += 10
        assertFalse(grace.isImporting(paired = true))
    }

    @Test
    fun leavingTheSpaceEndsTheGrace() {
        grace.isImporting(paired = false)
        assertTrue(grace.isImporting(paired = true))
        assertFalse(grace.isImporting(paired = false))
        assertFalse(grace.isImporting(paired = false))
    }
}
