import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/notify/NotificationLogic.kt.

/// What a tap on a notification does,.
public enum NotificationTapAction: Sendable, Hashable, CaseIterable {
    /// Link: open in the browser.
    case openLink
    /// Short plain text: copy it.
    case copyText
    /// Long or rich text: open the card.
    case openCard
    /// Image: open the preview (card with the large thumbnail).
    case previewImage
    /// Other file: open with the default app.
    case openFile
    /// Secret: biometric prompt, then copy.
    case authenticateAndCopy

    public static let shortTextMaxChars = 140

    public static func forItem(_ item: TomiItem) -> NotificationTapAction {
        switch item.kind {
        case .secret: return .authenticateAndCopy
        case .image: return .previewImage
        // A code file is read in Tomi's own code screen, not handed to another app: open the card.
        case .file: return item.attachments.contains(where: CodeFiles.isCodeFile) ? .openCard : .openFile
        case .link: return .openLink
        case .richText, .unreadable: return .openCard
        case .text:
            let plain = item.text?.plain ?? ""
            // Scalars, not Characters: "\r\n" is a single Character that is not "\n".
            // UTF-16 length, like Kotlin (an emoji or a combining mark is not "short" for free).
            return plain.utf16.count <= shortTextMaxChars && !plain.unicodeScalars.contains("\n") ? .copyText : .openCard
        }
    }
}

/// Groups notifications arriving less than `windowMs` apart. Feed it every new item; it tells whether the
/// notification should be a single one or a summary, and which item ids the summary replaces.
public final class NotificationBatcher: @unchecked Sendable {
    public static let defaultWindowMs: Int64 = 10_000

    public enum Decision: Hashable, Sendable {
        case single
        case summary(itemIds: [String])
    }

    private let windowMs: Int64
    private let lock = NSLock()
    private var lastArrivalMs: Int64?
    private var batch = [String]()

    public init(windowMs: Int64 = NotificationBatcher.defaultWindowMs) {
        self.windowMs = windowMs
    }

