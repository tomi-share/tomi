package eu.shaffe.tomi.ui.viewer

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class ViewerLogicTest {
    private val source = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-05T10:00:00Z")

    private val png = Attachment("a.png", "image/png", 10)
    private val jpg = Attachment("b.jpg", "image/jpeg", 10)
    private val pdf = Attachment("c.pdf", "application/pdf", 10)

    private fun item(attachments: List<Attachment>, secret: Boolean = false) =
        readableItem("i", source, now, attachments = attachments, secret = secret)

    // --- pager: which pages, which index ---------------------------------------------------------------------------

    @Test
    fun theViewerPagesThroughTheImagesOnlyAndKeepsTheirSlot() {
        val images = ViewerImages.of(item(listOf(pdf, png, pdf, jpg)))
        assertEquals(listOf(1, 3), images.map { it.slot })
        assertEquals(listOf("a.png", "b.jpg"), images.map { it.attachment.name })
        assertTrue(ViewerImages.of(item(listOf(pdf))).isEmpty())
        assertTrue(ViewerImages.of(item(emptyList())).isEmpty())
    }

    @Test
    fun theStartPageIsTheTappedImageAndFallsBackToTheFirst() {
        val images = ViewerImages.of(item(listOf(pdf, png, pdf, jpg)))
        assertEquals(0, ViewerImages.pageFor(images, 1))
        assertEquals(1, ViewerImages.pageFor(images, 3))
        // A file, an unknown index or a negative one: first page, never a crash.
        assertEquals(0, ViewerImages.pageFor(images, 0))
        assertEquals(0, ViewerImages.pageFor(images, 99))
        assertEquals(0, ViewerImages.pageFor(images, -1))
        assertEquals(0, ViewerImages.pageFor(emptyList(), 2))
    }

    @Test
    fun aPageMapsBackToTheSlotTheCoreIsAskedFor() {
        val images = ViewerImages.of(item(listOf(pdf, png, pdf, jpg)))
        assertEquals(1, ViewerImages.slotAt(images, 0))
        assertEquals(3, ViewerImages.slotAt(images, 1))
        // Out-of-range pages are clamped (the pager may report a page just before the list shrinks).
        assertEquals(3, ViewerImages.slotAt(images, 7))
        assertEquals(1, ViewerImages.slotAt(images, -3))
        assertNull(ViewerImages.slotAt(emptyList(), 0))
    }

    @Test
    fun thePositionIsShownAsPageOverTotal() {
        assertEquals("1/5", ViewerImages.positionLabel(0, 5))
        assertEquals("2/5", ViewerImages.positionLabel(1, 5))
        assertEquals("5/5", ViewerImages.positionLabel(4, 5))
        assertEquals("5/5", ViewerImages.positionLabel(9, 5))
        assertEquals("1/1", ViewerImages.positionLabel(0, 1))
    }

    // --- what may be on screen --------------------------------------------------------------------------------------

    @Test
    fun aHiddenItemShowsNothingUntilTheGatePassed() {
        val secret = item(listOf(png, jpg), secret = true)
        assertSame(ViewerContent.Locked, ViewerGate.content(secret, loaded = true, unlocked = false))
        val unlocked = ViewerGate.content(secret, loaded = true, unlocked = true)
        assertEquals(2, (unlocked as ViewerContent.Pages).images.size)
    }

    @Test
    fun aNormalItemNeedsNoUnlock() {
        val content = ViewerGate.content(item(listOf(png)), loaded = true, unlocked = false)
        assertEquals(1, (content as ViewerContent.Pages).images.size)
    }

    @Test
    fun missingEmptyAndUnreadableItemsHaveNothingToShow() {
        assertSame(ViewerContent.Loading, ViewerGate.content(null, loaded = false, unlocked = false))
        assertSame(ViewerContent.Empty, ViewerGate.content(null, loaded = true, unlocked = true))
        assertSame(ViewerContent.Empty, ViewerGate.content(item(listOf(pdf)), loaded = true, unlocked = true))
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)
        assertSame(ViewerContent.Empty, ViewerGate.content(unreadable, loaded = true, unlocked = true))
    }

    // --- EXIF -------------------------------------------------------------------------------------------------------

    @Test
    fun exifOrientationsMapToRotationAndMirror() {
        fun t(o: Int) = ExifOrientation.transformFor(o).let { it.rotationDegrees to it.flipHorizontal }
        assertEquals(0 to false, t(1))
        assertEquals(0 to true, t(2))
        assertEquals(180 to false, t(3))
        assertEquals(180 to true, t(4))
        assertEquals(90 to true, t(5))
        assertEquals(90 to false, t(6))
        assertEquals(-90 to true, t(7))
        assertEquals(-90 to false, t(8))
        // Missing or invalid value: untouched.
        assertTrue(ExifOrientation.transformFor(0).isIdentity)
        assertTrue(ExifOrientation.transformFor(42).isIdentity)
        assertTrue(ExifOrientation.transformFor(1).isIdentity)
    }

    @Test
    fun aPortraitPhotoStoredSidewaysIsDisplayedPortrait() {
        // The demo image: stored 1200 x 800 with orientation 6 (rotate 90 clockwise) -> shown 800 x 1200.
        val rotate90 = ExifOrientation.transformFor(6)
        assertTrue(rotate90.swapsAxes)
        assertEquals(800 to 1200, ExifOrientation.displayedSize(1200, 800, rotate90))
        assertEquals(800 to 1200, ExifOrientation.displayedSize(1200, 800, ExifOrientation.transformFor(8)))
        // Orientations that do not turn a quarter keep the size.
        assertEquals(1200 to 800, ExifOrientation.displayedSize(1200, 800, ExifOrientation.transformFor(3)))
        assertEquals(1200 to 800, ExifOrientation.displayedSize(1200, 800, ExifOrientation.transformFor(2)))
    }

    @Test
    fun largePhotosAreDownsampledByPowersOfTwo() {
        assertEquals(1, ImageSampling.sampleSize(1200, 800, 4096, 12_000_000L))
        assertEquals(2, ImageSampling.sampleSize(8000, 6000, 4096, 12_000_000L))
        // 12 MP limit: 4000 x 3000 = 12 MP passes, 4032 x 3024 = 12.2 MP is halved.
        assertEquals(1, ImageSampling.sampleSize(4000, 3000, 4096, 12_000_000L))
        assertEquals(2, ImageSampling.sampleSize(4032, 3024, 4096, 12_000_000L))
        assertEquals(8, ImageSampling.sampleSize(30000, 20000, 4096, 12_000_000L))
        assertEquals(1, ImageSampling.sampleSize(0, 0, 4096, 12_000_000L))
    }

    // --- zoom -------------------------------------------------------------------------------------------------------

    // A 1000 x 500 image fitted in a 1000 x 1000 view is 1000 x 500 at scale 1.
    private val viewW = 1000f
    private val viewH = 1000f
    private val fitW = 1000f
    private val fitH = 500f

    @Test
    fun theImageIsFittedInTheViewKeepingItsRatio() {
        assertEquals(1000f to 500f, ZoomMath.fitSize(2000f, 1000f, 1000f, 1000f))
        assertEquals(500f to 1000f, ZoomMath.fitSize(800f, 1600f, 1000f, 1000f))
        assertEquals(0f to 0f, ZoomMath.fitSize(0f, 0f, 1000f, 1000f))
    }

    @Test
    fun pinchZoomIsLimitedAndNeverLeavesAnEmptyBorder() {
        val zoomed = ZoomMath.transform(Placement(), 2f, 0f, 0f, 0f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(2f, zoomed.scale, 0.001f)
        // Wider than the view: can move sideways by (2000 - 1000) / 2; as high as 1000 = the view: no vertical move.
        val dragged = ZoomMath.transform(zoomed, 1f, 900f, 900f, 0f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(500f, dragged.x, 0.001f)
        assertEquals(0f, dragged.y, 0.001f)
        val tooMuch = ZoomMath.transform(zoomed, 100f, 0f, 0f, 0f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(ZoomMath.MAX_SCALE, tooMuch.scale, 0.001f)
        val tooLittle = ZoomMath.transform(zoomed, 0.01f, 0f, 0f, 0f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(1f, tooLittle.scale, 0.001f)
        assertEquals(0f, tooLittle.x, 0.001f)
    }

    @Test
    fun pinchKeepsThePointUnderTheFingersInPlace() {
        // Fingers 200 px right of the centre: after x2 the image has moved 200 px left so that point stays under them.
        val zoomed = ZoomMath.transform(Placement(), 2f, 0f, 0f, 200f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(-200f, zoomed.x, 0.001f)
    }

    @Test
    fun doubleTapZoomsInAroundThePointThenOutAgain() {
        val zoomedIn = ZoomMath.doubleTap(Placement(), 200f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(ZoomMath.DOUBLE_TAP_SCALE, zoomedIn.scale, 0.001f)
        assertTrue(ZoomMath.isZoomed(zoomedIn))
        assertTrue(zoomedIn.x < 0f)
        val zoomedOut = ZoomMath.doubleTap(zoomedIn, 0f, 0f, fitW, fitH, viewW, viewH)
        assertEquals(Placement(), zoomedOut)
        assertFalse(ZoomMath.isZoomed(zoomedOut))
    }

    // --- position counter visibility ---------------------------------------------------------------------------------

    @Test
    fun theCounterIsVisibleWithTheBarsOrWhileRecentlyActive() {
        assertTrue(ViewerCounter.isVisible(barsVisible = true, recentlyActive = true))
        assertTrue(ViewerCounter.isVisible(barsVisible = true, recentlyActive = false))
        assertTrue(ViewerCounter.isVisible(barsVisible = false, recentlyActive = true))
        assertFalse(ViewerCounter.isVisible(barsVisible = false, recentlyActive = false))
    }

    @Test
    fun theCounterWaitsAFewSecondsBeforeFadingOut() {
        assertEquals(2500L, ViewerCounter.IDLE_MS)
    }

    // --- gallery file name ------------------------------------------------------------------------------------------

    @Test
    fun theGalleryNameIsSafeAndMatchesTheMimeType() {
        assertEquals("photo.jpg", GalleryName.of("photo.jpg", "image/jpeg"))
        assertEquals("photo.JPEG", GalleryName.of("photo.JPEG", "image/jpeg"))
        assertEquals("photo.jpg", GalleryName.of("photo", "image/jpeg"))
        assertEquals("photo.png", GalleryName.of("photo.jpg", "image/png"))
        assertEquals("tomi-image.webp", GalleryName.of("", "image/webp"))
        // No path component, no forbidden character, whatever the sender wrote.
        assertEquals("passwd.jpg", GalleryName.of("../../etc/passwd", "image/jpeg"))
        assertEquals("a_b_c.png", GalleryName.of("a:b*c.png", "image/png"))
        assertTrue(GalleryName.of("x".repeat(500) + ".jpg", "image/jpeg").length <= 104)
    }

    // --- menu: no "save to the gallery" for a hidden item (product decision) -----------------------------------------------

    @Test
    fun aHiddenItemCannotBeSavedToTheGalleryButANormalOneCan() {
        assertFalse(ViewerActions.canSaveToGallery(item(listOf(png), secret = true)))
        assertTrue(ViewerActions.canSaveToGallery(item(listOf(png), secret = false)))
    }

    // --- slots: what the core is asked for once an attachment was removed ----------------------------------------------------

    @Test
    fun theSlotOfAnImagePageIsItsOwnNotItsPositionInTheList() {
        // Slots 0 and 2 were removed: the list holds slots 1 (png), 3 (pdf) and 4 (jpg) in positions 0, 1, 2.
        val attachments = listOf(png.withSlot(1), pdf.withSlot(3), jpg.withSlot(4))
        val images = ViewerImages.of(item(attachments))

        assertEquals(listOf(1, 4), images.map { it.slot })
        assertEquals(0, ViewerImages.pageFor(images, 1))
        assertEquals(1, ViewerImages.pageFor(images, 4))
        // A position that is not a slot of an image (2 was removed, 3 is the pdf): first page.
        assertEquals(0, ViewerImages.pageFor(images, 2))
        assertEquals(0, ViewerImages.pageFor(images, 3))
        assertEquals(4, ViewerImages.slotAt(images, 1))
    }

    @Test
    fun theSamePageKeepsItsSlotWhenAnotherImageDisappearsBeforeIt() {
        val before = ViewerImages.of(item(listOf(png.withSlot(0), jpg.withSlot(1), png.withSlot(2))))
        val after = ViewerImages.of(item(listOf(jpg.withSlot(1), png.withSlot(2))))

        // The pager is keyed by the slot: the image seen on page 1 (slot 1) is now on page 0, still slot 1.
        assertEquals(1, ViewerImages.slotAt(before, 1))
        assertEquals(0, ViewerImages.pageFor(after, 1))
        assertEquals(1, ViewerImages.slotAt(after, 0))
    }

    @Test
    fun noImageLeftMeansTheViewerHasNothingToShow() {
        val withPdfOnly = item(listOf(pdf.withSlot(2)))
        assertTrue(ViewerImages.of(withPdfOnly).isEmpty())
        assertSame(ViewerContent.Empty, ViewerGate.content(withPdfOnly, loaded = true, unlocked = true))
    }

    // --- delete: the image on screen, whatever else the item holds ----------------------------------------------------------

    @Test
    fun theLimitOfASingleImageItemIsGone() {
        // Several images, an image with a text: the removal is of that image only; the core deletes the item when nothing is left.
        val removable = listOf(
            item(listOf(png)),
            item(listOf(png, jpg)),
            item(listOf(png, pdf)),
            readableItem("t", source, now, attachments = listOf(png), text = RichText("legende")),
        )
        removable.forEach { assertTrue(eu.shaffe.tomi.ui.feed.AttachmentRemovals.canRemove(it)) }
    }
}
