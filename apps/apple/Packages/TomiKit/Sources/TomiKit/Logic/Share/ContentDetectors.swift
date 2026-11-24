import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/ContentDetectors.kt.
// Not ported: OwnAuthorityCheck, PackageManagerOwnAuthorityCheck and StreamPolicy (Android `content://` URIs
// and PackageManager); the iOS share extension hands over files it already copied (see SharedFile).

/// Detects Markdown syntax; the result is only a display hint (`source_format`), never a restriction.
public enum MarkdownDetector {
    private static let patterns: [NSRegularExpression] = [
        "^#{1,6}\\s+\\S",
        "^\\s*[-*+]\\s+\\S",
        "^\\s*\\d+\\.\\s+\\S",
        "^```",
        "\\[[^\\]\\n]+]\\([^)\\s]+\\)",
        "\\*\\*[^*\\n]+\\*\\*",
        "^>\\s+\\S",
    ].map { try! NSRegularExpression(pattern: $0, options: [.anchorsMatchLines]) }

    public static func looksLikeMarkdown(_ text: String) -> Bool {
        let range = NSRange(location: 0, length: (text as NSString).length)
        return patterns.contains { $0.firstMatch(in: text, range: range) != nil }
    }
}

/// Decides whether an HTML flavour (clipboard or share) tells anything the plain text does not. Same rules as
/// `HtmlFlavour` in ContentDetectors.kt (Android) and `detect.rs` (Windows): keep the three aligned.
///
/// Browsers and editors put an HTML flavour on every copy, even a plain selection (`<meta><span>text</span>`), and copy raw
/// Markdown as one `<div>` per line. Neither is a rendering: only real formatting tags are.
public enum HtmlFlavour {
    public enum Richness: Sendable {
        /// Nothing but wrappers.
        case plain
        /// Only lines and paragraphs (`div`, `p`, `br`, at least three of them), no formatting tag.
        case layout
        /// Real formatting tags: links, emphasis, headings, lists, tables, code...
        case formatted
    }

    private static let formattingTags: Set<String> = [
        "a", "b", "strong", "i", "em", "u", "ul", "ol", "li", "h1", "h2", "h3", "h4", "h5", "h6",
        "table", "pre", "code", "blockquote", "img", "hr",
    ]
    private static let layoutTags: Set<String> = ["p", "div", "br"]
    private static let layoutMinTags = 3
    private static let tagName = try! NSRegularExpression(pattern: "<([a-z0-9]+)")

    public static func richness(_ html: String) -> Richness {
        let lower = html.lowercased()
        let ns = lower as NSString
        var layout = 0
        for match in tagName.matches(in: lower, range: NSRange(location: 0, length: ns.length)) {
            let name = ns.substring(with: match.range(at: 1))
            if formattingTags.contains(name) { return .formatted }
            if layoutTags.contains(name) { layout += 1 }
        }
        return layout >= layoutMinTags ? .layout : .plain
    }

    /// True when `html` is worth sending next to `plain`. Wrappers are not. Lines of HTML are not either when the plain text is
    /// Markdown (the receivers would lay the raw `# Title` / `- item` out as HTML instead of rendering them).
    /// `plain` is nil when the HTML is all there is.
    public static func isWorthKeeping(_ html: String, plain: String?) -> Bool {
        switch richness(html) {
        case .formatted: return true
        case .layout: return plain.map { !MarkdownDetector.looksLikeMarkdown($0) } ?? true
        case .plain: return false
        }
    }
}

/// Minimal HTML to plain text conversion used only when an app shares HTML without a plain text version.
public enum HtmlText {
    private static let blockBreaks = try! NSRegularExpression(
        pattern: "<(br\\s*/?|/p|/div|/h[1-6]|/li|/tr)>", options: [.caseInsensitive])
    private static let scriptStyle = try! NSRegularExpression(
        pattern: "<(script|style)\\b.*?</\\1>", options: [.caseInsensitive, .dotMatchesLineSeparators])
    private static let tags = try! NSRegularExpression(pattern: "<[^>]*>")
    private static let manyBreaks = try! NSRegularExpression(pattern: "\n{3,}")

    public static func toPlain(_ html: String) -> String {
        let noScripts = replace(scriptStyle, in: html, with: "")
        let withBreaks = replace(blockBreaks, in: noScripts, with: "\n")
        var text = replace(tags, in: withBreaks, with: "")
            .replacingOccurrences(of: "&nbsp;", with: " ")
            .replacingOccurrences(of: "&lt;", with: "<")
            .replacingOccurrences(of: "&gt;", with: ">")
            .replacingOccurrences(of: "&quot;", with: "\"")
            .replacingOccurrences(of: "&#39;", with: "'")
            .replacingOccurrences(of: "&amp;", with: "&")
        // Kotlin `lines()`: \r\n, \n and \r end a line.
        text = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        let trimmed = text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .joined(separator: "\n")
        return replace(manyBreaks, in: trimmed, with: "\n\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func replace(_ regex: NSRegularExpression, in text: String, with template: String) -> String {
        regex.stringByReplacingMatches(
            in: text, range: NSRange(location: 0, length: (text as NSString).length), withTemplate: template)
    }
}
