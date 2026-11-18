import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/render/MarkdownChunksTest.kt.
//
// The detail cuts a long document into chunks rendered one by one. Rejoined (repeated headers and added fences aside),
// the chunks must give the source back; a cut must never change what a construct renders (ordered list, list item,
// indented code, setext heading, HTML comment); an oversized block is cut where it can be (table rows, fenced lines,
// bullet items).
final class MarkdownChunksTests: XCTestCase {
    private typealias Budget = MarkdownBlocks.Budget
    private typealias Chunk = MarkdownBlocks.Chunk

    private func lines(_ count: Int, _ prefix: String = "line") -> String { numberedLines(count, prefix) }

    private func isSeparator(_ s: String) -> Bool {
        let units = Array(s.utf16)
        return units.isEmpty || units == [0x0A] || units == [0x0D, 0x0A] || units == [0x0D]
    }

    private func budget(_ chars: Int, _ lineCount: Int, _ rows: Int) -> Budget {
        Budget(maxChars: chars, maxLines: lineCount, maxTableRows: rows)
    }

    /// Checks the invariants of `chunks` on `md` and returns them.
    private func checked(
        _ md: String, _ budget: Budget = MarkdownBlocks.chunkBudget, file: StaticString = #filePath, line: UInt = #line
    ) -> [Chunk] {
        let parts = MarkdownBlocks.chunks(md, budget: budget)
        if md.isEmpty {
            XCTAssertTrue(parts.isEmpty, file: file, line: line)
            return parts
        }
        XCTAssertFalse(parts.isEmpty, "at least one chunk", file: file, line: line)
        XCTAssertEqual(0, parts.first?.sourceStart, "starts at the beginning", file: file, line: line)
        let source = Array(md.utf16)
        for (index, chunk) in parts.enumerated() {
            let markdown = Array(chunk.markdown.utf16)
            XCTAssertFalse(markdown.isEmpty, "no empty chunk", file: file, line: line)
            let slice = source[chunk.sourceStart..<chunk.sourceEnd]
            XCTAssertTrue(
                slice.elementsEqual(markdown[chunk.prefixLength..<(chunk.prefixLength + slice.count)]),
                "the source is in the chunk as is", file: file, line: line
            )
            if chunk.asSource {
                XCTAssertTrue(slice.elementsEqual(markdown), "a source chunk is the exact source", file: file, line: line)
            }
            let next = index + 1 < parts.count ? parts[index + 1].sourceStart : source.count
            XCTAssertTrue(
                isSeparator(String(decoding: source[chunk.sourceEnd..<next], as: UTF16.self)),
                "only a line separator between chunks", file: file, line: line
            )
            if !chunk.oversized {
                // The body with every line separator as one "\n": its length and its lines.
                var length = 0
                var lineCount = 1
                var i = 0
                while i < chunk.bodyLength {
                    if markdown[i] == 0x0D && i + 1 < chunk.bodyLength && markdown[i + 1] == 0x0A { i += 1 }
                    if markdown[i] == 0x0D || markdown[i] == 0x0A { lineCount += 1 }
                    length += 1
                    i += 1
                }
                XCTAssertTrue(
                    length + 1 <= max(budget.maxChars, 0) || parts.count == 1, "chars within budget (\(length + 1))",
                    file: file, line: line
                )
                XCTAssertTrue(lineCount <= max(budget.maxLines, 0) || parts.count == 1, "lines within budget", file: file, line: line)
            }
        }
        return parts
    }

    /// The source rebuilt from the chunks (each chunk's own source range + the separator after it).
    private func rebuilt(_ md: String, _ parts: [Chunk]) -> String {
        let source = Array(md.utf16)
        var out: [UInt16] = []
        for (index, chunk) in parts.enumerated() {
            let markdown = Array(chunk.markdown.utf16)
            out.append(contentsOf: markdown[chunk.prefixLength..<(chunk.prefixLength + chunk.sourceEnd - chunk.sourceStart)])
            out.append(contentsOf: source[chunk.sourceEnd..<(index + 1 < parts.count ? parts[index + 1].sourceStart : source.count)])
        }
        return String(decoding: out, as: UTF16.self)
    }

