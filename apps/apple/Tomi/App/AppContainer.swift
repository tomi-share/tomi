import Foundation
import Security
import SwiftUI
#if canImport(UIKit)
import UIKit
#else
import AppKit
#endif
import UserNotifications
import TomiKit
import TomiRuntime

/// Holds what the whole app shares (Android `AppContainer`): the core, and later the
/// preferences, the biometric gate, the clipboard and the send scheduler.
@MainActor
@Observable
final class AppContainer {
    enum Phase {
        case starting
        case opening
        case ready(any TomiCore)
        /// The core could not be opened: its wrapping key is gone (restored phone, Keychain reset) or the database
        /// cannot be read (Android `StartupState.Unreadable`). `resetFailed`: a reset was asked and could not erase
        /// the local data either.
        case unreadable(resetFailed: Bool)
        /// The Keychain refuses this build (macOS: an app not signed with its application identifier). The data are
        /// not damaged: nothing is offered to erase them, the app says what is wrong.
        case misconfigured
    }

    private(set) var phase = Phase.starting
    let prefs = UserPrefs()
    let gate = BiometricGate()
    let network = NetworkMonitor()
    let feedback = FeedbackCenter()
    /// The feed state, created once the core is open (shared by the feed and the detail).
    private(set) var feed: FeedViewModel?
    /// The send screen state: a draft left there is still there when coming back.
    private(set) var send: SendViewModel?
    private(set) var picked: PickedFiles?
    private var scheduler: InAppSendScheduler?
    /// The automatic attempts of a pending departure, and the dialogs and banner of the departure (per core).
    private(set) var retrier: DepartureRetrier?
    private(set) var leave: LeaveSpaceModel?
    /// Notifications of received items, and what they already show.
    @ObservationIgnored let notifications = IncomingNotifications()
    @ObservationIgnored private var notifierState = IncomingNotifierState()
    @ObservationIgnored private var newDeviceState = NewDeviceNotifierState()
    @ObservationIgnored private var pairingGrace = PairingGrace()
    /// A screen to open in the feed's stack (a notification, a link, a shortcut), taken by the feed when it shows.
    var pendingRoute: Route?
    /// `tomi-share://open`: back to the feed (the stack is emptied).
    var resetPathRequested = false
    /// A pairing link opened the app while it is not in a space: the join screen offers it.
    var pendingJoin = false
    /// A creation link opened the app while it is not in a space: the creation screen offers it (relay and token).
    var pendingCreate = false
    @ObservationIgnored private var pendingSetupLink: ParsedSetupLink?
    #if DEBUG
    /// Debug builds: a screen of the entry to show without a click (`create`, `join`, `welcome`), for the tools that
    /// look at the app with no mouse.
    var debugEntryRoute: String?
    #endif
    /// A notification tap or a link that came before the core was ready (a cold start): replayed once it is.
    @ObservationIgnored private var pendingNotification: NotificationResponse?
    @ObservationIgnored private var pendingLink: DeepLink?
    /// The core whose items are followed (one chain of observation per core).
    @ObservationIgnored private var watchedCore: ObjectIdentifier?
    #if os(macOS)
    /// The core `coreLoaded` already ran for (weak: a released core never passes for a new one at the same address).
    @ObservationIgnored private weak var loadedCore: (any TomiCore)?
    #endif
    /// Screens showing a secret right now (`sensitiveScreen()`): covered in the app switcher and while the screen is
    /// recorded or mirrored.
    private(set) var sensitiveScreens = 0
    /// The screen is recorded, mirrored or shared (`UIScreen.isCaptured`).
    private(set) var screenCaptured = false
    @ObservationIgnored private var observers: [NSObjectProtocol] = []
    private var coordinator: SendCoordinator?
    /// What the share extension left in the inbox of the app group (nil without the app group).
    @ObservationIgnored private var shareIntake: ShareIntake?
    @ObservationIgnored private var inboxPurged = false
    /// The pairing screen is up: from the start on an unpaired device, and again after this device left the space. It
    /// stays up after the join to show "Appareil ajouté". Nil until the core has read its state.
    var pairingFlow: Bool?
    /// The settling of an interrupted pairing (retries for up to two minutes): cancelled when the core is replaced.
    @ObservationIgnored private var resumeTask: Task<Void, Never>?
    /// The first-launch explanation follows the pairing once.
    var onboardingFlow = false
    /// The pairing screen state; reads the current core at each step (the debug demo entry may replace it).
    @ObservationIgnored private(set) lazy var pairing = PairingViewModel { [weak self] in self?.core }
    /// Last scene phase seen, applied to the core once it is open (it may change meanwhile).
    private var scenePhase = ScenePhase.active
    /// The app went to the background since the core last came to the foreground.
    private var inBackground = true
    private(set) var mode: CoreMode
    private let paths: () throws -> CorePaths

    /// The container of the app (one per process): what the background tasks of iOS reach.
    @ObservationIgnored static weak var current: AppContainer?

