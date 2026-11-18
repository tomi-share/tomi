// swift-tools-version:5.9
// Everything that touches the Rust core: the adapter of `TomiKit.TomiCore` over `RustTomiCore`
// (real or demo), the wrapping key in the Keychain, the storage paths. Shared by the app and,
// later, its share extension. Tests run on the iOS simulator (the core is an iOS XCFramework).
import PackageDescription

let package = Package(
    name: "TomiRuntime",
    platforms: [.iOS(.v17)],
    products: [
        .library(name: "TomiRuntime", targets: ["TomiRuntime"]),
    ],
    dependencies: [
        .package(path: "../TomiCore"),
        .package(path: "../TomiKit"),
    ],
    targets: [
        .target(
            name: "TomiRuntime",
            dependencies: [
                .product(name: "TomiCore", package: "TomiCore"),
                .product(name: "TomiKit", package: "TomiKit"),
            ],
            path: "Sources/TomiRuntime"
        ),
        .testTarget(name: "TomiRuntimeTests", dependencies: ["TomiRuntime"], path: "Tests/TomiRuntimeTests"),
    ]
)
