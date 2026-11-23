import AppKit
import SwiftUI
import TomiKit
import UniformTypeIdentifiers

/// What the Mac gives to the send screen besides typing: the clipboard at a paste, and what is dropped on the window.
/// Nothing here reads the clipboard by itself: `paste` runs on the "Coller" button or on Command-V only.
@MainActor
enum MacSendInput {
    private static let concealedType = NSPasteboard.PasteboardType(SensitiveClipboard.concealedType)

    /// What a pasteboard holds for a send, read once.
    struct Content {
        /// Files copied in the Finder.
        var files: [URL] = []
        /// An image copied without a file behind it (a screenshot, an image of a web page), as PNG.
        var image: Data?
        var plain: String?
        var html: String?
        /// The copying app marked it as concealed (password managers).
        var concealed = false
    }

    static func read(_ board: NSPasteboard) -> Content {
        var content = Content()
        content.concealed = board.types?.contains(concealedType) == true
        let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
        content.files = urls.filter(\.isFileURL)
        // Copied files also put their names as text: the files are what was copied.
        guard content.files.isEmpty else { return content }
        content.plain = board.string(forType: .string) ?? board.string(forType: .URL)
        content.html = board.string(forType: .html)
        if content.plain == nil, content.html == nil {
            if let png = board.data(forType: .png) {
                content.image = png
            } else if let tiff = board.data(forType: .tiff), let bitmap = NSBitmapImageRep(data: tiff) {
                content.image = bitmap.representation(using: .png, properties: [:])
            }
        }
        return content
    }

    /// The clipboard holds files, or an image with no text beside it. Reads the types only, never the contents.
    static func holdsFilesOrImage(_ board: NSPasteboard = .general) -> Bool {
        guard let types = board.types else { return false }
        return types.contains(.fileURL)
            || (!types.contains(.string) && !types.contains(.html) && (types.contains(.png) || types.contains(.tiff)))
    }

    /// The copying app marked what it copied as concealed (password managers). Reads the types only.
    static func holdsConcealed(_ board: NSPasteboard = .general) -> Bool {
        board.types?.contains(concealedType) == true
    }

    /// "Coller": files and a copied image are attached, a text goes in the content with its rich variant.
    static func paste(into model: SendViewModel, picked: PickedFiles, board: NSPasteboard = .general) async {
        await fill(model, with: read(board), picked: picked)
    }

    /// Puts what was read from a pasteboard in the draft.
    static func fill(_ model: SendViewModel, with content: Content, picked: PickedFiles) async {
        if !content.files.isEmpty {
            await attach(content.files, to: model, picked: picked)
        } else if let image = content.image {
            let file = await picked.importData(image, name: L10n.text("macos_pasted_image_name"))
            model.onAttach(file.map { [$0] } ?? [], skipped: file == nil ? 1 : 0)
        } else {
            model.onPasted(plain: content.plain ?? content.html.map(HtmlText.toPlain), html: content.html, concealed: content.concealed)
        }
    }

    /// Copies the files into the app (the system lends them for the moment of the paste or of the drop) and adds them
    /// to the draft; the ones that cannot be read or are too big are counted as skipped.
    static func attach(_ urls: [URL], to model: SendViewModel, picked: PickedFiles) async {
        // Only what the draft still has room for is copied (a folder of 300 files is not copied to keep 20).
        let room = max(0, SendViewModel.maxAttachments - model.attachments.count)
        var files: [SharedFile] = []
        for url in urls.prefix(room) {
            if let file = await picked.importFile(url) { files.append(file) }
        }
        model.onAttach(files, skipped: urls.count - files.count)
    }