    init(mode: CoreMode = AppContainer.defaultMode(), paths: @escaping () throws -> CorePaths = { try CorePaths.applicationSupport() }) {
        self.mode = mode
        self.paths = paths
        Self.current = self
        // Links, QR codes and installation tokens are read by the core (one reading for the three platforms): before any
        // link is read, and before the screens that read them are built.
        DeepLinkParser.reader = CoreLinkReader()
        observeSecurityEvents()
        notifications.registerCategories()
        NotificationRouter.shared.handler = { [weak self] in self?.handleNotification($0) }
    }

    // MARK: Security events

    /// The device locks: the 2-minute biometric window closes and revealed items are masked. A screenshot was taken
    /// (iOS cannot prevent it): what was revealed is masked at once. The screen starts or stops being recorded.
    private func observeSecurityEvents() {
        #if os(macOS)
        // The screen locks, the Mac sleeps or the session is switched: the 2-minute window closes and revealed items
        // are masked. macOS tells neither a screenshot nor a recording.
        let lock = { [weak self] (_: Notification) -> Void in
            MainActor.assumeIsolated {
                self?.gate.window.invalidate()
                self?.surfaceLeft(closed: false)
                // Nobody is at the Mac: the decrypted files do not wait for their minute.
                self?.purgeClearCopies()
            }
        }
        let workspace = NSWorkspace.shared.notificationCenter
        observers = [
            // Back from sleep: the doorbell of the core starts afresh (its connection died with the sleep) and what
            // arrived meanwhile is fetched.
            workspace.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.wokeUp() }
            },
            DistributedNotificationCenter.default().addObserver(
                forName: Notification.Name("com.apple.screenIsLocked"), object: nil, queue: .main, using: lock
            ),
            workspace.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main, using: lock),
            workspace.addObserver(forName: NSWorkspace.screensDidSleepNotification, object: nil, queue: .main, using: lock),
            workspace.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification, object: nil, queue: .main, using: lock),
            // Quitting before the 30 s of a secret copy: it does not stay on the clipboard.
            NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { SensitiveClipboard.clearSecret() }
            },
        ]
        #else
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: UIApplication.protectedDataWillBecomeUnavailableNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.gate.window.invalidate()
                    self?.feed?.hideAll()
                }
            },
            center.addObserver(forName: UIApplication.userDidTakeScreenshotNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.feed?.hideAll() }
            },
            center.addObserver(forName: UIScreen.capturedDidChangeNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.updateCaptured() }
            },
        ]
        updateCaptured()
        #endif
    }

    private func updateCaptured() {
        #if os(macOS)
        // macOS has no public "the screen is being recorded": the window is kept out of captures instead.
        #else
        var captured = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.screen.isCaptured }.contains(true)
        #if DEBUG
        // The simulator is never "captured": `-TomiDebugScreenCaptured YES` shows the cover there.
        if UserDefaults.standard.bool(forKey: "TomiDebugScreenCaptured") { captured = true }
        #endif
        // Only when the capture starts: this also runs each time the app becomes active again, which Face ID itself
        // causes (a secret just revealed under Face ID must not be masked by its own prompt).
        let started = captured && !screenCaptured
        screenCaptured = captured
        if started {
            feed?.hideAll()
            FeedViewModel.screenCaptureStarted()
        }
        #endif
    }

    #if os(macOS)
    // MARK: Life of the Mac app
    //
    // The Mac app has no scene phase: it runs for days in the menu bar, its popover or its window shown now and then.
    // "On screen" is a surface being used (`surfaceVisible`); the core stays in the foreground the whole time (its
    // doorbell open), and what iOS does when it goes to the background is done here when the surfaces are left.

    /// Asked by the menus and the keyboard of the Mac app, taken by the screens: open the search of the feed, go back
    /// one screen (Escape).
    var searchRequests = 0
    var backRequests = 0
    /// Screens pushed over the first one in the surface on screen (told by the root): Escape goes back while there
    /// are some, then closes the popover.
    var stackDepth = 0

    /// A surface of the app (popover, window) is on screen and used. What arrives while none is: notified.
    private(set) var surfaceVisible = false
    @ObservationIgnored private var purgeTask: Task<Void, Never>?
    /// The decrypted files stay this long after the last use of Tomi: a file just opened in another app (Preview)
    /// must still be there (docs/macos-app.md, A10; the Windows app does the same).
    static var clearCopiesDelay: Duration = .seconds(60)
    @ObservationIgnored private var fallbackSync: Task<Void, Never>?

    func surfaceVisibilityChanged(_ visible: Bool) {
        guard visible != surfaceVisible else { return }
        surfaceVisible = visible
        purgeTask?.cancel()
        purgeTask = nil
        if visible {
            // Back to Tomi: the notifications of what was read meanwhile go, a pending departure gets its attempt.
            itemsChanged()
            retrier?.comeBack()
            scheduler?.retryNow()
            drainShares()
            // What was dropped while no surface was open is told now that someone looks.
            tellExpiredShares()
        } else {
            schedulePurge()
        }
    }

    /// The decrypted files go one minute from now unless a surface is used by then. Armed when the last surface is
    /// left, and when something is decrypted with no surface on screen (a file opened from a notification).
    func schedulePurge() {
        purgeTask?.cancel()
        purgeTask = Task { [weak self] in
            try? await Task.sleep(for: Self.clearCopiesDelay)
            guard !Task.isCancelled, let self, !self.surfaceVisible else { return }
            self.purgeClearCopies()
        }
    }

    /// Erases what Tomi decrypted to show or hand over (the cache of the core, the copies under their names) and
    /// what the cards prepared from the texts. Never the doorbell: the Mac app keeps receiving.
    func purgeClearCopies() {
        purgeTask?.cancel()
        purgeTask = nil
        // Nobody has used Tomi for a minute (or the Mac is locked): the 2 minutes of an authentication end here too,
        // as they do on iOS when the app goes to the background.
        gate.window.invalidate()
        feed?.fileRequest = nil
        FeedViewModel.sweepShareCopies()
        CardTexts.clear()
        guard let core else { return }
        Task { try? await core.purgePlaintextCache() }
    }

    /// The Mac woke up: a new doorbell loop (the core asks for it rather than waiting on a dead connection), and a
    /// sync. The stop also erases the decrypted files, which the lock before the sleep already did.
    private func wokeUp() {
        guard let core, !core.isDemo else { return }
        core.enterBackground()
        core.enterForeground()
        retrier?.comeBack()
        scheduler?.retryNow()
    }

    /// How often the Mac app asks the relay what waits, whatever the doorbell says (Windows: 5 minutes too).
    static var fallbackSyncInterval: Duration = .seconds(5 * 60)

    /// The Mac app runs for days without a window. Its doorbell can die silently (a connection the relay or a box on
    /// the way dropped without telling): a sync every few minutes fetches what it missed, and starts the doorbell
    /// again if it had given up. Never on the demo core, never outside a space.
    private func startFallbackSync(_ core: any TomiCore) {
        fallbackSync?.cancel()
        fallbackSync = nil
        guard !core.isDemo else { return }
        fallbackSync = Task { [weak self, weak core] in
            while !Task.isCancelled {
                try? await Task.sleep(for: Self.fallbackSyncInterval)
                guard !Task.isCancelled, let self, let core, self.core === core else { return }
                if core.state.isPaired { core.enterForeground() }
            }
        }
    }

    /// The network came back: the doorbell is started if it had given up, and what waits is fetched.
    func networkCameBack() {
        scheduler?.retryNow()
        guard let core, !core.isDemo else { return }
        core.enterForeground()
    }

    /// Received items not read yet (the dot of the menu bar icon, the badge of the Dock icon).
    var unreadCount: Int {
        guard let core, core.state.loaded, core.state.isPaired else { return 0 }
        let local = core.state.localDevice?.id
        return core.state.items.filter { item in
            !item.read && !item.archived && local.map { !item.isOutgoing(localDeviceId: $0) } ?? true
        }.count
    }

    /// The surface that shows the app (the popover, the window) is left, or closed: what a hidden item showed is
    /// masked again, and its viewer goes (it lives in the model: it would still be there at the next opening).
    /// `closed`: the surface is gone, every viewer goes with it.
    func surfaceLeft(closed: Bool) {
        guard let feed else { return }
        feed.hideAll()
        if closed {
            feed.viewerRequest = nil
            feed.fileRequest = nil
        } else if let request = feed.viewerRequest, feed.item(id: request.itemId)?.secret != false {
            feed.viewerRequest = nil
        }
    }
    #endif

    func sensitiveScreenAppeared() { sensitiveScreens += 1 }
    func sensitiveScreenDisappeared() { sensitiveScreens = max(0, sensitiveScreens - 1) }

    /// Covers the app: it is leaving the screen (the system takes its snapshot for the app switcher now) or the screen
    /// is captured, while a secret is shown.
    var coversSensitiveContent: Bool {
        let showsSecret = sensitiveScreens > 0 || !(feed?.revealedIds.isEmpty ?? true)
        return showsSecret && (scenePhase != .active || (screenCaptured && !captureAllowedForDev))
    }

    /// Developer builds only (Debug, Dev): "Continuer malgré le partage" of the cover, for an iPhone mirrored to the
    /// Mac while it is developed. Kept in memory for this run only; the code does not exist in Release builds.
    #if DEBUG || TOMI_DEV_TOOLS
    private(set) var captureAllowedForDev = false

    func allowCaptureForDev() {
        captureAllowedForDev = true
    }
    #else
    let captureAllowedForDev = false
    #endif

    /// The cover is there because the screen is shared or recorded while the app is on screen (it then offers a way
    /// out), not because the app is leaving the screen.
    var coverIsForCapture: Bool { screenCaptured && scenePhase == .active }

    /// "Masquer et revenir au fil" of the cover: every secret is masked and the screens that show one are closed.
    func leaveSensitiveContent() {
        feed?.hideAll()
        feed?.viewerRequest = nil
        feed?.fileRequest = nil
        resetPathRequested = true
        leaveEntryRequests += 1
    }

    /// Counts the "Masquer et revenir" of the cover: the entry (pairing code, space creation) goes back to Welcome.
    private(set) var leaveEntryRequests = 0

    // MARK: Notifications

    /// Follows the items of `core` for the notifications, whatever the screen shows (a view is not updated in the
    /// background).
    private func watchItems(_ core: any TomiCore) {
        let id = ObjectIdentifier(core)
        guard watchedCore != id else { return }
        watchedCore = id
        observeItems(core)
    }

    private func observeItems(_ core: any TomiCore) {
        withObservationTracking {
            // Paired too: the first reading after the pairing is the reference, never the first item that arrives.
            _ = core.state.items
            _ = core.state.isPaired
            // The devices too: another one joining is announced (`itemsChanged`).
            _ = core.state.devices
        } onChange: { [weak self] in
            Task { @MainActor [weak self] in
                guard let self, self.core === core, self.watchedCore == ObjectIdentifier(core) else { return }
                self.itemsChanged()
                self.observeItems(core)
            }
        }
    }

    /// The items changed: notifies what arrived while the app is not on screen, takes down what was read or is gone.
    func itemsChanged() {
        guard let core else { return }
        // The share extension refuses to send from a device that is not in a space.
        if core.state.loaded { AppGroup.defaults?.set(core.state.isPaired, forKey: AppGroup.isPairedKey) }
        // Checked at every change, paired or not, so that it sees the moment this device joins.
        let importing = core.state.loaded && pairingGrace.isImporting(paired: core.state.isPaired)
        guard core.state.isPaired else { return }
        let update = notifierState.update(
            items: core.state.items, localDeviceId: core.state.localDevice?.id,
            // On screen or just covered (Control Center, a call): the feed shows it (Android: started).
            appVisible: isOnScreen,
            importing: importing
        )
        notifications.cancel(update.cancel)
        update.show.forEach(notifications.show)
        // Another device joined the space (never in the demo, whose devices are made up).
        if !core.isDemo {
            newDeviceState.update(devices: core.state.devices, appVisible: isOnScreen, importing: importing)
                .forEach(notifications.showDeviceAdded)
        }
    }

    /// The feed is in front of the user: what arrives is not notified.
    private var isOnScreen: Bool {
        #if os(macOS)
        surfaceVisible
        #else
        scenePhase != .background
        #endif
    }

    /// A tap on a notification or one of its buttons (Android `NotificationActionActivity`).
    func handleNotification(_ response: NotificationResponse) {
        guard isReadyToRoute else {
            pendingNotification = response
            return
        }
        #if os(macOS)
        // Acted on with every surface closed: what it decrypts (a file opened) does not wait for a surface to be
        // left to get its minute.
        defer { if !surfaceVisible { schedulePurge() } }
        #endif
        if response.opensDevices {
            if core?.state.isPaired == true { pendingRoute = .devices }
            return
        }
        guard let feed, let item = feed.item(id: response.itemId) else {
            feedback.show(L10n.text("toast_item_not_found"))
            return
        }
        switch response.action {
        case IncomingNotifications.actionCopy?:
            feed.copy(item)
        case IncomingNotifications.actionView?:
            pendingRoute = .item(item.id)
        default:
            switch NotificationTapAction.forItem(item) {
            case .openLink:
                // Checked again here (`forItem` already did): nothing but http(s) is ever handed to the system.
                guard let url = item.url, LinkExtractor.isValidHTTPURL(url), let target = URL(string: url) else {
                    pendingRoute = .item(item.id)
                    break
                }
                feed.onCardOpened(item)
                SystemActions.open(target)
            case .copyText, .authenticateAndCopy:
                // A hidden item asks for biometrics first (`FeedViewModel.copy`).
                feed.copy(item)
            case .openFile:
                feed.openAttachment(item, index: 0)
            case .openCard, .previewImage:
                pendingRoute = .item(item.id)
            }
        }
    }

    // MARK: Links

    /// `tomi-share://…` (and `https://<relay>/l/…` or `/create…` once the build is associated with that relay's domain):
    /// strictly parsed by the core, it only navigates or pre-fills, never sends, joins nor creates by itself. The relay
    /// of a link is not trusted: the screens show it in full.
    func open(_ url: URL) {
        guard let link = DeepLinkParser.parse(url) else { return }
        guard isReadyToRoute else {
            pendingLink = link
            return
        }
        route(link)
    }

    /// The core has read its state and the first screen is chosen: a link or a tap can be acted on.
    private var isReadyToRoute: Bool {
        feed != nil && core?.state.loaded == true && pairingFlow != nil
    }

    private func route(_ link: DeepLink) {
        switch link {
        case .open:
            // Back to the feed.
            if core?.state.isPaired == true { pendingRoute = nil; resetPathRequested = true }
        case let .item(id):
            if core?.state.isPaired == true { pendingRoute = .item(id) }
        case let .send(title, url, text, dropped):
            guard core?.state.isPaired == true, let send else { return }
            send.prefill(title: title, url: url, text: text, droppedParameters: dropped)
            pendingRoute = .send
        case let .pair(relayUrl, code):
            // Only offered, on a device that is not in a space yet: "Rejoindre" does it.
            guard core?.state.isPaired == false else { return }
            pairing.offerLink(relayUrl: relayUrl, code: code)
            pendingJoin = true
        case let .setup(relayUrl, token):
            // Only offered, on a device that is not in a space yet: "Créer mon espace" does it, after the relay is read.
            guard core?.state.isPaired == false else { return }
            pendingSetupLink = ParsedSetupLink(relayUrl: relayUrl, token: token)
            pendingCreate = true
        }
    }

    /// The creation link waiting for the creation screen, taken once (the token is a single-use secret).
    func takeSetupLink() -> ParsedSetupLink? {
        defer { pendingSetupLink = nil }
        return pendingSetupLink
    }

    var core: (any TomiCore)? {
        if case let .ready(core) = phase { return core }
        return nil
    }

    /// Opens the core (off the main thread). Runs once, even when several windows ask.
    func start() async {
        guard case .starting = phase else { return }
        phase = .opening
        // While the core opens: the first use of the text rendering code, off the main thread (`TextRenderingWarmUp`).
        Task.detached(priority: .utility) { TextRenderingWarmUp.run() }
        // The card texts of the core that was open before (the demo, another space) are not this one's.
        CardTexts.clear()
        #if os(macOS)
        // macOS has no expiry of its own for the clipboard: a secret copied by a run that was stopped before its 30 s
        // is still there. Only what carries Tomi's mark; anything else copied since is the user's.
        SensitiveClipboard.clearSecret()
        #endif
        do {
            let paths = try self.paths()
            try paths.prepareSendDirectories()
            FeedViewModel.sweepShareCopies()
            let core = try await CoreFactory.open(
                mode: mode,
                paths: paths,
                appVersion: Self.appVersion,
                deviceName: Self.deviceName,
                demoAssets: Bundle.main.url(forResource: "demo", withExtension: nil)
            )
            let picked = PickedFiles(root: paths.picked)
            picked.wipe()
            let scheduler = InAppSendScheduler(core: core)
            let coordinator = SendCoordinator(core: core, scheduler: scheduler, staging: FileAttachmentStaging(root: paths.outgoing))
            self.picked = picked
            self.scheduler = scheduler
            self.coordinator = coordinator
            shareIntake?.stop()
            if let inbox = ShareInbox.shared() {
                // Once per run: a later core (demo reopened) may follow an intake still finishing a claimed share.
                if !inboxPurged {
                    inboxPurged = true
                    inbox.purge(now: Date())
                }
                let intake = ShareIntake(inbox: inbox, coordinator: coordinator)
                intake.onChanged = { [weak self] in self?.drainShares() }
                intake.onFailed = { [weak self] in self?.feedback.show(L10n.text("ios_share_inbox_failed")) }
                intake.onSkippedFiles = { [weak self] in self?.feedback.show(L10n.text("banner_files_skipped", $0)) }
                intake.onExpired = { [weak self] in self?.noteExpiredShares($0) }
                shareIntake = intake
            }
            send = SendViewModel(core: core, coordinator: coordinator, picked: picked)
            let retrier = DepartureRetrier(core: core)
            self.retrier = retrier
            leave = LeaveSpaceModel(core: core, followUp: retrier)
            feed = FeedViewModel(core: core, prefs: prefs, gate: gate, network: network, feedback: feedback)
            if Self.startsInDemo() { await enterDemoAtLaunch(core) }
            phase = .ready(core)
            AppGroup.defaults?.set(false, forKey: AppGroup.isUnreadableKey)
            apply(scenePhase, to: core)
        } catch WrapKeyError.keychain(errSecMissingEntitlement) {
            // A build that may not use the Keychain: the key may well be there, only out of reach of this build.
            phase = .misconfigured
        } catch {
            // Nothing is logged: the message of a Keychain or storage failure may carry a path.
            phase = .unreadable(resetFailed: false)
            // Launched for a background task before the first unlock, the Keychain is not readable yet: that is not
            // damaged data, the share extension is not told (`runInBackground` lets the screen try again).
            if scenePhase != .background && !Self.launchedInBackground {
                AppGroup.defaults?.set(true, forKey: AppGroup.isUnreadableKey)
            }
        }
    }

    /// "Réessayer" of "Données illisibles": opens again (a Keychain that was not readable yet).
    func retryOpening() {
        switch phase {
        case .unreadable, .misconfigured: break
        default: return
        }
        phase = .starting
        Task { await start() }
    }

    /// "Réinitialiser Tomi" of "Données illisibles" (Android `CoreStartup.reset`): erases the local data of the core,
    /// then its wrapping key, and opens a fresh core; this device is no longer in any space. A wipe that fails leaves
    /// the screen up with a message: a reset that keeps the damaged data must not look successful.
    func resetUnreadableData() {
        guard case .unreadable = phase else { return }
        phase = .opening
        let mode = self.mode
        let paths = self.paths
        Task {
            let wiped = await Task.detached(priority: .userInitiated) { () -> Bool in
                do {
                    let paths = try paths()
                    if case .demo = mode {
                        try? FileManager.default.removeItem(at: paths.demo)
                        return true
                    }
                    if FileManager.default.fileExists(atPath: paths.root.path) {
                        try FileManager.default.removeItem(at: paths.root)
                    }
                    // After the directory: if it could not be erased, the database still needs its key. Best effort
                    // (Android): with no database left, a key that stays is simply reused by the next opening.
                    try? KeychainWrapKeyStore().remove()
                    return true
                } catch {
                    return false
                }
            }.value
            guard wiped else {
                phase = .unreadable(resetFailed: true)
                return
            }
            phase = .starting
            await start()
        }
    }

    /// The core has read its state: chooses the first screen (once), resumes the sends left queued.
    func coreLoaded() {
        guard let core else { return }
        #if os(macOS)
        // The Mac app runs without a window: its delegate calls this as soon as the core is loaded, and every screen
        // that shows (the popover at each opening, the window) calls it again. Once per core.
        guard loadedCore !== core else { return }
        loadedCore = core
        startFallbackSync(core)
        #endif
        if pairingFlow == nil { pairingFlow = !core.state.isPaired }
        itemsChanged()
        watchItems(core)
        if !core.state.isPaired { cleanUpAfterLeaving() }
        // What came before the core was ready (a cold start from a link or a notification).
        if let link = pendingLink {
            pendingLink = nil
            route(link)
        }
        if let response = pendingNotification {
            pendingNotification = nil
            handleNotification(response)
        }
        // A departure left pending is settled first, then a pairing a stop interrupted (spec 7.9.4, Android
        // `settlePairingAndSync`).
        resumeTask?.cancel()
        let retrier = self.retrier
        resumeTask = Task {
            await retrier?.settleAtStart()
            guard !Task.isCancelled, !core.state.isPaired else { return }
            _ = await core.resumePairing()
        }
        resumeQueuedSends()
        drainShares()
    }

    /// Sends the shares of the extension past their undo window (the core must be ready and in a space).
    func drainShares() {
        guard let core, core.state.loaded, core.state.isPaired, let shareIntake else { return }
        Task { await shareIntake.drain() }
    }

    /// Shares were dropped for waiting longer than `ShareInbox.maxAge`: counted (the drop may come from a background
    /// task, with no screen) and told once, on screen.
    private func noteExpiredShares(_ count: Int) {
        if count > 0, let defaults = AppGroup.defaults {
            defaults.set(defaults.integer(forKey: AppGroup.expiredSharesKey) + count, forKey: AppGroup.expiredSharesKey)
        }
        tellExpiredShares()
    }

    private func tellExpiredShares() {
        guard isOnScreen, scenePhase == .active, let defaults = AppGroup.defaults, defaults.integer(forKey: AppGroup.expiredSharesKey) > 0 else { return }
        defaults.set(0, forKey: AppGroup.expiredSharesKey)
        feedback.show(L10n.text("ios_share_inbox_expired"))
    }

    /// This device is no longer in a space (it left, or the space is gone): back to the welcome screen, after the
    /// clean-up of what the app keeps outside the core. The demo core cannot join again once it left: a fresh one.
    func pairingLost() {
        pairing.reset()
        pairingFlow = true
        cleanUpAfterLeaving()
        notifierState.reset()
        newDeviceState.reset()
        // Nothing of the departure (an open dialog, a refusal, a series) nor of a draft outlives the space.
        if let core {
            retrier?.stop()
            let retrier = DepartureRetrier(core: core)
            self.retrier = retrier
            leave = LeaveSpaceModel(core: core, followUp: retrier)
            if let coordinator, let picked { send = SendViewModel(core: core, coordinator: coordinator, picked: picked) }
        }
        #if DEBUG || TOMI_DEV_TOOLS
        // The isolated demo core (the app hosting the tests, the previews) has no space to come back to.
        if case .demo = mode, let core, core.isDemo { reopenDemo() }
        #endif
    }

    /// What the app keeps outside the core goes with the space (Android `LeftSpaceCleanup`), each step on its own and
    /// idempotent: run again at every start while the device is not paired. Kept: the wrapping key, display settings.
    private func cleanUpAfterLeaving() {
        prefs.onboardingDone = false
        gate.window.invalidate()
        feed?.hideAll()
        // What the cards prepared from the texts of the space goes with it.
        CardTexts.clear()
        picked?.wipe()
        // The copies of the files of pending sends (the core refuses them now).
        if let outgoing = try? paths().outgoing {
            try? FileManager.default.removeItem(at: outgoing)
            try? paths().prepareSendDirectories()
        }
        notifications.cancelAll()
        // Shares not sent yet go with the space.
        ShareInbox.shared()?.removeAll()
        AppGroup.defaults?.set(false, forKey: AppGroup.isPairedKey)
        // A secret copied by Tomi (marked concealed) leaves the clipboard; anything else copied there is the user's.
        SensitiveClipboard.clearSecret()
    }

    /// The network or the app came back: one automatic attempt of a pending departure (at most one a minute).
    func departureComeBack() {
        retrier?.comeBack()
    }

    /// The network came back: a send that waits for its next try (up to 15 minutes away) leaves now.
    func retryWaitingSends() {
        scheduler?.retryNow()
    }

    /// Leaves the pairing screen, through the first-launch explanation the first time.
    func finishPairing(showOnboarding: Bool) {
        pairing.reset()
        onboardingFlow = showOnboarding && !prefs.onboardingDone
        pairingFlow = false
    }

    func finishOnboarding() {
        prefs.onboardingDone = true
        onboardingFlow = false
    }

    /// Items of this device left queued (app closed during the undo window, or offline) get a dispatch again. Called
    /// once the core has read its items.
    func resumeQueuedSends() {
        guard let core else { return }
        coordinator?.resumeQueued(localDeviceId: core.state.localDevice?.id)
    }

    // MARK: Background

    /// Items of this device still waiting for the relay.
    var hasQueuedSends: Bool {
        guard let core, let id = core.state.localDevice?.id else { return false }
        return core.state.items.contains { $0.isOutgoing(localDeviceId: id) && $0.deliveries.contains { $0.state == .queued } }
    }

    /// A background task of iOS: opens the core if the app was launched for it, fetches what waits (unless only the
    /// sending was asked) and pushes what is queued, the undo window being long over. True when it could run.
    func runInBackground(sendOnly: Bool) async -> Bool {
        // Launched for the task, without a scene: not on screen (no doorbell, no foreground work).
        if Self.launchedInBackground { scenePhase = .background }
        // The shares of the extension are only read with the device unlocked, so they are not sent from here; one that
        // waited past `ShareInbox.maxAge` is dropped (no content is read for that). Told at the next look.
        if let inbox = ShareInbox.shared() { noteExpiredShares(inbox.dropExpired(now: Date())) }
        var openedHere = false
        if case .starting = phase {
            openedHere = true
            await start()
        }
        // The screen is opening the core meanwhile: wait for it.
        for _ in 0..<50 {
            guard case .opening = phase else { break }
            try? await Task.sleep(for: .milliseconds(100))
        }
        switch phase {
        case .unreadable, .misconfigured:
            // Before the first unlock the files (and sometimes the Keychain) cannot be read: the screen will try
            // again. Only an opening made here is undone; "Données illisibles" already on screen (and its reset
            // failure) stays.
            if openedHere { phase = .starting }
            return false
        default:
            break
        }
        guard let core, !core.isDemo else { return false }
        // The state is read just after the opening: wait for it a moment.
        for _ in 0..<30 where !core.state.loaded {
            try? await Task.sleep(for: .milliseconds(100))
        }
        guard core.state.loaded, core.state.isPaired, !Task.isCancelled else { return false }
        // What arrives now is notified (the screen, which usually starts the following, may never open).
        watchItems(core)
        itemsChanged()
        if !sendOnly { try? await core.sync(pushToken: nil) }
        guard let id = core.state.localDevice?.id else { return true }
        let queued = core.state.items.filter { $0.isOutgoing(localDeviceId: id) && $0.deliveries.contains { $0.state == .queued } }
        for item in queued where !Task.isCancelled && Date().timeIntervalSince(item.createdAt) * 1000 >= Double(SendCoordinator.undoWindowMs) {
            _ = try? await core.dispatch(id: item.id)
        }
        return !Task.isCancelled
    }

    #if DEBUG || TOMI_DEV_TOOLS
    /// A fresh demo core, unpaired (the previous one left its space).
    private func reopenDemo() {
        resumeTask?.cancel()
        retrier?.stop()
        (core as? RustTomiCoreAdapter)?.shutdown()
        mode = .demo(startPaired: false, assets: Bundle.main.url(forResource: "demo", withExtension: nil))
        phase = .starting
        feed = nil
        send = nil
        leave = nil
        retrier = nil
        Task { await start() }
    }
    #endif

    func scenePhaseChanged(_ scenePhase: ScenePhase) {
        self.scenePhase = scenePhase
        if scenePhase == .active { updateCaptured() }
        #if os(iOS)
        // The minute of a copy runs from the moment Tomi is back in front (iOS does not tell when it was made).
        if scenePhase == .active { ClipboardWatcher.shared.look() }
        #endif
        // A background task left the opening to the screen (files not readable yet then): it starts now.
        if scenePhase == .active, case .starting = phase { Task { await start() } }
        // Revealed items are masked as soon as the app is no longer active (app switcher, Control Center, a call), before
        // the system takes its snapshot. The authentication window closes only in the background: Face ID itself makes
        // the scene inactive.
        if scenePhase != .active { feed?.hideAll() }
        if let core { apply(scenePhase, to: core) }
    }

    /// Foreground only after a real stay in the background (not after Control Center or a
    /// notification banner, which only pass through `.inactive`).
    private func apply(_ scenePhase: ScenePhase, to core: any TomiCore) {
        switch scenePhase {
        case .active where inBackground:
            inBackground = false
            core.enterForeground()
            retrier?.comeBack()
            // What a background task dropped is told now; then what waits is sent, and the rest past its age goes.
            tellExpiredShares()
            scheduler?.retryNow()
            drainShares()
        case .background where !inBackground:
            inBackground = true
            // The core erases its decrypted files and the links under their names go: an open file viewer would
            // point to nothing (not on `inactive`: Face ID makes the scene inactive).
            feed?.fileRequest = nil
            FeedViewModel.sweepShareCopies()
            #if os(iOS)
            if !core.isDemo { BackgroundWork.schedule(hasQueued: hasQueuedSends) }
            #endif
            gate.window.invalidate()
            core.enterBackground()
            // What the cards prepared from the texts is not kept while the app is off screen.
            CardTexts.clear()
        default:
            break
        }
    }

    /// Every build runs on the one core (the unified core); the demo is entered from the pairing screen, never at
    /// launch. The relay of the build (`TOMI_RELAY_URL`, `Config/Local.xcconfig`) is optional: without one the relay
    /// comes from the link or QR code of the pairing, the "Relay" field or the creation link, and a device that is in
    /// a space keeps the relay of that space. Developer builds (Debug, Dev) take three launch arguments:
    /// `-TomiRelayURL <url>` (a local relay), `-TomiStartInDemo YES` (`startsInDemo`) and `-TomiIsolatedDemo YES`.
    ///
    /// The app that hosts the unit tests is the exception: it must touch neither the space of the person at this
    /// device nor the network, so it opens the isolated demo core (`testing`).
    nonisolated static func defaultMode(
        arguments: UserDefaults = .standard, bundle: Bundle = .main, testing: Bool = AppContainer.isHostingTests
    ) -> CoreMode {
        var relay = configuredRelay(bundle: bundle)
        #if DEBUG || TOMI_DEV_TOOLS
        // `-TomiIsolatedDemo YES`: the same isolated demo core, to look at the app on a device that is in a real space
        // without opening that space (the demo of the unified core can only be entered from outside a space).
        if testing || arguments.bool(forKey: "TomiIsolatedDemo") {
            return .demo(startPaired: true, assets: bundle.url(forResource: "demo", withExtension: nil))
        }
        if let local = arguments.string(forKey: "TomiRelayURL").flatMap(URL.init(string:)) { relay = local }
        #endif
        return .real(relayURL: relay)
    }

    /// The relay compiled into the build, nil when there is none (empty setting, or an unresolved `$(...)`).
    nonisolated static func configuredRelay(bundle: Bundle = .main) -> URL? {
        relayURL(fromSetting: bundle.object(forInfoDictionaryKey: "TomiRelayURL") as? String)
    }

    /// The value of the `TOMI_RELAY_URL` build setting as the Info.plist carries it.
    nonisolated static func relayURL(fromSetting setting: String?) -> URL? {
        guard let text = setting?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty, !text.hasPrefix("$(") else {
            return nil
        }
        return URL(string: text)
    }

    /// The app was launched by XCTest to host the unit tests.
    nonisolated static var isHostingTests: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
    }

    /// Developer builds, `-TomiStartInDemo YES`: the core enters its demo at launch when the device is in no space
    /// (to look at the app without a space, without a click). Never in a Release build.
    nonisolated static func startsInDemo(arguments: UserDefaults = .standard) -> Bool {
        #if DEBUG || TOMI_DEV_TOOLS
        arguments.bool(forKey: "TomiStartInDemo")
        #else
        false
        #endif
    }

    /// Waits for the core to have read its state, then enters the demo if it can (no space, built with the demo).
    private func enterDemoAtLaunch(_ core: RustTomiCoreAdapter) async {
        for _ in 0..<100 where !core.state.loaded { try? await Task.sleep(for: .milliseconds(50)) }
        if core.canEnterDemo { try? await core.enterDemo() }
    }

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    static var deviceName: String {
        SystemActions.deviceName
    }

    /// iOS launched the app for a background task, without a scene. Never on macOS: the app has no such launch.
    private static var launchedInBackground: Bool {
        #if os(iOS)
        UIApplication.shared.applicationState == .background
        #else
        false
        #endif
    }
}
