package eu.shaffe.tomi.core

/** Summary of an item's sync/delivery state, computed from the per-device deliveries. */
sealed interface DeliveryStatus {
    /** Item received from another device. */
    data object Received : DeliveryStatus

    /** At least one recipient has not been reached because the item is still in the local queue. */
    data object Queued : DeliveryStatus

    /** Accepted by the relay, nobody has fetched it yet. */
    data object Sent : DeliveryStatus

    /** [delivered] of [total] devices have the item. */
    data class Delivered(val delivered: Int, val total: Int) : DeliveryStatus

    /** The item expired before [deviceNames] could fetch it; [delivered] devices did get it. */
    data class ExpiredBeforeDelivery(val delivered: Int, val deviceNames: List<String>) : DeliveryStatus

    data object Failed : DeliveryStatus

    companion object {
        fun of(item: TomiItem, localDeviceId: String): DeliveryStatus {
            if (!item.isOutgoing(localDeviceId)) return Received
            val deliveries = item.deliveries
            if (deliveries.isEmpty()) return Sent
            if (deliveries.any { it.state == DeliveryState.Failed }) return Failed
            if (deliveries.any { it.state == DeliveryState.Queued }) return Queued
            val delivered = deliveries.count { it.state == DeliveryState.Delivered }
            val expired = deliveries.filter { it.state == DeliveryState.ExpiredBeforeDelivery }
            return when {
                expired.isNotEmpty() -> ExpiredBeforeDelivery(delivered, expired.map { it.deviceName })
                delivered > 0 -> Delivered(delivered, deliveries.size)
                else -> Sent
            }
        }
    }
}
