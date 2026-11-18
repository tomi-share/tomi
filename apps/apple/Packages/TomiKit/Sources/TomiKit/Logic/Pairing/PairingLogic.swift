import Foundation

// Ports of apps/android/app/src/main/java/eu/shaffe/tomi/ui/pairing/{InstallTokenQr,Countdown}.kt,
// core/{DeviceNames,PairingErrors}.kt and the failure messages of ui/pairing/{PairingViewModel,CreateSpaceViewModel}.kt.

/// Light shape check of a scanned QR code before it is taken for an installation token (the QR of the relay
/// administration encodes the bare token, not a link). The core and the relay decide whether the token is valid.
///
/// Shape: once dashes and whitespace are removed and the text is lower-cased, 24 to 40 characters of the relay's base32
/// alphabet (digits and lower-case letters without i, l, o, u). A token is 32 of them, in 8 groups of 4.
public enum InstallTokenQr {
    private static let alphabet = Set("0123456789abcdefghjkmnpqrstvwxyz")
    /// Longest raw content looked at: a token with its dashes is 39 characters, the rest is a margin for spaces.
    private static let maxRaw = 64
    /// Longest token kept in the field.
    public static let maxInput = 128

    /// The scanned text trimmed, or nil when it does not look like an installation token. Never logs the content.
    public static func parse(_ text: String) -> String? {
        let raw = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty, raw.utf16.count <= maxRaw else { return nil }
        let normalized = raw.filter { $0 != "-" && !$0.isWhitespace }.lowercased()
        guard (24...40).contains(normalized.count), normalized.allSatisfy(alphabet.contains) else { return nil }
        return raw
    }
}

/// The name this device announces to the others when it joins a space (1 to 64 characters, no blank at either end, no
/// control character). The user can rename the device afterwards.
public enum DeviceNames {
    public static let fallback = "iPhone"
    private static let maxCharacters = 64

    /// `raw` with control characters and runs of blanks turned into one space, trimmed, cut at 64 characters (Unicode
    /// scalars, as the core counts). Nil when nothing is left.
    public static func clean(_ raw: String?) -> String? {
        guard let raw else { return nil }
        var spaced = ""
        var lastWasBlank = false
        for scalar in raw.unicodeScalars {
            let blank = CharacterSet.whitespacesAndNewlines.contains(scalar) || CharacterSet.controlCharacters.contains(scalar)
            if blank {
                if !lastWasBlank { spaced.unicodeScalars.append(" ") }
            } else {
                spaced.unicodeScalars.append(scalar)
            }
            lastWasBlank = blank
        }
        var cleaned = spaced.trimmingCharacters(in: .whitespaces)
        if cleaned.unicodeScalars.count > maxCharacters {
            cleaned = String(String.UnicodeScalarView(cleaned.unicodeScalars.prefix(maxCharacters)))
                .trimmingCharacters(in: .whitespaces)
        }
        return cleaned.isEmpty ? nil : cleaned
    }

    /// The first of `candidates` the core accepts once cleaned (`isValid` is the rule of the core), `fallback` otherwise.
    public static func choose(_ candidates: [String?], isValid: (String) -> Bool) -> String {
        candidates.lazy.compactMap(clean).first(where: isValid) ?? fallback
    }
}

/// The countdown of a pairing code, "1:48".
public enum Countdown {
    /// Minutes and seconds, rounded UP to the second: it reads 0:01 until the very end, never 0:00 while time is left.
    public static func format(remainingMs: Int64) -> String {
        let seconds = (max(remainingMs, 0) + 999) / 1000
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}

/// What the core throws during a pairing, as the screens understand it (spec/pairing-v1.md 11.5).
public enum PairingErrors {
    /// Delays between the tries of a blocking session call after a transient error.
    public static let retryDelaysMs: [Int64] = [1_000, 2_000, 4_000]

