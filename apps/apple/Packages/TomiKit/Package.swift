// swift-tools-version:5.9
// Model and pure logic of the Tomi apps: ports of the Android files (same names, same rules, same
// tests). Foundation, Observation and CryptoKit (the seal of the share inbox) only, no UI and no dependency on the
// core: it builds and tests on macOS in seconds (`swift test`).
import PackageDescription

let package = Package(
    name: "TomiKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "TomiKit", targets: ["TomiKit"]),
    ],
    targets: [
        .target(name: "TomiKit", path: "Sources/TomiKit"),
        .testTarget(name: "TomiKitTests", dependencies: ["TomiKit"], path: "Tests/TomiKitTests"),
    ]
)
