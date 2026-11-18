import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/HtmlSanitizer.kt.
//
// Deviation from Kotlin: no Jsoup on iOS and no dependency allowed, so the allow-list sanitiser is written here. It
// follows the Jsoup `Safelist.relaxed()` allow-list of tags and attributes, with the Kotlin changes (no `style`, `img src`
// only inline `data:` images) and is stricter on purpose: `a href` only `http`, `https`, `mailto`, `tel` (no `ftp`, no
// relative link; `tel` is the one addition to Jsoup's list, the same schemes as the markdown links of RichTextViews.kt,
// and a tel link only opens the dialer when tapped), no `cite` attribute, only base64 raster `data:image/*` sources, and an `img` whose source is refused
// is removed. The output is rebuilt from the tokens: nothing of the input is copied verbatim (text and attribute values
// are escaped), so what the browser parses is exactly what was checked.

/// Sanitises received HTML (DEV.md invariant 6) before it goes to the sandboxed web view.
/// Defence in depth: this allow-list, the CSP below, and the web view itself (no JS, no network, no file access).
public enum HtmlSanitizer {
    public static let csp = "default-src 'none'; img-src data:; style-src 'unsafe-inline'"

    /// Returns the cleaned body fragment: no script, iframe, form, event handler or remote resource.
    public static func clean(_ html: String) -> String {
        var cleaner = HtmlFragmentCleaner(html)
        return cleaner.run()
    }

    /// Full document to load in the web view: cleaned body, CSP meta tag, and theme colours. With `horizontalScroll` false
    /// (card preview) the page never scrolls sideways, so that a drag across a wide table is left to the card (swipe to
    /// archive). The colours are CSS values (`#RRGGBB`, `rgb(...)`); anything with other characters is replaced by a
    /// neutral keyword, so that a colour can never open a new CSS rule.
    public static func toDocument(
        _ html: String, textColorCss: String, linkColorCss: String, horizontalScroll: Bool = true
    ) -> String {
        let text = safeCssColor(textColorCss, fallback: "CanvasText")
        let link = safeCssColor(linkColorCss, fallback: "LinkText")
        var lines = [
            "<!DOCTYPE html>",
            "<html>",
            "<head>",
            "<meta charset=\"utf-8\">",
            "<meta http-equiv=\"Content-Security-Policy\" content=\"\(csp)\">",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
            "<style>",
            "html, body { margin: 0; padding: 0; background: transparent; color: \(text);",
            "  font-family: sans-serif; font-size: 15px; line-height: 1.4; word-wrap: break-word; }",
        ]
        if !horizontalScroll { lines.append("html, body { overflow-x: hidden; }") }
        lines += [
            "a { color: \(link); }",
            "img { max-width: 100%; height: auto; }",
            "pre, code { white-space: pre-wrap; }",
            "table { border-collapse: collapse; }",
            "td, th { border: 1px solid; padding: 2px 6px; }",
            "</style>",
            "</head>",
            "<body>\(clean(html))</body>",
            "</html>",
        ]
        return lines.joined(separator: "\n")
    }

    private static func safeCssColor(_ value: String, fallback: String) -> String {
        let trimmed = value.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty || trimmed.count > 64 { return fallback }
        let allowed = trimmed.unicodeScalars.allSatisfy { s in
            (s.value < 128 && (CharacterSet.alphanumerics.contains(s) || "#(),.% -/".unicodeScalars.contains(s)))
        }
        return allowed ? trimmed : fallback
    }
}

// MARK: - Tokeniser and rebuilder

private typealias Scalars = [Unicode.Scalar]

private struct HtmlFragmentCleaner {
    /// Tags kept (Jsoup `Safelist.relaxed()`).
    static let allowedTags: Set<String> = [
        "a", "b", "blockquote", "br", "caption", "cite", "code", "col", "colgroup", "dd", "div", "dl", "dt", "em",
        "h1", "h2", "h3", "h4", "h5", "h6", "i", "img", "li", "ol", "p", "pre", "q", "small", "span", "strike",
        "strong", "sub", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "u", "ul",
    ]
    static let voidTags: Set<String> = ["br", "img", "col"]

