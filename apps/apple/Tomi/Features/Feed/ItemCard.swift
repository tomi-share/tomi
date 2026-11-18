import SwiftUI
import TomiDesign
import TomiKit
import UIKit

/// One item of the feed. Port of `ItemCard.kt`: header (kind, source and time, pin, archive, unread dot, copy, menu),
/// body by kind (unreadable, hidden closed or revealed, text in the chosen mode, link, attachments), "Lire la suite",
/// footer (delivery and expiry). `expanded` is the detail screen: no limits, no tap action, no menu.
struct ItemCard: View {
    let item: TomiItem
    let model: FeedViewModel
    var expanded = false
    var onOpenDetail: () -> Void = {}

    @State private var overflow = CardOverflow()
    @State private var mode: RenderMode?
    @State private var confirmDeleteEverywhere = false
    /// A line of the revealed secret (mono 14 + 4 pt spacing), following Dynamic Type like its font.
    @ScaledMetric(relativeTo: .body) private var secretLineHeight: CGFloat = 20

    private var revealed: Bool { model.isRevealed(item) }
    private var outgoing: Bool { item.isOutgoing(localDeviceId: model.localDeviceId) }
    private var unread: Bool { !item.read && !outgoing }
    /// "Lire la suite" is on the card: the "N autres fichiers" button of the attachments gives way to a caption.
    private var showsReadMore: Bool { !expanded && CardTap.showsReadMore(item, revealed: revealed, overflow: overflow) }

