package eu.shaffe.tomi.core

import eu.shaffe.tomi.deeplink.DeepLink
import eu.shaffe.tomi.share.ParsedShare
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** The models that carry clear content must not print it: toString ends up in logs, crash reports and test failures. */
class RedactionTest {
    private val clear = "Hx7!pQ2-zR9-s3cr3t"
    private val device = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-06T10:00:00Z")

    private fun assertRedacted(printed: String) {
        assertFalse(printed, printed.contains(clear))
        assertFalse(printed, printed.contains("PC-Bureau"))
        assertTrue(printed, printed.contains("redacted"))
    }

    @Test
    fun richTextPrintsNoRepresentation() {
        assertRedacted(RichText(plain = clear, html = "<b>$clear</b>", markdown = "**$clear**").toString())
    }

    @Test
    fun aTomiItemPrintsNoContentButKeepsItsIdAndTheSecretFlag() {
        val item = readableItem(
            id = "item-1",
            sourceDevice = device,
            createdAt = now,
            title = clear,
            text = RichText(clear, markdown = clear),
            url = "https://example.test/$clear",
            attachments = listOf(Attachment("$clear.txt", "text/plain", 3, uri = "content://x/$clear")),
            secret = true,
        )
        val printed = item.toString()
        assertRedacted(printed)
        assertTrue(printed, printed.contains("item-1"))
        assertTrue(printed, printed.contains("secret=true"))
        assertRedacted(item.content.toString())
    }

    @Test
    fun equalityIsUnchangedByTheRedaction() {
        val a = readableItem("i", device, now, title = clear, secret = true)
        val b = readableItem("i", device, now, title = clear, secret = true)
        val other = readableItem("i", device, now, title = "other", secret = true)
        assertTrue(a == b && a.hashCode() == b.hashCode())
        assertFalse(a == other)
    }

    @Test
    fun draftsAndSharedContentPrintNoContent() {
        val draft = OutgoingDraft("d", title = clear, text = RichText(clear), url = clear, attachments = listOf(DraftAttachment("/p/$clear", clear, "text/plain", 1)))
        assertRedacted(draft.toString())
        assertRedacted(draft.attachments.single().toString())
        assertRedacted(ParsedShare(clear, RichText(clear), clear, emptyList(), looksSecret = true).toString())
        assertRedacted(DeepLink.Send(clear, clear, clear, droppedParameters = false).toString())
        assertRedacted(DeepLink.Pair(clear).toString())
    }
}
