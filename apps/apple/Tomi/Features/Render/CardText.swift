import SwiftUI
import TomiKit

/// What a feed card shows of a text: its bounded beginning (`MarkdownBlocks.preview` / `head`), cut BEFORE the guard,
/// the parser and the layout, and for a Markdown preview its parsed blocks. The full text stays in the model: the
/// detail, copy, share, search and the character count use it.
struct CardText: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    /// The card shows less than the text: "Lire la suite" without measuring anything.
    let truncated: Bool
    /// The parsed Markdown preview; nil for a text shown as is, or when the guard refused the preview.
    let blocks: [MarkdownBlock]?
    /// The beginning of the text shown as is (plain text, Markdown source), or what stands for a refused preview.
    let literal: String

    var description: String { "CardText(<redacted>, \(truncated))" }
    var debugDescription: String { description }
    var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// The card texts already prepared, so that a card is cut and parsed once and not at each evaluation of its `body`
/// (every scroll frame that builds it, every change of the feed). In memory only, bounded, emptied when the core is
/// replaced and when the app leaves the screen.
///
/// Never a hidden item: nothing is computed for a closed one, and a revealed one is cut on the spot (`head`), its
/// beginning never kept here.
@MainActor
enum CardTexts {
    enum Kind: UInt8 {
        /// The styled preview of a Markdown text.
        case markdown
        /// The Markdown source, shown as is.
        case source
        /// A plain text.
        case plain
    }

    private struct Key: Hashable {
        let itemId: String
        let kind: Kind
        /// The text of an item does not change; its length guards against an id that would come back with another.
        let length: Int
    }

    private static var cache: [Key: CardText] = [:]
    private static let maxEntries = 400

    static func text(itemId: String, kind: Kind, of text: RichText) -> CardText {
        let source = kind == .plain ? text.plain : (text.markdownSource ?? text.plain)
        let key = Key(itemId: itemId, kind: kind, length: source.utf8.count)
        if let hit = cache[key] { return hit }
        let made = make(kind, source: source, plain: text.plain)
        if cache.count >= maxEntries { cache.removeAll(keepingCapacity: true) }
        cache[key] = made
        return made
    }

    static func clear() {
        cache.removeAll()
    }

    /// The first lines of a text shown as is in a card (plain text, Markdown source, revealed hidden item).
    nonisolated static func head(_ text: String) -> MarkdownBlocks.Preview {
        MarkdownBlocks.head(text, maxLines: MarkdownBlocks.cardBudget.maxLines, maxChars: MarkdownBlocks.cardBudget.maxChars)
    }

    private static func make(_ kind: Kind, source: String, plain: String) -> CardText {
        switch kind {
        case .markdown:
            // The guard only sees what will be rendered: the preview, with the definitions it uses.
            let shown = MarkdownBlocks.preview(source)
            if let blocks = MarkdownRendering.blocks(shown.markdown) {
                return CardText(truncated: shown.truncated, blocks: blocks, literal: "")
            }
            let fallback = head(plain)
            return CardText(truncated: shown.truncated || fallback.truncated, blocks: nil, literal: fallback.markdown)
        case .source, .plain:
            let shown = head(source)
            return CardText(truncated: shown.truncated, blocks: nil, literal: shown.markdown)
        }
    }
}

/// Runs once, off the main thread at start-up, the text code that the first card and the first detail would otherwise
/// run for the first time on the main thread (Android `warmUpTextRendering`): a first call pays for the Unicode tables,
/// the Markdown parser of Foundation and the fonts of the attributed strings, far more than every call after it.
/// Nothing is kept; the sample holds no user content.
enum TextRenderingWarmUp {
    nonisolated static func run() {
        let sample = "# t\n\ntexte [l][r] `c` **g** *i*\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```swift\nlet x = 1\n```\n\n"
            + "1. un\n2. deux\n\n- [ ] t\n\n> q\n\n[r]: https://relay.example.org\n"
        let tight = MarkdownBlocks.Budget(maxChars: 40, maxLines: 4, maxTableRows: 1)
        _ = MarkdownBlocks.preview(sample, budget: tight)
        _ = MarkdownBlocks.head(sample, maxLines: 2, maxChars: 20)
        _ = MarkdownBlocks.chunks(sample, budget: tight)
        _ = MarkdownBlocks.sourceChunks(sample, maxLines: 2, maxChars: 20)
        _ = MarkdownRendering.blocks("![i](x) " + sample)
    }
}
