# Design analysis: sending files directly to a contact

Status: **analysis, no code and no spec change**. Nothing here is implemented. **Revised after a security
review: §13 supersedes §3.2, the import part of §3.4, §4 and §6 wherever they conflict.** Basis: `PROJECT.md`,
`spec/format-v1.md` (§5.8, §8.8, §12), `spec/api-v1.md` (§1.2-1.4, §3.1, §4.6, §4.8, §8.4-8.5),
`spec/pairing-v1.md` (§6-7), `docs/protocol-extensibility.md`, `docs/multi-person-evolution.md`,
`docs/FEATURES.md`, and the app sources (`apps/android`, `apps/apple`, `apps/windows`).
The relay and core **code** was not read: every statement about them comes from the specs and must be
checked before implementation (points marked "to verify").

## Summary

- A contact is **another person's space on the same relay**. This is architecture "M2" of
  `docs/protocol-extensibility.md` (one space per person, shared links between them), which the reserved
  `audience` field and the reserved `key_epoch >= 2^31` range were sized for.
- Nothing is written into the recipient's event log. The sender deposits an **offer** in a per-link
  **inbox** that belongs to the recipient's space. The recipient **accepts or refuses** every offer. On
  acceptance the item is re-sealed under the recipient's own space key and becomes an ordinary item.
- Contacts are added **in person or by a live call, never by message**, by reusing the pairing protocol
  (CPace, QR or typed code, 6-digit verification code, double confirmation). The two cases are stored and shown as
  two distinct levels (§13.11). There is no directory, no search and no add-by-handle.
- Format 1 is untouched and the frozen vectors do not change. The relay stays blind to content, but it
  learns the **social graph** (which space deposits to which). This must be disclosed.
- Large piece of work: relay API addition, core (identity, link, inbox import), then three UIs.

## 1. Requirements

1. Send files directly to another person (a "contact"), chosen at send time.
2. Contacts must be managed in a way people can trust.
3. Same relay for sender and recipient (simplifies routing; no federation).
4. **The recipient must explicitly accept every incoming transfer.**
5. Golden rules of `AGENTS.md` still hold: blind relay, secrets are sacred, additive format changes only.

## 2. Starting point

| Fact | Source | Consequence |
|---|---|---|
| One space = one person; no notion of "person" | `format-v1.md` §12 | an identity concept must be added |
| Relay hosts several spaces, one database each; "no request touches two spaces" | `api-v1.md` §1.4 | cross-space delivery is a new relay contract |
| Target scale: fewer than five spaces | `api-v1.md` §8.5 | matches "close contacts on one relay" |
| `audience` payload field and `key_epoch >= 2^31` reserved, ignored by v1 | `format-v1.md` §5.8 | room for recipient and link keys without a `format` 2 |
| No sender authentication between members | `format-v1.md` §12 | not acceptable between two people: must be added |
| Item-state channel ignores unknown types | `format-v1.md` §8.8.2 | can carry contact and link state to a person's own devices |
| Recommended path: one space per person plus shared groups (M2) | `protocol-extensibility.md` §3.2 | this document is the first concrete use of it |
| v1 relay rejects reserved epochs (`stale_key_epoch`) | `api-v1.md` §4.8 | link items cannot go through `POST /v1/items` |

**Correction after review** (`protocol-extensibility.md` §2, row 1 describes this as missing): keeping unknown
payload fields is already implemented (`format-v1.md` §5.4.4, `Item::raw_payload`). For contact offers the need
is the opposite: build the recipient's item from an allowlist (§13.3).

## 3. Architecture

### 3.1 Why an inbox and not the recipient's event log

Writing foreign items into the recipient's log would leave every non-updated device with items in
`KEY_PENDING` forever, and the log is the space's trust boundary. A dedicated **inbox** lives in the
recipient's space database but is a separate resource. v1 devices never see it (they do not announce the
new `capabilities`, `api-v1.md` appendix B.1 no. 29).

### 3.2 Objects

- **Contact identity.** A person-level Ed25519/X25519 key pair, distinct from device keys, shared by all of
  the person's devices through the encrypted item-state channel and the history transfer. It does not derive
  from the space key, so a space-key rotation does not change it.