    /// Attributes kept per tag (Jsoup relaxed, without `style`, `cite`, and with the stricter protocols).
    static let allowedAttributes: [String: Set<String>] = [
        "a": ["href", "title"],
        "col": ["span", "width"],
        "colgroup": ["span", "width"],
        "img": ["align", "alt", "height", "src", "title", "width"],
        "ol": ["start", "type"],
        "table": ["summary", "width"],
        "td": ["abbr", "axis", "colspan", "rowspan", "width"],
        "th": ["abbr", "axis", "colspan", "rowspan", "scope", "width"],
        "ul": ["type"],
    ]

    /// Elements removed WITH their content. Raw-text elements end at the first matching end tag.
    static let removedRawText: Set<String> = [
        "script", "style", "iframe", "noscript", "noembed", "noframes", "xmp", "textarea", "title",
    ]
    /// Removed with their content, nested ones counted. An unclosed one removes the rest of the input.
    static let removedNested: Set<String> = ["object", "template", "svg", "math", "applet"]
    /// Void elements removed (they have no content).
    static let removedVoid: Set<String> = ["embed"]

    static let maxDepth = 200

    let s: Scalars
    var i = 0
    var out = String.UnicodeScalarView()
    var stack: [String] = []

    init(_ html: String) {
        // NUL means nothing in HTML and is a classic filter-evasion character (`<scr\0ipt>`): drop it first.
        s = html.unicodeScalars.filter { $0.value != 0 }
    }

    mutating func run() -> String {
        let n = s.count
        while i < n {
            let c = s[i]
            if c == "<" {
                parseMarkup()
            } else if c == ">" {
                emit("&gt;")
                i += 1
            } else if c == "&" {
                if let end = entityEnd(from: i) {
                    out.append(contentsOf: s[i..<end])
                    i = end
                } else {
                    emit("&amp;")
                    i += 1
                }
            } else {
                out.append(c)
                i += 1
            }
        }
        for name in stack.reversed() { emit("</\(name)>") }
        stack = []
        return String(out)
    }

    private mutating func emit(_ text: String) {
        out.append(contentsOf: text.unicodeScalars)
    }

    // MARK: Markup

    /// `s[i]` is `<`.
    private mutating func parseMarkup() {
        let n = s.count
        guard i + 1 < n else {
            emit("&lt;")
            i += 1
            return
        }
        let next = s[i + 1]
        if next == "!" {
            if hasPrefix("<!--", at: i) {
                skipComment()
            } else {
                // Doctype, CDATA, bogus comment: dropped up to the next `>`.
                skipPastGreaterThan(from: i + 2)
            }
        } else if next == "?" {
            skipPastGreaterThan(from: i + 2)
        } else if next == "/" {
            if i + 2 < n, isAsciiLetter(s[i + 2]) {
                parseEndTag()
            } else {
                // `</>` or `</` + something else: a bogus comment for the browser, dropped here.
                skipPastGreaterThan(from: i + 2)
            }
        } else if isAsciiLetter(next) {
            parseStartTag()
        } else {
            emit("&lt;")
            i += 1
        }
    }

    private mutating func skipComment() {
        // `<!-->` and `<!--->` are complete (empty) comments; otherwise it ends at `-->` or `--!>`; unclosed: to the end.
        var j = i + 4
        if j < s.count, s[j] == ">" { i = j + 1; return }
        if j + 1 < s.count, s[j] == "-", s[j + 1] == ">" { i = j + 2; return }
        while j + 2 < s.count {
            if s[j] == "-", s[j + 1] == "-" {
                if s[j + 2] == ">" { i = j + 3; return }
                if s[j + 2] == "!", j + 3 < s.count, s[j + 3] == ">" { i = j + 4; return }
            }
            j += 1
        }
        i = s.count
    }

    private mutating func skipPastGreaterThan(from start: Int) {
        var j = start
        while j < s.count, s[j] != ">" { j += 1 }
        i = min(j + 1, s.count)
    }

