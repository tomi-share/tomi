import AppKit

/// "Send with Tomi" in the Services menu of macOS (the selection of any app, the files selected in the Finder):
/// what the system hands over joins the draft and the send screen shows. Nothing is sent: "Envoyer" stays the user's.
/// Declared in `Info.plist` (`NSServices`).
@MainActor
final class MacServices: NSObject {
    private let container: AppContainer
    private let showSend: () -> Void
    private let showSurface: () -> Void

    init(container: AppContainer, showSend: @escaping () -> Void, showSurface: @escaping () -> Void) {
        self.container = container
        self.showSend = showSend
        self.showSurface = showSurface
    }

    /// The pasteboard here is the one of the service: what the user selected and sent, never the clipboard. The
    /// service may be what launched Tomi: the core gets a moment to load before deciding.
    @objc func sendWithTomi(_ pasteboard: NSPasteboard, userData: String, error: AutoreleasingUnsafeMutablePointer<NSString>) {
        // Read now: the service pasteboard is only lent for the call.
        let content = MacSendInput.read(pasteboard)
        Task {
            let deadline = Date().addingTimeInterval(10)
            while container.core?.state.loaded != true, Date() < deadline {
                try? await Task.sleep(for: .milliseconds(100))
            }
            guard container.core?.state.isPaired == true, container.pairingFlow == false,
                  let send = container.send, let picked = container.picked, send.editable else {
                // Not in a space (or busy sending): the surface says where things stand.
                showSurface()
                return
            }
            await MacSendInput.fill(send, with: content, picked: picked)
            showSend()
        }
    }
}