- **Link.** Created when two people add each other. It holds a symmetric **link key** (identifier in the
  reserved `key_epoch` range), a per-link pseudonymous sending key, and the peer's identity public key.
  Both people's devices receive it. Sealing to a device reuses the existing X25519 scheme (`format-v1.md`
  §7.3) with a new label.
- **Offer.** One transfer: an envelope (format 1, encrypted under the link key) plus its blobs. The envelope
  carries sender, file names, sizes and types, so the relay sees only sizes. The sender signs the envelope
  hash with its identity key; the signature travels beside the envelope, not inside it, so the payload
  format is unchanged.

### 3.3 Offer lifecycle (enforced by the relay)

```
pending ──accept──► accepted      (recipient imported it into its own space)
   │ ├──refuse───► refused        (blobs purged at once)
   │ ├──withdraw─► withdrawn      (sender, only while pending)
   │ └──timeout──► expired        (default 7 days, blobs purged)
```

The relay enforces the transitions (it sees states, never content). `accepted` is terminal: after it,
withdrawal is refused and the sender is told "already received".

### 3.4 Sending and importing

1. Sender encrypts blobs and envelope, uploads both to the recipient's link inbox (authenticated by the
   link's pseudonymous key, no session in the recipient's space), and sees "waiting for answer".
2. Recipient devices are notified (doorbell). Any device may answer; the first answer wins and the others
   dismiss the request.
3. **Accept**: the device downloads, verifies the sender signature, re-seals the payload under the space key
   (new additive provenance field, see prerequisite) and asks the relay to **adopt** the inbox blobs into the
   new item, with no second upload. From then on pin, archive, expiry, search and history transfer apply.
4. **Refuse**: the relay purges the offer. The sender sees "not accepted" (neutral wording).

Upload happens **before** the answer (decided default): the sender may close the app, which matters on
mobile. Cost: a refused file briefly uses the recipient's quota and is stored encrypted without consent.
The alternative (offer first, upload after acceptance) requires the sender's device to stay reachable.

### 3.5 Relay changes (to specify in `api-v1.md`, to verify against `apps/relay`)

- Link registration: the recipient registers the allowed sender key per link; removal revokes at once and
  purges pending offers.
- Sender routes scoped to a link: deposit offer and blobs, read status, withdraw.
- Recipient routes: list inbox, accept (adopt blobs into an item), refuse.
- Limits: per-link quota and cap of pending offers, plus a **total cap across all links** (proposed: 5 offers and 100 MB per contact, 500 MB overall); new `rl_*` buckets keyed by link. The recipient's space
  bears the storage (`api-v1.md` §8.4), which is also the anti-abuse lever.
- Rate and quota keys must not let one person starve another (`api-v1.md` §8.5, shared limits).

## 4. Contacts people can trust

1. **Add in person, or by a live call (§13.11).** Reuse the pairing channel (`POST /v1/pairing/channels`, CPace, SAS) with a new
   purpose "contact". Identity keys travel in sealed frames using new TLV tags
   (`pairing-v1.md` §6.2: tags `0x01`-`0x7F` unknown are ignored, so this is additive). To verify: a joiner
   that belongs to another space claims a channel without a session, which the spec suggests is allowed.
2. **No discovery.** No directory, no search, no invitation by handle: nobody can message a stranger, and
   nobody can enumerate people on the relay.
3. **Safety code.** Derived from both identity keys, shown on the contact sheet, comparable in person or by
   phone. Changing it is never silent.
4. **Key change.** Revoking one of a person's devices rotates the contact identity (the revoked device held
   it). Contacts then see "Alex's safety code changed" and must re-verify. No automatic succession: a
   revocation implies suspicion.
5. **Names are local.** The contact name is chosen by the user (petname). The name the other person
   proposes is shown as "presents themselves as".
6. **Unilateral removal.** Removing or blocking a contact cuts its link at the relay and purges pending
   offers. The other side only observes "not accepted" or a delivery error.
7. **No secrets (default, to confirm).** Hidden items are not sent to contacts in the first version
   (golden rule 3 requires biometric confirmation and an explicit warning for any sharing of a secret).

## 5. Consent: the recipient decides

