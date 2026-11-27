# Releases

Tomi is a monorepo. The **apps built by CI (Android and Windows) share one version number and ship together in one GitHub
release**, tagged `vX.Y.Z`; the relay has its own tag prefix and its own version. macOS is set aside (it cannot be signed and
notarised in CI without a paid Apple Developer account), and iOS goes through the stores, so neither is in the GitHub release.
Client/relay compatibility is carried by the **protocol version** (`format`, frozen vectors in `spec/vectors/vN/`), not by
application versions.

## Tag scheme

| Product | Tag | GitHub release | Trigger | Status |
|---|---|---|---|---|
| Relay | `relay-vX.Y.Z` | Linux binaries | push of the tag (`release-relay.yml`) | **in place** |
| Android, development | `android-dev-<run>` | one immutable pre-release per run, release-signed APK `tomi-android-release.apk` (only when the release key is configured, see "Android release signing") | manual run of `build-apps.yml` | **in place** |
| Windows, development | `windows-dev-<run>` | one immutable pre-release per run, `tomi-windows-setup.exe` (unsigned) with `.sha256` and a build provenance attestation | manual run of `build-apps.yml` | **in place** |
| Android and Windows, stable | `vX.Y.Z` | non-prerelease, **the repository's "Latest"**: APK signed with the release key and NSIS installer (not code-signed yet), each with `.sha256` and attestation, one set of notes | push of the tag (`release-apps.yml`) | **in place**, not run yet ("Cutting a stable release"); needs the repository variable `ALLOW_UNSIGNED_WINDOWS_RELEASE` until the installer is signed |
| CLI | `cli-vX.Y.Z` | binaries | push of the tag | planned |
| iOS | none | distributed through TestFlight / App Store | | not applicable |

The `*-dev-<run>` releases are development builds, one per run of the workflow (`<run>` is the run number, with `-rN` appended
on a re-run). Each is created as a draft, filled, then published, and from then on it is immutable like every release of this
repository. The workflow keeps the last three of each product and deletes the older ones with their tag (deleting is allowed;
reusing a tag is not, and the run number never repeats). They are always marked as pre-releases and are not stable releases.
To fetch the newest one: `gh release list --limit 5`, or `gh release download android-dev-<run>`. The former rolling
`android-dev` tag is a dead name: GitHub never lets the tag of an immutable release be reused, so it is left
as it is (or deleted) and never written to again.

## Rules

1. The tag must match the version stored in the component's own files. The relay workflow enforces this against the
   Cargo workspace version; `release-apps.yml` does the same for Android (`versionName` in Gradle, and `versionCode` must grow)
   **and** Windows (`tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`) at once, with
   `.github/scripts/check-release-version.sh`: both must equal the tag. A pushed tag is never reused: fix forward with a new version.
2. The Cargo workspace version covers the Rust crates (relay, core, CLI). Android and Windows share the app version `X.Y.Z`
   (version 0.1.0 was published as `android-v0.1.0` and `windows-v0.1.0`; the grouped releases start at the next version). The Apple
   app's `MARKETING_VERSION` (`apps/apple/Config/Base.xcconfig`) is not checked: keep it at the same number by hand.
3. Release notes are scoped by path, because `crates/core` is shared:
   `git log <previous-tag>..<tag> --oneline -- apps/relay crates/core spec/` for the relay, and `apps/android`, `apps/windows`,
   `crates/core` and `spec` for an app release. A change in the core therefore shows up in every product it affects.
4. Each release states the protocol version it speaks and the minimum compatible relay or client version.
5. Release notes contain no secret and no sensitive detail. For a relay release, keep the original description (binaries,
   pointer to `infra/`) and append the changelog. There is no moving `relay-latest` pointer: releases are immutable on this repository, so assets of a published release cannot be replaced. The deployment script looks up the newest `relay-v*` release itself.
