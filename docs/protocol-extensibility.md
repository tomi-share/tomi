# Report: Tomi protocol extensibility and multi-recipient reservation

Status: **design report**, written together with the reservation it accompanies.
Only the reservation (part 1) is implemented; parts 3 and 4 are analysis.
Basis: `PROJECT.md`, `spec/format-v1.md`, `spec/api-v1.md`, `spec/pin-contract.md`,
`docs/multi-person-evolution.md` and `crates/core/src/` (item.rs, device.rs, keyring.rs,
pin.rs, history.rs, crypto.rs, local.rs, envelope.rs), branch `main`, November 2026.

## Summary

- Two extension points are reserved in format 1, with no effect in v1 and **without changing a
  single existing byte**: the `audience` field of the payload (who the item is intended for) and the
  range `key_epoch ≥ 2^31` (identifiers of future keys outside the space, the only way to restrict
  reading without changing the envelope). The 12 published vector files are unchanged; an additive
  file `reservations.json` fixes the behaviour of a v1 reader.
- `audience` is **not** a confidentiality boundary: the item remains encrypted under the space key, and
  every member can read it. Only a key that non-recipients do not hold restricts reading; that is the
  role of the reserved `key_epoch` range.
- These reservations avoid a `format` 2 for the payload and, if group keys are used, for the envelope.
  They do not avoid changing the **trust model**: today every member device can certify, revoke and delete
  everywhere. Putting a second person in the same space requires all devices to apply new rules, which
  means a version barrier. The most compatible path is to keep **one space per person** and add
  **shared groups** (part 3.2).

---

## 1. (a) What is reserved, where, and why

| Reservation | Where | v1 behaviour | Why |
|-------------|----|-----------------|----------|
| `audience` field of the payload: `{"to": [{"type": "device" \| "member" \| …, "id": "…"}]}`; absent or `null` = whole space | `spec/format-v1.md` §5.4 (table row), §5.4.4, **§5.8.1**; §12 (limit) | a v1 sender never writes it; a v1 reader ignores it, whatever its value | eventually describe an item's recipients **inside the encrypted payload** (the relay does not see them), without a new payload `format` |
| `key_epoch` range from `0x80000000` to `0xFFFFFFFF`: identifiers of keys outside the space (group, recipient) | `format-v1.md` §3.1, **§5.8.2**, §8.2, §8.3, §9.4, §10; `api-v1.md` §4.8 (two endpoints) | no space key is created or accepted there; an item or item state announcing it stays KEY_PENDING, never deleted; the v1 relay rejects it (`stale_key_epoch`) | the envelope carries only one encrypted CEK, under the key designated by `key_epoch`; another key is therefore the only way to restrict reading **without changing the binary layout** |
| `kind: "member"` row in the history transfer index | `format-v1.md` §5.8.3, §8.7.3 | ignored (unknown `kind`), never written | eventually list known people, following the pattern of the `device` rows |
| `capabilities` field of `ClientInfo` and `DeviceRecord` | `api-v1.md` §3.3, §4.3, appendix B.1 no. 29 | not sent, ignored by the relay | announce a feature added **within** format 1 (reading a group key), which `format` negotiation cannot signal |
| Nothing for labels, item state types, event types, error codes, transfer manifest | `format-v1.md` §5.8.4, `api-v1.md` §2.5 | already extensible (unknown = ignored) or not extensible by a field (fixed binary declarations) | a reservation would add nothing there |

### Choice of the name `audience`

- `recipients` is already the name of the devices that receive a key sealed in a
  `KeyRotation` (§8.3), and suggests per-recipient encryption.
- `scope` is already the scope of a tombstone (`local` / `everywhere`, §8.5).
- `audience` says "who this is intended for" without promising who can read it, like the `aud` claim
  of JWTs (RFC 7519 §4.1.3).

The form is an **object** (fields can be added alongside `to`) containing a **list of typed entries**
(one or more recipients, of different types, each entry able to gain fields). A future version that uses
it keeps "absent = whole space" and ignores entries of a type it does not know (§5.8.1, rule 3).

