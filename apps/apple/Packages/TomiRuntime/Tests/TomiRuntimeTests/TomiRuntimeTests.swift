import Foundation
import Security
import TomiKit
import XCTest
@testable import TomiRuntime

/// In-memory key store (the Keychain is not reachable from a hostless test bundle).
private final class MemoryKeyStore: WrapKeyStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var key: Data?
    private(set) var saves = 0

    func load() throws -> Data? {
        lock.withLock { key }
    }

    func save(_ key: Data) throws {
        try lock.withLock {
            if self.key != nil { throw WrapKeyError.keychain(errSecDuplicateItem) }
            self.key = key
            saves += 1
        }
    }

    func replace(_ key: Data) throws {
        lock.withLock { self.key = key }
    }

    /// Simulates another start that stored its key between our load and our save.
    func storeBehindTheBack(_ key: Data) {
        lock.withLock { self.key = key }
    }
}

/// Lets `load` miss once, as if another start was creating the key at the same moment.
private final class RacingKeyStore: WrapKeyStoring, @unchecked Sendable {
    let inner = MemoryKeyStore()
    let other = Data(repeating: 7, count: 32)
    private var firstLoad = true

    func load() throws -> Data? {
        if firstLoad {
            firstLoad = false
            inner.storeBehindTheBack(other)
            return nil
        }
        return try inner.load()
    }

    func save(_ key: Data) throws { try inner.save(key) }
    func replace(_ key: Data) throws { try inner.replace(key) }
}

@MainActor
final class TomiRuntimeTests: XCTestCase {
    /// Items of the Android demo, on top of the demo feed of the core (a generated week and years of history follow).
    private static let androidDemoItems = 23

    private func tempPaths() throws -> CorePaths {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        let paths = CorePaths(root: root)
        try paths.prepare()
        return paths
    }

    private func open(_ mode: CoreMode, keyStore: WrapKeyStoring = MemoryKeyStore()) async throws -> RustTomiCoreAdapter {
        try await CoreFactory.open(mode: mode, paths: try tempPaths(), appVersion: "0.1.0", deviceName: "iPhone", keyStore: keyStore)
    }

