# Tomi for macOS: how it works, decisions, limits, shared parts

**Status: decisions approved by the owner on 16 November 2026**, with four changes compared with the proposal (A7
clarified, A11 reversed, A12 without TestFlight for now, A14 with Intel). This document is the reference for the macOS
target, as `docs/archive/apple-spec-implementation.md` is for iOS. Progress: section 7.

Sources: the code of `apps/windows` (inventory of 16 November, not only its README), the code of `apps/apple`,
`docs/FEATURES.md`, `AGENTS.md`.

---

## 1. On one page

- **What it is.** The Mac client of Tomi: one more device in the space, just like a PC. It pairs, sends, receives,
  notifies. It changes nothing in the protocol, the relay or the core.
- **The usage comes from Windows.** The Mac is a *workstation*: an icon in the menu bar, a **popover** to send and
  receive quickly, a **full window** for the history, a **global shortcut**, drag and drop, clickable notifications.
  The app stays open all the time, with its windows closed.
- **The code comes from iOS.** The macOS target lives in `apps/apple`, same XcodeGen project, in SwiftUI with AppKit
  where SwiftUI is not enough (closed decision, `apple-spec-implementation.md` §1). It calls the Rust core through
  UniFFI, like iOS and Android. It is **not** a port of the Tauri app, and it is not Mac Catalyst.
- **Consequence.** When Windows and the mobiles differ on a rule (number of files, secret detection, length of a device
  name), the Mac follows **the mobiles**, because it shares their code (`TomiKit`). When they differ on a gesture
  (popover, keyboard, dragging, "Send to"), the Mac follows **Windows**, translated into the conventions of the Mac.
- **What the Mac does better than Windows**, because macOS allows it: authentication window closed on lock and on
  sleep, doorbell resumed on wake, dropping a file on the icon, an entry in the system "Share" menu, Quick Look,
  keyboard navigation in the feed.

---

## 2. How it works

### 2.1 The surfaces

| Surface | Role | Windows equivalent |
|---|---|---|
| **Menu bar icon** | Always present while Tomi runs. Click: opens or closes the popover. Right-click (or Option-click): menu Open / History / Settings / Quit. Unread dot on the icon. Accepts dropped files. | notification area icon |
| **Popover** | One column, anchored below the icon (≈ 400 × 600). Composer at the top, status banners, search, filters, feed. Closes on a click elsewhere or with Esc, unless it is pinned ("keep open"). | `popover` window |
| **Full window** | The feed at full size, the Detail, the Devices, pairing, the first launch. A single window. Closing it does not quit Tomi. | `history` window (tabs) |
| **Settings** | Standard Mac settings window (`⌘,`). | Settings tab |
| **Notifications** | System notifications, with actions. | toasts |

The popover and the window show the same state: a single process, a single open core, a single state container
(`AppContainer`). As long as the Mac is in no space, a click on the icon opens the full window on the welcome screen,
as on Windows (it also opens by itself once at launch).

Done in milestone M-4, with three clarifications compared with the table:
- **One surface at a time.** While the full window is open, the icon (and the global shortcut) brings it to the front
  instead of opening the popover: only one set of screens follows the core at a time. Difference from Windows, where
  both coexist; "Open in a window" is in the "⋯" menu of the feed and in the menu of the icon.
- **The popover is a panel**, not an `NSPopover`: an untitled window below the icon, centred on it. An `NSPopover` has
  no toolbar, so neither a back button nor screen buttons for the Detail, the Devices or the code screen.
  Consequences: no arrow pointing to the icon; the panel closes when switching to another app, on Esc (the whole
  panel, even from the Detail: "close the top layer" first is still to do with the keyboard, M-8), or on a second
  click on the icon; it stays open during a Touch ID prompt. A `tomi-share://` link or a notification received with
  the windows closed opens the panel (the window if it is open) on the requested screen.
- The settings have their own AppKit window (opening the SwiftUI settings scene programmatically does not work on
  macOS 27); "Settings…" in the app menu (`⌘,`) opens it.

**Presence in the Dock.** Tomi is a menu bar app: no icon in the Dock as long as no window is open; the Dock icon
appears while the full window is open (the unread count on that icon is still to do, M-5), so that `⌘Tab` and the menu
bar work normally (decision A3).

**Life cycle.**
- Opening at login (`SMAppService`), enabled once by default at the first launch and adjustable afterwards (decision
  A5; the announcement on the welcome screen is still to write, M-6). Launched that way, the app shows nothing: only
  the icon.
- Closing the window or the popover leaves Tomi running. `⌘Q` and "Quit" really quit: nothing is received or notified
  any more until the next launch (there is no push, see 7.2).
- A single instance (macOS guarantees it); launching Tomi again opens the popover.

### 2.2 Sending

The composer is the one of iOS (same rules, `SendViewModel`), dressed for the Mac: text area, attachments, "Hide
content" switch, expiry choice, "Send" button.

