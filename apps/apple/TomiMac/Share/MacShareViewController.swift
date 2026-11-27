import AppKit
import SwiftUI
import TomiDesign
import TomiKit

/// Entry of the share extension of the Mac app ("Partager" → Tomi in Safari, the Finder, Photos, any app): the sheet
/// and the logic of the iOS extension (`TomiShare/`), hosted in an AppKit view controller. The share is sent at once
/// with the default settings and a 3 s banner offers "Annuler" and "Options"; what looks like a secret is confirmed
/// first. The extension never opens the core: it hands the share to the app through the inbox of the app group, and
/// the app, which runs in the menu bar, sends it.
@objc(MacShareViewController)
final class MacShareViewController: NSViewController {
    private var incoming: URL?
    private var closed = false

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 420, height: 380))
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        preferredContentSize = view.frame.size
        TomiFonts.register()

        // Without the public key of the app the extension cannot seal a share, so it asks to open the app first (the key is
        // published at each start of the app: only an app not opened since the update lacks it).
        let publicKey = AppGroup.inboxPublicKey
        let paired = (AppGroup.defaults?.bool(forKey: AppGroup.isPairedKey) ?? false) && publicKey != nil
        let inbox = ShareInbox.shared(recipientKey: publicKey)
        let sender = InboxShareSender(inbox: inbox ?? ShareInbox(root: FileManager.default.temporaryDirectory))
        let unreadable = AppGroup.defaults?.bool(forKey: AppGroup.isUnreadableKey) ?? false
        let flow = ShareFlow(sender: sender, isPaired: paired, isUnreadable: unreadable)
        let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []

        let sheet = ShareSheetView(flow: flow) { [weak self] in self?.close() }
            // The controls of macOS draw a bezel the shared sheet does not expect.
            .buttonStyle(.plain)
            .toggleStyle(.switch)
        let host = NSHostingController(rootView: sheet)
        host.sizingOptions = []
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.width, .height]
        view.addSubview(host.view)

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

    /// Dismissed by the system: the copies made for the reading go (a deposited share keeps its own links).
    override func viewDidDisappear() {
        super.viewDidDisappear()
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
