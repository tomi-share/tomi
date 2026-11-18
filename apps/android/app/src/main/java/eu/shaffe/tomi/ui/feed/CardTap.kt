package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.share.LinkExtractor
import eu.shaffe.tomi.ui.code.CodeFiles

/** What a tap on a feed card does. Pure data: the screen performs it (see `ItemCard`). */
enum class CardTapAction {
    /** Open the link in the browser (same action as "Ouvrir" in the menu). */
    OpenLink,

    /** Show a hidden item (same action and same biometric gate as the "Afficher" button). */
    Reveal,

    /** Hide an item that is shown in clear again (same as the "Masquer" button). */
    Hide,

    /** Open the single (non-image, non-code) attachment with the system viewer; falls back to [OpenDetail] when it is not available. */
    OpenAttachment,

    /** Show the single code file in Tomi's own code screen (colours, search; behind the biometric gate for a hidden item). */
    OpenCodeFile,

    /** Show the single image full screen in Tomi's own viewer (zoom, EXIF orientation; behind the biometric gate for a hidden item). */
    OpenImage,

    /** Open the detail screen (`item/{id}`): the card is truncated, or nothing more natural exists. */
    OpenDetail,

    /** Nothing useful to do (the card already shows everything and has no natural action). */
    None,
}

/**
 * Performs a tap with the callbacks of the card. [CardActions.onCardOpened] always runs first, whatever the action,
 * so that tapping an unread item marks it read as before (the view model skips hidden, read, own and unreadable items).
 */
internal fun CardActions.handleTap(tap: CardTapAction, singleSlot: Int = 0) {
    onCardOpened()
    when (tap) {
        CardTapAction.OpenLink -> onOpenLink()
        CardTapAction.Reveal -> onReveal()
        CardTapAction.Hide -> onHide()
        CardTapAction.OpenAttachment -> onOpenAttachment()
        // Only chosen for an item whose single attachment is a code file, see [CardTap.actionFor]: [singleSlot] is ITS slot (not 0
        // once the item lost an attachment).
        CardTapAction.OpenCodeFile -> onOpenCodeFile(singleSlot)
        // Only chosen for an item whose single attachment is an image, see [CardTap.actionFor].
        CardTapAction.OpenImage -> onOpenImage(singleSlot)
        CardTapAction.OpenDetail -> onOpenDetail()
        CardTapAction.None -> Unit
    }
}

/**
 * What the card measured about its own rendering: whether a part of the content is cut by the card limits
 * (title 2 lines, text 4 lines, revealed secret 6 lines, Markdown / HTML 220 dp).
 */
data class CardOverflow(
    /** The title is ellipsized. */
    val title: Boolean = false,
    /** The text (plain, Markdown or HTML, or the revealed secret) is cut. */
    val body: Boolean = false,
) {
    val any: Boolean get() = title || body
}

/**
 * Tap rules of the feed cards. The detail screen is only the target of a tap when it brings something: reading a
 * truncated content. Everything else is available on the card itself, so a tap does the natural action of the type.
 *
 * | Item                                   | Tap                                                       |
 * |----------------------------------------|-----------------------------------------------------------|
 * | Link (valid http/https URL)            | [CardTapAction.OpenLink]                                  |
 * | Link whose text is cut, or bad URL     | [CardTapAction.OpenDetail]                                |
 * | Hidden item                            | [CardTapAction.Reveal]                                    |
 * | Revealed item, content complete        | [CardTapAction.Hide]                                      |
 * | Revealed item, content cut             | [CardTapAction.OpenDetail]                                |
 * | Short text / rich text, nothing cut    | [CardTapAction.None] (marks it read; copy = the icon)     |
 * | Text, rich text or title cut           | [CardTapAction.OpenDetail] (also "Lire la suite")         |
 * | Image, exactly one                     | [CardTapAction.OpenImage] (internal full-screen viewer)   |
 * | Code file, exactly one                 | [CardTapAction.OpenCodeFile] (internal code screen)       |
 * | Other file, exactly one                | [CardTapAction.OpenAttachment] (system viewer)            |
 * | Files or images, several               | [CardTapAction.OpenDetail] (each row opens its viewer)    |
 * | Unreadable                             | [CardTapAction.None] (the card already explains why)      |
 * | Title only, nothing cut                | [CardTapAction.None]                                      |
 */
object CardTap {

    fun actionFor(item: TomiItem, revealed: Boolean, overflow: CardOverflow = CardOverflow()): CardTapAction {
        if (!item.isReadable) return CardTapAction.None
        if (item.secret) {
            return when {
                !revealed -> CardTapAction.Reveal
                overflow.any -> CardTapAction.OpenDetail
                else -> CardTapAction.Hide
            }
        }
        if (needsDetail(item, overflow)) return CardTapAction.OpenDetail
        return when (item.kind()) {
            ItemKind.Link -> if (item.url?.let(LinkExtractor::isValidHttpUrl) == true) CardTapAction.OpenLink else CardTapAction.OpenDetail
            ItemKind.Image -> if (item.attachments.size == 1) CardTapAction.OpenImage else CardTapAction.OpenDetail
            ItemKind.File -> when {
                item.attachments.size != 1 -> CardTapAction.OpenDetail
                CodeFiles.isCodeFile(item.attachments.single()) -> CardTapAction.OpenCodeFile
                else -> CardTapAction.OpenAttachment
            }
            // A tap on a text never copies: the text is selectable and the card has a copy icon. It only marks the item read.
            ItemKind.Text, ItemKind.RichText -> CardTapAction.None
            // Handled above (not readable / secret); kept explicit so that a new kind forces a decision here.
            ItemKind.Secret, ItemKind.Unreadable -> CardTapAction.None
        }
    }

    /** The explicit copy icon of a text card (same action as "Copier" of the menu). Never for a hidden item, whose copy is gated. */
    fun showsCopyButton(item: TomiItem): Boolean = item.isReadable && !item.secret && item.text != null

    /**
     * Whether the card shows a "Lire la suite" affordance. True exactly when the tap opens the detail because the
     * content is cut (not for the fallbacks "bad URL" or "several attachments").
     */
    fun showsReadMore(item: TomiItem, revealed: Boolean, overflow: CardOverflow): Boolean =
        item.isReadable && (!item.secret || revealed) && needsDetail(item, overflow)

    /**
     * A cut content has to be read in the detail. Exception: a link keeps its natural action even when its title
     * is ellipsized (a page title is routinely longer than two lines and the URL is what matters); only a cut text
     * body sends a link to the detail.
     */
    private fun needsDetail(item: TomiItem, overflow: CardOverflow): Boolean =
        if (item.kind() == ItemKind.Link) overflow.body else overflow.any
}

/** What a tap on one attachment row of a card does. Each row routes by its own type, so a mixed item (images + code) works row by row. */
enum class AttachmentRowAction {
    /** Tomi's internal image viewer. */
    OpenImage,

    /** Tomi's code screen. */
    OpenCodeFile,

    /** Inert row (other files are opened with a tap on the card when alone, or from the detail). */
    None;

    companion object {
        /** [imageOpenable] / [codeOpenable]: the screen gave a callback for that type. */
        fun of(attachment: Attachment, imageOpenable: Boolean, codeOpenable: Boolean): AttachmentRowAction = when {
            attachment.isImage -> if (imageOpenable) OpenImage else None
            CodeFiles.isCodeFile(attachment) -> if (codeOpenable) OpenCodeFile else None
            else -> None
        }
    }
}
