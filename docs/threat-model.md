# Tomi threat model

What Tomi protects against, and what it does **not**. This page is written for people who want to decide whether to trust
Tomi with their data, and for reviewers. It is based on the specification (`spec/`), the code and `AGENTS.md`, and it was
checked against the code as of 2026-11-19. When this page and the code disagree, that is a bug in one of them: please
report it (see [`SECURITY.md`](../SECURITY.md)).

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
| Lost phone or PC, **unlocked** (or someone with a few minutes at it) | **Partly** | Non-secret items are readable. Hidden secrets are behind a local authentication that is a gate in the app, not a cryptographic barrier. |
| Device you revoked | The **future** only | It cannot reach the relay or read items sent after the rotation. It keeps what it already had. |
| Malware running as you on a device | **No** | Out of scope. It can read what the app can read. |
| Someone who steals a copy of the space key (or a device with it) later | **No retroactively** | There is no forward secrecy: old copies of items at that epoch are readable. |
| A malicious item (HTML, Markdown, link, file) | Yes, by sandboxing | Rendering has no script and no network access. Nothing executes without a user action. |
| Tampered download of the app | Partly | Checksums and build provenance exist; Windows installers are not code-signed yet. See [`RELEASES.md`](RELEASES.md). |

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
  that withholds a rotation keeps a revoked device effective for emitters that are not up to date.
- **Replay.** Writes are idempotent and applying an event twice changes nothing, so replay cannot alter or duplicate an item.
  Two limits: a client remembers a deletion (a tombstone) for **45 days**, and after that a relay that kept the encrypted
  envelope could serve it again, and an item you deleted or that was set to disappear after reading could reappear on a device;
  and the relay chooses the order in which it presents events.
- **Invent an unsigned revocation** ("by the administrator", "voluntary departure"). Clients accept relay-reported revocations
  and rotate the key automatically, so a malicious relay can make honest devices *permanently* drop a legitimate device,
  which then has to be paired again. This costs you availability, not confidentiality.
- **Rename devices** (names are not signed).
- Together with a **revoked device** that still has the old key: inject items sealed with the old epoch key. Mitigation: the
  clients write only with the latest epoch, refuse and flag as "unreadable (suspect)" an old-epoch item whose log position
  is after the rotation they applied, and discard old keys after the retention period below. That rule uses the position
  that the relay itself assigns, so a window remains for as long as old keys are kept (up to 31 days by default): a
  hardening of this check is known work (see "Known gaps").

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
its relay sessions at once and rotates the space key. Revoking from Android and iOS requires strong biometrics (Face ID or
Touch ID on iOS; Class 3 biometrics on Android), and on Windows Windows Hello or the account password. A phone that has no
such biometrics cannot confirm the action on Android; use another device or the relay's admin command.

**Unlocked, or in someone's hands for a few minutes.** Whoever can use the unlocked app can read every non-secret item, send
as you, and reach the Devices screen. Hidden secrets stay masked until you authenticate with biometrics (Windows Hello on
Windows). Sharing a secret through the system, and copying it, need that authentication. The authentication window lasts
two minutes. It is closed when the screen turns off on Android, in the background on iOS, and it is **not** closed on lock or
sleep on Windows. There is no app lock yet, and the Recents (app switcher) thumbnail can show the non-secret feed.

A thief with an unlocked device can also try to revoke your *other* devices before you revoke theirs; the first valid rotation
wins. Revocation requires the strong authentication above, which is the barrier. The recourse for you is `tomi-relay admin`
on the relay, and pairing again.

Adding a device is the other sensitive action on a borrowed unlocked device: the history transfer copies **everything**,
secrets and files included. Whether and how each platform asks for authentication before pairing is tracked in
[`FEATURES.md`](FEATURES.md); treat it as a known gap until that file says otherwise.

## Revoked devices

