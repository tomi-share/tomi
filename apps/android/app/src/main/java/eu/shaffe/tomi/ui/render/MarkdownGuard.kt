package eu.shaffe.tomi.ui.render

import com.swmansion.enriched.markdown.parser.MarkdownASTNode

/**
 * Keeps the markdown renderer 100% offline.
 *
 * Enriched Markdown renders natively (no WebView, no HTML) but its `ui` artifact bundles OkHttp and downloads `http(s)` images
 * itself, with no switch to turn that off. Tomi never wants a received text to make the phone contact a server (it would reveal that
 * and when the item was read, and "images" are exactly the classic tracking pixel). So no markdown image is ever given to the renderer:
 *
 * 1. [neutralizeImages] escapes every image opener `![` outside code (the image becomes a "!" followed by an ordinary link, which
 *    only does something when the user taps it).
 * 2. [safeMarkdown] then parses the result with the library's own parser and refuses to render if its tree still contains an
 *    `Image` node, whatever the reason (an escaping case missed above, a parser quirk). The caller then shows the plain text.
 *
 * Step 2 is the guarantee: the renderer only creates image requests for `Image` nodes of that very tree, so a markdown that passed
 * has none. Failing to parse also refuses to render (fail closed).
 */
object MarkdownGuard {
    /** Longer markdown is shown as plain text: it is not worth parsing twice on the main thread. */
    const val MAX_CHARS = 200_000

    /**
     * The markdown that may be handed to the renderer, or null when it must not be rendered (too long, unparsable, or still
     * containing an image after [neutralizeImages]). [parse] is the library parser; it returns null when it cannot parse.
     */
    fun safeMarkdown(markdown: String, parse: (String) -> MarkdownASTNode?): String? {
        if (markdown.isBlank()) return markdown
        if (markdown.length > MAX_CHARS) return null
        val sanitized = neutralizeImages(markdown)
        val tree = try {
            parse(sanitized)
        } catch (_: Exception) {
            null
        } ?: return null
        return if (containsImage(tree)) null else sanitized
    }

    /** True when [node] or any descendant is an image (block or inline). */
    fun containsImage(node: MarkdownASTNode): Boolean =
        node.type == MarkdownASTNode.NodeType.Image || node.children.any(::containsImage)

    /**
     * Escapes `![` (as `\![`) everywhere except in fenced code blocks and in backtick code spans, where the characters are
     * literal text and must stay as they are. Text is otherwise unchanged, line for line.
     */
    fun neutralizeImages(markdown: String): String {
        if (!markdown.contains("![")) return markdown
        val out = StringBuilder(markdown.length + 16)
        var fenceChar = ' '
        var fenceLength = 0
        var lineStart = 0
        while (lineStart <= markdown.length) {
            val newline = markdown.indexOf('\n', lineStart)
            val lineEnd = if (newline < 0) markdown.length else newline
            val line = markdown.substring(lineStart, lineEnd)
            if (fenceLength > 0) {
                out.append(line)
                if (closesFence(line, fenceChar, fenceLength)) fenceLength = 0
            } else {
                val opening = openingFence(line)
                if (opening != null) {
                    fenceChar = opening.first
                    fenceLength = opening.second
                    out.append(line)
                } else {
                    escapeInLine(line, out)
                }
            }
            if (newline >= 0) out.append('\n')
            lineStart = lineEnd + 1
        }
        return out.toString()
    }

    /** The fence character and length when [line] opens a fenced code block (up to 3 spaces of indent, then 3+ backticks or tildes). */
    private fun openingFence(line: String): Pair<Char, Int>? {
        val indent = line.takeWhile { it == ' ' }.length
        if (indent > 3 || indent >= line.length) return null
        val c = line[indent]
        if (c != '`' && c != '~') return null
        val run = line.drop(indent).takeWhile { it == c }.length
        if (run < 3) return null
        // A backtick fence cannot carry a backtick in its info string (that would be an inline code span).
        if (c == '`' && line.indexOf('`', indent + run) >= 0) return null
        return c to run
    }

    private fun closesFence(line: String, fenceChar: Char, fenceLength: Int): Boolean {
        val indent = line.takeWhile { it == ' ' }.length
        if (indent > 3) return false
        val rest = line.drop(indent)
        val run = rest.takeWhile { it == fenceChar }.length
        return run >= fenceLength && rest.drop(run).isBlank()
    }

    private fun escapeInLine(line: String, out: StringBuilder) {
        var i = 0
        while (i < line.length) {
            val c = line[i]
            when {
                // A backslash escapes the next character: "\\" is a literal backslash, so a "![" after it IS an image opener.
                c == '\\' && i + 1 < line.length -> {
                    out.append(c).append(line[i + 1])
                    i += 2
                }
                c == '`' -> {
                    val run = countRun(line, i, '`')
                    val close = findClosingRun(line, i + run, run)
                    if (close < 0) {
                        out.append(line, i, i + run)
                        i += run
                    } else {
                        out.append(line, i, close + run)
                        i = close + run
                    }
                }
                c == '!' && i + 1 < line.length && line[i + 1] == '[' -> {
                    out.append("\\!")
                    i++
                }
                else -> {
                    out.append(c)
                    i++
                }
            }
        }
    }

    private fun countRun(line: String, from: Int, c: Char): Int {
        var n = 0
        while (from + n < line.length && line[from + n] == c) n++
        return n
    }

    /** Start of the next run of exactly [length] backticks at or after [from], or -1. */
    private fun findClosingRun(line: String, from: Int, length: Int): Int {
        var i = from
        while (i < line.length) {
            if (line[i] == '`') {
                val run = countRun(line, i, '`')
                if (run == length) return i
                i += run
            } else {
                i++
            }
        }
        return -1
    }
}
