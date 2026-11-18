import XCTest
@testable import TomiKit

// Port of apps/android/app/src/test/java/eu/shaffe/tomi/ui/render/MarkdownGuardTest.kt.
//
// The markdown renderer must never fetch a remote image (offline requirement). The guard escapes image openers and
// refuses any markdown in which an unescaped image opener can still be found.
//
// Replaced: the tests that build md4c/Enriched `MarkdownASTNode` trees (anImageAnywhereInTheTreeIsFound,
// markdownWithoutImageIsHandedToTheRendererEscapedAndChecked's "parsed text" check, markdownWhoseTreeStillHasAnImageIsRefused,
// markdownThatCannotBeParsedIsRefused) by tests of the scanner `containsImage` on text. "The parser fails -> refused" has no
// equivalent: the scanner cannot fail; its fail-closed behaviour is covered by the bypass attempts below.
final class MarkdownGuardTests: XCTestCase {
    private func neutralize(_ text: String) -> String { MarkdownGuard.neutralizeImages(text) }

    // --- neutralizeImages -----------------------------------------------------------------------------------------------

    func testAnImageOpenerIsEscapedSoItBecomesALink() {
        XCTAssertEqual(neutralize("![alt](https://t.example/pixel.png)"), "\\![alt](https://t.example/pixel.png)")
        XCTAssertEqual(neutralize("a ![x](u) b ![y](v)"), "a \\![x](u) b \\![y](v)")
    }

    func testReferenceAndCollapsedImagesAreEscapedToo() {
        XCTAssertEqual(neutralize("![alt][ref]\n\n[ref]: https://t.example/p.png"), "\\![alt][ref]\n\n[ref]: https://t.example/p.png")
        XCTAssertEqual(neutralize("![ref][]"), "\\![ref][]")
        XCTAssertEqual(neutralize("![ref]"), "\\![ref]")
    }

    func testAnImageInsideALinkIsEscaped() {
        XCTAssertEqual(neutralize("[![a](u)](v)"), "[\\![a](u)](v)")
    }

    func testTextWithoutImageIsReturnedAsIs() {
        let text = "# Titre\n\n- a\n- b\n\n**gras** et [lien](https://a.example) ! [pas une image]\n"
        XCTAssertEqual(neutralize(text), text)
    }

    func testAnAlreadyEscapedOpenerIsNotEscapedTwice() {
        XCTAssertEqual(neutralize("\\![a](u)"), "\\![a](u)")
    }

    func testABackslashBeforeTheOpenerDoesNotHideTheImage() {
        // "\\" is a literal backslash, so the "![" after it still opens an image.
        XCTAssertEqual(neutralize("\\\\![a](u)"), "\\\\\\![a](u)")
    }

    func testCodeSpansAndFencedBlocksKeepTheirCharactersLiteral() {
        XCTAssertEqual(neutralize("`![a](u)` et ![b](v)"), "`![a](u)` et \\![b](v)")
        XCTAssertEqual(neutralize("``a ` ![x](u)`` ![y](v)"), "``a ` ![x](u)`` \\![y](v)")
        XCTAssertEqual(neutralize("```md\n![a](u)\n```\n![b](v)"), "```md\n![a](u)\n```\n\\![b](v)")
        XCTAssertEqual(neutralize("~~~\n![a](u)\n~~~~\n![b](v)"), "~~~\n![a](u)\n~~~~\n\\![b](v)")
    }

    func testAnUnclosedFenceKeepsEverythingAfterItLiteralLikeMarkdownDoes() {
        XCTAssertEqual(neutralize("```\n![a](u)\n![b](v)"), "```\n![a](u)\n![b](v)")
    }

    func testAFenceNeedsThreeCharactersAndAShortOneDoesNotCloseTheBlock() {
        XCTAssertEqual(neutralize("![a](u) ``"), "\\![a](u) ``")
        XCTAssertEqual(neutralize("````\n```\n![a](u)\n````\n![b](v)"), "````\n```\n![a](u)\n````\n\\![b](v)")
    }

    func testAnUnclosedBacktickIsPlainTextAndTheImageAfterItIsEscaped() {
        XCTAssertEqual(neutralize("` ![a](u)"), "` \\![a](u)")
    }

    func testLineEndingsAndTrailingNewlinesArePreserved() {
        XCTAssertEqual(neutralize("a\r\n![b](u)\r\n"), "a\r\n\\![b](u)\r\n")
        XCTAssertEqual(neutralize("![b](u)\n\n"), "\\![b](u)\n\n")
    }

    func testAnIndentedLineIsEscapedToo() {
        // Could be a lazy continuation or a list item: escaping is the safe side (the final scan is the backstop).
        XCTAssertEqual(neutralize("    ![a](u)"), "    \\![a](u)")
    }

    func testACombiningMarkAfterTheOpenerDoesNotHideIt() {
        // "[" + U+0301 is one Swift Character; the scan works on scalars so the opener is still found and escaped.
        XCTAssertEqual(neutralize("![\u{0301}a](u)"), "\\![\u{0301}a](u)")
        XCTAssertTrue(MarkdownGuard.containsImage("![\u{0301}a](u)"))
    }

    // --- containsImage / safeMarkdown --------------------------------------------------------------------------------------

