package eu.shaffe.tomi.core

import uniffi.tomi_core.DeliveryKind
import uniffi.tomi_core.DeviceStateKind
import uniffi.tomi_core.DispatchKind
import uniffi.tomi_core.FfiAttachment
import uniffi.tomi_core.FfiDelivery
import uniffi.tomi_core.FfiDeparture
import uniffi.tomi_core.FfiDepartureKind
import uniffi.tomi_core.FfiDepartureOutcome
import uniffi.tomi_core.FfiPendingReason
import uniffi.tomi_core.FfiRefusalReason
import uniffi.tomi_core.FfiDevice
import uniffi.tomi_core.FfiDraft
import uniffi.tomi_core.FfiDraftAttachment
import uniffi.tomi_core.FfiItem
import uniffi.tomi_core.FfiItemContent
import uniffi.tomi_core.FfiRelayRevocation
import uniffi.tomi_core.ReadPolicyKind
import uniffi.tomi_core.SpaceStateKind
import uniffi.tomi_core.TextContent
import uniffi.tomi_core.TextFormat
import uniffi.tomi_core.UnreadableKind
import java.time.Instant

/*
 * The UniFFI records of the Rust core, as the app's own model. These conversions are the only place where the generated records are read,
 * and each one is dropped as soon as it is converted: the `toString()` of a generated data class prints every field, content and
 * verification code included. Nothing here logs, formats or keeps a record, and no exception message is built from one.
 */

internal fun String.toDevicePlatform(): DevicePlatform = when (lowercase()) {
    "android" -> DevicePlatform.Android
    "windows" -> DevicePlatform.Windows
    else -> DevicePlatform.Other
}

internal fun FfiDevice.toDomain(): Device =
    Device(id, name, platform.toDevicePlatform(), lastSeenAtMs?.let(Instant::ofEpochMilli), isThisDevice, verified = state != DeviceStateKind.UNVERIFIED)

/** Devices that are still members: a revoked device is not shown in the list of devices of the space. */
internal fun FfiDevice.isMember(): Boolean = state == DeviceStateKind.VERIFIED || state == DeviceStateKind.UNVERIFIED

internal fun FfiRelayRevocation.toDomain(): RelayRevocation = RelayRevocation(
    deviceId = deviceId,
    deviceName = name,
    via = when (via) {
        "admin" -> RelayRevocationVia.Admin
        "self" -> RelayRevocationVia.Self
        else -> RelayRevocationVia.Other
    },
)

internal fun SpaceStateKind.toDomain(): SpaceState = when (this) {
    SpaceStateKind.ACTIVE -> SpaceState.Active
    SpaceStateKind.REVOKED -> SpaceState.Revoked
    SpaceStateKind.UPDATE_REQUIRED -> SpaceState.UpdateRequired
    SpaceStateKind.BLOCKED -> SpaceState.Blocked
    SpaceStateKind.NOT_FOUND -> SpaceState.NotFound
}

internal fun FfiPendingReason.toDomain(): PendingReason = when (this) {
    FfiPendingReason.NETWORK -> PendingReason.Network
    FfiPendingReason.SERVER -> PendingReason.Server
    FfiPendingReason.NOT_FOUND -> PendingReason.NotFound
    FfiPendingReason.NOT_SENT -> PendingReason.NotSent
}

internal fun FfiRefusalReason.toDomain(): RefusalReason = when (this) {
    FfiRefusalReason.LAST_DEVICE -> RefusalReason.LastDevice
    FfiRefusalReason.UPDATE_REQUIRED -> RefusalReason.UpdateRequired
    FfiRefusalReason.RELAY_TOO_OLD -> RefusalReason.RelayTooOld
    FfiRefusalReason.UNEXPECTED -> RefusalReason.Unexpected
}

internal fun FfiDepartureOutcome.toDomain(): DepartureResult = when (this) {
    FfiDepartureOutcome.Erased -> DepartureResult.Erased
    is FfiDepartureOutcome.Pending -> DepartureResult.Pending(reason.toDomain())
    is FfiDepartureOutcome.Refused -> DepartureResult.Refused(reason.toDomain())
    FfiDepartureOutcome.Cancelled -> DepartureResult.Cancelled
    FfiDepartureOutcome.NothingPending -> DepartureResult.NothingPending
}

internal fun FfiDeparture.toDomain(): Departure = Departure(
    kind = when (kind) {
        FfiDepartureKind.LEAVE -> DepartureKind.Leave
        FfiDepartureKind.DELETE_SPACE -> DepartureKind.DeleteSpace
    },
    wiping = wiping,
    sinceMs = sinceMs,
    last = last?.toDomain(),
)

internal fun DispatchKind.toDomain(): DispatchResult = when (this) {
    DispatchKind.SENT -> DispatchResult.Sent
    DispatchKind.RETRY_LATER -> DispatchResult.RetryLater
    // "Failed": refused for good by the relay, or expired before it left. Nothing more to try, which is what NotFound tells the scheduler;
    // the item itself shows its failed delivery in the feed.
    DispatchKind.NOT_FOUND, DispatchKind.FAILED -> DispatchResult.NotFound
}

private fun DeliveryKind.toDomain(): DeliveryState = when (this) {
    DeliveryKind.QUEUED -> DeliveryState.Queued
    DeliveryKind.SENT -> DeliveryState.Sent
    DeliveryKind.DELIVERED -> DeliveryState.Delivered
    DeliveryKind.EXPIRED_BEFORE_DELIVERY -> DeliveryState.ExpiredBeforeDelivery
    DeliveryKind.FAILED -> DeliveryState.Failed
}

