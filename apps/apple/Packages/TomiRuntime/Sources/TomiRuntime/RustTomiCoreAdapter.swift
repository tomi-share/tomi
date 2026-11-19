import Foundation
import TomiCore
import TomiKit
#if canImport(UIKit)
import UIKit
#endif

/// Serial queue for core calls: every method of `RustTomiCore` blocks (SQLite, HTTPS), so none
/// may run on the main thread, and running them one at a time keeps the order of the UI.
final class CoreQueue: @unchecked Sendable {
    private let queue: DispatchQueue

    init(label: String, qos: DispatchQoS) {
        queue = DispatchQueue(label: label, qos: qos)
    }

    func run<T: Sendable>(_ body: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    continuation.resume(returning: try body())
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }
}

/// `CoreListener` of the core: called on a core thread after every change; it only asks the
/// adapter to re-read (never a core call here, see `crates/core/src/ffi.rs`).
private final class ChangeListener: CoreListener, @unchecked Sendable {
    private let onChange: @Sendable () -> Void

    init(onChange: @escaping @Sendable () -> Void) {
        self.onChange = onChange
    }

    func onChanged() {
        onChange()
    }
}

/// What the adapter reads after a change, off the main thread.
private struct Snapshot: Sendable {
    var isDemo: Bool
    var isPaired: Bool
    var spaceState: SpaceState
    var updateURL: URL?
    var localDevice: Device?
    var devices: [Device]?
    var items: [TomiItem]?
    var departure: Departure?
    /// `rotation_stalled()`; false when not paired or in the demo.
    var rotationStalled = false
    /// What `take_relay_revocations()` handed over in this read (each entry comes out once).
    var relayRevocations: [RelayRevocation] = []
}

/// `TomiKit.TomiCore` over the Rust core, real or demo.
///
/// - Two serial queues: `local` for reads and local-first changes (pin, archive, mark read,
///   delete...), `network` for the calls that wait on the relay (sync, dispatch, downloads,
///   revocation, pairing). A slow sync never freezes the feed. The engine is thread-safe.
/// - The state is re-read after every change the core reports (coalesced) and published on the
///   main actor.
/// - While the app is on screen, a real core keeps its doorbell (WebSocket) open on a thread of
///   its own. When the core closes it by itself (device revoked, space reset, replaced
///   connection: `api-v1.md` §5) it stays closed until the app comes back to the foreground.
@MainActor
public final class RustTomiCoreAdapter: TomiKit.TomiCore {
    public let state = CoreState()
    public var isDemo: Bool { state.isDemo }
    /// The demo can be entered: a real core built with the demo, read its state, in no space.
    public var canEnterDemo: Bool { demoEntry != nil && supportsDemoEntry && state.loaded && !state.isPaired && !state.isDemo }
    /// The demo on screen was entered from the real core.
    public var canLeaveDemo: Bool { supportsDemoEntry && state.isDemo }

    private let core: RustTomiCore
    private let local = CoreQueue(label: "eu.shaffe.tomi.core.local", qos: .userInitiated)
    private let network = CoreQueue(label: "eu.shaffe.tomi.core.network", qos: .utility)
    /// Departure: an attempt blocks for minutes on a bad network. `cancelDeparture` and `canEraseWithoutProof` go
    /// through `local`, never behind it.
    private let departureQueue = CoreQueue(label: "eu.shaffe.tomi.core.departure", qos: .userInitiated)
    /// Departure calls running (`CoreState.departureRunning`).
    private var departureCalls = 0 {
        didSet { state.departureRunning = departureCalls > 0 }
    }
    /// Pairing and creation of the space: calls that block for seconds (the other device), kept apart so that a
    /// cancel (on `local`) is never queued behind the wait it must end.
    private let pairing = CoreQueue(label: "eu.shaffe.tomi.core.pairing", qos: .userInitiated)
    /// Erasing decrypted files never waits behind the other calls.
    private let purgeQueue = CoreQueue(label: "eu.shaffe.tomi.core.purge", qos: .userInitiated)
    private var listener: ChangeListener?
    /// The joining of the real core (`join_space`), one at a time. Its cancel goes through `joinControl`, never behind
    /// the wait it ends.
    private var join: FfiJoinSession?
    /// Moves at every cancel, even while the claim runs (no session yet): a claim that returns after it is closed.
    private var joinGeneration = 0
    private let joinControl = CoreQueue(label: "eu.shaffe.tomi.core.join.cancel", qos: .userInitiated)
    /// The name this device asks for when it joins a space (the core's rule decides, `DeviceNames.choose`).
    private let deviceName: String?
    /// What `enterDemo` opens the demo with (nil: this adapter never enters it).
    private let demoEntry: DemoEntry?
    /// The Rust core can enter the demo (built with it, and not itself a demo core).
    private let supportsDemoEntry: Bool
    private var refreshPending = false
    private var wantsDoorbell = false
    private var doorbellRunning = false
    /// The current doorbell is being stopped by the app (not closed by the core).
    private var doorbellStopRequested = false