| Input | How it works |
|---|---|
| **Paste** (`⌘V`, Edit menu) | The only read of the clipboard, never automatic. Text, link, rich text (the HTML is kept), image, files copied in the Finder. Content marked "concealed" by a password manager (`org.nspasteboard.ConcealedType`) locks "Hide content". |
| **Drag and drop** | On the whole full window; **on the menu bar icon** (opens the prefilled popover, still to do) and on the Dock icon when it is visible (still to do). Not on the popover itself, which closes as soon as one switches to the Finder to pick a file there. Prefill only: nothing leaves without "Send". |
| **File picker** | Standard open panel; photo library through the Photos picker. Each chosen file is copied at once into the working folder of the app. |
| **Global shortcut** | Opens the popover on the feed; closes it if it is already in front. `⌘V` on the feed then opens the send screen with what was pasted, `⌘↩` sends (shortcut, paste, send). Adjustable (decision A4): Control, or two keys among Command, Option and Control. |
| **System "Share" menu** | macOS share extension: Safari, Finder, Photos, any app. Same drop box as the iOS extension; as Tomi runs all the time, the send leaves at once. It is the equivalent of "Send to › Tomi". |
| **Services menu** | "Send with Tomi" on a selected text or on files (contextual menu of the Finder). Prefills the composer. |
| **`tomi-share://send?…`** | Prefills, never sends (same refusals as everywhere: no secret in a URL). Same parser as iOS (`DeepLink`). |
| **Shortcuts / Spotlight** | The "Send with Tomi" intent of iOS (App Intents) works as is. |
| **Command line** | `open -a Tomi fichier.pdf` prefills the composer. |

After "Send": "Sent · Undo" banner for 3 s (the item has not left yet), then the send queue of the core with retries; a
failed send stays visible with "Try again". Offline: "Offline · waiting" banner (as on the mobiles).

Send limits: those of the mobiles, through the shared code (25 MB per file, 100 MB per send, **20 files**).
Thumbnails: as on iOS, including those of PDFs, videos and documents (Quick Look).

### 2.3 Receiving and viewing

- **Feed**: Pinned / Recent sections, All / Links / Text / Files / Hidden / Archives filters, search, "unread" dot,
  automatic marking as read of a card that stays visible. Same rules as everywhere (`FeedFiltering`, `FeedSections`,
  `SeenTracker`).
- **Click on a card = the natural action of the type** (`CardTap`): link → open, Hidden → Show, long text → Detail,
  file → open. Everything the click does is also in the contextual menu (right-click) and on the keyboard.
- **Detail**: the floating bar of iOS (Pin, Share, Archive, "⋯", Copy as the main button); "Delete…" (from this device
  / everywhere) is in "⋯". Archiving from the Detail goes back to the list, with "Archived · Undo". The files of a batch
  each have their own menu (open, share, save, delete).
- **Files**:
  - `Space` on a card of the feed: opens what it contains (image viewer, file viewer, code screen);
  - "Open": the default application; an executable is never launched, it is shown in the Finder;
  - "Save": into `Downloads/Tomi` without a dialog (folder adjustable in the Settings);
  - **dragging a file out of Tomi** to the Finder, Mail, etc.;
  - "Share": the share menu of the Mac (AirDrop, Mail, Messages…);
  - built-in image viewer and code screen, as on iOS.
- **Copy**: text, rich text, image, files, to the clipboard of the Mac.
- **Archive**: contextual menu and Detail bar (no swipe, as on Windows).

### 2.4 Notifications and real time

- Tomi runs all the time: the WebSocket doorbell of the core stays open, with the windows closed. A fallback sync runs
  every 5 minutes, and **on every wake of the Mac and every return of the network** (Windows does not do it).
- System notification when an item arrives: title by type, preview, origin device, image thumbnail, "Copy" and "View"
  actions. Logic shared with iOS (`NotificationLogic`): close arrivals grouped, imported history never notified,
  nothing if Tomi is in front.
- **Hidden**: never any content nor title. The notification only says that an item arrived; the click asks for Touch ID
  or the password of the Mac, then copies (as on iOS), without opening a surface.
- **Click on a notification**: the obvious action of the type, as on Windows (link → open, short text → copy, single
  file → open, the rest → popover on the item) (decision A9).
- **Unread**: dot on the menu bar icon; count on the Dock icon when it is visible.
- The notification of an item is removed when the item is deleted, expired or archived.

### 2.5 Hidden content and security

| Topic | On the Mac |
|---|---|
| Show, copy, open, share a Hidden item | The native authentication of macOS, through the same code as iOS (`BiometricGate`, `LAContext`): the system offers what the Mac has, Touch ID, Apple Watch or the password of the Mac. Valid for 2 minutes. |
| Closing the 2-minute window | On screen lock, on sleep, on user switch, on leaving the space, and with the purge of the plaintext copies (one minute after the last surface used). (Windows cannot do it.) |
| Destructive actions (removing a device, deleting the space, "Erase anyway") | Same gate, every time, never remembered: biometrics only if the Mac has them (iOS rule), otherwise the password of the Mac (decision A7). |
| Re-hiding a shown Hidden item | When the window loses focus, when the popover closes, on lock, and after a delay (decision A8). Text not selectable. |
| Screenshots and screen sharing | The window is excluded from captures (`NSWindow.sharingType = .none`) as long as it shows a sensitive screen (pairing, invitation, space creation, file or image of a Hidden item, entry of hidden content) or the content of a shown Hidden item (decision A6). |
| Copying a Hidden item | Clipboard marked concealed and transient, **excluded from Universal Clipboard** (`currentHostOnly`), cleared after 30 s only if it has not changed in the meantime, and when the app quits. |
| "Save" a Hidden item | Refused (no permanent plaintext copy), as everywhere. Dragging out of Tomi: refused too. |
| Wrapping key of the database | Keychain of the Mac ("data protection" keychain, never synced, this device only). |
| Plaintext copies of attachments | A single copy (hard link to the cache of the core, read-only). Cleared one minute after the last use of Tomi, on lock, on quit and at startup (decision A10). |
| Markdown / HTML rendering | The one of iOS: no remote resource, HTML in a `WKWebView` with no script and no network. |
| Logging | Never any content (golden rule 3). |

### 2.6 Pairing, devices, departure, demo

Same screens and same rules as iOS (`PairingViewModel`, `InviteViewModel`, `DepartureViews`), in the full window.

