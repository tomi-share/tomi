import XCTest
@testable import TomiKit

// Port of the HTML sanitiser cases of apps/android/app/src/test/java/eu/shaffe/tomi/ui/ItemLogicTest.kt and of the wide HTML
// test of core/ArchiveTest.kt, plus cases for each removed category and for attribute tricks (there is no Jsoup behind
// this sanitiser, so it is tested harder than the Kotlin one).
//
// Replaced: the Jsoup parse of the generated document (`Jsoup.parse(doc)`, select meta/body) by string checks on the
// document, since no HTML parser is available in Foundation.
final class HtmlSanitizerTests: XCTestCase {
    private let png = "data:image/png;base64,AAAA"

    private func clean(_ html: String) -> String { HtmlSanitizer.clean(html) }

    private func assertAbsent(_ output: String, _ needles: [String], file: StaticString = #filePath, line: UInt = #line) {
        for needle in needles {
            XCTAssertFalse(output.lowercased().contains(needle.lowercased()), "\(needle) in \(output)", file: file, line: line)
        }
    }

    // --- ported ---------------------------------------------------------------------------------------------------------

    func testSanitiserRemovesScriptsHandlersFramesAndRemoteImages() {
        let dirty = "<p onclick=\"x()\">hi</p><script>alert(1)</script><iframe src=\"https://e.example\"></iframe>"
            + "<img src=\"https://tracker.example/p.png\"><img src=\"\(png)\">"
            + "<a href=\"javascript:alert(1)\">bad</a><form action=\"https://e.example\"><input></form>"
        let out = clean(dirty)
        assertAbsent(out, ["script", "onclick", "iframe", "tracker.example", "javascript:", "<form", "<input", "alert"])
        XCTAssertTrue(out.contains("hi"))
        XCTAssertTrue(out.contains("data:image/png"))
        XCTAssertTrue(out.contains("bad")) // the text of a refused link stays
    }

    func testSanitiserDropsInlineStylesOnEveryTag() {
        let dirty = "<p style=\"position:fixed;top:0;left:0;width:100%;height:100%\">overlay</p>"
            + "<span style=\"display:none\">hidden</span><img src=\"\(png)\" style=\"width:1px\">"
            + "<table style=\"x\"><tr><td style=\"y\">cell</td></tr></table><a href=\"https://example.org/\" style=\"color:red\">link</a>"
        let out = clean(dirty)
        assertAbsent(out, ["style", "position"])
        // The content itself is kept.
        XCTAssertTrue(out.contains("overlay"))
        XCTAssertTrue(out.contains("cell"))
        XCTAssertTrue(out.contains("href=\"https://example.org/\""))
    }

    func testGeneratedDocumentCarriesTheCspAndOnlyCleanedContent() {
        let doc = HtmlSanitizer.toDocument(
            "<p style=\"color:red\" onclick=\"x()\">kept</p><script>alert(1)</script><img src=\"https://tracker.example/p.png\">",
            textColorCss: "#000000", linkColorCss: "#0000FF"
        )
        // The literal policy is written out here on purpose: the test must fail if the constant is weakened.
        let cspMeta = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\">"
        XCTAssertEqual(HtmlSanitizer.csp, "default-src 'none'; img-src data:; style-src 'unsafe-inline'")
        XCTAssertEqual(doc.components(separatedBy: "Content-Security-Policy").count - 1, 1)
        XCTAssertTrue(doc.contains(cspMeta))
        // The policy lives in <head>, before any received content.
        let head = doc.range(of: "<head>")!, headEnd = doc.range(of: "</head>")!, body = doc.range(of: "<body>")!
        XCTAssertTrue(head.upperBound <= doc.range(of: cspMeta)!.lowerBound)
        XCTAssertTrue(doc.range(of: cspMeta)!.upperBound <= headEnd.lowerBound)
        XCTAssertTrue(headEnd.upperBound <= body.lowerBound)
        // The body only holds the cleaned fragment.
        XCTAssertTrue(doc.contains("<body><p>kept</p></body>"), doc)
        XCTAssertFalse(doc.contains("tracker.example"))
        XCTAssertFalse(doc.contains("alert"))
        XCTAssertTrue(doc.contains("color: #000000"))
        XCTAssertTrue(doc.contains("a { color: #0000FF; }"))
        XCTAssertTrue(doc.hasPrefix("<!DOCTYPE html>"))
        XCTAssertTrue(doc.contains("<meta charset=\"utf-8\">"))
        XCTAssertTrue(doc.contains("img { max-width: 100%; height: auto; }"))
        XCTAssertTrue(doc.contains("pre, code { white-space: pre-wrap; }"))
        XCTAssertTrue(doc.contains("td, th { border: 1px solid; padding: 2px 6px; }"))
    }

