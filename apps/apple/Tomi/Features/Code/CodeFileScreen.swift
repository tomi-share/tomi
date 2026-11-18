import SwiftUI
import TomiDesign
import TomiKit
import UIKit

/// The code file reader (Android `CodeFileScreen`), pushed on a tap on a code file of a card or of the detail.
///
/// - The decrypted copy of the core cache is read into memory only (2 MB at most): the app writes no clear copy.
/// - Monospace 13/19 with a line-number gutter, vertical scroll and sideways scroll unless "Retour à la ligne" is on
///   (one remembered choice for every file). No syntax colours in v1 (spec 11).
/// - Search in the file: occurrences highlighted, the current one stronger, previous / next, "n/m", scroll to it.
/// - Several code files: tabs on top and "2/3" in the bar.
/// - Hidden item: nothing is read before the gate, the app switcher shows the privacy cover, the screen closes when the
///   app goes to the background, and "Copier le fichier" goes through the sensitive clipboard.
struct CodeFileScreen: View {
    let request: ViewerRequest
    let model: FeedViewModel
    let prefs: UserPrefs
    let onClose: () -> Void
    let onDetails: (String) -> Void

    @Environment(\.scenePhase) private var scenePhase
    @State private var unlocked = false
    @State private var existed = false
    @State private var selected: Int?
    @State private var searching = false
    @State private var query = ""
    @State private var copyText: String?
    @State private var wrap: Bool

    init(request: ViewerRequest, model: FeedViewModel, prefs: UserPrefs, onClose: @escaping () -> Void, onDetails: @escaping (String) -> Void) {
        self.request = request
        self.model = model
        self.prefs = prefs
        self.onClose = onClose
        self.onDetails = onDetails
        _wrap = State(initialValue: prefs.codeWrapEnabled)
    }

