package eu.shaffe.tomi.ui.render

import org.jsoup.Jsoup
import org.jsoup.nodes.Document
import org.jsoup.safety.Safelist

/**
 * Sanitises received HTML (DEV.md invariant 6) before it goes to the sandboxed WebView.
 * Defence in depth: this allow-list, the CSP below, and the WebView itself (no JS, no network, no file access).
 */
object HtmlSanitizer {
    const val CSP = "default-src 'none'; img-src data:; style-src 'unsafe-inline'"

    private val safelist: Safelist = Safelist.relaxed()
        // Images only as inline data: URIs, never http(s).
        .removeProtocols("img", "src", "http", "https")
        .addProtocols("img", "src", "data")
    // No "style" attribute anywhere: inline CSS from a stranger can hide, overlay or fake parts of the page
    // (position:fixed phishing overlays, invisible text, CSS-only tracking selectors). Layout comes from our own <style>.
    // The CSP still allows 'unsafe-inline' styles, but only because the document's own <style> block needs it.

    /** Returns the cleaned body fragment: no script, iframe, form, event handler or remote resource. */
    fun clean(html: String): String =
        Jsoup.clean(html, "", safelist, Document.OutputSettings().prettyPrint(false))

    /**
     * Full document to load in the WebView: cleaned body, CSP meta tag, and theme colours. With [horizontalScroll] false
     * (card preview) the page never scrolls sideways, so that a drag across a wide table is left to the card (swipe to archive).
     */
    fun toDocument(html: String, textColorCss: String, linkColorCss: String, horizontalScroll: Boolean = true): String =
        """
        <!DOCTYPE html>
        <html>
        <head>
        <meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy" content="$CSP">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <style>
        html, body { margin: 0; padding: 0; background: transparent; color: $textColorCss;
          font-family: sans-serif; font-size: 15px; line-height: 1.4; word-wrap: break-word; }
        ${if (horizontalScroll) "" else "html, body { overflow-x: hidden; }"}
        a { color: $linkColorCss; }
        img { max-width: 100%; height: auto; }
        pre, code { white-space: pre-wrap; }
        table { border-collapse: collapse; }
        td, th { border: 1px solid; padding: 2px 6px; }
        </style>
        </head>
        <body>${clean(html)}</body>
        </html>
        """.trimIndent()
}
