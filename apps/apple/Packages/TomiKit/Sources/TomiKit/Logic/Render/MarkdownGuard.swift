import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/MarkdownGuard.kt.
//
// Deviation from Kotlin: md4c is replaced by the renderer of the app itself. After the escaping, the text is parsed
// with Foundation's `AttributedString(markdown:)` (the renderer the views use) and refused when any run still carries
// an image: the check is made on the very tree that is shown, like Kotlin checks the md4c tree it renders. The
// line-based scanner `containsImage` stays as a first, stricter filter.

/// Keeps the markdown renderer 100% offline.
///
/// A markdown renderer may download `http(s)` images itself. Tomi never wants a received text to make the phone contact a
/// server (it would reveal that and when the item was read, and "images" are exactly the classic tracking pixel). So no
/// markdown image is ever given to the renderer:
///
/// 1. `neutralizeImages` escapes every image opener `![` outside code (the image becomes a "!" followed by an ordinary
///    link, which only does something when the user taps it).
/// 2. `safeMarkdown` then scans the result with `containsImage`, and parses it with the renderer: it is refused when
///    an image opener can still be found or when the parsed text holds any image. The caller then shows the plain text.
/// 3. Views render the `AttributedString` returned by `render(_:syntax:)`: the tree that was checked, nothing else.
///
/// Raw HTML (`<img>`) is out of scope: the renderer used by the views never renders HTML from markdown.
///
/// Lengths count Swift characters; the scanning itself works on Unicode scalars, so that a combining mark after `[` or
/// `!` can never hide an image opener, and `\r\n` is two scalars like in Kotlin.
public enum MarkdownGuard {
    /// Longer markdown is shown as plain text: it is not worth scanning twice on the main thread.
    public static let maxChars = 200_000

    /// The markdown that may be handed to the renderer, or nil when it must not be rendered (too long, or still
    /// containing an image opener after `neutralizeImages`).
    public static func safeMarkdown(_ markdown: String) -> String? {
        if markdown.unicodeScalars.allSatisfy({ $0.properties.isWhitespace }) { return markdown }
        if markdown.count > maxChars { return nil }
        let sanitized = neutralizeImages(normalizeLineEndings(markdown))
        if containsImage(sanitized) { return nil }
        return parsed(sanitized, syntax: .full) == nil ? nil : sanitized
    }

    /// The markdown parsed for display with `syntax` (the options the view uses), or nil when it must be shown as
    /// plain text: too long, an image opener left, or any image in the parsed result.
    public static func render(
        _ markdown: String,
        syntax: AttributedString.MarkdownParsingOptions.InterpretedSyntax = .full
    ) -> AttributedString? {
        guard let sanitized = safeMarkdown(markdown) else { return nil }
        return parsed(sanitized, syntax: syntax)
    }

