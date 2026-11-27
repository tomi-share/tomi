package eu.shaffe.tomi.ui.render

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFontFamilyResolver
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.net.toUri
import com.swmansion.enriched.markdown.codehighlight.compose.codeHighlight
import com.swmansion.enriched.markdown.CodeBlockLayout
import com.swmansion.enriched.markdown.EnrichedMarkdown
import com.swmansion.enriched.markdown.compose.MarkdownStyle
import com.swmansion.enriched.markdown.compose.material3.materialMarkdownStyle
import com.swmansion.enriched.markdown.parser.MarkdownASTNode
import com.swmansion.enriched.markdown.parser.Parser
import com.swmansion.enriched.markdown.plugin.InternalPluginApi
import com.swmansion.enriched.markdown.plugin.MarkdownPlugin
import com.swmansion.enriched.markdown.spoiler.SpoilerOverlay
import eu.shaffe.tomi.ui.code.TomiCodeHighlightPlugin
import eu.shaffe.tomi.ui.theme.CodePalette
import eu.shaffe.tomi.ui.theme.TomiFonts
import java.io.ByteArrayInputStream
import kotlinx.coroutines.delay

private val ALLOWED_LINK_SCHEMES = setOf("http", "https", "mailto", "tel")

private const val PAGE_SETTLE_DELAY_MS = 150L

private const val RENDER_TAG = "TomiMarkdown"

/** How long a native Markdown view may stay without a height before it is created again, and how many times that is done. */
private const val RENDER_LANDING_TIMEOUT_MS = 2_500L
private const val MAX_RENDER_RETRIES = 3

/** Opens a link from rendered content, but only for harmless schemes (no intent:, file:, content:, javascript:). */
private fun openExternalLink(context: Context, uri: Uri) {
    if (uri.scheme?.lowercase() !in ALLOWED_LINK_SCHEMES) return
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
        // No app can handle it: ignore.
    }
}

/** The library's markdown parser, for [MarkdownGuard]. Any failure (native library missing included) is "cannot parse". */
internal fun parseWithLibrary(markdown: String): MarkdownASTNode? = try {
    Parser.shared.parseMarkdown(markdown, MarkdownEngineConfig.parserFlags)
} catch (e: Throwable) {
    Log.w("MarkdownGuard", "markdown parser unavailable: shown as plain text", e)
    null
}

/**
 * Markdown rendered by Enriched Markdown: native Android text (no WebView, no HTML), fenced code coloured by tree-sitter with the
 * charter palette. 100% offline: [MarkdownGuard] removes every image before the renderer sees the text, so nothing is fetched; links
 * only open on a tap, through [openExternalLink]. If the guard refuses the markdown, [fallbackPlain] is shown as text instead.
 *
 * [maxHeight] cuts the view (card); [onClipped] says whether the content is taller than that. [truncated]: [markdown] is
 * already a cut preview ([preview]), so the card is known to be cut: it takes exactly [maxHeight] at once (no jump from 0 to
 * the measured height) and [onClipped] gets true without waiting for a measure. [selectable] must be false for anything
 * sensitive: the library's own copy menus write to the clipboard without the sensitive flag.
 *
 * Without [maxHeight] (the detail), a long document is cut into chunks ([chunks]) rendered as a column of views revealed while
 * scrolling ([ChunkedMarkdown]); the guard then runs per chunk, and [fallbackPlain] only stands for a document of one chunk.
 */
