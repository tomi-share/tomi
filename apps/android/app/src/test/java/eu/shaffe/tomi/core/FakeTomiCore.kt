package eu.shaffe.tomi.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.time.Clock
import java.time.Duration
import java.time.Instant
import java.util.UUID

/**
 * In-memory stand-in for the Rust core, for the unit tests, filled with sample data. It performs no cryptography and no I/O.
 *
 * Behaviour: pairing succeeds for any well-formed input (the verification code is always the same), an invitation shows a fixed code and
 * a sample device joins a few seconds later, sends are accepted after a short delay and the first recipient reports "delivered" a bit
 * later, the second one stays on "sent". A departure succeeds at once unless this device is alone ([RefusalReason.LastDevice]) or a test
 * set [nextDepartureResult]; files have no URI.
 */
class FakeTomiCore(
    private val scope: CoroutineScope,
    private val clock: Clock = Clock.systemUTC(),
    private val thumbnailFactory: (seed: Int) -> ByteArray? = { null },
    private val dispatchLatencyMs: Long = 400,
    private val deliveryLatencyMs: Long = 2_000,
    startPaired: Boolean = false,
) : TomiCore {

    private val me = Device(LOCAL_ID, "Pixel 8", DevicePlatform.Android, clock.instant(), isThisDevice = true)
    private val pcOffice = Device("dev-pc-bureau", "PC-Bureau", DevicePlatform.Windows, clock.instant().minus(Duration.ofMinutes(3)), false)
    private val pcHome = Device("dev-pc-maison", "PC-Maison", DevicePlatform.Windows, clock.instant().minus(Duration.ofHours(20)), false)

    private val pairedState = MutableStateFlow(startPaired)
    private val localDeviceState = MutableStateFlow(me)
    private val devicesState = MutableStateFlow(listOf(me, pcOffice, pcHome))
    private val itemsState = MutableStateFlow(demoItems())
    private val spaceStateFlow = MutableStateFlow(SpaceState.Active)

    @Volatile
    private var pairingPending = false

    /** Guards the Queued -> Sent transition against [cancelIfQueued]. */
    private val outboxLock = Any()

    override val spaceState: StateFlow<SpaceState> = spaceStateFlow.asStateFlow()
    override val isPaired: StateFlow<Boolean> = pairedState.asStateFlow()
    override val localDevice: StateFlow<Device> = localDeviceState.asStateFlow()
    override val devices: Flow<List<Device>> = devicesState.asStateFlow()
    override val items: Flow<List<TomiItem>> = itemsState.asStateFlow()

    override suspend fun getItem(id: String): TomiItem? = itemsState.value.firstOrNull { it.id == id }

    override suspend fun enqueue(draft: OutgoingDraft) {
        val recipients = devicesState.value.filterNot { it.id == LOCAL_ID }
        val item = readableItem(
            id = draft.id,
            title = draft.title,
            text = draft.text,
            url = draft.url,
            attachments = draft.attachments.map { Attachment(it.name, it.mime, it.size) },
            secret = draft.secret,
            sourceDevice = DeviceRef(LOCAL_ID, localDeviceState.value.name),
            createdAt = clock.instant(),
            expiresAt = draft.expiresAt,
            readPolicy = draft.readPolicy,
            read = true,
            deliveries = recipients.map { DeviceDelivery(it.id, it.name, DeliveryState.Queued) },
        )
        itemsState.update { listOf(item) + it.filterNot { existing -> existing.id == item.id } }
    }

    override suspend fun sync(pushToken: String?) {
        // There is no relay. The real core fetches pending items and registers [pushToken] here.
    }

    override suspend fun dispatch(id: String): DispatchResult {
        val item = getItem(id) ?: return DispatchResult.NotFound
        if (item.deliveries.none { it.state == DeliveryState.Queued }) return DispatchResult.Sent
        delay(dispatchLatencyMs)
        // The "relay accepts the item" step and cancelIfQueued exclude each other: exactly one of them wins.
        val accepted = synchronized(outboxLock) {
            if (itemsState.value.none { it.id == id }) {
                false
            } else {
                updateDeliveries(id) { d -> if (d.state == DeliveryState.Queued) d.copy(state = DeliveryState.Sent) else d }
                true
            }
        }
        if (!accepted) return DispatchResult.NotFound
        scope.launch {
            delay(deliveryLatencyMs)
            val firstRecipient = getItem(id)?.deliveries?.firstOrNull()?.deviceId ?: return@launch
            updateDeliveries(id) { d ->
                if (d.deviceId == firstRecipient && d.state == DeliveryState.Sent) d.copy(state = DeliveryState.Delivered) else d
            }
        }
        return DispatchResult.Sent
    }

    override suspend fun cancelIfQueued(id: String): Boolean = synchronized(outboxLock) {
        val item = itemsState.value.firstOrNull { it.id == id } ?: return@synchronized false
        val stillQueued = item.deliveries.isNotEmpty() && item.deliveries.all { it.state == DeliveryState.Queued }
        if (stillQueued) itemsState.update { list -> list.filterNot { it.id == id } }
        stillQueued
    }

    override suspend fun deleteLocal(id: String) {
        itemsState.update { list -> list.filterNot { it.id == id } }
    }

    override suspend fun deleteEverywhere(id: String) {
        // The real core would send a signed tombstone; the demo just forgets the item.
        deleteLocal(id)
    }

    override suspend fun markRead(id: String) {
        val item = getItem(id) ?: return
        // An unreadable item is never consumed: the user has not seen it (see TomiCore.markRead).
        if (!item.isReadable) return
        if (item.readPolicy == ReadPolicy.AfterFirstRead && item.sourceDevice.id != LOCAL_ID) {
            // The real core also publishes a signed "everywhere" tombstone here; the demo has no other device.
            deleteLocal(id)
        } else {
            itemsState.update { list -> list.map { if (it.id == id) it.copy(read = true) else it } }
        }
    }

    /** No attachment has a file here. */
    override suspend fun attachmentUri(itemId: String, slot: Int): String? = null

    /** Tests: what [supportsAttachmentRemoval] answers (the real core: false until a session heard the relay say yes). */
    @Volatile
    var attachmentRemovalSupported: Boolean = true

    /** Tests: every (item, slot, everywhere) asked of [deleteAttachment], in order. */
    val attachmentRemovals = java.util.concurrent.CopyOnWriteArrayList<Triple<String, Int, Boolean>>()

    override suspend fun supportsAttachmentRemoval(): Boolean = attachmentRemovalSupported

    /**
     * Like the real core: idempotent (unknown slot ignored), the others keep their slot, the whole item goes when nothing is left
     * (no attachment, no text, no link) and a queued item refuses it. The demo has no other device: "everywhere" is the same.
     */
    override suspend fun deleteAttachment(itemId: String, slot: Int, everywhere: Boolean): AttachmentRemoval {
        attachmentRemovals += Triple(itemId, slot, everywhere)
        val item = getItem(itemId) ?: return AttachmentRemoval.AttachmentRemoved
        if (item.deliveries.isNotEmpty() && item.deliveries.all { it.state == DeliveryState.Queued }) {
            throw TomiCoreException("item still queued")
        }
        val readable = item.content as? ItemContent.Readable ?: return AttachmentRemoval.AttachmentRemoved
        // From now on the list can have gaps: every attachment gets the slot it has now.
        val slotted = readable.attachments.mapIndexed { index, a -> a.withSlot(a.slot ?: index) }
        val remaining = slotted.filterNot { it.slot == slot }
        if (remaining.size == slotted.size) return AttachmentRemoval.AttachmentRemoved
        if (remaining.isEmpty() && readable.text?.plain.isNullOrBlank() && readable.url.isNullOrBlank()) {
            deleteLocal(itemId)
            return AttachmentRemoval.ItemDeleted
        }
        itemsState.update { list -> list.map { if (it.id == itemId) it.copy(content = readable.copy(attachments = remaining)) else it } }
        return AttachmentRemoval.AttachmentRemoved
    }

    override suspend fun renameDevice(id: String, name: String) {
        val clean = name.trim().take(MAX_DEVICE_NAME)
        if (clean.isEmpty()) return
        devicesState.update { list -> list.map { if (it.id == id) it.copy(name = clean) else it } }
        if (id == LOCAL_ID) localDeviceState.update { it.copy(name = clean) }
    }

    override suspend fun revokeDevice(id: String) {
        if (id == LOCAL_ID) return
        devicesState.update { list -> list.filterNot { it.id == id } }
    }

    private val departureState = MutableStateFlow<Departure?>(null)
    override val departure: StateFlow<Departure?> = departureState.asStateFlow()
    override val departureRunning: StateFlow<Boolean> = MutableStateFlow(false).asStateFlow()

    /** Tests: when set, the next departure attempt gives this instead of its normal outcome (and changes what it says). */
    @Volatile
    var nextDepartureResult: DepartureResult? = null

    /** Tests: whether "Effacer quand même" is allowed. */
    @Volatile
    var eraseAllowed: Boolean = false

    /** Like the real core: refused when this device is alone in the space, otherwise the space is forgotten. */
    override suspend fun leaveSpace(): DepartureResult = attempt(DepartureKind.Leave) {
        if (devicesState.value.size <= 1) DepartureResult.Refused(RefusalReason.LastDevice) else DepartureResult.Erased
    }

    override suspend fun deleteSpace(): DepartureResult = attempt(DepartureKind.DeleteSpace) { DepartureResult.Erased }

    override suspend fun retryDeparture(): DepartureResult {
        val pending = departureState.value ?: return DepartureResult.Failed
        return attempt(pending.kind) { DepartureResult.Erased }
    }

    override suspend fun continueDeparture(): DepartureResult {
        val pending = departureState.value ?: return DepartureResult.NothingPending
        return attempt(pending.kind) { DepartureResult.Erased }
    }

    override suspend fun cancelDeparture(): Boolean {
        val pending = departureState.value ?: return false
        if (pending.wiping) return false
        departureState.value = null
        return true
    }

    override suspend fun canEraseWithoutProof(): Boolean = eraseAllowed && pairedState.value

    override suspend fun eraseWithoutProof(): EraseResult {
        if (!canEraseWithoutProof()) return EraseResult.NotAllowed
        forgetSpace()
        return EraseResult.Erased
    }

    /** Marker first, then the outcome: a pending result keeps the marker, a refusal removes it, an erasure forgets the space. */
    private fun attempt(kind: DepartureKind, normal: () -> DepartureResult): DepartureResult {
        if (!pairedState.value) return DepartureResult.Failed
        departureState.value = Departure(kind, wiping = false, sinceMs = clock.millis(), last = departureState.value?.last)
        val result = nextDepartureResult?.also { nextDepartureResult = null } ?: normal()
        when (result) {
            DepartureResult.Erased -> forgetSpace()
            is DepartureResult.Pending -> departureState.value = departureState.value?.copy(last = result.reason)
            is DepartureResult.Refused, DepartureResult.Cancelled, DepartureResult.NothingPending -> departureState.value = null
            DepartureResult.Failed -> Unit
        }
        return result
    }

    private fun forgetSpace() {
        itemsState.value = emptyList()
        devicesState.value = listOf(me)
        spaceStateFlow.value = SpaceState.Active
        departureState.value = null
        pairedState.value = false
    }

    override suspend fun pin(id: String) = setPinned(id, true)

    override suspend fun unpin(id: String) = setPinned(id, false)

    /**
     * Same semantics as the real core (spec/pin-contract.md): idempotent (an already-pinned item keeps its original
     * `pinnedAt`), unknown id ignored, effect visible at once in [items], and the order of [items] never changes (the
     * Pinned section is built by the UI from `pinnedAt`). Pinning never touches expiry or read state.
     */
    private fun setPinned(id: String, pinned: Boolean) {
        itemsState.update { list ->
            list.map { item ->
                when {
                    item.id != id || item.pinned == pinned -> item
                    // Pin and archive exclude each other: pinning an archived item brings it back to the feed.
                    pinned -> item.copy(pinned = true, pinnedAt = clock.instant(), archived = false, archivedAt = null)
                    else -> item.copy(pinned = false, pinnedAt = null)
                }
            }
        }
    }

    // DEMO ONLY: the archive state lives in memory here. The real state, synchronised between devices like the pin
    // (spec/archive-contract.md), comes with the Rust core and the spec; the UI will not change.
    override suspend fun archive(id: String) = setArchived(id, true)

    override suspend fun unarchive(id: String) = setArchived(id, false)

    /**
     * Same semantics as [pin]: idempotent (an already-archived item keeps its original `archivedAt`), unknown id ignored,
     * effect visible at once in [items], order of [items] unchanged. Archiving unpins; it never touches the read state,
     * the expiry or the deliveries (an "after first read" secret must not be consumed by a swipe).
     */
    private fun setArchived(id: String, archived: Boolean) {
        itemsState.update { list ->
            list.map { item ->
                when {
                    item.id != id -> item
                    archived && item.archived && !item.pinned -> item
                    archived -> item.copy(archived = true, archivedAt = item.archivedAt ?: clock.instant(), pinned = false, pinnedAt = null)
                    !item.archived -> item
                    else -> item.copy(archived = false, archivedAt = null)
                }
            }
        }
    }

    override suspend fun pairWithQr(payload: String): PairingResult {
        delay(PAIRING_LATENCY_MS)
        return if (payload.isBlank()) PairingResult.Failure(PairingFailure.InvalidCode) else awaitVerification(relay?.let(RelayAddress::authority) ?: DEFAULT_RELAY_AUTHORITY)
    }

    override suspend fun pairWithCode(code: String, relayUrl: String?): PairingResult {
        delay(PAIRING_LATENCY_MS)
        if (code.trim().length < MIN_CODE_LENGTH) return PairingResult.Failure(PairingFailure.InvalidCode)
        // Like the core: no relay at all (none on the device, none typed) is refused before anything is sent.
        val relay = relayUrl ?: relay ?: return PairingResult.Failure(PairingFailure.RelayNotConfigured)
        return awaitVerification(RelayAddress.authority(relay))
    }

    /** The relay of the device: a development build has one, a release build none (`null`). A test sets it. */
    
    var relay: String? = DEFAULT_RELAY

    override suspend fun relayUrl(): String? = relay

    override suspend fun confirmPairing(importHistory: Boolean) {
        if (!pairingPending) return
        pairingPending = false
        // Demo: the demo feed stands for the history of the space; "no" starts from an empty feed.
        if (!importHistory) itemsState.value = emptyList()
        pairedState.value = true
    }

    override suspend fun cancelPairing() {
        pairingPending = false
    }

    /** Demo invitation: a "Pixel" says hello a few seconds after the code is shown, then joins once the user confirmed. */
    override suspend fun startInvitation(): InviteSession {
        delay(PAIRING_LATENCY_MS / 2)
        return FakeInviteSession(clock) { name ->
            val device = Device("dev-demo-${UUID.randomUUID()}", name, DevicePlatform.Android, clock.instant(), isThisDevice = false)
            devicesState.update { it + device }
            device.id
        }
    }

    /** Demo: any non-blank token is the right one. The space starts empty, with this device alone in it. */
    override suspend fun createSpace(installToken: SensitiveText, deviceName: String, relayUrl: String?) {
        delay(PAIRING_LATENCY_MS)
        if (relayUrl == null && relay == null) throw PairingException(PairingFailure.RelayNotConfigured)
        // The relay used for the creation is kept once the space exists.
        relayUrl?.let { relay = it }
        if (installToken.isBlank) throw PairingException(PairingFailure.InstallTokenInvalid)
        if (!isValidDeviceName(deviceName)) throw PairingException(PairingFailure.Unexpected)
        localDeviceState.update { it.copy(name = deviceName.trim()) }
        devicesState.value = listOf(localDeviceState.value)
        itemsState.value = emptyList()
        pairedState.value = true
    }

    /** Demo: nothing is ever interrupted. */
    override suspend fun resumePairing(): ResumeOutcome = ResumeOutcome.NothingPending

    /** The rule of the real core: blanks at either end are trimmed first, then 1 to 64 characters and no control character. */
    override fun isValidDeviceName(name: String): Boolean {
        val trimmed = name.trim()
        return trimmed.isNotEmpty() && trimmed.codePointCount(0, trimmed.length) <= MAX_PAIRING_DEVICE_NAME && trimmed.none { it.isISOControl() }
    }

    /** Pretends another device just sent something. */
    fun simulateIncoming(kind: ItemKind) {
        val now = clock.instant()
        val source = DeviceRef(pcOffice.id, pcOffice.name)
        val id = UUID.randomUUID().toString()
        // No expiry unless the sender chose one (product decision): only the secret demo opts in, to "after first read".
        val item = when (kind) {
            ItemKind.Secret -> readableItem(
                id, source, now,
                secret = true,
                text = RichText("Tr0ub4dor&3-demo"),
                readPolicy = ReadPolicy.AfterFirstRead,
            )
            ItemKind.Link -> readableItem(id, source, now, title = "Documentation Android", url = "https://developer.android.com/")
            ItemKind.Image -> readableItem(
                id, source, now,
                attachments = listOf(demoPortrait()),
            )
            ItemKind.File -> readableItem(id, source, now, attachments = listOf(Attachment("facture.pdf", "application/pdf", 482_100)))
            ItemKind.RichText -> readableItem(
                id, source, now,
                text = RichText("Titre\nUn paragraphe.", markdown = "# Titre\n\nUn **paragraphe**."),
            )
            ItemKind.Text -> readableItem(id, source, now, text = RichText("Rendez-vous à 14 h salle B"))
            ItemKind.Unreadable -> TomiItem(
                id = id,
                content = ItemContent.Unreadable(UnreadableReason.UpdateRequired),
                sourceDevice = source,
                createdAt = now,
            )
        }
        itemsState.update { listOf(item) + it }
    }

    /** Fake: always the same verification code; the real core derives it from the key exchange. */
    private fun awaitVerification(relayAuthority: String): PairingResult {
        pairingPending = true
        return PairingResult.AwaitingVerification(
            peer = PairingPeer(pcOffice.name, DevicePlatform.Windows),
            verificationCode = VerificationCode(FAKE_VERIFICATION_CODE),
            historyItemCount = itemsState.value.size,
            historyBytes = DEMO_HISTORY_BYTES,
            memberCount = devicesState.value.size,
            expiresInMs = DEMO_PAIRING_VALIDITY_MS,
            relayAuthority = relayAuthority,
        )
    }

    private fun demoPortrait() = Attachment(DEMO_PORTRAIT_EXIF, "image/jpeg", 84_803, thumbnailFactory(3))

    private fun demoLandscape() = Attachment(DEMO_LANDSCAPE, "image/jpeg", 59_200, thumbnailFactory(1))

    private fun updateDeliveries(id: String, transform: (DeviceDelivery) -> DeviceDelivery) {
        itemsState.update { list ->
            list.map { if (it.id == id) it.copy(deliveries = it.deliveries.map(transform)) else it }
        }
    }

    private fun demoItems(): List<TomiItem> {
        val now = clock.instant()
        fun ago(d: Duration): Instant = now.minus(d)
        val office = DeviceRef(pcOffice.id, pcOffice.name)
        val home = DeviceRef(pcHome.id, pcHome.name)
        val local = DeviceRef(LOCAL_ID, me.name)
        return listOf(
            readableItem(
                id = "demo-link-1",
                title = "Material Design 3 dans Compose",
                url = "https://developer.android.com/develop/ui/compose/designsystems/material3",
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(2)),
                read = false,
            ),
            readableItem(
                id = "demo-secret-1",
                title = "Wi-Fi invités",
                text = RichText("Hx7!pQ2-demo-zR9"),
                secret = true,
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(9)),
                read = false,
                pinned = true,
                pinnedAt = ago(Duration.ofHours(30)),
            ),
            readableItem(
                id = "demo-md-1",
                text = RichText(
                    plain = "Courses\nLait\nPain\nPensez à vérifier la date.",
                    markdown = "# Courses\n\n- Lait\n- Pain\n\n**Pensez** à vérifier la [date](https://example.org).",
                ),
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(35)),
                // The only demo item with a time limit: the sender explicitly picked "1 day".
                expiresAt = ago(Duration.ofMinutes(35)).plus(Duration.ofDays(1)),
                read = false,
            ),
            // Markdown with fenced code: Rust and JavaScript are coloured by the Enriched Markdown grammars, PHP by our own
            // grammar (libtomi_highlight). The image at the end is never fetched (no network, see MarkdownGuard). "Markdown" shows this source
            // literally.
            readableItem(
                id = "demo-md-code",
                text = RichText(
                    plain = "Notes de code\nBloc Rust, bloc JavaScript, bloc PHP.",
                    markdown = DEMO_MARKDOWN_CODE,
                ),
                sourceDevice = home,
                createdAt = ago(Duration.ofMinutes(12)),
                read = false,
            ),
            // Several code files (Rust, JavaScript, Kotlin and PHP): code screen, file tabs, search. PHP is coloured by our own grammar;
            // the Kotlin file has no grammar and stays in plain monospace.
            readableItem(
                id = "demo-code-multi",
                title = "Projet démo (4 fichiers)",
                attachments = listOf(
                    Attachment(DEMO_CODE_RUST, "text/x-rust", 1_368),
                    Attachment(DEMO_CODE_JS, "text/javascript", 1_098),
                    Attachment(DEMO_CODE_KOTLIN, "text/x-kotlin", 1_087),
                    Attachment(DEMO_CODE_PHP, "application/x-httpd-php", 1_080),
                ),
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(18)),
                read = false,
            ),
            // One PHP file: a tap on the card opens the code screen directly.
            readableItem(
                id = "demo-code-php",
                attachments = listOf(Attachment(DEMO_CODE_PHP, "application/x-httpd-php", 1_080)),
                sourceDevice = home,
                createdAt = ago(Duration.ofMinutes(22)),
                read = false,
            ),
            // One code file: a tap on the card opens the code screen directly.
            readableItem(
                id = "demo-code-single",
                attachments = listOf(Attachment(DEMO_CODE_RUST, "text/x-rust", 1_368)),
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(25)),
                read = true,
            ),
            // Image + code file: each row opens its own viewer.
            readableItem(
                id = "demo-code-mixed",
                title = "Capture et script",
                attachments = listOf(demoLandscape(), Attachment(DEMO_CODE_JS, "text/javascript", 1_098)),
                sourceDevice = home,
                createdAt = ago(Duration.ofMinutes(40)),
                read = true,
            ),
            // Hidden item with a code file: the code screen asks for the biometric gate before reading anything.
            readableItem(
                id = "demo-secret-code",
                title = "Script de connexion (démo)",
                attachments = listOf(Attachment(DEMO_CODE_PYTHON, "text/x-python", 442)),
                secret = true,
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(45)),
                read = true,
            ),
            readableItem(
                id = "demo-sent-link",
                title = "Itinéraire du week-end",
                url = "https://example.org/itineraire",
                sourceDevice = local,
                createdAt = ago(Duration.ofHours(2)),
                read = true,
                deliveries = listOf(
                    DeviceDelivery(pcOffice.id, pcOffice.name, DeliveryState.Delivered),
                    DeviceDelivery(pcHome.id, pcHome.name, DeliveryState.Delivered),
                ),
                pinned = true,
                pinnedAt = ago(Duration.ofHours(1)),
            ),
            readableItem(
                id = "demo-html-1",
                text = RichText(
                    plain = "Réunion jeudi\nOrdre du jour : budget, planning.",
                    html = "<h2>Réunion jeudi</h2><p>Ordre du jour : <b>budget</b>, <i>planning</i>.</p>" +
                        "<script>alert('never runs')</script>",
                ),
                sourceDevice = home,
                createdAt = ago(Duration.ofHours(5)),
                read = true,
            ),
            readableItem(
                id = "demo-image-1",
                attachments = listOf(demoLandscape()),
                sourceDevice = office,
                createdAt = ago(Duration.ofHours(8)),
                read = true,
            ),
            // Portrait photo stored sideways with EXIF orientation 6: the viewer must show it upright.
            readableItem(
                id = "demo-image-portrait",
                attachments = listOf(demoPortrait()),
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(20)),
                read = false,
            ),
            // Several images: swipe between pages (the first is the landscape one, the second the portrait EXIF one).
            readableItem(
                id = "demo-image-multi",
                title = "Photos du week-end",
                attachments = listOf(demoLandscape(), demoPortrait(), demoLandscape()),
                sourceDevice = home,
                createdAt = ago(Duration.ofMinutes(50)),
                read = true,
            ),
            // Hidden item with an image: opening, saving and sharing the image all ask for the biometric gate.
            readableItem(
                id = "demo-secret-image",
                title = "Photo de la carte (démo)",
                attachments = listOf(demoPortrait()),
                secret = true,
                sourceDevice = office,
                createdAt = ago(Duration.ofMinutes(15)),
                read = true,
            ),
            readableItem(
                id = "demo-file-1",
                title = "Contrat",
                attachments = listOf(
                    Attachment("contrat-signe.pdf", "application/pdf", 842_000),
                    Attachment("annexe.docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", 56_000),
                ),
                sourceDevice = home,
                createdAt = ago(Duration.ofDays(1)),
                read = true,
            ),
            readableItem(
                id = "demo-sent-secret",
                text = RichText("Code-demo-4821-Kp"),
                secret = true,
                sourceDevice = local,
                createdAt = ago(Duration.ofHours(3)),
                readPolicy = ReadPolicy.AfterFirstRead,
                read = true,
                deliveries = listOf(
                    DeviceDelivery(pcOffice.id, pcOffice.name, DeliveryState.Delivered),
                    DeviceDelivery(pcHome.id, pcHome.name, DeliveryState.Sent),
                ),
            ),
            TomiItem(
                id = "demo-unreadable-1",
                content = ItemContent.Unreadable(UnreadableReason.UpdateRequired),
                sourceDevice = home,
                createdAt = ago(Duration.ofDays(1).plusHours(6)),
                read = true,
            ),
            readableItem(
                id = "demo-text-1",
                text = RichText("Digicode du portail : voir la note, ne pas répondre à ce message."),
                sourceDevice = office,
                createdAt = ago(Duration.ofDays(2)),
                read = true,
            ),
            // Archived items, to try the "Archives" filter and the un-archive swipe (they are absent from the other filters).
            readableItem(
                id = "demo-archived-link",
                title = "Recette de la tarte aux pommes",
                url = "https://example.org/recettes/tarte-aux-pommes",
                sourceDevice = home,
                createdAt = ago(Duration.ofDays(9)),
                read = true,
                archived = true,
                archivedAt = ago(Duration.ofDays(3)),
            ),
            readableItem(
                id = "demo-archived-text",
                text = RichText("Numéro de suivi du colis : 6A12345678901 (livré mardi dernier)."),
                sourceDevice = office,
                createdAt = ago(Duration.ofDays(12)),
                read = true,
                archived = true,
                archivedAt = ago(Duration.ofDays(5)),
            ),
            // A hidden item archived while still unread: archiving did not mark it read, so an "after first read" secret survives.
            readableItem(
                id = "demo-archived-secret",
                title = "Code du coffre (démo)",
                text = RichText("4821-Kp-demo"),
                secret = true,
                sourceDevice = office,
                createdAt = ago(Duration.ofDays(14)),
                readPolicy = ReadPolicy.AfterFirstRead,
                read = false,
                archived = true,
                archivedAt = ago(Duration.ofDays(2)),
            ),
            readableItem(
                id = "demo-archived-file",
                title = "Devis plombier",
                attachments = listOf(Attachment("devis-plombier.pdf", "application/pdf", 215_400)),
                sourceDevice = home,
                createdAt = ago(Duration.ofDays(20)),
                read = true,
                archived = true,
                archivedAt = ago(Duration.ofDays(10)),
            ),
        )
    }

    companion object {
        const val LOCAL_ID = "dev-android-local"

        /** The relay of a development build in the tests: nothing listens there. */
        const val DEFAULT_RELAY = "https://relay.example.org"
        const val DEFAULT_RELAY_AUTHORITY = "relay.example.org"

        /** File names of the sample attachments (there are no files behind them). */
        private const val DEMO_PORTRAIT_EXIF = "demo_portrait_exif.jpg"
        private const val DEMO_LANDSCAPE = "demo_paysage.jpg"
        private const val DEMO_CODE_RUST = "demo_main.rs"
        private const val DEMO_CODE_JS = "demo_app.js"
        private const val DEMO_CODE_KOTLIN = "demo_notes.kt"
        private const val DEMO_CODE_PHP = "demo_index.php"
        private const val DEMO_CODE_PYTHON = "demo_secret.py"

        private val DEMO_MARKDOWN_CODE = """
            # Notes de code

            Un **exemple** de bloc Rust, coloré par tree-sitter :

            ```rust
            // Additionne deux nombres
            fn add(a: i32, b: i32) -> i32 {
                a + b
            }

            fn main() {
                let total = add(40, 2);
                println!("Total = {total}");
            }
            ```

            Puis un bloc JavaScript :

            ```js
            const greet = (name) => 'Bonjour ' + name + ' !';
            console.log(greet('Tomi'), 42, true, null);
            ```

            PHP a sa propre grammaire tree-sitter (hors Enriched Markdown) :

            ```php
            function add(int ${'$'}a, int ${'$'}b): int {
                return ${'$'}a + ${'$'}b; // somme
            }

            echo "Total : " . add(40, 2) . PHP_EOL;
            ```

            ![pixel de suivi](https://example.org/tracking.png)

            L'image ci-dessus n'est jamais téléchargée.
        """.trimIndent()
        private const val MAX_DEVICE_NAME = 40
        private const val MAX_PAIRING_DEVICE_NAME = 64
        private const val MIN_CODE_LENGTH = 4
        private const val PAIRING_LATENCY_MS = 800L
        private const val FAKE_VERIFICATION_CODE = "482913"
        private const val DEMO_HISTORY_BYTES = 12_400_000L
        private const val DEMO_PAIRING_VALIDITY_MS = 120_000L
    }
}

