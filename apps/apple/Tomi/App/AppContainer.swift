import Foundation
import SwiftUI
import UIKit
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
    /// A notification tap or a link that came before the core was ready (a cold start): replayed once it is.
    @ObservationIgnored private var pendingNotification: NotificationResponse?
    @ObservationIgnored private var pendingLink: DeepLink?
    /// The core whose items are followed (one chain of observation per core).
    @ObservationIgnored private var watchedCore: ObjectIdentifier?
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
    }

    private func updateCaptured() {
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
    }

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
            appVisible: scenePhase != .background,
            importing: importing
        )
        notifications.cancel(update.cancel)
        update.show.forEach(notifications.show)
    }

    /// A tap on a notification or one of its buttons (Android `NotificationActionActivity`).
    func handleNotification(_ response: NotificationResponse) {
        guard isReadyToRoute else {
            pendingNotification = response
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
                UIApplication.shared.open(target)
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
            phase = .ready(core)
            AppGroup.defaults?.set(false, forKey: AppGroup.isUnreadableKey)
            apply(scenePhase, to: core)
        } catch {
            // Nothing is logged: the message of a Keychain or storage failure may carry a path.
            phase = .unreadable(resetFailed: false)
            // Launched for a background task before the first unlock, the Keychain is not readable yet: that is not
            // damaged data, the share extension is not told (`runInBackground` lets the screen try again).
            if scenePhase != .background && UIApplication.shared.applicationState != .background {
                AppGroup.defaults?.set(true, forKey: AppGroup.isUnreadableKey)
            }
        }
    }

    /// "Réessayer" of "Données illisibles": opens again (a Keychain that was not readable yet).
    func retryOpening() {
        guard case .unreadable = phase else { return }
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
        guard scenePhase == .active, let defaults = AppGroup.defaults, defaults.integer(forKey: AppGroup.expiredSharesKey) > 0 else { return }
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
        // Nothing of the departure (an open dialog, a refusal, a series) nor of a draft outlives the space.
        if let core {
            retrier?.stop()
            let retrier = DepartureRetrier(core: core)
            self.retrier = retrier
            leave = LeaveSpaceModel(core: core, followUp: retrier)
            if let coordinator, let picked { send = SendViewModel(core: core, coordinator: coordinator, picked: picked) }
        }
        #if DEBUG
        if let core, core.isDemo { reopenDemo() }
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
        if UIPasteboard.general.contains(pasteboardTypes: [SensitiveClipboard.tomiSecretType]) {
            UIPasteboard.general.items = []
        }
    }

    /// The network or the app came back: one automatic attempt of a pending departure (at most one a minute).
    func departureComeBack() {
        retrier?.comeBack()
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
        if UIApplication.shared.applicationState == .background { scenePhase = .background }
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
        if case .unreadable = phase {
            // Before the first unlock the files cannot be read: the screen will try again. Only an opening made
            // here is undone; "Données illisibles" already on screen (and its reset failure) stays.
            if openedHere { phase = .starting }
            return false
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

    #if DEBUG
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

    /// "Entrer en mode démo" of the pairing screen: the demo core, unpaired, replacing the current one if it is the
    /// real core (debug builds only).
    func openDemoForPairing() async -> (any TomiCore)? {
        if let core, core.isDemo { return core }
        resumeTask?.cancel()
        retrier?.stop()
        (core as? RustTomiCoreAdapter)?.shutdown()
        mode = .demo(startPaired: false, assets: Bundle.main.url(forResource: "demo", withExtension: nil))
        phase = .starting
        feed = nil
        send = nil
        await start()
        return core
    }
    #endif

    func scenePhaseChanged(_ scenePhase: ScenePhase) {
        self.scenePhase = scenePhase
        if scenePhase == .active { updateCaptured() }
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
            drainShares()
        case .background where !inBackground:
            inBackground = true
            // The core erases its decrypted files and the links under their names go: an open file viewer would
            // point to nothing (not on `inactive`: Face ID makes the scene inactive).
            feed?.fileRequest = nil
            FeedViewModel.sweepShareCopies()
            if !core.isDemo { BackgroundWork.schedule(hasQueued: hasQueuedSends) }
            gate.window.invalidate()
            core.enterBackground()
            // What the cards prepared from the texts is not kept while the app is off screen.
            CardTexts.clear()
        default:
            break
        }
    }

    /// Debug builds run on the demo core, already in the demo space (`-TomiStartUnpaired YES`: not yet,
    /// to go through the pairing screen); `-TomiCoreMode real` picks the real core against the debug
    /// relay URL. Release builds always run on the real core. The relay of the build (`TOMI_RELAY_URL`,
    /// `Config/Local.xcconfig`) is optional: without one the relay comes from the link or QR code of the pairing, the
    /// "Relais" field or the creation link.
    nonisolated static func defaultMode(arguments: UserDefaults = .standard, bundle: Bundle = .main) -> CoreMode {
        let relay = configuredRelay(bundle: bundle)
        #if TOMI_DEMO
        if arguments.string(forKey: "TomiCoreMode") != "real" {
            let paired = !arguments.bool(forKey: "TomiStartUnpaired")
            return .demo(startPaired: paired, assets: bundle.url(forResource: "demo", withExtension: nil))
        }
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

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.0.0"
    }

    static var deviceName: String {
        UIDevice.current.name
    }
}
