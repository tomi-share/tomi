package eu.shaffe.tomi.ui.code

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.time.Instant

class CodeFileLogicTest {
    private val now = Instant.parse("2026-11-05T10:00:00Z")
    private val source = DeviceRef("pc", "PC-Bureau")

    private fun item(attachments: List<Attachment>, secret: Boolean = false, text: RichText? = null) = readableItem(
        id = "1", sourceDevice = source, createdAt = now, attachments = attachments, secret = secret, text = text,
    )

    private val png = Attachment("a.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)
    private val rs = Attachment("main.rs", "text/x-rust", 10)
    private val js = Attachment("app.js", "text/javascript", 10)
    private val kt = Attachment("Notes.kt", "text/x-kotlin", 10)

    // --- extension -> language -------------------------------------------------------------------------------------

    @Test
    fun theExtensionsOfTheBriefAreCodeFiles() {
        listOf(
            "a.php", "a.rs", "a.js", "a.ts", "a.css", "a.html", "a.json", "a.py", "a.kt", "a.java", "a.sh", "a.go", "a.rb",
            "a.c", "a.cpp", "a.sql", "a.yaml", "a.toml", "a.md",
        ).forEach { assertTrue(it, CodeLanguages.isCodeFile(it)) }
    }

    @Test
    fun theExtensionsWithAGrammarMapToTheLibraryLanguages() {
        mapOf(
            "main.rs" to "rust", "app.js" to "javascript", "app.mjs" to "javascript", "x.jsx" to "javascript", "x.ts" to "typescript",
            "x.tsx" to "tsx", "s.css" to "css", "i.html" to "html", "i.htm" to "html", "d.json" to "json", "t.py" to "python",
            "T.java" to "java", "run.sh" to "bash", "run.bash" to "bash", "main.go" to "go", "m.c" to "c", "m.h" to "c",
            "c.yaml" to "yaml", "c.yml" to "yaml", "README.md" to "markdown",
            // Grammar added by our own native library (not in Enriched Markdown).
            "index.php" to "php", "view.phtml" to "php", "X.PHP" to "php",
        ).forEach { (name, grammar) -> assertEquals(name, grammar, CodeLanguages.grammarFor(name)) }
    }

    @Test
    fun aCodeFileWithoutGrammarStillOpensButIsNotColoured() {
        listOf("a.rb", "a.cpp", "a.sql", "a.toml", "Notes.kt", "build.gradle.kts", "X.KT").forEach {
            assertTrue(it, CodeLanguages.isCodeFile(it))
            assertNull(it, CodeLanguages.grammarFor(it))
        }
    }

    @Test
    fun extensionsAreCaseInsensitiveAndNeedARealExtension() {
        assertTrue(CodeLanguages.isCodeFile("MAIN.RS"))
        assertEquals("rust", CodeLanguages.grammarFor("MAIN.RS"))
        assertEquals("rs", CodeLanguages.extensionOf("dir/sub.name/main.rs"))
        assertNull(CodeLanguages.extensionOf("Makefile"))
        assertNull(CodeLanguages.extensionOf(".env"))
        assertNull(CodeLanguages.extensionOf("trailing."))
        assertFalse(CodeLanguages.isCodeFile("notes.pdf"))
        assertFalse(CodeLanguages.isCodeFile("photo.png"))
        assertFalse(CodeLanguages.isCodeFile("archive.zip"))
        assertFalse(CodeLanguages.isCodeFile("rs"))
    }

    @Test
    fun anImageIsNeverACodeFile() {
        assertTrue(CodeFiles.isCodeFile(rs))
        assertFalse(CodeFiles.isCodeFile(Attachment("diagram.rs", "image/png", 10)))
        assertFalse(CodeFiles.isCodeFile(pdf))
        assertFalse(CodeFiles.isCodeFile(png))
    }

    // --- attachment indices ----------------------------------------------------------------------------------------

    @Test
    fun codeFilesKeepTheirSlot() {
        val mixed = item(listOf(png, rs, pdf, js, kt))
        val files = CodeFiles.of(mixed)
        assertEquals(listOf(1, 3, 4), files.map { it.slot })
        assertEquals(listOf("main.rs", "app.js", "Notes.kt"), files.map { it.attachment.name })
        assertEquals(listOf("rust", "javascript", null), files.map { it.grammar })
    }

    @Test
    fun codeFilesKeepTheirOwnSlotWhenOthersWereRemoved() {
        // Slots 0 and 2 removed: png is slot 1, rs slot 3, js slot 5.
        val files = CodeFiles.of(item(listOf(png.withSlot(1), rs.withSlot(3), js.withSlot(5))))
        assertEquals(listOf(3, 5), files.map { it.slot })
        assertEquals(1, CodeFiles.positionFor(files, 5))
        assertEquals(0, CodeFiles.positionFor(files, 1))
        assertEquals(5, CodeFiles.slotAt(files, 1))
    }

    @Test
    fun theStartPositionIsTheFileOfTheTappedAttachment() {
        val files = CodeFiles.of(item(listOf(png, rs, pdf, js)))
        assertEquals(0, CodeFiles.positionFor(files, 1))
        assertEquals(1, CodeFiles.positionFor(files, 3))
        // Not a code file of the item: the first one.
        assertEquals(0, CodeFiles.positionFor(files, 0))
        assertEquals(0, CodeFiles.positionFor(files, 99))
    }

    @Test
    fun aPositionGivesTheSlotTheCoreIsAskedFor() {
        val files = CodeFiles.of(item(listOf(png, rs, pdf, js)))
        assertEquals(1, CodeFiles.slotAt(files, 0))
        assertEquals(3, CodeFiles.slotAt(files, 1))
        // Out of range is coerced, an item without code files has none.
        assertEquals(3, CodeFiles.slotAt(files, 7))
        assertEquals(1, CodeFiles.slotAt(files, -2))
        assertNull(CodeFiles.slotAt(emptyList(), 0))
    }

    @Test
    fun thePositionLabelIsOneBasedAndCoerced() {
        assertEquals("1/3", CodeFiles.positionLabel(0, 3))
        assertEquals("3/3", CodeFiles.positionLabel(2, 3))
        assertEquals("3/3", CodeFiles.positionLabel(9, 3))
        assertEquals("1/3", CodeFiles.positionLabel(-1, 3))
    }

    // --- secret gate -----------------------------------------------------------------------------------------------

    @Test
    fun aHiddenItemShowsNothingUntilTheGatePassed() {
        val secret = item(listOf(rs, js), secret = true)
        assertEquals(CodeContent.Locked, CodeGate.content(secret, loaded = true, unlocked = false))
        val unlocked = CodeGate.content(secret, loaded = true, unlocked = true)
        assertTrue(unlocked is CodeContent.Files)
        assertEquals(2, (unlocked as CodeContent.Files).files.size)
    }

    @Test
    fun aNormalItemNeedsNoGate() {
        val normal = item(listOf(rs))
        assertTrue(CodeGate.content(normal, loaded = true, unlocked = false) is CodeContent.Files)
    }

    @Test
    fun loadingEmptyAndNoCodeStates() {
        assertEquals(CodeContent.Loading, CodeGate.content(null, loaded = false, unlocked = false))
        assertEquals(CodeContent.Empty, CodeGate.content(null, loaded = true, unlocked = false))
        assertEquals(CodeContent.Empty, CodeGate.content(item(listOf(png, pdf)), loaded = true, unlocked = false))
        // A hidden item without a code file is locked first: the gate never reveals what the item holds.
        assertEquals(CodeContent.Locked, CodeGate.content(item(listOf(png), secret = true), loaded = true, unlocked = false))
    }

    @Test
    fun onlyAHiddenItemClosesWhenTheAppLeavesTheScreen() {
        assertTrue(CodeGate.closesOnStop(item(listOf(rs), secret = true), changingConfigurations = false))
        // A rotation stops the activity too and must not close the screen.
        assertFalse(CodeGate.closesOnStop(item(listOf(rs), secret = true), changingConfigurations = true))
        assertFalse(CodeGate.closesOnStop(item(listOf(rs)), changingConfigurations = false))
        assertFalse(CodeGate.closesOnStop(null, changingConfigurations = false))
    }

    // --- decoding --------------------------------------------------------------------------------------------------

    @Test
    fun aFileIsDecodedAsUtf8WithUniformLinesAndExpandedTabs() {
        val result = CodeDecoding.decode("a\r\n\tb\rc\n".toByteArray())
        assertTrue(result is CodeLoadResult.Text)
        result as CodeLoadResult.Text
        assertEquals("a\r\n\tb\rc\n", result.original)
        assertEquals("a\n    b\nc\n", result.display)
    }

    @Test
    fun aByteOrderMarkIsDropped() {
        val bytes = byteArrayOf(0xEF.toByte(), 0xBB.toByte(), 0xBF.toByte()) + "fn main() {}".toByteArray()
        assertEquals("fn main() {}", (CodeDecoding.decode(bytes) as CodeLoadResult.Text).original)
    }

    @Test
    fun accentsSurviveAndInvalidBytesDoNotCrash() {
        assertEquals("déjà ✓", (CodeDecoding.decode("déjà ✓".toByteArray()) as CodeLoadResult.Text).original)
        assertTrue(CodeDecoding.decode(byteArrayOf(0x66, 0xFF.toByte(), 0x6F)) is CodeLoadResult.Text)
    }

    @Test
    fun aBinaryFileIsRefused() {
        assertEquals(CodeLoadResult.Binary, CodeDecoding.decode(byteArrayOf(0x50, 0x4B, 0x00, 0x04)))
    }

    @Test
    fun aFileOverTheLimitIsRefusedWithoutReadingItAll() {
        val big = ByteArray(CodeLimits.MAX_BYTES + 1) { 'a'.code.toByte() }
        assertEquals(CodeLoadResult.TooLarge, CodeDecoding.decode(big))
        assertEquals(CodeLoadResult.TooLarge, CodeDecoding.read(ByteArrayInputStream(big)))
        val limit = ByteArray(CodeLimits.MAX_BYTES) { 'a'.code.toByte() }
        assertTrue(CodeDecoding.read(ByteArrayInputStream(limit)) is CodeLoadResult.Text)
    }

    @Test
    fun anEmptyFileIsAnEmptyDocument() {
        val text = CodeDecoding.read(ByteArrayInputStream(ByteArray(0))) as CodeLoadResult.Text
        assertEquals("", text.display)
        assertEquals(1, CodeDocument(text.display).lineCount)
    }

    @Test
    fun colouringIsLimitedLikeTheLibrary() {
        assertTrue(CodeLimits.canHighlight("fn main() {}", lineCount = 1))
        assertFalse(CodeLimits.canHighlight("x", lineCount = CodeLimits.HIGHLIGHT_MAX_LINES + 1))
        assertTrue(CodeLimits.canHighlight("x", lineCount = CodeLimits.HIGHLIGHT_MAX_LINES))
        assertFalse(CodeLimits.canHighlight("a".repeat(CodeLimits.HIGHLIGHT_MAX_BYTES + 1), lineCount = 1))
        // 50 KB of characters that take 2 bytes each is over the byte limit.
        assertFalse(CodeLimits.canHighlight("é".repeat(CodeLimits.HIGHLIGHT_MAX_BYTES / 2 + 1), lineCount = 1))
    }

    // --- document --------------------------------------------------------------------------------------------------

    @Test
    fun aDocumentConvertsOffsetsToLinesAndColumns() {
        val doc = CodeDocument("ab\n\ncde\nf")
        assertEquals(4, doc.lineCount)
        assertEquals(listOf("ab", "", "cde", "f"), (0 until doc.lineCount).map { doc.line(it) })
        assertEquals(0, doc.lineOf(0))
        assertEquals(0, doc.lineOf(2)) // the newline belongs to its line
        assertEquals(1, doc.lineOf(3))
        assertEquals(2, doc.lineOf(4))
        assertEquals(3, doc.lineOf(8))
        assertEquals(3, doc.lineOf(999))
        assertEquals(2, doc.columnOf(6))
        assertEquals(0, doc.columnOf(8))
    }

    @Test
    fun aTrailingNewlineAddsAnEmptyLastLine() {
        val doc = CodeDocument("a\n")
        assertEquals(2, doc.lineCount)
        assertEquals("", doc.line(1))
    }

    @Test
    fun theWidestLineIsCappedForTheLayout() {
        assertEquals(5, CodeDocument("abc\nabcde\nab").maxLineLength)
        assertEquals(CodeLimits.MAX_DISPLAY_LINE_CHARS, CodeDocument("a".repeat(CodeLimits.MAX_DISPLAY_LINE_CHARS * 3)).maxLineLength)
    }

    // --- search ----------------------------------------------------------------------------------------------------

    private val sample = "let Total = 1;\nlet total2 = total + 1;\n// TOTAL\n"

    @Test
    fun searchFindsEveryOccurrenceIgnoringCaseInOrder() {
        val matches = CodeSearch.find(sample, "total")
        assertEquals(4, matches.size)
        assertEquals(listOf(4, 19, 28, 42), matches.map { it.start })
        assertTrue(matches.all { it.end - it.start == 5 })
        assertTrue(matches.zipWithNext().all { (a, b) -> a.end <= b.start })
    }

    @Test
    fun searchOccurrencesNeverOverlap() {
        assertEquals(listOf(0, 2), CodeSearch.find("aaaa", "aa").map { it.start })
    }

    @Test
    fun anEmptyQueryOrNoMatchFindsNothing() {
        assertTrue(CodeSearch.find(sample, "").isEmpty())
        assertTrue(CodeSearch.find(sample, "zzz").isEmpty())
        assertTrue(CodeSearch.find("", "a").isEmpty())
    }

    @Test
    fun searchStopsAtTheListingCap() {
        val many = "a".repeat(CodeSearch.MAX_MATCHES + 100)
        assertEquals(CodeSearch.MAX_MATCHES, CodeSearch.find(many, "a").size)
    }

    @Test
    fun nextAndPreviousWrapAround() {
        assertEquals(1, CodeSearch.next(0, 4))
        assertEquals(0, CodeSearch.next(3, 4))
        assertEquals(3, CodeSearch.previous(0, 4))
        assertEquals(1, CodeSearch.previous(2, 4))
        // A single occurrence stays on itself.
        assertEquals(0, CodeSearch.next(0, 1))
        assertEquals(0, CodeSearch.previous(0, 1))
        // Nothing to navigate to.
        assertEquals(-1, CodeSearch.next(-1, 0))
        assertEquals(-1, CodeSearch.previous(-1, 0))
    }

    @Test
    fun theCounterIsOneBasedAndEmptyWhenThereIsNoMatch() {
        assertEquals("1/4", CodeSearch.counter(0, 4))
        assertEquals("4/4", CodeSearch.counter(3, 4))
        assertEquals("0/0", CodeSearch.counter(-1, 0))
        assertEquals("0/0", CodeSearch.counter(0, 0))
        assertEquals("5/${CodeSearch.MAX_MATCHES}+", CodeSearch.counter(4, CodeSearch.MAX_MATCHES))
    }

    @Test
    fun theFirstOccurrenceToSelectIsTheNextOneFromAnOffsetElseTheFirst() {
        val matches = CodeSearch.find(sample, "total")
        assertEquals(0, CodeSearch.firstFrom(matches, 0))
        assertEquals(1, CodeSearch.firstFrom(matches, 5))
        assertEquals(3, CodeSearch.firstFrom(matches, 42))
        assertEquals(0, CodeSearch.firstFrom(matches, 1000)) // past the last one: wrap to the first
        assertEquals(-1, CodeSearch.firstFrom(emptyList(), 0))
    }

    @Test
    fun anOccurrenceIsLocatedByLineAndColumnToScrollToIt() {
        val doc = CodeDocument(sample)
        val matches = CodeSearch.find(doc.text, "total")
        assertEquals(listOf(0, 1, 1, 2), matches.map { doc.lineOf(it.start) })
        assertEquals(listOf(4, 4, 13, 3), matches.map { doc.columnOf(it.start) })
    }

    @Test
    fun matchesAreBucketedByLineWithTheirRank() {
        val doc = CodeDocument(sample)
        val byLine = CodeLines.bucketMatches(doc, CodeSearch.find(doc.text, "total"))
        assertEquals(setOf(0, 1, 2), byLine.keys)
        assertEquals(listOf(0), byLine.getValue(0).map { it.index })
        assertEquals(listOf(1, 2), byLine.getValue(1).map { it.index })
        assertEquals(listOf(3), byLine.getValue(2).map { it.index })
        assertEquals(listOf(4 to 9), byLine.getValue(0).map { it.startCol to it.endCol })
    }

    @Test
    fun aMatchAcrossTwoLinesIsCutAtTheLineEnd() {
        val doc = CodeDocument("ab\ncd")
        val byLine = CodeLines.bucketMatches(doc, listOf(CodeMatch(1, 4)))
        assertEquals(listOf(1 to 2), byLine.getValue(0).map { it.startCol to it.endCol })
        assertEquals(listOf(0 to 1), byLine.getValue(1).map { it.startCol to it.endCol })
    }

    @Test
    fun horizontalScrollOnlyMovesWhenTheColumnIsOutOfView() {
        // 60 characters visible from the character 0.
        assertNull(CodeLines.horizontalScrollTarget(col = 20, visibleStartChar = 0, visibleChars = 60))
        // Far to the right: scrolls so that the column sits in the first third.
        assertEquals(100, CodeLines.horizontalScrollTarget(col = 120, visibleStartChar = 0, visibleChars = 60))
        // Back to the left of the view.
        assertEquals(0, CodeLines.horizontalScrollTarget(col = 3, visibleStartChar = 50, visibleChars = 60))
        assertNull(CodeLines.horizontalScrollTarget(col = 3, visibleStartChar = 0, visibleChars = 0))
    }

    // --- colour runs -----------------------------------------------------------------------------------------------

    @Test
    fun colourRunsAreCutAtLineEndsAndGroupedByLine() {
        val doc = CodeDocument("fn a()\n/* x\ny */\nz")
        // (start, end, colour): "fn" on line 0, a comment from line 1 to line 2.
        val flat = intArrayOf(0, 2, 0x11, 7, 16, 0x22)
        val byLine = CodeLines.bucketRuns(doc, flat)
        assertEquals(listOf(Triple(0, 2, 0x11)), byLine.getValue(0).map { Triple(it.startCol, it.endCol, it.argb) })
        assertEquals(listOf(Triple(0, 4, 0x22)), byLine.getValue(1).map { Triple(it.startCol, it.endCol, it.argb) })
        assertEquals(listOf(Triple(0, 4, 0x22)), byLine.getValue(2).map { Triple(it.startCol, it.endCol, it.argb) })
        assertNull(byLine[3])
    }

    @Test
    fun malformedRunsAreIgnoredWithoutCrashing() {
        val doc = CodeDocument("abc")
        val byLine = CodeLines.bucketRuns(doc, intArrayOf(5, 2, 1, -3, 99, 2, 1, 1))
        assertTrue(byLine.isEmpty() || byLine.values.flatten().all { it.startCol in 0..3 && it.endCol in 0..3 })
    }

    @Test
    fun noRunsMeansNoColour() {
        assertTrue(CodeLines.bucketRuns(CodeDocument("abc"), IntArray(0)).isEmpty())
    }

    // --- line wrap -------------------------------------------------------------------------------------------------

    @Test
    fun theWrapToggleFlipsTheState() {
        assertTrue(CodeWrap.toggled(false))
        assertFalse(CodeWrap.toggled(true))
        assertFalse(CodeWrap.toggled(CodeWrap.toggled(false)))
    }

    @Test
    fun theTextWidthIsWhatTheGutterThePaddingsLeave() {
        assertEquals(300, CodeWrap.textWidthPx(400, 40f, 50f, 10f))
        // Never zero or negative: a measure needs a width.
        assertEquals(1, CodeWrap.textWidthPx(20, 40f, 50f, 10f))
    }

    @Test
    fun aRowAlreadyComfortablyVisibleIsNotScrolled() {
        assertNull(CodeWrap.scrollDelta(rowTop = 300f, rowBottom = 319f, viewportStart = 0, viewportEnd = 1000, margin = 40f))
    }

    @Test
    fun aRowBelowTheViewportIsScrolledToAThirdOfIt() {
        // Row at 1500; target top = 1000 / 3 -> content moves up by 1500 - 333.33.
        val delta = CodeWrap.scrollDelta(rowTop = 1500f, rowBottom = 1519f, viewportStart = 0, viewportEnd = 1000, margin = 40f)!!
        assertEquals(1500f - 1000f / 3f, delta, 0.01f)
    }

    @Test
    fun aRowAboveTheViewportScrollsBackwards() {
        val delta = CodeWrap.scrollDelta(rowTop = -200f, rowBottom = -181f, viewportStart = 0, viewportEnd = 1000, margin = 40f)!!
        assertTrue(delta < 0f)
    }

    @Test
    fun aRowAtTheEdgeIsMovedIn() {
        // Inside the viewport, but within the margin of its bottom.
        assertTrue(CodeWrap.scrollDelta(rowTop = 970f, rowBottom = 989f, viewportStart = 0, viewportEnd = 1000, margin = 40f) != null)
    }

    @Test
    fun anEmptyViewportScrollsNothing() {
        assertNull(CodeWrap.scrollDelta(rowTop = 10f, rowBottom = 29f, viewportStart = 0, viewportEnd = 0, margin = 40f))
    }}
