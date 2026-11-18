@file:OptIn(InternalPluginApi::class)

package eu.shaffe.tomi.ui.code

import android.content.Context
import android.text.SpannableStringBuilder
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.util.Log
import android.util.LruCache
import com.swmansion.enriched.markdown.codehighlight.CodeHighlightPlugin
import com.swmansion.enriched.markdown.codehighlight.codeHighlightStyle
import com.swmansion.enriched.markdown.codehighlight.SyntaxTokenType
import com.swmansion.enriched.markdown.parser.MarkdownASTNode
import com.swmansion.enriched.markdown.plugin.CodeBlockDecorator
import com.swmansion.enriched.markdown.plugin.InternalPluginApi
import com.swmansion.enriched.markdown.plugin.MarkdownPlugin
import com.swmansion.enriched.markdown.plugin.PluginBlockSegment
import com.swmansion.enriched.markdown.plugin.PluginRegistry
import com.swmansion.enriched.markdown.renderer.NodeRenderer
import com.swmansion.enriched.markdown.renderer.RendererConfig
import com.swmansion.enriched.markdown.styles.StyleConfig

/**
 * JNI binding of `libtomi_highlight.so` (PHP grammar). Returns flat `(start, end, tokenType)` triplets, offsets in
 * UTF-16 units of the string the bytes were encoded from, token types numbered like `SyntaxTokenType`. Empty on any failure.
 * If the library cannot be loaded (an ABI that was not built), nothing crashes: PHP simply stays uncoloured.
 */
object ExtraGrammarNative {
    private val available: Boolean = try {
        System.loadLibrary("tomi_highlight")
        true
    } catch (e: Throwable) {
        Log.e("ExtraGrammarNative", "libtomi_highlight unavailable: PHP is shown without colours", e)
        false
    }

    private val EMPTY = IntArray(0)

    private external fun tokenize(code: ByteArray, language: String): IntArray

    fun tokens(code: ByteArray, language: String): IntArray {
        if (!available) return EMPTY
        return try {
            tokenize(code, language)
        } catch (e: Throwable) {
            Log.e("ExtraGrammarNative", "tokenize failed", e)
            EMPTY
        }
    }
}

/**
 * Code block decorator of the app. Enriched Markdown (code-highlight 0.3.0) has no API to register a grammar: its 14 grammars are
 * compiled in its own native library, and the classes that bind it (`SyntaxTokenSource`, `SyntaxTokenCache`,
 * `CodeHighlightDecorator`) are Kotlin-`internal`. What it does expose is the plugin registry: a plugin may register the
 * [CodeBlockDecorator] that colours the code of a fenced block. This one answers PHP itself, from [ExtraGrammarNative],
 * with the same mechanism as the library (one foreground colour span per token, colour read from the `CodeHighlightStyle` of the style, so the
 * charter palette applies), and hands every other language to the library's own decorator.
 */
private class TomiCodeBlockDecorator(private val library: CodeBlockDecorator?) : CodeBlockDecorator {
    /** Tokens by (language, code), bounded like the library's cache (about 4 MB there, 2 MB here). */
    private val cache = object : LruCache<String, IntArray>(CACHE_BYTES) {
        override fun sizeOf(key: String, value: IntArray): Int = key.length * 2 + value.size * 4 + 64
    }

    /**
     * Colours the code of one block. Colouring is cosmetic: whatever goes wrong (native failure, a change in the internal API of
     * the library, a bad token), the block stays readable and uncoloured, and the render thread never gets an exception.
     * The failure is logged at debug level with the exception class only: never the code, never the exception message.
     */
    override fun decorate(builder: SpannableStringBuilder, start: Int, end: Int, language: String?, style: StyleConfig, context: Context) {
        val added = ArrayList<ForegroundColorSpan>()
        try {
            val extra = ExtraGrammars.forInfoString(language)
            if (extra == null) {
                library?.decorate(builder, start, end, language, style, context)
                return
            }
            if (end <= start) return
            val code = builder.subSequence(start, end).toString()
            val tokens = tokensFor(code, extra)
            if (tokens.isEmpty()) return
            val colours = style.codeHighlightStyle()
            val types = SyntaxTokenType.entries
            var i = 0
            while (i + 2 < tokens.size) {
                val from = tokens[i]
                val to = tokens[i + 1]
                val type = types.getOrNull(tokens[i + 2])
                i += 3
                if (type == null || from < 0 || to > code.length || from >= to) continue
                val argb = colours[type] ?: continue
                val span = ForegroundColorSpan(argb)
                builder.setSpan(span, start + from, start + to, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
                added += span
            }
        } catch (e: Throwable) {
            // Back to plain text: take back the colour spans this function added. Spans the library may have set internally
            // on the extra == null path are not tracked here; leaving a few of them is cosmetic and never exposes content.
            added.forEach { builder.removeSpan(it) }
            if (Log.isLoggable(TAG, Log.DEBUG)) Log.d(TAG, "code colouring skipped (${e.javaClass.simpleName})")
        }
    }

    private fun tokensFor(code: String, language: String): IntArray {
        val key = language + '\u0000' + code
        cache.get(key)?.let { return it }
        val bytes = code.toByteArray(Charsets.UTF_8)
        val tokens = if (ExtraGrammars.withinLimits(bytes)) ExtraGrammarNative.tokens(bytes, language) else IntArray(0)
        cache.put(key, tokens)
        return tokens
    }

    /** Forgets the tokens kept in memory: their keys hold the code of the files shown (the device left its space). */
    fun clear() {
        cache.evictAll()
    }

    private companion object {
        const val TAG = "TomiCodeHighlight"
        const val CACHE_BYTES = 2 * 1024 * 1024
    }
}

/**
 * Replacement of the library's `CodeHighlightPlugin`: the same 14 grammars (through the library's own decorator), plus PHP.
 * Use it instead of `CodeHighlightPlugin` wherever markdown is rendered, and for the code file screen.
 */
object TomiCodeHighlightPlugin : MarkdownPlugin {
    private const val TAG = "TomiCodeHighlight"

    override val id: String = "eu.shaffe.tomi.codehighlight"

    /** The library's own decorator, captured from its plugin; null if that fails (everything then renders uncoloured). */
    private val libraryDecorator: CodeBlockDecorator? by lazy {
        try {
            var captured: CodeBlockDecorator? = null
            CodeHighlightPlugin.install(
                object : PluginRegistry {
                    override fun registerNodeRenderer(type: MarkdownASTNode.NodeType, factory: (RendererConfig, Context) -> NodeRenderer) = Unit
                    override fun registerBlockSegment(type: MarkdownASTNode.NodeType, segment: PluginBlockSegment<*>) = Unit
                    override fun registerCodeBlockDecorator(decorator: CodeBlockDecorator) {
                        captured = decorator
                    }
                },
            )
            captured
        } catch (e: Throwable) {
            Log.w(TAG, "tree-sitter decorator of the library unavailable: its grammars are shown without colours", e)
            null
        }
    }

    /** One decorator (one token cache) for markdown blocks and for the code file screen. */
    val decorator: CodeBlockDecorator by lazy { TomiCodeBlockDecorator(libraryDecorator) }

    override fun install(registry: PluginRegistry) {
        registry.registerCodeBlockDecorator(decorator)
    }

    /**
     * Empties our token cache (PHP): its keys hold the text of code files, which may come from a space this device has left. The cache of
     * the library's own decorator (the other grammars) is internal to it and cannot be reached from here.
     */
    fun clearCache() {
        (decorator as? TomiCodeBlockDecorator)?.clear()
    }
}