- Every incoming offer is a **request**. There is **no auto-accept** in this version.
- The request shows only: sender (contact name, plus the safety-code warning if it changed), number of
  files, names and sizes. **No preview and no thumbnail** before acceptance, and no parsing of the content:
  untrusted images are not decoded for someone who has not agreed.
- File names can deceive (`invoice.pdf.exe`): warn on executable types, especially on Windows.
- Actions: **Accept / Refuse / Block** (block = remove the contact).
- Anti-spam: maximum pending offers per contact; expiry of unanswered offers (7 days, relay limit is 30).
- Requests are not part of the feed. They appear in a "Requests" area with a counter; an item enters the
  feed only after acceptance.

## 6. Deletion and expiry semantics

| Action | Effect |
|---|---|
| Sender "delete everywhere" on a sent item | Today's tombstone, signed in the **sender's own space**: erases the sender's devices only. **It cannot affect the recipient**: the sender is not a member of the recipient's space and cannot sign for it. |
| Sender "withdraw" | Possible only while the offer is `pending`. The relay purges it. After acceptance the answer is "already received". |
| Recipient "delete everywhere" | Erases on the recipient's devices only. The sender is unaffected. |
| Expiry chosen at send time | **Revised in §13.8:** only a suggestion shown to the recipient at acceptance, applied if they opt in. It never erases the recipient's copy by itself. |
| "After reading" | Only meaningful after acceptance, in the recipient's space. |

Consequences for the interface: rename the scope "everywhere" to "on all my devices" on sent and received
items, and add the sentence "Alex keeps their copy" to the confirmation. A signed "please delete" request is
possible later but must never run automatically (it would let a contact erase a document the recipient
needs); at most it shows a prompt.

## 7. Privacy and threat model

| Adversary | Sees / can do | Mitigation |
|---|---|---|
| Relay or its operator | Content: nothing. Metadata: sizes, times, **which link deposits to which space**, offer states | Disclose it (`multi-person-shared-relay.md` §2). Per-link pseudonymous keys keep device identities out of the deposit |
| Unknown person on the relay | Nothing: no directory, no way to reach a space without a link | In-person link creation |
| A contact | Whatever they were sent and accepted. Can spam offers | Consent, per-link quota, pending cap, block |
| A contact's stolen device | Can deposit as that contact until the link is cut | Identity rotation on revocation, safety-code warning |
| Network | TLS metadata only | Unchanged |

No forward secrecy, as in v1 (`format-v1.md` §12): whoever gets a link key reads the link's offers still
stored.

## 8. Format, spec and compatibility

- Envelope and payload stay format 1. Frozen vectors in `spec/vectors/v1/` and `pairing-v1/` do not change.
  New statements (contact identity, link, signatures) get **new** vector files, additive.
- `audience` (`{"type":"contact","id":...}`) is used on the sender's own record only for display
  ("shared with Alex"); it is never a confidentiality boundary. It needs a spec meaning first.
- The provenance field on an accepted item must be registered in `format-v1.md` §5.4 before use.
- Deploy the relay **before** the apps, as for attachment removal (`docs/FEATURES.md` §8).
- A `capabilities` entry announces support; older devices ignore the feature and keep working.

## 9. Interface impact

### 9.1 Principle: extend the existing send screens, do not replace them

The send screens already exist on every platform and keep their structure: optional title, text or link,
attachments, the detected type with the line "Vers <devices>" (`detect_to_devices`, `fr.composer.to`), the
"Masquer le contenu" switch, the expiry chips or select, the floating bar on mobile (Coller, Joindre, Envoyer),
the "Envoyé · Annuler · Options" banner, and the share entry points (Android `ShareActivity` with
`SendOptionsSheet`, iOS `TomiShare`, Windows "Envoyer vers"). The feature adds **one section, "Destinataire"**,
built from the same widgets as the expiry section (a label plus chips on mobile, a select-like control on
Windows). Consequences:

- **No contact, no change.** With zero contacts the section is hidden and every screen is exactly today's.
- **The default is always "Mes appareils"**, and it is the only value the quick paths use (share-sheet direct
  send, Quick Settings tile, shortcuts, "Envoyer vers"). Nothing is ever pre-selected from memory.
