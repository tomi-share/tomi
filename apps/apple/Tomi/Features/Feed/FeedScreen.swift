import QuickLook
import SwiftUI
import UserNotifications
import TomiDesign
import TomiKit

/// The feed (Android `FeedScreen`): logotype header with the offline or filter chip and the "⋮" menu, or the search
/// field with its filter chips; "Épinglés" then "Récents" (or "Archives"); empty states; floating toolbar; feedback
/// banner. The whole page scrolls, header included (the toolbar keeps search and filters at hand). Hidden items are
/// masked again when the app leaves the screen (`AppContainer`).
struct FeedScreen: View {
    @Bindable var model: FeedViewModel
    let onOpenItem: (String) -> Void
    let onDevices: () -> Void
    let onSend: () -> Void
    /// "Aide au partage": the first-launch explanation, reachable again.
    var onShareHelp: () -> Void = {}
    /// The departure of this device: banner, "Se déconnecter" and its dialogs.
    var leave: LeaveSpaceModel?
    var gate: BiometricGate?
    /// The send that just left the send screen, still within its undo window: its "Envoyé / Annuler" banner follows
    /// the user to the feed. "Annuler" (or "Options") takes the item back into the send screen.
    var send: SendViewModel?
    var onUndoSend: () -> Void = {}
    /// The feed is the screen on top of the stack: nothing is pushed over it (a received card is only read while it is
    /// looked at).
    var isOnTop = true

    @State private var searching = false
    /// The page is being scrolled down (Android: the header hides). The bottom bar does not follow: it stays in place on
    /// every screen, as on Android.
    @State private var scrollingDown = false
    /// Reads the cards that stay on screen.
    @State private var reading = SeenReading()
    @Environment(\.scenePhase) private var scenePhase
    /// The content passes under the status block of the top band (unfolded iPhone Duo in portrait).
    @State private var underStatus = false
    /// The header that scrolls with the page is out of sight: scrolling up brings a copy of it back at once, scrolling
    /// down hides it (Android "enterAlways").
    @State private var pastHeader = false
    @FocusState private var searchFocused: Bool
    /// With VoiceOver the copy of the header is never drawn: it would cover cards VoiceOver can still reach.
    @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver
    #if os(macOS)
    /// The surface of the Mac app the feed is in (popover or window).
    @Environment(\.macSurface) private var macSurface
    @Environment(AppContainer.self) private var container
    /// The card the keyboard is on (arrows), drawn with a ring.
    @State private var keyboardSelection: String?
    #endif

    private static let topId = "feed-top"

    var body: some View {
        ActionBarScaffold { layout in
            ZStack(alignment: .bottom) {
                TomiColor.bg.ignoresSafeArea()
                feed(layout)
                if showsStickyHeader { stickyHeader(layout) }
                if let band = layout.topBand, band.holdsStatus { statusBackdrop(band) }
                if let send, let sent = send.submission {
                    SentBanner(deadline: sent.deadline, skippedFiles: sent.skippedFiles, undoing: send.undoing, onUndo: onUndoSend)
                        .padding(.horizontal, TomiSpace.screen)
                        .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 96)
                        .frame(maxWidth: TomiLayout.readableWidth)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                // Above the "Envoyé" banner while it shows (a "Copié" during its 5 s, `feedbackDuration`).
                FeedbackHost(
                    center: model.feedback,
                    bottomPadding: (layout.placement.isSide ? TomiSpace.s4 : 96) + (send?.submission != nil ? 88 : 0)
                )
            }
            .animation(.easeOut(duration: TomiMotion.medium), value: send?.submission)
            .animation(.easeOut(duration: TomiMotion.medium), value: showsStickyHeader)
        } bar: { axis in
            // At the bottom, the bar steps aside while typing a search (the keyboard is up).
            if axis == .vertical || !searchFocused {
                FloatingToolbar(model: model, searching: $searching, axis: axis, onDevices: onDevices, onSend: onSend)
            }
        }
        .onChange(of: searching) { _, now in searchFocused = now }
        .onChange(of: attended, initial: true) { _, now in reading.setAttended(now) }
        #if os(macOS)
        // Command-V on the feed: the send screen opens with what was pasted (the global shortcut, then a paste, then
        // Command-Return). Never while the search field is typed in, nor with another screen over the feed.
        .background(MacPasteShortcut {
            guard isOnTop, !searchFocused, model.viewerRequest == nil, model.fileRequest == nil,
                  let send = container.send, let picked = container.picked, send.editable else { return false }
            Task { await MacSendInput.paste(into: send, picked: picked) }
            onSend()
            return true
        })
        // The arrows move from card to card, Return does what a click on the card does, Space opens what it holds,
        // Command-C copies it (docs/macos-app.md, 2.7).
        .background(MacKeys { event in keyPressed(event) })
        .onChange(of: container.searchRequests) { _, _ in searching = true }
        #endif
        .departureDialogs(leave, gate: gate ?? BiometricGate())
        .onAppear {
            leave?.onEvent = { [feedback = model.feedback] event in feedback.show(Self.message(event)) }
            reading.onSeen = { [model = self.model] ids in
                let seen = ids.compactMap { model.item(id: $0) }
                model.onItemsSeen(seen)
            }
        }
    }

