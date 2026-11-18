package eu.shaffe.tomi.ui.render

/**
 * Bounded preview of a long text for a feed card (pure Kotlin, no Android: tested on the JVM).
 *
 * A card shows 220 dp (about 10 lines) of its body, yet the Markdown renderer used to guard, parse and lay out the WHOLE
 * document (a 45 KB file of tables = thousands of native views, built on the main thread, again at each LazyColumn reuse).
 * The preview cuts the source BEFORE the guard, the parser and the renderer: whole lines up to a budget, then adjusted so the
 * cut never lands inside a construct that would render differently (fenced code is closed, a table is only cut between two
 * rows). The budget (40 lines / 2,000 characters) is far beyond what the card shows, so an imperfect cut stays out of sight.
 *
 * The preview only lives in memory, for display: opening, copying, sharing and searching still use the full text.
 */
data class Budget(val maxChars: Int, val maxLines: Int, val maxTableRows: Int)

/** Budget of a feed card: the rendered height stays around 2,000 dp at most, the card shows 220 dp of it. */
val CARD_BUDGET = Budget(2_000, 40, 8)

/** A bounded text. [toString] never prints the content (secrets, logs). */
class Preview(val markdown: String, val truncated: Boolean) {
    override fun toString() = "Preview(<redacted>, $truncated)"
}

/** Cap of the reference definitions (`[id]: url`) kept for free: those of the preview and those appended from further down. */
private const val FREE_DEFINITIONS_CAP = 8 * 1024

/** A long line is cut inside only if this much room is left (else the preview simply stops before it). */
private const val MIN_PARTIAL_CHARS = 40

/**
 * Markdown preview: [md] itself (not truncated) when it fits in [budget], else its beginning, cut at a safe block boundary,
 * followed by the reference definitions it uses. Idempotent: the preview of a preview is itself, not truncated.
 */
fun preview(md: String, budget: Budget = CARD_BUDGET): Preview {
    if (md.isEmpty()) return Preview(md, false)
    val lines = splitLines(md)
    if (walk(md, lines, budget.maxLines, budget.maxChars, budget.maxTableRows).complete) return Preview(md, false)
    // One line and one character are kept back for the blank line that separates the appended definitions.
    val cut = preferBlockBoundary(walk(md, lines, budget.maxLines - 1, budget.maxChars - 1, budget.maxTableRows))
    val body = StringBuilder()
    body.append(md, 0, cut.endOffset(lines))
    trimTrailingBlank(body)
    cut.openFence?.let { fence -> body.append('\n').append(fence.closing) }
    val definitions = laterDefinitions(md, lines, cut, body, FREE_DEFINITIONS_CAP - cut.freeUsed)
    if (definitions.isNotEmpty()) {
        body.append("\n\n")
        definitions.joinTo(body, "\n")
    }
    return Preview(body.toString(), true)
}

/**
 * First lines of a plain text (plain text, Markdown source shown literally, revealed secret): at most [maxLines] lines and
 * [maxChars] characters, a too long line being cut on a space (never inside a surrogate pair or an obvious emoji cluster).
 */
fun head(text: String, maxLines: Int, maxChars: Int): Preview {
    var usedLines = 0
    var usedChars = 0
    var previousEnd = 0
    var start = 0
    val length = text.length
    // The lines of splitLines, read one at a time: a card only pays for the lines it keeps, whatever the length of the text.
    while (start < length) {
        var end = start
        while (end < length && text[end] != '\n' && text[end] != '\r') end++
        if (usedLines + 1 > maxLines) return Preview(text.substring(0, previousEnd).trimEnd(), true)
        val cost = end - start + 1
        if (usedChars + cost > maxChars) {
            val room = maxChars - usedChars - 1
            val kept = if (room > 0) start + partialCut(text, Line(start, end), room) else previousEnd
            return Preview(text.substring(0, kept).trimEnd(), true)
        }
        usedLines++
        usedChars += cost
        previousEnd = end
        start = when {
            end >= length -> length
            text[end] == '\r' && end + 1 < length && text[end + 1] == '\n' -> end + 2
            else -> end + 1
        }
    }
    return Preview(text, false)
}

// --- lines --------------------------------------------------------------------------------------------------------------

/** A line of the source: its content is [start, end), line separator excluded (\n, \r\n or \r). */
private class Line(val start: Int, val end: Int) {
    val length get() = end - start
}

/** Lines of [s]; a final line separator does not open an extra empty line. */
private fun splitLines(s: String): List<Line> {
    val lines = ArrayList<Line>()
    var start = 0
    var i = 0
    while (i < s.length) {
        val c = s[i]
        if (c == '\n' || c == '\r') {
            lines += Line(start, i)
            i += if (c == '\r' && i + 1 < s.length && s[i + 1] == '\n') 2 else 1
            start = i
        } else {
            i++
        }
    }
    if (start < s.length) lines += Line(start, s.length)
    return lines
}

