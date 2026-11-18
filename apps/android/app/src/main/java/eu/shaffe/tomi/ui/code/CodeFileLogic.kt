package eu.shaffe.tomi.ui.code

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.slotOf

/*
 * Pure logic of the code file screen (no Android types, unit tested): which attachments are code files, which tree-sitter
 * grammar colours them, what is allowed on screen, how a file is decoded, the in-file search and the per-line colour runs.
 */

/**
 * File extensions → tree-sitter grammar of Enriched Markdown (code-highlight 0.3.0 bundles 14 grammars: bash, c, css, go, html,
 * java, javascript, json, markdown, python, rust, tsx, typescript, yaml) plus the one that our own native library adds (php:
 * see ExtraGrammars). A code file without a grammar (kt, kts, rb, cpp, sql, toml, ...) still opens in the dedicated screen, in
 * plain monospace text, with the search.
 */
object CodeLanguages {
    private val grammarByExtension: Map<String, String> = mapOf(
        "rs" to "rust",
        "js" to "javascript", "mjs" to "javascript", "cjs" to "javascript", "jsx" to "javascript",
        "ts" to "typescript", "mts" to "typescript", "cts" to "typescript",
        "tsx" to "tsx",
        "css" to "css",
        "html" to "html", "htm" to "html",
        "json" to "json",
        "py" to "python",
        "java" to "java",
        "sh" to "bash", "bash" to "bash", "zsh" to "bash",
        "go" to "go",
        "c" to "c", "h" to "c",
        "yaml" to "yaml", "yml" to "yaml",
        "md" to "markdown", "markdown" to "markdown",
        // Not in Enriched Markdown: coloured by libtomi_highlight (ExtraGrammars).
        "php" to ExtraGrammars.PHP, "phtml" to ExtraGrammars.PHP,
    )

    /** Code extensions the dedicated screen opens even though no grammar colours them. */
    private val uncoloured: Set<String> = setOf(
        "kt", "kts", "rb", "cpp", "cc", "cxx", "hpp", "hh", "sql", "toml", "gradle", "swift", "cs", "lua", "dart", "scala",
    )

    /** Lower-case extension of a file name, without the dot; null when it has none (`Makefile`, `.env` counts as none). */
    fun extensionOf(fileName: String): String? {
        val name = fileName.substringAfterLast('/').substringAfterLast('\\')
        val dot = name.lastIndexOf('.')
        if (dot <= 0 || dot == name.length - 1) return null
        return name.substring(dot + 1).lowercase()
    }

    fun isCodeFile(fileName: String): Boolean = extensionOf(fileName)?.let { it in grammarByExtension || it in uncoloured } == true

    /** The grammar (fence language of the library) for [fileName], or null when there is none: the file is shown without colours. */
    fun grammarFor(fileName: String): String? = extensionOf(fileName)?.let(grammarByExtension::get)
}

/** One code file of an item: its attachment and the SLOT of that attachment (what the core is asked for, stable when others are removed). */
class CodeFile(val slot: Int, val attachment: Attachment) {
    val grammar: String? get() = CodeLanguages.grammarFor(attachment.name)
}

object CodeFiles {
    /** An image is never a code file, whatever its name. */
    fun isCodeFile(attachment: Attachment): Boolean = !attachment.isImage && CodeLanguages.isCodeFile(attachment.name)

    /**
     * The code files of [item], in order. A mixed item (images + code + pdf) only pages through its code files; each keeps the
     * slot of its attachment, because `TomiCore.attachmentUri(itemId, slot)` speaks in slots (not positions, once one was removed).
     */
    fun of(item: TomiItem): List<CodeFile> =
        item.attachments.mapIndexedNotNull { index, attachment -> if (isCodeFile(attachment)) CodeFile(item.attachments.slotOf(index), attachment) else null }

    /** Position of the file showing the attachment of [slot]; the first one when it is not a code file of the item. */
    fun positionFor(files: List<CodeFile>, slot: Int): Int =
        files.indexOfFirst { it.slot == slot }.coerceAtLeast(0)

    /** Slot of the file at [position] (coerced into range), or null when the item has no code file. */
    fun slotAt(files: List<CodeFile>, position: Int): Int? =
        files.getOrNull(position.coerceIn(0, maxOf(0, files.size - 1)))?.slot

