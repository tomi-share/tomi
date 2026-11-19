import Foundation

// Notices the core raises about the removal of devices (`spec/api-v1.md` §7.6, §12): a device the relay alone removed
// (`take_relay_revocations`), and a removal whose key rotation keeps failing (`rotation_stalled`).

/// A device the relay removed on its own authority, as the core reports it (once). The name comes from the relay's
/// device list (the relay can rename a device): it is text to show, never trusted for anything else.
public struct RelayRevocation: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    /// Who the relay says removed the device.
    public enum Via: Hashable, Sendable {
        /// The relay's administrator (`admin`).
        case administrator
        /// The device left on its own, as far as the relay says (`self`).
        case device
        /// Any other value the core may report later.
        case unknown

        /// `via` as the core sends it.
        public init(wire: String) {
            switch wire {
            case "admin": self = .administrator
            case "self": self = .device
            default: self = .unknown
            }
        }
    }

    public var deviceId: String
    public var name: String
    public var via: Via

    public init(deviceId: String, name: String, via: Via) {
        self.deviceId = deviceId
        self.name = name
        self.via = via
    }

    /// Key of the notice (one argument: the device name).
    public var messageKey: String {
        switch via {
        case .administrator: "ios_relay_revoked_admin"
        case .device: "ios_relay_revoked_self"
        case .unknown: "ios_relay_revoked_other"
        }
    }

    // A device name is personal: it never reaches a log or a dump.
    public var description: String { "RelayRevocation(redacted)" }
    public var debugDescription: String { description }
}

public enum RelayNotices {
    /// `pending` followed by what `taken` adds. A device is told once: an entry whose device is already pending, or
    /// repeated in `taken`, is dropped.
    public static func merged(_ pending: [RelayRevocation], with taken: [RelayRevocation]) -> [RelayRevocation] {
        var seen = Set(pending.map(\.deviceId))
        var result = pending
        for notice in taken where seen.insert(notice.deviceId).inserted {
            result.append(notice)
        }
        return result
    }
}