    private mutating func parseEndTag() {
        var j = i + 2
        var name = ""
        while j < s.count, !isTagNameEnd(s[j]) {
            name.unicodeScalars.append(asciiLower(s[j]))
            j += 1
        }
        // The rest of an end tag (attributes) is ignored up to `>`.
        while j < s.count, s[j] != ">" { j += 1 }
        i = min(j + 1, s.count)
        if Self.allowedTags.contains(name), !Self.voidTags.contains(name), let index = stack.lastIndex(of: name) {
            while stack.count > index {
                let open = stack.removeLast()
                emit("</\(open)>")
            }
        }
    }

    private mutating func parseStartTag() {
        let n = s.count
        var j = i + 1
        var name = ""
        while j < n, !isTagNameEnd(s[j]) {
            name.unicodeScalars.append(asciiLower(s[j]))
            j += 1
        }
        var attributes: [(name: String, value: String)] = []
        var seen = Set<String>()
        while true {
            while j < n, isHtmlSpace(s[j]) { j += 1 }
            if j >= n {
                i = n // end of input inside a tag: the browser drops the tag, so do we (and the rest)
                return
            }
            if s[j] == ">" {
                j += 1
                break
            }
            if s[j] == "/" {
                j += 1
                continue
            }
            var attrName = ""
            attrName.unicodeScalars.append(asciiLower(s[j]))
            j += 1
            while j < n, !isHtmlSpace(s[j]), s[j] != "/", s[j] != ">", s[j] != "=" {
                attrName.unicodeScalars.append(asciiLower(s[j]))
                j += 1
            }
            while j < n, isHtmlSpace(s[j]) { j += 1 }
            var value = ""
            if j < n, s[j] == "=" {
                j += 1
                while j < n, isHtmlSpace(s[j]) { j += 1 }
                if j >= n {
                    i = n
                    return
                }
                if s[j] == "\"" || s[j] == "'" {
                    let quote = s[j]
                    j += 1
                    let start = j
                    while j < n, s[j] != quote { j += 1 }
                    if j >= n {
                        i = n
                        return
                    }
                    value = String(String.UnicodeScalarView(s[start..<j]))
                    j += 1
                } else {
                    let start = j
                    while j < n, !isHtmlSpace(s[j]), s[j] != ">" { j += 1 }
                    value = String(String.UnicodeScalarView(s[start..<j]))
                }
            }
            // The first of two attributes with the same name wins, like in a browser.
            if seen.insert(attrName).inserted { attributes.append((attrName, value)) }
        }
        i = j

        if Self.removedRawText.contains(name) {
            skipRawText(name)
        } else if Self.removedNested.contains(name) {
            skipNested(name)
        } else if Self.removedVoid.contains(name) {
            return
        } else if Self.allowedTags.contains(name) {
            emitStartTag(name, attributes)
        }
        // Any other tag (form, input, font, link, meta...) is dropped; its text stays.
    }

    private mutating func emitStartTag(_ name: String, _ attributes: [(name: String, value: String)]) {
        let isVoid = Self.voidTags.contains(name)
        if !isVoid && stack.count >= Self.maxDepth { return }
        let allowed = Self.allowedAttributes[name] ?? []
        var rendered = ""
        for (attrName, rawValue) in attributes where allowed.contains(attrName) {
            let decoded = decodeEntities(rawValue)
            var value: String
            switch (name, attrName) {
            case ("a", "href"):
                guard let url = checkedURL(decoded, schemes: ["http://", "https://", "mailto:", "tel:"]) else { continue }
                value = url
            case ("img", "src"):
                guard let url = checkedDataImage(decoded) else { continue }
                value = url
            default:
                value = String(String.UnicodeScalarView(decoded.unicodeScalars.filter { !isControl($0) }))
            }
            rendered += " \(attrName)=\"\(escapeAttribute(value))\""
        }
        if name == "img", !rendered.contains(" src=\"") { return } // an image without an accepted source is removed
        emit("<\(name)\(rendered)>")
        if !isVoid { stack.append(name) }
    }

    // MARK: Removed elements

    /// After the start tag of a raw-text element: skips to after its first end tag (or to the end of the input).
    private mutating func skipRawText(_ name: String) {
        let needle = Array(name.unicodeScalars)
        var j = i
        while j + 1 < s.count {
            if s[j] == "<", s[j + 1] == "/", matchesName(needle, at: j + 2) {
                skipPastGreaterThan(from: j + 2 + needle.count)
                return
            }
            j += 1
        }
        i = s.count
    }

