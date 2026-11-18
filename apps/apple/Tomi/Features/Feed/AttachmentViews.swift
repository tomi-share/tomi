import CoreText
import SwiftUI
import TomiDesign
import TomiKit
import UIKit

// Attachments of a card (feed and detail). Port of `AttachmentViews.kt`:
// one attachment = ONE object with ONE touch target, a tile (type badge, name, size, chevron) or an image frame (thumbnail
// + caption). The rules are in `AttachmentLogic` (TomiKit). What a tap does: image viewer, code screen or file viewer
// (`FeedViewModel.openAttachmentRow`); every attachment opens something on iOS, so every tile has its chevron.

private enum AttachmentMetrics {
    static let tileMinHeight: CGFloat = 56
    static let badge: CGFloat = 40
    static let feedImageMaxHeight: CGFloat = 180
    static let detailImageMaxHeight: CGFloat = 200
    static let mosaicGap: CGFloat = 4
    /// Veil of the "+N" cell of a mosaic (black at 55 %, like the chips of the image viewer).
    static let mosaicVeil = Color.black.opacity(0.55)
}

/// The attachments of an item: three at most in a card ("N autres fichiers" for the rest, a mosaic for a batch of images),
/// all in the detail; the preview each file carries when the item is not hidden.
struct AttachmentList: View {
    let item: TomiItem
    let model: FeedViewModel
    let expanded: Bool
    let showThumbnails: Bool
    /// Unread item without a title: the names are semi-bold, as the title of an unread card is.
    var emphasizeNames = false
    /// The list sits in a block that is already `surfaceAlt` (revealed hidden item): tiles go to `surface`.
    var onAltBackground = false
    /// "N autres fichiers ›" button to the detail; false when "Lire la suite" already takes that place (a caption is shown instead).
    var moreAsButton = true
    /// Opens the detail ("+N" of a mosaic, "N autres fichiers", an attachment that opens nothing); nil in the detail itself.
    var onOpenDetail: (() -> Void)?

    var body: some View {
        let plan = AttachmentLogic.plan(item.attachments, expanded: expanded, showThumbnails: showThumbnails)
        VStack(alignment: .leading, spacing: TomiSpace.s2) {
            switch plan {
            case let .mosaic(shown, more):
                mosaic(shown: shown, more: more)
                if let summary {
                    Text(summary).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                }
            case let .items(entries, more):
                // "4 fichiers · 4,9 Mo" above the attachments of the detail.
                if expanded, let summary {
                    Text(summary)
                        .tomiText(.caption)
                        .fontWeight(.medium)
                        .foregroundStyle(TomiColor.textMuted)
                        .padding(.top, TomiSpace.s1)
                }
                ForEach(entries, id: \.index) { entry in
                    row(entry)
                }
                if more > 0 {
                    if moreAsButton, let onOpenDetail {
                        MoreFilesButton(count: more, action: onOpenDetail)
                    } else {
                        Text(L10n.plural("more_files_caption", more)).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
                    }
                }
            }
        }
    }

    // MARK: Rows

    /// One tile or one image frame. The touch is the row's own (inside the card, which has its own tap): each attachment
    /// opens by its type, and a long press in the detail has the file actions.
    @ViewBuilder
    private func row(_ entry: AttachmentLogic.Entry) -> some View {
        let index = entry.index
        let attachment = item.attachments[index]
        let action = AttachmentRowAction.of(attachment, imageOpenable: true, codeOpenable: true)
        let image = showThumbnails ? thumbnail(index) : nil
        Group {
            if entry.look == .frame, let image {
                ImageFrame(
                    attachment: attachment, image: image, maxHeight: frameMaxHeight, full: expanded, emphasized: emphasizeNames
                )
            } else {
                FileTile(
                    attachment: attachment, thumbnail: image, tonal: action != AttachmentRowAction.none,
                    full: expanded, emphasized: emphasizeNames, onAltBackground: onAltBackground
                )
            }
        }
        .contentShape(Rectangle())
        .onTapGesture { open(index) }
        // A long press in the detail; on a card the press stays the card's, whose menu has the same actions.
        .modifier(FileRowMenu(item: item, index: index, model: model, expanded: expanded) { open(index) })
        .accessibilityAddTraits(.isButton)
        .accessibilityHint(Text(clickLabel(for: action)))
    }

