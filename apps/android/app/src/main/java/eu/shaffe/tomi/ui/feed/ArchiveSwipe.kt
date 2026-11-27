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

/** What the swipe to the RIGHT of a card does: pin it, or unpin a pinned one. The core makes pin and archive exclusive. */
enum class PinAction {
    Pin,
    Unpin,
    ;

    /** The action that cancels this one (the "Undo" of the feedback banner). */
    val inverse: PinAction get() = if (this == Pin) Unpin else Pin
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
     * The action of the swipe to the left on [item], or null when the card has none. A pinned card does not archive by swipe (the
     * user wanted it on top; it swipes to the right to be unpinned, and the menu offers "Archive"), nor does an item that cannot be
     * archived. Everywhere else the swipe archives, except on an archived card (Archives view, or a search) where it un-archives.
     */
    fun swipeAction(item: TomiItem, localDeviceId: String): ArchiveAction? = when {
        item.pinned -> null
        !canArchive(item, localDeviceId) -> null
        else -> menuAction(item)
    }

    /**
     * The action of the swipe to the RIGHT on [item], or null when the card has none: "Unpin" on a pinned card (which swipes, to the
     * right only, to be unpinned), "Pin" on any other. Not on an archived card (Archives view, or a search): the core makes pin and
     * archive exclusive, so "Undo" could not bring it back where it was; its menu still offers "Pin".
     */
    fun pinSwipeAction(item: TomiItem): PinAction? = when {
        item.isShownAsArchived() -> null
        item.pinned -> PinAction.Unpin
        else -> PinAction.Pin
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
