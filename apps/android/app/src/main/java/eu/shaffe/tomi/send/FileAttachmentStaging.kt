package eu.shaffe.tomi.send

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import eu.shaffe.tomi.core.DraftAttachment
import eu.shaffe.tomi.share.SharedStream
import eu.shaffe.tomi.share.StreamPolicy
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/**
 * Copies shared content to `cache/outgoing/<draftId>/` with the attachment size limits
 * (25 MB per file, 100 MB per item). Files over the limit are skipped, the others are kept.
 */
class FileAttachmentStaging(private val context: Context) : AttachmentStaging {

    override suspend fun stage(draftId: String, streams: List<SharedStream>): StagedAttachments =
        withContext(Dispatchers.IO) {
            if (streams.isEmpty()) return@withContext StagedAttachments(emptyList(), 0)
            val dir = File(File(context.cacheDir, ROOT), draftId).apply { mkdirs() }
            val staged = mutableListOf<DraftAttachment>()
            var skipped = 0
            var total = 0L
            streams.forEachIndexed { index, stream ->
                val declared = stream.size
                if (declared != null && (declared > StreamPolicy.MAX_FILE_BYTES || total + declared > StreamPolicy.MAX_ITEM_BYTES)) {
                    skipped++
                    return@forEachIndexed
                }
                val name = safeName(stream.name ?: Uri.parse(stream.uri).lastPathSegment ?: "file-$index")
                val target = File(dir, "$index-$name")
                val copied = try {
                    copyBounded(stream.uri, target, StreamPolicy.MAX_FILE_BYTES.coerceAtMost(StreamPolicy.MAX_ITEM_BYTES - total))
                } catch (_: IOException) {
                    -1L
                } catch (_: SecurityException) {
                    -1L
                }
                if (copied < 0) {
                    target.delete()
                    skipped++
                } else {
                    total += copied
                    staged += DraftAttachment(target.absolutePath, name, stream.mime ?: "application/octet-stream", copied)
                }
            }
            StagedAttachments(staged, skipped)
        }

    override fun cleanup(draftId: String) {
        File(File(context.cacheDir, ROOT), draftId).deleteRecursively()
    }

    /** Returns the number of bytes copied, or -1 if the source is unreadable or larger than [limit]. */
    private fun copyBounded(uri: String, target: File, limit: Long): Long {
        val input = context.contentResolver.openInputStream(Uri.parse(uri)) ?: return -1
        input.use { source ->
            target.outputStream().use { sink ->
                val buffer = ByteArray(BUFFER_SIZE)
                var copied = 0L
                while (true) {
                    val read = source.read(buffer)
                    if (read < 0) break
                    copied += read
                    if (copied > limit) return -1
                    sink.write(buffer, 0, read)
                }
                return copied
            }
        }
    }

    private fun safeName(raw: String): String {
        val base = raw.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = base.filter { it.isLetterOrDigit() || it in "._- ()" }.trim().trim('.')
        return cleaned.take(MAX_NAME).ifEmpty { "file" }
    }

    internal companion object {
        /** Directory of `cacheDir` that holds the staged copies; emptied when the device leaves its space. */
        const val ROOT = "outgoing"

        /**
         * Deletes every staged copy. They are clear copies of what the user shared (a secret's file included) and are normally removed
         * at the end of the send; one left by a process that died mid-send must not stay. Called at start-up, before anything stages.
         */
        fun purgeAll(cacheDir: File) {
            File(cacheDir, ROOT).deleteRecursively()
        }

        private const val BUFFER_SIZE = 64 * 1024
        const val MAX_NAME = 120
    }
}

/** Reads display name and size of a content URI offered by another app. */
internal fun readStreamMetadata(context: Context, uri: Uri): SharedStream {
    val resolver = context.contentResolver
    var name: String? = null
    var size: Long? = null
    try {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE)
                if (nameIndex >= 0 && !cursor.isNull(nameIndex)) name = cursor.getString(nameIndex)
                if (sizeIndex >= 0 && !cursor.isNull(sizeIndex)) size = cursor.getLong(sizeIndex)
            }
        }
    } catch (_: RuntimeException) {
        // A misbehaving provider must not break the share: keep going without metadata.
    }
    return SharedStream(uri.toString(), resolver.getType(uri), name, size)
}