    #if os(macOS)
    private var orderedItems: [TomiItem] {
        let sections = model.sections
        return sections.pinned + sections.recent
    }

    /// True when the key was used here.
    private func keyPressed(_ event: NSEvent) -> Bool {
        guard isOnTop, !searchFocused, model.viewerRequest == nil, model.fileRequest == nil else { return false }
        let items = orderedItems
        guard !items.isEmpty else { return false }
        let current = keyboardSelection.flatMap { id in items.firstIndex { $0.id == id } }
        let command = event.modifierFlags.intersection([.command, .control, .option, .shift])
        switch (event.keyCode, command) {
        case (125, []): // down
            keyboardSelection = items[min((current ?? -1) + 1, items.count - 1)].id
            return true
        case (126, []): // up
            keyboardSelection = items[max((current ?? 1) - 1, 0)].id
            return true
        case (36, []), (76, []): // Return, Enter
            guard let index = current else { return false }
            activate(items[index])
            return true
        case (49, []): // Space
            guard let index = current else { return false }
            let item = items[index]
            if item.attachments.isEmpty { onOpenItem(item.id) } else { activate(item) }
            return true
        default:
            // Command-C by its character (the key is elsewhere on other layouts), when the card has a text or a link.
            guard command == .command, event.charactersIgnoringModifiers?.lowercased() == "c", let index = current,
                  items[index].text != nil || items[index].url != nil else { return false }
            model.copy(items[index])
            return true
        }
    }

    /// What a click on the card does (`CardTap`), from the keyboard, with one difference: only the card knows whether
    /// its text is cut (a click then opens the detail), so from here a cut text does the action of its kind, and a card
    /// whose click would do nothing opens its detail.
    private func activate(_ item: TomiItem) {
        model.onCardOpened(item)
        switch CardTap.actionFor(item, revealed: model.isRevealed(item)) {
        case .openLink:
            if let url = item.url.flatMap(URL.init(string:)) { SystemActions.open(url) }
        case .reveal: model.reveal(item)
        case .hide: model.hide(item.id)
        case .openImage: model.openViewer(item, attachmentIndex: 0)
        case .openCodeFile: model.openCode(item, attachmentIndex: 0)
        case .openAttachment: model.openAttachment(item, index: 0)
        case .openDetail, .none: onOpenItem(item.id)
        }
    }
    #endif

    /// A received card is read when it stays on screen (`SeenReading`): only while the app is active and the feed is the
    /// screen on top (nothing pushed over it, no viewer over the stack). The time spent elsewhere does not count.
    private var attended: Bool {
        scenePhase == .active && isOnTop && model.viewerRequest == nil && model.fileRequest == nil
    }