- The existing line "Vers PC bureau, Pixel 9" becomes "Vers Léa, après son accord".
- "Masquer le contenu" is disabled for a contact (the existing `secretLocked` mechanism); "Après lecture" is
  not offered for a contact (already the rule for files); the expiry caption says it is a request.
- The "Envoyé · Annuler" window is kept. Afterwards, the delivery status is the existing card footer extended
  with "En attente de réponse", "Accepté", "Non accepté".
- New screens only where there is nothing to extend: contact list, add a contact, contact sheet, requests,
  safety-code-changed alert, plus the recipient picker for many contacts.
- Vocabulary: "Ajouter un appareil" and "Ajouter un contact" must not be confusable.

### 9.2 Many contacts

The chip row is fine up to about four contacts, so it never lists everyone:

- The row shows **Mes appareils, the 3 most recently used contacts, and a "Tous · N" chip** that opens a
  searchable picker: on Android the existing full-height `ModalBottomSheet`, on iOS a sheet with search and a
  section index, on Windows a type-ahead listbox driven by the keyboard.
- The picker orders **Récents, then A to Z** with section letters and an index rail. Choosing a row closes it,
  with no confirm button.
- **Favoris** (starred contacts, a handful) come first in the picker and in the Mac sidebar.
- The contact list has search, filters (Tous, Favoris, À revérifier), sections and the index rail.
- Requests are listed most recent first, the first expanded and the others collapsed. There is **no
  "accept all"**: acceptance is per transfer, by rule.
- System share: Android publishes at most 4 direct-share shortcuts (recent contacts, ranked by use, through
  `SharingShortcut`); iOS can use share suggestions the same way. Everything else goes through "Tomi", then
  "Options". On Windows, "Envoyer vers" only gets shortcuts for favourites.
- Limits (proposed): pending offers are capped **per contact (5 offers, 100 MB) and in total (500 MB)**, so
  many contacts cannot fill the recipient's quota.
- The relay sizing note "fewer than five spaces" (`api-v1.md` §8.5) must be revisited if a relay hosts many
  people: the shared per-IP limits, the global quota lock and `rl_global_claim` all assume a handful of spaces.

### 9.3 Per platform

| Platform | Impact |
|---|---|
| **Android** | `SendScreen`: new "Destinataire" section between the detection row and the secret switch, same `FilterChip` style as the expiry chips; "Tous · N" opens a searchable sheet. `ContactsScreen` reached from the feed's floating toolbar next to the existing Devices button, with the feed's search field and chips. Requests screen. `SendOptionsSheet` gets the same section; up to 4 direct-share shortcuts. Actions in the arrival notification; contact QR masked from capture like the pairing QR. |
| **iPhone** | `SendScreen` keeps its form, floating glass bar and `ExpiryChips`-style chips; same "Destinataire" section, and "Tous · N" opens a searchable sheet with a section index. `TomiShare` cannot open the core, so a **names-only contact cache** goes into the App Group (no keys); its `ShareSettings` gets the section, and its default send stays "Mes appareils". Notification actions. Strings in `Localizable.xcstrings`. |
| **iPad** | Already built for iPhone and iPad (`TARGETED_DEVICE_FAMILY = 1,2`) but there is **no iPad-specific layout**: only the 680 pt readable column. Contacts are the first feature that justifies a sidebar split view (list with search, filters and index rail in the middle column, contact sheet on the right), drag and drop of files onto any contact, and popover sheets. |
| **Mac** | **No macOS target exists yet** (`apps/apple/README.md`: "macOS later"). Design for it now: the sidebar lists **favourites only** plus "Tous les contacts · N" (a long list would swamp it), Cmd+K to search, drag onto a favourite or hover on "Tous les contacts" so the list opens, a confirmation sheet, menu bar item, share extension, Services, and a contact exchange that does not need the camera. |
| **Windows** | `Composer.svelte`: the footer line "Vers {devices}" is replaced by a "Pour" control next to "Expiration", opening a type-ahead listbox (search field, Récents, A to Z, arrows and Enter) that fits the 380 px popover. A "Contacts" tab joins Historique, Réglages and Appareils in `HistoryView`, with search, filters and index rail; pending requests surface through a `StatusBanner`-style banner. No camera scan, so contact exchange by typed code or pasted link, like pairing. "Envoyer vers" cannot list everyone: favourites only, otherwise it opens the popover with the files attached. Toast with Accepter / Refuser. |

