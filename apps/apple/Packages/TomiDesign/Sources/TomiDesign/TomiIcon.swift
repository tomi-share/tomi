import SwiftUI

/// The icons of the app, as SF Symbols. One table for the whole app, built from the charter (§6.2,
/// Material Symbols on Android and Windows) so every screen says the same thing the same way.
///
/// Charter rules kept on Apple platforms (§6.3): no circular-arrow icon (sync, refresh...) next to
/// the logo, no padlock on hidden items (everything is encrypted), filled variants only for a
/// selected state and the unread dot, and the Tomi symbol is never an action icon.
public enum TomiIcon: String, CaseIterable, Sendable {
    // Kinds of content (charter §6.2)
    case link
    case text
    case markdown
    case file
    case image
    case code
    case masked
    case reveal
    // States (charter §6.2)
    case unread
    case sent
    case delivered
    case pending
    case expired
    case error
    case offline
    // Actions
    case search
    case filter
    case devices
    case send
    case pin
    case pinned
    case archive
    case unarchive
    case delete
    case copy
    case share
    case open
    case more
    case close
    case back
    case paste
    case attach
    case scanQR
    case readMore
    // Devices
    case phone
    case computer

    /// SF Symbol name.
    public var systemName: String {
        switch self {
        case .link: "link"
        case .text: "text.alignleft"
        case .markdown: "doc.richtext"
        case .file: "doc"
        case .image: "photo"
        case .code: "chevron.left.forwardslash.chevron.right"
        case .masked: "eye.slash"
        case .reveal: "eye"
        case .unread: "circle.fill"
        case .sent: "checkmark"
        case .delivered: "checkmark.circle"
        case .pending: "clock"
        case .expired: "clock.badge.xmark"
        case .error: "exclamationmark.circle"
        case .offline: "icloud.slash"
        case .search: "magnifyingglass"
        case .filter: "line.3.horizontal.decrease"
        case .devices: "laptopcomputer.and.iphone"
        case .send: "paperplane.fill"
        case .pin: "pin"
        case .pinned: "pin.fill"
        case .archive: "archivebox"
        case .unarchive: "tray.and.arrow.up"
        case .delete: "trash"
        case .copy: "doc.on.doc"
        case .share: "square.and.arrow.up"
        case .open: "arrow.up.right.square"
        case .more: "ellipsis"
        case .close: "xmark"
        case .back: "chevron.left"
        case .paste: "doc.on.clipboard"
        case .attach: "paperclip"
        case .scanQR: "qrcode.viewfinder"
        case .readMore: "chevron.right"
        case .phone: "iphone"
        case .computer: "desktopcomputer"
        }
    }

    /// Default colour of the icon (charter §6.2): muted, except the states that carry a meaning.
    public var tint: Color {
        switch self {
        case .masked: TomiColor.secret
        case .unread: TomiColor.brand
        case .delivered: TomiColor.success
        case .pending, .offline: TomiColor.secondary
        case .error: TomiColor.error
        case .reveal: TomiColor.text
        default: TomiColor.textMuted
        }
    }

    public var image: Image { Image(systemName: systemName) }
}