    /// Waits until `condition` holds on the main actor (the state is published asynchronously).
    private func eventually(_ what: String, timeout: TimeInterval = 10, _ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline {
            try? await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertTrue(condition(), what)
    }

    func testTheDemoCoreShowsTheDemoFeedAndReceivesItems() async throws {
        let core = try await open(.demo(startPaired: true, assets: nil))
        XCTAssertTrue(core.isDemo)
        // The demo feed of the core: the items of the Android demo on top of a generated week and years of history.
        await eventually("demo feed loaded") { core.state.loaded && core.state.items.count > Self.androidDemoItems }
        XCTAssertTrue(core.state.isPaired)
        XCTAssertGreaterThanOrEqual(core.state.items.filter(\.archived).count, 4)
        XCTAssertGreaterThanOrEqual(core.state.items.filter(\.pinned).count, 2)
        XCTAssertEqual(core.state.devices.count, 3)
        XCTAssertEqual(core.state.localDevice?.name, "iPhone")
        XCTAssertEqual(core.state.localDevice?.platform, .ios)
        XCTAssertTrue(core.state.items.contains { $0.kind == .unreadable })

        let feed = core.state.items.count
        try await core.simulateIncoming(.link)
        await eventually("the listener publishes the new item") { core.state.items.count == feed + 1 }
        let link = try XCTUnwrap(core.state.items.first)
        XCTAssertEqual(link.title, "Documentation Android")
        XCTAssertFalse(link.read)

        // Archive and pin go through the core and come back through the listener.
        try await core.archive(id: link.id)
        await eventually("archived") { core.state.items.first { $0.id == link.id }?.archived == true }
        try await core.pin(id: link.id)
        await eventually("pin unarchives") {
            let item = core.state.items.first { $0.id == link.id }
            return item?.pinned == true && item?.archived == false
        }
        core.shutdown()
    }

    func testTheDemoPairingThroughTheAdapter() async throws {
        let core = try await open(.demo(startPaired: false, assets: nil))
        await eventually("first reading") { core.state.loaded }
        XCTAssertFalse(core.state.isPaired)
        XCTAssertTrue(core.state.items.isEmpty)
        guard case let .awaitingVerification(prompt) = try await core.pairWithCode(code: "7-crayon-lune", relayUrl: nil) else {
            return XCTFail("the demo accepts the code")
        }
        XCTAssertEqual(prompt.peer, PairingPeer(name: "PC-Bureau", platform: .windows))
        XCTAssertEqual(prompt.verificationCode, "482913")
        XCTAssertGreaterThan(prompt.historyItemCount, Self.androidDemoItems)
        XCTAssertGreaterThan(prompt.historyBytes, 0)
        XCTAssertEqual(prompt.memberCount, 2)
        XCTAssertEqual(prompt.expiresInMs, 120_000)
        XCTAssertEqual(prompt.relayAuthority, "relay.example.org", "shown in full next to the verification code")
        try await core.confirmPairing(importHistory: false)
        XCTAssertTrue(core.state.isPaired)
        XCTAssertTrue(core.state.items.isEmpty, "no history asked")
        core.shutdown()
    }

    /// The relay typed next to the code is the one the verification screen shows (the demo core reports it as the real one does).
    func testTheRelayTypedNextToTheCodeIsTheOneShownWithTheVerificationCode() async throws {
        let core = try await open(.demo(startPaired: false, assets: nil))
        await eventually("first reading") { core.state.loaded }
        guard case let .awaitingVerification(prompt) = try await core.pairWithCode(
            code: "7-crayon-lune", relayUrl: "https://other.example.net:8443"
        ) else {
            return XCTFail("the demo accepts the code")
        }
        XCTAssertEqual(prompt.relayAuthority, "other.example.net:8443")
        await core.cancelPairing()
        core.shutdown()
    }

    /// The demo's "Tablette" answers with the real pairing protocol: the adapter drives an `FfiInviteSession`.
    func testAnInvitationIsAnsweredByTheDemoTablet() async throws {
        let core = try await open(.demo(startPaired: true, assets: nil))
        await eventually("first reading") { core.state.loaded }
        let session = try await core.startInvitation()
        XCTAssertNotNil(session.code)
        XCTAssertTrue(session.link?.contains("#") == true)
        XCTAssertGreaterThan(session.expiresInMs(), 0)
        let prompt = try await session.awaitPeer()
        XCTAssertEqual(prompt.peer, PairingPeer(name: "Tablette", platform: .android))
        XCTAssertEqual(prompt.verificationCode.count, 6)
        try await session.confirm()
        let added = try await session.awaitDone()
        XCTAssertEqual(added.name, "Tablette")
        try await core.sync(pushToken: nil)
        await eventually("the tablet in the devices") { core.state.devices.contains { $0.name == "Tablette" } }
        core.shutdown()
    }

    /// The departure through the adapter, with the real code of the core against the demo relay.
    func testADepartureStaysPendingOfflineThenErasesWithTheProof() async throws {
        let core = try await open(.demo(startPaired: true, assets: nil))
        await eventually("first reading") { core.state.loaded }
        await core.simulateDeparture(.offline)
        let first = await core.leaveSpace()
        XCTAssertEqual(first, .pending(.network))
        XCTAssertEqual(core.state.departure?.kind, .leave)
        XCTAssertFalse(core.state.departureRunning)
        XCTAssertTrue(core.state.isPaired, "nothing erased without a proof")
        await core.simulateDeparture(.normal)
        let second = await core.continueDeparture()
        XCTAssertEqual(second, .erased)
        XCTAssertFalse(core.state.isPaired)
        XCTAssertNil(core.state.departure)
        core.shutdown()
    }

    func testTheDemoCreatesTheSpaceWithItsTokenOnly() async throws {
        let core = try await open(.demo(startPaired: false, assets: nil))
        await eventually("first reading") { core.state.loaded }
        do {
            try await core.createSpace(installToken: "aaaa-bbbb-cccc-dddd-eeee-ffff-gggg-hhhh", deviceName: "iPhone", relayUrl: nil)
            XCTFail("a wrong token is refused")
        } catch let failure as PairingFailure {
            XCTAssertEqual(failure, .installTokenInvalid)
        }
        try await core.createSpace(installToken: "2345-6789-abcd-efgh-jkmn-pqrs-tvwx-yz23", deviceName: "Alice's iPhone", relayUrl: nil)
        XCTAssertTrue(core.state.isPaired)
        XCTAssertTrue(core.state.items.isEmpty)
        core.shutdown()
    }

    func testARealCoreOpensUnpairedAndRefusesAnUnreadableCodeLocally() async throws {
        let keys = MemoryKeyStore()
        let core = try await open(.real(relayURL: URL(string: "https://relay.example.org")!), keyStore: keys)
        XCTAssertFalse(core.isDemo)
        await eventually("first reading") { core.state.loaded }
        XCTAssertFalse(core.state.isPaired)
        XCTAssertEqual(keys.saves, 1, "a new database gets a new key")
        // Read by the core before any request: nothing reaches the relay, nothing is consumed.
        let result = try await core.pairWithCode(code: "pas un code", relayUrl: nil)
        XCTAssertEqual(result, .failure(.invalidCode))
        // No joining open: nothing to confirm, nothing thrown.
        try await core.confirmPairing(importHistory: true)
        XCTAssertFalse(core.state.isPaired)
        XCTAssertTrue(core.isValidDeviceName(" Alice's iPhone "))
        XCTAssertFalse(core.isValidDeviceName(String(repeating: "a", count: 65)))
        let resumed = await core.resumePairing()
        XCTAssertEqual(resumed, .nothingPending)
        // Sync without a space does nothing and never throws.
        try await core.sync(pushToken: nil)
        core.shutdown()
    }

    /// A release build has no relay: it comes from the link, the QR code, the field or the creation link. Without one the
    /// core refuses locally (nothing is sent), and so does an address that is not a relay.
    func testARealCoreWithoutARelayNeedsOneAndChecksTheOneItIsGiven() async throws {
        let core = try await open(.real(relayURL: nil), keyStore: MemoryKeyStore())
        await eventually("first reading") { core.state.loaded }
        let none = await core.currentRelay()
        XCTAssertNil(none)
        let missing = try await core.pairWithCode(code: "4821-crayon-renard", relayUrl: nil)
        XCTAssertEqual(missing, .failure(.relayNotConfigured))
        let invalid = try await core.pairWithCode(code: "4821-crayon-renard", relayUrl: "http://relay.example.org")
        XCTAssertEqual(invalid, .failure(.invalidRelay))
        let stillNone = await core.currentRelay()
        XCTAssertNil(stillNone, "nothing was kept")
        for relay in [nil, "http://relay.example.org"] as [String?] {
            do {
                try await core.createSpace(installToken: "2345-6789-abcd-efgh-jkmn-pqrs-tvwx-yz23", deviceName: "iPhone", relayUrl: relay)
                XCTFail("no valid relay, nothing to create on")
            } catch let failure as PairingFailure {
                XCTAssertEqual(failure, relay == nil ? .relayNotConfigured : .invalidRelay)
            }
        }
        XCTAssertFalse(core.state.isPaired)
        core.shutdown()
    }

    /// The reading of links is the core's: the Swift side only maps it.
    func testTheLinksAreReadByTheCore() {
        let reader = CoreLinkReader()
        let pair = reader.readPairLink("https://relay.example.org/l/pair#4821-crayon-renard")
        XCTAssertEqual(pair, ParsedPairLink(relayUrl: "https://relay.example.org", code: "4821-crayon-renard"))
        XCTAssertNil(reader.readPairLink("tomi-share://pair#4821-crayon-renard")?.relayUrl)
        XCTAssertNotNil(reader.readPairLink("tomi-share://pair#4821-crayon-renard"))
        XCTAssertNil(reader.readPairLink("http://relay.example.org/l/pair#4821-crayon-renard"))
        let token = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"
        XCTAssertEqual(
            reader.readSetupLink("https://relay.example.org/create#\(token)"),
            ParsedSetupLink(relayUrl: "https://relay.example.org", token: token)
        )
        XCTAssertTrue(reader.hasTokenShape(token))
        XCTAssertFalse(reader.hasTokenShape("4821-crayon-renard"))
        XCTAssertEqual(RelayAddress.canonical("relay.example.org", reader: reader), "https://relay.example.org")
        XCTAssertNil(RelayAddress.canonical("http://relay.example.org", reader: reader))
    }

    func testARealCoreEntersTheDemoAndLeavesIt() async throws {
        let core = try await open(.real(relayURL: URL(string: "https://relay.example.org")!))
        await eventually("first reading") { core.state.loaded }
        XCTAssertFalse(core.isDemo)
        XCTAssertTrue(core.canEnterDemo)
        XCTAssertFalse(core.canLeaveDemo)

        try await core.enterDemo()
        XCTAssertTrue(core.state.isPaired)
        XCTAssertTrue(core.isDemo)
        XCTAssertTrue(core.canLeaveDemo)
        XCTAssertFalse(core.canEnterDemo)
        XCTAssertGreaterThan(core.state.items.count, Self.androidDemoItems)

        await core.leaveDemo()
        XCTAssertFalse(core.state.isPaired)
        XCTAssertFalse(core.isDemo)
        XCTAssertTrue(core.state.items.isEmpty)
        XCTAssertTrue(core.canEnterDemo)

        // Leaving the space inside the demo leaves the demo too.
        try await core.enterDemo()
        let result = await core.leaveSpace()
        XCTAssertEqual(result, .erased)
        XCTAssertFalse(core.isDemo)
        XCTAssertFalse(core.state.isPaired)
        core.shutdown()
    }

    func testTheDemoCannotBeEnteredTwiceNorOverASpace() async throws {
        let core = try await open(.real(relayURL: URL(string: "https://relay.example.org")!))
        await eventually("first reading") { core.state.loaded }
        try await core.enterDemo()
        do {
            try await core.enterDemo()
            XCTFail("already in the demo")
        } catch {
            XCTAssertEqual((error as? TomiCoreError)?.kind, .invalidInput)
        }
        core.shutdown()
    }

    func testACoreOpenedAsADemoCannotEnterNorLeaveTheDemo() async throws {
        let core = try await open(.demo(startPaired: true, assets: nil))
        await eventually("first reading") { core.state.loaded }
        XCTAssertTrue(core.isDemo)
        XCTAssertFalse(core.canEnterDemo)
        XCTAssertFalse(core.canLeaveDemo)
        core.shutdown()
    }

    func testTheWrapKeyIsKeptAndNeverRecreatedForAnExistingDatabase() throws {
        let store = MemoryKeyStore()
        let first = try WrapKey.loadOrCreate(store: store, databaseExists: false)
        XCTAssertEqual(first.count, 32)
        XCTAssertEqual(try WrapKey.loadOrCreate(store: store, databaseExists: true), first)
        XCTAssertEqual(store.saves, 1)
        XCTAssertThrowsError(try WrapKey.loadOrCreate(store: MemoryKeyStore(), databaseExists: true)) { error in
            XCTAssertEqual(error as? WrapKeyError, .missingForExistingDatabase)
        }
    }

    func testAConcurrentStartKeepsTheKeyStoredFirst() throws {
        let store = RacingKeyStore()
        XCTAssertEqual(try WrapKey.loadOrCreate(store: store, databaseExists: false), store.other)
        // A malformed leftover without a database is replaced.
        let leftover = MemoryKeyStore()
        leftover.storeBehindTheBack(Data([1, 2, 3]))
        let key = try WrapKey.loadOrCreate(store: leftover, databaseExists: false)
        XCTAssertEqual(key.count, 32)
        XCTAssertEqual(try leftover.load(), key)
    }

    func testThePathsAreExcludedFromBackups() throws {
        let paths = try tempPaths()
        let values = try paths.root.resourceValues(forKeys: [.isExcludedFromBackupKey])
        XCTAssertEqual(values.isExcludedFromBackup, true)
        XCTAssertEqual(paths.database.lastPathComponent, "tomi.db")
    }
}
