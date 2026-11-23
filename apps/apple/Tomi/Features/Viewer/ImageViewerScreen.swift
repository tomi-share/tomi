import SwiftUI
import TomiDesign
import TomiKit

/// The internal full-screen image viewer (Android `ImageViewerScreen`), over the whole app.
///
/// - Night background whatever the theme, image centred. The controls show at the opening, and a tap shows or hides
///   them with the status bar (Photos): close at the top, the actions in the floating Liquid Glass bar of the app (at
///   the bottom, or in the side strip of iPhone Duo). Pulling the image down closes the viewer.
/// - EXIF orientation applied at decoding; pinch, double tap and pan (`ZoomableImage`); a horizontal swipe pages
///   between the images of the item, with the position "2/5" at the bottom, gone 2.5 s after the last interaction.
/// - Bar: share (primary), save to Photos and copy (never for a hidden item), delete, and "…" (save to Files, never for
///   a hidden item; details). In the side strip of iPhone Duo the close button is in the bar too.
/// - Hidden item: nothing is decrypted nor drawn before the gate, sharing asks it again (same window), the app switcher
///   shows the privacy cover, and the viewer closes when the app goes to the background.
struct ImageViewerScreen: View {
    let request: ViewerRequest
    let model: FeedViewModel
    let onClose: () -> Void
    let onDetails: (String) -> Void

    /// Night background of the charter (color.dark.bg).
    static let background = Color(red: 0x12 / 255, green: 0x16 / 255, blue: 0x20 / 255)

    @Environment(\.scenePhase) private var scenePhase
    @Environment(AppContainer.self) private var container
    @State private var unlocked = false
    @State private var existed = false
    /// The viewer showed pages once: when the last image goes (removed here or on another device) it closes.
    @State private var hadPages = false

    var body: some View {
        let item = model.item(id: request.itemId)
        let content = ViewerGate.content(item: item, loaded: model.loaded, unlocked: unlocked)
        ZStack {
            // The pages draw their own background, which fades while the image is pulled down.
            if case .pages = content {} else { Self.background.ignoresSafeArea() }
            switch content {
            case .loading:
                EmptyView()
            case .locked:
                message("viewer_locked")
            case .empty:
                message("viewer_no_image")
            case let .pages(images):
                if let item {
                    ViewerPages(item: item, images: images, startAttachmentIndex: request.attachmentIndex, model: model, onClose: onClose) {
                        onDetails(item.id)
                    }
                }
            }
            // The pages place the messages above their bar themselves.
            if case .pages = content {} else { FeedbackHost(center: model.feedback, bottomPadding: 64) }
        }
        .environment(\.colorScheme, .dark)
        // The escape gesture of VoiceOver (two-finger scrub) closes the viewer, whatever the controls.
        .accessibilityAction(.escape) { onClose() }
        .presentationBackground(.clear)
        // Presented above the root: the root's privacy cover would stay under it.
        .overlay {
            if container.coversSensitiveContent {
                PrivacyCover(captured: container.coverIsForCapture) { container.leaveSensitiveContent() }
            }
        }
        .sensitiveScreen(item?.secret == true)
        // A hidden image never stays on a recorded or mirrored screen.
        .onChange(of: container.screenCaptured, initial: true) { _, captured in
            if captured, item?.secret == true { onClose() }
        }
        .task(id: item?.id) {
            guard let item, item.secret else { return }
            unlocked = false
            if await model.authorizeView(item, file: false) { unlocked = true } else { onClose() }
        }
        // Deleting the item from here closes the viewer; an id that never existed shows the empty message.
        .onChange(of: item == nil, initial: true) { _, gone in
            if !gone { existed = true } else if existed { onClose() }
        }
        // Removing the last image of an item that has other content leaves nothing to page through: back to the feed.
        .onChange(of: content, initial: true) { _, new in
            if case .pages = new {
                hadPages = true
            } else if hadPages, case .empty = new {
                onClose()
            }
        }
        // "Supprimer l'image partout" is offered only when the relay can carry it.
        .task { await model.refreshAttachmentRemovalSupport() }
        // A hidden image does not stay on screen when the app leaves it.
        .onChange(of: scenePhase) { _, phase in
            if phase == .background, item?.secret == true { onClose() }
        }
    }

