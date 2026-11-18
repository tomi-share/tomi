import Foundation

// Port of `RelayAddress` / `RelayChoice` in apps/android/app/src/main/java/eu/shaffe/tomi/core/PairLinks.kt.

/// The "Relais" field of the pairing and creation screens (spec/pairing-v1.md 4.4 rule 6, 10.3), and how a relay is
/// shown. The judge of what a valid relay is stays the core: the text becomes a pairing link on that relay and goes
/// through the shared reading, so the app and the core never disagree.
public enum RelayAddress {
    /// Longest text looked at: a host name is at most 253 characters, plus the scheme, a port and a margin.
    public static let maxInput = 300

    /// A pairing code that is never used: it only lets the core's link reading judge a relay address.
    private static let probeCode = "4821-crayon-renard"

    /// The relay typed in a field as the core reads it (`https://host[:port]`, lower-case host), or nil when it is not a
    /// valid relay. The scheme may be left out ("relay.example.org"). Only https is accepted, and no user information,
    /// path, query or fragment. Nothing is sent and nothing is stored.
    public static func canonical(_ raw: String, reader: any LinkReading = DeepLinkParser.reader) -> String? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text.utf16.count <= maxInput else { return nil }
        let refused = text.unicodeScalars.contains {
            CharacterSet.controlCharacters.contains($0) || CharacterSet.whitespacesAndNewlines.contains($0)
                || $0 == "#" || $0 == "?"
        }
        guard !refused else { return nil }
        var base = text.contains("://") ? text : "https://" + text
        guard base.lowercased().hasPrefix("https://") else { return nil }
        while base.hasSuffix("/") { base.removeLast() }
        return reader.readPairLink(base + "/l/pair#" + probeCode)?.relayUrl
    }

    /// The authority (`host[:port]`, lower case, default port dropped) of a relay URL, to show in full; nil when the
    /// URL has no host.
    public static func authority(of relayUrl: String) -> String? {
        URL(string: relayUrl.trimmingCharacters(in: .whitespacesAndNewlines)).flatMap(DeepLinkParser.relayAuthority(of:))
    }

    /// The relay a screen is going to use, from the field and from the relay the device already has.
    public enum Choice: Equatable, Sendable {
        /// The field is empty: the relay of the device, which the core uses when none is passed.
        case device(authority: String)
        /// The relay of the field (or of the link that filled it).
        case typed(url: String, authority: String)
        /// The field is empty and the device has no relay: the user must give one.
        case missing
        /// The field holds something that is not a valid relay address.
        case invalid

        /// What to pass to the core (`relayUrl`): only a relay typed or read, never the device's own.
        public var urlToPass: String? {
            if case let .typed(url, _) = self { return url }
            return nil
        }

        /// The authority the user is shown in full before anything is sent to that relay.
        public var authority: String? {
            switch self {
            case let .device(authority), let .typed(_, authority): authority
            case .missing, .invalid: nil
            }
        }

        /// The text key of the error, nil when the choice is usable.
        public var errorKey: String? {
            switch self {
            case .missing: "pair_error_relay_required"
            case .invalid: "pair_error_relay_invalid"
            case .device, .typed: nil
            }
        }
    }

    /// `field`: what is in the "Relais" field; `deviceRelay`: the URL of the relay the device already has, if any.
    public static func choose(field: String, deviceRelay: String?, reader: any LinkReading = DeepLinkParser.reader) -> Choice {
        if !field.isBlank {
            guard let url = canonical(field, reader: reader) else { return .invalid }
            return .typed(url: url, authority: authority(of: url) ?? url)
        }
        guard let deviceRelay, let authority = authority(of: deviceRelay) else { return .missing }
        return .device(authority: authority)
    }
}
