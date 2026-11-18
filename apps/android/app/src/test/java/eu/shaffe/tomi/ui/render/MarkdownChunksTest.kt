package eu.shaffe.tomi.ui.render

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The detail cuts a long document into chunks rendered one by one. Rejoined (repeated headers and added fences aside), the
 * chunks must give the source back; a cut must never change what a construct renders (ordered list, list item, indented code,
 * setext heading, HTML comment); an oversized block is cut where it can be (table rows, fenced lines, bullet items).
 */
class MarkdownChunksTest {
    private fun lines(n: Int, prefix: String = "line") = (0 until n).joinToString("\n") { "$prefix $it" }

    private fun isSeparator(s: String) = s == "" || s == "\n" || s == "\r\n" || s == "\r"

    /** Checks the invariants of [chunks] on [md] and returns them. */
    private fun checked(md: String, budget: Budget = CHUNK_BUDGET): List<Chunk> {
        val parts = chunks(md, budget)
        if (md.isEmpty()) {
            assertTrue(parts.isEmpty())
            return parts
        }
        assertTrue("at least one chunk", parts.isNotEmpty())
        assertEquals("starts at the beginning", 0, parts.first().sourceStart)
        for ((index, chunk) in parts.withIndex()) {
            assertTrue("no empty chunk", chunk.markdown.isNotEmpty())
            val slice = md.substring(chunk.sourceStart, chunk.sourceEnd)
            assertEquals(
                "the source is in the chunk as is",
                slice,
                chunk.markdown.substring(chunk.prefixLength, chunk.prefixLength + slice.length),
            )
            if (chunk.asSource) assertEquals("a source chunk is the exact source", slice, chunk.markdown)
            val next = parts.getOrNull(index + 1)?.sourceStart ?: md.length
            assertTrue("only a line separator between chunks", isSeparator(md.substring(chunk.sourceEnd, next)))
            if (!chunk.oversized) {
                val body = chunk.markdown.substring(0, chunk.bodyLength).replace("\r\n", "\n").replace('\r', '\n')
                assertTrue("chars within budget (${body.length + 1})", body.length + 1 <= maxOf(budget.maxChars, 0) || parts.size == 1)
                assertTrue("lines within budget", body.split('\n').size <= maxOf(budget.maxLines, 0) || parts.size == 1)
            }
        }
        return parts
    }

    /** The source rebuilt from the chunks (each chunk's own source range + the separator after it). */
    private fun rebuilt(md: String, parts: List<Chunk>): String = buildString {
        for ((index, chunk) in parts.withIndex()) {
            append(chunk.markdown, chunk.prefixLength, chunk.prefixLength + chunk.sourceEnd - chunk.sourceStart)
            append(md, chunk.sourceEnd, parts.getOrNull(index + 1)?.sourceStart ?: md.length)
        }
    }

    private fun body(chunk: Chunk) = chunk.markdown.substring(0, chunk.bodyLength)

    // --- basics -------------------------------------------------------------------------------------------------------

    @Test
    fun aShortDocumentIsOneChunkAsIs() {
        val md = "# Title\n\nSome text.\n\n- a\n- b\n"
        val parts = checked(md)
        assertEquals(1, parts.size)
        // The final line separator is not part of the last chunk (the detail then renders [md] itself anyway).
        assertEquals(md.removeSuffix("\n"), parts.single().markdown)
        assertFalse(parts.single().asSource)
    }

    @Test
    fun emptyAndBlankInputs() {
        assertTrue(chunks("").isEmpty())
        assertEquals(1, checked("\n\n  \n").size)
        assertTrue(sourceChunks("").isEmpty())
    }

    @Test
    fun paragraphsAreCutOnBlankLinesAndRebuildTheSource() {
        val md = (0 until 200).joinToString("\n\n") { "Paragraph $it " + "word ".repeat(30) }
        val parts = checked(md)
        assertTrue(parts.size > 5)
        assertEquals(md, rebuilt(md, parts))
        assertTrue(parts.all { it.markdown.startsWith("Paragraph ") })
    }

