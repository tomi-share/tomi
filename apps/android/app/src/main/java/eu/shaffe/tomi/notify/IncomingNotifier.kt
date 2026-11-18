package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.isOutgoing
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * Turns "a new unread item arrived from another device" into a notification, unless the app is on screen.
 * With the real core, the same stream is fed by the sync that an FCM wake-up triggers.
 */
class IncomingNotifier(
    private val core: TomiCore,
    private val notifications: IncomingNotifications,
    private val scope: CoroutineScope,
    private val isAppVisible: () -> Boolean,
    private val nowMs: () -> Long = { System.nanoTime() / NANOS_PER_MS },
) {
    fun start(): Job = scope.launch {
        var known: Set<String>? = null
        val notified = mutableSetOf<String>()
        val grace = PairingGrace(nowMs)
        core.items.collect { list ->
            val previous = known
            known = list.mapTo(HashSet()) { it.id }
            val localId = core.localDevice.value.id

            // Dismiss notifications of items that were read elsewhere or deleted; with the last of them, the summary goes too.
            val unreadIds = list.filter { !it.read }.mapTo(HashSet()) { it.id }
            val hadNotified = notified.isNotEmpty()
            notified.filter { it !in unreadIds }.forEach {
                notifications.cancel(it)
                notified.remove(it)
            }
            if (hadNotified && notified.isEmpty()) notifications.cancelSummary()

            // The history imported right after the pairing is not news, whatever its size (checked on every emission to follow the state).
            val importing = grace.isImporting(core.isPaired.value)
            // The first emission is the existing history: it must not notify.
            if (previous == null || importing) return@collect
            val fresh = list.filter { it.id !in previous && !it.isOutgoing(localId) && !it.read }
            if (fresh.isEmpty() || isAppVisible()) return@collect
            fresh.asReversed().forEach {
                notified += it.id
                notifications.show(it)
            }
        }
    }

    private companion object {
        const val NANOS_PER_MS = 1_000_000L
    }
}
