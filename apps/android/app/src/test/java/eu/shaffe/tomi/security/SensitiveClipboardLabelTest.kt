package eu.shaffe.tomi.security

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Which clip the departure clears: only what Tomi labelled itself, told apart without ever reading the content. */
class SensitiveClipboardLabelTest {
    @Test
    fun aClipWithTheLabelOfTomiIsOurs() {
        assertTrue(SensitiveClipboard.isTomiClip("Tomi"))
        assertTrue("any CharSequence", SensitiveClipboard.isTomiClip(StringBuilder("Tomi")))
        assertEquals("Tomi", SensitiveClipboard.LABEL)
    }

    @Test
    fun aForeignClipIsLeftAlone() {
        assertFalse(SensitiveClipboard.isTomiClip(null))
        assertFalse(SensitiveClipboard.isTomiClip(""))
        assertFalse(SensitiveClipboard.isTomiClip("User Input"))
        assertFalse("case matters", SensitiveClipboard.isTomiClip("tomi"))
        assertFalse(SensitiveClipboard.isTomiClip("Tomi "))
    }
}
