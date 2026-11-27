package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.share.FileDownloads
import eu.shaffe.tomi.share.FileSharing
import eu.shaffe.tomi.share.ItemSharing
import java.net.IDN

/** A button under a notification of a received item. At most three are ever shown (Android's limit). */
enum class NotificationButton { Copy, View, Save, Share }

/**
 * What a notification of a received item may show, decided here without Android types so that it can be tested. The Android side
 * (`IncomingNotifications`) only maps these decisions to strings, styles and intents.
 *
 * Hard rule (AGENTS.md, golden rule 3): the content of a concealed item is never put in a notification, and it carries no button.
 */
internal object NotificationContent {
    /** Characters of the collapsed text line (and of a link title). */
    const val MAX_BODY = 120

    /** Characters of the expanded text (`BigTextStyle`). Everything shown is also read by the notification history and a paired watch. */
    const val MAX_BIG_TEXT = 400

    /** Lines of the expanded summary of a burst. */
    const val MAX_SUMMARY_LINES = 5

    /** A thumbnail above this is not decoded for a notification (the sender makes them small; this only bounds a hostile one). */
    const val MAX_THUMBNAIL_BYTES = 512 * 1024

    /** Longest side of the picture of a notification, in pixels: enough for the expanded view, light for the system. */
    const val MAX_PICTURE_EDGE = 450

    /**
     * Whether the notification of [item] must say nothing of its content and offer no button: a hidden item, an item this version cannot
     * read (it may well be a secret), and an "after first read" item, which the first copy, open or share DELETES everywhere. The UI only
     * lets the user choose that policy for a hidden item, but the core does not require it: another client may send one that is not hidden,
     * and a tap on a preview must not consume it.
     */
    fun isConcealed(item: TomiItem): Boolean = item.kind().let { it == ItemKind.Secret || it == ItemKind.Unreadable } ||
        item.readPolicy == ReadPolicy.AfterFirstRead

    /** The text line under the title for a text item, bounded: [MAX_BODY] characters, `…` when cut. Null when there is no text. */
    fun collapsedText(item: TomiItem): String? = item.text?.plain?.let { bounded(it.trim(), MAX_BODY) }?.takeIf { it.isNotEmpty() }

    /** The expanded text of a text item, bounded: [MAX_BIG_TEXT] characters, `…` when cut. Null when there is no text. */
    fun expandedText(item: TomiItem): String? = item.text?.plain?.let { bounded(it.trim(), MAX_BIG_TEXT) }?.takeIf { it.isNotEmpty() }

    /** [text] cut to [max] characters, the last one being `…` when something was dropped. Never leaves half of a surrogate pair. */
    fun bounded(text: String, max: Int): String {
        if (text.length <= max) return text
        var end = max - 1
        if (end > 0 && Character.isHighSurrogate(text[end - 1])) end--
        return text.substring(0, end).trimEnd() + "…"
    }

    /**
     * The host of an http(s) link, in ASCII (punycode for an internationalised name), lowercase, with no user, port, path or query: the
     * path and the query may carry a token, and a look-alike letter in a name is only visible once it is punycode. Null for anything else.
     */
    fun hostOf(url: String?): String? {
        val authority = HOST_AUTHORITY.find(url?.trim() ?: return null)?.groupValues?.get(1) ?: return null
        val hostAndPort = authority.substringAfterLast('@')
        if (hostAndPort.startsWith("[")) return hostAndPort.substringBefore(']').takeIf { ']' in hostAndPort }?.let { "$it]" }
        val host = hostAndPort.substringBefore(':').trimEnd('.').lowercase()
        if (host.isEmpty()) return null
        return try {
            IDN.toASCII(host, IDN.ALLOW_UNASSIGNED)
        } catch (_: IllegalArgumentException) {
            null
        }?.take(MAX_BODY)
    }

    /**
     * What the buttons under the notification of [item] are, in order: none for a concealed item; otherwise "Copy" or "View" (a short text
     * is copied by a tap, so it offers "View") and "Share", or "Save" and "Share" for files and images. "Save" is never offered for a
     * hidden item ([FileDownloads.canDownload]), and "Share" only when there is something to share.
     */
    fun buttonsFor(item: TomiItem): List<NotificationButton> {
        if (isConcealed(item)) return emptyList()
        val share = NotificationButton.Share.takeIf { ItemSharing.canShare(item) || FileSharing.canShare(item) }
        val first = when (item.kind()) {
            ItemKind.Link -> NotificationButton.Copy
            ItemKind.Image, ItemKind.File -> NotificationButton.Save.takeIf { FileDownloads.canDownload(item) }
            ItemKind.RichText, ItemKind.Text ->
                if (TapAction.forItem(item) == TapAction.CopyText) NotificationButton.View else NotificationButton.Copy
            ItemKind.Secret, ItemKind.Unreadable -> null
        }
        return listOfNotNull(first, share)
    }

    /**
     * The size of the decoded thumbnail's reduction: the largest power of two that keeps both sides at least [maxEdge], so that the
     * decoded picture is never larger than twice [maxEdge] and costs little memory. 1 when it already fits, or for an unusable size.
     */
    fun sampleSizeFor(width: Int, height: Int, maxEdge: Int = MAX_PICTURE_EDGE): Int {
        if (width <= 0 || height <= 0 || maxEdge <= 0) return 1
        var sample = 1
        while (width / (sample * 2) >= maxEdge && height / (sample * 2) >= maxEdge) sample *= 2
        return sample
    }

    /** Whether the notification of an item still has a reason to stay: unread, and not put away (archived) on any device. */
    fun isPending(item: TomiItem): Boolean = !item.read && !item.archived

    private val HOST_AUTHORITY = Regex("^https?://([^/?#\\\\]*)", RegexOption.IGNORE_CASE)
}
