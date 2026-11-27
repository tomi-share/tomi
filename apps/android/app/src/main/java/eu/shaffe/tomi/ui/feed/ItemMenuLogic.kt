package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.TomiItem

/** One action of the menu of a card (the "⋮" and the long press), in the order they are shown. */
enum class ItemMenuEntry {
    /** "Copier" (the text, else the link): only when the card has no copy icon of its own, i.e. a hidden item (behind the gate). */
    Copy,

    /** "Ouvrir" the link. */
    OpenLink,

    /** "Copier le lien" of an item that also has a text (the icon and "Copier" copy the text). */
    CopyLink,

    Share,

    /** "Épingler" / "Désépingler". */
    Pin,

    /** "Archiver" / "Désarchiver". */
    Archive,

    /** The detail screen: full content, type, expiry, status. Reachable for every item. */
    Details,

    /** "Supprimer…": ONE entry, which asks where (this device only, or everywhere). Always last, after a divider. */
    Delete,
}

/**
 * Which entries the menu of a card has, without Android types so that it is unit tested. The menu stays short: what the card already
 * does with one tap (the copy icon, the tap that opens a link, the swipe that archives) is not repeated first, and the two deletions
 * are one entry. The detail screen has the full set in its toolbar.
 */
object ItemMenuLogic {

    /**
     * [hasInlineCopy]: the card shows its own copy icon ([CardTap.showsCopyButton]). [canArchive]: [ArchiveSwipe.canArchive].
     * [canShare]: there is something to share ([eu.shaffe.tomi.share.ItemSharing] / [eu.shaffe.tomi.share.FileSharing]).
     */
    fun entries(
        item: TomiItem,
        revealed: Boolean,
        canArchive: Boolean,
        hasInlineCopy: Boolean,
        canShare: Boolean,
    ): List<ItemMenuEntry> = buildList {
        if ((item.text != null || item.url != null) && !hasInlineCopy) add(ItemMenuEntry.Copy)
        if (item.url != null && (!item.secret || revealed)) add(ItemMenuEntry.OpenLink)
        if (item.url != null && item.text != null && !item.secret) add(ItemMenuEntry.CopyLink)
        if (canShare) add(ItemMenuEntry.Share)
        add(ItemMenuEntry.Pin)
        if (canArchive) add(ItemMenuEntry.Archive)
        add(ItemMenuEntry.Details)
        add(ItemMenuEntry.Delete)
    }
}