    func testTheCardHtmlDocumentCannotScrollSidewaysButTheDetailOneCan() {
        let table = "<table><tr><td>x</td></tr></table>"
        let card = HtmlSanitizer.toDocument(table, textColorCss: "#000000", linkColorCss: "#0000FF", horizontalScroll: false)
        let detail = HtmlSanitizer.toDocument(table, textColorCss: "#000000", linkColorCss: "#0000FF")
        XCTAssertTrue(card.contains("overflow-x: hidden"))
        XCTAssertFalse(detail.contains("overflow-x"))
    }

    func testThemeColoursCannotOpenANewCssRuleOrCloseTheStyleBlock() {
        let doc = HtmlSanitizer.toDocument(
            "x", textColorCss: "red; } </style><script>alert(1)</script>", linkColorCss: "blue}body{display:none"
        )
        assertAbsent(doc, ["alert", "display:none"])
        XCTAssertEqual(doc.components(separatedBy: "</style>").count - 1, 1)
        XCTAssertTrue(HtmlSanitizer.toDocument("x", textColorCss: "rgb(1, 2, 3)", linkColorCss: "#00F").contains("color: rgb(1, 2, 3);"))
    }

    // --- structure kept ---------------------------------------------------------------------------------------------------

    func testAllowedFormattingTagsAndAttributesAreKept() {
        let html = "<h1>T</h1><p><b>b</b><i>i</i><em>e</em><strong>s</strong><u>u</u><code>c</code><br>x</p>"
            + "<ul type=\"disc\"><li>a</li></ul><ol start=\"3\"><li>b</li></ol><blockquote>q</blockquote><pre>p</pre>"
            + "<table width=\"100\"><caption>c</caption><thead><tr><th scope=\"col\" colspan=\"2\">h</th></tr></thead>"
            + "<tbody><tr><td rowspan=\"2\">d</td></tr></tbody></table>"
            + "<a href=\"mailto:a@b.example\" title=\"t\">m</a><a href=\"tel:+33123456789\">t</a><a href=\"HTTPS://Example.org/x?a=1&amp;b=2\">h</a>"
            + "<img src=\"\(png)\" alt=\"pic\" width=\"10\" height=\"10\">"
        let out = clean(html)
        XCTAssertEqual(out, html.replacingOccurrences(of: "HTTPS://Example.org/x?a=1&amp;b=2", with: "HTTPS://Example.org/x?a=1&amp;b=2"))
    }

    func testUnknownTagsAreUnwrappedAndTheirTextKept() {
        XCTAssertEqual(clean("<font color=red><center>hello</center></font>"), "hello")
        XCTAssertEqual(clean("<custom-tag>a</custom-tag>"), "a")
    }

    func testStrayLessThanAndGreaterThanAreEscaped() {
        XCTAssertEqual(clean("1 < 2 and 3 > 2"), "1 &lt; 2 and 3 &gt; 2")
        XCTAssertEqual(clean("a <<b>x</b>"), "a &lt;<b>x</b>")
        XCTAssertEqual(clean("end <"), "end &lt;")
        XCTAssertEqual(clean("a & b &amp; c &#169; &copy; &nope"), "a &amp; b &amp; c &#169; &copy; &amp;nope")
    }

