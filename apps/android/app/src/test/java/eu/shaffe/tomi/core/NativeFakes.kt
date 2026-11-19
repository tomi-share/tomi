package eu.shaffe.tomi.core

import uniffi.tomi_core.AttachmentRemovalKind
import uniffi.tomi_core.CoreListener
import uniffi.tomi_core.FfiAwaitedHistory
import uniffi.tomi_core.FfiDeparture
import uniffi.tomi_core.FfiDepartureOutcome
import uniffi.tomi_core.FfiDevice
import uniffi.tomi_core.FfiDraft
import uniffi.tomi_core.FfiHistoryAnnouncement
import uniffi.tomi_core.FfiInviteOutcome
import uniffi.tomi_core.FfiInviteSession
import uniffi.tomi_core.FfiItem
import uniffi.tomi_core.FfiJoinOutcome
import uniffi.tomi_core.FfiJoinPrompt
import uniffi.tomi_core.FfiJoinSession
import uniffi.tomi_core.FfiPairingPeer
import uniffi.tomi_core.FfiRelayRevocation
import uniffi.tomi_core.FfiSyncReport
import uniffi.tomi_core.FfiTransfer
import uniffi.tomi_core.FfiVerifyPrompt
import uniffi.tomi_core.DemoConfig
import uniffi.tomi_core.DemoDepartureMode
import uniffi.tomi_core.DemoIncomingKind
import uniffi.tomi_core.DispatchKind
import uniffi.tomi_core.InvitePhaseKind
import uniffi.tomi_core.NoHandle
import uniffi.tomi_core.PairingOutcome
import uniffi.tomi_core.ProvisionData
import uniffi.tomi_core.ResumeKind
import uniffi.tomi_core.RustTomiCoreInterface
import uniffi.tomi_core.SpaceStateKind
import uniffi.tomi_core.TomiException
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/*
 * Stand-ins for the UniFFI objects of the Rust core, to test RealTomiCore on the JVM without libtomi_core.so. The generated records
 * (FfiJoinPrompt...) are plain Kotlin data classes; the generated sessions are open classes that have a "no handle" constructor for
 * exactly this use.
 */

internal fun relayError(status: Int, code: String) = TomiException.Relay(status.toUShort(), code)

internal fun pairingError(code: String) = TomiException.Crypto(code)

/** The whole native API; what a test does not set up fails loudly. */
internal open class NativeFake : RustTomiCoreInterface {
    val calls = CopyOnWriteArrayList<String>()

    @Volatile
    var paired = false

    @Volatile
    var spaceState = SpaceStateKind.ACTIVE
    var registeredListener: CoreListener? = null

    /** The relay each call was given (null: none), to check what the app passes to the core. */
    @Volatile
    var lastJoinRelay: String? = null

    @Volatile
    var lastCreateRelay: String? = null

    /** What `relay_url()` answers: the relay of the device. */
    @Volatile
    var deviceRelay: String? = null
    var joinSpaceImpl: (String, String) -> FfiJoinSession = { _, _ -> throw AssertionError("joinSpace was not expected") }
    var startInvitationImpl: () -> FfiInviteSession = { throw AssertionError("startInvitation was not expected") }
    var createSpaceImpl: (String, String) -> Unit = { _, _ -> throw AssertionError("createSpace was not expected") }
    var hasPendingImpl: () -> Boolean = { false }
    var resumeImpl: () -> ResumeKind = { ResumeKind.NOTHING }
    var itemsImpl: () -> List<FfiItem> = { emptyList() }
    var devicesImpl: () -> List<FfiDevice> = { emptyList() }
    var syncImpl: () -> FfiSyncReport = { throw AssertionError("sync was not expected") }
    var leaveImpl: () -> FfiDepartureOutcome = { throw AssertionError("leaveSpace was not expected") }
    var deleteSpaceImpl: () -> FfiDepartureOutcome = { throw AssertionError("deleteSpace was not expected") }
    var retryImpl: () -> FfiDepartureOutcome = { throw AssertionError("retryDeparture was not expected") }
    var continueImpl: () -> FfiDepartureOutcome = { FfiDepartureOutcome.NothingPending }
    var cancelImpl: () -> Boolean = { false }

    @Volatile
    var departure: FfiDeparture? = null
    var canEraseImpl: () -> Boolean = { false }
    var eraseImpl: () -> Unit = { throw AssertionError("eraseWithoutProof was not expected") }
    var enqueued: FfiDraft? = null
    var attachmentPathImpl: (String, UInt) -> String? = { _, _ -> throw AssertionError("attachmentPath was not expected") }
    var deleteAttachmentImpl: (String, UInt, Boolean) -> AttachmentRemovalKind =
        { _, _, _ -> throw AssertionError("deleteAttachment was not expected") }
    var supportsAttachmentRemovalImpl: () -> Boolean = { false }
    var itemByIdImpl: (String) -> FfiItem? = { throw AssertionError("getItem was not expected") }

    private fun record(name: String) {
        calls += name
    }

