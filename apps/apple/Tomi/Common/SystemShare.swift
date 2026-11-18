import SwiftUI
import UIKit

/// Opens the system share sheet with a text (Android `ACTION_SEND` of an item). Returns false when
/// nothing can present it.
@MainActor
enum SystemShare {
    /// `sensitive` (a hidden item): the sheet's own "Copy" is removed, since it would write the secret to the general
    /// pasteboard without the local-only flag and the 30 s expiry of `SensitiveClipboard`.
    @discardableResult
    static func present(text: String, subject: String?, sensitive: Bool) -> Bool {
        guard let presenter = topViewController() else { return false }
        let controller = UIActivityViewController(activityItems: [ShareText(text: text, subject: subject)], applicationActivities: nil)
        if sensitive { controller.excludedActivityTypes = [.copyToPasteboard] }
        controller.popoverPresentationController?.sourceView = presenter.view
        controller.popoverPresentationController?.sourceRect = CGRect(
            x: presenter.view.bounds.midX, y: presenter.view.bounds.maxY - 80, width: 1, height: 1
        )
        presenter.present(controller, animated: true)
        return true
    }

    static func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first { $0.activationState == .foregroundActive }
            ?? UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
        var top = scene?.keyWindow?.rootViewController
        while let presented = top?.presentedViewController { top = presented }
        return top
    }
}

/// The shared text, with its subject (mail, notes) given the documented way.
private final class ShareText: NSObject, UIActivityItemSource {
    let text: String
    let subject: String?

    init(text: String, subject: String?) {
        self.text = text
        self.subject = subject
    }

    func activityViewControllerPlaceholderItem(_ controller: UIActivityViewController) -> Any { text }

    func activityViewController(_ controller: UIActivityViewController, itemForActivityType type: UIActivity.ActivityType?) -> Any? { text }

    func activityViewController(_ controller: UIActivityViewController, subjectForActivityType type: UIActivity.ActivityType?) -> String {
        subject ?? ""
    }
}