    func testTagsAreRebalancedAndNeverEscapeTheirDocument() {
        XCTAssertEqual(clean("<div><p>a</div>b"), "<div><p>a</p></div>b")
        XCTAssertEqual(clean("</div></body></html>text"), "text")
        XCTAssertEqual(clean("<b><i>x"), "<b><i>x</i></b>")
        let deep = String(repeating: "<div>", count: 1_000) + "x"
        XCTAssertLessThanOrEqual(clean(deep).components(separatedBy: "<div>").count - 1, 200)
    }

    // --- removed with their content -------------------------------------------------------------------------------------------

    func testScriptStyleFramesObjectsTemplatesAndForeignContentGoWithTheirContent() {
        let cases: [(String, [String])] = [
            ("a<script>alert(1)</script>b", ["alert", "script"]),
            ("a<style>body{display:none}</style>b", ["display", "style"]),
            ("a<iframe src=x>inner secret</iframe>b", ["inner", "iframe"]),
            ("a<object data=x><b>inner</b></object>b", ["inner", "object"]),
            ("a<embed src=x>b", ["embed"]),
            ("a<template><img src=x>inner</template>b", ["inner", "template"]),
            ("a<noscript><p>inner</p></noscript>b", ["inner", "noscript"]),
            ("a<svg onload=alert(1)><circle/><script>alert(2)</script></svg>b", ["alert", "svg", "circle"]),
            ("a<math><mi>inner</mi></math>b", ["inner", "math"]),
            ("a<applet code=x>inner</applet>b", ["inner", "applet"]),
            ("a<textarea><b>inner</b></textarea>b", ["inner", "textarea"]),
            ("a<title>inner</title>b", ["inner", "title"]),
            ("a<xmp><b>inner</b></xmp>b", ["inner", "xmp"]),
        ]
        for (html, gone) in cases {
            let out = clean(html)
            XCTAssertEqual(out, html.hasPrefix("a<embed") ? "ab" : "ab", html)
            assertAbsent(out, gone)
        }
    }

    func testNestedRemovedElementsAreMatchedAndAnUnclosedOneRemovesTheRest() {
        XCTAssertEqual(clean("a<svg><svg>x</svg>y</svg>b"), "ab")
        XCTAssertEqual(clean("a<template><template>x</template>y</template>b"), "ab")
        XCTAssertEqual(clean("a<svg><g>never closed <img src=\"\(png)\">"), "a")
        XCTAssertEqual(clean("a<script>never closed <b>x</b>"), "a")
        XCTAssertEqual(clean("a<style>never closed"), "a")
        // A longer name is another element.
        XCTAssertEqual(clean("a<scripts>b</scripts>c"), "abc")
        XCTAssertEqual(clean("a<script>x</scripts>y</script>b"), "ab")
    }

    func testRawTextElementsEndAtTheFirstEndTagWhateverTheCase() {
        XCTAssertEqual(clean("<SCRIPT>alert('</b>')</ScRiPt >ok"), "ok")
        XCTAssertEqual(clean("<script>/*</script>*/alert(1)//"), "*/alert(1)//") // the browser would end at the first one too
        XCTAssertEqual(clean("<style>a</style\n>ok"), "ok")
    }

    func testCommentsAndProcessingInstructionsAreDropped() {
        XCTAssertEqual(clean("a<!-- secret <script>alert(1)</script> -->b"), "ab")
        XCTAssertEqual(clean("a<!---->b<!-->c<!--->d"), "abcd")
        XCTAssertEqual(clean("a<!-- unclosed <b>x</b>"), "a")
        XCTAssertEqual(clean("a<!-- x --!>b"), "ab")
        XCTAssertEqual(clean("a<?xml version=\"1.0\"?>b"), "ab")
        XCTAssertEqual(clean("<!DOCTYPE html>a<![CDATA[ <b>x</b> ]]>b"), "ax ]]&gt;b")
        XCTAssertEqual(clean("a</ bogus>b</>c"), "abc")
        // Conditional comments of Office documents.
        XCTAssertEqual(clean("a<!--[if gte mso 9]><xml><o:x/></xml><![endif]-->b"), "ab")
    }

    // --- tag and attribute tricks ------------------------------------------------------------------------------------------------