    /// Room under the scroll view taken by the floating bar (the cards pass under it): none in the side strip.
    private func barClearance(_ layout: ScreenLayout) -> Double { layout.placement.isSide ? 0 : 96 }

    /// The one-line confirmations of the departure (Android `DepartureEventsEffect`).
    private static func message(_ event: LeaveEvent) -> String {
        switch event {
        case let .cancelled(kind): L10n.text(kind == .leave ? "departure_cancelled_leave" : "departure_cancelled_delete")
        case .failed: L10n.text("departure_failed")
        case .biometricUnavailable: L10n.text("delete_space_unavailable_biometric")
        }
    }

    /// Glass behind the status block of the top band while the content passes under it: the time, the camera and the
    /// Wi-Fi float over the feed instead of taking a band of their own. Centred on the status content, 14 pt narrower
    /// than the occupied corner on each side, with the same margin above and below the content.
    private func statusBackdrop(_ band: TopCornerBand) -> some View {
        let height = CGFloat(band.backdropHeight)
        let top = CGFloat(band.rowCenter) - height / 2
        let inset: CGFloat = 14
        return HStack(spacing: 0) {
            if band.leading > 0 {
                Color.clear.frame(width: CGFloat(band.leading) - 2 * inset, height: height).tomiGlass(in: Capsule())
                    .padding(.horizontal, inset)
            }
            Spacer(minLength: 0)
            if band.trailing > 0 {
                Color.clear.frame(width: CGFloat(band.trailing) - 2 * inset, height: height).tomiGlass(in: Capsule())
                    .padding(.horizontal, inset)
            }
        }
        .padding(.top, top)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .ignoresSafeArea(.container, edges: .top)
        .opacity(underStatus ? 1 : 0)
        .animation(.easeOut(duration: TomiMotion.short), value: underStatus)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    // MARK: Header

    private var showsStickyHeader: Bool { pastHeader && !scrollingDown && !searching && !voiceOver }

    /// The header back over the page while scrolling up, on the background of the page (under the status bar too).
    /// Hidden from VoiceOver (never drawn while it runs): the header of the page stays the one it reads.
    private func stickyHeader(_ layout: ScreenLayout) -> some View {
        header(layout)
            .padding(.bottom, TomiSpace.s1)
            .background(TomiColor.bg.ignoresSafeArea(.container, edges: .top))
            .overlay(alignment: .bottom) { Rectangle().fill(TomiColor.border).frame(height: 0.5) }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .ignoresSafeArea(.container, edges: layout.topBand == nil ? [] : .top)
            .transition(.move(edge: .top).combined(with: .opacity))
            .accessibilityHidden(true)
    }

    /// In the top band, beside the status block and centred on it; elsewhere at the top of the safe area.
    private func header(_ layout: ScreenLayout) -> some View {
        HStack(spacing: TomiSpace.s2) {
            Image("TomiLogotype")
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(height: 28)
                .foregroundStyle(TomiColor.logo)
                .accessibilityLabel(Text("app_name"))
            Spacer()
            if model.showsOfflineChip {
                chip(text: L10n.text("banner_offline"), icon: .offline, tint: TomiColor.secondary)
            } else if model.filter != .all {
                Button { model.filter = .all } label: {
                    chip(text: L10n.text(model.filter.labelKey), icon: model.filter.icon, tint: TomiColor.brand, closable: true)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("action_clear_filter"))
            }
            headerMenu
        }
        .frame(height: 56)
        .modifier(BandPadding(layout: layout, rowHeight: 56))
    }

    private func chip(text: String, icon: TomiIcon, tint: Color, closable: Bool = false) -> some View {
        HStack(spacing: TomiSpace.s1) {
            icon.image.font(.system(size: 13))
            Text(text).tomiText(.label)
            if closable { TomiIcon.close.image.font(.system(size: 11, weight: .semibold)) }
        }
        .foregroundStyle(tint)
        .padding(.horizontal, TomiSpace.s3)
        .frame(height: 32)
        .background(TomiColor.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(TomiColor.border))
    }

    private var headerMenu: some View {
        Menu {
            #if DEBUG || TOMI_DEV_TOOLS
            if model.core.isDemo {
                Button(L10n.text("ios_debug_simulate_link")) { model.simulateIncoming(.link) }
                Button(L10n.text("ios_debug_simulate_text")) { model.simulateIncoming(.text) }
                Button(L10n.text("ios_debug_simulate_image")) { model.simulateIncoming(.image) }
                Button(L10n.text("ios_debug_simulate_secret")) { model.simulateIncoming(.secret) }
                Button(L10n.text("ios_debug_simulate_unreadable")) { model.simulateIncoming(.unreadable) }
                #if os(iOS)
                // To see a notification: 5 s to leave the app first.
                Button(L10n.text("ios_debug_simulate_later")) {
                    let task = UIApplication.shared.beginBackgroundTask()
                    Task {
                        _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
                        try? await Task.sleep(for: .seconds(5))
                        model.simulateIncoming(.secret)
                        try? await Task.sleep(for: .seconds(3))
                        UIApplication.shared.endBackgroundTask(task)
                    }
                }
                #endif
            }
            #endif
            Button(L10n.text("filter_archives")) { model.filter = .archives }
            Button(L10n.text("menu_share_help"), action: onShareHelp)
            #if os(macOS)
            if let macSurface {
                Divider()
                if macSurface.isPopover {
                    Button(L10n.text("macos_menu_window"), action: macSurface.openWindow)
                }
                Button(L10n.text("macos_menu_settings"), action: macSurface.openSettings)
            }
            #endif
            #if DEBUG
            if model.core.isDemo {
                Menu(L10n.text("ios_debug_departure")) {
                    ForEach(DemoDepartureScenario.allCases, id: \.self) { scenario in
                        Button(L10n.text(scenario.debugLabelKey)) {
                            Task { await model.core.simulateDeparture(scenario) }
                        }
                    }
                }
            }
            #endif
            // Only when nothing is pending nor running: the banner handles the rest.
            if let leave, leave.canOfferLeave {
                Divider()
                Button(role: .destructive, action: leave.open) {
                    Label(L10n.text("menu_leave_space"), systemImage: "rectangle.portrait.and.arrow.right")
                }
            }
        } label: {
            TomiIcon.more.image
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(TomiColor.text)
                .frame(width: 44, height: 44)
                .tomiGlass(in: Circle(), fallback: .clear)
                .contentShape(Circle())
        }
        .menuOrder(.fixed)
        .accessibilityLabel(Text("action_more"))
    }

    private func searchHeader(_ layout: ScreenLayout) -> some View {
        VStack(alignment: .leading, spacing: TomiSpace.s2) {
            HStack(spacing: TomiSpace.s2) {
                Button {
                    model.setQuery("")
                    searching = false
                } label: {
                    TomiIcon.back.image.font(.system(size: 17, weight: .semibold)).foregroundStyle(TomiColor.text)
                        .frame(width: 32, height: 44)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text("action_close_search"))
                TextField(L10n.text("search_placeholder"), text: Binding(get: { model.query }, set: { model.setQuery($0) }))
                    .tomiText(.body)
                    .focused($searchFocused)
                    // The results are already live: "Rechercher" only closes the keyboard, the query stays.
                    .submitLabel(.search)
                    .autocorrectionDisabled()
                if !model.query.isEmpty {
                    Button { model.setQuery("") } label: {
                        TomiIcon.close.image.foregroundStyle(TomiColor.textMuted).frame(width: 32, height: 44)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, TomiSpace.s3)
            .tomiGlass(in: Capsule(), fallback: TomiColor.surfaceAlt)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: TomiSpace.s2) {
                    ForEach(FeedFilter.allCases, id: \.self) { filter in
                        TomiChip(LocalizedStringKey(filter.labelKey), icon: model.filter == filter ? .sent : nil, selected: model.filter == filter) {
                            model.filter = filter
                        }
                    }
                }
            }
            if let summary = searchSummary {
                Text(summary).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            }
        }
        .padding(.vertical, TomiSpace.s2)
        // The field (44 pt) is the row centred in the band; the chips and the count follow, as narrow.
        .modifier(BandPadding(layout: layout, rowHeight: 44 + 2 * TomiSpace.s2))
    }

    /// "2 résultats", with the note on the hidden items when one is among the results of a query; nothing without a
    /// query nor a filter (`FeedSearchDisplay`).
    private var searchSummary: String? {
        let results = model.visibleItems
        guard FeedSearchDisplay.showsCounter(query: model.query, filter: model.filter) else { return nil }
        let count = L10n.plural("search_results", results.count)
        guard FeedSearchDisplay.showsMaskedNote(query: model.query, results: results) else { return count }
        return L10n.text("search_summary_with_note", count, L10n.text("search_masked_note"))
    }

    // MARK: Feed

    private func feed(_ layout: ScreenLayout) -> some View {
        let sections = model.sections
        let band = layout.topBand
        // While searching, the results are one flat list (no section titles): pinned first, then the others.
        let flat = FeedSearchDisplay.isFlat(query: model.query)
        return ScrollViewReader { proxy in
            ScrollView {
                VStack(spacing: 0) {
                    Group {
                        if searching { searchHeader(layout) } else { header(layout) }
                    }
                    .id(Self.topId)
                    // Any demo (made-up data): said at all times, with the way out when it was entered from the real core.
                    if model.core.isDemo {
                        let leaveDemo: (() -> Void)? = model.core.canLeaveDemo ? { Task { await model.core.leaveDemo() } } : nil
                        DemoBanner(onLeave: leaveDemo)
                            .padding(.horizontal, TomiSpace.screen)
                            .padding(.top, TomiSpace.s2)
                            .frame(maxWidth: TomiLayout.readableWidth)
                            .frame(maxWidth: .infinity)
                    }
                    // A removal whose key rotation keeps failing: calm and persistent, gone as soon as the core clears it.
                    if model.showsRotationStalled {
                        RotationStalledBanner()
                            .padding(.horizontal, TomiSpace.screen)
                            .padding(.top, TomiSpace.s2)
                            .frame(maxWidth: TomiLayout.readableWidth)
                            .frame(maxWidth: .infinity)
                    }
                    // What the departure is doing, at all times, under the header (Android `DepartureBanner`).
                    if let leave, let gate {
                        DepartureBannerView(model: leave, gate: gate)
                            .padding(.horizontal, TomiSpace.screen)
                            .padding(.top, TomiSpace.s2)
                            .frame(maxWidth: TomiLayout.readableWidth)
                            .frame(maxWidth: .infinity)
                    }
                    if model.loaded && sections.isEmpty {
                        emptyState
                    } else {
                        LazyVStack(alignment: .leading, spacing: TomiSpace.s3) {
                            if !sections.pinned.isEmpty {
                                if !flat { sectionHeader("section_pinned", count: sections.pinned.count, icon: .pinned) }
                                ForEach(sections.pinned) { item in card(item, swipe: false) }
                            }
                            if !sections.recent.isEmpty {
                                if !flat {
                                    sectionHeader(model.filter == .archives ? "section_archives" : "section_recent", count: nil, icon: nil)
                                }
                                ForEach(sections.recent) { item in card(item, swipe: true) }
                            }
                        }
                        .padding(.horizontal, TomiSpace.screen)
                        .padding(.top, TomiSpace.s1)
                        // At the bottom, the last card stays entirely above the floating bar.
                        .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 112)
                        // A readable width on the wide screens (unfolded iPhone Duo, landscape), centred.
                        .frame(maxWidth: TomiLayout.readableWidth)
                        .frame(maxWidth: .infinity)
                        .animation(.easeOut(duration: TomiMotion.medium), value: sections)
                    }
                }
            }
            // With a top band, the page starts at the top of the screen and its header sits beside the status block.
            .ignoresSafeArea(.container, edges: band == nil ? [] : .top)
            .scrollDismissesKeyboard(.interactively)
            // The zone where a card counts as seen: the scroll view, less the floating bar over its bottom.
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                reading.setViewport(height: Double(height), clearance: barClearance(layout))
            }
            .modifier(ScrollDirectionReporter { down in
                if scrollingDown != down { scrollingDown = down }
                // Scrolling closes the archive pill left open.
                if model.openSwipeItemId != nil { model.openSwipeItemId = nil }
            } onLeftTop: { left in
                if underStatus != left { underStatus = left }
            } onPastHeader: { past in
                if pastHeader != past { pastHeader = past }
            })
            // Just sent: the item shows at the top of the feed, under its "Envoyé / Annuler" banner.
            .onChange(of: send?.submission?.id) { _, id in
                // Only where the item shows: not under another filter nor in a search.
                guard id != nil, model.filter == .all, model.query.isEmpty, !searching else { return }
                withAnimation(.easeOut(duration: TomiMotion.medium)) { proxy.scrollTo(Self.topId, anchor: .top) }
            }
            #if os(macOS)
            // The card the keyboard moved to comes into view.
            .onChange(of: keyboardSelection) { _, id in
                guard let id else { return }
                withAnimation(.easeOut(duration: TomiMotion.short)) { proxy.scrollTo(id, anchor: .center) }
            }
            #endif
            // The search field is at the top of the page: opening the search from the toolbar scrolls back to it.
            .onChange(of: searching) { _, now in
                if now { withAnimation(.easeOut(duration: TomiMotion.medium)) { proxy.scrollTo(Self.topId, anchor: .top) } }
            }
        }
    }

    private func sectionHeader(_ key: LocalizedStringKey, count: Int?, icon: TomiIcon?) -> some View {
        HStack(spacing: TomiSpace.s1) {
            if let icon { icon.image.font(.system(size: 12)).foregroundStyle(TomiColor.textMuted) }
            Text(key).tomiText(.subtitle).foregroundStyle(TomiColor.text)
            Spacer()
            if let count { Text("\(count)").tomiText(.caption).monospacedDigit().foregroundStyle(TomiColor.textMuted) }
        }
        .padding(.top, TomiSpace.s5 - TomiSpace.s3)
        .accessibilityAddTraits(.isHeader)
    }

    /// The card, and where it is in the scroll view: `SeenReading` reads it once it stayed on screen.
    private func card(_ item: TomiItem, swipe: Bool) -> some View {
        cardContent(item, swipe: swipe)
            #if os(macOS)
            .overlay {
                if keyboardSelection == item.id {
                    RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.brand, lineWidth: 2)
                }
            }
            #endif
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .scrollView) } action: { frame in
                reading.setFrame(item.id, minY: Double(frame.minY), height: Double(frame.height))
            }
            .onAppear { reading.appeared(item.id) }
            .onDisappear { reading.disappeared(item.id) }
    }

    @ViewBuilder
    private func cardContent(_ item: TomiItem, swipe: Bool) -> some View {
        let open = { onOpenItem(item.id) }
        if swipe, let action = ArchiveSwipe.swipeAction(item, localDeviceId: model.localDeviceId) {
            SwipeToArchive(item: item, action: action, model: model) {
                ItemCard(item: item, model: model, onOpenDetail: open)
            }
        } else {
            ItemCard(item: item, model: model, onOpenDetail: open)
        }
    }

    private var emptyState: some View {
        let key: String = model.filter == .archives && model.query.isEmpty
            ? "feed_empty_archives"
            : (!model.query.isEmpty || model.filter != .all ? "feed_empty_filtered" : "feed_empty")
        return Text(L10n.text(key))
            .tomiText(.body)
            .foregroundStyle(TomiColor.textMuted)
            .multilineTextAlignment(.center)
            .padding(.horizontal, TomiSpace.s6)
            .frame(maxWidth: .infinity)
            // In the middle of the screen, under the header that scrolls with it.
            .containerRelativeFrame(.vertical) { height, _ in height * 0.6 }
    }
}

