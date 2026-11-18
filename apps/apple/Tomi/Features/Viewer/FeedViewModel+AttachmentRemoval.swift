import Foundation
import TomiKit

/// Removing ONE attachment of an item (the image on screen in the viewer, a file row of the detail). Android
/// `FeedViewModel.deleteAttachment`. The item stays; the core deletes it, with the same scope, when nothing else is left in it.
extension FeedViewModel {
    /// Re-reads the capability (a field of the core, not the network). Called when a screen that offers the removal opens.
    func refreshAttachmentRemovalSupport() async {
        attachmentRemovalEverywhere = await core.supportsAttachmentRemoval()
    }

    /// Removes attachment `index` of `item` (its position in `item.attachments`; the core is asked for its SLOT), from this
    /// device only or `everywhere`. The message comes once the core did it; a failure is told and nothing is announced.
    func deleteAttachment(_ item: TomiItem, index: Int, everywhere: Bool) {
        guard AttachmentRemovals.canRemove(item), item.attachments.indices.contains(index) else { return }
        let attachment = item.attachments[index]
        let slot = item.attachments.slot(at: index)
        Task {
            do {
                let removal = try await core.deleteAttachment(itemId: item.id, slot: slot, everywhere: everywhere)
                if removal == .itemDeleted { hide(item.id) }
                feedback.show(L10n.text(AttachmentRemovals.feedbackKey(removal, image: attachment.isImage, everywhere: everywhere)))
            } catch {
                feedback.show(L10n.text("snackbar_action_failed"))
            }
        }
    }
}
