package eu.shaffe.tomi.ui.code

import com.swmansion.enriched.markdown.codehighlight.SyntaxTokenType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The native PHP highlighter (libtomi_highlight) returns token types as numbers, and TomiCodeHighlight turns them back with
 * `SyntaxTokenType.entries[n]`: the two enums must stay in the same order. These tests fail when Enriched Markdown reorders,
 * adds or removes a token type, or when the native enum drifts, instead of silently painting PHP with the wrong colours.
 */
class SyntaxTokenOrderTest {
    private val expected = listOf(
        "KEYWORD", "OPERATOR", "PUNCTUATION", "STRING", "NUMBER", "CONSTANT", "COMMENT",
        "FUNCTION", "TYPE", "VARIABLE", "PROPERTY", "TAG", "ATTRIBUTE", "EMBEDDED",
    )

    @Test
    fun theLibraryEnumHasTheOrderTheNativeCodeAssumes() {
        assertEquals(expected, SyntaxTokenType.entries.map { it.name })
    }

    @Test
    fun theNativeEnumHasTheSameOrderAsTheLibraryEnum() {
        // Unit tests run with the module directory (apps/android/app) as working directory.
        val header = File("src/main/cpp/highlight_core.h")
        assertTrue("highlight_core.h not found from ${File(".").absolutePath}", header.isFile)
        val lines = header.readLines()
        val begin = lines.indexOfFirst { it.contains("enum TokenType") }
        assertTrue("enum TokenType not found", begin >= 0)
        val names = lines.drop(begin + 1)
            .takeWhile { !it.contains("TOKEN_TYPE_COUNT") }
            .mapNotNull { Regex("^\\s*([A-Z_]+)\\b").find(it)?.groupValues?.get(1) }
        assertEquals(expected, names)
    }
}