    public func onArrival(itemId: String, nowMs: Int64) -> Decision {
        lock.withLock {
            if let last = lastArrivalMs {
                if nowMs - last >= windowMs { batch.removeAll() }
            } else {
                batch.removeAll()
            }
            batch.append(itemId)
            lastArrivalMs = nowMs
            return batch.count == 1 ? .single : .summary(itemIds: batch)
        }
    }
}

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/notify/{IncomingNotifier,IncomingNotifications}.kt (what to
// notify and what a notification says; posting it is the app's).

/// Turns "a new unread item arrived from another device" into notifications to show, unless the app is on screen, and
/// tells which notifications to take down (items read elsewhere, deleted or expired). The first reading is the existing
/// history: it notifies nothing.
public struct IncomingNotifierState: Sendable {
    private var known: Set<String>?
    private var notified: Set<String> = []

    public init() {}

    public struct Update: Equatable, Sendable {
        /// Oldest first.
        public var show: [TomiItem] = []
        public var cancel: [String] = []
    }

    /// `importing`: the history imported right after the pairing is arriving (`PairingGrace`); it is not news.
    public mutating func update(items: [TomiItem], localDeviceId: String?, appVisible: Bool, importing: Bool = false) -> Update {
        var update = Update()
        let previous = known
        known = Set(items.map(\.id))
        let unread = Set(items.filter { !$0.read }.map(\.id))
        for id in notified.subtracting(unread).sorted() {
            update.cancel.append(id)
            notified.remove(id)
        }
        guard let previous, !importing else { return update }
        let outgoing = { (item: TomiItem) in localDeviceId.map(item.isOutgoing(localDeviceId:)) ?? false }
        let fresh = items.filter { !previous.contains($0.id) && !outgoing($0) && !$0.read }
        guard !fresh.isEmpty, !appVisible else { return update }
        for item in fresh.reversed() {
            notified.insert(item.id)
            update.show.append(item)
        }
        return update
    }

    /// The device left its space: nothing is tracked any more.
    public mutating func reset() {
        known = nil
        notified = []
    }
}

/// The history a device imports right after it joined a space (the user said yes at pairing) arrives as a burst of
/// items that are not news (Android `PairingGrace`). Tells, for each change of the feed, whether the device is still
/// within `grace` of the moment it became paired while the app was running; those items are not notified. A device
/// that was already paired when the app started never has a grace period.
public struct PairingGrace: Sendable {
    /// Long enough for an import over a slow network, short enough that a real item sent minutes later still notifies.
    public static let defaultGrace: TimeInterval = 2 * 60

    private let grace: TimeInterval
    private var wasPaired: Bool?
    private var until = Date.distantPast
    private var active = false

    public init(grace: TimeInterval = PairingGrace.defaultGrace) {
        self.grace = grace
    }

    public mutating func isImporting(paired: Bool, now: Date = Date()) -> Bool {
        let before = wasPaired
        wasPaired = paired
        if before == false && paired {
            active = true
            until = now.addingTimeInterval(grace)
        }
        if !paired { active = false }
        if active && now >= until { active = false }
        return active
    }
}

/// What the notification of a received item says. Hard rule: nothing of a hidden item's content, and since iOS cannot
/// keep one notification off the lock screen (Android `VISIBILITY_SECRET`), a hidden or unreadable item gets the
/// generic public title, without even the device name.
public struct IncomingNotificationContent: Equatable, Sendable {
    /// The group of the notification (Android channels): links, text, files, secrets.
    public enum Group: String, Sendable { case links, text, files, secrets }

    /// The second button of the notification.
    public enum Secondary: String, Sendable { case copy, view }

    public enum Body: Equatable, Sendable {
        case key(String)
        case text(String)
    }

    public let group: Group
    /// Text key, with the source device name as its argument (nil: no argument).
    public let titleKey: String
    public let titleArgument: String?
    public let body: Body
    public let secondary: Secondary?

    public static let maxBody = 120

    public static func forItem(_ item: TomiItem) -> IncomingNotificationContent {
        let from = item.sourceDevice.name
        switch item.kind {
        case .secret:
            return .init(group: .secrets, titleKey: "notif_public_title", titleArgument: nil, body: .key("notif_body_secret"), secondary: nil)
        case .unreadable:
            // An unreadable item may well be a secret: treated like one.
            return .init(group: .secrets, titleKey: "notif_public_title", titleArgument: nil, body: .key("notif_body_unreadable"), secondary: nil)
        case .link:
            let body = item.title ?? item.url.flatMap { URL(string: $0)?.host }
            return .init(group: .links, titleKey: "notif_title_link", titleArgument: from,
                         body: body.map { .text(String($0.prefix(maxBody))) } ?? .key("notif_body_generic"),
                         secondary: secondary(item))
        case .image, .file:
            let names = item.attachments.map(\.name).joined(separator: ", ")
            return .init(group: .files, titleKey: item.kind == .image ? "notif_title_image" : "notif_title_file", titleArgument: from,
                         body: names.isEmpty ? .key("notif_body_generic") : .text(String(names.prefix(maxBody))),
                         secondary: secondary(item))
        case .text, .richText:
            let plain = item.text?.plain
            return .init(group: .text, titleKey: "notif_title_text", titleArgument: from,
                         body: plain.map { .text(String($0.prefix(maxBody))) } ?? .key("notif_body_generic"),
                         secondary: secondary(item))
        }
    }

    private static func secondary(_ item: TomiItem) -> Secondary? {
        guard item.isReadable else { return nil }
        switch NotificationTapAction.forItem(item) {
        case .openLink, .openCard: return .copy
        case .copyText, .previewImage, .openFile: return .view
        case .authenticateAndCopy: return nil
        }
    }
}