    func testUppercaseAndMixedCaseNamesAreFoldedBeforeTheCheck() {
        let out = clean("<P ONCLICK=\"x()\" STYLE=\"color:red\"><A HREF=\"JavaScript:alert(1)\" OnMouseOver=\"x()\">a</A><IMG SRC=\"HTTP://t.example/p.png\" onerror=\"x()\"></P><ScRiPt>alert(1)</sCrIpT>")
        XCTAssertEqual(out, "<p><a>a</a></p>")
    }

    func testUnquotedSingleQuotedAndWeirdlySpacedValuesAreRead() {
        XCTAssertEqual(clean("<a href=https://example.org/a title=x>l</a>"), "<a href=\"https://example.org/a\" title=\"x\">l</a>")
        XCTAssertEqual(clean("<a href='https://example.org/a'>l</a>"), "<a href=\"https://example.org/a\">l</a>")
        XCTAssertEqual(clean("<a\nhref\n=\n\"https://example.org/a\"\nonclick\n=\n\"x()\">l</a>"), "<a href=\"https://example.org/a\">l</a>")
        XCTAssertEqual(clean("<a href=javascript:alert(1)>l</a>"), "<a>l</a>")
        XCTAssertEqual(clean("<p/onclick=alert(1)>x</p>"), "<p>x</p>")
        XCTAssertEqual(clean("<p onclick=alert(1)//>x</p>"), "<p>x</p>")
        XCTAssertEqual(clean("<img/src=\"\(png)\"/onerror=alert(1)>"), "<img src=\"\(png)\">")
    }

    func testAnAttributeValueCannotCloseItsQuoteOrOpenATag() {
        let out = clean("<a href=\"https://example.org/\" title='\"><script>alert(1)</script>'>l</a><img src=\"\(png)\" alt=\"&quot;><b onclick=x>\">")
        assertAbsent(out, ["<script", "<b onclick"])
        XCTAssertTrue(out.contains("title=\"&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;\""), out)
        XCTAssertTrue(out.contains("alt=\"&quot;&gt;&lt;b onclick=x&gt;\""), out)
    }

    func testTheFirstOfTwoIdenticalAttributesWinsLikeInABrowser() {
        XCTAssertEqual(clean("<a href=\"https://good.example/\" href=\"javascript:alert(1)\">l</a>"), "<a href=\"https://good.example/\">l</a>")
        XCTAssertEqual(clean("<a href=\"javascript:alert(1)\" href=\"https://good.example/\">l</a>"), "<a>l</a>")
    }