/// The horizontal margins of the header, and its line: centred on the status content (in the top band beside the
/// occupied corners, or below the top edge of iPhone Duo), under the status bar on a classic iPhone. Never narrower than
/// the screen margin; readable width on wide screens.
private struct BandPadding: ViewModifier {
    let layout: ScreenLayout
    /// Height of the row to line up.
    let rowHeight: CGFloat

    func body(content: Content) -> some View {
        let band = layout.topBand
        content
            .padding(.leading, max(TomiSpace.screen, CGFloat(band?.leading ?? 0)))
            .padding(.trailing, max(TomiSpace.screen, CGFloat(band?.trailing ?? 0)))
            .padding(.top, layout.topPadding(rowHeight: rowHeight))
            .frame(maxWidth: TomiLayout.readableWidth)
            .frame(maxWidth: .infinity)
    }
}

/// Reports a scroll of at least `threshold` points in one direction (true: down), summed over the movement so that a
/// slow scroll counts too, and the return near the top (false). Small moves (a horizontal swipe that drifts) report
/// nothing. Also reports when the page leaves its very top and comes back (`onLeftTop`). iOS 18 and later; on iOS 17
/// the header of the page simply stays where it is (no copy of it over the page).
private struct ScrollDirectionReporter: ViewModifier {
    let onChange: (_ scrollingDown: Bool) -> Void
    let onLeftTop: (_ left: Bool) -> Void
    let onPastHeader: (_ past: Bool) -> Void
    private let threshold: CGFloat = 24
    private let top: CGFloat = 40
    /// Below this offset the header of the page is out of sight (its row and the status bar above it).
    private let headerOut: CGFloat = 120

