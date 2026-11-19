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
| Windows, development | `windows-dev` | rolling pre-release, `tomi-windows-setup.exe` (unsigned) with `.sha256` and a build provenance attestation | manual run of `build-apps.yml` | **in place** |
| Android, stable | `android-vX.Y.Z` | non-prerelease: APK signed with the release key, `.sha256`, attestation, notes | push of the tag (`release-apps.yml`) | **in place**, not run yet ("Cutting a stable release") |
| Windows, stable | `windows-vX.Y.Z` | non-prerelease: NSIS installer (not code-signed yet), `.sha256`, attestation, notes | push of the tag (`release-apps.yml`) | **in place**, not run yet; needs the repository variable `ALLOW_UNSIGNED_WINDOWS_RELEASE` until the installer is signed |
| CLI | `cli-vX.Y.Z` | binaries | push of the tag | planned |
| iOS | none | distributed through TestFlight / App Store | | not applicable |

The `*-dev` releases are rolling: they are created on first use by the workflow and their assets are overwritten by
each build. They are always marked as pre-releases and are not stable releases.

## Rules

1. The tag must match the version stored in the component's own file. The relay workflow enforces this against the
   Cargo workspace version; `release-apps.yml` does the same for Android (`versionName` in Gradle, and `versionCode` must grow)
   and Windows (`tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`), with `.github/scripts/check-release-version.sh`.
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

## Cutting a stable release

Android and Windows, from `main` only. The repository must be public: the attestations that the release notes tell users to
verify exist only for public repositories.

1. Choose the version `X.Y.Z` of the product. They are independent: `android-v0.2.0` says nothing about Windows.
   - Android: set `versionName = "X.Y.Z"` **and raise `versionCode`** in `apps/android/app/build.gradle.kts`. Android refuses an
     update whose `versionCode` did not grow, so this is a one-way door: never lower it, never reuse it.
   - Windows: set the same `version` in `apps/windows/src-tauri/tauri.conf.json`, `apps/windows/package.json` and
     `apps/windows/src-tauri/Cargo.toml`.
2. Add the user-visible changes to `CHANGELOG.md`, commit, and get it onto `main` (rebase, never a merge commit).
3. Optional dry run, no tag needed: **Actions > Release apps > Run workflow**, with the tag you are about to create (for example
   `android-v0.2.0`). It runs every check and the build, and publishes nothing.
4. Tag the commit on `main` and push the tag:

   ```sh
   git switch main && git pull --ff-only
   git tag -a android-v0.2.0 -m "Tomi for Android 0.2.0"
   git push origin android-v0.2.0
   ```

5. The workflow stops, with a message, when: the tag is not `android-vX.Y.Z` or `windows-vX.Y.Z`; a version file disagrees with
   the tag or did not go up; the commit is not on `main`; the repository is private; for Windows, `ALLOW_UNSIGNED_WINDOWS_RELEASE`
   is not `true`; for Android, the release signing secrets are missing, or the APK is debuggable, debug-signed, unsigned or
   signed by another certificate; a release already exists for the tag; or an attestation does not verify.
6. Otherwise it creates a **draft** release with the files (`tomi-android-X.Y.Z.apk`, `tomi-windows-X.Y.Z-setup.exe`, each with its
   `.sha256`) and generated notes (protocol compatibility, path-scoped changes, verification commands, and for Android the
   certificate fingerprint), verifies the attestation of each file, and publishes. Then check it: `gh release view android-v0.2.0`,
   and run the verification commands of the notes on a downloaded copy.

A published tag is never reused, a stable release is never replaced with `--clobber`, and a mistake is fixed forward with the next
version. When you only want a build to try, run the rolling `*-dev` builds from **Actions > Build apps**.

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

## Integrity of published artifacts

Every artifact that `build-apps.yml` publishes (the Android APK, the Windows installer) comes with two things, and the relay
release assets (`release-relay.yml`) carry a checksum as well:

- a **SHA-256 checksum file**, `<asset>.sha256`, in `sha256sum` format. The publish job checks each file against its checksum
  again before uploading, so a corruption between the build job and the release is caught.
- a **build provenance attestation** (`actions/attest-build-provenance`, Sigstore, signed by GitHub): it states that this exact
  file (by digest) was produced by that workflow of this repository at that commit. It is created for public repositories only
  (attestations of private repositories need a paid GitHub plan); the step is skipped, with the build otherwise unchanged,
  while the repository is private. The Android APK is attested only when it is publishable (signed release build).

Limits to know about:

- A checksum next to a download protects against corruption, **not against a tampered release**: whoever can replace the file
  can replace its `.sha256` too. The attestation and, for Android, the signing certificate are the checks that count.
- Attestation says *where the file was built from*, not that the source is free of bugs or that nobody with write access
  changed the workflow. It is evidence for a user, not a substitute for reviewing the repository.
- The rolling `android-dev` and `windows-dev` pre-releases are **replaced by every run** (`gh release upload --clobber`). That is
  deliberate and limited to those two tags (the relay workflow does the same for its `relay-latest` pointer): a rolling channel
  is meant to be overwritten, the workflow refuses to touch a
  release that is not a pre-release, removes stale assets, and states in the release notes which commit and which workflow
  run produced the current files; each replacement has its own checksum and its own attestation. **Stable releases never use
  `--clobber`**: they are created once from a `*-vX.Y.Z` tag, a published tag is never reused (rule 1), and a fix is a new version.
  When the stable workflows exist, also enable GitHub's *immutable releases* setting so that published assets cannot be
  altered after the fact.
