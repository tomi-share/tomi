package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The name a device announces when it joins (1 to 64 characters, no blank at either end, no control character). */
class DeviceNamesTest {
    @Test
    fun blanksAndControlCharactersBecomeOneSpace() {
        assertEquals("Pixel 8 Phone", DeviceNames.clean("  Pixel \t 8\u0007Phone   \n"))
    }

    @Test
    fun nothingLeftMeansNoName() {
        assertNull(DeviceNames.clean(null))
        assertNull(DeviceNames.clean(""))
        assertNull(DeviceNames.clean(" \t\n\u0007 "))
    }

    @Test
    fun aLongNameIsCutAtSixtyFourCharactersNeverInTheMiddleOfAPair() {
        val cut = DeviceNames.clean("a".repeat(100))!!
        assertEquals(64, cut.length)

        // Surrogate pairs count as one character: 63 letters then an emoji (2 chars) is 64 characters, kept whole.
        val withEmoji = "a".repeat(63) + "\uD83D\uDCF1" + "b".repeat(10)
        val cutEmoji = DeviceNames.clean(withEmoji)!!
        assertEquals(64, cutEmoji.codePointCount(0, cutEmoji.length))
        assertEquals("\uD83D\uDCF1", cutEmoji.substring(63))
    }

    @Test
    fun theTrailingBlankLeftByTheCutIsTrimmed() {
        val raw = "a".repeat(63) + " " + "b".repeat(10)
        assertEquals("a".repeat(63), DeviceNames.clean(raw))
    }

    @Test
    fun theFirstCandidateTheCoreAcceptsWins() {
        val names = listOf(null, "   ", "My  phone", "Pixel 8")
        assertEquals("Pixel 8", DeviceNames.choose(names) { it != "My phone" })
        assertEquals("My phone", DeviceNames.choose(names) { true })
    }

    @Test
    fun withoutAcceptedCandidateTheDeviceIsCalledAndroid() {
        assertEquals("Android", DeviceNames.choose(listOf("x", null)) { false })
        assertEquals("Android", DeviceNames.choose(emptyList()) { true })
    }
}
