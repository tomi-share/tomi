package eu.shaffe.tomi.ui.viewer

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ComponentName
import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.net.Uri
import android.provider.MediaStore
import androidx.exifinterface.media.ExifInterface
import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.share.ShareActivity
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.IOException
import java.io.InputStream

/**
 * Decodes an image honouring its EXIF orientation (a portrait photo stored sideways is shown upright), downsampled so that
 * neither the heap nor the GPU texture size is exceeded. No third-party image library: `BitmapFactory` + `ExifInterface`.
 */
object ImageBitmapLoader {
    /** Longest side kept for the viewer: enough to zoom on a phone screen, under the usual 4096 px texture limit. */
    const val VIEWER_MAX_EDGE = 4096
    private const val VIEWER_MAX_PIXELS = 12_000_000L

    /**
     * Loads a `content://` URI (refused otherwise: no `file://` access to the app's private files). Null when it cannot
     * be read or decoded; never throws. Blocking: call it off the main thread.
     */
    fun load(resolver: ContentResolver, uri: String, maxEdge: Int = VIEWER_MAX_EDGE): Bitmap? {
        val parsed = Uri.parse(uri)
        if (parsed.scheme != "content") return null
        return decode(
            open = {
                try {
                    resolver.openInputStream(parsed)
                } catch (_: IOException) {
                    null
                } catch (_: SecurityException) {
                    null
                }
            },
            maxEdge = maxEdge,
        )
    }

    /** Same from any stream source ([open] is called up to three times: size, orientation, pixels). */
    fun decode(open: () -> InputStream?, maxEdge: Int = VIEWER_MAX_EDGE): Bitmap? {
        return try {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            open()?.use { BitmapFactory.decodeStream(it, null, bounds) }
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null

            val orientation = readOrientation(open)
            val options = BitmapFactory.Options().apply {
                inSampleSize = ImageSampling.sampleSize(bounds.outWidth, bounds.outHeight, maxEdge, VIEWER_MAX_PIXELS)
            }
            val decoded = open()?.use { BitmapFactory.decodeStream(it, null, options) } ?: return null
            applyOrientation(decoded, orientation)
        } catch (_: IOException) {
            null
        } catch (_: OutOfMemoryError) {
            null
        }
    }

    private fun readOrientation(open: () -> InputStream?): Int = try {
        open()?.use { ExifInterface(it).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL) }
            ?: ExifInterface.ORIENTATION_NORMAL
    } catch (_: IOException) {
        ExifInterface.ORIENTATION_NORMAL
    } catch (_: RuntimeException) {
        ExifInterface.ORIENTATION_NORMAL
    }

    private fun applyOrientation(bitmap: Bitmap, orientation: Int): Bitmap {
        val transform = ExifOrientation.transformFor(orientation)
        if (transform.isIdentity) return bitmap
        val matrix = Matrix().apply {
            postRotate(transform.rotationDegrees.toFloat())
            if (transform.flipHorizontal) postScale(-1f, 1f)
        }
        val rotated = Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true)
        if (rotated !== bitmap) bitmap.recycle()
        return rotated
    }
}

/** Writing an image out of Tomi, on explicit request of the user (the callers authenticate first for a hidden item). */
object ImageExport {
    private const val GALLERY_FOLDER = "Pictures/Tomi"

    /**
     * Copies the file behind [uri] (`content://` only) into the shared gallery (`Pictures/Tomi`) with MediaStore: no storage
     * permission needed on API 33+. The entry is invisible to other apps (`IS_PENDING`) until the copy is complete, and removed
     * if it fails. Returns false on any failure.
     */
    suspend fun saveToGallery(context: Context, uri: String, attachment: Attachment): Boolean = withContext(Dispatchers.IO) {
        val source = Uri.parse(uri)
        if (source.scheme != "content" || !attachment.isImage) return@withContext false
        val resolver = context.contentResolver
        val pending = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, GalleryName.of(attachment.name, attachment.mime))
            put(MediaStore.Images.Media.MIME_TYPE, attachment.mime)
            put(MediaStore.Images.Media.RELATIVE_PATH, GALLERY_FOLDER)
            put(MediaStore.Images.Media.IS_PENDING, 1)
        }
        val target = try {
            resolver.insert(MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY), pending)
        } catch (_: RuntimeException) {
            null
        } ?: return@withContext false
        try {
            val input = resolver.openInputStream(source) ?: throw IOException("no stream")
            val output = resolver.openOutputStream(target) ?: throw IOException("no output")
            input.use { i -> output.use { o -> i.copyTo(o) } }
            resolver.update(target, ContentValues().apply { put(MediaStore.Images.Media.IS_PENDING, 0) }, null, null)
            true
        } catch (e: CancellationException) {
            resolver.delete(target, null, null)
            throw e
        } catch (_: Exception) {
            resolver.delete(target, null, null)
            false
        }
    }

    /**
     * Opens the Android share sheet (`ACTION_SEND`) with the image behind [uri]. The receiving app reads it through a
     * temporary read grant on the core's `content://` URI: no copy is made by Tomi, nothing is written to disk. The name of
     * the file is not put in the intent. Tomi's own share target is excluded. Returns false if no app can receive it.
     */
    fun share(context: Context, uri: String, attachment: Attachment, chooserTitle: String): Boolean {
        val stream = Uri.parse(uri)
        if (stream.scheme != "content" || !attachment.isImage) return false
        val send = Intent(Intent.ACTION_SEND).apply {
            type = attachment.mime
            putExtra(Intent.EXTRA_STREAM, stream)
            clipData = ClipData.newRawUri("", stream)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        val chooser = Intent.createChooser(send, chooserTitle)
            .putExtra(Intent.EXTRA_EXCLUDE_COMPONENTS, arrayOf(ComponentName(context, ShareActivity::class.java)))
        return try {
            context.startActivity(chooser)
            true
        } catch (_: ActivityNotFoundException) {
            false
        }
    }
}
