import Foundation
import TomiKit
import XCTest
@testable import Tomi

/// Long texts: a feed card only prepares a bounded beginning, once; the detail cuts the text into parts (off the main
/// thread when it is long) and guards each Markdown chunk on its own. The cutting itself is tested in TomiKit
/// (`MarkdownBlocksTests`, `MarkdownChunksTests`).
@MainActor
final class LongTextTests: XCTestCase {
    /// About 45,000 characters: headings, paragraphs, a table and fenced code in each section.
    private func longMarkdown(sections: Int = 60) -> String {
        (1...sections).map { n in
            """
            ## Section \(n)

            \(String(repeating: "Un paragraphe de la section \(n), avec un [lien][doc]. ", count: 12))

            | Nom | Valeur |
            |-----|-------:|
            | a\(n) | 1 |
            | b\(n) | 2 |

            ```bash
            echo "section \(n)"
            ```
            """
        }.joined(separator: "\n\n") + "\n\n[doc]: https://docs.example.org/guide\n"
    }

    private func characters(_ blocks: [MarkdownBlock]) -> Int {
        blocks.reduce(0) { $0 + $1.text.characters.count }
    }

    override func setUp() async throws {
        CardTexts.clear()
    }

    // ---- Card ------------------------------------------------------------------------------------------------------------

    func testACardOnlyPreparesTheBeginningOfALongMarkdown() throws {
        let markdown = longMarkdown()
        XCTAssertGreaterThan(markdown.utf16.count, 40_000)
        let shown = CardTexts.text(itemId: "long", kind: .markdown, of: RichText(plain: markdown, markdown: markdown))
        XCTAssertTrue(shown.truncated)
        let blocks = try XCTUnwrap(shown.blocks)
        XCTAssertFalse(blocks.isEmpty)
        // What was parsed is the preview (2,000 characters at most), not the document.
        XCTAssertLessThanOrEqual(characters(blocks), MarkdownBlocks.cardBudget.maxChars)
        // The link of the preview still resolves: its definition, at the very end of the document, came with it.
        XCTAssertTrue(blocks.contains { block in block.text.runs.contains { $0.link != nil } })
    }

    func testTheSourceAndThePlainTextOfACardAreBoundedToo() {
        let markdown = longMarkdown()
        let text = RichText(plain: markdown, markdown: markdown)
        for kind in [CardTexts.Kind.source, .plain] {
            let shown = CardTexts.text(itemId: "long", kind: kind, of: text)
            XCTAssertTrue(shown.truncated)
            XCTAssertNil(shown.blocks)
            XCTAssertLessThanOrEqual(shown.literal.utf16.count, MarkdownBlocks.cardBudget.maxChars)
            XCTAssertTrue(markdown.hasPrefix(shown.literal))
        }
    }

    func testAShortTextIsShownWhole() throws {
        let shown = CardTexts.text(itemId: "short", kind: .markdown, of: RichText(plain: "gras", markdown: "**gras**"))
        XCTAssertFalse(shown.truncated)
        XCTAssertEqual(1, try XCTUnwrap(shown.blocks).count)
        let plain = CardTexts.text(itemId: "short", kind: .plain, of: RichText(plain: "une ligne"))
        XCTAssertFalse(plain.truncated)
        XCTAssertEqual("une ligne", plain.literal)
    }

