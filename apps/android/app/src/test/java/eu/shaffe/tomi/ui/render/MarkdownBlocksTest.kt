package eu.shaffe.tomi.ui.render

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The card preview cuts a long text BEFORE the guard, the parser and the renderer. The cut must stay within the budget, never
 * leave a construct half open (fenced code, table) and be stable: the preview of a preview is itself.
 */
class MarkdownBlocksTest {
    private fun lines(n: Int, prefix: String = "line") = (0 until n).joinToString("\n") { "$prefix $it" }

    /** Checks the budget (whole lines, characters, appended definitions aside) and idempotence of a preview. */
    private fun checked(md: String, budget: Budget = CARD_BUDGET): Preview {
        val p = preview(md, budget)
        val again = preview(p.markdown, budget)
        assertFalse("a preview is never cut again", again.truncated)
        assertEquals("idempotent", p.markdown, again.markdown)
        val body = p.markdown.substringBefore("\n\n[")
        assertTrue("lines within budget", body.split('\n').size <= budget.maxLines)
        assertTrue("chars within budget", body.length <= budget.maxChars)
        return p
    }

    private fun assertWellFormedUtf16(s: String) {
        var i = 0
        while (i < s.length) {
            val c = s[i]
            if (Character.isHighSurrogate(c)) {
                assertTrue("high surrogate without its pair", i + 1 < s.length && Character.isLowSurrogate(s[i + 1]))
                i += 2
            } else {
                assertFalse("lone low surrogate", Character.isLowSurrogate(c))
                i++
            }
        }
    }

    // --- not truncated ----------------------------------------------------------------------------------------------

    @Test
    fun aShortTextIsKeptAsIs() {
        val md = "# Title\n\nSome **text** with a [link](https://e.example).\n\n- a\n- b\n"
        val p = preview(md)
        assertFalse(p.truncated)
        assertEquals(md, p.markdown)
    }

    @Test
    fun anEmptyTextIsKeptAsIs() {
        assertEquals("", preview("").markdown)
        assertFalse(preview("").truncated)
        assertEquals("", head("", 10, 100).markdown)
        assertFalse(head("", 10, 100).truncated)
    }

    @Test
    fun aShortUnclosedFenceIsKeptAsIs() {
        val md = "```\ncode\nmore"
        assertEquals(md, preview(md).markdown)
        assertFalse(preview(md).truncated)
    }

    // --- fenced code --------------------------------------------------------------------------------------------------

    @Test
    fun aCutInsideBacktickFencedCodeClosesIt() {
        val md = "intro\n\n```kotlin\n" + lines(100, "val x =") + "\n```\nafter"
        val p = checked(md)
        assertTrue(p.truncated)
        assertTrue(p.markdown.startsWith("intro\n\n```kotlin\nval x = 0\n"))
        assertTrue(p.markdown.endsWith("\n```"))
        assertFalse(p.markdown.contains("after"))
    }

    @Test
    fun aLongerFenceIsClosedWithTheSameLengthAndAShorterOneInsideDoesNotCloseIt() {
        val md = "````md\n```\ninside\n```\n" + lines(100, "code") + "\n````\n"
        val p = checked(md)
        assertTrue(p.truncated)
        assertTrue(p.markdown.startsWith("````md\n```\ninside\n```\ncode 0"))
        assertTrue(p.markdown.endsWith("\n````"))
    }

    @Test
    fun aTildeFenceWithBlankLinesAndPipesIsNotATable() {
        val inside = (0 until 60).joinToString("\n") { if (it % 3 == 0) "" else "| a | b |\n|---|---|" }
        val md = "~~~~\n$inside\n~~~~\nafter"
        val p = checked(md, Budget(2_000, 40, 1))
        assertTrue(p.truncated)
        assertTrue(p.markdown.endsWith("\n~~~~"))
        // Inside the fence the table rule (1 row) does not apply: the budget of lines does.
        assertTrue(p.markdown.split('\n').size > 10)
    }

    @Test
    fun anInfoStringWithABacktickIsNotAFence() {
        val md = "```not`a fence\n" + lines(100)
        val p = checked(md)
        assertTrue(p.truncated)
        assertFalse(p.markdown.endsWith("```"))
    }

    @Test
    fun aFenceNeverClosedInTheSourceIsClosedInThePreview() {
        val p = checked("text\n\n~~~\n" + lines(100))
        assertTrue(p.markdown.endsWith("\n~~~"))
    }

    @Test
    fun aTabIndentedFenceIsIndentedCodeNotAFence() {
        val p = checked("\t```\n" + lines(100))
        assertTrue(p.truncated)
        assertFalse(p.markdown.endsWith("```"))
    }

    // --- tables -------------------------------------------------------------------------------------------------------

