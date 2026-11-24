# AGENTS.md — Tomi

Technical conventions for anyone (human or automated tool) working on this repository.
Contribution workflow: see [CONTRIBUTING.md](CONTRIBUTING.md). Feature status per platform:
[docs/FEATURES.md](docs/FEATURES.md) (update it in the same commit as any behavior change).

## The product

**Tomi** is end-to-end encrypted sharing between the devices of **one person** (markdown, passwords/secrets, links).
A **blind** store-and-forward relay routes opaque, signed envelopes. Single-user by design.

Monorepo:

- `crates/core` (`tomi-core`) — protocol, crypto, format, runtime (SQLite persistence, network, UniFFI bindings).
- `crates/cli` (`tomi-cli`) — command-line client.
- `apps/relay` (`tomi-relay`) — the blind HTTP relay.
- `apps/android` — Kotlin / Jetpack Compose (Material 3), real core through UniFFI; a fake core exists for tests only.
- `apps/windows` — Tauri + Svelte.
- `apps/apple` — SwiftUI (iOS and macOS), real core through UniFFI (`TomiCore`). The Mac app is the `TomiMac` target of
  the same project: same screens, a menu bar app (`docs/macos-app.md`). Push for iPhone (APNs; written, not in
  service yet): `docs/push-apns.md`.
- `spec/` — normative specification (`format-v1.md`, `api-v1.md`, `pairing-v1.md`, pin and archive contracts) and frozen
  test vectors in `spec/vectors/`.
- `design/` — brand guidelines and assets. `infra/` — deployment guide and templates. `docs/` — design notes.

Identifiers: application id `eu.shaffe.tomi`, URL scheme `tomi-share://`.

## Golden rules

1. **The relay is blind.** It never decrypts anything and sees no content or key. It handles opaque bytes, verifies
   signatures and routes. Never make it read a content or a state.
2. **Single user in v1.** No ACLs, no effective recipient, one space key shared by all devices. The `audience` field and
   a `key_epoch` range are RESERVED with no cryptographic effect (see `docs/protocol-extensibility.md`). Never claim
   that a "recipient" protects anything.
3. **Secrets are sacred.** The title of a hidden secret is encrypted and kept out of the wire format and search. No
   secret content in clear at rest (SQLite, WAL, files), in logs, in notifications, or in an unprotected clipboard. System
   sharing of a secret only after biometric confirmation. Known trade-off: attachments of a secret are decrypted into
   `files/cache/` to be opened; this clear copy stays until the app goes to the background or the next start (then it is
   wiped). Precisely: on Android the purge runs 60 s after the app goes to the background (a grace so that "Open with" and
   sharing can finish), the core purges again when it opens at the next start, and if the process dies inside those 60 s the
   copy stays until that next start; on Windows it is purged 1 min after the last Tomi window stops being in use, on exit and
   at the next start. Do not extend these delays, do not hide that the copy exists and do not add any other clear copy. See
   `docs/threat-model.md`.
4. **Format compatibility.** Changes are ADDITIVE (optional field ignored by older readers, tolerated new type, new
   crypto label) without touching the format. A breaking change means a new `format` / new `vN/` vector directory.
   The frozen vectors in `spec/vectors/` never change; if code and vectors disagree, the vectors win.

## Product decisions already applied to core and spec

- **No expiry by default** ("Never"), secrets included. Options: Never, After reading (hidden items only), 1 h, 1 d,
  7 d, 30 d.
- **History transfer** when pairing: sealed for the new device, EVERYTHING included (secrets and files), no checkboxes,
  a single yes/no choice.
- **Pin**: part of the protocol, shared, encrypted (the relay only sees a random `event_id`), last-writer-wins, does not
  prevent relay purge, bounded by `at_ms <= 2^53-1`.
- **Hidden title**; no multi-person ACLs in v1 (future direction: one space per person + groups).

## UI conventions

- Android: Material 3 Expressive with a floating toolbar at the bottom (`material3` 1.5.0-alpha, so the API may move). Brand
  tokens in `design/tomi/tokens.json`, light and dark. The identity colors (night `#24324A`, sun `#FFC85C`) are reserved for
  the symbol. No circular-arrow icon and no spinner next to the logo (brand guidelines §6.3).
- iOS: native Liquid Glass (iOS 26+) for the floating bar and controls — a design of its own, not an imitation of Android.
  Behavior and content stay aligned with Android; any deliberate difference is recorded in `docs/FEATURES.md`.