### What changes in `crates/core`

| File | Change | Effect on the bytes |
|------|--------|---------------------|
| `src/format.rs`, `src/lib.rs` | public constant `SPACE_KEY_EPOCH_MAX = 0x7FFFFFFF` | none |
| `src/crypto.rs` | `check_space_key_epoch`: `SpaceKey::generate` / `from_bytes` reject 0 (already the case) **and** a reserved epoch (`invalid_input`) | none |
| `src/device.rs` | `TrustStore::new` rejects a reserved epoch; `KeyRotation::from_bytes` and `MemberList::from_bytes` reject it at decoding (`malformed_statement`) | none; unreachable in practice (+1 per rotation) |
| `src/item.rs`, `src/keyring.rs` | comments only: `audience` must **not** be declared in `WireItem` until a spec gives it meaning; `key_for_item` already classifies a reserved value as `KeyPending` | none |
| `examples/gen_spec_vectors.rs`, `tests/spec_vectors.rs` | generation and verification of `reservations.json` (independent reconstruction of the bytes, then public API) | new file only |

The behaviour "a v1 reader ignores `audience`" requires no code: `WireItem` does not use
`deny_unknown_fields`. This was checked before writing the spec: a payload carrying `audience` as an
object, an empty list, `null` or a string is read without error and yields the same `Item` as without
the field. The vector freezes this.

### Vectors: no existing file modified

The generator (`cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1`) refuses to
overwrite a file that would differ. It reported `unchanged` for the 12 published files, then
`wrote reservations.json`, then `added lines to SHA256SUMS`: a single line added, none modified. A second
run reports `unchanged` everywhere. Contents of `reservations.json`:

- 6 payloads carrying `audience` (one device; several recipients with an unknown type and unknown
  fields; secret with title; `null`; empty list; string), each equal to its `equivalent_json` without
  `audience`;
- a complete envelope carrying `audience`, which opens under the space key like the others;
- an item and an item-state event with epoch `0x80000001`: KEY_PENDING, before and after a rotation
  (never UNREADABLE_SUSPECT_EPOCH);
- a `KeyRotation` and a `MemberList` with epoch `0x80000000`, correctly signed by A, rejected at
  decoding (`malformed_statement`).

### Two findings from verification

1. **Unknown duplicate keys are not detected.** §5.4 says duplicate keys MUST be rejected. The reference
   parser only does so for the fields it declares: `{"audience":1,"audience":2}` is accepted, because serde
   skips unknown fields without tracking them. This is not exploitable (only a member can write a payload),
   but a strictly conforming reimplementation would diverge. This case was not frozen in a vector.
   Decision needed: either state in the spec "**known** duplicate keys", or make the parser strict (see 2, row 3).
2. **The v1 core drops unknown fields when it rewrites a payload.** Local re-sealing (`LocalKey::seal_item`)
   and history export (`HistoryIndexWriter::item_line`) re-serialise the `Item`, which keeps only the known
   fields. A v1 device that receives an item carrying `audience` and then forwards it sends it without
   `audience`. For `audience` this is acceptable (the field is informational, and a v1 device cannot open an
   item under a group key, so it does not export it). For any future additive field, it is a silent loss:
   this is the first recommendation in part 2.

---

## 2. (b) What else should be planned to stay without a format 2

Each row: benefit, cost **today**, risk if nothing is done, recommendation.

