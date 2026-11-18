package eu.shaffe.tomi.ui.theme

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TypographyFeaturesTest {
    private fun pieces(settings: String) = settings.split(',')

    @Test
    fun exactTextSwitchesTheContextualLigaturesOff() {
        assertTrue("calt=0" in pieces(TomiFonts.MONO_EXACT_FEATURES))
        assertTrue("liga=0" in pieces(TomiFonts.MONO_EXACT_FEATURES))
    }

    @Test
    fun aRevealedSecretKeepsTheSlashedZeroAndHasNoLigature() {
        // Android splits on commas and hands each piece to HarfBuzz: no space, one setting per piece.
        assertEquals(listOf("zero", "calt=0", "liga=0"), pieces(TomiFonts.MONO_SECRET_FEATURES))
    }
}
