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
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** "Partager" of a file / image item: direct for a normal item, behind the biometric gate for a hidden one, never decrypted before. */
@OptIn(ExperimentalCoroutinesApi::class)
class FileSharingTest {
    private val source = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-11T10:00:00Z")
    private val png = Attachment("a.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)

    private fun files(secret: Boolean = false) =
        readableItem("f", source, now, attachments = listOf(png, pdf), secret = secret)

    private class Gate(val outcome: AuthOutcome) {
        var calls = 0
        val authenticate: suspend () -> AuthOutcome = {
            calls++
            outcome
        }
    }

    /** Core stand-in: remembers every file it was asked to decrypt; [missing] indexes are not available. */
    private class Core(val missing: Set<Int> = emptySet()) {
        val asked = mutableListOf<Int>()
        val resolve: suspend (String, Int) -> String? = { id, index ->
            asked += index
            if (index in missing) null else "content://tomi.test/$id/$index"
        }
    }

    @Test
    fun anyItemWithFilesOffersTheFileShareEvenWithADescription() {
        assertTrue(FileSharing.canShare(files()))
        assertTrue(FileSharing.canShare(readableItem("t", source, now, attachments = listOf(png), text = RichText("legende"))))
        assertTrue(FileSharing.canShare(readableItem("l", source, now, attachments = listOf(png), url = "https://a.example")))
        assertFalse(FileSharing.canShare(readableItem("n", source, now, text = RichText("texte"))))
        assertFalse(FileSharing.canShare(TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)))
    }

    @Test
    fun theDescriptionAndLinkTravelAsTheCaptionOfTheFiles() = runTest {
        val item = readableItem("t", source, now, attachments = listOf(pdf), text = RichText("legende"), url = "https://a.example")
        val outcome = FileSharing.prepare(item, Gate(AuthOutcome.Failed).authenticate, Core().resolve)
        outcome as FileShareOutcome.Ready
        assertEquals("legende\nhttps://a.example", outcome.text)
        assertEquals(null, (FileSharing.prepare(files(), Gate(AuthOutcome.Failed).authenticate, Core().resolve) as FileShareOutcome.Ready).text)
    }

    @Test
    fun aHiddenItemNeverSharesItsTextAlongTheFiles() = runTest {
        val item = readableItem("t", source, now, attachments = listOf(pdf), text = RichText("mot de passe"), secret = true)
        val outcome = FileSharing.prepare(item, Gate(AuthOutcome.Success).authenticate, Core().resolve)
        outcome as FileShareOutcome.Ready
        assertEquals(null, outcome.text)
    }

    @Test
    fun aNormalItemSharesAllItsFilesWithoutAuthentication() = runTest {
        val gate = Gate(AuthOutcome.Failed)
        val core = Core()
        val outcome = FileSharing.prepare(files(), gate.authenticate, core.resolve)
        assertEquals(0, gate.calls)
        outcome as FileShareOutcome.Ready
        assertEquals(listOf("image/png", "application/pdf"), outcome.files.map { it.mime })
        assertEquals(listOf("content://tomi.test/f/0", "content://tomi.test/f/1"), outcome.files.map { it.uri })
    }

    @Test
    fun theCoreIsAskedForTheSlotsNotThePositions() = runTest {
        // Slots 0 and 2 of the original item were removed: the two files left are at positions 0 and 1 but are slots 1 and 3.
        val item = readableItem("f", source, now, attachments = listOf(png.withSlot(1), pdf.withSlot(3)))
        val core = Core()
        val outcome = FileSharing.prepare(item, Gate(AuthOutcome.Success).authenticate, core.resolve)

        outcome as FileShareOutcome.Ready
        assertEquals(listOf(1, 3), core.asked)
        assertEquals(listOf("content://tomi.test/f/1", "content://tomi.test/f/3"), outcome.files.map { it.uri })
        assertEquals(listOf("image/png", "application/pdf"), outcome.files.map { it.mime })
    }

    @Test
    fun aMissingSlotSkipsOnlyThatFile() = runTest {
        val item = readableItem("f", source, now, attachments = listOf(png.withSlot(1), pdf.withSlot(3)))
        val outcome = FileSharing.prepare(item, Gate(AuthOutcome.Success).authenticate, Core(missing = setOf(1)).resolve)

        outcome as FileShareOutcome.Ready
        assertEquals(listOf("content://tomi.test/f/3"), outcome.files.map { it.uri })
    }

    @Test
    fun aHiddenItemIsDecryptedOnlyAfterASuccessfulAuthentication() = runTest {
        val core = Core()
        val ok = FileSharing.prepare(files(secret = true), Gate(AuthOutcome.Success).authenticate, core.resolve)
        assertTrue(ok is FileShareOutcome.Ready)
        assertEquals(listOf(0, 1), core.asked)
    }

    @Test
    fun aRefusedHiddenItemDecryptsNothing() = runTest {
        for ((auth, expected) in listOf(
            AuthOutcome.Cancelled to FileShareOutcome.NotAuthenticated,
            AuthOutcome.Failed to FileShareOutcome.NotAuthenticated,
            AuthOutcome.Unavailable to FileShareOutcome.BiometricUnavailable,
        )) {
            val core = Core()
            val gate = Gate(auth)
            assertEquals(expected, FileSharing.prepare(files(secret = true), gate.authenticate, core.resolve))
            assertEquals(1, gate.calls)
            assertTrue(core.asked.isEmpty())
        }
    }

    @Test
    fun filesNotAvailableOnThisDeviceAreSkippedAndNoneMeansNothingToShare() = runTest {
        val some = FileSharing.prepare(files(), Gate(AuthOutcome.Success).authenticate, Core(missing = setOf(0)).resolve)
        some as FileShareOutcome.Ready
        assertEquals(listOf("application/pdf"), some.files.map { it.mime })

        val none = FileSharing.prepare(files(), Gate(AuthOutcome.Success).authenticate, Core(missing = setOf(0, 1)).resolve)
        assertEquals(FileShareOutcome.NothingToShare, none)
    }

    @Test
    fun theIntentTypeIsTheMostPreciseOneThatCoversEveryFile() {
        assertEquals("image/png", FileSharing.commonMime(listOf("image/png", "IMAGE/PNG")))
        assertEquals("image/*", FileSharing.commonMime(listOf("image/png", "image/jpeg")))
        assertEquals("*/*", FileSharing.commonMime(listOf("image/png", "application/pdf")))
    }

    @Test
    fun aSharedFileNeverPrintsItsUri() {
        assertEquals("SharedFile(<redacted>)", SharedFile("content://tomi.test/f/0", "image/png").toString())
    }
}