    /** "2/3" for the position 1 of 3 (position is 0-based). */
    fun positionLabel(position: Int, total: Int): String = "${position.coerceIn(0, maxOf(0, total - 1)) + 1}/$total"
}

/** What the code screen is allowed to put on screen. A hidden ("Masqué") item shows nothing until the biometric gate passed. */
sealed interface CodeContent {
    /** The item is not known yet. */
    data object Loading : CodeContent

    /** The item is gone (deleted, expired) or has no code file. */
    data object Empty : CodeContent

    /** A hidden item whose authentication has not succeeded: no file read, no decoding, no name. */
    data object Locked : CodeContent

    class Files(val files: List<CodeFile>) : CodeContent
}

object CodeGate {
    fun content(item: TomiItem?, loaded: Boolean, unlocked: Boolean): CodeContent {
        if (item == null) return if (loaded) CodeContent.Empty else CodeContent.Loading
        if (item.secret && !unlocked) return CodeContent.Locked
        val files = CodeFiles.of(item)
        return if (files.isEmpty()) CodeContent.Empty else CodeContent.Files(files)
    }

    /**
     * Whether the screen must close when the app leaves the screen (ON_STOP): a hidden item never stays on screen. A rotation
     * stops the activity too and does not close it (the gate is asked again after the re-creation, instant inside the window).
     */
    fun closesOnStop(item: TomiItem?, changingConfigurations: Boolean): Boolean = item?.secret == true && !changingConfigurations
}

/** Limits of what is read and coloured. */
object CodeLimits {
    /** Largest file that is read into memory (the screen says so beyond). */
    const val MAX_BYTES = 2 * 1024 * 1024

    /** Enriched Markdown does not parse a block over 50 KB of UTF-8 or 2,000 lines: it stays uncoloured (same limits here). */
    const val HIGHLIGHT_MAX_BYTES = 50 * 1024
    const val HIGHLIGHT_MAX_LINES = 2000

    /** A line longer than this is cut on screen (a minified bundle on one line would exceed the layout limits). */
    const val MAX_DISPLAY_LINE_CHARS = 4000

    /** Spaces a tab is shown as. */
    const val TAB_WIDTH = 4

    private const val BINARY_SNIFF_BYTES = 8000

    fun canHighlight(text: String, lineCount: Int): Boolean =
        lineCount <= HIGHLIGHT_MAX_LINES && text.length <= HIGHLIGHT_MAX_BYTES && text.toByteArray(Charsets.UTF_8).size <= HIGHLIGHT_MAX_BYTES

    internal fun looksBinary(bytes: ByteArray): Boolean {
        val end = minOf(bytes.size, BINARY_SNIFF_BYTES)
        for (i in 0 until end) if (bytes[i] == 0.toByte()) return true
        return false
    }
}

/** Result of reading a file, before any display. */
sealed interface CodeLoadResult {
    /** [original] is the text as stored (copied as is), [display] has uniform `\n` line ends and tabs expanded. */
    class Text(val original: String, val display: String) : CodeLoadResult

    data object TooLarge : CodeLoadResult
    data object Binary : CodeLoadResult
    data object Unavailable : CodeLoadResult
}

object CodeDecoding {
    /** Decodes the bytes of a file. [bytes] must not be longer than [CodeLimits.MAX_BYTES] + 1 (the reader stops there). */
    fun decode(bytes: ByteArray): CodeLoadResult {
        if (bytes.size > CodeLimits.MAX_BYTES) return CodeLoadResult.TooLarge
        if (CodeLimits.looksBinary(bytes)) return CodeLoadResult.Binary
        val start = if (bytes.size >= 3 && bytes[0] == 0xEF.toByte() && bytes[1] == 0xBB.toByte() && bytes[2] == 0xBF.toByte()) 3 else 0
        // Invalid sequences become U+FFFD: a file in another encoding is shown, not refused.
        val original = String(bytes, start, bytes.size - start, Charsets.UTF_8)
        return CodeLoadResult.Text(original, display(original))
    }

