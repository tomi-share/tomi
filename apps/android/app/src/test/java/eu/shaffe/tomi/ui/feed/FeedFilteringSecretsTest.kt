package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** The type filters must not class a hidden item by its content: it stays under "Masqués" */
class FeedFilteringSecretsTest {
    private fun item(id: String, secret: Boolean, url: String? = null, text: String? = null, files: List<Attachment> = emptyList()) =
        readableItem(
            id = id, url = url, text = text?.let(::RichText), attachments = files, secret = secret,
            sourceDevice = DeviceRef("pc", "PC"), createdAt = Instant.parse("2026-11-04T10:00:00Z"),
        )

    private val file = Attachment("secret.txt", "text/plain", 3)

    @Test
    fun aHiddenItemIsNotClassedByItsContentUnderTheTypeFilters() {
        val hidden = item("s", secret = true, url = "https://example.org", text = "mot de passe", files = listOf(file))
        assertFalse(FeedFiltering.matches(hidden, FeedFilter.Links))
        assertFalse(FeedFiltering.matches(hidden, FeedFilter.Text))
        assertFalse(FeedFiltering.matches(hidden, FeedFilter.Files))
        assertTrue(FeedFiltering.matches(hidden, FeedFilter.Secrets))
        assertTrue(FeedFiltering.matches(hidden, FeedFilter.All))
    }

    @Test
    fun aVisibleItemStillMatchesItsTypeFilters() {
        val visible = item("v", secret = false, url = "https://example.org", text = "hello", files = listOf(file))
        assertTrue(FeedFiltering.matches(visible, FeedFilter.Links))
        assertTrue(FeedFiltering.matches(visible, FeedFilter.Text))
        assertTrue(FeedFiltering.matches(visible, FeedFilter.Files))
        assertFalse(FeedFiltering.matches(visible, FeedFilter.Secrets))
    }

    @Test
    fun theLinksFilterListsOnlyVisibleLinks() {
        val items = listOf(item("link", false, url = "https://a.example"), item("hidden-link", true, url = "https://b.example"))
        assertEquals(listOf("link"), FeedFiltering.apply(items, FeedFilter.Links, "").map { it.id })
    }
}