When you remove a device, a remaining device creates a new space key (a new *epoch*), seals it for each remaining member, and
signs the rotation together with the list of revoked devices. The relay refuses a rotation that does not move the epoch by
exactly one or whose recipients are not exactly the remaining members, and cuts the revoked device's sessions and
WebSockets immediately. Other devices verify the signature and the recipients before switching.

What the revoked device **keeps**:

- Every space key it already had, up to the epoch of the rotation, and everything it already stored. As things stand in the
  code, a device that learns that it was revoked marks itself as blocked, but it does not erase its keys or its local data
  (the specification says it must erase the keys; this is a known gap). Treat a revoked device as one that has read
  everything up to the rotation.
- Any old encrypted copy of items it obtained (for example from a relay backup) at those epochs.

What it **loses**: access to the relay, and the ability to read anything sealed with a later epoch. A revoked key never
becomes a member again; a reinstalled device gets a new identity.

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

- Secrets are decrypted in the app's memory (the feed state holds the decrypted items on Android and Windows) and are only
  hidden in the interface until you authenticate. A hook on the authentication call, or a memory dump, yields them.
- On Windows the database key is sealed with DPAPI for your user account. Any process of the same user can unseal it, so the
  "entropy" label in the source is not a secret. Windows Hello only gates the interface.
- Windows toasts and Android notifications for non-secret items carry a preview. Notification listeners, the notification
  history and wearables can see it. Secret and unreadable items get a generic notification with no content.

Tomi does take measures that help against casual or partial access: the clipboard is marked sensitive and cleared after
30 seconds if it still holds the value; secret screens use `FLAG_SECURE` on Android, content protection on Windows and a
capture shield on iOS; no automatic clipboard reads; secrets never in logs, URLs or push messages (push messages have
constant content).

## Hidden secrets: the trade-offs, stated plainly

A *hidden* item is a secret in the interface: its title is encrypted and not searchable, it is masked, and showing, copying
or sharing it needs local authentication. The following are **accepted and documented**, not bugs:

