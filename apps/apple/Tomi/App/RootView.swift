import SwiftUI
import TomiDesign
import TomiKit

/// Screens reachable from the feed.
enum Route: Hashable {
    case item(String)
    case send
    case devices
    case invite
    case onboarding
    /// The code file reader.
    case code(ViewerRequest)
}

/// Root of the app: the opening state; then, on a device that has not joined a space, the pairing screen (and the
/// first-launch explanation after it); otherwise the feed in a navigation stack (detail, send, devices).
struct RootView: View {
    @Environment(AppContainer.self) private var container

    var body: some View {
        Group {
            if let feed = container.feed, let core = container.core, container.pairingFlow != nil {
                ReadyRoot(container: container, feed: feed, core: core)
            } else if case let .unreadable(resetFailed) = container.phase {
                UnreadableDataScreen(
                    resetFailed: resetFailed,
                    onRetry: { container.retryOpening() },
                    onReset: { container.resetUnreadableData() }
                )
            } else if case .misconfigured = container.phase {
                MisconfiguredScreen { container.retryOpening() }
            } else {
                opening
            }
        }
        // Under the cover, VoiceOver does not read what it hides either.
        .accessibilityHidden(container.coversSensitiveContent)
        .tint(TomiColor.brand)
        // A secret on screen: covered in the app switcher and while the screen is recorded or mirrored.
        .overlay {
            if container.coversSensitiveContent {
                PrivacyCover(
                    captured: container.coverIsForCapture,
                    pairing: container.pairingFlow == true,
                    onLeave: { container.leaveSensitiveContent() },
                    onAllowForDev: allowCaptureForDev
                )
                    .transition(.identity)
            }
        }
        // The first screen is chosen once the core has read its state (paired or not), never before.
        .onChange(of: container.core?.state.loaded == true, initial: true) { _, loaded in
            if loaded { container.coreLoaded() }
        }
    }

    private var allowCaptureForDev: (() -> Void)? {
        #if DEBUG || TOMI_DEV_TOOLS
        return { container.allowCaptureForDev() }
        #else
        return nil
        #endif
    }

    /// The logotype where the header of the feed puts it (beside an occupied corner, on the line of the status content
    /// or of the window controls), so that it does not move when the feed shows.
    private var opening: some View {
        ActionBarScaffold { layout in
            let band = layout.topBand
            ZStack {
                TomiColor.bg.ignoresSafeArea()
                VStack(alignment: .leading, spacing: TomiSpace.s5) {
                    Image("TomiLogotype")
                        .renderingMode(.template)
                        .resizable()
                        .scaledToFit()
                        .frame(height: 32)
                        .foregroundStyle(TomiColor.logo)
                        .accessibilityLabel(Text("app_name"))
                    TomiLinearProgress().frame(maxWidth: 160)
                    Spacer()
                }
                .padding(.leading, max(TomiSpace.screen, CGFloat(band?.leading ?? 0)))
                .padding(.trailing, max(TomiSpace.screen, CGFloat(band?.trailing ?? 0)))
                .padding(.top, layout.headerCenter == nil ? TomiSpace.s4 : layout.topPadding(rowHeight: 32))
                .frame(maxWidth: TomiLayout.readableWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
                .ignoresSafeArea(.container, edges: band == nil ? [] : .top)
            }
        } bar: { _ in
            EmptyView()
        }
    }
}

/// The app once the core is open: pairing, first launch, or the feed.
private struct ReadyRoot: View {
    let container: AppContainer
    let feed: FeedViewModel
    let core: any TomiCore
    @State private var path: [Route] = []

    var body: some View {
        content
            .onChange(of: core.state.isPaired) { _, paired in
                if !paired {
                    path = []
                    container.pairingLost()
                }
            }
            // "Envoyer avec Tomi" (App Shortcut).
            .onReceive(NotificationCenter.default.publisher(for: PendingShortcut.didChange)) { _ in takeShortcut() }
            .onAppear { takeShortcut() }
            // A notification, a link or a shortcut asks for a screen of the feed.
            .onChange(of: container.pendingRoute, initial: true) { _, _ in takePendingRoute() }
            // Asked during the entry (pairing, "Appareil ajouté"): taken once the feed shows.
            .onChange(of: container.pairingFlow) { _, _ in
                takePendingRoute()
                takeShortcut()
                takeRelayRevocations()
            }
            .onChange(of: container.onboardingFlow) { _, _ in takeRelayRevocations() }
            // The relay alone removed a device: a one-time notice, never a blocking one.
            .onChange(of: core.state.relayRevocations, initial: true) { _, _ in takeRelayRevocations() }
            .onChange(of: container.resetPathRequested) { _, reset in
                guard reset else { return }
                container.resetPathRequested = false
                path = []
            }
            // The network came back: one automatic attempt of a pending departure, and the sends that wait leave.
            .onChange(of: container.network.isOnline) { _, online in
                guard online else { return }
                container.departureComeBack()
                container.retryWaitingSends()
            }
            #if os(macOS)
            // Escape on the Mac goes back one screen (`MacSurfaces.escape`). Not from a screen whose way back does
            // more than leave (an invitation being shown, the first-launch page): they keep their own button.
            .onChange(of: path.count, initial: true) { _, depth in container.stackDepth = depth }
            .onChange(of: container.backRequests) { _, _ in
                switch path.last {
                case .invite?, .onboarding?, nil: break
                default: path.removeLast()
                }
            }
            #endif
    }

