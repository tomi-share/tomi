import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/render/MarkdownBlocksTest.kt.
//
// The card preview cuts a long text BEFORE the guard, the parser and the renderer. The cut must stay within the budget,
// never leave a construct half open (fenced code, table) and be stable: the preview of a preview is itself.
final class MarkdownBlocksTests: XCTestCase {
    private typealias Budget = MarkdownBlocks.Budget
    private typealias Preview = MarkdownBlocks.Preview

    private let cardBudget = MarkdownBlocks.cardBudget

    private func preview(_ md: String, _ budget: Budget = MarkdownBlocks.cardBudget) -> Preview {
        MarkdownBlocks.preview(md, budget: budget)
    }

    private func head(_ text: String, _ maxLines: Int, _ maxChars: Int) -> Preview {
        MarkdownBlocks.head(text, maxLines: maxLines, maxChars: maxChars)
    }

    private func lines(_ count: Int, _ prefix: String = "line") -> String { numberedLines(count, prefix) }

    /// Checks the budget (whole lines, characters, appended definitions aside) and idempotence of a preview.
    private func checked(
        _ md: String, _ budget: Budget = MarkdownBlocks.cardBudget, file: StaticString = #filePath, line: UInt = #line
    ) -> Preview {
        let p = preview(md, budget)
        let again = preview(p.markdown, budget)
        XCTAssertFalse(again.truncated, "a preview is never cut again", file: file, line: line)
        assertSame(p.markdown, again.markdown, "idempotent", file: file, line: line)
        let body = p.markdown.kSubstringBefore("\n\n[")
        XCTAssertLessThanOrEqual(body.kSplit("\n").count, budget.maxLines, "lines within budget", file: file, line: line)
        XCTAssertLessThanOrEqual(body.kLength, budget.maxChars, "chars within budget", file: file, line: line)
        return p
    }

    // --- not truncated ---------------------------------------------------------------------------------------------------

    func testAShortTextIsKeptAsIs() {
        let md = "# Title\n\nSome **text** with a [link](https://e.example).\n\n- a\n- b\n"
        let p = preview(md)
        XCTAssertFalse(p.truncated)
        assertSame(md, p.markdown)
    }

    func testAnEmptyTextIsKeptAsIs() {
        assertSame("", preview("").markdown)
        XCTAssertFalse(preview("").truncated)
        assertSame("", head("", 10, 100).markdown)
        XCTAssertFalse(head("", 10, 100).truncated)
    }

    func testAShortUnclosedFenceIsKeptAsIs() {
        let md = "```\ncode\nmore"
        assertSame(md, preview(md).markdown)
        XCTAssertFalse(preview(md).truncated)
    }

    // --- fenced code -----------------------------------------------------------------------------------------------------

    func testACutInsideBacktickFencedCodeClosesIt() {
        let md = "intro\n\n```kotlin\n" + lines(100, "val x =") + "\n```\nafter"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertTrue(p.markdown.kStartsWith("intro\n\n```kotlin\nval x = 0\n"))
        XCTAssertTrue(p.markdown.kEndsWith("\n```"))
        XCTAssertFalse(p.markdown.kContains("after"))
    }

    func testALongerFenceIsClosedWithTheSameLengthAndAShorterOneInsideDoesNotCloseIt() {
        let md = "````md\n```\ninside\n```\n" + lines(100, "code") + "\n````\n"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertTrue(p.markdown.kStartsWith("````md\n```\ninside\n```\ncode 0"))
        XCTAssertTrue(p.markdown.kEndsWith("\n````"))
    }

    func testATildeFenceWithBlankLinesAndPipesIsNotATable() {
        let inside = (0..<60).map { $0 % 3 == 0 ? "" : "| a | b |\n|---|---|" }.joined(separator: "\n")
        let md = "~~~~\n\(inside)\n~~~~\nafter"
        let p = checked(md, Budget(maxChars: 2_000, maxLines: 40, maxTableRows: 1))
        XCTAssertTrue(p.truncated)
        XCTAssertTrue(p.markdown.kEndsWith("\n~~~~"))
        // Inside the fence the table rule (1 row) does not apply: the budget of lines does.
        XCTAssertGreaterThan(p.markdown.kSplit("\n").count, 10)
    }