private fun String.lineText(line: Line) = substring(line.start, line.end)

/**
 * First character of [line] after an indent of 3 spaces at most, or NUL (empty line, deeper indent): what a fence or a
 * reference definition starts with, read without copying the line.
 */
private fun leadChar(md: String, line: Line): Char {
    var i = line.start
    val stop = minOf(line.end, line.start + 4)
    while (i < stop && md[i] == ' ') i++
    return if (i < line.end && i - line.start <= 3) md[i] else '\u0000'
}

private fun isBlank(text: String) = text.all { it == ' ' || it == '\t' }

// --- block recognition (a CommonMark / GFM subset: enough to never cut a construct in two) ------------------------------

/** A fence closes with its own indent: inside a list item, a fence at column 0 would end the item and open a new block. */
private class Fence(val char: Char, val length: Int, val indent: Int) {
    val closing get() = " ".repeat(indent) + char.toString().repeat(length)
}

/** Indentation of at most 3 spaces (a tab or 4 spaces make an indented code block, not a fence). */
private fun indentOk(text: String): Int? {
    var i = 0
    while (i < text.length && text[i] == ' ') i++
    return if (i <= 3) i else null
}

private fun fenceRun(text: String, from: Int): Pair<Char, Int>? {
    if (from >= text.length) return null
    val c = text[from]
    if (c != '`' && c != '~') return null
    var i = from
    while (i < text.length && text[i] == c) i++
    return if (i - from >= 3) c to i - from else null
}

/** Opening fence: ``` or ~~~ (3 or more); the info string of a backtick fence holds no backtick. */
private fun openingFence(text: String): Fence? {
    val indent = indentOk(text) ?: return null
    val (c, length) = fenceRun(text, indent) ?: return null
    if (c == '`' && text.indexOf('`', indent + length) >= 0) return null
    return Fence(c, length, indent)
}

/** Closing fence: same character, at least as long, nothing but spaces after. */
private fun closes(text: String, fence: Fence): Boolean {
    val indent = indentOk(text) ?: return false
    val (c, length) = fenceRun(text, indent) ?: return false
    return c == fence.char && length >= fence.length && isBlank(text.substring(indent + length))
}

private val DELIMITER_ROW = Regex("""^ {0,3}\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$""")
private val HEADING = Regex("""^ {0,3}#{1,6}(\s|$)""")
private val QUOTE = Regex("""^ {0,3}>""")
private val DEFINITION = Regex("""^ {0,3}\[((?:[^\[\]\\]|\\.){1,999})\]:\s*\S""")

/** Cells of a table row, split on unescaped pipes (leading and trailing pipes are borders). */
private fun cellCount(text: String): Int {
    var t = text.trim()
    if (t.startsWith("|")) t = t.substring(1)
    if (t.endsWith("|") && !t.endsWith("\\|")) t = t.substring(0, t.length - 1)
    var count = 1
    var i = 0
    while (i < t.length) {
        when (t[i]) {
            '\\' -> i++
            '|' -> count++
        }
        i++
    }
    return count
}

/** GFM table start: a header row with a pipe, then a delimiter row (`|---|`) with as many cells. */
private fun isTableStart(header: String, delimiter: String): Boolean =
    header.contains('|') && !isBlank(header) && DELIMITER_ROW.matches(delimiter) && cellCount(header) == cellCount(delimiter)

/** A line that ends a table instead of being one of its rows. */
private fun endsTable(text: String): Boolean {
    if (isBlank(text) || openingFence(text) != null) return true
    // First-character checks before the regexes: this runs on every table row.
    return when (firstNonSpace(text)) {
        '#' -> HEADING.containsMatchIn(text)
        '>' -> QUOTE.containsMatchIn(text)
        else -> false
    }
}

private fun definitionLabel(text: String): String? = DEFINITION.find(text)?.groupValues?.get(1)?.let(::normalizeLabel)

/** Label as CommonMark matches it: trimmed, inner whitespace runs as one space, lowercase (no regex: called per bracket). */
private fun normalizeLabel(label: String): String {
    val out = StringBuilder(label.length)
    var pendingSpace = false
    for (c in label) {
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\u000B' || c == '\u000C') {
            pendingSpace = out.isNotEmpty()
        } else {
            if (pendingSpace) out.append(' ')
            pendingSpace = false
            out.append(c)
        }
    }
    return out.toString().lowercase()
}

// --- the walk ----------------------------------------------------------------------------------------------------------

/**
 * Where a walk stopped. [lineCount] whole lines are kept, plus the first [partial] characters of the next one if not null;
 * [openFence] must then be closed; [freeUsed] = characters of the definitions kept for free.
 */
private class Cut(
    val complete: Boolean,
    val lineCount: Int,
    val partial: Int?,
    val openFence: Fence?,
    val freeUsed: Int,
    val inTable: Boolean,
    /** For each kept line: chars used before it, and whether a cut just before it lands between two blocks. */
    val charsBefore: IntArray,
    val usedChars: Int,
    val blockBoundaries: List<Int>,
) {
    fun endOffset(lines: List<Line>): Int = when {
        partial != null -> lines[lineCount].start + partial
        lineCount == 0 -> 0
        else -> lines[lineCount - 1].end
    }
}

