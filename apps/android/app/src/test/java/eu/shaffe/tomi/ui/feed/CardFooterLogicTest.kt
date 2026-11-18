package eu.shaffe.tomi.ui.feed

import eu.shaffe.tomi.core.DeliveryStatus
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CardFooterLogicTest {
    private val complete = DeliveryStatus.Delivered(2, 2)

    @Test
    fun fullDeliveryIsLeftOutOfTheFeedCardsButKeptOnTheDetail() {
        assertFalse(CardFooterLogic.showsDeliveryStatus(complete, expanded = false))
        assertTrue(CardFooterLogic.showsDeliveryStatus(complete, expanded = true))
    }

    @Test
    fun otherStatesStayVisibleEverywhere() {
        val others = listOf(
            DeliveryStatus.Delivered(1, 2),
            DeliveryStatus.Queued,
            DeliveryStatus.Sent,
            DeliveryStatus.Failed,
            DeliveryStatus.ExpiredBeforeDelivery(1, listOf("PC")),
        )
        for (status in others) {
            assertTrue("$status feed", CardFooterLogic.showsDeliveryStatus(status, expanded = false))
            assertTrue("$status detail", CardFooterLogic.showsDeliveryStatus(status, expanded = true))
        }
    }

    @Test
    fun aReceivedItemHasNoDeliveryLine() {
        assertFalse(CardFooterLogic.showsDeliveryStatus(DeliveryStatus.Received, expanded = false))
        assertFalse(CardFooterLogic.showsDeliveryStatus(DeliveryStatus.Received, expanded = true))
    }
}
