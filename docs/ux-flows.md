# Tomi — User journeys & UX

> **v1 scope: single user, multi-device.** A single space, a fleet of devices belonging to one user.
> Everything sent goes to all of the user's other devices — no targeting or recipients. Sharing with other
> people is a future evolution.

## Overview of the 3 surfaces

|                | Android                         | Windows                        | Relay                  |
|----------------|---------------------------------|--------------------------------|------------------------|
| Main role      | **Capture** (system share) + receive | **Browse/paste** + send   | transmit ciphertext    |
| Key interaction| "Share → Tomi"                  | Tray + window + clipboard      | invisible              |
| Notif          | FCM + local                     | native Windows                 | triggers FCM           |

Guiding idea: **Android = the sensor**, **Windows = the workstation**.
The dominant flow is Android → Windows, but everything works in both directions.

---

## Journey 1 — First installation & pairing (critical)

Goal: completed in under 1 minute, with no account and no password to type.

### Device A (first device, e.g. Windows)
1. Open the app → "No paired device" screen.
2. "Create my space" → generates the **space key** and an X25519 key pair locally.
3. Enter the relay URL once (e.g. `wss://relay.example.org`), which can be pre-filled via a configuration link.
4. Status: "Space created. Add a second device."

### Device B (e.g. Android)
1. Open the app → "Join a space".
2. On Windows: "Add a device" → **QR code** (relay URL + X25519 public key + single-use ephemeral pairing secret, expires after ~2 min).
3. Android scans it → ephemeral X25519 exchange via the relay → the **space key** is sent encrypted to the new device → **the same 6-digit short code** shown on both sides (protection against MITM).
4. "Device added". Both appear in the list.

DX: the QR code encodes everything (zero typing), the 6-digit code provides reassurance, and a single-use secret plus expiry means the QR cannot be reused.

---

## Journey 2 — Sending from Android via the system share sheet (core need)

