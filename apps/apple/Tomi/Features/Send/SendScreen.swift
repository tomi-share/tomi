import PhotosUI
import SwiftUI
import TomiDesign
import TomiKit
import TomiRuntime
import UIKit
import UniformTypeIdentifiers

/// The send screen (Android `SendScreen`): optional title, text or link, chosen files, what was detected and for
/// whom, "Masquer le contenu", expiry. "Coller", "Joindre" and "Envoyer" are in the floating bar. The clipboard is read
/// only when the user touches "Coller", never by itself (iOS then asks "Autoriser le collage ?" unless allowed in
/// Settings; the system `PasteButton`, which asks nothing, stays disabled inside the glass bar).
struct SendScreen: View {
    @Bindable var model: SendViewModel
    @Environment(\.scenePhase) private var scenePhase
    @State private var showSecret = false
    @State private var choosingPhotos = false
    @State private var choosingFiles = false
    @State private var photoSelection: [PhotosPickerItem] = []
    @FocusState private var focus: Field?
    let picked: PickedFiles
    let feedback: FeedbackCenter
    /// Sent: back to the feed, where the item shows up with the "Envoyé / Annuler" banner (Android).
    var onSent: () -> Void = {}

    private enum Field { case title, text }

    var body: some View {
        // Typing a hidden content: covered in the app switcher and while the screen is recorded.
        screen.sensitiveScreen(model.secret && !model.text.isBlank)
    }