    /// What was dropped on the window joins the draft: files are attached, a link or a text is added to the content.
    /// Nothing is sent: the send screen shows, "Envoyer" stays the user's.
    static func drop(_ providers: [NSItemProvider], into model: SendViewModel, picked: PickedFiles) async {
        var files: [URL] = []
        var texts: [String] = []
        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                if let url = await load(URL.self, from: provider) { files.append(url) }
            } else if provider.canLoadObject(ofClass: URL.self) {
                if let url = await load(URL.self, from: provider) { texts.append(url.absoluteString) }
            } else if provider.canLoadObject(ofClass: String.self) {
                if let text = await load(String.self, from: provider) { texts.append(text) }
            }
        }
        if !files.isEmpty { await attach(files, to: model, picked: picked) }
        if !texts.isEmpty { model.onPasted(plain: texts.joined(separator: "\n"), html: nil, concealed: false) }
    }

    private static func load<T: _ObjectiveCBridgeable>(_ type: T.Type, from provider: NSItemProvider) async -> T?
    where T._ObjectiveCType: NSItemProviderReading {
        await withCheckedContinuation { continuation in
            _ = provider.loadObject(ofClass: type) { value, _ in continuation.resume(returning: value) }
        }
    }
}

/// Command-V while the view is in the key window: `handler` says whether it took the paste (the event then stops).
struct MacPasteShortcut: NSViewRepresentable {
    let handler: () -> Bool

    func makeNSView(context: Context) -> MonitorView {
        let view = MonitorView()
        view.handler = handler
        return view
    }

    func updateNSView(_ view: MonitorView, context: Context) {
        view.handler = handler
    }

    final class MonitorView: NSView {
        var handler: (() -> Bool)?
        private var monitor: Any?

        deinit {
            if let monitor { NSEvent.removeMonitor(monitor) }
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.window, event.window === window, window.isKeyWindow,
                      event.modifierFlags.intersection([.command, .control, .option, .shift]) == .command,
                      event.charactersIgnoringModifiers?.lowercased() == "v",
                      // Not while a sheet or a panel is over the window.
                      window.attachedSheet == nil
                else { return event }
                return self.handler?() == true ? nil : event
            }
        }
    }
}

/// The window takes what is dragged onto it (files from the Finder, a link from a browser, a selected text): it joins
/// the draft and the send screen shows. Only on a device that is in a space, with the feed up.
struct MacDropTarget: ViewModifier {
    let container: AppContainer
    @State private var targeted = false

    private var accepts: Bool {
        container.core?.state.isPaired == true && container.pairingFlow == false && container.send?.editable == true
    }

    func body(content: Content) -> some View {
        content
            .onDrop(of: [.fileURL, .url, .utf8PlainText], isTargeted: $targeted) { providers in
                guard accepts, let send = container.send, let picked = container.picked else { return false }
                Task {
                    await MacSendInput.drop(providers, into: send, picked: picked)
                    container.pendingRoute = .send
                }
                return true
            }
            .overlay {
                if targeted && accepts {
                    RoundedRectangle(cornerRadius: 12)
                        .strokeBorder(Color.accentColor, style: StrokeStyle(lineWidth: 3, dash: [8, 6]))
                        .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                        .padding(8)
                        .allowsHitTesting(false)
                }
            }
    }
}

/// The keys pressed while the view is in the key window and no text is being typed: `handler` says whether it used
/// the key (the event then stops). For the keyboard of a screen that has no focusable control (the cards of the feed).
struct MacKeys: NSViewRepresentable {
    let handler: (NSEvent) -> Bool

    func makeNSView(context: Context) -> MonitorView {
        let view = MonitorView()
        view.handler = handler
        return view
    }

    func updateNSView(_ view: MonitorView, context: Context) {
        view.handler = handler
    }

    final class MonitorView: NSView {
        var handler: ((NSEvent) -> Bool)?
        private var monitor: Any?

        deinit {
            if let monitor { NSEvent.removeMonitor(monitor) }
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let monitor { NSEvent.removeMonitor(monitor) }
            monitor = nil
            guard window != nil else { return }
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.window, event.window === window, window.isKeyWindow,
                      window.attachedSheet == nil,
                      // A text being typed (a field, the search) keeps its keys.
                      !(window.firstResponder is NSText)
                else { return event }
                return self.handler?(event) == true ? nil : event
            }
        }
    }
}
