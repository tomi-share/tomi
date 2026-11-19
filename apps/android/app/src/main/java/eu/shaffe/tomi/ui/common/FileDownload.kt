package eu.shaffe.tomi.ui.common

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.share.FileDownloads
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * Copies the file behind [uri] (`content://` of the core only) into the shared `Downloads/Tomi` folder with MediaStore: no storage
 * permission is needed on API 29+ (minSdk is 33), and none is declared. Only for a file of a normal item: the callers never get here for
 * a hidden one ([FileDownloads.download] refuses it).
 *
 * The entry is only ever INSERTED, never updated or replaced: when the name is already taken (an earlier download), MediaStore gives
 * the new file a free one (`name (1).ext`), so nothing is overwritten. It stays invisible to other apps (`IS_PENDING`) until the copy is
 * complete and is removed if it fails. Returns false on any failure.
 */
suspend fun saveToDownloads(context: Context, uri: String, attachment: Attachment): Boolean = withContext(Dispatchers.IO) {
    val source = Uri.parse(uri)
    if (source.scheme != "content") return@withContext false
    val resolver = context.contentResolver
    val pending = ContentValues().apply {
        put(MediaStore.Downloads.DISPLAY_NAME, FileDownloads.fileName(attachment.name))
        put(MediaStore.Downloads.MIME_TYPE, FileDownloads.mimeType(attachment.mime))
        put(MediaStore.Downloads.RELATIVE_PATH, FileDownloads.RELATIVE_PATH)
        put(MediaStore.Downloads.IS_PENDING, 1)
    }
    val target = try {
        resolver.insert(MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY), pending)
    } catch (_: RuntimeException) {
        null
    } ?: return@withContext false
    try {
        val input = resolver.openInputStream(source) ?: throw IOException("no stream")
        val output = resolver.openOutputStream(target) ?: throw IOException("no output")
        input.use { i -> output.use { o -> i.copyTo(o) } }
        resolver.update(target, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
        true
    } catch (e: CancellationException) {
        resolver.delete(target, null, null)
        throw e
    } catch (_: Exception) {
        resolver.delete(target, null, null)
        false
    }
}