    @ViewBuilder
    private var content: some View {
        if container.pairingFlow == true {
            EntryFlow(container: container, core: core)
        } else if container.onboardingFlow {
            NavigationStack {
                OnboardingScreen { container.finishOnboarding() }
                    .screenControls()
            }
            .systemBarControls()
        } else {
            stack
        }
    }

    private var stack: some View {
        NavigationStack(path: $path) {
            FeedScreen(
                model: feed,
                onOpenItem: { path.append(.item($0)) },
                onDevices: { path.append(.devices) },
                onSend: { path.append(.send) },
                onShareHelp: { path.append(.onboarding) },
                leave: container.leave,
                gate: container.gate,
                send: container.send,
                onUndoSend: {
                    // The send screen shows the draft back once the core has cancelled, or says "too late".
                    if let send = container.send { Task { await send.undo() } }
                    if path.last != .send { path.append(.send) }
                },
                isOnTop: path.isEmpty
            )
            .hiddenNavigationBar()
            .appWindowTitle()
            .screenControls()
            .navigationDestination(for: Route.self) { route in
                destination(route).screenControls()
            }
        }
        .systemBarControls()
        // One presenter for the whole stack: the feed stays under the detail.
        // The image viewer, over everything (night background, no bars).
        .coverScreen(item: Binding(get: { feed.viewerRequest }, set: { feed.viewerRequest = $0 })) { request in
            ImageViewerScreen(request: request, model: feed, onClose: { feed.viewerRequest = nil }) { id in
                feed.viewerRequest = nil
                path.append(.item(id))
            }
        }
        // Another file (documents, audio, video): the file viewer, over everything like the image viewer.
        .coverScreen(item: Binding(get: { feed.fileRequest }, set: { feed.fileRequest = $0 })) { request in
            FileViewerScreen(request: request, model: feed, onClose: { feed.fileRequest = nil }) { id in
                feed.fileRequest = nil
                path.append(.item(id))
            }
        }
        // A code file: pushed on the stack.
        .onChange(of: feed.codeRequest) { _, request in
            guard let request else { return }
            feed.codeRequest = nil
            path.append(.code(request))
        }
    }

    @ViewBuilder
    private func destination(_ route: Route) -> some View {
        switch route {
        case let .item(id):
            ItemDetailScreen(itemId: id, model: feed)
        case .send:
            if let send = container.send, let picked = container.picked {
                SendScreen(model: send, picked: picked, feedback: container.feedback) {
                    if path.last == .send { path.removeLast() }
                }
            }
        case .devices:
            DevicesScreen(core: core, gate: container.gate, feedback: container.feedback) { path.append(.invite) }
        case .invite:
            InviteScreen(core: core)
        case let .code(request):
            CodeFileScreen(request: request, model: feed, prefs: container.prefs, onClose: {
                // With what was opened from it (its details): nothing of a hidden file stays a "back" away.
                if let index = path.lastIndex(of: .code(request)) { path.removeSubrange(index...) }
            }, onDetails: { path.append(.item($0)) })
        case .onboarding:
            OnboardingScreen(fromHelp: true) {
                container.prefs.onboardingDone = true
                if path.last == .onboarding { path.removeLast() }
            }
        }
    }

    private func takePendingRoute() {
        guard let route = container.pendingRoute, container.pairingFlow == false else { return }
        container.pendingRoute = nil
        path = [route]
    }

    /// Not during the entry (pairing, creation): the notice waits in the state until the feed shows.
    private func takeRelayRevocations() {
        guard container.pairingFlow == false, !container.onboardingFlow else { return }
        feed.showRelayRevocations()
    }