    private func message(_ key: LocalizedStringKey) -> some View {
        ZStack(alignment: .top) {
            Text(key)
                .tomiText(.body)
                .foregroundStyle(.white)
                .multilineTextAlignment(.center)
                .padding(TomiSpace.s6)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            ViewerCloseButton(action: onClose)
                .padding(.leading, TomiSpace.s2)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

/// The pages of the item, the counter and the bars.
private struct ViewerPages: View {
    let item: TomiItem
    let images: [ViewerImage]
    let startAttachmentIndex: Int
    let model: FeedViewModel
    let onClose: () -> Void
    let onDetails: () -> Void

    @State private var page: Int?
    @State private var barsVisible = true
    /// How far the image is pulled down (closing the viewer past `pullToClose`).
    @State private var pull: CGFloat = 0
    private static let pullToClose: CGFloat = 120
    @State private var recentlyActive = true
    /// Attachment index of the image whose "partout" removal waits for its confirmation (fixed when chosen, even if the
    /// pager moves meanwhile).
    @State private var confirmDeleteIndex: Int?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.accessibilityVoiceOverEnabled) private var voiceOver

    /// Clamped: the last image may have just been removed.
    private var current: Int {
        min(page ?? ViewerImages.pageFor(images, attachmentIndex: startAttachmentIndex), max(0, images.count - 1))
    }

    var body: some View {
        ActionBarScaffold { layout in
            ZStack {
                ViewerPager(count: images.count, current: Binding(get: { current }, set: { page = $0 })) { index in
                    let image = images[index]
                    ViewerPage(
                            itemId: item.id, attachmentIndex: image.attachmentIndex,
                            description: L10n.text("viewer_image_description", index + 1, images.count),
                            model: model,
                            // With VoiceOver the controls stay: hidden, they could not be found again.
                            onTap: { if !voiceOver { barsVisible.toggle() } },
                            onPullDown: pulled,
                            protected: item.secret
                    )
                }
                .ignoresSafeArea()
                .offset(y: pull)
                .scaleEffect(1 - min(pull, 400) / 2000)

                if showsCounter {
                    Text(verbatim: L10n.text("viewer_position", current + 1, images.count))
                        .tomiText(.label)
                        .foregroundStyle(.white)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 6)
                        .background(Color.black.opacity(0.55), in: Capsule())
                        // Above the bar when it is at the bottom.
                        .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 96)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
                        .transition(reduceMotion ? .identity : .opacity)
                }

                // Above the bar at the bottom, and above the counter when it shows.
                FeedbackHost(
                    center: model.feedback,
                    bottomPadding: (layout.placement.isSide ? TomiSpace.s4 : 96) + (showsCounter ? 44 : 0)
                )

                // In the side strip (iPhone Duo), the close button joins the bar; else it stays top leading.
                if showsControls && !layout.placement.isSide {
                    // On the line of the status content when the top edge has a band (iPhone Duo), else under the
                    // status bar.
                    let center = layout.headerCenter
                    ViewerCloseButton(action: onClose)
                        .padding(.leading, max(TomiSpace.s2, CGFloat(layout.topBand?.leading ?? 0)))
                        .padding(.top, center.map { max(0, $0 - ViewerCloseButton.size / 2) } ?? TomiSpace.s2)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        .ignoresSafeArea(edges: layout.topBand == nil ? [] : .top)
                        .transition(.opacity)
                }
            }
        } bar: { axis in
            // The actions in the floating Liquid Glass bar of the app: at the bottom, or in the side strip (iPhone Duo).
            if showsControls {
                actionBar(axis).transition(.opacity)
            }
        }
        .animation(.easeOut(duration: TomiMotion.medium), value: barsVisible)
        .animation(.easeOut(duration: TomiMotion.medium), value: recentlyActive)
        // The feed shows through while the image is pulled down.
        .background(ImageViewerScreen.background.opacity(1 - min(pull, 300) / 300).ignoresSafeArea())
        #if os(iOS)
        .statusBarHidden(!barsVisible)
        #endif
        .persistentSystemOverlays(barsVisible ? .automatic : .hidden)
        // The counter wakes up on a page change or a tap, and goes 2.5 s later while the bars are hidden.
        .task(id: "\(current)-\(barsVisible)") {
            recentlyActive = true
            try? await Task.sleep(for: .milliseconds(ViewerCounter.idleMs))
            if !Task.isCancelled { recentlyActive = false }
        }
        .alert(
            L10n.text("dialog_delete_image_everywhere_title"),
            isPresented: Binding(get: { confirmDeleteIndex != nil }, set: { if !$0 { confirmDeleteIndex = nil } })
        ) {
            Button(L10n.text("action_cancel"), role: .cancel) {}
            Button(L10n.text("viewer_delete_everywhere"), role: .destructive) {
                if let index = confirmDeleteIndex { model.deleteAttachment(item, index: index, everywhere: true) }
            }
        } message: {
            Text(L10n.text(item.secret ? "dialog_delete_image_everywhere_secret" : "dialog_delete_image_everywhere_message"))
        }
    }

