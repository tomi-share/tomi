package eu.shaffe.tomi.ui.render

/**
 * Heights the Markdown views were laid out at, kept to give a view the same height again BEFORE its rendering lands. The
 * rendering is asynchronous: a view first measures 0, then its height, so without this every card that comes back (scrolled
 * back into the feed, feed shown again after a detail) and every detail whose card was already rendered would jump from 0 to
 * its height, and a restored scroll position would be off.
 *
 * Sizes only, in memory only: an entry is the hash and the length of the text with the width and the scale it was laid out at,
 * never the text. A hash collision only reserves a wrong height until the rendering lands and corrects it.
 */
internal object RenderedHeights {
    private const val MAX_ENTRIES = 512

    private data class Key(val hash: Int, val length: Int, val width: Int, val density: Float, val fontScale: Float, val variant: Int)

    private val heights = object : LinkedHashMap<Key, Int>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Key, Int>): Boolean = size > MAX_ENTRIES
    }

    /** The height (px) [text] was last laid out at in [width] px at this scale, or null. [variant]: what else changes the height. */
    @Synchronized
    fun get(text: String, width: Int, density: Float, fontScale: Float, variant: Int): Int? =
        heights[Key(text.hashCode(), text.length, width, density, fontScale, variant)]

    @Synchronized
    fun put(text: String, width: Int, density: Float, fontScale: Float, variant: Int, height: Int) {
        if (height > 0) heights[Key(text.hashCode(), text.length, width, density, fontScale, variant)] = height
    }

    @Synchronized
    internal fun clear() = heights.clear()

    @get:Synchronized
    internal val size: Int get() = heights.size
}
