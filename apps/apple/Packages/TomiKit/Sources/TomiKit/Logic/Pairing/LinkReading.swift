import Foundation

// The reading of pairing links, creation links and installation tokens belongs to the core (`parse_pair_link`,
// `parse_setup_link`, `is_install_token_shape` of `crates/core/src/relay_url.rs`): one reading for the three platforms
// (spec/pairing-v1.md 4.4 and 10.3). TomiKit has no dependency on the core, so it sees it through this seam;
// `TomiRuntime.CoreLinkReader` is the implementation of the app, tests inject their own.

/// A pairing link, as the core reads it. **Secret**: the code. Prints nothing of it.
public struct ParsedPairLink: Sendable, Hashable {
    /// `https://<authority>` of the link, normalised; nil for the fallback `tomi-share://pair#<code>`. **Not trusted**:
    /// whoever shows the QR code chose it. The screens show it in full and the user confirms it.
    public let relayUrl: String?
    /// The code in its canonical form (`4821-crayon-renard`).
    public let code: String

    public init(relayUrl: String?, code: String) {
        self.relayUrl = relayUrl
        self.code = code
    }
}

/// A space creation link, as the core reads it. **Secret**: the token. Prints nothing of it.
public struct ParsedSetupLink: Sendable, Hashable {
    /// `https://<authority>` of the link, normalised. **Not trusted** (see `ParsedPairLink.relayUrl`).
    public let relayUrl: String
    /// The installation token as written in the link.
    public let token: String

    public init(relayUrl: String, token: String) {
        self.relayUrl = relayUrl
        self.token = token
    }
}

extension ParsedPairLink: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "ParsedPairLink(relay=\(relayUrl ?? "-"), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

extension ParsedSetupLink: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var description: String { "ParsedSetupLink(relay=\(relayUrl), <redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// Reads what a QR code, a pasted text or an opened URL may contain. Pure: no network, nothing stored.
public protocol LinkReading: Sendable {
    /// `https://<relay>/l/pair#<code>` of any valid https relay, or `tomi-share://pair#<code>`; nil for anything else.
    func readPairLink(_ text: String) -> ParsedPairLink?
    /// `https://<relay>/create#<token>` or `https://<relay>/create/<token>`; nil for anything else.
    func readSetupLink(_ text: String) -> ParsedSetupLink?
    /// The text has the shape of an installation token (the relay decides whether it is valid).
    func hasTokenShape(_ text: String) -> Bool
}

/// The reader of a process in which none was installed: reads nothing (links are refused, never guessed).
public struct UnavailableLinkReader: LinkReading {
    public init() {}
    public func readPairLink(_ text: String) -> ParsedPairLink? { nil }
    public func readSetupLink(_ text: String) -> ParsedSetupLink? { nil }
    public func hasTokenShape(_ text: String) -> Bool { false }
}
