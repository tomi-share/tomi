package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.ReadPolicy
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.SourceFormat
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.notify.NotificationButton.Copy
import eu.shaffe.tomi.notify.NotificationButton.Save
import eu.shaffe.tomi.notify.NotificationButton.Share
import eu.shaffe.tomi.notify.NotificationButton.View
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** What a notification of a received item may say and offer (no Android type: this is the part that must never leak). */
class NotificationContentTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-10T10:00:00Z")

    private fun text(body: String = "bonjour", readPolicy: ReadPolicy = ReadPolicy.Normal) =
        readableItem("t", office, now, text = RichText(body), readPolicy = readPolicy)

    private fun markdown() = readableItem("m", office, now, text = RichText("# Titre", markdown = "# Titre", sourceFormat = SourceFormat.Markdown))
    private fun link(title: String? = null, readPolicy: ReadPolicy = ReadPolicy.Normal) =
        readableItem("l", office, now, title = title, url = "https://example.com/a?token=1", readPolicy = readPolicy)

    private fun image(secret: Boolean = false) =
        readableItem("i", office, now, attachments = listOf(Attachment("photo.jpg", "image/jpeg", 1200)), secret = secret)

    private fun file() = readableItem("f", office, now, attachments = listOf(Attachment("notes.pdf", "application/pdf", 3400)))
    private fun hidden() = readableItem("h", office, now, text = RichText("pw"), secret = true)
    private fun unreadable() = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), office, now)

    // --- what is concealed -------------------------------------------------------------------------------------------------------

    @Test
    fun ordinaryItemsAreNotConcealed() {
        listOf(text(), markdown(), link(), image(), file()).forEach { assertFalse(NotificationContent.isConcealed(it)) }
    }

    @Test
    fun aHiddenItemAndAnUnreadableOneAreConcealed() {
        assertTrue(NotificationContent.isConcealed(hidden()))
        assertTrue(NotificationContent.isConcealed(unreadable()))
        assertTrue(NotificationContent.isConcealed(image(secret = true)))
    }

    @Test
    fun anAfterFirstReadItemIsConcealedEvenWhenItIsNotHidden() {
        // The first copy, open or share deletes it everywhere: a preview or a button on its notification would consume it unseen.
        assertTrue(NotificationContent.isConcealed(text(readPolicy = ReadPolicy.AfterFirstRead)))
        assertTrue(NotificationContent.isConcealed(link(readPolicy = ReadPolicy.AfterFirstRead)))
    }

    @Test
    fun aConcealedItemHasNoButtonAtAll() {
        listOf(hidden(), unreadable(), image(secret = true), text(readPolicy = ReadPolicy.AfterFirstRead)).forEach {
            assertEquals(emptyList<NotificationButton>(), NotificationContent.buttonsFor(it))
        }
    }

    @Test
    fun aTapOnAnAfterFirstReadItemOpensTheCardInsteadOfConsumingIt() {
        assertEquals(TapAction.OpenCard, TapAction.forItem(text(readPolicy = ReadPolicy.AfterFirstRead)))
        assertEquals(TapAction.OpenCard, TapAction.forItem(link(readPolicy = ReadPolicy.AfterFirstRead)))
        // The others keep their obvious action.
        assertEquals(TapAction.CopyText, TapAction.forItem(text()))
        assertEquals(TapAction.OpenLink, TapAction.forItem(link()))
        assertEquals(TapAction.AuthenticateAndCopy, TapAction.forItem(hidden()))
    }

    @Test
    fun aSummaryWithAnAfterFirstReadItemIsHidden() {
        val consumed = text(readPolicy = ReadPolicy.AfterFirstRead)
        assertTrue(IncomingNotifications.summaryHasSecret(listOf(text(), consumed), text()))
        assertTrue(IncomingNotifications.summaryHasSecret(emptyList(), consumed))
    }

    // --- buttons -----------------------------------------------------------------------------------------------------------------

    @Test
    fun aLinkOffersCopyAndShare() {
        assertEquals(listOf(Copy, Share), NotificationContent.buttonsFor(link()))
    }

    @Test
    fun aShortTextIsCopiedByTheTapSoItOffersViewAndShare() {
        assertEquals(listOf(View, Share), NotificationContent.buttonsFor(text("bonjour")))
    }

    @Test
    fun aLongOrRichTextOffersCopyAndShare() {
        assertEquals(listOf(Copy, Share), NotificationContent.buttonsFor(text("x".repeat(500))))
        assertEquals(listOf(Copy, Share), NotificationContent.buttonsFor(text("a\nb")))
        assertEquals(listOf(Copy, Share), NotificationContent.buttonsFor(markdown()))
    }

    @Test
    fun anImageAndAFileOfferSaveAndShare() {
        assertEquals(listOf(Save, Share), NotificationContent.buttonsFor(image()))
        assertEquals(listOf(Save, Share), NotificationContent.buttonsFor(file()))
    }

    // --- link host ---------------------------------------------------------------------------------------------------------------

    @Test
    fun theHostHasNoPathQueryUserOrPort() {
        assertEquals("example.com", NotificationContent.hostOf("https://Example.COM:8443/a/b?token=secret#frag"))
        assertEquals("good.com", NotificationContent.hostOf("https://evil.com@good.com/"))
        assertEquals("good.com", NotificationContent.hostOf("https://good.com\\@evil.com/"))
        assertEquals("example.com", NotificationContent.hostOf("  http://example.com.  "))
    }

    @Test
    fun anInternationalisedHostIsShownAsPunycode() {
        assertEquals("xn--bcher-kva.example", NotificationContent.hostOf("https://bücher.example/"))
        // A Cyrillic "а" in "apple": the look-alike is visible once it is punycode.
        assertTrue(NotificationContent.hostOf("https://аpple.com/")!!.startsWith("xn--"))
    }

    @Test
    fun aHostThatIsNotAnHttpHostIsNull() {
        assertNull(NotificationContent.hostOf(null))
        assertNull(NotificationContent.hostOf(""))
        assertNull(NotificationContent.hostOf("ftp://example.com/"))
        assertNull(NotificationContent.hostOf("javascript:alert(1)"))
        assertNull(NotificationContent.hostOf("https:///path"))
    }

    @Test
    fun anIpv6HostKeepsItsBrackets() {
        assertEquals("[::1]", NotificationContent.hostOf("http://[::1]:8080/x"))
    }

    // --- bounded text ------------------------------------------------------------------------------------------------------------

    @Test
    fun aTextWithinTheLimitIsKeptAsIs() {
        assertEquals("abcd", NotificationContent.bounded("abcd", 4))
    }

    @Test
    fun aLongTextIsCutWithAnEllipsisWithinTheLimit() {
        assertEquals("abc…", NotificationContent.bounded("abcdef", 4))
        assertEquals(NotificationContent.MAX_BODY, NotificationContent.bounded("x".repeat(500), NotificationContent.MAX_BODY).length)
    }

    @Test
    fun aCutNeverLeavesHalfOfASurrogatePair() {
        val text = "a".repeat(118) + "😀" + "tail"
        val cut = NotificationContent.bounded(text, 120)
        assertFalse(cut.any { Character.isSurrogate(it) })
        assertTrue(cut.endsWith("…"))
    }

    @Test
    fun theExpandedTextIsLongerThanTheCollapsedOne() {
        val long = text("x".repeat(1000))
        assertEquals(NotificationContent.MAX_BODY, NotificationContent.collapsedText(long)!!.length)
        assertEquals(NotificationContent.MAX_BIG_TEXT, NotificationContent.expandedText(long)!!.length)
        assertNull(NotificationContent.collapsedText(text("   ")))
    }

    // --- picture size ------------------------------------------------------------------------------------------------------------

    @Test
    fun aSmallThumbnailIsNotReduced() {
        assertEquals(1, NotificationContent.sampleSizeFor(300, 200))
    }

    @Test
    fun aLargeThumbnailIsReducedByPowersOfTwoButKeepsItsEdge() {
        assertEquals(4, NotificationContent.sampleSizeFor(4000, 3000))
        assertEquals(2, NotificationContent.sampleSizeFor(900, 900))
        // The short side drives it: a panorama is not reduced below the edge.
        assertEquals(1, NotificationContent.sampleSizeFor(8000, 600))
    }

    @Test
    fun anUnusableSizeIsNotReduced() {
        assertEquals(1, NotificationContent.sampleSizeFor(0, 10))
        assertEquals(1, NotificationContent.sampleSizeFor(10, -1))
    }

    // --- which notifications stay ------------------------------------------------------------------------------------------------

    @Test
    fun anItemStaysPendingOnlyWhileUnreadAndNotArchived() {
        assertTrue(NotificationContent.isPending(text()))
        assertFalse(NotificationContent.isPending(readableItem("r", office, now, text = RichText("x"), read = true)))
        assertFalse(NotificationContent.isPending(readableItem("a", office, now, text = RichText("x"), archived = true)))
    }
}
