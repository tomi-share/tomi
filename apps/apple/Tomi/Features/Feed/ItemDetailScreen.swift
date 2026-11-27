import QuickLook
import SwiftUI
import TomiDesign
import TomiKit

/// Detail of an item (Android `ItemDetailScreen`): the card unfolded, the hint of a hidden item, the metadata (type,
/// expiry, pinned, status), and the floating toolbar (copy, pin, share, archive, and more: open, copy link, delete).
/// Goes back by itself when the item is deleted while on screen.
struct ItemDetailScreen: View {
    let itemId: String
    @Bindable var model: FeedViewModel
    @Environment(\.dismiss) private var dismiss
    @State private var existed = false
    @State private var confirmDeleteEverywhere = false

    var body: some View {
        ActionBarScaffold { layout in
            ZStack(alignment: .bottom) {
                TomiColor.bg.ignoresSafeArea()
                if let item = model.item(id: itemId) {
                    ScrollView {
                        VStack(alignment: .leading, spacing: TomiSpace.s4) {
                            ItemCard(item: item, model: model, expanded: true)
                            if item.secret {
                                Text(L10n.text(model.isRevealed(item) ? "detail_revealed_hint" : "detail_masked_hint"))
                                    .tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                            }
                            metadata(item)
                        }
                        .padding(.horizontal, TomiSpace.screen)
                        // Below the top edge of iPhone Duo, the first card starts on the line of the status content.
                        .padding(.top, max(TomiSpace.s2, layout.topPadding(rowHeight: 2 * TomiSpace.s5)))
                        .padding(.bottom, layout.placement.isSide ? TomiSpace.s4 : 112)
                        .frame(maxWidth: TomiLayout.readableWidth)
                        .frame(maxWidth: .infinity)
                    }
                } else if model.loaded {
                    Text(L10n.text("detail_not_found"))
                        .tomiText(.body).foregroundStyle(TomiColor.textMuted)
                        .padding(TomiSpace.s6)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                FeedbackHost(center: model.feedback, bottomPadding: layout.placement.isSide ? TomiSpace.s4 : 96)
            }
        } bar: { axis in
            if let item = model.item(id: itemId) {
                toolbar(item, axis: axis)
                    .deleteConfirmation(isPresented: $confirmDeleteEverywhere, item: item, model: model)
            }
        }
        .navigationTitle(Text("ios_detail_title"))
        .inlineNavigationTitle()
        // "Delete the file everywhere" (rows of a batch) is offered only when the relay can carry it.
        .task { await model.refreshAttachmentRemovalSupport() }
        // The item may arrive after the screen (loading): it counts as seen as soon as it is there, and the screen goes
        // back by itself only once it has been seen and then disappears.
        .onChange(of: model.item(id: itemId) != nil, initial: true) { _, present in
            if present, !existed, let item = model.item(id: itemId) {
                existed = true
                model.onCardOpened(item)
            } else if !present && existed {
                dismiss()
            }
        }
    }

    // MARK: Metadata

    private func metadata(_ item: TomiItem) -> some View {
        VStack(spacing: 0) {
            row("meta_type", typeLabel(item))
            Divider().overlay(TomiColor.border)
            row("meta_sent_at", formatPreciseDateTime(item.createdAt))
            Divider().overlay(TomiColor.border)
            row("meta_expiry", expiryText(item))
            Divider().overlay(TomiColor.border)
            row("meta_pinned", L10n.text(item.pinned ? "meta_yes" : "meta_no"), icon: item.pinned ? .pinned : nil)
            Divider().overlay(TomiColor.border)
            row("meta_status", statusText(item))
        }
        .background(TomiColor.surface, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        .overlay(RoundedRectangle(cornerRadius: TomiRadius.medium).strokeBorder(TomiColor.border))
    }

    private func row(_ key: LocalizedStringKey, _ value: String, icon: TomiIcon? = nil) -> some View {
        HStack {
            Text(key).tomiText(.label).foregroundStyle(TomiColor.textMuted)
            Spacer()
            if let icon { icon.image.font(.system(size: 12)).foregroundStyle(TomiColor.textMuted) }
            Text(value).tomiText(.body).foregroundStyle(TomiColor.text).multilineTextAlignment(.trailing)
        }
        .padding(.horizontal, TomiSpace.s4)
        .frame(minHeight: 48)
    }

    /// For a hidden item the type is told without revealing anything but the shape (file, link or text).
    private func typeLabel(_ item: TomiItem) -> String {
        let kind: ItemKind
        if item.secret {
            kind = !item.attachments.isEmpty ? (item.attachments.allSatisfy(\.isImage) ? .image : .file) : (item.url != nil ? .link : .text)
        } else {
            kind = item.kind
        }
        switch kind {
        case .unreadable: return L10n.text("kind_unreadable")
        case .secret: return L10n.text("kind_secret")
        case .image: return L10n.text("kind_image")
        case .file: return L10n.text("kind_file")
        case .link: return L10n.text("kind_link")
        case .richText: return L10n.text("kind_rich_text")
        case .text: return L10n.text("kind_text")
        }
    }

    private func expiryText(_ item: TomiItem) -> String {
        if item.readPolicy == .afterFirstRead { return L10n.text("meta_after_read") }
        if let date = item.expiresAt { return formatDateTime(date) }
        return L10n.text("meta_never")
    }

    private func statusText(_ item: TomiItem) -> String {
        L10n.delivery(deliveryLabel(DeliveryStatus.of(item, localDeviceId: model.localDeviceId), isOnline: model.isOnline))
    }

    // MARK: Toolbar

    private func toolbar(_ item: TomiItem, axis: Axis) -> some View {
        FloatingActionBar(axis: axis) {
            FloatingBarButton(icon: item.pinned ? .pinned : .pin, label: item.pinned ? "action_unpin" : "action_pin", selected: item.pinned) {
                model.togglePin(item)
            }
            // Sharing is the main way out of the app: always one tap away, ahead of archive and delete (as on Android).
            if ItemSharing.canShare(item) {
                FloatingBarButton(icon: .share, label: "action_share") { model.share(item) }
            }
            if ArchiveSwipe.canArchive(item, localDeviceId: model.localDeviceId) {
                let unarchive = ArchiveSwipe.menuAction(item) == .unarchive
                FloatingBarButton(icon: unarchive ? .unarchive : .archive, label: unarchive ? "action_unarchive" : "action_archive") {
                    model.applyArchiveAction(item)
                    // Back to the list, where the banner "Archivé · Annuler" is shown (same as Android and Windows).
                    dismiss()
                }
            }
            // Always there: it holds the deletion.
            Menu {
                let url = item.url.flatMap(URL.init(string:)).flatMap { url in
                    (!item.secret || model.isRevealed(item)) && LinkExtractor.isValidHTTPURL(url.absoluteString) ? url : nil
                }
                let copyLink = item.text != nil && item.url != nil && !item.secret
                if let url {
                    Button {
                        model.onCardOpened(item)
                        SystemActions.open(url)
                    } label: { Label(L10n.text("action_open"), systemImage: TomiIcon.open.systemName) }
                }
                if copyLink {
                    Button { model.copyLink(item) } label: { Label(L10n.text("action_copy_link"), systemImage: TomiIcon.link.systemName) }
                }
                if url != nil || copyLink { Divider() }
                // One entry, as on Android: the dialog asks where (this device only, or everywhere).
                Button(role: .destructive) { confirmDeleteEverywhere = true } label: {
                    Label(L10n.text("action_delete_ellipsis"), systemImage: TomiIcon.delete.systemName)
                }
            } label: { FloatingBarIcon(icon: .more) }
            .accessibilityLabel(Text("action_more"))
        } primary: {
            if item.isReadable && (item.text != nil || item.url != nil) {
                FloatingPrimaryButton(icon: .copy, label: "action_copy") { model.copy(item) }
            }
        }
    }
}

#if DEBUG
#Preview("Détail Markdown") {
    NavigationStack { ItemDetailScreen(itemId: PreviewFixtures.markdown.id, model: PreviewFixtures.model()) }
}

#Preview("Détail masqué") {
    NavigationStack { ItemDetailScreen(itemId: PreviewFixtures.secret.id, model: PreviewFixtures.model()) }
}
#endif
