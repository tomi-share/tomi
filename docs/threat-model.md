# Tomi threat model

What Tomi protects against, and what it does **not**. This page is written for people who want to decide whether to trust
Tomi with their data, and for reviewers. It is based on the specification (`spec/`), the code and `AGENTS.md`, and it was
checked against the code as of 2026-11-19, after the security-audit fixes were merged (they are listed under "Fixed since
the review" below). Where a protection has only been unit-tested and not tried on a device, the text says so. When this page
and the code disagree, that is a bug in one of them: please report it (see [`SECURITY.md`](../SECURITY.md)).

Tomi is end-to-end encrypted sharing between the devices of **one person** (text, links, files, secrets). A **blind**
relay stores and forwards opaque, signed envelopes. There is one *space* per person, one symmetric **space key** shared by
all the devices of that space, and no accounts, recipients or per-device permissions in version 1.

Normative details live in [`spec/format-v1.md`](../spec/format-v1.md) (§1 threat model, §8 trust and rotation, §12 known
limits), [`spec/api-v1.md`](../spec/api-v1.md) (§1.2 what the relay knows, §1.3 transport threat model) and
[`spec/pairing-v1.md`](../spec/pairing-v1.md) (§2). Per-platform behavior is tracked in [`FEATURES.md`](FEATURES.md).

## Summary

| Adversary | Protected? | Short version |
|---|---|---|
| Network attacker | Yes | TLS, and everything is end-to-end encrypted on top of it. They see the relay's address and traffic volume and timing. |
| Relay compromised or malicious (its operator, a hacked server, a stolen backup) | Content: **yes**. Metadata: **no**. Availability: **no** | It cannot read or forge content. It sees who talks when and how much, and it can delay, drop or replay. |
| Google (FCM) | Yes | Receives an empty wake-up with constant content. It learns the time and the device. |
| Lost phone or PC, **locked** | Mostly the OS's job | Tomi relies on the platform's lock and key storage. Revoke the device from another one. |
| Lost phone or PC, **unlocked** (or someone with a few minutes at it) | **Partly** | Non-secret items are readable. Hidden secrets are behind a local authentication that is a gate in the app, not a cryptographic barrier. Revoking, adding a device and deleting the space need a fresh authentication. |
| Device you revoked | The **future** only | It cannot reach the relay or read items sent after the rotation. It keeps the content it already opened; it erases its space keys when it sees the signed rotation. |
| Malware running as you on a device | **No** | Out of scope. It can read what the app can read. |
| Someone who steals a copy of the space key (or a device with it) later | **No retroactively** | There is no forward secrecy: old copies of items at that epoch are readable. |
| A malicious item (HTML, Markdown, link, file) | Yes, by sandboxing | Rendering has no script and no network access. Nothing executes without a user action. |
| Tampered download of the app | Partly | Checksums and build provenance exist for the Windows installer and the Android APK; the installer is not code-signed, and the Android release key is not configured yet. See [`RELEASES.md`](RELEASES.md). |

## What is in the design

- **Cryptography.** XChaCha20-Poly1305 for content, HKDF-SHA256 for derivation, Ed25519 (strict verification) for signed
  statements, X25519 for sealing keys, CPace on ristretto255 for pairing. All are standard primitives from maintained
  libraries; Tomi writes none of them. Every label is domain-separated and every ciphertext is bound to its context
  (`item_id`, `key_epoch`) by its authenticated data. The core forbids `unsafe` code and zeroizes key material. The
  vectors in `spec/vectors/` are frozen and checked by tests.
- **One implementation.** All clients use the same Rust core. The relay depends on the core without its runtime features
  and holds no key.
- **What is encrypted.** The type of an item, its title, text, URL, file names and types, thumbnails and the *secret* flag
  are inside the ciphertext. The title of a hidden secret is encrypted like its content and is not searchable.

## The relay

### What a compromised or malicious relay cannot do

- Read content, titles, file names, the secret flag, which items are pinned or archived. It holds no key.
- Forge or alter an item: an envelope is authenticated, and its authenticated data binds the item identifier and the key
  epoch, so envelopes cannot be swapped between items or epochs.
- Add a device of its own to your space. A device enters the trust store only through face-to-face pairing or a statement
  signed by an existing member (`format-v1.md` §8.4), and the relay never supplies the public key a key is sealed to
  (decision D38).
- Forge a deletion (tombstone) or a key rotation. Both are signed by a member device and verified by every client.
- Take over a pairing: see "Pairing" below.

### What it does see (metadata)

Listed exactly in `spec/api-v1.md` §1.2. In short, per space: the device identifiers, **names** (in clear: they are useful
to the operator who has to choose which device to revoke), platforms, public keys, app versions, activity and presence
times, push tokens; per item: its identifier, the sending device, creation and expiry times, the key epoch, the envelope
size (rounded up to a 256-byte block), and its position in the log; per file: its encrypted size, accurate to within 16 bytes
per chunk, and the number of files; delivery and **read receipts** with their time; deletions, with their time; and for pin
and archive changes, that a device emitted *some* state event, and when (not which item and not what the state means).
It does not see the number of items in a history transfer or their types. A relay hosting several spaces sees the metadata
of all of them. There is no padding of files and no dummy traffic: the pattern of when you send, how much, and from which
device is visible to whoever runs the relay.

### What it can do (availability and ordering are out of scope)

- **Refuse service, delay, drop** items, events, tombstones or rotations. The format detects alteration, not omission. A relay
  that withholds a rotation keeps a revoked device effective for emitters that are not up to date. A relay cannot make a
  client loop forever on a page that never advances: a page with `has_more` that moves the cursor by nothing ends the sync
  (it asks again next time), and an event that cannot be parsed is skipped rather than blocking the cursor.
- **Replay.** Writes are idempotent and applying an event twice changes nothing, so replay cannot alter or duplicate an item.
  A client remembers a deletion (a tombstone) for `max_ttl + 60 days` (**90 days** by default). An item that reaches a device
  with an authenticated creation date older than that is shown as "unreadable (suspect)", kept raw, never retried and never
  consumed, so an item you deleted or that was set to disappear after reading is not brought back by a relay that kept the
  envelope. Limits: the check measures age from the moment the device first receives the item, and the relay still chooses
  the order in which it presents events. The age is measured with the **device's own clock**, on purpose: the relay's clock is
  the relay's word, and a relay that could shift it would switch this check off. The same goes for the `max_ttl` the relay
  announces, which sets both windows: a device never uses more than **one year** of it, so a relay cannot stretch the
  windows past that by announcing ten. The price of the device's own clock is that a legitimate item is also refused (and
  stays so) when the clock skew between sender and receiver, plus the time it waited in the sender's outbox, exceeds 60 days
  (between 60 and 90 days depending on how long the relay held it): a visible failure, never a leak.