- **Create the space** (installation key), **join** (scanned QR, code typed or pasted, `tomi-share://pair#…` link that
  fills the field without ever joining on its own), **add a device** (code + QR shown by the Mac, which the phone
  scans), verification code to compare, history question.
- **Linking the Mac from a mobile already in the space** (decision A11): the mobile opens "Add a device" and shows its
  QR; on the Mac, "Scan the QR code" opens the camera of the Mac and reads the QR: as on iOS, the scan counts as "Join"
  (scanning is the gesture), whereas a received link or a pasted code wait for the click on "Join". Both screens then
  show the verification code to compare. The camera is only requested on the click on "Scan the QR code". A Mac with
  no camera (Mac mini, closed laptop) keeps entering the code. The iOS scanner (`DataScannerViewController`) does not
  exist on macOS: the Mac one is written with AVFoundation and Vision, behind the same interface.
- The Mac announces itself as `platform = "macos"` (already accepted by the core and known to the apps:
  `DevicePlatform.macos`). Proposed name: the name of the Mac.
- **Devices**: list, rename, remove. **Departure**: Disconnect, Delete the space, "Erase anyway", banner and retries.
  **Demo**: discreet "Try with sample data" entry on "Join your space", "Leave the demo" banner.
- **Unreadable data** (database without its key, for example after a Time Machine restore on another Mac): the iOS
  screen, "Try again" / "Reset Tomi".
- **First launch**: content specific to the Mac (global shortcut, Share menu, opening at login, notification
  permission).

### 2.7 Keyboard and menus

Standard menu bar (Tomi, File, Edit, View, Window, Help). Every action has a menu entry.

| Action | Mac | Windows |
|---|---|---|
| Open / close the popover | global shortcut (A4) | `Ctrl+Shift+Space` |
| Send | `⌘↩` | `Ctrl+Enter` |
| Search | `⌘F` | button (popover), `Ctrl+F` (code screen) |
| Copy the item | `⌘C` (with no text selection) | `Ctrl+C` |
| Open | `⌘O` | `Ctrl+O` |
| Save | `⌘S` | `Ctrl+S` |
| Quick Look | `Space` | none |
| Delete… | `⌘⌫` | `Del` |
| Undo from the banner | `⌘Z` | `Ctrl+Z` |
| Back / close the layer | `Esc`, `⌘[` | `Esc`, `Alt+←` |
| Move through the feed | `↑` `↓`, `↩` = natural action | none |
| Settings | `⌘,` | tab |
| Close the window / quit | `⌘W` / `⌘Q` | close / Quit menu |

### 2.8 Settings

Global shortcut · Open Tomi at login · Notifications · Save folder · version · Quit Tomi. "Disconnect" is not there:
it is in the "⋯" menu of the feed, as on iOS. Theme: the one of the system, as on iOS (no manual setting in v1). The
Preview / Markdown mode is remembered from a card, as elsewhere.

### 2.9 Storage

- **Sandboxed** app: everything lives in its container
  (`~/Library/Containers/eu.shaffe.tomi/Data/Library/Application Support/eu.shaffe.tomi/`: `tomi.db`, `files/`,
  `demo/`, working folders), excluded from backups. Nothing to write: these are the paths of `CorePaths`.
- Drop box of the share extension: container of the app group of the Mac, `<team>.eu.shaffe.tomi` (the iOS one,
  `group.eu.shaffe.tomi`, would require a provisioning profile on macOS).
- Settings: `UserDefaults` of the app.
- Uninstalling without "Disconnect" leaves the Mac a member of the space until it is removed from another device (as
  on Windows).

---

## 3. From Windows to the Mac: translation table

| Windows (actual code) | Mac | Note |
|---|---|---|
| Notification area icon, badge, menu | Menu bar icon, dot, menu | |
| Popover 400×600, anchored to the icon, hidden on focus loss, pinnable | Same | |
| Full window with History / Settings / Devices tabs | Full window + standard Settings window | Mac convention |
| Feed as a grid (columns ≥ 320 px), full-frame Detail | Feed as a readable column, Detail in the same window | decision A2 |
| `Ctrl+Shift+Space` | adjustable global shortcut | A4 |
| Launch at login, enabled once by default | Login item (`SMAppService`) | A5 |
| "Send to › Tomi" (`Tomi.lnk`) | Share menu + Services | better integrated |
| Paste in the composer only | Same (`⌘V`) | |
| Drop on the composer only | Drop on popover, window, icon, Dock | |
| Dragging a file out of the window | Same | |
| WinRT toasts, Copy / Open buttons | System notifications, Copy / View | |
| Windows Hello, fallback to the account password | Touch ID, fallback to the login password | same logic |
| 2-minute window **not closed** on lock | Closed on lock and on sleep | Windows gap A.8 absent |
| `contentProtected` on both windows | `sharingType = .none` on a sensitive screen | A6 |
| Clipboard: exclusion formats, clearing at 30 s | Concealed / transient types, current host only, clearing at 30 s | |
| DPAPI (`wrap.key`) | Keychain | |
| `open/` as a read-only hard link, 1 min purge | Same | A10 |
| Mark-of-the-Web on files taken out | Quarantine attribute | to check (set by the sandbox) |
| No QR scan | QR scan with the camera of the Mac | A11: the Mac follows iOS |
| Manual history transfer (Devices tab) | At pairing only | the Mac follows iOS (open gap A.24) |
| "Inactive" hint of a device | Hint under the row of the device, with "Remove" | as on iOS (added to both on 17 November 2026) |
| No updater, unsigned NSIS installer | Sandboxed app signed locally; TestFlight later | A12 |
| `fr.ts` written by hand | Texts generated from `strings.xml` | as on iOS |
| System / Light / Dark theme | System | as on iOS |