    @Test
    fun aCutPrefersTheLineBeforeAHeading() {
        val section = "## Section\n\n" + (0 until 6).joinToString("\n\n") { "para $it " + "x".repeat(150) } + "\n\n"
        val md = section.repeat(20)
        val parts = checked(md)
        assertTrue(parts.size > 3)
        assertTrue(parts.all { it.markdown.startsWith("## Section") })
    }

    @Test
    fun toStringNeverShowsTheContent() {
        val parts = chunks("hunter2 is the password\n\n" + "p\n\n".repeat(3_000))
        assertTrue(parts.size > 1)
        assertFalse(parts.first().toString().contains("hunter2"))
    }

    // --- fenced code --------------------------------------------------------------------------------------------------

    @Test
    fun aLongFenceIsReopenedWithItsOwnOpeningLine() {
        val md = "intro\n\n```kotlin title=\"a\"\n" + lines(450, "val x =") + "\n```\n\nafter"
        val parts = checked(md)
        assertEquals(md, rebuilt(md, parts))
        val code = parts.filter { it.markdown.contains("val x =") }
        assertTrue(code.size >= 4)
        for (chunk in code) {
            val text = body(chunk)
            assertTrue(text.startsWith("```kotlin title=\"a\"\n") || text.startsWith("intro\n\n```kotlin title=\"a\"\n"))
            assertTrue(text.endsWith("\n```") || text.endsWith("\n```\n"))
        }
        // Each line exactly once, in order.
        val seen = code.flatMap { body(it).split('\n') }.filter { it.startsWith("val x =") }
        assertEquals((0 until 450).map { "val x = $it" }, seen)
    }

    @Test
    fun longerTildeIndentedAndUnclosedFences() {
        val tilde = "~~~~ md\n```\ninside\n```\n" + lines(300, "| a |\n\ncode") + "\n~~~~\n"
        val parts = checked(tilde)
        assertTrue(parts.size > 1)
        assertTrue(parts.all { body(it).startsWith("~~~~ md\n") })
        assertTrue(parts.dropLast(1).all { body(it).endsWith("\n~~~~") })

        val indented = "- item\n\n  ```\n" + lines(300, "  code") + "\n  ```\n"
        val p2 = checked(indented)
        assertTrue(p2.size > 1)
        assertTrue(p2.drop(1).all { body(it).startsWith("  ```\n") })
        assertTrue(p2.dropLast(1).all { body(it).endsWith("\n  ```") })

        val unclosed = "text\n\n```\n" + lines(400)
        val p3 = checked(unclosed)
        assertTrue(p3.size > 1)
        assertEquals(unclosed, rebuilt(unclosed, p3))
        assertFalse(p3.last().markdown.endsWith("```"))
    }

    @Test
    fun anInfoStringWithABacktickIsNotAFence() {
        val md = "```not`a fence\n\n" + (0 until 300).joinToString("\n\n") { "p $it" }
        val parts = checked(md)
        assertTrue(parts.size > 1)
        // Not a fence: the blank lines after it are chunk starts and nothing is reopened.
        assertTrue(parts.all { it.prefixLength == 0 })
        assertTrue(parts[1].markdown.startsWith("p "))
    }

    @Test
    fun blankLinesInsideAFenceAreNeverChunkStarts() {
        val inside = (0 until 40).joinToString("\n") { if (it % 2 == 0) "" else "line $it" }
        val md = "```\n$inside\n```\n\nafter"
        val parts = checked(md, Budget(4_000, 1_000, 30))
        assertEquals(1, parts.size)
    }

    // --- tables -------------------------------------------------------------------------------------------------------

    @Test
    fun aLongTableRepeatsItsHeaderAndKeepsEachRowOnce() {
        val header = "| a | b \\| c | `d` |\n|---|:-:|---|"
        val rows = (0 until 100).map { "| $it | x \\| y | `p\\|q` |" }
        val md = "intro\n\n$header\n" + rows.joinToString("\n") + "\n\nafter"
        val parts = checked(md)
        assertEquals(md, rebuilt(md, parts))
        val tableChunks = parts.filter { it.markdown.contains("| x \\| y |") }
        assertTrue(tableChunks.size >= 4)
        for (chunk in tableChunks) {
            val text = body(chunk)
            assertTrue(text.startsWith(header) || text.startsWith("intro\n\n$header"))
            assertTrue(text.split('\n').count { it.startsWith("| ") && it.contains("x \\| y") } <= 30)
        }
        val seen = tableChunks.flatMap { body(it).split('\n') }.filter { it.contains("x \\| y") }
        assertEquals(rows, seen)
    }