    func testAnInfoStringWithABacktickIsNotAFence() {
        let md = "```not`a fence\n" + lines(100)
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertFalse(p.markdown.kEndsWith("```"))
    }

    func testAFenceNeverClosedInTheSourceIsClosedInThePreview() {
        let p = checked("text\n\n~~~\n" + lines(100))
        XCTAssertTrue(p.markdown.kEndsWith("\n~~~"))
    }

    func testATabIndentedFenceIsIndentedCodeNotAFence() {
        let p = checked("\t```\n" + lines(100))
        XCTAssertTrue(p.truncated)
        XCTAssertFalse(p.markdown.kEndsWith("```"))
    }

    // --- tables ----------------------------------------------------------------------------------------------------------

    func testALongTableIsCutBetweenRowsAfterItsDelimiter() {
        let rows = (0..<30).map { "| \($0) | x |" }.joined(separator: "\n")
        let md = "intro\n\n| a | b |\n|---|:-:|\n\(rows)\n\nafter"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        let expected = "intro\n\n| a | b |\n|---|:-:|\n" + (0..<8).map { "| \($0) | x |" }.joined(separator: "\n")
        assertSame(expected, p.markdown)
    }

    func testATableWhoseHeadDoesNotFitIsLeftOut() {
        let md = lines(38) + "\n| a | b |\n|---|---|\n| 1 | 2 |"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertFalse(p.markdown.kContains("|"))
        assertSame(lines(38), p.markdown)
    }

    func testAHeadingUnderlineIsNotATableDelimiter() {
        let md = "a | b\n---\n" + lines(100)
        let p = checked(md, Budget(maxChars: 2_000, maxLines: 40, maxTableRows: 0))
        XCTAssertTrue(p.markdown.kStartsWith("a | b\n---\nline 0"))
    }

    // --- other blocks ----------------------------------------------------------------------------------------------------

    func testALongListKeepsWholeItems() {
        let p = checked((0..<100).map { "- item \($0)" }.joined(separator: "\n"))
        XCTAssertTrue(p.truncated)
        XCTAssertTrue(p.markdown.kSplit("\n").allSatisfy { $0.kStartsWith("- item ") })
    }

    func testTheCutPrefersTheEndOfAParagraphNearTheBudget() {
        let block = "a\nb\nc\nd\ne\n\n"
        let p = checked(block * 20)
        XCTAssertTrue(p.truncated)
        assertSame((block * 6).kTrimEnd(), p.markdown)
    }

    func testAGiantLineIsCutOnASpace() {
        let md = "word " * 10_000
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertTrue((1_000...2_000).contains(p.markdown.kLength))
        XCTAssertTrue(md.kStartsWith(p.markdown))
        XCTAssertTrue(p.markdown.kEndsWith("word"))
    }

    func testAGiantLineWithoutSpaceIsCutAnyway() {
        let p = checked("a" * 50_000)
        XCTAssertTrue(p.truncated)
        XCTAssertTrue((1_000...2_000).contains(p.markdown.kLength))
    }

    func testACutNeverSplitsASurrogatePairOrAnEmojiCluster() {
        let faces = checked("😀" * 3_000).markdown
        assertWellFormed(faces)
        XCTAssertFalse(faces.isEmpty)

        let family = "👨\u{200D}👩\u{200D}👧"
        let families = checked(family * 1_000).markdown
        assertWellFormed(families)
        XCTAssertTrue(families.kEndsWith(family))

        let flags = checked("🇫🇷" * 1_000).markdown
        XCTAssertEqual(0, flags.kLength % 4)

        let thumbs = checked("👍🏽" * 1_000).markdown
        XCTAssertTrue(thumbs.kEndsWith("👍🏽"))

        for cut in [head("😀" * 3_000, 40, 2_001), head("e\u{301}" * 3_000, 40, 2_000)] {
            assertWellFormed(cut.markdown)
            XCTAssertFalse(cut.markdown.kEndsWith("e"))
        }
    }

