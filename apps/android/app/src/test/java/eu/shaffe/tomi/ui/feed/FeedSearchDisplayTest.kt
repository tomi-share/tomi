package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class FeedSearchDisplayTest {
    private fun item(id: String, secret: Boolean = false) = readableItem(
        id = id, text = RichText("x"), secret = secret,
        sourceDevice = DeviceRef("pc", "PC"), createdAt = Instant.parse("2026-11-04T10:00:00Z"),
    )

    @Test
    fun counterIsHiddenWithoutQueryAndWithTheAllFilter() {
        assertFalse(FeedSearchDisplay.showsCounter("", FeedFilter.All))
        assertFalse(FeedSearchDisplay.showsCounter("   ", FeedFilter.All))
    }

    @Test
    fun counterIsShownForAQueryOrAFilter() {
        assertTrue(FeedSearchDisplay.showsCounter("wifi", FeedFilter.All))
        assertTrue(FeedSearchDisplay.showsCounter("", FeedFilter.Links))
        assertTrue(FeedSearchDisplay.showsCounter("", FeedFilter.Archives))
        assertTrue(FeedSearchDisplay.showsCounter("wifi", FeedFilter.Secrets))
    }

    @Test
    fun maskedNoteNeedsAQueryAndAHiddenItemAmongTheResults() {
        val plain = item("a")
        val hidden = item("s", secret = true)
        assertTrue(FeedSearchDisplay.showsMaskedNote("pc", listOf(plain, hidden)))
        assertFalse(FeedSearchDisplay.showsMaskedNote("pc", listOf(plain)))
        assertFalse(FeedSearchDisplay.showsMaskedNote("pc", emptyList()))
        // The "Secrets" filter alone is not a search: no note without a query.
        assertFalse(FeedSearchDisplay.showsMaskedNote("", listOf(hidden)))
        assertFalse(FeedSearchDisplay.showsMaskedNote("  ", listOf(hidden)))
    }

    @Test
    fun resultsAreFlatOnlyForARealQuery() {
        assertTrue(FeedSearchDisplay.isFlat("wifi"))
        assertFalse(FeedSearchDisplay.isFlat(""))
        assertFalse(FeedSearchDisplay.isFlat(" "))
    }
}