    public init(core: RustTomiCore, deviceName: String? = nil, demoEntry: DemoEntry? = nil) {
        self.core = core
        self.deviceName = deviceName
        self.demoEntry = demoEntry
        self.supportsDemoEntry = core.supportsDemoEntry()
        state.isDemo = core.isDemo()
        let listener = ChangeListener { [weak self] in
            Task { @MainActor in self?.scheduleRefresh() }
        }
        core.setListener(listener: listener)
        self.listener = listener
        scheduleRefresh(delay: .zero)
    }

    deinit {
        // Safety net when `shutdown` was not called: never leave a doorbell open.
        core.stopDoorbell()
        core.setListener(listener: nil)
    }

    /// Unregisters the listener (the core keeps a reference to it otherwise) and stops the doorbell.
    public func shutdown() {
        wantsDoorbell = false
        stopDoorbell()
        core.setListener(listener: nil)
        listener = nil
    }

    // MARK: State

    private func scheduleRefresh(delay: Duration = .milliseconds(80)) {
        guard !refreshPending else { return }
        refreshPending = true
        Task { @MainActor in
            if delay > .zero { try? await Task.sleep(for: delay) }
            refreshPending = false
            await refresh()
        }
    }

    /// Re-reads everything the UI shows, now.
    public func refresh() async {
        let core = self.core
        // What did not change keeps its storage (see `ItemReuse`): the merge runs off the main thread, with the read.
        let previous = state.items
        guard let snapshot = try? await local.run({ Self.read(core, previous: previous) }) else { return }
        state.isDemo = snapshot.isDemo
        state.isPaired = snapshot.isPaired
        state.spaceState = snapshot.spaceState
        state.updateURL = snapshot.updateURL
        state.localDevice = snapshot.localDevice
        // Only a change is published: the notification of a new device follows the list.
        if let devices = snapshot.devices, devices != state.devices { state.devices = devices }
        // Only a change is published: every screen that reads the feed is drawn again when it is written. Comparing is
        // cheap here, an unchanged item being the very storage of the one on screen.
        if let items = snapshot.items, items != state.items { state.items = items }
        state.departure = snapshot.departure
        if snapshot.rotationStalled != state.rotationStalled { state.rotationStalled = snapshot.rotationStalled }
        state.addRelayRevocations(snapshot.relayRevocations)
        state.loaded = true
        startDoorbellIfNeeded()
    }

    private nonisolated static func read(_ core: RustTomiCore, previous: [TomiItem]) -> Snapshot {
        let paired = core.isPaired()
        var snapshot = Snapshot(
            isDemo: core.isDemo(),
            isPaired: paired,
            spaceState: CoreMapping.spaceState(core.spaceState()),
            updateURL: core.updateUrl().flatMap(URL.init(string:)),
            localDevice: nil,
            devices: paired ? nil : [],
            items: paired ? nil : [],
            departure: core.departure().map(CoreMapping.departure)
        )
        guard paired else { return snapshot }
        // This device stays known even once revoked (its own items must still read as sent).
        snapshot.localDevice = (try? core.localDevice()).map { CoreMapping.device($0, keepRevoked: true) } ?? nil
        snapshot.devices = (try? core.devices())?.compactMap { CoreMapping.device($0) }
        snapshot.items = (try? core.items()).map { ItemReuse.reuseUnchanged(previous: previous, fresh: $0.map(CoreMapping.item)) }
        // Read on the core's change notification (a sync that records a removal or a failed rotation reports one), never
        // polled. The demo has no relay. A failed read loses nothing: the core keeps the notices until they are taken.
        if !snapshot.isDemo {
            snapshot.rotationStalled = core.rotationStalled()
            snapshot.relayRevocations = ((try? core.takeRelayRevocations()) ?? []).map(CoreMapping.relayRevocation)
        }
        return snapshot
    }

