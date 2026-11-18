import Foundation
import Observation

// Mirror of apps/android/app/src/main/java/eu/shaffe/tomi/core/TomiCore.kt: same operations,
// same semantics. The implementation is the Rust core (`TomiRuntime.RustTomiCoreAdapter`, real
// or demo); the app never implements any of it.

/// What the UI observes. Updated on the main actor by the core adapter after every change.
@MainActor
@Observable
public final class CoreState {
    /// True once this device belongs to a space.
    public var isPaired = false
    public var spaceState = SpaceState.active
    /// Link to open when `spaceState` is `.updateRequired`.
    public var updateURL: URL?
    /// This device (nil before the device belongs to a space).
    public var localDevice: Device?
    /// Devices of the space, this one included, revoked ones left out.
    public var devices: [Device] = []
    /// Items sent and received, newest first, queued outgoing items included. An expired item
    /// disappears; an unreadable one stays (`ItemContent.unreadable`).
    public var items: [TomiItem] = []
    /// The first reading after opening is done (empty states can be shown).
    public var loaded = false
    /// The departure pending on this device, if any.
    public var departure: Departure?
    /// A call of the departure runs in the core (an attempt, even a cancelled one, or an erasure).
    public var departureRunning = false

    public init() {}
}

/// Boundary between the UI and the Tomi core. Every call may throw `TomiCoreError`; the UI treats
/// any error as the failure of that operation, never as a reason to crash. Calls never block the
/// main thread.
@MainActor
public protocol TomiCore: AnyObject {
    var state: CoreState { get }
    /// A demo core: no network, simulated pairing, `simulateIncoming` available.
    var isDemo: Bool { get }

    func getItem(id: String) async throws -> TomiItem?

    /// Fetches what is waiting on the relay. Called at start-up, on wake-ups and when the app comes
    /// back. `pushToken` is the current push token when it is new or changed. Does nothing when
    /// the device is not paired.
    func sync(pushToken: String?) async throws

    /// Stores the draft in the core's persistent outbox; it shows in `items` as queued. Attachment
    /// files are read before this returns. Nothing is sent yet: the app calls `dispatch` after the
    /// 3 s undo window.
    func enqueue(_ draft: OutgoingDraft) async throws
    /// Tries to push a queued item to the relay. Safe to call several times.
    func dispatch(id: String) async throws -> DispatchResult
    /// The "Undo" of the send banner: erases the item if, and only if, it has not left the
    /// device yet (atomic with `dispatch`). Never deletes an item already sent.
    func cancelIfQueued(id: String) async throws -> Bool

    /// Removes the item from this device only.
    func deleteLocal(id: String) async throws
    /// Signed tombstone: relay purge and erasure on every device (best effort).
    func deleteEverywhere(id: String) async throws
    /// Marks the item read on all devices; consumes an "after first read" item everywhere. Never
    /// consumes an unreadable item.
    func markRead(id: String) async throws
    /// Path of the decrypted attachment, downloading it if needed. Nil if unavailable or removed. `slot` is `Attachment.slot`
    /// (stable position in the payload), NOT the position in `attachments`: they differ once an attachment was removed.
    func attachmentPath(itemId: String, slot: Int) async throws -> String?
    /// Removes ONE attachment of an item (`slot` is `Attachment.slot`, never the position), from this device only or
    /// `everywhere` (signed declaration to the relay and the devices). The others stay; when nothing else is left in the item
    /// the whole item is deleted with the same scope (`.itemDeleted`). Idempotent. Throws while the item is still queued
    /// (both scopes): the UI does not offer it then. No biometry, like deleting an item.
    func deleteAttachment(itemId: String, slot: Int, everywhere: Bool) async throws -> AttachmentRemoval
    /// The relay takes "remove everywhere" (capability heard at the last session); "from this device" is always possible.
    func supportsAttachmentRemoval() async -> Bool

    func renameDevice(id: String, name: String) async throws
    /// Removes a device from the space and rotates the space key. The UI asks for strong biometrics first.
    func revokeDevice(id: String) async throws

