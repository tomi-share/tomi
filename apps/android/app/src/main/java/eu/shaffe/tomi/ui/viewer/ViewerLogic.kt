package eu.shaffe.tomi.ui.viewer

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.slotOf
import kotlin.math.max
import kotlin.math.min

/*
 * Pure logic of the internal image viewer (no Android types, unit tested): which attachments are pages, what a page index
 * means for the core, what is allowed to be on screen, how EXIF orientation and zoom are computed.
 */

/** One page of the viewer: an image attachment and its SLOT (what the core is asked for, and the key of the page). */
class ViewerImage(val slot: Int, val attachment: Attachment)

object ViewerImages {
    /**
     * The image attachments of [item], in order. A mixed item (image + PDF) only pages through its images; each page keeps
     * the slot of its attachment, because `TomiCore.attachmentUri(itemId, slot)` and `deleteAttachment` speak in slots: once an
     * attachment was removed, the slot is no longer the position in the list.
     */
    fun of(item: TomiItem): List<ViewerImage> =
        item.attachments.mapIndexedNotNull { index, attachment ->
            if (attachment.isImage) ViewerImage(item.attachments.slotOf(index), attachment) else null
        }

    /** Page showing the attachment of [slot]; the first page when it is not an image of the item. */
    fun pageFor(images: List<ViewerImage>, slot: Int): Int =
        images.indexOfFirst { it.slot == slot }.coerceAtLeast(0)

    /** Slot of [page] (coerced into range), or null when the item has no image. */
    fun slotAt(images: List<ViewerImage>, page: Int): Int? =
        images.getOrNull(page.coerceIn(0, max(0, images.size - 1)))?.slot

    /** "2/5" for the page index 1 of 5. */
    fun positionLabel(page: Int, total: Int): String = "${page.coerceIn(0, max(0, total - 1)) + 1}/$total"
}

/** When the "2/5" position counter of a multi-image viewer is on screen: with the bars, or shortly after an interaction. */
object ViewerCounter {
    /** Idle time without interaction (page change, swipe, tap) after which the counter fades out while the bars are hidden. */
    const val IDLE_MS = 2500L

    /** Visible while the bars are shown, or while the user was recently active. */
    fun isVisible(barsVisible: Boolean, recentlyActive: Boolean): Boolean = barsVisible || recentlyActive
}

/** What the viewer is allowed to put on screen. A hidden ("Masqué") item shows nothing until the biometric gate passed. */
sealed interface ViewerContent {
    /** The item is not known yet. */
    data object Loading : ViewerContent

    /** The item is gone (deleted, expired) or has no image. */
    data object Empty : ViewerContent

    /** A hidden item whose authentication has not succeeded: no thumbnail, no decrypted file, no name. */
    data object Locked : ViewerContent

    class Pages(val images: List<ViewerImage>) : ViewerContent
}

object ViewerGate {
    fun content(item: TomiItem?, loaded: Boolean, unlocked: Boolean): ViewerContent {
        if (item == null) return if (loaded) ViewerContent.Empty else ViewerContent.Loading
        if (item.secret && !unlocked) return ViewerContent.Locked
        val images = ViewerImages.of(item)
        return if (images.isEmpty()) ViewerContent.Empty else ViewerContent.Pages(images)
    }
}

/** Which actions of the viewer menu exist for an item. */
object ViewerActions {
    /**
     * "Enregistrer dans la galerie" (a permanent clear copy in the shared storage) is not offered for a hidden ("Masqué") item
     * (product decision). Sharing a hidden image stays available: it goes through the biometric gate.
     */
    fun canSaveToGallery(item: TomiItem): Boolean = !item.secret

    // "Supprimer" always concerns the image on screen, by its slot (several images or other content in the item are fine: the core
    // removes that attachment only, and the whole item only when nothing else is left). "Partout" additionally needs the relay to
    // support it (`TomiCore.supportsAttachmentRemoval`), see [AttachmentRemovals] and the screens.
}

/** EXIF `Orientation` (tag 0x0112) as a rotation (degrees clockwise) followed by an optional horizontal mirror. */
object ExifOrientation {
    class Transform(val rotationDegrees: Int, val flipHorizontal: Boolean) {
        val isIdentity: Boolean get() = rotationDegrees == 0 && !flipHorizontal

        /** True when width and height are exchanged by the transform (quarter turns). */
        val swapsAxes: Boolean get() = rotationDegrees % 180 != 0
    }

    private val IDENTITY = Transform(0, false)

    /** Same table as ExifInterface / Glide: 2 mirror, 3 rotate 180, 4 flip vertical, 5 transpose, 6 rotate 90, 7 transverse, 8 rotate 270. */
    fun transformFor(orientation: Int): Transform = when (orientation) {
        2 -> Transform(0, true)
        3 -> Transform(180, false)
        4 -> Transform(180, true)
        5 -> Transform(90, true)
        6 -> Transform(90, false)
        7 -> Transform(-90, true)
        8 -> Transform(-90, false)
        else -> IDENTITY
    }

    /** Size of the image as displayed, given the stored size. */
    fun displayedSize(width: Int, height: Int, transform: Transform): Pair<Int, Int> =
        if (transform.swapsAxes) height to width else width to height
}