    // --- lists --------------------------------------------------------------------------------------------------------

    @Test
    fun aLooseOrderedListIsNeverCut() {
        val md = (1..150).joinToString("\n\n") { "$it. step " + "w ".repeat(20) }
        val parts = checked(md)
        assertEquals(1, parts.size)
        assertEquals(md, parts.single().markdown)
    }

    @Test
    fun aHugeOrderedListBecomesSourceRatherThanRestartingAtOne() {
        val md = (1..600).joinToString("\n\n") { "$it. step " + "w ".repeat(20) }
        val parts = checked(md)
        assertEquals(1, parts.size)
        assertTrue(parts.single().asSource)
    }

    @Test
    fun aTightBulletListIsCutBetweenTopLevelItemsOnly() {
        val md = (0 until 200).joinToString("\n") { "- item $it\n  - sub $it\n    continued $it" }
        val parts = checked(md)
        assertTrue(parts.size > 1)
        assertEquals(md, rebuilt(md, parts))
        for (chunk in parts) {
            val first = body(chunk).split('\n').first()
            assertTrue("starts with a top-level item: $first", first.startsWith("- item "))
            val last = body(chunk).split('\n').last()
            assertTrue("ends with a whole item", last.startsWith("    continued "))
        }
    }

    @Test
    fun indentedContinuationsAndCodeAreNeverOrphaned() {
        val item = "- item\n\n  continued paragraph\n\n      indented code\n\n  more\n"
        val md = (0 until 20).joinToString("\n") { item } + "\n" + "Para\n\n".repeat(400)
        val parts = checked(md, Budget(400, 100, 30))
        for (chunk in parts) {
            val first = body(chunk).trimStart('\n', '\r').first()
            assertTrue("never starts indented", first != ' ' && first != '\t')
        }
    }

    // --- other blocks -------------------------------------------------------------------------------------------------

    @Test
    fun aSetextHeadingStaysWithItsText() {
        val md = (0 until 300).joinToString("\n\n") { "Heading $it\n---------\n\ntext $it" }
        val parts = checked(md, Budget(500, 100, 30))
        assertTrue(parts.size > 5)
        for (chunk in parts) assertFalse(body(chunk).startsWith("---"))
    }

    @Test
    fun aRuleAloneIsABlock() {
        val md = (0 until 300).joinToString("\n\n") { "text $it\n\n---" }
        val parts = checked(md, Budget(500, 100, 30))
        assertTrue(parts.size > 5)
        assertEquals(md, rebuilt(md, parts))
    }

    @Test
    fun anHtmlCommentWithBlankLinesIsNeverCut() {
        val comment = "<!--\n" + (0 until 30).joinToString("\n\n") { "hidden $it" } + "\n-->"
        val md = "intro\n\n$comment\n\nafter\n\n" + "para\n\n".repeat(300)
        val parts = checked(md, Budget(200, 100, 30))
        val holding = parts.filter { it.markdown.contains("hidden") }
        assertEquals(1, holding.size)
        assertTrue(holding.single().markdown.contains("<!--") && holding.single().markdown.contains("-->"))
    }

    // --- reference definitions ----------------------------------------------------------------------------------------

    @Test
    fun referenceDefinitionsAtTheEndGoWithEachChunkUsingThem() {
        val md = (0 until 100).joinToString("\n\n") { if (it % 10 == 0) "See [the doc][Doc] and [x]. $it" else "para $it " + "w ".repeat(40) } +
            "\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[unused]: https://e.example/u"
        val parts = checked(md)
        assertTrue(parts.size > 2)
        for (chunk in parts) {
            val uses = body(chunk).contains("[the doc][Doc]")
            val definesHere = body(chunk).contains("[doc]: ")
            if (uses && !definesHere) {
                assertTrue(chunk.markdown.endsWith("\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\""))
            }
            assertTrue("a definition at most once", chunk.markdown.split("[doc]: ").size <= 2)
            assertFalse(!definesHere && chunk.markdown.contains("[unused]:"))
        }
    }