    // MARK: Calls

    /// Runs `body` on `queue`; core errors become `TomiCoreError`.
    private func call<T: Sendable>(
        on queue: CoreQueue,
        _ body: @escaping @Sendable (RustTomiCore) throws -> T
    ) async throws -> T {
        let core = self.core
        do {
            return try await queue.run { try body(core) }
        } catch let error as TomiError {
            throw CoreMapping.error(error)
        }
    }

    public func getItem(id: String) async throws -> TomiItem? {
        try await call(on: local) { try $0.getItem(id: id).map(CoreMapping.item) }
    }

    /// Without a space the core does nothing (no error), so it is always safe to call.
    public func sync(pushToken: String?) async throws {
        _ = try await call(on: network) { core -> Bool in
            guard core.isPaired() else { return false }
            _ = try core.sync(pushToken: pushToken)
            return true
        }
    }

    /// Too large for the relay with its thumbnails (a long rich text and many images): sent again without them, as
    /// Android (`RealTomiCore.enqueue`) and Windows (`fit_payload`) do.
    public func enqueue(_ draft: OutgoingDraft) async throws {
        let ffi = CoreMapping.draft(draft)
        do {
            _ = try await call(on: local) { try $0.enqueue(draft: ffi) }
        } catch let error as TomiCoreError where error.kind == .invalidInput && ffi.attachments.contains(where: { $0.thumbnail != nil }) {
            var bare = draft
            bare.attachments = draft.attachments.map { attachment in
                var without = attachment
                without.thumbnail = nil
                without.thumbnailMime = nil
                return without
            }
            let retry = CoreMapping.draft(bare)
            _ = try await call(on: local) { try $0.enqueue(draft: retry) }
        }
    }

    public func dispatch(id: String) async throws -> DispatchResult {
        try await call(on: network) { CoreMapping.dispatch(try $0.dispatch(id: id)) }
    }

    /// Atomic with `dispatch` inside the core, whatever the queue.
    public func cancelIfQueued(id: String) async throws -> Bool {
        try await call(on: local) { try $0.cancelIfQueued(id: id) }
    }

    public func deleteLocal(id: String) async throws {
        try await call(on: local) { try $0.deleteLocal(id: id) }
    }

    /// Local first: the tombstone leaves through the outbox.
    public func deleteEverywhere(id: String) async throws {
        try await call(on: local) { try $0.deleteEverywhere(id: id) }
    }

    public func markRead(id: String) async throws {
        try await call(on: local) { try $0.markRead(id: id) }
    }

    /// May download the file first.
    /// `slot` is `Attachment.slot` (position in the payload, stable), never the position in the displayed list.
    public func attachmentPath(itemId: String, slot: Int) async throws -> String? {
        guard let slot = UInt32(exactly: slot) else { return nil }
        return try await call(on: network) { try $0.attachmentPath(itemId: itemId, index: slot) }
    }

    /// Local first (no network): "everywhere" is signed and leaves through the outbox. A slot that does not fit the core's
    /// index never designates an attachment: nothing to do.
    public func deleteAttachment(itemId: String, slot: Int, everywhere: Bool) async throws -> AttachmentRemoval {
        guard let slot = UInt32(exactly: slot) else { return .attachmentRemoved }
        return try await call(on: local) {
            CoreMapping.removal(try $0.deleteAttachment(id: itemId, slot: slot, everywhere: everywhere))
        }
    }

    public func supportsAttachmentRemoval() async -> Bool {
        (try? await call(on: local) { $0.supportsAttachmentRemoval() }) ?? false
    }

