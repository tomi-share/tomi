package eu.shaffe.tomi.ui.send

import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.share.SharePayloadParser
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A `tomi-share://send` link from a web page must never replace what the user is writing. */
class DeepLinkPrefillTest {
    private fun link(title: String? = "Page", url: String? = "https://a.example", text: String? = null) =
        DeepLink.Send(title, url, text, droppedParameters = false)

    @Test
    fun anEmptyDraftIsFilledByTheLink() {
        val result = DeepLinkPrefill.apply("", "", hasAttachments = false, link = link(text = "note"))
        assertEquals("Page", result.title)
        assertEquals("https://a.example\nnote", result.text)
        assertFalse(result.appended)
    }

    @Test
    fun aBlankDraftCountsAsEmpty() {
        assertFalse(DeepLinkPrefill.apply("  ", "\n ", hasAttachments = false, link = link()).appended)
    }

    @Test
    fun aDraftInProgressIsKeptAndTheLinkIsAppended() {
        val result = DeepLinkPrefill.apply("Mon titre", "mot de passe: hunter2", hasAttachments = false, link = link())
        assertEquals("Mon titre", result.title)
        assertEquals("mot de passe: hunter2\nhttps://a.example", result.text)
        assertTrue(result.appended)
    }

    @Test
    fun aDraftWithOnlyAttachmentsKeepsThemAndGetsTheLinkAsText() {
        val result = DeepLinkPrefill.apply("", "", hasAttachments = true, link = link())
        assertEquals("https://a.example", result.text)
        assertTrue(result.appended)
    }

    @Test
    fun theLinkTitleOnlyFillsAMissingTitle() {
        assertEquals("Page", DeepLinkPrefill.apply("", "texte", hasAttachments = false, link = link()).title)
        assertEquals("Mien", DeepLinkPrefill.apply("Mien", "texte", hasAttachments = false, link = link()).title)
    }

    @Test
    fun theAppendedTextStaysWithinTheLimit() {
        val result = DeepLinkPrefill.apply("", "x".repeat(SharePayloadParser.MAX_TEXT), hasAttachments = false, link = link())
        assertEquals(SharePayloadParser.MAX_TEXT, result.text.length)
    }

    @Test
    fun aLinkWithNothingToAddLeavesTheDraftAlone() {
        val result = DeepLinkPrefill.apply("T", "texte", hasAttachments = false, link = link(title = null, url = null, text = null))
        assertEquals("T", result.title)
        assertEquals("texte", result.text)
        assertFalse(result.appended)
        // The caller must not reset the draft (secret switch, attachments) when nothing was appended.
        assertTrue(result.keepsDraft)
    }

    @Test
    fun anEmptyDraftIsNotKept() {
        assertFalse(DeepLinkPrefill.apply("", "", hasAttachments = false, link = link()).keepsDraft)
    }
}
