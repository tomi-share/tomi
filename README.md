# Tomi

Tomi shares information end-to-end encrypted between one person's own devices (Windows,
Android, iOS, plus an early-stage command-line client), through a blind store-and-forward relay that
notifies connected clients in real time over WebSocket (mobile push notifications are not implemented yet).

Shared content can be Markdown or HTML text, links, files and images, and passwords or other
secrets. The relay only ever handles opaque encrypted bytes: it never sees content or keys.

Tomi is single-user by design: a space contains only the devices of one person, and there are no
accounts, recipients, or per-device permissions in version 1.

The full project framework, design decisions and UX flows are described in
[`PROJECT.md`](PROJECT.md) and in [`docs/`](docs/).

## Structure

```
tomi/
  apps/
    relay/          # Rust — Axum + tokio-tungstenite + SQLx: the blind relay server
    windows/        # Rust + Tauri, Svelte frontend — Windows client
    android/        # Kotlin + Jetpack Compose — Android client
    apple/          # SwiftUI — iOS client
  crates/
    core/           # Rust — crypto, item format, sync, local storage (+ UniFFI bindings)
    cli/            # Rust — the `tomi` command-line client (early stage)
  spec/             # Protocol and format specifications + frozen test vectors
  design/           # Visual identity (design tokens and assets)
  infra/            # Deployment (systemd, Caddy, install and update scripts)
  docs/             # Project framework, UX, feature matrix, developer guide
  justfile          # Single entry point for development commands
```

## Status

- **Core (`crates/core`)**: complete. It is the only implementation of the cryptography, the
  item format and synchronization, shared by every client through Rust directly or UniFFI.
- **Relay (`apps/relay`)**: implemented, with multi-space support.
- **Clients**: Android, Windows and iOS are built on the real core. The remaining differences
  between platforms are listed in [`docs/FEATURES.md`](docs/FEATURES.md), the
  per-platform feature matrix.

## Getting started

Prerequisites: Rust (via rustup), the Android SDK and NDK for Android builds, `cargo-ndk`, and
[`just`](https://github.com/casey/just).

```
just build          # build the Rust workspace (core, cli, relay)
just dev-relay      # run the relay locally
just test           # run the workspace tests
just test-vectors   # check the specification test vectors (spec/vectors/)
```

See the [`justfile`](justfile) for the Android and Windows commands.

## Documentation

- [`PROJECT.md`](PROJECT.md): project framework, architecture and design decisions.
- [`spec/`](spec/): protocol, format and contract specifications, with frozen test vectors.
- [`docs/DEV.md`](docs/DEV.md): developer guide (setup, builds, releases).
- [`docs/FEATURES.md`](docs/FEATURES.md): feature matrix across platforms.
- [`CONTRIBUTING.md`](CONTRIBUTING.md): how to contribute.

## Repository

<https://github.com/tomi-share/tomi>

## License

Licensed under the Apache License, Version 2.0. See [`LICENSE`](LICENSE).
