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
