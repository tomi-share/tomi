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

### Changed
- iOS: long texts get bounded previews; String Catalog and generated design tokens cleaned up.

### Security
- Core: the epoch-ordering rule no longer relies on relay-chosen positions; a sync page without progress ends the loop; SQLite `secure_delete` and WAL truncation; tombstones kept 90 days and items older than that refused as suspect; space keys erased on a verified signed self-revocation; persisted `rotation_stalled` flag and relay-revocation notices (not displayed by the apps yet).
- Relay: every `X-Forwarded-For` line is read.
- Android: fresh authentication before adding a device; the screen lock accepted for destructive actions; authentication window closed in the background; guaranteed purge of the decrypted attachment cache; alert when a device joins.
- iOS: share inbox excluded from backups and dropped after 24 h; fresh authentication before adding a device; alert when a device joins; network loads blocked in the HTML web view.
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
