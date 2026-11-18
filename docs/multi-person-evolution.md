# Report: Tomi protocol evolvability and opening to a second person

Status: **analysis report**. No code or spec change is requested. Based on a review of
`PROJECT.md`, `spec/format-v1.md`, `spec/api-v1.md`, `spec/pin-contract.md` and `crates/core/src/`
(device.rs, keyring.rs, item.rs, history.rs, pin.rs, crypto.rs).

## Summary

- **The protocol core is designed to evolve additively.** Adding the pin after the fact proves it: a new
  event type that is both signed *and* encrypted, without breaking the existing format.
- **The trust and encryption model is structurally single-user.** There is no notion of "person": the unit
  is the device, and a **single space key** is shared by all of them.
- **Opening to a second person "as one more device" is nearly free, but it gives them the whole vault.**
  A **real** multi-person sharing model (per-person confidentiality, targeted revocation) is a large project,
  partly a cryptographic redesign.

---

## 1. Evolvability: what is easy

- **Three-number versioning**: `format` (in every envelope, negotiated between devices via the minimum of the
  `format_max` values), `api` (REST prefix `/v1`), `key_epoch` (key rotation counter, not a version).
- **Tolerance of unknown values**, applied consistently at every layer:
  - unknown JSON field: ignored without error (the main vehicle for additive changes);
  - unknown enum value: degraded to the safest option (`read_policy` → `after_first_read`);
  - unknown `format`: the item is marked **unreadable, "update required"**, and is never deleted;
  - unknown item state type: opened, decrypted, then ignored (`ItemStateKind::Unknown`).
- **Domain-separated registry of crypto labels** (`tomi/v1/sig|aad|hkdf/...`): a new message type is added by
  giving it a new label, without touching existing ones.
- **Space already reserved** for future state types (`MAX_ITEM_STATE_PADDED_LEN = 2048`).
- **Test vectors**: managed additively (a new vector means no regression; a breaking change means a new
  `v2/` directory, leaving `v1/` untouched). This is not a constraint.

## 2. Evolvability: what is costly

- **Changing the binary structure of an existing message** (fixed lengths, strict parsers) requires
  incrementing `format`. Version negotiation handles this, but it is no longer "one extra field".

---

## 3. Opening sharing to a second person (e.g. a partner)

### Central assumption of the current design
- **Single space key** per epoch: everything is encrypted under one `SpaceKey`. The spec states it:
  "no per-recipient encryption — the system is single-user".
- **The unit is the device, not the person.** `MemberList` is a list of devices; pairing is device-to-device;
  revocation is a global key rotation.
- **No ACL or per-item recipient**: an item goes to the whole space.
- **History transfer "everything included, no checkboxes"**: designed so that a user can recover their own
  thread on a new phone.

### Two scenarios

**A. "Their device becomes one more device" — nearly free, but full trust**
- It is enough to pair it. It receives the current space key.
- Consequence: it sees **all future content, all secrets, all passwords**, with no partition. History
  transfer would also hand it the entire past.
- Removing it = **key rotation for all of the user's devices**; it keeps locally whatever it has already read.
- This is not "sharing with someone"; it extends the user's circle of full trust.

**B. Real multi-person sharing (per-person confidentiality, targeted revocation) — a large project**

| Component | Nature | Scale |
|---|---|---|
| Notion of a "person/member" grouping devices | new concept, absent today | large |
| Key model (moving away from the single space key) | cryptographic redesign | redesign |
| ACL / per-item recipient | new format (`format`+1) | redesign |
| History transfer (filtering by person) | reusable mechanism, policy to be revisited | medium/large |
| MemberList / targeted revocation | depends on the key model | large |
| Pairing (to whom / with which rights) | grafted onto the existing flow | medium |
| Per-person confidential secrets | depends on the key model | large |
| UI of both apps (members, recipient, revocation) | additive but non-trivial | medium |
| Relay | mostly additive (already blind, knows distinct devices) | small/medium |

### Hard constraints that cannot be avoided
The **single space key** and the **absence of recipient/ACL**. As long as they stay in place, adding a
person means giving them the entire space. Changing them requires a `format` v2 (coexistence handled by
version negotiation).

### What already helps for a future multi-person model
- The primitive "encrypt for a specific recipient" **already exists** (transport of the space key at
  pairing time, via X25519): per-recipient encryption would not start from zero.
- The signed trust chain (DeviceCert / MemberList / KeyRotation / Tombstone) is extensible.
- The separation between apps and protocol (frozen contract) limits the impact on the UI.

---

## 4. Recommendation (no change now)

- **Keep the single-user model for v1.** It is coherent and it is the actual need.
- **If multi-person sharing is a medium-term goal**: one low-cost decision to make early — reserve space in
  the format for a future "recipient/scope" field on the item (like the space already reserved for state
  types). Nearly free today, and it would avoid a `v2/` later.
- **Do not build per-recipient keys now**: premature until the need is firm.