- In the Android job the signing secrets are in the environment of the Gradle step that builds and signs the APK, and that step
  also compiles the Rust dependencies. A compromised build-time dependency could therefore read them. What reduces this:
  dependency versions pinned by lockfiles and audited in CI (`security.yml`), actions pinned by commit SHA, and keeping the
  secrets in a protected Environment (see "Android release signing"). If a leak is suspected, treat the key as burned:
  generate a new one and say so in the release notes.

## Verifying a download

Windows installer (replace the repository if you use a fork):

```sh
# 1. checksum (against corruption; the .sha256 file is downloaded from the same release)
sha256sum --check tomi-windows-setup.exe.sha256
#    PowerShell: (Get-FileHash .\tomi-windows-setup.exe -Algorithm SHA256).Hash.ToLower()  -> compare with the file content

# 2. build provenance (needs the GitHub CLI, gh >= 2.49; the repository must be public)
gh attestation verify tomi-windows-setup.exe --repo tomi-share/tomi
#    stricter: also require that the signer is this workflow
gh attestation verify tomi-windows-setup.exe --repo tomi-share/tomi \
  --signer-workflow tomi-share/tomi/.github/workflows/build-apps.yml
```

A successful `gh attestation verify` prints the workflow, the commit and the run that built the file; any failure means you
should not install it. The Windows installer is **not code-signed yet**, so SmartScreen will warn: verifying the attestation
is the way to know what you are about to run (see the plan below).

Android APK:

```sh
sha256sum --check tomi-android-release.apk.sha256
gh attestation verify tomi-android-release.apk --repo tomi-share/tomi
# the signing certificate (Android build-tools) must be the project's one:
apksigner verify --print-certs tomi-android-release.apk      # compare "certificate SHA-256 digest" with the fingerprint below
```

Release certificate SHA-256 fingerprint: **not published yet** (the release key has not been created, see "Not done yet").
It will be added here, and the notes of every stable Android release carry it. Android refuses to update an app signed with another
key, so a build signed with a different certificate is not an update of this one.

Relay binaries: `sha256sum --check tomi-relay-<target>.sha256` as above.

## Windows code signing: plan

Not done, and not configured in this repository: nothing in the workflows pretends otherwise. SmartScreen currently warns for
the installer, which is why verification (above) is documented. The plan, in order:

1. **SignPath Foundation** (<https://signpath.org/>), which offers code signing for free to open-source projects. The usual
   conditions, to confirm on their site when applying: an OSI-approved license (Tomi is Apache-2.0), a public repository,
   releases built by a CI pipeline that SignPath can trace back to the source, and multi-factor authentication for the people
   who approve signing. In practice the Windows job would upload the unsigned installer as an artifact, a SignPath signing
   request would be submitted from the workflow (they provide a GitHub Action for this), and the signed file would then be
   what gets checksummed, attested and published. The certificate would name SignPath Foundation as publisher. Application,
   project configuration and the signing policy are owner tasks: they need an account and cannot be done from the repository.
2. If that does not work out, a commercial alternative such as Microsoft's Azure Trusted Signing (check current availability
   and eligibility), or an EV/OV certificate.
3. Later: the Tauri updater with a minisign public key embedded in `tauri.conf.json` and the private key kept in CI secrets only,
   so that installed copies update through a verified channel.

Even with a certificate, SmartScreen reputation builds up with downloads; a signed installer can still show a warning at first.

## Not done yet

- [ ] Create the Android release keystore and configure the `ANDROID_RELEASE_*` secrets (see "Android release signing").
      The workflow is ready for it; until it is done, CI publishes no APK at all (it builds an unsigned one as an artifact).
- [ ] Code signing for the Windows installer (SmartScreen currently warns). Until then a `windows-v*` release needs the
      repository variable `ALLOW_UNSIGNED_WINDOWS_RELEASE=true`, a conscious decision. Plan: "Windows code signing: plan".
- [x] Workflows for `android-v*` and `windows-v*` tags (`release-apps.yml`): version checks, build, draft, attestation
      check, publish, generated notes with the protocol compatibility. Written without a CI run available: the first dry run
      (Actions > Release apps > Run workflow) is its first test.
- [ ] `cli-v*` workflow and binaries, if the CLI is to be distributed.
- [ ] Minimum compatible versions in the notes (rule 4): the notes state the protocol and relay API version and the relay version at
      release time; a stricter "minimum relay version" needs a decision on what counts as incompatible.
- [ ] Decide the first public versions (Android and Windows are at 0.1.0; the crates are at the workspace version in `Cargo.toml`).
- [ ] Optional: repository variable and secret checklist for a new fork: `TOMI_RELAY_URL` and `ANDROID_RELEASE_CERT_SHA256`
      (variables), the `ANDROID_RELEASE_*` secrets and `FORGE_DEPLOY_URL` (secret); documented in this file and in `docs/DEV.md`.