- macOS: a **menu bar app**. The icon next to the clock opens the feed in a popover; the full window opens on demand (the
  Dock icon exists only while it is open). Screens and rules are those of iOS: the same files of `apps/apple/Tomi`
  compile for both, what is specific to the Mac lives in `apps/apple/TomiMac`. A shared screen is not duplicated for the
  Mac: `#if os(macOS)` in the file, or a common name in `Tomi/Common/Platform.swift`. A screen change is checked on
  **both** platforms. Every button or menu a screen puts in `.toolbar` carries `barControl()`, and the content of a
  pushed screen carries `screenControls()` (`apps/apple/README.md`); without them the bar buttons shrink to their
  symbol. To look at the Mac app without taking over the session of the person at the Mac, use the `tomi-share://debug-…`
  tools and the off-screen launch (`apps/apple/README.md`, "macOS app"): never simulated keystrokes or clicks on their
  desktop. Any deliberate difference with iOS is recorded in `docs/FEATURES.md` (macOS column).
- UI strings follow the language of the device; English is the reference and the fallback (`res/values/strings.xml`,
  `Localizable.xcstrings` generated from it, `apps/windows/src/lib/i18n/`). How to add a language: `docs/LOCALIZATION.md`.
  Code, comments and documentation are in English.

## Build and checks

- **Rust** (before any core change): `cargo check --workspace`, `cargo test --workspace`,
  `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`. The frozen vectors must stay
  "unchanged": `cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1` and
  `cargo run -p tomi-core --example gen_pairing_vectors -- spec/vectors/pairing-v1`.
- The relay must depend on `tomi-core` with `default-features = false` (otherwise SQLite/rustls/uniffi are duplicated).
- **Android**: `cd apps/android && ./gradlew assembleDebug testDebugUnitTest lint` (look for `BUILD SUCCESSFUL`).
- **Windows**: in `apps/windows/src-tauri`: `cargo check` and `cargo test`; in `apps/windows`: `npm run check`.
- **iOS**: Swift packages under `apps/apple/Packages`; the app project is generated with XcodeGen from `apps/apple/project.yml`.
- **Apple generated files** (`Localizable.xcstrings`, `TomiTokens.swift`) are derived, and CI ("Apple (generated files up to date)")
  fails when they are stale. So **any change to `apps/android/app/src/main/res/values/strings.xml`,
  a file of `apps/apple/tools/strings-apple/` or `design/tomi/tokens.json` must ship in the same commit as the regenerated files**,
  even for an Android-only change. Regenerate with `just apple-project` (or `python3 -I apps/apple/tools/strings-from-android.py`
  and `tokens-to-swift.py` from `apps/apple`). With no working Python (e.g. the Windows Store stub), edit the catalog by hand
  exactly as the script would (keys sorted, `"extractionState" : "manual"`, `fr` unit `translated`; write `%1$@` in `ios_` keys,
  never `%1$s`), then check that `git diff` shows only that entry. Run this check before every push that touches those sources.
- CI builds and release publishing: see `docs/DEV.md` and `.github/workflows/`.

## Git hygiene

- Make coherent, separate commits; do not commit generated artifacts (`target/`, `build/`, `.gradle/`, `node_modules/`, APKs,
  keys) and never commit a secret.
- Do not commit code that does not compile or has not been reviewed.
- Line endings are normalized by `.gitattributes`: do not rewrite files only to change line endings.
- **Committing a finished feature needs no confirmation.** Once a feature is developed, reviewed, then tested (the
  checks of "Build and checks" for every platform it touches pass, and it compiles), commit it without asking the user.
  The review comes before the tests and is what satisfies "has been reviewed" above. This replaces any default "commit
  only when asked". Commit on the branch currently checked out where you work (the shared checkout or your own worktree):
  do not switch or create a branch for it. It covers committing only: never push, tag a release or open a PR without being
  asked.
- **Commit lock (several agents sharing one checkout).** The index is shared, so two agents staging at once corrupt each
  other's commits. Before the first `git add`, take the lock by creating the ref `refs/agents/committing`; creating it
  with an all-zero expected old value fails if it already exists, which makes it atomic:
  `git update-ref --create-reflog -m "<agent / feature>" refs/agents/committing HEAD 0000000000000000000000000000000000000000`.
  If it fails, another agent is staging: wait and retry. See who holds it and since when with
  `git reflog show --date=relative refs/agents/committing`; if it is older than 10 minutes, do not delete it, tell the
  user. Stage only the files of your own feature, by explicit path (never `git add -A`, `git add .` or `git commit -a`:
  other agents' unfinished changes are in the same working tree), check with `git diff --cached`, commit, then release with
  `git update-ref -d refs/agents/committing`, **also when the commit fails** (hook, conflict). The ref is local
  coordination only: it lives outside `refs/heads` and `refs/tags`, and is never pushed.
- **Exception: a dedicated worktree** (`git worktree`, own index and own branch) needs no lock. The lock is for agents
  working in the same checkout.
