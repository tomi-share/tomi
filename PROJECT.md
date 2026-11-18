# Tomi — Project framework

> Information sharing, end-to-end encrypted between one person's own devices (Windows, Android,
> iOS), through a relay on a VPS/EC2 host, with a notification when new information arrives.

## 0. Name

**Tomi**: back and forth between one's own devices.

| Usage | Value |
|-------|-------|
| Display name | Tomi |
| URL scheme | `tomi-share://` |
| CLI client | `tomi` (e.g. `tomi send file.pdf`) |
| Server binary | `tomi-relay` (administration: `tomi-relay admin status`) |
| Repository | `tomi` (public: https://github.com/tomi-share/tomi) |
| Android identifier (applicationId) | `eu.shaffe.tomi` (final once the app is installed) |
| Windows identifier (AUMID / bundle) | `eu.shaffe.tomi` |
| Relay domain | `relay.example.org` (example placeholder: clients take the relay address from the pairing link/QR code or the optional `TOMI_RELAY_URL` build default; see `docs/DEV.md`) |
| Android App Links | `https://relay.example.org/l/...` + `/.well-known/assetlinks.json` |

## 1. Goal

A "personal multi-device push": a small message bus between one person's devices.
Information sent from one device appears on the others, with a notification.

**Scope of v1: single user, multiple devices.** One person owns all the devices of a single
**space**. Everything sent goes to all the other devices of the space: no multiple recipients,
no targeting, no permissions between devices. Sharing with other people is a future extension,
outside v1.
Content can be: **text**, **Markdown/HTML**, **links**, **files/images**, and **passwords / secrets**.
Rich text is captured as is; the **rendering (Markdown / HTML / plain) is chosen when viewing**
on the receiving device, not when sending. A `source_format` hint detected at sending time only
serves as the default display mode.

## 2. Guiding principles

- **Mandatory E2EE**: the relay never sees plaintext. It only stores and forwards encrypted blobs.
  Keys live on the devices.
- **Native per platform**: no single cross-platform framework. Each client is idiomatic and makes
  full use of its OS (Android sharing, Windows native notifications and clipboard).
- **A solid shared contract**: a single message contract, locked by test vectors common to all
  languages.
- **Personal, single-user use**: no accounts, no third-party identities. One space = a fleet of
  devices belonging to one person. Device-to-device pairing.
- **No automatic clipboard reading**: the app never reads the clipboard by itself, neither in the
  background nor on a single click. To send copied content, the user **pastes** it explicitly
  (`Ctrl+V`, long press → Paste) into an input field.
- **Versioned protocol**: devices running different app versions must coexist in the same space
  (see section 6).

## 3. Stack (validated decisions)

| Component | Choice | Details |
|-----------|--------|---------|
| Relay / API | **Rust** | Axum + tokio-tungstenite (WebSocket), SQLx |
| Relay database | **SQLite** (WAL mode) | encrypted blobs on disk; backup = copy of the folder |
| Client core | **`core` Rust crate** | crypto, item format, sync, local storage, HTTP/WS client |
| Windows client | **Rust + Tauri** | webview + `core` directly |
| Android client | **Native Kotlin** | Jetpack Compose + `core` via UniFFI; ACTION_SEND sharing, notifications, Keystore |
| iOS client | **Swift + SwiftUI** | `core` via UniFFI (`TomiCore`) |
| Android versions | **minSdk 33** (Android 13); targetSdk and compileSdk 36 (Android 16) | 33 = sensitive clipboard handling + notification permission |
| CLI | **`tomi`** (Rust) | `core` + a thin layer of commands |
| Android notifications | **FCM** | a bare doorbell (empty message); sending sits behind an interface so UnifiedPush can be added later |
| Windows notifications | native | via a persistent WebSocket |
| Pairing | **QR + short code (PAKE)** | device-to-device key exchange |
| Updates | **served by the relay** | Tauri updater (Windows) + downloadable APK (Android) |
| Reverse proxy / TLS | **Caddy** | automatic Let's Encrypt |

### Why Rust on the API side
The relay does almost no business logic (E2EE → zero logic on plaintext). It is a message bus:
concurrent I/O, long-lived connections, low latency — where Rust excels.
A decisive bonus: the contract and the cryptography live in crates shared by the relay, Windows
and the CLI, so they are defined once and checked by the compiler. EC2 deployment = one binary +
systemd + Caddy. A PHP stack (PHP-FPM + Reverb + Redis) would have added a heavy stack for little
value here.

### Why a single Rust core, including on Android
Writing the cryptography and the envelope format in Rust **and** in Kotlin guarantees they will
diverge (nonce, AAD, encoding, edge cases). The `core` crate is therefore the only implementation;
Android calls it via UniFFI and keeps a 100% native Kotlin interface. Test vectors serve to freeze
the format over time, not to reconcile two codebases.

### Sync: REST for data, WebSocket/FCM to wake up
The relay numbers each event (item, deletion, rotation) with an increasing `seq`.
Each device keeps a `last_seq` cursor and asks "everything since N" when woken by the doorbell
(WebSocket, FCM) and at startup. A single algorithm covers real time, offline use and catch-up.
The cursor advances the reading of the *journal*, but does not by itself mean an item has been
received: an item with blobs can appear in the journal before the device has finished downloading
its files. The "delivered" status therefore comes from an **explicit per-device acknowledgement**
(`ack`, set once the item and its blobs are complete), distinct from the "the relay may purge"
acknowledgement (`release`). Details in `spec/api-v1.md` §4/§8.

## 4. Monorepo structure

```
tomi/
  apps/
    relay/         # Rust — Axum + tokio-tungstenite + SQLx
    windows/       # Rust — Tauri
    android/       # Kotlin + Jetpack Compose
    apple/         # Swift + SwiftUI (iOS)
  crates/
    core/          # Rust — crypto, E2EE envelope, sync, local storage (+ UniFFI bindings)
    cli/           # Rust — the `tomi` binary
  spec/            # format specifications + test vectors frozen per version
  infra/           # systemd + Caddy, EC2 deployment
  justfile         # single entry point: dev-relay, android, test, release
  .github/workflows/  # CI with path filters
  docs/            # specs, UX, user flows
```

## 5. Security model (summary)

- **End-to-end encryption (E2EE)** for all content: the relay sees and stores only ciphertext,
  holds no keys and never decrypts.
- **Single symmetric space key**: the space has one key shared by all its devices. All content is
  encrypted with this key (ChaCha20-Poly1305). No per-recipient encryption — the model is single-user.
- Each device generates a key pair (X25519) at installation, used only to **transport the space
  key** securely during pairing.
- QR pairing: a device that is already a member displays a QR code, the new device scans it →
  ephemeral X25519 exchange → the space key is sent encrypted to the new device.
- **Revocation**: removing a device from the space → **rotation of the space key**; the revoked
  device can no longer decrypt new items.
- Secrets (passwords) are never logged nor cached in plaintext on the server side.
- **Files/images**: encrypted locally, then uploaded as encrypted blobs; the relay only sees
  ciphertext. v1 limit: **25 MB per file**.
- **Received HTML** is rendered in a **sandboxed webview** (no JS, no network, strict CSP) to prevent
  script execution and exfiltration; `plain` serves as the fallback.
- **Optional expiry**, chosen at capture time; **none by default**, secrets included (the "Never"
  choice). If the user picks one, each device deletes its local copy at that date. The relay only
  keeps an undelivered item until `min(expiry, 30 days)`: this is a separate retention deadline, which
  the relay can see in plaintext, whereas the intended expiry stays inside the ciphertext. This is
  data hygiene, not a cryptographic guarantee of destruction.
- **Local storage**: a received item is re-sealed under a device-specific local key (random,
  Keystore / DPAPI); old space keys are discarded 31 days (30 d + 1 d) after the rotation that
  replaces them. An item from an epoch already replaced that arrives after the rotation is refused
  ("unreadable, suspicious"), never deleted.
- **Deletion at two scopes**, from any device (sender or receiver): *local* (this device only) or
  *everywhere* (signed tombstone → relay purge + erasure on all devices). The relay only sees the
  `item_id`. Revocation is best-effort.
- **"After first read"**: an explicit option, never a default. It is not a separate mechanism. The
  first device that reveals or copies an item marked `read_policy = after_first_read` itself emits, at
  that moment, a signed tombstone of scope *everywhere* — exactly as a manual deletion would.
- **Secret title**: it is part of the secret. Encrypted with the content, masked in the feed and
  notifications, excluded from search, and revealed only together with the rest of the content
  (Android and Windows).
- **History transfer**: a member device can transfer its **entire** feed (secrets and files
  included, with no partial option) to another device: a single-use key sealed for the sole
  recipient, a signed manifest, and temporary blobs on the relay (≤ 7 days). No old space key is
  distributed; the relay only sees sizes.
- **Pins**: protocol state, shared by all devices. A "pin / unpin" decision is signed by the device
  then encrypted under the space key (a 320-byte state event): the relay only sees a random
  `event_id`, neither the targeted item nor the state. Devices converge with last-writer-wins on the
  signed timestamp (bounded to 2^53 − 1 ms). A pin changes neither relay purging nor file downloads
  (which would reveal the item to the relay); it keeps the local copy and moves the item to the top of
  the feed. It never extends an explicit expiry. It is included in the history transfer.

## 6. Protocol versioning

Problem: devices are not updated at the same time. A PC may be on v3 while a phone is still on v2.
No one should lose messages.

### Three distinct numbers
| Number | Where | Role |
|--------|-------|------|
| `format` | in each encrypted envelope (and in the AAD) | says how to decrypt and read the item |
| `api` | REST path (`/v1/...`) + WS handshake | contract between client and relay |
| `key_epoch` | in each envelope | which space key encrypted the item (rotation) |

`key_epoch` is not a protocol version: it changes at each revocation, not at each release.

### Negotiation
- On connection, each device announces `{ format_min, format_max, app_version }`.
  The relay records it per device.
- The relay computes the **effective version of the space** = the smallest `format_max` among the
  **active** devices. It returns it to each device.
- A sender always writes in the effective version of the space, so that all active devices can
  read what it sends.
- A device inactive for more than N days (e.g. 30) drops out of the computation, so a forgotten old
  tablet does not block changes. When it comes back, it receives "update required" if it can no
  longer read the effective version.

### Compatibility rules
- **Additive change** (new optional field): no new version. Readers ignore unknown fields.
- **Multi-recipient reservations** (no effect in v1, `spec/format-v1.md` §5.8): an `audience` field
  in the payload (absent = the whole space; ignored by a v1 reader; **not** a confidentiality
  boundary, every member reads everything) and a `key_epoch ≥ 2^31` range for future keys outside
  the space (a v1 reader keeps such an item "waiting for key"). No existing byte changes. What cannot
  be reserved: `docs/protocol-extensibility.md`.
- **Breaking change** (new algorithm, new file split, new mandatory field): `format` + 1. Activated
  only when the effective version of the space allows it.
- An item in an unknown format displays "Unreadable content, update the app"; it is never deleted or
  silently ignored.
- The relay advertises the `api` range it supports and answers a client that is too old with an
  explicit "update required" error, including the update link.
- Readers keep the ability to read old formats as long as items of that format may be on the relay
  (30 days max) or kept raw locally (unreadable items), before dropping support. Items already opened
  are re-sealed locally and no longer depend on it.

### Safeguards in the repository
- `spec/` contains the specification of each `format` and its **test vectors** (`spec/vectors/v1/`,
  `v2/`, …), never modified once published.
- CI checks that each client reads all the vectors of the formats still supported.

## 7. Decisions

| Topic | Decision | Reason |
|-------|----------|--------|
| Client core | A single Rust crate `core`, used by Windows, CLI and Android (UniFFI) | one cryptographic implementation to audit |
| Sync | REST = the only data path; WebSocket and FCM = doorbell; `seq` cursor per device | one algorithm for real time and offline use |
| Relay purge | As soon as all active devices have the item (+ a few minutes of grace), in addition to the deadline; bounded by `max_ttl_secs` (30 days) even without expiry | less ciphertext at rest |
| Expiry (D1) | None by default, for all types; options: Never (default), After read, 1 h, 1 d, 7 d, 30 d | nothing disappears without an explicit choice |
| New-device history (D2) | The relay does not replay history (`joined_seq`). A member device can **transfer its feed**, encrypted for the new device only, at pairing or later (Devices screen). A single yes/no choice, **everything included** (secrets and files), with no checkboxes. No old key is distributed | recover one's feed on a new phone even if the old one is lost (the PC serves as the source); pairing is already the moment of full trust |
| Secrets | No expiry by default; options: after first read, or a duration. Masked (title included), excluded from search, monthly review of secrets older than 30 days | keep a useful password without exposing it |
| Local storage | Items re-sealed under a device-local key; old space keys discarded 30 d + 1 d after rotation | do not keep every space key forever |
| Pinning (instead of tags) | Boolean "pinned" flag per item, **in the protocol**: signed and encrypted event (`POST /v1/item-states`), with no plaintext `item_id` (the relay only sees a random `event_id`); last-writer-wins convergence; included in history transfer. Relay purge unchanged (decision (a)): each device keeps its local copy. Does not override an explicit expiry ("after read", duration) | retrieve important items on all of one's devices without revealing to the relay which ones they are |
| Multi-recipient (reservation) | **Reserve without implementing**: `audience` field in the encrypted payload, `key_epoch ≥ 2^31` for keys outside the space, `capabilities` in the API; nothing is used in v1, no existing vector changes (a `reservations.json` vector file is added) | allow a future multi-person sharing without a new envelope or payload `format`; the trust model itself would have to change (see `docs/protocol-extensibility.md`) |
| Notifications | The active device rings on its own, the others receive silently + an "unread" badge; "read" state synchronized; close-together sends grouped | avoids multi-device noise |
| CLI | `tomi` shipped in v1 | nearly free with the Rust core |
| Relay database | SQLite | one space, a few devices |
| Windows | Tauri | popover, drag and drop, Markdown/HTML rendering, installer |
| Android push | FCM, behind an interface | reliable under Doze; UnifiedPush can be added later |
| Updates | Served by the relay | decided while the repository was private |
| Passkeys | Dropped | Dashlane has no PRF support; the gain is too small |
| Space creation | Single-space relay + single-use installation token | prevents third parties from using the relay |
| Administration / recovery | `tomi-relay admin` over SSH | SSH is the recovery factor |

## 8. Open questions

1. Cryptographic details of the E2EE envelope (to be specified in `spec/`), including a possible
   switch to XChaCha20-Poly1305.
2. Rewrite of `docs/ux-flows.md` to integrate the walkthrough, daily friction points and these
   decisions.
