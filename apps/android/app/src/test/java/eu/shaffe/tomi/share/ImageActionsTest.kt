package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import eu.shaffe.tomi.security.AuthOutcome
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/**
 * Display, save to the gallery and share of an image: direct for a normal item, behind the same biometric gate as "Afficher"
 * (and as the text share) for an item hidden by the sender, and never a decrypted file requested before that.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class ImageActionsTest {
    private val source = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-05T10:00:00Z")
    private val png = Attachment("a.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)

    private fun normal() = readableItem("n", source, now, attachments = listOf(pdf, png))
    private fun hidden() = readableItem("s", source, now, attachments = listOf(pdf, png), secret = true)

    /** Counts gate prompts and answers with [outcome]. */
    private class Gate(val outcome: AuthOutcome) {
        var calls = 0
        val authenticate: suspend () -> AuthOutcome = {
            calls++
            outcome
        }
    }

    /** Core stand-in: remembers every file it was asked to decrypt. */
    private class Core {
        val asked = mutableListOf<Pair<String, Int>>()
        val actions = ImageActions { itemId, index ->
            asked += itemId to index
            "content://tomi.test/$itemId/$index"
        }
    }

    private class Sink {
        val written = mutableListOf<String>()
        val shared = mutableListOf<String>()
        var writeResult = true
        var shareResult = true
        val write: suspend (String, Attachment) -> Boolean = { uri, _ ->
            written += uri
            writeResult
        }
        val launch: (String, Attachment) -> Boolean = { uri, _ ->
            shared += uri
            shareResult
        }
    }

    // --- normal item: no gate ----------------------------------------------------------------------------------------

    @Test
    fun aNormalImageIsViewedSavedAndSharedWithoutAnyAuthentication() = runTest {
        val gate = Gate(AuthOutcome.Failed)
        val core = Core()
        val sink = Sink()
        assertEquals(ImageActionResult.Done, core.actions.authorizeView(normal(), gate.authenticate))
        assertEquals(ImageActionResult.Done, core.actions.save(normal(), 1, gate.authenticate, sink.write))
        assertEquals(ImageActionResult.Done, core.actions.share(normal(), 1, gate.authenticate, sink.launch))
        assertEquals(0, gate.calls)
        assertEquals(listOf("content://tomi.test/n/1"), sink.written)
        assertEquals(listOf("content://tomi.test/n/1"), sink.shared)
    }

    // --- hidden item: gate first -------------------------------------------------------------------------------------

    @Test
    fun aHiddenImageAsksTheGateToBeViewedSavedAndShared() = runTest {
        val gate = Gate(AuthOutcome.Success)
        val core = Core()
        val sink = Sink()
        assertEquals(ImageActionResult.Done, core.actions.authorizeView(hidden(), gate.authenticate))
        assertEquals(ImageActionResult.Done, core.actions.save(hidden(), 1, gate.authenticate, sink.write))
        assertEquals(ImageActionResult.Done, core.actions.share(hidden(), 1, gate.authenticate, sink.launch))
        assertEquals(3, gate.calls)
        assertEquals(1, sink.written.size)
        assertEquals(1, sink.shared.size)
    }

    @Test
    fun aHiddenImageIsNeverDecryptedWrittenOrSharedWithoutASuccessfulAuthentication() = runTest {
        for (refusal in listOf(AuthOutcome.Cancelled, AuthOutcome.Failed, AuthOutcome.Unavailable)) {
            val gate = Gate(refusal)
            val core = Core()
            val sink = Sink()
            val expected = if (refusal == AuthOutcome.Unavailable) ImageActionResult.BiometricUnavailable else ImageActionResult.NotAuthenticated

            assertEquals(refusal.name, expected, core.actions.authorizeView(hidden(), gate.authenticate))
            assertEquals(refusal.name, expected, core.actions.save(hidden(), 1, gate.authenticate, sink.write))
            assertEquals(refusal.name, expected, core.actions.share(hidden(), 1, gate.authenticate, sink.launch))

            assertTrue("no file may be decrypted for a refused user ($refusal)", core.asked.isEmpty())
            assertTrue("nothing may reach the gallery ($refusal)", sink.written.isEmpty())
            assertTrue("nothing may reach a share intent ($refusal)", sink.shared.isEmpty())
        }
    }

    @Test
    fun theFileOfAHiddenImageIsRequestedAfterTheGateAnswers() = runTest {
        val order = mutableListOf<String>()
        val actions = ImageActions { _, _ ->
            order += "decrypt"
            "content://tomi.test/s/1"
        }
        actions.save(hidden(), 1, { order += "gate"; AuthOutcome.Success }) { _, _ ->
            order += "write"
            true
        }
        assertEquals(listOf("gate", "decrypt", "write"), order)
    }

    // --- failures ----------------------------------------------------------------------------------------------------

    @Test
    fun anAttachmentThatIsNotAnImageIsUnavailableAndAsksNothing() = runTest {
        val gate = Gate(AuthOutcome.Success)
        val core = Core()
        val sink = Sink()
        // Index 0 is the PDF, 5 does not exist.
        assertEquals(ImageActionResult.Unavailable, core.actions.save(hidden(), 0, gate.authenticate, sink.write))
        assertEquals(ImageActionResult.Unavailable, core.actions.share(hidden(), 5, gate.authenticate, sink.launch))
        assertEquals(0, gate.calls)
        assertTrue(core.asked.isEmpty())
        assertTrue(sink.written.isEmpty())
    }

    @Test
    fun aFileTheCoreCannotProvideIsUnavailable() = runTest {
        val actions = ImageActions { _, _ -> null }
        val sink = Sink()
        assertEquals(ImageActionResult.Unavailable, actions.save(normal(), 1, Gate(AuthOutcome.Success).authenticate, sink.write))
        assertTrue(sink.written.isEmpty())
    }

    @Test
    fun aFailedWriteAndANoAppShareAreReported() = runTest {
        val core = Core()
        val sink = Sink().apply {
            writeResult = false
            shareResult = false
        }
        val gate = Gate(AuthOutcome.Success)
        assertEquals(ImageActionResult.Failed, core.actions.save(normal(), 1, gate.authenticate, sink.write))
        assertEquals(ImageActionResult.Unavailable, core.actions.share(normal(), 1, gate.authenticate, sink.launch))
    }

    @Test
    fun anUnreadableItemHasNoImage() = runTest {
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)
        val gate = Gate(AuthOutcome.Success)
        assertEquals(ImageActionResult.Unavailable, Core().actions.authorizeView(unreadable, gate.authenticate))
        assertEquals(0, gate.calls)
    }

    // --- file production enforces the gate (not only the screens) ---------------------------------------------------------

    @Test
    fun aHiddenFileIsNotDecryptedWhenTheAuthorizationWindowIsClosed() = runTest {
        val core = Core()
        assertNull(core.actions.fileForView(hidden(), 1) { false })
        assertTrue("nothing may be decrypted", core.asked.isEmpty())
    }

    @Test
    fun aHiddenFileIsServedWhileTheAuthorizationWindowIsOpen() = runTest {
        val core = Core()
        assertEquals("content://tomi.test/s/1", core.actions.fileForView(hidden(), 1) { true })
        assertEquals(listOf("s" to 1), core.asked)
    }

    @Test
    fun aNormalFileNeverNeedsTheAuthorizationWindow() = runTest {
        val core = Core()
        assertEquals("content://tomi.test/n/1", core.actions.fileForView(normal(), 1) { false })
    }

    @Test
    fun anUnknownUnreadableOrOutOfRangeItemGivesNoFileAndNoDecryption() = runTest {
        val core = Core()
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)
        assertNull(core.actions.fileForView(null, 0) { true })
        assertNull(core.actions.fileForView(unreadable, 0) { true })
        assertNull(core.actions.fileForView(normal(), 9) { true })
        assertNull(core.actions.fileForView(hidden(), -1) { true })
        assertTrue(core.asked.isEmpty())
    }

    // --- slots: an attachment is found by its slot, not by its position in the list ------------------------------------------

    @Test
    fun anImageIsReachedByItsSlotWhenTheListHasGaps() = runTest {
        // Slots 0 and 1 were removed: the png is at position 1 of the list but is slot 4; the pdf (position 0) is slot 2.
        val item = readableItem("g", source, now, attachments = listOf(pdf.withSlot(2), png.withSlot(4)))
        val core = Core()
        val sink = Sink()

        assertEquals("content://tomi.test/g/4", core.actions.fileForView(item, 4) { true })
        assertEquals(ImageActionResult.Done, core.actions.save(item, 4, Gate(AuthOutcome.Success).authenticate, sink.write))
        assertEquals(listOf("g" to 4, "g" to 4), core.asked)
        // The position 1 is no slot of this item, and slot 0 was removed: nothing is requested.
        assertNull(core.actions.fileForView(item, 1) { true })
        assertNull(core.actions.fileForView(item, 0) { true })
        assertEquals(ImageActionResult.Unavailable, core.actions.share(item, 2, Gate(AuthOutcome.Success).authenticate, sink.launch))
        assertEquals("only the two requests for slot 4", 2, core.asked.size)
    }
}
