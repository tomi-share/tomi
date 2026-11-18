# Tomi for Windows

The Windows client of Tomi: a notification-area icon, a "popover" window to send and receive, and a full
window (history, devices, settings). Tauri 2 (Rust) + Svelte 5. The Rust backend calls the `runtime` of
`tomi-core` **directly** (SQLite database, sync engine, outgoing queue, history transfer, HTTPS / WebSocket
transport): no cryptographic primitive is written here, and there is no UniFFI access.

The user interface is in French (the default locale, `src/lib/i18n/fr.ts`). French UI labels are quoted as
they appear on screen in this document (for example « Se déconnecter », "Log out of this PC").

## Status

| Topic | Status |
|-------|--------|
| Real core (`src-tauri/src/real_core.rs`) | wired: sending, receiving through the journal, pin, archive, deletion, read-once, files, history transfer |
| Pairing / creating a space | wired to `spec/pairing-v1.md`: create the space (first device), add a device (code, QR code, verification code), join (code, link, `tomi-share://pair#…`), resume after an interruption. See "Pairing" below. Known limit: no camera QR code scanning on this PC (type the code, or paste the link). |
| Leaving a space | wired to the engine (`spec/api-v1.md` §7.9): « Se déconnecter », « Supprimer l'espace », « Effacer quand même », retries, cancellation, resume at start-up. See "Leaving the space" below. |
| Demo mode | **All builds, release included**: on a PC that is in no space, the « Rejoindre un espace » screen offers a discreet « Essayer avec des données de démonstration » button. The real core then enters a demo space (`tomi_core::runtime::unified`, feature `demo`): in memory, no network, already joined; files under `%LOCALAPPDATA%\eu.shaffe.tomi\demo`, erased on exit and at start-up. The feed shows « Mode démo : données fictives, rien n'est envoyé. » and « Quitter la démo » (back to the welcome screen, like « Se déconnecter »). In the demo, pairing is refused and « Ajouter un appareil » is hidden. In addition, a **debug** build started with `TOMI_DEMO=1` runs on the older in-memory core (`fake_core.rs`); nothing is encrypted, sent or kept; pairing there is a short scenario (« Ajouter un appareil »: a code, then a "Pixel 9" that shows the verification code `482 913`). |

## Product decisions applied

- **Pin**: shared between devices, encrypted, last-writer-wins (`spec/pin-contract.md`).
  Commands `pin_item` / `unpin_item` (the name `set_pinned` means "keep the popover open").
  A "Épinglés" section at the top of the feed, most recently pinned first.
- **Archive**: "I am done with this item". Synchronised between devices like the pin
  (`spec/archive-contract.md`). Commands `archive_item` / `unarchive_item` (`TomiCore::archive` / `unarchive`,
  idempotent, unknown ids ignored, immediate local effect); `ItemView` carries `archived` and `archivedAt`
  (camelCase). No swipe gesture on PC: an entry « Archiver » / « Désarchiver » right after « Épingler » in the
  « ⋮ » menu (and on right click) of each card, and a button in the detail bar. An outgoing send still queued or
  failed does not have this entry (a failure must stay visible); an unreadable item and a hidden item (always
  « Masqué », no biometric check) can be archived.
  - **Filtering** (on the Rust side, `view::is_listed`, a port of `FeedFiltering.kt`): an archived item only
    appears under the « Archives » filter; « Tous » and the type filters exclude it (so « Récents » does not
    show it). During a non-empty search under « Tous », archived items are searched too (an « Archivé » badge on
    the card). A hidden archived item stays searchable without its title or content. The pin wins at display
    time (`Item::is_shown_archived` = `archived && !pinned`).
  - **User feedback**: a banner « Archivé · Annuler » / « Désarchivé · Annuler » (5 s, `feed.say` with an action,
    `Flash.svelte`); « Annuler » makes the reverse call and, if archiving had unpinned the item, pins it again
    (`ArchiveSwipe.undo`). Failure: « Impossible de modifier l'archive. Réessaie. ». Archiving never marks an item
    as read. The text of the empty « Archives » state differs from Android (which talks about swiping a card).
  - The toast of an item (tag = its id) is removed when the item is deleted, expires or is archived.
    No keyboard shortcut.