    /// Pins on all devices (spec/pin-contract.md). Idempotent, unknown id ignored, immediate
    /// local effect; the network never fails it. Pinning an archived item unarchives it.
    func pin(id: String) async throws
    func unpin(id: String) async throws
    /// Out of "Recent", still readable and searchable. Never marks read, never downloads, never
    /// changes an expiry; unpins a pinned item. Local to this device for now.
    func archive(id: String) async throws
    func unarchive(id: String) async throws

    /// The relay this device talks to now (`https://host[:port]`), nil when it has none. Read it while no pairing or
    /// creation is under way: during one the core points at the candidate relay.
    func currentRelay() async -> String?

    /// Joining the space with an invitation code (typed, or read from a scanned QR code or an opened link) and the
    /// relay that goes with it: the one of the link or of the "Relais" field, nil when the device already has its
    /// relay. **A relay that was read is not trusted**: the result carries its authority for the screen to show in full
    /// (`PairingPrompt.relayAuthority`). Success means the device has NOT joined yet: the user compares the codes, then
    /// `confirmPairing` or `cancelPairing`. Nothing is kept before it worked.
    func pairWithCode(code: String, relayUrl: String?) async throws -> PairingResult
    /// The codes match: deposits the confirmation, then waits for the other device to confirm too. Throws a
    /// `PairingFailure`. Without a pairing in progress, does nothing.
    func confirmPairing(importHistory: Bool) async throws
    /// Idempotent; ends a `confirmPairing` still waiting (it then throws `.cancelled`). Never throws.
    func cancelPairing() async
    // MARK: Departure (Android `TomiCore` leaveSpace...). Every call goes to its end even when its caller goes away;
    // none throws: the answers of the relay are in the result, and a failure is `.failed`.

    /// "Se déconnecter": writes the departure, then one attempt (minutes at most on a bad network).
    func leaveSpace() async -> DepartureResult
    /// "Supprimer l'espace", the last device (after `.refused(.lastDevice)`), behind strong biometrics.
    func deleteSpace() async -> DepartureResult
    /// "Réessayer": a gesture, the request is sent again.
    func retryDeparture() async -> DepartureResult
    /// Automatic (start-up, back-off, network back): within 15 min of the gesture sends again, later only verifies.
    func continueDeparture() async -> DepartureResult
    /// "Annuler": immediate, never waits for the attempt in flight. False when nothing was pending (or erasing).
    func cancelDeparture() async -> Bool
    /// "Effacer quand même" is allowed right now (a recent "not found" or "relay too old" verdict).
    func canEraseWithoutProof() async -> Bool
    /// Erases this device without a proof of the relay.
    func eraseWithoutProof() async -> EraseResult
    /// Demo core only: how the relay answers the next departures. Does nothing on a real core.
    func simulateDeparture(_ scenario: DemoDepartureScenario) async

    /// Adds a device to the space from this one: opens a channel at the relay and returns the session whose code and
    /// link the screen shows. The caller owns it (`cancel` when the screen closes before the end). Throws a
    /// `PairingFailure` (`.spaceFull`, `.anotherPairingOpen`, `.network`...).
    func startInvitation() async throws -> any InviteSession
    /// Creates the space from this device, its first member, with the install token of the relay (used for this
    /// call only, never stored). `relayUrl` is the relay of the creation link or of the "Relais" field, nil when the
    /// device already has its relay; it is not trusted, the screen shows it in full before this call, and it is kept
    /// only when the space is created. Throws a `PairingFailure`.
    func createSpace(installToken: String, deviceName: String, relayUrl: String?) async throws
    /// Settles a pairing a stop interrupted (call at start-up, when not paired).
    func resumePairing() async -> ResumeOutcome
    /// The core's own rule for a device name (1 to 64 characters, no blank at the ends, no control character),
    /// after trimming.
    func isValidDeviceName(_ name: String) -> Bool

    /// Demo core only: another device sends something now.
    func simulateIncoming(_ kind: SimulatedIncoming) async throws

    /// Erases every decrypted attachment (call when the app leaves the screen).
    func purgePlaintextCache() async throws
    /// The app is on screen: keep this device up to date (doorbell).
    func enterForeground()
    /// The app left the screen: stop the doorbell, erase the decrypted files.
    func enterBackground()
}