    func testJavascriptSchemesAreRefusedWhateverTheEncodingOrSpacing() {
        let attacks = [
            "javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            " javascript:alert(1)",
            "\u{01}javascript:alert(1)",
            "java\tscript:alert(1)",
            "java\nscript:alert(1)",
            "java\rscript:alert(1)",
            "java script:alert(1)",
            "&#106;avascript:alert(1)",
            "&#x6A;avascript:alert(1)",
            "&#106avascript:alert(1)",
            "jav&#x61;script&colon;alert(1)",
            "javascript&#58;alert(1)",
            "&#0000106&#0000097vascript:alert(1)",
            "java&Tab;script:alert(1)",
            "java&NewLine;script:alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html,<script>alert(1)</script>",
            "data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
            "file:///etc/passwd",
            "ftp://example.org/a",
            "//example.org/a",
            "/relative",
            "relative.html",
            "#anchor",
            "http",
            "\u{2028}javascript:alert(1)",
            "javascript\u{200B}:alert(1)",
        ]
        for attack in attacks {
            let out = clean("<a href=\"\(attack.replacingOccurrences(of: "\"", with: "&quot;"))\">x</a>")
            XCTAssertFalse(out.contains("href"), "\(attack.debugDescription) gave \(out)")
            XCTAssertFalse(out.lowercased().contains("script:"), "\(attack.debugDescription) gave \(out)")
        }
    }

    func testOnlyHttpHttpsMailtoAndTelLinksSurviveAndAreNormalised() {
        XCTAssertEqual(clean("<a href=\" \thttps://example.org/a\n\">x</a>"), "<a href=\"https://example.org/a\">x</a>")
        XCTAssertEqual(clean("<a href=\"htt&#112;s://example.org/\">x</a>"), "<a href=\"https://example.org/\">x</a>")
        XCTAssertEqual(clean("<a href=\"http://example.org/?a=1&amp;b=2&c=3\">x</a>"), "<a href=\"http://example.org/?a=1&amp;b=2&amp;c=3\">x</a>")
        XCTAssertEqual(clean("<a href=\"mailto:a@b.example?subject=x\">x</a>"), "<a href=\"mailto:a@b.example?subject=x\">x</a>")
        XCTAssertEqual(clean("<a href=\"tel:+33123456789\">x</a>"), "<a href=\"tel:+33123456789\">x</a>")
    }

    func testOnlyBase64RasterDataImagesSurviveAsImageSources() {
        let refused = [
            "https://tracker.example/p.png", "http://tracker.example/p.png", "//tracker.example/p.png", "p.png",
            "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=", "data:image/svg+xml,<svg onload=alert(1)>",
            "data:text/html;base64,AAAA", "data:image/png,<script>", "data:image/png;base64,AA\"onerror=x",
            "data:image/png;base64,AAAA<", "javascript:alert(1)", "",
            "&#100;ata:text/html,x",
        ]
        for source in refused {
            let out = clean("<p>a</p><img src=\"\(source.replacingOccurrences(of: "\"", with: "&quot;"))\" alt=\"x\">")
            XCTAssertEqual(out, "<p>a</p>", "\(source.debugDescription) gave \(out)")
        }
        XCTAssertEqual(clean("<img src=\"DATA:IMAGE/PNG;BASE64,AAAA\">"), "<img src=\"DATA:IMAGE/PNG;BASE64,AAAA\">")
        for type in ["png", "jpeg", "jpg", "gif", "webp", "bmp"] {
            let src = "data:image/\(type);base64,QUJD"
            XCTAssertEqual(clean("<img src=\"\(src)\">"), "<img src=\"\(src)\">")
        }
        XCTAssertEqual(clean("<img src=\"\(png)\">"), "<img src=\"\(png)\">")
        XCTAssertEqual(clean("<img src=\"data:image/png;base64,AA\nAA\">"), "<img src=\"data:image/png;base64,AAAA\">")
        // srcset, lowsrc, background and the like are not in the allow-list.
        XCTAssertEqual(clean("<img src=\"\(png)\" srcset=\"https://t.example/p.png 2x\" lowsrc=\"https://t.example/q.png\">"), "<img src=\"\(png)\">")
        XCTAssertEqual(clean("<table background=\"https://t.example/p.png\"><tr><td background=\"https://t.example/q.png\">x</td></tr></table>"),
                       "<table><tr><td>x</td></tr></table>")
    }

    func testEventHandlersAndDangerousAttributesNeverSurviveOnAnyTag() {
        let attrs = ["onclick", "onerror", "onload", "onmouseover", "onfocus", "onanimationstart", "style", "class", "id",
                     "formaction", "action", "srcdoc", "xlink:href", "data", "ping", "target", "rel", "background", "poster", "cite"]
        for attr in attrs {
            for tag in ["p", "a", "img", "div", "span", "td", "table", "blockquote"] {
                let src = tag == "img" ? " src=\"\(png)\"" : ""
                let out = clean("<\(tag)\(src) \(attr)=\"https://t.example/x\">x</\(tag)>")
                XCTAssertFalse(out.contains(attr + "="), "\(attr) kept on \(tag): \(out)")
                XCTAssertFalse(out.contains("t.example"), "\(attr) value kept on \(tag): \(out)")
            }
        }
    }

    func testMarkupBrokenByNulBytesOrControlCharactersCannotSneakThrough() {
        XCTAssertEqual(clean("<scr\u{0000}ipt>alert(1)</scr\u{0000}ipt>ok"), "ok")
        XCTAssertEqual(clean("<img src=\"\(png)\" on\u{0000}error=\"x()\">"), "<img src=\"\(png)\">")
        XCTAssertEqual(clean("<a href=\"https://example.org/\u{0001}x\">l</a>"), "<a>l</a>")
        XCTAssertEqual(clean("<p title=\"a\u{0001}b\u{009F}c\">x</p>"), "<p>x</p>") // title is not allowed on p
        XCTAssertEqual(clean("<a title=\"a\u{0001}b\u{009F}c\">x</a>"), "<a title=\"abc\">x</a>")
    }

    func testAnUnterminatedTagOrQuoteDropsTheRestInsteadOfLeakingIt() {
        XCTAssertEqual(clean("ok<img src=\"\(png)\" alt=\"never closed"), "ok")
        XCTAssertEqual(clean("ok<a href=\"https://example.org/\""), "ok")
        XCTAssertEqual(clean("ok<b"), "ok")
        XCTAssertEqual(clean("ok<"), "ok&lt;")
    }

    func testCombiningMarksCannotDisguiseMarkup() {
        // "<" followed by a combining mark is one Swift Character; the scan works on scalars.
        XCTAssertEqual(clean("<\u{0301}b>x"), "&lt;\u{0301}b&gt;x")
        // Not the script element for a browser either: an unknown tag, dropped, its text inert.
        XCTAssertEqual(clean("<script\u{0301}>x</script>"), "x")
    }

    func testMutationVectorsAreFlattenedToInertOutput() {
        let vectors = [
            "<noscript><p title=\"</noscript><img src=x onerror=alert(1)>\">",
            "<svg></p><style><a id=\"</style><img src=1 onerror=alert(1)>\">",
            "<math><mtext><table><mglyph><style><!--</style><img title=\"--&gt;&lt;/mglyph&gt;&lt;img&Tab;src=1&Tab;onerror=alert(1)&gt;\">",
            "<form><math><mtext></form><form><mglyph><style></math><img src onerror=alert(1)>",
            "<xmp><b title=\"</xmp><img src=x onerror=alert(1)>\">",
            "<textarea></textarea><img src=x onerror=alert(1)>",
            "<a href=\"x\" title=\"</a><img src=x onerror=alert(1)>\">y</a>",
            "<listing>&lt;img src=x onerror=alert(1)&gt;</listing>",
            "<style><style/><img src=x onerror=alert(1)>",
        ]
        for vector in vectors {
            let out = clean(vector)
            assertAbsent(out, ["<img", "<svg", "<math", "<style", "<script", "<form", "<noscript"])
            // Whatever is left is only allowed tags: the only `<` are those of the allow-list.
            let tags = out.components(separatedBy: "<").dropFirst().map { chunk in
                chunk.prefix { $0.isLetter || $0.isNumber || $0 == "/" }.lowercased()
            }
            for tag in tags {
                XCTAssertTrue(["a", "/a", "b", "/b", "p", "/p", "i", "/i", "table", "/table", "tr", "/tr", "td", "/td", "div", "/div", "span", "/span"].contains(tag), "\(tag) in \(out) for \(vector)")
            }
        }
    }

    func testTheOutputIsAFixedPointOfTheSanitiser() {
        let inputs = [
            "<p onclick=x>hi &amp; <b>bye</p><script>x</script>",
            "<a href=\"HTTP://Example.org/?a=1&b=2\">x</a>1 < 2",
            "<table><tr><td>a<td>b</table>",
            "<img src=\"\(png)\" alt=\"a&quot;b\">",
        ]
        for input in inputs {
            let once = clean(input)
            XCTAssertEqual(clean(once), once, input)
        }
    }

    func testHugeInputsAreHandledInLinearTime() {
        let big = String(repeating: "<p>a &amp; <b>b</b> < c</p>", count: 20_000)
        let start = Date()
        let out = clean(big)
        XCTAssertGreaterThan(out.count, 20_000)
        XCTAssertLessThan(Date().timeIntervalSince(start), 5)
        // Many unclosed script tags: the first one removes the rest, once.
        let scripts = String(repeating: "<script>", count: 50_000)
        let t2 = Date()
        XCTAssertEqual(clean(scripts), "")
        XCTAssertLessThan(Date().timeIntervalSince(t2), 5)
    }
}