- **Hidden content** (formerly "secret"): the title, text, link and file names are encrypted together with the
  rest; nothing of them goes through the feed, the notifications or the search (a hidden item is only searchable
  by device). « Afficher », « Copier » and « Ouvrir » require Windows Hello (`unlock.rs`, Rust side; fallback: see
  "Fallback without Hello"), valid for 2 minutes (closed when leaving the space; a Hello prompt still open at that
  moment opens nothing). « Copier » goes through the sensitive clipboard (wiped after 30 s).
  « Enregistrer » is refused for a hidden item (no permanent plain-text copy); the toast of a hidden item only has
  an « Ouvrir » button (popover), never « Copier ». The format has no secret fields: a hidden item is a text (plus
  title, link, and files if any).
  « Afficher » on a hidden « Après lecture » item (without files) does not consume it: the front end calls
  `consume_revealed` once the value is really displayed (window in the foreground), and Rust only accepts the call
  while the Hello window is open; if the window lost focus, nothing is consumed. The generic `mark_read` command
  refuses these items.
  **Known limit**: the 2-minute Hello window is not closed when the Windows session is locked (Win+L) or when the
  machine sleeps: Tauri does not expose these events (`WTS_SESSION_LOCK`, `WM_POWERBROADCAST`), and catching them
  would require subclassing the window in `unsafe` code. The window therefore stays open until its end (2 minutes
  after the verification), even if the session was locked in the meantime.
- **Expiration**: « Jamais » (never) by default, hidden items included. Choices: Jamais, Après lecture (hidden items
  without files only), 1 h, 1 j, 7 j, 30 j. A choice that is not offered for this kind of content falls back to
  « Jamais », except « Après lecture » to which a file is added (hidden item): the choice is kept, the send is
  blocked and the message is highlighted.
- **History transfer**: everything (hidden items and files included), without checkboxes, a single yes / no choice,
  sealed for the receiving device. Devices tab.

### Fallback without Windows Hello

