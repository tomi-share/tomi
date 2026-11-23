import PhotosUI
import SwiftUI
import TomiDesign
import TomiKit
import TomiRuntime
#if canImport(UIKit)
import UIKit
#else
import AppKit
#endif
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
        .inlineNavigationTitle()
        .photosPicker(isPresented: $choosingPhotos, selection: $photoSelection, maxSelectionCount: SendViewModel.maxAttachments, matching: .any(of: [.images, .videos]))
        .fileImporter(isPresented: $choosingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
            if case let .success(urls) = result { Task { await attachFiles(urls) } }
        }
        .onChange(of: photoSelection) { _, items in
            guard !items.isEmpty else { return }
            photoSelection = []
            Task { await attachPhotos(items) }
        }
        .onAppear {
            model.screenAppeared()
            offerClipboard()
        }
        // What was offered is not kept once the screen is left: it is looked at again at the next opening.
        .onDisappear { model.dismissSuggestion() }
        // Nor past its minute, with the screen left open.
        .task(id: model.suggestion) {
            guard model.suggestion != nil else { return }
            try? await Task.sleep(for: .seconds(ClipboardSuggestions.maxAge))
            if !Task.isCancelled, !ClipboardWatcher.shared.stillOffers() { model.dismissSuggestion() }
        }
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
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { showSecret = false } else { offerClipboard() }
        }
        .onChange(of: model.secret) { _, secret in if !secret { showSecret = false } }
        #if os(macOS)
        // Command-V on this screen is "Coller": files, an image, a text with its rich variant. Left to the field
        // being typed in when it would only insert plain text at the cursor.
        .background(MacPasteShortcut { pasteShortcut() })
        #endif
    }

    #if os(macOS)
    /// True when the paste was taken here (the key goes no further).
    private func pasteShortcut() -> Bool {
        guard model.editable else { return false }
        // Files or a copied image: always attached, a text field could do nothing with them.
        if MacSendInput.holdsFilesOrImage() {
            paste()
            return true
        }
        // In the title: the field's own paste, at the cursor.
        if focus == .title { return false }
        // In a content already started: at the cursor too, unless the copy is marked concealed (the content is
        // then hidden and locked, which only "Coller" does).
        if focus == .text && !model.text.isEmpty && !MacSendInput.holdsConcealed() { return false }
        paste()
        return true
    }
    #endif

    // MARK: Clipboard suggestion

    /// Offers what was copied less than a minute ago, on an empty draft. Nothing is typed: it shows in the place of
    /// the content until the person takes it, writes something else or dismisses it.
    private func offerClipboard() {
        model.offer(ClipboardWatcher.shared.suggestion())
    }

    /// Takes the suggestion: what a paste does (the same reading of the clipboard, the same rules for a hidden
    /// content).
    /// False, with nothing pasted, when the clipboard is no longer what was offered (something else was copied
    /// since, or the minute is over): the screen shows what is there now instead, and nothing is sent unseen.
    @discardableResult
    private func useSuggestion() async -> Bool {
        guard model.suggestion != nil else { return false }
        guard ClipboardWatcher.shared.stillOffers() else {
            offerClipboard()
            return false
        }
        ClipboardWatcher.shared.consume()
        model.dismissSuggestion()
        await pasteNow()
        return true
    }

    private func dismissSuggestion() {
        ClipboardWatcher.shared.consume()
        model.dismissSuggestion()
    }

    /// "Envoyer" with nothing written but a suggestion whose content the app knows (macOS): it is what is sent.
    private var canSendSuggestion: Bool {
        !model.canSend && model.editable && model.suggestion?.isRead == true
    }

    private func suggestionRow(_ suggestion: ClipboardSuggestion) -> some View {
        HStack(spacing: TomiSpace.s2) {
            Button { Task { await useSuggestion() } } label: {
                HStack(spacing: TomiSpace.s2) {
                    TomiIcon.paste.image.foregroundStyle(TomiColor.brand)
                    Text("ios_clip_hint").tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text(verbatim: L10n.text("ios_clip_use") + " : " + suggestion.ghostText))
            Button(action: dismissSuggestion) {
                TomiIcon.close.image
                    .foregroundStyle(TomiColor.textMuted)
                    .frame(width: 32, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(Text("ios_clip_dismiss"))
        }
        .padding(.horizontal, TomiSpace.s1)
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
            if let suggestion = model.suggestion { suggestionRow(suggestion) }
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
                // A suggestion shows where the content will be, as a placeholder does: nothing is typed.
                TextField(
                    text: binding, prompt: Text(verbatim: model.suggestion?.ghostText ?? L10n.text("send_content_label")),
                    axis: .vertical
                ) { Text("send_content_label") }
                    .onKeyPress(.tab) {
                        guard model.suggestion != nil, model.text.isEmpty else { return .ignored }
                        Task { await useSuggestion() }
                        return .handled
                    }
                    .lineLimit(4...10)
                    .tomiText(.body)
                    .focused($focus, equals: .text)
                    .autocapitalization(sentences: !model.secret)
                    .autocorrectionDisabled(model.secret)
                    .asciiKeyboard(model.secret)
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
            FloatingPrimaryButton(icon: .send, label: "action_send", enabled: model.canSend || canSendSuggestion) {
                focus = nil
                Task {
                    if canSendSuggestion, await !useSuggestion() { return }
                    await model.send()
                }
            }
            // Command-Return sends (Return alone is a new line in the text).
            .keyboardShortcut(.return, modifiers: .command)
        }
    }

    /// Marker that password managers put on what they copy (nspasteboard.org convention), the iOS counterpart of
    /// Android's `EXTRA_IS_SENSITIVE`.
    static let concealedType = "org.nspasteboard.ConcealedType"

    /// Reads the clipboard for the draft, on the user's gesture: "Coller", Command-V on this screen (Mac), or taking
    /// the suggestion. The only other reading is the Mac's look at a copy made less than a minute ago, to offer it
    /// (`ClipboardWatcher`); iOS reads nothing before the gesture. Takes the text and, when the source gave some, the HTML of the first item; a copied
    /// link counts as text. iOS takes nothing else (no image, no file).
    private func paste() {
        Task { await pasteNow() }
    }

    private func pasteNow() async {
        #if os(macOS)
        // On the Mac a paste also takes copied files and a copied image (`MacSendInput`).
        await MacSendInput.paste(into: model, picked: picked)
        #else
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
        #endif
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