Mock-ups are kept out of this repository (`docs/FEATURES.md` §3), in the private archive. They cover every
platform above (Android, iPhone, share sheets, iPad, Mac, Windows) and were drawn from the existing send screens
and the charter tokens (`design/tomi/tokens.json`). They are a **design reference, not a spec**.
`docs/FEATURES.md` must get a new section in the same commit as the first behaviour change. UI strings are
French in `strings.xml`, `Localizable.xcstrings` and `fr.ts`.

## 10. Decisions

Closed:

- Recipient consent is mandatory for every transfer.
- Deletion by the sender never reaches the recipient's copy; withdrawal only while pending.
- Contacts are added in person or by a live call, as two labelled levels (§13.11); no discovery.
- Upload happens before the answer.

Proposed defaults, **to confirm**:

1. No secrets sent to contacts in the first version.
2. The sender keeps a metadata-only record (name, size, status), not the file bytes.
3. One shared contact identity across a person's devices (simpler), not a key per device.
4. One recipient per send.
5. A refusal is shown to the sender as "not accepted". Whether the recipient may ignore silently is open.
6. Unanswered offers expire after 7 days.

## 11. Risks and gaps

- **No push on Android and iOS today** (`FEATURES.md` §6): a request is seen only when the app is open or
  syncing. Hence the generous expiry; push becomes much more useful.
- **Many contacts.** Every contact adds a link key distributed to all of a person's devices, a pending-offer
  budget and a row in several lists. The UI scales through recents, favourites and search (§9.2); the relay
  sizing, written for fewer than five spaces, has not been checked for more.
- The relay learns the social graph. Operators who host other people must say so.
- A refused upload briefly consumes the recipient's quota; the disk budget in
  `multi-person-shared-relay.md` §4 must be re-checked.
- Identity rotation on revocation will interrupt every contact; frequency should be low.
- End-to-end behaviour between two real clients is still unproven even without this feature
  (`FEATURES.md`, "Not validated on a device").

## 12. Suggested order

1. Spec: contact identity, link, offer lifecycle, inbox routes, new vectors; keep-unknown-fields in the core.
2. Relay: link registration, inbox, adoption, limits, tests; deploy first.
3. Core: identity and link storage, contact exchange over the pairing channel, offer send/import, FFI.
4. Apps: contacts, requests, recipient chooser, statuses, notifications, `FEATURES.md`.

---

## 13. Revised model after security review

Status: **supersedes §3.2, the import part of §3.4, §4 (identity and key change) and §6 (expiry) wherever they
conflict.** An independent review (26 findings, 17 checked against `crates/core` and `apps/relay`) found holes in
the first model. Decisions taken: **per-device keys certified by the person (no shared secret across devices)**
and **files re-encrypted on acceptance**. The goal is no known compromise; §13.9 lists what is inherent and cannot
be removed.

### 13.1 Identity: per-device keys, no person-level secret

- There is no identity secret key shared by a person's devices. Each device keeps its own signing and key-agreement
  keys (the existing `sign_pk` and `kx_pk`).
- **Person root.** At contact exchange, each side pins the other's founding device certificate and `space_id`.
  The safety code is derived from the pinned roots.
- **Device set.** A signed, versioned statement (new label, `space_id`-bound, counter `n` strictly increasing)
  lists the person's active devices. It is verified as a chain from the pinned root, like the trust store
  (`format-v1.md` §8.4). The relay stores only the latest opaque statement per link.
- **Fork rule, fail closed.** Two valid statements with the same `n` mean the person's space is in conflict
  (a thief and the owner both acted). The link is suspended until a re-verification (§13.11: in person, or by live call with the full safety code).
- **New device.** A device added by a valid chain is shown to the contact ("Léa a un nouvel appareil"), and
  offers are encrypted to it only after the contact confirms it. Until then offers go to confirmed devices only.
- **Revocation.** The owner publishes `n+1` without the device. Contacts stop encrypting to it and reject its
  signatures. At the relay, a deposit requires a live session of the sender's own space (§13.4), so the revoked
  device is cut at once without waiting for any contact.
- **New root** (all devices lost, new space): never accepted silently; alert and re-verification (§13.11).