    public static func failure(of error: TomiCoreError) -> PairingFailure {
        switch error.kind {
        case let .crypto(code):
            switch code {
            case "pairing_failed": return .wrongCode
            case "pairing_expired": return .expired
            case "pairing_taken": return .taken
            case "pairing_cancelled": return .cancelled
            case "pairing_unsupported": return .updateRequired
            case "relay_not_configured": return .relayNotConfigured
            default: return .unexpected
            }
        case .network: return .network
        case let .relay(status, code): return relayFailure(status: status, code: code)
        // The reason is the Debug form of the core's `Blocked` ("UpdateRequired { .. }", "Revoked"...).
        case .blocked: return error.reason.hasPrefix("UpdateRequired") ? .updateRequired : .blocked
        case .invalidInput, .storage, .internal: return .unexpected
        }
    }

    private static func relayFailure(status: Int, code: String) -> PairingFailure {
        if status == 429 || status >= 500 { return .network }
        switch code {
        case "too_many_devices": return .spaceFull
        case "channel_limit": return .anotherPairingOpen
        case "install_token_invalid": return .installTokenInvalid
        case "space_exists": return .spaceExists
        case "update_required", "format_unsupported": return .updateRequired
        default: return .unexpected
        }
    }

    /// Errors after which a session is left as it was ("call again"): network, storage, relay 429 or 5xx.
    public static func isTransient(_ error: TomiCoreError) -> Bool {
        switch error.kind {
        case .network, .storage: return true
        case let .relay(status, _): return status == 429 || status >= 500
        default: return false
        }
    }

    /// The message of a failure of the new device's pairing (resource key).
    public static func joinMessageKey(_ reason: PairingFailure) -> String {
        switch reason {
        case .invalidCode: "pair_error_invalid"
        case .wrongCode: "pair_error_wrong_code"
        case .expired: "pair_error_expired"
        case .network: "pair_error_network"
        case .taken: "pair_error_taken"
        case .interrupted: "pair_error_interrupted"
        case .cancelled: "pair_error_cancelled"
        case .updateRequired: "pair_error_update"
        case .blocked: "pair_error_blocked"
        case .spaceFull: "pair_error_space_full"
        case .unsupported: "ios_pair_error_unsupported"
        case .relayNotConfigured: "pair_error_relay_required"
        case .invalidRelay: "pair_error_relay_invalid"
        case .anotherPairingOpen, .installTokenInvalid, .spaceExists, .unexpected: "pair_error_unexpected"
        }
    }

    /// The message of a failure of an invitation, on the inviting device (resource key).
    public static func inviteMessageKey(_ reason: PairingFailure) -> String {
        switch reason {
        case .expired: "invite_error_expired"
        case .cancelled: "invite_error_cancelled"
        case .network, .interrupted: "pair_error_network"
        case .spaceFull: "pair_error_space_full"
        case .anotherPairingOpen: "invite_error_busy"
        case .updateRequired: "pair_error_update"
        case .blocked: "pair_error_blocked"
        case .unsupported: "ios_pair_error_unsupported"
        case .invalidCode, .wrongCode, .taken, .installTokenInvalid, .spaceExists, .relayNotConfigured, .invalidRelay,
             .unexpected: "invite_error_failed"
        }
    }

    /// The message of a failure of the creation of the space (resource key).
    public static func createMessageKey(_ reason: PairingFailure) -> String {
        switch reason {
        case .installTokenInvalid: "create_error_token"
        case .spaceExists: "create_error_space_exists"
        case .network, .interrupted: "pair_error_network"
        case .updateRequired: "pair_error_update"
        case .blocked: "pair_error_blocked"
        case .unsupported: "ios_pair_error_unsupported"
        case .relayNotConfigured: "pair_error_relay_required"
        case .invalidRelay: "pair_error_relay_invalid"
        case .invalidCode, .wrongCode, .expired, .taken, .cancelled, .spaceFull, .anotherPairingOpen, .unexpected:
            "pair_error_unexpected"
        }
    }
}
