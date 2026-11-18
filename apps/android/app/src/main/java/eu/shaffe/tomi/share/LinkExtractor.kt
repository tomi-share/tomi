package eu.shaffe.tomi.share

import java.net.URI

/** Result of splitting a shared string such as `Page title https://example.org/a`. */
data class LinkParts(val url: String, val leftover: String?)

object LinkExtractor {

    private val URL_REGEX = Regex("https?://[^\\s<>\"']+", RegexOption.IGNORE_CASE)
    private const val TRAILING_PUNCTUATION = ".,;:!?)]}>\u2019\u201D"
    private const val SEPARATORS = " \t\n-\u2013\u2014:|\u00B7\u2022"
    private const val MAX_LEFTOVER_CHARS = 300
    private const val MAX_LEFTOVER_LINES = 3
    const val MAX_URL_LENGTH = 2048

    /** True if [value] is a single http(s) URL with a host and nothing else. */
    fun isSingleUrl(value: String): Boolean {
        val v = value.trim()
        if (v.isEmpty() || v.length > MAX_URL_LENGTH || v.any { it.isWhitespace() }) return false
        return isValidHttpUrl(v)
    }

    /** Validates scheme (http/https only) and host. */
    fun isValidHttpUrl(value: String): Boolean {
        if (value.length > MAX_URL_LENGTH) return false
        return try {
            val uri = URI(value)
            (uri.scheme.equals("http", true) || uri.scheme.equals("https", true)) && !uri.host.isNullOrEmpty()
        } catch (_: Exception) {
            false
        }
    }

    /**
     * Finds exactly one URL located at the very start or very end of [text] (the shape produced by Chrome and
     * most apps: "Title URL"). A URL buried in the middle of a sentence, several URLs, or too much surrounding
     * text means the string is plain text and null is returned.
     */
    fun extract(text: String): LinkParts? {
        val matches = URL_REGEX.findAll(text).toList()
        if (matches.size != 1) return null
        val match = matches[0]
        val url = trimTrailing(match.value)
        if (url.isEmpty() || !isValidHttpUrl(url)) return null

        val before = text.substring(0, match.range.first)
        // Punctuation stripped from the end of the URL ("see https://x.org.") is not "text after the URL".
        val after = text.substring(match.range.last + 1)
        if (before.isNotBlank() && after.isNotBlank()) return null

        val leftover = (before + after).trim().trim { it in SEPARATORS }
        if (leftover.length > MAX_LEFTOVER_CHARS || leftover.lines().size > MAX_LEFTOVER_LINES) return null
        return LinkParts(url, leftover.ifEmpty { null })
    }

    private fun trimTrailing(url: String): String {
        var end = url.length
        while (end > 0 && url[end - 1] in TRAILING_PUNCTUATION) {
            // Keep a closing parenthesis that balances an opening one inside the URL (Wikipedia style links).
            if (url[end - 1] == ')' && url.substring(0, end).count { it == '(' } >= url.substring(0, end).count { it == ')' }) break
            end--
        }
        return url.substring(0, end)
    }
}
