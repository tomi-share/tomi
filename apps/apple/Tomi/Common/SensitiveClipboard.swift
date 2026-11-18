import UIKit
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
    }
}
