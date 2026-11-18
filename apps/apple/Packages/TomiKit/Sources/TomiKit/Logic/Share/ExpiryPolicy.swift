import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/share/ExpiryPolicy.kt.

/// Expiry values offered in the chips of the send screen and of the options sheet.
public enum ExpiryChoice: String, Sendable, Hashable, CaseIterable, Codable {
    case never, hour1, day1, day7, day30, afterFirstRead

    /// Lifetime in seconds; nil for "never" and "after first read" (Kotlin `duration`).
    public var duration: TimeInterval? {
        switch self {
        case .never, .afterFirstRead: nil
        case .hour1: 3600
        case .day1: 86_400
        case .day7: 7 * 86_400
        case .day30: 30 * 86_400
        }
    }
}

public struct ResolvedExpiry: Sendable, Hashable {
    public var expiresAt: Date?
    public var readPolicy: ReadPolicy

    public init(expiresAt: Date?, readPolicy: ReadPolicy) {
        self.expiresAt = expiresAt
        self.readPolicy = readPolicy
    }
}

/// Owner decision: nothing expires unless the user explicitly asks for it, secrets included. The default is
/// `ExpiryChoice.never` for every item; a duration or "after first read" is an opt-in chosen per item, and the app
/// never adds an expiry of its own (no hidden cap, no stricter default for secrets).
public enum ExpiryPolicy {
    /// Default for every item, secret or not.
    public static let defaultChoice: ExpiryChoice = .never

    /// Choices offered, default first: Never, [After first read, hidden content without attachment only], 1 h, 1 d, 7 d, 30 d
    /// (no 5 min, product decision). "After first read" is only offered:
    /// - for a hidden item: for other content, merely opening the card marks it read, which would erase it as soon as it is seen;
    /// - for an item WITHOUT attachment (`hasAttachment` false): "after first read" destroys the item once it is consulted, so
    ///   an attached file would be lost right after "Afficher" (product decision).
    public static func options(secret: Bool, hasAttachment: Bool) -> [ExpiryChoice] {
        var choices: [ExpiryChoice] = [.never]
        if secret && !hasAttachment { choices.append(.afterFirstRead) }
        choices.append(contentsOf: [.hour1, .day1, .day7, .day30])
        return choices
    }

    public static func isAllowed(_ choice: ExpiryChoice, secret: Bool, hasAttachment: Bool) -> Bool {
        options(secret: secret, hasAttachment: hasAttachment).contains(choice)
    }

    /// Called when the "secret" switch or the attachments change. The user's choice is kept when it is still allowed; turning a
    /// secret into plain content, or attaching a file, drops "after first read" back to `defaultChoice`. Turning the switch on
    /// never adds an expiry.
    public static func onContentChanged(current: ExpiryChoice, secret: Bool, hasAttachment: Bool) -> ExpiryChoice {
        isAllowed(current, secret: secret, hasAttachment: hasAttachment) ? current : defaultChoice
    }

    /// A choice that is not allowed for this content falls back to `defaultChoice`, i.e. no expiry.
    public static func resolve(_ choice: ExpiryChoice, secret: Bool, hasAttachment: Bool, now: Date) -> ResolvedExpiry {
        let safe = isAllowed(choice, secret: secret, hasAttachment: hasAttachment) ? choice : defaultChoice
        switch safe {
        case .afterFirstRead:
            // Read once, then gone everywhere (tombstone). No time limit is added on top of what the user chose.
            return ResolvedExpiry(expiresAt: nil, readPolicy: .afterFirstRead)
        default:
            return ResolvedExpiry(expiresAt: safe.duration.map { now.addingTimeInterval($0) }, readPolicy: .normal)
        }
    }
}
