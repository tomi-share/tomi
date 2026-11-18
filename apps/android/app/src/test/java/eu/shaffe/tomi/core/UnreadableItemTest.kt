package eu.shaffe.tomi.core

import eu.shaffe.tomi.ui.feed.FeedFilter
import eu.shaffe.tomi.ui.feed.FeedFiltering
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

@OptIn(ExperimentalCoroutinesApi::class)
class UnreadableItemTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-04T10:00:00Z")

    private fun unreadable(readPolicy: ReadPolicy = ReadPolicy.Normal, reason: UnreadableReason = UnreadableReason.UpdateRequired) = TomiItem(
        id = "u1",
        content = ItemContent.Unreadable(reason),
        sourceDevice = office,
        createdAt = now,
        readPolicy = readPolicy,
    )

    @Test
    fun anUnreadableItemExposesNoContentAndHasItsOwnKind() {
        val item = unreadable()
        assertFalse(item.isReadable)
        assertEquals(ItemKind.Unreadable, item.kind())
        assertNull(item.title)
        assertNull(item.text)
        assertNull(item.url)
        assertTrue(item.attachments.isEmpty())
        assertFalse(item.secret)
        assertEquals(UnreadableReason.UpdateRequired, (item.content as ItemContent.Unreadable).reason)
    }

    @Test
    fun aReadableItemKeepsItsKindAndShortcuts() {
        val secret = readableItem("s", office, now, title = "Wi-Fi", text = RichText("pw"), secret = true)
        assertTrue(secret.isReadable)
        assertEquals(ItemKind.Secret, secret.kind())
        assertEquals("Wi-Fi", secret.title)
        assertTrue(secret.secret)
    }

    @Test
    fun openingAnUnreadableCardNeverMarksItRead() {
        // For "after first read" the core would erase content the user never saw.
        assertFalse(unreadable(ReadPolicy.AfterFirstRead).shouldMarkReadWhenOpened("me"))
        assertFalse(unreadable().shouldMarkReadWhenOpened("me"))
        // Control cases: the rule is not simply "never".
        assertTrue(readableItem("t", office, now, text = RichText("x")).shouldMarkReadWhenOpened("me"))
        assertFalse(readableItem("t", office, now, text = RichText("x"), read = true).shouldMarkReadWhenOpened("me"))
        assertFalse(readableItem("s", office, now, text = RichText("x"), secret = true).shouldMarkReadWhenOpened("me"))
        assertFalse(readableItem("o", DeviceRef("me", "Pixel"), now, text = RichText("x")).shouldMarkReadWhenOpened("me"))
    }

    @Test
    fun theFeedKeepsAnUnreadableItemUnderAllButNeverMatchesContentFilters() {
        val all = listOf(unreadable(), readableItem("l", office, now, url = "https://a.example"))
        assertEquals(listOf("u1", "l"), FeedFiltering.apply(all, FeedFilter.All, "").map { it.id })
        for (filter in listOf(FeedFilter.Links, FeedFilter.Text, FeedFilter.Files, FeedFilter.Secrets)) {
            assertFalse("$filter", FeedFiltering.apply(listOf(unreadable()), filter, "").isNotEmpty())
        }
        // Still findable by the device it came from.
        assertEquals(listOf("u1"), FeedFiltering.apply(listOf(unreadable()), FeedFilter.All, "bureau").map { it.id })
    }

    @Test
    fun theFakeCoreDoesNotConsumeAnUnreadableItemWhenMarkedRead() = runTest {
        val core = FakeTomiCore(backgroundScope)
        core.simulateIncoming(ItemKind.Unreadable)
        val incoming = core.items.first().first { !it.isReadable && it.id != "demo-unreadable-1" }

        core.markRead(incoming.id)

        val after = core.getItem(incoming.id)
        assertNotNull("an unreadable item must survive markRead", after)
        assertFalse(after!!.read)
    }

    @Test
    fun sourceFormatIsDerivedWhenTheCoreDoesNotProvideIt() {
        assertEquals(SourceFormat.Plain, RichText("a").sourceFormat)
        assertEquals(SourceFormat.Markdown, RichText("a", markdown = "**a**").sourceFormat)
        assertEquals(SourceFormat.Html, RichText("a", html = "<b>a</b>").sourceFormat)
        // An explicit value from the core wins.
        assertEquals(SourceFormat.Html, RichText("a", html = "<b>a</b>", markdown = "**a**", sourceFormat = SourceFormat.Html).sourceFormat)
    }
}