    /**
     * Reads a stream into memory and decodes it. Stops as soon as the file is known to be over [CodeLimits.MAX_BYTES], without
     * reading the rest. Nothing is written anywhere: the text only lives in the memory of the screen.
     */
    fun read(input: java.io.InputStream): CodeLoadResult {
        val out = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(16 * 1024)
        var total = 0
        while (true) {
            val n = input.read(buffer)
            if (n < 0) break
            total += n
            if (total > CodeLimits.MAX_BYTES) return CodeLoadResult.TooLarge
            out.write(buffer, 0, n)
        }
        return decode(out.toByteArray())
    }

    /** `\r\n` and `\r` become `\n`; a tab becomes [CodeLimits.TAB_WIDTH] spaces. Search and colours work on this text. */
    fun display(original: String): String {
        val unified = original.replace("\r\n", "\n").replace('\r', '\n')
        return if ('\t' in unified) unified.replace("\t", " ".repeat(CodeLimits.TAB_WIDTH)) else unified
    }
}

/** A text cut into lines, with offset ↔ (line, column) conversions. Lines are split on `\n` only (see [CodeDecoding.display]). */
class CodeDocument(val text: String) {
    private val starts: IntArray

    init {
        var count = 1
        for (c in text) if (c == '\n') count++
        starts = IntArray(count)
        var line = 1
        for (i in text.indices) if (text[i] == '\n') starts[line++] = i + 1
    }

    val lineCount: Int get() = starts.size

    fun lineStart(line: Int): Int = starts[line]

    /** End of [line] (exclusive), the `\n` not included. */
    fun lineEnd(line: Int): Int = if (line + 1 < starts.size) starts[line + 1] - 1 else text.length

    fun line(line: Int): String = text.substring(starts[line], lineEnd(line))

    /** The line that contains the offset [offset] (the `\n` that ends a line belongs to it). */
    fun lineOf(offset: Int): Int {
        val target = offset.coerceIn(0, text.length)
        var low = 0
        var high = starts.size - 1
        while (low < high) {
            val mid = (low + high + 1) ushr 1
            if (starts[mid] <= target) low = mid else high = mid - 1
        }
        return low
    }

    fun columnOf(offset: Int): Int = offset.coerceIn(0, text.length) - starts[lineOf(offset)]

    val maxLineLength: Int
        get() {
            var max = 0
            for (i in 0 until lineCount) max = maxOf(max, lineEnd(i) - starts[i])
            return minOf(max, CodeLimits.MAX_DISPLAY_LINE_CHARS)
        }
}

/** An occurrence of the search text: [start] inclusive, [end] exclusive, offsets in the display text. */
data class CodeMatch(val start: Int, val end: Int)

object CodeSearch {
    /** More occurrences than this are not listed (the counter then shows "n/5000+"). */
    const val MAX_MATCHES = 5000

    /** Occurrences of [query] in [text], case-insensitive, in order, never overlapping. An empty query has none. */
    fun find(text: String, query: String): List<CodeMatch> {
        if (query.isEmpty()) return emptyList()
        val matches = ArrayList<CodeMatch>()
        var from = 0
        while (matches.size < MAX_MATCHES) {
            val at = text.indexOf(query, from, ignoreCase = true)
            if (at < 0) break
            matches.add(CodeMatch(at, at + query.length))
            from = at + query.length
        }
        return matches
    }

    /** Next occurrence, wrapping to the first one after the last. -1 when there is none. */
    fun next(current: Int, count: Int): Int = if (count <= 0) -1 else (current + 1).mod(count)

    /** Previous occurrence, wrapping to the last one before the first. -1 when there is none. */
    fun previous(current: Int, count: Int): Int = if (count <= 0) -1 else (current - 1).mod(count)

    /** The occurrence to select when the search text changes: the first one at or after [fromOffset], else the first one. */
    fun firstFrom(matches: List<CodeMatch>, fromOffset: Int): Int {
        if (matches.isEmpty()) return -1
        val index = matches.indexOfFirst { it.start >= fromOffset }
        return if (index >= 0) index else 0
    }

    /** "3/12" (1-based) for the occurrence [current] of [count]; "0/0" when there is none; "3/5000+" past the listing cap. */
    fun counter(current: Int, count: Int): String =
        if (count <= 0 || current < 0) "0/0" else "${current.coerceIn(0, count - 1) + 1}/$count" + if (count >= MAX_MATCHES) "+" else ""
}