If Windows Hello is **unavailable** (no sensor and no PIN, disabled by policy), the strong verification (show / copy /
open a hidden item, delete the space, « Effacer quand même », revoke) falls back to the **password of the current
Windows account** (`account_auth.rs`: the system credential prompt `CredUIPromptForWindowsCredentials`, then
`LogonUserW`). This fallback is **weaker** than Hello (no biometric factor) but usable everywhere; golden rule 3
(secrets) is unchanged.
Safeguards: Hello stays the priority and is the only one used when it is available (no fallback after a failure, a
cancellation or a busy device); the verified account must be the current user (the token's SID is compared with the
SID of the process); an empty password counts as a failed try (« compte sans mot de passe » only if Windows answers
"restricted account" or if the field was empty on all 3 tries); 3 tries, then, cumulated across actions, 3 failures
block the fallback for 60 s without asking anything (« trop d'essais »); a locked-out account is reported as such;
the verification uses `LOGON32_LOGON_INTERACTIVE` (cached passwords of Microsoft / domain accounts while offline);
the password is wiped from memory right away. After 150 s the action is refused; the prompt may stay on screen (it
is drawn by the system, probably by another process: closing it is only attempted on a best-effort basis), and its
answer is ignored.

The fallback also applies when Hello declares itself unavailable at the time of the request (Remote Desktop, sensor
removed), not only on a PC that never had it.

## Interface (charter and parity with Android)

- **Theme**: the tokens of `design/tomi/tokens.json` are CSS variables (`src/app.css`), light and dark. The theme
  follows Windows by default; Réglages › Thème forces Clair or Sombre (display preference kept in the webview's
  `localStorage`, shared by both windows). Night `#24324A` and sun `#FFC85C` are only used for the symbol
  (`Logo.svelte`, `tomi.ico`); the interface uses `brand`. No circular-arrow icon and no spinner: linear progress
  bars only.
- **Fonts**: Inter and JetBrains Mono (SIL OFL 1.1), copied from `design/fonts/` into `src/assets/fonts/` with their
  licence texts, bundled by Vite (no network, CSP unchanged). Icons: Material Symbols Outlined (Apache 2.0), paths in
  `src/lib/icons.ts`.
- **Text**: two modes as on Android, « Aperçu » and « Markdown » (the literal source, only if the item has a Markdown
  representation). HTML is never shown as source; it is sanitised on the Rust side (`render.rs`) then displayed in an
  `<iframe sandbox>` with no script and no network. A `settings.json` written by an earlier version (modes `markdown` /
  `html` / `plain`) is still read.
- **Clicking a card**: the natural action of the type (`src/lib/itemLogic.ts`, a port of `CardTap.kt`); everything
  the click does is also in the « ⋮ » menu (right click on the card) or in a visible button, for the keyboard.
- **Full-screen views**: detail (« Lire la suite »), image viewer (zoom, drag, arrows) and code file (colouring
  `src/lib/highlight.ts`, search, line wrapping). The last two read the file through `read_image` / `read_code_file`
  (never for hidden content: « Afficher » then « Ouvrir »).

## Pairing

The protocol and the network orchestration are in `tomi-core` (`runtime::pairing`); this client only drives them and
displays them. No key, no space key, no `ISK` goes through the application: the core provisions the device itself.

| Screen (Devices tab, and Welcome on first launch) | Core call |
|---|---|
| **Créer mon espace**: installation token or creation link (hidden, never kept) + « Relais » field + PC name; the chosen relay is shown before « Créer » | `Engine::create_space_at` (§10) |
| **Ajouter un appareil**: code `4821-crayon-renard`, QR code of the link `https://<relay>/l/pair#<code>`, countdown, then the verification code to compare | `Engine::start_invitation` → `InviteSession` (§7.2) |
| **Rejoindre un espace**: code or link typed/pasted (word completion from 4 letters) + « Relais » field (folded under « Autre relais » if the PC has one, required otherwise, pre-filled by a link), then the verification code with the relay spelled out, history question (yes by default), « Les codes sont identiques » | `Engine::join_space_at` → `JoinSession` (§7.3, §9) |

- **Who calls what.** Core sessions block (long waits of 25 s): `pairing.rs` (`PairingHost`) gives each one a thread and
  publishes a `PairingView`; the window never waits for it, it reads it (`pairing_state`) and receives the
  `pairing-changed` event: a reloaded page finds the same screen again, and no request waits for minutes in the window.
  Only the session's thread calls `poll` / `confirm` (the core refuses a `confirm` during a `poll`): the commands record
  an *intent* (confirm with the history answer, cancel) and wake this thread. While the user compares the codes, the
  thread does not stay on the relay: it checks the channel every 1.5 s (`poll(0)`), so that the other device's refusal
  is seen and a click is taken immediately.
- **Tauri commands**: `pairing_options`, `pairing_state`, `pairing_create_space`, `pairing_invite`, `pairing_join`,
  `pairing_confirm`, `pairing_cancel`, `pairing_leave` (the screen is left), `pairing_dismiss`,
  `pairing_complete_word`, `take_pair_link`. Errors are `AppError`s (`error.pairing_*`), without any code or frame byte.
- **Expired code, code that led nowhere**: the screen shows a new one by itself (two renewals at most: a device can open
  three channels in a row, then one every six minutes, `rl_device_pairing`), then waits for the user. A refusal *after*
  the verification code has been shown is final.
- **Leaving the screen** (switching tab, closing the window) abandons a displayed code or codes being compared, and
  closes the channel (§11.2): the code does not stay valid for a screen that is no longer there. A pairing that **both
  users have already confirmed** continues in the background (nothing secret is displayed any more, and cancelling would
  leave a device that may already be registered in limbo); the screen finds it again. **Quitting Tomi** closes the channel
  the same way, except for a new device whose verified batch is already sealed: it stays in place and the next start-up
  settles it (« Reprise »).
- **Deep link** `tomi-share://pair#<code>`: the code (re-read by the core, shown in its canonical form) fills the
  « Rejoindre » form; **nothing starts** before the click (§4.4). A PC that is already paired ignores it. The link
  `https://…/l/pair#…` does not launch the application on Windows: it is pasted into the field.
- **Resume** (§11.4, §10.2): at start-up, if a verified batch or the keys of a founder are sealed in the database,
  `resume_pairing` settles it with the relay (a spaced new try if the relay does not answer). The result is shown once
  (« L'appairage interrompu est terminé » / « n'a pas abouti »).
- **Secrets.** The code, its link, the QR code and the verification code are `SecretText`s (wiped from memory, `Debug`
  hidden); they leave the view as soon as the screen no longer shows them. Nothing is logged, copied to the clipboard or
  written to disk; the installation token is kept nowhere. The screen lives in the full window, which is excluded from
  screen captures.

Optional end-to-end test against a real relay (ignored by default): space creation, invitation, join with history, refusal,
wrong code.

```powershell
cargo build -p tomi-relay                     # at the root of the repository
cd apps/windows/src-tauri
$env:TOMI_E2E_RELAY_BIN = "..\..\..\target\debug\tomi-relay.exe"
cargo test --lib -- --ignored pairing_over_a_real_relay
```

## Leaving the space

The engine of `tomi-core` (`runtime::leave`) does everything: it writes the marker, sends `DELETE /v1/me` or
`DELETE /v1/space`, and **only erases on a positive proof from the relay** (§7.9.1). `real_core.rs` never decides what
counts as a proof and never calls the erasure "by hand": it runs the calls outside the async threads, converts the types
one by one (`DepartureOutcome`, `DepartureView`, `PendingReason`, `RefusalReason`; no wildcard, a variant added by the
engine breaks the compilation) and passes **all** errors through `map_failure` (`Rejected` = « verdict trop vieux, rien
n'a été effacé » (verdict too old, nothing was erased), `Storage` = « échec, réessaie » (failed, try again)). The rules of
the interface (Windows Hello before « Supprimer l'espace » and « Effacer quand même », anti-tap, retries, banner) are in
`departure.rs`; `departure_host.rs` cleans up what the application keeps outside the core.
**Uninstalling without « Se déconnecter » first** leaves the device a member of the space until you remove it from another
device.

- **Calls.** `leave_space`, `delete_space`, `retry_departure`: paired devices only (`NotPaired` otherwise).
  `continue_departure`, `cancel_departure`, `erase_without_proof` and the reads go through `departure_call` (built on
  `engine_or_err`): `continue_departure` also runs on an unpaired PC, where it only removes a marker left there. A
  `wiping` interrupted by a stop is completed when the engine opens, not by this call. `departure_available()` is true as
  soon as the engine exists (false only if the database cannot be opened).
- **After an `Erased`** (proof from the relay or « Effacer quand même »): the doorbell is stopped by a flag **specific to each
  loop** (`Doorbell`, shared by `start_doorbell` and the pairing hook; the next one starts with a new flag), the decrypted
  feed kept in memory (`RealCore.snapshot`) is emptied, and the decrypted copies in `open/` are erased. **No
  `RealCore::stop`**: it would stop the change bridge and the clean-up, which a following pairing needs. The rest
  (clipboard, toasts, `staging/`, home screen) is `departure_host::finish_erasure`.
- **« Annuler » before the marker is written.** The engine writes the marker in its first transaction but only tells the
  application when the attempt ends. So that an « Annuler » never claims to have worked without effect: an in-flight gesture
  is tracked (`Gestures`); if « Annuler » arrives before the marker, the request is kept (answer `true` promised) and
  executed as soon as the marker appears (checked every 40 ms), or the gesture does not start at all if it has not yet called
  the engine. The appearance of the marker is announced (`CoreEvent::DepartureChanged` → `departure-changed`). As with a
  cancellation during an attempt, a request already sent goes to the end and erases nothing (§7.9.2).
- **Start-up.** `LeaveFlow::resume_at_startup` runs **before** the pairing resume (`TomiCore::resume_pairing`, called by
  `setup` in `lib.rs`; `RealCore::start` no longer starts it).
- **Tests.** Conversion table, proof (`Erased` + host clean-up), network cut (nothing erased, « Annuler »), last device, space
  not found → « Effacer quand même », « Annuler » before the marker, doorbell. End to end against a real relay (optional,
  ignored by default, same mechanics as pairing):

```powershell
cargo build -p tomi-relay                     # at the root of the repository
cd apps/windows/src-tauri
$env:TOMI_E2E_RELAY_BIN = "..\..\..\target\debug\tomi-relay.exe"
cargo test --lib -- --ignored departure_over_a_real_relay
```

## Unreadable items

An item that the core cannot read is kept, never deleted. `ItemView.unreadableReason` is `"updateRequired"` (an update of
Tomi will make it readable) or `"other"` (key not arrived yet, key replaced by a rotation, failed decryption: a sync or an
update may help), and `null` for a readable item. Same two-sentence split as Android
(`unreadable_update_required` / `unreadable_decryption_failed`). The toast of such an item only shows « Élément de {appareil} »
and the reason, never any content, with the single button « Ouvrir ».

## Local storage and secrets at rest

- Database and files encrypted by the core, in `%LOCALAPPDATA%\eu.shaffe.tomi\` (`tomi.db`, `store\`). Titles, texts, links,
  names and file contents are sealed there; only the metadata listed in `crates/core/src/runtime/store.rs` (ids, dates,
  flags) stays in clear.
- The keys (device, local key, space keys) are sealed in the database under a **wrap key** of 32 random bytes, kept in
  `wrap.key` **sealed by DPAPI** for the current Windows user (`src/wrapkey.rs`). Known gap with `spec/api-v1.md` §7.1: no
  hardware-backed key and no user presence check on each use. DPAPI protects against theft of the files (another PC, another
  account), not against another process running as the same user.
- If the database exists but not its key (or if Windows cannot open it), **no new key is created**: the core reports itself
  as « indisponible », the window explains it, nothing is deleted.
- Received files are only decrypted on request (open, save, drag, copy), in `…\store\files\cache\` and `…\open\` (named like
  the file). The `open\` file is a **hard link** (`std::fs::hard_link`) to the one in the core's cache: plain bytes are only
  written once (golden rule 3), under two names; a real copy is only made if the link is impossible (another volume, a file
  system without hard links). Both names are erased (the core for `cache\`, `wipe_dir` for `open\`): removing one does not
  remove the other. These files are erased one minute after the last use of Tomi, on closing, and at the next start-up.
  "Use" = popover shown (if it is pinned: only while it has focus) or full window in the foreground: a forgotten pinned popover
  does not keep copies indefinitely.
  - **Read-only.** The file is set to read-only (`FILE_ATTRIBUTE_READONLY`, shared by both names) before the link: the core
    renders its cache file without checking it, so an editor that writes in place (VS Code, Notepad) must not be able to change
    it; it refuses and offers « Enregistrer sous ». Deleting a read-only file works (`remove_file`, `remove_dir_all` and
    `wipe_dir` of the Rust standard library ignore the attribute; tested, including by the purge of the real core).
    « Enregistrer » removes the attribute from the delivered copy; a file **pasted or dropped** from outside Tomi keeps the
    attribute (remove it in the file's properties). The Mark-of-the-Web is written by briefly clearing the attribute (a stream
    cannot be written to a read-only file).
  - **File open in another program.** A file opened without delete sharing (viewer, editor) prevents deleting the name it was
    opened under; the other name is not blocked (checked on Windows 10), but both names remain as long as the file is open, and
    the plain bytes with them. Cases that then fail, **without data loss**: the purge (the core returns "still in use", the clean-up
    starts again the next minute, on closing and at start-up), deleting an item (`remove_file` of the core, silent: the encrypted
    copy is deleted, the plain copy goes at the purge), and leaving the space (`wipe_files`: the marker stays `wiping`, see
    "Leaving the space": the « effacement en cours » banner stays displayed, neither « Réessayer » nor « Annuler » changes
    anything, the resume is automatic at the next start-up; a file still open at that moment does not block a new pairing).
  - A real copy is only used if the link is impossible (another volume, a file system without hard links, rights); this copy
    never overwrites anything. A name already taken is deleted, then the link is tried once more, otherwise it is an error.
- **« Envoyer vers › Tomi »** (`%APPDATA%\Microsoft\Windows\SendTo\Tomi.lnk`): written by the application on the first launch,
  when the executable changes path, or when there is a **new file** at the same path (update, reinstall: creation date
  remembered in `settings.json`, `sendtoStamp`). Not at every launch: whoever deleted it on purpose does not see it come back.
- **Uninstalling**: the NSIS uninstaller deletes this shortcut (`src-tauri/installer-hooks.nsh`, `NSIS_HOOK_POSTUNINSTALL`,
  except during an update). Reinstalling the application recreates it (new executable file, so a new creation date).
  **Remember to « Se déconnecter » before uninstalling** (Devices tab): otherwise the device stays a member of the space until
  you remove it from another device. The uninstaller does not display it (message not added: accented text and cancellation
  of an NSIS uninstaller cannot be tested without an installer build).
- Working copies of pasted or dropped files (`staging\`, in the user's own `%LOCALAPPDATA%`) are in clear for as long as the
  send takes (after the 3 s cancellation window, the core encrypts them into the database). A send that fails keeps its files
  until the retry, the rejection or the next launch. This folder is emptied on closing **and** at start-up.

## Development

```powershell
cd apps/windows
npm install
npm run check          # svelte-check
npm run build          # the front end (dist/)
npm run tauri dev      # dev window (TOMI_DEMO=1 for the demo mode)
npm run tauri build    # NSIS installer

cd src-tauri
cargo fmt -- --check ; cargo clippy --all-targets -- -D warnings ; cargo test
```

The Cargo workspace of `src-tauri` is standalone (excluded from the root workspace; `Cargo.lock` and `target` are local);
`tomi-core` is taken with `default-features = false, features = ["runtime", "http", "demo"]` (no `ffi`; `demo` for the demo space, in every build).

**The relay is configurable** (`spec/pairing-v1.md` §3.3, §4.3, §4.4, §10): a release build has no default relay. It comes
from a pairing link `https://<relay>/l/pair#<code>` (pasted or scanned), a creation link `https://<relay>/create#<token>`,
or the « Relais » field of the forms (`https://relay.example.org`); the `tomi-share://pair#<code>` fallback does not carry one.
**A relay read this way is not trusted**: the verification screen shows it in full with the 6-digit code, and nothing is saved
before the confirmation (the core guarantees it). A PC already in a space keeps the relay of its space.

- Default relay of a build: **compile-time** environment variable `TOMI_DEFAULT_RELAY_URL` (read by `option_env!`, for example
  `$env:TOMI_DEFAULT_RELAY_URL = 'https://relay.example.org'; npm run tauri build`). The CI fills it from the repository variable
  `TOMI_RELAY_URL` (the same one as for the APK). If absent, empty or invalid: no default relay, the app asks for one.
- A debug build can target a local relay at launch: `TOMI_RELAY_URL=http://127.0.0.1:8080`.
