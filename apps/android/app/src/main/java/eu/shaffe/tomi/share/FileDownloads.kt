package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.findBySlot

/** Result of [FileDownloads.download]. */
sealed interface FileDownloadResult {
    /** The file was written to the downloads folder. */
    data object Done : FileDownloadResult

    /** A hidden item: a permanent clear copy is forbidden (golden rule 3), nothing was decrypted or written. */
    data object Forbidden : FileDownloadResult

    /** Unknown slot, unreadable item or file not available on this device (not downloaded yet): nothing was written. */
    data object Unavailable : FileDownloadResult

    /** The file was found but writing it failed (no space, storage refused...). */
    data object Failed : FileDownloadResult
}

/**
 * "Télécharger" of ONE file of an item to the shared `Downloads/Tomi` folder (the "⋮" menu of a file row), in one place that does not
 * depend on Android types.
 *
 * Product decision shared with every platform (see `docs/FEATURES.md`, "Save the image / file"): saving a file out of Tomi is
 * FORBIDDEN for a hidden ("Masqué") item. It would leave a permanent clear copy outside the app, which golden rule 3 rules out; there
 * is no biometric workaround, the entry is simply not offered ([canDownload]) and [download] refuses anyway, before the core is asked
 * to decrypt anything. A normal item needs no authentication.
 */
object FileDownloads {
    /** Where the files go, relative to the root of the shared storage (`MediaStore.Downloads` requires the `Download/` prefix). */
    const val RELATIVE_PATH = "Download/Tomi"

    private val FORBIDDEN = Regex("[\\\\/:*?\"<>|\\u0000-\\u001f]")
    private const val MAX_NAME_LENGTH = 100
    private const val MAX_EXTENSION_LENGTH = 16
    private const val FALLBACK_NAME = "tomi-file"
    private const val FALLBACK_MIME = "application/octet-stream"

    /** True if the rows of the files of [item] may offer "Télécharger": a readable item that is not hidden. */
    fun canDownload(item: TomiItem): Boolean = item.isReadable && !item.secret && item.attachments.isNotEmpty()

    /**
     * Resolves the file of [slot] ([resolveUri]: `TomiCore.attachmentUri`, decrypted on demand; null when not available) and hands it to
     * [write], which copies it to the downloads folder and returns true on success. A hidden item is refused first
     * ([FileDownloadResult.Forbidden]): [resolveUri] is never called for it.
     */
    suspend fun download(
        item: TomiItem,
        slot: Int,
        resolveUri: suspend (itemId: String, slot: Int) -> String?,
        write: suspend (uri: String, attachment: Attachment) -> Boolean,
    ): FileDownloadResult {
        if (!item.isReadable) return FileDownloadResult.Unavailable
        if (item.secret) return FileDownloadResult.Forbidden
        val attachment = item.attachments.findBySlot(slot) ?: return FileDownloadResult.Unavailable
        val uri = resolveUri(item.id, slot) ?: return FileDownloadResult.Unavailable
        return if (write(uri, attachment)) FileDownloadResult.Done else FileDownloadResult.Failed
    }

    /**
     * Display name of the saved file: the name of the attachment without any path or character a file system rejects, at most
     * [MAX_NAME_LENGTH] characters (the extension is kept when the name is cut). It is NOT made unique here: the system does it when
     * the name is already taken (`name (1).ext`), so that an earlier download is never overwritten.
     */
    fun fileName(name: String): String {
        val clean = FORBIDDEN.replace(name.substringAfterLast('/').substringAfterLast('\\'), "_").trim().trim('.')
        if (clean.isEmpty()) return FALLBACK_NAME
        val dot = clean.lastIndexOf('.')
        val extension = if (dot > 0 && clean.length - dot <= MAX_EXTENSION_LENGTH) clean.substring(dot) else ""
        val base = clean.removeSuffix(extension)
        return base.take(MAX_NAME_LENGTH - extension.length) + extension
    }

    /** MIME type of the saved file: the one of the attachment, a generic binary type when it has none. */
    fun mimeType(mime: String): String = mime.trim().ifEmpty { FALLBACK_MIME }
}
