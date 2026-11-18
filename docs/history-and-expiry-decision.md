# Decision: default expiry (D1) and history for a new device (D2)

Status: **proposal**, pending maintainer validation. No file in `spec/`, `apps/` or `PROJECT.md` is modified
here; this document describes the delta to be applied by their maintainers.

References: `PROJECT.md` §5, §7 (lines 130, 187-189); `spec/format-v1.md` §5.4.2, §5.7, §7.3, §8, §12, §13;
`spec/api-v1.md` §2.8, §4.6, §4.8-4.10, §7, §8, B.1 no. 12.

## 0. Summary

> **SETTLED MAINTAINER DECISIONS.**
> - **D1: no default expiry.** No item (secrets included) expires unless explicitly chosen.
>   The default is "Never" for everything. "After first read" and durations (1 h, 1 d, 7 d, 30 d) remain options.
> - **D2: history transfer, option A, EVERYTHING included, no checkboxes.** The UX is a single binary
>   choice at pairing time (and from the Devices screen): transfer the history, yes or no. No granularity
>   between secrets and files. Accepted consequence: **kept secrets and all files are copied** to the new
>   device. Rationale: pairing is already the moment of full trust (the new device receives the space key);
>   adding checkboxes would make the UX heavier without real benefit. The transfer remains sealed for the
>   recipient's `x_pk` alone; the relay sees only sizes.
> - **Secret title: hidden.** The title is part of the secret: hidden in the feed, excluded from search, and
>   revealed only together with the rest of the content. Applies to Android and Windows.
> - **Pinning (chosen, instead of a tag system).** No tag taxonomy in v1: poor value-to-cost ratio for
>   single-user use, and in tension with "hidden title" (the tag of a secret would itself be sensitive).
>   Instead, a boolean **pinned** state per item:
>     - a pinned item **survives local purging** (it is not removed from the feed when the relay has purged
>       it after delivery); it therefore remains readable offline until the user unpins or deletes it;
>     - it **moves to the top** of the feed (a section or a "pinned first" sort);
>     - **UPDATED DECISION: the pin is part of the PROTOCOL (shared between devices), not local.**
>       Design adopted, to be recorded in the spec:
>         - **encrypted**: the pinned state must not leak to the relay (knowing which item is important is
>           sensitive metadata, especially for a secret). It travels in a signed and encrypted message.
>           **Adopted implementation, stricter**: the relay sees only a random `event_id` (plus the sender,
>           the epoch, the time and a constant size of 320 bytes), **never the `item_id` nor the state**;
>           otherwise the parity of events for the same item would reveal the state. Pinning also does not
>           trigger any file download, which would identify the item to the relay (`spec/format-v1.md` §8.8).
>         - **convergence by timestamped event**, similar to the deletion tombstone: `pin`/
>           `unpin(item_id, at)`; each device applies the most recent one (last-writer-wins on the timestamp).
>           Builds on the tombstone/event-stream mechanism already present in the core.
>         - **history transfer**: the pinned state is now part of what is transferred (it is in the protocol),
>           so a new device recovers its pins.
>         - interaction with expiry: unchanged. Pinning does not revive an item with an explicit expiry
>           (e.g. "after first read"); it only protects against purging of an item without expiry.
>         - **relay purge: DECISION = (a).** The pin synchronises the display and local retention on each
>           device; the relay purges normally once delivered to all active devices. A pinned item is therefore
>           NOT retained on the EC2 host: each device keeps its local copy beyond the relay purge. Consistent
>           with rejecting option C, "relay archive". (Option (b), retaining on the relay, was discarded.)
>           Implementation consequence: pin convergence goes through the encrypted event stream, but must NOT
>           change the relay purge policy; the relay does not read the pinned state.
>   Estimated size: medium (new encrypted protocol event + convergence + vectors + UI in both apps).
> - Open (non-blocking): maximum duration of an undelivered item on the relay (default retained: 30 d); monthly
>   review of old secrets (retained: yes); encrypted backup B (later); token prefixes `nvs1.`/`nvp1.` to be
>   settled when implementing the relay.

(Token prefixes settled since, when implementing the relay: `nvs1.`→`tms1.`, `nvp1.`→`tmp1.`.)

- **D1** requires **no byte change**: in format 1, `expires_at` is already optional and `read_policy` already
  defaults to `until_expiry`; the vectors cover `expires_at = null` (`payload-json.json`, third case of
  `item-text.json`). However, **two dates must be separated**: the expiry chosen by the user (inside the
  ciphertext, often absent) and the retention deadline on the relay (API field `expires_at`, always present,
  ≤ 30 d). Local storage must also be made independent of epoch keys, otherwise a device would have to keep
  its old space keys forever.
