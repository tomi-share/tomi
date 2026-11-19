package eu.shaffe.tomi.share

import eu.shaffe.tomi.core.Attachment
import eu.shaffe.tomi.core.DeviceRef
import eu.shaffe.tomi.core.ItemContent
import eu.shaffe.tomi.core.TomiItem
import eu.shaffe.tomi.core.UnreadableReason
import eu.shaffe.tomi.core.readableItem
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

/** "Télécharger" of one file: allowed for a normal item, FORBIDDEN for a hidden one (nothing decrypted, nothing written). */
@OptIn(ExperimentalCoroutinesApi::class)
class FileDownloadsTest {
    private val source = DeviceRef("pc", "PC-Bureau")
    private val now = Instant.parse("2026-11-11T10:00:00Z")
    private val png = Attachment("a.png", "image/png", 10)
    private val pdf = Attachment("a.pdf", "application/pdf", 10)

    private fun files(secret: Boolean = false) =
        readableItem("f", source, now, attachments = listOf(png.withSlot(1), pdf.withSlot(3)), secret = secret)

    /** Core stand-in: remembers the slots it was asked to decrypt; [missing] slots are not available. */
    private class Core(val missing: Set<Int> = emptySet()) {
        val asked = mutableListOf<Int>()
        val resolve: suspend (String, Int) -> String? = { id, slot ->
            asked += slot
            if (slot in missing) null else "content://tomi.test/$id/$slot"
        }
    }

    /** Writer stand-in: remembers what it was asked to write. */
    private class Writer(val succeeds: Boolean = true) {
        val written = mutableListOf<Pair<String, Attachment>>()
        val write: suspend (String, Attachment) -> Boolean = { uri, attachment ->
            written += uri to attachment
            succeeds
        }
    }

    @Test
    fun onlyANormalReadableItemWithFilesOffersTheDownload() {
        assertTrue(FileDownloads.canDownload(files()))
        assertFalse(FileDownloads.canDownload(files(secret = true)))
        assertFalse(FileDownloads.canDownload(readableItem("n", source, now)))
        assertFalse(FileDownloads.canDownload(TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)))
    }

    @Test
    fun aHiddenItemIsNeverDownloadedNorDecrypted() = runTest {
        val core = Core()
        val writer = Writer()
        assertEquals(FileDownloadResult.Forbidden, FileDownloads.download(files(secret = true), 1, core.resolve, writer.write))
        // No decryption, no write: there is no permanent clear copy of a secret, and no workaround.
        assertTrue(core.asked.isEmpty())
        assertTrue(writer.written.isEmpty())
    }

    @Test
    fun aNormalFileIsResolvedBySlotAndWritten() = runTest {
        val core = Core()
        val writer = Writer()
        assertEquals(FileDownloadResult.Done, FileDownloads.download(files(), 3, core.resolve, writer.write))
        assertEquals(listOf(3), core.asked)
        // The file written is the one of slot 3 (not the one at position 1 of another list): its URI and its attachment agree.
        assertEquals(listOf("content://tomi.test/f/3"), writer.written.map { it.first })
        assertEquals(listOf("a.pdf"), writer.written.map { it.second.name })
        assertEquals(listOf(3), writer.written.map { it.second.slot })
    }

    @Test
    fun aFileNotDownloadedYetOnThisDeviceIsUnavailableAndNothingIsWritten() = runTest {
        val writer = Writer()
        assertEquals(FileDownloadResult.Unavailable, FileDownloads.download(files(), 1, Core(missing = setOf(1)).resolve, writer.write))
        assertTrue(writer.written.isEmpty())
    }

    @Test
    fun anUnknownSlotOrAnUnreadableItemIsUnavailableWithoutAskingTheCore() = runTest {
        val core = Core()
        val writer = Writer()
        assertEquals(FileDownloadResult.Unavailable, FileDownloads.download(files(), 0, core.resolve, writer.write))
        val unreadable = TomiItem("u", ItemContent.Unreadable(UnreadableReason.UpdateRequired), source, now)
        assertEquals(FileDownloadResult.Unavailable, FileDownloads.download(unreadable, 1, core.resolve, writer.write))
        assertTrue(core.asked.isEmpty())
        assertTrue(writer.written.isEmpty())
    }

    @Test
    fun aWriteErrorIsReportedAsFailed() = runTest {
        assertEquals(FileDownloadResult.Failed, FileDownloads.download(files(), 1, Core().resolve, Writer(succeeds = false).write))
    }

    @Test
    fun theSavedNameHasNoPathNorForbiddenCharacter() {
        assertEquals("rapport.pdf", FileDownloads.fileName("rapport.pdf"))
        assertEquals("passwd", FileDownloads.fileName("../../etc/passwd"))
        assertEquals("c_d.txt", FileDownloads.fileName("a\\b\\c:d.txt"))
        assertEquals("a_b_c.txt", FileDownloads.fileName("a?b*c.txt"))
        assertEquals("tomi-file", FileDownloads.fileName("  .. "))
        assertEquals("tomi-file", FileDownloads.fileName(""))
        assertEquals("bashrc", FileDownloads.fileName(".bashrc"))
    }

    @Test
    fun aLongNameIsCutBeforeItsExtension() {
        val name = FileDownloads.fileName("x".repeat(300) + ".pdf")
        assertEquals(100, name.length)
        assertTrue(name.endsWith(".pdf"))
        // A suffix that is not an extension is cut like any other text.
        assertEquals(100, FileDownloads.fileName("y".repeat(150)).length)
    }

    @Test
    fun theMimeTypeFallsBackToABinaryType() {
        assertEquals("application/pdf", FileDownloads.mimeType("application/pdf"))
        assertEquals("application/octet-stream", FileDownloads.mimeType(" "))
    }

    @Test
    fun theFolderIsUnderTheDownloadsRootAsMediaStoreRequires() {
        assertEquals("Download/Tomi", FileDownloads.RELATIVE_PATH)
    }
}
