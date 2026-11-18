package eu.shaffe.tomi.ui

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.DeliveryStatus
import eu.shaffe.tomi.core.DeviceDelivery
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemKind
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.SourceFormat
import eu.shaffe.tomi.core.kind
import eu.shaffe.tomi.notify.NotificationBatcher
import eu.shaffe.tomi.notify.TapAction
import eu.shaffe.tomi.security.AuthWindow
import eu.shaffe.tomi.security.ClipboardClearPolicy
import eu.shaffe.tomi.ui.feed.FeedFilter
import eu.shaffe.tomi.ui.feed.FeedFiltering
import eu.shaffe.tomi.ui.render.HtmlSanitizer
import eu.shaffe.tomi.ui.render.PreviewKind
import eu.shaffe.tomi.ui.render.RenderMode
import eu.shaffe.tomi.ui.render.RenderModes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class ItemLogicTest {
    private val me = "me"
    private val office = DeviceRef("pc", "PC-Bureau")
    private val phone = DeviceRef(me, "Pixel")
    private val now = Instant.parse("2026-11-04T10:00:00Z")

    private fun item(
        id: String = "1",
        title: String? = null,
        text: RichText? = null,
        url: String? = null,
        attachments: List<Attachment> = emptyList(),
        secret: Boolean = false,
        source: DeviceRef = office,
        deliveries: List<DeviceDelivery> = emptyList(),
    ) = readableItem(
        id = id, title = title, text = text, url = url, attachments = attachments, secret = secret,
        sourceDevice = source, createdAt = now, deliveries = deliveries,
    )

    // --- kind and tap action -------------------------------------------------------------------------------------

    @Test
    fun kindFollowsTheCompositeFieldsAndASecretAlwaysWins() {
        assertEquals(ItemKind.Link, item(url = "https://a.example").kind())
        assertEquals(ItemKind.Secret, item(url = "https://a.example", secret = true).kind())
        assertEquals(ItemKind.Image, item(attachments = listOf(Attachment("a.png", "image/png", 1))).kind())
        assertEquals(ItemKind.File, item(attachments = listOf(Attachment("a.pdf", "application/pdf", 1))).kind())
        assertEquals(ItemKind.RichText, item(text = RichText("a", markdown = "# a")).kind())
        assertEquals(ItemKind.Text, item(text = RichText("a")).kind())
    }

    @Test
    fun notificationTapDoesTheObviousAction() {
        assertEquals(TapAction.OpenLink, TapAction.forItem(item(url = "https://a.example")))
        assertEquals(TapAction.CopyText, TapAction.forItem(item(text = RichText("court"))))
        assertEquals(TapAction.OpenCard, TapAction.forItem(item(text = RichText("x".repeat(200)))))
        assertEquals(TapAction.OpenCard, TapAction.forItem(item(text = RichText("deux\nlignes"))))
        assertEquals(TapAction.OpenCard, TapAction.forItem(item(text = RichText("a", html = "<b>a</b>"))))
        assertEquals(TapAction.PreviewImage, TapAction.forItem(item(attachments = listOf(Attachment("a.jpg", "image/jpeg", 1)))))
        assertEquals(TapAction.OpenFile, TapAction.forItem(item(attachments = listOf(Attachment("a.pdf", "application/pdf", 1)))))
        // A code file is read in Tomi's own code screen: the notification opens the card instead of another app.
        assertEquals(TapAction.OpenCard, TapAction.forItem(item(attachments = listOf(Attachment("main.rs", "text/x-rust", 1)))))
        assertEquals(TapAction.AuthenticateAndCopy, TapAction.forItem(item(text = RichText("pw"), secret = true)))
    }

    // --- notification batching -----------------------------------------------------------------------------------

    @Test
    fun arrivalsLessThanTenSecondsApartAreGrouped() {
        val batcher = NotificationBatcher()
        assertEquals(NotificationBatcher.Decision.Single, batcher.onArrival("a", 0))
        assertEquals(NotificationBatcher.Decision.Summary(listOf("a", "b")), batcher.onArrival("b", 9_000))
        assertEquals(NotificationBatcher.Decision.Summary(listOf("a", "b", "c")), batcher.onArrival("c", 18_000))
        // A quiet period of 10 s or more starts a new batch.
        assertEquals(NotificationBatcher.Decision.Single, batcher.onArrival("d", 28_000))
    }

    // --- delivery status -----------------------------------------------------------------------------------------

    @Test
    fun deliveryStatusSummarisesPerDeviceStates() {
        fun d(id: String, state: DeliveryState) = DeviceDelivery(id, "dev-$id", state)
        assertEquals(DeliveryStatus.Received, DeliveryStatus.of(item(source = office), me))
        assertEquals(DeliveryStatus.Queued, DeliveryStatus.of(item(source = phone, deliveries = listOf(d("1", DeliveryState.Queued))), me))
        assertEquals(DeliveryStatus.Sent, DeliveryStatus.of(item(source = phone, deliveries = listOf(d("1", DeliveryState.Sent))), me))
        assertEquals(
            DeliveryStatus.Delivered(1, 2),
            DeliveryStatus.of(item(source = phone, deliveries = listOf(d("1", DeliveryState.Delivered), d("2", DeliveryState.Sent))), me),
        )
        assertEquals(
            DeliveryStatus.ExpiredBeforeDelivery(1, listOf("dev-2")),
            DeliveryStatus.of(item(source = phone, deliveries = listOf(d("1", DeliveryState.Delivered), d("2", DeliveryState.ExpiredBeforeDelivery))), me),
        )
    }

    // --- rendering mode ------------------------------------------------------------------------------------------

    @Test
    fun thereAreOnlyTwoRenderModesAndNoneShowsHtmlMarkup() {
        assertEquals(listOf(RenderMode.Preview, RenderMode.Source), RenderMode.entries)
    }

    @Test
    fun theMarkdownModeIsOnlyOfferedWhenTheItemHasMarkdown() {
        assertEquals(listOf(RenderMode.Preview, RenderMode.Source), RenderModes.available(RichText("a", markdown = "**a**")))
        // HTML only, or plain only: the preview is the one and only mode (no segmented control).
        assertEquals(listOf(RenderMode.Preview), RenderModes.available(RichText("a", html = "<b>a</b>")))
        assertEquals(listOf(RenderMode.Preview), RenderModes.available(RichText("a")))
        // Markdown and HTML together: still two modes, never a third one showing the HTML.
        assertEquals(
            listOf(RenderMode.Preview, RenderMode.Source),
            RenderModes.available(RichText("a", html = "<b>a</b>", markdown = "**a**")),
        )
    }

    @Test
    fun renderModeDefaultsToThePreviewThenToTheLastChoice() {
        val md = RichText("a", markdown = "**a**")
        val html = RichText("a", html = "<b>a</b>")
        val plain = RichText("a")
        assertEquals(RenderMode.Preview, RenderModes.resolve(md, null))
        assertEquals(RenderMode.Preview, RenderModes.resolve(html, null))
        assertEquals(RenderMode.Preview, RenderModes.resolve(plain, null))
        // The remembered choice applies when the item supports it.
        assertEquals(RenderMode.Source, RenderModes.resolve(md, RenderMode.Source))
        // An item without Markdown ignores a remembered "Markdown" mode.
        assertEquals(RenderMode.Preview, RenderModes.resolve(html, RenderMode.Source))
        assertEquals(RenderMode.Preview, RenderModes.resolve(plain, RenderMode.Source))
    }

    @Test
    fun copyPutsWhatIsDisplayed() {
        val both = RichText("plain", html = "<b>plain</b>", markdown = "# plain\n\n**x**")
        // Markdown mode shows the literal source (with its syntax), not the plain text.
        assertEquals("# plain\n\n**x**", RenderModes.copyContent(both, RenderMode.Source).plain)
        // Captured as Markdown: the preview is the rendered Markdown, its copy is the plain text.
        val markdownPreview = RenderModes.copyContent(both, RenderMode.Preview)
        assertEquals("plain", markdownPreview.plain)
        assertNull(markdownPreview.html)
        // Captured as HTML: the preview is the sandboxed HTML, its copy carries the HTML with the plain text.
        val htmlPreview = RenderModes.copyContent(both.copy(sourceFormat = SourceFormat.Html), RenderMode.Preview)
        assertEquals("plain", htmlPreview.plain)
        assertEquals("<b>plain</b>", htmlPreview.html)
        assertEquals("plain", RenderModes.copyContent(RichText("plain"), RenderMode.Preview).plain)
    }

    @Test
    fun thePreviewFollowsTheFormatTheContentWasCapturedIn() {
        assertEquals(PreviewKind.Markdown, RenderModes.previewKind(RichText("a", markdown = "**a**")))
        assertEquals(PreviewKind.Html, RenderModes.previewKind(RichText("a", html = "<b>a</b>")))
        assertEquals(PreviewKind.Plain, RenderModes.previewKind(RichText("a")))
        val both = RichText("a", html = "<b>a</b>", markdown = "**a**")
        assertEquals(PreviewKind.Markdown, RenderModes.previewKind(both.copy(sourceFormat = SourceFormat.Markdown)))
        assertEquals(PreviewKind.Html, RenderModes.previewKind(both.copy(sourceFormat = SourceFormat.Html)))
        assertEquals(PreviewKind.Plain, RenderModes.previewKind(both.copy(sourceFormat = SourceFormat.Plain)))
        // A declared format whose representation is missing falls back to the other one, then to the plain text.
        assertEquals(PreviewKind.Html, RenderModes.previewKind(RichText("a", html = "<b>a</b>", sourceFormat = SourceFormat.Markdown)))
        assertEquals(PreviewKind.Plain, RenderModes.previewKind(RichText("a", sourceFormat = SourceFormat.Html)))
        // Windows omits `markdown` when it equals the plain text: source_format alone says the plain text is Markdown.
        val declared = RichText("**a**", sourceFormat = SourceFormat.Markdown)
        assertEquals(PreviewKind.Markdown, RenderModes.previewKind(declared))
        assertEquals(listOf(RenderMode.Preview, RenderMode.Source), RenderModes.available(declared))
    }

    // --- feed filters and search ---------------------------------------------------------------------------------

    @Test
    fun filtersMatchTheCompositeFields() {
        val link = item(id = "l", url = "https://a.example", title = "Doc")
        val text = item(id = "t", text = RichText("hello"))
        val file = item(id = "f", attachments = listOf(Attachment("a.pdf", "application/pdf", 1)))
        val secret = item(id = "s", text = RichText("pw"), secret = true)
        val all = listOf(link, text, file, secret)
        assertEquals(listOf("l"), FeedFiltering.apply(all, FeedFilter.Links, "").map { it.id })
        // A hidden item is not classed by its content: it shows under "Masqués" only.
        assertEquals(listOf("t"), FeedFiltering.apply(all, FeedFilter.Text, "").map { it.id })
        assertEquals(listOf("f"), FeedFiltering.apply(all, FeedFilter.Files, "").map { it.id })
        assertEquals(listOf("s"), FeedFiltering.apply(all, FeedFilter.Secrets, "").map { it.id })
        assertEquals(4, FeedFiltering.apply(all, FeedFilter.All, "").size)
    }

    @Test
    fun searchNeverLooksInsideHiddenItemsNorAtTheirTitle() {
        val secret = item(id = "s", title = "Wi-Fi invités", text = RichText("Hx7!pQ2-zR9"), url = "https://wifi.example", secret = true)
        val text = item(id = "t", title = "Wi-Fi maison", text = RichText("rendez-vous Hx7"))
        // The title of a hidden item is excluded from the search, like its content.
        assertFalse(FeedFiltering.matchesQuery(secret, "wi-fi"))
        assertFalse(FeedFiltering.matchesQuery(secret, "invités"))
        assertFalse(FeedFiltering.matchesQuery(secret, "Hx7"))
        assertFalse(FeedFiltering.matchesQuery(secret, "wifi.example"))
        // It can still be found by the device it came from, and a normal title is searched.
        assertTrue(FeedFiltering.matchesQuery(secret, "bureau"))
        assertTrue(FeedFiltering.matchesQuery(text, "wi-fi"))
        // Whole-feed check: only the normal item answers to the title.
        assertEquals(listOf("t"), FeedFiltering.apply(listOf(secret, text), FeedFilter.All, "wi-fi").map { it.id })
        assertTrue(FeedFiltering.matchesQuery(text, "hx7"))
        assertTrue(FeedFiltering.matchesQuery(text, "bureau"))
    }

    // --- HTML sanitising -----------------------------------------------------------------------------------------

    @Test
    fun sanitiserRemovesScriptsHandlersFramesAndRemoteImages() {
        val dirty = """<p onclick="x()">hi</p><script>alert(1)</script><iframe src="https://e.example"></iframe>""" +
            """<img src="https://tracker.example/p.png"><img src="data:image/png;base64,AAAA">""" +
            """<a href="javascript:alert(1)">bad</a><form action="https://e.example"><input></form>"""
        val clean = HtmlSanitizer.clean(dirty)
        assertFalse(clean.contains("script", ignoreCase = true))
        assertFalse(clean.contains("onclick", ignoreCase = true))
        assertFalse(clean.contains("iframe", ignoreCase = true))
        assertFalse(clean.contains("tracker.example"))
        assertFalse(clean.contains("javascript:"))
        assertFalse(clean.contains("<form", ignoreCase = true))
        assertTrue(clean.contains("hi"))
        assertTrue(clean.contains("data:image/png"))
    }

    @Test
    fun sanitiserDropsInlineStylesOnEveryTag() {
        val dirty = """<p style="position:fixed;top:0;left:0;width:100%;height:100%">overlay</p>""" +
            """<span style="display:none">hidden</span><img src="data:image/png;base64,AAAA" style="width:1px">""" +
            """<table style="x"><tr><td style="y">cell</td></tr></table><a href="https://example.org/" style="color:red">link</a>"""
        val clean = HtmlSanitizer.clean(dirty)
        assertFalse(clean, clean.contains("style", ignoreCase = true))
        assertFalse(clean.contains("position", ignoreCase = true))
        // The content itself is kept.
        assertTrue(clean.contains("overlay"))
        assertTrue(clean.contains("cell"))
        assertTrue(clean.contains("""href="https://example.org/""""))
    }

    @Test
    fun generatedDocumentCarriesTheCspAndOnlyCleanedContent() {
        val doc = HtmlSanitizer.toDocument(
            """<p style="color:red" onclick="x()">kept</p><script>alert(1)</script><img src="https://tracker.example/p.png">""",
            "#000000",
            "#0000FF",
        )
        val parsed = org.jsoup.Jsoup.parse(doc)
        // The literal policy is written out here on purpose: the test must fail if the constant is weakened.
        val csp = parsed.select("meta[http-equiv=Content-Security-Policy]")
        assertEquals(1, csp.size)
        assertEquals("default-src 'none'; img-src data:; style-src 'unsafe-inline'", csp.attr("content"))
        // The policy lives in <head>, before any received content.
        assertEquals(1, parsed.head().select("meta[http-equiv=Content-Security-Policy]").size)
        // The body only holds the cleaned fragment.
        val body = parsed.body()
        assertEquals("kept", body.text())
        assertTrue(body.select("script, [onclick], [style]").isEmpty())
        // The remote image loses its source (the tag itself may stay, empty): nothing can be fetched.
        assertTrue(body.select("[src]").all { it.attr("src").startsWith("data:") })
        assertFalse(doc.contains("tracker.example"))
        assertTrue(doc.contains("color: #000000"))
        assertTrue(doc.contains("a { color: #0000FF; }"))
    }

    // --- biometric window and clipboard clearing -----------------------------------------------------------------

    @Test
    fun authenticationIsValidForTwoMinutes() {
        var clock = 0L
        val window = AuthWindow(nowMs = { clock })
        assertFalse(window.isValid())
        window.markAuthenticated()
        clock = 119_999
        assertTrue(window.isValid())
        clock = 120_000
        assertFalse(window.isValid())
        window.markAuthenticated()
        window.invalidate()
        assertFalse(window.isValid())
    }

    @Test
    fun clipboardIsClearedOnlyIfItStillHoldsOurClip() {
        assertTrue(ClipboardClearPolicy.shouldClear(expectedTimestamp = 100, currentTimestamp = 100))
        assertFalse(ClipboardClearPolicy.shouldClear(expectedTimestamp = 100, currentTimestamp = 250))
        // Unknown on either side: clear, because the content may be a secret.
        assertTrue(ClipboardClearPolicy.shouldClear(expectedTimestamp = 0, currentTimestamp = 250))
        assertTrue(ClipboardClearPolicy.shouldClear(expectedTimestamp = 100, currentTimestamp = null))
        assertEquals(30_000L, ClipboardClearPolicy.CLEAR_DELAY_MS)
    }

    @Test
    fun foregroundCheckOnlyClearsAnOverdueClipThatIsProvablyOurs() {
        val ours = 100L
        val deadline = 30_000L
        val action = { expected: Long, current: Long?, now: Long -> ClipboardClearPolicy.onForeground(expected, current, now, deadline) }
        // Not overdue yet: the background job is still in charge.
        assertEquals(ClipboardClearPolicy.ForegroundAction.Wait, action(ours, ours, 29_999))
        // Overdue and still ours: clear.
        assertEquals(ClipboardClearPolicy.ForegroundAction.Clear, action(ours, ours, 30_000))
        // The user copied something else: leave it alone, stop tracking.
        assertEquals(ClipboardClearPolicy.ForegroundAction.Forget, action(ours, 250L, 40_000))
        // We never knew our own timestamp: cannot prove anything, do not wipe.
        assertEquals(ClipboardClearPolicy.ForegroundAction.Forget, action(0L, 250L, 40_000))
        // Not readable (no focus yet, or empty): keep tracking and retry later.
        assertEquals(ClipboardClearPolicy.ForegroundAction.Retry, action(ours, null, 40_000))
    }
}
