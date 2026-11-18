// swift-tools-version:5.9
// Swift package wrapping the Rust core (tomi-core) for the iOS app.
// Both `tomi_coreFFI.xcframework` and `Sources/TomiCore/Generated/tomi_core.swift` are produced by
// apps/apple/tools/build-core.sh (`just apple-core`) and are not committed.
import PackageDescription

let package = Package(
    name: "TomiCore",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "TomiCore", targets: ["TomiCore"]),
    ],
    targets: [
        // Static Rust library + C header + module map, one slice for devices and one for simulators.
        .binaryTarget(name: "tomi_coreFFI", path: "tomi_coreFFI.xcframework"),
        // UniFFI-generated Swift API (`import tomi_coreFFI` inside), re-exported as the `TomiCore` module.
        .target(
            name: "TomiCore",
            dependencies: ["tomi_coreFFI"],
            path: "Sources/TomiCore"
        ),
        .testTarget(
            name: "TomiCoreTests",
            dependencies: ["TomiCore"],
            path: "Tests/TomiCoreTests"
        ),
    ]
)