### 13.2 Offers: sealed per recipient device, no long-lived link key

- An offer is **not** an item and **not** under a space key. It is a new, separate "offer" format with its own
  label registry and vectors (format 1 items are untouched). The content key is sealed once per confirmed
  recipient device with the existing X25519 scheme (`format-v1.md` §7.3) and new labels.
- No link key exists, so there is nothing to distribute to new devices (the old §4 problem: item states expire
  after 45 days, pairing only provisions the current key, history transfer is optional). A device that joins
  later is simply added to the device set; it cannot read offers sealed before it existed, which is intended.
- The sender's **signature is inside the sealed payload** and covers
  `label || recipient root || offer_id || sender device || created_at || offer hash || blob list`. This binds the
  link, the recipient and the time (no replay of an old offer, no cross-link replay), and the relay cannot test it
  against candidate keys.
- The recipient keeps a **ledger of seen offer ids per link**, so a refused or expired offer cannot be
  re-presented by the relay.
- New HKDF, AAD and signing labels are registered for all of this (`format-v1.md` §4.2); deposits use their own
  signed prefix, distinct from `tomi/v1/sig/relay-auth`.

### 13.3 Files: re-encrypted on acceptance, new item built from an allowlist

- On accept, the recipient builds a **new item** with a **recipient-chosen `item_id`**. Its payload is built from an
  allowlist only: file names, sizes and a locally derived type, plus a provenance field. Everything set by the
  sender is dropped: `secret`, `read_policy`, `expires_at`, `source_device`, `audience`, `thumbnail`, `text.html`
  and every unknown field.
- File contents are streamed chunk by chunk, decrypted from the offer and **re-encrypted as ordinary attachments**
  (new `item_id`, normal upload path), as in the history transfer rule. Cost: the recipient downloads and uploads
  once, and briefly holds both copies in quota.
- The MIME type is never trusted: it is derived locally after acceptance.
- **Accept is two-phase.** `claim` (a lease, one device wins) then import, then `commit`, which marks the offer
  `accepted`, records the new `item_id` as a receipt, and purges the offer's blobs. A crash or a
  `stale_key_epoch` before `commit` lets the lease expire and the offer returns to `pending`, so the sender never
  sees "Accepté" for a file that was lost. The sender's withdrawal cannot cut in after `claim`.

### 13.4 Relay

- **Deposits are authenticated by the sender's own session** (resolved to its space, `api-v1.md` §1.4), naming the
  link. There is no pseudonymous per-link key: it bought little privacy, since the relay sees the graph and the
  timing anyway, and it cannot be cut by the sender after a theft.
- **Own budget for inboxes**, separate from the space quota: per sender space and in total, counting
  **reservations** (a declared size is reserved before any byte arrives). Reservations without an offer expire.
  A per-space quota becomes mandatory whenever the feature is on. The orphan-blob sweep (24 h) is tied to the offer
  lifetime through an `offer_id` column, so a 7-day offer keeps its files.
- Offers are listed only when all their blobs are complete. One link per peer root. One offer state machine,
  enforced by the relay, including `claim` leases.
- Separate rate-limit buckets for contact exchange and deposits, so contacts cannot starve device pairing
  (`rl_global_claim` and the two open pairing channels per space are shared today).
- **Capabilities.** The relay announces a `features` flag; recipient devices register a "contact-capable" flag with
  the link, so a sender knows whether its offer can ever be read. An offer to a person with no capable device is
  refused at once instead of expiring after 7 days.
- Removal or blocking is indistinguishable for the sender from "not accepted". Removing a contact withdraws my
  outgoing offers to it.

### 13.5 Contact exchange

- The purpose is **bound into the CPace transcript** (a new label) or uses a distinct protocol version, never a
  non-critical TLV tag: an old inviter would otherwise ignore the purpose and send `provision` (the space key) to
  another person. Critical tags (0x80 and up) only.
- A separate frame sequence **without `provision`**, and a joiner path for a device that already has a space.
- The relay must be identical on both sides; a QR naming another relay is refused.
- The pairing code is `channel-word-word`; the 6 digits are the verification code, confirmed on both sides.

### 13.6 Consent and what is parsed before it