    func testAnImageOpenerIsFoundInEveryImageSyntax() {
        XCTAssertTrue(MarkdownGuard.containsImage("![a](https://t.example/p.png)"))
        XCTAssertTrue(MarkdownGuard.containsImage("![alt][ref]\n\n[ref]: https://t.example/p.png"))
        XCTAssertTrue(MarkdownGuard.containsImage("![ref][]"))
        XCTAssertTrue(MarkdownGuard.containsImage("![ref]"))
        XCTAssertTrue(MarkdownGuard.containsImage("[![a](u)](v)")) // nested in a link
        XCTAssertTrue(MarkdownGuard.containsImage("> - item\n>   ![a](u)")) // quote, list
        XCTAssertTrue(MarkdownGuard.containsImage("| a |\n|---|\n| ![a](u) |")) // table cell
        XCTAssertTrue(MarkdownGuard.containsImage("\\\\![a](u)")) // after an escaped backslash
        XCTAssertTrue(MarkdownGuard.containsImage("    ![a](u)")) // raw indented line
    }

    func testEscapedOrCodeOpenersAreNotImages() {
        XCTAssertFalse(MarkdownGuard.containsImage("no image at all"))
        XCTAssertFalse(MarkdownGuard.containsImage("! [not an image] and [link](u)"))
        XCTAssertFalse(MarkdownGuard.containsImage("\\![a](u)"))
        XCTAssertFalse(MarkdownGuard.containsImage("```md\n![a](u)\n```\ntext"))
        XCTAssertFalse(MarkdownGuard.containsImage("~~~\n![a](u)\n~~~~\ntext"))
        XCTAssertFalse(MarkdownGuard.containsImage("```\n![a](u)\n![b](v)")) // unclosed top-level fence: code to the end
        XCTAssertFalse(MarkdownGuard.containsImage("Use `![alt](url)` to embed an image."))
        XCTAssertFalse(MarkdownGuard.containsImage("``a ` ![x](u)`` b"))
    }

    func testAnImageOpenerAfterAClosedFenceIsFound() {
        XCTAssertTrue(MarkdownGuard.containsImage("```\ncode\n```\n![a](u)"))
    }

    func testMarkdownWithoutImageIsHandedToTheRendererEscaped() {
        XCTAssertEqual(MarkdownGuard.safeMarkdown("a ![x](https://t.example/p.png)"), "a \\![x](https://t.example/p.png)")
        XCTAssertEqual(MarkdownGuard.safeMarkdown("# a"), "# a")
        // Code keeps its characters and the text is accepted.
        XCTAssertEqual(MarkdownGuard.safeMarkdown("```md\n![a](u)\n```\n![b](v)"), "```md\n![a](u)\n```\n\\![b](v)")
    }

    func testBlankMarkdownNeedsNoScanning() {
        XCTAssertEqual(MarkdownGuard.safeMarkdown("  \n"), "  \n")
        XCTAssertEqual(MarkdownGuard.safeMarkdown(""), "")
    }

    func testHugeMarkdownIsNotRendered() {
        let huge = String(repeating: "a", count: MarkdownGuard.maxChars + 1)
        XCTAssertNil(MarkdownGuard.safeMarkdown(huge))
        let limit = String(repeating: "a", count: MarkdownGuard.maxChars)
        XCTAssertEqual(MarkdownGuard.safeMarkdown(limit), limit)
        XCTAssertEqual(MarkdownGuard.maxChars, 200_000)
    }

    func testTheLengthCountsCharactersNotScalars() {
        // 100 001 characters of 2 scalars each: over the limit by characters would be false, so it must be accepted.
        let text = String(repeating: "e\u{0301}", count: 100_001)
        XCTAssertEqual(text.count, 100_001)
        XCTAssertNotNil(MarkdownGuard.safeMarkdown(text))
    }

    // --- Cases the escaping alone gets wrong: the final scan refuses them (md4c does the same on Android) -------------------

    func testABacktickSwallowedByATagCannotHideAnImage() {
        // `neutralizeImages` sees a code span `">![i](u)<a href="` ; a renderer sees two tags and an image between them.
        XCTAssertNil(MarkdownGuard.safeMarkdown("<a href=\"`\">![i](u)<a href=\"`\">"))
    }

    func testABacktickInALinkDestinationCannotHideAnImage() {
        XCTAssertNil(MarkdownGuard.safeMarkdown("[a](`) ![i](v) `"))
    }

    func testABacktickRunPairedAcrossLinesCannotHideAnImage() {
        // Line 1 pairs with the first backtick of line 2 for a renderer; the "code" of line 2 is then an image.
        XCTAssertNil(MarkdownGuard.safeMarkdown("x `\ny ` ![i](v) `"))
    }

    func testATableCellSplitCannotHideAnImage() {
        XCTAssertNil(MarkdownGuard.safeMarkdown("| `a | ![i](u)` |"))
    }

    func testAFenceInsideAnHtmlBlockIsNotAFence() {
        // The "fence" is HTML text; the blank line ends the HTML block and the image after it is real.
        XCTAssertNil(MarkdownGuard.safeMarkdown("<div>\n```\n\n![i](u)\n```"))
        // After the blank line a fence is a fence again.
        XCTAssertNotNil(MarkdownGuard.safeMarkdown("<div>\n\n```\n![i](u)\n```"))
        // An indented fence may belong to a list item that ends before the image.
        XCTAssertNil(MarkdownGuard.safeMarkdown("- item\n  ```\n![i](u)\n```"))
    }

    func testAnHtmlCommentCannotHideAFenceThatHidesAnImage() {
        XCTAssertNil(MarkdownGuard.safeMarkdown("<!--\n\n```\n-->\n![i](u)\n```"))
    }
}