What Windows does not have and the Mac will not have either in v1: the rule "the active device rings alone" (described
in `PROJECT.md`, implemented nowhere), push.

---

## 4. What is shared

### 4.1 By layer

| Layer | Where | Shared by | What the Mac changes there |
|---|---|---|---|
| Protocol, formats, frozen vectors | `spec/` | all | nothing |
| Relay | `apps/relay` | all | nothing (`"macos"` is already a valid platform; no APNs) |
| Core: crypto, sync, storage, pairing, departure, demo | `crates/core` | all | **nothing** (no platform-specific code in the core) |
| Core contract | `crates/core/src/ffi.rs` (UniFFI) | Android, iOS, **Mac** | nothing. Windows, for its part, calls the internal Rust API without UniFFI. |
| Core packaged for Swift | `Packages/TomiCore` (XCFramework) | iOS, **Mac** | add a macOS slice to the build script |
| Core adapter, keychain, paths, send queue, thumbnails | `Packages/TomiRuntime` | iOS, **Mac** | declare macOS; `platform`; "data protection" option of the keychain |
| Pure logic (ported from Android, tested case for case) | `Packages/TomiKit` | iOS, extension, **Mac** | nothing (already compiles on macOS) |
| Charter: colours, typography, icons, logo | `design/tomi/tokens.json` → `Packages/TomiDesign` | all (Android, Windows through their own outputs) | nothing (AppKit branch already there) |
| Texts, every language | `strings.xml` files of Android → `Localizable.xcstrings` | Android, iOS, **Mac** | keys specific to the Mac (`macos_…`) for what is said differently |
| View models and SwiftUI screens | `apps/apple/Tomi/` | iOS, **Mac** (in part) | see 4.2 |
| Desktop shell (icon, popover, shortcut, menus, Services) | new `apps/apple/TomiMac/` | Mac only | to write |
| Demo files | `apps/apple/DemoAssets/demo` | iOS, **Mac** | nothing |

Rules the Mac therefore inherits **from the mobiles** and not from Windows: `CardTap`, `FeedFiltering`, `ExpiryPolicy`,
`SecretDetector` (iOS heuristic), `LinkExtractor`, `ShareLimits` (20 files), `NotificationLogic`, `MarkdownBlocks` and
`MarkdownGuard`, `DeepLink`, device name of 40 characters, 3 s undo.

### 4.2 Code of the iOS app reused

File-by-file inventory of the 58 files of `apps/apple/Tomi/`:

- **27 compile as is on macOS**: all the view models (feed, send, pairing, invitation), the Markdown and text
  rendering, the departure, the demo banner, the biometric gate, the network monitor, the reception of shares, the
  "Send with Tomi" intent.
- **≈ 20 need a small `#if os(macOS)` branch**: opening a URL, clipboard, images (`UIImage`), navigation bar modifiers,
  application delegate, HTML view (`NSViewRepresentable`).
- **≈ 10 must be rewritten**: anti-capture layer (an iOS-specific trick), QR scanner, image zoom, viewers (Quick Look
  and the AppKit video player), share and export, background tasks (useless on the Mac), floating bar.

Sharing mode chosen: the Mac target compiles the shared files of `Tomi/` by their path (as `TomiShare` already does for
four files) and adds `TomiMac/` for what is its own. No mass move and no new package in v1 (decision A13).

### 4.3 What is **not** shared

- **No code with Windows.** The Windows app (Svelte + Rust) is only a behaviour reference. The two only meet through
  the core, the spec and the charter.
- **Nothing between the Mac and the iPhone of the same person outside the relay.** No iCloud, no Handoff, no shared
  keychain, no app group common to the two devices. The Mac and the iPhone are two devices of the space, paired like
  any others; the space key only exists in their respective databases. Apple's Universal Clipboard is the only system
  bridge: it is cut for Hidden items.
- **On the same Mac**: the app and its share extension only share the drop box of the app group and two flags
  (paired, unreadable). The extension never opens the core. Each user account of the Mac has its own container, hence
  its own device.
- **Identity**: same identifier `eu.shaffe.tomi` as the iOS app (a single App Store listing), same `tomi-share://`
  scheme.

---

## 5. Limits

### 5.1 Imposed by the platform or a dependency

1. **No push**: the relay has no APNs. Mac switched off, asleep, or Tomi quit: nothing arrives; everything is caught up
   on resume. (Android, iOS and Windows have the same limit.)
2. **Capture exclusion not guaranteed**: `sharingType = .none` is respected by the classic capture tools, but its
   effect with the recent capture APIs is to be checked on macOS 27. macOS has no public equivalent of "the screen is
   being recorded": no "Screen shared" cover as on iOS.
3. **Notification of a Hidden item**: no equivalent of `VISIBILITY_SECRET`; content never in the notification.
4. **Wrapping key**: no user presence at each use (known gap with `api-v1.md` §7.1, common to all platforms). The
   keychain protects against file theft, not against another process of the same account that would have the rights
   of the app.
5. **Links `https://relay.example.org/l/…`** (on the relay of the space, chosen at pairing): do not open the app as
   long as the relay does not serve an `apple-app-site-association` and the app has no associated domain (same gap as
   iOS, A.11). The link is pasted.
6. **Sandbox**: saving outside Downloads requires a folder chosen once by the user.
7. **App Nap**: macOS slows down an app with no window; the doorbell must declare itself as an activity to stay
   responsive, without preventing sleep (not done yet). Same cause: the minute of the purge of the plaintext copies can
   last more than a minute when Tomi has no window.
