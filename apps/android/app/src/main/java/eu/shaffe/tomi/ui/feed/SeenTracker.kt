package eu.shaffe.tomi.ui.feed

import androidx.compose.foundation.lazy.LazyListLayoutInfo

/**
 * Reading by looking: a card that stays on screen for [dwellMs] counts as seen. A quick scroll past reads nothing.
 *
 * Pure bookkeeping (no clock, no Compose): the caller says which keys are on screen and when, and asks which ones
 * are due. A key that was reported as due is not due again until it has left the screen.
 */
internal class SeenTracker(private val dwellMs: Long = DWELL_MS) {
    private val since = HashMap<String, Long>()
    private val reported = HashSet<String>()

    /** The keys that are on screen now (replaces the previous answer). */
    fun update(onScreen: Set<String>, now: Long) {
        since.keys.retainAll(onScreen)
        reported.retainAll(onScreen)
        for (key in onScreen) since.getOrPut(key) { now }
    }

    /** The keys that stayed long enough, once each. */
    fun takeDue(now: Long): List<String> {
        val due = since.filter { (key, start) -> key !in reported && now - start >= dwellMs }.keys.toList()
        reported += due
        return due
    }

    /** Time left before the next key is due, or `null` when none is waiting. */
    fun nextDelay(now: Long): Long? =
        since.filterKeys { it !in reported }.values.minOfOrNull { (it + dwellMs - now).coerceAtLeast(0L) }

    /** Everything is forgotten: the screen was not attended (paused, covered), the time did not count. */
    fun reset() {
        since.clear()
        reported.clear()
    }

    companion object {
        /** The same dwell as the Windows feed. */
        const val DWELL_MS = 1_500L

        /** Share of a card (or of the visible zone, for a taller card) that must be on screen. */
        const val VISIBLE_SHARE = 0.6f

        /**
         * The keys of the rows that are on screen enough, inside the zone left free by the content padding (the
         * header and the toolbar float over the list).
         */
        fun visibleKeys(info: LazyListLayoutInfo, share: Float = VISIBLE_SHARE): Set<String> {
            val top = info.viewportStartOffset + info.beforeContentPadding
            val bottom = info.viewportEndOffset - info.afterContentPadding
            val zone = bottom - top
            if (zone <= 0) return emptySet()
            return info.visibleItemsInfo.mapNotNullTo(HashSet()) { row ->
                (row.key as? String)?.takeIf { visibleEnough(row.offset, row.size, top, bottom, share) }
            }
        }

        /** [offset]/[size] is the row, [top]/[bottom] the free zone, all in px along the scroll axis. */
        fun visibleEnough(offset: Int, size: Int, top: Int, bottom: Int, share: Float = VISIBLE_SHARE): Boolean {
            val zone = bottom - top
            if (size <= 0 || zone <= 0) return false
            val shown = (minOf(offset + size, bottom) - maxOf(offset, top)).coerceAtLeast(0)
            // A small tolerance: 60 % of 200 px must count, whatever the rounding of the float.
            return shown.toFloat() / minOf(size, zone) >= share - 0.001f
        }
    }
}