    private func takeShortcut() {
        guard PendingShortcut.openSend, container.pairingFlow == false else { return }
        PendingShortcut.openSend = false
        path = [.send]
    }

}

/// Routes of the entry, a device not in a space yet.
private enum EntryRoute: Hashable {
    case create, join
}

/// Welcome, then "Crée ton espace" or "Rejoins ton espace" (Android `TomiRoot`: `welcome`, `create`, `pair`). From the
/// creation, "Rejoindre l'espace existant" goes on to the join; the end of either leaves the entry for good.
private struct EntryFlow: View {
    let container: AppContainer
    let core: any TomiCore
    @State private var path: [EntryRoute] = []

    var body: some View {
        NavigationStack(path: $path) {
            WelcomeScreen(onCreateSpace: { path.append(.create) }, onJoinSpace: { path.append(.join) })
                .screenControls()
                .navigationDestination(for: EntryRoute.self) { route in
                    Group {
                    switch route {
                    case .create:
                        CreateSpaceDestination(core: core, takeLink: { container.takeSetupLink() }) {
                            path.append(.join)
                        } onFinished: {
                            container.finishPairing(showOnboarding: true)
                        }
                    case .join:
                        PairingScreen(
                            model: container.pairing,
                            localDeviceName: core.state.localDevice?.name ?? AppContainer.deviceName,
                            onFinished: { container.finishPairing(showOnboarding: true) },
                            onDemoEntered: { container.finishPairing(showOnboarding: false) }
                        )
                    }
                    }
                    .screenControls()
                }
        }
        .systemBarControls()
        // A pairing link opened the app: the join screen, with the link offered.
        .onChange(of: container.pendingJoin, initial: true) { _, join in
            guard join else { return }
            container.pendingJoin = false
            path = [.join]
        }
        // A creation link opened the app: the creation screen, with its relay and token offered.
        .onChange(of: container.pendingCreate, initial: true) { _, create in
            guard create else { return }
            container.pendingCreate = false
            path = [.create]
        }
        #if DEBUG
        .onChange(of: container.debugEntryRoute, initial: true) { _, route in
            guard let route else { return }
            container.debugEntryRoute = nil
            path = route == "create" ? [.create] : (route == "join" ? [.join] : [])
        }
        #endif
        // A pairing settled at start-up while the join screen waits on its first step.
        .onChange(of: core.state.isPaired) { _, paired in
            container.pairing.pairedChanged(paired)
            // Paired while on Welcome (a pairing interrupted by a stop, settled at start-up): nothing left to enter.
            if paired && path.isEmpty { container.finishPairing(showOnboarding: true) }
        }
        // "Masquer et revenir" of the privacy cover: the pairing code or the new space leave the screen.
        .onChange(of: container.leaveEntryRequests) { _, _ in path = [] }
        #if os(macOS)
        // Told so that Escape does not close anything here; the screens of the entry go back by their own button
        // only (creating a space or joining one has steps that must not be skipped).
        .onChange(of: path.count, initial: true) { _, depth in container.stackDepth = depth }
        #endif
        .onChange(of: path) { old, new in
            // Back from the join to an earlier screen: the pairing in progress (if any) is left.
            if old.contains(.join) && !new.contains(.join) { container.pairing.reset() }
        }
    }
}

/// "Crée ton espace" with a model of its own: the token lives as long as the screen.
private struct CreateSpaceDestination: View {
    let core: any TomiCore
    /// The creation link that opened the app, if any: taken once, when the model is built.
    let takeLink: () -> ParsedSetupLink?
    let onJoinInstead: () -> Void
    let onFinished: () -> Void
    /// Built once, when the screen shows (not at every evaluation of the destination).
    @State private var model: CreateSpaceViewModel?

    var body: some View {
        Group {
            if let model {
                CreateSpaceScreen(model: model, onJoinInstead: onJoinInstead, onFinished: onFinished)
            } else {
                TomiColor.bg.ignoresSafeArea()
            }
        }
        .onAppear {
            if model == nil {
                let created = CreateSpaceViewModel(core: core, suggestedName: AppContainer.deviceName)
                if let link = takeLink() { created.offerLink(relayUrl: link.relayUrl, token: link.token) }
                #if DEBUG
                // Tests against a local relay on a simulator that cannot type: `-TomiDebugInstallToken <token>`.
                if let token = UserDefaults.standard.string(forKey: "TomiDebugInstallToken") { created.onTokenChange(token) }
                #endif
                model = created
            }
        }
    }
}

#Preview {
    RootView().environment(AppContainer(mode: .demo(startPaired: true, assets: nil)))
}
