package eu.shaffe.tomi.ui.code

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ExtraGrammarsTest {
    @Test
    fun phpFenceTagsAreOurs() {
        listOf("php", "PHP", " php ", "phtml", "php title=\"x.php\"").forEach { assertEquals(it, ExtraGrammars.PHP, ExtraGrammars.forInfoString(it)) }
    }

    @Test
    fun theLibraryGrammarsAndUnknownTagsAreNotOurs() {
        listOf("rust", "js", "python", "yaml", "tsx", "java", "swift", "ruby", "", "  ", "phpx", "kotlin", "Kotlin", "kt", "kts", "kotlin {1,3}").forEach {
            assertNull(it, ExtraGrammars.forInfoString(it))
        }
        assertNull(ExtraGrammars.forInfoString(null))
    }

    @Test
    fun limitsAreThoseOfEnrichedMarkdown() {
        assertFalse(ExtraGrammars.withinLimits(ByteArray(0)))
        assertTrue(ExtraGrammars.withinLimits("echo 1;".toByteArray()))
        assertTrue(ExtraGrammars.withinLimits(ByteArray(ExtraGrammars.MAX_BYTES) { 'a'.code.toByte() }))
        assertFalse(ExtraGrammars.withinLimits(ByteArray(ExtraGrammars.MAX_BYTES + 1) { 'a'.code.toByte() }))
        assertTrue(ExtraGrammars.withinLimits("\n".repeat(ExtraGrammars.MAX_LINES - 1).toByteArray()))
        assertFalse(ExtraGrammars.withinLimits("\n".repeat(ExtraGrammars.MAX_LINES).toByteArray()))
    }
}