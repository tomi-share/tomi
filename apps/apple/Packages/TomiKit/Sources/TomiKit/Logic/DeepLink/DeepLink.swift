import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/deeplink/DeepLink.kt.

/// Commands that can arrive through `tomi-share://` or an `https://<relay>/l/...` / `https://<relay>/create...` link.
///
/// Any web page can trigger these URLs, so by design none of them performs an action: they only pre-fill the
/// compose field, open a screen, or hand an opaque pairing payload to a screen that still waits for the user.
/// The cases that carry content print nothing of it (`description`, `debugDescription`, `customMirror`).
public enum DeepLink: Hashable, Sendable {
    case open

    /// Pre-fills the compose field. `droppedParameters` is true if a parameter was ignored
    /// (too long, invalid, not http(s)) so the UI can tell the user.
    case send(title: String?, url: String?, text: String?, droppedParameters: Bool)

    case item(id: String)

    /// `https://<relay>/l/pair#<code>` (Universal Link, QR code) or `tomi-share://pair#<code>`, read by the core: opens the
    /// pairing screen with the code and the relay of the link (nil for the `tomi-share://` form). **The relay of a link is
    /// not trusted**: the screens show it in full next to the verification code. Never joins by itself, the user presses
    /// "Rejoindre" (spec/pairing-v1.md 4.4). The code is a pairing secret: never printed.
    case pair(relayUrl: String?, code: String)

    /// `https://<relay>/create#<token>`: opens the creation screen with the relay and the installation token of the
    /// link. Same trust as `pair`; never creates by itself. The token is a secret: never printed.
    case setup(relayUrl: String, token: String)
}

extension DeepLink: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String {
        switch self {
        case .open: "Open"
        case let .send(_, _, _, dropped): "Send(droppedParameters=\(dropped), <redacted>)"
        case let .item(id): "Item(id=\(id))"
        case .pair: "Pair(<redacted>)"
        case .setup: "Setup(<redacted>)"
        }
    }

    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .enum) }
}

public enum DeepLinkParser {
    public static let maxURILength = 8 * 1024
    public static let maxURLParam = LinkExtractor.maxURLLength
    public static let maxTitleParam = 300
    public static let maxTextParam = 4 * 1024
    /// Longest pairing or creation link read (a code, or a token of 40 characters, and a relay authority).
    public static let maxPairPayload = 512
    /// The reading of pairing and creation links: the core (`TomiRuntime.CoreLinkReader`), installed once by the app at
    /// start, before any link is read. Until then nothing is read (links are refused). `parse` also takes it as a
    /// parameter for tests.
    nonisolated(unsafe) public static var reader: any LinkReading = UnavailableLinkReader()

    /// The authority the core shows for a relay (`HttpTransport::relay_authority`): the host, lower-cased, and the port
    /// when it is not the default of the scheme. Nil for a URL without host.
    public static func relayAuthority(of relay: URL) -> String? {
        guard let host = relay.host?.lowercased(), !host.isEmpty else { return nil }
        let bracketed = host.contains(":") ? "[\(host)]" : host
        let defaultPort = relay.scheme?.lowercased() == "https" ? 443 : 80
        guard let port = relay.port, port != defaultPort else { return bracketed }
        return "\(bracketed):\(port)"
    }

    private static let maxParams = 16
    private static let idRegex = try! NSRegularExpression(pattern: "\\A[A-Za-z0-9_-]{1,64}\\z")

    /// Returns nil for anything that is not a recognised, well-formed Tomi link.
    public static func parse(_ url: URL?, reader: any LinkReading = DeepLinkParser.reader) -> DeepLink? {
        parse(url?.absoluteString, reader: reader)
    }

    /// Same as `parse(_: URL?)` for a raw string (what Android's tests feed).
    public static func parse(_ raw: String?, reader: any LinkReading = DeepLinkParser.reader) -> DeepLink? {
        guard let raw, !raw.isBlank, raw.utf16.count <= maxURILength else { return nil }
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let components = URLComponents(string: trimmed) else { return nil }
        switch components.scheme?.lowercased() {
        case "tomi-share": return parseCustomScheme(components, trimmed, reader: reader)
        case "https": return parseAppLink(components, trimmed, reader: reader)
        default: return nil
        }
    }

