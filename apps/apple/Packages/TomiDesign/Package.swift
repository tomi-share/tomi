// swift-tools-version:5.9
// Tomi charter for SwiftUI: colours (light and dark), typography, icons, logo, base components.
// Values come from design/tomi/tokens.json through tools/tokens-to-swift.py (TomiTokens.swift).
// No dependency on the core: it builds and tests on macOS in seconds (`swift test`).
import PackageDescription

let package = Package(
    name: "TomiDesign",
    defaultLocalization: "fr",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "TomiDesign", targets: ["TomiDesign"]),
    ],
    targets: [
        .target(
            name: "TomiDesign",
            path: "Sources/TomiDesign",
            resources: [.copy("Resources/Fonts")]
        ),
        .testTarget(
            name: "TomiDesignTests",
            dependencies: ["TomiDesign"],
            path: "Tests/TomiDesignTests"
        ),
    ]
)