@Composable
fun RichMarkdownView(
    markdown: String,
    fallbackPlain: String,
    selectable: Boolean,
    modifier: Modifier = Modifier,
    maxHeight: Dp? = null,
    onClipped: ((Boolean) -> Unit)? = null,
    truncated: Boolean = false,
) {
    // The detail (no height limit): a long document is rendered as chunks of whole blocks, each its own view (MarkdownBlocks).
    // Cutting a long one happens off the main thread: the screen opens on a blank text, never on a blocked transition.
    // A short one is a column of one chunk: the same room is kept for it until its rendering lands (see ProgressiveColumn).
    if (maxHeight == null) {
        val parts = rememberOffMain(markdown, SYNC_CHUNKING_CHARS) { chunks(it) }
        if (parts == null) {
            PendingText(modifier)
            return
        }
        if (parts.isNotEmpty()) {
            ChunkedMarkdown(parts, markdown, fallbackPlain, selectable, modifier)
            return
        }
    }
    val context = LocalContext.current
    val safe = remember(markdown) { MarkdownGuard.safeMarkdown(markdown, ::parseWithLibrary) }
    val currentOnClipped by rememberUpdatedState(onClipped)
    if (safe == null) {
        Text(
            fallbackPlain,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = if (maxHeight != null) CARD_PLAIN_LINES else Int.MAX_VALUE,
            overflow = TextOverflow.Ellipsis,
            modifier = modifier,
            onTextLayout = { result -> currentOnClipped?.invoke(result.hasVisualOverflow || truncated) },
        )
        return
    }
    val style = rememberTomiMarkdownStyle()
    HeightLimited(maxHeight, truncated, onClipped, modifier) {
        TomiMarkdownText(
            markdown = safe,
            style = style,
            selectable = selectable,
            plugins = MARKDOWN_PLUGINS,
            onLinkClick = { url -> openExternalLink(context, url.toUri()) },
            modifier = Modifier.fillMaxWidth(),
            fallbackPlain = if (maxHeight != null) fallbackPlain else null,
        )
    }
}

/** One view of a chunked document: a chunk to render, or a piece of a chunk shown as plain text. No content in toString. */
private sealed interface DetailPart {
    val weight: Int
}

private class MarkdownPart(val markdown: String, val last: Boolean) : DetailPart {
    override val weight = textWeight(markdown)
    override fun toString() = "MarkdownPart(<redacted>)"
}

private class SourcePart(val text: String) : DetailPart {
    override val weight = textWeight(text)
    override fun toString() = "SourcePart(<redacted>)"
}

/**
 * The chunks of a document in the detail, revealed progressively ([ProgressiveColumn]); a short document is one chunk. The guard
 * runs PER CHUNK, when it is revealed: no chunk ever reaches the renderer with an image, and a chunk the guard refuses is shown
 * as text (the other chunks are still rendered): its own text, or [fallbackPlain] when the document is that single chunk. A
 * chunk too long to cut ([Chunk.asSource]) is shown as plain monospace text. Every chunk but the last keeps the bottom margin
 * of its last block, so the chunks are spaced as the whole document was. Selection is per chunk; copying, sharing and searching
 * use the full text.
 */
