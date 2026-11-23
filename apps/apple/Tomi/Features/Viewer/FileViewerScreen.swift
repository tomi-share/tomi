import AVKit
import QuickLook
#if os(macOS)
import PDFKit
import QuickLookUI
#endif
import SwiftUI
import TomiDesign
import TomiKit
import UniformTypeIdentifiers

/// The file viewer (iOS): the files Tomi does not show in its image viewer or code screen, in the same frame as the
/// image viewer. Night background, close top leading, the actions in the floating Liquid Glass bar (at the bottom, or
/// in the side strip of iPhone Duo): "Partager" as the primary button (it is also "Ouvrir dans…"), save to Files,
/// delete, "…" (details).
///
/// - Audio and video iOS plays: the system player (controls, picture in picture).
/// - Documents Quick Look draws (PDF, Office, iWork, rich text...): Quick Look, inside the screen.
/// - Anything else (MKV, archives...): a page with the file and "Ouvrir dans…", the apps that can read it.
/// - A hidden item's file is never drawn here (Quick Look and the player draw outside the layer that captures leave out):
///   a page offers to share it, behind the gate. Its name is not shown either.
struct FileViewerScreen: View {
    let request: ViewerRequest
    let model: FeedViewModel
    let onClose: () -> Void
    let onDetails: (String) -> Void

    private enum Content: Equatable {
        case loading
        case media(URL)
        case document(URL)
        /// Neither played nor drawn here: offered to another app.
        case elsewhere
        case hidden
        case failed
    }

    @Environment(AppContainer.self) private var container
    @State private var content = Content.loading
    @State private var link: URL?
    @State private var existed = false
    @State private var confirmDeleteEverywhere = false

    var body: some View {
        let item = model.item(id: request.itemId)
        Group {
            if let item, item.attachments.indices.contains(request.attachmentIndex) {
                screen(item, attachment: item.attachments[request.attachmentIndex])
                    .deleteEverywhereConfirmation(isPresented: $confirmDeleteEverywhere, item: item, model: model)
            } else {
                // Gone before it showed (deleted elsewhere): nothing to show, the viewer closes.
                ImageViewerScreen.background.ignoresSafeArea()
                    .onAppear { if model.loaded { onClose() } }
            }
        }
        .environment(\.colorScheme, .dark)
        .accessibilityAction(.escape) { onClose() }
        .presentationBackground(.clear)
        .overlay {
            if container.coversSensitiveContent {
                PrivacyCover(captured: container.coverIsForCapture) { container.leaveSensitiveContent() }
            }
        }
        .task(id: request.id) { await load(item) }
        .onDisappear { removeLink() }
        // Deleting the item from here closes the viewer.
        .onChange(of: item == nil, initial: true) { _, gone in
            if !gone { existed = true } else if existed { onClose() }
        }
    }

    private func screen(_ item: TomiItem, attachment: Attachment) -> some View {
        ActionBarScaffold { layout in
            ZStack {
                ImageViewerScreen.background.ignoresSafeArea()
                contentView(item, attachment: attachment)
                    // Above the bar at the bottom (the player's controls and the end of a document stay reachable);
                    // in the side strip of iPhone Duo the bar is beside it already.
                    .padding(.bottom, layout.placement.isSide ? 0 : 88)
                    .padding(.top, 56)
                FeedbackHost(center: model.feedback, bottomPadding: layout.placement.isSide ? TomiSpace.s4 : 96)
                VStack(spacing: 0) {
                    header(item, attachment: attachment, layout: layout)
                    Spacer(minLength: 0)
                }
            }
        } bar: { axis in
            actionBar(item, axis: axis)
        }
    }