8. **Clipboard**: macOS has no native expiry (home-made timer, lost if Tomi is killed before 30 s; caught up at the
   next startup if the Tomi mark is still there, and when quitting the app). The paste privacy alerts introduced by
   recent macOS versions are to be checked with `⌘V`.

### 5.2 Accepted (choices of this proposal)

1. No organised distribution for now: the app is built and signed on the owner's Mac (TestFlight set aside, no update
   tool).
2. No manual theme setting (as on iOS).
3. No manual history transfer (state of iOS). The inactive device hint, deleting a single attachment and the
   highlighting of the code screen were added to iOS and the Mac together (November 2026).
4. A single full window.
5. A file opened in another app loses its plaintext copy at the purge (one minute after the last use of Tomi): to keep
   it, "Save".
6. `⌘Q` stops reception.
7. The interface follows the language of the Mac (`docs/LOCALIZATION.md`), English when Tomi does not have it. So
   do the texts the system shows for Tomi: the camera permission sentence and "Send with Tomi" in the Services menu.

### 5.3 Inherited from the project

"The active device rings alone" not implemented; three secret heuristics (the Mac takes the iOS one); limit of 20
files outside the spec; silent `device_set_mismatch` alert; no full Apple CI on every PR (macOS runner x10).

---

## 6. Decisions (approved on 16 November 2026)

Each row: the choice made, what it rules out, why. A7, A11, A12 and A14 carry the changes requested by the owner at
approval.

| No. | Topic | Decision | Ruled out | Reason |
|---|---|---|---|---|
| A1 | Minimum version | **macOS 26** for the app (the packages stay at macOS 14 for the tests) | macOS 14 with opaque fallbacks | Liquid Glass without double rendering; a single user, on macOS 27. Pitfall to know: `#available(iOS 26, *)` is always true on macOS. |
| A2 | Full window | **Reuse the iOS screen stack** (feed as a readable column → Detail), native Mac toolbar | the Windows grid; three columns sidebar / list / detail | the least new code, behaviour already tested; the sidebar can come later without breaking anything |
| A3 | Dock | **Menu bar app**; Dock icon only when the full window is open | always in the Dock; never in the Dock | the Windows usage (resident, discreet) without losing `⌘Tab` or the menu bar |
| A4 | Global shortcut | **`⌃⇧Space`** by default, adjustable; on a conflict the previous one is kept | `⌥Space`, `⌃⌥Space` | same fingers as on Windows; free by default on macOS. No third-party dependency (system global shortcut API, compatible with the sandbox, no accessibility permission). |
| A5 | Opening at login | **Enabled once by default** at the first launch, announced on the welcome screen, adjustable | disabled by default | without it, nothing is received after a restart; it is the behaviour of Windows. macOS shows its own notification anyway. |
| A6 | Screenshots | **Window excluded from captures only when it shows a sensitive screen** (shown Hidden item, pairing code), through the `sensitiveScreen` counter of iOS; lifted in Debug / Dev | permanent exclusion of both windows (Windows) | as Android (counted `FLAG_SECURE`) and iOS; the feed stays capturable for help and mockups |
| A7 | Authentication | **Reuse the iOS gate as is** (`BiometricGate`): native macOS calls (LocalAuthentication), which offer Touch ID, Apple Watch or the password of the Mac depending on the machine. Destructive actions: biometrics only if the Mac has them, otherwise the password of the Mac. | a home-made prompt; biometrics only with no fallback | requested by the owner: same usage as Face ID on iOS. The fallback to the password is necessary: a Mac mini or a closed laptop has no Touch ID. |
| A8 | Re-hiding a Hidden item | **Focus loss, popover closed, lock, and 15 s** | loss of activity only (iOS); 30 s (Android) | on a workstation the window stays active for a long time; 15 s is the Windows value |
| A9 | Click on a notification | **The obvious action of the type** (rule of Windows), written in `TomiKit` | always open the app | it is the most frequent reception gesture; the rule becomes reusable by iOS |
| A10 | Plaintext copies | **Purge one minute after the last use**, on lock, on quit, at startup | purge as soon as Tomi is no longer in front (mobile rule) | opening a file in Preview puts Tomi in the background: an immediate purge would break the opening. Gap with golden rule 3 to note, identical to Windows (A.9). |
| A11 | QR scan | **Yes**: the Mac scans with its camera the QR shown by a mobile already in the space (see 2.6) | entering the code only (Windows) | requested by the owner: link the Mac from their phone without typing a code. Cost: camera permission (on click only), camera entitlement of the sandbox, an AVFoundation + Vision scanner to write. |
| A12 | Distribution | **Sandbox + hardened runtime from the first milestone; local signing** (`Signing.local.xcconfig`). TestFlight set aside for now. | TestFlight right now; notarised Developer ID served by the relay | decision of the owner. The sandbox keeps every path open for later. |
| A13 | Sharing code with iOS | **Files of `Tomi/` compiled by path + `#if os(macOS)` + new `TomiMac/` folder** | new `TomiUI` package; copy of the screens | additive, no global refactor (repository rule); the package can be extracted later |
| A14 | Processors | **Universal binary: Apple silicon first, Intel as well** | Apple silicon only | requested by the owner if it is not complicated, and it is not: one more Rust target (`x86_64-apple-darwin`) merged by `lipo`, as for the iOS simulator. Only Apple silicon is tried; the Intel slice is compiled, not tested on a machine. |
| A15 | Texts specific to the Mac | **`macos_…` keys** added by the generator, next to the `ios_…` keys | reuse the iOS texts as they are | "Touch ID", "this Mac", "click"; `strings.xml` stays the source of truth |
| A16 | Diverging limits and rules | **Those of the mobiles** (20 files, 40 characters, iOS secret heuristic) | those of Windows | they are in `TomiKit`; deviating would create a fourth set of rules |

