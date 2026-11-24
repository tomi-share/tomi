import XCTest
@testable import TomiKit

// Port of apps/android/.../share/SharePayloadParserTest.kt, without the StreamPolicy cases (content:// URIs and
// PackageManager: Android only) plus the ParsedShare redaction case of core/RedactionTest.kt, the limits, and
// ContentDetectors (MarkdownDetector, HtmlText) which Android covers only through this test.
final class SharePayloadParserTests: XCTestCase {
    private func parse(_ input: ShareInput) -> ParsedShare { SharePayloadParser.parse(input) }

    private func file(_ name: String, size: Int64 = 1) -> SharedFile {
        SharedFile(url: URL(fileURLWithPath: "/tmp/share/\(name)"), name: name, mime: "image/jpeg", size: size)
    }

    func testChromeShareWithTitleAndUrlInText() {
        let parsed = parse(ShareInput(text: "Kotlin – Wikipédia https://fr.wikipedia.org/wiki/Kotlin"))
        XCTAssertEqual(parsed.url, "https://fr.wikipedia.org/wiki/Kotlin")
        XCTAssertEqual(parsed.title, "Kotlin – Wikipédia")
        XCTAssertNil(parsed.text)
    }

    func testTitleAndUrlOnSeparateLines() {
        let parsed = parse(ShareInput(text: "Un article\nhttps://example.org/a?b=1&c=2"))
        XCTAssertEqual(parsed.url, "https://example.org/a?b=1&c=2")
        XCTAssertEqual(parsed.title, "Un article")
    }

    func testSubjectBecomesTitleAndBareUrlIsTheLink() {
        let parsed = parse(ShareInput(subject: "Titre de la page", text: "https://example.org/"))
        XCTAssertEqual(parsed.url, "https://example.org/")
        XCTAssertEqual(parsed.title, "Titre de la page")
        XCTAssertNil(parsed.text)
    }

    func testSubjectEqualToLeftoverIsNotDuplicated() {
        let parsed = parse(ShareInput(subject: "Titre", text: "Titre https://example.org/x"))
        XCTAssertEqual(parsed.title, "Titre")
        XCTAssertNil(parsed.text)
        XCTAssertEqual(parsed.url, "https://example.org/x")
    }

    func testSubjectAndDifferentLeftoverKeepsBothAsComposite() {
        let parsed = parse(ShareInput(subject: "Sujet", text: "Voir cette page https://example.org/x"))
        XCTAssertEqual(parsed.title, "Sujet")
        XCTAssertEqual(parsed.text?.plain, "Voir cette page")
        XCTAssertEqual(parsed.url, "https://example.org/x")
    }

    func testTrailingPunctuationIsNotPartOfTheUrl() {
        let parsed = parse(ShareInput(text: "Regarde https://example.org/page."))
        XCTAssertEqual(parsed.url, "https://example.org/page")
    }

    func testBalancedParenthesisStaysInTheUrl() {
        let parsed = parse(ShareInput(text: "https://en.wikipedia.org/wiki/Kotlin_(programming_language)"))
        XCTAssertEqual(parsed.url, "https://en.wikipedia.org/wiki/Kotlin_(programming_language)")
    }

    func testUrlInTheMiddleOfASentenceStaysText() {
        let parsed = parse(ShareInput(text: "Va voir https://example.org/x pour les détails"))
        XCTAssertNil(parsed.url)
        XCTAssertEqual(parsed.text?.plain, "Va voir https://example.org/x pour les détails")
    }

    func testTwoUrlsStayText() {
        let parsed = parse(ShareInput(text: "https://a.example/1 https://b.example/2"))
        XCTAssertNil(parsed.url)
        XCTAssertEqual(parsed.text?.plain, "https://a.example/1 https://b.example/2")
    }

    func testNonHttpSchemesAreNotLinks() {
        XCTAssertNil(parse(ShareInput(text: "javascript:alert(1)")).url)
        XCTAssertNil(parse(ShareInput(text: "file:///data/data/eu.shaffe.tomi/x")).url)
    }

    func testPlainTextWithSubject() {
        let parsed = parse(ShareInput(subject: "Courses", text: "Lait\nPain"))
        XCTAssertEqual(parsed.title, "Courses")
        XCTAssertEqual(parsed.text?.plain, "Lait\nPain")
        XCTAssertNil(parsed.url)
    }

    func testHtmlTextIsKeptNextToPlain() {
        let parsed = parse(ShareInput(text: "Bonjour monde", htmlText: "<p>Bonjour <b>monde</b></p>"))
        XCTAssertEqual(parsed.text?.plain, "Bonjour monde")
        XCTAssertEqual(parsed.text?.html, "<p>Bonjour <b>monde</b></p>")
    }