- **Invent an unsigned revocation** ("by the administrator", "voluntary departure"), by emitting an explicit `device_revoked`
  event. Honest devices follow it with a signed rotation, which makes the removal permanent, so a malicious relay can make
  them drop a legitimate device, which then has to be paired again. This costs you availability, not confidentiality. The
  core now records each such revocation so that the app can tell you (`take_relay_revocations`), and Android, iOS and Windows
  show it as a one-time notice (iOS code is not compiled yet; nothing was tried on a device). The core records a notice only for a device
it already knows and has verified, accepts only the `via` values `admin` and `self`, shows a name stripped of control characters and
limited to 64 characters, and keeps at most 32 pending notices. A device that is merely missing from the relay's device list during a full resync is no longer treated
  as revoked and never triggers a rotation by itself. The one exception: when the relay also says that a rotation is owed, a verified device it no longer lists is handled like an unsigned revocation (reported, left out of the rotation), so that the rotation can complete instead of failing for good. An empty list, or one that omits this device itself, removes nobody. This gives the relay no power it did not have, and no key is erased.
- **Rename devices** (names are not signed).
- Together with a **revoked device** that still has the old key: inject items sealed with the old epoch key. Mitigation: the
  clients write only with the latest epoch, refuse and flag as "unreadable (suspect)" an old-epoch item whose log position
  is after the rotation they applied, and discard old keys after the retention period below. The position the relay assigns
  is no longer taken on trust: a client never applies an event whose position is not above its cursor, and after the relay
  reports a log older than the cursor (a restore from backup) it moves the cursor to that head and treats every item of a
  replaced epoch it sees for the first time as suspect. A window remains (`format-v1.md` §12): an item injected **before**
  the client applies the rotation, with a position lower than the rotation's, is accepted, for as long as old keys are kept
  (up to 31 days by default). The device that posts the rotation does not take the position the relay answers either, nor the
  position at which the relay later serves the event: it places the rotation at the log position it had reached just
  before posting, so an old-epoch item above it is suspect. The price is that an honest item that another device sealed
  under the old key during the round trip of that very post is hidden on the device that rotated.