6. **GitHub's "Latest" release is the app release `vX.Y.Z`, and only that.** `release-apps.yml` creates it with `--latest`; the
   relay releases and the development builds are created with `--latest=false`, so that "Latest" always means "the newest Tomi
   app" and never a relay or a test build. (`releases/latest` is therefore usable for the apps, e.g.
   `https://github.com/tomi-share/tomi/releases/latest`; it is not for the relay, which is found by its tag prefix:
   `gh release list --json tagName,isDraft --jq '[.[] | select((.isDraft|not) and (.tagName|startswith("relay-v")))][0].tagName'`,
   as `infra/forge/deploy.sh` does.) Until the first `vX.Y.Z` is published the badge stays on `android-v0.1.0`, which GitHub
   cannot unset; the new release replaces it.
7. Relay release: bump the workspace version in `Cargo.toml`, push `relay-vX.Y.Z`, then check the optional deploy hook step
   (see `infra/forge/` and the `FORGE_DEPLOY_URL` secret) without assuming the deployment has finished.

## Cutting a stable release

Android and Windows together, from `main` only. The repository must be public: the attestations that the release notes tell users
to verify exist only for public repositories.

1. Choose the version `X.Y.Z`. It is the same for both apps, and a release always contains both, even if only one changed.
   - Android: set `versionName = "X.Y.Z"` **and raise `versionCode`** in `apps/android/app/build.gradle.kts`. Android refuses an
     update whose `versionCode` did not grow, so this is a one-way door: never lower it, never reuse it.
   - Windows: set the same `version` in `apps/windows/src-tauri/tauri.conf.json`, `apps/windows/package.json` and
     `apps/windows/src-tauri/Cargo.toml`.
   - Apple: set `MARKETING_VERSION` to the same number in `apps/apple/Config/Base.xcconfig` (and raise `CURRENT_PROJECT_VERSION`).
     Nothing checks it, since the release does not carry the Apple apps.
2. Add the user-visible changes to `CHANGELOG.md`, commit, and get it onto `main` (rebase, never a merge commit).
3. Optional dry run, no tag needed: **Actions > Release apps > Run workflow**, with the tag you are about to create (for example
   `v0.2.0`). It runs every check and both builds, and publishes nothing.
4. Tag the commit on `main` and push the tag:

   ```sh
   git switch main && git pull --ff-only
   git tag -a v0.2.0 -m "Tomi 0.2.0"
   git push origin v0.2.0
   ```

5. The workflow stops, with a message, when: the tag is not `vX.Y.Z`; a version file (Android or Windows) disagrees with the tag
   or did not go up; the commit is not on `main`; the repository is private; `ALLOW_UNSIGNED_WINDOWS_RELEASE` is not `true` (while
   the installer is unsigned); the Android release signing secrets are missing, or the APK is debuggable, debug-signed, unsigned
   or signed by another certificate; either build fails; a release already exists for the tag; or an attestation does not verify.
   Nothing is published unless **both** files are good.
6. Otherwise it creates a **draft** release with the files (`tomi-android-X.Y.Z.apk`, `tomi-windows-X.Y.Z-setup.exe`, each with its
   `.sha256`) and generated notes (protocol compatibility, path-scoped changes, verification commands, and for Android the
   certificate fingerprint), verifies the attestation of each file, and publishes it as the repository's "Latest". Then check it:
   `gh release view v0.2.0`, and run the verification commands of the notes on a downloaded copy.

A published tag is never reused, a stable release is never replaced with `--clobber`, and a mistake is fixed forward with the next
version. When you only want a build to try, run a development build (`*-dev-<run>`) from **Actions > Build apps**.

## Android release signing

What CI publishes for Android is **only** a non-debuggable `release` build signed with the project's dedicated release key.
A debug build is debuggable (anyone with USB debugging authorised can read the app's database and use its Keystore key
through `run-as` or JDWP), and a build signed with a debug key has no safe update path. Neither is ever published.

### What the workflow does

`.github/workflows/build-apps.yml` (manual run; input `android_variant`, default `release`):

