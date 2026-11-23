import Foundation
import TomiCore
import TomiKit

/// Which core the app runs on.
public enum CoreMode: Sendable, Equatable {
    /// The real core. `relayURL` is the relay of a device that has none of its own yet (nil in a build without a default
    /// relay: it then comes from the link or QR code of the pairing, the "Relais" field or the creation link). `https://`
    /// only, except `http://localhost` when the Rust core itself is a debug build (`just apple-core --debug`), whatever
    /// the app configuration.
    case real(relayURL: URL?)
    /// The demo core: no network, the demo space of every Tomi app. `assets` holds the bundled
    /// demo files (images, code files); built-in stand-ins replace the missing ones.
    case demo(startPaired: Bool, assets: URL?)
}

/// What the real core needs to enter the demo (`RustTomiCoreAdapter.enterDemo`).
public struct DemoEntry: Sendable {
    public let filesDir: String
    public let assetsDir: String?
    public let appVersion: String
    public let deviceName: String

    public init(filesDir: String, assetsDir: String?, appVersion: String, deviceName: String) {
        self.filesDir = filesDir
        self.assetsDir = assetsDir
        self.appVersion = appVersion
        self.deviceName = deviceName
    }
}

public enum CoreFactory {
    /// What this build announces to the space as its platform (`CoreConfig.platform`): the other
    /// devices show it in their device list.
    public static var platform: String {
        #if os(macOS)
        "macos"
        #else
        "ios"
        #endif
    }

    /// Opens the Rust core. Blocks (database, and for the demo the writing of its history):
    /// never call it on the main thread; prefer `open`.
    public static func openRust(
        mode: CoreMode,
        paths: CorePaths,
        appVersion: String,
        deviceName: String,
        keyStore: WrapKeyStoring = KeychainWrapKeyStore()
    ) throws -> RustTomiCore {
        do {
            switch mode {
            case let .real(relayURL):
                let exists = FileManager.default.fileExists(atPath: paths.database.path)
                let key = try WrapKey.loadOrCreate(store: keyStore, databaseExists: exists)
                return try RustTomiCore.open(config: CoreConfig(
                    dbPath: paths.database.path,
                    filesDir: paths.files.path,
                    wrapKey: key,
                    relayUrl: relayURL?.absoluteString,
                    appVersion: appVersion,
                    platform: platform,
                    // The pairing screen asks "get the history?" itself.
                    autoImport: false
                ))
            case let .demo(startPaired, assets):
                return try RustTomiCore.openDemo(config: DemoConfig(
                    filesDir: paths.demo.path,
                    assetsDir: assets?.path,
                    platform: platform,
                    appVersion: appVersion,
                    deviceName: deviceName,
                    startPaired: startPaired
                ))
            }
        } catch let error as TomiError {
            throw CoreMapping.error(error)
        }
    }

    /// Opens the core off the main thread and wraps it for the UI.
    /// `demoAssets`: the bundled demo files, for a real core that may enter the demo.
    @MainActor
    public static func open(
        mode: CoreMode,
        paths: CorePaths,
        appVersion: String,
        deviceName: String,
        keyStore: WrapKeyStoring = KeychainWrapKeyStore(),
        demoAssets: URL? = nil
    ) async throws -> RustTomiCoreAdapter {
        let core = try await Task.detached(priority: .userInitiated) {
            try openRust(mode: mode, paths: paths, appVersion: appVersion, deviceName: deviceName, keyStore: keyStore)
        }.value
        var entry: DemoEntry?
        if case .real = mode {
            entry = DemoEntry(filesDir: paths.demo.path, assetsDir: demoAssets?.path, appVersion: appVersion, deviceName: deviceName)
        }
        return RustTomiCoreAdapter(core: core, deviceName: deviceName, demoEntry: entry)
    }
}