### Who is the relay?

Possibly you. The intended deployment is a relay you control (`infra/`). If someone else hosts it, they are in the position of
the compromised relay above, minus the surprise: they see all of your metadata. Tomi does not hide the relay: the pairing
screens show its full address, and a relay named in a link is only kept once pairing succeeds.

## Lost, stolen or unlocked devices

**Locked and lost.** The data on the device is protected by what the platform provides, not by anything Tomi adds on top:
the device lock and the encrypted storage, with the database key held by Android Keystore, the iOS Keychain or Windows DPAPI.
That key is deliberately usable **without the user being present**, because the app must receive and store items in the
background. Consequences: on iOS and Android, a phone that has been unlocked once since boot ("after first unlock") is
exposed to forensic extraction in a way a cold-boot phone is not; on Windows, DPAPI binds the key to the user's logon.
Tomi's remedy is to **revoke the device** from another one (or with `tomi-relay admin revoke` on the relay host), which cuts
its relay sessions at once and rotates the space key. Revoking asks for a fresh authentication, never remembered, every time:
on Android strong biometrics **or the screen lock** (a PIN is enough, so a phone with only a PIN can still revoke; a passive
face unlock needs a confirmation tap), on iOS Face ID or Touch ID only (not the passcode, so an iPhone without enrolled
biometrics cannot revoke from the phone; aligning iOS on the passcode is an open decision), and on Windows Windows Hello or
the account password. Otherwise use another device or the relay's admin command.

**Unlocked, or in someone's hands for a few minutes.** Whoever can use the unlocked app can read every non-secret item, send
as you, and reach the Devices screen. Hidden secrets stay masked until you authenticate with biometrics or the screen lock
(Windows Hello on Windows). Sharing a secret through the system, and copying it, need that authentication. The authentication
window lasts two minutes. It is closed when the screen turns off **and when the app goes to the background** on Android, on
lock and in the background on iOS, and on session lock, disconnect, logoff and sleep on Windows (`session_watch.rs`; the
Windows decision logic is unit-tested, the window was not tried on a PC). There is no app lock yet, and the Recents (app
switcher) thumbnail can show the non-secret feed.

A thief with an unlocked device can also try to revoke your *other* devices before you revoke theirs; the first valid rotation
wins. Revocation requires the fresh authentication above, which is the barrier, and it is a weaker one than the biometrics
alone on Android and Windows, since the screen lock or the account password also opens it. The recourse for you is
`tomi-relay admin` on the relay, and pairing again.

Adding a device is the other sensitive action on a borrowed unlocked device: the history transfer copies **everything**,
secrets and files included. All three clients now ask for a fresh authentication before it: on Android before the invitation
is opened and again before the final "codes identical" tap (strong biometrics or screen lock); on iOS Face ID, Touch ID or the
passcode before the invitation code is made (a device with no screen lock at all cannot add a device); on Windows Windows Hello
when the inviting PC confirms the codes. A device that joins is also announced on the others: a local notification on Android
and iOS (only while the app runs on iOS) and a toast on Windows (not for a device that joined while Tomi was closed; the list
is not persisted). These are detection controls: if you did not add the device, revoke it. None of this was tried on a PC, and
the Android and iOS parts are not verified on a device beyond unit tests ([`FEATURES.md`](FEATURES.md)).

## Revoked devices