---

## 7. Proposed implementation

Additive milestones, in order; each one compiles, passes its tests and leaves iOS intact.

**Progress.** M-1 done on 16 November 2026 (branch `apple/macos`, reviewed before commit): the XCFramework has a
`macos-arm64_x86_64` slice, `TomiCore` and `TomiRuntime` declare macOS 14, the core announces itself as `"macos"`
(`CoreFactory.platform`), the wrapping key asks for the "data protection" keychain on the Mac. Checked on an Apple
silicon Mac: 5 `TomiCore` tests and 11 `TomiRuntime` tests through `swift test`, linking of the Intel slice
(`swift build --arch x86_64 --build-tests`), iOS build of `TomiRuntime`. Not checked: the Intel slice at run time (no
Intel Mac), the keychain (it needs an app signed with a team), the tests on the iOS simulator (not run again). Note:
macOS 27 no longer exists for Intel; the Intel slice serves the Macs that stayed on macOS 26.

M-2 done on 16 November 2026: `TomiMac` target (sandbox, hardened runtime, macOS 26), a window, the feed and the
Detail on the demo core. Difference from the planned split: rather than a Mac-specific root limited to the feed, **the
whole iOS app compiles on the Mac** (send, devices, pairing, departure, viewers, code screen), by excluding five UIKit
files replaced in `TomiMac/` and with `#if os(macOS)` branches elsewhere (`apps/apple/README.md`, "macOS app"). The
following milestones check and finish these screens instead of porting them. Checked on screen on the Mac: the demo
feed, the menu of a card, the Detail opened by `tomi-share://item/<id>`. State of the screens compiled but **not yet
tried on the Mac**: send, devices, pairing, departure, viewers, code screen, share, save. Provisional choices of this
milestone: the action bar floats at the bottom of the window as on iPhone (it will serve as is for the popover; its
place in the full window is still to decide with the menus, M-4 and M-8); the QR scanner is a stand-in that says "no
camera" (M-6); no swipe to archive (menu and Detail bar).

M-3 done on 16 November 2026: on the send screen, "Paste" and `⌘V` also take the files copied in the Finder and a
copied image; the window accepts what is dropped on it; `⌘↩` sends. Checked on screen in the demo: prefill through
`tomi-share://send`, send, "Sent · Undo · Options" banner. Not yet tried (no mouse in the development session):
dropping files and pasting files or an image.

M-4 done on 16 November 2026: menu bar app (`LSUIElement`), icon next to the clock (16 px drawing of the charter,
unread badge), popover on click, menu on right-click (Open Tomi, Open in a window, Settings…, Quit Tomi), full window
on demand with a Dock icon while it is open (position and size remembered), adjustable global shortcut `⌃⇧Space`,
opening at login (setting; enabled once by default outside Debug), app icon generated from the charter
(`tools/mac-app-icon.swift`), fallback to the password of the Mac for destructive actions without Touch ID (A7).
Checked on screen: nothing but the icon at launch, the popover, the Detail with its back button, the window, the
settings, the global shortcut (open then close), Esc, the welcome window of an unpaired Mac. Independent review of M-3
and M-4: no iOS regression; fixed before commit: `⌘V` that hijacked pasting in the title or while typing, viewer of a
Hidden item still there on reopening, global shortcut with Command or Option alone, tracking of the core by the
delegate. Not yet tried: the right-click menu, the icon in the Dock, changing the shortcut, opening at login (never
enabled by a Debug build), Touch ID. Rest of this milestone, postponed: pinning the popover ("keep open"), dropping a
file on the icon.

M-5 done on 16 November 2026: the life cycle of the Mac.
- **On screen or not.** A surface in use (popover, or window that has the keyboard) = Tomi is "on screen": nothing is
  notified. No surface: what arrives is notified (system notifications, logic shared with iOS) and counted (badge of
  the menu bar icon, count on the Dock icon when it is there). A click on a notification opens the popover on the
  requested screen.
- **Plaintext copies** (A10): purged 60 s after the last surface used, on lock, on sleep, on session switch and when
  quitting (the doorbell, for its part, is never stopped).
- **Wake and network**: on wake of the Mac, a new doorbell loop and a sync; on return of the network, a sync.
- **Hidden items** (A8): re-hidden after 15 s, when the surface is left or closed, on lock; the viewer and the code
  screen of a Hidden item close when the surface is left. A sheet of the same window or the Touch ID prompt do not
  count as "leaving".
- **Real core**: the Mac app is signed with the team of `Config/Signing.local.xcconfig` and a profile
  (`keychain-access-groups` entitlement), without which the "data protection" keychain refuses it. A badly signed build
  shows it ("This version of Tomi is not signed correctly") instead of offering to erase the data.
- **Tests**: `TomiMacTests` target, the 79 tests of the app on macOS plus 23 specific to the Mac (paste on a private
  pasteboard, shortcut, life cycle).
- Settings: notification state (allow, or open the settings of the Mac).
- Independent review: no iOS regression; fixed before commit: temporary links erased when quitting, purge armed also
  for a file opened from a notification, phase of a surface recomputed from the AppKit state (it could stay "in use"
  after a cancelled prompt), surface that does not come back by itself, development tools silent during a pairing,
  "Try again" on the "badly signed" screen.