1. **Attachments of a secret are decrypted to a cache to be opened.** They are written into `files/cache/` under the core's
   private data directory (the app's own directory, outside backups on Android and iOS), because other apps and viewers need
   a file. This is the one accepted clear copy (`AGENTS.md`, golden rule 3). It is wiped when the app goes to the background
   or at the next start:
   - **Android:** the purge runs **60 seconds after the app goes to the background** (a short grace so that "Open with…" and
     sharing can finish), and the core purges the cache again when it opens at the next start. If the system kills the process
     during those 60 seconds, the clear copy stays until the next start, which can be much later. The cache directory is not
     exported; files are handed to other apps only through grants given after an explicit Open or Share.
   - **Windows:** purged **one minute** after no Tomi window is in use, when the item is removed, on exit, and at the next
     start after a crash. The opened file is a read-only hard link to the cache entry (one clear copy, not two).
   - **iOS:** purged when the app goes to the background and at the next start; a temporary link is used for viewing.
2. **Secrets in memory.** A decrypted secret is a normal string in the app's memory while it is shown, and, as above, in the
   decrypted feed state on some platforms. Strings cannot be reliably zeroized in Kotlin, Swift or JavaScript.
3. **No other clear copy.** Not in SQLite, in the write-ahead log or in other files; not in logs, notifications or an
   unprotected clipboard. System sharing of a secret needs biometrics first, and the share sheet excludes copy, save and print
   actions where the platform allows it.
4. **Local deletion is not erasure.** Deleted or consumed items may remain in free pages of the local database until they are
   overwritten. The database is sealed, but the key is on the same device.
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
Links open only as http, https, mailto or tel, on a user gesture. Files from other devices are not run on open (on Windows,
executable types are revealed in Explorer rather than launched, and files carry the Mark-of-the-Web). A decryption error never
deletes an item: it is shown as unreadable.

## Distribution and updates

- **Android:** CI publishes only non-debuggable APKs signed with a dedicated release key, and fails if an APK is debuggable or
  debug-signed. Checksums and build provenance are published. The release key and its fingerprint are not configured yet:
  until then no APK is published by CI.
- **Windows:** the installer has a checksum and a build provenance attestation, but is **not code-signed** (SmartScreen warns).
  There is no auto-update.
- **iOS:** built from source; TestFlight or App Store distribution is planned.
- How to verify a download: [`RELEASES.md`](RELEASES.md#verifying-a-download). Dependencies are pinned by lockfiles and
  audited in CI. Anyone can fork and distribute their own build; forks must use their own identifiers and keys
  ([`FORKING.md`](FORKING.md)).

## Known gaps

Found by a security review of the specification, the core, the relay and the three clients, and not fixed at the time of
writing. Check [`CHANGELOG.md`](../CHANGELOG.md) and [`FEATURES.md`](FEATURES.md) for the current state; some may already be
fixed when you read this.

- **Core:** the epoch-ordering rule relies on the position number the relay assigns (above). A failure of the automatic
  rotation after a relay-reported revocation is not shown to the user. A revoked device does not erase its space keys. Tombstones
  are pruned after 45 days (resurrection by a malicious relay). The local database does not use secure deletion during normal
  operation. The relay's login signature is not bound to the relay's address (needs a TLS man-in-the-middle). Device names and
  exact file sizes are visible metadata.
- **Android:** adding a device did not require local authentication when reviewed; revocation, space deletion and "erase anyway"
  need Class 3 biometrics, so users without them cannot do them on the phone; the authentication window survives going to the
  background; the plaintext cache purge can be skipped if the process dies in the 60-second grace; no app lock; the app exposes
  a share target any app can call, which sends after three seconds.
- **iOS:** the database class and the Keychain item are readable after first unlock; shares made through the share extension
  wait **in clear** in the app group container, which is not excluded from backups, until the app is next opened; tapping a
  notification for a link does not check the URL scheme; HTML sanitization is hand-written (contained by CSP and disabled JS).
- **Windows:** the Hello window is not closed on lock or sleep; the list of file types never launched on open is incomplete;
  a copied secret stays on the clipboard if the app crashes before the 30-second wipe.
- **All:** no optional app lock, and no notification when a new device is added to your space.

## Planned or under consideration

Not commitments. In rough order of value for cost:

1. **Sealed secrets.** Each device holds a key pair for hidden secrets; incoming secrets are re-sealed to its public key (which
   needs no authentication, so receiving in the background keeps working) and opening one needs a private key that requires
   user presence: a Secure Enclave key with user presence on iOS, a Keystore key with a user-authentication timeout on
   Android, a Windows Hello–bound key on Windows. This turns the biometric gate into a real cryptographic barrier against bulk
   extraction. It is a core and specification change.
2. **Fixing the known gaps** above, starting with the epoch-ordering check, erasing keys on a verified revocation,
   authentication before adding a device, and the backup exclusion of the iOS share inbox.
3. **Optional app lock** with a timeout, and hiding the app in the recents list.
4. **Notify on a new device**, and a **safety number** for the member list, comparable between devices, to detect a rogue member.
5. **Periodic automatic key rotation** (for example monthly), giving a bounded "forward secrecy lite": a space key stolen later
   would not open relay backups older than about two months.
6. **An optional printable recovery key** for the "all devices lost" case (`format-v1.md` §13).
7. **Windows code signing** (SignPath Foundation or an alternative), and a verified update channel
   ([`RELEASES.md`](RELEASES.md)).
8. Longer term, multi-person sharing as one space per person plus groups
   ([`multi-person-evolution.md`](multi-person-evolution.md)): a different threat model, to be written when it exists.

## Out of scope

A device that is already compromised or rooted; physical access combined with the device PIN; social engineering; traffic
analysis beyond what is listed above; a misconfigured self-hosted relay (open ports, no TLS, weak host hardening) and volumetric
denial of service; metadata the relay needs by design. Reporting a vulnerability: [`SECURITY.md`](../SECURITY.md).