/**
 * Demo invitation of [FakeTomiCore]: same shape as the real one, no network. The code is always the same, the new device ("Pixel (démo)")
 * says hello 3 s after the code is shown with the fixed verification code 482913, and joins 1.2 s after the user confirmed.
 */
internal class FakeInviteSession(
    private val clock: Clock,
    private val register: (name: String) -> String,
) : InviteSession {
    private val startedAt = clock.millis()

    @Volatile
    private var over = false

    override val channelId: String = "4821"

    override fun code(): SensitiveText? = if (over) null else SensitiveText(DEMO_CODE)

    override fun link(): SensitiveText? = if (over) null else SensitiveText("https://relay.example.org/l/pair#$DEMO_CODE")

    override fun expiresInMs(): Long = (VALIDITY_MS - (clock.millis() - startedAt)).coerceAtLeast(0)

    override suspend fun awaitPeer(): InvitePrompt {
        delay(PEER_DELAY_MS)
        return InvitePrompt(PairingPeer(DEMO_PEER, DevicePlatform.Android), VerificationCode(FAKE_VERIFICATION_CODE))
    }

    override suspend fun confirm() = Unit

    override suspend fun awaitDone(): InvitedDevice {
        delay(DONE_DELAY_MS)
        over = true
        return InvitedDevice(register(DEMO_PEER), DEMO_PEER, DevicePlatform.Android, historyAnnounced = false)
    }

    override suspend fun cancel() {
        over = true
    }

    private companion object {
        const val DEMO_CODE = "4821-crayon-renard"
        const val DEMO_PEER = "Pixel (démo)"
        const val FAKE_VERIFICATION_CODE = "482913"
        const val VALIDITY_MS = 120_000L
        const val PEER_DELAY_MS = 3_000L
        const val DONE_DELAY_MS = 1_200L
    }
}
