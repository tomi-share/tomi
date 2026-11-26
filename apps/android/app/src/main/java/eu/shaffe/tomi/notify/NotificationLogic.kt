package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.Device
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.ui.code.CodeFiles

/** What a tap on a notification does. */
enum class TapAction {
    /** Link: open in the browser. */
    OpenLink,

    /** Short plain text: copy it. */
    CopyText,

    /** Long or rich text: open the card. */
    OpenCard,

    /** Image: open the preview (card with the large thumbnail). */
    PreviewImage,

    /** Other file: open with the default app. */
    OpenFile,

    /** Secret: biometric prompt, then copy. */
    AuthenticateAndCopy,
    ;

    companion object {
        const val SHORT_TEXT_MAX_CHARS = 140

        fun forItem(item: TomiItem): TapAction = when (item.kind()) {
            ItemKind.Secret -> AuthenticateAndCopy
            ItemKind.Image -> PreviewImage
            // A code file is read in Tomi's own code screen (colours, search), not handed to another app: open the card.
            ItemKind.File -> if (item.attachments.any(CodeFiles::isCodeFile)) OpenCard else OpenFile
            ItemKind.Link -> OpenLink
            ItemKind.RichText, ItemKind.Unreadable -> OpenCard
            ItemKind.Text -> {
                val plain = item.text?.plain.orEmpty()
                if (plain.length <= SHORT_TEXT_MAX_CHARS && '\n' !in plain) CopyText else OpenCard
            }
        }
    }
}

/**
 * Groups notifications arriving less than [windowMs] apart. Feed it every new item; it tells whether the
 * notification should be a single one or a summary, and which item ids the summary replaces.
 */
class NotificationBatcher(private val windowMs: Long = DEFAULT_WINDOW_MS) {
    private var lastArrivalMs: Long? = null
    private val batch = mutableListOf<String>()

    sealed interface Decision {
        data object Single : Decision
        data class Summary(val itemIds: List<String>) : Decision
    }

    @Synchronized
    fun onArrival(itemId: String, nowMs: Long): Decision {
        val last = lastArrivalMs
        if (last == null || nowMs - last >= windowMs) batch.clear()
        batch.add(itemId)
        lastArrivalMs = nowMs
        return if (batch.size == 1) Decision.Single else Decision.Summary(batch.toList())
    }

    companion object {
        const val DEFAULT_WINDOW_MS = 10_000L
    }
}

/**
 * The history that a device imports right after it joined a space (the user said yes at pairing) arrives as a burst of items that are
 * not news: announcing it as a pile of notifications ("27 nouveaux éléments") is wrong. This tells, for each emission of the feed, whether
 * the device is still within [graceMs] of the moment it became paired while the app was running; those items are history, not notified.
 * A device that was already paired when the notifier started never has a grace period.
 */
class PairingGrace(private val nowMs: () -> Long, private val graceMs: Long = DEFAULT_GRACE_MS) {
    private var wasPaired: Boolean? = null
    private var untilMs = 0L
    private var active = false

    @Synchronized
    fun isImporting(paired: Boolean): Boolean {
        val before = wasPaired
        wasPaired = paired
        if (before == false && paired) {
            active = true
            untilMs = nowMs() + graceMs
        }
        if (!paired) active = false
        if (active && nowMs() >= untilMs) active = false
        return active
    }

    companion object {
        /** Long enough for an import over a slow network, short enough that a real item sent minutes later still notifies. */
        const val DEFAULT_GRACE_MS = 2 * 60_000L
    }
}

/**
 * Spots a device that joined the space since the last emission of the device list: the detection control against someone adding a
 * device to the space (a notification on the OTHER devices lets the owner revoke it). The first emission is the existing list, and the
 * list that arrives while this device is itself being paired ([importing]) is not news; the known ids still follow, so that nothing is
 * announced twice. This device is never announced to itself.
 */
class NewDeviceDetector {
    private var known: Set<String>? = null

    @Synchronized
    fun onDevices(devices: List<Device>, importing: Boolean): List<Device> {
        val previous = known
        // Only a device whose certificate the space accepted counts: the relay alone can list one that nobody added, and an
        // alert about it would be a false alarm. It is announced if it becomes verified later.
        val verified = devices.filter { it.verified }
        known = verified.mapTo(HashSet()) { it.id }
        if (previous == null || importing) return emptyList()
        return verified.filter { it.id !in previous && !it.isThisDevice }
    }
}