    /// After the start tag of a removed element that can nest: skips to after its matching end tag.
    private mutating func skipNested(_ name: String) {
        let needle = Array(name.unicodeScalars)
        var depth = 1
        var j = i
        while j + 1 < s.count {
            if s[j] == "<" {
                if s[j + 1] == "/", matchesName(needle, at: j + 2) {
                    depth -= 1
                    if depth == 0 {
                        skipPastGreaterThan(from: j + 2 + needle.count)
                        return
                    }
                } else if matchesName(needle, at: j + 1) {
                    depth += 1
                }
            }
            j += 1
        }
        i = s.count
    }

    /// `name` at `position`, followed by a space, `/` or `>` (so `<scripts>` is not `<script>`).
    private func matchesName(_ name: Scalars, at position: Int) -> Bool {
        guard position + name.count <= s.count else { return false }
        for (k, c) in name.enumerated() where asciiLower(s[position + k]) != c { return false }
        let end = position + name.count
        return end >= s.count || isTagNameEnd(s[end])
    }

    // MARK: Entities and values

    private func hasPrefix(_ text: String, at position: Int) -> Bool {
        let prefix = Array(text.unicodeScalars)
        guard position + prefix.count <= s.count else { return false }
        for (k, c) in prefix.enumerated() where s[position + k] != c { return false }
        return true
    }

    /// End (exclusive) of a well-formed character reference starting at `start` (`&amp;`, `&#38;`, `&#x26;`), or nil.
    /// Such a reference in text only ever produces a character, never markup, so it is passed through.
    private func entityEnd(from start: Int) -> Int? {
        var j = start + 1
        guard j < s.count else { return nil }
        let first = j
        if s[j] == "#" {
            j += 1
            let hex = j < s.count && (s[j] == "x" || s[j] == "X")
            if hex { j += 1 }
            let digitsStart = j
            while j < s.count, hex ? isHexDigit(s[j]) : isDigit(s[j]) { j += 1 }
            guard j > digitsStart, j - digitsStart <= 8 else { return nil }
        } else {
            while j < s.count, isAsciiLetter(s[j]) || isDigit(s[j]) { j += 1 }
            guard j > first, j - first <= 32 else { return nil }
        }
        guard j < s.count, s[j] == ";" else { return nil }
        return j + 1
    }
}

// MARK: - Free helpers (file private)

private func isAsciiLetter(_ c: Unicode.Scalar) -> Bool {
    (c.value >= 65 && c.value <= 90) || (c.value >= 97 && c.value <= 122)
}

private func isDigit(_ c: Unicode.Scalar) -> Bool { c.value >= 48 && c.value <= 57 }

private func isHexDigit(_ c: Unicode.Scalar) -> Bool {
    isDigit(c) || (c.value >= 65 && c.value <= 70) || (c.value >= 97 && c.value <= 102)
}

private func asciiLower(_ c: Unicode.Scalar) -> Unicode.Scalar {
    c.value >= 65 && c.value <= 90 ? Unicode.Scalar(c.value + 32)! : c
}

private func isHtmlSpace(_ c: Unicode.Scalar) -> Bool {
    c == " " || c == "\t" || c == "\n" || c == "\r" || c.value == 0x0C
}

private func isTagNameEnd(_ c: Unicode.Scalar) -> Bool { isHtmlSpace(c) || c == "/" || c == ">" }

private func isControl(_ c: Unicode.Scalar) -> Bool {
    c.value < 0x20 || (c.value >= 0x7F && c.value <= 0x9F)
}

private func escapeAttribute(_ value: String) -> String {
    var result = String.UnicodeScalarView()
    for c in value.unicodeScalars {
        switch c {
        case "&": result.append(contentsOf: "&amp;".unicodeScalars)
        case "\"": result.append(contentsOf: "&quot;".unicodeScalars)
        case "<": result.append(contentsOf: "&lt;".unicodeScalars)
        case ">": result.append(contentsOf: "&gt;".unicodeScalars)
        default: result.append(c)
        }
    }
    return String(result)
}