/**
 * Takes whole lines while they fit in [maxLines] / [maxChars] (each line costs its length + 1), room being kept for the fence
 * to close. A table is kept from its delimiter row on, at most [maxTableRows] rows; reference definitions are free up to the cap.
 */
private fun walk(md: String, lines: List<Line>, maxLines: Int, maxChars: Int, maxTableRows: Int): Cut {
    var usedLines = 0
    var usedChars = 0
    var freeUsed = 0
    var fence: Fence? = null
    var inTable = false
    var tableRows = 0
    val charsBefore = IntArray(lines.size + 1)
    val boundaries = ArrayList<Int>()
    var i = 0

    fun stop(partial: Int? = null): Cut {
        charsBefore[i] = usedChars
        return Cut(false, i, partial, fence, freeUsed, inTable, charsBefore, usedChars, boundaries)
    }
    fun fits(lineCost: Int, charCost: Int, after: Fence?): Boolean {
        val reserveLines = if (after != null) 1 else 0
        val reserveChars = after?.let { it.closing.length + 1 } ?: 0
        return usedLines + lineCost + reserveLines <= maxLines && usedChars + charCost + reserveChars <= maxChars
    }
    fun take(line: Line, free: Boolean = false) {
        charsBefore[i] = usedChars
        if (free) {
            freeUsed += line.length + 1
        } else {
            usedLines++
            usedChars += line.length + 1
        }
        i++
    }
    /** Room left for part of a too long line (null: not worth cutting inside it). */
    fun partialRoom(after: Fence?): Int? {
        if (!fits(1, 0, after)) return null
        val room = maxChars - usedChars - 1 - (after?.let { it.closing.length + 1 } ?: 0)
        return room.takeIf { it >= MIN_PARTIAL_CHARS }
    }

    while (i < lines.size) {
        val line = lines[i]
        val text = md.lineText(line)
        val open = fence
        if (open != null) {
            val after = if (closes(text, open)) null else open
            if (!fits(1, line.length + 1, after)) {
                val room = partialRoom(open).takeIf { after != null }
                val partial = room?.let { partialCut(md, line, it) }?.takeIf { it > 0 && !closes(text.substring(0, it), open) }
                return stop(partial)
            }
            take(line)
            fence = after
            if (after == null) boundaries += i
            continue
        }
        if (inTable) {
            if (!endsTable(text)) {
                if (tableRows >= maxTableRows || !fits(1, line.length + 1, null)) return stop()
                take(line)
                tableRows++
                continue
            }
            inTable = false
            boundaries += i
        }
        val opening = openingFence(text)
        if (opening != null) {
            if (!fits(1, line.length + 1, opening)) return stop()
            if (i > 0) boundaries += i
            take(line)
            fence = opening
            continue
        }
        if (i + 1 < lines.size && isTableStart(text, md.lineText(lines[i + 1]))) {
            val next = lines[i + 1]
            if (!fits(2, line.length + 1 + next.length + 1, null)) return stop()
            if (i > 0) boundaries += i
            take(line)
            take(next)
            inTable = true
            tableRows = 0
            continue
        }
        if (definitionLabel(text) != null && freeUsed + line.length + 1 <= FREE_DEFINITIONS_CAP) {
            take(line, free = true)
            continue
        }
        if (!fits(1, line.length + 1, null)) {
            val partial = if (isBlank(text)) null else partialRoom(null)?.let { partialCut(md, line, it) }?.takeIf { it > 0 }
            // Never inside a reference definition: its prefix would define the label with a truncated URL.
            return stop(partial?.takeIf { openingFence(text.substring(0, it)) == null && definitionLabel(text) == null })
        }
        if (isBlank(text)) boundaries += i
        take(line)
    }
    charsBefore[i] = usedChars
    return Cut(true, i, null, fence, freeUsed, inTable, charsBefore, usedChars, boundaries)
}

/**
 * A cut that stopped mid-block moves back to the last blank line (or other block boundary) when that keeps at least 3/4 of the
 * lines and characters: the preview then ends on a whole paragraph. A cut between two table rows or inside a long line stays.
 */
private fun preferBlockBoundary(cut: Cut): Cut {
    if (cut.complete || cut.partial != null || cut.inTable || cut.lineCount == 0) return cut
    val best = cut.blockBoundaries.lastOrNull { at ->
        at in 1 until cut.lineCount && at * 4 >= cut.lineCount * 3 && cut.charsBefore[at] * 4 >= cut.usedChars * 3
    } ?: return cut
    // A boundary is never inside fenced code (lines in a fence are not boundaries, a closing fence is one AFTER it).
    return Cut(false, best, null, null, cut.freeUsed, false, cut.charsBefore, cut.charsBefore[best], cut.blockBoundaries)
}

