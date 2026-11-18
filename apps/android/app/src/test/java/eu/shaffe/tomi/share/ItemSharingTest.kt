package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.RichText
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.security.AuthOutcome
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** System share of an item: direct for a normal item, behind the biometric gate for a hidden ("Masqué") one. */
@OptIn(ExperimentalCoroutinesApi::class)
class ItemSharingTest {
    private val office = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-04T10:00:00Z")
    private val password = "Hx7!pQ2-demo-zR9"

    private fun normal() = readableItem("n", office, now, title = "Doc", text = RichText("bonjour"), url = "https://a.example")
    private fun hidden() = readableItem("s", office, now, title = "Wi-Fi invités", text = RichText(password), secret = true)

    /** Counts how many times the gate is asked and answers with [outcome]. */
    private class Gate(val outcome: AuthOutcome) {
        var calls = 0
        val authenticate: suspend () -> AuthOutcome = {
            calls++
            outcome
        }
    }

    @Test
    fun aNormalItemIsSharedDirectlyWithoutAnyAuthentication() = runTest {
        val gate = Gate(AuthOutcome.Failed)
        val outcome = ItemSharing.prepare(normal(), gate.authenticate)
        assertEquals(0, gate.calls)
        outcome as ShareOutcome.Ready
        assertEquals("bonjour\nhttps://a.example", outcome.payload.text)
        assertEquals("Doc", outcome.payload.subject)
    }

    @Test
    fun aHiddenItemIsSharedOnlyAfterASuccessfulAuthentication() = runTest {
        val gate = Gate(AuthOutcome.Success)
        val outcome = ItemSharing.prepare(hidden(), gate.authenticate)
        assertEquals(1, gate.calls)
        outcome as ShareOutcome.Ready
        assertEquals(password, outcome.payload.text)
    }

    @Test
    fun aHiddenItemNeverProducesAPayloadWhenAuthenticationIsCancelledFailedOrUnavailable() = runTest {
        for (failure in listOf(AuthOutcome.Cancelled, AuthOutcome.Failed)) {
            val gate = Gate(failure)
            assertEquals(ShareOutcome.NotAuthenticated, ItemSharing.prepare(hidden(), gate.authenticate))
            assertEquals(1, gate.calls)
        }
        val unavailable = Gate(AuthOutcome.Unavailable)
        assertEquals(ShareOutcome.BiometricUnavailable, ItemSharing.prepare(hidden(), unavailable.authenticate))
    }

    @Test
    fun theContentOfAHiddenItemIsNotReadBeforeTheGateAnswers() = runTest {
        // The gate itself checks the state of the world at the time it is asked: nothing has been built yet, and what
        // the caller gets back for a refusal never carries the content.
        var askedBeforeAnyPayload = false
        val outcome = ItemSharing.prepare(hidden()) {
            askedBeforeAnyPayload = true
            AuthOutcome.Cancelled
        }
        assertTrue(askedBeforeAnyPayload)
        assertFalse(outcome is ShareOutcome.Ready)
    }

    @Test
    fun aPayloadNeverPrintsItsContent() {
        val payload = SharePayload(password, "Wi-Fi invités")
        assertFalse(payload.toString().contains(password))
        assertFalse(payload.toString().contains("Wi-Fi"))
    }

    @Test
    fun unreadableItemsAndFileOnlyItemsHaveNothingToShareAndAskNoAuthentication() = runTest {
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), office, now)
        val filesOnly = readableItem("f", office, now, attachments = listOf(Attachment("a.pdf", "application/pdf", 1)), secret = true)
        val gate = Gate(AuthOutcome.Success)
        assertEquals(ShareOutcome.NothingToShare, ItemSharing.prepare(unreadable, gate.authenticate))
        assertEquals(ShareOutcome.NothingToShare, ItemSharing.prepare(filesOnly, gate.authenticate))
        assertEquals(0, gate.calls)
        assertFalse(ItemSharing.canShare(unreadable))
        assertFalse(ItemSharing.canShare(filesOnly))
        assertTrue(ItemSharing.canShare(hidden()))
    }

    @Test
    fun theIntentOfAHiddenItemCarriesASensitiveClipAndANeutralPreviewTitle() {
        val payload = ItemSharing.payloadFor(hidden())!!
        assertTrue(payload.sensitive)
        val plan = ShareIntentPlan.of(payload, "Contenu masqué")
        assertTrue(plan.sensitiveClip)
        assertEquals("Contenu masqué", plan.previewTitle)
        assertEquals(password, plan.text)
        // The title of a hidden item is hidden too: no EXTRA_SUBJECT, even if the payload carries one.
        assertEquals("Wi-Fi invités", payload.subject)
        assertNull(plan.subject)
        assertFalse(plan.toString().contains(password))
        assertFalse(payload.toString().contains(password))
    }

    @Test
    fun theIntentOfANormalItemIsUnchanged() {
        val payload = ItemSharing.payloadFor(normal())!!
        assertFalse(payload.sensitive)
        val plan = ShareIntentPlan.of(payload, "Contenu masqué")
        assertFalse(plan.sensitiveClip)
        assertNull(plan.previewTitle)
        assertEquals("bonjour\nhttps://a.example", plan.text)
        assertEquals("Doc", plan.subject)
    }

    @Test
    fun aTitleOnlyItemSharesItsTitleAsText() {
        val payload = ItemSharing.payloadFor(readableItem("t", office, now, title = "Rappel"))!!
        assertEquals("Rappel", payload.text)
        assertNull(payload.subject)
    }
}