    private static func parseCustomScheme(_ components: URLComponents, _ raw: String, reader: any LinkReading) -> DeepLink? {
        switch components.host?.lowercased() {
        case "open":
            return .open
        case "send":
            return parseSend(components.percentEncodedQuery)
        case "item":
            var path = components.path
            if path.hasPrefix("/") { path.removeFirst() }
            guard fullMatch(idRegex, path) else { return nil }
            return .item(id: path)
        case "pair":
            // tomi-share://pair#<code> exactly: the code is in the fragment, never in the query (a query reaches logs and
            // servers); no path, port, user or query is tolerated.
            guard components.path.isEmpty, components.port == nil, components.user == nil, components.password == nil,
                  components.percentEncodedQuery == nil
            else { return nil }
            return pairLink(raw, reader: reader)
        default:
            return nil
        }
    }

    private static func parseAppLink(_ components: URLComponents, _ raw: String, reader: any LinkReading) -> DeepLink? {
        // Any relay: the OS delivers an https link only for a domain this build is associated with, and the relay of
        // a link is shown in full for the user to confirm (never trusted).
        let path = components.path
        if path == "/l/pair" { return pairLink(raw, reader: reader) }
        if path == "/create" || path.hasPrefix("/create/") {
            guard raw.utf16.count <= maxPairPayload, let link = reader.readSetupLink(raw) else { return nil }
            return .setup(relayUrl: link.relayUrl, token: link.token)
        }
        return path.hasPrefix("/l/") ? .open : nil
    }

    /// The pairing link, read by the core from the whole text: the code and the relay it names.
    private static func pairLink(_ raw: String, reader: any LinkReading) -> DeepLink? {
        guard raw.utf16.count <= maxPairPayload, let link = reader.readPairLink(raw) else { return nil }
        return .pair(relayUrl: link.relayUrl, code: link.code)
    }

    private static func parseSend(_ rawQuery: String?) -> DeepLink {
        let params = parseQuery(rawQuery)
        var dropped = false

        func bounded(_ name: String, _ max: Int) -> String? {
            guard let value = params[name] else { return nil }
            if value.utf16.count > max {
                dropped = true
                return nil
            }
            return TextSanitizer.clean(value, maxLength: max)
        }

        var url = bounded("url", maxURLParam)
        if let candidate = url, !LinkExtractor.isSingleURL(candidate) {
            dropped = true
            url = nil
        }
        let title = bounded("title", maxTitleParam)
        let text = bounded("text", maxTextParam)
        // Note: there is intentionally no "secret" parameter. Secrets are pasted by hand, never put in a URL.
        return .send(title: title, url: url, text: text, droppedParameters: dropped)
    }

    /// Decodes the query; unknown parameters are kept in the map but ignored by callers. First occurrence wins.
    static func parseQuery(_ rawQuery: String?) -> [String: String] {
        guard let rawQuery, !rawQuery.isEmpty else { return [:] }
        var result: [String: String] = [:]
        for pair in rawQuery.split(separator: "&", omittingEmptySubsequences: false) {
            if pair.isEmpty { continue }
            let key: Substring
            let value: Substring
            if let eq = pair.firstIndex(of: "=") {
                key = pair[..<eq]
                value = pair[pair.index(after: eq)...]
            } else {
                key = pair
                value = ""
            }
            guard let decodedKey = decode(key), let decodedValue = decode(value) else { continue }
            if result[decodedKey] == nil && result.count < maxParams { result[decodedKey] = decodedValue }
        }
        return result
    }

    /// Java `URLDecoder.decode(value, UTF-8)`: `+` is a space, `%XX` a byte, an incomplete or non-hex escape is an error (nil).
    private static func decode(_ value: Substring) -> String? {
        var bytes: [UInt8] = []
        let input = Array(value.utf8)
        var i = 0
        while i < input.count {
            let byte = input[i]
            switch byte {
            case UInt8(ascii: "+"):
                bytes.append(UInt8(ascii: " "))
                i += 1
            case UInt8(ascii: "%"):
                guard i + 2 < input.count, let high = hexValue(input[i + 1]), let low = hexValue(input[i + 2]) else { return nil }
                bytes.append(high << 4 | low)
                i += 3
            default:
                bytes.append(byte)
                i += 1
            }
        }
        return String(decoding: bytes, as: UTF8.self)
    }

    private static func hexValue(_ byte: UInt8) -> UInt8? {
        switch byte {
        case UInt8(ascii: "0")...UInt8(ascii: "9"): byte - UInt8(ascii: "0")
        case UInt8(ascii: "a")...UInt8(ascii: "f"): byte - UInt8(ascii: "a") + 10
        case UInt8(ascii: "A")...UInt8(ascii: "F"): byte - UInt8(ascii: "A") + 10
        default: nil
        }
    }

    private static func fullMatch(_ regex: NSRegularExpression, _ value: String) -> Bool {
        regex.firstMatch(in: value, range: NSRange(location: 0, length: (value as NSString).length)) != nil
    }
}
