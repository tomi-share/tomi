package eu.shaffe.tomi.ui.render

import com.swmansion.enriched.markdown.MarkdownOptions
import com.swmansion.enriched.markdown.parser.Md4cFlags

/**
 * THE md4c configuration of Tomi's markdown, in one place: the renderer is given [options], and [MarkdownGuard] parses with
 * [parserFlags], derived from those same options. [MarkdownGuard] only protects the phone from remote images if it sees exactly the
 * tree that the renderer builds (an autolink, a highlight or a math flag could otherwise produce an image node that the guard never saw).
 *
 * The library converts options to flags with an internal function (`MarkdownOptions.toParserFlags`), so the same mapping is written
 * here; [MarkdownEngineConfigTest] checks it field by field, and that the library's own defaults still agree with it.
 */
internal object MarkdownEngineConfig {
    val options: MarkdownOptions = MarkdownOptions.Default

    val parserFlags: Md4cFlags = flagsFor(options)

    /** The flags the renderer parses with for [options] (the library's `toParserFlags`: no LaTeX math, everything else one to one). */
    fun flagsFor(options: MarkdownOptions): Md4cFlags = Md4cFlags(
        underline = options.underline,
        latexMath = false,
        superscript = options.superscript,
        subscript = options.subscript,
        highlight = options.highlight,
        permissiveAutolinks = options.permissiveAutolinks,
        hardSoftBreaks = options.hardSoftBreaks,
        preserveBlankLines = options.preserveBlankLines,
        admonitions = options.admonitions,
    )
}