| # | Topic | Benefit | Cost today | Risk if nothing is done | Recommendation |
|---|-------|---------|------------|-------------------------|----------------|
| 1 | **Keep unknown payload fields** (keep the original JSON next to the `Item`, reuse it on local re-sealing and on export) | "additive" becomes true end to end: a field added later survives passing through an older device | medium: `Item` keeps its original bytes; `LocalKey` and `HistoryIndexWriter` reuse them; tests. No exchanged byte changes | every future additive field is lost on devices not yet updated, permanently for their local copies | **do before the first additive field is actually used**; the only item on this list that really underpins the promise "without a format 2" |
| 2 | **Capabilities announced per device** (`capabilities`, reserved name) | a barrier for a feature added within format 1: the sender knows which devices can read a group key | small: the relay stores the list and returns it in DeviceRecord; the core defines the names | without it, an item under a group key stays "pending key" on a non-updated device, and the sender does not know; the only other barrier is a new `format` | reserve the name now; write the mechanism together with the first feature that needs it |
| 3 | **Rule for duplicate JSON keys** | a single rule, the same for every implementation | very small to align the spec ("known keys"); medium for a strict parser (a pre-pass that detects duplicates at all depths) | possible divergence with a reimplementation (pure Kotlin, for example); no effect as long as `crates/core` is the only implementation | align the spec with actual behaviour, unless a reimplementation is planned |
| 4 | **Item schema version number** in the payload | none in practice: `format` and the additive rule already cover both cases | trivial | none; on the contrary, an internal version invites breaking changes that an older reader would not see (it ignores the field) | **do not do** |
| 5 | **Field namespace** (`x_…`, `ext` object) | avoid an experiment colliding with a future official field | small | low: a single team, and the table in §5.4 serves as the registry | do not do; register each new field in §5.4 before use |
| 6 | **Member / person identifier** | fix the shape of `member` entries now | small to write, but binding | none for compatibility: `member` is reserved and a v1 reader ignores it | **do not fix**: the shape depends on the crypto model (a person key, or an MLS identifier) |
| 7 | **`sender` field distinct from `source_device`**; `recipient` field | `recipient`: this is `audience`, already reserved | trivial | none: an unauthenticated `sender` would be as forgeable as `source_device` (§12) | do not add `sender`; derive the sending person from `source_device` and a device → person link that is **signed**. The real need is sender authentication (part 3, point 5) |
| 8 | **Size and padding margins** | room for recipient metadata | none | low: `audience` costs a few dozen bytes per recipient in a 1 MiB payload; `max_envelope_bytes` keeps a 64 KiB margin; item states go up to 2,048 padded bytes | nothing to do; bound `to` to 1,024 entries (like `MAX_STATEMENT_ENTRIES`) on the day `audience` gets a meaning |
| 9 | **Reserves in enumerations** | — | — | tombstone `scope` (`0x01`, `0x02`): any other value is rejected (`malformed_statement`), so a "for recipients" scope cannot be additive, reserved or not. `read_policy`, `source_format`, state types, index `kind` and `outcome` already degrade cleanly on unknown values. `UnreadableReason`: a new variant ("not for this device") changes the apps' UniFFI API, not the format | nothing to reserve; a targeted deletion will be a new declaration type |
| 10 | **`version` of the transfer index header** | — | — | a reader rejects any `version ≠ 1`: moving to 2 breaks transfer to an older device | keep `version = 1` and only add fields or `kind` values; to be written in §8.7.3 if the point arises |

---

## 3. (c) Limits: what cannot be additive

### 3.1 What will require a `format` 2, or at least a version barrier

1. **Restricting reading of an item already encrypted under the space key.** Not possible after the fact:
   any member who holds the space key can read it. Existing items will remain readable by the whole space.
   This is not a format problem; it is the nature of a shared key.
2. **Arbitrary recipients per item** ("this link for Alex and my PC only"). The envelope has only one slot
   for an encrypted CEK (§5.2, offset 48, 48 bytes). Three options: one group key per recipient set (compatible
   with the reserved range, but one group must be created per combination); one envelope per set (the payload
   is duplicated, and so are the files, because a blob attaches to only one item, `api-v1.md` §4.8); or an
   envelope with several encrypted CEKs, which changes the binary layout: **format 2**.
3. **Targeted revocation.** Today, revoking = rotating **the** space key sealed for all other members (§8.3),
   and any member can revoke any other. Revoking only from a group requires one rotation per group (a new
   declaration, additive in itself), but also authority rules: who may revoke whom. A v1 device would accept a
   `KeyRotation` signed by another person's device that revokes mine. The new rules only protect if **all**
   devices apply them: v1 devices must be excluded, which only a version barrier can do.