    /// The controls show with the bars, and step aside while the image is pulled down.
    private var showsControls: Bool { barsVisible && pull == 0 }

    /// "2/5", gone with the controls while the image is pulled down.
    private var showsCounter: Bool {
        images.count > 1 && pull == 0 && ViewerCounter.isVisible(barsVisible: barsVisible, recentlyActive: recentlyActive)
    }

    private var attachmentIndex: Int { ViewerImages.attachmentIndexAt(images, page: current) ?? 0 }

    private func pulled(_ distance: CGFloat, _ ended: Bool) {
        guard ended else {
            pull = distance
            return
        }
        if distance > Self.pullToClose {
            onClose()
        } else {
            withAnimation(.spring(duration: 0.3)) { pull = 0 }
        }
    }

    /// The capsule: close (in the side strip only), the quick actions (save to Photos and copy, never for a hidden
    /// item), delete and "…" (save to Files, details); "Partager" as the primary button.
    private func actionBar(_ axis: Axis) -> some View {
        FloatingActionBar(axis: axis) {
            if axis == .vertical {
                ViewerBarButton(systemName: TomiIcon.close.systemName, label: "ios_viewer_close", action: onClose)
            }
            if ViewerActions.canSaveToGallery(item) {
                ViewerBarButton(systemName: "photo.badge.arrow.down", label: "viewer_save") {
                    model.saveImage(item, attachmentIndex: attachmentIndex)
                }
                ViewerBarButton(systemName: TomiIcon.copy.systemName, label: "ios_copy_image") {
                    model.copyImage(item, attachmentIndex: attachmentIndex)
                }
            }
            // "Supprimer" concerns the image on screen only (the item stays; the core deletes it when nothing is left in it).
            // Not offered while the item is still queued: the core refuses it then.
            if AttachmentRemovals.canRemove(item) {
                Menu {
                    Button { model.deleteAttachment(item, index: attachmentIndex, everywhere: false) } label: {
                        Label(L10n.text("viewer_delete_local"), systemImage: TomiIcon.delete.systemName)
                    }
                    if model.attachmentRemovalEverywhere {
                        Button(role: .destructive) { confirmDeleteIndex = attachmentIndex } label: {
                            Label(L10n.text("viewer_delete_everywhere"), systemImage: "trash.slash")
                        }
                    }
                } label: { FloatingBarIcon(icon: .delete) }
                .accessibilityLabel(Text("viewer_action_delete"))
            }
            Menu {
                // Never Files for a hidden item (no lasting clear copy). On the Mac the button of the bar already saves
                // to the one folder there is.
                if ViewerActions.canSaveToGallery(item), SystemActions.savesImagesInTwoPlaces {
                    Button { model.saveAttachmentToFiles(item, index: attachmentIndex) } label: {
                        Label(L10n.text("ios_save_to_files"), systemImage: "folder")
                    }
                }
                Button(action: onDetails) {
                    Label(L10n.text("action_details"), systemImage: "info.circle")
                }
            } label: { FloatingBarIcon(icon: .more) }
            .accessibilityLabel(Text("action_more"))
        } primary: {
            FloatingPrimaryButton(icon: .share, label: "action_share") {
                model.shareImage(item, attachmentIndex: attachmentIndex)
            }
        }
    }
}

