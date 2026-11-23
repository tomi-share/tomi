#!/usr/bin/env bash
# Builds tomi-core for iOS and macOS and packages it for Swift:
#   1. libtomi_core.a for the iOS device (arm64), the iOS simulator (arm64 + x86_64, merged with lipo)
#      and macOS (arm64 + x86_64, merged with lipo: Apple silicon first, Intel alongside);
#   2. the Swift bindings (tomi_core.swift + C header + module map) with the crate's own uniffi-bindgen;
#   3. apps/apple/Packages/TomiCore/tomi_coreFFI.xcframework, consumed by the SwiftPM package of the same folder.
#
# Prerequisites: rustup with the targets aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios,
# aarch64-apple-darwin, x86_64-apple-darwin (`rustup target add ...`) and Xcode (clang is used by `cc`
# for SQLite and ring).
# Usage: apps/apple/tools/build-core.sh [--debug] [--no-demo] [--platform=ios|macos|all]
#   --debug     Rust debug build (default: release, optimised).
#   --no-demo   leave the demo mode out (`openDemo` and `enterDemo` then answer an error, and the app
#               hides its demo entry). Never for a TestFlight / App Store archive: the released app
#               offers the demo.
#   --platform  which slices to build (default: all). `ios` or `macos` alone gives an XCFramework
#               that only links for that platform: quicker, for a CI job or a session on one platform.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PKG="$ROOT/apps/apple/Packages/TomiCore"
OUT="$ROOT/target/apple"
PROFILE=release
CARGO_PROFILE_FLAG=--release
FEATURES=ffi,demo
PLATFORM=all
for arg in "$@"; do
  case "$arg" in
    --debug) PROFILE=debug; CARGO_PROFILE_FLAG= ;;
    --no-demo) FEATURES=ffi ;;
    --platform=*) PLATFORM="${arg#--platform=}" ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

WITH_IOS=false
WITH_MACOS=false
case "$PLATFORM" in
  ios) WITH_IOS=true ;;
  macos) WITH_MACOS=true ;;
  all) WITH_IOS=true; WITH_MACOS=true ;;
  *) echo "unknown platform: $PLATFORM (ios, macos or all)" >&2; exit 2 ;;
esac

# Minimum OS versions, read by `cc` when compiling sqlite3.c and ring (keeps the .a consistent with
# what links it). macOS: the minimum of the SwiftPM packages (their tests run on the Mac), not the
# one of the Mac app, which is higher. It is only given to the macOS targets: exported, it would
# also reach the host builds (uniffi-bindgen, build scripts) and make cargo rebuild them whenever
# another build runs without it.
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
MACOS_MIN="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
unset MACOSX_DEPLOYMENT_TARGET

TARGETS=""
if $WITH_IOS; then TARGETS="aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios"; fi
if $WITH_MACOS; then TARGETS="$TARGETS aarch64-apple-darwin x86_64-apple-darwin"; fi

cd "$ROOT"
echo "==> Building tomi-core ($PROFILE, features $FEATURES) for: $TARGETS"
# `cargo rustc --crate-type staticlib`: Apple apps link a Rust library statically; the crate itself
# only declares lib + cdylib so that the other builds do not produce the archive.
for target in $TARGETS; do
  case "$target" in
    *-apple-darwin)
      MACOSX_DEPLOYMENT_TARGET="$MACOS_MIN" cargo rustc -p tomi-core --lib $CARGO_PROFILE_FLAG --locked \
        --features "$FEATURES" --target "$target" --crate-type staticlib ;;
    *)
      cargo rustc -p tomi-core --lib $CARGO_PROFILE_FLAG --locked --features "$FEATURES" \
        --target "$target" --crate-type staticlib ;;
  esac
done

rm -rf "$OUT"
mkdir -p "$OUT/include" "$OUT/swift"

# One library per platform variant: slices of the same variant are merged into a fat archive.
XCFRAMEWORK_ARGS=()
if $WITH_IOS; then
  mkdir -p "$OUT/device" "$OUT/simulator"
  cp "target/aarch64-apple-ios/$PROFILE/libtomi_core.a" "$OUT/device/libtomi_core.a"
  lipo -create \
    "target/aarch64-apple-ios-sim/$PROFILE/libtomi_core.a" \
    "target/x86_64-apple-ios/$PROFILE/libtomi_core.a" \
    -output "$OUT/simulator/libtomi_core.a"
  XCFRAMEWORK_ARGS+=(-library "$OUT/device/libtomi_core.a" -headers "$OUT/include")
  XCFRAMEWORK_ARGS+=(-library "$OUT/simulator/libtomi_core.a" -headers "$OUT/include")
fi
if $WITH_MACOS; then
  mkdir -p "$OUT/macos"
  lipo -create \
    "target/aarch64-apple-darwin/$PROFILE/libtomi_core.a" \
    "target/x86_64-apple-darwin/$PROFILE/libtomi_core.a" \
    -output "$OUT/macos/libtomi_core.a"
  XCFRAMEWORK_ARGS+=(-library "$OUT/macos/libtomi_core.a" -headers "$OUT/include")
fi

echo "==> Generating Swift bindings"
# `--library` reads the UniFFI metadata embedded in the archive: the bindings always match the built code.
# Any single-architecture archive carries the same metadata: the API has no platform-specific part.
if $WITH_IOS; then
  BINDINGS_SOURCE="target/aarch64-apple-ios/$PROFILE/libtomi_core.a"
else
  BINDINGS_SOURCE="target/aarch64-apple-darwin/$PROFILE/libtomi_core.a"
fi
cargo run -p tomi-core --release --locked --features ffi-bindgen --bin uniffi-bindgen -- \
  generate --library "$BINDINGS_SOURCE" --language swift --out-dir "$OUT/swift"

cp "$OUT/swift/tomi_coreFFI.h" "$OUT/include/"
# clang only picks up a module map named module.modulemap next to the headers.
cp "$OUT/swift/tomi_coreFFI.modulemap" "$OUT/include/module.modulemap"

echo "==> Assembling the XCFramework"
rm -rf "$PKG/tomi_coreFFI.xcframework"
xcodebuild -create-xcframework "${XCFRAMEWORK_ARGS[@]}" \
  -output "$PKG/tomi_coreFFI.xcframework" > /dev/null

mkdir -p "$PKG/Sources/TomiCore/Generated"
cp "$OUT/swift/tomi_core.swift" "$PKG/Sources/TomiCore/Generated/tomi_core.swift"

echo "==> Done ($PLATFORM): $PKG/tomi_coreFFI.xcframework and Sources/TomiCore/Generated/tomi_core.swift"
