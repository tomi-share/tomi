package eu.shaffe.tomi.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.tomi_core.DeliveryKind
import uniffi.tomi_core.DeviceStateKind
import uniffi.tomi_core.DispatchKind
import uniffi.tomi_core.FfiAttachment
import uniffi.tomi_core.FfiDelivery
import uniffi.tomi_core.FfiDevice
import uniffi.tomi_core.FfiItem
import uniffi.tomi_core.FfiItemContent
import uniffi.tomi_core.ReadPolicyKind
import uniffi.tomi_core.SpaceStateKind
import uniffi.tomi_core.TextContent
import uniffi.tomi_core.TextFormat
import uniffi.tomi_core.UnreadableKind
import java.time.Instant

/** The records of the core, as the app's model (RealCoreMapping.kt). */
class RealCoreMappingTest {
    private fun item(content: FfiItemContent, pinned: Boolean = false, archived: Boolean = false) = FfiItem(
        id = "item-1",
        content = content,
        sourceDeviceId = "dev-pc",
        sourceDeviceName = "PC-Bureau",
        createdAtMs = 1_800_000_000_000L,
        expiresAtMs = 1_800_000_100_000L,
        readPolicy = ReadPolicyKind.AFTER_FIRST_READ,
        read = true,
        outgoing = false,
        deliveries = listOf(FfiDelivery("dev-phone", "Pixel", DeliveryKind.EXPIRED_BEFORE_DELIVERY)),
        pinned = pinned,
        pinnedAtMs = if (pinned) 1_800_000_050_000L else null,
        archived = archived,
        archivedAtMs = if (archived) 1_800_000_060_000L else null,
    )

    private fun readable(secret: Boolean = false) = FfiItemContent.Readable(
        title = "Wi-Fi",
        text = TextContent("plain", "<b>html</b>", "**md**", TextFormat.MARKDOWN),
        url = "https://example.org",
        attachments = listOf(FfiAttachment(0u, "a.png", "image/png", 42u, byteArrayOf(1, 2), "image/png", "/data/x/a.png", true)),
        secret = secret,
    )

    @Test
    fun theSlotOfAnAttachmentIsKeptNotItsPositionInTheList() {
        // Slot 0 was removed: the list only holds the others, and they keep the position they had in the payload.
        val content = FfiItemContent.Readable(
            title = null,
            text = null,
            url = null,
            attachments = listOf(
                FfiAttachment(1u, "b.png", "image/png", 1u, null, null, null, true),
                FfiAttachment(3u, "d.pdf", "application/pdf", 1u, null, null, null, true),
            ),
            secret = false,
        )
        val attachments = item(content).toDomain()!!.attachments

        assertEquals(listOf(1, 3), attachments.map { it.slot })
        assertEquals(1, attachments.slotOf(0))
        assertEquals(3, attachments.slotOf(1))
        assertEquals("d.pdf", attachments.findBySlot(3)?.name)
        assertNull("slot 0 is gone", attachments.findBySlot(0))
        assertNull("a position is not a slot", attachments.findBySlot(2))
    }

    @Test
    fun withoutASlotThePositionIsTheSlot() {
        val list = listOf(Attachment("a", "image/png", 1), Attachment("b", "image/png", 1))
        assertEquals(1, list.slotOf(1))
        assertEquals("b", list.findBySlot(1)?.name)
        assertNull(list.findBySlot(2))
    }