- **D2**: recommendation **A, "sealed history transfer"**. A member device (the PC, or the old phone if it still
  exists) exports its feed, encrypts it for the **only** new device (key sealed for its `x_pk`, signed
  manifest), and uploads it to the relay as temporary blobs (≤ 7 d). Offered at pairing time and from the
  Devices screen. No old epoch key is distributed, and the relay remains a mailbox. **B** (encrypted backup) may
  later reuse the same format for the "all devices lost" case. **C** (archive on the relay) is discarded.

---

## 1. D1 — No default expiry

### 1.1 What exists today

| Where | Current rule |
|-------|--------------|
| `PROJECT.md` §5 (line 130) | "Short default for secrets (1 h)" |
| `PROJECT.md` §7 (line 189) | Secrets: "after first read"; "never" option removed |
| `format-v1.md` §5.4.2 (line 289) | `after_first_read` = "default for secrets" |
| `format-v1.md` §5.7 | a single date: the relay purges the undelivered item **and** the device erases its copy at `expires_at` |
| `format-v1.md` §8.3 (end) | old space keys kept "as long as items from those epochs may exist (maximum expiry duration)" |
| `api-v1.md` §4.8 | `expires_at` **mandatory**, `> now`, `≤ now + max_ttl_secs` (30 d) |
| `api-v1.md` §8.1 | purge at `expires_at`, at tombstone, or after `release` by all active devices + 300 s |
| `docs/ux-flows.md` | bullets: text/link/file 7 d, secret 1 h |
| Android `FakeTomiCore` | demo secrets `AfterFirstRead` + 1 h |

### 1.2 Impact and choices

**a. Format: nothing to change in the bytes.** `expires_at` absent or `null` = never; `read_policy` absent =
`until_expiry`; unknown value = `after_first_read` (unchanged, restrictive). No `format = 2`, no new mandatory
vector. Only the normative text that designates a default changes.

**b. Two dates, plus one.** The rule "one visible duration" still holds for the user, but the spec must
distinguish:

| Date | Where | Used by | Value |
|------|-------|---------|-------|
| User expiry | encrypted payload (`expires_at`, optional, authenticated) | devices: local erasure | absent by default; explicit chip otherwise |
| Relay deadline | API field `expires_at` (in clear, not authenticated) | the relay: purge of an **undelivered** item | `min(user expiry, server_now + max_ttl_secs)` |