| Situation | Artifact (workflow artifact `android-apk`) | Published to `android-dev-<run>`? |
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
base64 -w0 tomi-release.jks | gh secret set --env release ANDROID_RELEASE_KEYSTORE_B64      # PowerShell: [Convert]::ToBase64String([IO.File]::ReadAllBytes("tomi-release.jks"))
gh secret set --env release ANDROID_RELEASE_KEYSTORE_PASSWORD
gh secret set --env release ANDROID_RELEASE_KEY_ALIAS      # tomi
gh secret set --env release ANDROID_RELEASE_KEY_PASSWORD   # the same password

# the public fingerprint, to pin
keytool -list -v -keystore tomi-release.jks -alias tomi | grep 'SHA256:'
gh variable set ANDROID_RELEASE_CERT_SHA256   # paste the value, with or without colons
```

Keep **offline backups** of the keystore and its password in two separate places: if the key is lost, installed apps can never be
updated (Android refuses an update signed by another key), and users would have to uninstall and pair again. Never commit it
(`.gitignore` already excludes `*.jks` and `*.keystore`, hence the file name above). When the first release is published, also publish the
certificate fingerprint in the release notes so that users can compare it (see "Verifying a download" below).

Hardening, applied: the four secrets are **environment secrets** of a GitHub Environment named `release`, restricted to the `main`
branch and to the `v*` tags (the `android-v*` and `windows-v*` rules of the first release were replaced), and the `android` job of `build-apps.yml` uses `environment: release`
(so does `sign-windows`, for the SignPath token). A workflow file changed on another branch cannot read them: anyone who can push a
workflow to a branch of the repository could otherwise exfiltrate repository secrets. The consequence: a manual **Build apps** run
must be started from `main` (a run from another branch is refused at the Android job). Set the secrets with
`gh secret set NAME --env release`, not at repository level.

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
- Development builds (`android-dev-<run>`, `windows-dev-<run>`) are **never replaced**: GitHub's *immutable releases* setting is on
  for this repository, so once published a release cannot get, lose or change an asset, and its tag can never be reused, not even
  after the release is deleted. A moving "latest dev" pointer is therefore impossible; each run publishes a new pre-release
  whose notes state which commit and which workflow run produced it, with its own checksum and its own attestation. Only the
  pruning deletes anything, and only whole development releases (with their tag). No workflow uses `--clobber`; stable
  releases are created once from a `*-vX.Y.Z` tag, a published tag is never reused (rule 1), and a fix is a new version.
  Keep the *immutable releases* setting on: it is what makes a published file impossible to alter after the fact.
- In the Android job the signing secrets are in the environment of the Gradle step that builds and signs the APK, and that step
  also compiles the Rust dependencies. A compromised build-time dependency could therefore read them. What reduces this:
  dependency versions pinned by lockfiles and audited in CI (`security.yml`), actions pinned by commit SHA, and keeping the
  secrets in a protected Environment (see "Android release signing"). If a leak is suspected, treat the key as burned:
  generate a new one and say so in the release notes.

## Verifying a download

Windows installer (replace the repository if you use a fork, and X.Y.Z with the version of the release):

```sh
# 1. checksum (against corruption; the .sha256 file is downloaded from the same release)
sha256sum --check tomi-windows-X.Y.Z-setup.exe.sha256
#    PowerShell: (Get-FileHash .\tomi-windows-X.Y.Z-setup.exe -Algorithm SHA256).Hash.ToLower()  -> compare with the file content

# 2. build provenance (needs the GitHub CLI, gh >= 2.49; the repository must be public)
gh attestation verify tomi-windows-X.Y.Z-setup.exe --repo tomi-share/tomi
#    stricter: also require that the signer is this workflow
gh attestation verify tomi-windows-X.Y.Z-setup.exe --repo tomi-share/tomi \
  --signer-workflow tomi-share/tomi/.github/workflows/build-apps.yml
