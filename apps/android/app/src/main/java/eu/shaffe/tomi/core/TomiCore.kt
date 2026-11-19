package eu.shaffe.tomi.core

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.emptyFlow

/**
 * Boundary between the Android UI and the Tomi core.
 *
 * The implementation is the Rust `tomi-core` crate exposed through UniFFI ([RealTomiCore]); the tests use an in-memory fake. The core
 * owns everything sensitive: key storage, encryption, the wire protocol, pairing, local storage and relay synchronisation. The Android
 * layer must never implement any of that; it only calls this interface.
 *
 * Contract notes for the implementation:
 * - Item and device ids are opaque strings. The caller generates the id of an outgoing item so that retries are idempotent.
 * - [items] is newest first and includes outgoing items that are still queued (with [DeliveryState.Queued]).
 * - Items past their `expiresAt` must disappear from [items] (local erasure is the core's job).
 * - A decryption failure or an unknown content version must never remove an item (see DEV.md invariant 7): the
 *   item stays in [items] with [ItemContent.Unreadable], so the UI can say "update needed / unreadable".
 * - Suspending calls may throw [TomiCoreException] (or a subclass). The UI treats any non-cancellation
 *   exception as a failure of the operation and never as a reason to crash.
 * - [attachmentUri] is a `content://` URI of [AttachmentProvider] (not exported, read grants only), which
 *   `PackageManagerOwnAuthorityCheck` recognises as Tomi's own, so it is never re-shared into Tomi.
 */
interface TomiCore {
    /** True once this device belongs to a space. */
    val isPaired: StateFlow<Boolean>

    /** State of the space (revoked device, protocol too recent for this app). */
    val spaceState: StateFlow<SpaceState>

    /**
     * True while the removal of a device is NOT finished: the key rotation that must follow a revocation keeps failing (`device_set_mismatch`)
     * and the core retries it every hour. The removed device may still hold the current space key meanwhile: the UI shows a persistent,
     * non-alarming banner, and nothing when false. Persisted by the core; re-read after a [sync] and on every change the core announces.
     * Defaults to false for a core without a rotation (the tests' fake sets it).
     */
    val rotationStalled: StateFlow<Boolean> get() = NOT_STALLED

    /**
     * Devices that the relay alone removed (its administrator, or a departure it reports), each delivered ONCE: the core hands each one over
     * a single time and this flow buffers what nobody collects yet. The UI shows a one-time, non-blocking notice for each. Defaults to none.
     */
    val relayRevocations: Flow<RelayRevocation> get() = emptyFlow()

    /** This device (never null once the core is initialised). */
    val localDevice: StateFlow<Device>

    /** All devices of the space, this one included. */
    val devices: Flow<List<Device>>

    /** Items sent and received, newest first. */
    val items: Flow<List<TomiItem>>

    suspend fun getItem(id: String): TomiItem?

    /**
     * Fetches what is waiting on the relay. Called when an FCM data message wakes the app, and at start-up.
     * [pushToken] is the current FCM registration token when it is new or changed: the core registers it with
     * the relay. Safe to call often; does nothing when the device is not paired.
     */
    suspend fun sync(pushToken: String? = null)

    /**
     * Opens the real-time channel to the relay (the "doorbell"): every item sent by another device is fetched as soon as the relay
     * announces it, without waiting for the next [sync]. Meant to run while the app is in the foreground; idempotent. Does nothing
     * for a core without a relay (tests), which is why it has a default.
     */
    fun startLiveSync() {}

    /** Closes the channel opened by [startLiveSync]. Idempotent. */
    fun stopLiveSync() {}

    /**
     * Stores [draft] locally in the core's own persistent outbox and makes it appear in [items] as queued.
     * Attachment files are read (and may then be deleted by the caller) before this returns.
     * Nothing is sent to the relay yet: the app schedules [dispatch], after a 5 s undo window.
     */
    suspend fun enqueue(draft: OutgoingDraft)

    /** Tries to push a queued item to the relay. Safe to call several times for the same id. */
    suspend fun dispatch(id: String): DispatchResult