    /// Close (top leading, on the line of the status content of iPhone Duo) and the name of the file.
    private func header(_ item: TomiItem, attachment: Attachment, layout: ScreenLayout) -> some View {
        let center = layout.headerCenter
        return HStack(spacing: TomiSpace.s3) {
            if !layout.placement.isSide { ViewerCloseButton(action: onClose) }
            Text(verbatim: displayName(item, attachment: attachment))
                .tomiMono(size: 13)
                .foregroundStyle(.white.opacity(0.85))
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: layout.placement.isSide ? .center : .leading)
        }
        .padding(.leading, max(TomiSpace.s2, CGFloat(layout.topBand?.leading ?? 0)))
        .padding(.trailing, max(TomiSpace.screen, CGFloat(layout.topBand?.trailing ?? 0)))
        .padding(.top, center.map { max(0, $0 - ViewerCloseButton.size / 2) } ?? TomiSpace.s2)
        .ignoresSafeArea(edges: layout.topBand == nil ? [] : .top)
    }

    @ViewBuilder
    private func contentView(_ item: TomiItem, attachment: Attachment) -> some View {
        switch content {
        case .loading:
            TomiLinearProgress().frame(maxWidth: 160)
        case let .media(url):
            MediaPlayerView(url: url)
        case let .document(url):
            QuickLookView(url: url)
        case .elsewhere:
            FilePlaceholder(
                symbol: symbol(attachment), name: attachment.name, detail: detail(attachment),
                title: "ios_file_preview_none_title", message: "ios_file_preview_none",
                action: "ios_file_open_in"
            ) { model.shareAttachment(item, index: request.attachmentIndex) }
        case .hidden:
            FilePlaceholder(
                symbol: TomiIcon.masked.systemName, name: L10n.text("ios_file_number", request.attachmentIndex + 1),
                detail: formatFileSize(attachment.size),
                title: "ios_file_hidden_title", message: "ios_file_hidden", action: "action_share"
            ) { model.shareAttachment(item, index: request.attachmentIndex) }
        case .failed:
            Text("ios_file_load_failed")
                .tomiText(.body)
                .foregroundStyle(.white)
                .multilineTextAlignment(.center)
                .padding(TomiSpace.s6)
        }
    }

    /// Bar: close (side strip only), delete and "…" (save to Files, never for a hidden item; details); "Partager".
    private func actionBar(_ item: TomiItem, axis: Axis) -> some View {
        let index = request.attachmentIndex
        return FloatingActionBar(axis: axis) {
            if axis == .vertical {
                ViewerBarButton(systemName: TomiIcon.close.systemName, label: "ios_viewer_close", action: onClose)
            }
            #if os(macOS)
            // The Mac hands the file to the app it chose for it (the iOS screen has "Ouvrir dans…" in its share sheet).
            if !item.secret {
                ViewerBarButton(systemName: "arrow.up.forward.app", label: "action_open") {
                    model.openAttachmentInDefaultApp(item, index: index)
                }
            }
            #endif
            if !item.secret {
                ViewerBarButton(systemName: "folder", label: "ios_save_to_files") {
                    model.saveAttachmentToFiles(item, index: index)
                }
            }
            Menu {
                Button { model.deleteLocal(item) } label: {
                    Label(L10n.text("action_delete_local"), systemImage: TomiIcon.delete.systemName)
                }
                Button(role: .destructive) { confirmDeleteEverywhere = true } label: {
                    Label(L10n.text("action_delete_everywhere"), systemImage: "trash.slash")
                }
            } label: { FloatingBarIcon(icon: .delete) }
            .accessibilityLabel(Text("action_delete"))
            Menu {
                Button { onDetails(item.id) } label: {
                    Label(L10n.text("action_details"), systemImage: "info.circle")
                }
            } label: { FloatingBarIcon(icon: .more) }
            .accessibilityLabel(Text("action_more"))
        } primary: {
            FloatingPrimaryButton(icon: .share, label: "action_share") {
                model.shareAttachment(item, index: index)
            }
        }
    }

    // MARK: Loading

    private func load(_ item: TomiItem?) async {
        guard let item, item.attachments.indices.contains(request.attachmentIndex) else { return }
        let attachment = item.attachments[request.attachmentIndex]
        if item.secret {
            content = .hidden
            return
        }
        let file = await model.viewFile(itemId: item.id, attachmentIndex: request.attachmentIndex)
        // Closed meanwhile (a long decryption): no link left behind.
        guard !Task.isCancelled else { return }
        guard let file, let named = FeedViewModel.viewLink(file, name: Self.linkName(attachment)) else {
            content = .failed
            return
        }
        removeLink()
        link = named
        if Task.isCancelled {
            removeLink()
            return
        }
        switch FilePreview.kind(attachment) {
        case .media:
            let playable = (try? await AVURLAsset(url: named).load(.isPlayable)) ?? false
            content = playable ? .media(named) : .elsewhere
        default:
            #if os(iOS)
            content = QLPreviewController.canPreview(named as NSURL) ? .document(named) : .elsewhere
            #else
            // The Mac's Quick Look view draws any file, at worst its icon and its type.
            content = .document(named)
            #endif
        }
    }

    /// The name Quick Look and the player read the type in: one without extension (from another app) gets the one of
    /// its type.
    private static func linkName(_ attachment: Attachment) -> String {
        let name = FeedViewModel.fileName(attachment)
        guard (name as NSString).pathExtension.isEmpty,
              let ext = UTType(mimeType: attachment.mime)?.preferredFilenameExtension else { return name }
        return "\(name).\(ext)"
    }

    private func removeLink() {
        if let link { FeedViewModel.removeViewLink(link) }
        link = nil
    }

    // MARK: Texts

    private func displayName(_ item: TomiItem, attachment: Attachment) -> String {
        item.secret ? L10n.text("ios_file_number", request.attachmentIndex + 1) : attachment.name
    }

    /// "PDF · 1,2 Mo": the kind iOS gives the type, then the size.
    private func detail(_ attachment: Attachment) -> String {
        let ext = CodeLanguages.extensionOf(attachment.name)
        let type = ext.flatMap { UTType(filenameExtension: $0) } ?? UTType(mimeType: attachment.mime)
        let size = formatFileSize(attachment.size)
        guard let description = type?.localizedDescription else { return size }
        return "\(description) · \(size)"
    }

    private func symbol(_ attachment: Attachment) -> String {
        if attachment.mime.hasPrefix("video/") { return "film" }
        if attachment.mime.hasPrefix("audio/") { return "waveform" }
        if attachment.mime.contains("zip") || attachment.mime.contains("compressed") { return "doc.zipper" }
        return TomiIcon.file.systemName
    }
}

