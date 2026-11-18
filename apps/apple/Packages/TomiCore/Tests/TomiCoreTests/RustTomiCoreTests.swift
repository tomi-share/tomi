import XCTest
@testable import TomiCore

/// Smoke tests of the Rust core through the generated Swift bindings. No network: `open` only touches the disk.
final class RustTomiCoreTests: XCTestCase {
    private func config(dir: URL, wrapKey: [UInt8] = [UInt8](repeating: 3, count: 32), relayUrl: String? = "https://relay.example.org") -> CoreConfig {
        CoreConfig(
            dbPath: dir.appendingPathComponent("tomi.db").path,
            filesDir: dir.appendingPathComponent("files").path,
            wrapKey: Data(wrapKey),
            relayUrl: relayUrl,
            appVersion: "0.1.0",
            platform: "ios",
            autoImport: false
        )
    }

    private func tempDir() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func testOpensAnUnprovisionedCore() throws {
        let core = try RustTomiCore.open(config: config(dir: try tempDir()))
        XCTAssertFalse(core.isPaired())
        XCTAssertEqual(core.spaceState(), .active)
        // Not provisioned: the feed refuses to work instead of crashing.
        XCTAssertThrowsError(try core.items()) { error in
            guard case TomiError.InvalidInput = error else { return XCTFail("unexpected error \(error)") }
        }
    }

    func testRefusesAWrapKeyThatIsNot32Bytes() throws {
        XCTAssertThrowsError(try RustTomiCore.open(config: config(dir: try tempDir(), wrapKey: [1, 2, 3])))
    }

    func testRefusesPlainHttpOutsideLocalhost() throws {
        XCTAssertThrowsError(try RustTomiCore.open(config: config(dir: try tempDir(), relayUrl: "http://relay.example.org")))
    }

    /// No relay is compiled into a release build: the device has none until a link, a QR code, a field or a creation
    /// link gives one, and nothing is kept before it worked.
    func testWithoutADefaultRelayTheCoreHasNoneUntilOneIsGiven() throws {
        let core = try RustTomiCore.open(config: config(dir: try tempDir(), relayUrl: nil))
        XCTAssertNil(core.relayUrl())
        XCTAssertThrowsError(try core.joinSpace(codeOrLink: "4821-crayon-renard", deviceName: "iPhone")) { error in
            guard case let TomiError.Crypto(code) = error else { return XCTFail("unexpected error \(error)") }
            XCTAssertEqual(code, "relay_not_configured")
        }
        XCTAssertNil(core.relayUrl(), "nothing was kept")
        XCTAssertThrowsError(try core.setRelay(url: "http://relay.example.org"), "https only")
        try core.setRelay(url: "https://relay.example.org")
        XCTAssertEqual(core.relayUrl(), "https://relay.example.org")
    }

    /// The single reading of links shared by the three platforms (`crates/core/src/relay_url.rs`).
    func testLinksAndTokensAreReadByTheCore() {
        let token = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"
        let pair = parsePairLink(text: "https://relay.example.org/l/pair#4821-crayon-renard")
        XCTAssertEqual(pair?.relayUrl, "https://relay.example.org")
        XCTAssertEqual(pair?.code, "4821-crayon-renard")
        let fallback = parsePairLink(text: "tomi-share://pair#4821-crayon-renard")
        XCTAssertNotNil(fallback)
        XCTAssertNil(fallback?.relayUrl, "the fallback form names no relay")
        XCTAssertNil(parsePairLink(text: "http://relay.example.org/l/pair#4821-crayon-renard"))
        let setup = parseSetupLink(text: "https://relay.example.org/create#\(token)")
        XCTAssertEqual(setup?.relayUrl, "https://relay.example.org")
        XCTAssertEqual(setup?.token, token)
        XCTAssertNil(parseSetupLink(text: "https://relay.example.org/create"))
        XCTAssertTrue(isInstallTokenShape(text: token))
        XCTAssertFalse(isInstallTokenShape(text: "4821-crayon-renard"))
    }

    func testARealCoreCannotPairYet() throws {
        let core = try RustTomiCore.open(config: config(dir: try tempDir()))
        XCTAssertFalse(core.isDemo())
        XCTAssertEqual(try core.pairWithCode(code: "4821-crayon-renard"), .failure(reason: .unsupported))
    }

    /// The demo core: the real engine on an in-memory demo space, the same on every platform.
    func testDemoCorePairsAndShowsTheDemoFeed() throws {
        let core = try RustTomiCore.openDemo(config: DemoConfig(
            filesDir: try tempDir().path,
            assetsDir: nil,
            platform: "ios",
            appVersion: "0.1.0",
            deviceName: "iPhone",
            startPaired: false
        ))
        XCTAssertTrue(core.isDemo())
        XCTAssertFalse(core.isPaired())
        guard case let .awaitingVerification(device, code, count, _, members, _, relayAuthority) = try core.pairWithCode(code: "7-crayon-lune") else {
            return XCTFail("the demo pairing accepts any code of 4 characters or more")
        }
        XCTAssertEqual(device.name, "PC-Bureau")
        XCTAssertEqual(code, "482913")
        // The items of the Android demo on top of a generated week and years of history.
        XCTAssertGreaterThan(count, 23)
        XCTAssertEqual(members, 2)
        XCTAssertEqual(relayAuthority, "relay.example.org", "shown in full next to the verification code")
        try core.confirmPairing(importHistory: true)
        XCTAssertTrue(core.isPaired())

        let items = try core.items()
        XCTAssertEqual(items.count, Int(count))
        XCTAssertGreaterThanOrEqual(items.filter(\.archived).count, 4)
        XCTAssertGreaterThanOrEqual(items.filter(\.pinned).count, 2)

        try core.simulateIncoming(kind: .link)
        XCTAssertEqual(try core.items().count, items.count + 1)
    }
}