Choice: **keep the API field `expires_at` mandatory** and redefine its meaning ("retention deadline on the
relay"). No change to the relay or to validation; only the client changes the computation.
Discarded alternative: make the field optional with a relay-side default. The result is the same, but it would
require changing the relay, tests A45 and the schema for no gain. A useful side effect: the relay no longer learns
the real expiry when it exceeds 30 d or is absent.

**c. Relay purge of an item without expiry.** Rules (a), (b) and (c) of `api-v1.md` §8.1 remain the same.
An item without expiry is purged 300 s after `release` by all active devices, or at the latest at
`created_at + max_ttl_secs`. The **relay retention ceiling already exists**: it is `max_ttl_secs` (30 d).

**d. Device offline or lost (not revoked).** It remains "active" for up to 30 d after its last
`last_seen_at` and holds back the purge of all items during that time. The ceiling bounds retention to 30 d per
item, as it does today for a 30 d item. What changes is the **typical case**: an undelivered "1 h" secret used to
disappear after 1 h; without expiry, it waits 30 d on the relay (encrypted). Measures:
- sender status: `ExpiredBeforeDelivery` becomes "not delivered to X (kept 30 d on the relay)" when the
  user expiry is absent or later than the relay deadline;
- Devices screen: "'Pixel 8' has not retrieved anything for N days" beyond 7 d, with a Revoke button (UI only;
  `last_seen_at` is already in `DeviceRecord`);
- option: reduce `max_ttl_secs` to 14 d (question Q3).

**e. Relay quota.** The worst case does not change: 30 d of undelivered traffic, bounded by the relay-wide total
`max_relay_bytes` (1 GiB by default; no per-space quota by default, configurable from the web panel or
`max_space_bytes`).
Local storage no longer has a bound. Clients will need a screen "Space used / Clean up files older than N
months". This is UI only, with no spec change.

**f. Old space keys (hidden consequence).** With items that live indefinitely, the current rule
(§8.3: keep `SK_e` as long as items of the epoch exist) would force keeping **all** space keys. This removes the
mitigation of §12 ("discard old keys") against a revoked device colluding with a compromised relay. Correction
(also necessary for D2):
1. **Local storage under a local key `LSK`**, random, specific to the device, kept in the Keystore or via
   DPAPI, never derived from the space key. This settles `format-v1.md` §13.3 without a new HKDF label.
   After decryption, the item is re-sealed locally under `LSK`. The raw envelope is kept only for the states
   `UNREADABLE_UPDATE_REQUIRED`, `UNDECRYPTABLE` and `KEY_PENDING`.
2. `SK_e` is erased `max_ttl_secs + 1 d` after the rotation `e → e+1` is applied, unless a raw local item still
   depends on it.
3. **Epoch ordering rule** (client side): an `item` event of epoch `e` whose `seq` is greater than the `seq` of the
   already applied rotation `e → e+1` is **rejected** and shown as "unreadable (suspect)". It is never deleted
   (invariant 7). An honest relay cannot produce this case, provided the `stale_key_epoch` check happens in the
   write transaction that assigns the `seq`. `api-v1.md` §4.8 must require this explicitly.

**g. Secrets kept indefinitely: risks and measures.**

| Risk | Measure |
|------|---------|
| Device stolen and unlocked, PC left open | secrets hidden; display and copy behind biometrics or Windows Hello (invariant 4, already planned); app lock at launch proposed when the first kept secret is enabled |
| Forgotten accumulation | **secret review**: once a month, if secrets are older than 30 d, a card in the feed proposes "12 secrets kept for more than 30 days: Review / Later". Bulk "delete everywhere" |
| Case-by-case choice | the chip remains at capture: **Never** (default for all types), After first read, 1 h, 1 d, 7 d, 30 d. For an automatically detected secret, "After first read" is the second chip, highlighted but not preselected |
| Copy outside the app | Android: `allowBackup=false` / `dataExtractionRules` excluding the database (to verify in `apps/android`); Windows: database in `%LOCALAPPDATA%`, outside synced folders |
| Transfer to a new device (D2) | "Include secrets" option at transfer time (Q1) |

The "Keep" option is now only used for items with an explicit expiry; it remains forbidden for `after_first_read`.

### 1.3 Spec delta for D1

| Document | Section | Change |
|----------|---------|--------|
| `PROJECT.md` | §5 "Expiry" | "Expiry is **optional**, chosen at capture; **none by default**, secrets included. The relay keeps an undelivered item only until `min(expiry, 30 d)`." |
| `PROJECT.md` | §5 "After first read" | add "explicit option, never a default" |
| `PROJECT.md` | §7 Secrets | "No default expiry; options: after first read, duration. Hidden, monthly review" |
| `PROJECT.md` | §7 Relay purge | add "bounded by `max_ttl_secs` (30 d) even without expiry" |
| `PROJECT.md` | §7 (new line) | "Local storage: re-sealed under a per-device local key; space keys discarded 30 d + 1 d after rotation" |
| `format-v1.md` | §5.4.2 | replace "(default for secrets, PROJECT.md §7)" with "The client chooses this value only on explicit user request (PROJECT.md §7)" |
| `format-v1.md` | §5.7, 2nd paragraph | two dates: the payload carries the user expiry (local erasure); the clear-text copy provided to the relay is `min(expiry, now + max_ttl)` and serves only its purge |
| `format-v1.md` | §8.3, last paragraph | retention of `SK_e`: `max_ttl_secs + 1 d` after rotation, or as long as a raw local item depends on it |
| `format-v1.md` | §9.1, last sentence | "max TTL" → "as long as items of this format may be on the relay or kept raw locally" |
| `format-v1.md` | §12 "Revoked device + compromised relay" | add the epoch ordering rule (1.2 f.3) |
| `format-v1.md` | §13.3 | closed: `LSK` random, not derived |
| `api-v1.md` | §4.8, `expires_at` row | meaning: "retention deadline on the relay. The client sends `min(payload expiry, estimated_server_time + max_ttl_secs)`". Checks do not change, except one added requirement: the `stale_key_epoch` check happens in the transaction that assigns the `seq` |
| `api-v1.md` | §7.1 | `items: re-sealed under LSK`; `space_keys` with abandonment date |
| `api-v1.md` | §7.3 | epoch ordering rule; status "not delivered (kept 30 d)"; local erasure according to the **payload** expiry |
| `api-v1.md` | §8.1, last paragraph | local erasure follows the payload expiry, absent by default; the relay follows its deadline |
| `docs/ux-flows.md` | bullet defaults | "Never" everywhere (docs zone) |

Vectors: nothing mandatory. A case "secret without expiry or `read_policy` → `until_expiry`" may go into the new
D2 file (existing files are frozen).

---

## 2. D2 — Recovering items on a new phone

### 2.1 Inherited constraints

- The relay is not trusted; it holds no key (invariant 1). It must not be able to read, forge, replay or inject
  a history.
- There is no persistent confidentiality (§12). Any leaked epoch key opens all ciphertexts of that epoch still
  stored somewhere. Every ciphertext kept for a long time is therefore a risk.
- A new device receives only the key of the current epoch. `joined_seq` hides the earlier log from it.
- The pairing channel is limited to 8 messages of 8 KiB per side, for 120 s (§4.6). It is too small to carry a
  history, but it authenticates both devices (PAKE).
- With D1, the PC keeps the whole feed (received and sent), except what was deleted, consumed "after first read",
  or expired before delivery. **The PC is therefore a complete source when the phone is lost.**

### 2.2 Comparison

| Criterion | A. Device-to-device transfer | B. Export / encrypted backup | C. Archive on the relay + old keys | D1'. Permanent "archive" device | D2'. Recovery phrase |
|-----------|------|------|------|------|------|
| Old phone lost, PC remaining | **yes** (the PC is the source) | only if a recent backup exists | yes | yes if the archive runs | covers only keys; without B, no item |
| All devices lost | no | **yes** if the file is kept off-device | no (keys lost) | yes if the archive survives | yes with B |
| Who can read | only the recipient (key sealed for its `x_pk`, identity verified by PAKE or by the certificate chain); the relay sees only sizes | whoever has the file **and** the phrase; offline brute force if the phrase is weak | anyone holding an old key + a copy of the database, with no time limit | as A, plus an always-on machine that is a member | whoever has the phrase reads everything |
| Persistent confidentiality | unchanged (temporary ciphertext ≤ 7 d) | degraded: the file survives rotations and deletions | **strongly degraded**: the entire history at rest on the EC2 host | unchanged | degraded (one phrase = everything) |
| Revoked device | cannot receive (not a member), nor send (revoked signer rejected) | an old backup remains readable | with a copy of the database, it reads everything until its revocation (today: ≤ 30 d undelivered) | not applicable | not applicable |
| Relay cost | transient (≤ 1 GiB, ≤ 7 d) | nil | unbounded growth, files included; purge (c) disappears | nil | nil |
| Spec / core | medium: 1 signed declaration, 1 sealed key, STREAM reuse, 3 routes, 2 events | medium: KDF (Argon2id), file format, list of tombstones to replay | medium to large: relay archive mode, "archive keys" declaration, quotas, interaction with `joined_seq` | small (CLI as a daemon) | medium (derivation, key escrow) |
| UX | at pairing: "Transfer history from Desktop PC? 812 items, 340 MB", then progress; can be redone from Devices | manual step, phrase to remember, dated backup | transparent | requires a machine permanently on | a sheet of paper to keep |
| Secrets | option at source (Q1); hidden on arrival | in the file, therefore exposed with it | all, forever | as A | as B |
| Files | option, ceiling, new keys | heavy file | all kept on the relay | as A | as B |

Also discarded: direct transfer over the local network (NAT, Windows firewall, Android in the background, for no
gain in confidentiality); sending the history over the pairing channel (too small).

### 2.3 Recommendation: A, "sealed history transfer"

- Covers the case cited (phone lost, only the PC remains) and the case "new phone, old one still around".
- Adds no durable ciphertext on the relay and distributes no old epoch key. The invariants and the spirit of the
  relay-as-mailbox principle are preserved.
- Any trusted member can send it to any member. The same mechanism therefore serves pairing, recovery after a
  failure, or a device returning after 45 d (`cursor_expired`).
- A `tomi` CLI on a home machine can act as the archive device (D1') without adding anything: it is a member like
  any other.
- **B later** (v1.x, Q5): the same index file (§2.5) encrypted under a printable recovery key (256 bits, rather than
  a weak phrase), together with the list of known tombstones so that deleted items are not resurrected.

### 2.4 Flow

At pairing (A = inviting device, N = new device):

1. Usual pairing (`format-v1.md` §8.2, steps 1 to 3). On A, the pairing screen shows "Transfer history?" with the
   checkboxes *Secrets (12)* and *Files (340 MB)* (local estimate).
   In its final encrypted message, A adds `history: { "transfer_id": uuid, "items": n, "bytes": b }`, an additive
   field of the pairing content (defined with the channel, `format-v1.md` §13.1). N displays "History being prepared
   on Desktop PC".
2. A calls `POST /v1/devices`. N receives `joined_seq = J`.
3. A synchronises until `cursor ≥ J`, takes a snapshot `as_of_seq = cursor`, writes the index, encrypts the parts
   and files, uploads the blobs (§4.9, resumable), then calls `POST /v1/transfers`.
   This is a persistent, resumable task, like the outbox. If A goes offline, it resumes later.
4. N receives the `history_transfer` event. It verifies the manifest, downloads and imports in batches, then calls
   `DELETE /v1/transfers/{id}` with `outcome: "imported"`. A displays "812 items transferred to Pixel 8".

Later: Devices → Pixel 8 → "Send my history", with the same options.

Consistency with the log: since `as_of_seq ≥ J`, everything after the snapshot reaches N through the log
(`seq > J`). What lies between `J` and `as_of_seq` arrives from both sides, and the import skips duplicates by
`item_id`. A tombstone issued after the snapshot arrives through the log: if N has already applied it, the import
skips the item (`tombstoned`); otherwise, the tombstone erases the imported item when it arrives.

### 2.5 Format (addition to `format-v1.md`, without changing `format = 1`)

**Labels** (§4.2): `tomi/v1/hkdf/history-transport`, `tomi/v1/aad/history-transport`,
`tomi/v1/aad/history-chunk`, `tomi/v1/sig/history-transfer`.

**Transfer key `HK`**: 32 random bytes per transfer, single use (to encrypt the index parts).
It is sealed for the recipient R as in §7.3, with other labels:

```
K       = HKDF(E_pub || R_xpk, X25519(e, R_xpk), "tomi/v1/hkdf/history-transport", 32)
AAD     = "tomi/v1/aad/history-transport" || 0x00 || uuid16(space_id) || uuid16(transfer_id) || R_edpk
sealed_hk = E_pub (32) || nonce (24) || XC_Seal(K, nonce, AAD, HK) (48)          // 104 octets
```

The sender takes `R_xpk` from **its own** trust store (the rule that sealing applies only to keys from one's own trust store).

**`HistoryTransfer` manifest** (new §8.7, common rules of §8.1):

```
TBS = "tomi/v1/sig/history-transfer" || 0x00 || uuid16(space_id) || uuid16(transfer_id)
      || u64be(as_of_seq) || u64be(created_at_ms) || sender_ed_pk (32) || recipient_ed_pk (32)
      || sealed_hk (104)
      || u32be(n_parts) || n_parts × ( uuid16(blob_id) || nonce_prefix (19) )
wire = TBS || signature (64)          // 316 + 35·n_parts octets ; 1 ≤ n_parts ≤ 1024
```

Exception to §8.1: the list of parts follows the **part order**; it is not sorted. Duplicate `blob_id`
values are rejected.

Verification by R, in order: `space_id` (`space_mismatch`); signer is a trusted, non-revoked member
(`untrusted_signer` / `revoked_device`); signature (`invalid_signature`); `recipient_ed_pk` is its own
(`not_a_recipient`); opening of `sealed_hk` (`decryption_failed`).

**Index parts**: part `p` is encrypted with STREAM (§6) under `HK`, with the `nonce_prefix` of part `p`
(random and distinct for each part) and:

```
AAD_hist = "tomi/v1/aad/history-chunk" || 0x00 || u32be(format) || uuid16(transfer_id) || u32be(p) || u32be(n_parts)
```

Each part ≤ `max_blob_bytes`. Number of chunks per part: `N = ceil(len(blob) / 65552)` (derived from the blob
size, since no clear-text `size` is authenticated here; the final flag is authoritative). The AAD prevents
swapping or removing parts. The count is signed.

**Clear-text index** = concatenation of the parts = **JSON Lines** in UTF-8 (one object per line), readable as a
stream without loading everything into memory:

```json
{"kind":"header","version":1,"space_id":"…","transfer_id":"…","as_of_seq":1793754000123,"source_device":"1aa4eb9f…"}
{"kind":"device","device_id":"4a3d2129…","name":"Pixel 8","platform":"android","revoked":true}
{"kind":"item","payload":{ …object §5.4 as decrypted… },"origin_seq":1793750400120,"received_at":1793750402000,"read":true,"kept":false,"files":[{"blob_id":"…","key":"<b64 32>","nonce_prefix":"<b64 19>"},null]}
{"kind":"end","items":812,"skipped":{"unreadable":3,"files_excluded":41,"secrets_excluded":0}}
```

- The `header` (first line) must match the manifest, otherwise `malformed_payload`. A missing `end` line yields
  `history_incomplete` (new code). An unknown `kind` is ignored (additive rule §5.4.4).
- `payload` is re-read with the rules of §5.4; two entries with the same `id` → `malformed_payload`.
- The `device` lines make it possible to display the name of a revoked sender (the old phone), which
  `GET /v1/devices` no longer returns.
- `files[i]` corresponds to `payload.attachments[i]`: a **new blob**, encrypted with STREAM using a key
  `FK'` and a **fresh** prefix (never the original `FK`, to rule out any nonce reuse) and the
  `AAD_chunk` of §6.3 (same `item_id`, same index). `null` = file not transferred. The thumbnail, in the
  payload, remains available.
- The clear-text size of the index is capped by `max_transfer_bytes` (relay).

**Not exported**: unreadable items (`UNREADABLE*`, `KEY_PENDING`; they are counted in
`skipped`), items still in the outbox (they will arrive through the log), acknowledgements and delivery
statuses.

### 2.6 API (addition to `api-v1.md`, new §4.13)

**`POST /v1/transfers`** — B (sender)

| Field | Type | Req. | Rule |
|-------|------|------|------|
| `transfer_id` | uuid | yes | equal to the one in the manifest |
| `manifest` | statement | yes | `HistoryTransfer` signed by the caller |
| `blob_ids` | list of uuid | yes | index parts **and** files; contains all the parts of the manifest; ≤ `max_blobs_per_transfer`; no duplicates |

```json
{ "transfer_id": "c1a7e0d2-5b3f-4e8a-9c61-2d4f6a8b0c1e", "manifest": "b64…", "blob_ids": ["…", "…"] }
```

Checks, in order: idempotence (same manifest bytes, same set of blobs → `200`, original response; otherwise
`409 transfer_id_conflict`); manifest (decoding, `space_id`, signer = caller, signature, recipient = non-revoked
member different from the caller, `joined_seq(dest) ≤ as_of_seq ≤ head_seq`) → `422 validation_failed`,
`field = "/manifest"`; blobs (owned by the caller `404`, complete `409 blob_incomplete`, not attached
`409 blob_attached`); total ≤ `max_transfer_bytes` (`413`); open transfers to this recipient < `transfer_max_open`
(`409 transfer_limit`); quota (`507`).

Effects: attach the blobs to the transfer, `expires_at = now + transfer_ttl_secs`, `history_transfer` event,
a ping to the recipient (FCM `NORMAL`). Response `201`:

```json
{ "transfer_id": "c1a7e0d2-…", "seq": 1793754000456, "expires_at": "2026-11-11T09:00:00.000Z" }
```

**`GET /v1/transfers/{transfer_id}`** — B (sender or recipient, otherwise `404`):

```json
{
  "transfer_id": "c1a7e0d2-…",
  "from_device_id": "1aa4eb9f…", "to_device_id": "4a3d2129…",
  "manifest": "b64…",
  "blobs": [{ "blob_id": "…", "size": 1048720 }],
  "created_at": "…", "expires_at": "…", "seq": 1793754000456
}
```

**`DELETE /v1/transfers/{transfer_id}`** — B (sender: cancellation; recipient: completion). Optional body
`{ "outcome": "imported" | "declined" | "failed" }` (informational). Effects: blobs deleted and quota released;
row kept for 45 d for idempotence; `history_transfer_closed` event. `204`, idempotent.

**Blobs** (§4.9): `GET`/`HEAD /v1/blobs/{id}` are also visible to the **recipient** of a non-expired transfer
that contains the blob. A blob attached to a transfer returns `409 blob_attached` for `POST /v1/items` and
for `DELETE /v1/blobs`.

**Events** (§4.10):

| `type` | `data` | FCM ping |
|--------|--------|----------|
| `history_transfer` | `transfer_id`, `from_device_id`, `to_device_id`, `total_bytes`, `expires_at` | `NORMAL`, recipient only |
| `history_transfer_closed` | `transfer_id`, `by_device_id`, `outcome` (string, unknown values tolerated) | no |

Older clients ignore these types (existing rule). The `joined_seq` filter stays as it is: history goes through
the transfer, never through the log.

**New codes** (§2.5): `transfer_id_conflict` (409); `transfer_limit` (409, extension `transfer_max_open`).

**Limits and configuration** (§2.8, §8.6; published in `hello.limits`, additive addition):
`max_transfer_bytes` = 1,073,741,824; `max_blobs_per_transfer` = 10,000; `transfer_ttl_secs` = 604,800 (7 d);
`transfer_max_open` = 2 per recipient. Rate limit `rl_device_transfers`: 10 per hour, burst 3.

**Purge** (§8.1): a transfer is purged at `expires_at`, at `DELETE`, or on revocation of its sender or its
recipient (within the transaction of §3.4). The collection delay for orphan blobs (24 h) remains the same.
**Quota** (§8.4): transfer blobs count.
**What the relay sees** (§1.2, and golden rule 1 in `AGENTS.md`): per transfer, the sender, the recipient, the
number and size of the blobs, `as_of_seq` and the public keys. It does not see the number of items, their
types, or the share of secrets.
**Annex A**: table `transfers(transfer_id, from, to, manifest, as_of_seq, created_at, expires_at, closed_at,
outcome)`; column `blobs.transfer_id` (mutually exclusive with `item_id`).

**Acceptance checks** (§11, new):
- **A140** Valid transfer A → B: `201`, `history_transfer` event; B reads the transfer and downloads its
  blobs; C (member) receives `404` on the transfer and on its blobs.
- **A141** Manifest signed by someone other than the caller, with a different `space_id`, recipient not a member
  or equal to the caller: `422` (`/manifest`).
- **A142** `as_of_seq < joined_seq` of the recipient or `> head_seq`: `422`.
- **A143** Identical replay: `200`, no event; same `transfer_id` with a different manifest:
  `409 transfer_id_conflict`.
- **A144** `DELETE` by B: blobs gone (database and disk), `used_bytes` decreases, `history_transfer_closed`,
  then `GET` → `404`.
- **A145** Transfer older than 7 d: purged. Revocation of B or of A: transfer purged.
- **A146** Total > `max_transfer_bytes`: `413`; quota exceeded: `507`; 3rd transfer open to B:
  `409 transfer_limit`.
- **A147** Blob attached to a transfer then referenced by `POST /v1/items`: `409 blob_attached`, and vice versa.

### 2.7 Import rules (core, normative in `api-v1.md` §7)

- Import in batches within local transactions; insertion only if the `item_id` is absent from the items and from
  `tombstoned`. A resume after a crash is therefore idempotent.
- Imported item: `origin = imported(from_device)`, `read` taken from the index (no notification, no badge).
  **No `ack`, `read` or `release` is sent to the relay** for it, since the relay would respond `404`
  (`joined_seq`).
- Local expiry follows the payload. `kept: true` restores the "Keep" option.
- An unread `after_first_read` item remains single-use. Revealing it on N issues the tombstone everywhere, which is
  accepted for any `item_id` (§4.8).
- Secrets: hidden like all secrets; never in the completion notification ("812 items imported").
- Files not transferred: the card shows the thumbnail and "File left on Desktop PC".

### 2.8 Vectors to add

New file `spec/vectors/v1/history-transfer.json`, plus one **added line** in `SHA256SUMS` and in the table of
`README.md`. Existing files do not change. Contents, with devices A, B, C and D from `device.json`:
- `sealed_hk` from A for B (fixed ephemeral key), intermediate steps (`shared`, `K`, AAD);
- A → B manifest with 2 parts: hexadecimal TBS, signature;
- STREAM index parts (clear JSON Lines, encrypted); one file re-encrypted (`FK'`) for the item in
  `item-attachment.json`;
- secret without `expires_at` or `read_policy` → `until_expiry` (D1);
- failures: `sealed_hk` opened by C (`decryption_failed`); signer D (`untrusted_signer`); signer C
  after `rotation.json` (`revoked_device`); recipient ≠ self (`not_a_recipient`); parts 0 and 1 swapped
  (`decryption_failed`); altered `n_parts` (`invalid_signature`); missing `end` line (`history_incomplete`);
  different `header.transfer_id` (`malformed_payload`); last chunk of a part removed (`stream_truncated`).

### 2.9 What changes in the code

**Rust core (`crates/core`)**:
- `labels`: the 4 labels; `error.rs`: `history_incomplete`.
- new `history.rs`: `HistoryTransfer::{to_bytes, from_bytes, verify_signature}`;
  `seal_history_key` / `open_history_key`; `HistoryWriter` (entries → STREAM parts ≤ `max_blob_bytes`);
  `HistoryReader` (parts → entry iterator, streamed reading).
- `TrustStore::verify_history_transfer(&m) -> Result<HistoryKey>` (rules of §2.5).
- local storage (still to be written): `LSK`, re-sealing, `origin`, idempotent import, discarding old
  keys (§1.2 f).
- synchronisation: `history_transfer` event → resumable download task (`Range`) and import,
  then `DELETE`; sender-side export task (snapshot at `as_of_seq ≥ J`, resumable upload); epoch
  ordering rule; relay deadline computation (D1).
- relay (`apps/relay`): 3 routes, 2 events, table, purge, quota, limits (checked with `tomi-core`).

**`TomiCore` (Kotlin, `apps/android`)**:
- D1: no type changes (`expiresAt: Instant?` and `ReadPolicy.Normal` as default already exist). To align:
  the chips (default "Never"), `FakeTomiCore` (demo secrets without the imposed 1 h), the label of
  `DeliveryState.ExpiredBeforeDelivery` ("not delivered, kept 30 d"), the "secret review" card.
- D2, additions:

```kotlin
data class HistoryOptions(val includeSecrets: Boolean = true, val includeFiles: Boolean = true)
data class HistoryEstimate(val items: Int, val secrets: Int, val files: Int, val fileBytes: Long, val unreadable: Int)
enum class TransferDirection { Outgoing, Incoming }
enum class TransferState { Preparing, Uploading, Waiting, Downloading, Importing, Done, Failed, Cancelled }
data class HistoryTransfer(val id: String, val direction: TransferDirection, val peer: DeviceRef,
                           val state: TransferState, val progress: Float?, val imported: Int?, val skipped: Int?)

// interface TomiCore
val historyTransfers: Flow<List<HistoryTransfer>>
suspend fun estimateHistory(options: HistoryOptions): HistoryEstimate
suspend fun sendHistory(toDeviceId: String, options: HistoryOptions): String   // transferId
suspend fun cancelHistoryTransfer(id: String)
// TomiItem : + val importedFrom: DeviceRef? = null
// Attachment  : + val transferred: Boolean = true   (false = "left on the source device")
// PairingResult.Success : + val historyOffer: HistoryEstimate? = null
```

**Windows (Tauri)**: the same commands; in the pairing screen on the inviting side, the Secrets and
Files checkboxes with the estimate.

### 2.10 `PROJECT.md` delta for D2

- §7, line "New device history": "The relay does not replay history (`joined_seq`). A member device can **transfer
  its feed**, encrypted for the new device alone, at pairing or later; secrets and files optional. No old key is
  distributed."
- §5: a bullet "History transfer: single-use key sealed for the recipient, signed manifest, temporary blobs on the
  relay (≤ 7 d)".
- `api-v1.md` B.1 no. 12: the reason becomes "history goes through a sealed transfer (§4.13), not through the
  log".

---

## 3. Estimated sizes

| Item | Size |
|------|------|
| D1: spec and `PROJECT.md` texts, client-side relay deadline computation | small |
| D1: chips, labels, monthly secret review (Android, Windows) | small |
| D1: local storage under `LSK`, discarding old keys, epoch ordering rule | medium (local storage has to be written anyway) |
| D2: format (labels, sealed key, manifest, JSON Lines index) and vectors | medium |
| D2: relay (3 routes, 2 events, purge, quota, A140-A147) | medium |
| D2: core, resumable export and import tasks, UniFFI | medium |
| D2: pairing UI, Devices, progress (Android and Windows) | small to medium |
| B later (backup under recovery key, D2 format reused) | medium |
| C (not retained) | large |

## 4. Open questions for the maintainers

1. **Secrets in the transfer**: included by default, untickable checkbox (recommended) / excluded by default, tick
   box / never transferred?
2. **Files in the transfer**: all / all up to 200 MB, beyond that ask (recommended) / never
   (thumbnails only)?
3. **Maximum duration of an undelivered item on the relay**: 30 d, as today (recommended) / 14 d / 7 d?
4. **Monthly review of secrets older than 30 days** in the feed: yes (recommended) / no?
5. **Encrypted backup (B) for the "all devices lost" case**: in v1.x with a printable recovery key /
   later / never?
