import CoreGraphics
import Foundation
import ImageIO
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

    func testTheAppDeclaresTheTomiLinks() {
        // `tomi-share://…` reaches the app only if its Info.plist says so (iOS and macOS each have their own).
        let types = Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]] ?? []
        let schemes = types.flatMap { $0["CFBundleURLSchemes"] as? [String] ?? [] }
        XCTAssertTrue(schemes.contains("tomi-share"), "schemes declared: \(schemes)")
    }

    func testTheImageOfANotificationIsMadeSmall() throws {
        // A 1200 × 800 picture, as a thumbnail of the core could be.
        let size = CGSize(width: 1200, height: 800)
        let context = try XCTUnwrap(CGContext(
            data: nil, width: Int(size.width), height: Int(size.height), bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        context.setFillColor(CGColor(red: 0.2, green: 0.4, blue: 0.8, alpha: 1))
        context.fill(CGRect(origin: .zero, size: size))
        let image = try XCTUnwrap(context.makeImage())
        let encoded = NSMutableData()
        let destination = try XCTUnwrap(CGImageDestinationCreateWithData(encoded, "public.png" as CFString, 1, nil))
        CGImageDestinationAddImage(destination, image, nil)
        XCTAssertTrue(CGImageDestinationFinalize(destination))

        let small = try XCTUnwrap(NotificationImage.make(encoded as Data))
        let source = try XCTUnwrap(CGImageSourceCreateWithData(small as CFData, nil))
        let result = try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil))
        XCTAssertEqual(max(result.width, result.height), NotificationImage.maxPixels)
        XCTAssertEqual(CGImageSourceGetType(source) as String?, "public.jpeg")
        XCTAssertNil(NotificationImage.make(Data("not an image".utf8)))
    }

    func testEveryBuildStartsOnTheOneCoreAgainstTheRelayOfTheSpace() {
        let defaults = UserDefaults(suiteName: "AppContainerTests")!
        defer { defaults.removePersistentDomain(forName: "AppContainerTests") }
        // No demo at launch, no local relay unless asked: what a developer build runs is what is published. The relay
        // is the one of the build when it has one (`Config/Local.xcconfig`), else it comes from the pairing.
        XCTAssertEqual(AppContainer.defaultMode(arguments: defaults, testing: false), .real(relayURL: AppContainer.configuredRelay()))
        XCTAssertFalse(AppContainer.startsInDemo(arguments: defaults))
        defaults.set(true, forKey: "TomiStartInDemo")
        XCTAssertTrue(AppContainer.startsInDemo(arguments: defaults), "asked by the developer: the demo of that same core")
        XCTAssertEqual(AppContainer.defaultMode(arguments: defaults, testing: false), .real(relayURL: AppContainer.configuredRelay()))
    }

    func testADeveloperCanLookAtTheAppWithoutOpeningTheRealSpace() {
        let defaults = UserDefaults(suiteName: "AppContainerTests")!
        defaults.set(true, forKey: "TomiIsolatedDemo")
        defer { defaults.removePersistentDomain(forName: "AppContainerTests") }
        guard case .demo = AppContainer.defaultMode(arguments: defaults, testing: false) else {
            return XCTFail("the isolated demo core: nothing of the space of this device is opened")
        }
    }

    func testTheAppThatHostsTheTestsStaysOutOfTheRealSpace() {
        XCTAssertTrue(AppContainer.isHostingTests)
        guard case let .demo(startPaired, assets) = AppContainer.defaultMode() else {
            return XCTFail("the host of the tests opens the isolated demo core: no network, nobody's space")
        }
        XCTAssertTrue(startPaired)
        let images = try? FileManager.default.contentsOfDirectory(atPath: assets?.path ?? "")
        XCTAssertTrue(images?.contains("demo_paysage.jpg") == true, "the Android demo assets are bundled")
    }

    func testADeveloperCanPointTheCoreAtALocalRelay() {
        let defaults = UserDefaults(suiteName: "AppContainerTests")!
        defaults.set("http://localhost:8080", forKey: "TomiRelayURL")
        defer { defaults.removePersistentDomain(forName: "AppContainerTests") }
        XCTAssertEqual(AppContainer.defaultMode(arguments: defaults, testing: false), .real(relayURL: URL(string: "http://localhost:8080")!))
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
