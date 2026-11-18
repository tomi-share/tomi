import Foundation

/// What the UI decides around "remove ONE attachment of an item" (image viewer, file rows of the detail), without UIKit so
/// that it is unit tested. Mirror of Android `AttachmentRemovals.kt`. The removal itself is
/// `TomiCore.deleteAttachment`, by SLOT.
public enum AttachmentRemovals {
    /// True while every delivery of the item is still queued (it has not left the device): the core refuses a removal then,
    /// so the UI does not offer one for the few seconds the item stays queued.
    public static func isQueued(_ item: TomiItem) -> Bool {
        !item.deliveries.isEmpty && item.deliveries.allSatisfy { $0.state == .queued }
    }

    /// Whether an attachment of `item` may be removed from here: a readable item that has left the device.
    public static func canRemove(_ item: TomiItem) -> Bool {
        item.isReadable && !item.attachments.isEmpty && !isQueued(item)
    }

    /// Whether the rows of the files of `item` get a "Supprimer ce fichier" action. An item with a single attachment has
    /// none: that file IS the item, and "Supprimer" of the menu already says so (removing it would delete the item anyway).
    public static func canRemoveFromBatch(_ item: TomiItem) -> Bool {
        canRemove(item) && item.attachments.count >= 2
    }

    /// The key of the message once the core answered: a last-thing removal is an item deletion with the same scope.
    public static func feedbackKey(_ removal: AttachmentRemoval, image: Bool, everywhere: Bool) -> String {
        switch removal {
        case .itemDeleted:
            return everywhere ? "snackbar_deleted_everywhere" : "snackbar_deleted_local"
        case .attachmentRemoved:
            switch (image, everywhere) {
            case (true, true): return "snackbar_image_deleted_everywhere"
            case (true, false): return "snackbar_image_deleted_local"
            case (false, true): return "snackbar_file_deleted_everywhere"
            case (false, false): return "snackbar_file_deleted_local"
            }
        }
    }
}