    override fun isPaired(): Boolean {
        record("isPaired")
        return paired
    }

    override fun spaceState(): SpaceStateKind = spaceState

    override fun setListener(listener: CoreListener?) {
        registeredListener = listener
    }

    override fun joinSpace(codeOrLink: String, deviceName: String, relayUrl: String?): FfiJoinSession {
        record("joinSpace")
        lastJoinRelay = relayUrl
        return joinSpaceImpl(codeOrLink, deviceName)
    }

    override fun startInvitation(): FfiInviteSession {
        record("startInvitation")
        return startInvitationImpl()
    }

    override fun createSpace(installToken: String, deviceName: String, relayUrl: String?) {
        record("createSpace")
        lastCreateRelay = relayUrl
        createSpaceImpl(installToken, deviceName)
    }

    override fun hasPendingPairing(): Boolean {
        record("hasPendingPairing")
        return hasPendingImpl()
    }

    override fun resumePairing(): ResumeKind {
        record("resumePairing")
        return resumeImpl()
    }

    override fun leaveSpace(): FfiDepartureOutcome {
        record("leaveSpace")
        return leaveImpl()
    }

    override fun deleteSpace(): FfiDepartureOutcome {
        record("deleteSpace")
        return deleteSpaceImpl()
    }

    override fun retryDeparture(): FfiDepartureOutcome {
        record("retryDeparture")
        return retryImpl()
    }

    override fun continueDeparture(): FfiDepartureOutcome {
        record("continueDeparture")
        return continueImpl()
    }

    override fun cancelDeparture(): Boolean {
        record("cancelDeparture")
        return cancelImpl()
    }

    override fun departure(): FfiDeparture? = departure

    override fun canEraseWithoutProof(): Boolean {
        record("canEraseWithoutProof")
        return canEraseImpl()
    }

    override fun eraseWithoutProof() {
        record("eraseWithoutProof")
        eraseImpl()
    }

    override fun items(): List<FfiItem> = itemsImpl()

    override fun devices(): List<FfiDevice> = devicesImpl()

    override fun sync(pushToken: String?): FfiSyncReport = syncImpl()

    override fun enqueue(draft: FfiDraft): String {
        enqueued = draft
        return draft.id.orEmpty()
    }

    override fun maybeRotate(): Boolean {
        record("maybeRotate")
        return false
    }

    override fun dispatch(id: String): DispatchKind = DispatchKind.SENT

    override fun localDevice(): FfiDevice = FfiDevice("me", "Pixel 8", "android", uniffi.tomi_core.DeviceStateKind.VERIFIED, null, true)

    // Not used by the tests.
    override fun acceptImport(transferId: String) = unexpected()

    override fun attachmentPath(itemId: String, index: UInt): String? {
        record("attachmentPath:$itemId:$index")
        return attachmentPathImpl(itemId, index)
    }

    override fun deleteAttachment(id: String, slot: UInt, everywhere: Boolean): AttachmentRemovalKind {
        record("deleteAttachment:$id:$slot:$everywhere")
        return deleteAttachmentImpl(id, slot, everywhere)
    }

    override fun supportsAttachmentRemoval(): Boolean = supportsAttachmentRemovalImpl()

    override fun awaitedHistory(): FfiAwaitedHistory? = unexpected()

    override fun cancelIfQueued(id: String): Boolean = unexpected()

    override fun cancelTransfer(transferId: String) = unexpected()

    override fun declineImport(transferId: String) = unexpected()

    override fun deleteEverywhere(id: String) = unexpected()

    override fun deleteLocal(id: String) = unexpected()

    override fun getItem(id: String): FfiItem? = itemByIdImpl(id)

    override fun markRead(id: String) = unexpected()

    override fun pin(id: String) = unexpected()

    override fun archive(id: String) = unexpected()

    override fun unarchive(id: String) = unexpected()

    override fun provision(data: ProvisionData) = unexpected()

    override fun purgePlaintextCache() = unexpected()

    override fun renameDevice(id: String, name: String) = unexpected()

    override fun revokeDevice(id: String) = unexpected()

    override fun runDoorbell() = unexpected()

    override fun startHistoryExport(deviceId: String): String = unexpected()

    override fun stopDoorbell() = unexpected()

    override fun takeResyncNotice(): Boolean = unexpected()

    // The notices about the removal of a device: harmless defaults (RealTomiCore reads them after every change), settable by a test.
    @Volatile
    var rotationStalledNow = false
    val pendingRelayRevocations = CopyOnWriteArrayList<FfiRelayRevocation>()

    override fun rotationStalled(): Boolean = rotationStalledNow

    /** Handed over once, like the core: what was returned is gone. */
    override fun takeRelayRevocations(): List<FfiRelayRevocation> =
        synchronized(pendingRelayRevocations) { pendingRelayRevocations.toList().also { pendingRelayRevocations.clear() } }

    override fun transfers(): List<FfiTransfer> = unexpected()

    override fun unpin(id: String) = unexpected()