    public func renameDevice(id: String, name: String) async throws {
        try await call(on: network) { try $0.renameDevice(id: id, name: name) }
    }

    public func revokeDevice(id: String) async throws {
        try await call(on: network) { try $0.revokeDevice(id: id) }
    }

    public func pin(id: String) async throws {
        try await call(on: local) { try $0.pin(id: id) }
    }

    public func unpin(id: String) async throws {
        try await call(on: local) { try $0.unpin(id: id) }
    }

    public func archive(id: String) async throws {
        try await call(on: local) { try $0.archive(id: id) }
    }

    public func unarchive(id: String) async throws {
        try await call(on: local) { try $0.unarchive(id: id) }
    }

    // MARK: Departure (Android `RealTomiCore` leaveSpace...)
    //
    // Every call goes to its end even if its caller goes away (the continuation is resumed by the queue), and the state
    // is read again afterwards, whatever happened. The answers of the relay are in the outcome; a core error is
    // `.failed` (nothing was erased).

    public func leaveSpace() async -> DepartureResult { await departureCall { try $0.leaveSpace() } }
    public func deleteSpace() async -> DepartureResult { await departureCall { try $0.deleteSpace() } }
    public func retryDeparture() async -> DepartureResult { await departureCall { try $0.retryDeparture() } }
    public func continueDeparture() async -> DepartureResult { await departureCall { try $0.continueDeparture() } }

    public func cancelDeparture() async -> Bool {
        let cancelled = (try? await call(on: local) { try $0.cancelDeparture() }) ?? false
        await refresh()
        return cancelled
    }

    public func canEraseWithoutProof() async -> Bool {
        (try? await call(on: local) { $0.canEraseWithoutProof() }) ?? false
    }

    public func eraseWithoutProof() async -> EraseResult {
        departureCalls += 1
        defer { departureCalls -= 1 }
        let result: EraseResult
        do {
            try await call(on: departureQueue) { try $0.eraseWithoutProof() }
            result = .erased
        } catch let error as TomiCoreError where error.kind == .invalidInput {
            result = .notAllowed
        } catch {
            result = .failed
        }
        await afterDeparture()
        return result
    }

    /// Demo core only: how the relay answers the next departures (debug menu).
    public func simulateDeparture(_ scenario: DemoDepartureScenario) async {
        let mode: DemoDepartureMode = switch scenario {
        case .normal: .normal
        case .offline: .offline
        case .serverError: .serverError
        case .lastDevice: .lastDevice
        case .reset: .reset
        case .oldRelay: .oldRelay
        }
        _ = try? await call(on: local) { try $0.simulateDeparture(mode: mode) }
    }

    private func departureCall(_ body: @escaping @Sendable (RustTomiCore) throws -> FfiDepartureOutcome) async -> DepartureResult {
        departureCalls += 1
        // The marker is written before the first network call: shown while the attempt runs.
        scheduleRefresh(delay: .milliseconds(150))
        defer { departureCalls -= 1 }
        let result: DepartureResult
        do {
            result = CoreMapping.departureResult(try await call(on: departureQueue, body))
        } catch {
            result = .failed
        }
        await afterDeparture()
        return result
    }

    /// After a departure call, whatever it gave: the state is read again; once erased the doorbell stops.
    private func afterDeparture() async {
        await refresh()
        if !state.isPaired {
            wantsDoorbell = false
            stopDoorbell()
        }
    }

    // MARK: Pairing
    //
    // The demo core simulates the pairing on the former calls of the contract (`pair_with_*`, `confirm_pairing`): the
    // two devices of a real pairing cannot both be simulated. The real core joins through a `join_space` session
    // (Android `RealTomiCore.joinWith`).

    /// Outside a pairing or a creation under way (the core then points at the candidate relay).
    public func currentRelay() async -> String? {
        (try? await call(on: local) { $0.relayUrl() }) ?? nil
    }

    /// The code and the relay that goes with it (the link's or the field's, nil: the device's own). Never logged.
    public func pairWithCode(code: String, relayUrl: String?) async throws -> PairingResult {
        if state.isPaired { return .alreadyPaired }
        if !isDemo { return await joinWith(code, relayUrl: relayUrl) }
        return try await call(on: pairing) { CoreMapping.pairing(try $0.pairWithCode(code: code, relayUrl: relayUrl)) }
    }

