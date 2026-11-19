# Releases

Tomi is a monorepo with several independently shipped products. Each product has its own tag prefix and its own
version; there is no global version number. Client/relay compatibility is carried by the **protocol version**
(`format`, frozen vectors in `spec/vectors/vN/`), not by application versions.

## Tag scheme

| Product | Tag | GitHub release | Trigger | Status |
|---|---|---|---|---|
| Relay | `relay-vX.Y.Z` | Linux binaries | push of the tag (`release-relay.yml`) | **in place** |
| Relay (moving pointer) | `relay-latest` | same assets as the latest relay release | rewritten by the workflow; do not edit by hand | **in place** |
| Android, development | `android-dev` | rolling pre-release, release-signed APK `tomi-android-release.apk` (only when the release key is configured, see "Android release signing") | manual run of `build-apps.yml` | **in place** |
| Windows, development | `windows-dev` | rolling pre-release, `tomi-windows-setup.exe` (unsigned) | manual run of `build-apps.yml` | **in place** |
| Android, stable | `android-vX.Y.Z` | APK signed with the release key | push of the tag | planned (see below) |
| Windows, stable | `windows-vX.Y.Z` | signed installer | push of the tag | planned (see below) |
| CLI | `cli-vX.Y.Z` | binaries | push of the tag | planned |
| iOS | none | distributed through TestFlight / App Store | | not applicable |

The `*-dev` releases are rolling: they are created on first use by the workflow and their assets are overwritten by
each build. They are always marked as pre-releases and are not stable releases.

## Rules

1. The tag must match the version stored in the component's own file. The relay workflow already enforces this against the
   Cargo workspace version; the Android (`versionName` in Gradle) and Windows (`tauri.conf.json`) workflows must do the same.
   A pushed tag is never reused: fix forward with a new version.
2. The Cargo workspace version covers the Rust crates (relay, core, CLI). Android and Windows have their own version numbers.
3. Release notes are scoped by path, because `crates/core` is shared:
   `git log <previous-tag>..<tag> --oneline -- apps/relay crates/core spec/` for the relay, and the matching app directory
   plus `crates/core` for a client. A change in the core therefore shows up in every product it affects.
4. Each release states the protocol version it speaks and the minimum compatible relay or client version.
5. Release notes contain no secret and no sensitive detail. For a relay release, keep the original description (binaries,
   pointer to `infra/`) and append the changelog. Do not edit the description of `relay-latest`.
6. Relay release: bump the workspace version in `Cargo.toml`, push `relay-vX.Y.Z`, then check the optional deploy hook step
   (see `infra/forge/` and the `FORGE_DEPLOY_URL` secret) without assuming the deployment has finished.

## Android release signing

What CI publishes for Android is **only** a non-debuggable `release` build signed with the project's dedicated release key.
A debug build is debuggable (anyone with USB debugging authorised can read the app's database and use its Keystore key
through `run-as` or JDWP), and a build signed with a debug key has no safe update path. Neither is ever published.

### What the workflow does

`.github/workflows/build-apps.yml` (manual run; input `android_variant`, default `release`):

| Situation | Artifact (workflow artifact `android-apk`) | Published to `android-dev`? |
|---|---|---|
| `release`, the four secrets below are set | `tomi-android-release.apk`, signed with the release key | **Yes**, after the APK check |
| `release`, no secret (a fork, a contributor) | `tomi-android-release-UNSIGNED.apk`, does not install | No |
| `release`, only some of the secrets | the run fails | No |
| `debug` | `tomi-android-debug-DEBUG-ONLY.apk`, debuggable, debug-signed | No |