    /// Height cap of the picture of an image frame. Detail: a lone image whole (the sender already crops it to 3:4 at most),
    /// several ones cropped at 200 pt.
    private var frameMaxHeight: CGFloat? {
        if !expanded { return AttachmentMetrics.feedImageMaxHeight }
        return item.attachments.count == 1 ? nil : AttachmentMetrics.detailImageMaxHeight
    }

    /// What the tap does, as a VoiceOver hint (Android `onClickLabel`).
    private func clickLabel(for action: AttachmentRowAction) -> String {
        switch action {
        case .openImage: L10n.text("action_view_image")
        case .openCodeFile: L10n.text("action_view_code")
        case .none: L10n.text("action_open")
        }
    }

    /// An image or a code file opens in Tomi (a hidden one asks the gate there); another file in the file viewer, which
    /// never draws a hidden item's file.
    private func open(_ index: Int) {
        if !model.openAttachmentRow(item, index: index) { onOpenDetail?() }
    }

    // MARK: Mosaic

    /// Images only (2 or more): 2 square cells, or a large one on the left (2/3) and two small ones; "+N" on the last cell,
    /// which opens the detail. Each other cell opens the image viewer at its index.
    private func mosaic(shown: Int, more: Int) -> some View {
        MosaicLayout(cells: shown) {
            ForEach(0..<shown, id: \.self) { index in
                mosaicCell(index: index, plus: index == shown - 1 ? more : 0)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: TomiRadius.medium))
    }

    private func mosaicCell(index: Int, plus: Int) -> some View {
        MosaicCell(image: thumbnail(index), more: plus)
            .contentShape(Rectangle())
            .onTapGesture {
                if plus > 0 { onOpenDetail?() } else { open(index) }
            }
            // The veil and its "+N" are one element: "3 autres fichiers"; an image reads as its name.
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Text(verbatim: plus > 0 ? L10n.plural("more_files", plus) : item.attachments[index].name))
            .accessibilityAddTraits(.isButton)
            .accessibilityHint(Text(L10n.text(plus > 0 ? "action_details" : "action_view_image")))
    }

    // MARK: Summary and thumbnails

    /// "4 fichiers · 4,9 Mo" / "5 images · 11,4 Mo", from 2 attachments on.
    private var summary: String? {
        let attachments = item.attachments
        guard AttachmentLogic.showsSummary(attachments) else { return nil }
        let key = AttachmentLogic.isAllImages(attachments) ? "attachments_summary_images" : "attachments_summary_files"
        return L10n.plural(key, attachments.count, formatFileSize(AttachmentLogic.totalSize(attachments)))
    }

    /// Only the thumbnails the plan draws are asked for (a batch of 20 photos must not cost 20 decodes on every redraw).
    private func thumbnail(_ index: Int) -> UIImage? {
        item.attachments[index].thumbnail.flatMap { ThumbnailCache.image(itemId: item.id, index: index, data: $0) }
    }
}

/// Closed hidden item: only the count, with a paperclip. No name, no size, no thumbnail, no type (golden rule 3).
struct HiddenFilesCount: View {
    let count: Int

    var body: some View {
        HStack(spacing: TomiSpace.s1) {
            TomiIcon.attach.image
                .font(.system(size: 13))
                .frame(width: TomiIconSize.small, height: TomiIconSize.small)
                .foregroundStyle(TomiColor.textMuted)
                .accessibilityHidden(true)
            Text(L10n.plural("secret_files_hidden", count)).tomiText(.caption).foregroundStyle(TomiColor.textMuted)
        }
    }
}

