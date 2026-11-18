import Foundation

// Port of apps/android/.../share/LinkExtractor.kt.

/// Result of splitting a shared string such as `Page title https://example.org/a`.
public struct LinkParts: Hashable, Sendable {
    public var url: String
    public var leftover: String?

    public init(url: String, leftover: String?) {
        self.url = url
        self.leftover = leftover
    }
}

public enum LinkExtractor {
    public static let maxURLLength = 2048
    private static let urlRegex = try! NSRegularExpression(pattern: "https?://[^\\s<>\"']+", options: [.caseInsensitive])
    private static let trailingPunctuation: Set<Character> = [".", ",", ";", ":", "!", "?", ")", "]", "}", ">", "\u{2019}", "\u{201D}"]
    private static let separators: Set<Character> = [" ", "\t", "\n", "-", "\u{2013}", "\u{2014}", ":", "|", "\u{00B7}", "\u{2022}"]
    private static let maxLeftoverChars = 300
    private static let maxLeftoverLines = 3

    /// True if `value` is a single http(s) URL with a host and nothing else.
    public static func isSingleURL(_ value: String) -> Bool {
        let v = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if v.isEmpty || v.utf16.count > maxURLLength || v.contains(where: \.isWhitespace) { return false }
        return isValidHTTPURL(v)
    }

    /// Validates the scheme (http / https only) and the host, as strictly as Java's `URI` on Android: no character
    /// outside RFC 3986 (no auto-encoding), well-formed `%XX` escapes, at most one `#`, and a host that is a DNS name
    /// (labels of letters, digits and inner hyphens, the last one starting with a letter), an IPv4 address or a
    /// bracketed IPv6 address.
    public static func isValidHTTPURL(_ value: String) -> Bool {
        if value.utf16.count > maxURLLength || value.contains(where: \.isWhitespace) { return false }
        if value.filter({ $0 == "#" }).count > 1 || !hasValidEscapes(value) { return false }
        // Java accepts non-ASCII characters ("other") outside the authority but never in the host: refuse them there,
        // percent-encode them elsewhere, then parse without any auto-encoding.
        guard let schemeEnd = value.range(of: "://") else { return false }
        let rest = value[schemeEnd.upperBound...]
        let authority = rest.prefix { $0 != "/" && $0 != "?" && $0 != "#" }
        if !authority.unicodeScalars.allSatisfy(\.isASCII) { return false }
        let encoded = String(value.unicodeScalars.map { scalar in
            scalar.isASCII ? String(scalar) : String(scalar).utf8.map { String(format: "%%%02X", $0) }.joined()
        }.joined())
        guard let components = URLComponents(string: encoded, encodingInvalidCharacters: false),
              let scheme = components.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = components.encodedHost, !host.isEmpty
        else { return false }
        return isValidHost(host)
    }

    private static func hasValidEscapes(_ value: String) -> Bool {
        let bytes = Array(value.utf8)
        var i = 0
        while i < bytes.count {
            if bytes[i] == UInt8(ascii: "%") {
                guard i + 2 < bytes.count, isHex(bytes[i + 1]), isHex(bytes[i + 2]) else { return false }
                i += 3
            } else {
                i += 1
            }
        }
        return true
    }

    private static func isHex(_ b: UInt8) -> Bool {
        (48...57).contains(b) || (65...70).contains(b) || (97...102).contains(b)
    }

    private static func isValidHost(_ host: String) -> Bool {
        if host.hasPrefix("[") { return host.hasSuffix("]") && host.count > 2 }
        let labels = host.split(separator: ".", omittingEmptySubsequences: false).map(String.init)
        if labels.count == 4, labels.allSatisfy({ !$0.isEmpty && $0.count <= 3 && $0.allSatisfy(\.isASCIIDigitChar) && Int($0)! <= 255 }) {
            return true
        }
        let named = labels.last == "" && labels.count > 1 ? Array(labels.dropLast()) : labels
        guard let last = named.last, let firstOfLast = last.first, firstOfLast.isASCIILetterChar else { return false }
        return named.allSatisfy { label in
            !label.isEmpty && label.first != "-" && label.last != "-"
                && label.allSatisfy { $0.isASCIILetterChar || $0.isASCIIDigitChar || $0 == "-" }
        }
    }

    /// Finds exactly one URL at the very start or the very end of `text` (the shape produced by
    /// browsers and most apps: "Title URL"). A URL in the middle of a sentence, several URLs, or too
    /// much surrounding text means the string is plain text: nil.
    public static func extract(_ text: String) -> LinkParts? {
        let ns = text as NSString
        let matches = urlRegex.matches(in: text, range: NSRange(location: 0, length: ns.length))
        guard matches.count == 1, let match = matches.first else { return nil }
        let url = trimTrailing(ns.substring(with: match.range))
        guard !url.isEmpty, isValidHTTPURL(url) else { return nil }

        let before = ns.substring(to: match.range.location)
        // Punctuation stripped from the end of the URL ("see https://x.org.") is not "text after the URL".
        let after = ns.substring(from: match.range.location + match.range.length)
        if !before.isBlank && !after.isBlank { return nil }

        let leftover = trim((before + after).trimmingCharacters(in: .whitespacesAndNewlines), separators)
        // Kotlin `lines()`: \r\n, \n and \r all end a line; lengths in UTF-16 units like Kotlin.
        let lines = leftover.replacingOccurrences(of: "\r\n", with: "\n").split(omittingEmptySubsequences: false) {
            $0 == "\n" || $0 == "\r"
        }.count
        if leftover.utf16.count > maxLeftoverChars || lines > maxLeftoverLines { return nil }
        return LinkParts(url: url, leftover: leftover.isEmpty ? nil : leftover)
    }

    private static func trimTrailing(_ url: String) -> String {
        var chars = Array(url)
        while let last = chars.last, trailingPunctuation.contains(last) {
            // Keep a closing parenthesis that balances an opening one inside the URL (Wikipedia links).
            if last == ")" && chars.filter({ $0 == "(" }).count >= chars.filter({ $0 == ")" }).count { break }
            chars.removeLast()
        }
        return String(chars)
    }

    private static func trim(_ s: String, _ set: Set<Character>) -> String {
        var chars = Substring(s)
        while let first = chars.first, set.contains(first) { chars.removeFirst() }
        while let last = chars.last, set.contains(last) { chars.removeLast() }
        return String(chars)
    }
}

extension String {
    /// Kotlin `isBlank`: empty or only whitespace.
    var isBlank: Bool { allSatisfy(\.isWhitespace) }
}

extension Character {
    fileprivate var isASCIILetterChar: Bool { isASCII && isLetter }
    fileprivate var isASCIIDigitChar: Bool { isASCII && isNumber }
}