    // The demo space of the core, entered from the pairing screen.
    @Volatile
    var demo = false
    var demoSupported = false
    var enterDemoImpl: (DemoConfig) -> Unit = { throw AssertionError("enterDemo was not expected") }
    var leaveDemoImpl: () -> Boolean = { false }

    override fun isDemo(): Boolean = demo

    override fun supportsDemoEntry(): Boolean {
        record("supportsDemoEntry")
        return demoSupported
    }

    override fun enterDemo(config: DemoConfig) {
        record("enterDemo")
        enterDemoImpl(config)
    }

    override fun leaveDemo(): Boolean {
        record("leaveDemo")
        return leaveDemoImpl()
    }

    override fun pairWithQr(payload: String): PairingOutcome = unexpected()

    override fun pairWithCode(code: String, relayUrl: String?): PairingOutcome = unexpected()

    override fun relayUrl(): String? = deviceRelay

    override fun setRelay(url: String) = unexpected()

    override fun confirmPairing(importHistory: Boolean) = unexpected()

    override fun cancelPairing() = unexpected()

    override fun simulateIncoming(kind: DemoIncomingKind) = unexpected()

    override fun simulateDeparture(mode: DemoDepartureMode) = unexpected()

    override fun updateUrl(): String? = unexpected()

    private fun unexpected(): Nothing = throw AssertionError("not expected by the test")
}

internal fun joinPrompt(
    sas: String = "482 913",
    inviter: FfiPairingPeer = FfiPairingPeer("PC-Bureau", "windows"),
    members: Int? = 3,
    history: uniffi.tomi_core.FfiHistoryOffer? = null,
    relayAuthority: String = "relay.example.org",
) = FfiJoinPrompt(relayAuthority, sas, inviter, members?.toUShort(), history)

/** A joining that the test drives. Every call is recorded in [events]; `cancel()` also releases a blocked wait. */
internal open class JoinFake(
    private val onWaitForInviter: () -> FfiJoinPrompt = { joinPrompt() },
    private val onConfirm: (Boolean) -> Unit = {},
    private val onWaitDone: () -> FfiJoinOutcome = { FfiJoinOutcome("device-1", "space-1", null) },
) : FfiJoinSession(NoHandle) {
    val events = CopyOnWriteArrayList<String>()
    val cancelled = CountDownLatch(1)
    val closed = CountDownLatch(1)
    val enteredWait = CountDownLatch(1)

    /** When set, [waitForInviter] blocks until `cancel()` and then reports `pairing_cancelled`, like the core does. */
    @Volatile
    var blockUntilCancelled = false

    override fun waitForInviter(): FfiJoinPrompt {
        events += "waitForInviter"
        enteredWait.countDown()
        if (blockUntilCancelled) {
            cancelled.await(10, TimeUnit.SECONDS)
            throw pairingError("pairing_cancelled")
        }
        return onWaitForInviter()
    }

    override fun confirm(importHistory: Boolean) {
        events += "confirm($importHistory)"
        onConfirm(importHistory)
    }

    override fun waitDone(): FfiJoinOutcome {
        events += "waitDone"
        return onWaitDone()
    }

    override fun cancel() {
        events += "cancel"
        cancelled.countDown()
    }

    override fun expiresInMs(): Long = 90_000L

    override fun close() {
        events += "close"
        closed.countDown()
        super.close()
    }
}

/** An invitation that the test drives. */
internal open class InviteFake(
    private val onWaitForPeer: () -> FfiVerifyPrompt = { FfiVerifyPrompt("482 913", FfiPairingPeer("Pixel", "android")) },
    private val onWaitDone: () -> FfiInviteOutcome = { FfiInviteOutcome("device-2", "Pixel", "android", null) },
) : FfiInviteSession(NoHandle) {
    val events = CopyOnWriteArrayList<String>()
    val cancelled = CountDownLatch(1)
    val closed = CountDownLatch(1)
    val enteredWait = CountDownLatch(1)

    @Volatile
    var blockUntilCancelled = false

    override fun code(): String? = "4821-crayon-renard"

    override fun link(): String? = "https://relay.example.org/l/pair#4821-crayon-renard"

    override fun channelId(): String = "4821"

    override fun expiresInMs(): Long = 120_000L

    override fun phase(): InvitePhaseKind = InvitePhaseKind.AWAIT_PEER

    override fun waitForPeer(): FfiVerifyPrompt {
        events += "waitForPeer"
        enteredWait.countDown()
        if (blockUntilCancelled) {
            cancelled.await(10, TimeUnit.SECONDS)
            throw pairingError("pairing_cancelled")
        }
        return onWaitForPeer()
    }

    override fun confirm() {
        events += "confirm"
    }

    override fun waitDone(): FfiInviteOutcome {
        events += "waitDone"
        return onWaitDone()
    }

    override fun cancel() {
        events += "cancel"
        cancelled.countDown()
    }

    override fun close() {
        events += "close"
        closed.countDown()
        super.close()
    }
}

internal val announcement = FfiHistoryAnnouncement("transfer-1", 812u, 340_000_000u)
