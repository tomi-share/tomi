import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/render/RenderMode.kt.

/// How a text card is displayed. Chosen when reading, not when sending.
///
/// - `preview` ("Aperçu", default): the styled rendering. Markdown goes through the native markdown renderer, HTML
///   through the sandboxed view, plain text as text.
/// - `source` ("Markdown"): the Markdown source of the text, literally, in the monospace font (`#`, `-`, "```" stay
///   visible). Only offered when the item carries a Markdown representation.
///
/// HTML is never shown as source: there is no mode in which markup is displayed as text.
public enum RenderMode: Sendable, Hashable, CaseIterable {
    case preview, source

    /// Key of the label of the segmented control in the strings of the app (`render_preview` "Aperçu",
    /// `render_markdown` "Markdown").
    public var labelKey: String {
        switch self {
        case .preview: "render_preview"
        case .source: "render_markdown"
        }
    }
}

/// The renderer behind `RenderMode.preview`. HTML always goes through the sandboxed view, Markdown through the native
/// renderer.
public enum PreviewKind: Sendable, Hashable {
    case markdown, html, plain
}

/// What "Copy" puts on the clipboard: exactly what the card currently shows.
public struct CopyContent: Hashable, Sendable, CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    public var plain: String
    public var html: String?

    public init(plain: String, html: String? = nil) {
        self.plain = plain
        self.html = html
    }

    /// It is the text that goes to the clipboard, possibly a secret: never printed.
    public var description: String { "CopyContent(<redacted>)" }
    public var debugDescription: String { description }
    public var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

public enum RenderModes {
    /// Modes the card can offer, in display order. `.source` only exists for an item that has Markdown.
    public static func available(_ text: RichText) -> [RenderMode] {
        var modes: [RenderMode] = [.preview]
        if text.markdownSource != nil { modes.append(.source) }
        return modes
    }

    /// The last chosen mode if this item supports it, otherwise the styled preview.
    public static func resolve(_ text: RichText, preferred: RenderMode?) -> RenderMode {
        if let preferred, available(text).contains(preferred) { return preferred }
        return .preview
    }

    /// What the `.preview` of `text` is made of: the format the content was captured in, with the others as fallback.
    public static func previewKind(_ text: RichText) -> PreviewKind {
        switch text.sourceFormat {
        case .markdown:
            if text.markdown != nil { return .markdown }
            if text.html != nil { return .html }
            // Declared Markdown without a separate field: the plain text is the Markdown.
            return .markdown
        case .html:
            if text.html != nil { return .html }
            if text.markdown != nil { return .markdown }
            return .plain
        case .plain:
            return .plain
        }
    }

    public static func copyContent(_ text: RichText, mode: RenderMode) -> CopyContent {
        switch mode {
        case .preview:
            // The styled HTML travels with its plain text; a Markdown or plain preview copies the plain text.
            return CopyContent(plain: text.plain, html: previewKind(text) == .html ? text.html : nil)
        case .source:
            return CopyContent(plain: text.markdownSource ?? text.plain)
        }
    }
}
