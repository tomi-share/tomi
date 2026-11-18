package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.DeliveryStatus

/** What the footer line of a card says about the delivery of the item. */
object CardFooterLogic {

    /** Every device has the item. */
    fun isFullyDelivered(status: DeliveryStatus): Boolean = status is DeliveryStatus.Delivered && status.delivered == status.total

    /**
     * Whether the delivery state (icon and label) is shown. A complete delivery is the normal case and says nothing new on a
     * feed card, so it is left out there; the other states (partial, queued, sent, failed, expired) stay. The detail screen
     * (the [expanded] card) keeps "Livré à tous", so the confirmation remains reachable. A received item has no delivery line.
     */
    fun showsDeliveryStatus(status: DeliveryStatus, expanded: Boolean): Boolean = when {
        status == DeliveryStatus.Received -> false
        isFullyDelivered(status) -> expanded
        else -> true
    }
}