private fun trimTrailingBlank(sb: StringBuilder) {
    var end = sb.length
    while (end > 0 && sb[end - 1].isWhitespace()) end--
    sb.setLength(end)
}

/**
 * Reference definitions found after the cut (outside fenced code) whose label the preview uses and does not define itself,
 * first definition of a label only (CommonMark), within [cap] characters.
 */
private fun laterDefinitions(md: String, lines: List<Line>, cut: Cut, body: CharSequence, cap: Int): List<String> {
    if (cap <= 0) return emptyList()
    val bodyLower = body.toString().lowercase()
    if (!bodyLower.contains('[')) return emptyList()
    // Labels the preview defines itself (and therefore already resolves), then those already appended.
    val defined = HashSet<String>()
    var fence: Fence? = null
    val result = ArrayList<String>()
    var used = 0
    for ((index, line) in lines.withIndex()) {
        // This runs over the whole document for each card: only a line that starts with a fence character or a bracket can
        // matter, the others are skipped without being copied out of the source.
        val lead = leadChar(md, line)
        val open = fence
        if (open != null) {
            if (lead == open.char && closes(md.lineText(line), open)) fence = null
            continue
        }
        if (lead != '[' && lead != '`' && lead != '~') continue
        val text = md.lineText(line)
        val opening = openingFence(text)
        if (opening != null) {
            fence = opening
            continue
        }
        val label = definitionLabel(text) ?: continue
        if (index < cut.lineCount) {
            defined += label
            continue
        }
        if (label in defined) continue
        defined += label
        if (!bodyLower.contains("[$label]")) continue
        val definition = text.trim()
        if (used + definition.length + 1 > cap) continue
        used += definition.length + 1
        result += definition
    }
    return result
}

// --- cutting inside a line ---------------------------------------------------------------------------------------------

/** Length of the prefix of [line] kept within [room] characters: on the last space of its second half, else on a safe boundary. */
private fun partialCut(s: String, line: Line, room: Int): Int {
    if (room <= 0) return 0
    if (room >= line.length) return line.length
    val limit = line.start + room
    var space = -1
    var j = limit
    while (j > line.start + room / 2) {
        if (s[j] == ' ' || s[j] == '\t') {
            space = j
            break
        }
        j--
    }
    val end = if (space > line.start) space else safeBoundary(s, limit, line.start)
    var trimmed = end
    while (trimmed > line.start && (s[trimmed - 1] == ' ' || s[trimmed - 1] == '\t')) trimmed--
    return trimmed - line.start
}

/** Moves [at] back (not below [floor]) until it no longer splits a surrogate pair or an obvious emoji / combining cluster. */
internal fun safeBoundary(s: CharSequence, at: Int, floor: Int = 0): Int {
    var p = at.coerceIn(floor, s.length)
    while (p > floor && p < s.length && !isBoundary(s, p)) p--
    return p
}

private fun isBoundary(s: CharSequence, p: Int): Boolean {
    val c = s[p]
    val prev = s[p - 1]
    if (Character.isLowSurrogate(c) && Character.isHighSurrogate(prev)) return false
    if (prev == '‍' || c == '‍') return false // zero width joiner
    if (c in '︀'..'️' || c == '⃣') return false // variation selectors, keycap
    val cp = Character.codePointAt(s, p)
    if (cp in 0x1F3FB..0x1F3FF) return false // skin tone modifiers
    if (cp in 0xE0020..0xE007F) return false // tag sequences (subdivision flags)
    when (Character.getType(cp).toByte()) {
        Character.NON_SPACING_MARK, Character.ENCLOSING_MARK, Character.COMBINING_SPACING_MARK -> return false
    }
    if (isRegionalIndicator(cp)) {
        // Flags are pairs of regional indicators: a cut after an odd number of them splits a flag.
        var count = 0
        var q = p
        while (q > 0) {
            val before = Character.codePointBefore(s, q)
            if (!isRegionalIndicator(before)) break
            count++
            q -= Character.charCount(before)
        }
        if (count % 2 == 1) return false
    }
    return true
}

private fun isRegionalIndicator(cp: Int) = cp in 0x1F1E6..0x1F1FF

// --- chunks of the detail ----------------------------------------------------------------------------------------------
//
// The detail screen shows the whole text. Given to the renderer in one piece, a long Markdown document becomes ONE native view
// measured and laid out at once (a 45 KB file of tables = thousands of text views built on the main thread, a height of tens
// of thousands of pixels). The detail renders it as a column of chunks instead, each its own view, revealed while scrolling.
//
// A chunk is a run of whole top-level blocks. The source is only cut where the two halves render as the whole did: on a blank
// line followed by a line at column 0 (never inside a list item, indented code or other nested content), outside fenced code
// and outside the HTML blocks that run past blank lines. An ordered list is never cut (the renderer numbers every list from 1).
// A block too big for one chunk is cut further: a table between two rows (its header repeated), fenced code between two lines
// (closed, then reopened with its own opening line), a tight bullet list between two top-level items. A block that cannot be
// cut and is still very long is shown as plain text (its exact content), not rendered.
//
// The chunks only live in memory, for display: copying, sharing and searching still use the full text.