```

A successful `gh attestation verify` prints the workflow, the commit and the run that built the file; any failure means you
should not install it. The Windows installer is **not code-signed yet**, so SmartScreen will warn: verifying the attestation
is the way to know what you are about to run (see "Windows code signing" below).

Android APK:

```sh
sha256sum --check tomi-android-X.Y.Z.apk.sha256
gh attestation verify tomi-android-X.Y.Z.apk --repo tomi-share/tomi
# the signing certificate (Android build-tools) must be the project's one:
apksigner verify --print-certs tomi-android-X.Y.Z.apk      # compare "certificate SHA-256 digest" with the fingerprint below
```

Release certificate SHA-256 fingerprint (the same key signs every Android release, from `android-v0.1.0` on):

```
968d4b1fb3536f34b411f553242bb38162fd64f9fa5aeeb88b080ad2b46ed577
```

In the colon-separated form that `keytool` prints: `96:8D:4B:1F:B3:53:6F:34:B4:11:F5:53:24:2B:B3:81:62:FD:64:F9:FA:5A:EE:B8:8B:08:0A:D2:B4:6E:D5:77`.
The notes of every stable Android release carry it as well. Android refuses to update an app signed with another key, so a build
signed with a different certificate is not an update of this one: if the fingerprint of a download differs, do not install it.

Relay binaries: `sha256sum --check tomi-relay-<target>.sha256` as above.

## Windows code signing

The Windows installer is **not code-signed yet**, and nothing in the repository pretends otherwise: SmartScreen warns, which is
why verification (above) is documented. The project is applying for free code signing from the
[SignPath Foundation](https://signpath.org/); the public side of that is [`CODE_SIGNING_POLICY.md`](CODE_SIGNING_POLICY.md)
(roles, process, privacy), linked from the README and from the notes of every Windows release.

### How it works in the pipeline

Signing is **off by default and switched on by one repository variable**, `SIGNPATH_ORGANIZATION_ID`. Without it, the pipeline is
what it has always been: build, checksum, attestation, unsigned installer. With it (`build-apps.yml`, also used by
`release-apps.yml`):

1. the `windows` job builds the installer and uploads it, unsigned, as `tomi-windows-setup-unsigned.exe`;
2. the `sign-windows` job submits it to SignPath with the SignPath GitHub action (pinned by commit SHA). SignPath checks the origin
   (this repository, this commit, this run, GitHub-hosted runners) and **waits for an approver to approve the request by hand**
   (the job waits up to two hours);
3. the signed file is downloaded and must pass `Get-AuthenticodeSignature` with the status `Valid`, otherwise the job fails and
   nothing is published (it fails closed);
4. only then is the signed file checksummed, attested and uploaded as `windows-installer`, the artifact that the release
   takes. The checksum and the attestation are those of the signed file.

With signing on, `release-apps.yml` no longer needs `ALLOW_UNSIGNED_WINDOWS_RELEASE`, and the notes of the release carry the
attribution that SignPath requires.

### Turning it on (after the SignPath Foundation accepts the project)

Account and project configuration need a SignPath account and cannot be done from the repository.

1. **SignPath, organization:** add the predefined trusted build system *GitHub.com* and link it to the project. Installing the
   SignPath GitHub App is optional but recommended (it lets SignPath evaluate the audit log).
2. **SignPath, project** (its slug becomes `SIGNPATH_PROJECT_SLUG`): set the repository URL for origin verification to
   `https://github.com/tomi-share/tomi`, and create this artifact configuration. The artifact is one `.exe`, not a zip. The
   product name and version must be enforced for every build, which the SignPath Foundation requires:

   ```xml
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <parameters>
       <parameter name="version" required="true" />
     </parameters>
     <pe-file product-name="Tomi" product-version="${version}">
       <authenticode-sign />
     </pe-file>
   </artifact-configuration>
   ```

   The workflow passes the `version` parameter from `apps/windows/src-tauri/tauri.conf.json`. A local build of version 0.1.0 has
   `ProductName` `Tomi`, `ProductVersion` `0.1.0` and `FileVersion` `0.1.0` (`CompanyName` is empty), which is what this
   configuration expects. Check it again after a change of the installer settings:
   `(Get-Item .\tomi-windows-setup.exe).VersionInfo | Format-List ProductName, ProductVersion, FileVersion`.
