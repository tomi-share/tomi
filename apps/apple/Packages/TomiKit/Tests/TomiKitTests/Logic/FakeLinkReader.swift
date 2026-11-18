import Foundation
@testable import TomiKit

/// A stand-in for the core's reading of links (`TomiRuntime.CoreLinkReader`), which tests cannot call (the core is an
/// iOS XCFramework). It follows the same rules on the cases the tests use: `https://<relay>/l/pair#<code>` of a valid
/// https relay, `tomi-share://pair#<code>`, `https://<relay>/create#<token>` or `/create/<token>`, and a token of 24 to 40
/// characters of the relay's base32 alphabet. The relay is returned normalised (`https://host[:port]`, lower case,
/// default port dropped).
struct FakeLinkReader: LinkReading {
    private static let alphabet = Set("0123456789abcdefghjkmnpqrstvwxyz")

    func readPairLink(_ text: String) -> ParsedPairLink? {
        // Newer Foundations percent-encode a space instead of refusing it; the core refuses a link with whitespace.
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).contains(where: \.isWhitespace) else { return nil }
        guard let components = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              let fragment = components.fragment, !fragment.isEmpty
        else { return nil }
        switch components.scheme?.lowercased() {
        case "tomi-share":
            guard components.host?.lowercased() == "pair" else { return nil }
            return ParsedPairLink(relayUrl: nil, code: fragment)
        case "https":
            guard components.path == "/l/pair", let relay = Self.origin(components) else { return nil }
            return ParsedPairLink(relayUrl: relay, code: fragment)
        default:
            return nil
        }
    }

    func readSetupLink(_ text: String) -> ParsedSetupLink? {
        guard let components = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              components.scheme?.lowercased() == "https", let relay = Self.origin(components)
        else { return nil }
        let token: String
        if components.path == "/create", let fragment = components.fragment {
            token = fragment
        } else if components.path.hasPrefix("/create/"), components.fragment == nil {
            token = String(components.path.dropFirst("/create/".count))
        } else {
            return nil
        }
        guard hasTokenShape(token) else { return nil }
        return ParsedSetupLink(relayUrl: relay, token: token)
    }

    func hasTokenShape(_ text: String) -> Bool {
        let normalized = text.trimmingCharacters(in: .whitespacesAndNewlines).filter { $0 != "-" && !$0.isWhitespace }.lowercased()
        return (24...40).contains(normalized.count) && normalized.allSatisfy(Self.alphabet.contains)
    }

    /// `https://host[:port]` of a valid relay: an ASCII host, no user information, a valid port.
    private static func origin(_ components: URLComponents) -> String? {
        guard let host = components.host?.lowercased(), !host.isEmpty, host.unicodeScalars.allSatisfy(\.isASCII),
              components.user == nil, components.password == nil
        else { return nil }
        if let port = components.port {
            guard (1...65535).contains(port) else { return nil }
            return port == 443 ? "https://\(host)" : "https://\(host):\(port)"
        }
        return "https://\(host)"
    }
}
