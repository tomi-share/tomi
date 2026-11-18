package eu.shaffe.tomi.send

import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import eu.shaffe.tomi.core.DraftAttachment
import eu.shaffe.tomi.core.DraftThumbnail
import eu.shaffe.tomi.ui.viewer.ImageBitmapLoader
import java.io.ByteArrayOutputStream
import java.io.File
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * The thumbnail an outgoing image carries (spec/format-v1.md: "vignette (quelques Ko)"), shown in the feed of every device without
 * downloading the file. Made from the staged copy, upright (EXIF), framed by [ThumbnailSize.plan] (a tall image is cut to its
 * centre, as the cards crop it anyway), at most [MAX_BYTES]: JPEG, or PNG when the image has transparency and the PNG stays small
 * enough (otherwise JPEG on a white ground). Anything that cannot be decoded or made small enough simply has no thumbnail.
 *
 * Larger than "quelques Ko" on purpose: the Android card shows it across its whole width (about 1000 px on a phone), and a
 * smaller one was visibly blurred there.
 */
object DraftThumbnails {
    /** Each thumbnail travels inside the item (in base64), which the relay caps at about 1 MB; [toFfi] also budgets them together. */
    const val MAX_BYTES = 120 * 1024
    private val JPEG_QUALITIES = intArrayOf(80, 65, 50)

    /** Decoding size: enough for a [ThumbnailSize.MAX_WIDTH] px wide crop of a tall screenshot, without decoding a 50 Mpx photo. */
    private const val DECODE_MAX_EDGE = 2400

    /** Blocking (decodes the file): called off the main thread, by the core call that enqueues the draft. Never throws. */
    fun make(attachment: DraftAttachment): DraftThumbnail? {
        if (!attachment.mime.startsWith("image/")) return null
        val file = File(attachment.path)
        var decoded: Bitmap? = null
        var scaled: Bitmap? = null
        return try {
            val source = ImageBitmapLoader.decode(open = { if (file.isFile) file.inputStream() else null }, maxEdge = DECODE_MAX_EDGE)
                ?: return null
            decoded = source
            val plan = ThumbnailSize.plan(source.width, source.height)
            val small = if (plan.isIdentity(source.width, source.height)) {
                source
            } else {
                val matrix = Matrix().apply { setScale(plan.scale, plan.scale) }
                Bitmap.createBitmap(source, 0, plan.top, source.width, plan.cropHeight, matrix, true)
            }
            scaled = small
            encode(small)
        } catch (_: Exception) {
            null
        } catch (_: OutOfMemoryError) {
            null
        } finally {
            scaled?.takeIf { it !== decoded }?.recycle()
            decoded?.recycle()
        }
    }

    private fun encode(bitmap: Bitmap): DraftThumbnail? {
        if (bitmap.hasAlpha()) {
            compress(bitmap, Bitmap.CompressFormat.PNG, 100)?.let { return DraftThumbnail(it, "image/png") }
        }
        val opaque = if (bitmap.hasAlpha()) flatten(bitmap) else bitmap
        try {
            for (quality in JPEG_QUALITIES) {
                compress(opaque, Bitmap.CompressFormat.JPEG, quality)?.let { return DraftThumbnail(it, "image/jpeg") }
            }
            return null
        } finally {
            if (opaque !== bitmap) opaque.recycle()
        }
    }

    /** The encoded bytes, or null if they do not fit in [MAX_BYTES]. */
    private fun compress(bitmap: Bitmap, format: Bitmap.CompressFormat, quality: Int): ByteArray? {
        val out = ByteArrayOutputStream()
        if (!bitmap.compress(format, quality, out)) return null
        return out.toByteArray().takeIf { it.size <= MAX_BYTES }
    }

    /** A transparent image drawn on white (JPEG has no alpha: it would turn black). */
    private fun flatten(bitmap: Bitmap): Bitmap {
        val opaque = Bitmap.createBitmap(bitmap.width, bitmap.height, Bitmap.Config.ARGB_8888)
        Canvas(opaque).apply {
            drawColor(Color.WHITE)
            drawBitmap(bitmap, 0f, 0f, null)
        }
        return opaque
    }
}

/**
 * Framing of a thumbnail: the rows `top until top + cropHeight` of the image (all its width), scaled by `scale` (never above 1).
 * A tall image is cut to its centre at [MAX_TALL_RATIO] (height / width); the result fits in [MAX_WIDTH] x [MAX_HEIGHT].
 */
object ThumbnailSize {
    const val MAX_WIDTH = 600
    const val MAX_HEIGHT = 800
    const val MAX_TALL_RATIO = 4.0 / 3.0

    data class Plan(val top: Int, val cropHeight: Int, val scale: Float) {
        fun isIdentity(width: Int, height: Int): Boolean = top == 0 && cropHeight == height && scale == 1f

        /** Size of the result, each side at least 1 px. */
        fun size(width: Int): Pair<Int, Int> = max(1, (width * scale).roundToInt()) to max(1, (cropHeight * scale).roundToInt())
    }

    fun plan(width: Int, height: Int): Plan {
        val cropHeight = min(height, max(1, ceil(width * MAX_TALL_RATIO).toInt()))
        val top = (height - cropHeight) / 2
        val scale = minOf(1.0, MAX_WIDTH.toDouble() / width, MAX_HEIGHT.toDouble() / cropHeight).toFloat()
        return Plan(top, cropHeight, scale)
    }
}
