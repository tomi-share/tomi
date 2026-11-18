import XCTest
@testable import TomiKit

// Port of the cases of apps/android/app/src/test/java/eu/shaffe/tomi/ui/ItemLogicTest.kt about render modes, copy
// content, feed filters and search. The HTML sanitiser cases are in HtmlSanitizerTests.swift; the kind, notification,
// delivery status, authentication and clipboard cases belong to other batches.
final class ItemLogicRenderTests: XCTestCase {
    private let office = DeviceRef(id: "pc", name: "PC-Bureau")
    private let now = ISO8601DateFormatter().date(from: "2026-11-04T10:00:00Z")!

    private func item(
        id: String = "1", title: String? = nil, text: RichText? = nil, url: String? = nil,
        attachments: [Attachment] = [], secret: Bool = false
    ) -> TomiItem {
        TomiItem.readable(
            id: id, sourceDevice: office, createdAt: now, title: title, text: text, url: url, attachments: attachments,
            secret: secret
        )
    }

    // --- rendering mode ---------------------------------------------------------------------------------------------

    func testThereAreOnlyTwoRenderModesAndNoneShowsHtmlMarkup() {
        XCTAssertEqual(RenderMode.allCases, [.preview, .source])
        XCTAssertEqual(RenderMode.preview.labelKey, "render_preview")
        XCTAssertEqual(RenderMode.source.labelKey, "render_markdown")
    }

    func testTheMarkdownModeIsOnlyOfferedWhenTheItemHasMarkdown() {
        XCTAssertEqual(RenderModes.available(RichText(plain: "a", markdown: "**a**")), [.preview, .source])
        // HTML only, or plain only: the preview is the one and only mode (no segmented control).
        XCTAssertEqual(RenderModes.available(RichText(plain: "a", html: "<b>a</b>")), [.preview])
        XCTAssertEqual(RenderModes.available(RichText(plain: "a")), [.preview])
        // Markdown and HTML together: still two modes, never a third one showing the HTML.
        XCTAssertEqual(RenderModes.available(RichText(plain: "a", html: "<b>a</b>", markdown: "**a**")), [.preview, .source])
    }

    func testRenderModeDefaultsToThePreviewThenToTheLastChoice() {
        let md = RichText(plain: "a", markdown: "**a**")
        let html = RichText(plain: "a", html: "<b>a</b>")
        let plain = RichText(plain: "a")
        XCTAssertEqual(RenderModes.resolve(md, preferred: nil), .preview)
        XCTAssertEqual(RenderModes.resolve(html, preferred: nil), .preview)
        XCTAssertEqual(RenderModes.resolve(plain, preferred: nil), .preview)
        // The remembered choice applies when the item supports it.
        XCTAssertEqual(RenderModes.resolve(md, preferred: .source), .source)
        // An item without Markdown ignores a remembered "Markdown" mode.
        XCTAssertEqual(RenderModes.resolve(html, preferred: .source), .preview)
        XCTAssertEqual(RenderModes.resolve(plain, preferred: .source), .preview)
    }

    func testCopyPutsWhatIsDisplayed() {
        var both = RichText(plain: "plain", html: "<b>plain</b>", markdown: "# plain\n\n**x**")
        // Markdown mode shows the literal source (with its syntax), not the plain text.
        XCTAssertEqual(RenderModes.copyContent(both, mode: .source).plain, "# plain\n\n**x**")
        XCTAssertNil(RenderModes.copyContent(both, mode: .source).html)
        // Captured as Markdown: the preview is the rendered Markdown, its copy is the plain text.
        let markdownPreview = RenderModes.copyContent(both, mode: .preview)
        XCTAssertEqual(markdownPreview.plain, "plain")
        XCTAssertNil(markdownPreview.html)
        // Captured as HTML: the preview is the sandboxed HTML, its copy carries the HTML with the plain text.
        both.sourceFormat = .html
        let htmlPreview = RenderModes.copyContent(both, mode: .preview)
        XCTAssertEqual(htmlPreview.plain, "plain")
        XCTAssertEqual(htmlPreview.html, "<b>plain</b>")
        XCTAssertEqual(RenderModes.copyContent(RichText(plain: "plain"), mode: .preview).plain, "plain")
        // Source without Markdown (a remembered mode applied by mistake) falls back to the plain text.
        XCTAssertEqual(RenderModes.copyContent(RichText(plain: "plain"), mode: .source).plain, "plain")
    }

    func testCopyContentNeverPrintsTheText() {
        let copy = CopyContent(plain: "hunter2", html: "<b>hunter2</b>")
        XCTAssertFalse("\(copy)".contains("hunter2"))
        XCTAssertFalse(String(reflecting: copy).contains("hunter2"))
        XCTAssertFalse(String(describing: Mirror(reflecting: copy).children.map { "\($0.value)" }).contains("hunter2"))
    }

