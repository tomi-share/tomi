package eu.shaffe.tomi.ui.render

import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.SourceFormat

/**
 * How a text card is displayed. Chosen when reading, not when sending.
 *
 * - [Preview] ("Aperçu", default): the styled rendering. Markdown goes through the native markdown renderer, HTML through the
 *   sandboxed view, plain text as text.
 * - [Source] ("Markdown"): the Markdown source of the text, literally, in the monospace font (`#`, `-`, `` ``` `` stay visible).
 *   Only offered when the item carries a Markdown representation.
 *
 * HTML is never shown as source: there is no mode in which markup is displayed as text.
 */
enum class RenderMode { Preview, Source }

/** The renderer behind [RenderMode.Preview]. HTML always goes through the sandboxed view, Markdown through the native renderer. */
enum class PreviewKind { Markdown, Html, Plain }

/** What "Copy" puts on the clipboard: exactly what the card currently shows. */
data class CopyContent(val plain: String, val html: String? = null) {
    /** It is the text that goes to the clipboard, possibly a secret: never printed. */
    override fun toString(): String = "CopyContent(<redacted>)"
}

object RenderModes {
    /** Modes the card can offer, in display order. [RenderMode.Source] only exists for an item that has Markdown. */
    fun available(text: RichText): List<RenderMode> = buildList {
        add(RenderMode.Preview)
        if (hasMarkdown(text)) add(RenderMode.Source)
    }

    /**
     * True when the item is Markdown. A sender (Windows) omits the `markdown` field when it equals the plain text and only
     * declares `source_format = markdown`: the plain text then IS the Markdown source.
     */
    private fun hasMarkdown(text: RichText): Boolean = text.markdown != null || text.sourceFormat == SourceFormat.Markdown

    /** The last chosen mode if this item supports it, otherwise the styled preview. */
    fun resolve(text: RichText, preferred: RenderMode?): RenderMode =
        preferred?.takeIf { it in available(text) } ?: RenderMode.Preview

    /** What the [RenderMode.Preview] of [text] is made of: the format the content was captured in, with the others as fallback. */
    fun previewKind(text: RichText): PreviewKind = when (text.sourceFormat) {
        SourceFormat.Markdown -> when {
            text.markdown != null -> PreviewKind.Markdown
            text.html != null -> PreviewKind.Html
            // Declared Markdown without a separate field: the plain text is the Markdown.
            else -> PreviewKind.Markdown
        }
        SourceFormat.Html -> when {
            text.html != null -> PreviewKind.Html
            text.markdown != null -> PreviewKind.Markdown
            else -> PreviewKind.Plain
        }
        SourceFormat.Plain -> PreviewKind.Plain
    }

    fun copyContent(text: RichText, mode: RenderMode): CopyContent = when (mode) {
        // The styled HTML travels with its plain text; a Markdown or plain preview copies the plain text.
        RenderMode.Preview -> CopyContent(plain = text.plain, html = text.html.takeIf { previewKind(text) == PreviewKind.Html })
        RenderMode.Source -> CopyContent(text.markdown ?: text.plain)
    }
}