When you remove a device, a remaining device creates a new space key (a new *epoch*), seals it for each remaining member, and
signs the rotation together with the list of revoked devices. The relay refuses a rotation that does not move the epoch by
exactly one or whose recipients are not exactly the remaining members, and cuts the revoked device's sessions and
WebSockets immediately. Other devices verify the signature and the recipients before switching.

What the revoked device **keeps**:

- Everything it already stored. When an honest client sees a **verified signed rotation** that lists it as revoked, it marks
  itself blocked and erases its space keys (every epoch). It does **not** erase the content it had already opened, which is
  re-sealed under a local key independent of the space key (`format-v1.md` §8.3 step 6: wiping local data is a separate
  gesture, since any unlocked member can revoke the others). A revocation that only the relay announces (`403 revoked`, an
  unsigned `device_revoked`) blocks the device but erases nothing, because it proves nothing. The erasure is also performed by
  the revoked device's own code: a device that was modified, or that never reconnects, keeps its keys. Treat a revoked device
  as one that has read everything up to the rotation.
- Any space key it copied out, and any old encrypted copy of items it obtained (for example from a relay backup) at those
  epochs.

What it **loses**: access to the relay, and the ability to read anything sealed with a later epoch. A revoked key never
becomes a member again; a reinstalled device gets a new identity.

**A rotation that does not complete.** If the automatic rotation keeps failing because the relay's device set and the client's
disagree (`device_set_mismatch`), the core retries after an hour and sets a persisted `rotation_stalled` flag (cleared when a
rotation succeeds or another member's is applied). Until then the revoked device may still hold the current space key. All three apps show
a calm banner while the flag is set (not tried on a device; the iOS code is not compiled yet). A device that left or was removed while another one was offline for more than 45 days (nobody rotated in between) no longer causes this: the full resync that follows the absence sees the relay owing a rotation and the device missing from its list, so it handles that device as removed by the relay and the rotation completes.

**Key retention.** After applying a rotation `e → e+1`, the remaining devices keep the old key `e` for `max_ttl + 1 day`
(31 days with the default 30-day maximum) so they can still read items that were in transit under it, then erase it. Items
you have already opened are re-sealed under a local key that is independent of the space key, so they do not depend on the
old key. There is **no periodic rotation**: the key changes only when a device is revoked.

## No forward secrecy, and what rotation does and does not do

- Whoever obtains the space key of an epoch can read every item of that epoch that is still available, including old
  ciphertext kept by the relay or its backups. Rotation protects the **future**, not the past. The same holds for a history
  transfer: its ciphertext stays on the relay for up to seven days, and whoever later obtains the receiving device's key and
  a copy of those blobs can read it.
- There is no automatic recovery after a compromise: a compromised device is a reader until you revoke it.
- Items are not signed per sender. They are encrypted under the shared space key, so one of your own devices can produce an
  item that appears to come from another (`source_device` is only an indication). Acceptable for one person; it is the reason
  a compromised member device is out of scope.
- There is no recovery if **all** your devices are lost (no master password, no recovery kit yet). The space is gone and you
  create a new one.

## Compromised devices and malware as the same user

Out of scope, and the design says so (`format-v1.md` §12): "member device compromised and active". Malware that runs as you,
or an attacker who has root or can inject code into the app, can read what the app can read. Specifically:

- Secrets are decrypted in the app's memory (the feed state holds the decrypted items on Android; on Windows the bridge
  between the core and the interface keeps only a keyed fingerprint per item between two passes since the audit, but the core
  still decrypts the whole feed at each pass and the unsealed database key stays in memory) and are only hidden in the
  interface until you authenticate. A hook on the authentication call, or a memory dump, yields them. A real fix needs the
  sealed-secrets design below.
- On Windows the database key is sealed with DPAPI for your user account. Any process of the same user can unseal it, so the
  "entropy" label in the source is not a secret. Windows Hello only gates the interface.
- Windows toasts and Android notifications for non-secret items carry a preview. Notification listeners, the notification
  history and wearables can see it. Secret and unreadable items get a generic notification with no content, and so does an
  "after first read" item that is not hidden (the first copy, open or share deletes it everywhere: a preview or a button
  would consume it unseen; applied on Android and Windows, see `docs/FEATURES.md` §6 for iOS). On Android the preview of
  a non-secret item is richer (expanded text of 400 characters, a 450 px picture decoded in memory from the thumbnail, the
  host of a link without its path or query); a concealed item is also kept on the phone (`setLocalOnly`, not bridged to a
  watch). Where the lock screen shows previews is the user's system setting: `VISIBILITY_PRIVATE` only hides them when
  the phone is set to hide sensitive content.

