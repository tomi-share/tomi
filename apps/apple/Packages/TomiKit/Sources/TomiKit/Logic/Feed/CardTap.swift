import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/CardTap.kt.
// Kotlin's `CardTapAction` keeps its name (`NotificationTapAction` is the notification enum of NotificationLogic.swift).

/// What a tap on a feed card does. Pure data: the screen performs it.
public enum CardTapAction: Sendable, Hashable, CaseIterable {
    /// Open the link in the browser (same action as "Ouvrir" in the menu).
    case openLink
    /// Show a hidden item (same action and same biometric gate as the "Afficher" button).
    case reveal
    /// Hide an item that is shown in clear again (same as the "Masquer" button).
    case hide
    /// Open the single (non-image, non-code) attachment with the system viewer; falls back to `openDetail` when it is not available.
    case openAttachment
    /// Show the single code file in Tomi's own code screen (search; behind the biometric gate for a hidden item).
    case openCodeFile
    /// Show the single image full screen in Tomi's own viewer (zoom, EXIF orientation; behind the biometric gate for a hidden item).
    case openImage
    /// Open the detail screen: the card is truncated, or nothing more natural exists.
    case openDetail
    /// Nothing useful to do (the card already shows everything and has no natural action).
    case none
}

/// Callbacks of a card used by `handleTap` (the copy callback of the Kotlin `CardActions` is not part of the tap logic).
public struct CardActions {
    public var onOpenCodeFile: (Int) -> Void
    public var onCardOpened: () -> Void
    public var onOpenDetail: () -> Void
    public var onOpenAttachment: () -> Void
    public var onOpenImage: (Int) -> Void
    public var onReveal: () -> Void
    public var onHide: () -> Void
    public var onOpenLink: () -> Void

    public init(
        onOpenCodeFile: @escaping (Int) -> Void, onCardOpened: @escaping () -> Void, onOpenDetail: @escaping () -> Void,
        onOpenAttachment: @escaping () -> Void, onOpenImage: @escaping (Int) -> Void, onReveal: @escaping () -> Void,
        onHide: @escaping () -> Void, onOpenLink: @escaping () -> Void
    ) {
        self.onOpenCodeFile = onOpenCodeFile
        self.onCardOpened = onCardOpened
        self.onOpenDetail = onOpenDetail
        self.onOpenAttachment = onOpenAttachment
        self.onOpenImage = onOpenImage
        self.onReveal = onReveal
        self.onHide = onHide
        self.onOpenLink = onOpenLink
    }

    /// Performs a tap with the callbacks of the card. `onCardOpened` always runs first, whatever the action,
    /// so that tapping an unread item marks it read as before (the view model skips hidden, read, own and unreadable items).
    public func handleTap(_ tap: CardTapAction) {
        onCardOpened()
        switch tap {
        case .openLink: onOpenLink()
        case .reveal: onReveal()
        case .hide: onHide()
        case .openAttachment: onOpenAttachment()
        // Only chosen for an item whose single attachment is a code file (index 0), see `CardTap.actionFor`.
        case .openCodeFile: onOpenCodeFile(0)
        // Only chosen for an item whose single attachment is an image (index 0), see `CardTap.actionFor`.
        case .openImage: onOpenImage(0)
        case .openDetail: onOpenDetail()
        case .none: break
        }
    }
}

/// What the card measured about its own rendering: whether a part of the content is cut by the card limits
/// (title 2 lines, text 4 lines, revealed secret 6 lines, Markdown / HTML 220 pt).
public struct CardOverflow: Hashable, Sendable {
    /// The title is ellipsized.
    public var title: Bool
    /// The text (plain, Markdown or HTML, or the revealed secret) is cut.
    public var body: Bool

    public init(title: Bool = false, body: Bool = false) {
        self.title = title
        self.body = body
    }

    public var any: Bool { title || body }
}

