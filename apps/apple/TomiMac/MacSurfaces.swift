import AppKit
import ServiceManagement
import SwiftUI
import TomiKit

/// Where a screen of Tomi is shown on the Mac, and what it can ask of the app from there.
struct MacSurfaceContext {
    /// In the popover of the menu bar icon (else in the full window).
    var isPopover: Bool
    var openWindow: () -> Void
    var openSettings: () -> Void
}

extension EnvironmentValues {
    /// Nil outside the surfaces of the Mac app (previews).
    @Entry var macSurface: MacSurfaceContext?
}

/// Where a surface stands, in the words the shared screens know (the scene phase of iOS): `active` while it has
/// the keyboard (a card is read only while it is looked at), `inactive` while a sheet of its own or a system prompt is
/// over it (a typed secret is masked, nothing closes), `background` once it is left for something else or closed (the
/// viewer and the code screen of a hidden item close).
@MainActor
@Observable
final class SurfaceActivity {
    var phase = ScenePhase.background
}

/// The two surfaces of the Mac app, shown one at a time over the same container: the popover of the menu bar icon
/// (feed, send, detail, at hand) and the full window (the same screens with room, and the entry of a Mac that is in
/// no space yet). While the window is open the icon brings it forward instead of opening the popover: one set of
/// screens follows the core at a time.
@MainActor
final class MacSurfaces: NSObject, NSWindowDelegate {
    static let popoverSize = NSSize(width: 400, height: 600)
    static let windowSize = NSSize(width: 720, height: 780)
    static let windowMinSize = NSSize(width: 420, height: 480)

    private let container: AppContainer
    /// The button of the menu bar icon, the anchor of the popover.
    var statusButton: () -> NSStatusBarButton? = { nil }
    /// The global shortcut, shown and changed in the settings.
    var shortcut: MacGlobalShortcut?
    /// The shortcut that sends the selection of the app in front.
    var sendShortcut: MacGlobalShortcut?
    private var popover: PopoverPanel?
    private var deactivation: NSObjectProtocol?
    private var observers: [NSObjectProtocol] = []
    /// The screen is locked, the Mac sleeps or the session is switched.
    private var sessionLocked = false
    /// Waits for the end of an authentication prompt to work out the phases again.
    private var promptWatch: Task<Void, Never>?
    /// The top left corner of the popover, kept when its height changes (a toolbar that shows or goes).
    private var popoverTop = NSPoint.zero
    private var window: NSWindow?
    private var settings: NSWindow?
    private var settingsClosed: NSObjectProtocol?
    private let popoverActivity = SurfaceActivity()
    private let windowActivity = SurfaceActivity()

