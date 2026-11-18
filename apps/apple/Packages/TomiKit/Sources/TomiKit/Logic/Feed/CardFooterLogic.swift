import Foundation

// Port of apps/android/app/src/main/java/eu/shaffe/tomi/ui/feed/CardFooterLogic.kt.

/// What the footer line of a card says about the delivery of the item.
public enum CardFooterLogic {
    /// Every device has the item.
    public static func isFullyDelivered(_ status: DeliveryStatus) -> Bool {
        if case let .delivered(delivered, total) = status { return delivered == total }
        return false
    }

    /// Whether the delivery state (icon and label) is shown. A complete delivery is the normal case and says nothing new
    /// on a feed card, so it is left out there; the other states (partial, queued, sent, failed, expired) stay. The
    /// detail screen (`expanded`) keeps "Livré à tous", so the confirmation remains reachable. A received item has no
    /// delivery line.
    public static func showsDeliveryStatus(_ status: DeliveryStatus, expanded: Bool) -> Bool {
        if status == .received { return false }
        if isFullyDelivered(status) { return expanded }
        return true
    }
}