    @Test
    fun aReadableItemKeepsEverythingTheUiNeeds() {
        val domain = item(readable(secret = true), pinned = true).toDomain()!!

        assertEquals("item-1", domain.id)
        assertTrue(domain.secret)
        assertEquals("Wi-Fi", domain.title)
        assertEquals("https://example.org", domain.url)
        assertEquals(RichText("plain", "<b>html</b>", "**md**", SourceFormat.Markdown), domain.text)
        assertEquals(DeviceRef("dev-pc", "PC-Bureau"), domain.sourceDevice)
        assertEquals(Instant.ofEpochMilli(1_800_000_000_000L), domain.createdAt)
        assertEquals(Instant.ofEpochMilli(1_800_000_100_000L), domain.expiresAt)
        assertEquals(ReadPolicy.AfterFirstRead, domain.readPolicy)
        assertTrue(domain.read)
        assertEquals(listOf(DeviceDelivery("dev-phone", "Pixel", DeliveryState.ExpiredBeforeDelivery)), domain.deliveries)
        assertTrue(domain.pinned)
        assertEquals(Instant.ofEpochMilli(1_800_000_050_000L), domain.pinnedAt)
        assertFalse(domain.archived)
        assertNull(domain.archivedAt)
    }

    @Test
    fun anArchivedItemKeepsItsArchiveState() {
        val domain = item(readable(), archived = true).toDomain()!!

        assertTrue(domain.archived)
        assertEquals(Instant.ofEpochMilli(1_800_000_060_000L), domain.archivedAt)
        assertFalse(domain.pinned)
        assertTrue("archiving never changes the read state", domain.read)
    }

    @Test
    fun theLocalPathOfAnAttachmentIsNotPassedOnAsAUri() {
        val attachment = item(readable()).toDomain()!!.attachments.single()
        assertEquals("a.png", attachment.name)
        assertEquals(42L, attachment.size)
        assertNull("the file is reached through TomiCore.attachmentUri", attachment.uri)
        assertTrue(attachment.isImage)
    }

    @Test
    fun theMappedItemPrintsNoContent() {
        val printed = item(readable(secret = true)).toDomain().toString()
        assertFalse(printed, printed.contains("Wi-Fi"))
        assertFalse(printed, printed.contains("PC-Bureau"))
        assertFalse(printed, printed.contains("example.org"))
    }

    @Test
    fun anUnreadableItemIsKeptWithItsReason() {
        fun reason(kind: UnreadableKind) = (item(FfiItemContent.Unreadable(kind)).toDomain()!!.content as ItemContent.Unreadable).reason

        assertEquals(UnreadableReason.UpdateRequired, reason(UnreadableKind.UPDATE_REQUIRED))
        assertEquals(UnreadableReason.DecryptionFailed, reason(UnreadableKind.DECRYPTION_FAILED))
        assertEquals("a key that has not arrived yet is unreadable for now", UnreadableReason.DecryptionFailed, reason(UnreadableKind.KEY_PENDING))
    }

    @Test
    fun anItemSealedWithAReplacedKeyIsNotShown() {
        assertNull(item(FfiItemContent.Unreadable(UnreadableKind.SUSPECT)).toDomain())
    }

    @Test
    fun aDraftReachesTheCoreAsTheUserMadeIt() {
        val draft = OutgoingDraft(
            id = "id-1",
            title = "T",
            text = RichText("p", markdown = "m"),
            url = "https://example.org",
            attachments = listOf(DraftAttachment("/tmp/a.txt", "a.txt", "text/plain", 3)),
            secret = true,
            expiresAt = Instant.ofEpochMilli(1_800_000_000_000L),
            readPolicy = ReadPolicy.AfterFirstRead,
        )

        val ffi = draft.toFfi()

        assertEquals("id-1", ffi.id)
        assertEquals("T", ffi.title)
        assertEquals(TextContent("p", null, "m", TextFormat.MARKDOWN), ffi.text)
        assertEquals("https://example.org", ffi.url)
        assertEquals("/tmp/a.txt", ffi.attachments.single().path)
        assertEquals("a.txt", ffi.attachments.single().name)
        assertTrue(ffi.secret)
        assertEquals(1_800_000_000_000L, ffi.expiresAtMs)
        assertEquals(ReadPolicyKind.AFTER_FIRST_READ, ffi.readPolicy)
    }

