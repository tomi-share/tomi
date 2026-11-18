package eu.shaffe.tomi.core

/**
 * The name this device announces to the others when it joins a space (spec/pairing-v1.md 6.4: 1 to 64 characters, no blank at either
 * end, no control character). The user can rename the device afterwards, from the list of devices.
 */
object DeviceNames {
    const val FALLBACK = "Android"
    private const val MAX_CODE_POINTS = 64

    /**
     * [raw] with control characters and runs of blanks turned into one space, trimmed, and cut at 64 characters (never in the middle of a
     * surrogate pair). Null when nothing is left.
     */
    fun clean(raw: String?): String? {
        if (raw == null) return null
        val spaced = buildString {
            var lastWasBlank = false
            raw.forEach { c ->
                val blank = c.isWhitespace() || c.isISOControl()
                if (blank) {
                    if (!lastWasBlank) append(' ')
                } else {
                    append(c)
                }
                lastWasBlank = blank
            }
        }.trim()
        if (spaced.isEmpty()) return null
        val cut = if (spaced.codePointCount(0, spaced.length) > MAX_CODE_POINTS) {
            spaced.substring(0, spaced.offsetByCodePoints(0, MAX_CODE_POINTS)).trimEnd()
        } else {
            spaced
        }
        return cut.ifEmpty { null }
    }

    /** The first of [candidates] that the core accepts once cleaned ([isValid] is the rule of the core), [FALLBACK] otherwise. */
    fun choose(candidates: List<String?>, isValid: (String) -> Boolean): String =
        candidates.asSequence().mapNotNull(::clean).firstOrNull(isValid) ?: FALLBACK
}
