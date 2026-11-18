import Foundation
import TomiKit
import TomiRuntime
import XCTest
@testable import Tomi

@MainActor
final class AppContainerTests: XCTestCase {
    private func tempPaths() throws -> CorePaths {
        let paths = CorePaths(root: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
        try paths.prepare()
        return paths
    }

    func testDebugBuildsStartOnTheDemoCoreWithTheBundledDemoFiles() {
        guard case let .demo(startPaired, assets) = AppContainer.defaultMode() else {
            return XCTFail("debug builds run on the demo core")
        }
        XCTAssertTrue(startPaired)
        let images = try? FileManager.default.contentsOfDirectory(atPath: assets?.path ?? "")
        XCTAssertTrue(images?.contains("demo_paysage.jpg") == true, "the Android demo assets are bundled")
    }

    func testTheRealModeUsesTheCompiledDebugRelay() {
        let defaults = UserDefaults(suiteName: "AppContainerTests")!
        defaults.set("real", forKey: "TomiCoreMode")
        defer { defaults.removePersistentDomain(forName: "AppContainerTests") }
        XCTAssertEqual(AppContainer.defaultMode(arguments: defaults), .real(relayURL: URL(string: "http://localhost:8080")!))
    }

    /// No relay is compiled into the repository: an empty setting (or one Xcode left unresolved) is "no default relay".
    func testAnEmptyRelaySettingMeansNoDefaultRelay() {
        XCTAssertNil(AppContainer.relayURL(fromSetting: nil))
        XCTAssertNil(AppContainer.relayURL(fromSetting: ""))
        XCTAssertNil(AppContainer.relayURL(fromSetting: "  \n"))
        XCTAssertNil(AppContainer.relayURL(fromSetting: "$(TOMI_RELAY_URL)"))
        XCTAssertEqual(AppContainer.relayURL(fromSetting: " https://relay.example.org "), URL(string: "https://relay.example.org"))
    }

    func testTheContainerOpensTheDemoCore() async throws {
        let paths = try tempPaths()
        let container = AppContainer(
            mode: .demo(startPaired: true, assets: Bundle.main.url(forResource: "demo", withExtension: nil)),
            paths: { paths }
        )
        await container.start()
        let core = try XCTUnwrap(container.core)
        // The demo feed of the core: the 23 items of the Android demo on top of a generated week and years of history.
        let deadline = Date().addingTimeInterval(120)
        while !(core.state.loaded && core.state.items.count > 23) && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertGreaterThan(core.state.items.count, 23)
        // With the bundled assets the demo images are the real photos, with thumbnails.
        let photos = try XCTUnwrap(core.state.items.first { $0.title == "Photos du week-end" })
        XCTAssertEqual(photos.attachments.map(\.name), ["demo_paysage.jpg", "demo_portrait_exif.jpg", "demo_paysage.jpg"])
        XCTAssertTrue(photos.attachments.allSatisfy { $0.thumbnail != nil })
        (core as? RustTomiCoreAdapter)?.shutdown()
    }
}
