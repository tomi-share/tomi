package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.findBySlot
import eu.shaffe.tomi.core.slotOf
import eu.shaffe.tomi.security.AuthOutcome

/** One file handed to the system share sheet: the `content://` URI of the decrypted copy and its MIME type. Never printed. */
class SharedFile(val uri: String, val mime: String) {
    override fun toString(): String = "SharedFile(<redacted>)"
}

/** Result of [FileSharing.prepare]. */
sealed interface FileShareOutcome {
    /**
     * The share sheet may be opened with [files] (at least one). [text]: the description / link of the item, handed along as the
     * caption of the files (null for a hidden item, whose text stays out of the share sheet, or when there is none).
     */
    class Ready(val files: List<SharedFile>, val text: String? = null) : FileShareOutcome

    /** The item has no file to share (unreadable, no attachment) or none of them is available on this device. */
    data object NothingToShare : FileShareOutcome

    /** A hidden item and no screen lock or biometrics is set up: it stays locked. */
    data object BiometricUnavailable : FileShareOutcome

    /** A hidden item and the user cancelled or failed the authentication: nothing was decrypted or shared. */
    data object NotAuthenticated : FileShareOutcome
}

/**
 * "Partager" of a file / image item to another app, in one place that does not depend on Android types.
 *
 * It applies to every item that has attachments, even when it also carries a description or a link: the user wants the files out
 * of "Partager", and the text then travels along as their caption ([FileShareOutcome.Ready.text]). Same rule as the text share and the image viewer (product decision): a hidden ("Masqué") item can
 * be shared, but nothing is decrypted before [authenticate] returned [AuthOutcome.Success] (same gate, same 2-minute window
 * as "Afficher"). The files are decrypted on demand by [resolveUri] (the core cache), handed to the caller and dropped.
 */
object FileSharing {

    /** True if "Partager" should hand out the attachments of [item] (does not decrypt anything). */
    fun canShare(item: TomiItem): Boolean = item.isReadable && item.attachments.isNotEmpty()

    suspend fun prepare(
        item: TomiItem,
        authenticate: suspend () -> AuthOutcome,
        resolveUri: suspend (itemId: String, slot: Int) -> String?,
    ): FileShareOutcome {
        if (!canShare(item)) return FileShareOutcome.NothingToShare
        gate(item, authenticate)?.let { return it }
        val files = item.attachments.mapIndexedNotNull { index, attachment ->
            // The slot, not the position: they differ once an attachment was removed from the item.
            resolveUri(item.id, item.attachments.slotOf(index))?.let { SharedFile(it, attachment.mime) }
        }
        return if (files.isEmpty()) FileShareOutcome.NothingToShare else FileShareOutcome.Ready(files, caption(item))
    }

    /**
     * "Partager" of ONE file of [item] (the "⋮" menu of a file row), by its [slot]. Same gate as [prepare] for a hidden item, and the
     * file is decrypted on demand only after it. The description and the link of the item are NOT handed along: the user shares this
     * file, not the item ([FileShareOutcome.Ready.text] is null). [FileShareOutcome.NothingToShare] when the slot is unknown or the
     * file is not available on this device.
     */
    suspend fun prepareFile(
        item: TomiItem,
        slot: Int,
        authenticate: suspend () -> AuthOutcome,
        resolveUri: suspend (itemId: String, slot: Int) -> String?,
    ): FileShareOutcome {
        if (!canShare(item)) return FileShareOutcome.NothingToShare
        val attachment = item.attachments.findBySlot(slot) ?: return FileShareOutcome.NothingToShare
        gate(item, authenticate)?.let { return it }
        val uri = resolveUri(item.id, slot) ?: return FileShareOutcome.NothingToShare
        return FileShareOutcome.Ready(listOf(SharedFile(uri, attachment.mime)))
    }

    /** The biometric gate of a hidden item (same window as "Afficher"): null when the files may be decrypted, else the refusal. */
    private suspend fun gate(item: TomiItem, authenticate: suspend () -> AuthOutcome): FileShareOutcome? {
        if (!item.secret) return null
        return when (authenticate()) {
            AuthOutcome.Success -> null
            AuthOutcome.Unavailable -> FileShareOutcome.BiometricUnavailable
            AuthOutcome.Cancelled, AuthOutcome.Failed -> FileShareOutcome.NotAuthenticated
        }
    }

    /** The text and the link of the item as a caption of the shared files; never for a hidden item. */
    private fun caption(item: TomiItem): String? {
        if (item.secret) return null
        return listOfNotNull(item.text?.plain, item.url).filter { it.isNotBlank() }.joinToString("\n").ifEmpty { null }
    }

    /** MIME type of the intent for [mimes]: the type itself when all agree, the wildcard of the major type when only that agrees, else the full wildcard. */
    fun commonMime(mimes: List<String>): String {
        val distinct = mimes.map { it.lowercase() }.distinct()
        if (distinct.size == 1) return distinct.first()
        val majors = distinct.map { it.substringBefore('/') }.distinct()
        return if (majors.size == 1) "${majors.first()}/*" else "*/*"
    }
}
