#if DEBUG
import AppKit
import TomiKit
import UserNotifications

/// Debug builds only: what lets an automated session look at the app and drive it without a mouse nor a screen
/// recording permission. Each tool is a link (`open -a Tomi "tomi-share://debug-…"`); what it produces is written on the
/// standard output of the app as one line `TOMI-DEBUG <name> <base64>` (launch the app with `open --stdout <file>`):
/// the container of a signed app cannot be read from outside, its output can.
///
/// - `debug-snapshot`: a PNG of each visible window (`window-<n>`). The app asks the window server for its own
///   windows, as the screen shows them.
/// - `debug-state`: what the app thinks of itself (`state`).
/// - `debug-items`: the items of the demo core (`items`), to open one with `tomi-share://item/<id>`.
/// - `debug-incoming`: a link arrives from another device of the demo space.
/// - `debug-window`, `debug-settings`: open the full window, the settings.
/// - `debug-command?name=<command>`: what a menu or a key asks (`search`, `new-send`, `devices`, `window`, `escape`,
///   `filter-<name>`).
/// - `debug-open-file?id=<item>&index=<n>&as=<file|image|code>`: opens an attachment of a demo item in the file
///   viewer, the image viewer or the code screen.
/// - `debug-remove-file?id=<item>&index=<n>`: removes one attachment of a demo item from this device, as its menu
///   does (to see what a screen showing it does next).
/// - `debug-suggest?kind=<text|files|image|hidden|none>`: the send screen offers a made-up copy, as it does for
///   something copied less than a minute ago (the clipboard itself is neither read nor written).
/// - `debug-go?to=<screen>`: shows a screen without a click (`send`, `devices`, `invite`, `onboarding`, `feed`; on a
///   Mac that is in no space: `create`, `join`, `welcome`).
///
/// Nothing of a real space ever leaves the app this way: the snapshot and the items are refused once the app is in
/// a real space, and while a pairing code or an installation key is on screen (allowed on the demo core, and on the
/// other entry screens of a real core that is in no space yet).
@MainActor
enum DebugSnapshot {
    /// `CGWindowListCreateImage`, no longer declared by the SDK but still there: the one call that gives the pixels
    /// of our own window as the screen shows them (glass, web views), where drawing the view tree gives a blank page.
    private typealias CreateImage = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?

    static func handle(_ url: URL, surfaces: MacSurfaces?) -> Bool {
        guard url.scheme == "tomi-share", let host = url.host, host.hasPrefix("debug-") else { return false }
        switch host {
        case "debug-items": listItems()
        case "debug-window": surfaces?.showWindow()
        case "debug-settings": surfaces?.showSettings()
        case "debug-state": writeState(surfaces)
        case "debug-incoming": simulateIncoming()
        case "debug-snapshot": snapshot()
        case "debug-go": go(url, surfaces)
        case "debug-open-file": openFile(url, surfaces)
        case "debug-remove-file": removeFile(url)
        case "debug-suggest": suggest(url)
        case "debug-command": command(url, surfaces)
        default: break
        }
        return true
    }

    /// One record on the standard output.
    private static func emit(_ name: String, _ data: Data) {
        let line = "TOMI-DEBUG \(name) \(data.base64EncodedString())\n"
        FileHandle.standardOutput.write(Data(line.utf8))
    }

    /// True on the demo core, and on a real core that is in no space while no sensitive screen is up (a pairing
    /// code, an installation key being typed: `sensitiveScreen()`).
    private static var mayExpose: Bool {
        guard let container = AppContainer.current else { return false }
        guard let core = container.core else { return true }
        if core.isDemo { return true }
        return core.state.loaded && !core.state.isPaired && container.sensitiveScreens == 0
            && (container.feed?.revealedIds.isEmpty ?? true)
    }

    private static func snapshot() {
        guard mayExpose, let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else {
            emit("refused", Data())
            return
        }
        let createImage = unsafeBitCast(symbol, to: CreateImage.self)
        var index = 0
        for window in NSApp.windows where window.isVisible {
            // A window the window server does not know (yet) has no usable number.
            guard let number = UInt32(exactly: window.windowNumber), number > 0 else { continue }
            // This window only (kCGWindowListOptionIncludingWindow = 8), its own bounds (kCGWindowImageBoundsIgnoreFraming = 1).
            guard let image = createImage(.null, 8, number, 1)?.takeRetainedValue() else { continue }
            guard image.width > 0, image.height > 0 else { continue }
            let bitmap = NSBitmapImageRep(cgImage: image)
            guard let png = bitmap.representation(using: .png, properties: [:]) else { continue }
            emit("window-\(index)", png)
            index += 1
        }
        emit("snapshot-done", Data("\(index)".utf8))
    }