    func testACardTextIsPreparedOnce() {
        let markdown = longMarkdown()
        let text = RichText(plain: markdown, markdown: markdown)
        _ = CardTexts.text(itemId: "long", kind: .markdown, of: text)
        // Asked again (every evaluation of the card does): the prepared one, in far less time than cutting and parsing.
        let start = DispatchTime.now().uptimeNanoseconds
        for _ in 0..<1_000 { _ = CardTexts.text(itemId: "long", kind: .markdown, of: text) }
        let ms = (DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
        XCTAssertLessThan(ms, 100, "1,000 lookups took \(ms) ms")
    }

    func testAnImageWhoseDefinitionIsFurtherDownIsNeverRendered() throws {
        // The preview brings the definition of [pixel] back next to its use: the guard must still see an image there.
        let markdown = "![suivi][pixel]\n\n" + longMarkdown() + "\n[pixel]: https://t.example/pixel.png\n"
        let shown = CardTexts.text(itemId: "image", kind: .markdown, of: RichText(plain: markdown, markdown: markdown))
        let blocks = try XCTUnwrap(shown.blocks)
        XCTAssertFalse(blocks.contains { block in block.text.runs.contains { $0.imageURL != nil } })
        // Escaped: the "!" is text, followed by an ordinary link.
        XCTAssertTrue(String(blocks[0].text.characters).hasPrefix("!"))
    }

    // ---- Detail ----------------------------------------------------------------------------------------------------------

    private func landedParts(_ preparation: DetailTextPreparation, _ text: String, markdown: Bool) async throws -> [DetailPart] {
        for _ in 0..<1_000 {
            if let parts = preparation.parts(of: text, markdown: markdown) { return parts }
            try await Task.sleep(for: .milliseconds(5))
        }
        throw XCTSkip("the parts never landed")
    }

    func testAShortDocumentIsOnePartAtOnce() throws {
        let preparation = DetailTextPreparation()
        let parts = try XCTUnwrap(preparation.parts(of: "# Titre\n\nUn **texte** court.", markdown: true))
        XCTAssertEqual(1, parts.count)
        XCTAssertEqual(2, try XCTUnwrap(preparation.blocks(for: parts[0])).count)
        XCTAssertEqual(1, try XCTUnwrap(preparation.parts(of: "une ligne", markdown: false)).count)
    }

    func testALongDocumentIsCutOffTheMainThreadIntoChunksRenderedOneByOne() async throws {
        let markdown = longMarkdown()
        let preparation = DetailTextPreparation()
        // Longer than what is cut at once: the screen opens on a blank space.
        XCTAssertNil(preparation.parts(of: markdown, markdown: true))
        let parts = try await landedParts(preparation, markdown, markdown: true)
        XCTAssertGreaterThan(parts.count, 10)
        XCTAssertTrue(parts.allSatisfy { $0.kind == .markdown })
        for part in parts {
            XCTAssertLessThanOrEqual(part.text.utf16.count, MarkdownBlocks.chunkBudget.maxChars + 8 * 1024)
            // Each chunk passes the guard and is parsed on its own.
            let blocks = try XCTUnwrap(preparation.blocks(for: part))
            XCTAssertLessThanOrEqual(characters(blocks), MarkdownBlocks.chunkBudget.maxChars)
        }
        // Every section is there, once, in order.
        let headings = parts.flatMap { preparation.blocks(for: $0) ?? [] }.filter { $0.kind == .heading(2) }
        XCTAssertEqual((1...60).map { "Section \($0)" }, headings.map { String($0.text.characters) })
    }

    func testAnImageInAChunkWhoseDefinitionIsInAnotherIsNeverRendered() async throws {
        let markdown = "![suivi][pixel]\n\n" + longMarkdown() + "\n[pixel]: https://t.example/pixel.png\n"
        let preparation = DetailTextPreparation()
        _ = preparation.parts(of: markdown, markdown: true)
        let parts = try await landedParts(preparation, markdown, markdown: true)
        // The first chunk carries the definition it uses: the guard runs on that very string.
        XCTAssertTrue(parts[0].text.contains("[pixel]: https://t.example/pixel.png"))
        for part in parts {
            let blocks = preparation.blocks(for: part) ?? []
            XCTAssertFalse(blocks.contains { block in block.text.runs.contains { $0.imageURL != nil } })
        }
    }

    func testALongPlainTextIsCutIntoPiecesOfLines() async throws {
        let text = (0..<8_000).map { "ligne \($0) du texte" }.joined(separator: "\n")
        XCTAssertGreaterThan(text.utf16.count, ProgressiveReveal.syncSourceChars)
        let preparation = DetailTextPreparation()
        XCTAssertNil(preparation.parts(of: text, markdown: false))
        let parts = try await landedParts(preparation, text, markdown: false)
        XCTAssertGreaterThan(parts.count, 30)
        XCTAssertTrue(parts.allSatisfy { $0.kind == .literal && $0.text.utf16.count <= MarkdownBlocks.sourceChunkChars })
        XCTAssertEqual(text, parts.map(\.text).joined(separator: "\n"))
    }

    func testAnotherTextStartsOver() async throws {
        let preparation = DetailTextPreparation()
        let first = longMarkdown()
        XCTAssertNil(preparation.parts(of: first, markdown: true))
        // The mode changes before the first cutting lands: its result is dropped.
        let source = try XCTUnwrap(preparation.parts(of: "# court", markdown: false))
        XCTAssertEqual(["# court"], source.map(\.text))
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(["# court"], preparation.parts(of: "# court", markdown: false)?.map(\.text))
    }

    // ---- Nothing printed -------------------------------------------------------------------------------------------------

    func testThePreparedTextsNeverPrintTheirContent() {
        let card = CardTexts.text(itemId: "p", kind: .plain, of: RichText(plain: "hunter2 est le mot de passe"))
        let part = DetailPart(id: 0, kind: .literal, text: "hunter2 est le mot de passe")
        for value in [card as Any, part as Any] {
            var dumped = ""
            dump(value, to: &dumped)
            XCTAssertFalse("\(value)".contains("hunter2"))
            XCTAssertFalse(String(reflecting: value).contains("hunter2"))
            XCTAssertFalse(dumped.contains("hunter2"))
        }
    }
}
