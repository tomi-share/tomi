import AppKit
import SwiftUI
import TomiKit
import UserNotifications

/// The life of the Mac app: it starts with nothing but its menu bar icon, opens the core, and keeps running with
/// every window closed (the doorbell of the core stays open). Owns the container shared by every surface.
@MainActor
final class MacAppDelegate: NSObject, NSApplicationDelegate {
    let container = AppContainer()
    /// The delegate of the running app (tests reach the app's own container through it).
    private(set) static weak var shared: MacAppDelegate?
    private var surfaces: MacSurfaces?
    private var statusItem: MacStatusItem?
    private(set) var shortcut: MacGlobalShortcut?
    private(set) var sendShortcut: MacGlobalShortcut?
    private var services: MacServices?
    /// The entry (pairing) was shown by itself once for this run: closing its window does not bring it back.
    private var offeredEntry = false
    private var wasOnline = true
    private var wantedScreen = false
    private var quitting = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        Self.shared = self
        let surfaces = MacSurfaces(container: container)
        self.surfaces = surfaces
        statusItem = MacStatusItem(
            onClick: { [weak surfaces] button in surfaces?.toggle(from: button) },
            onOpenWindow: { [weak surfaces] in surfaces?.showWindow() },
            onSettings: { [weak surfaces] in surfaces?.showSettings() }
        )
        surfaces.statusButton = { [weak self] in self?.statusItem?.button }
        shortcut = MacGlobalShortcut(.open) { [weak surfaces] in surfaces?.toggleFromShortcut() }
        sendShortcut = MacGlobalShortcut(.sendSelection) { [weak self] in self?.sendSelectionPressed() }
        shortcut?.other = sendShortcut
        sendShortcut?.other = shortcut
        surfaces.sendShortcut = sendShortcut
        // When the clipboard changes (a number, nothing of what was copied): the send screen offers a fresh copy.
        ClipboardWatcher.shared.startWatching()
        surfaces.shortcut = shortcut
        // "Send with Tomi" of the Services menu.
        let services = MacServices(container: container, showSend: { [weak self, weak surfaces] in
            surfaces?.showForLink()
            self?.container.pendingRoute = .send
        }, showSurface: { [weak surfaces] in
            surfaces?.showForLink()
        })
        self.services = services
        NSApp.servicesProvider = services
        // Before anything arrives: a click on a notification may be what launched Tomi.
        UNUserNotificationCenter.current().delegate = NotificationRouter.shared
        follow()
        MacLoginItem.enableOnceByDefault()
        Task { await container.start() }
    }

    /// What the app does outside its windows depends on this state of the container.
    private struct Followed {
        var loaded = false
        var unread = 0
        /// A notification or a link asks for a screen of the feed.
        var wantsScreen = false
        var online = true
        /// The core could not be opened: a screen says why.
        var failed = false
        /// A screen is pushed over the first one (the popover then shows its title in its bar).
        var pushed = false
    }

    /// Follows what the app shows outside its windows: the first screen of a Mac that is in no space, the unread dot
    /// of the menu bar icon and the badge of the Dock icon, a screen asked by a notification, the network coming back.
    /// Runs with no window open (a SwiftUI view is not there to do it). The state is only read inside the tracking;
    /// what follows from it is done after, so that nothing else gets tracked.
    private func follow() {
        let state = withObservationTracking {
            read()
        } onChange: { [weak self] in
            Task { @MainActor [weak self] in self?.follow() }
        }
        apply(state)
    }

    private func read() -> Followed {
        var state = Followed()
        state.online = container.network.isOnline
        state.pushed = container.stackDepth > 0
        switch container.phase {
        case .unreadable, .misconfigured: state.failed = true
        default: break
        }
        guard let core = container.core, core.state.loaded else { return state }
        state.loaded = true
        state.unread = container.unreadCount
        // A route to show, a pairing link, or a file a notification opened.
        state.wantsScreen = container.pendingRoute != nil || container.pendingJoin || container.pendingCreate
            || container.feed?.fileRequest != nil || container.feed?.viewerRequest != nil
        // Read for the tracking only: `apply` chooses the first screen from it.
        _ = container.pairingFlow
        return state
    }

    /// In a space, and macOS has never been asked: the notifications are asked for now. The Mac app lives in the menu
    /// bar, often with no window: a notification is the only way it can say something arrived. Asked once per run;
    /// the system shows its question only while the answer is not known.
    private var askedNotifications = false
    private func askForNotificationsOnce() {
        guard !askedNotifications else { return }
        askedNotifications = true
        Task {
            let center = UNUserNotificationCenter.current()
            guard await center.notificationSettings().authorizationStatus == .notDetermined else { return }
            _ = try? await center.requestAuthorization(options: [.alert, .badge, .sound])
        }
    }

    private func apply(_ state: Followed) {
        surfaces?.popoverTitleFollows(pushed: state.pushed)
        statusItem?.setUnread(state.unread > 0)
        // Seen only while the Dock icon is there (the full window open).
        NSApp.dockTile.badgeLabel = state.unread > 0 ? "\(state.unread)" : nil
        if state.online && !wasOnline { container.networkCameBack() }
        wasOnline = state.online
        // The core cannot be opened: the window says why, once by itself (the icon shows it again).
        if state.failed {
            if !offeredEntry {
                offeredEntry = true
                surfaces?.showWindow()
            }
            return
        }
        guard state.loaded else { return }
        container.coreLoaded()
        // Not in a space: the entry (create, join) needs the full window, shown once by itself.
        if container.pairingFlow == true {
            if !offeredEntry {
                offeredEntry = true
                surfaces?.showWindow()
            }
        } else {
            offeredEntry = false
            askForNotificationsOnce()
        }
        // A notification was clicked with every window closed: the surface that shows the screen it asks for. Once
        // per request: a surface the user closes over a request still pending does not come back by itself.
        if state.wantsScreen, !wantedScreen, surfaces?.isAnySurfaceShown == false { surfaces?.showForLink() }
        wantedScreen = state.wantsScreen
    }

    func showSettings() {
        surfaces?.showSettings()
    }

    /// The shortcut "send the selection" (Control-Shift-V by default; Control-Shift-Space only opens Tomi). From
    /// another app: what is selected there is copied, and the send screen opens with it offered (`MacSelectionCopy`).
    /// With Tomi already in front: the send screen, nothing copied. On a Mac that is in no space: Tomi opens.
    private func sendSelectionPressed() {
        let inSpace = container.core?.state.isPaired == true && container.pairingFlow == false
        guard inSpace else {
            surfaces?.toggleFromShortcut()
            return
        }
        guard !NSApp.isActive else {
            perform(.newSend)
            return
        }
        guard MacSelectionCopy.allowed else {
            // macOS asks once; the send screen opens meanwhile, without the selection.
            MacSelectionCopy.requestAccessOnce()
            perform(.newSend)
            return
        }
        guard !MacSelectionCopy.copying else { return }
        Task {
            _ = await MacSelectionCopy.copySelection()
            // Noted while Tomi is still behind: once in front, a change of the clipboard is no longer offered.
            ClipboardWatcher.shared.look()
            perform(.newSend)
        }
    }

    /// What the menus of the app ask (Command-N, Command-F, the filters): the surface comes up on the right screen.
    enum Command {
        case newSend, window, search, devices
        case filter(FeedFilter)
    }

    func perform(_ command: Command) {
        // A sheet or an alert is up: it is answered first.
        if NSApp.modalWindow != nil || NSApp.keyWindow?.attachedSheet != nil { return }
        if case .window = command { return surfaces?.showWindow() ?? () }
        surfaces?.showForLink()
        // Not in a space: the surface shows the entry, nothing else to ask of it.
        guard container.core?.state.isPaired == true, container.pairingFlow == false else { return }
        switch command {
        case .newSend:
            container.pendingRoute = .send
        case .devices:
            container.pendingRoute = .devices
        case .search:
            // The search of the feed; a screen over it (the code screen has its own) keeps the keys.
            guard container.stackDepth == 0 else { return }
            container.searchRequests += 1
        case let .filter(filter):
            container.resetPathRequested = true
            container.feed?.filter = filter
        case .window:
            break
        }
    }

    // MARK: Links

    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            #if DEBUG
            if DebugSnapshot.handle(url, surfaces: surfaces) { continue }
            #endif
            // A link asks for a screen: the surface that shows it comes first (the window when it is open or when the
            // Mac is not in a space yet, else the popover).
            surfaces?.showForLink()
            container.open(url)
        }
    }

    // MARK: Application

    /// Quitting: the decrypted files do not outlive the app (the next start erases them too), then it quits. The
    /// purge of the core gets two seconds: a purge that hangs does not hold the quit.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        // The copies under their names (a file being viewed or shared), whatever the core.
        FeedViewModel.sweepShareCopies()
        guard let core = container.core, !core.isDemo, !quitting else { return .terminateNow }
        quitting = true
        var replied = false
        let reply = {
            guard !replied else { return }
            replied = true
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        Task {
            try? await core.purgePlaintextCache()
            reply()
        }
        Task {
            try? await Task.sleep(for: .seconds(2))
            reply()
        }
        return .terminateLater
    }

    /// Closing the window leaves Tomi running in the menu bar.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// Tomi launched again while it runs (Spotlight, the Dock): its window, or its popover.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        surfaces?.showForLink()
        return false
    }
}
