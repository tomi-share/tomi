@file:OptIn(InternalPluginApi::class)

package eu.shaffe.tomi.ui.code

import android.content.Context
import android.text.SpannableStringBuilder
import android.text.style.ForegroundColorSpan
import android.util.Log
import com.swmansion.enriched.markdown.codehighlight.CodeHighlightStyle
import com.swmansion.enriched.markdown.codehighlight.CodeHighlightStyleKey
import com.swmansion.enriched.markdown.plugin.InternalPluginApi
import com.swmansion.enriched.markdown.styles.StyleConfig
import eu.shaffe.tomi.ui.theme.CodePalette

/**
 * Tree-sitter tokens of a whole file, for the code file screen.
 *
 * Enriched Markdown only exposes its tree-sitter binding to markdown code blocks. This class runs the decorator that colours a
 * block ([TomiCodeHighlightPlugin.decorator]: the library's own one for its 14 grammars, ours for PHP) on the file
 * text as if it were the content of a ```` ```lang ```` fence, and reads the colour spans back. Same grammars, same palette
 * mechanism, same token cache as the code blocks of a Preview card. The decorator type is the library's "not supported for
 * third-party use" API (`InternalPluginApi`): the version is pinned (0.3.0) and the screen falls back to uncoloured text if
 * anything about it fails.
 */
object TreeSitterHighlighter {
    private const val TAG = "TreeSitterHighlighter"

    /** One style per palette: [StyleConfig.default] is not free. */
    private val styles = HashMap<CodePalette, StyleConfig>()

    private fun styleFor(context: Context, palette: CodePalette): StyleConfig = synchronized(styles) {
        styles.getOrPut(palette) {
            StyleConfig.default(context.applicationContext).withExtension(CodeHighlightStyleKey, palette.toHighlightStyle())
        }
    }

    /**
     * Colour runs of [text] for the fence [grammar]: flat `(start, end, argb)` triplets, offsets in [text]. Empty when the grammar
     * is unknown to the library, the text is over its limits (50 KB / 2,000 lines), or anything fails. Blocking: call it off the
     * main thread.
     */
    fun highlight(context: Context, text: String, grammar: String, palette: CodePalette): IntArray {
        return try {
            val decorator = TomiCodeHighlightPlugin.decorator
            val builder = SpannableStringBuilder(text)
            decorator.decorate(builder, 0, builder.length, grammar, styleFor(context, palette), context.applicationContext)
            val spans = builder.getSpans(0, builder.length, ForegroundColorSpan::class.java)
            val flat = IntArray(spans.size * 3)
            spans.forEachIndexed { i, span ->
                flat[i * 3] = builder.getSpanStart(span)
                flat[i * 3 + 1] = builder.getSpanEnd(span)
                flat[i * 3 + 2] = span.foregroundColor
            }
            flat
        } catch (e: Throwable) {
            Log.w(TAG, "highlight failed: file shown without colours", e)
            EMPTY
        }
    }

    private val EMPTY = IntArray(0)
}

/** The 14 token colours of the library, from the charter palette (operators, punctuation, variables, embedded included). */
fun CodePalette.toHighlightStyle(): CodeHighlightStyle = CodeHighlightStyle(
    keyword = keyword,
    operator = operator,
    punctuation = punctuation,
    string = string,
    number = number,
    constant = constant,
    comment = comment,
    function = function,
    type = type,
    variable = variable,
    property = property,
    tag = tag,
    attribute = attribute,
    embedded = embedded,
)