/** Budget of one chunk of the detail: about a screen or two of rendered text. */
val CHUNK_BUDGET = Budget(4_000, 100, 30)

/** A block that cannot be cut and is longer than this is shown as plain text instead of being parsed and rendered. */
private const val UNSPLITTABLE_SOURCE_CHARS = 16_000

/**
 * A chunk of a Markdown document: [markdown] is rendered on its own (or shown as plain text when [asSource]). [toString] never
 * prints the content (secrets, logs).
 *
 * Bookkeeping, for the tests: the chunk carries the source range [sourceStart, sourceEnd) at [prefixLength] in [markdown] (a
 * repeated table header or a reopened fence comes before it), and [bodyLength] characters of [markdown] precede the appended
 * reference definitions. [oversized]: the chunk could not be cut within the budget.
 */
class Chunk(
    val markdown: String,
    val asSource: Boolean,
    internal val sourceStart: Int = 0,
    internal val sourceEnd: Int = markdown.length,
    internal val prefixLength: Int = 0,
    internal val bodyLength: Int = markdown.length,
    internal val oversized: Boolean = false,
) {
    override fun toString() = "Chunk(<redacted>)"
}

private val ORDERED_ITEM = Regex("""^ {0,3}\d{1,9}[.)](?:[ \t]|$)""")
private val BULLET_ITEM = Regex("""^[-+*][ \t]+\S""")
private val THEMATIC_BREAK = Regex("""^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$""")
private val HTML_RAW_TAGS = listOf("pre", "script", "style", "textarea")

private const val SPLIT_NONE: Byte = 0
private const val SPLIT_FENCE: Byte = 1
private const val SPLIT_TABLE: Byte = 2
private const val SPLIT_LIST: Byte = 3

private fun firstNonSpace(text: String): Char? {
    for (c in text) if (c != ' ' && c != '\t') return c
    return null
}

private fun isOrderedItem(text: String): Boolean = firstNonSpace(text)?.isDigit() == true && ORDERED_ITEM.containsMatchIn(text)

private fun isHeading(text: String): Boolean = firstNonSpace(text) == '#' && HEADING.containsMatchIn(text)

private fun isThematicBreak(text: String): Boolean {
    val c = firstNonSpace(text)
    return (c == '-' || c == '*' || c == '_') && THEMATIC_BREAK.matches(text)
}

/** A bullet list item at column 0 with some content ("-", "+" or "*", then a space). */
private fun isBulletItem(text: String): Boolean {
    if (text.isEmpty()) return false
    val c = text[0]
    return (c == '-' || c == '+' || c == '*') && BULLET_ITEM.containsMatchIn(text)
}

/** End marker of an HTML block that runs past blank lines (CommonMark types 1 to 5) starting on [text], else null. */
private fun htmlBlockEnd(text: String): String? {
    val indent = indentOk(text) ?: return null
    if (indent >= text.length || text[indent] != '<') return null
    val head = text.substring(indent, minOf(text.length, indent + 12)).lowercase()
    if (head.startsWith("<!--")) return "-->"
    if (head.startsWith("<![cdata[")) return "]]>"
    if (head.startsWith("<?")) return "?>"
    if (head.length > 2 && head[1] == '!' && head[2].isLetter()) return ">"
    for (tag in HTML_RAW_TAGS) {
        if (head.startsWith("<$tag")) {
            val after = head.getOrNull(tag.length + 1)
            if (after == null || after == ' ' || after == '\t' || after == '>') return "</$tag>"
        }
    }
    return null
}

/** Start of an HTML block that ends at the next blank line (CommonMark types 6 and 7, loosely: any tag at the start). */
private fun startsHtmlUntilBlank(text: String): Boolean {
    val indent = indentOk(text) ?: return false
    if (indent + 1 >= text.length || text[indent] != '<') return false
    val next = text[indent + 1]
    return next.isLetter() || next == '/'
}

/** A line that starts a block of its own (it ends a list item it is not indented into). */
private fun opensBlock(text: String): Boolean =
    openingFence(text) != null || isHeading(text) || isThematicBreak(text) || isBulletItem(text) ||
        (firstNonSpace(text) == '>' && QUOTE.containsMatchIn(text)) || startsHtmlUntilBlank(text)

/**
 * What a single pass over the source learns, per line: may a chunk start here ([cut]), is it a heading, a table body row,
 * where an oversized block may be cut further ([splitKind], [splitRef] = the opening fence line or the table header line).
 */
private class Outline(val md: String, val lines: List<Line>) {
    val n = lines.size
    val cut = BooleanArray(n)
    val heading = BooleanArray(n)
    val row = BooleanArray(n)
    val splitKind = ByteArray(n)
    val splitRef = IntArray(n)