3. **SignPath, signing policy** (its slug becomes `SIGNPATH_SIGNING_POLICY_SLUG`): a release-signing policy where **each request is
   approved by hand** by the approver named in the code signing policy, with origin verification required.
4. **SignPath, CI user:** create a user with *submitter* permission on that policy and an API token for it.
5. **GitHub:** store the token, then the three variables (`SIGNPATH_ORGANIZATION_ID` last, it is the switch):

   ```sh
   gh secret set --env release SIGNPATH_API_TOKEN
   gh api -X POST repos/tomi-share/tomi/actions/variables -f name=SIGNPATH_PROJECT_SLUG -f value=<project slug>
   gh api -X POST repos/tomi-share/tomi/actions/variables -f name=SIGNPATH_SIGNING_POLICY_SLUG -f value=<policy slug>
   gh api -X POST repos/tomi-share/tomi/actions/variables -f name=SIGNPATH_ORGANIZATION_ID -f value=<organization id>
   ```

6. **Test:** run **Actions > Build apps** with only Windows ticked and *Publish* unticked, approve the request in SignPath, and
   check the artifact: the installer is signed (`Get-AuthenticodeSignature`, publisher *SignPath Foundation*).
7. **Then** update [`CODE_SIGNING_POLICY.md`](CODE_SIGNING_POLICY.md) (status and the attribution line, see the comment in that
   file) and the README, and cut a `vX.Y.Z` release.

Even with a certificate, SmartScreen reputation builds up with downloads: a signed installer can still show a warning at first.
If the SignPath route does not work out, the alternatives are Microsoft's Azure Trusted Signing (check current availability and
eligibility) or an EV/OV certificate, wired into the same job. Later: the Tauri updater, with a minisign public key embedded in
`tauri.conf.json` and the private key kept in CI secrets only, so that installed copies update through a verified channel.

## Not done yet

- [x] The Android release keystore exists and the `ANDROID_RELEASE_*` secrets and `ANDROID_RELEASE_CERT_SHA256` are configured
      (see "Android release signing"). The certificate fingerprint is published in "Verifying a download" and in the notes of
      `android-v0.1.0`. Keep the keystore and its password backed up: they cannot be recovered from GitHub.
- [ ] Code signing for the Windows installer (SmartScreen currently warns): the pipeline is ready and off by default; the SignPath
      Foundation application and its configuration are owner tasks ("Windows code signing"). Until then a `vX.Y.Z` release
      needs the repository variable `ALLOW_UNSIGNED_WINDOWS_RELEASE=true`, a conscious decision.
- [x] Workflow for `vX.Y.Z` tags, Android and Windows in one release (`release-apps.yml`): version checks, build, draft, attestation
      check, publish, generated notes with the protocol compatibility. Written without a CI run available: the first dry run
      (Actions > Release apps > Run workflow) is its first test.
- [ ] `cli-v*` workflow and binaries, if the CLI is to be distributed.
- [ ] Minimum compatible versions in the notes (rule 4): the notes state the protocol and relay API version and the relay version at
      release time; a stricter "minimum relay version" needs a decision on what counts as incompatible.
- [ ] Decide the first public versions (Android and Windows are at 0.1.0; the crates are at the workspace version in `Cargo.toml`).
- [ ] Optional: repository variable and secret checklist for a new fork: `TOMI_RELAY_URL` and `ANDROID_RELEASE_CERT_SHA256`
      (variables), the `ANDROID_RELEASE_*` secrets and `FORGE_DEPLOY_URL` (secret); documented in this file and in `docs/DEV.md`.
