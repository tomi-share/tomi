package eu.shaffe.tomi.ui.render

/**
 * Runs once, on a background thread at start-up, the text code that the first card and the first detail would otherwise run
 * for the first time on the main thread: a first call loads the classes, compiles the regular expressions and loads the native
 * Markdown parser, which costs far more than every call after it. Nothing is kept; a failure is ignored (the screens then pay
 * that first call themselves, as they would without this).
 */
internal fun warmUpTextRendering() {
    try {
        val sample = "# t\n\ntexte [l][r] `c`\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```kotlin\nval x = 1\n```\n\n" +
            "1. un\n2. deux\n\n- [ ] t\n\n> q\n\n[r]: https://relay.example.org\n"
        val tight = Budget(maxChars = 40, maxLines = 4, maxTableRows = 1)
        preview(sample, tight)
        head(sample, 2, 20)
        chunks(sample, tight)
        sourceChunkRanges(sample, 2, 20)
        MarkdownGuard.safeMarkdown("![i](x) $sample", ::parseWithLibrary)
    } catch (_: Throwable) {
        // Best effort.
    }
}