- Before consent, the core extracts **names, sizes and count only**, from an offer envelope capped at a few KiB.
  No thumbnail, no HTML, no declared MIME type reaches the UI.
- **No "Accepter" action in any notification** (Android, iOS, Windows toast); "Refuser" at most, and only on an
  unlocked device. Notification text is generic ("Nouvelle demande") and names the contact only once unlocked.
  The warnings (executable type, safety code changed, new device) are on the request sheet and cannot be skipped.

### 13.7 Hidden content

- The core, not only the UI, **refuses to send a hidden item to a contact on every path** (share sheet, tile,
  shortcut, "Envoyer vers", forwarding an existing hidden item).
- On the recipient side, an offer carrying `secret: true` is rejected, not imported.
- Pending offer metadata (names) is stored sealed under the local key, never in clear in SQLite.

### 13.8 Expiry, deletion and the sender's record

- The sender's chosen expiry is **only a suggestion** shown at acceptance ("Léa propose de le garder 7 jours"),
  applied only if the recipient opts in. The default on the recipient's copy stays "Jamais". An unknown
  `read_policy` is ignored for contact offers (it is not degraded to `after_first_read`). This removes the
  contradiction with "the sender never erases the recipient's copy".
- The sender's other devices share the offer through a new **item-state type "outgoing offer"** (offer id, link,
  cached status, names), sealed in the sender's space, so any device can show the status or withdraw.
- The feed shows the **provenance field**, not the relay row's `sender_device_id`, which would display my own
  accepting device.

### 13.9 What cannot be removed (inherent, not holes)

- The relay operator sees **who deposits to whom, when, and the sizes**, and can withhold or delay offers.
- A contact who accepted a file keeps it; no deletion reaches it.
- No forward secrecy for an offer while it is stored (mitigated by per-device sealing and a 7-day lifetime).
- A device stolen **before** its revocation can read what it receives until the owner revokes it.
- Per-contact device confirmation and re-verification add friction by design.
- Verification by live call is only as strong as the recognition of the person on that call: a voice or a video
  can be imitated. This is why the level is labelled and never silent (§13.11).

### 13.10 Corrections to the first version

- "Keep unknown payload fields" is **already implemented** (`format-v1.md` §5.4.4, `Item::raw_payload`); the real
  need is the opposite filtering for contact offers (§13.3).
- `audience` has the reserved form `{"to":[{"type":...,"id":...}]}` and only `device` and `member` types
  (`format-v1.md` §5.8.1); a v1 sender must not write it, so a `contact` type needs a spec first.
- `docs/multi-person-shared-relay.md` is listed as outdated (`FEATURES.md`, open gap 28); the relay sizing must be
  rechecked from `apps/relay/src/config.rs` and `store.rs` (`max_relay_bytes` 1 GiB shared, global reservation lock).
- Item states are limited to 2,048 padded bytes (relay cap 4,096): one statement per link, never a contact list.

### 13.11 Verification levels: in person or by live call

Decision: two people who cannot meet can still add or re-verify a contact, through a live call, as a **distinct,
labelled level**.

- Each link stores a level: **"Vérifié en personne"** (QR scan, or both screens side by side) or **"Vérifié à
  distance"** (live voice or video call). The app cannot detect which happened: the user declares it at the start,
  and the label says so.
- The protocol is the same for both (CPace code, 6-digit verification code compared on both sides, double
  confirmation). What differs is the channel used to compare, which must be out of band.
- **Remote rules.** An explicit screen requires a live call before any code appears. The short code is read aloud
  and **never sent by message**: remote mode has **no share, copy or link button and no QR**. The 6 digits are compared
  aloud, and each side must hear or see the other.
- **Remote re-verification after a suspended link or a new root** (the cases where an account may be compromised)
  requires, **in addition**, the **full safety code** (5 groups of 4 digits) read aloud and compared. Comparing
  only the 6 digits is not enough there. A single mismatch means stop and do not confirm.
- The level is the one of the **latest** verification: re-verifying remotely after an in-person verification lowers
  the label to "à distance". The contact sheet always offers "Vérifier en personne" to raise it.
- The level changes nothing in the cryptography. It is information shown on the contact sheet, in lists and on
  requests ("de Léa · vérifiée à distance").
