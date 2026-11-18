import Foundation

// Port of `PairingInput` in apps/android/app/src/main/java/eu/shaffe/tomi/ui/pairing/PairingViewModel.kt.

/// What the pairing screen accepts from the user: a short code typed by hand, or a scanned QR code.
public enum PairingInput {
    /// Longest code kept in the field; the core decides whether a well-formed code is right.
    public static let maxCodeInput = 64

    /// A typed code such as `7-crayon-renard`. The core reads it (spec/pairing-v1.md 4.4: separators, capitals,
    /// accents, the first four letters of a word), and a code it cannot read costs nothing: only what is clearly not a
    /// code is refused here (empty, too long, a control character).
    public static func isValidCode(_ raw: String) -> Bool {
        let code = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return !code.isEmpty && code.utf16.count <= maxCodeInput
            && !code.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) }
    }

    /// A scanned QR code must be a pairing link (`https://<relay>/l/pair#<code>` of any relay, or `tomi-share://pair#<code>`),
    /// read by the core, nothing else: any other content (a web address, a payment code) is refused before it reaches
    /// the core. The relay of the link is not trusted: the screens show it in full for the user to confirm.
    public static func linkFromQR(_ raw: String, reader: any LinkReading = DeepLinkParser.reader) -> ParsedPairLink? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf16.count <= DeepLinkParser.maxPairPayload else { return nil }
        return reader.readPairLink(text)
    }

    /// The 6-digit verification code in two groups of three, as it is read aloud ("482 193").
    public static func groupedVerificationCode(_ code: String) -> String {
        guard code.count > 3 else { return code }
        let middle = code.index(code.startIndex, offsetBy: 3)
        return "\(code[..<middle]) \(code[middle...])"
    }
}

/// What the space creation screen accepts in its token field or from a scanned QR code: a creation link
/// (`https://<relay>/create#<token>`, which also names the relay) or the bare installation token (the QR code of the relay
/// administration). Both are read by the core. The token is a secret: nothing here prints it.
public enum CreateInput {
    /// Longest text read; a link is longer than a token.
    public static let maxRaw = 512

    public enum Reading: Equatable, Sendable, CustomStringConvertible {
        /// A creation link: its relay (not trusted, shown in full) and its token.
        case link(relayUrl: String, token: String)
        /// A bare token, trimmed.
        case token(String)

        public var description: String {
            switch self {
            case let .link(relayUrl, _): "link(relay=\(relayUrl), <redacted>)"
            case .token: "token(<redacted>)"
            }
        }
    }

    /// Nil for anything that is neither: a web address, a pairing link, a payment code.
    public static func read(_ raw: String, reader: any LinkReading = DeepLinkParser.reader) -> Reading? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf16.count <= maxRaw else { return nil }
        if let link = reader.readSetupLink(text) { return .link(relayUrl: link.relayUrl, token: link.token) }
        if reader.hasTokenShape(text) { return .token(text) }
        return nil
    }
}
