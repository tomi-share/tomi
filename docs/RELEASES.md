# Releases

Tomi is a monorepo with several independently shipped products. Each product has its own tag prefix and its own
version; there is no global version number. Client/relay compatibility is carried by the **protocol version**
(`format`, frozen vectors in `spec/vectors/vN/`), not by application versions.

## Tag scheme

| Product | Tag | GitHub release | Trigger | Status |
|---|---|---|---|---|
| Relay | `relay-vX.Y.Z` | Linux binaries | push of the tag (`release-relay.yml`) | **in place** |
| Relay (moving pointer) | `relay-latest` | same assets as the latest relay release | rewritten by the workflow; do not edit by hand | **in place** |
| Android, development | `android-dev` | rolling pre-release, APK `tomi-debug.apk` / `tomi-preview.apk` | manual run of `build-apps.yml` | **in place** |
| Windows, development | `windows-dev` | rolling pre-release, `tomi-windows-setup.exe` (unsigned) | manual run of `build-apps.yml` | **in place** |
| Android, stable | `android-vX.Y.Z` | signed APK | push of the tag | planned (see below) |
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

## Not done yet

- [ ] Release signing for Android (a real release keystore, stored as a repository secret) before creating `android-v*` tags.
      Until then the APKs are debug or preview builds signed with a debug key.
- [ ] Code signing for the Windows installer (SmartScreen currently warns), before creating `windows-v*` tags.
- [ ] Workflows for `android-v*` and `windows-v*` tags: build, check that the tag matches the version file, publish a
      non-prerelease with generated notes.
- [ ] `cli-v*` workflow and binaries, if the CLI is to be distributed.
- [ ] A "minimum compatible versions" table in each release's notes (rule 4), possibly generated from the version files.
- [ ] Decide the first public version (the crates are currently at the workspace version in `Cargo.toml`).
- [ ] Optional: repository variable and secret checklist for a new fork: `TOMI_RELAY_URL` (variable), `ANDROID_DEBUG_KEYSTORE_B64`
      and `FORGE_DEPLOY_URL` (secrets); documented in `docs/DEV.md`.