/** Line wrap of the code screen: off (default) = sideways scroll, on = lines fold to the screen width. The choice is stored in `UserPrefs`. */
object CodeWrap {
    /** The other state. */
    fun toggled(enabled: Boolean): Boolean = !enabled

    /** Width (px) left to the text of a line, once the gutter, the gap after it and the right padding are taken off the viewport. At least 1. */
    fun textWidthPx(viewportWidthPx: Int, gutterPx: Float, gapPx: Float, endPaddingPx: Float): Int =
        (viewportWidthPx - gutterPx - gapPx - endPaddingPx).toInt().coerceAtLeast(1)

    /**
     * Distance (px, positive = content moves up) to scroll so that the row [rowTop, rowBottom) is comfortably in the viewport
     * [viewportStart, viewportEnd) (all in the coordinates of the list): null when it already is, else the row lands a third
     * down the viewport. [margin] is the room kept above and below the row (capped at a quarter of the viewport).
     */
    fun scrollDelta(rowTop: Float, rowBottom: Float, viewportStart: Int, viewportEnd: Int, margin: Float): Float? {
        val height = (viewportEnd - viewportStart).toFloat()
        if (height <= 0f) return null
        val room = minOf(margin, height / 4f)
        if (rowTop >= viewportStart + room && rowBottom <= viewportEnd - room) return null
        return rowTop - (viewportStart + height / 3f)
    }
}

/** A coloured part of one line: columns [startCol, endCol) of the line. */
class LineRun(val startCol: Int, val endCol: Int, val argb: Int)

/** An occurrence on one line (a match that crosses lines is cut at each line end). [index] is its rank in the list of matches. */
class LineMatch(val startCol: Int, val endCol: Int, val index: Int)

object CodeLines {
    /** Splits [start, end) at line ends and gives each piece to [block] with its line and columns. Empty pieces are skipped. */
    inline fun forEachLinePiece(doc: CodeDocument, start: Int, end: Int, block: (line: Int, startCol: Int, endCol: Int) -> Unit) {
        val from = start.coerceIn(0, doc.text.length)
        val to = end.coerceIn(from, doc.text.length)
        if (to <= from) return
        var line = doc.lineOf(from)
        var cursor = from
        while (cursor < to && line < doc.lineCount) {
            val lineStart = doc.lineStart(line)
            val lineEnd = doc.lineEnd(line)
            val pieceEnd = minOf(to, lineEnd)
            if (pieceEnd > cursor) block(line, cursor - lineStart, pieceEnd - lineStart)
            cursor = lineEnd + 1
            line++
        }
    }

    /**
     * Colour runs `(start, end, argb)` of the whole text, cut at line ends and grouped by line. A line without colour has no
     * entry. Runs on a line are in the order given (the library lists tokens by position).
     */
    fun bucketRuns(doc: CodeDocument, flat: IntArray): Map<Int, List<LineRun>> {
        val result = HashMap<Int, MutableList<LineRun>>()
        var i = 0
        while (i + 2 < flat.size) {
            val argb = flat[i + 2]
            forEachLinePiece(doc, flat[i], flat[i + 1]) { line, a, b -> result.getOrPut(line) { ArrayList() }.add(LineRun(a, b, argb)) }
            i += 3
        }
        return result
    }

    /** Occurrences grouped by line. */
    fun bucketMatches(doc: CodeDocument, matches: List<CodeMatch>): Map<Int, List<LineMatch>> {
        val result = HashMap<Int, MutableList<LineMatch>>()
        matches.forEachIndexed { index, match ->
            forEachLinePiece(doc, match.start, match.end) { line, a, b -> result.getOrPut(line) { ArrayList() }.add(LineMatch(a, b, index)) }
        }
        return result
    }

    /** Horizontal position (in characters) to scroll to so that the column [col] is visible: null when it already is. */
    fun horizontalScrollTarget(col: Int, visibleStartChar: Int, visibleChars: Int): Int? {
        if (visibleChars <= 0) return null
        val margin = minOf(8, visibleChars / 4)
        return if (col < visibleStartChar + margin || col > visibleStartChar + visibleChars - margin) maxOf(0, col - visibleChars / 3) else null
    }
}
