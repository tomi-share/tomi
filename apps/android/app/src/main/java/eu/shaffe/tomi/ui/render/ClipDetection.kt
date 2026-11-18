package eu.shaffe.tomi.ui.render

/**
 * Decides from two measured heights whether a height-limited view clips its content. Used for Markdown and HTML,
 * which are bounded by a maximum height instead of a line count (plain text uses the text layout's own overflow flag).
 */
object ClipDetection {
    /** One or two pixels of rounding (density scaling of a WebView, line metrics) are not a truncation. */
    const val TOLERANCE_PX = 2

    /** [contentHeight]: full height of the content in px. [visibleHeight]: height the view actually got; 0 = not laid out yet. */
    fun isClipped(contentHeight: Int, visibleHeight: Int): Boolean =
        visibleHeight > 0 && contentHeight > visibleHeight + TOLERANCE_PX
}