    @Test
    fun nothingIsAppendedAfterAnUnclosedFence() {
        val md = "[doc]: https://e.example/doc\n\nSee [doc].\n\n" + "para\n\n".repeat(1_000) + "```\nSee [doc] in code\n" + lines(20)
        val parts = checked(md)
        assertTrue(parts.size > 2)
        assertFalse(parts.last().markdown.contains("https://e.example/doc"))
    }

    // --- line ends, characters, degenerate input ----------------------------------------------------------------------

    @Test
    fun crlfAndCrSourcesRebuildExactly() {
        for (sep in listOf("\r\n", "\r")) {
            val md = (0 until 400).joinToString(sep) { if (it % 3 == 0) "" else "line $it with\ttabs" } + sep
            val parts = checked(md, Budget(500, 30, 30))
            assertTrue(parts.size > 1)
            assertEquals(md, rebuilt(md, parts))
        }
    }

    @Test
    fun aGiantLineIsShownAsSource() {
        val md = "intro\n\n" + "a".repeat(50_000) + "\n\nafter"
        val parts = checked(md)
        val source = parts.filter { it.asSource }
        assertEquals(1, source.size)
        // The source chunk is the exact source (its trailing blank line included).
        assertEquals("a".repeat(50_000), source.single().markdown.trimEnd())
        assertTrue(sourceChunks(source.single().markdown).all { it.length <= 4_000 })
        // A single giant line alone is one source chunk.
        val alone = checked("😀".repeat(25_000))
        assertTrue(alone.single().asSource)
    }

    @Test
    fun emojiAtAChunkBoundaryIsKeptWhole() {
        val md = (0 until 500).joinToString("\n\n") { "👨‍👩‍👧 paragraph $it 🇫🇷" }
        val parts = checked(md, Budget(300, 100, 30))
        assertEquals(md, rebuilt(md, parts))
        for (chunk in parts) assertTrue(chunk.markdown.startsWith("👨‍👩‍👧"))
    }

    @Test
    fun degenerateBudgetsNeverLoop() {
        val md = "# T\n\n```\n" + lines(50) + "\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\n- a\n- b\n\n" + "x".repeat(5_000)
        for (b in listOf(Budget(0, 0, 0), Budget(-5, -5, -5), Budget(1, 1, 0), Budget(10, 2, 1))) {
            val parts = chunks(md, b)
            assertTrue(parts.isNotEmpty())
            assertEquals(md, rebuilt(md, parts))
            assertTrue(parts.all { it.markdown.isNotEmpty() })
        }
        for ((l, c) in listOf(0 to 0, -1 to -1, 1 to 1)) {
            val pieces = sourceChunkRanges("ab\ncd\n😀😀\n", l, c)
            assertTrue(pieces.isNotEmpty())
        }
    }

    // --- source pieces ------------------------------------------------------------------------------------------------

    @Test
    fun sourcePiecesRebuildTheText() {
        val texts = listOf(
            lines(1_000),
            (0 until 300).joinToString("\r\n") { "l $it" },
            "word ".repeat(10_000),
            "😀".repeat(5_000),
            "a\n" + "\n".repeat(150),
            "\n\n\nx",
            "👨‍👩‍👧".repeat(2_000),
        )
        for (text in texts) {
            val ranges = sourceChunkRanges(text, 100, 4_000)
            var at = 0
            for (range in ranges) {
                assertTrue(isSeparator(text.substring(at, range[0])))
                assertTrue(range[1] > range[0])
                at = range[1]
            }
            assertTrue(isSeparator(text.substring(at)))
            assertEquals(0, ranges.first()[0])
            val pieces = sourceChunks(text)
            assertTrue(pieces.none { it.isBlank() } || pieces.size == 1)
            for (piece in pieces) {
                if (piece.isNotEmpty()) {
                    assertFalse(Character.isLowSurrogate(piece.first()))
                    assertFalse(Character.isHighSurrogate(piece.last()))
                }
                assertFalse(piece.startsWith("‍"))
            }
        }
        assertTrue(sourceChunks(lines(1_000)).all { it.split('\n').size <= 100 && it.length <= 4_000 })
        assertEquals(1, sourceChunks("short").size)
    }

    // --- real document, fuzz, performance -----------------------------------------------------------------------------