    /** Label -> line of its first reference definition. */
    val definitions = HashMap<String, Int>()

    /** Prefix sums of the line costs (length + 1) and of the table rows. */
    val charsBefore = IntArray(n + 1)
    val rowsBefore = IntArray(n + 1)

    /** The source ends inside fenced code or a raw HTML block: nothing may be appended after its last line. */
    var openAtEnd = false
        private set

    init {
        var fence: Fence? = null
        var fenceLine = -1
        var fenceSplittable = false
        var htmlEnd: String? = null
        var htmlUntilBlank = false
        var tableHeader = -1
        var tableSplittable = false
        var orderedOpen = false
        var prevBlank = true
        var seenContent = false
        for (i in 0 until n) {
            val text = md.lineText(lines[i])
            val open = fence
            when {
                open != null -> {
                    if (closes(text, open)) {
                        fence = null
                    } else if (fenceSplittable && i >= fenceLine + 2) {
                        splitKind[i] = SPLIT_FENCE
                        splitRef[i] = fenceLine
                    }
                    prevBlank = false
                }
                htmlEnd != null -> {
                    if (text.contains(htmlEnd, ignoreCase = true)) htmlEnd = null
                    prevBlank = false
                }
                isBlank(text) -> {
                    tableHeader = -1
                    htmlUntilBlank = false
                    prevBlank = true
                }
                else -> {
                    val col0 = text[0] != ' ' && text[0] != '\t'
                    val ordered = isOrderedItem(text)
                    // A chunk starts on a blank line followed by column 0, never on the next item of an open ordered list.
                    if (seenContent && prevBlank && col0 && !(ordered && orderedOpen)) cut[i] = true
                    seenContent = true
                    if (col0 && isHeading(text)) heading[i] = true
                    var consumed = false
                    if (tableHeader >= 0) {
                        if (i == tableHeader + 1) {
                            consumed = true // delimiter row
                        } else if (!endsTable(text)) {
                            row[i] = true
                            if (tableSplittable && i >= tableHeader + 3) {
                                splitKind[i] = SPLIT_TABLE
                                splitRef[i] = tableHeader
                            }
                            consumed = true
                        } else {
                            tableHeader = -1
                        }
                    }
                    if (!consumed) {
                        if (ordered) {
                            orderedOpen = true
                        } else if (orderedOpen && col0 && (prevBlank || opensBlock(text))) {
                            orderedOpen = false
                        }
                        // Between two top-level items of a bullet list (never a thematic break such as "* * *").
                        if (col0 && !orderedOpen && !htmlUntilBlank && !prevBlank && isBulletItem(text) &&
                            !isThematicBreak(text)
                        ) {
                            splitKind[i] = SPLIT_LIST
                        }
                        val opening = openingFence(text)
                        val rawEnd = if (opening == null) htmlBlockEnd(text) else null
                        when {
                            opening != null -> {
                                fence = opening
                                fenceLine = i
                                fenceSplittable = !orderedOpen && !htmlUntilBlank
                            }
                            rawEnd != null -> {
                                val from = (indentOk(text) ?: 0) + 2
                                if (text.indexOf(rawEnd, from, ignoreCase = true) < 0) htmlEnd = rawEnd
                            }
                            startsHtmlUntilBlank(text) -> htmlUntilBlank = true
                            i + 1 < n && text.contains('|') && isTableStart(text, md.lineText(lines[i + 1])) -> {
                                tableHeader = i
                                tableSplittable = !orderedOpen && !htmlUntilBlank
                            }
                            firstNonSpace(text) == '[' -> definitionLabel(text)?.let { if (it !in definitions) definitions[it] = i }
                        }
                    }
                    prevBlank = false
                }
            }
            charsBefore[i + 1] = charsBefore[i] + lines[i].length + 1
            rowsBefore[i + 1] = rowsBefore[i] + if (row[i]) 1 else 0
        }
        openAtEnd = fence != null || htmlEnd != null
    }

    fun fits(from: Int, to: Int, budget: Budget): Boolean =
        charsBefore[to] - charsBefore[from] <= budget.maxChars && to - from <= budget.maxLines &&
            rowsBefore[to] - rowsBefore[from] <= budget.maxTableRows

    /** Text that reopens the construct cut before line [at]: the fence's own opening line, or the table header + delimiter. */
    fun prefixFor(at: Int): String = when (splitKind[at]) {
        SPLIT_FENCE -> md.lineText(lines[splitRef[at]]) + "\n"
        SPLIT_TABLE -> md.lineText(lines[splitRef[at]]) + "\n" + md.lineText(lines[splitRef[at] + 1]) + "\n"
        else -> ""
    }

    fun prefixLines(at: Int): Int = when (splitKind[at]) {
        SPLIT_FENCE -> 1
        SPLIT_TABLE -> 2
        else -> 0
    }