The `preview` Gradle variant (release code signed with the debug key, for judging speed on a developer's phone) is not built
by CI and must never be distributed.

The APK check, `.github/scripts/verify-android-apk.sh`, runs twice on a publishable APK: right after the build, and again in the
publish job on the file that is about to be uploaded. It fails the run if the APK is debuggable (`aapt2 dump badging` and the
manifest attribute), is not signed (`apksigner verify`), has more than one signer, is signed with an Android debug certificate,
or is signed by a certificate other than the expected one. The expected certificate is the repository variable
`ANDROID_RELEASE_CERT_SHA256` if set, otherwise the certificate of the keystore used for the build. It fails closed: a missing
tool or an output it cannot parse is a failure.

### Secrets and variables to configure

Repository secrets (Settings > Secrets and variables > Actions). Set **all four or none**:

| Name | Content |
|---|---|
| `ANDROID_RELEASE_KEYSTORE_B64` | the release keystore file, base64-encoded |
| `ANDROID_RELEASE_KEYSTORE_PASSWORD` | the keystore password |
| `ANDROID_RELEASE_KEY_ALIAS` | the key alias in the keystore |
| `ANDROID_RELEASE_KEY_PASSWORD` | the key password (the same as the keystore password for a PKCS12 keystore) |

Repository variable (public, not a secret), recommended: `ANDROID_RELEASE_CERT_SHA256`, the SHA-256 fingerprint of the release
certificate (see below). With it, a mistake or a tampered secret that signs with another key makes the run fail.

The old secret `ANDROID_DEBUG_KEYSTORE_B64` is no longer used; delete it.

### Creating the keystore (once, on a trusted machine, never in the repository)

```sh
keytool -genkeypair -v -storetype PKCS12 -keystore tomi-release.jks -alias tomi \
  -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Tomi release signing"
```

Choose one strong password; PKCS12 uses it for both the store and the key. Then:

```sh
# the secrets (GitHub CLI; secrets are stored encrypted by GitHub and cannot be read back)
base64 -w0 tomi-release.jks | gh secret set ANDROID_RELEASE_KEYSTORE_B64      # PowerShell: [Convert]::ToBase64String([IO.File]::ReadAllBytes("tomi-release.jks"))
gh secret set ANDROID_RELEASE_KEYSTORE_PASSWORD
gh secret set ANDROID_RELEASE_KEY_ALIAS      # tomi
gh secret set ANDROID_RELEASE_KEY_PASSWORD   # the same password

# the public fingerprint, to pin
keytool -list -v -keystore tomi-release.jks -alias tomi | grep 'SHA256:'
gh variable set ANDROID_RELEASE_CERT_SHA256   # paste the value, with or without colons
```

Keep **offline backups** of the keystore and its password in two separate places: if the key is lost, installed apps can never be
updated (Android refuses an update signed by another key), and users would have to uninstall and pair again. Never commit it
(`.gitignore` already excludes `*.jks` and `*.keystore`, hence the file name above). When the first release is published, also publish the
certificate fingerprint in the release notes so that users can compare it (see "Verifying a download" below).

Hardening, recommended: put the four secrets in a GitHub **Environment** (for example `release`) restricted to the `main`
branch, so that a workflow file changed on another branch cannot read them, and reference it with `environment: release` on the
`android` job. Anyone who can push a workflow to a branch of the repository can otherwise exfiltrate repository secrets.

### Building a release by hand

The Gradle build reads `TOMI_RELEASE_KEYSTORE`, `TOMI_RELEASE_KEYSTORE_PASSWORD`, `TOMI_RELEASE_KEY_ALIAS` and
`TOMI_RELEASE_KEY_PASSWORD`, as environment variables or Gradle properties (`~/.gradle/gradle.properties` keeps them out of the
repository). Without `TOMI_RELEASE_KEYSTORE`, `assembleRelease` produces an unsigned APK. `-PtomiApplicationId=...` (forks, see
[FORKING.md](FORKING.md)) works with all of this; a fork uses its own keystore, secrets and fingerprint.

## Not done yet

- [ ] Create the Android release keystore and configure the `ANDROID_RELEASE_*` secrets (see "Android release signing").
      The workflow is ready for it; until it is done, CI publishes no APK at all (it builds an unsigned one as an artifact).
- [ ] Code signing for the Windows installer (SmartScreen currently warns), before creating `windows-v*` tags.
- [ ] Workflows for `android-v*` and `windows-v*` tags: build, check that the tag matches the version file, publish a
      non-prerelease with generated notes.
- [ ] `cli-v*` workflow and binaries, if the CLI is to be distributed.
- [ ] A "minimum compatible versions" table in each release's notes (rule 4), possibly generated from the version files.
- [ ] Decide the first public version (the crates are currently at the workspace version in `Cargo.toml`).
- [ ] Optional: repository variable and secret checklist for a new fork: `TOMI_RELAY_URL` and `ANDROID_RELEASE_CERT_SHA256`
      (variables), the `ANDROID_RELEASE_*` secrets and `FORGE_DEPLOY_URL` (secret); documented in this file and in `docs/DEV.md`.
