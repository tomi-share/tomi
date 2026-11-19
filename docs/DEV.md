# Tomi — Development guide (for contributors)

Read this first. Product context: `PROJECT.md`, then `docs/ux-flows.md`.
When these documents disagree, **`PROJECT.md` (sections 5, 6, 7) and this guide are authoritative**;
the other documents describe intent and use cases.

## Environment

- Repository root: your local clone (for example `<repo>`), on Windows or any other platform. The commands below use PowerShell on Windows.
- Rust 1.99 (MSVC). In a new PowerShell session, refresh the PATH before calling `cargo`:
  `$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH","Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH","User")`
- Android: SDK in `%LOCALAPPDATA%\Android\sdk` (`ANDROID_HOME`), platform 37.0 (`platforms;android-37.0`), build-tools 36.0.0, NDK 27, JDK 21 (Gradle 9.9.1 and AGP 9.5.1 through the wrapper;
  the SDK platform and build-tools are downloaded by AGP when missing).
  `minSdk 33`, `compileSdk 37`, `targetSdk 36`. Physical test device: any Android 16 phone.
- Not installed by default: `gradle` (use the wrapper), `just`. Do not assume a tool is missing or present; check.
- App identifier: `eu.shaffe.tomi`. Relay: configurable; no relay address is compiled into the Android app by default
  (`-PtomiRelayUrl=https://relay.example.org`, or `~/.gradle/gradle.properties` to keep it out of the repository;
  in CI, repository variable `TOMI_RELAY_URL`). Without it, the address comes from the invite link/QR code, or from the « Relais » (Relay) field.

## Conventions

- **Code, identifiers, comments, and technical error messages: English.**
  **Documentation: English. User-facing UI strings: French** (the default locale; keep them externalized).
- No `unsafe` without a written justification. No `unwrap()` / `expect()` outside tests.
- Dependencies: well-known, maintained crates/libraries, with versions pinned by `Cargo.lock` / Gradle catalogs.
  Flag any unusual package name. Never write a cryptographic primitive yourself.
- Formatting: `cargo fmt` and `cargo clippy -- -D warnings` must be clean. Kotlin: official style.

## Code areas

| Area | Contents |
|------|----------|
| `spec/`, `crates/core/` | Protocol specification, crypto, storage, network runtime (Rust) |
| `apps/relay/` | Blind store-and-forward relay server |
| `crates/cli/` | Command-line client (on top of `core`) |
| `apps/android/` | Android app (Kotlin / Compose) |
| `apps/windows/` | Windows app (Tauri + Svelte) |
| `apps/apple/` | iOS app (SwiftUI) |
| `infra/`, `.github/workflows/`, `justfile` | Deployment, CI, and task runner |
| `docs/`, `PROJECT.md`, root `Cargo.toml` | Documentation and workspace configuration |

Keep changes within the area you are working on. If a change is needed elsewhere (for example, a type
that `core` must expose), describe it in your pull request instead of making it silently.
Add new documentation as new files in `docs/`; do not rewrite existing documents without a reason.

## Git

Commit in coherent, separate batches. Check what is staged before committing, and never commit secrets,
generated artifacts (`target/`, `build/`, `.gradle/`, `node_modules/`, APKs, keys), or code that does not
build or has not been reviewed.

## Security invariants (non-negotiable)

1. **The relay holds no keys and never sees plaintext.** It only knows:
   `item_id`, the sending `device_id`, `created_at`, `expires_at`, the encrypted size, `key_epoch`, `seq`.
   Type, secret flag, file names, titles, previews: all of it is **inside** the ciphertext.
   *Note:* the relay also knows, because operation requires it (access control, versioning, device activity
   notifications) and without revealing content: the device name, platform, `app_version`,
   `format_min`/`format_max`, each device's push token, its activity/presence dates, and delivery/read
   receipts. None of this reveals the type, content, or secret status of an item. See `spec/api-v1.md` §1.2.
2. **No automatic clipboard reads.** The clipboard is read only when the user explicitly pastes
   (`Ctrl+V` or a « Coller » (Paste) button).
