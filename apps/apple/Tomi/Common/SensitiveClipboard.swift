#if canImport(UIKit)
import UIKit
#else
import AppKit
#endif
import UniformTypeIdentifiers

/// Writes to the clipboard (Android `SensitiveClipboard`). A hidden item stays on this device only
/// (never Universal Clipboard) and leaves the clipboard after 30 s. PR-7 adds the check at the next
/// foreground.
@MainActor
enum SensitiveClipboard {
    static let clearDelay: TimeInterval = 30
    /// Marks a secret copy (the convention clipboard managers honour): it is also how leaving the space finds a copy
    /// made by Tomi.
    static let concealedType = "org.nspasteboard.ConcealedType"
    /// Tomi's own mark on a secret copy (password managers set the concealed type too: never theirs).
    static let tomiSecretType = "eu.shaffe.tomi.secret"

    #if os(macOS)
    /// Marks a copy that should not be kept (clipboard histories honour it, nspasteboard.org).
    static let transientType = "org.nspasteboard.TransientType"

    /// The clearing of the last secret copy, waiting for its 30 s (macOS has no expiry of its own).
    private static var pendingClear: Task<Void, Never>?

    /// macOS: this Mac only for a secret (`currentHostOnly`: never Universal Clipboard), with the marks clipboard
    /// managers honour, and cleared after 30 s only if the clipboard still holds this copy (its change count).
    static func copy(plain: String, html: String? = nil, sensitive: Bool) {
        let board = NSPasteboard.general
        pendingClear?.cancel()
        pendingClear = nil
        board.prepareForNewContents(with: sensitive ? .currentHostOnly : [])
        board.setString(plain, forType: .string)
        if let html { board.setString(html, forType: .html) }
        defer { ClipboardWatcher.shared.noticeOwnCopy() }
        guard sensitive else { return }
        board.setString("", forType: NSPasteboard.PasteboardType(concealedType))
        board.setString("", forType: NSPasteboard.PasteboardType(transientType))
        board.setString("", forType: NSPasteboard.PasteboardType(tomiSecretType))
        let written = board.changeCount
        pendingClear = Task {
            try? await Task.sleep(for: .seconds(clearDelay))
            guard !Task.isCancelled else { return }
            clearSecret(ifChangeCountIs: written)
        }
    }

    /// Empties the clipboard if it still holds a secret copied by Tomi (leaving the space, quitting the app, the 30 s).
    static func clearSecret(ifChangeCountIs count: Int? = nil) {
        let board = NSPasteboard.general
        if let count, board.changeCount != count { return }
        guard board.types?.contains(NSPasteboard.PasteboardType(tomiSecretType)) == true else { return }
        board.clearContents()
    }
    #else
    static func copy(plain: String, html: String? = nil, sensitive: Bool) {
        var item: [String: Any] = [UTType.utf8PlainText.identifier: plain]
        if let html { item[UTType.html.identifier] = html }
        var options: [UIPasteboard.OptionsKey: Any] = [:]
        if sensitive {
            item[concealedType] = ""
            item[tomiSecretType] = ""
            options[.localOnly] = true
            options[.expirationDate] = Date().addingTimeInterval(clearDelay)
        }
        UIPasteboard.general.setItems([item], options: options)
        ClipboardWatcher.shared.noticeOwnCopy()
    }

    /// Empties the clipboard if it holds a secret copied by Tomi; anything else copied there is the user's.
    static func clearSecret() {
        if UIPasteboard.general.contains(pasteboardTypes: [tomiSecretType]) {
            UIPasteboard.general.items = []
        }
    }
    #endif
}
