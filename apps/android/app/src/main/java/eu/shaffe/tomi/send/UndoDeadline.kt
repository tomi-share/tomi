package eu.shaffe.tomi.send

/**
 * End of the undo window, on the monotonic real-time clock (`SystemClock.elapsedRealtime()`, which keeps counting
 * in deep sleep and ignores both wall-clock changes and the animation scale of the developer options).
 *
 * The same value drives the banner (what the user sees) and the background job (when the item may leave), so the
 * "Undo" button is honest. The decisive guard stays [eu.shaffe.tomi.core.TomiCore.cancelIfQueued].
 */
class UndoDeadline(val atElapsedMs: Long, val windowMs: Long) {

    /** Time left for the user to undo, never negative. */
    fun remainingMs(nowElapsedMs: Long): Long = (atElapsedMs - nowElapsedMs).coerceAtLeast(0L)

    /** Progress bar value: 1 at the start, 0 at the deadline. */
    fun fraction(nowElapsedMs: Long): Float =
        if (windowMs <= 0L) 0f else (remainingMs(nowElapsedMs).toFloat() / windowMs).coerceIn(0f, 1f)

    fun isOver(nowElapsedMs: Long): Boolean = remainingMs(nowElapsedMs) == 0L

    /**
     * How long a background job must still wait before it may push the item. A value outside `1..windowMs` is
     * ignored: it means the clock was reset by a reboot, and the window is long over.
     */
    fun waitBeforeDispatchMs(nowElapsedMs: Long): Long {
        val remaining = atElapsedMs - nowElapsedMs
        return if (remaining in 1..windowMs) remaining else 0L
    }

    companion object {
        fun startingAt(nowElapsedMs: Long, windowMs: Long) = UndoDeadline(nowElapsedMs + windowMs, windowMs)
    }
}