    var body: some View {
        let item = model.item(id: request.itemId)
        let content = CodeGate.content(item: item, loaded: model.loaded, unlocked: unlocked)
        let files: [CodeFile] = if case let .files(list) = content { list } else { [] }
        let position = CodeFiles.positionFor(files, attachmentIndex: selected ?? request.attachmentIndex)
        let current = files.indices.contains(position) ? files[position] : nil

        VStack(spacing: 0) {
            switch content {
            case .loading:
                Spacer()
            case .locked:
                message("code_locked")
            case .empty:
                message("code_no_file")
            case .files:
                if let item, let current {
                    if files.count > 1 { tabs(files, selected: position) }
                    CodeBody(
                        item: item, file: current, model: model, wrap: wrap, searching: $searching, query: $query,
                        onTextReady: { copyText = $0 }
                    )
                    .id(current.attachmentIndex)
                }
            }
        }
        .background(TomiColor.bg.ignoresSafeArea())
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                // A hidden item shows no name before the gate (no current file until then).
                Text(verbatim: current?.attachment.name ?? "")
                    .tomiMono(size: 15)
                    .foregroundStyle(TomiColor.text)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            ToolbarItemGroup(placement: .topBarTrailing) {
                if files.count > 1 {
                    Text(verbatim: CodeFiles.positionLabel(position: position, total: files.count))
                        .tomiText(.label)
                        .foregroundStyle(TomiColor.textMuted)
                }
                if let item, current != nil {
                    Button {
                        if searching { query = "" }
                        searching.toggle()
                    } label: {
                        TomiIcon.search.image
                    }
                    .accessibilityLabel(Text("code_search"))
                    Menu {
                        Button {
                            if let copyText { model.copyCode(item, text: copyText) }
                        } label: {
                            Label(L10n.text("code_copy_file"), systemImage: TomiIcon.copy.systemName)
                        }
                        .disabled(copyText == nil)
                        if let current {
                            Button { model.shareAttachment(item, index: current.attachmentIndex) } label: {
                                Label(L10n.text("action_share"), systemImage: TomiIcon.share.systemName)
                            }
                            if !item.secret {
                                Button { model.saveAttachmentToFiles(item, index: current.attachmentIndex) } label: {
                                    Label(L10n.text("ios_save_to_files"), systemImage: "folder")
                                }
                            }
                        }
                        Toggle(isOn: Binding(get: { wrap }, set: { wrap = $0; prefs.codeWrapEnabled = $0 })) {
                            Label(L10n.text("code_wrap"), systemImage: "text.word.spacing")
                        }
                        Button { onDetails(item.id) } label: {
                            Label(L10n.text("action_details"), systemImage: "info.circle")
                        }
                    } label: {
                        TomiIcon.more.image
                    }
                    .accessibilityLabel(Text("action_more"))
                }
            }
        }
        .sensitiveScreen(item?.secret == true)
        .task(id: item?.id) {
            guard let item, item.secret else { return }
            unlocked = false
            if await model.authorizeView(item, file: true) { unlocked = true } else { onClose() }
        }
        .onChange(of: item == nil, initial: true) { _, gone in
            if !gone { existed = true } else if existed { onClose() }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .background, CodeGate.closesOnStop(item: item, changingConfigurations: false) { onClose() }
        }
    }

    private func message(_ key: LocalizedStringKey) -> some View {
        Text(key)
            .tomiText(.body)
            .foregroundStyle(TomiColor.text)
            .multilineTextAlignment(.center)
            .padding(TomiSpace.s6)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func tabs(_ files: [CodeFile], selected: Int) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: TomiSpace.s2) {
                ForEach(Array(files.enumerated()), id: \.element.attachmentIndex) { index, file in
                    Button {
                        self.selected = file.attachmentIndex
                        copyText = nil
                    } label: {
                        Text(verbatim: file.attachment.name)
                            .tomiMono(size: 13)
                            .lineLimit(1)
                            .padding(.horizontal, TomiSpace.s3)
                            .padding(.vertical, 6)
                            .foregroundStyle(index == selected ? TomiColor.brand : TomiColor.text)
                            .background(index == selected ? TomiColor.brandContainer : TomiColor.surface, in: Capsule())
                            .overlay(Capsule().strokeBorder(index == selected ? .clear : TomiColor.border))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(index == selected ? .isSelected : [])
                }
            }
            .padding(.horizontal, TomiSpace.screen)
            .padding(.vertical, TomiSpace.s1)
        }
    }
}

/// The file shown: read, then displayed with the search.
private struct CodeBody: View {
    let item: TomiItem
    let file: CodeFile
    let model: FeedViewModel
    let wrap: Bool
    @Binding var searching: Bool
    @Binding var query: String
    let onTextReady: (String?) -> Void

    private enum Loaded {
        case loading
        case failed(LocalizedStringKey)
        case ready(CodeDocument, original: String, maxLineLength: Int)
    }

    @State private var loaded = Loaded.loading

    var body: some View {
        Group {
            switch loaded {
            case .loading:
                Spacer()
            case let .failed(key):
                Text(key)
                    .tomiText(.body)
                    .foregroundStyle(TomiColor.text)
                    .multilineTextAlignment(.center)
                    .padding(TomiSpace.s6)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case let .ready(doc, _, maxLineLength):
                CodeReader(
                    doc: doc, maxLineLength: maxLineLength, wrap: wrap, protected: item.secret,
                    searching: $searching, query: $query
                )
            }
        }
        .task(id: "\(item.id)#\(file.attachmentIndex)") {
            onTextReady(nil)
            guard let url = await model.viewFile(itemId: item.id, attachmentIndex: file.attachmentIndex) else {
                loaded = .failed("code_unavailable")
                return
            }
            let next: Loaded = await Task.detached(priority: .userInitiated) {
                switch FeedViewModel.readCode(url) {
                case .unavailable: .failed("code_unavailable")
                case .tooLarge: .failed("code_too_large")
                case .binary: .failed("code_binary")
                case let .text(original, display):
                    // Measured once: the width of the block, not at every row.
                    { let doc = CodeDocument(display); return .ready(doc, original: original, maxLineLength: doc.maxLineLength) }()
                }
            }.value
            // Another tab was chosen meanwhile: its file, not this one, is shown and copied.
            guard !Task.isCancelled else { return }
            loaded = next
            if case let .ready(_, original, _) = next { onTextReady(original) }
        }
    }
}

