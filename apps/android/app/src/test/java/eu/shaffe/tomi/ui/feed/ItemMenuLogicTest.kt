package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Archive
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Copy
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.CopyLink
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Delete
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Details
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.OpenLink
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Pin
import eu.shaffe.tomi.ui.feed.ItemMenuEntry.Share
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** The menu of a card is short: what the card does with one tap is not repeated first, and the two deletions are one entry. */
class ItemMenuLogicTest {
    private val now = Instant.parse("2026-11-05T10:00:00Z")
    private val source = DeviceRef("pc", "PC-Bureau")

    private fun item(
        text: RichText? = null,
        url: String? = null,
        attachments: List<Attachment> = emptyList(),
        secret: Boolean = false,
    ) = readableItem(id = "1", sourceDevice = source, createdAt = now, text = text, url = url, attachments = attachments, secret = secret)

    /** The entries as the card asks for them: its own copy icon decides [ItemMenuLogic.entries]'s `hasInlineCopy`. */
    private fun entries(
        item: TomiItem,
        revealed: Boolean = false,
        canArchive: Boolean = true,
        canShare: Boolean = true,
    ) = ItemMenuLogic.entries(item, revealed, canArchive, hasInlineCopy = CardTap.showsCopyButton(item), canShare = canShare)

    @Test
    fun aTextCardDoesNotRepeatItsCopyIcon() {
        assertEquals(listOf(Share, Pin, Archive, Details, Delete), entries(item(text = RichText("court"))))
    }

    @Test
    fun aLinkCardOpensItWithoutRepeatingTheIconOrTheTap() {
        // The copy icon copies the link, the tap opens it: the menu keeps "Ouvrir" (and nothing else about the link).
        assertEquals(listOf(OpenLink, Share, Pin, Archive, Details, Delete), entries(item(url = "https://a.example")))
    }

    @Test
    fun aTextAndLinkCardOffersTheLinkApart() {
        assertEquals(
            listOf(OpenLink, CopyLink, Share, Pin, Archive, Details, Delete),
            entries(item(text = RichText("court"), url = "https://a.example")),
        )
    }

    @Test
    fun aHiddenItemKeepsCopyInTheMenuBecauseItsCopyIsGated() {
        val secret = item(text = RichText("Hx7!pQ2"), secret = true)
        assertEquals(listOf(Copy, Share, Pin, Archive, Details, Delete), entries(secret))
    }

    @Test
    fun aHiddenLinkOpensOnlyOnceRevealedAndNeverOffersTheLinkApart() {
        val secret = item(text = RichText("note"), url = "https://a.example", secret = true)
        assertEquals(listOf(Copy, Share, Pin, Archive, Details, Delete), entries(secret, revealed = false))
        assertEquals(listOf(Copy, OpenLink, Share, Pin, Archive, Details, Delete), entries(secret, revealed = true))
    }

    @Test
    fun aFileHasNoCopyNorLinkEntry() {
        val file = item(attachments = listOf(Attachment("a.pdf", "application/pdf", 10)))
        assertEquals(listOf(Share, Pin, Archive, Details, Delete), entries(file))
    }

    @Test
    fun anItemThatCannotBeArchivedOrSharedHasNoSuchEntry() {
        assertEquals(listOf(Pin, Details, Delete), entries(item(text = RichText("court")), canArchive = false, canShare = false))
    }

    @Test
    fun deleteIsAlwaysTheLastAndTheOnlyDeletion() {
        val all = listOf(
            item(text = RichText("a")), item(url = "https://a.example"), item(text = RichText("a"), url = "https://a.example"),
            item(text = RichText("s"), secret = true), item(attachments = listOf(Attachment("a.png", "image/png", 10))),
        )
        for (item in all) {
            val list = entries(item, revealed = true)
            assertEquals(Delete, list.last())
            assertEquals(1, list.count { it == Delete })
            assertTrue(Details in list && Pin in list)
        }
    }
}
