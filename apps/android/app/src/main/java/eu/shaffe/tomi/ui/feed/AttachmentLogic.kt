package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import java.util.Locale

/**
 * Pure rules of the attachment tiles of a card (feed and detail). No Android, no Compose: unit-tested.
 * The views are in `AttachmentViews.kt`.
 */
object AttachmentLogic {
    /** Attachments shown by a feed card before "N autres fichiers" (the detail shows them all). */
    const val MAX_VISIBLE = 3

    /** Longest extension that is still shown as such in the badge of a tile ("PDF", "WEBP"); "drawio" gets an icon instead. */
    const val MAX_BADGE_EXTENSION = 4

    /** An ellipsis that fits is worth more than a long "extension" made of the end of a sentence. */
    private const val MAX_EXTENSION_LENGTH = 10

    /** Result of [splitExtension]: `radical` + `extension` (the dot included, e.g. ".pdf") put back together give the name. */
    data class NameParts(val radical: String, val extension: String)

    /**
     * Splits `Courrier banque.pdf` into "Courrier banque" and ".pdf". No extension (empty [NameParts.extension]) for a name without a
     * dot, with a leading dot only (".gitignore"), ending with a dot, or whose "extension" has a space or is too long to be one.
     */
    fun splitExtension(name: String): NameParts {
        val dot = name.lastIndexOf('.')
        if (dot <= 0) return NameParts(name, "")
        val extension = name.substring(dot)
        if (extension.length == 1 || extension.length > MAX_EXTENSION_LENGTH || extension.any { it.isWhitespace() }) {
            return NameParts(name, "")
        }
        return NameParts(name.substring(0, dot), extension)
    }

    /** The upper-case extension shown in the badge of a tile ("PDF", "KT"), or null when there is none or it is longer than 4 letters. */
    fun badgeExtension(name: String): String? {
        val extension = splitExtension(name).extension.removePrefix(".")
        if (extension.isEmpty() || extension.length > MAX_BADGE_EXTENSION) return null
        if (!extension.all { it.isLetterOrDigit() }) return null
        return extension.uppercase(Locale.ROOT)
    }

    /**
     * The name cut BEFORE its extension so that it fits [maxLines] lines: "Courrier banque fonds de rési….pdf". [fits] says whether a
     * candidate text fits (in the views: the measure of the text in the available width). It is assumed monotonic (a shorter radical
     * fits whenever a longer one does), which is what lets the search be a dichotomy. A name that already fits is returned as is.
     * Without an extension, the end of the name is cut ("Courrier banque rési…").
     */
    fun fitFileName(name: String, fits: (String) -> Boolean): String {
        if (fits(name)) return name
        val (radical, extension) = splitExtension(name)
        fun candidate(length: Int): String = radical.take(safeCut(radical, length)).trimEnd() + "…" + extension
        // The longest radical length that still fits; 0 (just "…" + extension) is the floor, even if that does not fit either.
        var low = 0
        var high = radical.length - 1
        while (low < high) {
            val middle = (low + high + 1) / 2
            if (fits(candidate(middle))) low = middle else high = middle - 1
        }
        return candidate(low)
    }

    /** Never cuts between the two halves of a surrogate pair (an emoji in a file name). */
    private fun safeCut(text: String, length: Int): Int =
        if (length in 1 until text.length && Character.isHighSurrogate(text[length - 1])) length - 1 else length

    /** How one attachment is drawn when it is not in a mosaic. */
    enum class Look {
        /** A tile: type badge, name, size, action. */
        Tile,

        /** An image frame: the thumbnail and its caption in a single object. */
        Frame,
    }

    class Entry(val index: Int, val look: Look)

    sealed interface Plan {
        /** Only images (2 or more), none of them without a thumbnail: [shown] cells (2 or 3), "+[more]" on the last when [more] > 0. */
        class Mosaic(val shown: Int, val more: Int) : Plan

        /** Tiles and frames in order, then "[more] autres fichiers" when [more] > 0. */
        class Items(val entries: List<Entry>, val more: Int) : Plan
    }

    /**
     * What a card shows for its attachments.
     * - Detail ([expanded]): everything, in order; an image with a thumbnail is a frame, anything else a tile.
     * - Feed: [MAX_VISIBLE] attachments at most. Images only (2 or more, with thumbnails): a mosaic. A single image with a thumbnail: a
     *   frame. Everything else: tiles (an image becomes a tile with a mini-thumbnail).
     * - [showThumbnails] false (a revealed hidden item): nothing but tiles, no thumbnail anywhere.
     */
    fun plan(attachments: List<Attachment>, expanded: Boolean, showThumbnails: Boolean): Plan {
        fun framed(attachment: Attachment) = showThumbnails && attachment.isImage && attachment.thumbnail != null
        if (expanded) {
            return Plan.Items(attachments.mapIndexed { index, a -> Entry(index, if (framed(a)) Look.Frame else Look.Tile) }, more = 0)
        }
        val visible = attachments.take(MAX_VISIBLE)
        val more = attachments.size - visible.size
        if (attachments.size >= 2 && visible.all(::framed) && attachments.all { it.isImage }) {
            return Plan.Mosaic(shown = visible.size, more = more)
        }
        val single = attachments.size == 1 && framed(attachments[0])
        return Plan.Items(
            visible.mapIndexed { index, a -> Entry(index, if (single && framed(a)) Look.Frame else Look.Tile) },
            more = more,
        )
    }

    /** "4 fichiers · 4,9 Mo" is shown above 2 attachments or more of the detail; "5 images · …" when they are all images. */
    fun showsSummary(attachments: List<Attachment>): Boolean = attachments.size >= 2

    fun isAllImages(attachments: List<Attachment>): Boolean = attachments.isNotEmpty() && attachments.all { it.isImage }

    fun totalSize(attachments: List<Attachment>): Long = attachments.sumOf { it.size }
}