    @Test
    fun aDeviceKeepsItsPlatformAndWhetherItIsThisOne() {
        val device = FfiDevice("d", "PC", "windows", DeviceStateKind.VERIFIED, 1_800_000_000_000L, false).toDomain()
        assertEquals(Device("d", "PC", DevicePlatform.Windows, Instant.ofEpochMilli(1_800_000_000_000L), false), device)
        assertEquals(DevicePlatform.Android, "android".toDevicePlatform())
        assertEquals(DevicePlatform.Other, "cli".toDevicePlatform())
        assertEquals(DevicePlatform.Other, "unknown".toDevicePlatform())
    }

    @Test
    fun onlyTheDeviceTheSpaceDidNotVerifyIsMarkedAsSuch() {
        fun verified(state: DeviceStateKind) = FfiDevice("d", "PC", "windows", state, null, false).toDomain().verified

        assertTrue(verified(DeviceStateKind.VERIFIED))
        assertFalse(verified(DeviceStateKind.UNVERIFIED))
    }

    @Test
    fun aRevokedDeviceIsNotAMemberAnyMore() {
        fun member(state: DeviceStateKind) = FfiDevice("d", "PC", "windows", state, null, false).isMember()

        assertTrue(member(DeviceStateKind.VERIFIED))
        assertTrue(member(DeviceStateKind.UNVERIFIED))
        assertFalse(member(DeviceStateKind.REVOKED))
        assertFalse(member(DeviceStateKind.REVOKED_BY_RELAY))
    }

    @Test
    fun theStateOfTheSpaceAndTheResultOfASendAreMapped() {
        assertEquals(SpaceState.Active, SpaceStateKind.ACTIVE.toDomain())
        assertEquals(SpaceState.Revoked, SpaceStateKind.REVOKED.toDomain())
        assertEquals(SpaceState.UpdateRequired, SpaceStateKind.UPDATE_REQUIRED.toDomain())
        assertEquals(SpaceState.Blocked, SpaceStateKind.BLOCKED.toDomain())
        assertEquals(SpaceState.NotFound, SpaceStateKind.NOT_FOUND.toDomain())
        assertEquals(DispatchResult.Sent, DispatchKind.SENT.toDomain())
        assertEquals(DispatchResult.RetryLater, DispatchKind.RETRY_LATER.toDomain())
        assertEquals(DispatchResult.NotFound, DispatchKind.NOT_FOUND.toDomain())
        assertEquals("nothing more to try", DispatchResult.NotFound, DispatchKind.FAILED.toDomain())
    }

    @Test
    fun anOutgoingImageCarriesItsThumbnailUnlessTheItemIsHidden() {
        val image = DraftAttachment("/tmp/p.jpg", "p.jpg", "image/jpeg", 3)
        val thumb = DraftThumbnail(byteArrayOf(1, 2, 3), "image/jpeg")
        val shown = OutgoingDraft(id = "a", attachments = listOf(image)).toFfi { thumb }.attachments.single()
        assertTrue(byteArrayOf(1, 2, 3).contentEquals(shown.thumbnail))
        assertEquals("image/jpeg", shown.thumbnailMime)

        val hidden = OutgoingDraft(id = "b", attachments = listOf(image), secret = true).toFfi { thumb }.attachments.single()
        assertNull(hidden.thumbnail)
        assertNull(hidden.thumbnailMime)
    }

    @Test
    fun theThumbnailsOfOneItemStayWithinTheirBudget() {
        val image = DraftAttachment("/tmp/p.jpg", "p.jpg", "image/jpeg", 3)
        val big = DraftThumbnail(ByteArray(THUMBNAILS_BUDGET_BYTES / 2 - 1), "image/jpeg")
        val ffi = OutgoingDraft(id = "c", attachments = List(4) { image }).toFfi { big }
        assertEquals(listOf(true, true, false, false), ffi.attachments.map { it.thumbnail != null })
    }
}