@Composable
private fun ChunkedMarkdown(parts: List<Chunk>, markdown: String, fallbackPlain: String, selectable: Boolean, modifier: Modifier) {
    val items = remember(parts) {
        val list = ArrayList<DetailPart>()
        parts.forEachIndexed { index, chunk ->
            if (chunk.asSource) {
                sourceChunks(chunk.markdown).forEach { list += SourcePart(it) }
            } else {
                list += MarkdownPart(chunk.markdown, last = index == parts.lastIndex)
            }
        }
        list
    }
    val context = LocalContext.current
    val style = rememberTomiMarkdownStyle()
    val plugins = MARKDOWN_PLUGINS
    ProgressiveColumn(items.size, weight = { items[it].weight }, resetKey = markdown, modifier = modifier) { index ->
        when (val part = items[index]) {
            is MarkdownPart -> {
                val safe = remember(part.markdown) { MarkdownGuard.safeMarkdown(part.markdown, ::parseWithLibrary) }
                if (safe == null) {
                    Text(
                        if (items.size == 1) fallbackPlain else part.markdown,
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.fillMaxWidth(),
                    )
                } else {
                    TomiMarkdownText(
                        markdown = safe,
                        style = style,
                        selectable = selectable,
                        plugins = plugins,
                        onLinkClick = { url -> openExternalLink(context, url.toUri()) },
                        modifier = Modifier.fillMaxWidth(),
                        allowTrailingMargin = !part.last,
                    )
                }
            }
            is SourcePart -> Text(
                part.text,
                style = MaterialTheme.typography.bodyMedium.copy(fontFamily = TomiFonts.mono, fontSize = 13.sp, lineHeight = 19.sp),
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

/**
 * Same native view as the library's `EnrichedMarkdownText`, minus its flash of unstyled content: the library composable first
 * shows the text with its own light default style (dark text, pink inline code: unreadable in dark mode) and swaps in the real
 * style once a coroutine has resolved it. Here the style is resolved synchronously, before the view exists, so the first
 * rendered frame already carries the right colours. `resolveStyleConfig` is flagged "internal, not for third-party use": the library
 * version is pinned (0.3.0), like the other `InternalPluginApi` uses (ui/code).
 */
@OptIn(InternalPluginApi::class)
@Composable
private fun TomiMarkdownText(
    markdown: String,
    style: MarkdownStyle,
    selectable: Boolean,
    plugins: List<MarkdownPlugin>,
    onLinkClick: (String) -> Unit,
    modifier: Modifier = Modifier,
    /** Plain text shown when the rendering never lands (after the retries); null: keep waiting. */
    fallbackPlain: String? = null,
    /** Keeps the bottom margin of the last block: true for a chunk followed by another one (spacing of the whole document). */
    allowTrailingMargin: Boolean = false,
) {
    val context = LocalContext.current
    val density = LocalDensity.current
    val fontResolver = LocalFontFamilyResolver.current
    // One StyleConfig per view, never shared: the library renders each view on its own worker thread and mutates the config
    // while rendering (StyleConfig.withParagraphOverride), so a config shared between views would race.
    val styleConfig = remember(style, context, density, fontResolver) { style.resolveStyleConfig(context, density, fontResolver) }
    val currentOnLinkClick by rememberUpdatedState(onLinkClick)
    val onLinkPress = remember { { url: String -> currentOnLinkClick(url) } }

    // The library renders on three fixed worker threads and drops a result that arrives stale; a view whose rendering never lands
    // stays at zero height for good (a blank card). Seen in the feed: only some cards of a list rendered. So when a view still has
    // no height after RENDER_LANDING_TIMEOUT_MS it is created again (a new view: new render, maybe another worker), up to
    // MAX_RENDER_RETRIES times. Leaving the screen cancels the wait.
    var attempt by remember(markdown) { mutableIntStateOf(0) }
    val landed = remember(markdown, attempt) { BooleanArray(1) }
    // Set once the last attempt has waited in vain: the card then shows `fallbackPlain` instead of a blank area.
    var gaveUp by remember(markdown) { mutableStateOf(false) }
    // Bumped when the library adds or removes its rendered segments. Read in the layout block below, it makes Compose measure the
    // node again even if the view's own requestLayout never reached Compose (a blank card at start-up, toggling the tab fixed it).
    var segmentsTick by remember(markdown, attempt) { mutableIntStateOf(0) }
    LaunchedEffect(markdown, attempt) {
        delay(RENDER_LANDING_TIMEOUT_MS)
        if (landed[0]) return@LaunchedEffect
        if (attempt >= MAX_RENDER_RETRIES) {
            if (fallbackPlain != null) {
                Log.w(RENDER_TAG, "markdown view not rendered after ${MAX_RENDER_RETRIES} retries: shown as plain text")
                gaveUp = true
            }
        } else {
            Log.w(RENDER_TAG, "markdown view not rendered after ${RENDER_LANDING_TIMEOUT_MS} ms (attempt ${attempt + 1}): recreated")
            attempt++
        }
    }
    if (gaveUp && fallbackPlain != null) {
        Text(
            fallbackPlain,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = CARD_PLAIN_LINES,
            overflow = TextOverflow.Ellipsis,
            modifier = modifier,
        )
        return
    }

    key(attempt) {
    AndroidView(
        factory = { ctx ->
            // Settings that never change (prepareForViewReuse puts the first three back to these same values), except the
            // trailing margin, set again in `update`.
            EnrichedMarkdown(ctx).apply {
                setOnHierarchyChangeListener(object : ViewGroup.OnHierarchyChangeListener {
                    override fun onChildViewAdded(parent: View?, child: View?) { segmentsTick++ }
                    override fun onChildViewRemoved(parent: View?, child: View?) { segmentsTick++ }
                })
                setEnableTaskListItemToggle(true)
                setSpoilerOverlay(SpoilerOverlay.Particles())
                setAllowTrailingMargin(allowTrailingMargin)
                setOptions(MarkdownEngineConfig.options)
                setCodeBlockLayout(CodeBlockLayout.Wrapping)
                // The content goes in here too, like the library's own wrapper: `update` only runs once the view is attached to
                // the window, i.e. after the first measure, so rendering started there would begin a frame later and leave the
                // card at zero height longer (a visible size jump). Style and selectability first: the render uses them.
                setOnLinkPressCallback(onLinkPress)
                setPlugins(plugins)
                setMarkdownStyle(styleConfig)
                setIsSelectable(selectable)
                setMarkdownContent(markdown)
            }
        },
        modifier = modifier
            .layout { measurable, constraints ->
                segmentsTick // read: a change measures this node again
                val placeable = measurable.measure(constraints)
                layout(placeable.width, placeable.height) { placeable.place(0, 0) }
            }
            .reserveRenderedHeight(markdown, variant = if (allowTrailingMargin) 1 else 0) { landed[0] = true },
        // Runs after attach and after every reuse (prepareForViewReuse clears the content). Each setter ignores an unchanged
        // value, so right after the factory it does nothing and a recomposition re-renders nothing.
        update = { view ->
            view.setOnLinkPressCallback(onLinkPress) // cleared by prepareForViewReuse
            view.setPlugins(plugins) // cleared by prepareForViewReuse
            view.setAllowTrailingMargin(allowTrailingMargin) // a no-op when unchanged
            view.setMarkdownStyle(styleConfig)
            view.setIsSelectable(selectable)
            view.setMarkdownContent(markdown)
        },
        onReset = { view -> view.prepareForViewReuse() },
        onRelease = { view -> view.prepareForViewReuse() },
    )
    }
}

/**
 * While the view has no height yet (its rendering has not landed), gives it the height [markdown] was last laid out at in this
 * width ([RenderedHeights]); once it has one, remembers it. The layout around it then does not jump when the text was already
 * rendered once: a card that comes back into the feed, the feed after a detail, the detail of a text its card showed whole.
 */
private fun Modifier.reserveRenderedHeight(markdown: String, variant: Int, onRendered: () -> Unit): Modifier = layout { measurable, constraints ->
    val placeable = measurable.measure(constraints)
    val height = if (placeable.height > 0) {
        onRendered()
        RenderedHeights.put(markdown, placeable.width, density, fontScale, variant, placeable.height)
        placeable.height
    } else {
        (RenderedHeights.get(markdown, placeable.width, density, fontScale, variant) ?: 0).coerceAtMost(constraints.maxHeight)
    }
    layout(placeable.width, height) { placeable.place(0, 0) }
}

private const val CARD_PLAIN_LINES = 4

/** The only plugin: fenced code coloured by tree-sitter. One list for every view (the view ignores an unchanged list). */
private val MARKDOWN_PLUGINS: List<MarkdownPlugin> = listOf(TomiCodeHighlightPlugin)

/** A document up to this length is cut into chunks at once; a longer one off the main thread ([rememberOffMain]). */
private const val SYNC_CHUNKING_CHARS = 8_000

/**
 * Lays [content] out at its natural height, shows at most [maxHeight] of it (cut at the top edge) and tells [onClipped] whether the
 * rest is hidden. Measuring the natural height here, instead of trusting the view to report one under a height limit, keeps
 * "Lire la suite" correct whatever the native view does with an AT_MOST measure spec.
 *
 * [truncated] (the content is a cut preview, far taller than [maxHeight]): the box is exactly [maxHeight] high from the first
 * frame and the content is cut without being measured, so the card does not jump from 0 to its height when the asynchronous
 * native rendering lands.
 */
@Composable
private fun HeightLimited(
    maxHeight: Dp?,
    truncated: Boolean,
    onClipped: ((Boolean) -> Unit)?,
    modifier: Modifier,
    content: @Composable () -> Unit,
) {
    if (maxHeight == null) {
        Box(modifier) { content() }
        return
    }
    val currentOnClipped by rememberUpdatedState(onClipped)
    if (truncated) {
        SideEffect { currentOnClipped?.invoke(true) }
        Box(modifier.height(maxHeight).clipToBounds()) {
            Box(Modifier.fillMaxWidth().wrapContentHeight(align = Alignment.Top, unbounded = true)) { content() }
        }
        return
    }
    val maxPx = with(LocalDensity.current) { maxHeight.roundToPx() }
    Box(modifier.heightIn(max = maxHeight).clipToBounds()) {
        Box(
            Modifier
                .fillMaxWidth()
                .wrapContentHeight(align = Alignment.Top, unbounded = true)
                .onSizeChanged { size -> currentOnClipped?.invoke(ClipDetection.isClipped(size.height, maxPx)) },
        ) { content() }
    }
}

/** Markdown style of Tomi: Material colours of the app, Inter text, JetBrains Mono code, and the charter palette for code colours. */
@Composable
private fun rememberTomiMarkdownStyle(): MarkdownStyle {
    val scheme = MaterialTheme.colorScheme
    val dark = scheme.surface.luminance() < 0.5f
    return remember(scheme, dark) { tomiMarkdownStyle(scheme, CodePalette.of(dark)) }
}

private fun tomiMarkdownStyle(scheme: ColorScheme, palette: CodePalette): MarkdownStyle =
    materialMarkdownStyle(scheme).merge {
        paragraph {
            fontFamily = TomiFonts.ui
            fontSize = 15.sp
            lineHeight = 22.sp
            marginBottom = 8.dp
        }
        h1 {
            fontFamily = TomiFonts.ui
            fontSize = 20.sp
            lineHeight = 26.sp
            fontWeight = FontWeight.SemiBold
            marginBottom = 8.dp
        }
        h2 {
            fontFamily = TomiFonts.ui
            fontSize = 18.sp
            lineHeight = 24.sp
            fontWeight = FontWeight.SemiBold
            marginBottom = 8.dp
        }
        h3 {
            fontFamily = TomiFonts.ui
            fontSize = 16.sp
            lineHeight = 22.sp
            fontWeight = FontWeight.SemiBold
            marginBottom = 6.dp
        }
        h4 {
            fontFamily = TomiFonts.ui
            fontSize = 15.sp
            lineHeight = 22.sp
            fontWeight = FontWeight.SemiBold
        }
        h5 {
            fontFamily = TomiFonts.ui
            fontSize = 15.sp
            lineHeight = 22.sp
            fontWeight = FontWeight.SemiBold
        }
        h6 {
            fontFamily = TomiFonts.ui
            fontSize = 15.sp
            lineHeight = 22.sp
            fontWeight = FontWeight.SemiBold
        }
        code {
            fontFamily = TomiFonts.mono
            fontSize = 13.sp
        }
        codeBlock {
            fontFamily = TomiFonts.mono
            fontSize = 13.sp
            lineHeight = 19.sp
            color = Color(palette.text)
            backgroundColor = Color(palette.background)
            borderColor = scheme.outlineVariant
            borderWidth = 1.dp
            cornerRadius = 8.dp
            padding = 12.dp
            marginTop = 4.dp
            marginBottom = 8.dp
        }
        codeHighlight {
            keyword = Color(palette.keyword)
            operator = Color(palette.operator)
            punctuation = Color(palette.punctuation)
            string = Color(palette.string)
            number = Color(palette.number)
            constant = Color(palette.constant)
            comment = Color(palette.comment)
            function = Color(palette.function)
            type = Color(palette.type)
            variable = Color(palette.variable)
            property = Color(palette.property)
            tag = Color(palette.tag)
            attribute = Color(palette.attribute)
            embedded = Color(palette.embedded)
        }
    }

/**
 * Hardened WebView for received HTML: JavaScript off, network blocked, no file/content access, no navigation.
 * The content is sanitised by [HtmlSanitizer] and carries the CSP `default-src 'none'; img-src data:; style-src 'unsafe-inline'`.
 */
@Composable
fun SandboxedHtmlView(
    html: String,
    textColor: Color,
    linkColor: Color,
    maxHeight: Dp = 280.dp,
    modifier: Modifier = Modifier,
    onClipped: ((Boolean) -> Unit)? = null,
    /** False in a feed card: the page does not scroll sideways, so a horizontal drag on it swipes the card instead. */
    horizontalScroll: Boolean = true,
) {
    val document = remember(html, textColor, linkColor, horizontalScroll) {
        HtmlSanitizer.toDocument(html, textColor.toCss(), linkColor.toCss(), horizontalScroll)
    }
    val currentOnClipped by rememberUpdatedState(onClipped)
    AndroidView(
        modifier = modifier.heightIn(max = maxHeight),
        factory = { context ->
            createSandboxedWebView(context) { web -> currentOnClipped?.invoke(isHtmlClipped(web)) }.apply {
                addOnLayoutChangeListener { v, _, _, _, _, _, _, _, _ -> currentOnClipped?.invoke(isHtmlClipped(v as WebView)) }
            }
        },
        update = { view ->
            if (view.tag != document) {
                view.tag = document
                view.loadDataWithBaseURL(null, document, "text/html", "utf-8", null)
            }
        },
        onRelease = { it.destroy() },
    )
}

/** The page is taller than the (height-limited) WebView. `contentHeight` is in CSS px, hence the density factor. */
private fun isHtmlClipped(view: WebView): Boolean {
    val contentPx = Math.ceil(view.contentHeight * view.resources.displayMetrics.density.toDouble()).toInt()
    return ClipDetection.isClipped(contentPx, view.height)
}

@SuppressLint("SetJavaScriptEnabled")
private fun createSandboxedWebView(context: Context, onPageSettled: (WebView) -> Unit): WebView = WebView(context).apply {
    layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
    setBackgroundColor(android.graphics.Color.TRANSPARENT)
    isLongClickable = false
    with(settings) {
        javaScriptEnabled = false
        allowFileAccess = false
        allowContentAccess = false
        blockNetworkLoads = true
        blockNetworkImage = true
        domStorageEnabled = false
        javaScriptCanOpenWindowsAutomatically = false
        setSupportMultipleWindows(false)
        setGeolocationEnabled(false)
        mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        cacheMode = WebSettings.LOAD_NO_CACHE
    }
    webViewClient = object : WebViewClient() {
        // The content height is only known a moment after the page is loaded; the layout listener covers the rest.
        override fun onPageFinished(view: WebView, url: String?) {
            view.postDelayed({ onPageSettled(view) }, PAGE_SETTLE_DELAY_MS)
        }

        // The document never navigates. A tap on a link opens the system browser (http/https/mailto/tel only).
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
            if (request.hasGesture()) openExternalLink(view.context, request.url)
            return true
        }

        // Belt and braces on top of blockNetworkLoads and the CSP: answer every http(s) request with nothing.
        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? =
            when (request.url.scheme?.lowercase()) {
                "http", "https", "file", "content", "ftp" -> WebResourceResponse("text/plain", "utf-8", ByteArrayInputStream(ByteArray(0)))
                else -> null
            }
    }
}

private fun Color.toCss(): String {
    val argb = toArgb()
    return String.format(java.util.Locale.ROOT, "#%06X", argb and 0xFFFFFF)
}
