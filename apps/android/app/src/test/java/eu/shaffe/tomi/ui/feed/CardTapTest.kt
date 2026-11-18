package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.ui.render.ClipDetection
import eu.shaffe.tomi.ui.render.RenderMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class CardTapTest {
    private val now = Instant.parse("2026-11-05T10:00:00Z")
    private val source = DeviceRef("pc", "PC-Bureau")

    private fun item(
        title: String? = null,
        text: RichText? = null,
        url: String? = null,
        attachments: List<Attachment> = emptyList(),
        secret: Boolean = false,
    ) = readableItem(
        id = "1", sourceDevice = source, createdAt = now,
        title = title, text = text, url = url, attachments = attachments, secret = secret,
    )

    private val png = Attachment("a.png", "image/png", 10)
    private val png2 = Attachment("b.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)
    private val zip = Attachment("a.zip", "application/zip", 10)
    private val rs = Attachment("main.rs", "text/x-rust", 10)
    private val js = Attachment("app.js", "text/javascript", 10)
    private val kt = Attachment("Notes.kt", "application/octet-stream", 10)
    private val cut = CardOverflow(body = true)
    private val titleCut = CardOverflow(title = true)

    // --- links ---------------------------------------------------------------------------------------------------

    @Test
    fun tapOnALinkOpensIt() {
        assertEquals(CardTapAction.OpenLink, CardTap.actionFor(item(url = "https://a.example"), revealed = false))
        assertEquals(CardTapAction.OpenLink, CardTap.actionFor(item(title = "Page", url = "https://a.example"), revealed = false))
    }

    @Test
    fun aLongPageTitleDoesNotStopALinkFromOpening() {
        val link = item(title = "Un titre de page vraiment très long ".repeat(5), url = "https://a.example")
        assertEquals(CardTapAction.OpenLink, CardTap.actionFor(link, revealed = false, overflow = titleCut))
        assertFalse(CardTap.showsReadMore(link, revealed = false, overflow = titleCut))
    }

    @Test
    fun aLinkWithACutTextBodyOpensTheDetail() {
        val link = item(text = RichText("long ".repeat(200)), url = "https://a.example")
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(link, revealed = false, overflow = cut))
        assertTrue(CardTap.showsReadMore(link, revealed = false, overflow = cut))
    }

    @Test
    fun aUrlThatCannotBeOpenedFallsBackToTheDetail() {
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(url = "ftp://a.example/x"), revealed = false))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(url = "javascript:alert(1)"), revealed = false))
        // No "Lire la suite": nothing is cut, the detail is only a fallback.
        assertFalse(CardTap.showsReadMore(item(url = "ftp://a.example/x"), revealed = false, overflow = CardOverflow()))
    }

    // --- hidden items --------------------------------------------------------------------------------------------

    @Test
    fun tapOnAHiddenItemRevealsItAndNeverOpensTheDetail() {
        val secret = item(text = RichText("Hx7!pQ2"), secret = true)
        assertEquals(CardTapAction.Reveal, CardTap.actionFor(secret, revealed = false))
        // Nothing is rendered while hidden, so a stale overflow flag must not change anything or show "Lire la suite".
        assertEquals(CardTapAction.Reveal, CardTap.actionFor(secret, revealed = false, overflow = cut))
        assertFalse(CardTap.showsReadMore(secret, revealed = false, overflow = cut))
        val secretLink = item(url = "https://a.example", secret = true)
        assertEquals(CardTapAction.Reveal, CardTap.actionFor(secretLink, revealed = false))
    }

    @Test
    fun secondTapOnARevealedItemHidesItAgainUnlessItIsCut() {
        val secret = item(text = RichText("Hx7!pQ2"), secret = true)
        assertEquals(CardTapAction.Hide, CardTap.actionFor(secret, revealed = true))
        assertFalse(CardTap.showsReadMore(secret, revealed = true, overflow = CardOverflow()))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(secret, revealed = true, overflow = cut))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(secret, revealed = true, overflow = titleCut))
        assertTrue(CardTap.showsReadMore(secret, revealed = true, overflow = cut))
    }

    @Test
    fun aRevealedItemNeverCopiesOrOpensOnTap() {
        // The sensitive copy goes through the menu / the detail toolbar (clipboard flag, 30 s clearing).
        val secretLink = item(url = "https://a.example", secret = true)
        assertEquals(CardTapAction.Hide, CardTap.actionFor(secretLink, revealed = true))
        val secretFile = item(attachments = listOf(pdf), secret = true)
        assertEquals(CardTapAction.Hide, CardTap.actionFor(secretFile, revealed = true))
    }

    // --- text ----------------------------------------------------------------------------------------------------

    @Test
    fun tapOnAShortTextNeverCopiesIt() {
        // The tap only marks the item read (handleTap always does that first); it copies nothing and opens nothing.
        assertEquals(CardTapAction.None, CardTap.actionFor(item(text = RichText("court")), revealed = false))
        assertEquals(CardTapAction.None, CardTap.actionFor(item(title = "Titre", text = RichText("court")), revealed = false))
        assertEquals(CardTapAction.None, CardTap.actionFor(item(text = RichText("a", markdown = "**a**")), revealed = false))
        assertEquals(CardTapAction.None, CardTap.actionFor(item(text = RichText("a", html = "<b>a</b>")), revealed = false))
        assertFalse(CardTap.showsReadMore(item(text = RichText("court")), revealed = false, overflow = CardOverflow()))
    }

    @Test
    fun noTapActionCopiesAnyMore() {
        assertTrue(CardTapAction.entries.none { it.name == "Copy" })
    }

    @Test
    fun aTextCardHasAnExplicitCopyButton() {
        assertTrue(CardTap.showsCopyButton(item(text = RichText("court"))))
        assertTrue(CardTap.showsCopyButton(item(text = RichText("a", markdown = "**a**"))))
        assertTrue(CardTap.showsCopyButton(item(text = RichText("a", html = "<b>a</b>"))))
        assertTrue(CardTap.showsCopyButton(item(title = "Titre", text = RichText("court"), url = "https://a.example")))
    }

    @Test
    fun noCopyButtonWithoutTextNorOnAHiddenItem() {
        assertFalse(CardTap.showsCopyButton(item(url = "https://a.example")))
        assertFalse(CardTap.showsCopyButton(item(title = "Titre")))
        assertFalse(CardTap.showsCopyButton(item(attachments = listOf(pdf))))
        // A hidden item copies through its menu, behind the biometric gate and the sensitive clipboard.
        assertFalse(CardTap.showsCopyButton(item(text = RichText("Hx7!pQ2"), secret = true)))
        val unreadable = TomiItem(
            id = "u", content = ItemContent.Unreadable(UnreadableReason.UpdateRequired), sourceDevice = source, createdAt = now,
        )
        assertFalse(CardTap.showsCopyButton(unreadable))
    }

    @Test
    fun tapOnACutTextOpensTheDetail() {
        val long = item(text = RichText("long ".repeat(300)))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(long, revealed = false, overflow = cut))
        assertTrue(CardTap.showsReadMore(long, revealed = false, overflow = cut))
        val rich = item(text = RichText("a", markdown = "# a\n".repeat(100)))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(rich, revealed = false, overflow = cut))
        assertTrue(CardTap.showsReadMore(rich, revealed = false, overflow = cut))
    }

    @Test
    fun aCutTitleOpensTheDetailForATextItem() {
        val text = item(title = "titre ".repeat(50), text = RichText("court"))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(text, revealed = false, overflow = titleCut))
        assertTrue(CardTap.showsReadMore(text, revealed = false, overflow = titleCut))
    }

    @Test
    fun aTitleOnlyItemHasNothingToCopy() {
        assertEquals(CardTapAction.None, CardTap.actionFor(item(title = "Titre"), revealed = false))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(title = "titre ".repeat(50)), revealed = false, overflow = titleCut))
    }

    // --- files and images ----------------------------------------------------------------------------------------

    @Test
    fun tapOnASingleFileOpensItWithTheSystemViewer() {
        assertEquals(CardTapAction.OpenAttachment, CardTap.actionFor(item(attachments = listOf(pdf)), revealed = false))
        assertEquals(CardTapAction.OpenAttachment, CardTap.actionFor(item(attachments = listOf(zip)), revealed = false))
    }

    @Test
    fun tapOnASingleCodeFileOpensTheCodeScreen() {
        assertEquals(CardTapAction.OpenCodeFile, CardTap.actionFor(item(attachments = listOf(rs)), revealed = false))
        assertEquals(CardTapAction.OpenCodeFile, CardTap.actionFor(item(attachments = listOf(kt)), revealed = false))
        assertFalse(CardTap.showsReadMore(item(attachments = listOf(rs)), revealed = false, overflow = CardOverflow()))
    }

    @Test
    fun tapOnAHiddenItemWithACodeFileKeepsItsHiddenItemMeaning() {
        val secretCode = item(attachments = listOf(rs), secret = true)
        assertEquals(CardTapAction.Reveal, CardTap.actionFor(secretCode, revealed = false))
        assertEquals(CardTapAction.Hide, CardTap.actionFor(secretCode, revealed = true))
    }

    @Test
    fun severalCodeFilesOrAMixOpenTheDetailAndEachRowRoutesByItsType() {
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(attachments = listOf(rs, js)), revealed = false))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(attachments = listOf(png, rs)), revealed = false))
        // Row by row: image -> image viewer, code -> code screen, other file -> inert.
        assertEquals(AttachmentRowAction.OpenImage, AttachmentRowAction.of(png, imageOpenable = true, codeOpenable = true))
        assertEquals(AttachmentRowAction.OpenCodeFile, AttachmentRowAction.of(rs, imageOpenable = true, codeOpenable = true))
        assertEquals(AttachmentRowAction.None, AttachmentRowAction.of(pdf, imageOpenable = true, codeOpenable = true))
        // A screen that gave no callback for a type leaves that row inert.
        assertEquals(AttachmentRowAction.None, AttachmentRowAction.of(png, imageOpenable = false, codeOpenable = true))
        assertEquals(AttachmentRowAction.None, AttachmentRowAction.of(rs, imageOpenable = true, codeOpenable = false))
    }

    @Test
    fun anImageNamedLikeCodeIsStillAnImage() {
        val odd = Attachment("diagram.rs", "image/png", 10)
        assertEquals(AttachmentRowAction.OpenImage, AttachmentRowAction.of(odd, imageOpenable = true, codeOpenable = true))
        assertEquals(CardTapAction.OpenImage, CardTap.actionFor(item(attachments = listOf(odd)), revealed = false))
    }

    @Test
    fun tapOnASingleImageOpensTheInternalViewer() {
        assertEquals(CardTapAction.OpenImage, CardTap.actionFor(item(attachments = listOf(png)), revealed = false))
        // Whatever the format of the image: the viewer is for every image/* attachment.
        val jpeg = Attachment("a.jpg", "image/jpeg", 10)
        assertEquals(CardTapAction.OpenImage, CardTap.actionFor(item(attachments = listOf(jpeg)), revealed = false))
        assertFalse(CardTap.showsReadMore(item(attachments = listOf(png)), revealed = false, overflow = CardOverflow()))
    }

    @Test
    fun tapOnAnImageWithACutTextOrTitleOpensTheDetailFirst() {
        val image = item(text = RichText("long ".repeat(300)), attachments = listOf(png))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(image, revealed = false, overflow = cut))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(title = "t", attachments = listOf(png)), revealed = false, overflow = titleCut))
    }

    @Test
    fun tapOnAHiddenItemWithAnImageNeverOpensTheViewerDirectly() {
        // The image of a hidden item is reached only after "Afficher" (thumbnail / name of the revealed card), then the viewer
        // asks the biometric gate itself. The tap on the card keeps its hidden-item meaning.
        val secretImage = item(attachments = listOf(png), secret = true)
        assertEquals(CardTapAction.Reveal, CardTap.actionFor(secretImage, revealed = false))
        assertEquals(CardTapAction.Hide, CardTap.actionFor(secretImage, revealed = true))
    }

    @Test
    fun tapOnSeveralAttachmentsOpensTheDetail() {
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(attachments = listOf(png, png2)), revealed = false))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(item(attachments = listOf(png, pdf)), revealed = false))
        // Several files is not a truncation of text: no "Lire la suite".
        assertFalse(CardTap.showsReadMore(item(attachments = listOf(png, png2)), revealed = false, overflow = CardOverflow()))
    }

    @Test
    fun aFileWithACutTextOpensTheDetail() {
        val file = item(text = RichText("long ".repeat(300)), attachments = listOf(pdf))
        assertEquals(CardTapAction.OpenDetail, CardTap.actionFor(file, revealed = false, overflow = cut))
    }

    // --- unreadable ----------------------------------------------------------------------------------------------

    @Test
    fun tapOnAnUnreadableItemDoesNothing() {
        val unreadable = TomiItem(
            id = "u", content = ItemContent.Unreadable(UnreadableReason.UpdateRequired), sourceDevice = source, createdAt = now,
        )
        assertEquals(CardTapAction.None, CardTap.actionFor(unreadable, revealed = false))
        assertEquals(CardTapAction.None, CardTap.actionFor(unreadable, revealed = false, overflow = cut))
        assertFalse(CardTap.showsReadMore(unreadable, revealed = false, overflow = cut))
    }

    // --- dispatch ------------------------------------------------------------------------------------------------

    private class Recorder {
        val calls = mutableListOf<String>()
        var copyMode: RenderMode? = null
        val actions = CardActions(
            onOpenCodeFile = { index -> calls += "code:$index" },
            onCardOpened = { calls += "opened" },
            onOpenDetail = { calls += "detail" },
            onOpenAttachment = { calls += "attachment" },
            onOpenImage = { index -> calls += "image:$index" },
            onReveal = { calls += "reveal" },
            onHide = { calls += "hide" },
            onCopy = { mode ->
                copyMode = mode
                calls += "copy"
            },
            onOpenLink = { calls += "link" },
        )
    }

    @Test
    fun everyTapMarksTheItemOpenedFirstThenDoesItsAction() {
        val expected = mapOf(
            CardTapAction.OpenLink to listOf("opened", "link"),
            CardTapAction.Reveal to listOf("opened", "reveal"),
            CardTapAction.Hide to listOf("opened", "hide"),
            CardTapAction.OpenAttachment to listOf("opened", "attachment"),
            CardTapAction.OpenCodeFile to listOf("opened", "code:0"),
            CardTapAction.OpenImage to listOf("opened", "image:0"),
            CardTapAction.OpenDetail to listOf("opened", "detail"),
            CardTapAction.None to listOf("opened"),
        )
        assertEquals(CardTapAction.entries.toSet(), expected.keys)
        expected.forEach { (tap, calls) ->
            val recorder = Recorder()
            recorder.actions.handleTap(tap)
            assertEquals(tap.name, calls, recorder.calls)
        }
    }

    @Test
    fun theTapOnATextCardNeverReachesTheCopyCallback() {
        val recorder = Recorder()
        recorder.actions.handleTap(CardTap.actionFor(item(text = RichText("court")), revealed = false))
        // Marked read, nothing else: no copy, no detail.
        assertEquals(listOf("opened"), recorder.calls)
        assertEquals(null, recorder.copyMode)
    }

    @Test
    fun theCopyIconCallsTheSameCopyActionAsTheMenu() {
        val recorder = Recorder()
        recorder.actions.onCopy(RenderMode.Source)
        assertEquals(listOf("copy"), recorder.calls)
        assertEquals(RenderMode.Source, recorder.copyMode)
    }

    // --- clipping of Markdown / HTML -----------------------------------------------------------------------------

    @Test
    fun contentTallerThanItsViewIsClipped() {
        // 220 dp at density 2.75 = 605 px visible; 900 px of content.
        assertTrue(ClipDetection.isClipped(contentHeight = 900, visibleHeight = 605))
    }

    @Test
    fun contentThatFitsOrIsWithinRoundingIsNotClipped() {
        assertFalse(ClipDetection.isClipped(contentHeight = 300, visibleHeight = 300))
        assertFalse(ClipDetection.isClipped(contentHeight = 100, visibleHeight = 300))
        assertFalse(ClipDetection.isClipped(contentHeight = 302, visibleHeight = 300))
        assertTrue(ClipDetection.isClipped(contentHeight = 303, visibleHeight = 300))
    }

    @Test
    fun aViewNotLaidOutYetIsNeverReportedAsClipped() {
        assertFalse(ClipDetection.isClipped(contentHeight = 900, visibleHeight = 0))
    }
}
