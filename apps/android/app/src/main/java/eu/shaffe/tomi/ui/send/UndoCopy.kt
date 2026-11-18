package eu.shaffe.tomi.ui.send

import android.os.SystemClock
import eu.shaffe.tomi.send.UndoDeadline
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The copy of what was just sent, kept in memory only so that "Undo" can bring it back to the compose screen (it may be
 * a secret). It drops itself at [UndoDeadline], on its own scope: that does not depend on any screen still being there
 * (the banner may be gone: detail screen opened, app in the background...). [clear] drops it at once.
 */
class UndoCopy<T : Any>(
    private val scope: CoroutineScope,
    private val elapsedRealtimeMs: () -> Long = { SystemClock.elapsedRealtime() },
) {
    var value: T? = null
        private set

    private var expiry: Job? = null

    /** Keeps [copy] until [deadline], then drops it and calls [onExpired] (hide the banner). A new copy replaces the previous one. */
    fun keep(copy: T, deadline: UndoDeadline, onExpired: () -> Unit = {}) {
        expiry?.cancel()
        value = copy
        expiry = scope.launch {
            delay(deadline.remainingMs(elapsedRealtimeMs()))
            value = null
            expiry = null
            onExpired()
        }
    }

    /** Drops the copy now (undo done, new draft, view model cleared). */
    fun clear() {
        expiry?.cancel()
        expiry = null
        value = null
    }
}