/// A file Tomi does not draw: its icon, name and kind, why, and one way on.
private struct FilePlaceholder: View {
    let symbol: String
    let name: String
    let detail: String
    let title: LocalizedStringKey
    let message: LocalizedStringKey
    let action: LocalizedStringKey
    let onAction: () -> Void

    var body: some View {
        ScrollView {
            VStack(spacing: TomiSpace.s4) {
                Image(systemName: symbol)
                    .font(.system(size: 44, weight: .light))
                    .foregroundStyle(.white.opacity(0.85))
                    .frame(width: 96, height: 96)
                    .background(.white.opacity(0.08), in: RoundedRectangle(cornerRadius: TomiRadius.large))
                    .accessibilityHidden(true)
                VStack(spacing: TomiSpace.s1) {
                    Text(verbatim: name)
                        .tomiMono(size: 15)
                        .foregroundStyle(.white)
                        .multilineTextAlignment(.center)
                        .lineLimit(3)
                    Text(verbatim: detail).tomiText(.caption).foregroundStyle(.white.opacity(0.6))
                }
                VStack(spacing: TomiSpace.s2) {
                    Text(title).tomiText(.subtitle).foregroundStyle(.white).accessibilityAddTraits(.isHeader)
                    Text(message)
                        .tomiText(.body)
                        .foregroundStyle(.white.opacity(0.7))
                        .multilineTextAlignment(.center)
                }
                .padding(.top, TomiSpace.s2)
                Button(action: onAction) { Text(action) }
                    .buttonStyle(.tomiPrimary)
                    .padding(.top, TomiSpace.s2)
            }
            .padding(.horizontal, TomiSpace.screen)
            .padding(.vertical, TomiSpace.s6)
            .frame(maxWidth: 420)
            .frame(maxWidth: .infinity)
        }
        .scrollBounceBehavior(.basedOnSize)
    }
}

#if os(iOS)
/// The system player (controls, picture in picture), paused when the viewer goes.
private struct MediaPlayerView: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> AVPlayerViewController {
        // Heard with the silent switch on, as in any player; the other apps' audio resumes after.
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try? AVAudioSession.sharedInstance().setActive(true)
        let controller = AVPlayerViewController()
        controller.player = AVPlayer(url: url)
        controller.view.backgroundColor = .clear
        return controller
    }

    func updateUIViewController(_ controller: AVPlayerViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: AVPlayerViewController, coordinator: ()) {
        controller.player?.pause()
        controller.player = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

/// Quick Look inside the screen, one file.
private struct QuickLookView: UIViewControllerRepresentable {
    let url: URL

    func makeCoordinator() -> Source { Source(url: url) }

    func makeUIViewController(context: Context) -> QLPreviewController {
        let controller = QLPreviewController()
        controller.dataSource = context.coordinator
        return controller
    }

    func updateUIViewController(_ controller: QLPreviewController, context: Context) {
        if context.coordinator.url != url {
            context.coordinator.url = url
            controller.reloadData()
        }
    }

    final class Source: NSObject, QLPreviewControllerDataSource {
        var url: URL

        init(url: URL) { self.url = url }

        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }

        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem {
            url as NSURL
        }
    }
}
#else
/// The system player of the Mac, paused when the viewer goes.
private struct MediaPlayerView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = AVPlayer(url: url)
        view.controlsStyle = .floating
        return view
    }

    func updateNSView(_ view: AVPlayerView, context: Context) {}

    static func dismantleNSView(_ view: AVPlayerView, coordinator: ()) {
        view.player?.pause()
        view.player = nil
    }
}

/// The document inside the screen, one file: a PDF is drawn by PDFKit (in the app itself: scroll, zoom, selection),
/// anything else by Quick Look.
private struct QuickLookView: View {
    let url: URL

    var body: some View {
        if url.pathExtension.lowercased() == "pdf" {
            PDFDocumentView(url: url)
        } else {
            QuickLookPreview(url: url)
        }
    }
}

private struct PDFDocumentView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> PDFView {
        let view = PDFView()
        view.autoScales = true
        view.displayMode = .singlePageContinuous
        view.backgroundColor = .clear
        view.document = PDFDocument(url: url)
        return view
    }

    func updateNSView(_ view: PDFView, context: Context) {
        if view.document?.documentURL != url { view.document = PDFDocument(url: url) }
    }
}

private struct QuickLookPreview: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> QLPreviewView {
        let view = QLPreviewView(frame: .zero, style: .normal) ?? QLPreviewView()
        view.autostarts = false
        view.previewItem = url as NSURL
        return view
    }

    func updateNSView(_ view: QLPreviewView, context: Context) {
        if (view.previewItem as? NSURL) as URL? != url { view.previewItem = url as NSURL }
    }

    static func dismantleNSView(_ view: QLPreviewView, coordinator: ()) {
        view.close()
    }
}
#endif