    private static func query(_ url: URL, _ name: String) -> String? {
        URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == name }?.value
    }

    /// `tomi-share://debug-go?to=<screen>`: shows a screen without a click. The screens of the feed (`send`, `devices`,
    /// `invite`, `onboarding`, `feed`) and those of the entry of a Mac that is in no space (`create`, `join`, `welcome`).
    private static func go(_ url: URL, _ surfaces: MacSurfaces?) {
        guard let container = AppContainer.current, let screen = query(url, "to") else { return }
        surfaces?.showForLink()
        switch screen {
        case "send": container.pendingRoute = .send
        case "devices": container.pendingRoute = .devices
        case "invite": container.pendingRoute = .invite
        case "onboarding": container.pendingRoute = .onboarding
        case "feed": container.resetPathRequested = true
        case "create", "join", "welcome": container.debugEntryRoute = screen
        default: break
        }
    }

    /// `tomi-share://debug-command?name=<command>`: what a menu or a key of the app asks (`search`, `new-send`, `devices`,
    /// `window`, `escape`, `filter-<all|links|text|files|secrets|archives>`).
    private static func command(_ url: URL, _ surfaces: MacSurfaces?) {
        guard let name = query(url, "name"), let delegate = MacAppDelegate.shared else { return }
        let filters: [String: FeedFilter] = [
            "all": .all, "links": .links, "text": .text, "files": .files, "secrets": .secrets, "archives": .archives,
        ]
        switch name {
        case "search": delegate.perform(.search)
        case "new-send": delegate.perform(.newSend)
        case "devices": delegate.perform(.devices)
        case "window": delegate.perform(.window)
        case "escape": surfaces?.escape(closesPopover: true)
        default:
            if name.hasPrefix("filter-"), let filter = filters[String(name.dropFirst(7))] { delegate.perform(.filter(filter)) }
        }
    }

    /// `tomi-share://debug-open-file?id=<item>&index=<n>`: opens an attachment of a demo item as a click on it would
    /// (image viewer, file viewer or code screen).
    private static func openFile(_ url: URL, _ surfaces: MacSurfaces?) {
        guard let container = AppContainer.current, let core = container.core, core.isDemo, let feed = container.feed,
              let id = query(url, "id"), let item = feed.item(id: id) else { return }
        surfaces?.showForLink()
        let index = Int(query(url, "index") ?? "0") ?? 0
        switch query(url, "as") {
        case "image": feed.viewerRequest = ViewerRequest(itemId: item.id, attachmentIndex: index)
        case "code": feed.codeRequest = ViewerRequest(itemId: item.id, attachmentIndex: index)
        default: feed.openAttachment(item, index: index)
        }
    }

    /// `tomi-share://debug-remove-file?id=<item>&index=<n>`: demo core only.
    private static func removeFile(_ url: URL) {
        guard let container = AppContainer.current, let core = container.core, core.isDemo, let feed = container.feed,
              let id = query(url, "id"), let item = feed.item(id: id) else { return }
        feed.deleteAttachment(item, index: Int(query(url, "index") ?? "0") ?? 0, everywhere: false)
    }

    /// `tomi-share://debug-suggest?kind=<kind>`: demo core only.
    private static func suggest(_ url: URL) {
        guard let container = AppContainer.current, container.core?.isDemo == true, let send = container.send else { return }
        switch query(url, "kind") {
        case "text": send.offer(.text(preview: "https://example.org/un-article-a-lire", isLink: true))
        case "files": send.offer(.files(names: ["devis-2026.pdf", "plan.png", "notes.txt", "budget.xlsx"]))
        case "image": send.offer(.image)
        case "hidden": send.offer(.hidden)
        default: send.dismissSuggestion()
        }
    }

    private static func writeState(_ surfaces: MacSurfaces?) {
        guard let container = AppContainer.current else { return }
        Task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            let delivered = await UNUserNotificationCenter.current().deliveredNotifications().count
            let phase: String = switch container.phase {
            case .starting: "starting"
            case .opening: "opening"
            case .ready: "ready"
            case .unreadable: "unreadable"
            case .misconfigured: "misconfigured"
            }
            let core = container.core
            let lines = [
                "phase=\(phase) demo=\(core?.isDemo == true) loaded=\(core?.state.loaded == true) paired=\(core?.state.isPaired == true)",
                "surfaceVisible=\(container.surfaceVisible)",
                "popover=\(surfaces?.isPopoverShown == true) window=\(surfaces?.isWindowOpen == true)",
                surfaces?.debugState ?? "",
                "unread=\(container.unreadCount)",
                "revealed=\(container.feed?.revealedIds.count ?? 0)",
                "notifications=\(settings.authorizationStatus.rawValue) delivered=\(delivered)",
                "policy=\(NSApp.activationPolicy().rawValue) active=\(NSApp.isActive)",
            ]
            emit("state", Data(lines.joined(separator: " ; ").utf8))
        }
    }

    private static func simulateIncoming() {
        guard let core = AppContainer.current?.core, core.isDemo else { return }
        Task { try? await core.simulateIncoming(.link) }
    }

    /// Demo core only: nothing of a real space is ever written.
    private static func listItems() {
        guard let core = AppContainer.current?.core, core.isDemo else { return }
        let lines = core.state.items.map { item in
            [item.id, "\(item.kind)", item.secret ? "secret" : "-", item.pinned ? "pinned" : "-", item.archived ? "archived" : "-",
             "files:\(item.attachments.count)", item.title ?? ""].joined(separator: "\t")
        }
        emit("items", Data(lines.joined(separator: "\n").utf8))
    }
}
#endif