    private func body(_ chunk: Chunk) -> String { chunk.markdown.kSub(0, chunk.bodyLength) }

    // --- basics ----------------------------------------------------------------------------------------------------------

    func testAShortDocumentIsOneChunkAsIs() {
        let md = "# Title\n\nSome text.\n\n- a\n- b\n"
        let parts = checked(md)
        XCTAssertEqual(1, parts.count)
        // The final line separator is not part of the last chunk (the detail then renders `md` itself anyway).
        assertSame(md.kRemoveSuffix("\n"), parts[0].markdown)
        XCTAssertFalse(parts[0].asSource)
    }

    func testEmptyAndBlankInputs() {
        XCTAssertTrue(MarkdownBlocks.chunks("").isEmpty)
        XCTAssertEqual(1, checked("\n\n  \n").count)
        XCTAssertTrue(MarkdownBlocks.sourceChunks("").isEmpty)
    }

    func testParagraphsAreCutOnBlankLinesAndRebuildTheSource() {
        let md = (0..<200).map { "Paragraph \($0) " + "word " * 30 }.joined(separator: "\n\n")
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 5)
        assertSame(md, rebuilt(md, parts))
        XCTAssertTrue(parts.allSatisfy { $0.markdown.kStartsWith("Paragraph ") })
    }

    func testACutPrefersTheLineBeforeAHeading() {
        let section = "## Section\n\n" + (0..<6).map { "para \($0) " + "x" * 150 }.joined(separator: "\n\n") + "\n\n"
        let md = section * 20
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 3)
        XCTAssertTrue(parts.allSatisfy { $0.markdown.kStartsWith("## Section") })
    }

    func testToStringNeverShowsTheContent() {
        let parts = MarkdownBlocks.chunks("hunter2 is the password\n\n" + "p\n\n" * 3_000)
        XCTAssertGreaterThan(parts.count, 1)
        XCTAssertFalse("\(parts[0])".contains("hunter2"))
        XCTAssertFalse(String(reflecting: parts[0]).contains("hunter2"))
        var dumped = ""
        dump(parts[0], to: &dumped)
        XCTAssertFalse(dumped.contains("hunter2"))
    }

    // --- fenced code -----------------------------------------------------------------------------------------------------

    func testALongFenceIsReopenedWithItsOwnOpeningLine() {
        let md = "intro\n\n```kotlin title=\"a\"\n" + lines(450, "val x =") + "\n```\n\nafter"
        let parts = checked(md)
        assertSame(md, rebuilt(md, parts))
        let code = parts.filter { $0.markdown.kContains("val x =") }
        XCTAssertGreaterThanOrEqual(code.count, 4)
        for chunk in code {
            let text = body(chunk)
            XCTAssertTrue(text.kStartsWith("```kotlin title=\"a\"\n") || text.kStartsWith("intro\n\n```kotlin title=\"a\"\n"))
            XCTAssertTrue(text.kEndsWith("\n```") || text.kEndsWith("\n```\n"))
        }
        // Each line exactly once, in order.
        let seen = code.flatMap { body($0).kSplit("\n") }.filter { $0.kStartsWith("val x =") }
        XCTAssertEqual((0..<450).map { "val x = \($0)" }, seen)
    }

    func testLongerTildeIndentedAndUnclosedFences() {
        let tilde = "~~~~ md\n```\ninside\n```\n" + lines(300, "| a |\n\ncode") + "\n~~~~\n"
        let parts = checked(tilde)
        XCTAssertGreaterThan(parts.count, 1)
        XCTAssertTrue(parts.allSatisfy { body($0).kStartsWith("~~~~ md\n") })
        XCTAssertTrue(parts.dropLast().allSatisfy { body($0).kEndsWith("\n~~~~") })

        let indented = "- item\n\n  ```\n" + lines(300, "  code") + "\n  ```\n"
        let p2 = checked(indented)
        XCTAssertGreaterThan(p2.count, 1)
        XCTAssertTrue(p2.dropFirst().allSatisfy { body($0).kStartsWith("  ```\n") })
        XCTAssertTrue(p2.dropLast().allSatisfy { body($0).kEndsWith("\n  ```") })

        let unclosed = "text\n\n```\n" + lines(400)
        let p3 = checked(unclosed)
        XCTAssertGreaterThan(p3.count, 1)
        assertSame(unclosed, rebuilt(unclosed, p3))
        XCTAssertFalse(p3[p3.count - 1].markdown.kEndsWith("```"))
    }

    func testAnInfoStringWithABacktickIsNotAFence() {
        let md = "```not`a fence\n\n" + (0..<300).map { "p \($0)" }.joined(separator: "\n\n")
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 1)
        // Not a fence: the blank lines after it are chunk starts and nothing is reopened.
        XCTAssertTrue(parts.allSatisfy { $0.prefixLength == 0 })
        XCTAssertTrue(parts[1].markdown.kStartsWith("p "))
    }

    func testBlankLinesInsideAFenceAreNeverChunkStarts() {
        let inside = (0..<40).map { $0 % 2 == 0 ? "" : "line \($0)" }.joined(separator: "\n")
        let md = "```\n\(inside)\n```\n\nafter"
        let parts = checked(md, budget(4_000, 1_000, 30))
        XCTAssertEqual(1, parts.count)
    }

    // --- tables ----------------------------------------------------------------------------------------------------------

    func testALongTableRepeatsItsHeaderAndKeepsEachRowOnce() {
        let header = "| a | b \\| c | `d` |\n|---|:-:|---|"
        let rows = (0..<100).map { "| \($0) | x \\| y | `p\\|q` |" }
        let md = "intro\n\n\(header)\n" + rows.joined(separator: "\n") + "\n\nafter"
        let parts = checked(md)
        assertSame(md, rebuilt(md, parts))
        let tableChunks = parts.filter { $0.markdown.kContains("| x \\| y |") }
        XCTAssertGreaterThanOrEqual(tableChunks.count, 4)
        for chunk in tableChunks {
            let text = body(chunk)
            XCTAssertTrue(text.kStartsWith(header) || text.kStartsWith("intro\n\n\(header)"))
            XCTAssertLessThanOrEqual(text.kSplit("\n").filter { $0.kStartsWith("| ") && $0.kContains("x \\| y") }.count, 30)
        }
        let seen = tableChunks.flatMap { body($0).kSplit("\n") }.filter { $0.kContains("x \\| y") }
        XCTAssertEqual(rows, seen)
    }

    // --- lists -----------------------------------------------------------------------------------------------------------

    func testALooseOrderedListIsNeverCut() {
        let md = (1...150).map { "\($0). step " + "w " * 20 }.joined(separator: "\n\n")
        let parts = checked(md)
        XCTAssertEqual(1, parts.count)
        assertSame(md, parts[0].markdown)
    }

    func testAHugeOrderedListBecomesSourceRatherThanRestartingAtOne() {
        let md = (1...600).map { "\($0). step " + "w " * 20 }.joined(separator: "\n\n")
        let parts = checked(md)
        XCTAssertEqual(1, parts.count)
        XCTAssertTrue(parts[0].asSource)
    }

    func testATightBulletListIsCutBetweenTopLevelItemsOnly() {
        let md = (0..<200).map { "- item \($0)\n  - sub \($0)\n    continued \($0)" }.joined(separator: "\n")
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 1)
        assertSame(md, rebuilt(md, parts))
        for chunk in parts {
            let chunkLines = body(chunk).kSplit("\n")
            XCTAssertTrue(chunkLines[0].kStartsWith("- item "), "starts with a top-level item")
            XCTAssertTrue(chunkLines[chunkLines.count - 1].kStartsWith("    continued "), "ends with a whole item")
        }
    }

    func testIndentedContinuationsAndCodeAreNeverOrphaned() {
        let item = "- item\n\n  continued paragraph\n\n      indented code\n\n  more\n"
        let md = Array(repeating: item, count: 20).joined(separator: "\n") + "\n" + "Para\n\n" * 400
        let parts = checked(md, budget(400, 100, 30))
        for chunk in parts {
            let first = Array(body(chunk).utf16).first { $0 != 0x0A && $0 != 0x0D }
            XCTAssertTrue(first != 0x20 && first != 0x09, "never starts indented")
        }
    }

    // --- other blocks ----------------------------------------------------------------------------------------------------

    func testASetextHeadingStaysWithItsText() {
        let md = (0..<300).map { "Heading \($0)\n---------\n\ntext \($0)" }.joined(separator: "\n\n")
        let parts = checked(md, budget(500, 100, 30))
        XCTAssertGreaterThan(parts.count, 5)
        for chunk in parts { XCTAssertFalse(body(chunk).kStartsWith("---")) }
    }

    func testARuleAloneIsABlock() {
        let md = (0..<300).map { "text \($0)\n\n---" }.joined(separator: "\n\n")
        let parts = checked(md, budget(500, 100, 30))
        XCTAssertGreaterThan(parts.count, 5)
        assertSame(md, rebuilt(md, parts))
    }

    func testAnHtmlCommentWithBlankLinesIsNeverCut() {
        let comment = "<!--\n" + (0..<30).map { "hidden \($0)" }.joined(separator: "\n\n") + "\n-->"
        let md = "intro\n\n\(comment)\n\nafter\n\n" + "para\n\n" * 300
        let parts = checked(md, budget(200, 100, 30))
        let holding = parts.filter { $0.markdown.kContains("hidden") }
        XCTAssertEqual(1, holding.count)
        XCTAssertTrue(holding[0].markdown.kContains("<!--") && holding[0].markdown.kContains("-->"))
    }

    // --- reference definitions -------------------------------------------------------------------------------------------

    func testReferenceDefinitionsAtTheEndGoWithEachChunkUsingThem() {
        let md = (0..<100).map { $0 % 10 == 0 ? "See [the doc][Doc] and [x]. \($0)" : "para \($0) " + "w " * 40 }
            .joined(separator: "\n\n")
            + "\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\"\n[unused]: https://e.example/u"
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 2)
        for chunk in parts {
            let uses = body(chunk).kContains("[the doc][Doc]")
            let definesHere = body(chunk).kContains("[doc]: ")
            if uses && !definesHere {
                XCTAssertTrue(chunk.markdown.kEndsWith("\n\n[doc]: https://e.example/doc\n[x]: https://e.example/x \"X\""))
            }
            XCTAssertLessThanOrEqual(chunk.markdown.kSplit("[doc]: ").count, 2, "a definition at most once")
            XCTAssertFalse(!definesHere && chunk.markdown.kContains("[unused]:"))
        }
    }

    func testNothingIsAppendedAfterAnUnclosedFence() {
        let md = "[doc]: https://e.example/doc\n\nSee [doc].\n\n" + "para\n\n" * 1_000 + "```\nSee [doc] in code\n" + lines(20)
        let parts = checked(md)
        XCTAssertGreaterThan(parts.count, 2)
        XCTAssertFalse(parts[parts.count - 1].markdown.kContains("https://e.example/doc"))
    }

    // --- line ends, characters, degenerate input -------------------------------------------------------------------------

    func testCrlfAndCrSourcesRebuildExactly() {
        for sep in ["\r\n", "\r"] {
            let md = (0..<400).map { $0 % 3 == 0 ? "" : "line \($0) with\ttabs" }.joined(separator: sep) + sep
            let parts = checked(md, budget(500, 30, 30))
            XCTAssertGreaterThan(parts.count, 1)
            assertSame(md, rebuilt(md, parts))
        }
    }

    func testAGiantLineIsShownAsSource() {
        let md = "intro\n\n" + "a" * 50_000 + "\n\nafter"
        let parts = checked(md)
        let source = parts.filter(\.asSource)
        XCTAssertEqual(1, source.count)
        // The source chunk is the exact source (its trailing blank line included).
        assertSame("a" * 50_000, source[0].markdown.kTrimEnd())
        XCTAssertTrue(MarkdownBlocks.sourceChunks(source[0].markdown).allSatisfy { $0.kLength <= 4_000 })
        // A single giant line alone is one source chunk.
        let alone = checked("😀" * 25_000)
        XCTAssertEqual(1, alone.count)
        XCTAssertTrue(alone[0].asSource)
    }

    func testEmojiAtAChunkBoundaryIsKeptWhole() {
        let md = (0..<500).map { "👨\u{200D}👩\u{200D}👧 paragraph \($0) 🇫🇷" }.joined(separator: "\n\n")
        let parts = checked(md, budget(300, 100, 30))
        assertSame(md, rebuilt(md, parts))
        for chunk in parts { XCTAssertTrue(chunk.markdown.kStartsWith("👨\u{200D}👩\u{200D}👧")) }
    }

    func testDegenerateBudgetsNeverLoop() {
        let md = "# T\n\n```\n" + lines(50) + "\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\n- a\n- b\n\n" + "x" * 5_000
        for b in [budget(0, 0, 0), budget(-5, -5, -5), budget(1, 1, 0), budget(10, 2, 1)] {
            let parts = MarkdownBlocks.chunks(md, budget: b)
            XCTAssertFalse(parts.isEmpty)
            assertSame(md, rebuilt(md, parts))
            XCTAssertTrue(parts.allSatisfy { !$0.markdown.isEmpty })
        }
        for (l, c) in [(0, 0), (-1, -1), (1, 1)] {
            let pieces = MarkdownBlocks.sourceChunkRanges("ab\ncd\n😀😀\n", l, c)
            XCTAssertFalse(pieces.isEmpty)
        }
    }

    // --- source pieces ---------------------------------------------------------------------------------------------------

    func testSourcePiecesRebuildTheText() {
        let texts = [
            lines(1_000),
            (0..<300).map { "l \($0)" }.joined(separator: "\r\n"),
            "word " * 10_000,
            "😀" * 5_000,
            "a\n" + "\n" * 150,
            "\n\n\nx",
            "👨\u{200D}👩\u{200D}👧" * 2_000,
        ]
        for text in texts {
            let units = Array(text.utf16)
            let ranges = MarkdownBlocks.sourceChunkRanges(text, 100, 4_000)
            var at = 0
            for range in ranges {
                XCTAssertTrue(isSeparator(String(decoding: units[at..<range.start], as: UTF16.self)))
                XCTAssertGreaterThan(range.end, range.start)
                // Never between the two halves of a surrogate pair.
                XCTAssertFalse(units[range.start] & 0xFC00 == 0xDC00)
                XCTAssertFalse(units[range.end - 1] & 0xFC00 == 0xD800)
                at = range.end
            }
            XCTAssertTrue(isSeparator(String(decoding: units[at...], as: UTF16.self)))
            XCTAssertEqual(0, ranges.first?.start)
            let pieces = MarkdownBlocks.sourceChunks(text)
            XCTAssertEqual(ranges.count, pieces.count)
            XCTAssertTrue(pieces.allSatisfy { !$0.kIsBlank } || pieces.count == 1)
            for piece in pieces {
                assertWellFormed(piece)
                XCTAssertFalse(piece.utf16.first == 0x200D)
            }
        }
        XCTAssertTrue(MarkdownBlocks.sourceChunks(lines(1_000)).allSatisfy { $0.kSplit("\n").count <= 100 && $0.kLength <= 4_000 })
        XCTAssertEqual(1, MarkdownBlocks.sourceChunks("short").count)
    }

    // --- real document, fuzz, performance --------------------------------------------------------------------------------

    /// The project's own feature matrix when the test runs from the repository (else an equivalent synthetic document).
    private func bigDocument() -> String {
        // apps/apple/Packages/TomiKit/Tests/TomiKitTests/Logic/<this file>: the repository is seven folders up.
        var root = URL(fileURLWithPath: #filePath)
        for _ in 0..<8 { root.deleteLastPathComponent() }
        if let text = try? String(contentsOf: root.appendingPathComponent("docs/FEATURES.md"), encoding: .utf8) {
            return text
        }
        var out = ""
        for t in 0..<14 {
            out += "## Section \(t)\n\nSome prose about the section, " + "words " * 40 + "\n\n"
            out += "- point one\n- point two\n  - nested\n\n"
            out += "| A | B | C | D | E | F | G | H |\n|---|---|---|---|---|---|---|---|\n"
            for r in 0..<12 { out += "| r\(r) | ✅ text | ❌ | ➖ | `code` | longer cell text here | x | y |\n" }
            out += "\n"
        }
        return out
    }

    func testTheFeatureMatrixMakesAReasonableNumberOfChunks() {
        let md = bigDocument()
        let parts = checked(md)
        assertSame(md, rebuilt(md, parts))
        XCTAssertTrue((4...40).contains(parts.count), "chunks: \(parts.count)")
        XCTAssertTrue(parts.allSatisfy { $0.markdown.kLength <= 12_000 })
        XCTAssertFalse(parts.contains(where: \.asSource))
        // Every chunk holding table rows (but not the table's start) begins with the repeated header.
        for chunk in parts {
            let chunkLines = body(chunk).kSplit("\n")
            if chunkLines[0].kStartsWith("|") && chunk.prefixLength > 0 {
                let delimiter = Array(chunkLines[1].utf16)
                XCTAssertTrue(MarkdownBlocks.isDelimiterRow(delimiter, MarkdownBlocks.Line(start: 0, end: delimiter.count)))
            }
        }
    }

    func testRandomDocumentsKeepTheInvariants() {
        let pieces = [
            "# Heading", "", "", "", "para text with words", "- item", "  - sub", "1. item", "2) item", "> quote", "```", "````",
            "~~~", "  ```", "\t```", "```py`x", "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r",
            "[Ref] and [other][ref]", "<div>", "<!--", "-->", "<pre>", "</pre>", "---", "===", "a | b", "* * *",
            "👨\u{200D}👩\u{200D}👧 🇫🇷 \u{E9}", "😀" * 700, "word " * 300, "y" * 3_000, "\u{0}", "    indented", "\r", "\t- tab",
        ]
        var rnd = JavaRandom(42)
        for _ in 0..<2_000 {
            let n = 1 + rnd.nextInt(200)
            let separator = rnd.nextBoolean() ? "\n" : "\r\n"
            let md = (0..<n).map { _ in pieces[rnd.nextInt(pieces.count)] }.joined(separator: separator)
            let chosen: Budget
            if rnd.nextBoolean() {
                chosen = MarkdownBlocks.chunkBudget
            } else {
                let chars = rnd.nextInt(600) - 20
                let lineCount = rnd.nextInt(20) - 2
                chosen = budget(chars, lineCount, rnd.nextInt(5) - 1)
            }
            let parts = checked(md, chosen)
            assertSame(md, rebuilt(md, parts), "rebuilt")
            let maxLines = 1 + rnd.nextInt(30)
            let ranges = MarkdownBlocks.sourceChunkRanges(md, maxLines, rnd.nextInt(500))
            if !md.isEmpty { XCTAssertFalse(ranges.isEmpty) }
        }
    }

    func testAMegabyteStaysLinear() {
        let unit = "## Title\n\nSee [l1] and [l2].\n\n| a | b |\n|---|---|\n" + (0..<40).map { "| \($0) | v |" }.joined(separator: "\n")
            + "\n\n```\n" + lines(150) + "\n```\n\n" + (0..<60).map { "- item \($0)" }.joined(separator: "\n") + "\n\n"
        let md = unit * (1_048_576 / unit.kLength + 1) + (0..<2_000).map { "[l\($0)]: https://e.example/\($0)" }.joined(separator: "\n")
        XCTAssertGreaterThan(md.kLength, 1_048_576)
        // Best of 3 (the suite runs in parallel with other tests).
        var parts: [Chunk] = []
        var best = Int.max
        for _ in 0..<3 {
            best = min(best, elapsedMilliseconds {
                parts = MarkdownBlocks.chunks(md)
                _ = MarkdownBlocks.sourceChunks(md)
            })
        }
        XCTAssertLessThan(best, 1_000, "chunks of 1 MiB took \(best) ms")
        XCTAssertGreaterThan(parts.count, 100)
    }
}