    /**
     * Atomically cancels the send of an item that has not left the device yet: if, and only if, every delivery
     * of the item is still [DeliveryState.Queued], the item is erased locally and `true` is returned.
     * Returns `false` when it is too late (the relay already accepted it, or it failed) or when the id is unknown;
     * the item is then left untouched. Atomic with [dispatch]: a [dispatch] that is already pushing the item
     * must either win (this returns `false`) or find the item gone (it returns [DispatchResult.NotFound]).
     * This is what the "Undo" of the send banner calls: it must never delete an item that was already sent.
     */
    suspend fun cancelIfQueued(id: String): Boolean

    /**
     * Removes the item from this device only. For an item that is still queued this cancels the send;
     * prefer [cancelIfQueued] when the caller needs to know whether the item had already left.
     */
    suspend fun deleteLocal(id: String)

    /** Sends a signed tombstone: relay purge and erasure on every device (best effort). */
    suspend fun deleteEverywhere(id: String)

    /**
     * Marks the item read on all devices. For an item with [ReadPolicy.AfterFirstRead] the core must also
     * erase its content and drop it from [items] (the UI keeps what is on screen until it is hidden), AND it must
     * publish a signed tombstone "everywhere" (like [deleteEverywhere]) so that the other devices and the relay
     * erase their copy too: reading the secret once on one device consumes it for the whole space.
     * An [ItemContent.Unreadable] item must not be consumed by this call.
     */
    suspend fun markRead(id: String)

    /**
     * Erases the decrypted copies that [attachmentUri] made (AGENTS.md, rule 3: the clear copy lives until the app goes to the
     * background). May throw if a file could not be removed; the rest is erased anyway.
     */
    suspend fun purgePlaintextCache() {}

    /**
     * Local URI (content://) of an attachment, downloading/decrypting it if needed. Null if unavailable (or removed).
     * [slot] is [Attachment.slot], NEVER the position in the list: they differ once an attachment was removed from the item.
     */
    suspend fun attachmentUri(itemId: String, slot: Int): String?

    /**
     * Removes ONE attachment of the item (the others stay), identified by its [Attachment.slot]. [everywhere] false: from this device
     * only; true: also on the other devices (a signed statement through the relay; the UI offers it only when
     * [supportsAttachmentRemoval]). Idempotent: an unknown or already removed slot does nothing. When nothing is left in the item
     * (no attachment, no text, no link) the core deletes the whole item with the same scope and answers
     * [AttachmentRemoval.ItemDeleted]. Fails ([TomiCoreException]) for "everywhere", and for the local removal too, while the item is
     * still queued for sending. No biometrics (like deleting an item).
     */
    suspend fun deleteAttachment(itemId: String, slot: Int, everywhere: Boolean): AttachmentRemoval

    /** True when the relay accepts "remove everywhere"; false until a session answered. The UI hides that scope otherwise. */
    suspend fun supportsAttachmentRemoval(): Boolean

    suspend fun renameDevice(id: String, name: String)

    /** Removes a device from the space and rotates the space key. The UI asks for strong biometrics first. */
    suspend fun revokeDevice(id: String)

    // --- Leaving the space (spec/api-v1.md 7.9) -------------------------------------------------------------------------------------
    //
    // The core erases this device only on a positive proof of the relay. Every call below that talks to the relay may last MINUTES on a
    // bad network (up to about 9 minutes for a departure, 18 for a deletion, crates/core/src/runtime/leave.rs "Duration of an attempt"):
    // they run off the main thread, to their end even when the caller goes away, and never throw except CancellationException.

    /** The departure pending on this device, null when none. Re-read on every change of the core, and after every call below. */
    val departure: StateFlow<Departure?>

    /**
     * True while a departure attempt (or an erasure without proof) runs in the core. A cancelled attempt keeps running to its end (it
     * erases nothing then): this stays true until it is over.
     */
    val departureRunning: StateFlow<Boolean>

    /** "Se déconnecter": writes the departure marker, then one attempt. Afterwards [departure] says what is pending, if anything. */
    suspend fun leaveSpace(): DepartureResult