private fun FfiDelivery.toDomain(): DeviceDelivery = DeviceDelivery(deviceId, deviceName, state.toDomain())

private fun ReadPolicyKind.toDomain(): ReadPolicy = when (this) {
    ReadPolicyKind.NORMAL -> ReadPolicy.Normal
    ReadPolicyKind.AFTER_FIRST_READ -> ReadPolicy.AfterFirstRead
}

private fun ReadPolicy.toFfi(): ReadPolicyKind = when (this) {
    ReadPolicy.Normal -> ReadPolicyKind.NORMAL
    ReadPolicy.AfterFirstRead -> ReadPolicyKind.AFTER_FIRST_READ
}

private fun TextFormat.toDomain(): SourceFormat = when (this) {
    TextFormat.PLAIN -> SourceFormat.Plain
    TextFormat.MARKDOWN -> SourceFormat.Markdown
    TextFormat.HTML -> SourceFormat.Html
}

private fun SourceFormat.toFfi(): TextFormat = when (this) {
    SourceFormat.Plain -> TextFormat.PLAIN
    SourceFormat.Markdown -> TextFormat.MARKDOWN
    SourceFormat.Html -> TextFormat.HTML
}

private fun TextContent.toDomain(): RichText = RichText(plain, html, markdown, sourceFormat.toDomain())

private fun RichText.toFfi(): TextContent = TextContent(plain, html, markdown, sourceFormat.toFfi())

/** The file itself is reached through [TomiCore.attachmentUri]: the path of the core's decrypted cache is not a URI the UI could open. */
private fun FfiAttachment.toDomain(): Attachment =
    Attachment(
        name,
        mime,
        if (size > Long.MAX_VALUE.toULong()) Long.MAX_VALUE else size.toLong(),
        thumbnail,
        uri = null,
        // Stable position in the payload: what the core is asked for. A slot above Int.MAX_VALUE cannot exist (a payload is a few MB).
        slot = if (slot > Int.MAX_VALUE.toUInt()) Int.MAX_VALUE else slot.toInt(),
    )

/**
 * The item as the app's model, or null for an item that must not be shown: one sealed with a key that a rotation replaced
 * ([UnreadableKind.SUSPECT], "not displayed" in the contract of the core). Every other unreadable item stays in the feed.
 */
internal fun FfiItem.toDomain(): TomiItem? {
    val converted = when (val c = content) {
        is FfiItemContent.Readable -> ItemContent.Readable(
            title = c.title,
            text = c.text?.toDomain(),
            url = c.url,
            attachments = c.attachments.map { it.toDomain() },
            secret = c.secret,
        )
        is FfiItemContent.Unreadable -> when (c.reason) {
            UnreadableKind.UPDATE_REQUIRED -> ItemContent.Unreadable(UnreadableReason.UpdateRequired)
            // A key that has not arrived yet is retried after the next rotation: for the user it is "unreadable for now".
            UnreadableKind.DECRYPTION_FAILED, UnreadableKind.KEY_PENDING -> ItemContent.Unreadable(UnreadableReason.DecryptionFailed)
            UnreadableKind.SUSPECT -> return null
        }
    }
    return TomiItem(
        id = id,
        content = converted,
        sourceDevice = DeviceRef(sourceDeviceId, sourceDeviceName),
        createdAt = Instant.ofEpochMilli(createdAtMs),
        expiresAt = expiresAtMs?.let(Instant::ofEpochMilli),
        readPolicy = readPolicy.toDomain(),
        read = read,
        deliveries = deliveries.map { it.toDomain() },
        pinned = pinned,
        pinnedAt = pinnedAtMs?.let(Instant::ofEpochMilli),
        archived = archived,
        archivedAt = archivedAtMs?.let(Instant::ofEpochMilli),
    )
}

/**
 * The draft the core stores in its outbox. The thumbnails of attachments are the sender's job: [thumbnailOf] makes them (null for
 * none; it reads the staged file, so call this off the main thread). A hidden item carries none: its images are only shown after
 * the biometric gate, never as a preview. Together they stay under [THUMBNAILS_BUDGET_BYTES]: the next ones are left out.
 */
internal fun OutgoingDraft.toFfi(thumbnailOf: (DraftAttachment) -> DraftThumbnail? = { null }): FfiDraft {
    var budget = THUMBNAILS_BUDGET_BYTES
    return FfiDraft(
        id = id,
        title = title,
        text = text?.toFfi(),
        url = url,
        attachments = attachments.map {
            val thumbnail = (if (secret || budget <= 0) null else thumbnailOf(it))?.takeIf { t -> t.bytes.size <= budget }
            if (thumbnail != null) budget -= thumbnail.bytes.size
            FfiDraftAttachment(path = it.path, name = it.name, mime = it.mime, thumbnail = thumbnail?.bytes, thumbnailMime = thumbnail?.mime)
        },
        secret = secret,
        expiresAtMs = expiresAt?.toEpochMilli(),
        readPolicy = readPolicy.toFfi(),
    )
}

/** All the thumbnails of one item, in bytes (base64 adds a third): well under the ~1 MB the relay takes for an item. */
internal const val THUMBNAILS_BUDGET_BYTES = 384 * 1024