    /// Parses with Foundation's markdown parser; nil if it fails or if any run holds an image.
    private static func parsed(
        _ markdown: String,
        syntax: AttributedString.MarkdownParsingOptions.InterpretedSyntax
    ) -> AttributedString? {
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: false,
            interpretedSyntax: syntax,
            failurePolicy: .returnPartiallyParsedIfPossible
        )
        guard let attributed = try? AttributedString(markdown: markdown, options: options) else { return nil }
        if attributed.runs.contains(where: { $0.imageURL != nil }) { return nil }
        return attributed
    }

    /// CommonMark ends a line on `\r\n`, `\n` and a lone `\r`: the guard works on `\n` only.
    static func normalizeLineEndings(_ text: String) -> String {
        text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
    }

    /// Escapes `![` (as `\![`) everywhere except in fenced code blocks and in backtick code spans, where the characters
    /// are literal text and must stay as they are. Text is otherwise unchanged, line for line.
    public static func neutralizeImages(_ markdown: String) -> String {
        let scalars = Array(markdown.unicodeScalars)
        guard indexOfImageOpener(scalars, from: 0) != nil else { return markdown }
        var out = String.UnicodeScalarView()
        var fenceChar: Unicode.Scalar = " "
        var fenceLength = 0
        for (offset, line) in splitLines(scalars).enumerated() {
            if offset > 0 { out.append("\n") }
            if fenceLength > 0 {
                out.append(contentsOf: line)
                if closesFence(line, fenceChar: fenceChar, fenceLength: fenceLength) { fenceLength = 0 }
            } else if let opening = openingFence(line) {
                fenceChar = opening.char
                fenceLength = opening.length
                out.append(contentsOf: line)
            } else {
                escapeInLine(line, into: &out)
            }
        }
        return String(out)
    }

    /// True when `markdown` still contains something the renderer could take for an image: an unescaped `![` (inline,
    /// reference, collapsed, shortcut or nested in a link: they all start with it).
    ///
    /// Written to over-refuse rather than to miss, and independent of the line-by-line logic of `neutralizeImages`:
    ///  - an opener is only ignored inside a top-level fenced code block (opened at column 0, not inside an HTML block),
    ///    or inside an inline code span that closes on the same line;
    ///  - a code span is only trusted when every backtick run before it is itself closed on its own line (so the pairing
    ///    cannot be shifted by a run that the renderer would pair across lines) and the paragraph has no `<`, `(`, `[`,
    ///    `]` or `|` outside code spans (a tag, link destination, link text or table cell could swallow a backtick and
    ///    make the renderer see the "code" as markup).
    /// Indented code blocks are not skipped: `neutralizeImages` escaped them, and an escaped opener is not flagged.
    public static func containsImage(_ markdown: String) -> Bool {
        let scalars = Array(markdown.unicodeScalars)
        guard indexOfImageOpener(scalars, from: 0) != nil else { return false }
        var fence: (char: Unicode.Scalar, length: Int)?
        var html: HtmlBlockEnd?
        var chunk: [[Unicode.Scalar]] = []
        for line in splitLines(scalars) {
            if let open = fence {
                if closesFence(line, fenceChar: open.char, fenceLength: open.length) { fence = nil }
                continue
            }
            if let end = html {
                switch end {
                case .blankLine:
                    if isBlank(line) {
                        html = nil
                        if scanChunk(chunk) { return true }
                        chunk = []
                        continue
                    }
                case let .marker(marker):
                    if containsText(line, marker) { html = nil }
                }
                chunk.append(line)
                continue
            }
            if isBlank(line) {
                if scanChunk(chunk) { return true }
                chunk = []
                continue
            }
            if let opening = openingFence(line), leadingSpaces(line) == 0 {
                if scanChunk(chunk) { return true }
                chunk = []
                fence = (opening.char, opening.length)
                continue
            }
            if let start = htmlBlockStart(line) {
                switch start {
                case .blankLine: html = .blankLine
                case let .marker(marker):
                    // The end marker may be on the start line itself.
                    if !containsText(Array(line.dropFirst()), marker) { html = start }
                }
            }
            chunk.append(line)
        }
        return scanChunk(chunk)
    }

    // MARK: - Escaping

    /// The fence character and length when `line` opens a fenced code block (up to 3 spaces of indent, then 3+ backticks
    /// or tildes).
    private static func openingFence(_ line: [Unicode.Scalar]) -> (char: Unicode.Scalar, length: Int)? {
        let indent = leadingSpaces(line)
        if indent > 3 || indent >= line.count { return nil }
        let c = line[indent]
        if c != "`" && c != "~" { return nil }
        let run = countRun(line, from: indent, c)
        if run < 3 { return nil }
        // A backtick fence cannot carry a backtick in its info string (that would be an inline code span).
        if c == "`" && line[(indent + run)...].contains("`") { return nil }
        return (c, run)
    }

    private static func closesFence(_ line: [Unicode.Scalar], fenceChar: Unicode.Scalar, fenceLength: Int) -> Bool {
        let indent = leadingSpaces(line)
        if indent > 3 { return false }
        let run = countRun(line, from: indent, fenceChar)
        return run >= fenceLength && isBlank(Array(line[(indent + run)...]))
    }

    private static func escapeInLine(_ line: [Unicode.Scalar], into out: inout String.UnicodeScalarView) {
        var i = 0
        while i < line.count {
            let c = line[i]
            if c == "\\" && i + 1 < line.count {
                // A backslash escapes the next character: "\\" is a literal backslash, so a "![" after it IS an image opener.
                out.append(c)
                out.append(line[i + 1])
                i += 2
            } else if c == "`" {
                let run = countRun(line, from: i, "`")
                let close = findClosingRun(line, from: i + run, length: run)
                if close < 0 {
                    out.append(contentsOf: line[i..<(i + run)])
                    i += run
                } else {
                    out.append(contentsOf: line[i..<(close + run)])
                    i = close + run
                }
            } else if c == "!" && i + 1 < line.count && line[i + 1] == "[" {
                out.append("\\")
                out.append("!")
                i += 1
            } else {
                out.append(c)
                i += 1
            }
        }
    }

    // MARK: - Scanning

    private enum HtmlBlockEnd {
        case blankLine
        case marker([Unicode.Scalar])
    }

    /// How an HTML block opened by `line` ends (CommonMark 4.6), or nil when the line does not open one (at column 0 to 3).
    /// Deliberately broad: a line that looks like a tag start is treated as an HTML block, where no fence is recognised.
    private static func htmlBlockStart(_ line: [Unicode.Scalar]) -> HtmlBlockEnd? {
        let indent = leadingSpaces(line)
        guard indent <= 3, indent < line.count, line[indent] == "<" else { return nil }
        let rest = Array(line[indent...])
        let lower = rest.map(asciiLower)
        func starts(_ text: String) -> Bool { lower.starts(with: Array(text.unicodeScalars)) }
        if starts("<!--") { return .marker(Array("-->".unicodeScalars)) }
        if starts("<?") { return .marker(Array("?>".unicodeScalars)) }
        if starts("<![cdata[") { return .marker(Array("]]>".unicodeScalars)) }
        for name in ["script", "pre", "style", "textarea"] where starts("<" + name) {
            let after = rest.count > name.count + 1 ? rest[name.count + 1] : " "
            if after == " " || after == "\t" || after == ">" || after == "\n" || after == "\r" {
                return .marker(Array(("</" + name + ">").unicodeScalars))
            }
        }
        if rest.count > 1, rest[1] == "!" || rest[1] == "/" || rest[1].properties.isAlphabetic { return .blankLine }
        return nil
    }

    /// Scans the lines of one paragraph-like chunk (contiguous non-blank lines outside fenced code). See `containsImage`.
    private static func scanChunk(_ lines: [[Unicode.Scalar]]) -> Bool {
        var outsideHazard = false
        var spanHoldsOpener = false
        var pairingCertain = true
        for line in lines {
            var i = 0
            while i < line.count {
                let c = line[i]
                if c == "\\" && i + 1 < line.count {
                    i += 2
                } else if c == "`" {
                    let run = countRun(line, from: i, "`")
                    let close = findClosingRun(line, from: i + run, length: run)
                    if close < 0 {
                        pairingCertain = false
                        i += run
                    } else {
                        if indexOfImageOpener(Array(line[i..<(close + run)]), from: 0) != nil {
                            if !pairingCertain { return true }
                            spanHoldsOpener = true
                        }
                        i = close + run
                    }
                } else if c == "!" && i + 1 < line.count && line[i + 1] == "[" {
                    return true
                } else {
                    if c == "<" || c == "(" || c == "[" || c == "]" || c == "|" { outsideHazard = true }
                    i += 1
                }
            }
        }
        return spanHoldsOpener && outsideHazard
    }

    // MARK: - Scalar helpers

    private static func splitLines(_ scalars: [Unicode.Scalar]) -> [[Unicode.Scalar]] {
        scalars.split(separator: "\n", omittingEmptySubsequences: false).map(Array.init)
    }

    private static func leadingSpaces(_ line: [Unicode.Scalar]) -> Int {
        var n = 0
        while n < line.count && line[n] == " " { n += 1 }
        return n
    }

    /// CommonMark blank line: spaces and tabs only (a no-break space is content).
    private static func isBlank(_ line: [Unicode.Scalar]) -> Bool {
        line.allSatisfy { $0 == " " || $0 == "\t" }
    }

    private static func countRun(_ line: [Unicode.Scalar], from: Int, _ c: Unicode.Scalar) -> Int {
        var n = 0
        while from + n < line.count && line[from + n] == c { n += 1 }
        return n
    }

    /// Start of the next run of exactly `length` backticks at or after `from`, or -1.
    private static func findClosingRun(_ line: [Unicode.Scalar], from: Int, length: Int) -> Int {
        var i = from
        while i < line.count {
            if line[i] == "`" {
                let run = countRun(line, from: i, "`")
                if run == length { return i }
                i += run
            } else {
                i += 1
            }
        }
        return -1
    }

    private static func indexOfImageOpener(_ scalars: [Unicode.Scalar], from: Int) -> Int? {
        var i = from
        while i + 1 < scalars.count {
            if scalars[i] == "!" && scalars[i + 1] == "[" { return i }
            i += 1
        }
        return nil
    }

    private static func asciiLower(_ s: Unicode.Scalar) -> Unicode.Scalar {
        s.value >= 65 && s.value <= 90 ? Unicode.Scalar(s.value + 32)! : s
    }

    /// Case-insensitive (ASCII) search of `needle` in `line`.
    private static func containsText(_ line: [Unicode.Scalar], _ needle: [Unicode.Scalar]) -> Bool {
        guard !needle.isEmpty, line.count >= needle.count else { return false }
        let n = needle.map(asciiLower)
        for start in 0...(line.count - needle.count) {
            var matches = true
            for k in 0..<n.count where asciiLower(line[start + k]) != n[k] {
                matches = false
                break
            }
            if matches { return true }
        }
        return false
    }
}
