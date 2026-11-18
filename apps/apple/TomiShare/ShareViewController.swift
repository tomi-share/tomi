import SwiftUI
import TomiDesign
import TomiKit
import UIKit

/// Entry of the share extension (Android `ShareActivity`): a SwiftUI sheet over the source app (iOS presents it as a
/// full sheet; the Android translucent activity has no equivalent). The share is sent at
/// once with the default settings and a 3 s banner offers "Annuler" and "Options"; what looks like a secret is
/// confirmed first. The extension never opens the core: it hands the share to the app through the inbox of the app group.
@objc(ShareViewController)
final class ShareViewController: UIViewController {
    private var incoming: URL?
    private var closed = false

    override func viewDidLoad() {
        super.viewDidLoad()
        TomiFonts.register()

        let paired = AppGroup.defaults?.bool(forKey: AppGroup.isPairedKey) ?? false
        let inbox = ShareInbox.shared()
        let sender = InboxShareSender(inbox: inbox ?? ShareInbox(root: FileManager.default.temporaryDirectory))
        let unreadable = AppGroup.defaults?.bool(forKey: AppGroup.isUnreadableKey) ?? false
        let flow = ShareFlow(sender: sender, isPaired: paired, isUnreadable: unreadable)
        let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []

        let host = UIHostingController(rootView: ShareSheetView(flow: flow) { [weak self] in self?.close() })
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)

        Task { [weak self] in
            await flow.start {
                guard let inbox else { throw CocoaError(.fileNoSuchFile) }
                let directory = try inbox.makeIncomingDirectory()
                self?.incoming = directory
                let result = await ShareItemReader(directory: directory).read(items)
                sender.skippedFiles = result.skipped
                return result.input
            }
        }
    }

    /// Swiped away: the copies made for the reading go (a deposited share keeps its own links).
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if let incoming { try? FileManager.default.removeItem(at: incoming) }
    }

    private func close() {
        guard !closed else { return }
        closed = true
        // The inbox holds its own links to the files: the copies made for the reading go.
        if let incoming { try? FileManager.default.removeItem(at: incoming) }
        extensionContext?.completeRequest(returningItems: nil)
    }
}
