package eu.shaffe.tomi.share

/** Cleans untrusted text coming from intents and deep links. Pure Kotlin so it can be unit-tested on the JVM. */
object TextSanitizer {

    /**
     * Removes control characters (except `\n` and `\t`) and bidirectional override characters,
     * normalises line endings, and trims the result. Returns null if nothing is left.
     */
    fun clean(raw: CharSequence?, maxLength: Int = Int.MAX_VALUE): String? {
        if (raw == null) return null
        val normalised = raw.toString().replace("\r\n", "\n").replace('\r', '\n')
        val sb = StringBuilder(minOf(normalised.length, 1024))
        for (ch in normalised) {
            if (isDropped(ch)) continue
            sb.append(ch)
        }
        val out = sb.toString().trim()
        if (out.isEmpty()) return null
        return if (out.length > maxLength) out.substring(0, maxLength) else out
    }

    private fun isDropped(ch: Char): Boolean {
        if (ch == '\n' || ch == '\t') return false
        if (ch.isISOControl()) return true
        // Bidi embedding/override/isolate characters can be used to spoof what a link or title says.
        return ch in '\u202A'..'\u202E' || ch in '\u2066'..'\u2069'
    }
}
