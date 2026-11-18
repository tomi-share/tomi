package eu.shaffe.tomi.notify

import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** The summary "N new items" is hidden (secret channel, nothing on the lock screen) as soon as one item may be a secret. */
class IncomingNotificationsSummaryTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-10T10:00:00Z")

    private fun text(id: String) = readableItem(id, office, now, text = RichText("bonjour"))
    private fun hidden(id: String) = readableItem(id, office, now, text = RichText("pw"), secret = true)
    private fun unreadable(id: String) = TomiItem(id, ItemContent.Unreadable(UnreadableReason.UpdateRequired), office, now)

    @Test
    fun aHiddenItemMakesTheSummaryHidden() {
        assertTrue(IncomingNotifications.summaryHasSecret(listOf(text("a"), hidden("b")), text("c")))
        assertTrue(IncomingNotifications.summaryHasSecret(listOf(text("a")), hidden("c")))
    }

    @Test
    fun anUnreadableItemIsTreatedAsASecretToo() {
        assertTrue(IncomingNotifications.summaryHasSecret(listOf(text("a"), unreadable("b")), text("c")))
        assertTrue(IncomingNotifications.summaryHasSecret(listOf(text("a")), unreadable("c")))
        assertTrue(IncomingNotifications.summaryHasSecret(emptyList(), unreadable("c")))
    }

    @Test
    fun aSummaryOfNormalItemsOnlyStaysOrdinary() {
        assertFalse(IncomingNotifications.summaryHasSecret(listOf(text("a"), text("b")), text("c")))
    }
}
