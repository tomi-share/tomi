package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.TomiCore
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.isOutgoing

/** What the swipe of a card (or the "Archive" entry of its menu) asks the core to do. */
enum class ArchiveAction {
    Archive,
    Unarchive,
    ;

    /** The action that cancels this one (the "Undo" of the feedback banner). */
    val inverse: ArchiveAction get() = if (this == Archive) Unarchive else Archive
}

/**
 * Pure rules of the archive gesture. The UI only goes through
 * [TomiCore.archive] / [TomiCore.unarchive]: there is no local archive state in the app.
 */
object ArchiveSwipe {

    /**
     * An outgoing item that is still queued or failed stays where it is: a failed send must remain visible (section 2.4), and
     * the queue entry belongs to the send flow.
     */
    fun canArchive(item: TomiItem, localDeviceId: String): Boolean =
        !(item.isOutgoing(localDeviceId) && item.deliveries.any { it.state == DeliveryState.Queued || it.state == DeliveryState.Failed })

    /** The action of the menu entry: "Unarchive" for an archived item, "Archive" for any other. */
    fun menuAction(item: TomiItem): ArchiveAction =
        if (item.isShownAsArchived()) ArchiveAction.Unarchive else ArchiveAction.Archive

    /**
     * The action of the swipe to the left on [item], or null when the card does not swipe. A pinned card never swipes (the user
     * wanted it on top; the menu offers "Unpin" and "Archive"), nor does an item that cannot be archived. Everywhere else the
     * swipe archives, except on an archived card (Archives view, or a search) where it un-archives.
     */
    fun swipeAction(item: TomiItem, localDeviceId: String): ArchiveAction? = when {
        item.pinned -> null
        !canArchive(item, localDeviceId) -> null
        else -> menuAction(item)
    }

    /** Runs [action] on the core. The one place that maps an [ArchiveAction] to the contract. */
    suspend fun perform(core: TomiCore, action: ArchiveAction, itemId: String) = when (action) {
        ArchiveAction.Archive -> core.archive(itemId)
        ArchiveAction.Unarchive -> core.unarchive(itemId)
    }

    /**
     * Undoes an [action] done on [item] (as it was BEFORE the action). Un-archiving brings the item back to "Recent"; if the
     * archive had unpinned it, the pin is restored too.
     */
    suspend fun undo(core: TomiCore, action: ArchiveAction, itemId: String, wasPinned: Boolean) {
        perform(core, action.inverse, itemId)
        if (action == ArchiveAction.Archive && wasPinned) core.pin(itemId)
    }
}
