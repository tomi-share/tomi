package eu.shaffe.tomi.send

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ThumbnailSizeTest {
    @Test
    fun aTallScreenshotIsCutToItsCentreAndKeepsItsWidth() {
        val plan = ThumbnailSize.plan(1080, 2340)
        assertEquals(1440, plan.cropHeight)
        assertEquals((2340 - 1440) / 2, plan.top)
        assertEquals(600 to 800, plan.size(1080))
    }

    @Test
    fun aLandscapePhotoIsOnlyScaledDown() {
        val plan = ThumbnailSize.plan(4000, 3000)
        assertEquals(0, plan.top)
        assertEquals(3000, plan.cropHeight)
        assertEquals(600 to 450, plan.size(4000))
    }

    @Test
    fun aSmallImageIsLeftAsItIs() {
        assertTrue(ThumbnailSize.plan(100, 50).isIdentity(100, 50))
    }

    @Test
    fun aThinImageKeepsAtLeastOnePixel() {
        assertEquals(600 to 1, ThumbnailSize.plan(10_000, 3).size(10_000))
    }
}