    func testHtmlOnlyGetsAPlainFallback() {
        let parsed = parse(ShareInput(htmlText: "<h1>Titre</h1><p>Un &amp; deux</p><script>x()</script>"))
        XCTAssertEqual(parsed.text?.plain, "Titre\nUn & deux")
        XCTAssertNotNil(parsed.text?.html)
    }

    func testMarkdownSyntaxIsDetectedAsSourceFormat() {
        let parsed = parse(ShareInput(text: "# Titre\n\n- un\n- deux"))
        XCTAssertEqual(parsed.text?.markdown, "# Titre\n\n- un\n- deux")
    }

    // Same cases as detect.rs (Windows) and SharePayloadParserTest.kt (Android): keep them aligned.
    func testMarkdownCopiedAsLinesOfHtmlStaysMarkdown() {
        let plain = "# Titre\n\nDécisions : a\n\n## Conclusion"
        let html = "<div># Titre</div><div><br></div><div>Décisions : a</div><div><br></div><div>## Conclusion</div>"
        let parsed = parse(ShareInput(text: plain, htmlText: html))
        XCTAssertEqual(parsed.text?.markdown, plain)
        XCTAssertNil(parsed.text?.html)
    }

    func testAWrapperOnlyHtmlFlavourIsNotKept() {
        let parsed = parse(ShareInput(text: "# Titre\n- a", htmlText: "<meta charset='utf-8'><span style=\"color: red\"># Titre</span>"))
        XCTAssertEqual(parsed.text?.markdown, "# Titre\n- a")
        XCTAssertNil(parsed.text?.html)
        let plain = parse(ShareInput(text: "bonjour", htmlText: "<meta charset='utf-8'><span>bonjour</span>"))
        XCTAssertNil(plain.text?.html)
        XCTAssertNil(plain.text?.markdown)
    }

    func testLinesOfHtmlWithoutMarkdownAreKeptAsHtml() {
        let parsed = parse(ShareInput(text: "a\nb\nc", htmlText: "<div>a</div><div>b</div><div>c</div>"))
        XCTAssertEqual(parsed.text?.html, "<div>a</div><div>b</div><div>c</div>")
        XCTAssertNil(parsed.text?.markdown)
    }

    func testFormattingTagsWinOverMarkdownSyntaxInThePlainText() {
        let parsed = parse(ShareInput(text: "# Titre", htmlText: "<h1>Titre</h1>"))
        XCTAssertEqual(parsed.text?.html, "<h1>Titre</h1>")
        XCTAssertNil(parsed.text?.markdown)
    }

    func testPlainSentenceIsNotMarkdown() {
        let parsed = parse(ShareInput(text: "Rendez-vous demain à 14 h."))
        XCTAssertNil(parsed.text?.markdown)
    }

    // Android: streamsFromOtherAppsAreKeptButOwnAndFileUrisAreDropped. The own-authority and file:// filtering is
    // StreamPolicy (Android only); what remains is that shared files are kept, in order.
    func testSharedFilesAreKept() {
        let parsed = parse(ShareInput(files: [file("a.png", size: 10)]))
        XCTAssertEqual(parsed.files.map(\.name), ["a.png"])
    }

    func testAnIntentWithOneTextAndTwoFilesIsASingleCompositeItem() {
        let parsed = parse(ShareInput(subject: "Photos", text: "Voici", files: [file("1.jpg", size: 1), file("2.jpg", size: 2)]))
        XCTAssertEqual(parsed.title, "Photos")
        XCTAssertEqual(parsed.text?.plain, "Voici")
        XCTAssertEqual(parsed.files.count, 2)
    }

    func testEmptyShareIsEmpty() {
        XCTAssertTrue(parse(ShareInput(subject: "", text: "   ")).isEmpty)
    }

    func testControlAndBidiCharactersAreRemoved() {
        let parsed = parse(ShareInput(subject: "A\u{202E}B\u{0007}C", text: "x"))
        XCTAssertEqual(parsed.title, "ABC")
    }

    func testSensitiveClipOrPasswordManagerMarksTheShareAsSecret() {
        XCTAssertTrue(parse(ShareInput(text: "hello world", markedSensitive: true)).looksSecret)
        XCTAssertTrue(parse(ShareInput(text: "hello world", sourcePackage: "com.dashlane")).looksSecret)
        XCTAssertFalse(parse(ShareInput(text: "hello world", sourcePackage: "com.android.chrome")).looksSecret)
    }

