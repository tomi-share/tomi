package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import uniffi.tomi_core.AttachmentRemovalKind
import uniffi.tomi_core.FfiAttachment
import uniffi.tomi_core.FfiItem
import uniffi.tomi_core.FfiItemContent
import uniffi.tomi_core.FfiSyncReport
import uniffi.tomi_core.ReadPolicyKind
import uniffi.tomi_core.TomiException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The removal of ONE attachment through the real adapter, on the JVM, against a stand-in of the UniFFI object (NativeFakes.kt): the SLOT
 * (never the position in the list) is what the core is asked for, both scopes, both answers, errors and the relay capability.
 */
class RealTomiCoreAttachmentTest {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @After
    fun tearDown() {
        scope.cancel()
    }

    private fun core(native: NativeFake, uriOf: (String, String?, String?) -> String? = { path, _, _ -> "content://tomi.test$path" }) =
        RealTomiCore(native, scope, { true }, { listOf("Pixel 8") }, Dispatchers.IO, emptyList(), emptyList(), attachmentUriOf = uriOf)

    private fun attachment(slot: UInt, name: String, mime: String) = FfiAttachment(slot, name, mime, 1u, null, null, null, true)

    /** An item whose slots 0 and 2 of the original payload were removed: the list only holds the slots 1 and 3. */
    private fun itemWithGaps() = FfiItem(
        "item-1",
        FfiItemContent.Readable(
            null, null, null,
            listOf(attachment(1u, "b.png", "image/png"), attachment(3u, "d.pdf", "application/pdf")),
            false,
        ),
        "dev-pc", "PC-Bureau", 1_800_000_000_000L, null, ReadPolicyKind.NORMAL, false, false, emptyList(), false, null, false, null,
    )

    @Test
    fun theUriIsAskedBySlotAndTheMimeTypeIsThatOfTheSameSlot() = runBlocking {
        val asked = mutableListOf<UInt>()
        var mimeSeen: String? = "not called"
        val native = NativeFake().apply {
            attachmentPathImpl = { _, slot ->
                asked += slot
                "/cache/item-1_$slot"
            }
            itemByIdImpl = { itemWithGaps() }
        }
        var nameSeen: String? = "not called"
        val core = core(native) { path, mime, name ->
            mimeSeen = mime
            nameSeen = name
            "content://tomi.test$path"
        }

        assertEquals("content://tomi.test/cache/item-1_3", core.attachmentUri("item-1", 3))

        assertEquals(listOf(3u), asked)
        // The list position of slot 3 is 1; position 3 does not exist. The type is the pdf's, found by slot.
        assertEquals("application/pdf", mimeSeen)
        assertEquals("the real name of the same slot", "d.pdf", nameSeen)
    }

    @Test
    fun aHiddenItemNeverGivesTheNameOfItsFile() = runBlocking {
        var nameSeen: String? = "not called"
        val native = NativeFake().apply {
            attachmentPathImpl = { _, slot -> "/cache/item-1_$slot" }
            itemByIdImpl = {
                FfiItem(
                    "item-1",
                    FfiItemContent.Readable(null, null, null, listOf(attachment(0u, "secret.pdf", "application/pdf")), true),
                    "dev-pc", "PC-Bureau", 1_800_000_000_000L, null, ReadPolicyKind.NORMAL, false, false, emptyList(), false, null, false, null,
                )
            }
        }
        core(native) { path, _, name ->
            nameSeen = name
            "content://tomi.test$path"
        }.attachmentUri("item-1", 0)
        assertNull(nameSeen)
    }

    @Test
    fun aRemovedAttachmentHasNoUri() = runBlocking {
        val native = NativeFake().apply { attachmentPathImpl = { _, _ -> null } }
        assertNull(core(native).attachmentUri("item-1", 0))
        assertNull("a negative slot never reaches the core", core(NativeFake()).attachmentUri("item-1", -1))
    }

    @Test
    fun aLocalRemovalAsksTheCoreForThatSlotOnlyAndSendsNothing() = runBlocking {
        val native = NativeFake().apply { deleteAttachmentImpl = { _, _, _ -> AttachmentRemovalKind.ATTACHMENT_REMOVED } }

        assertEquals(AttachmentRemoval.AttachmentRemoved, core(native).deleteAttachment("item-1", 3, everywhere = false))

        assertEquals(listOf("deleteAttachment:item-1:3:false"), native.calls.filter { it.startsWith("deleteAttachment") })
        assertFalse("a local removal has nothing to flush", native.calls.contains("sync"))
    }

    @Test
    fun anEverywhereRemovalIsFlushedThroughTheOutbox() = runBlocking {
        val synced = CountDownLatch(1)
        val native = NativeFake().apply {
            paired = true // a sync of an unpaired device does nothing
            deleteAttachmentImpl = { _, _, _ -> AttachmentRemovalKind.ATTACHMENT_REMOVED }
            syncImpl = {
                synced.countDown()
                FfiSyncReport(false, 0u, 0u, 0u, false, 0u, false, false)
            }
        }

        assertEquals(AttachmentRemoval.AttachmentRemoved, core(native).deleteAttachment("item-1", 1, everywhere = true))

        assertEquals(listOf("deleteAttachment:item-1:1:true"), native.calls.filter { it.startsWith("deleteAttachment") })
        assertTrue("the statement must leave without waiting for the next wake", synced.await(5, TimeUnit.SECONDS))
    }

    @Test
    fun theLastThingOfTheItemIsAnItemDeletion() = runBlocking {
        val native = NativeFake().apply { deleteAttachmentImpl = { _, _, _ -> AttachmentRemovalKind.ITEM_DELETED } }
        assertEquals(AttachmentRemoval.ItemDeleted, core(native).deleteAttachment("item-1", 0, everywhere = false))
    }

    @Test
    fun aNegativeSlotIsANoOpLikeAnUnknownOne() = runBlocking {
        val native = NativeFake()
        assertEquals(AttachmentRemoval.AttachmentRemoved, core(native).deleteAttachment("item-1", -1, everywhere = false))
        assertTrue(native.calls.none { it.startsWith("deleteAttachment") })
    }

    @Test
    fun aRefusalOfTheCoreIsAnExceptionTheUiReports() = runBlocking {
        val native = NativeFake().apply { deleteAttachmentImpl = { _, _, _ -> throw TomiException.InvalidInput("queued") } }
        try {
            core(native).deleteAttachment("item-1", 1, everywhere = true)
            fail("the refusal must reach the caller")
        } catch (_: TomiCoreException) {
            // reported by the view model as "ActionFailed"
        }
    }

    @Test
    fun theRelayCapabilityIsTheCoresAnswer() = runBlocking {
        val native = NativeFake()
        assertFalse(core(native).supportsAttachmentRemoval())
        native.supportsAttachmentRemovalImpl = { true }
        assertTrue(core(native).supportsAttachmentRemoval())
    }
}
