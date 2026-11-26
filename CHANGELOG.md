# Changelog

All notable changes are listed here, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Tomi ships several products with their own tags and versions (see [`docs/RELEASES.md`](docs/RELEASES.md)); this file
tracks the project as a whole and names the products concerned. Protocol compatibility is carried by the `format`
version and the frozen vectors in `spec/vectors/`, not by application versions.

## [Unreleased]

### Added
- Community files for the public repository: `SECURITY.md`, `CODE_OF_CONDUCT.md`, `TRADEMARKS.md`, issue and pull
  request templates, `CODEOWNERS`, Dependabot configuration and a dependency audit workflow (`cargo-deny`, `npm audit`).
- `docs/FORKING.md`: where the identity of each platform lives, for forks that distribute their own build.
- Android: the application id can be overridden with `-PtomiApplicationId=...` (default unchanged: `eu.shaffe.tomi`).
- Core: unified core, with a demo mode that is entered and left from the real core (Android, Windows, iOS).
- Windows, iOS: delete a single attachment, on this device only or everywhere.
- CI: stable releases of the Android app and the Windows installer from `android-vX.Y.Z` / `windows-vX.Y.Z` tags
  (`release-apps.yml`): the tag must match the version files (and `versionCode` must grow), the commit must be on `main`,
  every file is checked and attested before the release is published, and the notes state the protocol compatibility
  ([`docs/RELEASES.md`](docs/RELEASES.md)).

### Changed
- iOS: long texts get bounded previews; String Catalog and generated design tokens cleaned up.

### Security
- Core: the epoch-ordering rule no longer relies on relay-chosen positions; a sync page without progress ends the loop; SQLite `secure_delete` and WAL truncation; tombstones kept 90 days and items older than that refused as suspect; space keys erased on a verified signed self-revocation; persisted `rotation_stalled` flag and relay-revocation notices, shown by the three apps (a calm banner while a key rotation keeps failing, and a one-time notice for a device the relay alone removed); relay-announced removals are validated (known verified device, `admin` or `self` only, bounded name and list), and a rotation blocked by a device the relay dropped while this one was away now completes.
- Core: the device that posts a key rotation no longer trusts the position the relay answers for it: it judges the items of the old epoch against the sync cursor it had just before the post.
- Core: the `max_ttl` that a relay announces no longer sets retention windows (tombstones, item age, old keys) beyond one year; a device the space lists as a member stays verified whatever certificate the relay sends along; a revoked device is never put back among the members by a record the relay serves.
- Android, iOS, Windows: the notice of a device removed by the relay alone is taken from the core only while a screen can show it (Android: while the feed is started; iOS: in the foreground; Mac: while the popover or window is open; Windows: when a visible window asks), so a background sync can no longer take it and lose it with the process.
- Android, iOS, Windows: the "new device" alert is raised only for a device whose certificate the space accepted, so a relay cannot make up an alert by listing a device nobody added, and a known device the relay makes look unverified for a moment is not announced again; the device name is bounded in the Android alert like on the other two; Android and iOS show "not verified" next to such a device in the list, as Windows does.
- Relay: every `X-Forwarded-For` line is read.
- Android: fresh authentication before adding a device; the screen lock accepted for destructive actions; authentication window closed in the background; guaranteed purge of the decrypted attachment cache; alert when a device joins.
- iOS: share inbox excluded from backups and dropped after 7 days; fresh authentication before adding a device; alert when a device joins; network loads blocked in the HTML web view.
- Windows: authentication window closed on session lock and sleep; fresh Hello before adding a device; a secret left on the clipboard by a crash is emptied at start; longer list of file types never launched; toast when a device joins.
- CI: only non-debuggable, signed Android release APKs are published; checksums and build provenance for published apps.

### Fixed
- Android: a Markdown view whose rendering never lands is recreated.
- Core: `expect()` is allowed in the demo mock helper so that `clippy --features demo` passes.

## [0.5.2] - 2026-11-18

Relay and core (`relay-v0.5.2`, Cargo workspace version 0.5.2).

### Changed
- CI: a missing default relay is reported as a notice, not a warning.

## Before 0.5.2

First public state of the repository: the Rust core (protocol, crypto, format, runtime), the blind relay with
multi-space support, the Android, Windows and iOS clients on the real core, the specification and its frozen test
vectors (`spec/vectors/v1`, `spec/vectors/pairing-v1`), and the deployment material in `infra/`.
