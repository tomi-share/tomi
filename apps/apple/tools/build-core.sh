#!/usr/bin/env bash
# Builds tomi-core for iOS and packages it for Swift:
#   1. libtomi_core.a for the device (arm64) and the simulator (arm64 + x86_64, merged with lipo);
#   2. the Swift bindings (tomi_core.swift + C header + module map) with the crate's own uniffi-bindgen;
#   3. apps/apple/Packages/TomiCore/tomi_coreFFI.xcframework, consumed by the SwiftPM package of the same folder.
#
# Prerequisites: rustup with the targets aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios
# (`rustup target add ...`) and Xcode (clang is used by `cc` for SQLite and ring).
# Usage: apps/apple/tools/build-core.sh [--debug] [--no-demo]
#   --debug    Rust debug build (default: release, optimised).
#   --no-demo  leave the demo mode out (`openDemo` and `enterDemo` then answer an error, and the app
#              hides its demo entry). Never for a TestFlight / App Store archive: the released app
#              offers the demo.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PKG="$ROOT/apps/apple/Packages/TomiCore"
OUT="$ROOT/target/ios"
PROFILE=release
CARGO_PROFILE_FLAG=--release
FEATURES=ffi,demo
for arg in "$@"; do
  case "$arg" in
    --debug) PROFILE=debug; CARGO_PROFILE_FLAG= ;;
    --no-demo) FEATURES=ffi ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

# Minimum iOS version, read by `cc` when compiling sqlite3.c and ring (keeps the .a consistent with the app).
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"

cd "$ROOT"
echo "==> Building tomi-core ($PROFILE, features $FEATURES) for iOS targets"
# `cargo rustc --crate-type staticlib`: iOS links a Rust library statically; the crate itself
# only declares lib + cdylib so that the other builds do not produce the archive.
for target in aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
  cargo rustc -p tomi-core --lib $CARGO_PROFILE_FLAG --locked --features "$FEATURES" \
    --target "$target" --crate-type staticlib
done

rm -rf "$OUT"
mkdir -p "$OUT/device" "$OUT/simulator" "$OUT/include" "$OUT/swift"

cp "target/aarch64-apple-ios/$PROFILE/libtomi_core.a" "$OUT/device/libtomi_core.a"
# One library per platform variant: the two simulator slices are merged into a fat archive.
lipo -create \
  "target/aarch64-apple-ios-sim/$PROFILE/libtomi_core.a" \
  "target/x86_64-apple-ios/$PROFILE/libtomi_core.a" \
  -output "$OUT/simulator/libtomi_core.a"

echo "==> Generating Swift bindings"
# `--library` reads the UniFFI metadata embedded in the archive: the bindings always match the built code.
cargo run -p tomi-core --release --locked --features ffi-bindgen --bin uniffi-bindgen -- \
  generate --library "$OUT/device/libtomi_core.a" --language swift --out-dir "$OUT/swift"

cp "$OUT/swift/tomi_coreFFI.h" "$OUT/include/"
# clang only picks up a module map named module.modulemap next to the headers.
cp "$OUT/swift/tomi_coreFFI.modulemap" "$OUT/include/module.modulemap"

echo "==> Assembling the XCFramework"
rm -rf "$PKG/tomi_coreFFI.xcframework"
xcodebuild -create-xcframework \
  -library "$OUT/device/libtomi_core.a" -headers "$OUT/include" \
  -library "$OUT/simulator/libtomi_core.a" -headers "$OUT/include" \
  -output "$PKG/tomi_coreFFI.xcframework" > /dev/null

mkdir -p "$PKG/Sources/TomiCore/Generated"
cp "$OUT/swift/tomi_core.swift" "$PKG/Sources/TomiCore/Generated/tomi_core.swift"

echo "==> Done: $PKG/tomi_coreFFI.xcframework and Sources/TomiCore/Generated/tomi_core.swift"
