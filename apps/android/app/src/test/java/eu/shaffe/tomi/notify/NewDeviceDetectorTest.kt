package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.Device
import eu.shaffe.tomi.core.DevicePlatform
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** A device added to the space is announced once; the existing list and this device's own pairing are not news. */
class NewDeviceDetectorTest {
    private val detector = NewDeviceDetector()

    private fun device(id: String, mine: Boolean = false) =
        Device(id, "Name $id", DevicePlatform.Android, lastActivity = null, isThisDevice = mine)

    private val me = device("me", mine = true)
    private val phone = device("phone")
    private val pc = device("pc")

    @Test
    fun theFirstListIsTheExistingOneAndAnnouncesNothing() {
        assertTrue(detector.onDevices(listOf(me, phone), importing = false).isEmpty())
    }

    @Test
    fun aDeviceThatAppearsIsAnnouncedOnce() {
        detector.onDevices(listOf(me, phone), importing = false)
        assertEquals(listOf(pc), detector.onDevices(listOf(me, phone, pc), importing = false))
        assertTrue(detector.onDevices(listOf(me, phone, pc), importing = false).isEmpty())
    }

    @Test
    fun aRenameOrARevocationAnnouncesNothing() {
        detector.onDevices(listOf(me, phone, pc), importing = false)
        assertTrue(detector.onDevices(listOf(me, phone.copy(name = "Renamed"), pc), importing = false).isEmpty())
        assertTrue(detector.onDevices(listOf(me, phone), importing = false).isEmpty())
    }

    @Test
    fun theListReceivedWhileThisDeviceIsBeingPairedIsNotNewsAndIsNotAnnouncedLater() {
        detector.onDevices(emptyList(), importing = false)
        assertTrue(detector.onDevices(listOf(me, phone, pc), importing = true).isEmpty())
        assertTrue(detector.onDevices(listOf(me, phone, pc), importing = false).isEmpty())
    }

    @Test
    fun aDeviceThatTheSpaceDidNotVerifyIsNotAnnouncedUntilItIs() {
        val forged = pc.copy(verified = false)
        detector.onDevices(listOf(me, phone), importing = false)
        assertTrue(detector.onDevices(listOf(me, phone, forged), importing = false).isEmpty())
        assertTrue(detector.onDevices(listOf(me, phone, forged), importing = false).isEmpty())
        assertEquals(listOf(pc), detector.onDevices(listOf(me, phone, pc), importing = false))
    }

    @Test
    fun aKnownDeviceThatTheRelayMakesLookUnverifiedForAMomentIsNotAnnouncedWhenItIsVerifiedAgain() {
        detector.onDevices(listOf(me, phone), importing = false)
        assertTrue(detector.onDevices(listOf(me, phone.copy(verified = false)), importing = false).isEmpty())
        assertTrue(detector.onDevices(listOf(me, phone), importing = false).isEmpty())
    }

    @Test
    fun aListWithoutThisDeviceIsNotAReference() {
        detector.onDevices(listOf(me, phone), importing = false)
        assertTrue(detector.onDevices(emptyList(), importing = false).isEmpty())
        assertTrue(detector.onDevices(listOf(me, phone), importing = false).isEmpty())
    }

    @Test
    fun thisDeviceIsNeverAnnouncedToItself() {
        detector.onDevices(emptyList(), importing = false)
        assertTrue(detector.onDevices(listOf(me), importing = false).isEmpty())
    }
}