3. **Never put secrets in logs, URLs, notifications, or FCM.** FCM messages are *data messages* with
   **constant content** (`{"w":"1"}`, wake-up only, never any data tied to an item).
4. **Secret content is shown in plaintext only on explicit user action**, and the clipboard is marked
   sensitive (`ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory = 0`
   / `CanUploadToCloudClipboard = 0` on Windows; `ClipDescription.EXTRA_IS_SENSITIVE` on Android),
   then cleared after 30 s if it still contains the value.
5. **`tomi-share://` URLs only pre-fill fields.** A URL never triggers sending, deletion, revocation,
   or a settings change; no secret ever goes in a URL.
6. **Received HTML** is rendered in a sandboxed context, without JavaScript and without network access
   (`default-src 'none'; img-src data:`), after sanitization.
7. **A decryption error never deletes an item**: it displays the item as « illisible » (unreadable).

## Definition of done

- The code compiles, `cargo fmt` and `clippy` are clean (Rust), and the tests for the affected area pass.
- You have **run** what you claim. If something could not be verified (physical device, network,
  signature), say so explicitly; do not assume it.
- Your final report is **short** and states: files created or modified, commands run with their results,
  what was not verified, deviations from the documents (and why), open questions, and any changes
  needed in other areas.
- Do not invent product decisions: if a point is missing or contradictory, apply the simplest and
  safest option and flag it in the report.

## Release builds in CI (Android, Windows)

Workflow `.github/workflows/build-apps.yml`, **manual trigger only** (Actions tab, or
`gh workflow run build-apps.yml -f android_variant=release`). It builds the Android APK (`release`, the default, or `debug`)
and the Windows NSIS installer, then uploads them to the pre-releases `android-dev` and
`windows-dev` (created on first use; `publish=false`: artifacts only). macOS/iOS builds are not run in CI, because macOS runners are more expensive.
Tag naming, stable releases and changelog rules: see [RELEASES.md](RELEASES.md).

- Quota: private repositories on the free plan get a limited monthly CI minute allowance, and Windows runners consume minutes at twice the normal rate.
- Android: only a non-debuggable `release` APK signed with the release key (secrets `ANDROID_RELEASE_KEYSTORE_B64`,
  `ANDROID_RELEASE_KEYSTORE_PASSWORD`, `ANDROID_RELEASE_KEY_ALIAS`, `ANDROID_RELEASE_KEY_PASSWORD`) is ever published, and a check
  (`.github/scripts/verify-android-apk.sh`) fails the run on a debuggable or debug-signed APK. Without the secrets (forks) the
  workflow still builds, but produces a clearly named UNSIGNED or DEBUG-ONLY APK as a workflow artifact, never a release asset.
  Setup, keystore creation and verification: [RELEASES.md](RELEASES.md). `./gradlew assemblePreview` (release code, debug key) stays
  a local tool for judging speed; it is never distributed.
- The Windows installer is not code-signed (a SmartScreen warning is expected). Every published artifact has a `.sha256` and a build provenance attestation (`gh attestation verify`); see [RELEASES.md](RELEASES.md#verifying-a-download) and the code signing plan.
- Relay address for CI builds: the repository variable TOMI_RELAY_URL (see Environment).
- Optional secret FORGE_DEPLOY_URL: URL of a deploy hook called after a relay release is published (see infra/forge/); the step does nothing when the secret is absent.
- Android: `libtomi_core.so` is built with `--features demo` (entering the demo from "Join your space"), in two places that must stay in agreement: the Gradle task `buildTomiCoreLibs` (`app/build.gradle.kts`) and the `cargo ndk` pre-step of `ci.yml` (same in the `just android-ndk-libs` recipe).
- Windows app CI (`ci.yml`, job `windows-app`, on `windows-latest`, on every push/PR touching `apps/windows/`, `crates/core/` or `ci.yml`): `npm ci`, `npm run check`, `npm run build` in `apps/windows`, then `cargo test --locked` in `apps/windows/src-tauri`. No clippy and no installer (the installer stays in `build-apps.yml`). Counts double against the quota.
