package eu.shaffe.tomi.ui.render

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RenderedHeightsTest {
    @After
    fun tearDown() = RenderedHeights.clear()

    @Test
    fun aHeightIsFoundAgainForTheSameTextWidthAndScale() {
        RenderedHeights.put("# titre\n\ntexte", width = 900, density = 3f, fontScale = 1f, variant = 0, height = 240)
        // Another instance of the same text (a new read of the feed): the key is the content, not the object.
        val again = String("# titre\n\ntexte".toCharArray())
        assertEquals(240, RenderedHeights.get(again, 900, 3f, 1f, 0))
    }

    @Test
    fun anotherTextWidthScaleOrVariantIsUnknown() {
        RenderedHeights.put("texte", 900, 3f, 1f, 0, 240)
        assertNull(RenderedHeights.get("autre", 900, 3f, 1f, 0))
        assertNull(RenderedHeights.get("texte", 1200, 3f, 1f, 0))
        assertNull(RenderedHeights.get("texte", 900, 2f, 1f, 0))
        assertNull(RenderedHeights.get("texte", 900, 3f, 1.3f, 0))
        assertNull(RenderedHeights.get("texte", 900, 3f, 1f, 1))
    }

    @Test
    fun aViewWithoutHeightIsNotRemembered() {
        RenderedHeights.put("texte", 900, 3f, 1f, 0, 0)
        assertNull(RenderedHeights.get("texte", 900, 3f, 1f, 0))
    }

    @Test
    fun theLastHeightWinsAndTheOldestEntriesLeave() {
        RenderedHeights.put("texte", 900, 3f, 1f, 0, 240)
        RenderedHeights.put("texte", 900, 3f, 1f, 0, 300)
        assertEquals(300, RenderedHeights.get("texte", 900, 3f, 1f, 0))
        repeat(2_000) { RenderedHeights.put("t$it", 900, 3f, 1f, 0, 10) }
        assertTrue(RenderedHeights.size <= 512)
        assertNull(RenderedHeights.get("texte", 900, 3f, 1f, 0))
        assertEquals(10, RenderedHeights.get("t1999", 900, 3f, 1f, 0))
    }

    @Test
    fun theFirstScreenDecidesHowManyItemsAreComposedAtOnce() {
        // One long item fills the screen: one rendering to open the detail.
        assertEquals(1, initialItems(10, { 4_000 }, screenWeight = 1_600f))
        // A short first item (a title before a long table) takes the next one with it.
        assertEquals(2, initialItems(10, { if (it == 0) 140 else 4_000 }, 1_600f))
        // Never more than four, never more than there are, and none of nothing.
        assertEquals(4, initialItems(10, { 10 }, 1_600f))
        assertEquals(3, initialItems(3, { 10 }, 1_600f))
        assertEquals(0, initialItems(0, { 10 }, 1_600f))
    }
}