1. From any app → select text/page → **Share**.
2. **Tomi** in the share sheet (target `ACTION_SEND` / `ACTION_SEND_MULTIPLE`).
3. Lightweight bottom sheet (not the whole app):
   - content preview, **type detected automatically** (Link / Markdown / Hidden)
   - "Hide content" toggle
   - **"Expires in…" setting** (default depends on the type, see below)
   - **Send** button (goes to all of the user's other devices)
4. Local encryption → relay upload → "Sent" toast, return to the originating app.

### Sending copied content (without sharing)
For copied content (e.g. from a password manager):
open the app, or the **"Tomi" Quick Settings tile**, which opens the input area directly
→ long press → **Paste** → Send.
The app never reads the clipboard on its own.

### Automatic type detection
- `http(s)://` on a single line → **Link**
- Markdown syntax (`#`, `-`, fences, `[]()`) → **Markdown**
- shared from a password app, or ticked → **Hidden**

---

## Journey 3 — Receiving information (notification + opening)

- **App closed (Android)**: FCM wakes the app → notification "New item — Link from [Windows]". Content stays encrypted until opened.
- **App open / Windows**: WebSocket pushes → native notification. Windows: toast with *Copy / Open / View* actions.

### Context actions by type
- **Link** → "Open" action (browser).
- **Rich text** → "View" action, with a **rendering toggle on the card: Markdown / HTML / Raw**
  (default = the detected `source_format`; can be changed on the fly).
- **Image file** → thumbnail + "Save" / "Share" / "Open".
- **Other file** → icon + size + "Save".
- **Hidden** → notification "Hidden content · <device>" **with no preview**, "Copy" action → clipboard **auto-cleared after ~30 s**.
  A file can also be hidden (no thumbnail, no preview).

### Deletion (two scopes, on both sides)
Two explicit options on each item, available **on any device (sender or receiver)** — this is the user's own
network of devices, with no permissions between them:
- **"Delete from this device"**: erases the local copy only; the others keep theirs.
- **"Delete everywhere"**: erases on all devices + purges the blob on the relay (with
  confirmation, especially for hidden content).

---

## Journey 4 — Browsing history (the feed)

Single screen: reverse-chronological list.
- Card: type icon, preview (hidden for hidden content → "Show"), source device, time, sync status.
- Actions per card: Copy, Open, View rendering, **Delete from this device** / **Delete everywhere**.
- Filters: All / Links / Markdown / Hidden.
- Search: **local only** (the relay holds ciphertext only).
- Hidden content: hidden by default, revealed on tap, re-hidden after reading.

---

## Journey 5 — Sending from Windows (the tray drop zone)

The Windows tray is a **quick drop box**, not just an icon.

### Triggering
- Left click on the tray icon → **small lightweight window (popover)** anchored near the tray.
- Alternative: a global shortcut (e.g. `Ctrl+Alt+V`) opens the same area.

### The drop zone (accepts everything)
- **Drag & drop** of one or more files from File Explorer.
- **Picker** ("Browse…") to select files.
- **Paste** (`Ctrl+V`): text, clipboard image, or file paths.
- **Type/paste text** directly.

### Automatic detection
- Image / binary → **File** item (thumbnail if image).
- Single-line `http(s)://` text → **Link**.
- Text with markdown/HTML syntax → **Rich text**.
- Any other text → **Text**.
- Several files → **a single multi-file item** (grouped by default).

### Expiry setting (at capture)
In the popover: **"Expires in…"**, default depending on the type (see the Expiry section).

### No rendering choice at send time
Rich text is captured **as is**, with no selector. Only a detected format hint is stored
(`source_format`: markdown | html | plain), for information. The rendering choice is made **when viewing**
(see Journey 3).

### Sending
- **Send** button (or `Ctrl+Enter`).
- Files/images are **encrypted locally** then uploaded as encrypted blobs; the message references these blobs. The relay only sees ciphertext.
- Progress bar for large files, then "Sent", and the popover closes.
- No one-click clipboard sending: the user opens the popover and **pastes**.

### v1 limit
- **25 MB per file** (keeps the relay light). Chunking to be considered later.

---

## Journey 6 — Managing devices

"Devices" screen: list (name, platform, last activity), **rename**, **revoke**
(lost/stolen device → removed from the space, no longer decrypts new items → **space key rotation**).

---

## Expiry & deletion

### Set at capture
"Expires in…" setting in the Windows popover and the Android bottom sheet.
Default: **Never**, for all types (no expiry by default). Options: 1 h, 1 d, 7 d, 30 d, never.
- **Text / link / markdown**: **Never** (1 h, 1 d, 7 d, 30 d)
- **Hidden**: **Never** (after reading, 1 h, 1 d, 7 d, 30 d)
- **Files**: **Never** (1 h, 1 d, 7 d, 30 d)
- The relay keeps an undelivered item for at most 30 d, even without expiry.

### What expiry does
1. **Relay**: purges the encrypted blob at expiry (TTL). Keeps the server clean — reliable.
2. **Devices**: erases the received local copy at expiry.

> Note: local expiry is **data hygiene, not a cryptographic guarantee**.
> A recipient could technically keep a copy.
> For personal secrets, this is the right level (it limits leftovers without over-promising).

### Deletion (two scopes)
- **Local**: erases the copy on this device only.
- **Everywhere**: issues a signed **tombstone** `{item_id, deleted_at}` → the relay purges the encrypted blob
  and propagates the tombstone (WebSocket/FCM) → each device erases its copy on receipt.

Available on **any device**, sender or receiver. The relay only sees the
`item_id` (not the content) → E2EE holds.

> **Best-effort** revocation: an offline device applies the tombstone when it reconnects;
> a device that has already decrypted the content may have copied it elsewhere. No guarantee of destruction.

---

## Content types & rendering (contract)

A message carries:
- a **type**: `text` | `link` | `richtext` | `file` | `files`
- for `richtext`: `source_format` = `markdown` | `html` | `plain` (detected format hint at send time, **informational**)
- for `file`/`files`: list of blobs (name, size, MIME type), encrypted
- a `secret` flag (on any type)
- an `expires_at` (set at capture; relay TTL + local erasure)

**Rendering is chosen when viewing, not when sending.** The receiving device
offers a Markdown / HTML / Raw toggle, defaulting to `source_format`.

HTML rendering: **sandboxed webview, no JS, no network, strict CSP**.
`plain` is always available as a safety net.

---

## Cross-cutting UX principles

- **The secret first**: never in clear text without an explicit action, never in a notification, never logged, clipboard auto-cleared.
- **The clipboard is read only when pasting**: no automatic capture, no one-click clipboard sending.
- **Minimal friction at capture**: Android share = 2 taps max.
- **Visible sync status**: sent / delivered; "offline — pending" banner.
- **No account**: identity = the device; the QR code replaces login.
- **Cross-device consistency**: same types, actions and icons everywhere.
