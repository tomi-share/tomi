// Port of apps/android/app/src/main/java/eu/shaffe/tomi/core/DeliveryStatus.kt.

/// Summary of an item's delivery state, computed from the per-device deliveries.
public enum DeliveryStatus: Hashable, Sendable {
    /// Item received from another device.
    case received
    /// At least one recipient has not been reached because the item is still in the local queue.
    case queued
    /// Accepted by the relay, nobody has fetched it yet.
    case sent
    /// `delivered` of `total` devices have the item.
    case delivered(delivered: Int, total: Int)
    /// The item expired before `deviceNames` could fetch it; `delivered` devices did get it.
    case expiredBeforeDelivery(delivered: Int, deviceNames: [String])
    case failed

    public static func of(_ item: TomiItem, localDeviceId: String) -> DeliveryStatus {
        if !item.isOutgoing(localDeviceId: localDeviceId) { return .received }
        let deliveries = item.deliveries
        if deliveries.isEmpty { return .sent }
        if deliveries.contains(where: { $0.state == .failed }) { return .failed }
        if deliveries.contains(where: { $0.state == .queued }) { return .queued }
        let delivered = deliveries.filter { $0.state == .delivered }.count
        let expired = deliveries.filter { $0.state == .expiredBeforeDelivery }
        if !expired.isEmpty { return .expiredBeforeDelivery(delivered: delivered, deviceNames: expired.map(\.deviceName)) }
        if delivered > 0 { return .delivered(delivered: delivered, total: deliveries.count) }
        return .sent
    }
}
