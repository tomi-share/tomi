# Tomi — single entry point for the monorepo
# Usage: just <recipe>        (`just --list` lists the recipes)
#
# On Windows, recipes run in PowerShell; elsewhere, in `sh`.
# Each line of a recipe is a separate invocation: change directory with the
# [working-directory] attribute, not with `cd`.

set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# Gradle wrapper: gradlew.bat on Windows, gradlew elsewhere.
gradlew := if os_family() == "windows" { '.\gradlew.bat' } else { './gradlew' }

default:
    @{{ just_executable() }} --list

# --- Global checks (same as CI) ---

# Everything CI checks on the Rust side: formatting, clippy, tests.
check: fmt-check lint test

# --- Core & relay (Rust) ---

build:
    cargo build --workspace

test:
    cargo test --workspace
    cargo test -p tomi-core --features demo

# Requires crates/core/tests/spec_vectors.rs.
test-vectors:
    cargo test -p tomi-core --test spec_vectors

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p tomi-core --all-targets --features demo -- -D warnings

dev-relay:
    cargo run -p tomi-relay

# --- Android ---
# Prerequisites: ANDROID_HOME, ANDROID_NDK_HOME (NDK 27), `cargo install cargo-ndk`,
# and the Rust targets: rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

android-ndk-libs:
    cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 -o apps/android/app/src/main/jniLibs build --release -p tomi-core --features demo

[working-directory('apps/android')]
android-build:
    {{ gradlew }} assembleDebug

[working-directory('apps/android')]
android-install:
    {{ gradlew }} installDebug

# JVM unit tests.
[working-directory('apps/android')]
android-test:
    {{ gradlew }} testDebugUnitTest

[working-directory('apps/android')]
android-lint:
    {{ gradlew }} lintDebug

# --- Apple (iOS, macOS) ---
# Prerequisites: rustup with the aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios, aarch64-apple-darwin and
# x86_64-apple-darwin targets, and Xcode.
# See apps/apple/README.md, docs/archive/apple-spec-implementation.md and docs/macos-app.md.

# Builds tomi-core for iOS and macOS (Apple silicon + Intel), generates the Swift bindings and assembles
# apps/apple/Packages/TomiCore/tomi_coreFFI.xcframework. `--platform=ios` or `--platform=macos`: one platform only.
# Demo mode is included, also for a TestFlight / App Store archive (the published app offers the demo).
apple-core *ARGS:
    apps/apple/tools/build-core.sh {{ ARGS }}

# Tests for the TomiCore Swift package (bindings + core) on the Mac, then on the iPhone Duo simulator.
[working-directory('apps/apple/Packages/TomiCore')]
apple-core-test:
    swift test
    xcodebuild test -scheme TomiCore -destination "platform=iOS Simulator,id=$({{ justfile_directory() }}/apps/apple/tools/simulator-udid.sh)" -quiet

# Regenerates the derived files from their sources of truth (design tokens, Android strings) and the Xcode project.
[working-directory('apps/apple')]
apple-project:
    python3 -I tools/tokens-to-swift.py
    python3 -I tools/strings-from-android.py
    xcodegen generate

# All Apple tests: packages on macOS (the runtime too, on the macOS slice of the core), then runtime and app on the
# iPhone Duo simulator.
[working-directory('apps/apple')]
apple-test: apple-project
    swift test --package-path Packages/TomiKit
    swift test --package-path Packages/TomiDesign
    swift test --package-path Packages/TomiRuntime
    cd Packages/TomiRuntime && xcodebuild test -scheme TomiRuntime -destination "platform=iOS Simulator,id=$(../../tools/simulator-udid.sh)" -quiet
    xcodebuild test -project Tomi.xcodeproj -scheme Tomi -destination "platform=iOS Simulator,id=$(tools/simulator-udid.sh)" -quiet

# --- Windows (Tauri) ---
# Prerequisites: cargo install tauri-cli --locked

[working-directory('apps/windows')]
windows-dev:
    cargo tauri dev

[working-directory('apps/windows')]
windows-build:
    cargo tauri build

# --- Infra ---

# Deployment runs from a workstation to the EC2 host: the step-by-step procedure is in infra/README.md.
deploy-relay:
    @echo "See infra/README.md (build through GitHub Actions: tag relay-vX.Y.Z, then install.sh / update.sh on the host)"