    private var screen: some View {
        ActionBarScaffold { layout in
            ZStack(alignment: .bottom) {
                TomiColor.bg.ignoresSafeArea()
                ScrollView {
                    form
                        .disabled(!model.editable)
                        .padding(.horizontal, TomiSpace.screen)
                        .padding(.top, max(TomiSpace.s2, layout.topPadding(rowHeight: 48)))
                        .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 112)
                        .frame(maxWidth: TomiLayout.readableWidth)
                        .frame(maxWidth: .infinity)
                }
                .scrollDismissesKeyboard(.interactively)
                if let sent = model.submission {
                    SentBanner(deadline: sent.deadline, skippedFiles: sent.skippedFiles, undoing: model.undoing) {
                        Task { await model.undo() }
                    }
                    .padding(.horizontal, TomiSpace.screen)
                    .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 96)
                    .frame(maxWidth: TomiLayout.readableWidth)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
                FeedbackHost(center: feedback, bottomPadding: layout.placement.isSide ? TomiSpace.s4 : 96)
            }
            .animation(.easeOut(duration: TomiMotion.medium), value: model.submission)
        } bar: { axis in
            toolbar(axis: axis)
        }
        .navigationTitle(Text("send_title"))
        .navigationBarTitleDisplayMode(.inline)
        .photosPicker(isPresented: $choosingPhotos, selection: $photoSelection, maxSelectionCount: SendViewModel.maxAttachments, matching: .any(of: [.images, .videos]))
        .fileImporter(isPresented: $choosingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { Task { await attachFiles(urls) } }
        }
        .onChange(of: photoSelection) { _, items in
            guard !items.isEmpty else { return }
            photoSelection = []
            Task { await attachPhotos(items) }
        }
        .onAppear { model.screenAppeared() }
        .onChange(of: model.leaveAfterSend) { _, leave in
            guard leave else { return }
            model.consumeLeaveAfterSend()
            focus = nil
            onSent()
        }
        .onChange(of: model.notice) { _, key in
            guard let key else { return }
            feedback.show(L10n.text(key))
            model.consumeNotice()
        }
        // A revealed secret is masked again as soon as the app is no longer active, or when the switch goes off.
        .onChange(of: scenePhase) { _, phase in if phase != .active { showSecret = false } }
        .onChange(of: model.secret) { _, secret in if !secret { showSecret = false } }
    }

    // MARK: Form

    private var form: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s3) {
            TextField(L10n.text("send_title_label"), text: Binding(get: { model.title }, set: { model.onTitleChange($0) }))
                .tomiText(.body)
                .focused($focus, equals: .title)
                .submitLabel(.next)
                .onSubmit { focus = .text }
                .tomiFieldBox(focused: focus == .title)
            contentField
            ForEach(model.attachments) { item in
                AttachmentRow(name: item.file.name) { model.onRemoveAttachment(item.id) }
            }
            DetectionRow(model: model)
            Divider().overlay(TomiColor.border)
            Toggle(isOn: Binding(get: { model.secret }, set: { model.onSecretChange($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("label_secret").tomiText(.label).foregroundStyle(TomiColor.text)
                    Text(model.secret ? "label_secret_on" : "label_secret_off").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
            }
            .tint(TomiColor.brand)
            .disabled(model.secretLocked)
            Text(L10n.text("expiry_label")).tomiText(.label).foregroundStyle(TomiColor.text).padding(.top, TomiSpace.s1)
            ExpiryChips(options: model.expiryOptions, selected: model.expiry) { model.onExpiryChange($0) }
            if model.secret && !model.hasAttachment {
                Text("expiry_after_read_hint").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
            }
        }
    }

    /// Text or link, 4 to 10 lines. Hidden content: no correction nor suggestion (the keyboard learns nothing), and
    /// dots in place of the text until the eye is touched; it is edited revealed (a `SecureField` would be one line, and
    /// erases everything at the first key after coming back to it).
    @ViewBuilder
    private var contentField: some View {
        let binding = Binding(get: { model.text }, set: { model.onTextChange($0) })
        HStack(alignment: .top, spacing: TomiSpace.s2) {
            if model.secret && !showSecret && !model.text.isEmpty {
                Text(String(repeating: "•", count: min(model.text.count, 24)))
                    .tomiText(.mono)
                    .foregroundStyle(TomiColor.secret)
                    .frame(maxWidth: .infinity, minHeight: 4 * 22, alignment: .topLeading)
                    .contentShape(Rectangle())
                    .onTapGesture { showSecret = true }
                    .accessibilityLabel(Text("detect_masked"))
            } else {
                TextField(L10n.text("send_content_label"), text: binding, axis: .vertical)
                    .lineLimit(4...10)
                    .tomiText(.body)
                    .focused($focus, equals: .text)
                    .textInputAutocapitalization(model.secret ? .never : .sentences)
                    .autocorrectionDisabled(model.secret)
                    .keyboardType(model.secret ? .asciiCapable : .default)
                    .privacySensitive(model.secret)
            }
            if model.secret {
                Button { showSecret.toggle() } label: {
                    (showSecret ? TomiIcon.masked : TomiIcon.reveal).image
                        .foregroundStyle(TomiColor.textMuted)
                        .frame(width: 32, height: 24)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(Text(showSecret ? "action_hide" : "action_reveal"))
            }
        }
        .tomiFieldBox(focused: focus == .text)
    }

    // MARK: Toolbar

    private func toolbar(axis: Axis) -> some View {
        FloatingActionBar(axis: axis) {
            FloatingBarButton(icon: .paste, label: "action_paste") { paste() }
                .disabled(!model.editable)
            Menu {
                Button { choosingPhotos = true } label: { Label(L10n.text("ios_attach_photos"), systemImage: "photo.on.rectangle") }
                Button { choosingFiles = true } label: { Label(L10n.text("ios_attach_files"), systemImage: "folder") }
            } label: {
                FloatingBarIcon(icon: .attach)
            }
            .menuOrder(.fixed)
            .disabled(!model.editable)
            .accessibilityLabel(Text("action_attach"))
        } primary: {
            FloatingPrimaryButton(icon: .send, label: "action_send", enabled: model.canSend) {
                focus = nil
                Task { await model.send() }
            }
        }
    }

    /// Marker that password managers put on what they copy (nspasteboard.org convention), the iOS counterpart of
    /// Android's `EXTRA_IS_SENSITIVE`.
    static let concealedType = "org.nspasteboard.ConcealedType"

    /// The single place in the app that reads the clipboard, on the user's touch only. Takes the text and, when the
    /// source gave some, the HTML of the first item; a copied link counts as text. Nothing else (no image, no file).
    private func paste() {
        let board = UIPasteboard.general
        // The checks below do not read the clipboard (no prompt); then one read of what is there.
        let concealed = board.contains(pasteboardTypes: [Self.concealedType])
        let hasHTML = board.contains(pasteboardTypes: [UTType.html.identifier])
        guard board.hasStrings || board.hasURLs || hasHTML else {
            model.onPasted(plain: nil, html: nil, concealed: false)
            return
        }
        let plain = board.hasStrings ? board.string : (board.hasURLs ? board.url?.absoluteString : nil)
        let html = hasHTML ? board.data(forPasteboardType: UTType.html.identifier).flatMap { String(data: $0, encoding: .utf8) } : nil
        model.onPasted(plain: plain ?? html.map(HtmlText.toPlain), html: html, concealed: concealed)
    }

    // MARK: Attachments

    private func attachFiles(_ urls: [URL]) async {
        var files: [SharedFile] = []
        for url in urls {
            if let file = await picked.importFile(url) { files.append(file) }
        }
        model.onAttach(files, skipped: urls.count - files.count)
    }

    private func attachPhotos(_ items: [PhotosPickerItem]) async {
        var files: [SharedFile] = []
        for item in items {
            if let media = try? await item.loadTransferable(type: PickedMedia.self), let file = await picked.importMedia(media) {
                files.append(file)
            }
        }
        model.onAttach(files, skipped: items.count - files.count)
    }
}
