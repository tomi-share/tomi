package eu.shaffe.tomi.deeplink

import eu.shaffe.tomi.core.CoreLinkReader
import eu.shaffe.tomi.core.LinkReader
import eu.shaffe.tomi.core.SensitiveText
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.share.TextSanitizer
import java.net.URI
import java.net.URLDecoder

/**
 * Commands that can arrive through `tomi-share://` or the `https://<relay>/l/pair`, `https://<relay>/create` App Links (any relay: the address is
 * read from the link, see [LinkReader]).
 *
 * Any web page can trigger these URLs, so by design none of them performs an action: they only pre-fill the
 * compose field, open a screen, or hand an opaque pairing payload to a screen that still waits for the user.
 */
sealed interface DeepLink {
    data object Open : DeepLink

    /**
     * Pre-fills the compose field. [droppedParameters] is true if a parameter was ignored
     * (too long, invalid, not http(s)) so the UI can tell the user.
     */
    data class Send(
        val title: String?,
        val url: String?,
        val text: String?,
        val droppedParameters: Boolean,
    ) : DeepLink {
        override fun toString(): String = "Send(droppedParameters=$droppedParameters, <redacted>)"
    }

    data class Item(val id: String) : DeepLink

    /**
     * `https://<relay>/l/pair#<code>` (App Link, QR code) or `tomi-share://pair#<code>`: opens the pairing screen with the payload; never joins by
     * itself, the user presses "Rejoindre" (spec/pairing-v1.md 4.4). The payload is rebuilt from what the core read: the https form on
     * [relayUrl], or `tomi-share://pair#<code>` when the link names no relay (the device then uses its own).
     *
     * [relayUrl] (`https://host[:port]`) is NOT trusted, whoever showed the link chose it: the screen shows it, and the verification screen
     * shows it again in full.
     */
    data class Pair(val payload: String, val relayUrl: String? = null) : DeepLink {
        /** The payload is a pairing secret: never printed. */
        override fun toString(): String = "Pair(<redacted>)"
    }

    /**
     * `https://<relay>/create#<token>` or `/create/<token>`: opens the "create your space" screen with the relay and the installation token
     * filled in. Never creates anything by itself, the user presses "Créer mon espace". Same distrust of [relayUrl] as [Pair].
     */
    data class Setup(val relayUrl: String, val token: SensitiveText) : DeepLink {
        /** The token is a secret: never printed. */
        override fun toString(): String = "Setup(<redacted>)"
    }
}

object DeepLinkParser {
    const val MAX_URI_LENGTH = 8 * 1024
    const val MAX_URL_PARAM = LinkExtractor.MAX_URL_LENGTH
    const val MAX_TITLE_PARAM = 300
    const val MAX_TEXT_PARAM = 4 * 1024
    const val MAX_PAIR_PAYLOAD = 512

    private val ID_REGEX = Regex("^[A-Za-z0-9_-]{1,64}$")

    /**
     * Returns null for anything that is not a recognised, well-formed Tomi link. The pairing and creation links are read by the core
     * ([links]): the relay comes from the link itself, there is no relay compiled into the parser.
     */
    fun parse(raw: String?, links: LinkReader = CoreLinkReader): DeepLink? {
        if (raw.isNullOrBlank() || raw.length > MAX_URI_LENGTH) return null
        val text = raw.trim()
        val uri = try {
            URI(text)
        } catch (_: Exception) {
            return null
        }
        return when (uri.scheme?.lowercase()) {
            "tomi-share" -> parseCustomScheme(uri, text, links)
            "https" -> parseAppLink(text, links)
            else -> null
        }
    }

    private fun parseCustomScheme(uri: URI, text: String, links: LinkReader): DeepLink? {
        val host = uri.host?.lowercase() ?: uri.authority?.lowercase()
        return when (host) {
            "open" -> DeepLink.Open
            "send" -> parseSend(uri.rawQuery)
            "item" -> {
                val id = uri.path?.removePrefix("/")?.takeIf { ID_REGEX.matches(it) } ?: return null
                DeepLink.Item(id)
            }
            // tomi-share://pair#<code>: the code is in the fragment, never in the query (a query reaches logs and servers).
            "pair" -> pairLink(text, links)
            else -> null
        }
    }

    /** `https://<any relay>/l/pair#<code>` and `/create`: the only two paths of an App Link. Anything else is not for this app. */
    private fun parseAppLink(text: String, links: LinkReader): DeepLink? =
        pairLink(text, links) ?: links.parseSetup(text)?.let { DeepLink.Setup(it.relayUrl, it.token) }

    /**
     * The pairing payload is rebuilt from what the core read (relay and canonical code), never taken as written: no user information,
     * query or other fragment survives.
     */
    private fun pairLink(text: String, links: LinkReader): DeepLink.Pair? {
        val link = links.parsePair(text) ?: return null
        val code = link.code.reveal()
        val payload = if (link.relayUrl != null) "${link.relayUrl}/l/pair#$code" else "tomi-share://pair#$code"
        return if (payload.length <= MAX_PAIR_PAYLOAD) DeepLink.Pair(payload, link.relayUrl) else null
    }

    private fun parseSend(rawQuery: String?): DeepLink {
        val params = parseQuery(rawQuery)
        var dropped = false

        fun bounded(name: String, max: Int): String? {
            val value = params[name] ?: return null
            if (value.length > max) {
                dropped = true
                return null
            }
            return TextSanitizer.clean(value, max)
        }

        var url = bounded("url", MAX_URL_PARAM)
        if (url != null && !LinkExtractor.isSingleUrl(url)) {
            dropped = true
            url = null
        }
        val title = bounded("title", MAX_TITLE_PARAM)
        val text = bounded("text", MAX_TEXT_PARAM)
        // Note: there is intentionally no "secret" parameter. Secrets are pasted by hand, never put in a URL.
        return DeepLink.Send(title = title, url = url, text = text, droppedParameters = dropped)
    }

    /** Decodes the query; unknown parameters are kept in the map but ignored by callers. First occurrence wins. */
    internal fun parseQuery(rawQuery: String?): Map<String, String> {
        if (rawQuery.isNullOrEmpty()) return emptyMap()
        val result = LinkedHashMap<String, String>()
        for (pair in rawQuery.split('&')) {
            if (pair.isEmpty()) continue
            val key = decode(pair.substringBefore('=')) ?: continue
            val value = decode(pair.substringAfter('=', "")) ?: continue
            if (key !in result && result.size < MAX_PARAMS) result[key] = value
        }
        return result
    }

    private fun decode(value: String): String? = try {
        URLDecoder.decode(value, Charsets.UTF_8)
    } catch (_: IllegalArgumentException) {
        null
    }

    private const val MAX_PARAMS = 16
}
