package eu.shaffe.tomi.core

/**
 * A [LinkReader] for the unit tests that follows the rules of crates/core/src/relay_url.rs (the real one is checked against the host build
 * of the core in RealCoreHostIntegrationTest): https only, host[:port] without user information, the paths `/l/pair` and `/create`, a
 * code like `4821-crayon-renard`, a token of 24 to 40 characters of the relay's base32 alphabet.
 */
internal object TestLinkReader : LinkReader {
    private val authority = Regex("^[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?(:[0-9]{1,5})?$")
    private val code = Regex("^[0-9]{1,4}-[a-z]{3,}-[a-z]{3,}$")
    private const val ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz"

    private fun relayOf(rest: String): String? {
        val auth = rest.substringBefore('/').substringBefore('#')
        if (!authority.matches(auth)) return null
        return "https://" + auth.lowercase()
    }

    override fun parsePair(text: String): PairLink? {
        val t = text.trim()
        if (t.startsWith("tomi-share://pair#")) return codeOf(t.substringAfter('#'))?.let { PairLink(null, SensitiveText(it)) }
        if (!t.startsWith("https://")) return null
        val rest = t.removePrefix("https://")
        val relay = relayOf(rest) ?: return null
        val path = rest.removePrefix(rest.substringBefore('/')).substringBefore('#')
        if (path != "/l/pair" || !rest.contains('#')) return null
        return codeOf(rest.substringAfter('#'))?.let { PairLink(relay, SensitiveText(it)) }
    }

    private fun codeOf(raw: String): String? = raw.lowercase().replace(' ', '-').takeIf { code.matches(it) }

    override fun parseSetup(text: String): SetupLink? {
        val t = text.trim()
        if (!t.startsWith("https://")) return null
        val rest = t.removePrefix("https://")
        val relay = relayOf(rest) ?: return null
        val tail = rest.removePrefix(rest.substringBefore('/').substringBefore('#'))
        val token = when {
            tail.startsWith("/create#") -> tail.removePrefix("/create#")
            tail.startsWith("/create/") -> tail.removePrefix("/create/")
            else -> return null
        }
        return if (isInstallTokenShape(token)) SetupLink(relay, SensitiveText(token)) else null
    }

    override fun isInstallTokenShape(text: String): Boolean {
        val n = text.trim().filterNot { it == '-' || it.isWhitespace() }.lowercase()
        return n.length in 24..40 && n.all { it in ALPHABET }
    }
}
