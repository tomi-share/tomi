import XCTest
@testable import TomiKit

// Same documents, same outputs as Android. The expected values were computed with the Kotlin implementation
// (apps/android/.../ui/render/MarkdownBlocks.kt compiled as is and run on the JVM) over the random documents of the two
// fuzz tests (java.util.Random(42)): a 64-bit FNV-1a of every output of `preview`, `head`, `chunks` (text, flags and
// offsets) and `sourceChunkRanges`, code unit by code unit.
//
// A change of the rules on one platform must be reported on the other: this test then fails until the Kotlin value is
// computed again (same loop as below, `Fnv.add` on each value in the same order).
final class MarkdownParityTests: XCTestCase {
    private typealias Budget = MarkdownBlocks.Budget

    private struct Fnv {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325

        private mutating func mix(_ value: UInt64) { hash = (hash ^ value) &* 0x100_0000_01b3 }

        mutating func add(_ text: String) {
            for unit in text.utf16 { mix(UInt64(unit)) }
            mix(0xFFFF)
        }

        mutating func add(_ number: Int) { mix(UInt64(UInt32(truncatingIfNeeded: number))) }
        mutating func add(_ flag: Bool) { mix(flag ? 1 : 2) }
    }

    private func addAll(_ f: inout Fnv, _ md: String, _ budget: Budget, _ lines: Int, _ chars: Int) {
        let p = MarkdownBlocks.preview(md, budget: budget)
        f.add(p.markdown)
        f.add(p.truncated)
        let h = MarkdownBlocks.head(md, maxLines: budget.maxLines, maxChars: budget.maxChars)
        f.add(h.markdown)
        f.add(h.truncated)
        for c in MarkdownBlocks.chunks(md, budget: budget) {
            f.add(c.markdown)
            f.add(c.asSource)
            f.add(c.sourceStart)
            f.add(c.sourceEnd)
            f.add(c.prefixLength)
            f.add(c.bodyLength)
            f.add(c.oversized)
        }
        for r in MarkdownBlocks.sourceChunkRanges(md, lines, chars) {
            f.add(r.start)
            f.add(r.end)
        }
    }

    func testTheDocumentsOfThePreviewFuzzGiveWhatAndroidGives() {
        let pieces = [
            "# Heading", "", "", "para text with words", "- item", "1. item", "> quote", "```", "````", "~~~", "  ```", "\t```",
            "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r", "[Ref]", "<div>", "---", "a | b",
            "👨\u{200D}👩\u{200D}👧 🇫🇷 e\u{301}",
            "😀" * 1_500, "word " * 300, "y" * 3_000, "\u{0}", "    indented", "\r",
        ]
        var rnd = JavaRandom(42)
        var f = Fnv()
        for _ in 0..<3_000 {
            let n = 1 + rnd.nextInt(120)
            let separator = rnd.nextBoolean() ? "\n" : "\r\n"
            let md = (0..<n).map { _ in pieces[rnd.nextInt(pieces.count)] }.joined(separator: separator)
            let budget: Budget
            if rnd.nextBoolean() {
                budget = MarkdownBlocks.cardBudget
            } else {
                let chars = 1 + rnd.nextInt(300)
                let lineCount = 1 + rnd.nextInt(12)
                budget = Budget(maxChars: chars, maxLines: lineCount, maxTableRows: rnd.nextInt(4))
            }
            addAll(&f, md, budget, 100, 4_000)
        }
        XCTAssertEqual("f40883be3d3aeac5", String(f.hash, radix: 16))
    }

    func testTheDocumentsOfTheChunksFuzzGiveWhatAndroidGives() {
        let pieces = [
            "# Heading", "", "", "", "para text with words", "- item", "  - sub", "1. item", "2) item", "> quote", "```", "````",
            "~~~", "  ```", "\t```", "```py`x", "| a | b |", "|---|---|", "| 1 | 2 \\| x |", "[ref]: https://e.example/r",
            "[Ref] and [other][ref]", "<div>", "<!--", "-->", "<pre>", "</pre>", "---", "===", "a | b", "* * *",
            "👨\u{200D}👩\u{200D}👧 🇫🇷 \u{E9}", "😀" * 700, "word " * 300, "y" * 3_000, "\u{0}", "    indented", "\r", "\t- tab",
        ]
        var rnd = JavaRandom(42)
        var f = Fnv()
        for _ in 0..<2_000 {
            let n = 1 + rnd.nextInt(200)
            let separator = rnd.nextBoolean() ? "\n" : "\r\n"
            let md = (0..<n).map { _ in pieces[rnd.nextInt(pieces.count)] }.joined(separator: separator)
            let budget: Budget
            if rnd.nextBoolean() {
                budget = MarkdownBlocks.chunkBudget
            } else {
                let chars = rnd.nextInt(600) - 20
                let lineCount = rnd.nextInt(20) - 2
                budget = Budget(maxChars: chars, maxLines: lineCount, maxTableRows: rnd.nextInt(5) - 1)
            }
            let lines = 1 + rnd.nextInt(30)
            let chars = rnd.nextInt(500)
            addAll(&f, md, budget, lines, chars)
        }
        XCTAssertEqual("514717d32e69925", String(f.hash, radix: 16))
    }

    /// A document of one's own, compared with the value the Kotlin implementation gives for it (skipped without
    /// `TOMI_PARITY_DOCUMENT` and `TOMI_PARITY_HASH` in the environment).
    func testADocumentGivesWhatAndroidGives() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let path = environment["TOMI_PARITY_DOCUMENT"], let expected = environment["TOMI_PARITY_HASH"] else {
            throw XCTSkip("no document to compare")
        }
        let md = try String(contentsOfFile: path, encoding: .utf8)
        var f = Fnv()
        addAll(&f, md, MarkdownBlocks.cardBudget, 100, 4_000)
        for c in MarkdownBlocks.chunks(md) {
            f.add(c.markdown)
            f.add(c.asSource)
        }
        XCTAssertEqual(expected, String(f.hash, radix: 16))
    }
}