    public func confirmPairing(importHistory: Bool) async throws {
        if !isDemo {
            try await confirmJoin(importHistory: importHistory)
            return
        }
        do {
            try await call(on: pairing) { try $0.confirmPairing(importHistory: importHistory) }
        } catch let error as TomiCoreError {
            throw PairingErrors.failure(of: error)
        }
        await refresh()
    }

    public func cancelPairing() async {
        if !isDemo {
            await abandonJoin()
            return
        }
        _ = try? await call(on: local) { try $0.cancelPairing() }
    }

    /// Claims the code, then waits for the inviter's side to be verified: the codes to compare. `relayUrl` is the relay
    /// of the link or of the "Relais" field (not trusted: the core points at it for this session only and the prompt
    /// carries its authority, which the screen shows in full).
    private func joinWith(_ codeOrLink: String, relayUrl: String?) async -> PairingResult {
        // A previous attempt that nobody settled is closed first: one pairing at a time, and the relay counts the
        // open channels.
        await abandonJoin()
        // The core reports an unreadable code, an invalid name and "already provisioned" with the same InvalidInput:
        // the last two are settled before the call, so that what it reports is only about the code.
        if (try? await call(on: local, { $0.isPaired() })) == true {
            await refresh()
            return .alreadyPaired
        }
        let name = DeviceNames.choose([deviceName, DeviceNames.fallback], isValid: coreAcceptsDeviceName)
        let generation = joinGeneration
        let session: FfiJoinSession
        do {
            session = try await call(on: pairing) {
                try $0.joinSpace(codeOrLink: codeOrLink, deviceName: name, relayUrl: relayUrl)
            }
        } catch let error as TomiCoreError {
            return await joinRefused(error, code: codeOrLink, relayGiven: relayUrl != nil)
        } catch {
            return .failure(.unexpected)
        }
        // The screen was left during the claim: the channel is closed at once, not kept for the inviter.
        guard generation == joinGeneration else {
            _ = try? await joinControl.run { session.cancel() }
            return .failure(.cancelled)
        }
        join = session
        do {
            let prompt = try await runJoin(session, retrying: true) { try $0.waitForInviter() }
            guard join === session else { return .failure(.cancelled) }
            return .awaitingVerification(PairingPrompt(
                peer: PairingPeer(name: prompt.inviter.name, platform: DevicePlatform(core: prompt.inviter.platform)),
                verificationCode: prompt.sas.filter(\.isNumber),
                historyItemCount: Int(clamping: prompt.history?.items ?? 0),
                historyBytes: Int64(clamping: prompt.history?.bytes ?? 0),
                memberCount: prompt.memberCount.map(Int.init),
                expiresInMs: max(session.expiresInMs(), 0),
                relayAuthority: prompt.relayAuthority
            ))
        } catch let error as TomiCoreError {
            dropJoin(session)
            // The code is spent once claimed: after a broken connection the user needs a new one, not a retry.
            let reason = PairingErrors.failure(of: error)
            return .failure(reason == .network ? .interrupted : reason)
        } catch {
            dropJoin(session)
            return .failure(.interrupted)
        }
    }

    /// `join_space` refused: `InvalidInput` is an unreadable code or an invalid relay once the device is unpaired and its
    /// name valid. The code is told apart by reading it the way the core does, as the fallback link (`tomi-share://pair#...`):
    /// readable, the relay is the culprit. Nothing of the code is kept or logged.
    private func joinRefused(_ error: TomiCoreError, code: String, relayGiven: Bool) async -> PairingResult {
        guard error.kind == .invalidInput else { return .failure(PairingErrors.failure(of: error)) }
        // A stopped pairing may have been settled (provisioned) between the check and the call.
        if (try? await call(on: local, { $0.isPaired() })) == true {
            await refresh()
            return .alreadyPaired
        }
        if relayGiven, CoreLinkReader().readPairLink("tomi-share://pair#" + code.trimmingCharacters(in: .whitespacesAndNewlines)) != nil {
            return .failure(.invalidRelay)
        }
        return .failure(.invalidCode)
    }