    /** The project's own feature matrix when the test runs from the repository (else an equivalent synthetic document). */
    private fun bigDocument(): String {
        val file = listOf("../../../docs/FEATURES.md", "../../docs/FEATURES.md", "docs/FEATURES.md")
            .map(::File).firstOrNull { it.isFile }
        if (file != null) return file.readText()
        return buildString {
            for (t in 0 until 14) {
                append("## Section $t\n\nSome prose about the section, ").append("words ".repeat(40)).append("\n\n")
                append("- point one\n- point two\n  - nested\n\n")
                append("| A | B | C | D | E | F | G | H |\n|---|---|---|---|---|---|---|---|\n")
                for (r in 0 until 12) append("| r$r | ✅ text | ❌ | ➖ | `code` | longer cell text here | x | y |\n")
                append('\n')
            }
        }
    }

    @Test
    fun theFeatureMatrixMakesAReasonableNumberOfChunks() {
        val md = bigDocument()
        val parts = checked(md)
        assertEquals(md, rebuilt(md, parts))
        assertTrue("chunks: ${parts.size}", parts.size in 4..40)
        assertTrue(parts.all { it.markdown.length <= 12_000 })
        assertTrue(parts.none { it.asSource })
        // Every chunk holding table rows (but not the table's start) begins with the repeated header.
        for (chunk in parts) {
            val first = body(chunk).split('\n').first()
            if (first.startsWith("|") && chunk.prefixLength > 0) {
                assertTrue(DELIMITER_LINE.matches(body(chunk).split('\n')[1]))
            }
        }
    }

    @Test
    fun randomDocumentsKeepTheInvariants() {
        val pieces = listOf(
            "# Heading", "", "", "", "para text with words", "- item", "  - sub", "1. item", "2) item", "> quote", "```", "````",
            "~~~", "  ```", "\t```", "```py`x", "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r",
            "[Ref] and [other][ref]", "<div>", "<!--", "-->", "<pre>", "</pre>", "---", "===", "a | b", "* * *",
            "👨‍👩‍👧 🇫🇷 é", "😀".repeat(700), "word ".repeat(300), "y".repeat(3_000), "\u0000", "    indented", "\r", "\t- tab",
        )
        val rnd = java.util.Random(42)
        repeat(2_000) {
            val n = 1 + rnd.nextInt(200)
            val md = (0 until n).joinToString(if (rnd.nextBoolean()) "\n" else "\r\n") { pieces[rnd.nextInt(pieces.size)] }
            val budget = if (rnd.nextBoolean()) {
                CHUNK_BUDGET
            } else {
                Budget(rnd.nextInt(600) - 20, rnd.nextInt(20) - 2, rnd.nextInt(5) - 1)
            }
            val parts = checked(md, budget)
            assertEquals("rebuilt: ${md.hashCode()}", md, rebuilt(md, parts))
            val ranges = sourceChunkRanges(md, 1 + rnd.nextInt(30), rnd.nextInt(500))
            if (md.isNotEmpty()) assertTrue(ranges.isNotEmpty())
        }
    }

    @Test
    fun aMegabyteStaysLinear() {
        val unit = "## Title\n\nSee [l1] and [l2].\n\n| a | b |\n|---|---|\n" + (0 until 40).joinToString("\n") { "| $it | v |" } +
            "\n\n```\n" + lines(150) + "\n```\n\n" + (0 until 60).joinToString("\n") { "- item $it" } + "\n\n"
        val md = unit.repeat(1_048_576 / unit.length + 1) + (0 until 2_000).joinToString("\n") { "[l$it]: https://e.example/$it" }
        assertTrue(md.length > 1_048_576)
        // Best of 3 (the first run includes the JIT warm-up; the suite runs in parallel with other tests).
        var parts = emptyList<Chunk>()
        val ms = (0 until 3).minOf {
            val start = System.nanoTime()
            parts = chunks(md)
            sourceChunks(md)
            (System.nanoTime() - start) / 1_000_000
        }
        assertTrue("chunks of 1 MiB took $ms ms", ms < 1_000)
        assertTrue(parts.size > 100)
    }

    private companion object {
        val DELIMITER_LINE = Regex("""^ {0,3}\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$""")
    }
}
