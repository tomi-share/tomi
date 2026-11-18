package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.AttachmentRemoval
import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.TomiItem

/**
 * What the UI decides around "remove ONE attachment of an item" (image viewer, file rows of the detail), without Android types so that
 * it is unit tested. The removal itself is [eu.shaffe.tomi.core.TomiCore.deleteAttachment], by SLOT.
 */
object AttachmentRemovals {
    /**
     * True while every delivery of the item is still queued (it has not left the device): the core refuses a removal then
     * ("everywhere" would send the blob anyway), so the UI does not offer one for the few seconds the item stays queued.
     */
    fun isQueued(item: TomiItem): Boolean =
        item.deliveries.isNotEmpty() && item.deliveries.all { it.state == DeliveryState.Queued }

    /** Whether an attachment of [item] may be removed from here: a readable item that has left the device. */
    fun canRemove(item: TomiItem): Boolean = item.isReadable && item.attachments.isNotEmpty() && !isQueued(item)

    /**
     * Whether the rows of the files of [item] get a "Supprimer ce fichier" action. An item with a single attachment has none: that file
     * IS the item, and "Supprimer" of the card / detail menu already says so (removing it would delete the item anyway).
     */
    fun canRemoveFromBatch(item: TomiItem): Boolean = canRemove(item) && item.attachments.size >= 2

    /** The two scopes, "this device" first. "Everywhere" only when the relay can carry the removal (older relay: not offered). */
    fun scopes(relaySupportsRemoval: Boolean): List<RemovalScope> =
        if (relaySupportsRemoval) listOf(RemovalScope.ThisDevice, RemovalScope.Everywhere) else listOf(RemovalScope.ThisDevice)

    /** The message after the core answered: a last-thing removal is an item deletion with the same scope. */
    fun feedback(removal: AttachmentRemoval, image: Boolean, everywhere: Boolean): FeedEvent = when (removal) {
        AttachmentRemoval.ItemDeleted -> if (everywhere) FeedEvent.DeletedEverywhere else FeedEvent.DeletedLocal
        AttachmentRemoval.AttachmentRemoved -> FeedEvent.AttachmentDeleted(image, everywhere)
    }
}

enum class RemovalScope { ThisDevice, Everywhere }
