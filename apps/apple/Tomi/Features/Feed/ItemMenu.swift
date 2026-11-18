import SwiftUI
import TomiDesign
import TomiKit
import UIKit

/// The "⋮" menu of a card (and its long-press menu), in the order of Android: pin, archive, copy, copy the link, open,
/// share, details, then the two deletions ("Supprimer partout" is destructive and asks for confirmation).
struct ItemMenu: View {
    let item: TomiItem
    let model: FeedViewModel
    let onDetails: () -> Void
    @Binding var confirmDelete: Bool

    private var revealed: Bool { model.isRevealed(item) }

    var body: some View {
        Button { model.togglePin(item) } label: {
            Label(L10n.text(item.pinned ? "action_unpin" : "action_pin"), systemImage: TomiIcon.pin.systemName)
        }
        if ArchiveSwipe.canArchive(item, localDeviceId: model.localDeviceId) {
            let unarchive = ArchiveSwipe.menuAction(item) == .unarchive
            Button { model.applyArchiveAction(item) } label: {
                Label(
                    L10n.text(unarchive ? "action_unarchive" : "action_archive"),
                    systemImage: (unarchive ? TomiIcon.unarchive : TomiIcon.archive).systemName
                )
            }
        }
        if item.isReadable && (item.text != nil || item.url != nil) {
            let onlyLink = item.text == nil
            Button { model.copy(item) } label: {
                Label(L10n.text(onlyLink ? "action_copy_link" : "action_copy"), systemImage: TomiIcon.copy.systemName)
            }
        }
        if item.text != nil && item.url != nil && !item.secret {
            Button { model.copyLink(item) } label: {
                Label(L10n.text("action_copy_link"), systemImage: TomiIcon.link.systemName)
            }
        }
        if let url = item.url.flatMap(URL.init(string:)), !item.secret || revealed, LinkExtractor.isValidHTTPURL(url.absoluteString) {
            Button {
                model.onCardOpened(item)
                UIApplication.shared.open(url)
            } label: {
                Label(L10n.text("action_open"), systemImage: TomiIcon.open.systemName)
            }
        }
        if ItemSharing.canShare(item) {
            Button { model.share(item) } label: {
                Label(L10n.text("action_share"), systemImage: TomiIcon.share.systemName)
            }
        }
        // The files of the item: one, its actions here; several, one submenu each.
        if item.isReadable && !item.attachments.isEmpty {
            if item.attachments.count == 1 {
                Section { FileActions(item: item, index: 0, model: model) }
            } else {
                Menu {
                    // A hidden item tells nothing of its files but their number, even shown: a menu is drawn by the
                    // system, outside the layer that captures leave out.
                    let named = !item.secret
                    ForEach(Array(item.attachments.enumerated()), id: \.offset) { index, attachment in
                        Menu(named ? attachment.name : L10n.text("ios_file_number", index + 1)) {
                            FileActions(item: item, index: index, model: model)
                        }
                    }
                } label: {
                    Label(L10n.text("ios_files_menu"), systemImage: TomiIcon.file.systemName)
                }
            }
        }
        Button(action: onDetails) {
            Label(L10n.text("action_details"), systemImage: "info.circle")
        }
        Divider()
        Button { model.deleteLocal(item) } label: {
            Label(L10n.text("action_delete_local"), systemImage: TomiIcon.delete.systemName)
        }
        Button(role: .destructive) { confirmDelete = true } label: {
            Label(L10n.text("action_delete_everywhere"), systemImage: TomiIcon.delete.systemName)
        }
    }
}

extension View {
    /// "Supprimer partout ?" with the message of a hidden item when it is one (Android `DeleteEverywhereDialog`).
    func deleteEverywhereConfirmation(isPresented: Binding<Bool>, item: TomiItem, model: FeedViewModel) -> some View {
        alert(L10n.text("dialog_delete_everywhere_title"), isPresented: isPresented) {
            Button(L10n.text("action_cancel"), role: .cancel) {}
            Button(L10n.text("action_delete_everywhere"), role: .destructive) { model.deleteEverywhere(item) }
        } message: {
            Text(L10n.text(item.secret ? "dialog_delete_everywhere_secret" : "dialog_delete_everywhere_message"))
        }
    }
}

/// What can be done with one file of an item: open it (when the caller can), share it (a hidden item asks the gate),
/// save it to Photos (images) or to Files. Never Photos nor Files for a hidden item (no lasting clear copy).
struct FileActions: View {
    let item: TomiItem
    let index: Int
    let model: FeedViewModel
    var onOpen: (() -> Void)?

    var body: some View {
        let attachment = item.attachments[index]
        if let onOpen {
            Button(action: onOpen) {
                Label(L10n.text("action_open"), systemImage: TomiIcon.open.systemName)
            }
        }
        Button { model.shareAttachment(item, index: index) } label: {
            Label(L10n.text(item.attachments.count == 1 ? "ios_share_file" : "action_share"), systemImage: TomiIcon.share.systemName)
        }
        if !item.secret {
            if attachment.isImage {
                Button { model.saveImage(item, attachmentIndex: index) } label: {
                    Label(L10n.text("viewer_save"), systemImage: "photo.badge.arrow.down")
                }
            }
            Button { model.saveAttachmentToFiles(item, index: index) } label: {
                Label(L10n.text("ios_save_to_files"), systemImage: "folder")
            }
        }
    }
}
