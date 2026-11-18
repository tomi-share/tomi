import SwiftUI
import TomiDesign
import TomiKit

/// Markdown rendered natively, never from anything but the tree `MarkdownGuard.render` checked (no image can be in it,
/// so nothing is ever fetched). Port of `RichMarkdownView` (RichTextViews.kt): paragraph Inter 15/22, headings 20 / 18
/// / 16 semibold, code in mono 13 on `surfaceAlt`. Links open only for http, https, mailto and tel. Returns nil when the
/// guard refuses the text: the caller shows the plain text.
struct MarkdownView: View {
    let blocks: [MarkdownBlock]

    init?(markdown: String) {
        guard let rendered = MarkdownGuard.render(markdown, syntax: .full) else { return nil }
        blocks = MarkdownBlock.split(rendered)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: TomiSpace.s2) {
            ForEach(blocks) { block in
                block.view
            }
        }
        .environment(\.openURL, OpenURLAction { url in
            SafeLinks.isOpenable(url) ? .systemAction(url) : .discarded
        })
    }
}

enum SafeLinks {
    static func isOpenable(_ url: URL) -> Bool {
        ["http", "https", "mailto", "tel"].contains(url.scheme?.lowercased() ?? "")
    }
}

/// One block of the parsed markdown (paragraph, heading, list item, quote, code block), with its inline text styled.
struct MarkdownBlock: Identifiable {
    enum Kind: Equatable {
        case paragraph
        case heading(Int)
        case listItem(marker: String, depth: Int)
        case quote
        case code
    }

    let id: Int
    let kind: Kind
    let text: AttributedString

    @ViewBuilder
    var view: some View {
        switch kind {
        case .paragraph:
            Text(text).tomiText(.body).foregroundStyle(TomiColor.text)
        case let .heading(level):
            Text(text)
                .font(.custom(TomiTokens.FontFamily.ui, size: level == 1 ? 20 : level == 2 ? 18 : level == 3 ? 16 : 15).weight(.semibold))
                .foregroundStyle(TomiColor.text)
                .padding(.top, TomiSpace.s1)
        case let .listItem(marker, depth):
            HStack(alignment: .firstTextBaseline, spacing: TomiSpace.s2) {
                Text(marker).tomiText(.body).foregroundStyle(TomiColor.textMuted)
                Text(text).tomiText(.body).foregroundStyle(TomiColor.text)
            }
            .padding(.leading, CGFloat(depth) * TomiSpace.s4)
        case .quote:
            Text(text)
                .tomiText(.body)
                .foregroundStyle(TomiColor.textMuted)
                .padding(.leading, TomiSpace.s3)
                .overlay(alignment: .leading) { Rectangle().fill(TomiColor.border).frame(width: 3) }
        case .code:
            Text(text)
                .font(.custom(TomiTokens.FontFamily.mono, size: 13))
                .foregroundStyle(TomiColor.text)
                .lineSpacing(6)
                .padding(TomiSpace.s3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border))
        }
    }

    /// Groups the runs by block (the presentation intent of the parser) and styles their inline parts.
    static func split(_ rendered: AttributedString) -> [MarkdownBlock] {
        var blocks: [MarkdownBlock] = []
        var currentIntent: PresentationIntent?
        var current = AttributedString()
        var first = true

        func flush() {
            guard !current.characters.isEmpty else { return }
            blocks.append(MarkdownBlock(id: blocks.count, kind: kind(of: currentIntent), text: current))
            current = AttributedString()
        }

        for run in rendered.runs {
            let intent = run.presentationIntent
            if first || intent != currentIntent {
                flush()
                currentIntent = intent
                first = false
            }
            var piece = AttributedString(rendered[run.range].characters)
            style(&piece, inline: run.inlinePresentationIntent, link: run.link, isCode: kind(of: intent) == .code)
            current += piece
        }
        flush()
        return blocks
    }

    private static func kind(of intent: PresentationIntent?) -> Kind {
        guard let intent else { return .paragraph }
        var depth = -1
        var marker: String?
        var result: Kind = .paragraph
        // `components` go from the innermost block outwards.
        for component in intent.components {
            switch component.kind {
            case let .header(level):
                if case .paragraph = result { result = .heading(level) }
            case .codeBlock:
                return .code
            case .blockQuote:
                if case .paragraph = result { result = .quote }
            case let .listItem(ordinal):
                if marker == nil { marker = "\(ordinal)." }
                depth += 1
            case .unorderedList:
                if marker?.hasSuffix(".") == true, depth == 0 { marker = "•" }
            default:
                break
            }
        }
        if let marker { return .listItem(marker: marker, depth: max(0, depth)) }
        return result
    }

    private static func style(_ piece: inout AttributedString, inline: InlinePresentationIntent?, link: URL?, isCode: Bool) {
        guard !isCode else { return }
        let ui = TomiTokens.FontFamily.ui
        var font = Font.custom(ui, size: 15)
        if let inline {
            if inline.contains(.code) { font = .custom(TomiTokens.FontFamily.mono, size: 13) }
            if inline.contains(.stronglyEmphasized) { font = font.weight(.semibold) }
            if inline.contains(.emphasized) { font = font.italic() }
            if inline.contains(.strikethrough) { piece.strikethroughStyle = .single }
            if inline.contains(.code) { piece.backgroundColor = TomiColor.surfaceAlt }
        }
        piece.font = font
        if let link, SafeLinks.isOpenable(link) {
            piece.link = link
            piece.foregroundColor = TomiColor.brand
            piece.underlineStyle = .single
        }
    }
}