    @Test
    fun aLongTableIsCutBetweenRowsAfterItsDelimiter() {
        val rows = (0 until 30).joinToString("\n") { "| $it | x |" }
        val md = "intro\n\n| a | b |\n|---|:-:|\n$rows\n\nafter"
        val p = checked(md)
        assertTrue(p.truncated)
        val expected = "intro\n\n| a | b |\n|---|:-:|\n" + (0 until 8).joinToString("\n") { "| $it | x |" }
        assertEquals(expected, p.markdown)
    }

    @Test
    fun aTableWhoseHeadDoesNotFitIsLeftOut() {
        val md = lines(38) + "\n| a | b |\n|---|---|\n| 1 | 2 |"
        val p = checked(md)
        assertTrue(p.truncated)
        assertFalse(p.markdown.contains('|'))
        assertEquals(lines(38), p.markdown)
    }

    @Test
    fun aHeadingUnderlineIsNotATableDelimiter() {
        val md = "a | b\n---\n" + lines(100)
        val p = checked(md, Budget(2_000, 40, 0))
        assertTrue(p.markdown.startsWith("a | b\n---\nline 0"))
    }

    // --- other blocks -------------------------------------------------------------------------------------------------

    @Test
    fun aLongListKeepsWholeItems() {
        val p = checked((0 until 100).joinToString("\n") { "- item $it" })
        assertTrue(p.truncated)
        assertTrue(p.markdown.split('\n').all { it.startsWith("- item ") })
    }

    @Test
    fun theCutPrefersTheEndOfAParagraphNearTheBudget() {
        val block = "a\nb\nc\nd\ne\n\n"
        val p = checked(block.repeat(20))
        assertTrue(p.truncated)
        assertEquals(block.repeat(6).trimEnd(), p.markdown)
    }

    @Test
    fun aGiantLineIsCutOnASpace() {
        val md = "word ".repeat(10_000)
        val p = checked(md)
        assertTrue(p.truncated)
        assertTrue(p.markdown.length in 1_000..2_000)
        assertTrue(md.startsWith(p.markdown))
        assertTrue(p.markdown.endsWith("word"))
    }

    @Test
    fun aGiantLineWithoutSpaceIsCutAnyway() {
        val p = checked("a".repeat(50_000))
        assertTrue(p.truncated)
        assertTrue(p.markdown.length in 1_000..2_000)
    }

    @Test
    fun aCutNeverSplitsASurrogatePairOrAnEmojiCluster() {
        val faces = checked("😀".repeat(3_000)).markdown
        assertWellFormedUtf16(faces)
        assertTrue(faces.isNotEmpty())

        val family = "👨‍👩‍👧"
        val families = checked(family.repeat(1_000)).markdown
        assertWellFormedUtf16(families)
        assertTrue(families.endsWith(family))

        val flags = checked("🇫🇷".repeat(1_000)).markdown
        assertEquals(0, flags.length % 4)

        val thumbs = checked("👍🏽".repeat(1_000)).markdown
        assertTrue(thumbs.endsWith("👍🏽"))

        for (cut in listOf(head("😀".repeat(3_000), 40, 2_001), head("é".repeat(3_000), 40, 2_000))) {
            assertWellFormedUtf16(cut.markdown)
            assertFalse(cut.markdown.endsWith("e"))
        }
    }

    @Test
    fun crlfAndCrLineEndsAreLines() {
        for (sep in listOf("\r\n", "\r")) {
            val md = (0 until 100).joinToString(sep) { "line $it" }
            val p = checked(md)
            assertTrue(p.truncated)
            assertTrue(md.startsWith(p.markdown))
            assertFalse(p.markdown.endsWith("\r"))
            assertEquals(39, p.markdown.split(sep).size)
        }
        val short = "a\r\nb\r\n"
        assertEquals(short, preview(short).markdown)
    }

    // --- reference definitions ----------------------------------------------------------------------------------------

    @Test
    fun usedReferenceDefinitionsFromFurtherDownAreAppended() {
        val md = "See [the doc][doc], [X] and [code].\n\n" + lines(100) +
            "\n\n```\n[code]: https://e.example/in-a-fence\n```\n\n" +
            "[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[unused]: https://e.example/u\n[code]: https://e.example/code"
        val p = checked(md)
        assertTrue(p.truncated)
        assertTrue(
            p.markdown.endsWith("\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[code]: https://e.example/code"),
        )
        assertFalse(p.markdown.contains("unused"))
        assertFalse(p.markdown.contains("in-a-fence"))
    }

    @Test
    fun aDefinitionAlreadyInThePreviewIsNotRepeated() {
        val md = "[a] here\n\n[a]: https://e.example/1\n\n" + lines(100) + "\n\n[a]: https://e.example/2"
        val p = checked(md)
        assertFalse(p.markdown.contains("e.example/2"))
        assertTrue(p.markdown.contains("e.example/1"))
    }

    // --- head ---------------------------------------------------------------------------------------------------------

