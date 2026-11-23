import AppKit

/// Opens the share menu of the Mac with a text (the iOS counterpart is `Tomi/Common/SystemShare.swift`). Returns
/// false when no window can show it.
@MainActor
enum SystemShare {
    /// `sensitive` (a hidden item): only the services that send it to someone, never one that would keep a clear copy
    /// (`MacFileActions`).
    @discardableResult
    static func present(text: String, subject: String?, sensitive: Bool) -> Bool {
        MacFileActions.share(items: [text], subject: subject, sensitive: sensitive, folder: nil)
    }
}