private let namedReferences: [String: String] = [
    "amp": "&", "lt": "<", "gt": ">", "quot": "\"", "apos": "'", "nbsp": "\u{00A0}", "colon": ":",
    "tab": "\t", "newline": "\n", "sol": "/", "lpar": "(", "rpar": ")", "period": ".", "comma": ",",
    "semi": ";", "num": "#", "excl": "!", "quest": "?", "equals": "=", "commat": "@", "percnt": "%",
]

/// Decodes the character references of an attribute value: numeric ones (with or without the final `;`, as browsers
/// do) and a small set of named ones. An unknown name stays as it is. The result is re-escaped on output, so what is
/// checked is exactly what the browser will see.
private func decodeEntities(_ value: String) -> String {
    let s = Array(value.unicodeScalars)
    guard s.contains("&") else { return value }
    var out = String.UnicodeScalarView()
    var i = 0
    while i < s.count {
        guard s[i] == "&" else {
            out.append(s[i])
            i += 1
            continue
        }
        var j = i + 1
        if j < s.count, s[j] == "#" {
            j += 1
            let hex = j < s.count && (s[j] == "x" || s[j] == "X")
            if hex { j += 1 }
            let start = j
            var number: UInt64 = 0
            while j < s.count, hex ? isHexDigit(s[j]) : isDigit(s[j]) {
                number = min(number * (hex ? 16 : 10) + UInt64(Int(String(s[j]), radix: hex ? 16 : 10) ?? 0), 0x110000)
                j += 1
            }
            if j > start {
                if j < s.count, s[j] == ";" { j += 1 }
                if number == 0 || number > 0x10FFFF || (0xD800...0xDFFF).contains(number) {
                    out.append("\u{FFFD}")
                } else {
                    out.append(Unicode.Scalar(UInt32(number)) ?? "\u{FFFD}")
                }
                i = j
                continue
            }
        } else {
            let start = j
            while j < s.count, isAsciiLetter(s[j]) || isDigit(s[j]) { j += 1 }
            if j > start, j < s.count, s[j] == ";" {
                let name = String(String.UnicodeScalarView(s[start..<j])).lowercased()
                if let replacement = namedReferences[name] {
                    out.append(contentsOf: replacement.unicodeScalars)
                    i = j + 1
                    continue
                }
            }
        }
        out.append("&")
        i += 1
    }
    return String(out)
}

/// What a browser does to a URL before reading its scheme: leading and trailing control characters and spaces go, tabs
/// and line breaks go anywhere. Any other control character refuses the value.
private func normalisedURL(_ value: String) -> String? {
    let all = Array(value.unicodeScalars)
    // Index-based trim: linear even for a value made of hundreds of thousands of spaces.
    guard let start = all.firstIndex(where: { $0.value > 0x20 }), let end = all.lastIndex(where: { $0.value > 0x20 })
    else { return nil }
    var scalars = Array(all[start...end])
    scalars.removeAll { $0 == "\t" || $0 == "\n" || $0 == "\r" }
    if scalars.isEmpty || scalars.count > 100_000 { return nil }
    if scalars.contains(where: isControl) { return nil }
    return String(String.UnicodeScalarView(scalars))
}

private func checkedURL(_ value: String, schemes: [String]) -> String? {
    guard let url = normalisedURL(value) else { return nil }
    let lower = String(String.UnicodeScalarView(url.unicodeScalars.prefix(10).map(asciiLower)))
    return schemes.contains(where: { lower.hasPrefix($0) }) ? url : nil
}

/// Only `data:image/<raster type>;base64,<base64>`: no SVG (it can carry script), no non-base64 payload.
private func checkedDataImage(_ value: String) -> String? {
    guard let url = normalisedURL(value) else { return nil }
    let scalars = Array(url.unicodeScalars)
    for type in ["png", "jpeg", "jpg", "gif", "webp", "bmp", "avif"] {
        let prefix = Array("data:image/\(type);base64,".unicodeScalars)
        guard scalars.count > prefix.count, zip(scalars, prefix).allSatisfy({ asciiLower($0) == $1 }) else { continue }
        let payload = scalars[prefix.count...]
        let base64 = payload.allSatisfy { isAsciiLetter($0) || isDigit($0) || $0 == "+" || $0 == "/" || $0 == "=" }
        return base64 ? url : nil
    }
    return nil
}
