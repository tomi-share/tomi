package eu.shaffe.tomi.ui.common

import eu.shaffe.tomi.ui.theme.CodePalette
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FeedbackPaletteTest {
    private val palettes = mapOf("light" to FeedbackPalette.Light, "dark" to FeedbackPalette.Dark)

    @Test
    fun theCardIsDarkInkInBothModes() {
        assertEquals(0xFF1B2130.toInt(), FeedbackPalette.Light.container)
        assertEquals(0xFF242A38.toInt(), FeedbackPalette.Dark.container)
        assertEquals(FeedbackPalette.Dark, FeedbackPalette.of(dark = true))
        assertEquals(FeedbackPalette.Light, FeedbackPalette.of(dark = false))
    }

    @Test
    fun textActionsAndCheckAreReadableOnTheCard() {
        palettes.forEach { (name, palette) ->
            assertTrue("$name text", CodePalette.contrast(palette.content, palette.container) >= 4.5)
            assertTrue("$name action", CodePalette.contrast(palette.action, palette.container) >= 4.5)
            // The check is a graphical object next to its text: 3:1 (WCAG 1.4.11), reached by far.
            assertTrue("$name check", CodePalette.contrast(palette.done, palette.container) >= 4.5)
        }
    }

    @Test
    fun theIdentityColoursAreNotUsed() {
        val night = 0xFF24324A.toInt()
        palettes.forEach { (name, palette) ->
            val colours = listOfNotNull(palette.container, palette.content, palette.action, palette.done, palette.border)
            assertFalse("$name uses the sun", CodePalette.IDENTITY_SUN in colours)
            assertFalse("$name uses the night", night in colours)
        }
    }
}