    @Test
    fun headKeepsTheFirstLines() {
        assertEquals("a\nb", head("a\nb\nc", 2, 100).markdown)
        assertTrue(head("a\nb\nc", 2, 100).truncated)
        val all = head("a\nb\n", 2, 100)
        assertEquals("a\nb\n", all.markdown)
        assertFalse(all.truncated)
        val long = head("one two three four five six", 5, 15)
        assertEquals("one two three", long.markdown)
        assertTrue(long.truncated)
        assertEquals("x\r\ny", head("x\r\ny\r\nz", 2, 100).markdown)
    }

    @Test
    fun toStringNeverShowsTheContent() {
        val p = preview("hunter2 is the password")
        assertFalse(p.toString().contains("hunter2"))
        assertFalse(head("hunter2", 1, 3).toString().contains("hun"))
    }

    // --- review probes ------------------------------------------------------------------------------------------------

    @Test
    fun degenerateBudgetsNeverLoopNorThrow() {
        val md = "# T\n\n```\n" + lines(50) + "\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n" + "x".repeat(5_000)
        for (b in listOf(Budget(0, 0, 0), Budget(-5, -5, -5), Budget(1, 1, 0), Budget(10, 2, 1), Budget(45, 3, 1))) {
            val p = preview(md, b)
            assertTrue(p.truncated)
            assertWellFormedUtf16(p.markdown)
        }
        assertEquals("", head("abc\ndef", 0, 100).markdown)
        assertEquals("", head("abc\ndef", 10, 0).markdown)
        assertEquals("", head("abc\ndef", -1, -1).markdown)
    }

    @Test
    fun aFenceInsideAListItemIsClosedWithItsIndent() {
        val md = "See [doc].\n\n- item\n\n  ```\n" + lines(100, "  code") + "\n  ```\n\n[doc]: https://e.example/doc"
        val p = checked(md)
        assertTrue(p.truncated)
        // A closing fence at column 0 would end the list item and open a new fence swallowing the appended definition.
        assertTrue(p.markdown.endsWith("\n  ```\n\n[doc]: https://e.example/doc"))
    }

    @Test
    fun aDefinitionIsNeverCutInsideItsUrl() {
        // Past the free cap, a definition costs like any line; cutting it would define [x] with a truncated URL.
        // 300 definitions: about 8 KB free, the last 20 are charged; the [x] line comes with ~1,300 characters of room.
        val free = (0 until 300).joinToString("\n") { "[d$it]: https://e.example/$it" }
        val url = "https://e.example/" + "a".repeat(3_000)
        val md = "See [x].\n\n$free\n" + lines(5) + "\n[x]: $url\n" + lines(50)
        val p = checked(md)
        assertTrue(p.truncated)
        val defs = p.markdown.split('\n').filter { it.startsWith("[x]:") }
        assertTrue(defs.all { it == "[x]: $url" })
        assertTrue(p.markdown.contains("\nline 4"))
    }

    @Test
    fun aMegabyteOfDefinitionsStaysLinear() {
        val used = (0 until 300).joinToString(" ") { "[l$it]" }
        val md = used + "\n\n" + lines(100) + "\n\n" + (0 until 120_000).joinToString("\n") { "[d$it]: u" }
        assertTrue(md.length > 1_000_000)
        val start = System.nanoTime()
        repeat(3) { preview(md) }
        val ms = (System.nanoTime() - start) / 3_000_000
        assertTrue("preview of 1 MiB took $ms ms", ms < 1_000)
    }

    @Test
    fun randomDocumentsKeepTheInvariants() {
        val pieces = listOf(
            "# Heading", "", "", "para text with words", "- item", "1. item", "> quote", "```", "````", "~~~", "  ```", "\t```",
            "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r", "[Ref]", "<div>", "---", "a | b",
            "👨‍👩‍👧 🇫🇷 é",
            "😀".repeat(1_500),"word ".repeat(300), "y".repeat(3_000), "\u0000", "    indented", "\r",
        )
        val rnd = java.util.Random(42)
        repeat(3_000) {
            val n = 1 + rnd.nextInt(120)
            val md = (0 until n).joinToString(if (rnd.nextBoolean()) "\n" else "\r\n") { pieces[rnd.nextInt(pieces.size)] }
            val budget = if (rnd.nextBoolean()) CARD_BUDGET else Budget(1 + rnd.nextInt(300), 1 + rnd.nextInt(12), rnd.nextInt(4))
            val p = preview(md, budget)
            assertWellFormedUtf16(p.markdown)
            val again = preview(p.markdown, budget)
            assertFalse("re-cut: ${md.hashCode()}", again.truncated)
            assertEquals(p.markdown, again.markdown)
            val h = head(md, budget.maxLines, budget.maxChars)
            assertWellFormedUtf16(h.markdown)
            assertTrue(md.startsWith(h.markdown))
            assertTrue(h.markdown.replace("\r\n", "\n").length <= maxOf(budget.maxChars, 0))
        }
    }
}
