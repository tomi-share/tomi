package eu.shaffe.tomi.share

/** Detects Markdown syntax; the result is only a display hint (`source_format`), never a restriction. */
object MarkdownDetector {
    private val PATTERNS = listOf(
        Regex("(?m)^#{1,6}\\s+\\S"),
        Regex("(?m)^\\s*[-*+]\\s+\\S"),
        Regex("(?m)^\\s*\\d+\\.\\s+\\S"),
        Regex("(?m)^```"),
        Regex("\\[[^\\]\\n]+]\\([^)\\s]+\\)"),
        Regex("\\*\\*[^*\\n]+\\*\\*"),
        Regex("(?m)^>\\s+\\S"),
    )

    fun looksLikeMarkdown(text: String): Boolean = PATTERNS.any { it.containsMatchIn(text) }
}

/**
 * Decides whether an HTML flavour (clipboard or share) tells anything the plain text does not. Same rules as
 * `detect.rs` (Windows) and `HtmlFlavour` (Apple): keep the three aligned.
 *
 * Browsers and editors put an HTML flavour on every copy, even a plain selection (`<meta><span>text</span>`), and copy raw
 * Markdown as one `<div>` per line. Neither is a rendering: only real formatting tags are.
 */
object HtmlFlavour {
    enum class Richness {
        /** Nothing but wrappers. */
        Plain,

        /** Only lines and paragraphs (`div`, `p`, `br`, at least three of them), no formatting tag. */
        Layout,

        /** Real formatting tags: links, emphasis, headings, lists, tables, code... */
        Formatted,
    }

    private val FORMATTING_TAGS = setOf(
        "a", "b", "strong", "i", "em", "u", "ul", "ol", "li", "h1", "h2", "h3", "h4", "h5", "h6",
        "table", "pre", "code", "blockquote", "img", "hr",
    )
    private val LAYOUT_TAGS = setOf("p", "div", "br")
    private val TAG_NAME = Regex("<([a-z0-9]+)")
    private const val LAYOUT_MIN_TAGS = 3

    fun richness(html: String): Richness {
        var layout = 0
        for (match in TAG_NAME.findAll(html.lowercase(java.util.Locale.ROOT))) {
            val name = match.groupValues[1]
            if (name in FORMATTING_TAGS) return Richness.Formatted
            if (name in LAYOUT_TAGS) layout++
        }
        return if (layout >= LAYOUT_MIN_TAGS) Richness.Layout else Richness.Plain
    }

    /**
     * True when [html] is worth sending next to [plain]. Wrappers are not. Lines of HTML are not either when the plain text is
     * Markdown (the receivers would lay the raw `# Title` / `- item` out as HTML instead of rendering them).
     * [plain] is null when the HTML is all there is.
     */
    fun isWorthKeeping(html: String, plain: String?): Boolean = when (richness(html)) {
        Richness.Formatted -> true
        Richness.Layout -> plain == null || !MarkdownDetector.looksLikeMarkdown(plain)
        Richness.Plain -> false
    }
}

/** Minimal HTML to plain text conversion used only when an app shares HTML without a plain text version. */
object HtmlText {
    private val BLOCK_BREAKS = Regex("(?i)<(br\\s*/?|/p|/div|/h[1-6]|/li|/tr)>")
    private val SCRIPT_STYLE = Regex("(?is)<(script|style)\\b.*?</\\1>")
    private val TAGS = Regex("<[^>]*>")

    fun toPlain(html: String): String {
        val noScripts = SCRIPT_STYLE.replace(html, "")
        val withBreaks = BLOCK_BREAKS.replace(noScripts, "\n")
        return TAGS.replace(withBreaks, "")
            .replace("&nbsp;", " ")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&amp;", "&")
            .lines().joinToString("\n") { it.trim() }
            .replace(Regex("\n{3,}"), "\n\n")
            .trim()
    }
}

/** Tells whether a `content://` authority is served by Tomi itself. */
fun interface OwnAuthorityCheck {
    /** [authority] is the authority of the URI as returned by `Uri.getAuthority()`, possibly with a `userId@` prefix. */
    fun isOwn(authority: String): Boolean
}

/**
 * Asks the system who owns a content authority, instead of comparing strings: `owningPackageOf` is
 * `{ packageManager.resolveContentProvider(it, 0)?.packageName }` in production.
 *
 * The authority is tried as given and in lower case, because a provider lookup is exact and the caller does not
 * control the case used by the sender; the part before the last `@` (Android's `userId@authority` form) is ignored.
 * If the lookup itself fails, the URI is treated as ours: refusing a file is better than leaking one.
 */
class PackageManagerOwnAuthorityCheck(
    private val ownPackage: String,
    private val owningPackageOf: (authority: String) -> String?,
) : OwnAuthorityCheck {
    override fun isOwn(authority: String): Boolean {
        val bare = authority.substringAfterLast('@')
        return setOf(bare, bare.lowercase(java.util.Locale.ROOT)).any { candidate ->
            try {
                owningPackageOf(candidate) == ownPackage
            } catch (_: RuntimeException) {
                true
            }
        }
    }
}

/** Decides which content:// URIs received through an intent we agree to read. */
object StreamPolicy {
    /**
     * Only `content://` URIs are accepted. `file://` URIs (which could point at this app's private files)
     * and content URIs served by this very app are refused, so another app cannot make Tomi send its own data.
     * [scheme] and [authority] are what `Uri.getScheme()` / `Uri.getAuthority()` return.
     */
    fun isAcceptable(scheme: String?, authority: String?, own: OwnAuthorityCheck): Boolean {
        if (!"content".equals(scheme, ignoreCase = true)) return false
        if (authority.isNullOrEmpty() || authority.substringAfterLast('@').isEmpty()) return false
        return !own.isOwn(authority)
    }

    /**
     * Same decision for a URI kept as a string. It is parsed like `android.net.Uri` does (the authority is
     * percent-decoded), so `content://eu.shaffe%2Etomi.provider/x` cannot slip through.
     */
    fun isAcceptableUri(uri: String, own: OwnAuthorityCheck): Boolean {
        val parsed = try {
            java.net.URI(uri)
        } catch (_: java.net.URISyntaxException) {
            return false
        }
        return isAcceptable(parsed.scheme, parsed.authority, own)
    }

    const val MAX_FILE_BYTES: Long = 25L * 1024 * 1024
    const val MAX_ITEM_BYTES: Long = 100L * 1024 * 1024
    const val MAX_STREAMS: Int = 20
}