    func testThePreviewFollowsTheFormatTheContentWasCapturedIn() {
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a", markdown: "**a**")), .markdown)
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a", html: "<b>a</b>")), .html)
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a")), .plain)
        var both = RichText(plain: "a", html: "<b>a</b>", markdown: "**a**")
        both.sourceFormat = .markdown
        XCTAssertEqual(RenderModes.previewKind(both), .markdown)
        both.sourceFormat = .html
        XCTAssertEqual(RenderModes.previewKind(both), .html)
        both.sourceFormat = .plain
        XCTAssertEqual(RenderModes.previewKind(both), .plain)
        // A declared format whose representation is missing falls back to the other one, then to the plain text.
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a", html: "<b>a</b>", sourceFormat: .markdown)), .html)
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a", sourceFormat: .html)), .plain)
        XCTAssertEqual(RenderModes.previewKind(RichText(plain: "a", markdown: "**a**", sourceFormat: .html)), .markdown)
        // Windows omits `markdown` when it equals the plain text: source_format alone says the plain text is Markdown.
        let declared = RichText(plain: "**a**", sourceFormat: .markdown)
        XCTAssertEqual(RenderModes.previewKind(declared), .markdown)
        XCTAssertEqual(RenderModes.available(declared), [.preview, .source])
        XCTAssertEqual(declared.markdownSource, "**a**")
        XCTAssertEqual(RenderModes.copyContent(declared, mode: .source).plain, "**a**")
        XCTAssertNil(RichText(plain: "a").markdownSource)
    }

    // --- feed filters and search --------------------------------------------------------------------------------------

    func testFiltersMatchTheCompositeFields() {
        let link = item(id: "l", title: "Doc", url: "https://a.example")
        let text = item(id: "t", text: RichText(plain: "hello"))
        let file = item(id: "f", attachments: [Attachment(name: "a.pdf", mime: "application/pdf", size: 1)])
        let secret = item(id: "s", text: RichText(plain: "pw"), secret: true)
        let all = [link, text, file, secret]
        XCTAssertEqual(FeedFiltering.apply(all, .links, "").map(\.id), ["l"])
        XCTAssertEqual(FeedFiltering.apply(all, .text, "").map(\.id), ["t"])
        XCTAssertEqual(FeedFiltering.apply(all, .files, "").map(\.id), ["f"])
        XCTAssertEqual(FeedFiltering.apply(all, .secrets, "").map(\.id), ["s"])
        XCTAssertEqual(FeedFiltering.apply(all, .all, "").count, 4)
    }

    func testSearchNeverLooksInsideHiddenItemsNorAtTheirTitle() {
        let secret = item(id: "s", title: "Wi-Fi invités", text: RichText(plain: "Hx7!pQ2-zR9"), url: "https://wifi.example", secret: true)
        let text = item(id: "t", title: "Wi-Fi maison", text: RichText(plain: "rendez-vous Hx7"))
        // The title of a hidden item is excluded from the search, like its content.
        XCTAssertFalse(FeedFiltering.matchesQuery(secret, "wi-fi"))
        XCTAssertFalse(FeedFiltering.matchesQuery(secret, "invités"))
        XCTAssertFalse(FeedFiltering.matchesQuery(secret, "Hx7"))
        XCTAssertFalse(FeedFiltering.matchesQuery(secret, "wifi.example"))
        // It can still be found by the device it came from, and a normal title is searched.
        XCTAssertTrue(FeedFiltering.matchesQuery(secret, "bureau"))
        XCTAssertTrue(FeedFiltering.matchesQuery(text, "wi-fi"))
        // Whole-feed check: only the normal item answers to the title.
        XCTAssertEqual(FeedFiltering.apply([secret, text], .all, "wi-fi").map(\.id), ["t"])
        XCTAssertTrue(FeedFiltering.matchesQuery(text, "hx7"))
        XCTAssertTrue(FeedFiltering.matchesQuery(text, "bureau"))
    }

    func testSearchLooksAtLinksAndFileNamesOfNormalItemsAndIgnoresBlankQueries() {
        let file = item(id: "f", attachments: [Attachment(name: "Facture-Mars.pdf", mime: "application/pdf", size: 1)])
        let link = item(id: "l", url: "https://example.org/Page")
        XCTAssertTrue(FeedFiltering.matchesQuery(file, "facture"))
        XCTAssertTrue(FeedFiltering.matchesQuery(link, "EXAMPLE.org"))
        XCTAssertFalse(FeedFiltering.matchesQuery(link, "facture"))
        XCTAssertTrue(FeedFiltering.matchesQuery(link, ""))
        XCTAssertTrue(FeedFiltering.matchesQuery(link, "  \n"))
        // The query is trimmed.
        XCTAssertTrue(FeedFiltering.matchesQuery(link, "  page "))
    }
}
