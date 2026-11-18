import SwiftUI
import TomiDesign
import TomiKit

// The long texts of the detail (Android `ChunkedMarkdown`, `ChunkedText` and `ProgressiveColumn`).
//
// The detail shows the whole text. Guarded, parsed and laid out in one piece, a long document froze the screen (a
// 200 KB Markdown = thousands of text views built at once on the main thread). It is shown as a column of parts
// instead (`MarkdownBlocks.chunks` / `sourceChunks`), each its own view.
//
// Deviation from Android: the column is a `LazyVStack`. Compose cannot nest a lazy list in a scrolling column, so
// Android reveals the parts by hand (`ProgressiveColumn`) and reserves their room itself; a SwiftUI lazy stack builds
// only the parts near the screen, frees them when they are far, and estimates the height of those it did not build.
// The parts only live in memory: copy, share and search use the full text.

/// One view of a long text in the detail: a Markdown chunk to render, or a piece of text shown as is. Never prints its
/// content.
struct DetailPart: Identifiable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    enum Kind: Sendable {
        case markdown
        /// Plain text, Markdown source, revealed hidden item, or a chunk too long to cut (`Chunk.asSource`).
        case literal
    }

    let id: Int
    let kind: Kind
    let text: String

    var description: String { "DetailPart(<redacted>)" }
    var debugDescription: String { description }
    var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

enum DetailParts {
    /// The parts of a Markdown document: its chunks, a chunk too long to cut being shown as pieces of plain text.
    nonisolated static func markdown(_ markdown: String) -> [DetailPart] {
        var parts: [DetailPart] = []
        for chunk in MarkdownBlocks.chunks(markdown) {
            if chunk.asSource {
                for piece in MarkdownBlocks.sourceChunks(chunk.markdown) {
                    parts.append(DetailPart(id: parts.count, kind: .literal, text: piece))
                }
            } else {
                parts.append(DetailPart(id: parts.count, kind: .markdown, text: chunk.markdown))
            }
        }
        return parts
    }

    /// The parts of a text shown as is: pieces of whole lines.
    nonisolated static func literal(_ text: String) -> [DetailPart] {
        MarkdownBlocks.sourceChunks(text).enumerated().map { DetailPart(id: $0.offset, kind: .literal, text: $0.element) }
    }
}

/// The parts of one text of the detail, cut once: at once for a short text, off the main thread for a long one (the
/// screen then opens on a blank space, never on a frozen transition). Also parses the Markdown chunks: ahead of the
/// scroll and off the main thread for a long document, on demand otherwise. The guard runs on each chunk, on the very
/// string the parser gets (repeated table header, added fence and appended definitions included).
///
/// Lives with the view that shows the text (`@State`): a revealed hidden item is only cut in memory, and what was cut
/// goes away with the view when it is hidden again.
@MainActor
@Observable
final class DetailTextPreparation {
    /// Bumped when the parts of a long text land: the view that asked is drawn again.
    private var landed = 0
    @ObservationIgnored private var text: String?
    @ObservationIgnored private var markdown = false
    @ObservationIgnored private var parts: [DetailPart]?
    /// Parsed chunks by part id; `.some(nil)`: the guard refused the chunk, it is shown as text.
    @ObservationIgnored private var blocks: [Int: [MarkdownBlock]?] = [:]
    @ObservationIgnored private var task: Task<Void, Never>?

    /// A document longer than this is not parsed ahead in full: its chunks are parsed when they come on screen.
    private nonisolated static let maxAheadParts = 400

    /// The height of a tall iPhone screen, in points: enough to tell which parts the first screen shows.
    private nonisolated static let nominalScreenHeight = 1_000.0

    deinit {
        task?.cancel()
    }

    /// The parts of `text`, or nil while a long text is being cut. Called from a `body`: the cutting starts at the first
    /// evaluation (not at a later `onAppear`), and the same text asked again costs a comparison.
    func parts(of text: String, markdown: Bool) -> [DetailPart]? {
        _ = landed
        if self.markdown == markdown, let current = self.text, current == text { return parts }
        task?.cancel()
        task = nil
        self.text = text
        self.markdown = markdown
        blocks = [:]
        let limit = markdown ? ProgressiveReveal.syncChunkingChars : ProgressiveReveal.syncSourceChars
        if text.utf16.count <= limit {
            parts = markdown ? DetailParts.markdown(text) : DetailParts.literal(text)
            return parts
        }
        parts = nil
        task = Task.detached(priority: .userInitiated) { [weak self] in
            let made = markdown ? DetailParts.markdown(text) : DetailParts.literal(text)
            // The parts of the first screen come parsed with the list: the screen is drawn in one go.
            let first = Self.firstScreen(made).map { ($0.id, MarkdownRendering.blocks($0.text)) }
            guard !Task.isCancelled else { return }
            await self?.land(made, first: first, for: text, markdown: markdown)
            // Then the next ones, ahead of the scroll.
            let ready = Set(first.map(\.0))
            for part in made.prefix(Self.maxAheadParts) where part.kind == .markdown && !ready.contains(part.id) {
                if Task.isCancelled { return }
                let rendered = MarkdownRendering.blocks(part.text)
                await self?.store(rendered, for: part.id, of: text, markdown: markdown)
            }
        }
        return nil
    }

