import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/ItemSharing.kt.

/// What goes into the system share sheet when the user shares an item to another app.
/// `description` never prints the content: a payload can hold a hidden item.
public struct SharePayload: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var text: String
    public var subject: String?

    public init(text: String, subject: String?) {
        self.text = text
        self.subject = subject
    }

    public var description: String { "SharePayload(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// Result of `ItemSharing.prepare`.
public enum ShareOutcome: Hashable, Sendable {
    /// The share sheet may be opened with the payload.
    case ready(SharePayload)
    /// The item has no text or link to share (unreadable item, or files only).
    case nothingToShare
    /// A hidden item and no screen lock or biometrics is set up: it stays locked.
    case biometricUnavailable
    /// A hidden item and the user cancelled or failed the authentication: nothing is shared.
    case notAuthenticated
}

/// Decides what "Share" does, in one place that does not depend on UI types.
///
/// Rule (product decision): a normal item is shared directly; a hidden ("Masqué") item can be shared too, but its content is
/// NEVER read into a payload before `authenticate` returned `.success`. The payload is built after the
/// authentication, handed to the caller and not kept anywhere here.
public enum ItemSharing {
    /// True if the "Share" action should be offered for `item` (does not read the content of a hidden item into a payload).
    public static func canShare(_ item: TomiItem) -> Bool { item.isReadable && hasShareableContent(item) }

    public static func prepare(_ item: TomiItem, authenticate: () async -> AuthOutcome) async -> ShareOutcome {
        if !canShare(item) { return .nothingToShare }
        if item.secret {
            switch await authenticate() {
            case .success: break
            case .unavailable: return .biometricUnavailable
            case .cancelled, .failed: return .notAuthenticated
            }
        }
        guard let payload = payload(for: item) else { return .nothingToShare }
        return .ready(payload)
    }

    /// Text of the item as plain text (the displayed text, then the link), with the title as subject.
    /// Attachments are not shared here. Callers must have authenticated first when `item.secret` is true; use `prepare`.
    static func payload(for item: TomiItem) -> SharePayload? {
        if !item.isReadable { return nil }
        let parts = [item.text?.plain, item.url].compactMap { value -> String? in
            guard let value, !value.isBlank else { return nil }
            return value
        }
        let title = item.title.flatMap { $0.isBlank ? nil : $0 }
        var body = parts.joined(separator: "\n")
        if body.isEmpty {
            guard let title else { return nil }
            body = title
        }
        return SharePayload(text: body, subject: parts.isEmpty ? nil : title)
    }

    private static func hasShareableContent(_ item: TomiItem) -> Bool {
        !(item.text?.plain ?? "").isBlank || !(item.url ?? "").isBlank || !(item.title ?? "").isBlank
    }
}