    /// "Rejoindre": deposits the confirmation, then waits until the other person confirmed too and this device is
    /// provisioned (which can take the whole life of the channel; `cancelPairing` ends the wait).
    private func confirmJoin(importHistory: Bool) async throws {
        guard let session = join else { return }
        do {
            try await runJoin(session, retrying: false) { try $0.confirm(importHistory: importHistory) }
            _ = try await runJoin(session, retrying: true) { try $0.waitDone() }
        } catch let error as TomiCoreError {
            // A transient error or "not the moment" leaves the session alive: try again, or cancel (which closes it).
            if !PairingErrors.isTransient(error) && error.kind != .invalidInput { dropJoin(session) }
            let reason = PairingErrors.failure(of: error)
            throw reason == .network ? PairingFailure.interrupted : reason
        }
        dropJoin(session)
        await refresh()
    }

    /// Runs a blocking call of the session on `pairing`; transient errors are tried again after 1, 2 and 4 s.
    private func runJoin<T: Sendable>(
        _ session: FfiJoinSession, retrying: Bool, _ body: @escaping @Sendable (FfiJoinSession) throws -> T
    ) async throws -> T {
        do {
            return try await pairing.run {
                var attempt = 0
                while true {
                    do {
                        return try body(session)
                    } catch let error as TomiError {
                        let mapped = CoreMapping.error(error)
                        guard retrying, PairingErrors.isTransient(mapped), attempt < PairingErrors.retryDelaysMs.count else {
                            throw mapped
                        }
                        Thread.sleep(forTimeInterval: Double(PairingErrors.retryDelaysMs[attempt]) / 1000)
                        attempt += 1
                    }
                }
            }
        }
    }

    /// Closes the pending joining at the relay and wipes its secrets; a call that waits on it returns cancelled.
    private func abandonJoin() async {
        joinGeneration += 1
        guard let session = join else { return }
        join = nil
        _ = try? await joinControl.run { session.cancel() }
    }

    /// The session ended by itself: forgotten.
    private func dropJoin(_ session: FfiJoinSession) {
        if join === session { join = nil }
    }

    public func startInvitation() async throws -> any InviteSession {
        do {
            let session = try await call(on: pairing) { try $0.startInvitation() }
            return RustInviteSession(session)
        } catch let error as TomiCoreError {
            throw PairingErrors.failure(of: error)
        }
    }

    /// `relayUrl`: the relay of the creation link or of the "Relais" field, which the screen showed in full; the core
    /// keeps it only once the space is created. `InvalidInput` with a relay given is that relay (the name and the token
    /// were checked by the screen, and the device is not paired).
    public func createSpace(installToken: String, deviceName: String, relayUrl: String?) async throws {
        do {
            try await call(on: pairing) {
                try $0.createSpace(installToken: installToken, deviceName: deviceName, relayUrl: relayUrl)
            }
        } catch let error as TomiCoreError {
            if error.kind == .invalidInput, relayUrl != nil, (try? await call(on: local, { $0.isPaired() })) != true {
                throw PairingFailure.invalidRelay
            }
            throw PairingErrors.failure(of: error)
        }
        await refresh()
    }

    /// Asks the relay about a pairing a stop interrupted; after a transient error, tries again 2, 5, 15, 30 and 60 s
    /// later (Android `RealTomiCore.resumePairing`).
    public func resumePairing() async -> ResumeOutcome {
        if state.isPaired { return .nothingPending }
        for delay in [0, 2, 5, 15, 30, 60] {
            if delay > 0 {
                do { try await Task.sleep(for: .seconds(delay)) } catch { return .unreachable }
            }
            do {
                guard try await call(on: pairing, { try $0.hasPendingPairing() }) else { return .nothingPending }
                switch try await call(on: pairing, { try $0.resumePairing() }) {
                case .nothing: return .nothingPending
                case .discarded: return .discarded
                case .provisioned:
                    await refresh()
                    return .provisioned
                }
            } catch let error as TomiCoreError where PairingErrors.isTransient(error) {
                continue
            } catch {
                return .nothingPending
            }
        }
        return .unreachable
    }

