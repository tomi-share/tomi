# Tomi Apple (iOS, then macOS) — Implementation specification and workflow

> Historical implementation notes for the iOS app, kept for reference; docs/FEATURES.md and the code are authoritative.

Execution document. It is written to be followed step by step by a contributor, one pull request at a time,
without having to reread the whole repository. The general rules of the repository are in `AGENTS.md` and `docs/DEV.md`, which take precedence.

Last updated: 12 November 2026 (unified core: demo from the real core, section 0.3; long texts, section 0.4). Sections 0 and 12
(status) must be kept up to date with each PR.

---

## 0. Status

Working branch: **`apple/ios`**, branched from the `main` of 9 November 2026 (`c7c8129`, relay 0.4.0). All of the Apple
work is there, commit by commit, for a single pull request. The earlier stacked branches (`apple/core-xcframework` …
`apple/send-pairing-devices`) were replaced by it.

| PR | Topic | Status |
|---|---|---|
| PR-1 | Apple tooling (core as a static library), `TomiCore` package | done |
| PR-2 | Core: universal demo mode + contract (pairing, archive, simulation) | done, carried over to the new core (0.1) |
| PR-3 | Apple: project skeleton, `TomiDesign`, `TomiKit` (model), `TomiRuntime` (adapter), empty app | done (deviations: section 5.10) |
| PR-4 | Apple: pure logic ported from Android with its tests | done (deviations: section 6.1) |
| PR-5 | Apple: Feed, Card, Detail | done (deviations: section 7.1) |
| PR-6 | Apple: Send, Pairing, Onboarding, Devices | done (deviations: section 8.1) |
| PR-7 | Catch-up: feed, cards, texts and details from Android of 9 November | done (0.1) |
| PR-8 | Entry: Welcome, Create your space, Join your space, first launch | done (0.1) |
| PR-9 | Add a device (invite from the iPhone) | done (0.1) |
| PR-10 | Leaving the space (sign out, delete the space, erase) | done (0.1) |
| PR-11 | Security, notifications, links (formerly PR-7, section 9) | done (9.1) |
| PR-12 | Share extension (formerly PR-8, section 10) | done |
| PR-13 | Image viewer, code screen (formerly PR-9, section 11) | done |
| PR-14 | Switch to the real core, background tasks, TestFlight (formerly PR-10, section 12) | done (except TestFlight) |
| — | Fixes after testing on an iPhone (viewer, capture cache, files) | done (0.2) |
| — | Rebase onto the `main` of 10 November, carry-over of its changes, Dev configuration | done (0.2) |
| — | Unified core: demo from the real core, in the Release | done (0.3) |
| — | Long texts: capped previews in the feed, chunked detail | done (0.4) |

Sections 9 to 12 keep their former PR numbers (PR-7 to PR-10) in their titles: read them as PR-11 to PR-14.

Out of scope: APNs push and `apple-app-site-association` on the relay side, the macOS target. (The synchronized archive
arrived with the `main` of 10 November: section 0.2.)

### 0.1 Revised plan: the `main` of 9 November 2026

**What changed in the repository** (50 commits; to read: `spec/pairing-v1.md`):
- Core: real CPace pairing (invitation and join sessions, `create_space` by installation key,
  `start_invitation`, `join_space`, resumption, expected history), v2 space departure (positive proof, receipt,
  `leave_space`, `delete_space`, `erase_without_proof`), real-time doorbell, `NotFound` state. No archive or demo.
- Android: real core only (the Kotlin fake core now exists only in the tests; no more "Enter demo mode"). New
  Welcome, Create your space, reworked Join your space, Add a device, departure (dialog, banner), v2 first launch
  screens. Feed: flat search, conditional counter, filter hidden during search, "Delivered to all" on the Detail only,
  mono without ligatures, hidden-item stripe without space, `errorContainer`.

**Maintainer decision**: the demo core remains the basis of all Apple development, until the end of the listed PRs;
switching to the real core is the last one (PR-14). Consequences:
- The demo mode (`crates/core/src/runtime/demo.rs`, feature `demo`) and the local archive were carried over to the new
  core (done: 464 core tests green, clippy and fmt clean). The demo files (images, code) now live in
  `apps/apple/DemoAssets/demo` (Android no longer has them).
- Each PR that adds a flow (space creation, invitation, departure) also extends the demo to simulate it: the desktop
  stand-in ("PC-Bureau") plays the other device. The Swift contract (`TomiKit.TomiCore`) follows the Android one
  (`core/TomiCore.kt`): same operations, same direction, so that the final wiring only touches the adapter.
- Texts: keys removed from Android but still useful to the demo live in `tools/strings-apple.json` (`ios_debug_*`).

**PR-7 — Catch-up (feed, cards, texts)**. Source: commits `ea9ca7e`, `6c219e1`, `5ab47b5`, `1892995` and the
9 November sections of the Android UI decisions. Flat search (pinned items first, without headings), counter only with a
query or a filter, hidden-items sentence only if there are hidden items in the results, Filters button hidden
during search (the filter stays active), keyboard "Search" only closes the keyboard; no more "Delivered to all"
on cards; URL, revealed secret and codes in mono without ligatures (`calt`, `liga` at 0); hidden-item stripe drawn
behind the card; `errorContainer` colour; date formats ("Yesterday", "expires in 23 hours"); texts renamed from
`strings.xml`. Doorbell: `startLiveSync` / `stopLiveSync` in the contract (no effect in demo).

*Done*: `FeedSearchDisplay` and `CardFooterLogic` (TomiKit, tests ported from Android), `TomiMonoFont` (TomiDesign: `calt`
and `liga` at 0, no strikethrough for the revealed secret; tests on the shaped glyphs, which show that JetBrains Mono
did change `://` on iOS too) applied to URLs, revealed secret, source mode, file names and `tomiText(.mono)`.
Deviations: the hidden-item stripe was already drawn as an overlay (`TomiCard`); date formats already compliant;
`errorContainer` has no iOS usage for now (to be picked up with departure, PR-10); the doorbell already matches the contract
in the form `enterForeground` / `enterBackground` (the adapter only opens it when paired), no separate `startLiveSync`;
Markdown (code blocks) keeps its ligatures, like Android's code viewer.

**PR-8 — Entry**. Kotlin `ui/pairing/{WelcomeScreen,CreateSpaceScreen,PairingScreen,PairingViewModel}.kt`,
`ui/onboarding/OnboardingScreen.kt`.
Welcome (two entry points: first device / Tomi is already on another device); Create your space (installation key by QR code,
by paste or typing, "Where do I find the key?" sheet, device name); Join your space reworked (illustration, scan,
entry of the invitation code, verification, waiting for the other device, history, detailed errors
`pair_error_*`); v2 first launch (iOS: share sheet favourites). Demo: `create_space` accepts a demo key,
joining goes through waiting for PC-Bureau.

*Done*:
- Demo core: `create_space` accepts `DEMO_INSTALL_TOKEN` (`2345-6789-abcd-efgh-jkmn-pqrs-tvwx-yz23`, dashes,
  spaces and case ignored) and starts from an empty feed; the name typed is applied; any other key gives `403
  install_token_invalid`. The confirmation waits 2.5 s for PC-Bureau (cancellable: `pairing_cancelled`). The response carries
  `history_bytes`, `member_count` (2) and `expires_in_ms` (120 s). Pairing stays simulated: a real CPace in demo would
  impose a relay that is not available for "Create" and available for "Join", which are incompatible in one same world.
  Accepted consequence: a space "created" in demo already shows PC-Bureau and PC-Maison under Devices.
- Swift contract aligned with Android: `PairingPrompt` (pair and platform, members, history size, validity),
  `PairingFailure` with 14 reasons (+ `unsupported` until PR-14), `.alreadyPaired`, `createSpace`, `resumePairing`
  (called at startup), `isValidDeviceName` (core rule). Adapter: dedicated `pairing` queue; cancellation goes
  through another queue to interrupt the wait. Pure logic ported with its tests: `InstallTokenQr`, `DeviceNames`,
  `Countdown`, `PairingErrors` (mapping of core errors, messages), `PairingInput.isValidCode` (Android rule:
  non-empty, 64 characters at most, no control characters).
- Screens: Welcome (rewritten version of 9 November: two entry points at the bottom), Create your space (scan that fills in
  without submitting, key hidden with "Show", "Where do I find the key?" sheet, name alert, "Join the existing
  space"), Join your space reworked (path Devices › Add a device, illustration, input sub-screen,
  verification with members or platform and a real countdown, history with size, waiting for the other
  device, "Cancel" at each step, the end cannot be left by going back). v2 first launch (X, "Later" except
  from help).
- Deviations: the "Is Tomi at the top?" step of the first launch waits for the share extension (PR-12); hiding
  the pairing screens during capture is done with security (PR-11); the link `tomi-share://pair` will open the screen with the
  "Invitation link received" card in PR-11 (links); the real `join_space` / `FfiJoinSession` path is PR-14.

**PR-9 — Add a device**. `ui/pairing/{InviteScreen,InviteViewModel,InviteFlow}.kt`.
QR code and invitation code, validity period, verification, waiting, end; screen hidden during capture. Demo: an invitation
session that PC-Maison (simulated) joins.