    init(container: AppContainer) {
        self.container = container
        super.init()
        let center = NotificationCenter.default
        let workspace = NSWorkspace.shared.notificationCenter
        let distributed = DistributedNotificationCenter.default()
        let refresh = { [weak self] (_: Notification) -> Void in
            MainActor.assumeIsolated { self?.refreshPhases() }
        }
        let locked = { [weak self] (_: Notification) -> Void in
            MainActor.assumeIsolated {
                self?.sessionLocked = true
                self?.refreshPhases()
            }
        }
        let unlocked = { [weak self] (_: Notification) -> Void in
            MainActor.assumeIsolated {
                self?.sessionLocked = false
                self?.refreshPhases()
            }
        }
        // Kept for the life of the app (this object lives as long as its delegate).
        observers = [
            center.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main, using: refresh),
            center.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main, using: refresh),
            // Nobody is at the Mac: every surface counts as left, even one that kept the keyboard.
            distributed.addObserver(forName: Notification.Name("com.apple.screenIsLocked"), object: nil, queue: .main, using: locked),
            distributed.addObserver(forName: Notification.Name("com.apple.screenIsUnlocked"), object: nil, queue: .main, using: unlocked),
            workspace.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main, using: locked),
            workspace.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main, using: unlocked),
            workspace.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main, using: locked),
            workspace.addObserver(forName: NSWorkspace.sessionDidBecomeActiveNotification, object: nil, queue: .main, using: unlocked),
        ]
    }

    var isWindowOpen: Bool { window != nil }

    /// Tomi comes in front of the other apps. A menu bar app has no Dock icon to click: opened from its icon, its
    /// shortcut, a notification or a link, it must take the keyboard itself, which the cooperative `activate()` of
    /// macOS 14 does not always grant (the popover would then float over the other apps without the keyboard).
    private static func bringForward() {
        #if DEBUG
        if debugOffscreen { return }
        #endif
        NSApp.activate(ignoringOtherApps: true)
    }

    #if DEBUG
    /// `-TomiDebugOffscreen YES` (debug builds): every window of Tomi opens off the screens and never takes the
    /// keyboard, the Dock icon never shows. An automated session can then open the surfaces (`tomi-share://open`,
    /// `tomi-share://debug-window`) and look at them (`tomi-share://debug-snapshot`) while someone is using the Mac, with nothing
    /// showing up on the screen. The surfaces count as used, so that the screens draw as they do in front of a user.
    static let debugOffscreen = UserDefaults.standard.bool(forKey: "TomiDebugOffscreen") || debugBehind
    static let offscreenOrigin = NSPoint(x: -30_000, y: -30_000)
    /// `-TomiDebugBehind YES`: as the off-screen mode, but the windows are on the screen, behind every other window
    /// and without the keyboard. For what the system only draws in a window that is on a screen (Quick Look, the
    /// player): to use briefly.
    static let debugBehind = UserDefaults.standard.bool(forKey: "TomiDebugBehind")
    #endif

    private func setDockIcon(_ shown: Bool) {
        #if DEBUG
        if Self.debugOffscreen { return }
        #endif
        NSApp.setActivationPolicy(shown ? .regular : .accessory)
    }

    /// Puts a window on screen: in front with the keyboard, or, in the off-screen debug mode, far from every screen.
    private func present(_ window: NSWindow) {
        #if DEBUG
        if Self.debugBehind {
            window.setFrameOrigin(NSScreen.main?.visibleFrame.origin ?? .zero)
            window.orderBack(nil)
            return
        }
        if Self.debugOffscreen {
            window.setFrameOrigin(Self.offscreenOrigin)
            window.orderFront(nil)
            return
        }
        #endif
        window.makeKeyAndOrderFront(nil)
    }
    var isPopoverShown: Bool { popover != nil }
    var isAnySurfaceShown: Bool { popover != nil || window != nil }

    #if DEBUG
    /// For `tomi-share://debug-state`.
    var debugState: String {
        "phases popover=\(popoverActivity.phase) window=\(windowActivity.phase) locked=\(sessionLocked) "
            + "key=\(NSApp.keyWindow.map { String(describing: type(of: $0)) } ?? "nil") "
            + "popoverKey=\(popover?.isKeyWindow == true) windowKey=\(window?.isKeyWindow == true)"
    }
    #endif

    // MARK: Popover

    /// The click on the menu bar icon.
    func toggle(from button: NSStatusBarButton) {
        if isPopoverShown {
            closePopover()
        } else if needsWindow {
            showWindow()
        } else {
            showPopover(from: button)
        }
    }

    /// The global shortcut: the popover (or the window) comes up; pressed again while it has the keyboard, it goes.
    func toggleFromShortcut() {
        if isPopoverShown, popover?.isKeyWindow == true {
            closePopover()
        } else if needsWindow {
            showWindow()
        } else if let button = statusButton() {
            if isPopoverShown { closePopover() }
            showPopover(from: button)
        }
    }

    /// The window is already open, the Mac is in no space yet (creating or joining one needs room), or the core could
    /// not be opened (the screen that says why).
    private var needsWindow: Bool {
        if window != nil || container.pairingFlow == true { return true }
        switch container.phase {
        case .unreadable, .misconfigured: return true
        default: return false
        }
    }

    /// Escape: closes what is on top, one thing at a time (docs/macos-app.md, 2.7): a viewer, then a
    /// screen pushed over the first one, then the popover itself (the full window stays).
    func escape(closesPopover: Bool) {
        if let feed = container.feed, feed.viewerRequest != nil || feed.fileRequest != nil {
            feed.viewerRequest = nil
            feed.fileRequest = nil
        } else if container.stackDepth > 0 {
            container.backRequests += 1
        } else if closesPopover {
            closePopover()
        }
    }

    /// The title of the popover shows with the bar of a pushed screen, and goes with it on the feed.
    func popoverTitleFollows(pushed: Bool) {
        popover?.titleVisibility = pushed ? .visible : .hidden
    }

    /// The popover is a panel under the menu bar icon rather than an `NSPopover`: a window has a toolbar, which the
    /// screens pushed over the feed need (back, title, their own buttons); a popover has none.
    private func showPopover(from button: NSStatusBarButton) {
        let panel = PopoverPanel(
            contentRect: NSRect(origin: .zero, size: Self.popoverSize),
            styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false
        )
        // The feed has no bar and shows no title. A screen pushed over it has its bar, with its title in it: the
        // place of the title is also what sends the buttons the screen puts at the end of the bar to the trailing
        // edge (without it they sit against the back button). See `popoverTitleFollows`.
        panel.titleVisibility = container.stackDepth > 0 ? .visible : .hidden
        panel.titlebarAppearsTransparent = true
        panel.isMovable = false
        panel.isFloatingPanel = true
        // Above the windows, under the menus and the system dialogs (Quick Look, an authentication prompt).
        panel.level = .statusBar
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.moveToActiveSpace, .transient, .fullScreenAuxiliary]
        panel.animationBehavior = .utilityWindow
        for kind in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            panel.standardWindowButton(kind)?.isHidden = true
        }
        panel.onCancel = { [weak self] in self?.escape(closesPopover: true) }
        let host = NSHostingController(rootView: root(popover: true))
        host.sizingOptions = []
        // The toolbar of the screens, not their title (the panel shows none).
        host.sceneBridgingOptions = [.toolbars]
        panel.contentViewController = host
        panel.setContentSize(Self.popoverSize)
        panel.setFrameOrigin(Self.origin(under: button, size: panel.frame.size))
        #if DEBUG
        if Self.debugOffscreen, !Self.debugBehind { panel.setFrameOrigin(Self.offscreenOrigin) }
        #endif
        popoverTop = NSPoint(x: panel.frame.minX, y: panel.frame.maxY)
        panel.delegate = self
        popover = panel
        // The app comes forward: the popover takes the keyboard (typing, Command-V).
        Self.bringForward()
        present(panel)
        refreshPhases()
        // Left for another app: the popover closes, as a menu would.
        deactivation = NotificationCenter.default.addObserver(
            forName: NSApplication.didResignActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.appDeactivated() }
        }
    }

    /// Under the menu bar icon, centred on it, kept inside the screen.
    private static func origin(under button: NSStatusBarButton, size: NSSize) -> NSPoint {
        guard let buttonWindow = button.window else { return .zero }
        let anchor = buttonWindow.convertToScreen(button.convert(button.bounds, to: nil))
        let screen = buttonWindow.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? .zero
        let margin: CGFloat = 8
        let x = min(max(anchor.midX - size.width / 2, screen.minX + margin), screen.maxX - size.width - margin)
        return NSPoint(x: x, y: anchor.minY - 6 - size.height)
    }

    private func appDeactivated() {
        // A system prompt (Touch ID, the password of the Mac) takes the app's place for a moment: the popover stays,
        // and `refreshPhases` looks again once the prompt is over.
        guard !container.gate.isPrompting else { return }
        closePopover()
    }

    func closePopover() {
        guard let panel = popover else { return }
        popover = nil
        if let deactivation { NotificationCenter.default.removeObserver(deactivation) }
        deactivation = nil
        // What a hidden item showed does not stay shown behind a closed popover.
        container.surfaceLeft(closed: true)
        refreshPhases()
        panel.delegate = nil
        panel.orderOut(nil)
        panel.contentViewController = nil
    }

    // MARK: Window

    func showWindow() {
        closePopover()
        if window == nil {
            let window = SurfaceWindow(
                contentRect: NSRect(origin: .zero, size: Self.windowSize),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
            )
            window.title = L10n.text("app_name")
            window.isReleasedWhenClosed = false
            window.contentMinSize = Self.windowMinSize
            window.onCancel = { [weak self] in self?.escape(closesPopover: false) }
            let host = NSHostingController(rootView: root(popover: false))
            host.sizingOptions = []
            // The screens put their back button, their title and their buttons in the toolbar of the window.
            host.sceneBridgingOptions = [.toolbars, .title]
            window.contentViewController = host
            window.setContentSize(Self.windowSize)
            window.center()
            // The place and the size the user gave it come back at the next opening.
            window.setFrameAutosaveName("TomiMainWindow")
            window.delegate = self
            self.window = window
        }
        // With a window open Tomi is an ordinary app: Dock icon, menu bar, Command-Tab.
        setDockIcon(true)
        Self.bringForward()
        if let window { present(window) }
        refreshPhases()
    }

    func windowDidBecomeKey(_ notification: Notification) {
        refreshPhases()
    }

    func windowDidResignKey(_ notification: Notification) {
        // What took the keyboard is known at the next turn of the run loop.
        DispatchQueue.main.async { [weak self] in self?.refreshPhases() }
    }

    /// Works out where each surface stands from the state of AppKit, whatever just happened (a window gained or lost
    /// the keyboard, the app came forward or went back, the session locked, a prompt ended):
    /// - `active`: the surface has the keyboard;
    /// - `inactive`: something of its own is over it, and it is still being used: the prompt that is about to show
    ///   a hidden item, or a sheet of this window while Tomi is the app in front;
    /// - `background`: anything else. It is left: what a hidden item showed is masked again and its viewers close
    ///   (docs/macos-app.md, A8).
    private func refreshPhases() {
        let key = NSApp.keyWindow
        let prompting = container.gate.isPrompting
        func phase(of surface: NSWindow?) -> ScenePhase {
            #if DEBUG
            if Self.debugOffscreen { return surface == nil ? .background : .active }
            #endif
            guard let surface, !sessionLocked else { return .background }
            if key === surface { return .active }
            if prompting { return .inactive }
            if NSApp.isActive, key?.sheetParent === surface { return .inactive }
            return .background
        }
        apply(phase(of: popover), to: popoverActivity)
        apply(phase(of: window), to: windowActivity)
        let used = popoverActivity.phase != .background || windowActivity.phase != .background
        container.surfaceVisibilityChanged(used)
        #if DEBUG
        if UserDefaults.standard.bool(forKey: "TomiTracePhases") {
            FileHandle.standardOutput.write(Data("TOMI-TRACE \(debugState) active=\(NSApp.isActive) used=\(used)\n".utf8))
        }
        #endif
        // Another window of Tomi took the keyboard (the settings): the popover closes.
        if popover != nil, popoverActivity.phase == .background, NSApp.isActive, let key, key === window || key === settings {
            closePopover()
        }
        // The prompt ends without any window event when the user went elsewhere meanwhile: look again then.
        if prompting, promptWatch == nil {
            promptWatch = Task { [weak self] in
                while self?.container.gate.isPrompting == true {
                    try? await Task.sleep(for: .milliseconds(150))
                }
                // The app takes the keyboard back a moment after the prompt.
                try? await Task.sleep(for: .milliseconds(300))
                guard let self else { return }
                self.promptWatch = nil
                self.refreshPhases()
                // Tomi did not come back in front: the popover goes as it would have without the prompt.
                if self.isPopoverShown, !NSApp.isActive { self.closePopover() }
            }
        }
    }

    private func apply(_ phase: ScenePhase, to activity: SurfaceActivity) {
        guard activity.phase != phase else { return }
        let wasUsed = activity.phase != .background
        activity.phase = phase
        if wasUsed, phase == .background { container.surfaceLeft(closed: false) }
    }

    func windowDidResize(_ notification: Notification) {
        guard let panel = popover, notification.object as? NSWindow === panel, panel.frame.maxY != popoverTop.y else { return }
        panel.setFrameTopLeftPoint(popoverTop)
    }

    func windowWillClose(_ notification: Notification) {
        guard notification.object as? NSWindow === window else { return }
        container.surfaceLeft(closed: true)
        window?.contentViewController = nil
        window = nil
        refreshPhases()
        // Back to a menu bar app: no Dock icon while no window is open.
        setDockIcon(false)
    }

    // MARK: Others

    /// A link, a notification or a second launch asks for Tomi: the window when it is open or needed, else the popover.
    func showForLink() {
        if needsWindow {
            showWindow()
        } else if !isPopoverShown, let button = statusButton() {
            showPopover(from: button)
        }
    }

    /// The settings window, one at a time (kept while it is open).
    func showSettings() {
        closePopover()
        if settings == nil {
            let host = NSHostingController(rootView: MacSettingsView(shortcut: shortcut, sendShortcut: sendShortcut))
            let window = SurfaceWindow(contentViewController: host)
            window.styleMask = [.titled, .closable]
            window.title = L10n.text("macos_settings_title")
            window.isReleasedWhenClosed = false
            window.center()
            settings = window
            settingsClosed = NotificationCenter.default.addObserver(
                forName: NSWindow.willCloseNotification, object: window, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.settingsDidClose() }
            }
        }
        Self.bringForward()
        if let settings { present(settings) }
    }

    private func settingsDidClose() {
        if let settingsClosed { NotificationCenter.default.removeObserver(settingsClosed) }
        settingsClosed = nil
        settings?.contentViewController = nil
        settings = nil
    }

    private func root(popover: Bool) -> MacSurfaceRoot {
        MacSurfaceRoot(
            container: container,
            activity: popover ? popoverActivity : windowActivity,
            context: MacSurfaceContext(
                isPopover: popover,
                openWindow: { [weak self] in self?.showWindow() },
                openSettings: { [weak self] in self?.showSettings() }
            )
        )
    }
}

