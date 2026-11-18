package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.AttachmentRemoval
import eu.shaffe.tomi.core.DeliveryState
import eu.shaffe.tomi.core.DeviceDelivery
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** What the UI decides around "remove ONE attachment of an item": where it is offered, which scopes, which message. */
class AttachmentRemovalsTest {
    private val source = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-11T10:00:00Z")
    private val png = Attachment("a.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)

    private fun item(vararg attachments: Attachment, deliveries: List<DeviceDelivery> = emptyList(), secret: Boolean = false) =
        readableItem("i", source, now, attachments = attachments.toList(), deliveries = deliveries, secret = secret)

    private fun delivery(state: DeliveryState) = DeviceDelivery("d", "PC", state)

    // --- where the action exists -------------------------------------------------------------------------------------------

    @Test
    fun theRowsOfABatchOfSeveralFilesGetTheAction() {
        assertTrue(AttachmentRemovals.canRemoveFromBatch(item(png, pdf)))
        assertTrue(AttachmentRemovals.canRemoveFromBatch(item(pdf, pdf, pdf, secret = true)))
    }

    @Test
    fun aSingleFileIsTheItemAndHasNoRowAction() {
        assertFalse(AttachmentRemovals.canRemoveFromBatch(item(pdf)))
        // The viewer still removes it (the core then deletes the item): that is `canRemove`.
        assertTrue(AttachmentRemovals.canRemove(item(pdf)))
    }

    @Test
    fun nothingIsOfferedWithoutAttachmentOrForAnUnreadableItem() {
        assertFalse(AttachmentRemovals.canRemove(readableItem("t", source, now, text = RichText("texte"))))
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)
        assertFalse(AttachmentRemovals.canRemove(unreadable))
        assertFalse(AttachmentRemovals.canRemoveFromBatch(unreadable))
    }

    @Test
    fun anItemStillQueuedForSendingOffersNoRemoval() {
        val queued = item(png, pdf, deliveries = listOf(delivery(DeliveryState.Queued), delivery(DeliveryState.Queued)))
        assertTrue(AttachmentRemovals.isQueued(queued))
        assertFalse(AttachmentRemovals.canRemove(queued))
        assertFalse(AttachmentRemovals.canRemoveFromBatch(queued))
    }

    @Test
    fun anItemThatLeftTheDeviceOffersTheRemoval() {
        val sent = item(png, pdf, deliveries = listOf(delivery(DeliveryState.Queued), delivery(DeliveryState.Sent)))
        assertFalse("one delivery is out: the item has left", AttachmentRemovals.isQueued(sent))
        assertTrue(AttachmentRemovals.canRemove(sent))
        // A received item has no delivery at all.
        assertFalse(AttachmentRemovals.isQueued(item(png, pdf)))
    }

    // --- scopes ------------------------------------------------------------------------------------------------------------

    @Test
    fun everywhereIsOfferedOnlyWhenTheRelaySupportsIt() {
        assertEquals(listOf(RemovalScope.ThisDevice, RemovalScope.Everywhere), AttachmentRemovals.scopes(relaySupportsRemoval = true))
        assertEquals(listOf(RemovalScope.ThisDevice), AttachmentRemovals.scopes(relaySupportsRemoval = false))
    }

    // --- the message after the core answered -------------------------------------------------------------------------------

    @Test
    fun anAttachmentRemovalSaysWhichKindAndWhichScope() {
        assertEquals(
            FeedEvent.AttachmentDeleted(image = true, everywhere = false),
            AttachmentRemovals.feedback(AttachmentRemoval.AttachmentRemoved, image = true, everywhere = false),
        )
        assertEquals(
            FeedEvent.AttachmentDeleted(image = true, everywhere = true),
            AttachmentRemovals.feedback(AttachmentRemoval.AttachmentRemoved, image = true, everywhere = true),
        )
        assertEquals(
            FeedEvent.AttachmentDeleted(image = false, everywhere = false),
            AttachmentRemovals.feedback(AttachmentRemoval.AttachmentRemoved, image = false, everywhere = false),
        )
        assertEquals(
            FeedEvent.AttachmentDeleted(image = false, everywhere = true),
            AttachmentRemovals.feedback(AttachmentRemoval.AttachmentRemoved, image = false, everywhere = true),
        )
    }

    @Test
    fun theLastThingOfTheItemGivesTheMessageOfAnItemDeletion() {
        assertEquals(FeedEvent.DeletedLocal, AttachmentRemovals.feedback(AttachmentRemoval.ItemDeleted, image = true, everywhere = false))
        assertEquals(FeedEvent.DeletedEverywhere, AttachmentRemovals.feedback(AttachmentRemoval.ItemDeleted, image = false, everywhere = true))
    }
}