*Done*: the invitation is not simulated. The demo core (`DemoWorld::answer_invitation`, wired into
`start_invitation`) makes a new "Tablette" (Android) join with the real protocol over the in-memory relay:
real code, QR, random verification code, confirmation on both sides, device registered. The app therefore already drives the
real `FfiInviteSession` (`RustInviteSession`: waits on a dedicated queue, cancellation on another, transient retries
at 1, 2 and 4 s), which will not change in PR-14. Contract: `startInvitation`, `InviteSession`,
`InvitePrompt`, `InvitedDevice`. The flow (`InviteFlow`, TomiKit, tested on macOS) follows Android: code renewal
(expired, abandoned, wrong code) up to 5 times in a row; "They are different" closes the channel and makes a
new code (not counted); leaving closes the channel; sync after the addition for exporting the history. Screen: native QR (CoreImage, black
on white), code in mono without ligatures and not selectable, validity, verification, waiting, end, failure with "New
code". Entry through the "Add a device" button in Devices. Deviations: hiding during capture in PR-11; the
demo does not yet play errors (expired code, refusal on the Tablet side); during verification, going back counts as
refusal (like Android's system back): one then leaves with "Cancel".

**PR-10 — Leaving the space**. `ui/feed/{LeaveSpaceDialog,EraseWithoutProofDialog,
DepartureBanner}.kt`, `core/DepartureRetrier.kt`. Sign out, delete the space
(buttons inactive for 2 s, strong biometrics), erase without proof, pending departure banner, "space not found" state.
Demo: departure with proof, with refusal, and without network.

*Done*:
- Demo core: `simulate_departure(mode)` sets the in-memory relay for the following departures (normal, offline,
  transient error, last device, relay reset, relay too old); the departure itself goes through the real
  core code (proof, receipt, marker). ⋮ menu "Demo: simulate the server on departure" in Debug. After a departure, the demo
  restarts from a new unpaired core (the old one can no longer join).
- TomiKit: domain (`DepartureKind`, `PendingReason`, `RefusalReason`, `DepartureResult`, `Departure`, `EraseResult`),
  `CoreState.departure` / `departureRunning`, 7 contract calls. `LeaveSpaceModel` (faithful port of
  `LeaveSpaceViewModel`: dialogs, banner, generations, 2 s arming, retry of a deletion under strong biometrics each time,
  cancellation that catches up on the marker of a partially run attempt, "Erase anyway" which rereads the
  core just before the prompt) and `DepartureRetrier` (2 to 120 s with 0 to 20 % jitter, then on network or foreground
  return, at most once a minute), with their tests.
- Adapter: separate `departure` queue (an attempt can last minutes), cancellation and authorization through `local`,
  `departureRunning` counter, state reread after each call, doorbell stopped once erased.
- App: banner at the top of the feed (pending, erasing, cancelled, space not found, removed), red "Sign out"
  entry in the menu (only when no departure is running), sheet with the three dialogs (a system alert allows neither a checkbox nor
  an armed button after 2 s; the sheet cannot be dismissed with a gesture during a call), strong biometrics
  (`deviceOwnerAuthenticationWithBiometrics`, no passcode fallback) to delete and erase, end messages. At startup
  `continueDeparture` runs before `resumePairing`. Cleanup outside the core at every `isPaired == false`: first launch
  to redo, biometric window, revealed items, chosen files, notifications, clipboard if it contains a
  secret copied by Tomi (sensitive copies are now marked `org.nspasteboard.ConcealedType`).
- Deviations: scheduled sends of the old space are not purged one by one (the erased core refuses them); the open
  question in the Apple notes about "strong" biometrics is settled by Face ID / Touch ID alone. On departure,
  the departure model, the retrier and the send draft are recreated (nothing survives the space), the send copies
  erased; the clipboard is only emptied if it carries Tomi's mark (`eu.shaffe.tomi.secret`). The cancellation that catches up
  on a partially run attempt retries every 40 ms. Known remainders, with no security effect: an automatic refusal
  arriving while another sheet of the feed is open waits for it to close; a tap during erasure (banner
  without button) would be shown as a failure.

**PR-11 to PR-13**: sections 9 to 11 (content unchanged), completed by the list of screens hidden during
capture and notifications, and the mockups 11 and 12.

**PR-14 — Switch to the real core**. Adapter on the pairing sessions and departure, like `RealTomiCore.kt`
(`joinWith`, waiting for double confirmation, `PairingErrors` errors), doorbell, `BGProcessingTask`, TestFlight.
The demo mode remains available in Debug.

### 0.2 After PR-14 (10 November 2026)

**Fixes after testing on a real iPhone** (commit `apple : visionneuse en barre flottante…`):
- Viewer: the controls move into the floating Liquid Glass bar (`ActionBarScaffold`): at the bottom, or in the
  right-hand strip on an iPhone Duo in landscape (the close button then joins the bar; otherwise it sits at the top left, on the
  status bar line). Quick buttons "Save to photos" and "Copy image" (clipboard, image type),
  Delete menu (here / everywhere), "…" menu (Save to Files, Details), Share as the main button. Swipe down to close, without
  getting in the way of the pager or pinch (`PullDelegate`).
- Privacy cache: during capture (recording, mirroring, screen sharing), a text and an exit button
  (hidden secrets, back to the feed or the home screen); VoiceOver does not read beneath; hidden content cannot be shared
  during capture, and an open share sheet closes when capture starts.
- Files: "Files" menu on cards (one submenu per file), "Save to Files" (temporary copy erased afterwards), names of the files of an unrevealed hidden item kept
  silent ("File 2"); Face ID usage text (`NSFaceIDUsageDescription`, required on a real iPhone).

**Rebase onto the `main` of 10 November** (35 commits):
- Core conflicts (`ffi.rs`, `runtime/actions.rs`, `runtime/store.rs`): the local archive of PR-2 (sections 4.7 and
  4.10, table `item_archives`, `Engine::archived_items`) was **replaced** by the synchronized archive of `main` (table
  `archives`, signed item state, `ItemView.archived`). Kept from PR-2: the demo mode, simulated pairing,
  `simulate_incoming` / `simulate_departure`. 461 core tests (with `demo`) and clippy green. The commit message
  `core : mode démo universel, archive locale…` still refers to a local archive.
- Android CI: the fake core used by the tests (`NativeFakes.kt`) implements the FFI members added by the demo (`isDemo`,
  `pairWithQr` / `pairWithCode`, `confirmPairing`, `cancelPairing`, `simulateIncoming`, `simulateDeparture`).

**Changes from `main` carried over into the app**:
- Filters Links / Text / Files: a hidden item is no longer classified there, only under "Hidden" (`FeedFiltering`).
- Markdown declared (`sourceFormat = markdown`) without a `markdown` field, as Windows sends it: rendered as Markdown with the
  toggle (`RichText.markdownSource`).
- "Unreadable data" (`UnreadableDataScreen`, `AppContainer.Phase.unreadable`): "Retry" and "Reset Tomi"
  (confirmation; erases `Application Support/eu.shaffe.tomi` then the Keychain key, on a best-effort basis; in demo, the
  demo folder only). The share extension says so (App Group key `core.isUnreadable`). A background task
  only undoes its own failed opening; the foreground reruns the opening left pending. As on Android, any
  opening error leads to this screen (including a full storage); no app test for this path yet.
- Core errors: "Deleted" only on success (otherwise `snackbar_action_failed`); Devices:
  `devices_rename_failed`, `devices_revoke_failed`, busy state during revocation (bar, `devices_revoking`,
  announced to VoiceOver); `copyLink` refuses a hidden item.
- After a successful send, return to the feed, scrolled to the top (without filter or search), with the "Sent /
  Undo / Options" banner; "Undo" reopens the send screen, which restores the draft. Short messages appear
  above the banner.
- `PairingGrace` (2 min): history imported right after pairing does not trigger notifications.
- Feed: the header comes back as soon as you scroll up (copy over it, not with VoiceOver; iOS 18 and later). The floating bar
  at the bottom no longer hides on scroll (feed, send, detail), as on Android; only the header hides (`scrollingDown`).
- Send undo delay 3 s (`SendCoordinator.undoWindowMs`, `InboxShareSender`); confirmations keep 5 s
  (`feedbackDuration`, decoupled). The token `behavior.undo-window-ms` of `design/tomi/tokens.json` was set to 3000
  (`TomiTokens.swift` updated by hand, without Python: rerunning `tools/tokens-to-swift.py` on a Mac must
  produce the same file; `TomiMotion.undoWindow` is not used by the app).
- Detail: "Sent at" line (`meta_sent_at`, `formatPreciseDateTime`, "5 novembre 2026 à 13:47:09"), after "Type".
- Read by the view: a receipt becomes read when it stays on screen (60 % of the card, or of the visible area for a
  taller card, for 1.5 s; feed in the foreground, app active, nothing pushed on top; never a hidden item, an unreadable one
  or a local send). `SeenTracker` (TomiKit, tested), `SeenReading` (card positions via `onGeometryChange` in
  `.scrollView`), `FeedViewModel.onItemsSeen`. The area ignores the sticky header and removes 96 pt under the bottom bar.
- Floating bar: **iOS Liquid Glass, deliberately untinted** (maintainer decision: iOS keeps its own design,
  we do not imitate Android's tint `d7b2b6e`). The iOS 17 to 25 fallback remains the opaque surface of the style guide. Only the
  selected button is `brand` at 24 % (`barSelected`, glass included), accepted by the maintainer.
- Already in place on iOS: erasure at startup of plain-text send copies, copying the link of a hidden item without text,
  send thumbnails (360 px, Android 320).

**Capture cache "Shared or recorded screen" and Dev configuration**:
- The cache during capture is a real page (`PrivacyCover`): title, explanation depending on the case (pairing code
  or hidden content), steps to stop sharing, automatic resumption when capture stops, button within the margins.
- **Dev** configuration (scheme "Tomi Dev", `Config/Dev.xcconfig`): Release (real core, relay provided by the link/QR, the "Relay" field or `Config/Local.xcconfig`,
  no demo files) plus `TOMI_DEV_TOOLS`. In Dev and in Debug, the cache offers "Continue despite sharing"
  (for the duration of the launch), for an iPhone mirrored to the Mac during development. This code does not exist in
  Release; never archive Dev. `-TomiDebugScreenCaptured YES` (Debug) shows the cache on the simulator.

**Capture protection (specific to Apple; details in `apps/apple/README.md`, "Confidentialité à l'écran")**:
the content of a hidden item when displayed (card, detail, viewer, code screen) and the QR / invitation code are drawn in
the layer of a password field, which iOS leaves empty in screenshots, recordings and mirroring
(`CaptureShield`, line by line for the code screen; neutral preview of long-press menus of a hidden item; names of
files of a hidden item never shown in the "Files" submenu, even when displayed). Fixed along the way: returning from Face ID (the app becomes active again) re-triggered the masking for
"capture start" during a recording, so the hidden item was not displayed and the cache did not appear;
masking now only happens at the real start of capture.

**File preview (details in `apps/apple/README.md`, "Aperçu des fichiers")**: sent thumbnails aligned with
Android (`DraftThumbnails` / `ThumbnailSize`: 600 × 800, height at most 4/3 of the width, ≤ 120 KB, budget of 384 KB,
nothing for a hidden item, a new send without thumbnails if the item exceeds it; iOS used to send 360 px without a budget, and a
thumbnail for a hidden item); Quick Look thumbnails for PDFs, videos and documents at send time (3 s at most per file),
displayed by the cards; image in notifications (thumbnail reduced to 300 px, never
for a hidden item); file viewer (Apple player, built-in Quick Look, text to the code screen, "Open
in…" page) instead of the system Quick Look sheet.

**Verification**: app (66), TomiRuntime, TomiCore, TomiKit (424) and TomiDesign (13) green; review done
and corrected; iPhone Duo simulator: feed, header on scroll, back to the feed after sending, cache and bypass;
Dev build installed on the iPhone. Not verified: "Undo" from the feed on the simulator (3 s window), the
"Unreadable data" screen under real conditions.

### 0.3 Unified core (November 2026)

The real core can enter the demo and leave it without closing its engine (`runtime/unified.rs`, FFI `enter_demo`,
`leave_demo`, `supports_demo_entry`). The iOS app keeps the same core for its whole life: "Rejoins ton espace" (Join
your space) offers a small "Essayer avec des données de démonstration" (Try with demo data) button, the feed shows a
"Quitter la démo" (Leave the demo) banner, and the Release embeds the demo and its files. The Debug mode that starts on
a demo core stays. Trade-offs: `demo` feature off by default; demo refused if the core is in a space or has a pairing
pending (A2); the demo always comes already joined (A3); the "Simuler la réception…" (Simulate receiving…) menus in
Debug and Dev only (A4); what arrives through sharing lands in the demo feed, erased on exit (A5). Android and Windows
take the same entry and banner from their own branches, with texts shared through `strings.xml` (`demo_enter`,
`demo_banner`, `demo_leave`). Not compiled here: to validate on a Mac (TomiKit, TomiDesign, TomiRuntime and app tests, a
Release build for the simulator with the `demo` folder, a manual run on a real iPhone, a TestFlight archive).

### 0.4 Long texts (November 2026)

Port of the Android solution for long texts and a smooth feed and detail view. No change to the protocol, the core or the
relay.

- **Pure logic (`TomiKit`)**: `Logic/Render/MarkdownBlocks.swift` ports `MarkdownBlocks.kt` file for file (`preview`,
  `head`, `chunks`, `sourceChunks`, same thresholds), with the tests of `MarkdownBlocksTest.kt` and
  `MarkdownChunksTest.kt`. `MarkdownParityTests` compares the outputs with those of Android's Kotlin (compiled as is and
  run on the JVM) over the 5,000 documents of the two fuzzers: identical to the character. `Model/ItemReuse.swift` ports
  `ItemReuse.kt`, `Logic/Render/ProgressiveReveal.swift` the estimates of `ProgressiveColumn.kt`.
- **Feed card**: capped preview prepared once per card (`CardTexts`, in memory, emptied on a core change, on leaving the
  space and in the background), fixed height of 220 pt when it is truncated, "Lire la suite" (Read more) without
  measuring; `MarkdownView` no longer keeps or parses anything, it draws blocks that were prepared before
  (`MarkdownRendering.blocks`); `MarkdownGuard.render` no longer parses the text twice.
- **Detail**: column of chunks (`ChunkedMarkdown`, `ChunkedLiteral`), guard per chunk, splitting off the main thread
  above 8,000 characters, first-screen chunks parsed before display, the next ones ahead of the scroll.
- **Around it**: warm-up of the rendering code at launch (`TextRenderingWarmUp`), unchanged feed items reused at each
  reading of the core (the adapter only republishes the feed when it changed), `FeedViewModel.item(id:)` no longer sorts
  the feed.
- **Secret items**: nothing is computed for a closed secret item; a revealed one is cut in place (`head`, chunks of the
  detail), never cached; `Preview`, `Chunk`, `CardText` and `DetailPart` print no content.

Differences with Android (noted in the matrix):

1. Names under the `MarkdownBlocks` namespace (`MarkdownBlocks.preview`, `.Budget`, `.Preview`, `.Chunk`): `Preview`
   alone would clash with SwiftUI. The text is read in UTF-16 units to count like Kotlin; the regular expressions are
   written by hand; `head` only reads the beginning of the text.
2. The detail is a `LazyVStack`, not a `ProgressiveColumn`: Compose cannot nest a lazy list in a scrolling column,
   SwiftUI can. No hand-reserved space, no 600 ms grace delay, no saved chunk count: the stack builds the chunks near the
   screen, releases the distant ones and estimates the height of the others. Checked when opening the 200 KB document:
   2 chunks built out of 66.
3. No `RenderedHeights`: SwiftUI rendering is synchronous, a card has its height from its first frame.
4. A long revealed secret item, in the detail: each chunk is in the capture-excluded layer (`CaptureShield`), the block
   background around it; a single protected block would force the whole text to be laid out at once.
5. As on Android, the guard applies per chunk: a Markdown of more than 200,000 characters is rendered in chunks in the
   detail, it is no longer shown as plain text in one block.

Measurements (iPhone Duo simulator, iOS 27.2, Release build, core demo: 3,049 items, one 200 KB Markdown): time during
which the main thread stays busy without yielding, read by a temporary probe, not committed.

| Scenario | Before (unified-core branch) | After |
|---|---|---|
| The 200 KB Markdown card arrives in the feed | 2.7 s, then a 17.0 s block | no pass above 30 ms |
| Scrolling with that card on screen | ≈ 28 ms per frame (all above 16.7 ms) | no pass above 16.7 ms |
| Opening the detail of that Markdown | 29.0 s block | 0.22 to 0.48 s on first opening (screen push), 0.08 to 0.10 s afterwards |
| Scrolling the detail | 0.84 s and 0.66 s, then ≈ 30 ms per frame | passes of 22 to 54 ms when a chunk arrives, 2 per second at most |

What remains and does not come from long texts (same values before and after): the first HTML card of the feed creates
the process's first `WKWebView` (≈ 0.6 s on the simulator), image thumbnails are decoded on the main thread at their
first appearance (≈ 0.2 s for the demo's large image), and entering the demo costs ≈ 0.3 s.

Checked by the author on a Mac: `swift test` of TomiKit (536 tests), TomiRuntime tests (11) and app tests (79) on a
simulator, parity with Android's Kotlin, a manual run on a Release simulator (feed, "Lire la suite", detail in Preview
and Markdown). Hand-tested on a real iPhone (Dev build): smooth feed and detail. Not checked: measurements on a real
iPhone (the above are from the simulator), rotation, a long revealed secret item on screen (Face ID), Dynamic Type, iPad.
This slice of the port has not been compiled in the open-source repository: validate on a Mac.

---

## 1. Architecture decisions (closed)

1. **Folder**: `apps/apple/` holds all Apple clients. A single Xcode project generated by XcodeGen from
   `apps/apple/project.yml`; the iOS target is called `Tomi`, the macOS target will come in the same project. Nothing is
   added in `apps/ios/` or `apps/macos/`.
2. **Shared code in local SwiftPM packages** (`apps/apple/Packages/`):
   - `TomiCore`: the Rust core (XCFramework + Swift generated by UniFFI). Generated, never edited by hand.
   - `TomiKit`: model and pure logic, **without UIKit or dependency on the core**, testable on macOS in a few seconds.
   - `TomiDesign`: style guide (colours, typography, logo, icons, base components). No dependency on the core.
   - `TomiRuntime`: everything that touches the real or demo core (adapter, Keychain, paths, send queue, extension).
     Depends on `TomiCore` and `TomiKit`. Tested on the simulator.
3. **No "fake core" in Swift.** The universal test double is the **demo mode of the Rust core** (PR-2): the real engine
   (`Engine`) on an in-memory SQLite store, and the relay double already used by the core's tests
   (`crates/core/src/runtime/mock.rs`). Same behaviour on all platforms, a single update.
   Android (`FakeTomiCore.kt`) and Windows (`fake_core.rs`) may switch to it later; that is not covered in this document.
4. **Single contract**: the Swift `TomiCore` protocol is the exact mirror of `apps/android/.../core/TomiCore.kt`
   (same methods, same semantics, same comments translated). Any change to the contract goes first through
   `crates/core/src/ffi.rs`, then through the adapters. No business logic in the views.
5. **Texts and tokens generated from the sources of truth**: `Localizable.xcstrings` is produced from
   `apps/android/app/src/main/res/values/strings.xml` (same keys, same French texts), `Tokens.swift` from
   `design/tomi/tokens.json`. Both generated files are committed (Xcode needs them) with a header "generated, do not
   edit"; the scripts are in `apps/apple/tools/`.
6. **Icons**: SF Symbols, with a single mapping table (`TomiDesign/TomiIcon.swift`) reproducing the table of the
   style guide §6.2. Rationale: native rendering, Dynamic Type, no font to embed. The §6.3 rules still apply: no
   circular arrow near the logo, no circular indicator, never the symbol as an action icon.
7. **Swift**: Xcode 27, iOS 17 minimum, SwiftUI, Observation (`@Observable`), Swift 5 language mode with
   `-strict-concurrency=targeted` (to be tightened later). No third-party dependency unless explicitly decided in this document.
8. **Share extension**: option B (the core opens inside the extension, App Group + shared Keychain, only one process
   opens the core at a time). Details in PR-8.
9. **Reference simulator**: "iPhone Duo" on iOS 27.2; an iPhone 17 as a second device for the classic screen.
10. **Branches and PRs**: one branch `apple/<topic>` per PR, into `main`. A PR never edits `apps/android/`,
    `apps/windows/`, `apps/relay/`, `spec/` or `design/tomi/` (unless explicitly stated). PRs touching
    `crates/core` (PR-2 only) are reviewed before merge (`AGENTS.md`).

---

## 2. Conventions

- **Language**: code, identifiers, comments, technical commit messages in English; screen texts and
  documentation in French (the app's default locale), informal "tu" form, "Masqué" / "Afficher" (never "Secret" / "Révéler").
- **Commit messages**: `apple : <subject>` or `core : <subject>`, in French like the existing history, written into
  a file (`git commit -F`) because of the accents. One commit per coherent batch.
- **Swift naming**: types `UpperCamelCase`, members `lowerCamelCase`, one type per file, the file carries the name of the
  type. Ports of Android files keep the original file name (`CardTap.kt` → `CardTap.swift`) so that the
  correspondence stays obvious.
- **Tests**: XCTest. Each logic file ported from Android has its ported test file (`CardTapTest.kt` →
  `CardTapTests.swift`), case by case. A ported test that makes no sense on iOS is removed with a comment.
- **Secrets**: never item content in a `print`, a `description`, a log, a notification or a crash report. The
  `CustomStringConvertible` implementations of content types are redacted like Kotlin's `toString`.
- **Do not invent product decisions**: a missing point is resolved with the simplest and safest solution,
  reported in the PR (`docs/DEV.md`, "Definition of done").
- **Real iPhone first**: when the development iPhone is connected and usable (`xcrun devicectl list devices`
  shows it as "connected"), build, install and test on it (scheme "Tomi Dev", `xcrun devicectl device
  install app` then `process launch`), not on a simulator: the development Mac lacks resources. The
  simulator is only used for what cannot be tested on the iPhone (driving the interface by hand, fake Face ID,
  `-TomiDebug*` arguments, layouts of other devices such as the iPhone Duo), one at a time, shut down afterwards.

### Reference commands

```sh
# core → XCFramework + generated Swift (rerun after any change to crates/core/src/ffi.rs)
just apple-core
# tests of the TomiCore package on the simulator
just apple-core-test
# Xcode project (never committed)
cd apps/apple && xcodegen generate
# app build
xcodebuild -project apps/apple/Tomi.xcodeproj -scheme Tomi -destination "platform=iOS Simulator,id=$(apps/apple/tools/simulator-udid.sh)" -quiet build
# tests of the pure packages (macOS, fast)
swift test --package-path apps/apple/Packages/TomiKit
swift test --package-path apps/apple/Packages/TomiDesign
# tests of a package linked to the core (simulator)
xcodebuild test -scheme TomiRuntime -destination "platform=iOS Simulator,id=$(../../tools/simulator-udid.sh)" -quiet   # from apps/apple/Packages/TomiRuntime
# app tests
xcodebuild test -project apps/apple/Tomi.xcodeproj -scheme Tomi -destination "platform=iOS Simulator,id=$(apps/apple/tools/simulator-udid.sh)" -quiet
# screenshot of the running simulator
xcrun simctl io booted screenshot /tmp/capture.png
```

Note: the `rtk` hook filters the output of `cargo` and `xcodebuild`; prefix with `rtk proxy` to read a test summary.
Two simulators are named "iPhone Duo": `name=iPhone Duo` is ambiguous for `xcodebuild`. Always target
the identifier: `-destination "platform=iOS Simulator,id=$(apps/apple/tools/simulator-udid.sh)"` (the script takes the
first available simulator with that name; `simulator-udid.sh "iPhone 17"` for another one).

### Definition of "done" for a PR

1. The verification commands listed in the PR's section pass, and their output is quoted in the description.
2. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` stay
   green if `crates/` was touched; the frozen vectors stay "unchanged".
3. No generated artefact is committed (`Tomi.xcodeproj`, `.build`, `DerivedData`, XCFramework, Swift generated by UniFFI).
4. The PR description lists: files created/modified, commands run and their result, what could not be verified,
   deviations from this document and why, open questions.
5. Sections 0 and 12 of this document are updated.

---

## 3. PR-1 — Apple tooling (core as a static library), `TomiCore` package

**Branch**: `apple/core-xcframework`. **Area**: `justfile`, `apps/apple/`, `docs/` (no core file).

Already done locally (see section 0). Contents:

- `justfile`: recipes `apple-core` (options passed to the script) and `apple-core-test`.
- `apps/apple/tools/build-core.sh`: `cargo rustc --crate-type staticlib` for the three iOS targets (the crate keeps
  `lib` + `cdylib`: the relay, Windows and the CLI do not produce a static archive), `lipo` for the simulator,
  `uniffi-bindgen --library`, `xcodebuild -create-xcframework`, copy of the generated Swift. Options `--debug` and `--no-demo`.
- `apps/apple/tools/simulator-udid.sh`: identifier of the "iPhone Duo" simulator (two simulators carry this name).
- `apps/apple/Packages/TomiCore/`: `Package.swift` (binaryTarget `tomi_coreFFI` + target `TomiCore` + tests),
  `Tests/TomiCoreTests/RustTomiCoreTests.swift` (3 smoke tests).
- `apps/apple/.gitignore`, `apps/apple/README.md`, this document.

**Verification**: `just apple-core` then `just apple-core-test` → `** TEST SUCCEEDED **`, 3 tests.

---

## 4. PR-2 — Core: universal demo mode + contract extension

**Branch**: `apple/core-demo-contract`. **Area**: `crates/core/` only. **Review required before merge.**
No change to format, spec, vectors or crypto. Everything is additive.

### 4.1 Goal

Allow any app (iOS first) to run on the real engine without network or real pairing, with demonstration data identical
to Android's, and fix in the FFI the complete contract of the apps (pairing, archive,
simulation) even where the real implementation does not exist yet.

### 4.2 Cargo feature

In `crates/core/Cargo.toml`:

```toml
[features]
default = ["runtime", "http", "ffi"]
runtime = ["dep:rusqlite"]
http = [...]            # unchanged
ffi = [...]             # unchanged
# In-memory relay double + seeded demo space behind the same Engine (apps' demo mode). No network.
demo = ["runtime"]
```

`apps/apple/tools/build-core.sh` builds with `--features ffi,demo`. Android and Windows do not change (Android builds
`demo` too since 12 November 2026, see 0.3).

### 4.3 `runtime/mock.rs`: make the double available outside tests

- `#[cfg(test)] pub(crate) mod mock;` becomes `#[cfg(any(test, feature = "demo"))] pub(crate) mod mock;` in `runtime/mod.rs`.
- `MockRelay::new(space_id, clock: Arc<TestClock>)` becomes `new(space_id, clock: Arc<dyn Clock>)`; the tests pass
  `TestClock` (which already implements `Clock`), the demo mode passes `SystemClock`. Adapt the fields and all the calls
  in `mock.rs` and `tests.rs`; no other behaviour change.
- If `mock.rs` uses `#[cfg(test)]` helpers from other modules, make them `#[cfg(any(test, feature = "demo"))]`.

### 4.4 New module `runtime/demo.rs` (`#[cfg(feature = "demo")]`)

```rust
pub struct DemoConfig {
    pub files_dir: PathBuf,          // staging/cache of the demo device (subfolder "demo" of the app's files_dir)
    pub assets_dir: Option<PathBuf>, // folder provided by the app containing the demo files (see 4.6)
    pub platform: String,            // "ios", "android", ...
    pub app_version: String,
    pub start_paired: bool,          // true = space already joined at startup (skips the demo pairing)
}

pub struct DemoWorld { /* relay: MockRelay, clock: Arc<SystemClock>, this: Arc<Engine>, peers: Vec<DemoPeer>,
                         pending_pairing: Mutex<Option<PendingPairing>>, worker: thread JoinHandle + stop flag */ }

impl DemoWorld {
    pub fn open(config: DemoConfig) -> Result<Arc<DemoWorld>>;
    pub fn engine(&self) -> Arc<Engine>;                // the engine of "this device"
    pub fn pair_with_code(&self, code: &str) -> PairingOutcome;
    pub fn pair_with_qr(&self, payload: &str) -> PairingOutcome;
    pub fn confirm_pairing(&self, import_history: bool) -> Result<()>;
    pub fn cancel_pairing(&self);
    pub fn simulate_incoming(&self, kind: DemoIncomingKind) -> Result<()>;
}
```

Behaviour, modelled on `apps/android/.../core/FakeTomiCore.kt`:

- **Space**: space key epoch 1, three devices that trust each other (as `World::new` in `runtime/tests.rs`):
  this device (name: `"Cet appareil"`, replaced by the app via `rename_device` if it wants), **PC-Bureau** (`windows`),
  **PC-Maison** (`windows`). The two peers each have their own `Engine` on an in-memory store (`Store::open_in_memory`,
  with its own `files_dir` under `files_dir/peers/<name>/`). This device also uses an in-memory store.
- **`start_paired = false`**: this device is **not** provisioned at opening (`is_provisioned() == false`); it becomes
  so at `confirm_pairing`. **`start_paired = true`**: provisioned and registered at opening, history imported.
- **`pair_with_code`**: code valid if length ≥ 4 after trim, otherwise `Failure(InvalidCode)`; waits 800 ms (as Android);
  returns `AwaitingVerification { device: PC-Bureau, verification_code: "482913", history_item_count: 20 }`
  (20 = number of demo items in 4.6). `pair_with_qr`: non-empty payload → same result, empty → `InvalidCode`.
- **`confirm_pairing(import_history)`**: provisions this device (keys, `TrustStore` with the two peers, space key),
  registers it on the double (`relay.add_device`), then if `import_history` runs the **seeding** (4.6); otherwise the feed
  stays empty. Without a pending pairing: `InvalidInput`.
- **`cancel_pairing`**: forgets the pending pairing; nothing is joined.
- **Seeding**: each demo item is sent by the indicated peer (`peer.enqueue` + `peer.dispatch`), then this device
  runs `sync()`. Items "sent from this phone" are queued by this device itself (`enqueue` + `dispatch`).
  Past `created_at` dates are obtained by going through the double (do not falsify the core): accept that all
  items are dated "now" in send order; the feed order is enough for the demo.
- **Deliveries**: a background thread runs `sync()` on PC-Bureau every 2 s (its `ack`s make this device's sends
  "Delivered to PC-Bureau"); PC-Maison never syncs (stays "Sent"). The thread stops when `DemoWorld`
  is dropped (atomic flag + `join`).
- **`simulate_incoming(kind)`**: PC-Bureau queues and sends an item of the requested type (contents identical to
  `FakeTomiCore.simulateIncoming`), then this device runs `sync()`; the core listener fires. For `Unreadable`:
  take the sealed envelope normally and replace its first 4 bytes (`format`, big-endian) by `FORMAT_MAX + 1`
  before depositing it on the double (the reader classifies it `UpdateRequired` without trying to decrypt, cf.
  `envelope.rs::read_envelope_header`). Depositing an envelope directly on the double goes through the `MockRelay` API
  (`post_item` or an existing internal equivalent) and not through an `Engine`.
- Everything the real engine already does (atomic undo, `after_first_read`, pin, tombstones, expiry, transfers)
  is **not** reimplemented: the real code runs.

### 4.5 Extended FFI contract (`src/ffi.rs`)

Add, keeping everything that exists:

```rust
#[derive(uniffi::Enum)] pub enum PairingFailureKind { InvalidCode, Expired, Network, Unsupported }

#[derive(uniffi::Enum)]
pub enum PairingOutcome {
    AwaitingVerification { device: FfiDevice, verification_code: String, history_item_count: u32 },
    Failure { reason: PairingFailureKind },
}

#[derive(uniffi::Enum)] pub enum DemoIncomingKind { Link, Text, RichText, Image, File, Secret, Unreadable }

#[derive(uniffi::Record)]
pub struct DemoConfig { pub files_dir: String, pub assets_dir: Option<String>, pub platform: String,
                        pub app_version: String, pub start_paired: bool }

// FfiItem: two fields added (archive, cf. TomiItem.archived / archivedAt on Android)
pub archived: bool,
pub archived_at_ms: Option<i64>,
```

`RustTomiCore` methods added:

| Method | Real | Demo |
|---|---|---|
| `#[uniffi::constructor] open_demo(config: DemoConfig) -> FfiResult<Arc<Self>>` | `Internal { reason: "demo mode not compiled in" }` if the `demo` feature is absent | opens `DemoWorld` and the engine |
| `is_demo() -> bool` | `false` | `true` |
| `pair_with_qr(payload: String) -> FfiResult<PairingOutcome>` | `Ok(Failure { reason: Unsupported })` | 4.4 |
| `pair_with_code(code: String) -> FfiResult<PairingOutcome>` | same | 4.4 |
| `confirm_pairing(import_history: bool) -> FfiResult<()>` | `Blocked { reason: "pairing_unsupported" }` | 4.4 |
| `cancel_pairing() -> FfiResult<()>` | `Ok(())` | 4.4 |
| `archive(id: String) -> FfiResult<()>` | 4.7 | same (same code) |
| `unarchive(id: String) -> FfiResult<()>` | 4.7 | same |
| `simulate_incoming(kind: DemoIncomingKind) -> FfiResult<()>` | `InvalidInput { reason: "not a demo core" }` | 4.4 |

`RustTomiCore` gains a field `demo: Option<Arc<DemoWorld>>` (`None` outside demo). The constructors `open` and
`with_engine` keep their signature. The doc of `CoreConfig.platform` and `FfiDevice.platform` lists `ios` and `macos`.

### 4.6 Demo data

Same list as `FakeTomiCore.kt` (inventory in the Kotlin file itself): unread Material 3 link; pinned secret "Wi-Fi invités"; Markdown "Courses" (expires in 1 day); Markdown with Rust/JS/PHP code blocks and a tracking image; 4 code files; a PHP-only file; a Rust-only file; image + JS file; secret with `.py`; link sent from this phone, delivered, pinned; HTML with `<script>`; landscape image; portrait with EXIF; 3 images; secret with image; contract (PDF + DOCX); secret sent `after_first_read`, partially delivered; unreadable item; text "Digicode"; 4 archived items (link, text, unread secret `after_first_read`, PDF). Total: 19 items + 4 archived = 23.

Files: the texts and code are Rust constants (`demo_data.rs`, texts taken from `debug/assets/demo/*.kt|js|rs|php|py`
on Android). Images and binaries come from `assets_dir` provided by the app (expected names: `demo_paysage.jpg`,
`demo_portrait_exif.jpg`, `demo_facture.pdf`, `demo_contrat.docx`; thumbnails `*.thumb.jpg` alongside, read as they are
as the draft's `thumbnail`). A missing file → the item is created without the missing attachment (never an error).

### 4.7 Archive in the engine (real and demo, one code path)

> **Replaced** at the rebase of 10 November 2026 by the synchronized archive of `main` (section 0.2). Sections 4.7 and
> 4.10 (points 1, 2, 11, 12) describe the former local archive, kept for history.

Decision: **local** storage for now (the core does not yet know the protocol event; it will be wired to this same table
by the "synchronized archive" work). Contract identical to `TomiCore.kt` (`archive` / `unarchive`).

- `runtime/store.rs`: table `item_archives (item_id TEXT PRIMARY KEY, archived_at_ms INTEGER NOT NULL)` created by
  the existing migration (add it to the schema's `CREATE TABLE IF NOT EXISTS`; do not change the schema version if the
  store has none). Methods `archive_set(item_id, at_ms)`, `archive_clear(item_id)`, `archive_all() -> BTreeMap<String, i64>`.
- `runtime/actions.rs`: `Engine::archive(id)`: no-op if unknown or already archived; refuses (`InvalidInput`) an outgoing item
  still `Queued` or `Failed`; unpins first (`unpin`, which goes through the outgoing queue as today); writes the
  table; `notify()`. `Engine::unarchive(id)` is symmetric. `Engine::pin(id)` clears the item's archive (pinning unarchives).
  `list_items` / `get_item` fill `archived`, `archived_at_ms`. `delete_local`, tombstone and expiry delete the row.
  Never `mark_read`, `ack`, `read`, `release` or download.
- Tests (`runtime/tests.rs`): archive then unarchive; idempotence; unknown ignored; archiving unpins, pinning
  unarchives; an unread `after_first_read` survives archiving; refusal of a `Queued` outgoing item; deletion clears the archive.

### 4.8 Demo mode tests (`runtime/demo.rs`, `#[cfg(test)]`)

- `open` with `start_paired = true`: 23 items, 3 devices, the item "Wi-Fi invités" pinned, 4 archived.
- `start_paired = false`: `is_provisioned() == false`; `pair_with_code("abc")` → `InvalidCode`; `"7-crayon-lune"` →
  `AwaitingVerification` code `482913`; `confirm_pairing(false)` → provisioned, 0 items; `confirm_pairing(true)` → 23 items.
- `simulate_incoming(Link)` → one more item, unread, from PC-Bureau; `Unreadable` → `ItemContentView::Unreadable { UpdateRequired }`.
- Send from this device then wait ≤ 5 s → delivery `Delivered` for PC-Bureau, `Sent` for PC-Maison.
- FFI test (`ffi.rs`): `open_demo` → `is_demo()`, the pairing path, `archive` / `unarchive`.

### 4.10 Done and accepted deviations (read before PR-3)

Implemented in `crates/core`: `demo` feature; `runtime/mock.rs` available under `cfg(any(test, feature = "demo"))`,
clock `Arc<dyn Clock>`, no `unwrap` left; `runtime/demo.rs` (`DemoWorld`) and `runtime/demo_data.rs`; schema v3
(`item_archives`); `Engine::archive`, `unarchive`, `archived_items`; FFI contract of 4.5; 13 tests added.
Deviations from sections 4.4 to 4.7, all in the direction of compatibility:

1. **`runtime::ItemView` did not change**: the Windows app builds literal `ItemView`s in its tests. The archive is
   read by `Engine::archived_items()` and merged only into `FfiItem` (`archived`, `archived_at_ms`).
2. **`archive` of an outgoing `Queued`/`Failed` item has no effect** (not `InvalidInput`): the Kotlin contract says that only
   storage errors surface. The interface does not offer the action anyway (`canArchive`).
3. **`confirm_pairing` without a pending pairing is ignored** (not `InvalidInput`), as `FakeTomiCore` and `PairingFlowTest`.
   A pairing requested while the device is already in the space returns `InvalidInput`.
4. **`DemoConfig.device_name`** added (name of this device in the demo space).
5. **Dates**: the history is written in a virtual past (demo clock that only moves forward): the feed's times
   are those of Android. Pins and archives are set at opening (real time).
6. **The secret sent "after reading"** is dated 1 h 30 ago (Android: 3 h) so that it stays "sent" to PC-Maison,
   which updates only once, just after the link sent ("delivered to all").
7. **Files**: images and code read from `assets_dir` (Android's demo assets); otherwise a 1 × 1 PNG image and short
   code excerpts. PDFs are generated (valid one-page PDF); "annexe.docx" becomes "annexe.txt".
   Thumbnail: `<name>.thumb.jpg` if it exists, otherwise the image itself if it is at most 128 KiB.
8. **No doorbell in demo**: `run_doorbell` blocks until `stop_doorbell` without network; a background thread syncs
   PC-Bureau and this device every 2 s (disabled in unit tests, which start it explicitly).
9. **Demo pairing latency**: 800 ms as on Android, none in tests.
10. The Apple script builds with `--features ffi,demo` by default; **never pass `--no-demo` for a TestFlight or App
    Store archive** since the unified core (0.3): the released app offers the demo (`openDemo` and `enterDemo` return an
    error without it, and the app hides its entry). The recipe `just apple-core --no-demo` passes it to the script.
11. **Archive table**: `item_archives (item_id, archived, at_ms)`, last decision (archiving as unarchiving),
    same shape as `pins`, so that a future shared last-writer-wins state can merge into it without a new migration.
    Re-archiving after a pin has won takes a new time.
12. **Local archive but shared unpinning**: archiving a pinned item unpins it on all devices (the pin
    is shared) while the archive stays local. The Kotlin contract announces the archive as "on all devices": a temporary deviation
    until the synchronized archive. The interface's "Undo" must re-pin by itself (already the case on Android,
    `ArchiveSwipe.undo`); to be reproduced on iOS.
13. **Robust demo mode** (review): absolute `files_dir` required; demo folders erased at closing and at opening;
    the background thread only syncs this device once joining is complete (a "No" to the history is never
    caught up); no `join` of the thread on itself if the last reference falls in a callback; asset thumbnails
    capped at 128 KiB; key copies erased from memory.
14. **CI and `justfile`**: core clippy and tests also run with `--features demo`.

### 4.9 PR-2 verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p tomi-core --all-targets --features demo -- -D warnings
cargo test -p tomi-core --features demo
cargo test --workspace
cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1   # must print "unchanged"
just apple-core && just apple-core-test                                # bindings regenerated with the new contract
```

The relay (`apps/relay`) depends on `tomi-core` with `default-features = false`: check that it still compiles.

---

## 5. PR-3 — Apple: project skeleton, `TomiDesign`, `TomiKit` (model), `TomiRuntime`, empty app

**Branch**: `apple/skeleton`. **Area**: `apps/apple/`, `.github/workflows/ci.yml` (job `apple`, optional, see 5.8).
**Prerequisites**: PR-2 merged (the full FFI contract), `brew install xcodegen`.

### 5.1 Files to create

```
apps/apple/
  project.yml
  Config/Base.xcconfig            # base PRODUCT_BUNDLE_IDENTIFIER, versions, SWIFT_VERSION, IPHONEOS_DEPLOYMENT_TARGET=17.0
  Config/Debug.xcconfig           # TOMI_RELAY_URL=http://localhost:8080 ; TOMI_DEMO_AVAILABLE=1
  Config/Release.xcconfig         # no default relay (TOMI_RELAY_URL empty in Base) ; Config/Local.xcconfig, git-ignored, may provide one (Local.example.xcconfig committed)
  Config/Signing.local.xcconfig   # git-ignored: DEVELOPMENT_TEAM=… ; a Signing.local.example.xcconfig is committed
  Tomi/
    App/TomiApp.swift              # @main, AppContainer in the environment, scenePhase → container
    App/AppContainer.swift         # mirror of TomiApplication.kt/AppContainer: core, prefs, biometricGate, clipboard, scheduler…
    App/RootView.swift             # mirror of TomiRoot.kt: NavigationStack, destinations, deep links
    App/Routes.swift               # enum Route { pair, onboarding, feed, send, devices, item(id), viewer(id, index), code(id, index) }
    Resources/Assets.xcassets      # AppIcon (generated placeholder night/sun, cf. 5.7), AccentColor = brand
    Resources/Localizable.xcstrings   # generated by tools/strings-from-android.py
    Resources/Fonts/Inter-Variable.ttf, JetBrainsMono-Variable.ttf   # copied from design/fonts/ + both OFL licences
    Resources/Demo/                # copies of apps/android/app/src/debug/assets/demo/*.jpg + thumbnails *.thumb.jpg,
                                   # demo_facture.pdf, demo_contrat.docx (small demo files) — Debug only
    Info.plist                     # CFBundleURLTypes tomi-share:// ; NSCameraUsageDescription ; NSFaceIDUsageDescription ;
                                   # NSPhotoLibraryAddUsageDescription ; UIBackgroundModes fetch,processing ; BGTaskSchedulerPermittedIdentifiers
    Tomi.entitlements              # associated-domains applinks:<relay> (to declare per build: the host is no longer unique, not yet enabled) ; app groups group.eu.shaffe.tomi ; keychain-access-groups
  Packages/TomiDesign/
    Package.swift
    Sources/TomiDesign/Tokens.swift            # generated by tools/tokens-to-swift.py
    Sources/TomiDesign/TomiTheme.swift         # ColorScheme → colours ; style-guide radii ; spacing
    Sources/TomiDesign/TomiTypography.swift    # Font.custom Inter/JetBrains Mono + style-guide sizes ; font registry
    Sources/TomiDesign/TomiIcon.swift          # enum TomiIcon → SF Symbol (style-guide table §6.2 + action icons)
    Sources/TomiDesign/TomiLogo.swift          # symbol Shape (style-guide geometry §2.3) + wordmark as SVG image → PNG@3x
    Sources/TomiDesign/Components/             # TomiCard, TomiChip, TomiPrimaryButton, TomiLinearProgress, TomiFeedbackBanner
    Tests/TomiDesignTests/TokensTests.swift    # AA contrast of the style-guide §4.3 pairs ; no sun yellow in the UI
  Packages/TomiKit/
    Package.swift
    Sources/TomiKit/Model/Model.swift          # port of Model.kt (see 5.3)
    Sources/TomiKit/Model/TomiCore.swift       # protocol (see 5.4)
    Sources/TomiKit/Model/DeliveryStatus.swift # port of DeliveryStatus.kt
    Tests/TomiKitTests/RedactionTests.swift    # port of RedactionTest.kt
  Packages/TomiRuntime/
    Package.swift                              # depends on TomiCore, TomiKit
    Sources/TomiRuntime/RustTomiCoreAdapter.swift   # see 5.5
    Sources/TomiRuntime/CoreFactory.swift           # opens the real or demo core (5.6)
    Sources/TomiRuntime/WrapKeyStore.swift          # Keychain: 32 bytes, AfterFirstUnlockThisDeviceOnly, shared group
    Sources/TomiRuntime/CorePaths.swift             # Application Support (or App Group): tomi.db, files/, demo/
    Sources/TomiRuntime/CoreMapping.swift           # Ffi* → TomiKit (FfiItem → TomiItem, etc.)
    Tests/TomiRuntimeTests/AdapterTests.swift       # demo: opening, items, listener → update
  tools/strings-from-android.py
  tools/tokens-to-swift.py
  tools/build-core.sh                          # existing; add `demo` to the features
```

### 5.2 `project.yml` (XcodeGen)

- `name: Tomi`, `options: { bundleIdPrefix: eu.shaffe, deploymentTarget: { iOS: "17.0" }, xcodeVersion: "27.0" }`.
- `packages`: `TomiCore`, `TomiKit`, `TomiDesign`, `TomiRuntime` as `path:`.
- `targets.Tomi`: `type: application`, `platform: iOS`, `sources: [Tomi]`, `dependencies`: the 4 packages,
  `settings.base`: `PRODUCT_BUNDLE_IDENTIFIER: eu.shaffe.tomi`, `SWIFT_VERSION: 5.0`,
  `SWIFT_STRICT_CONCURRENCY: targeted`, `CODE_SIGN_ENTITLEMENTS: Tomi/Tomi.entitlements`, `configFiles` pointing to `Config/`.
  `info.properties` or an explicit `Info.plist`. `scheme` with `testTargets: [TomiTests]`.
- `targets.TomiTests`: `type: bundle.unit-test`, `sources: [TomiTests]` (created empty in PR-3, filled from PR-5 on).
- The `TomiShare` target is added in PR-8, the macOS target later.
- `Config/Signing.local.xcconfig` included by `Base.xcconfig` via `#include? "Signing.local.xcconfig"` (optional: without
  a team, the simulator build still works).

### 5.3 `TomiKit/Model/Model.swift` — port of `Model.kt`

1:1 port, value types:

```swift
public enum SourceFormat { case plain, markdown, html }
public struct RichText: Equatable { plain, html?, markdown?, sourceFormat (default derived as in Kotlin) ; description redacted }
public struct Attachment: Equatable { name, mime, size: Int64, thumbnail: Data?, localPath: String?, available: Bool ; isImage }
public enum ReadPolicy { case normal, afterFirstRead }
public enum DeliveryState { case queued, sent, delivered, expiredBeforeDelivery, failed }
public struct DeviceRef: Equatable { id, name }
public struct DeviceDelivery: Equatable { deviceId, deviceName, state }
public enum UnreadableReason { case updateRequired, decryptionFailed, keyPending, suspect }   // 4 values from the FFI
public enum ItemContent: Equatable { case readable(title: String?, text: RichText?, url: String?, attachments: [Attachment], secret: Bool)
                                     case unreadable(UnreadableReason) }
public struct TomiItem: Identifiable, Equatable { id, content, sourceDevice, createdAt: Date, expiresAt: Date?, readPolicy, read,
                                                  deliveries, pinned, pinnedAt: Date?, archived, archivedAt: Date?
                                                  + shortcuts title/text/url/attachments/secret/isReadable ; description redacted }
public func isOutgoing(localDeviceId:), shouldMarkReadWhenOpened(localDeviceId:), kind() -> ItemKind   // same rules
public enum ItemKind { case unreadable, secret, image, file, link, richText, text }
public enum DevicePlatform { case android, windows, ios, macos, cli, other }
public struct Device: Identifiable, Equatable { id, name, platform, lastActivity: Date?, isThisDevice }
public struct DraftAttachment { path, name, mime, size: Int64, thumbnail: Data?, thumbnailMime: String? ; description redacted }
public struct OutgoingDraft { id, title?, text?, url?, attachments, secret, expiresAt?, readPolicy ; description redacted }
public enum DispatchResult { case sent, retryLater, notFound, failed }
public enum SpaceState { case active, revoked, updateRequired, blocked }
public struct TomiCoreError: Error { kind: Kind (storage, network, relay(status, code), blocked, invalidInput, crypto(code), internal), reason }
public enum PairingFailure { case invalidCode, expired, network, unsupported }
public enum PairingResult { case awaitingVerification(device: Device, verificationCode: String, historyItemCount: Int)
                            case failure(PairingFailure) }
```

Accepted difference from Kotlin: `Attachment.uri` becomes `localPath` (absolute path returned by the core); the
`Attachment.available` and `UnreadableReason.keyPending/suspect` come from the FFI and did not exist on Android.

### 5.4 `TomiKit/Model/TomiCore.swift` — the protocol

```swift
@MainActor @Observable public final class CoreState {
    public var isPaired: Bool
    public var spaceState: SpaceState
    public var localDevice: Device?
    public var devices: [Device]
    public var items: [TomiItem]     // most recent first, outgoing queued items included
    public var loaded: Bool          // first read done
}

public protocol TomiCore: AnyObject {
    var state: CoreState { get }     // observed by the views; updated on the main actor
    var isDemo: Bool { get }
    func getItem(id: String) async throws -> TomiItem?
    func sync(pushToken: String?) async throws
    func enqueue(_ draft: OutgoingDraft) async throws
    func dispatch(id: String) async throws -> DispatchResult
    func cancelIfQueued(id: String) async throws -> Bool
    func deleteLocal(id: String) async throws
    func deleteEverywhere(id: String) async throws
    func markRead(id: String) async throws
    func attachmentPath(itemId: String, index: Int) async throws -> String?
    func renameDevice(id: String, name: String) async throws
    func revokeDevice(id: String) async throws
    func pin(id: String) async throws
    func unpin(id: String) async throws
    func archive(id: String) async throws
    func unarchive(id: String) async throws
    func pairWithQr(payload: String) async throws -> PairingResult
    func pairWithCode(code: String) async throws -> PairingResult
    func confirmPairing(importHistory: Bool) async throws
    func cancelPairing() async throws
    func simulateIncoming(kind: DemoIncomingKind) async throws   // demo only
    func purgePlaintextCache() async throws
    func enterForeground()            // starts the doorbell (real); no-op in demo
    func enterBackground()            // stops the doorbell, purges the plaintext cache
}
```

The comments of `TomiCore.kt` are carried over method by method (already in English there).

### 5.5 `TomiRuntime/RustTomiCoreAdapter.swift`

- Wraps `RustTomiCore` (real or demo, same class). Every protocol method does
  `try await queue.run { ... }` where `queue` is a serial queue (`DispatchQueue(label: "eu.shaffe.tomi.core")`): **no
  core call on the main thread**.
- `CoreListener`: class `Listener: CoreListener, @unchecked Sendable` which, on `onChanged()`, schedules a coalesced
  re-read (at most one every 80 ms) on the serial queue: `items()`, `devices()`, `localDevice()`, `spaceState()`,
  `isPaired()`, then publication on `@MainActor` into `CoreState`. The adapter keeps a strong reference to the listener and
  calls `setListener(nil)` in `shutdown()`.
- Errors: `TomiError` (UniFFI) → `TomiCoreError`; the `reason` message never contains content (guaranteed by the core).
- `enterForeground()` (real): a thread named `tomi-doorbell` that calls `runDoorbell()`; `enterBackground()`:
  `stopDoorbell()`, then `purgePlaintextCache()` on the queue. In demo: `enterForeground` performs a `sync()`.
- `CoreMapping.swift`: conversions `FfiItem → TomiItem` (ms → `Date`), `FfiDevice → Device` (`platform` string → enum,
  unknown → `.other`), `OutgoingDraft → FfiDraft`, `PairingOutcome → PairingResult`.

### 5.6 `TomiRuntime/CoreFactory.swift`

```swift
public enum CoreMode { case real(relayURL: URL?), demo(startPaired: Bool, assets: URL?) }  // relayURL nil: no default relay
public static func open(mode: CoreMode, paths: CorePaths, platform: String, appVersion: String) throws -> any TomiCore
```

- Real: `WrapKeyStore.loadOrCreate()` (32 bytes `SecRandomCopyBytes`, Keychain `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`,
  `kSecAttrSynchronizable = false`, access group shared with the extension), `CoreConfig(dbPath, filesDir, wrapKey, relayUrl,
  appVersion, platform: "ios", autoImport: false)`, `RustTomiCore.open`.
  If the database exists but not the key: explicit error, **never create a new key** (Windows rule carried over).
- Demo: `RustTomiCore.openDemo(config: DemoConfig(filesDir: paths.demoDir, assetsDir: Bundle.main.url(forResource: "Demo"...)?.path,
  platform: "ios", appVersion: …, deviceName: UIDevice.current.name, startPaired: …))`. Available only if
  `TOMI_DEMO_AVAILABLE=1` (Debug). The app's demo assets are the Android files
  (`apps/android/app/src/debug/assets/demo/*`), referenced by path in `project.yml` (no copy).
- `CorePaths`: `Application Support/eu.shaffe.tomi/` (PR-8 will move it into the App Group container), `isExcludedFromBackup = true`,
  protection `.completeUntilFirstUserAuthentication` on the folder.

### 5.7 Generator scripts (`apps/apple/tools/`)

- `strings-from-android.py`: reads `apps/android/app/src/main/res/values/strings.xml`, writes
  `apps/apple/Tomi/Resources/Localizable.xcstrings` (xcstrings JSON format, `sourceLanguage: "fr"`, one entry per
  `<string>` and per `<plurals>` with `one`/`other` variations). Conversions: `%1$s` → `%1$@`, `%1$d` → `%1$lld`,
  `\'` → `'`, `\n` kept, XML entities decoded. Keys keep the Android name (`action_copy`). Python 3 without
  dependencies; run with `python3 -I`. The few iOS-only strings (e.g. adapted `onboarding_*`, `share_*`) live in
  `Tomi/Resources/Localizable+ios.xcstrings` (edited by hand) and take precedence on duplicate keys (documented in the script).
- `tokens-to-swift.py`: reads `design/tomi/tokens.json`, writes `Packages/TomiDesign/Sources/TomiDesign/Tokens.swift`:
  `public enum Tokens { public enum Light { static let bg = Color(hex: 0xF6F4EF) ... } public enum Dark {...}
  public enum Font { title = (size: 20, line: 26, weight: 600) ... } public enum Space { s1 = 4 ... } public enum Radius {...}
  public enum Motion {...} public enum Behavior { secretClipboardClearMs = 30000, undoWindowMs = 5000 } }`.
- Both generated files start with `// GENERATED by tools/<script> from <source> — do not edit.`
- App icon: until `design/build/tomi-export.mjs` has an iOS output, generate a provisional 1024 PNG
  (night tile `#24324A`, 22 % radius, sun symbol `#FFC85C` at 0.7, style-guide geometry §2.3) with a small script
  `tools/appicon-placeholder.py` (Pillow allowed locally, not committed to CI) and note it as provisional.

### 5.8 CI (optional, to propose in the PR)

Job `apple` in `.github/workflows/ci.yml` (`runs-on: macos-15`, filtered on `apps/apple/**` and `crates/core/**`):
rustup + iOS targets, `just apple-core` (or the script), `xcodegen`, `xcodebuild build` and `xcodebuild test` on an available
iPhone simulator of the runner, `swift test` for the pure packages. `infra` area: propose it in the PR; the maintainer decides.

### 5.9 PR-3 verification

- `python3 -I apps/apple/tools/strings-from-android.py && python3 -I apps/apple/tools/tokens-to-swift.py`: stable output
  (rerunning changes nothing: `git diff --exit-code`).
- `swift test --package-path apps/apple/Packages/TomiKit` and `TomiDesign`: green.
- `just apple-core` (features `ffi,demo`), `xcodegen generate`, simulator app build without Swift warnings.
- `xcodebuild test -scheme TomiRuntime`: the adapter opens the demo core (`startPaired: true`), `state.items.count == 23`
  after the first load, `simulateIncoming(.link)` brings the count to 24 through the listener.
- Launch the app on the iPhone Duo: themed empty screen (`bg` background, logo at the top), light and dark (capture both).

### 5.10 Done and accepted deviations (read before PR-4)

Done: `project.yml` (targets `Tomi` and `TomiTests`), `Config/*.xcconfig`, partial `Tomi/Info.plist`, asset catalogue;
`TomiDesign` (dynamic light/dark colours, typography, icons, symbol, components), `TomiKit`
(model and `TomiCore` protocol), `TomiRuntime` (adapter, Keychain, paths, factory); token and text generators;
`just apple-project` and `just apple-test` recipes. Tests: TomiDesign 8, TomiKit 9 (macOS), TomiRuntime 6, app 6
(iPhone Duo simulator, iOS 27.2). Deviations:

1. **Wordmark**: no redrawn `TomiLogo.swift`. The style guide's generated SVG (`design/tomi/logo-horizontal.svg`) is
   copied into `Assets.xcassets/TomiLogotype` (vector, "template" rendering, tint `TomiColor.logo`). `TomiSymbol` (SwiftUI
   shape of the symbol, geometry tested against `logo.svg`) serves future uses (animation).
2. **Provisional app icon**: rendered from `design/tomi/logo-app.svg` as a full square variant (iOS applies its
   mask), 1024 px opaque. To be replaced by an iOS output of `design/build/tomi-export.mjs`.
3. **`TomiKit.TomiCore` protocol**: the Rust core module is also called `TomiCore`. Never qualify by the module name
   `TomiCore.`; write `TomiKit.TomiCore` for the protocol. TomiKit types reuse no generated type name
   (`SimulatedIncoming` rather than `DemoIncomingKind`, `PairingResult` rather than `PairingOutcome`).
4. **`CoreState`** also carries `updateURL`; `Attachment` carries `thumbnailMime` and `available` (FFI fields);
   `DispatchResult.failed` exists (the FFI returns it, Android does not).
5. **Debug starts on the already-paired demo core** until the pairing screen exists (PR-6);
   `-TomiCoreMode real` forces the real core. The launch argument is read from `UserDefaults`.
6. **No entitlements** for now (App Group, shared Keychain, Associated Domains): they require a signing
   team and arrive with PR-7 and PR-8.
7. **`Localizable+ios.xcstrings`** does not exist yet: no iOS-specific string is needed at this stage.
8. **macOS CI not added** (the proposal of 5.8 is left to the maintainer).
9. **Captures**: the iPhone Duo has two screens; `xcrun simctl io <udid> screenshot --display=1` (local folder
   `apps/apple/build-shots/`, git-ignored). The "iPhone Duo" simulator can be recreated by Xcode with a new identifier:
   always go through `tools/simulator-udid.sh`.
10. **Independent review** (one blocking point, fixed with the remarks):
    - doorbell: never restarted when the core closes it itself (revocation, space reset, replaced connection,
      `api-v1.md` §5); only for an active space; the stop is repeated while the feed runs (the core resets its
      stop flag to zero at the start of `run_doorbell`: a clean fix to plan in the core);
    - two queues: local (reads, pin, archive, read, deletions) and network (sync, send, downloads,
      revocation, pairing); purge of decrypted files on its own queue, in an iOS background task;
      cache folder with `complete` protection;
    - `AppContainer`: "opening" phase (the core is never opened twice), a single window
      (`UIApplicationSupportsMultipleScenes = NO`), scene phase remembered during opening, foreground only
      after a real background transition;
    - Keychain: duplicate handled (the key stored first is kept; an invalid leftover without a database is replaced), tests
      hosted by the app; local device kept even when revoked; fallback `deinit`; `Attachment` fully redacted.
    - Left for later: localized error screen and recovery path ("key lost for an existing database", product
      decision), `ITSAppUsesNonExemptEncryption` (PR-10), `kSecUseDataProtectionKeychain` (macOS target), shared
      Keychain access group (PR-8), and the real core in Debug requires a core built in debug (`just apple-core --debug`).

---

## 6. PR-4 — Apple: pure logic ported from Android with its tests

**Branch**: `apple/kit-logic`. **Area**: `apps/apple/Packages/TomiKit/`.

Port each file below into `Sources/TomiKit/Logic/` **together with its test** in `Tests/TomiKitTests/`. Authoritative
source: the Kotlin file, read before writing. Same names, same constants, same test cases.

| Kotlin (`apps/android/app/src/main/java/eu/shaffe/tomi/`) | Swift | Ported test |
|---|---|---|
| `ui/feed/CardTap.kt` | `CardTap.swift` (`enum TapAction`, `handleTap`, `showsReadMore`) | `CardTapTest.kt` |
| `ui/feed/FeedFiltering.kt`, `FeedSections.kt`, `ArchiveSwipe.kt` | `FeedFiltering.swift` (`FeedFilter`, `filter`, `search`), `FeedSections.swift`, `ArchiveSwipe.swift` (`canArchive`, `undo`) | `ArchiveTest.kt` (pure parts), `PinTest.kt` |
| `ui/feed/SwipeArchiveLogic.kt` | `SwipeArchiveLogic.swift` (anchors, thresholds 88 pt / 58 % / 75 %, 10 pt hysteresis, decision on release) | `SwipeToArchiveTest.kt` (logic parts) |
| `share/ContentDetectors.kt` | `ContentDetectors.swift` (`MarkdownDetector`, `HtmlText.toPlain`) | via `SharePayloadParserTest` |
| `share/SecretDetector.kt` | `SecretDetector.swift` | `SecretDetectorTest.kt` |
| `share/LinkExtractor.kt` | `LinkExtractor.swift` | via `SharePayloadParserTest` |
| `share/SharePayloadParser.kt` | `SharePayloadParser.swift` (`ShareInput`, `ParsedShare`, `looksSecret`); `content://` URIs become file `URL`s provided by the extension | `SharePayloadParserTest.kt` (without the Android `StreamPolicy` cases) |
| `share/TextSanitizer.kt` | `TextSanitizer.swift` | included |
| `share/ExpiryPolicy.kt` | `ExpiryPolicy.swift` (`ExpiryChoice`, `options(for:)`, `onContentChanged`, `resolve`) | `ExpiryPolicyTest.kt` |
| `share/ItemSharing.kt` | `ItemSharing.swift` (`canShare`, `payload`); the biometric part stays in the app | `ItemSharingTest.kt` (pure part) |
| `deeplink/DeepLink.kt` | `DeepLink.swift` (`DeepLink`, `DeepLinkParser.parse(URL)`); pairing and creation links read by the core (`LinkReading`, `CoreLinkReader`), the relay named in the link never trusted | `DeepLinkParserTest.kt` |
| `send/UndoDeadline.kt` | `UndoDeadline.swift` (injectable monotonic clock, `remainingMs`, `fraction`, `isOver`, `waitBeforeDispatchMs`) | `UndoSendTest.kt` (pure parts) |
| `ui/render/RenderMode.kt`, `ClipDetection.kt`, `MarkdownGuard.kt` | `RenderMode.swift`, `ClipDetection.swift`, `MarkdownGuard.swift` (`![` escaping; the md4c parse is replaced by a regex-based image-node detection on the escaped text — note the deviation) | `MarkdownGuardTest.kt`, part of `ItemLogicTest.kt` |
| `ui/render/HtmlSanitizer.kt` | `HtmlSanitizer.swift`: allow-list of tags/attributes by regex (no Jsoup) + document with CSP `default-src 'none'; img-src data:; style-src 'unsafe-inline'` | part of `ItemLogicTest.kt` |
| `notify/NotificationLogic.kt` | `NotificationLogic.swift` (`TapAction.forItem`, `NotificationBatcher` 10 s) | part of `ItemLogicTest.kt` |
| `ui/common/Formatting.kt` | `Formatting.swift` (`fr_FR` locale, timestamps, sizes `2,4 Mo`, remaining time, `expiryLabel`, `deliveryLabel`) | part of `ItemLogicTest.kt` |
| `security/AuthWindow.kt` | `AuthWindow.swift` (2-minute window, injectable monotonic clock, `ClipboardClearPolicy`) | part of `ItemLogicTest.kt` |
| `ui/viewer/ViewerLogic.kt` (pure) | `ViewerLogic.swift` (`ViewerImages`, `ViewerCounter`, `ZoomMath`, `GalleryName`); EXIF/subsampling stay on the app side | `ViewerLogicTest.kt` (pure parts) |
| `ui/code/CodeFileLogic.kt` (pure) | `CodeFileLogic.swift` (extensions, limits, decoding, line/column document, search) without syntax colouring | `CodeFileLogicTest.kt` (without colouring) |

Rules: no UIKit in `TomiKit`; `Foundation` only. Clocks are injected (`() -> Int64` monotonic milliseconds).
Regexes use Swift `Regex` (iOS 16+) or `NSRegularExpression`.

**Verification**: `swift test --package-path apps/apple/Packages/TomiKit` green; the number of tests is quoted in the PR, with the
list of Android cases not ported and why.

### 6.1 Done and accepted deviations (read before PR-5)

Done in four steps: the shared building blocks (`TextSanitizer`, `LinkExtractor`, `AuthWindow` with `AuthOutcome` and
`ClipboardClearPolicy`), then three disjoint batches ported in parallel, each in its own worktree,
integrated here. 23 logic files in `Sources/TomiKit/Logic/{Share,DeepLink,Format,Security,Code,Viewer,Feed,Notify,Render,Send}`,
313 TomiKit tests green on macOS (`swift test`), no warnings. Deviations to know about for PR-5:

1. **Names**: the card action is called `CardTapAction` (not `TapAction`, which the notifications use); `CardActions`
   has no `onCopy` (copying goes through `RenderMode`); `TomiItem.isShownAsArchived` is a property; swipe constants suffixed
   `Dp` but in points.
2. **Sharing**: `SharedFile(url:name:mime:size:)` replaces Android's `content://` streams; `ShareLimits` holds 25 MB per
   file, 100 MB per item, 20 files (the parser truncates at 20, the size check is `ShareLimits.exceeds`);
   `StreamPolicy` does not exist on iOS (the extension copies the files itself).
3. **Formatting**: `Formatting.swift` does not produce French text but structured values carrying the key of
   `strings.xml` and its arguments (`DeliveryLabel`, `RemainingTime`, `TimestampLabel`, `ExpiryChoice.resourceKey`); only
   the time ("14:02"), the date ("6 nov.") and the sizes ("2,4 Mo") are formatted in French inside TomiKit. PR-5
   resolves the keys with `Localizable.xcstrings`.
4. **Code**: no colouring in v1; the tests for splitting colour passes are not ported. Positions in a code
   document are in UTF-16 units (as in Kotlin and `NSRange`).
5. **Swipe**: only the decision logic is ported (anchors, thresholds 88 pt / 58 % / 75 %, hysteresis, decision on release
   by distance, single haptic feedback, single delivery); the animation will be written in SwiftUI.
6. **Security, stricter than Android**:
   - `HtmlSanitizer` is a dependency-free tokenizer that rebuilds the HTML: "relaxed" allow-list, links http,
     https, mailto and tel only, images `data:image/...;base64` only (otherwise the image disappears), entities decoded
     before the scheme check, dangerous elements removed with their content, depth capped;
   - `MarkdownGuard` replaces the md4c parse with a scanner that refuses any text where an image could remain; it also refuses
     bypasses that Kotlin's escaping let through (a backtick swallowed by an HTML tag, a code-block close inside a list...). Consequence: legitimate Markdown that cites `![` inside code next to a link may be displayed as plain text.
7. **Deep links**: `DeepLinkParser.parse` takes the link reader (`LinkReading`) as a parameter: the core reads `https://<relay>/l/pair#…` and `/create#…` links from any relay, and the relay read is shown in full; parameter decoding rewritten (an invalid `%XX` discards the parameter).
8. **Not ported**: everything that tests the storage of `FakeTomiCore` (now tested in the Rust core), `SendCoordinator` and
   the scheduler (PR-6), tests tied to a Compose API (real drag state, image clock).
9. **Independent review** (one blocking point, fixed with the remarks; 322 tests, including one test per bypass
   found):
   - **`MarkdownGuard`**: the scanner alone let images through to Apple's engine (`\r` line endings, GFM automatic links, Unicode spaces after a block close, "inline" mode). The final check is now
     the `AttributedString(markdown:)` parse itself, refused as soon as an image remains; **views must display the
     `AttributedString` rendered by `MarkdownGuard.render(_:syntax:)`**, with the syntax they use, and never
     render HTML from Markdown;
   - lengths in UTF-16 units as in Kotlin (secret detection, "short" notification text, deep-link caps,
     `TextSanitizer`, remainder of a link); secret detection never weaker than Android's;
   - URL validation as strict as `java.net.URI` (no automatic encoding, valid `%XX`, a single `#`, DNS host,
     IPv4 or IPv6; non-ASCII accepted outside the host only);
   - `tomi-share://pair#code` exact (no path, port, user or query); dates always Gregorian; linear sort of the
     leading spaces of URLs in `HtmlSanitizer`; `NotificationTapAction` (renamed); `RenderMode.labelKey` (key of
     `strings.xml`); `MonotonicClock.nowMs()` for `UndoDeadline`; gallery names capped in bytes.
   - **Remainder** (minor): `tel:` kept in HTML links (documented); code search case-insensitive but does not
     fold `İ` and rescans the whole text on each query; rounding of `formatFileSize` to compare with Android; ICU `\s`/`\d`
     broader than Java in `MarkdownDetector`; `ShareInput.sourcePackage` and the list of password managers are Android notions: **PR-6/PR-8 must provide the iOS signal** (clipboard item
     `org.nspasteboard.ConcealedType` -> `markedSensitive`); `DeepLinkParser.parse(nil)` ambiguous between its two overloads;
     swipe constants as `Float`.

---

## 7. PR-5 — Apple: Feed, Card, Detail

**Branch**: `apple/feed`. **Area**: `apps/apple/Tomi/Features/Feed/`, `TomiDesign/Components/` if needed.
Authoritative sources: `FeedScreen.kt`, `ItemCard.kt`, `ItemDetailScreen.kt`, `FeedViewModel.kt`, `SwipeToArchive.kt`,
`FloatingToolbars.kt`, `Feedback.kt`, and the Android UI inventory (section 3 of the former Apple review).

Files:

```
Features/Feed/FeedViewModel.swift      # @MainActor @Observable; port of FeedViewModel.kt: filter, query (≤ 200 chars),
                                       # revealedIds + snapshots, auto-hide after 30 s, hideAll, onCardOpened, reveal, copy,
                                       # copyLink, togglePin, applyArchiveAction/undoArchive, deleteLocal/Everywhere, events
Features/Feed/FeedScreen.swift         # header (32 pt logo, Offline or active-filter chip, ⋮ menu), search mode,
                                       # Pinned/Recent (Archives) sections, empty states, floating bar, snackbar
Features/Feed/FloatingToolbar.swift    # custom floating bar (no TabBar): Search, Filters (menu of 6 filters + dot),
                                       # Devices, square radius-12 Send FAB in brand; never hides (as on Android)
Features/Feed/ItemCard.swift           # all variants (header, unreadable body / hidden closed-open / normal,
                                       # attachment list 3 max + thumbnails, "Read more", status and expiry footer)
Features/Feed/ItemMenu.swift           # ⋮ menu in Android order; DeleteEverywhereDialog
Features/Feed/SwipeToArchive.swift     # two-step gesture (SwipeArchiveLogic): 88 pt pill, 58 % threshold, haptic on arming,
                                       # 220/180 ms animations, return if the item is still there after 450 ms, accessibility action
Features/Feed/ItemDetailScreen.swift   # expanded card, MetadataList (Type, Expiry, Pinned, Status), toolbar Copy/Pin/Archive/Delete/⋮
Features/Feed/FeedEvents.swift         # enum FeedEvent → snackbar message (Android table), 5 s duration, immediate replacement
Features/Render/RichTextViews.swift    # Markdown: AttributedString(markdown:) with style-guide styling (h1 20/26 … mono code 13),
                                       # http/https/mailto/tel links; HTML: WKWebView without JS/network/cache, CSP, truncation measurement
```

Rules to follow (all come from Android):
- Text of a revealed secret: mono, no strikethrough zero (`"zero"` feature), **not selectable**, 6 lines max; title never
  shown while the hidden item is closed; files: only their count.
- Tap on a card: `CardTap.handleTap`; long press: menu. A tap never copies.
- Pinned cards have no swipe; outgoing `Queued`/`Failed` items cannot be archived; never deleted by swipe.
- Android `FlagSecure` → on iOS: revealed content is hidden as soon as `scenePhase != .active` (`hideAll()`), and screen
  capture protection is handled in PR-7.
- No circular indicator: `ProgressView` **linear** only.
- Demo: in Debug, the header's ⋮ menu offers "Simulate receiving …" (5 entries) via `simulateIncoming`.

**Verification**: app launched in demo (`startPaired: true`) on the iPhone Duo; captures (light + dark): feed, search with
filter, hidden card closed/open, open swipe, Markdown detail, hidden-item detail; `xcodebuild test` of
`TomiTests` (ViewModel: reveal/auto-hide, filter, events).

### 7.1 Done and accepted deviations (read before PR-6)

Feed, card (all variants), menu, archive gesture, detail and navigation are in place on the Rust core in demo mode
(23 items). 19 `TomiTests` and 336 TomiKit tests green. Deviations and decisions to know about:

1. **File layout.** `FeedEvents.swift` does not exist: the ViewModel calls `FeedbackCenter.show` directly
   with the Android key of the message (`Common/Feedback.swift`, 5 s, immediate replacement, "Undo" action).
   `RichTextViews.swift` is split into `Render/MarkdownView.swift`, `Render/SandboxedHTMLView.swift` and
   `Render/ClippedContent.swift` (truncation measurement for "Read more").
2. **Part of PR-7 brought forward, in minimal form** (`Tomi/Common/`): `BiometricGate` (Face ID / passcode,
   2-minute window, injectable evaluation for tests and previews), `SensitiveClipboard` (`.localOnly` + 30 s expiry),
   `NetworkMonitor`, `UserPrefs` (preferred rendering mode), `SystemShare`, `L10n`. PR-7 keeps invalidation on
   lock, the biometrics-only variant and screen capture protection.
3. **Attachments**: opened in Quick Look (`.quickLookPreview`) until the viewers of PR-9; a hidden item
   requires authentication first.
4. **Markdown**: the view renders only the `AttributedString` returned by `MarkdownGuard.render` (never an image),
   split into blocks (headings, lists, quotes, code); links open only if they are http(s), mailto or tel.
   **HTML**: non-persistent `WKWebView`, JavaScript off, navigation limited to `about:` and to safe links tapped.
5. **Navigation**: `RootView` holds a `NavigationStack` (routes `item(id)`, `send`, `devices`). Send and Devices
   are provisional screens until PR-6. The detail returns to the feed on its own if the item disappears.
6. **iPhone Duo** (maintainer request, based on "Design for iPhone Duo" and "Prepare your app for iPhone Duo",
   Apple Tech Talks). Measurements on the simulator (safe area, `containerCornerInsets`, `reservedRegions`):
   - **Folded** (outer screen, compact width) and **unfolded in landscape**: the system puts the time, camera and network
     in an 84 pt vertical strip on the side (`toolbarVerticalEdge`, iOS 27.1). The floating bars of the feed and
     the detail go there, vertical, in glass, aligned at the bottom like the system's bottom bars; the content keeps the full
     height and never hides the bar.
   - **Unfolded in portrait**: no side strip; the bar stays horizontal at the bottom and does not hide
     (maintainer request). The system reserves 82 pt at the top, but only the top-right corner is occupied
     (144 × 82 pt: time, camera, Wi-Fi). The feed therefore starts at the top of the screen: logo and menu (or search field
     and filters) sit in that band, beside the block and centred on its height, narrowed by its width.
     When the content scrolls under the block, a glass capsule appears behind the time and the camera (they "float").
     Alignment measured to the pixel: the status content is centred at 48 pt from the top (Safari aligns its buttons at the same
     place), i.e. 7/12 of the band, not its half; the header and the capsule (same margin above and below) are centred there.
     Without a top margin (folded, unfolded landscape), the header is also centred at 48 pt, below the screen's rounded corner.
     Folded in landscape, the camera is at the bottom of the side strip (occupied corner 84 × 82): the vertical bar goes
     above it (`ActionBarPlacement.sideBottomClearance`).
     Pure logic: `TopCornerBand` (same file as `ActionBarPlacement`, tests).
   - **Classic iPhone**: bar at the bottom, always in place (Android no longer hides it on scroll; only the feed header hides when scrolling down and returns when scrolling up); no strip (the island is in the middle,
     the rounded corners are less tall than the margin).
   Shared component: `Common/FloatingActionBar.swift` (`ActionBarScaffold` which passes a `ScreenLayout` to the content,
   `FloatingActionBar`, `tomiGlass`). On large screens, the feed, its header and the detail sit in a centred column
   of 680 pt (`TomiLayout.readableWidth`). The whole feed page scrolls, header and filters included; opening search
   from the bar scrolls back to the top, where the field is.
7. **Previews**: `Tomi/Preview/PreviewFixtures.swift` (`#if DEBUG`) provides 14 Swift items covering each variant and an
   in-memory `PreviewCore` (pin, archive, read, delete change the state as the real core does). It also serves
   the ViewModel tests. The app itself still runs on the Rust core.
8. **Search bar**: when typing, the bottom floating bar fades out (the keyboard is up); in the side strip
   it stays. Search: the title of a hidden item is not searched either (Android rule); only the source device is.
9. **Not verified on the simulator**: revealing a hidden item (the simulator has neither an enrolled Face ID nor a
   passcode), covered by the ViewModel tests; the detail's help text still speaks of "fingerprint" (Android string), to adapt in PR-7.
10. **Archive gesture**: from iOS 18, a SwiftUI `DragGesture` placed on a card blocks the touch scrolling of the
    `ScrollView` (observed on the simulator: the wheel scrolled, the finger did not). The swipe is therefore a
    `UIPanGestureRecognizer` (`UIGestureRecognizerRepresentable`) that starts only on a mostly horizontal movement;
    iOS 17 keeps the `DragGesture`. The feed's sticky header hides when scrolling down and returns when scrolling up (iOS 18+, `onScrollGeometryChange`),
    and scrolling closes the open archive pill.
11. **Liquid Glass** (iOS 26+): the capsule of the floating bars is interactive glass (`glassEffect`, inside a
    `GlassEffectContainer`), the main button is `brand`-tinted glass (square, radius 12 from the style guide). The current state
    reads at a glance: search open or filter applied = `brand` icon on a `brandContainer` chip (and a dot on
    Filters), like the selected tab of a system bar. The header's "⋮" button and the search field are
    also glass; menus are the system ones (native iOS 26 glass). iOS 17 to 25 keep the opaque surface. Menus
    use `menuOrder(.fixed)` ("All" stays first when the menu opens upwards).
12. **Independent review integrated**: URL of a revealed hidden item not selectable; masking as soon as the scene
    is no longer active (app switcher, Control Centre), the authentication window only closes in the background;
    file of a hidden item never in Quick Look (the detail opens, as on Android) and preview closed on masking; delivery
    labels with typed arguments (a device named "42" crashed the feed); a single Quick Look presenter
    (RootView); share sheet of a hidden item without "Copy" and subject via `UIActivityItemSource`; revealed secret in mono with
    strikethrough zero, 6 lines, character count in UTF-16 and only if there is text; automatic return to the detail even
    if the item arrives after the screen; scroll detection by accumulation (24 pt). The detail's copy mode already follows
    the card (the Preview/Markdown toggle saves the preference that copying reads).

---

## 8. PR-6 — Apple: Send, Pairing, Onboarding, Devices

**Branch**: `apple/send-pairing-devices`. **Area**: `apps/apple/Tomi/Features/{Send,Pairing,Onboarding,Devices}/`, `TomiRuntime/Send/`.
Sources: `SendScreen.kt`, `SendComponents.kt`, `SendViewModel.kt`, `SendCoordinator.kt`, `FileAttachmentStaging.kt`,
`WorkManagerSendScheduler.kt`, `PairingScreen.kt`, `PairingViewModel.kt`, `QrScanner.kt`, `OnboardingScreen.kt`, `DevicesScreen.kt`.

- `TomiRuntime/Send/SendCoordinator.swift`: `submit(ShareInput) → Submission` (staging of files in
  `files/outgoing/<draftId>/`, limits 25 MB / 100 MB / 20 files, sanitised names, `skipped`), `enqueue`, deadline
  `now + 5000 ms` (monotonic clock), scheduling of `dispatch`, staging cleanup; `cancel(Submission)` →
  `cancelIfQueued` then cancellation of the task.
- `TomiRuntime/Send/SendScheduler.swift`: protocol `SendScheduling`; "in-app" implementation (a `Task` that waits for the end of
  the window then `dispatch`; `RetryLater` → exponential retry starting at 15 s while the app lives); `ensureScheduled` at startup
  for items still `Queued`. The `BGProcessingTask`s come in PR-10.
- `Features/Send/SendViewModel.swift` + `SendScreen.swift` + `SendComponents.swift` (`DetectionRow`, `ExpiryChips`,
  `SentBanner` with a linear bar refreshed every 50 ms, `SendOptionsSheet`). **Paste** button = the only read of
  `UIPasteboard.general` (`string`, `html` via `UTType.html` if present); no reliable equivalent of the Android "sensitive" flag:
  only the `SecretDetector` heuristic is applied (deviation documented). Attachments: `PhotosPicker` +
  `fileImporter(allowsMultipleSelection: true)`, safe copy into the staging (`startAccessingSecurityScopedResource`).
  Thumbnails: generated by the app for images (`UIImage` resized to 360 px, JPEG 0.8): this is the only place where
  the app produces thumbnails, as the FFI expects (`FfiDraftAttachment.thumbnail`).
- `Features/Pairing/`: phases Input → Working → Verify → History → Done; QR scan with `DataScannerViewController`
  (VisionKit) or `AVCaptureSession` + `AVCaptureMetadataOutput` (fallback); camera permission requested on tap; 6-digit code
  mono 28 semibold tabular in two groups; history yes/no ("Yes" preselected); "Device added" screen then
  `UNUserNotificationCenter.requestAuthorization`. System back in Verify/History = `cancelPairing`. In Debug: a
  "Enter demo mode" button (opens a demo core with `startPaired: false` and chains `pairWithCode("7-crayon-lune")` + `confirmPairing(true)`).
- `Features/Onboarding/`: iOS-adapted text ("Add Tomi to your share favourites: Share → More → Edit"), a button
  that opens a `UIActivityViewController` with a sample text; `onboardingDone` in `UserPrefs`.
- `Features/Devices/`: list, rename (40 characters), revoke behind `LAContext` `.deviceOwnerAuthenticationWithBiometrics`
  (biometrics only, never remembered), snackbar if unavailable.
- `App/UserPrefs.swift`: `UserDefaults` suite `eu.shaffe.tomi` (App Group in PR-8): `render_mode`, `code_wrap`, `onboarding_done`.

**Verification**: full demo path from pairing (code, verification, history, notifications) to a send
cancelled within 3 s, then a send delivered to PC-Bureau (the status changes to "Delivered to 1 of 2"); captures.

### 8.1 Done and accepted deviations (read before PR-7)

Verified on the iPhone Duo simulator (demo core): pairing by short code ("test"), verification 482 913,
history (23 items), "Device added", notification permission, first-launch screen, feed; sending a
text with the "Sent · Undo · Options" banner, then the item "Delivered to 1 of 2"; device list. 52 `TomiTests`
(coordinator, scheduler, file copies, Send screen, pairing) and 339 TomiKit tests green.

1. **Paste**: a bar button that reads `UIPasteboard` only on tap (iOS asks "Allow Paste?" unless the setting is changed).
   The system `PasteButton`, which asks for nothing, stays disabled in the glass capsule
   (secure control judged covered): abandoned. Equivalent of the Android `EXTRA_IS_SENSITIVE` flag: the type
   `org.nspasteboard.ConcealedType` set by password managers (locks "Hide" as on
   Android); otherwise the `SecretDetector` heuristic. Pasted HTML is kept when the source provides it.
2. **Attachments**: "Attach" menu → Photos (`PhotosPicker`, images and videos, original name and format) or Files
   (`fileImporter`, secure access). A chosen file is copied immediately into `files/picked/<id>/` (the system lends it
   only for the duration of the choice), then into `files/outgoing/<draftId>/` at send time (limits 25 MB / 100 MB / 20, names
   cleaned); the copies leave after the undo window, on removal, or at the next startup. Image thumbnail
   made by ImageIO (360 px, JPEG 0.8, without metadata: no GPS position).
3. **Sending**: `TomiRuntime/Send` (`SendCoordinator`, `InAppSendScheduler`, `FileAttachmentStaging`). The scheduler
   waits for the end of the window on the monotonic clock then `dispatch`, exponential retry from 15 s (cap 15 min); at
   startup, items from this device still "queued" are sent again. No background task before PR-10: an offline send
   waits for the app to come back. `SendOptionsSheet` is not used in the app ("Options" renders the draft,
   as on Android): it will come with the share extension (PR-8).
4. **Pairing**: `PairingInput` (TomiKit, tests) validates the code and accepts from a QR only the relay's pairing link.
   Scanner: VisionKit `DataScannerViewController`, `AVCaptureSession` fallback; on the simulator, no camera: a message
   and a redirect to the code. Camera requested on tap (`NSCameraUsageDescription`); refusal → link to Settings.
   Back during verification or history = `cancelPairing`. "Enter demo mode" (Debug): the unpaired demo core
   (replaces the real one if needed), then code and confirmation, straight to the feed. Unpaired Debug launch:
   argument `-TomiStartUnpaired YES`. The first screen is chosen once the core state has been read; a device removed from
   the space goes back to pairing.
5. **First launch**: iOS text (Share → More → Edit → favourites), a line saying that Tomi appears in the share sheet
   only with the extension (PR-8). Reachable through "Share help" in the feed's "⋮" menu.
6. **Devices**: list (icon, "This device · iOS · 5 min ago"), rename (40 characters, alert with a field), revoke
   behind Face ID / Touch ID only (`BiometricGate.authenticateStrong`, never the passcode, never remembered), otherwise
   "revocation impossible".
7. **Texts specific to the Apple apps**: `tools/strings-apple.json` (keys `ios_`, merged by `strings-from-android.py`),
   for what Android says differently (first launch, no camera, iOS / macOS platforms).
8. **Link `tomi-share://send` and `…/l/pair#`**: `SendViewModel.prefill` and `PairingViewModel.offerPayload` exist; wiring the
   links comes in PR-7.
9. **Hidden content while typing**: dots instead of the text until the eye is tapped, and editing is done
   revealed. No `SecureField`: a single line, and it clears everything on the first keystroke after returning to it.
10. **Independent review integrated**: "Cancel" runs only once and the end of the window waits for an
   ongoing cancellation (otherwise the files of the rendered draft were erased); the end of the window belongs to the
   ViewModel, not the view; form, "Paste" and "Attach" inactive during sending; the rendered draft cleanly replaces
   what was started in the meantime; "They are different" and "Back" inactive during confirmation,
   "Device added" only from verification or history; an item resumed at startup waits for the end of its
   window; a little background time (UIKit) so that a send leaves if the user switches app right away;
   revocation: no "Enter the code" button, locked biometrics = "revocation impossible"; "Paste" checks
   what is there before reading (a single request); iCloud file read via `NSFileCoordinator`. Remaining for PR-10: immediate
   retry when the network returns (today the retry follows its delay, 15 min at most).

---

## 9. PR-7 — Apple: security, notifications, links

**Branch**: `apple/security-notifications`. **Area**: `apps/apple/Tomi/{Security,Notify,App}/`, `TomiRuntime/Security/`.
Sources: `BiometricGate.kt`, `Authenticator.kt`, `SensitiveClipboard.kt`, `SecureWindow.kt`, `NotificationChannels.kt`,
`IncomingNotifier.kt`, `IncomingNotifications.kt`, `NotificationActionActivity.kt`, `DeepLink.kt`, `MainActivity.kt`.

- `BiometricGate`: `LAContext`, `.deviceOwnerAuthentication` (biometrics or passcode) for `forSecret/forShare/forImageView/
  forFileView/forImageSave` with the 2-minute `AuthWindow`; `.deviceOwnerAuthenticationWithBiometrics` for
  `forRevocation`, never remembered. The window is invalidated at `scenePhase == .background` and on
  `UIApplication.protectedDataWillBecomeUnavailableNotification` (lock).
- `SensitiveClipboard`: `UIPasteboard.general.setItems([[UTType.plainText.identifier: text]], options: [.localOnly: true,
  .expirationDate: now + 30 s])` for a secret; memory (`changeCount`, deadline); `clearIfStillOurs()` on return to the foreground
  (compare `changeCount`, never read the content). Non-secret: `setItems` without expiry, "Tomi" label absent on iOS.
- `ScreenCaptureShield`: on screens showing a secret (Send in hidden mode with text, revealed card, viewer,
  code), show an opaque veil when `UIScreen.main.isCaptured` or on `userDidTakeScreenshotNotification` (after the fact,
  the secrets are hidden). The limit is documented: iOS does not prevent capture.
- Notifications: `UNNotificationCategory` `incoming_links`, `incoming_text`, `incoming_files`, `incoming_secrets`; actions
  "Copy" / "View" depending on `TapAction`; secret: `interruptionLevel .active`, no preview, title "Hidden content received from X",
  generic body; `IncomingNotifier` observes `CoreState.items` (first emission ignored; cancels notifications of items
  read elsewhere or deleted; nothing if `scenePhase == .active`); 10 s grouping → summary "N new items";
  `UNUserNotificationCenterDelegate`: tap → `TapAction` (link → `UIApplication.open` + `markRead`; secret → open the app
  on a transparent view, biometrics, sensitive copy; image → viewer; file → QuickLook; short text → copy).
- Links: `onOpenURL` → strict `DeepLinkParser` (`tomi-share://send|item|open`, `https://<relay>/l/pair#…`, `https://<relay>/create#…`, read by the core); only
  navigates or pre-fills; `applinks:<relay>` is not in the entitlements (the host is no longer unique: to declare per
  build, with that relay's AASA file).
- App Shortcuts (`AppIntents`): "Send with Tomi" → opens the Send screen (equivalent of the Android tile).

**Verification**: `TomiTests` on `AuthWindow`/`ClipboardClearPolicy` (already ported) and `IncomingNotifier` (with a
test `TomiCore` double **in the tests only**); on the simulator: `simulateIncoming(.secret)` with the app in the background
→ notification without preview; simulated Face ID (`Features > Face ID > Enrolled`) for "Show".

### 9.1 Done (PR-11) and deviations

- Security: the 2-minute biometric window closes on lock (`protectedDataWillBecomeUnavailable`) and in the
  background; revealed items are masked again on lock, after a screenshot
  (`userDidTakeScreenshotNotification`: iOS does not prevent it) and as soon as the screen is recorded or mirrored. Sensitive screens
  (`sensitiveScreen()`: Create your space, Join your space, Add a device, Send of a hidden item being typed,
  and the feed when a hidden item is revealed) are covered by a veil (`PrivacyCover`) in the app switcher preview
  and during screen recording. The clipboard of a secret keeps the system's 30 s expiry and
  `localOnly` (Android's `changeCount` tracking is useless: iOS clears it itself).
- Notifications: `IncomingNotifierState` and `IncomingNotificationContent` (TomiKit, tests); the app tracks items with
  `withObservationTracking` (a view is not updated in the background), posts through `UNUserNotificationCenter`,
  groups by thread (`threadIdentifier`: native grouping replaces Android's "N new items" summary),
  "Copy" / "View" buttons, tap routing by `NotificationTapAction` (link opened, short text copied,
  hidden item copied after biometrics, file opened, otherwise Detail). Deviation: a hidden or unreadable item only shows the
  public title "Tomi: new item" (iOS cannot keep a notification off the lock screen like
  `VISIBILITY_SECRET`). Debug menu "Simulate a hidden item in 5 s" to see a notification.
- Links: `onOpenURL` → strict `DeepLinkParser`; `tomi-share://send` pre-fills the Send screen (nothing is sent), `tomi-share://item/…`
  opens the Detail, `tomi-share://pair#…` (device outside the space) opens Join your space with the card "Invitation link
  received". `applinks:<relay>` is not in the entitlements: it requires a team signature, the relay's AASA file
  and, since the host is no longer unique, a declaration per build (PR-14, TestFlight).
- "Send with Tomi" shortcut (AppIntents): opens Send.
- Cold start: a link or notification tap arriving before the core has read its state is kept and replayed
  at the end of `coreLoaded()`; what is requested during onboarding (pairing, "Device added") is taken as soon as
  the feed appears.
- The veil also covers the screen under a system request (`inactive` state: Face ID, alert): accepted; it must be in
  place before the system snapshot taken when going to the background.
- With the real core (PR-14), no notification leaves while the app is suspended: background tasks or
  push (doorbell) will be needed; the demo core only does what happens while the app is open or just after.
- Not verified on screen: opening the app by tapping a notification (the simulator does not reliably unlock
  the lock screen); the logic is the same as for links.

---

## 10. PR-8 — Apple: share extension

**Branch**: `apple/share-extension`. **Area**: `apps/apple/TomiShare/`, `project.yml`, `TomiRuntime/`.
Sources: `ShareActivity.kt`, `ShareViewModel.kt`, `ShareIntentReader.kt`.

- Target `TomiShare` (`type: app-extension`, extension point `com.apple.share-services`), `NSExtensionActivationRule`:
  text, URL, images, files (`NSExtensionActivationSupportsText`, `...WebURLWithMaxCount 1`, `...ImageWithMaxCount 20`,
  `...FileWithMaxCount 20`). SwiftUI interface hosted by a `UIViewController` (not `SLComposeServiceViewController`).
- **App Group** `group.eu.shaffe.tomi`: `CorePaths` moves into the shared container; `WrapKeyStore` uses the shared
  Keychain access group; `UserPrefs` uses the group's suite. Migration: on the app's first launch, move
  `tomi.db` and `files/` from the old location to the group (or migrate nothing if the app was never published: state it
  in the PR; no version is published to date, so **no migration**, the App Group location is the only one).
- **A single process on the core**: non-blocking exclusive `flock` on `<group>/core.lock`, taken when the core opens,
  released at `shutdown()`. The app calls `shutdown()` at `scenePhase == .background` (after `stopDoorbell` and the purge) and
  reopens the core at `.active`. The extension: takes the lock; if it is busy, retries after 1 s, then a "Open Tomi" state with
  a button that launches the app (`tomi-share://send?...` pre-filled, never an automatic send).
- Flow: `ShareInputReader` (NSItemProvider → `ShareInput`: text, URL, HTML, files copied into the staging, subject) →
  `SharePayloadParser.parse` → not paired: "Open Tomi" card; `looksSecret`: confirmation sheet; otherwise **immediate send**
  (`enqueue` then `dispatch` right after the **short 3 s undo window held by the extension**:
  "Sent · Undo · Options" banner, linear bar, `cancelIfQueued` on Undo); `dispatch` before closing;
  `RetryLater`: the item stays queued, the app will send it (banner "Waiting for network").
- Extension memory limits: no thumbnail for images above 12 MP (left empty), files copied as streams.

**Verification**: from the simulator's Safari, Share → Tomi: link sent in one tap with a 5 s banner; a pasted password
triggers the sheet; app closed/open: the lock works (manual test described in the PR); captures.

### 10.1 Done (PR-12) and deviations

- **The extension does not open the core** (demo core until PR-14: it is erased at each opening, so an extension
  would have its own space). It reads the share (`ShareItemReader`: text, link, HTML, title, files copied as long as
  the system lends them, 25 MB and 20 files at most), passes it to `SharePayloadParser`, then drops it into the app group's
  **share inbox** (`ShareInbox`, TomiKit, tested) with the end of its 3 s undo window. "Undo" takes it back; "Options" takes it back then
  re-drops it with the new settings. The app sends what has passed its window (`ShareIntake`) when the core starts, when
  returning to the foreground and on the extension's Darwin notification: it **claims** it first (atomic rename), so a late "Undo"
  answers "Too late". A claimed share interrupted (app killed while sending) is discarded at the next startup: never sent twice.
  If the core refuses the share, it goes back to the inbox (retried on the next pass) and the app says so ("A share
  could not be sent"): never lost without a word. Item limits (25 MB per file, 100 MB in total,
  20 files) are applied as soon as it is read, before any copy. The inbox has `complete` protection (read only when
  the device is unlocked); reading copies leave at closing, even when the sheet is swiped away.
- `ShareFlow` (TomiKit, port of `ShareViewModel`, tested): immediate send, confirmation sheet for what looks like
  a secret ("Hide content" forced if the source marked it sensitive), "Undo", "Options", Too late,
  Failed, Not paired (the app writes `core.isPaired` into the group's preferences).
- iOS presents a share extension as a **full-screen sheet** (`.medium` heights and transparent presentation
  are not supported): Tomi header, preview of what leaves (dots for a secret), "Sent" banner
  at the bottom. No "Open Tomi" button (a share extension cannot open its app without a private API): the
  "Not paired" message tells the user to open the app.
- No source-app identifier on iOS (`SecretDetector.isPasswordManager` does not apply) and no "sensitive" clipboard
  mark in a share: only the content heuristic triggers the confirmation.
- Deferred to PR-14 (real core): `CorePaths` in the group, the shared Keychain access group (it requires the signing
  team), the `core.lock` lock and direct sending by the extension when the app is not running; the share inbox
  remains the fallback path (core busy). `ExpiryChips`, `SentBanner`, `FlowLayout`, `TomiLayout` and the button styles
  are shared by the app and the extension (`Tomi/Common/SendShared.swift`, `TomiLayout.swift`,
  `FormComponents.swift`).
- Not verified on screen: "Options" and the banner's "Undo" (driving the simulator is slower than the 3 s window;
  it was 5 s before 10 November 2026); covered by `ShareFlowTests` and `ShareInboxTests`. Verified: Safari → Share → Tomi, 5 s banner, then
  the app opened sends the link ("Delivered to 1 of 2").

---

## 11. PR-9 — Apple: image viewer, code screen

**Branch**: `apple/viewer-code`. **Area**: `apps/apple/Tomi/Features/{Viewer,Code}/`.
Sources: `ImageViewerScreen.kt`, `ImageIo.kt`, `CodeFileScreen.kt`, `TomiCodeHighlight.kt`.

- Viewer: fixed `#121620` background, `TabView(.page)` over the item's images, zoom 1–6 (`MagnifyGesture`), double-tap 2.5,
  bounded pan, bars hidden on tap, "2/5" counter 2.5 s after interaction, menu (Save to Photos except for a hidden item via
  `PHPhotoLibrary`, Share, Details, Delete), EXIF via `CGImageSource` + subsampling (`kCGImageSourceThumbnailMaxPixelSize` 4096),
  biometrics if hidden, closes in the background.
- Code: mono 13/19 display with gutter, persistent line wrap, search (counter, ↑/↓, 5000 max, scrolling to the
  match), multi-file tabs, limits 2 MB / binary / 50 KB-2000 lines. **Colouring**: v1 without colouring
  (no "No colouring for this language" banner, plain mono); SwiftTreeSitter is a later option, a separate decision
  (third-party dependency).
- Both screens share `FeedViewModel` (`imageFile`, `codeFile` go through the biometric window).

**Verification**: demo: image items (landscape, EXIF portrait shown upright, 3 images), secret with image (biometrics),
code files (4 tabs, search); captures.

### 11.1 Done (PR-13) and deviations

- Viewer (`Features/Viewer`): full-screen cover above the stack, night background, status bar hidden; a
  tap shows or hides the bar (back, menu); `TabView(.page)` pages; zoom by `UIScrollView` (1 to 6, double
  tap 2.5 around the point, back to fit), which leaves horizontal swiping to the pager while the image is
  fitted; "2/5" counter that fades 2.5 s after the last interaction (without animation with Reduce Motion). `CGImageSource` decoding: EXIF orientation applied, 4096 px and 12 Mpx at most. Menu: Save
  to the gallery (Photos, "add only" permission requested at first use, never for a hidden item), Share
  (copy under its own name in the temporary folder, erased when the sheet closes; for a hidden item: biometrics,
  neither "Copy" nor "Save image" in the sheet), Details, Delete from this device / everywhere.
- Code screen (`Features/Code`): pushed on the stack; mono 13/19 with gutter; vertical scrolling, horizontal
  unless "Line wrap" (preference `code_wrap`, shared by all files); search (highlighted matches,
  current one stronger, ↑/↓, "n/m", scrolling to the match, including by column), tabs for several
  files and "2/3" in the bar; Copy the file (sensitive clipboard for a hidden item), Details. No
  colouring in v1 (no "No colouring" banner).
- Both screens go through `ImageActions` (TomiKit): a hidden item requires biometrics before any read, and a
  file is requested from the core only while the 2-minute window is open; app-switcher veil
  (`sensitiveScreen`) and closing when going to the background. Tapping an attachment line opens the screen for
  its type; another file of a hidden item is never handed to Quick Look.
  The viewer has its own veil (it is presented above the root) and closes on a hidden item as soon as
  the screen is recorded or mirrored; the code screen closes with what was opened from it (Details). Accepted
  deviations: iOS cannot prevent a screenshot (no equivalent of `FLAG_SECURE`); a page loaded
  after the end of the 2-minute window shows "Unable to display" instead of asking for biometrics again (as
  on Android). The temporary copy of a share is protected (`complete`) and swept at startup and in the background.
- Not verified on screen: typing in search (the simulator does not receive a paste), pinch and double tap
  (not drivable), the counter (no demo item with several images); the logic is covered
  by `ViewerLogicTests` / `CodeFileLogicTests` (TomiKit) and `FeedViewModelTests`. Verified: EXIF portrait upright,
  landscape, bars and menu, sharing, code screen (php, rs) and search bar.
- The viewer's commands have since moved from the top bar into the floating bar (section 0.2).
- Note outside this PR: the card thumbnail of `demo_portrait_exif.jpg` stays sideways (the demo core's thumbnail
  does not apply the orientation); the viewer displays it upright.

---

## 12. PR-10 — Apple: real core, background, TestFlight

**Branch**: `apple/real-core`. **Area**: `apps/apple/`. **Prerequisites**: a reachable relay (local `just dev-relay` in
Debug via `http://localhost:8080`) and, for a real pairing, the core's CPace work. Without it, the real core is tested
with `provision` from a `ProvisionData` produced by a test tool (outside the app).

- Real mode by default in Release; Debug chooses demo or real (pairing screen: demo button).
- `BGAppRefreshTask` `eu.shaffe.tomi.refresh` (sync every 15 min at best) and `BGProcessingTask` `eu.shaffe.tomi.send`
  (dispatch of `Queued` items when the network is available); `SendScheduler` registers them.
- `NWPathMonitor` for the "Offline" chip and the "Waiting for network" label.
- `take_resync_notice()` → banner "Older items could not be retrieved".
- Status screens: `Revoked`, `UpdateRequired` (`update_url` link or TestFlight), `Blocked`.
- Signing in Xcode (team, capabilities: App Groups, Keychain Sharing, Associated Domains, Background Modes, Push later),
  archive, internal TestFlight. Signing secrets stay in `Config/Signing.local.xcconfig` (git-ignored).

**Verification**: simulator + local relay + CLI/Windows: send, receive via doorbell, pin, delete everywhere,
history transfer; signed archive build; TestFlight installed on the maintainer's iPhone.

### 12.1 Done (PR-14) and deviations

- **Real pairing**: the adapter joins through a `join_space` session (port of `RealTomiCore.joinWith`): one at a
  time, "already in a space" and the device name set before the call, waiting for the code through
  `wait_for_inviter`, then `confirm` and `wait_done` (transient errors retried after 1, 2 and 4 s; "Cancel"
  goes through a separate queue and unblocks the wait). The demo core keeps the former simulated calls. Space
  creation and invitation already went through the core.
- **Pairing and creation links**: read by the core (`LinkReading` / `CoreLinkReader`: `parse_pair_link`,
  `parse_setup_link`, `is_install_token_shape`), from any https relay; the relay named in a link is not trusted, it
  is shown in full with the six-digit code (`PairingPrompt.relayAuthority`) and, for a creation, before
  the token is sent. No more hard-coded host in the app.
- **Background tasks** (`BackgroundWork`): `eu.shaffe.tomi.refresh` (sync, 15 min at best) and
  `eu.shaffe.tomi.send` (sending queued items, network required), requested when going to the background, real core
  only; a task launched while the app is closed opens the core itself.
- **Verified** against a local relay (`cargo run -p tomi-relay`, Rust core in debug for `http://localhost`) with a
  second device played from the Mac (small Rust program on `tomi-core`: space creation, invitation):
  same verification code on both sides, "Device added", then a text sent from the iPhone received by the other
  device. Only one simulator was running (limited Mac).
- **Deviations**: as on Android, no dedicated "Update required" / "Blocked" screen nor `take_resync_notice`
  banner (revocation and space-not-found go through the departure banner). The share extension
  does not open the core: the app group's inbox (section 10.1) works with the real core, the app sends when it
  opens; the core in the app group, the `core.lock` lock and the shared Keychain group are therefore not
  needed and are not done. The push doorbell (APNs) remains to be done on the relay side.
- A background task launched without a scene does nothing that the foreground does (no doorbell), follows
  items to notify them, and ends in time when iOS takes control back; `BGProcessingTask` in practice runs
  when the device is idle. A send already pushed by the app and pushed again by the task goes through the core's
  `network` queue twice, which answers "already sent": no double send. A "Cancel" during the claim of the code
  closes the channel as soon as it returns. Known remainder: a retry wait (4 s at most) may delay the next
  pairing call.
- To test the real core on an iPhone before TestFlight: scheme "Tomi Dev" (section 0.2).
- **Remaining for the maintainer**: signing with the Apple team (`Config/Signing.local.xcconfig`), App Groups and
  Background Modes capabilities in the developer portal, Release archive (Rust core in release: `build-core.sh` without `--debug`) and
  TestFlight.

---

## 13. Execution brief for a contributor

To be given as is to whoever takes a PR, replacing `<N>`:

> You work in the `tomi` repository (read `AGENTS.md`, `docs/DEV.md`, then `docs/archive/apple-spec-implementation.md`).
> Implement **PR-<N>** exactly as described in the corresponding section: create the indicated branch, touch only the
> indicated area, port the Android files listed after reading them in full, write the requested tests, run the
> verification commands and quote their output. Do not invent product decisions: when in doubt, choose the simplest and
> safest solution and report it. Do not edit `apps/android`, `apps/windows`, `apps/relay`, `spec/`, `design/tomi/`.
> Do not commit generated files. End with a short report: files created/modified, commands and results, what is not
> verified, deviations and why, open questions; update sections 0 and 12 of the spec.

For PR-2, add: "Review before any commit; fix the remarks before committing."