    public func isValidDeviceName(_ name: String) -> Bool { coreAcceptsDeviceName(name) }

    public func simulateIncoming(_ kind: SimulatedIncoming) async throws {
        let ffi = CoreMapping.incoming(kind)
        try await call(on: network) { try $0.simulateIncoming(kind: ffi) }
    }

    // MARK: Demo entered from the real core

    public func enterDemo() async throws {
        guard let entry = demoEntry, supportsDemoEntry else {
            throw TomiCoreError(.invalidInput, reason: "demo entry not supported")
        }
        // Never on a device that belongs to a space (nor over a demo): the same rule as `canEnterDemo`, checked again here
        // so that no caller can reach the demo's fixed codes from a provisioned device.
        guard canEnterDemo else {
            throw TomiCoreError(.invalidInput, reason: "demo entry refused")
        }
        // A joining left open on the real core is closed first: nothing real stays pending behind the demo.
        await abandonJoin()
        try await call(on: pairing) { core in
            try core.enterDemo(config: DemoConfig(
                filesDir: entry.filesDir,
                assetsDir: entry.assetsDir,
                platform: "ios",
                appVersion: entry.appVersion,
                deviceName: entry.deviceName,
                startPaired: true
            ))
        }
        await refresh()
    }

    public func leaveDemo() async {
        _ = try? await call(on: pairing) { $0.leaveDemo() }
        await refresh()
    }

    public func purgePlaintextCache() async throws {
        try await call(on: purgeQueue) { try $0.purgePlaintextCache() }
    }

    // MARK: Foreground / background

    public func enterForeground() {
        wantsDoorbell = true
        startDoorbellIfNeeded()
        Task { try? await self.sync(pushToken: nil) }
    }

    /// Stops the doorbell and erases the decrypted files, inside an iOS background task so the
    /// erasure is not cut short by the suspension.
    public func enterBackground() {
        wantsDoorbell = false
        stopDoorbell()
        #if canImport(UIKit)
        let task = UIApplication.shared.beginBackgroundTask(withName: "tomi-purge")
        Task {
            try? await self.purgePlaintextCache()
            if task != .invalid { UIApplication.shared.endBackgroundTask(task) }
        }
        #else
        Task { try? await self.purgePlaintextCache() }
        #endif
    }

    /// One doorbell thread at most, and only for a paired, active real core. A doorbell the core
    /// closed by itself is not restarted until the next `enterForeground`.
    private func startDoorbellIfNeeded() {
        guard wantsDoorbell, !doorbellRunning, !isDemo, state.isPaired, state.spaceState == .active else { return }
        doorbellRunning = true
        doorbellStopRequested = false
        let core = self.core
        let thread = Thread { [weak self] in
            core.runDoorbell()
            Task { @MainActor in self?.doorbellEnded() }
        }
        thread.name = "tomi-doorbell"
        thread.qualityOfService = .utility
        thread.start()
    }

    private func doorbellEnded() {
        doorbellRunning = false
        if doorbellStopRequested {
            // Stopped by the app: back on screen in the meantime, open it again.
            doorbellStopRequested = false
            startDoorbellIfNeeded()
        } else {
            // Closed by the core (revoked, space reset, replaced): wait for the next foreground.
            wantsDoorbell = false
        }
    }

    /// Asks the doorbell to stop until it has. `runDoorbell` clears the stop flag when it starts,
    /// so a stop sent just before that would be lost: it is repeated while the thread runs.
    private func stopDoorbell() {
        guard doorbellRunning else { return }
        doorbellStopRequested = true
        core.stopDoorbell()
        Task { @MainActor in
            for _ in 0..<50 {
                try? await Task.sleep(for: .milliseconds(100))
                guard doorbellRunning, !wantsDoorbell else { return }
                core.stopDoorbell()
            }
        }
    }
}

/// The core's rule for a device name (file scope: the adapter's method of the same name would shadow it).
private func coreAcceptsDeviceName(_ name: String) -> Bool { isValidDeviceName(name: name) }