    /** Text that closes the construct cut before line [at] (fenced code only, with the indent of its opening). */
    fun suffixFor(at: Int): String =
        if (splitKind[at] == SPLIT_FENCE) "\n" + openingFence(md.lineText(lines[splitRef[at]]))!!.closing else ""
}

/** Lines [from, to) of the source, then [prefix] / [suffix] added around them. */
private class Piece(val from: Int, val to: Int, val prefix: String, val suffix: String, val oversized: Boolean)

/**
 * The detail's chunks of [md]: runs of whole blocks within [budget] (characters, lines, table rows), cut as described above.
 * Each rendered chunk carries the reference definitions it uses (8 KB at most). Linear in the length of [md].
 */
fun chunks(md: String, budget: Budget = CHUNK_BUDGET): List<Chunk> {
    if (md.isEmpty()) return emptyList()
    if (md.isBlank()) return listOf(Chunk(md, false))
    val outline = Outline(md, splitLines(md))
    val pieces = ArrayList<Piece>()
    val starts = (0 until outline.n).filter { it == 0 || outline.cut[it] }
    fun segmentEnd(s: Int) = if (s + 1 < starts.size) starts[s + 1] else outline.n
    var s = 0
    while (s < starts.size) {
        val a = starts[s]
        if (!outline.fits(a, segmentEnd(s), budget)) {
            resplit(outline, a, segmentEnd(s), budget, pieces)
            s++
            continue
        }
        var e = s + 1
        while (e < starts.size && outline.fits(a, segmentEnd(e), budget)) e++
        // Stopped by the budget (not by an oversized block): rather cut just before a heading in the second half.
        if (e < starts.size && !outline.heading[starts[e]] && outline.fits(starts[e], segmentEnd(e), budget)) {
            val total = outline.charsBefore[segmentEnd(e - 1)] - outline.charsBefore[a]
            for (k in e - 1 downTo s + 1) {
                if ((outline.charsBefore[starts[k]] - outline.charsBefore[a]) * 2 < total) break
                if (outline.heading[starts[k]]) {
                    e = k
                    break
                }
            }
        }
        pieces += Piece(a, segmentEnd(e - 1), "", "", false)
        s = e
    }
    return pieces.map { toChunk(outline, it) }
}

/**
 * Cuts the oversized block of lines [a, b) at its split points: each piece as long as possible within [budget], the reopening
 * prefix and closing suffix included. A stretch without any split point stays whole (marked oversized).
 */
private fun resplit(o: Outline, a: Int, b: Int, budget: Budget, out: MutableList<Piece>) {
    var start = a
    var prefix = ""
    var chars = 0
    var lineCount = 0
    var rows = 0
    var lastOption = -1
    var over = false

    fun begin(at: Int) {
        start = at
        prefix = if (at == a) "" else o.prefixFor(at)
        chars = prefix.length
        lineCount = if (at == a) 0 else o.prefixLines(at)
        rows = 0
        lastOption = -1
        over = false
    }
    fun emit(to: Int, oversized: Boolean) {
        out += Piece(start, to, prefix, if (to < b) o.suffixFor(to) else "", oversized)
    }

    begin(a)
    var j = a
    while (j < b) {
        if (j > start && o.splitKind[j] != SPLIT_NONE) {
            if (over) {
                emit(j, true)
                begin(j)
                continue
            }
            lastOption = j
        }
        // Room kept for the fence that would close this piece right after line j.
        val closing = if (j + 1 < b && o.splitKind[j + 1] == SPLIT_FENCE) o.suffixFor(j + 1).length else 0
        val row = if (o.row[j]) 1 else 0
        val fits = chars + o.lines[j].length + 1 + closing <= budget.maxChars &&
            lineCount + 1 + (if (closing > 0) 1 else 0) <= budget.maxLines && rows + row <= budget.maxTableRows
        if (!over && !fits) {
            if (lastOption > start) {
                emit(lastOption, false)
                val at = lastOption
                begin(at)
                j = at
                continue
            }
            over = true
        }
        chars += o.lines[j].length + 1
        lineCount++
        rows += row
        j++
    }
    emit(b, over)
}

private fun toChunk(o: Outline, piece: Piece): Chunk {
    val start = o.lines[piece.from].start
    val end = o.lines[piece.to - 1].end
    if (piece.oversized && end - start > UNSPLITTABLE_SOURCE_CHARS) {
        return Chunk(o.md.substring(start, end), true, start, end, 0, end - start, true)
    }
    val body = StringBuilder(piece.prefix.length + end - start + piece.suffix.length)
    body.append(piece.prefix).append(o.md, start, end).append(piece.suffix)
    val bodyLength = body.length
    // Definitions appended after an unclosed fence would show as code.
    val definitions = if (piece.to == o.n && o.openAtEnd) emptyList() else usedDefinitions(o, body, piece.from, piece.to)
    if (definitions.isNotEmpty()) {
        body.append("\n\n")
        definitions.joinTo(body, "\n")
    }
    return Chunk(body.toString(), false, start, end, piece.prefix.length, bodyLength, piece.oversized)
}