/// A round button of the capsule with a system symbol (`FloatingBarButton` takes the icons of the charter only).
struct ViewerBarButton: View {
    let systemName: String
    let label: LocalizedStringKey
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 20))
                .foregroundStyle(TomiColor.text)
                .frame(width: 48, height: 48)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(label))
    }
}

/// The close button of the viewer, top leading: a round Liquid Glass button (Photos).
struct ViewerCloseButton: View {
    static let size: CGFloat = 44
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            TomiIcon.close.image
                .font(.system(size: 17, weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: Self.size, height: Self.size)
                .tomiGlass(in: Circle(), fallback: Color.black.opacity(0.45))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text("ios_viewer_close"))
    }
}

/// One page: the decrypted file of the core cache, read and decoded off the main thread, never copied.
private struct ViewerPage: View {
    let itemId: String
    let attachmentIndex: Int
    let description: String
    let model: FeedViewModel
    let onTap: () -> Void
    var onPullDown: (CGFloat, Bool) -> Void = { _, _ in }
    /// A hidden item's image: left out of captures.
    var protected = false

    private enum State { case loading, failed, ready(PlatformImage) }
    @SwiftUI.State private var state = State.loading

    var body: some View {
        Group {
            switch state {
            case .loading:
                Color.clear
            case .failed:
                Text("viewer_load_failed")
                    .tomiText(.body)
                    .foregroundStyle(.white)
                    .multilineTextAlignment(.center)
                    .padding(TomiSpace.s6)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .contentShape(Rectangle())
                    .onTapGesture(perform: onTap)
            case let .ready(image):
                // The whole screen whatever the bars: showing them must not fit the image again (and lose the zoom).
                ZoomableImage(image: image, description: description, onTap: onTap, onPullDown: onPullDown, protected: protected)
                    .ignoresSafeArea()
            }
        }
        // A page out of sight gives its memory back (a decoded page can weigh 48 MB).
        .onDisappear { state = .loading }
        .task(id: "\(itemId)#\(attachmentIndex)") {
            guard let url = await model.viewFile(itemId: itemId, attachmentIndex: attachmentIndex) else {
                state = .failed
                return
            }
            let image = await Task.detached(priority: .userInitiated) { FeedViewModel.loadImage(url) }.value
            state = image.map(State.ready) ?? .failed
        }
    }
}

/// The images of the item, one at a time. iOS: a horizontal swipe pages between them. macOS, which has no paging
/// view: the current one, with the arrow keys and an arrow on each side.
private struct ViewerPager<Page: View>: View {
    let count: Int
    @Binding var current: Int
    @ViewBuilder let page: (Int) -> Page

    var body: some View {
        #if os(iOS)
        TabView(selection: $current) {
            ForEach(0..<count, id: \.self) { index in
                page(index).tag(index)
            }
        }
        .tabViewStyle(.page(indexDisplayMode: .never))
        #else
        ZStack {
            if (0..<count).contains(current) {
                page(current).id(current)
            }
            HStack {
                arrow("chevron.left", label: "macos_viewer_previous", to: current - 1)
                Spacer()
                arrow("chevron.right", label: "macos_viewer_next", to: current + 1)
            }
            .padding(.horizontal, TomiSpace.s3)
        }
        .focusable()
        .focusEffectDisabled()
        .onKeyPress(.leftArrow) { move(to: current - 1) }
        .onKeyPress(.rightArrow) { move(to: current + 1) }
        #endif
    }

    #if os(macOS)
    private func move(to index: Int) -> KeyPress.Result {
        guard (0..<count).contains(index) else { return .ignored }
        current = index
        return .handled
    }

    @ViewBuilder
    private func arrow(_ symbol: String, label: LocalizedStringKey, to index: Int) -> some View {
        if (0..<count).contains(index) {
            Button { current = index } label: {
                Image(systemName: symbol)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(.white)
                    .frame(width: 36, height: 36)
                    .background(Color.black.opacity(0.45), in: Circle())
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(label))
        }
    }
    #endif
}