Tomi does take measures that help against casual or partial access: the clipboard is marked sensitive and cleared after
30 seconds if it still holds the value (on Windows, a secret left on the clipboard by a crash is emptied at the next start,
only if the clipboard still carries Tomi's secret marker; its content is never read); secret screens use `FLAG_SECURE` on
Android, content protection on Windows (`WDA_EXCLUDEFROMCAPTURE`, read in the source of the windowing library, not tried with
a capture tool) and a capture shield on iOS (to verify on an iPhone); no automatic clipboard reads; secrets never in logs,
URLs or push messages (push messages have constant content). On Android, autofill is excluded from windows that show a
secret and, on Android 14 and later, the window is marked so that only accessibility tools such as TalkBack can read it, and
overlay windows are hidden on the pairing, invitation and creation screens; these Android measures are not verified on a
device.

## Hidden secrets: the trade-offs, stated plainly

A *hidden* item is a secret in the interface: its title is encrypted and not searchable, it is masked, and showing, copying
or sharing it needs local authentication. The following are **accepted and documented**, not bugs:

1. **Attachments of a secret are decrypted to a cache to be opened.** They are written into `files/cache/` under the core's
   private data directory (the app's own directory, outside backups on Android and iOS), because other apps and viewers need
   a file. This is the one accepted clear copy (`AGENTS.md`, golden rule 3). It is wiped when the app goes to the background
   or at the next start:
   - **Android:** the purge runs **60 seconds after the app goes to the background** (a short grace so that "Open with…" and
     sharing can finish), and the core purges the cache again when it opens at the next start. If the system kills the process
     during those 60 seconds, a WorkManager job scheduled at two minutes (`PlaintextPurgeWorker`, cancelled when the app comes
     back) restarts the process and purges anyway; this guarantee is not verified on a device, and until it runs the clear
     copy stays. The cache directory is not exported; files are handed to other apps only through grants given after an
     explicit Open or Share.
   - **Windows:** purged **one minute** after no Tomi window is in use, when the item is removed, on exit, and at the next
     start after a crash. The opened file is a read-only hard link to the cache entry (one clear copy, not two).
   - **iOS:** purged when the app goes to the background and at the next start; a temporary link is used for viewing.
2. **Secrets in memory.** A decrypted secret is a normal string in the app's memory while it is shown, and, as above, in the
   decrypted feed state on some platforms. Strings cannot be reliably zeroized in Kotlin, Swift or JavaScript.
3. **No other clear copy, with one exception on iOS.** Not in SQLite, in the write-ahead log or in other files; not in logs,
   notifications or an unprotected clipboard. System sharing of a secret needs biometrics first, and the share sheet excludes
   copy, save and print actions where the platform allows it. The exception is the **iOS share extension**: it cannot open the
   core, so what you share to Tomi waits **unencrypted by Tomi** in the app group's inbox, secrets included, until the app is
   next opened and sends it. The inbox is excluded from backups (iCloud, Finder) and a share that nothing sent within 7 days
   is dropped (you are told once); the drop runs when the app, or a background task, next looks at the inbox. Until then the
   copy sits under whatever file protection the system gives that directory, with no Tomi key.
4. **Local deletion is stronger than before, and still not erasure.** The local database runs with `secure_delete` on for the
   life of the connection, and the write-ahead log is truncated (best effort: a checkpoint blocked by a reader is retried later) right after a deletion and
   periodically during maintenance, so a deleted or consumed record does not stay readable in a free page or in the log. This does not reach
   copies the file system or flash storage may keep, or a backup of the database file. The database is sealed, but its key is
   on the same device.
5. **The relay does not know which items are secret**, and does not know the "after reading" policy either.

## Biometrics: a gate in the app, not a key

On every platform today, "biometrics" or "Windows Hello" is **a check performed by the application**, not something the
encryption keys depend on:

| Platform | Database key held by | Is the key bound to user authentication? | What the biometric prompt does |
|---|---|---|---|
| Android | an AES key in Android Keystore, no user-authentication requirement | **No** (on purpose: the core must open in the background) | `BiometricPrompt` returns success, and the app then reveals the item. No cryptographic object is tied to it. |
| iOS | the Keychain, accessible after first unlock, this device only; data directory excluded from backup | **No** | `LAContext` returns a boolean. |
| Windows | a key sealed by DPAPI for the user | **No** | A Windows Hello (`UserConsentVerifier`) check in the Rust side of the app, with an account-password fallback that is weaker than Hello. |

So the prompt stops a person who has the unlocked device and the app, which is valuable. It does **not** stop someone who
extracts the key (forensics on a phone in the "after first unlock" state, same-user malware on Windows, root on Android)
or hooks the check. The specification already records the gap (`api-v1.md` §7.1 asks for user presence at each use of the
key; the clients do not provide it yet). Enrolling or removing a fingerprint has no effect on any key, because none is bound.

## Pairing

Pairing is the one moment trust is created outside the certificate chain, so it is specified and tested closely
(`spec/pairing-v1.md`). The inviter shows a code `<channel>-<word>-<word>`; the new device types or scans it.

- **CPace** (a password-authenticated key exchange on ristretto255) derives session keys from the two words. The words carry
  22 bits of secret. That is low for a password, which is why the protocol allows **one attempt per channel**: a wrong guess
  fails the session and closes the channel, so a guess succeeds with probability about 2⁻²² and there is no offline attack
  against a transcript, even for the relay.
- A **6-digit verification code** is derived from the session and shown on both devices; you confirm on **each** that they
  match. It protects against a code someone saw (a photo of the QR, screen sharing) combined with an attacker who is also the
  relay: with a commitment made before the code is revealed, the chance that the two codes coincide for an attacker is about
  10⁻⁶.
- The inviter sends the space key only after you confirm on both devices, and the new device is registered on the relay only
  after it verified and accepted the batch.
- A link or QR code can only *propose* a relay. A page that opens a `tomi-share://` link cannot start pairing or send
  anything: it can only pre-fill fields. The relay named in a link is displayed in full and kept only if pairing succeeds,
  but **if you are tricked into joining an attacker's space you will send your items to the attacker**. Read the relay address
  on the verification screen, and only join a space you started or one you recognize.
- Out of scope: a compromised inviter or new device, and a user who confirms a verification code that differs.
- Pairing hands over the whole history, secrets and files included (one yes/no choice, no checkboxes). That is why adding a
  device is a sensitive action.

## Expiry and deletion

Expiry and deletion are **best effort** (`format-v1.md` §12). Exported or copied files, backups, offline devices and copies
transferred to another device are not reached. The default is **no expiry**, secrets included; options are After reading
(hidden items only), 1 hour, 1 day, 7 days, 30 days. The relay purges what it holds at the latest after its maximum retention
(30 days by default).

## Malicious content

A compromised paired device is the only way to deliver hostile content to you, since a relay cannot forge items. Received
HTML and Markdown are sanitized and rendered without JavaScript and without network access, so there is no tracking pixel.
On iOS the HTML web view also carries a content rule list that blocks http(s), ws(s) and ftp loads (the sanitizer itself is
hand-written; the page's JavaScript is disabled and a CSP applies). Links open only as http, https, mailto or tel, on a user
gesture; tapping a notification for a link on iOS opens it only if it is http(s). A `tomi-share://send` link from outside never
replaces a draft on Android (the link is appended and you are told), and a pairing link on Android shows the relay it names in large type.
Files from other devices are not run on open (on Windows, a deny list of executable and script-carrying types, extended after the
audit, is revealed in Explorer rather than launched, and files carry the Mark-of-the-Web; a deny list can still miss a type). A
decryption error never deletes an item: it is shown as unreadable.

## Distribution and updates

- **Android:** CI publishes only non-debuggable `release` APKs signed with a dedicated release key, checks that again in the
  publish job (`verify-android-apk.sh` fails on a debuggable, unsigned or debug-signed APK, or one signed with another
  certificate than the pinned one), and publishes a checksum and a build provenance attestation with it. Without the signing
  secrets the build is a labelled, unsigned or debug-only workflow artifact and is never published. The release key and its
  fingerprint are not configured yet: until then no APK is published by CI.
- **Windows:** the installer has a checksum and a build provenance attestation, but is **not code-signed** (SmartScreen warns).
  There is no auto-update.
- **Relay binaries:** a checksum only; no attestation.
- **iOS:** built from source; TestFlight or App Store distribution is planned.
- How to verify a download: [`RELEASES.md`](RELEASES.md#verifying-a-download). Dependencies are pinned by lockfiles and
  audited in CI. Anyone can fork and distribute their own build; forks must use their own identifiers and keys
  ([`FORKING.md`](FORKING.md)).

## Fixed since the review

A security review of the specification, the core, the relay and the three clients found the points below, which are now
fixed in the merged code (see also [`CHANGELOG.md`](../CHANGELOG.md) and the rows of [`FEATURES.md`](FEATURES.md)). Fixed
means the code does it and a test covers the decision; it does not mean it was tried on every device.

- **Core:**
  - The epoch-ordering rule no longer trusts a relay-chosen position: the cursor is monotonic, and a log rewind freezes
    superseded epochs (`api-v1.md` §7.3, §7.5).
  - A sync page that makes no progress ends the loop, and a malformed event no longer blocks the cursor (`api-v1.md` §7.2).
  - SQLite `secure_delete` stays on and the write-ahead log is truncated after deletions (`format-v1.md` §7.2).
  - Tombstones are kept `max_ttl + 60 days` (90 days by default) and an item older than that when it first arrives is refused
    as suspect (`format-v1.md` §9.4, age rule).
  - A device erases its space keys on a verified signed rotation that revokes it (`format-v1.md` §8.3 step 6).
  - The core persists a `rotation_stalled` flag when the automatic rotation keeps failing, and records revocations that only
    the relay announced (`rotation_stalled()`, `take_relay_revocations()`). Only an explicit `device_revoked` event, or a device
    missing from a resync list while the relay says a rotation is owed, starts a rotation.
- **Relay:** every `X-Forwarded-For` line is read and a malformed entry is distrusted (`api-v1.md` §8.5).
- **Android:** a fresh authentication before adding a device; the screen lock (PIN) accepted for revocation, space deletion and
  "erase anyway"; the authentication window closed when the app goes to the background; the clear-text attachment cache purge
  backed by a WorkManager job; an alert when a device joins; autofill, accessibility and overlay hardening (not verified on a
  device); a deep link no longer discards a draft; the demo mode refused on a provisioned device.
- **iOS:** the share inbox is excluded from backups and shares older than 7 days are dropped; a fresh Face ID / Touch ID /
  passcode check before a device is added; an alert when a device joins; a notification's link is opened only if it is http(s);
  a content rule list blocks network loads in the HTML web view; the `Tomi Dev` scheme archives with `Release`; the demo mode
  is refused on a provisioned device.
- **Windows:** the Hello window is closed on session lock, disconnect, logoff and sleep; a fresh Hello when the inviting PC
  confirms a new device; a secret left on the clipboard by a crash is emptied at the next start; the list of file types never
  launched is much longer; the bridge no longer keeps decrypted items between passes; a toast when a device joins; the demo
  space is refused by the command itself. None of the Windows items was tried on a PC.
- **Release:** CI publishes only signed non-debuggable Android release APKs, with checksums and build provenance for the apps
  it builds.

## What is still NOT protected

Open at the time of writing. [`FEATURES.md`](FEATURES.md) (section "Open gaps") has the current state.

- **Hidden secrets are a gate, not a key.** Biometrics and Windows Hello are checked by the app; no key is bound to them (see
  "Biometrics: a gate in the app, not a key"). Extracting the database key, hooking the app or same-user malware yields the
  secrets.
- **No optional app lock** with a timeout, and the Recents thumbnail can show the non-secret feed.
- **No per-message forward secrecy.** The space key of an epoch opens every item of that epoch, including relay backups and
  history-transfer blobs. The key changes only when a device is revoked.
- **No hardware-bound keys.** The database key is wrapped by the OS key store but is usable without the user, and is not a
  Secure Enclave, StrongBox or TPM key bound to user presence.
- **iOS data protection is the "after first unlock" class** for the database and the Keychain item, because the app must work
  in the background.
- **Metadata is visible to the relay:** device names (in clear, unsigned), platforms, public keys, activity and presence
  times, exact encrypted file sizes (to within 16 bytes per chunk), the number of files, envelope sizes, delivery and
  read-receipt times, deletion times, and the fact and time of pin or archive changes. There is no padding and no dummy
  traffic.
- **The two notices are best-effort.** The `rotation_stalled` banner and the "removed by the relay" notice are shown by the
  three apps, but the core hands a revocation notice over only once and the apps keep it in memory: if the process dies
  before it is displayed, the notice is lost. Notices are built from unsigned relay events, so a hostile relay can fake one
  (it can already block a device; the core ignores an unknown device or an unknown cause, so it cannot invent a notice about a device you never had), and the "new device added" alerts do not check that the device is verified.
- **iOS:** revocation needs Face ID or Touch ID (the passcode is not accepted, unlike Android); the share inbox holds clear
  text until the app next runs; HTML sanitization is hand-written.
- **Android:** the app exposes a share target that any app can call; it sends immediately and offers a 3-second undo.
- **Windows:** the Hello fallback to the account password is weaker than Hello; the clear-text attachment cache is kept one
  minute after last use; the file types never launched on open are a deny list; the new-device toast misses a device that
  joined while Tomi was closed.
- **Transport:** the relay's login signature is not bound to the relay's address (exploiting that needs a TLS
  man-in-the-middle).
- **No recovery** if every device is lost.

## Planned or under consideration

Not commitments. In rough order of value for cost:

1. **Sealed secrets** (device-key-bound secret storage). Each device holds a key pair for hidden secrets; incoming secrets are
   re-sealed to its public key (which needs no authentication, so receiving in the background keeps working) and opening one
   needs a private key that requires user presence: a Secure Enclave key with user presence on iOS, a Keystore key with a
   user-authentication timeout on Android, a Windows Hello–bound key on Windows. Biometrics or Hello would then gate the key
   rather than the interface, a real cryptographic barrier against bulk extraction. It is a core and specification change,
   and the only real fix for the decrypted feed the Windows core still builds at each pass.
2. **Optional app lock** with a timeout, and hiding the app in the recents list.
3. **iOS "protection renforcée"** (stronger protection): an optional mode with a stricter data-protection class, trading
   background receipt for keys that are unavailable while the phone is locked. Not designed in the repository yet.
4. **Hardware-bound keys** where the platform offers them (StrongBox, Secure Enclave, TPM), together with sealed secrets.
5. **Making the notices reliable** (acknowledge only after display, announce verified devices only), and a **safety
   number** for the member list, comparable between devices, to detect a rogue member.
6. **Periodic automatic key rotation** (for example monthly), giving a bounded "forward secrecy lite": a space key stolen later
   would not open relay backups older than about two months. This is not per-message forward secrecy, which Tomi does not
   provide.
7. **An optional printable recovery key** for the "all devices lost" case (`format-v1.md` §13).
8. **Windows code signing** (SignPath Foundation or an alternative), a verified update channel ([`RELEASES.md`](RELEASES.md)),
   the Android release key, and an attestation for the relay binaries.
9. **Less metadata for the relay** (for example signed or hidden device names, padding of file sizes). This changes the
   protocol and the operator's ability to choose which device to revoke.
10. Longer term, multi-person sharing as one space per person plus groups
    ([`multi-person-evolution.md`](multi-person-evolution.md)): a different threat model, to be written when it exists.

## Out of scope

A device that is already compromised or rooted; physical access combined with the device PIN; social engineering; traffic
analysis beyond what is listed above; a misconfigured self-hosted relay (open ports, no TLS, weak host hardening) and volumetric
denial of service; metadata the relay needs by design. Reporting a vulnerability: [`SECURITY.md`](../SECURITY.md).