4. **Flat trust model.** `TrustStore` is a set of devices that are all fully trusted: a `DeviceCert` signed by
   any member admits a device (§8.4), an `everywhere` tombstone from any member erases everyone's copy (§8.5),
   and the relay accepts a rotation from any authorised device. With a second person in the same space, that
   person could add devices, revoke the user's devices and delete the user's items. These rules are enforced by
   each v1 device: they cannot be hardened additively.
5. **Sender authentication between members** (§12). The envelope has no signature slot, and the JSON is not
   canonical, so one cannot sign "the payload without the signature field". Options: a signed and encrypted
   side declaration (modelled on the pin: additive, but one more event per item, which the relay can correlate);
   or a signed envelope, in format 2. For a single user this is acceptable; between two people, it is not.
6. **Relay.** It is single-space, requires `key_epoch` = current epoch (`stale_key_epoch`), purges when "all active
   devices" have the item, and broadcasts the log to everyone. Routing and purging per group change its contract:
   a new API version or significant additions. This is not a format issue, but it is work.
7. **Persistent confidentiality and post-compromise security.** Absent (§12). Obtaining them (ratchet, MLS)
   replaces all key management: a redesign.

### 3.2 What the reservations still allow: two architectures

| | M1: a second person **inside** my space | M2: **one space per person** + shared groups |
|---|---|---|
| Idea | the space contains the devices of two people; people and rights are added | each person keeps their space, their v1 rules and their space key; a group has its own key (identifier in the reserved range), known to its members' devices |
| Existing trust rules | to be redesigned (points 3 and 4) | unchanged within each space; only the **group's** rules are new |
| v1 devices | dangerous (old rules): must be excluded, hence a version barrier | safe: they cannot read the group's items (KEY_PENDING) and receive no rights over the other person |
| Format | de facto 2 | envelope and payload in format 1 (`key_epoch` reserved, `audience`); new declarations for groups |
| Relay | changes | becomes multi-space and routes by group |

Recommendation: **M2**. The reservations made here are sized for it.

---

## 4. (d) Multi-recipient: which cryptographic model

### 4.1 The models

| Model | Principle | Persistent confidentiality / post-compromise | Cost per item | Cost of a membership change | Format 1 compatibility |
|-------|-----------|----------------------------------------------|---------------|-----------------------------|------------------------|
| **A. Envelope per recipient** (age / HPKE style) | the CEK is encrypted once per recipient device with its `x_pk` | no / no | ~104 bytes per recipient device | none for future items: encryption for the revoked device stops | no within the envelope (a single CEK slot): format 2, or encrypted CEKs sent alongside the envelope (API field) with an unusable v1 slot |
| **B. Static group key with rotation** (the current model, applied to a group) | one key per group, sealed for each member device; rotation when a member leaves | no / no (as today) | none: v1 envelope unchanged, `key_epoch` = reserved identifier | one rotation sealed for each remaining device | **yes** for items; new declarations for the group |
| **C. Sender keys** (Signal groups) | each sending device has a chain key distributed peer-to-peer, with a per-message ratchet | forward secrecy yes / no | low | redistribution of all sender keys | yes for the envelope, but per-sender state fits poorly with items kept indefinitely and with history transfer |
| **D. MLS** (RFC 9420, TreeKEM) | group key agreement through a tree, epochs, commits ordered by the delivery service | yes / yes | low | O(log n) | in principle yes (MLS epoch key → reserved identifier), but MLS state per group and per device, and one more library (OpenMLS in Rust) to audit and bind through UniFFI |

Recommendation: **B** to start (two or three people, few groups): it is closest to the existing design and fits
in the reserved range. **A** if arbitrary recipients per item are wanted (preferably in format 2). **D** only if
persistent confidentiality or post-compromise security become requirements, or if groups grow. **C** is not
recommended here: its complexity does not provide post-compromise security and conflicts with the model of
"persistent items + history transfer".

### 4.2 What the current design already facilitates