/// Tap rules of the feed cards. The detail screen is only the target of a tap when it brings something: reading a
/// truncated content. Everything else is available on the card itself, so a tap does the natural action of the type.
///
/// | Item                                   | Tap                                                       |
/// |----------------------------------------|-----------------------------------------------------------|
/// | Link (valid http/https URL)            | `.openLink`                                               |
/// | Link whose text is cut, or bad URL     | `.openDetail`                                             |
/// | Hidden item                            | `.reveal`                                                 |
/// | Revealed item, content complete        | `.hide`                                                   |
/// | Revealed item, content cut             | `.openDetail`                                             |
/// | Short text / rich text, nothing cut    | `.none` (marks it read; copy = the icon)                  |
/// | Text, rich text or title cut           | `.openDetail` (also "Lire la suite")                      |
/// | Image, exactly one                     | `.openImage` (internal full-screen viewer)                |
/// | Code file, exactly one                 | `.openCodeFile` (internal code screen)                    |
/// | Other file, exactly one                | `.openAttachment` (system viewer)                         |
/// | Files or images, several               | `.openDetail` (each row opens its viewer)                 |
/// | Unreadable                             | `.none` (the card already explains why)                   |
/// | Title only, nothing cut                | `.none`                                                   |
public enum CardTap {
    public static func actionFor(_ item: TomiItem, revealed: Bool, overflow: CardOverflow = CardOverflow()) -> CardTapAction {
        if !item.isReadable { return .none }
        if item.secret {
            if !revealed { return .reveal }
            return overflow.any ? .openDetail : .hide
        }
        if needsDetail(item, overflow) { return .openDetail }
        switch item.kind {
        case .link:
            if let url = item.url, LinkExtractor.isValidHTTPURL(url) { return .openLink }
            return .openDetail
        case .image:
            return item.attachments.count == 1 ? .openImage : .openDetail
        case .file:
            if item.attachments.count != 1 { return .openDetail }
            return CodeFiles.isCodeFile(item.attachments[0]) ? .openCodeFile : .openAttachment
        // A tap on a text never copies: the text is selectable and the card has a copy icon. It only marks the item read.
        case .text, .richText:
            return .none
        // Handled above (not readable / secret); kept explicit so that a new kind forces a decision here.
        case .secret, .unreadable:
            return .none
        }
    }

    /// The explicit copy icon of a text or link card (same action as "Copier" of the menu: the text, else the link). Never for a hidden
    /// item, whose copy is gated.
    public static func showsCopyButton(_ item: TomiItem) -> Bool { item.isReadable && !item.secret && (item.text != nil || item.url != nil) }

    /// Whether the card shows a "Lire la suite" affordance. True exactly when the tap opens the detail because the
    /// content is cut (not for the fallbacks "bad URL" or "several attachments").
    public static func showsReadMore(_ item: TomiItem, revealed: Bool, overflow: CardOverflow) -> Bool {
        item.isReadable && (!item.secret || revealed) && needsDetail(item, overflow)
    }

    /// A cut content has to be read in the detail. Exception: a link keeps its natural action even when its title
    /// is ellipsized (a page title is routinely longer than two lines and the URL is what matters); only a cut text
    /// body sends a link to the detail.
    private static func needsDetail(_ item: TomiItem, _ overflow: CardOverflow) -> Bool {
        item.kind == .link ? overflow.body : overflow.any
    }
}

/// What a tap on one attachment row of a card does. Each row routes by its own type, so a mixed item (images + code) works row by row.
public enum AttachmentRowAction: Sendable, Hashable {
    /// Tomi's internal image viewer.
    case openImage
    /// Tomi's code screen.
    case openCodeFile
    /// Inert row (other files are opened with a tap on the card when alone, or from the detail).
    case none

    /// `imageOpenable` / `codeOpenable`: the screen gave a callback for that type.
    public static func of(_ attachment: Attachment, imageOpenable: Bool, codeOpenable: Bool) -> AttachmentRowAction {
        if attachment.isImage { return imageOpenable ? .openImage : .none }
        if CodeFiles.isCodeFile(attachment) { return codeOpenable ? .openCodeFile : .none }
        return .none
    }
}