    /** "Supprimer l'espace", only after [RefusalReason.LastDevice], an irreversible confirmation and strong biometrics. Same contract. */
    suspend fun deleteSpace(): DepartureResult

    /** "Réessayer" (a gesture: the request is sent again). */
    suspend fun retryDeparture(): DepartureResult

    /** Automatic: start-up, back-off, network back. Resends the request only within 15 min of the last gesture, verifies only later. */
    suspend fun continueDeparture(): DepartureResult

    /** "Annuler". Immediate, never erases, does not wait for an attempt in progress. True if a departure was cancelled. */
    suspend fun cancelDeparture(): Boolean

    /** "Effacer quand même" is allowed by the core right now (an answer "not found" or "relay too old" less than 10 minutes old). */
    suspend fun canEraseWithoutProof(): Boolean

    /**
     * "Effacer quand même": erases everything this device holds WITHOUT any proof of the relay (the device may stay registered there as a
     * member nobody can reach). The UI asks for strong biometrics first, every time, behind an honest confirmation; the core cannot check
     * that it did. [EraseResult.NotAllowed] when the core no longer allows it.
     */
    suspend fun eraseWithoutProof(): EraseResult

    /**
     * Pins the item on all devices. Idempotent (nothing is sent when it is already pinned), no effect for an unknown
     * id, immediate local effect: [items] is re-emitted with `pinned` / `pinnedAt` up to date. Only local storage errors
     * are thrown ([TomiCoreException]); a network error is never an error of this call.
     * Contract: spec/pin-contract.md. The app never handles the signed statement or the convergence timestamp.
     * Pinning an archived item unarchives it (see [archive]).
     */
    suspend fun pin(id: String)

    /** Unpins the item on all devices. Same semantics as [pin]. */
    suspend fun unpin(id: String)

    /**
     * Archives the item on all devices: it leaves the "Recent" section and the other filters, and shows under the
     * "Archives" filter. Nothing is erased, and the item stays searchable. Same semantics as [pin]: idempotent, no effect
     * for an unknown id, immediate local effect ([items] is re-emitted with `archived` / `archivedAt` up to date), and only
     * local storage errors are thrown ([TomiCoreException]); a network error is never an error of this call.
     *
     * Archiving NEVER marks the item read (an "after first read" secret would be consumed), never sends `ack` / `read` /
     * `release`, never downloads anything and never changes an expiry.
     * Pin and archive exclude each other: archiving a pinned item unpins it (and [pin] unarchives, see [pin]).
     *
     * Contract: spec/archive-contract.md (state synchronised between devices, same
     * mechanism as the pin). The real core implements it ([RealTomiCore]); the fake core mimics it for the tests.
     */
    suspend fun archive(id: String)

    /** Brings an archived item back to its place in "Recent". Same semantics as [archive] (idempotent, unknown id ignored). */
    suspend fun unarchive(id: String)

    // --- Pairing (spec/pairing-v1.md) ---------------------------------------------------------------------------------------

    /**
     * Joins a space with the link of a scanned QR code or of an App Link. A success is [PairingResult.AwaitingVerification]: the device
     * only joins the space after [confirmPairing]. Suspends for the network and for the other device (seconds). The link never starts
     * anything by itself: the caller calls this when the user pressed "Rejoindre". Cancelling the coroutine abandons the pairing.
     */
    suspend fun pairWithQr(payload: String): PairingResult

    /**
     * Joins a space with a short code typed by the user (e.g. `7-crayon-renard`). Same result as [pairWithQr]. [relayUrl] is the relay typed
     * next to the code (`https://host[:port]`, see [RelayAddress.canonical]), null when the device has its own. A relay that was typed is
     * not trusted: it is shown in full on the verification screen ([PairingResult.AwaitingVerification.relayAuthority]) and the core keeps
     * it only once the pairing succeeded. [PairingFailure.RelayNotConfigured] when there is no relay at all.
     */
    suspend fun pairWithCode(code: String, relayUrl: String? = null): PairingResult

