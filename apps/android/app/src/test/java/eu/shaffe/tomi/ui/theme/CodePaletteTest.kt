package eu.shaffe.tomi.ui.theme

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CodePaletteTest {
    private val palettes = mapOf("light" to CodePalette.Light, "dark" to CodePalette.Dark)

    @Test
    fun theBlockAndTextColoursComeFromTheCharterTokens() {
        assertEquals(0xFFECE9E2.toInt(), CodePalette.Light.background)
        assertEquals(0xFF1B2130.toInt(), CodePalette.Light.text)
        assertEquals(0xFF242A38.toInt(), CodePalette.Dark.background)
        assertEquals(0xFFE9E7E1.toInt(), CodePalette.Dark.text)
    }

    @Test
    fun tokensFollowTheDesignTokens() {
        with(CodePalette.Light) {
            assertEquals(0xFF2E4C80.toInt(), keyword)
            assertEquals(0xFF6F4F7C.toInt(), function)
            assertEquals(0xFF6F4F7C.toInt(), type)
            assertEquals(0xFF2E6B45.toInt(), string)
            assertEquals(0xFF8C5F14.toInt(), number)
            assertEquals(0xFF8C5F14.toInt(), constant)
            assertEquals(0xFF565C6B.toInt(), comment)
            assertEquals(0xFF3A4152.toInt(), operator)
            assertEquals(0xFF6F4F7C.toInt(), tag)
            assertEquals(0xFF8C5F14.toInt(), attribute)
        }
        with(CodePalette.Dark) {
            assertEquals(0xFFA9BFEA.toInt(), keyword)
            assertEquals(0xFFCDB3D9.toInt(), function)
            assertEquals(0xFFCDB3D9.toInt(), type)
            assertEquals(0xFF8FCFA6.toInt(), string)
            assertEquals(0xFFE0B160.toInt(), number)
            assertEquals(0xFFE0B160.toInt(), constant)
            assertEquals(0xFFA6ABB8.toInt(), comment)
            assertEquals(0xFFC3C1BB.toInt(), operator)
            assertEquals(0xFFCDB3D9.toInt(), tag)
            assertEquals(0xFFE0B160.toInt(), attribute)
        }
    }

    @Test
    fun everyTokenReachesAaContrastOnTheBlockBackground() {
        palettes.forEach { (name, palette) ->
            palette.tokenColors.forEach { color ->
                val ratio = CodePalette.contrast(color, palette.background)
                assertTrue("$name #${Integer.toHexString(color)} on the block: $ratio", ratio >= 4.5)
            }
            assertTrue("$name gutter", CodePalette.contrast(palette.gutter, palette.background) >= 4.5)
        }
    }

    @Test
    fun searchHighlightsKeepTheTextReadableAndTheCurrentOneIsMorePressed() {
        palettes.forEach { (name, palette) ->
            assertTrue("$name match", CodePalette.contrast(palette.text, palette.searchMatch) >= 4.5)
            assertTrue("$name current", CodePalette.contrast(palette.text, palette.searchCurrent) >= 4.5)
            // The current occurrence is visibly different from the other ones.
            assertTrue(palette.searchCurrent != palette.searchMatch)
            assertTrue(CodePalette.contrast(palette.searchCurrent, palette.searchMatch) >= 1.3)
            // Highlights stand out from the block itself.
            assertTrue(CodePalette.contrast(palette.searchMatch, palette.background) >= 1.2)
        }
    }

    @Test
    fun theSunColourIsReservedToTheSymbol() {
        palettes.values.forEach { palette ->
            assertFalse(palette.tokenColors.contains(CodePalette.IDENTITY_SUN))
            assertFalse(palette.searchMatch == CodePalette.IDENTITY_SUN)
            assertFalse(palette.searchCurrent == CodePalette.IDENTITY_SUN)
            assertFalse(palette.gutter == CodePalette.IDENTITY_SUN)
        }
    }

    @Test
    fun contrastIsTheWcagRatio() {
        assertEquals(21.0, CodePalette.contrast(0xFF000000.toInt(), 0xFFFFFFFF.toInt()), 0.01)
        assertEquals(1.0, CodePalette.contrast(0xFF777777.toInt(), 0xFF777777.toInt()), 0.0001)
        assertEquals(CodePalette.contrast(0xFF123456.toInt(), 0xFFEEEEEE.toInt()), CodePalette.contrast(0xFFEEEEEE.toInt(), 0xFF123456.toInt()), 0.0)
    }

    @Test
    fun theDarkAndLightPalettesAreChosenByTheTheme() {
        assertTrue(CodePalette.of(dark = true) === CodePalette.Dark)
        assertTrue(CodePalette.of(dark = false) === CodePalette.Light)
    }
}