    func testCrlfAndCrLineEndsAreLines() {
        for sep in ["\r\n", "\r"] {
            let md = (0..<100).map { "line \($0)" }.joined(separator: sep)
            let p = checked(md)
            XCTAssertTrue(p.truncated)
            XCTAssertTrue(md.kStartsWith(p.markdown))
            XCTAssertFalse(p.markdown.kEndsWith("\r"))
            XCTAssertEqual(39, p.markdown.kSplit(sep).count)
        }
        let short = "a\r\nb\r\n"
        assertSame(short, preview(short).markdown)
    }

    // --- reference definitions -------------------------------------------------------------------------------------------

    func testUsedReferenceDefinitionsFromFurtherDownAreAppended() {
        let md = "See [the doc][doc], [X] and [code].\n\n" + lines(100)
            + "\n\n```\n[code]: https://e.example/in-a-fence\n```\n\n"
            + "[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[unused]: https://e.example/u\n[code]: https://e.example/code"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        XCTAssertTrue(
            p.markdown.kEndsWith("\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[code]: https://e.example/code")
        )
        XCTAssertFalse(p.markdown.kContains("unused"))
        XCTAssertFalse(p.markdown.kContains("in-a-fence"))
    }

    func testADefinitionAlreadyInThePreviewIsNotRepeated() {
        let md = "[a] here\n\n[a]: https://e.example/1\n\n" + lines(100) + "\n\n[a]: https://e.example/2"
        let p = checked(md)
        XCTAssertFalse(p.markdown.kContains("e.example/2"))
        XCTAssertTrue(p.markdown.kContains("e.example/1"))
    }

    // --- head ------------------------------------------------------------------------------------------------------------

    func testHeadKeepsTheFirstLines() {
        assertSame("a\nb", head("a\nb\nc", 2, 100).markdown)
        XCTAssertTrue(head("a\nb\nc", 2, 100).truncated)
        let all = head("a\nb\n", 2, 100)
        assertSame("a\nb\n", all.markdown)
        XCTAssertFalse(all.truncated)
        let long = head("one two three four five six", 5, 15)
        assertSame("one two three", long.markdown)
        XCTAssertTrue(long.truncated)
        assertSame("x\r\ny", head("x\r\ny\r\nz", 2, 100).markdown)
    }

    func testToStringNeverShowsTheContent() {
        let p = preview("hunter2 is the password")
        XCTAssertFalse("\(p)".contains("hunter2"))
        XCTAssertFalse(String(describing: head("hunter2", 1, 3)).contains("hun"))
        XCTAssertFalse(String(reflecting: p).contains("hunter2"))
        var dumped = ""
        dump(p, to: &dumped)
        XCTAssertFalse(dumped.contains("hunter2"))
    }

    // --- review probes ---------------------------------------------------------------------------------------------------

    func testDegenerateBudgetsNeverLoopNorThrow() {
        let md = "# T\n\n```\n" + lines(50) + "\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n" + "x" * 5_000
        let budgets = [
            Budget(maxChars: 0, maxLines: 0, maxTableRows: 0), Budget(maxChars: -5, maxLines: -5, maxTableRows: -5),
            Budget(maxChars: 1, maxLines: 1, maxTableRows: 0), Budget(maxChars: 10, maxLines: 2, maxTableRows: 1),
            Budget(maxChars: 45, maxLines: 3, maxTableRows: 1),
        ]
        for b in budgets {
            let p = preview(md, b)
            XCTAssertTrue(p.truncated)
            assertWellFormed(p.markdown)
        }
        assertSame("", head("abc\ndef", 0, 100).markdown)
        assertSame("", head("abc\ndef", 10, 0).markdown)
        assertSame("", head("abc\ndef", -1, -1).markdown)
    }

    func testAFenceInsideAListItemIsClosedWithItsIndent() {
        let md = "See [doc].\n\n- item\n\n  ```\n" + lines(100, "  code") + "\n  ```\n\n[doc]: https://e.example/doc"
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        // A closing fence at column 0 would end the list item and open a new fence swallowing the appended definition.
        XCTAssertTrue(p.markdown.kEndsWith("\n  ```\n\n[doc]: https://e.example/doc"))
    }