- **Sealing to a device already exists**: `seal_space_key` (§7.3) performs an ephemeral X25519 exchange, an HKDF
  whose salt binds both public keys, then XChaCha20-Poly1305 with an AAD binding the space, the key identifier and
  the recipient's Ed25519 key (104 bytes). The same scheme carries `HK` (§7.4) with its own labels. Sealing a group
  key (model B) or a CEK (model A) for a device is this same scheme with a new label.
- **`SpaceKeyRing` already indexes keys by a `u32`**: a ring per group, with identifiers in the reserved range,
  reuses the logic (current key, retention, epoch ordering rule).
- **The relay log provides a total order (`seq`)**: this is what a group rotation or MLS commits require.
- **The rule that sealing applies only to a key from one's own trust store** and the framework of signed declarations (label,
  fixed fields, strict signature) extend to person and group certificates.
- **Acknowledgements tolerate unreadable items**: a device that cannot read an item still sends `ack` ("stored",
  `api-v1.md` §4.8). An item without files is therefore purged normally; an item with files that the device does not
  download is purged at the latest at the relay deadline.
- **The encrypted item-state channel** (§8.8.2), whose unknown types are ignored, can carry group-management
  messages without a new endpoint.

### 4.3 What the current design blocks

- a single encrypted CEK slot per envelope (model A);
- `TrustStore`: devices with no notion of person, full trust between all of them (3.1, point 4);
- `KeyRotation`: a single key per space, global revocation;
- unauthenticated `source_device` (3.1, point 5);
- history transfer exports everything the sender can read (harmless in M2, where a device only exports to another
  device of the same person; dangerous in M1);
- the pin is encrypted under the space key and names the `item_id`: in M1, the whole space would learn that a group
  item is pinned; no effect in M2;
- the relay: single-space, `stale_key_epoch`, purge and broadcast to the whole space.

### 4.4 Size per component (architecture M2, model B)

Indicative scale: **small** < 1 week, **medium** 1 to 3 weeks, **large** > 3 weeks,
**redesign** = all layers (core, relay, both apps). Rough estimate, without prototype.

| Component | Nature | Size | Builds on |
|-----------|--------|------|-----------|
| Person identity and verification between people (new declaration, face-to-face verification flow) | new concept | large | declaration framework, pairing channel |
| Group keys: creation, sealing per device, ring per group | new, reuses §7.3 | medium | reserved `key_epoch` range, `SpaceKeyRing` |
| Items under a group key | almost nothing | small | v1 envelope unchanged |
| Membership changes and revocation within a group (signed group rotation) | new | medium to large | ordered log, `KeyRotation` model |
| Group authority rules (who adds, who removes, who deletes everywhere within the group) | new | medium (M2); redesign in M1 | per-space `TrustStore`, unchanged |
| Sender authentication | new | medium (side declaration); large (format 2) | item-state channel |
| Multi-space relay, routing and purging per group | API contract | large | `key_epoch` already in clear |
| Announced capabilities | addition | small | reserved `capabilities` name |
| Retaining unknown fields (2, row 1) | core fix | medium | — |
| `audience`: "Shared with …" display, choice of the device that rings | addition | small | reserved field |
| History transfer of group items | policy | medium | existing mechanism |
| UI of both apps (people, groups, recipients, revocation) | addition | large | core ↔ apps contract |
| MLS variant in place of static group keys | replaces the "group keys" and "membership changes" components | redesign | ordered log, Ed25519 identities |

---

## 5. Verification

Commands run at the repository root after the changes:

| Command | Result |
|---------|--------|
| `cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1` | 12 files `unchanged`, `reservations.json` created, one line added to `SHA256SUMS`; second pass: everything `unchanged` |
| `cargo check --workspace` | OK |
| `cargo test --workspace` | OK: 28 unit tests of `tomi-core`, 14 vector tests (including `reservations` and `manifest_is_complete_and_unchanged`) |
| `cargo fmt --all -- --check` | OK |
| `cargo clippy --workspace --all-targets -- -D warnings` | OK, no warnings |

Changes are limited to `crates/core` and `spec/`; `apps/`, `design/` and `infra/` are not affected.