/// "N autres fichiers ›": text button aligned to the right like "Lire la suite", opens the detail.
private struct MoreFilesButton: View {
    let count: Int
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: TomiSpace.s1) {
                Text(L10n.plural("more_files", count))
                TomiIcon.readMore.image.font(.system(size: 12, weight: .semibold))
            }
            .tomiText(.label)
            .foregroundStyle(TomiColor.brand)
            .frame(minHeight: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

// MARK: - Name

/// A file name in the proportional font (Inter 14/20 medium, semi-bold when `emphasized`). `maxLines` nil: the whole name,
/// which wraps even without a space (detail). Otherwise the name is cut BEFORE its extension so that it fits in `maxLines`
/// lines ("Courrier banque fonds de rési….pdf"), measured in the width the text really has (`AttachmentLogic.fitFileName`).
private struct FileNameText: View {
    let name: String
    let emphasized: Bool
    let maxLines: Int?
    @State private var width: CGFloat = 0
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        if let maxLines {
            // The tail truncation is only the safety net (and the first pass, before the width is known): the shown text has
            // been measured to fit.
            Text(verbatim: FileNameFit.fit(name, lines: maxLines, width: width, emphasized: emphasized, dynamicTypeSize: dynamicTypeSize))
                .tomiText(.label)
                .fontWeight(emphasized ? .semibold : .medium)
                .foregroundStyle(TomiColor.text)
                .lineLimit(maxLines)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(GeometryReader { geo in
                    Color.clear.preference(key: FileNameWidthKey.self, value: geo.size.width)
                })
                .onPreferenceChange(FileNameWidthKey.self) { width = $0 }
        } else {
            Text(verbatim: name)
                .tomiText(.label)
                .fontWeight(emphasized ? .semibold : .medium)
                .foregroundStyle(TomiColor.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

private struct FileNameWidthKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

/// Measures a file name with TextKit, in the font and the width of the text view, to cut it before its extension.
@MainActor
private enum FileNameFit {
    private static var cache: [String: String] = [:]

    static func fit(_ name: String, lines: Int, width: CGFloat, emphasized: Bool, dynamicTypeSize: DynamicTypeSize) -> String {
        // No width yet (first pass) or an absurd one: the whole name.
        guard width > 2, width < 10_000 else { return name }
        let key = "\(lines)|\(width)|\(emphasized)|\(dynamicTypeSize)|\(name)"
        if let hit = cache[key] { return hit }
        let font = uiFont(emphasized: emphasized, dynamicTypeSize: dynamicTypeSize)
        // One point less than the text has: a rounding must never push the extension to a line more.
        let available = width - 1
        let shown = AttachmentLogic.fitFileName(name) { candidate in
            lineCount(candidate, font: font, width: available) <= lines
        }
        if cache.count > 400 { cache.removeAll() }
        cache[key] = shown
        return shown
    }

    /// The font of `.tomiText(.label)` with the weight of the name, scaled for Dynamic Type like `Font.custom(relativeTo:)`.
    private static func uiFont(emphasized: Bool, dynamicTypeSize: DynamicTypeSize) -> UIFont {
        TomiFonts.register()
        let traits = UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(dynamicTypeSize))
        let size = UIFontMetrics(forTextStyle: .subheadline).scaledValue(for: TomiTextStyle.label.size, compatibleWith: traits)
        let weight: UIFont.Weight = emphasized ? .semibold : .medium
        // Inter is a variable font: the weight trait alone may resolve to the default instance (Regular, narrower than what
        // is drawn), so the `wght` axis is set too (500 / 600). If the drawn text is lighter, the measure errs on the safe
        // side (a cut a little early, never the extension pushed to a third line).
        let variation = UIFontDescriptor.AttributeName(rawValue: kCTFontVariationAttribute as String)
        let descriptor = UIFontDescriptor(fontAttributes: [
            .family: TomiTokens.FontFamily.ui,
            .traits: [UIFontDescriptor.TraitKey.weight: weight],
            variation: [NSNumber(value: 0x7767_6874 as UInt32): NSNumber(value: emphasized ? 600 : 500)],
        ])
        return UIFont(descriptor: descriptor, size: size)
    }

    /// The number of lines `text` takes in `width` (word wrap, a word longer than the line breaks by characters).
    private static func lineCount(_ text: String, font: UIFont, width: CGFloat) -> Int {
        let storage = NSTextStorage(string: text, attributes: [.font: font])
        let container = NSTextContainer(size: CGSize(width: width, height: .greatestFiniteMagnitude))
        container.lineFragmentPadding = 0
        let layout = NSLayoutManager()
        layout.addTextContainer(container)
        storage.addLayoutManager(layout)
        _ = layout.glyphRange(for: container)
        var lines = 0
        var glyph = 0
        let total = layout.numberOfGlyphs
        while glyph < total {
            var range = NSRange(location: 0, length: 0)
            _ = layout.lineFragmentRect(forGlyphAt: glyph, effectiveRange: &range)
            glyph = max(NSMaxRange(range), glyph + 1)
            lines += 1
        }
        return lines
    }
}

private struct SizeText: View {
    let size: Int64

    var body: some View {
        Text(formatFileSize(size)).tomiText(.caption).foregroundStyle(TomiColor.textMuted).lineLimit(1)
    }
}

private struct RowChevron: View {
    var body: some View {
        TomiIcon.readMore.image
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(TomiColor.textMuted)
            .padding(.trailing, TomiSpace.s1)
            .accessibilityHidden(true)
    }
}

// MARK: - Tile

/// File tile: type badge, name (2 lines in the feed, whole in the detail), size below, and the chevron on the right.
private struct FileTile: View {
    let attachment: Attachment
    let thumbnail: UIImage?
    let tonal: Bool
    let full: Bool
    let emphasized: Bool
    let onAltBackground: Bool

    var body: some View {
        HStack(spacing: TomiSpace.s3) {
            TypeBadge(attachment: attachment, thumbnail: thumbnail, tonal: tonal, onAltBackground: onAltBackground)
            VStack(alignment: .leading, spacing: 0) {
                FileNameText(name: attachment.name, emphasized: emphasized, maxLines: full ? nil : 2)
                SizeText(size: attachment.size)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            RowChevron()
        }
        .padding(TomiSpace.s2)
        .frame(maxWidth: .infinity, minHeight: AttachmentMetrics.tileMinHeight, alignment: .leading)
        .background(onAltBackground ? TomiColor.surface : TomiColor.surfaceAlt, in: RoundedRectangle(cornerRadius: TomiRadius.medium))
        // Reads as one element ("name, size"); the badge and the chevron are decoration; the whole name, not the cut one.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "\(attachment.name), \(formatFileSize(attachment.size))"))
    }
}

/// Type badge 40 x 40: the mini-thumbnail of a file that has one; else the upper-case extension (4 letters at most) or an
/// icon. Tonal (brand container, mono) when the file opens in Tomi's own viewer or code screen, neutral (surface + border)
/// for any other file.
private struct TypeBadge: View {
    let attachment: Attachment
    let thumbnail: UIImage?
    let tonal: Bool
    let onAltBackground: Bool

    var body: some View {
        content
            .frame(width: AttachmentMetrics.badge, height: AttachmentMetrics.badge)
            .clipShape(RoundedRectangle(cornerRadius: TomiRadius.small))
            .accessibilityHidden(true)
    }

    @ViewBuilder
    private var content: some View {
        if let thumbnail {
            Image(uiImage: thumbnail)
                .resizable()
                .scaledToFill()
                .frame(width: AttachmentMetrics.badge, height: AttachmentMetrics.badge)
                .clipped()
                .overlay {
                    // iOS draws a play button on the preview of a video (README, "Aperçu des fichiers").
                    if attachment.mime.hasPrefix("video/") {
                        Image(systemName: "play.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(.white)
                            .frame(width: 20, height: 20)
                            .background(Color.black.opacity(0.45), in: Circle())
                    }
                }
        } else {
            label
                .frame(width: AttachmentMetrics.badge, height: AttachmentMetrics.badge)
                .background(fill)
                .overlay {
                    if !tonal {
                        RoundedRectangle(cornerRadius: TomiRadius.small).strokeBorder(TomiColor.border, lineWidth: TomiBorder.width)
                    }
                }
        }
    }

    private var fill: Color {
        if tonal { return TomiColor.brandContainer }
        return onAltBackground ? TomiColor.surfaceAlt : TomiColor.surface
    }

    private var tint: Color { tonal ? TomiColor.brand : TomiColor.text }

    /// An image has no extension label: its icon (the name tells the rest).
    private var extensionLabel: String? {
        attachment.isImage ? nil : AttachmentLogic.badgeExtension(attachment.name)
    }

    private var icon: TomiIcon {
        if attachment.isImage { return .image }
        return CodeFiles.isCodeFile(attachment) ? .code : .file
    }

    @ViewBuilder
    private var label: some View {
        if let extensionLabel {
            Group {
                if tonal {
                    Text(extensionLabel).tracking(0.5).tomiMono(size: 12)
                } else {
                    Text(extensionLabel).tracking(0.5).tomiText(.caption)
                }
            }
            .fontWeight(.semibold)
            .foregroundStyle(tint)
            .lineLimit(1)
            .minimumScaleFactor(0.6)
        } else {
            icon.image.font(.system(size: 18)).foregroundStyle(tint)
        }
    }
}

// MARK: - Image frame

/// Image: the thumbnail and its caption (name, size, chevron) in one frame, one touch target.
private struct ImageFrame: View {
    let attachment: Attachment
    let image: UIImage
    /// Height cap of the picture (cropped at the centre beyond); nil: the picture whole.
    let maxHeight: CGFloat?
    let full: Bool
    let emphasized: Bool

    var body: some View {
        VStack(spacing: 0) {
            CroppedImageLayout(ratio: ratio, maxHeight: maxHeight) {
                Image(uiImage: image).resizable().scaledToFill()
            }
            .clipped()
            HStack(spacing: TomiSpace.s2) {
                FileNameText(name: attachment.name, emphasized: emphasized, maxLines: full ? nil : 1)
                SizeText(size: attachment.size).fixedSize(horizontal: true, vertical: false)
                RowChevron()
            }
            .padding(.leading, TomiSpace.s3)
            .padding(.top, TomiSpace.s2)
            .padding(.trailing, TomiSpace.s2)
            .padding(.bottom, TomiSpace.s2)
            .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
        }
        .frame(maxWidth: .infinity)
        .background(TomiColor.surfaceAlt)
        .clipShape(RoundedRectangle(cornerRadius: TomiRadius.medium))
        // The caption says the name; the frame is read as one element.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "\(attachment.name), \(formatFileSize(attachment.size))"))
    }

    private var ratio: CGFloat {
        max(image.size.width / max(image.size.height, 1), 0.01)
    }
}

/// The picture of a frame: full width, the height of its ratio capped at `maxHeight`; its single child fills that box
/// (`scaledToFill`) and what goes beyond is cropped at the centre by the caller's `clipped()`.
private struct CroppedImageLayout: Layout {
    let ratio: CGFloat
    let maxHeight: CGFloat?

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        var width: CGFloat = 300
        if let proposed = proposal.width, proposed.isFinite { width = proposed }
        var height = width / ratio
        if let maxHeight { height = min(height, maxHeight) }
        return CGSize(width: width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for subview in subviews {
            subview.place(
                at: CGPoint(x: bounds.midX, y: bounds.midY), anchor: .center,
                proposal: ProposedViewSize(width: bounds.width, height: bounds.height)
            )
        }
    }
}

// MARK: - Mosaic

/// The cells of a mosaic of images: 2 cells of 2.2 : 1 together, or a large one on the left (2/3) and two small ones (5 : 3).
private struct MosaicLayout: Layout {
    let cells: Int

    private var ratio: CGFloat { cells == 2 ? 2.2 : 5.0 / 3.0 }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        var width: CGFloat = 300
        if let proposed = proposal.width, proposed.isFinite { width = proposed }
        return CGSize(width: width, height: width / ratio)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let gap = AttachmentMetrics.mosaicGap
        var rects: [CGRect] = []
        if cells == 2 {
            let width = (bounds.width - gap) / 2
            rects = [
                CGRect(x: bounds.minX, y: bounds.minY, width: width, height: bounds.height),
                CGRect(x: bounds.minX + width + gap, y: bounds.minY, width: width, height: bounds.height),
            ]
        } else {
            let large = (bounds.width - gap) * 2 / 3
            let small = bounds.width - gap - large
            let height = (bounds.height - gap) / 2
            rects = [
                CGRect(x: bounds.minX, y: bounds.minY, width: large, height: bounds.height),
                CGRect(x: bounds.minX + large + gap, y: bounds.minY, width: small, height: height),
                CGRect(x: bounds.minX + large + gap, y: bounds.minY + height + gap, width: small, height: height),
            ]
        }
        for (subview, rect) in zip(subviews, rects) {
            subview.place(
                at: rect.origin, anchor: .topLeading,
                proposal: ProposedViewSize(width: rect.width, height: rect.height)
            )
        }
    }
}

private struct MosaicCell: View {
    let image: UIImage?
    /// "+N" on the veil of the last cell when above 0.
    let more: Int

    var body: some View {
        Color.clear
            .overlay {
                if let image {
                    Image(uiImage: image).resizable().scaledToFill()
                } else {
                    TomiColor.surfaceAlt
                }
            }
            .overlay {
                if more > 0 {
                    ZStack {
                        AttachmentMetrics.mosaicVeil
                        Text(verbatim: "+\(more)").tomiText(.title).foregroundStyle(.white)
                    }
                }
            }
            .clipped()
    }
}

// MARK: - Thumbnails

/// Decoded thumbnails, by item and attachment (they are decoded once, not on every redraw of a card). In memory only; a
/// hidden item never has any (`showThumbnails` is false for it).
@MainActor
private enum ThumbnailCache {
    private static let cache: NSCache<NSString, UIImage> = {
        let cache = NSCache<NSString, UIImage>()
        cache.countLimit = 200
        return cache
    }()

    static func image(itemId: String, index: Int, data: Data) -> UIImage? {
        let key = "\(itemId)#\(index)#\(data.count)" as NSString
        if let hit = cache.object(forKey: key) { return hit }
        guard let decoded = UIImage(data: data) else { return nil }
        cache.setObject(decoded, forKey: key)
        return decoded
    }
}

// MARK: - File actions

/// The long-press menu of a file row, in the detail only (on a card the press stays the card's). A hidden item's row
/// gets a neutral preview: the system copies the row for it, outside the layer that captures leave out.
private struct FileRowMenu: ViewModifier {
    let item: TomiItem
    let index: Int
    let model: FeedViewModel
    let expanded: Bool
    let onOpen: () -> Void
    /// "Supprimer le fichier partout" waits for its confirmation, which names the file.
    @State private var confirmDeleteEverywhere = false

    /// The removal of this file only: a batch of 2 or more, readable, already out of the device.
    private var canRemove: Bool { AttachmentRemovals.canRemoveFromBatch(item) }

    func body(content: Content) -> some View {
        menu(content)
            .alert(L10n.text("dialog_delete_file_everywhere_title"), isPresented: $confirmDeleteEverywhere) {
                Button(L10n.text("action_cancel"), role: .cancel) {}
                Button(L10n.text("file_delete_everywhere"), role: .destructive) {
                    model.deleteAttachment(item, index: index, everywhere: true)
                }
            } message: {
                Text(deleteEverywhereMessage)
            }
    }

    /// A hidden item tells nothing of its file, not even its name.
    private var deleteEverywhereMessage: String {
        guard item.attachments.indices.contains(index), !item.secret else {
            return L10n.text("dialog_delete_file_everywhere_secret")
        }
        return L10n.text("dialog_delete_file_everywhere_named", item.attachments[index].name)
    }

    @ViewBuilder
    private func menu(_ content: Content) -> some View {
        if !expanded {
            content
        } else if item.secret {
            content.contextMenu {
                actions
            } preview: {
                Label(L10n.text("ios_file_number", index + 1), systemImage: TomiIcon.file.systemName)
                    .tomiText(.body)
                    .padding(TomiSpace.s4)
            }
        } else {
            content.contextMenu { actions }
        }
    }

    @ViewBuilder
    private var actions: some View {
        FileActions(
            item: item, index: index, model: model, onOpen: onOpen,
            onDeleteLocal: canRemove ? { model.deleteAttachment(item, index: index, everywhere: false) } : nil,
            onDeleteEverywhere: canRemove && model.attachmentRemovalEverywhere ? { confirmDeleteEverywhere = true } : nil
        )
    }
}