/**
 * Reference definitions the chunk [body] (lines [from, to)) uses without defining them: the first definition of each label, in
 * order of first use, within [FREE_DEFINITIONS_CAP] characters. Labels are read from the brackets of the body (linear).
 */
private fun usedDefinitions(o: Outline, body: CharSequence, from: Int, to: Int): List<String> {
    if (o.definitions.isEmpty()) return emptyList()
    val seen = HashSet<String>()
    val result = ArrayList<String>()
    var used = 0
    var i = 0
    while (i < body.length) {
        if (body[i] != '[') {
            i++
            continue
        }
        var close = -1
        var k = i + 1
        val limit = minOf(body.length, i + 1 + 999)
        while (k < limit) {
            val c = body[k]
            if (c == '\\') {
                k += 2
                continue
            }
            if (c == '[') break
            if (c == ']') {
                close = k
                break
            }
            k++
        }
        if (close < 0) {
            i++
            continue
        }
        val label = normalizeLabel(body.substring(i + 1, close))
        i = close
        if (label.isEmpty() || !seen.add(label)) continue
        val line = o.definitions[label] ?: continue
        if (line in from until to) continue
        val definition = o.md.lineText(o.lines[line]).trim()
        if (used + definition.length + 1 > FREE_DEFINITIONS_CAP) continue
        used += definition.length + 1
        result += definition
    }
    return result
}

/**
 * Pieces of a text shown literally (plain text, Markdown source, revealed secret), each its own text view: whole lines, at
 * most [maxLines] lines and [maxChars] characters; a longer line is cut after a space (else on a safe boundary, never inside a
 * surrogate pair or an emoji cluster). A piece of blank lines only joins its neighbour. Rejoined with their line separators,
 * the pieces are the text.
 */
fun sourceChunks(text: String, maxLines: Int = SOURCE_CHUNK_LINES, maxChars: Int = SOURCE_CHUNK_CHARS): List<String> =
    sourceChunkRanges(text, maxLines, maxChars).map { text.substring(it[0], it[1]) }

/** Budget of one piece of a text shown literally in the detail. */
internal const val SOURCE_CHUNK_LINES = 100
internal const val SOURCE_CHUNK_CHARS = 4_000

/** [sourceChunks] as [start, end) ranges of [text]. */
internal fun sourceChunkRanges(text: String, maxLines: Int, maxChars: Int): List<IntArray> {
    if (text.isEmpty()) return emptyList()
    val lineCap = maxOf(1, maxLines)
    val charCap = maxOf(2, maxChars)
    val ranges = ArrayList<IntArray>()
    var pieceStart = -1
    var pieceEnd = 0
    var usedLines = 0
    var usedChars = 0
    for (line in splitLines(text)) {
        var pos = line.start
        while (true) {
            val remaining = line.end - pos
            if (pieceStart >= 0 && (usedLines + 1 > lineCap || usedChars + remaining + 1 > charCap)) {
                ranges += intArrayOf(pieceStart, pieceEnd)
                pieceStart = -1
            }
            if (pieceStart < 0) {
                pieceStart = pos
                usedLines = 0
                usedChars = 0
            }
            if (remaining + 1 <= charCap) {
                usedLines++
                usedChars += remaining + 1
                pieceEnd = line.end
                break
            }
            // Longer than a whole piece: its beginning makes a piece of its own.
            val cut = cutInside(text, pos, line.end, charCap - 1)
            ranges += intArrayOf(pieceStart, cut)
            pieceStart = -1
            pos = cut
        }
    }
    if (pieceStart >= 0) ranges += intArrayOf(pieceStart, pieceEnd)
    // Blank pieces join a neighbour (no view for empty lines only).
    val merged = ArrayList<IntArray>(ranges.size)
    for (range in ranges) {
        val blank = (range[0] until range[1]).all { text[it].isWhitespace() }
        if (blank && merged.isNotEmpty()) {
            merged.last()[1] = range[1]
        } else if (merged.size == 1 && (merged[0][0] until merged[0][1]).all { text[it].isWhitespace() }) {
            merged[0][1] = range[1]
        } else {
            merged += range
        }
    }
    return merged
}

/** End of a piece taken from [pos] in a line ending at [end], at most [room] (>= 1) characters, never empty. */
private fun cutInside(s: String, pos: Int, end: Int, room: Int): Int {
    val limit = pos + room
    if (limit >= end) return end
    var j = limit - 1
    while (j > pos + room / 2) {
        if (s[j] == ' ' || s[j] == '\t') return j + 1
        j--
    }
    val back = safeBoundary(s, limit, pos)
    if (back > pos) return back
    // A cluster longer than the room: keep it whole.
    var p = limit
    while (p < end && !isBoundary(s, p)) p++
    return p
}