    var body: some View {
        TomiCard(secretAccent: item.secret) {
            VStack(alignment: .leading, spacing: TomiSpace.s2) {
                header
                content
                if showsReadMore {
                    Button(action: onOpenDetail) {
                        HStack(spacing: TomiSpace.s1) {
                            Text("action_read_more")
                            TomiIcon.readMore.image.font(.system(size: 12, weight: .semibold))
                        }
                        .tomiText(.label)
                        .foregroundStyle(TomiColor.brand)
                    }
                    .buttonStyle(.plain)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                }
                footer
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { if !expanded { handleTap() } }
        .modifier(CardMenu(item: item, enabled: !expanded) {
            ItemMenu(item: item, model: model, onDetails: onOpenDetail, confirmDelete: $confirmDeleteEverywhere)
        })
        .accessibilityAction(named: Text(accessibilityTapLabel)) { handleTap() }
        .deleteEverywhereConfirmation(isPresented: $confirmDeleteEverywhere, item: item, model: model)
    }

    // MARK: Tap

    private func handleTap() {
        let action = CardTap.actionFor(item, revealed: revealed, overflow: overflow)
        model.onCardOpened(item)
        switch action {
        case .openLink:
            if let url = item.url.flatMap(URL.init(string:)) { UIApplication.shared.open(url) }
        case .reveal: model.reveal(item)
        case .hide: model.hide(item.id)
        case .openImage: model.openViewer(item, attachmentIndex: 0)
        case .openCodeFile: model.openCode(item, attachmentIndex: 0)
        case .openAttachment: model.openAttachment(item, index: 0)
        case .openDetail: onOpenDetail()
        case .none: break
        }
    }

    private var accessibilityTapLabel: String {
        switch CardTap.actionFor(item, revealed: revealed, overflow: overflow) {
        case .openLink: L10n.text("action_open")
        case .reveal: L10n.text("action_reveal")
        case .hide: L10n.text("action_hide")
        case .openImage: L10n.text("action_view_image")
        case .openCodeFile: L10n.text("action_view_code")
        case .openAttachment: L10n.text("action_open")
        case .openDetail, .none: L10n.text("action_details")
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: TomiSpace.s2) {
            kindIcon.image
                .font(.system(size: TomiIconSize.medium - 4))
                .foregroundStyle(item.secret ? TomiColor.secret : TomiColor.textMuted)
                .frame(width: TomiIconSize.medium)
            Text(sourceAndTime)
                .tomiText(.caption)
                .foregroundStyle(TomiColor.textMuted)
                .lineLimit(1)
            if item.pinned {
                TomiIcon.pinned.image.font(.system(size: 11)).foregroundStyle(TomiColor.textMuted)
                    .accessibilityLabel(Text("pinned_badge"))
            }
            if item.isShownAsArchived {
                TomiIcon.archive.image.font(.system(size: 11)).foregroundStyle(TomiColor.textMuted)
                    .accessibilityLabel(Text("archived_badge"))
            }
            if !item.read && !outgoing {
                Circle().fill(TomiColor.brand).frame(width: 8, height: 8)
                    .accessibilityLabel(Text("unread_badge"))
            }
            Spacer(minLength: 0)
            if !expanded && CardTap.showsCopyButton(item) {
                iconButton(.copy, label: "action_copy") { model.copy(item, mode: mode) }
            }
            if !expanded {
                Menu {
                    ItemMenu(item: item, model: model, onDetails: onOpenDetail, confirmDelete: $confirmDeleteEverywhere)
                } label: {
                    TomiIcon.more.image
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(TomiColor.textMuted)
                        .frame(width: 40, height: 40)
                        .contentShape(Rectangle())
                }
                .accessibilityLabel(Text("action_more"))
            }
        }
        .frame(minHeight: expanded ? 24 : 40)
    }

    private func iconButton(_ icon: TomiIcon, label: LocalizedStringKey, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            icon.image
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(TomiColor.textMuted)
                .frame(width: 40, height: 40)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(label))
    }

    private var kindIcon: TomiIcon {
        switch item.kind {
        case .unreadable: .error
        case .secret: .masked
        case .image: .image
        case .file: .file
        case .link: .link
        case .richText: .markdown
        case .text: .text
        }
    }

    private var sourceAndTime: String {
        let source = outgoing ? L10n.text("source_this_phone") : item.sourceDevice.name
        return "\(source) · \(formatTimestamp(item.createdAt, now: Date()).frenchText)"
    }

    // MARK: Body

    @ViewBuilder
    private var content: some View {
        if !item.isReadable {
            unreadableBody
        } else if item.secret {
            secretBody
        } else {
            normalBody
        }
    }

    private var unreadableBody: some View {
        let key: String = {
            if case .unreadable(.updateRequired) = item.content { return "unreadable_update_required" }
            return "unreadable_decryption_failed"
        }()
        return Text(L10n.text(key)).tomiText(.body).foregroundStyle(TomiColor.textMuted)
    }

    @ViewBuilder
    private var secretBody: some View {
        Text("label_masked").tomiText(.label).foregroundStyle(TomiColor.secret)
        if !revealed {
            HStack {
                Text("preview_secret_masked").font(.custom(TomiTokens.FontFamily.mono, size: 15)).foregroundStyle(TomiColor.secret)
                Spacer()
                Button { model.reveal(item) } label: {
                    Label { Text("action_reveal") } icon: { TomiIcon.reveal.image }
                        .tomiText(.label)
                        .foregroundStyle(TomiColor.text)
                        .frame(minHeight: 48)
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, TomiSpace.s3)
            .background(TomiColor.secretContainer, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
            // File names, sizes, thumbnails and types of a hidden item are hidden too; only the count is shown.
            if !item.attachments.isEmpty { HiddenFilesCount(count: item.attachments.count) }
        } else {
            // What the hidden item holds is drawn in the layer that captures leave out (screenshots, recordings).
            if let title = item.title {
                limitedText(title, lines: 2, style: .subtitle, cut: \.title, protected: true)
            }
            if let text = item.text?.plain {
                // Six lines at most in a card, plus the block's padding.
                ClippedContent(maxHeight: expanded ? nil : 6 * secretLineHeight + 2 * TomiSpace.s3, clipped: bodyCut) {
                    Text(text)
                        .tomiMono(size: 14, slashedZero: true)
                        .lineSpacing(4)
                        .foregroundStyle(TomiColor.text)
                        .padding(TomiSpace.s3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
                        .captureProtected()
                }
                // Never selectable: a hidden item is copied through the gated, sensitive copy only.
            }
            if let url = item.url { urlLine(url, selectable: false).captureProtected() }
            if !item.attachments.isEmpty {
                // The block is already surfaceAlt: tiles go to surface. No thumbnail, so no mosaic either.
                AttachmentList(
                    item: item, model: model, expanded: expanded, showThumbnails: false,
                    onAltBackground: true, moreAsButton: !showsReadMore, onOpenDetail: detailOpener()
                )
                .captureProtected()
            }
            HStack {
                if let text = item.text?.plain {
                    // UTF-16 length, as Android counts.
                    Text(L10n.plural("characters_count", text.utf16.count)).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
                Spacer()
                Button { model.hide(item.id) } label: {
                    Label { Text("action_hide") } icon: { TomiIcon.masked.image }
                        .tomiText(.label).foregroundStyle(TomiColor.text)
                }
                .buttonStyle(.plain)
            }
        }
    }

    @ViewBuilder
    private var normalBody: some View {
        if let title = item.title {
            limitedText(title, lines: 2, style: .body, weight: item.read || outgoing ? .medium : .semibold, cut: \.title)
                .textSelection(.enabled)
        }
        if let text = item.text {
            let modes = RenderModes.available(text)
            let current = mode ?? RenderModes.resolve(text, preferred: model.preferredRenderMode)
            if modes.count > 1 {
                Picker("", selection: Binding(get: { current }, set: { mode = $0; model.setRenderMode($0) })) {
                    ForEach(modes, id: \.self) { m in Text(LocalizedStringKey(m.labelKey)).tag(m) }
                }
                .pickerStyle(.segmented)
                .frame(maxWidth: 240)
            }
            textBody(text, mode: current)
        }
        if let url = item.url { urlLine(url) }
        if !item.attachments.isEmpty {
            AttachmentList(
                item: item, model: model, expanded: expanded, showThumbnails: true,
                // Unread without a title: the names carry the weight the title would.
                emphasizeNames: unread && item.title == nil, moreAsButton: !showsReadMore, onOpenDetail: detailOpener()
            )
        }
    }

    /// "+N" of a mosaic / "N autres fichiers": the detail, and a tap that opens the card counts as opening it. Nil in the detail.
    private func detailOpener() -> (() -> Void)? {
        if expanded { return nil }
        return {
            model.onCardOpened(item)
            onOpenDetail()
        }
    }

    @ViewBuilder
    private func textBody(_ text: RichText, mode: RenderMode) -> some View {
        switch (mode, RenderModes.previewKind(text)) {
        case (.source, _):
            ClippedContent(maxHeight: expanded ? nil : 8 * 19, clipped: bodyCut) {
                Text(text.markdownSource ?? text.plain)
                    .tomiMono(size: 13).lineSpacing(6)
                    .foregroundStyle(TomiColor.text)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
        case (.preview, .markdown):
            ClippedContent(maxHeight: expanded ? nil : 220, clipped: bodyCut) {
                if let markdown = text.markdownSource, let view = MarkdownView(markdown: markdown) {
                    view.textSelection(.enabled)
                } else {
                    plainText(text.plain)
                }
            }
        case (.preview, .html):
            HTMLBlock(html: text.html ?? "", expanded: expanded, clipped: bodyCut)
        case (.preview, .plain):
            limitedText(text.plain, lines: 4, style: .body, cut: \.body).textSelection(.enabled)
        }
    }

    private func plainText(_ value: String) -> some View {
        Text(value).tomiText(.body).foregroundStyle(TomiColor.text).frame(maxWidth: .infinity, alignment: .leading)
    }

    private func limitedText(
        _ value: String, lines: Int, style: TomiTextStyle, weight: Font.Weight? = nil, cut: WritableKeyPath<CardOverflow, Bool>,
        protected: Bool = false
    ) -> some View {
        ClippedContent(maxHeight: expanded ? nil : CGFloat(lines) * style.lineHeight, clipped: Binding(
            get: { overflow[keyPath: cut] }, set: { overflow[keyPath: cut] = $0 }
        )) {
            Text(value)
                .tomiText(style)
                .fontWeight(weight)
                .foregroundStyle(TomiColor.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                // Inside the clip: the measure of the text stays the card's.
                .captureProtected(protected)
        }
    }

    private var bodyCut: Binding<Bool> {
        Binding(get: { overflow.body }, set: { overflow.body = $0 })
    }

    /// The link of the item. Never selectable on a hidden item: it is copied only through the gated, sensitive copy.
    @ViewBuilder
    private func urlLine(_ url: String, selectable: Bool = true) -> some View {
        let line = Text(url)
            .tomiMono(size: 13)
            .foregroundStyle(TomiColor.brand)
            .lineLimit(expanded ? nil : 1)
            .truncationMode(.middle)
        if selectable { line.textSelection(.enabled) } else { line.textSelection(.disabled) }
    }

    // MARK: Footer

    @ViewBuilder
    private var footer: some View {
        if item.isReadable {
            if item.expiresAt != nil, item.readPolicy != .afterFirstRead {
                // The countdown ("expire dans…") refreshes by itself; TimelineView stops when the card is off screen.
                TimelineView(.periodic(from: .now, by: 15)) { context in footerLine(now: context.date) }
            } else {
                footerLine(now: Date())
            }
        }
    }

    @ViewBuilder
    private func footerLine(now: Date) -> some View {
        let status = DeliveryStatus.of(item, localDeviceId: model.localDeviceId)
        let parts = footerParts(status, now: now)
        if !parts.text.isEmpty {
            HStack(spacing: TomiSpace.s1) {
                if let icon = parts.icon { icon.image.font(.system(size: 12)).foregroundStyle(parts.tint) }
                Text(parts.text).tomiText(.caption).foregroundStyle(parts.textTint)
            }
        }
    }

    private func footerParts(_ status: DeliveryStatus, now: Date) -> (icon: TomiIcon?, tint: Color, textTint: Color, text: String) {
        var pieces: [String] = []
        var icon: TomiIcon?
        var tint = TomiColor.textMuted
        var textTint = TomiColor.textMuted
        // "Livré à tous" is left out of the feed cards, kept on the detail screen (`CardFooterLogic`).
        if CardFooterLogic.showsDeliveryStatus(status, expanded: expanded) {
            pieces.append(L10n.delivery(deliveryLabel(status, isOnline: model.isOnline)))
            switch status {
            case .queued:
                icon = .pending
                tint = TomiColor.secondary
                textTint = TomiColor.secondary
            case .failed:
                icon = .error
                tint = TomiColor.error
            case let .delivered(delivered, total):
                icon = .delivered
                tint = delivered == total ? TomiColor.success : TomiColor.textMuted
            case .sent:
                icon = .sent
            case .expiredBeforeDelivery:
                icon = .expired
            case .received:
                break
            }
        }
        if item.readPolicy == .afterFirstRead {
            pieces.append(L10n.text("expires_after_read"))
        } else if let remaining = formatRemaining(item.expiresAt, now: now) {
            pieces.append(L10n.text("expires_in", L10n.plural(remaining.resourceKey, remaining.count)))
        }
        return (icon, tint, textTint, pieces.joined(separator: " · "))
    }
}

/// HTML preview with its measured height (220 pt in a card, 520 pt in the detail, like Android).
private struct HTMLBlock: View {
    let html: String
    let expanded: Bool
    @Binding var clipped: Bool
    @State private var height: CGFloat = 60

    var body: some View {
        let limit: CGFloat = expanded ? 520 : 220
        SandboxedHTMLView(html: html, horizontalScroll: expanded, contentHeight: $height)
            .frame(height: min(max(height, 24), limit))
            .onChange(of: height) { _, new in
                let cut = ClipDetection.isClipped(contentHeight: Int(new), visibleHeight: Int(limit))
                if cut != clipped { clipped = cut }
            }
    }
}

extension L10n {
    /// The delivery status line, with typed arguments (a count is a number, device names stay a string even when a
    /// name is only digits).
    static func delivery(_ label: DeliveryLabel) -> String {
        switch label {
        case let .deliveredPartial(delivered, total): text(label.resourceKey, delivered, total)
        case let .expired(delivered, devices): text(label.resourceKey, delivered, devices as NSString)
        default: text(label.resourceKey)
        }
    }
}

#if DEBUG
#Preview("Cartes") {
    let model = PreviewFixtures.model()
    ScrollView {
        VStack(spacing: TomiSpace.s3) {
            ForEach(PreviewFixtures.all) { item in
                ItemCard(item: item, model: model)
            }
        }
        .padding(TomiSpace.screen)
    }
    .background(TomiColor.bg)
}
#endif

/// The long-press menu of a card. A hidden item gets a neutral preview: the system copies the card for it, outside
/// the layer that captures leave out, and the item may be shown in it.
private struct CardMenu<Menu: View>: ViewModifier {
    let item: TomiItem
    let enabled: Bool
    @ViewBuilder let menu: () -> Menu

    func body(content: Content) -> some View {
        if !enabled {
            content
        } else if item.secret {
            content.contextMenu(menuItems: menu) {
                Label(L10n.text("label_masked"), systemImage: TomiIcon.masked.systemName)
                    .tomiText(.label)
                    .foregroundStyle(TomiColor.secret)
                    .padding(TomiSpace.s4)
            }
        } else {
            content.contextMenu(menuItems: menu)
        }
    }
}