    init(
        onChange: @escaping (_ scrollingDown: Bool) -> Void,
        onLeftTop: @escaping (_ left: Bool) -> Void,
        onPastHeader: @escaping (_ past: Bool) -> Void
    ) {
        self.onChange = onChange
        self.onLeftTop = onLeftTop
        self.onPastHeader = onPastHeader
    }

    @State private var anchor: CGFloat = 0
    @State private var atTop = true

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: CGFloat.self) { geometry in
                geometry.contentOffset.y + geometry.contentInsets.top
            } action: { _, offset in
                onLeftTop(offset > TomiSpace.s2)
                onPastHeader(offset > headerOut)
                if offset < top {
                    anchor = offset
                    if !atTop {
                        atTop = true
                        onChange(false)
                    }
                    return
                }
                atTop = false
                let moved = offset - anchor
                if abs(moved) >= threshold {
                    anchor = offset
                    onChange(moved > 0)
                }
            }
        } else {
            content
        }
    }
}

#if DEBUG
#Preview("Fil") {
    FeedScreen(model: PreviewFixtures.model(), onOpenItem: { _ in }, onDevices: {}, onSend: {})
}

#Preview("Fil vide") {
    FeedScreen(model: PreviewFixtures.model(items: []), onOpenItem: { _ in }, onDevices: {}, onSend: {})
}
#endif

#if DEBUG
extension DemoDepartureScenario {
    /// Debug menu of the demo core: how the relay answers the next departures.
    var debugLabelKey: String {
        switch self {
        case .normal: "ios_debug_departure_normal"
        case .offline: "ios_debug_departure_offline"
        case .serverError: "ios_debug_departure_server"
        case .lastDevice: "ios_debug_departure_last_device"
        case .reset: "ios_debug_departure_reset"
        case .oldRelay: "ios_debug_departure_old_relay"
        }
    }
}
#endif