Checked on the Mac: the internal state at each step in the demo (nothing on screen at launch, popover, Esc, item
arrived with the view closed, window and Dock icon); the real core that opens and reopens in the signed build (welcome
screen of a Mac with no space); the "badly signed" screen without a profile; 102 tests on macOS, 79 on iOS.
The behaviour of the surfaces with the keyboard and in front (popover closing when switching app, re-hiding when
leaving the window) could not be replayed after the last fix: the Mac was in use, and the tests have since been run in
off-screen mode (`-TomiDebugOffscreen YES`), which never takes the keyboard.
**Not yet tried**, because it needs the owner's hand or their approval of a system prompt: a notification shown
(permission to give in the Settings of Tomi), Touch ID and the re-hiding of a shown Hidden item, the wake after a
sleep, the purge at 60 s, the capture exclusion (`-TomiProtectCaptures YES`), and any real exchange with another
device (the Mac is not in any space yet).

M-6 done on 16 November 2026: entering a space on the Mac.
- **QR scan** (A11): the camera of the Mac reads the QR shown by a device already in the space
  (`TomiMac/QRScanner.swift`, AVFoundation + Vision; the camera images are read on the Mac and never kept). The camera
  is only requested on the click on "Scan the QR code". **A Mac with no camera** (Mac mini, Mac Studio, closed laptop
  without a webcam) does not have this button active: one enters the invitation code (`4821-crayon-renard`) shown by
  the phone. The protocol does not allow the reverse (the Mac showing a QR to scan in order to join): it is always the
  device already a member that shows the code.
- **Texts of the Mac** (A15): the catalogue carries, for 43 keys, a "Mac" variant ("this Mac", Touch ID or the password
  of the Mac, "click") next to the text of the phones; a device variation of the catalogue, resolved by the system.
  Source: `_mac` and `_mac_replacements` of `tools/strings-apple/<lang>.json`.
- **First launch**: the same screen as iOS, with what a menu bar app must say (the icon next to the clock, the
  shortcut, opening at login); its button opens the settings.
- Checked on screen (off-screen, demo): "Join your space", "Create your space", Devices, "Add a device" (QR and code),
  first launch. Tests: decoding of a generated QR, texts of the Mac (108 tests on macOS).
