import Foundation
import XCTest
@testable import TomiKit

/// Inputs that got an image through the first, scanner-only version of the guard (independent review of PR-4).
/// The renderer's own parse (`render`) is now the final check: none of them may yield an image, whatever the syntax.
final class MarkdownGuardBypassTests: XCTestCase {
    private let bypasses = [
        "`x\r\r![x](http://e/p.png)\r\r`",
        "`x\r\n\r\n![x](http://e/p.png)\r\n\r\n`",
        "http://x.com/`a ![i](http://e/p.png) `",
        "www.x.com/`a ![i](http://e/p.png) `",
        "HTTPS://x.com/`a ![i](http://e/p.png) `",
        "ftp://x.com/`a ![i](http://e/p.png) `",
        "```\n```\u{00A0}\n```\n![x](http://e/p.png)\n```",
        "~~~\n~~~\u{2003}\n~~~\n![x](http://e/p.png)\n~~~",
        "`a\n\u{00A0}\n` ![x](http://e/p.png) `",
        "~~~\n![x](http://e/p.png)\n~~~",
        "  ```\n![x](http://e/p.png)\n  ```\n![y](http://e/q.png)",
        "[ref]: http://e/p.png\n\n![a][ref]",
        "![a]\n\n[a]: http://e/p.png",
        "[![a](http://e/p.png)](http://e)",
    ]

    private func hasImage(_ s: AttributedString) -> Bool {
        s.runs.contains { $0.imageURL != nil }
    }

    func testNoKnownBypassRendersAnImageInAnySyntax() {
        for input in bypasses {
            for syntax: AttributedString.MarkdownParsingOptions.InterpretedSyntax in [.full, .inlineOnly, .inlineOnlyPreservingWhitespace] {
                if let rendered = MarkdownGuard.render(input, syntax: syntax) {
                    XCTAssertFalse(hasImage(rendered), "image rendered for \(input.debugDescription) with \(syntax)")
                }
            }
            if let safe = MarkdownGuard.safeMarkdown(input),
               let parsed = try? AttributedString(markdown: safe) {
                XCTAssertFalse(hasImage(parsed), "image left in safeMarkdown for \(input.debugDescription)")
            }
        }
    }

    func testOrdinaryMarkdownStillRenders() throws {
        let rendered = try XCTUnwrap(MarkdownGuard.render("# Courses\n\n- Lait\n- **Pain**\n\nVoir [la date](https://example.org)."))
        XCTAssertTrue(String(rendered.characters).contains("Pain"))
        XCTAssertTrue(rendered.runs.contains { $0.link != nil })
        XCTAssertNotNil(MarkdownGuard.render("Ligne 1\r\nLigne 2"), "CRLF is fine")
    }

    func testAnEscapedImageBecomesAnInertLink() throws {
        let rendered = try XCTUnwrap(MarkdownGuard.render("Voir ![le schéma](http://e/p.png) ici"))
        XCTAssertFalse(hasImage(rendered))
        XCTAssertTrue(String(rendered.characters).contains("!le schéma"))
    }
}