    func testADefinitionIsNeverCutInsideItsUrl() {
        // Past the free cap, a definition costs like any line; cutting it would define [x] with a truncated URL.
        // 300 definitions: about 8 KB free, the last 20 are charged; the [x] line comes with ~1,300 characters of room.
        let free = (0..<300).map { "[d\($0)]: https://e.example/\($0)" }.joined(separator: "\n")
        let url = "https://e.example/" + "a" * 3_000
        let md = "See [x].\n\n\(free)\n" + lines(5) + "\n[x]: \(url)\n" + lines(50)
        let p = checked(md)
        XCTAssertTrue(p.truncated)
        let defs = p.markdown.kSplit("\n").filter { $0.kStartsWith("[x]:") }
        XCTAssertTrue(defs.allSatisfy { $0 == "[x]: \(url)" })
        XCTAssertTrue(p.markdown.kContains("\nline 4"))
    }

    func testAMegabyteOfDefinitionsStaysLinear() {
        let used = (0..<300).map { "[l\($0)]" }.joined(separator: " ")
        let md = used + "\n\n" + lines(100) + "\n\n" + (0..<120_000).map { "[d\($0)]: u" }.joined(separator: "\n")
        XCTAssertGreaterThan(md.kLength, 1_000_000)
        let ms = elapsedMilliseconds { for _ in 0..<3 { _ = preview(md) } } / 3
        XCTAssertLessThan(ms, 1_000, "preview of 1 MiB took \(ms) ms")
    }

    func testHeadOnlyReadsTheBeginningOfTheText() {
        // Not in the Kotlin tests: `head` of a megabyte costs what the budget allows, not the megabyte.
        let text = lines(200_000)
        XCTAssertGreaterThan(text.kLength, 1_000_000)
        let ms = elapsedMilliseconds { for _ in 0..<200 { _ = head(text, 40, 2_000) } }
        XCTAssertLessThan(ms, 1_000, "200 heads of 1 MiB took \(ms) ms")
        let one = head("x" * 1_000_000, 40, 2_000)
        XCTAssertTrue(one.truncated)
        XCTAssertEqual(1_999, one.markdown.kLength)
    }

    func testRandomDocumentsKeepTheInvariants() {
        let pieces = [
            "# Heading", "", "", "para text with words", "- item", "1. item", "> quote", "```", "````", "~~~", "  ```", "\t```",
            "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r", "[Ref]", "<div>", "---", "a | b",
            "👨\u{200D}👩\u{200D}👧 🇫🇷 e\u{301}",
            "😀" * 1_500, "word " * 300, "y" * 3_000, "\u{0}", "    indented", "\r",
        ]
        var rnd = JavaRandom(42)
        for _ in 0..<3_000 {
            let n = 1 + rnd.nextInt(120)
            let separator = rnd.nextBoolean() ? "\n" : "\r\n"
            let md = (0..<n).map { _ in pieces[rnd.nextInt(pieces.count)] }.joined(separator: separator)
            let budget: Budget
            if rnd.nextBoolean() {
                budget = cardBudget
            } else {
                let chars = 1 + rnd.nextInt(300)
                let lineCount = 1 + rnd.nextInt(12)
                budget = Budget(maxChars: chars, maxLines: lineCount, maxTableRows: rnd.nextInt(4))
            }
            let p = preview(md, budget)
            assertWellFormed(p.markdown)
            let again = preview(p.markdown, budget)
            XCTAssertFalse(again.truncated, "re-cut")
            assertSame(p.markdown, again.markdown)
            let h = head(md, budget.maxLines, budget.maxChars)
            assertWellFormed(h.markdown)
            XCTAssertTrue(md.kStartsWith(h.markdown))
            XCTAssertLessThanOrEqual(h.markdown.kReplacing("\r\n", with: "\n").kLength, max(budget.maxChars, 0))
        }
    }
}