- **Not tried**: the real scan (the development Mac is a Mac mini with no camera), and a real pairing with a phone (it
  needs the owner's hand on both devices).

M-7 done on 16 November 2026: files on the Mac.
- **View**: image → viewer (zoom with the trackpad or double-click, arrows and ← → keys between the images); code →
  code screen; PDF → drawn by PDFKit in the screen; audio and video → player of the Mac; the rest → built-in Quick
  Look. A viewer covers the whole window, bar included.
- **Open**: button of the file viewer, the app the Mac has chosen for this type. A received program (app, script,
  executable) is never launched: it is shown in the Finder. Never for a Hidden item (it is shared, after
  authentication).
- **Save**: without a dialog, into `Downloads/Tomi` or into the folder chosen in the Settings (kept by a
  security-scoped bookmark); a name already taken gets a number, nothing is ever replaced; the copy is writable.
  Refused for a Hidden item.
- **Dragging out of Tomi**: a file tile drags to the Finder or another app; the file is decrypted at the moment of the
  drop. Never for a Hidden item.
- **Share**: the share menu of the Mac (reduced list for a Hidden item).
- Difference from the plan: no `Space` for Quick Look from the feed (it comes with the keyboard, M-8).
- Checked off-screen in the demo: image viewer (2/4, arrows), PDF, code screen with tabs. Tests of saving (numbering,
  never replacing, writable copy).
- **Not tried** (it needs a mouse, or it would write into the owner's folders): dragging a file out, "Open" in another
  app, a real save into Downloads, choosing the folder, built-in Quick Look for a file that is neither PDF nor image
  nor code (it does not draw in an off-screen window), the audio and video player.

M-8 done on 16 November 2026: the rest of the Mac around the app.
- **Share menu**: share extension `TomiMacShare` ("Share › Tomi" in Safari, the Finder, Photos…). It takes the sheet
  and the logic of the iOS extension into an AppKit controller, never opens the core and drops the share into the box
  of the app group; the app, which runs in the menu bar, sends it. If Tomi is not running, the share waits for its next
  launch.
- **App group of the Mac**: `<team>.eu.shaffe.tomi` and not `group.eu.shaffe.tomi` (the iOS one): macOS grants it to
  what the team signed without a provisioning profile. The app and the extension give it in their Info.plist
  (`TomiAppGroup`), read by `AppGroup.identifier`.
- **Services**: "Send with Tomi" on a selected text or on files (Services menu); prefills the send.
- **Keyboard**: in the feed, `↑` `↓` from card to card (ring), `↩` = what a click on the card does, `Space` = open what
  it contains, `⌘C` = copy it; `Esc` closes what is on top, one thing at a time (viewer, pushed screen, then the
  popover); `⌘V` on the feed prepares a send.
- **Menus**: New send `⌘N`, Open in a window `⌘0`, Search `⌘F`, "Feed" menu with the six filters (`⌘1` to `⌘6`) and
  Devices, Settings… `⌘,`.
- Independent review of M-6 to M-8: no iOS regression, no hidden content that escapes; fixed before commit: "Open" no
  longer hands over to an app that launches or installs (Terminal profile, package, Java archive, disk image, link,
  archive: shown in the Finder), camera stopped in every case, Esc that respects the screens having their own back
  (space creation, pairing, invitation), service called before the core is loaded.
- Differences from table 2.7, left as they are: `⌘O`, `⌘S`, `⌘⌫` and `⌘Z` have no shortcut (the actions are in the
  bar and in the menus of the cards); nor does `⌘[` (Esc and the back button).
- Checked: end-to-end test of the share (drop into the box of the group, send by the app on the demo core); the
  extension is registered by the system (`pluginkit`); off-screen: New send, Esc, Search, filter.
- **Not tried** by hand: the Share menu and the Services menu from another app, the arrows in the feed.

Review of the feature table, done on 17 November 2026 (iOS and Mac together): what iOS, and so the Mac, lacked compared
with Android and Windows.
- **Deleting a single attachment** ("from this device" / "everywhere"): Delete menu of the image viewer, menu of the
  rows of a batch of files in the Detail. The viewer follows the shown image by its position (slot): a removal, here
  or from another device, shifts neither what is shown nor what the actions target.
- **Inactive device hint** (more than 30 days) with "Remove", on the Devices screen.
- **Syntax highlighting of the code screen**: dependency-free highlighter ported from Windows (`CodeHighlighter`),
  palette of the charter; "No syntax highlighting for this language" and "file too long" lines as on Android.
- **Pending sends**: they leave again right away on return of the network, of the app to the front and on wake from
  sleep, instead of waiting for their next attempt (up to 15 minutes).
- **Notifications**: the one of an archived item is removed (as for read, deleted, expired).
- **Top bar**: the bar buttons (back, screen buttons) were squashed onto their symbol, with no margin, because of the
  plain style given to all buttons; they take the style of the system again. The feed and the code screen kept the
  title of the previous screen: they carry the name of the app. In the popover, the title of the pushed screen shows in
  its bar.
- **Fallback sync**: every 5 minutes while the Mac is in a space (it was described in 2.4 and missing from the code);
  it catches a doorbell that died silently and restarts it.
- **Tests added** after the line-by-line review: pin, "Delete here", prefill through a link, announcement of an
  unreadable item, notification thumbnail, `tomi-share://` scheme declared by the app, name of a hidden file kept out
  of the confirmation.
- **Clipboard suggestion** (iOS and Mac, requested on 17 November): when the send screen opens, what was copied less
  than a minute ago is offered instead of the content, without being entered. On the Mac the content is read and shown
  (a password is offered without being shown); "Send" takes it as is. On iOS only the kind is known before the
  gesture. It is an intended gap with the rule "the clipboard is only read on a gesture".
- **Two global shortcuts**, each adjustable: `⌃⇧Space` opens (or closes) Tomi, as before; `⌃⇧V` sends the selection:
  from another app, it makes that app copy its selection (⌘C), then opens the send with it. The selection therefore
  replaces what the clipboard held. macOS asks once for the "Accessibility" permission; without it, `⌃⇧V` opens the
  send without copying anything. The same keys cannot serve both. **Not tried**: I can neither send keys nor touch the
  clipboard of the session; also to check that the sandbox does let the key sending through.
- **Notifications**: the Mac app asks for permission as soon as it is in a space, if macOS has never asked (before:
  only at the end of pairing or from the Settings). Observed on 17 November: on the owner's Mac, macOS refused the
  notifications of Tomi without Tomi appearing in System Settings; several copies of the app (including the iPhone app
  launched on the Mac, same identifier) were registered, cleaned up since.
- **Off-screen checks**: `-TomiIsolatedDemo YES` opens the isolated demo core, to look at the app on a Mac that is in a
  real space without ever opening that space.
- **Captures**: every sensitive screen now takes the window out of captures (pairing, space creation, entry of hidden
  content, file of a Hidden item), no longer only the content of a shown Hidden item and the invitation QR: the
  `sensitiveScreen` marking of iOS had no effect on the Mac.
- **Clipboard**: a secret copied by a Tomi session stopped before its 30 seconds is cleared at the next startup (only
  if it still carries the Tomi mark).
- **A single "Save" entry** for an image (iOS has two, Photos and Files; on the Mac they wrote to the same place).
- **CI**: the full Apple job (`apple-app`, on push to `main` or on demand) also builds the core for macOS and runs the
  tests of the Mac app **without signing**; the two classes that need the team (keychain, app group) do not run there.
  Tried locally under these conditions, not yet on a runner.
- Checked off-screen on the Mac: removal of the shown image then of an image placed before it, colours and message of
  the code screen, top bar in the window and the popover.
- **Not tried**: the inactivity hint on screen (no demo device is inactive), the real return of the network, these
  screens on an iPhone (the tests pass on the simulator; same screen code as the Mac).

---

## 8. To check during implementation

- "Data protection" keychain (`kSecUseDataProtectionKeychain`, set in M-1): it requires an app signed with an
  application identifier, hence a team in `Signing.local.xcconfig` from milestone M-2; a "Sign to Run Locally"
  signature is not enough. To try on the signed app: creation, reread, database without a key.
- Do not mistake a signing fault for unreadable data: on the Mac, a keychain refused for lack of an entitlement
  (`errSecMissingEntitlement`) would today lead to the "Unreadable data" screen and its "Reset Tomi" button, which
  would erase a healthy database. To tell apart before the first launch on the real core (review of M-1).
- The plaintext copies of attachments have no file protection class on the Mac (it only exists on iOS): the compromise
  of golden rule 3 is not made worse, the purge stays the only protection.
- Actual effect of `sharingType = .none` on macOS 27 (screenshot, recording, sharing in a video call).
- App group on macOS: settled in milestone M-8 by a group prefixed with the team, without a profile.
- Can the share extension launch Tomi if it is not running; otherwise the share waits for the next launch, as on iOS.
- `⌘V` in the composer and the clipboard privacy alerts.
- Quarantine attribute on the files saved or dragged out of Tomi.
- Behaviour of the doorbell with App Nap, windows closed, over several hours.
- `⌃⇧Space` free on the owner's Mac (input methods, launchers).