    /**
     * The user compared the 6-digit codes and they are identical. Finishes the pairing; [importHistory] is the single yes/no answer to
     * "get the history of the space?" (secrets and pins included, no per-type choice), ignored when nothing was offered. Suspends until
     * the other device is confirmed too and this device belongs to the space (minutes at most): the space key only arrives once BOTH
     * people confirmed. Failures are [PairingException]. Without a pending pairing it does nothing.
     */
    suspend fun confirmPairing(importHistory: Boolean)

    /** The codes differ (or the user left the pairing): forgets the pending pairing, nothing is joined. Also unblocks a pending wait. */
    suspend fun cancelPairing()

    /**
     * Adds a device to the space from this one: opens a channel at the relay and returns the session whose code and link the screen
     * shows. The caller owns the session: [InviteSession.cancel] when the screen closes before the end. Failures are [PairingException]
     * ([PairingFailure.SpaceFull], [PairingFailure.AnotherPairingOpen], [PairingFailure.Network]...).
     */
    suspend fun startInvitation(): InviteSession

    /**
     * Creates the space from this device, the first one: [installToken] is the token the relay prints at its start (typed or pasted,
     * used for this call only, never kept), [deviceName] the name of this device in the space (see [isValidDeviceName]). Failures are
     * [PairingException]: [PairingFailure.InstallTokenInvalid] (the token was not consumed: retry), [PairingFailure.SpaceExists]
     * (offer "Rejoindre"), [PairingFailure.Network] (retry).
     *
     * [relayUrl] is the relay of the creation link or of the "Relais" field (`https://host[:port]`), null to use the relay of the device. It is
     * not trusted: the screen shows it in full before "Créer", and the core keeps it only once the space is created.
     * [PairingFailure.RelayNotConfigured] when there is no relay at all (nothing is sent).
     */
    suspend fun createSpace(installToken: SensitiveText, deviceName: String, relayUrl: String? = null)

    /**
     * Settles a pairing or a space creation that a stop interrupted (spec 11.4, 10.2). Call it at start-up. It asks the relay whether it
     * registered this device: if so the device is provisioned, otherwise the pending batch is erased. Retries by itself, with a back-off,
     * while the relay cannot be reached ([ResumeOutcome.Unreachable] when it gave up). Does nothing when already paired or when nothing
     * is pending.
     */
    suspend fun resumePairing(): ResumeOutcome

    // --- Demo (the core's demo space, entered from the pairing screen) ------------------------------------------------------------

    /**
     * True while the core runs its demo space: fictitious data, nothing is sent. [isPaired] is true meanwhile. Defaults to a core
     * without a demo (the tests' fake).
     */
    val isDemo: StateFlow<Boolean> get() = NOT_DEMO

    /** Whether this core can open its demo space now (built with it, not in a space, no pairing pending in the core). */
    fun supportsDemoEntry(): Boolean = false

    /**
     * Opens the demo space (blocks a few seconds and writes its files: runs off the main thread). Throws [TomiCoreException] when the
     * core refuses (in a space, pairing pending) or has no demo. A pairing pending in this class is abandoned first.
     */
    suspend fun enterDemo() {}

    /** Leaves the demo space and erases its files: back to the real, unpaired core. Does nothing outside the demo. */
    suspend fun leaveDemo() {}

    /**
     * The relay this device talks to now (`https://host[:port]`): the one of its space once paired, otherwise the build default (a
     * development build) or the one a previous attempt kept; null when there is none (a release build: the relay then comes from a link,
     * a QR code or the "Relais" field).
     */
    suspend fun relayUrl(): String?

    /**
     * Whether [name] is a valid device name, by the exact rule of the core: blanks at either end are trimmed first (the core trims the
     * name it is given too), then 1 to 64 characters and no control character. Callers pass the trimmed name on.
     */
    fun isValidDeviceName(name: String): Boolean
}

/** [TomiCore.rotationStalled] of a core without a rotation. */
private val NOT_STALLED: StateFlow<Boolean> = MutableStateFlow(false)

/** [TomiCore.isDemo] of a core without a demo. */
private val NOT_DEMO: StateFlow<Boolean> = MutableStateFlow(false)
