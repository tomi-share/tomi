package eu.shaffe.tomi.ui.theme

import androidx.compose.material3.ColorScheme
import androidx.compose.material3.contentColorFor
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * `contentColorFor` compares by VALUE and tests errorContainer before the surface roles: an errorContainer equal to one
 * of them made every `Surface(color = surfaceVariant)` inherit onErrorContainer (red text, red icons).
 */
class ThemeContentColorTest {
    private val schemes: Map<String, ColorScheme> = mapOf("light" to LightScheme, "dark" to DarkScheme)

    @Test
    fun surfaceVariantGivesOnSurfaceVariant() {
        for ((name, scheme) in schemes) {
            assertEquals(name, scheme.onSurfaceVariant, scheme.contentColorFor(scheme.surfaceVariant))
        }
    }

    @Test
    fun theHighSurfacesGiveOnSurface() {
        for ((name, scheme) in schemes) {
            // Compared by value, so two harmless equalities of the charter show up: in the dark scheme surfaceDim == background
            // (gives onBackground, same value as onSurface); in the light scheme surfaceContainerHigh/Highest and surfaceDim ==
            // surfaceVariant, which is tested first (gives onSurfaceVariant: the charter gives them no other value to tell them apart).
            for ((role, color) in mapOf(
                "surfaceContainerHigh" to scheme.surfaceContainerHigh,
                "surfaceContainerHighest" to scheme.surfaceContainerHighest,
                "surfaceBright" to scheme.surfaceBright,
                "surfaceDim" to scheme.surfaceDim,
            )) {
                val expected = if (color == scheme.surfaceVariant) scheme.onSurfaceVariant else scheme.onSurface
                assertEquals("$name $role", expected, scheme.contentColorFor(color))
            }
        }
    }

    @Test
    fun errorContainerIsDistinctFromEverySurfaceRole() {
        for ((name, scheme) in schemes) {
            val others = listOf(
                scheme.background, scheme.surface, scheme.surfaceVariant, scheme.surfaceDim, scheme.surfaceBright,
                scheme.surfaceContainerLowest, scheme.surfaceContainerLow, scheme.surfaceContainer,
                scheme.surfaceContainerHigh, scheme.surfaceContainerHighest, scheme.primaryContainer,
                scheme.secondaryContainer, scheme.tertiaryContainer, scheme.inverseSurface, scheme.error,
            )
            for (other in others) assertNotEquals("$name errorContainer", other, scheme.errorContainer)
        }
    }

    @Test
    fun onErrorContainerStaysReadableOnErrorContainer() {
        for ((name, scheme) in schemes) {
            assertTrue("$name contrast", contrast(scheme.onErrorContainer, scheme.errorContainer) >= 4.5)
        }
    }

    private fun contrast(a: Color, b: Color): Double {
        val la = a.luminance().toDouble()
        val lb = b.luminance().toDouble()
        return (maxOf(la, lb) + 0.05) / (minOf(la, lb) + 0.05)
    }
}