object ImageSampling {
    /**
     * Power-of-two `inSampleSize` that brings the image under [maxEdge] pixels on its longest side and under [maxPixels]
     * in total, so that a 50 MP photo does not exhaust the heap nor exceed the GPU texture size.
     */
    fun sampleSize(width: Int, height: Int, maxEdge: Int, maxPixels: Long): Int {
        if (width <= 0 || height <= 0) return 1
        var sample = 1
        while (max(width, height) / sample > maxEdge || (width / sample).toLong() * (height / sample) > maxPixels) {
            sample *= 2
        }
        return sample
    }
}

/** Scale and translation of the image inside the viewer: the image is drawn at `scale`, then moved by (`x`, `y`) pixels from the centre. */
data class Placement(val scale: Float = 1f, val x: Float = 0f, val y: Float = 0f)

object ZoomMath {
    const val MIN_SCALE = 1f
    const val MAX_SCALE = 6f
    const val DOUBLE_TAP_SCALE = 2.5f

    /** Zoom level (as a share of the fitted image) above which the image counts as zoomed in. */
    private const val ZOOMED = 1.05f

    /** Size of the image once fitted (letterboxed) into the view, at scale 1. */
    fun fitSize(contentWidth: Float, contentHeight: Float, viewWidth: Float, viewHeight: Float): Pair<Float, Float> {
        if (contentWidth <= 0f || contentHeight <= 0f) return 0f to 0f
        val factor = min(viewWidth / contentWidth, viewHeight / contentHeight)
        return contentWidth * factor to contentHeight * factor
    }

    /** Keeps the scale within bounds and the image from leaving the view: no empty border once it is larger than the view. */
    fun clamp(p: Placement, fitWidth: Float, fitHeight: Float, viewWidth: Float, viewHeight: Float): Placement {
        val scale = p.scale.coerceIn(MIN_SCALE, MAX_SCALE)
        val maxX = max(0f, (fitWidth * scale - viewWidth) / 2f)
        val maxY = max(0f, (fitHeight * scale - viewHeight) / 2f)
        return Placement(scale, p.x.coerceIn(-maxX, maxX), p.y.coerceIn(-maxY, maxY))
    }

    /**
     * Pinch / drag step: multiplies the zoom by [zoom] keeping the point under the fingers ([centroidX], [centroidY], relative
     * to the centre of the view) in place, then moves by ([panX], [panY]).
     */
    fun transform(
        p: Placement,
        zoom: Float,
        panX: Float,
        panY: Float,
        centroidX: Float,
        centroidY: Float,
        fitWidth: Float,
        fitHeight: Float,
        viewWidth: Float,
        viewHeight: Float,
    ): Placement {
        val scale = (p.scale * zoom).coerceIn(MIN_SCALE, MAX_SCALE)
        val ratio = scale / p.scale
        val moved = Placement(
            scale = scale,
            x = (p.x - centroidX) * ratio + centroidX + panX,
            y = (p.y - centroidY) * ratio + centroidY + panY,
        )
        return clamp(moved, fitWidth, fitHeight, viewWidth, viewHeight)
    }

    fun isZoomed(p: Placement): Boolean = p.scale > ZOOMED

    /** Double tap: back to the fitted image when zoomed in, otherwise zoom to [DOUBLE_TAP_SCALE] around the tapped point. */
    fun doubleTap(
        p: Placement,
        tapX: Float,
        tapY: Float,
        fitWidth: Float,
        fitHeight: Float,
        viewWidth: Float,
        viewHeight: Float,
    ): Placement =
        if (isZoomed(p)) {
            Placement()
        } else {
            transform(p, DOUBLE_TAP_SCALE / p.scale, 0f, 0f, tapX, tapY, fitWidth, fitHeight, viewWidth, viewHeight)
        }
}

/** File name given to an image saved to the gallery: no path characters, an extension that matches the MIME type. */
object GalleryName {
    private val FORBIDDEN = Regex("[\\\\/:*?\"<>|\\u0000-\\u001f]")
    private const val MAX_LENGTH = 100
    private const val FALLBACK = "tomi-image"

    fun of(name: String, mime: String): String {
        val extension = extensionFor(mime)
        val clean = FORBIDDEN.replace(name.substringAfterLast('/').substringAfterLast('\\'), "_").trim().trim('.')
        val base = clean.ifEmpty { FALLBACK }.take(MAX_LENGTH)
        val known = KNOWN_EXTENSIONS.getValue(extension)
        val hasMatchingExtension = known.any { base.endsWith(".$it", ignoreCase = true) }
        return if (hasMatchingExtension) base else "${base.substringBeforeLast('.', base).ifEmpty { FALLBACK }}.$extension"
    }

    private val KNOWN_EXTENSIONS: Map<String, List<String>> = mapOf(
        "jpg" to listOf("jpg", "jpeg"),
        "png" to listOf("png"),
        "webp" to listOf("webp"),
        "gif" to listOf("gif"),
        "heic" to listOf("heic", "heif"),
        "avif" to listOf("avif"),
        "bmp" to listOf("bmp"),
    )

    private fun extensionFor(mime: String): String = when (mime.lowercase()) {
        "image/jpeg", "image/jpg", "image/pjpeg" -> "jpg"
        "image/png" -> "png"
        "image/webp" -> "webp"
        "image/gif" -> "gif"
        "image/heic", "image/heif" -> "heic"
        "image/avif" -> "avif"
        "image/bmp" -> "bmp"
        else -> "jpg"
    }
}