/// The popover of the menu bar icon: a panel that takes the keyboard, and closes on Escape when no screen took it.
final class PopoverPanel: NSPanel {
    var onCancel: (() -> Void)?

    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    #if DEBUG
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect {
        MacSurfaces.debugOffscreen && !MacSurfaces.debugBehind ? frameRect : super.constrainFrameRect(frameRect, to: screen)
    }
    #endif
}

/// A window of Tomi (the full window, the settings). In the off-screen debug mode it may sit away from every screen.
final class SurfaceWindow: NSWindow {
    /// Escape, when no control took it.
    var onCancel: (() -> Void)?

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }

    #if DEBUG
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect {
        MacSurfaces.debugOffscreen && !MacSurfaces.debugBehind ? frameRect : super.constrainFrameRect(frameRect, to: screen)
    }
    #endif
}

/// The screens of the app in a surface of the Mac.
struct MacSurfaceRoot: View {
    let container: AppContainer
    let activity: SurfaceActivity
    let context: MacSurfaceContext

    var body: some View {
        RootView()
            .macScreenDefaults()
            .modifier(MacDropTarget(container: container))
            .environment(container)
            .environment(\.macSurface, context)
            // An AppKit window has no scene: the shared screens read the phase of their surface instead.
            .environment(\.scenePhase, activity.phase)
            .frame(
                minWidth: context.isPopover ? MacSurfaces.popoverSize.width : MacSurfaces.windowMinSize.width,
                maxWidth: context.isPopover ? MacSurfaces.popoverSize.width : .infinity,
                minHeight: context.isPopover ? MacSurfaces.popoverSize.height : MacSurfaces.windowMinSize.height,
                maxHeight: context.isPopover ? MacSurfaces.popoverSize.height : .infinity
            )
    }
}

/// Tomi opened when the session opens: without it nothing is received after a restart until Tomi is opened by hand.
@MainActor
enum MacLoginItem {
    private static let initialisedKey = "loginItemInitialised"

    static var isEnabled: Bool { SMAppService.mainApp.status == .enabled }

    /// False when the system refused (the toggle then shows the real state).
    @discardableResult
    static func setEnabled(_ enabled: Bool) -> Bool {
        do {
            if enabled { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            return true
        } catch {
            return false
        }
    }

    /// Turned on once, at the first launch of a published build (as the Windows app does); the user's later choice
    /// in the settings is never overridden. Never in a debug build: a build being developed does not install itself.
    static func enableOnceByDefault(defaults: UserDefaults = .standard) {
        #if !DEBUG
        guard !defaults.bool(forKey: initialisedKey) else { return }
        // Recorded only once the system accepted: a refusal is tried again at the next launch.
        if setEnabled(true) { defaults.set(true, forKey: initialisedKey) }
        #endif
    }
}