/// The lines, the gutter and the search.
private struct CodeReader: View {
    let doc: CodeDocument
    let maxLineLength: Int
    let wrap: Bool
    /// A hidden item's file: drawn in the layer that captures leave out.
    var protected = false
    @Binding var searching: Bool
    @Binding var query: String

    @State private var current = -1
    @State private var matches: [CodeMatch] = []
    @State private var byLine: [Int: [LineMatch]] = [:]
    @FocusState private var searchFocused: Bool

    private static let fontSize: CGFloat = 13
    private static let lineHeight: CGFloat = 19
    private static let gapChars = 2
    private static let maxQuery = 200
    /// Width of one character of the monospace font.
    private static let charWidth: CGFloat = {
        let font = TomiMonoFont.ctFont(size: fontSize) as UIFont
        return ("0000000000" as NSString).size(withAttributes: [.font: font]).width / 10
    }()

    var body: some View {
        VStack(spacing: 0) {
            if searching {
                searchRow
            }
            ScrollViewReader { proxy in
                ScrollView(wrap ? .vertical : [.vertical, .horizontal]) {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(0..<doc.lineCount, id: \.self) { line in
                            // Line by line (the reader keeps its search, scrolling and lazy loading).
                            row(line, matches: byLine[line])
                                .captureProtected(protected)
                                .id(line)
                        }
                    }
                    .padding(.vertical, TomiSpace.s2)
                    .padding(.horizontal, TomiSpace.s2)
                }
                .onChange(of: current) { _, _ in scroll(proxy) }
                // Once per query (a file can hold 2 MB), after a short pause in the typing, off the main thread.
                .task(id: query) {
                    if !query.isEmpty { try? await Task.sleep(for: .milliseconds(150)) }
                    guard !Task.isCancelled else { return }
                    let doc = self.doc
                    let query = self.query
                    let found = await Task.detached(priority: .userInitiated) {
                        let matches = CodeSearch.find(text: doc.text, query: query)
                        return (matches, CodeLines.bucketMatches(doc, matches: matches))
                    }.value
                    guard !Task.isCancelled else { return }
                    matches = found.0
                    byLine = found.1
                    let first = CodeSearch.firstFrom(matches, fromOffset: 0)
                    if first == current { scroll(proxy) } else { current = first }
                }
            }
            .background(TomiColor.surfaceAlt)
            .overlay(Rectangle().strokeBorder(TomiColor.border))
            .padding(.horizontal, TomiSpace.s2)
            .padding(.bottom, TomiSpace.s2)
        }
    }

    private var gutterChars: Int { String(doc.lineCount).count }

    /// Width of the block when lines are not folded: the longest line plus the gutter.
    private var contentWidth: CGFloat {
        CGFloat(gutterChars + Self.gapChars + min(maxLineLength, CodeLimits.maxDisplayLineChars) + 3) * Self.charWidth
    }

    @ViewBuilder
    private func row(_ line: Int, matches: [LineMatch]?) -> some View {
        let text = lineText(line, matches: matches)
        HStack(alignment: .top, spacing: CGFloat(Self.gapChars) * Self.charWidth) {
            Text(verbatim: String(line + 1))
                .tomiMono(size: Self.fontSize)
                .foregroundStyle(TomiColor.textMuted)
                .frame(width: CGFloat(gutterChars) * Self.charWidth, alignment: .trailing)
                .accessibilityHidden(true)
            Text(text)
                .tomiMono(size: Self.fontSize)
                .foregroundStyle(TomiColor.text)
                .lineLimit(wrap ? nil : 1)
                .fixedSize(horizontal: !wrap, vertical: true)
                .frame(maxWidth: wrap ? .infinity : nil, alignment: .leading)
        }
        .frame(minHeight: Self.lineHeight, alignment: .topLeading)
        .frame(width: wrap ? nil : contentWidth, alignment: .leading)
    }

    /// One line, the occurrences on top (the text keeps its colour on the highlight).
    private func lineText(_ line: Int, matches: [LineMatch]?) -> AttributedString {
        let raw = doc.line(line)
        let units = Array(raw.utf16)
        let cut = units.count > CodeLimits.maxDisplayLineChars
        var limit = min(units.count, CodeLimits.maxDisplayLineChars)
        // Never half a character: a cut on a high surrogate keeps the pair out.
        if cut, limit > 0, UTF16.isLeadSurrogate(units[limit - 1]) { limit -= 1 }
        let shownUnits = Array(units.prefix(limit))
        let shown = (String(utf16CodeUnits: shownUnits, count: shownUnits.count)) + (cut ? "…" : "")
        var text = AttributedString(shown)
        guard let matches else { return text }
        for match in matches {
            let end = min(match.endCol, shownUnits.count)
            guard match.startCol < end else { continue }
            let range = NSRange(location: match.startCol, length: end - match.startCol)
            guard let swiftRange = Range(range, in: shown), let attributed = Range(swiftRange, in: text) else { continue }
            text[attributed].backgroundColor = match.index == current ? TomiColor.sun.opacity(0.75) : TomiColor.sun.opacity(0.3)
        }
        return text
    }

    private var searchRow: some View {
        HStack(spacing: TomiSpace.s1) {
            TextField(L10n.text("code_search_placeholder"), text: Binding(
                get: { query },
                // One line: a line end would make an occurrence that cannot be shown.
                set: { query = String(($0.split(separator: "\n", omittingEmptySubsequences: false).first ?? "").prefix(Self.maxQuery)) }
            ))
            .tomiText(.body)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .submitLabel(.search)
            .focused($searchFocused)
            .onSubmit { current = CodeSearch.next(current: current, count: matches.count) }
            Text(verbatim: CodeSearch.counter(current: current, count: matches.count))
                .tomiMono(size: 13)
                .foregroundStyle(TomiColor.textMuted)
            Button { current = CodeSearch.previous(current: current, count: matches.count) } label: {
                Image(systemName: "chevron.up").frame(width: 36, height: 44)
            }
            .disabled(matches.isEmpty)
            .accessibilityLabel(Text("code_search_previous"))
            Button { current = CodeSearch.next(current: current, count: matches.count) } label: {
                Image(systemName: "chevron.down").frame(width: 36, height: 44)
            }
            .disabled(matches.isEmpty)
            .accessibilityLabel(Text("code_search_next"))
            Button {
                searching = false
                query = ""
            } label: {
                TomiIcon.close.image.frame(width: 36, height: 44)
            }
            .accessibilityLabel(Text("action_close_search"))
        }
        .foregroundStyle(TomiColor.text)
        .padding(.leading, TomiSpace.s4)
        .padding(.trailing, TomiSpace.s1)
        .background(TomiColor.surfaceAlt, in: Capsule())
        .padding(.horizontal, TomiSpace.screen)
        .padding(.vertical, TomiSpace.s2)
        .onAppear { searchFocused = true }
    }

    /// Brings the current occurrence into view: its line, and its column when the lines are not folded.
    private func scroll(_ proxy: ScrollViewProxy) {
        guard matches.indices.contains(current) else { return }
        let match = matches[current]
        let line = doc.lineOf(match.start)
        var x: CGFloat = 0
        if !wrap, contentWidth > 0 {
            let column = CGFloat(gutterChars + Self.gapChars + doc.columnOf(match.start)) * Self.charWidth
            x = min(max(column / contentWidth, 0), 1)
        }
        withAnimation(.easeOut(duration: TomiMotion.medium)) {
            proxy.scrollTo(line, anchor: UnitPoint(x: x, y: 0.4))
        }
    }
}