    /// The parsed blocks of a Markdown part, or nil when the guard refuses it.
    func blocks(for part: DetailPart) -> [MarkdownBlock]? {
        if let known = blocks[part.id] { return known }
        let rendered = MarkdownRendering.blocks(part.text)
        blocks[part.id] = .some(rendered)
        return rendered
    }

    private func isCurrent(_ text: String, _ markdown: Bool) -> Bool {
        self.markdown == markdown && self.text == text
    }

    /// The Markdown parts the first screen shows: those that fill a screen by their estimated height, and the one after
    /// (the lazy stack builds a little beyond the screen).
    private nonisolated static func firstScreen(_ parts: [DetailPart]) -> [DetailPart] {
        let filling = ProgressiveReveal.initialItems(
            count: parts.count,
            weight: { ProgressiveReveal.textWeight(parts[$0].text) },
            screenWeight: ProgressiveReveal.screenWeight(height: nominalScreenHeight)
        )
        return parts.prefix(filling + 1).filter { $0.kind == .markdown }
    }

    private func land(_ made: [DetailPart], first: [(id: Int, blocks: [MarkdownBlock]?)], for text: String, markdown: Bool) {
        guard isCurrent(text, markdown) else { return }
        for part in first { blocks[part.id] = .some(part.blocks) }
        parts = made
        landed += 1
    }

    private func store(_ rendered: [MarkdownBlock]?, for id: Int, of text: String, markdown: Bool) {
        guard isCurrent(text, markdown), blocks[id] == nil else { return }
        blocks[id] = .some(rendered)
    }
}

/// Stands for a long text that is being cut: a screen of blank space, so that what follows the text (the metadata of
/// the detail) does not show at the top and jump down a moment later. No progress indicator (charter).
private struct PendingText: View {
    var body: some View {
        Color.clear
            .frame(maxWidth: .infinity)
            .containerRelativeFrame(.vertical)
            .accessibilityHidden(true)
    }
}

/// A Markdown document in the detail: its chunks, each rendered on its own. A chunk the guard refuses is shown as text
/// (its own text, or `fallbackPlain` when the document is that single chunk); the other chunks are still rendered. A
/// short document is one chunk, on the same path. Selection is per block; copy and share use the full text.
struct ChunkedMarkdown: View {
    let markdown: String
    let fallbackPlain: String
    @State private var preparation = DetailTextPreparation()

    var body: some View {
        if let parts = preparation.parts(of: markdown, markdown: true) {
            LazyVStack(alignment: .leading, spacing: TomiSpace.s2) {
                ForEach(parts) { part in
                    MarkdownPart(part: part, alone: parts.count == 1, fallbackPlain: fallbackPlain, preparation: preparation)
                }
            }
        } else {
            PendingText()
        }
    }
}

private struct MarkdownPart: View {
    let part: DetailPart
    let alone: Bool
    let fallbackPlain: String
    let preparation: DetailTextPreparation

    var body: some View {
        switch part.kind {
        case .markdown:
            if let blocks = preparation.blocks(for: part) {
                MarkdownView(blocks: blocks).textSelection(.enabled)
            } else {
                Text(alone ? fallbackPlain : part.text)
                    .tomiText(.body)
                    .foregroundStyle(TomiColor.text)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
        case .literal:
            // Too long to cut: its exact text, not rendered.
            Text(part.text)
                .tomiMono(size: 13).lineSpacing(6)
                .foregroundStyle(TomiColor.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
        }
    }
}

/// A text shown as is in the detail (plain text, Markdown source, revealed hidden item): pieces of whole lines, each
/// its own text built by `piece`. A short text stays one text. `spacing` is the line spacing of the text style, so
/// that two pieces are as far apart as two lines.
struct ChunkedLiteral<Piece: View>: View {
    let text: String
    let spacing: CGFloat
    @ViewBuilder let piece: (String) -> Piece
    @State private var preparation = DetailTextPreparation()

    var body: some View {
        if let parts = preparation.parts(of: text, markdown: false) {
            if parts.count <= 1 {
                piece(text)
            } else {
                LazyVStack(alignment: .leading, spacing: spacing) {
                    ForEach(parts) { part in
                        piece(part.text)
                    }
                }
            }
        } else {
            PendingText()
        }
    }
}