    func testAPasswordLikeTokenLooksSecretButALinkNeverDoes() {
        XCTAssertTrue(parse(ShareInput(text: "Xk9#mPq2!vLw")).looksSecret)
        XCTAssertFalse(parse(ShareInput(text: "https://example.org/Xk9mPq2vLw")).looksSecret)
    }

    // --- iOS additions: limits of the share extension (Android StreamPolicy constants) --------------------------------

    func testAtMostTwentyFilesAreKept() {
        let many = (0..<25).map { file("\($0).jpg") }
        let parsed = parse(ShareInput(files: many))
        XCTAssertEqual(parsed.files.map(\.name), (0..<20).map { "\($0).jpg" })
        XCTAssertEqual(ShareLimits.maxFiles, 20)
    }

    func testFileAndItemSizeLimits() {
        XCTAssertEqual(ShareLimits.maxFileBytes, 25 * 1024 * 1024)
        XCTAssertEqual(ShareLimits.maxItemBytes, 100 * 1024 * 1024)
        XCTAssertFalse(ShareLimits.exceeds([file("a", size: ShareLimits.maxFileBytes)]))
        XCTAssertTrue(ShareLimits.exceeds([file("a", size: ShareLimits.maxFileBytes + 1)]))
        let fiveBig = (0..<5).map { file("\($0)", size: ShareLimits.maxFileBytes) }
        XCTAssertTrue(ShareLimits.exceeds(fiveBig), "5 x 25 MB is over 100 MB")
        XCTAssertFalse(ShareLimits.exceeds(Array(fiveBig.prefix(4))))
    }

    func testTextAndHtmlAreCapped() {
        let parsed = parse(ShareInput(text: String(repeating: "a ", count: 60_000), htmlText: "<b>" + String(repeating: "b", count: 600_000)))
        XCTAssertEqual(parsed.text?.html?.count, SharePayloadParser.maxHTML)
        let textOnly = parse(ShareInput(text: String(repeating: "a ", count: 60_000)))
        XCTAssertLessThanOrEqual(textOnly.text?.plain.count ?? 0, SharePayloadParser.maxText)
    }

    // --- RedactionTest.kt: draftsAndSharedContentPrintNoContent (ParsedShare part) -----------------------------------------

    func testParsedShareAndShareInputPrintNoContent() {
        let clear = "Hx7!pQ2-zR9-s3cr3t"
        let device = "PC-Bureau"
        let shared = SharedFile(url: URL(fileURLWithPath: "/p/\(clear)"), name: clear, mime: "text/plain", size: 1)
        let parsed = ParsedShare(title: clear, text: RichText(plain: clear), url: clear, files: [shared], looksSecret: true)
        let input = ShareInput(subject: clear, text: clear, htmlText: clear, files: [shared])
        for printed in [String(describing: parsed), String(reflecting: parsed), String(describing: input), String(describing: shared)] {
            XCTAssertFalse(printed.contains(clear), printed)
            XCTAssertFalse(printed.contains(device), printed)
            XCTAssertTrue(printed.contains("redacted"), printed)
        }
        var dumped = ""
        dump(parsed, to: &dumped)
        XCTAssertFalse(dumped.contains(clear))
    }

    // --- ContentDetectors (MarkdownDetector / HtmlText) ------------------------------------------------------------------

    func testMarkdownDetectorPatterns() {
        for md in ["# t", "text\n## t", "- a", "  * a", "1. a", "```swift", "[x](https://a.b)", "**gras**", "> quote"] {
            XCTAssertTrue(MarkdownDetector.looksLikeMarkdown(md), md)
        }
        for plain in ["a - b", "1.5 litre", "#hashtag", "**", "[x] (y)", "Bonjour"] {
            XCTAssertFalse(MarkdownDetector.looksLikeMarkdown(plain), plain)
        }
    }

    func testHtmlToPlain() {
        XCTAssertEqual(HtmlText.toPlain("<p>a</p><p>b</p>"), "a\nb")
        XCTAssertEqual(HtmlText.toPlain("a<br/>b<BR>c"), "a\nb\nc")
        XCTAssertEqual(HtmlText.toPlain("<style>p{x:y}</style>&lt;ok&gt;&nbsp;&quot;q&quot; &#39;s&#39;"), "<ok> \"q\" 's'")
        XCTAssertEqual(HtmlText.toPlain("<div>a</div><div></div><div></div><div></div><div>b</div>"), "a\n\nb")
        XCTAssertEqual(HtmlText.toPlain("&amp;lt;"), "&lt;")
    }
}
