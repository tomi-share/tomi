package eu.shaffe.tomi.ui.code

/**
 * Pure logic of the tree-sitter grammars that Enriched Markdown (code-highlight 0.3.0) does not bundle: PHP. It is
 * compiled in our own native library (`libtomi_highlight.so`, sources in `src/main/cpp`) and plugged into the library's code
 * block decorator through its public `SyntaxTokenSource` interface (see `TomiSyntaxTokenSource`). Not Android, so unit tested.
 */
object ExtraGrammars {
    const val PHP = "php"

    /** Same limits as Enriched Markdown: a block over 50 KB of UTF-8 or 2,000 lines is not parsed (stays uncoloured). */
    const val MAX_BYTES = 50 * 1024
    const val MAX_LINES = 2000

    /**
     * The extra grammar for the info string of a code fence (`php`, `PHP`, `phtml`, `php title="x"`), or null when it is not
     * one of ours (the library's own grammars then apply). Case-insensitive; only the first word counts.
     */
    fun forInfoString(info: String?): String? {
        val word = info?.trim()?.lowercase()?.split(' ', '\t', '{', ',')?.firstOrNull().orEmpty()
        return when (word) {
            "php", "phtml" -> PHP
            else -> null
        }
    }

    /** Whether [code] may be parsed: not empty, not over [MAX_BYTES] nor [MAX_LINES] lines. */
    fun withinLimits(code: ByteArray): Boolean {
        if (code.isEmpty() || code.size > MAX_BYTES) return false
        var lines = 1
        for (b in code) if (b == '\n'.code.toByte()) lines++
        return lines <= MAX_LINES
    }
}
