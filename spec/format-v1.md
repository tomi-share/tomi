# Tomi — Format v1 specification

Status: **normative** for `format = 1`. Reference implementation: `crates/core`.
Frozen vectors: `spec/vectors/v1/` (§11). If this document and the vectors disagree,
**the vectors are authoritative** and this document must be corrected (the vectors themselves
are never modified).

The words **MUST**, **MUST NOT**, **SHOULD**, **MAY** have the meaning of MUST, MUST NOT,
SHOULD, MAY in RFC 2119.

---

## 0. Notation

- `||`: byte concatenation.
- `u8(x)`, `u32be(x)`, `u64be(x)`: unsigned integer on 1, 4 or 8 bytes, big-endian.
- `"text"`: the ASCII bytes of the string, **without** terminator. `0x00` is written separately.
- `uuid16(s)`: the 16 bytes of the UUID `s` (RFC 9562, byte order of the text).
- `hex(b)`: lowercase hexadecimal.
- `XC_Seal(k, n, aad, m)`: XChaCha20-Poly1305, output `ciphertext || tag(16)`.
- `XC_Open(k, n, aad, c)`: inverse; fails if the tag does not verify.
- `HKDF(sel, ikm, info, L)`: HKDF-SHA256 (RFC 5869), extract then expand.
  `sel = ∅` means "absent", which is equivalent to 32 zero bytes (RFC 5869 §2.2).
- `Sign(sk, m)` / `Verify(pk, m, sig)`: pure Ed25519 (RFC 8032 §5.1), no context.

---

## 1. Scope and threat model

A **space** = one user and their devices. A single symmetric **space key**,
shared by all member devices, protects all content (PROJECT.md §5). There is no
per-recipient encryption. Two extension points are reserved, with no effect in
v1, for a future multi-recipient sharing model (§5.8).

What this format protects:

| Adversary | Protected? |
|------------|-----------|
| Compromised relay (EC2, database, backups) | Yes: confidentiality and integrity of content; it cannot read, forge, permute items, or substitute a public key during a rotation (D38). |
| Network | Yes (in addition to TLS). |
| Google / FCM | Sees only an empty wake-up (outside this format). |
| Revoked device | No longer reads encrypted items after the rotation. |
| Compromised and active member device | **No** (out of scope, see §12). |

**What the relay sees in clear** (DEV.md invariant 1): `item_id`, emitting `device_id`,
`created_at`, `expires_at`, encrypted size, `key_epoch`, `seq`, plus the bytes of the
envelope header (§5.2: `format`, `key_epoch`, `item_id`, nonces), the devices'
public keys, and the signed statements (§8), which contain only public keys,
identifiers and **sealed** keys. Content type, secret flag, title, text, URL, file names and
MIME types, thumbnails: all are **inside** the ciphertext.
The title of a secret item is therefore encrypted exactly like its content: no title
appears in clear in the envelope or in the metadata sent to the relay (§5.4.5).

For a history transfer (§8.7), the relay additionally sees: the sending device and the
receiving device, the number and size of the blobs, `as_of_seq` and the sealed key.
It does not see the number of items, their types, or the proportion of secrets.

For an item state event (pin, §8.8), the relay sees a random `event_id`, the emitting
device, the epoch, the time and a constant size (320 bytes). It does not see the
`item_id` concerned, nor whether it is a pin or an unpin.

---

## 2. Primitives and libraries

| Use | Primitive | Rust crate (version pinned by `Cargo.lock`) |
|-------|-----------|-----------------------------------------------|
| Authenticated encryption | XChaCha20-Poly1305, random 192-bit nonce (draft-irtf-cfrg-xchacha-03) | `chacha20poly1305` 0.11 (RustCrypto) |
| Key derivation | HKDF-SHA256 (RFC 5869) | `hkdf` 0.13 + `sha2` 0.11 |
| Key agreement | X25519 (RFC 7748) | `x25519-dalek` 3 |
| Signature | Ed25519 (RFC 8032), strict verification | `ed25519-dalek` 3 |
| Hashing | SHA-256 | `sha2` 0.11 |
| Randomness | System CSPRNG | `getrandom` 0.4 |

No primitive is implemented in Tomi.

### 2.1 Why RustCrypto / dalek rather than libsodium

- `chacha20poly1305` provides XChaCha20-Poly1305 conforming to the IETF draft (same construction
  as libsodium `crypto_aead_xchacha20poly1305_ietf`); the draft's §A.3.1 vector
  is verified in `primitives.json`.
- The crate was audited by NCC Group (2020, funded by MobileCoin), with no
  significant finding. Honest caveat: the audit covered an earlier version (0.9); the
  0.11 used here has not, to our knowledge, been re-audited. The external vectors
  (§11) guard against regression.
- The dalek crates are the reference X25519/Ed25519 implementations of the Rust
  ecosystem (Quarkslab audit of `curve25519-dalek` in 2019).
- Pure Rust: no C library to build for each Android (NDK) ABI nor for
  Windows MSVC. `sodiumoxide` has been archived since 2022; `libsodium-sys` would require a
  C toolchain per target.

### 2.2 XChaCha20-Poly1305

32-byte key, 24-byte nonce, 16-byte tag appended to the end of the ciphertext. All nonces
in this format are **randomly drawn** (192 bits: collision is negligible even with a
key shared for years), except the STREAM nonces of attachments (§6.2), which
are derived from a random prefix and a counter under a key specific to each file.

### 2.3 HKDF-SHA256

Output of 32 bytes unless stated otherwise. Each `info` is a unique label from the
registry in §4.2.

### 2.4 Ed25519: strict verification

`Verify` MUST reject: a non-canonical `S` (`S ≥ L`), a public key or `R` of
small order, a public key that does not decompress (semantics of
`verify_strict` in `ed25519-dalek`). The vector `tombstone_non_canonical_s` in
`failures.json` checks the first point. Ed25519 signatures are deterministic: the
signature vectors are therefore reproducible bit for bit.

### 2.5 X25519

Secrets are arbitrary 32 bytes (“clamped” by the primitive). A shared secret
that is entirely zero (public key of small order) MUST be rejected.

---

## 3. Identifiers, encodings and numbers

### 3.1 The three numbers

| Number | Type | Where | Role |
|--------|------|----|------|
| `format` | `u32`, ≥ 1 | first 4 bytes of each envelope, and in the AAD | how to read the item; this document describes `format = 1` |
| `api` | `u32` | REST path `/v1/...` and WS handshake | client ↔ relay contract (outside this document) |
| `key_epoch` | `u32`, from 1 to 2^31 − 1; beyond that: **reserved** (§5.8.2) | bytes 4..8 of the envelope, and in the AAD | which space key sealed the item; +1 at each rotation |

**Invariant for all future formats**: the first 4 bytes of any envelope
are `u32be(format)`. This is the only byte guaranteed stable across formats; it allows an
older reader to recognize a format it cannot read (§9.1).

Implementation constants: `FORMAT_MIN = 1`, `FORMAT_MAX = 1`, `API_VERSION = 1`,
`SPACE_KEY_EPOCH_MAX = 2^31 − 1` (§5.8.2).

### 3.2 Identifiers

| Identifier | Text form | Binary form | Generated by |
|-------------|-------------|---------------|-----------|
| `item_id` | UUID v4, lowercase, with hyphens (36 chars) | `uuid16` | the emitter, at random |
| `space_id` | same | `uuid16` | the first device, when the space is created |
| `blob_id` | same | (text only, in the JSON) | the emitter, per attachment |
| `device_id` | 32 lowercase hexadecimal characters | 16 bytes | derived from the Ed25519 key (§7.1) |

A reader MUST reject a non-canonical UUID (uppercase, without hyphens, braces).

### 3.3 Time

All dates are **milliseconds since the Unix epoch (UTC)**: signed JSON integer
in the payload, `u64be` in signed statements.

---

## 4. Key hierarchy and derivation

### 4.1 Keys

| Key | Size | Origin | Single use |
|------|--------|---------|--------------|
| `SK_e` space key of epoch `e` | 32 | random, at creation (e = 1) or at each rotation | **never used directly**: only as HKDF IKM |
| `K_wrap_e` | 32 | `HKDF(∅, SK_e, "tomi/v1/hkdf/cek-wrap", 32)` | encrypt the CEKs |
| `CEK` content key | 32 | random, one per item | encrypt the payload of a single item |
| `FK` file key | 32 | random, one per attachment | encrypt the chunks of a single file |
| `ed_sk` / `ed_pk` | 32 / 32 | random at installation | sign statements (§8) and authenticate to the relay |
| `x_sk` / `x_pk` | 32 / 32 | random at installation | receive the space key (§7.3) |
| `K_transport` | 32 | `HKDF(E_pub ‖ R_xpk, X25519(e, R_xpk), "tomi/v1/hkdf/key-transport", 32)` | seal a space key for a device |
| `HK` history transfer key | 32 | random, one per transfer | encrypt the index shares of a single transfer (§8.7) |
| `K_hist` | 32 | `HKDF(E_pub ‖ R_xpk, X25519(e, R_xpk), "tomi/v1/hkdf/history-transport", 32)` | seal `HK` for the recipient (§7.4) |
| `FK'` | 32 | random, one per transferred file | re-encrypt a file for a transfer (§8.7.3); never equal to the original `FK` |
| `LSK` local key | 32 | random, one per installation, in the OS keystore | re-seal open items locally (§7.2); never derived from a space key, never transmitted |
| `K_state_e` | 32 | `HKDF(∅, SK_e, "tomi/v1/hkdf/item-state", 32)` | encrypt item state events (pin, §8.8.2) |

The CEK, `FK`, `FK'`, `HK` and `LSK` are random keys; CEK, `FK`, `FK'` and `HK` are
used for a single operation (a payload, a file, a transfer) and are therefore not
derived.

### 4.2 Label registry (domain separation)

Each label serves **a single** purpose. Any new use of a key MUST
use a new label; no raw key serves two purposes.

| Label | Type | Section |
|-----------|------|---------|
| `tomi/v1/hkdf/cek-wrap` | HKDF info | §4.1 |
| `tomi/v1/hkdf/key-transport` | HKDF info | §7.3 |
| `tomi/v1/aad/cek-wrap` | AAD | §5.1 |
| `tomi/v1/aad/payload` | AAD | §5.1 |
| `tomi/v1/aad/chunk` | AAD | §6.3 |
| `tomi/v1/aad/key-transport` | AAD | §7.3 |
| `tomi/v1/device-id` | hash prefix | §7.1 |
| `tomi/v1/sig/device-cert` | signed prefix | §8.2 |
| `tomi/v1/sig/member-list` | signed prefix | §8.2 |
| `tomi/v1/sig/key-rotation` | signed prefix | §8.3 |
| `tomi/v1/sig/tombstone` | signed prefix | §8.5 |
| `tomi/v1/hkdf/history-transport` | HKDF info | §7.4 |
| `tomi/v1/aad/history-transport` | AAD | §7.4 |
| `tomi/v1/aad/history-chunk` | AAD | §8.7.2 |
| `tomi/v1/sig/history-transfer` | signed prefix | §8.7.1 |
| `tomi/v1/aad/local-item` | AAD (local storage, never transmitted) | §7.2 |
| `tomi/v1/hkdf/item-state` | HKDF info | §8.8.2 |
| `tomi/v1/aad/item-state` | AAD | §8.8.2 |
| `tomi/v1/sig/pin` | signed prefix (and content type of an item state) | §8.8.1 |
| `tomi/v1/sig/archive` | signed prefix (and content type of an item state) | §8.9.1 |
| `tomi/v1/sig/attachment-tombstone` | signed prefix | §8.10.1 |
| `tomi/v1/sig/relay-auth` | **reserved** for the relay authentication challenge (API spec) | — |

Why tombstones are not "signed" with a subkey of the space key:
the relay must be able to verify a tombstone in order to purge the item, and it holds no
symmetric key. An Ed25519 signature from the device is verifiable by everyone, relay
included, and identifies the authoring device. No symmetric subkey is therefore derived
for tombstones or for the other statements. The pin (§8.8) is the opposite case:
the relay does not need to read it, so it is signed **and then** encrypted under `K_state_e`.

The `v1` in the labels designates the family of this document; the `format` number is bound
separately in each AAD.

---

## 5. Item envelope (format 1)

### 5.1 Canonical AAD

AADs are fixed-length byte strings, with no separator other than the `0x00`
that follows the label (the following fields all have a fixed length, so the encoding is
unambiguous):

```
AAD_wrap    = "tomi/v1/aad/cek-wrap" || 0x00 || u32be(format) || u32be(key_epoch) || uuid16(item_id)
AAD_payload = "tomi/v1/aad/payload"  || 0x00 || u32be(format) || u32be(key_epoch) || uuid16(item_id)
```

Example (`aad.json`, format 1, epoch 1, item `0e7b5a3c-1d2f-4a6b-8c9d-0e1f2a3b4c5d`):

```
AAD_wrap = 746f6d692f76312f6161642f63656b2d77726170   "tomi/v1/aad/cek-wrap"
           00                                               separator
           00000001                                         format
           00000001                                         key_epoch
           0e7b5a3c1d2f4a6b8c9d0e1f2a3b4c5d                 item_id
```

The AAD binds the ciphertext to its `item_id`, its epoch and its format: the relay cannot
swap two blobs, change the announced epoch, or have an item read in another
format (vectors `aad_item_id_altered`, `aad_epoch_altered`).

### 5.2 Binary layout

| Offset | Length | Field |
|----------|----------|-------|
| 0 | 4 | `u32be(format)` = `00000001` |
| 4 | 4 | `u32be(key_epoch)` |
| 8 | 16 | `uuid16(item_id)` |
| 24 | 24 | `wrap_nonce` (random) |
| 48 | 48 | `wrapped_cek = XC_Seal(K_wrap_e, wrap_nonce, AAD_wrap, CEK)` (32 + 16) |
| 96 | 24 | `payload_nonce` (random) |
| 120 | n | `payload_ct = XC_Seal(CEK, payload_nonce, AAD_payload, padded_payload)` |

Total length = `120 + len(padded_payload) + 16`, with `len(padded_payload)` a multiple
of 256, between 256 and 1 048 576. Example, start of the envelope `text_simple`:

```
00000001 00000001 0e7b5a3c1d2f4a6b8c9d0e1f2a3b4c5d
606162636465666768696a6b6c6d6e6f7071727374757677            wrap_nonce
eedbb379d4ec06bbb6d33b55ba2aa45f320774d6786ecc488f57e7409a7c3e50
ca6127ae078b3d78626c5c886d94467c                            wrapped_cek
808182838485868788898a8b8c8d8e8f9091929394959697            payload_nonce
...                                                          payload_ct (512 + 16 bytes)
```

### 5.3 Payload padding

`padded_payload = json || 0x80 || 0x00*`, so that the length is the smallest
multiple of 256 strictly greater than `len(json)` (ISO/IEC 7816-4). A 255-byte
JSON yields 256 bytes; a 256-byte JSON yields 512.

Purpose: do not reveal to the relay the exact length of a secret (a password of 8 or of
20 characters produces the same encrypted size).

Padding removal (reader): the length MUST be a multiple of 256; the last
non-zero byte MUST be `0x80`; it MUST be followed by at most 255 zeros (minimal
padding). Otherwise: error `malformed_envelope`.

### 5.4 JSON payload

JSON (RFC 8259) encoded in strict UTF-8, without BOM, with one object at the root. **A key
repeated within the same object MUST cause the payload to be rejected** (`malformed_payload`),
in any object at any depth, whether or not the key is known (including a reserved field,
§5.8), and even if both values are identical. Keys are compared after decoding escapes: `"audience"` and `"\u0061udience"` are the same
key. The same key in two different objects is not a duplicate. Without this rule, two
implementations could retain different values for the same field. A number that the reference
reader cannot represent (outside the range of a 64-bit float) also makes the payload malformed,
even in an unknown field. **Field order and whitespace are not normative**: a reader never
re-serializes the JSON to verify it (integrity comes from the AEAD). Vectors: `payload-extensions.json`.

| Field | JSON type | Required | Default | Meaning |
|-------|-----------|-------------|--------|------|
| `id` | UUID string | yes | | MUST equal the `item_id` of the header |
| `source_device` | string (32 hex) | yes | | `device_id` of the emitter |
| `created_at` | integer (ms) | yes | | capture date |
| `expires_at` | integer (ms) or `null` | no | none (never) | expiration **chosen by the user**; absent by default, secrets included (§5.7) |
| `read_policy` | string | no | `"until_expiry"` | §5.4.2 |
| `secret` | boolean | no | `false` | secret content: masked, sensitive clipboard (DEV.md inv. 4) |
| `title` | string or `null` | no | | title; for a secret, **is part of the secret** (§5.4.5) |
| `text` | object or `null` | no | | §5.4.1 |
| `url` | string or `null` | no | | link |
| `attachments` | array | no | `[]` | §5.4.3; the index in this array is the `attachment_index` of §6 |
| `audience` | object or `null` | **reserved** | absent = whole space | recipients of a future multi-recipient sharing model (§5.8.1); never written in v1, ignored by a v1 reader whatever its value; **no cryptographic effect** |

An emitter SHOULD fill in at least one of `text`, `url`, `attachments`. A reader
accepts an empty item.

#### 5.4.1 `text`

| Field | Type | Required | Meaning |
|-------|------|-------------|------|
| `plain` | string | yes | plain text, always present (display fallback) |
| `html` | string | no | HTML as captured; sanitized and rendered in a sandbox **at display time** (DEV.md inv. 6) |
| `markdown` | string | no | Markdown representation |
| `source_format` | `"plain"` \| `"markdown"` \| `"html"` | no (`"plain"`) | default rendering only; the receiver chooses when viewing |

#### 5.4.2 `read_policy`

- `"until_expiry"`: the item stays on the device until `expires_at` (or forever).
- `"after_first_read"`: erased from the device after the first explicit reveal/copy.
  The client chooses this value only on explicit request from the user,
  never by default, even for a secret (PROJECT.md §7, decision D1).
- **Unknown value**: MUST be treated as `"after_first_read"` (the most
  restrictive). An unknown value of `source_format` is treated as `"plain"`.

#### 5.4.3 `attachments[i]`

| Field | Type | Required | Meaning |
|-------|------|-------------|------|
| `blob_id` | UUID string | yes | identifier of the encrypted blob at the relay |
| `name` | string | yes | file name (never seen by the relay) |
| `mime` | string | yes | MIME type |
| `size` | integer ≥ 0 | yes | plaintext size, in bytes; sets the number of chunks |
| `key` | base64 (RFC 4648 §4, with `=`) | yes | `FK`, exactly 32 bytes |
| `nonce_prefix` | base64 | yes | exactly 19 bytes |
| `thumbnail` | object `{mime, data}` | no | thumbnail (a few KB), `data` in base64 (decision D20) |
| `removed` | boolean | no | **local copy only**: `true` = attachment removed, entry emptied but kept in place (§8.10.4); never written into an envelope |

The base64 MUST be canonical (zero padding bits); otherwise `malformed_payload`.

#### 5.4.4 Unknown fields and values

An unknown JSON field, at the root or in a nested object, MUST be **ignored without
error**: this is how additive changes remain within `format = 1`
(PROJECT.md §6). A required field that is missing or of the wrong type yields
`malformed_payload`. An explicit `null` on an optional field is equivalent to its absence.
A **reserved** field (§5.8) is an unknown field for a v1 reader: it is ignored, whatever
its value.

**Unknown fields are preserved.** Ignoring a field for the logic does not mean losing it:
a device that re-seals an open item (local storage, §7.2; new envelope under a space key)
or exports it in a history transfer (§8.7.3) MUST keep **all** unknown fields, at any depth,
with their values; it MUST NOT rebuild the payload from only the fields it knows. Otherwise an
additive field written by an up-to-date device would be erased by the first device not yet
up to date that touches the item, permanently for its local copy. The reference core keeps the
received payload **byte for byte** (`Item::raw_payload`) and rewrites it as is; only the raw
line breaks between two tokens are removed in the history index, which is in
JSON Lines (§8.7.3). This preservation holds as long as the item is not modified: an item
whose known fields have been changed can no longer be sealed with its original bytes,
the client must explicitly rewrite it (and accept the loss of unknown fields). An
item that the device creates itself has no original bytes: its payload is serialized
from its fields. Vectors `preserved` of `payload-extensions.json`.

#### 5.4.5 Title of a secret

For an item with `secret: true`, the `title` is part of the secret. It is encrypted in the
payload in the same way as `text` and `url`: the format provides no title, preview or
clear label, neither in the envelope header (§5.2), nor in the metadata provided
to the relay (§5.7), nor in a transfer manifest (§8.7.1). A client:

- MUST NOT display the title of a secret without explicit reveal (feed card,
  notification, recent list, lock screen): it displays a generic label;
- MUST NOT index a secret for local search, neither its title nor its content;
- re-seals the title with the rest of the payload in its local storage (§7.2).

`crates/core` exposes `Item::visible_title()` (`None` for a secret) and
`Item::searchable_text()` (`None` for a secret); the `payloads` vectors of
`history-transfer.json` fix this behavior.

### 5.5 Sealing (emitter)

1. Choose `item_id` (random UUID v4) and the current space key `SK_e`.
2. Serialize the JSON (§5.4), pad it (§5.3). Refuse if `len(padded) > 1 MiB`.
3. Draw `CEK` (32), `wrap_nonce` (24), `payload_nonce` (24) at random.
4. `K_wrap_e = HKDF(∅, SK_e, "tomi/v1/hkdf/cek-wrap", 32)`.
5. `wrapped_cek = XC_Seal(K_wrap_e, wrap_nonce, AAD_wrap, CEK)`.
6. `payload_ct = XC_Seal(CEK, payload_nonce, AAD_payload, padded_payload)`.
7. Assemble according to §5.2. Erase `CEK` from memory.

The emitter writes to the **effective version of the space** (PROJECT.md §6). With
`FORMAT_MAX = 1`, this is always 1.

### 5.6 Opening (reader)

Inputs: the envelope, the `item_id` under which the relay listed it, the known space
keys.

1. Fewer than 4 bytes → `malformed_envelope`. Read `format`; if it is not in
   `[FORMAT_MIN, FORMAT_MAX]` → `unsupported_format` (§9.1).
2. Check the length (§5.2) → otherwise `malformed_envelope`.
3. `item_id` in the header ≠ announced `item_id` → `item_id_mismatch`.
4. Choose `SK_e` for `key_epoch`; unknown epoch → the item remains "unreadable"
   until the corresponding rotation is received (`key_epoch_mismatch` if the key
   provided belongs to another epoch).
5. `CEK = XC_Open(K_wrap_e, wrap_nonce, AAD_wrap, wrapped_cek)` → otherwise `decryption_failed`.
6. `padded = XC_Open(CEK, payload_nonce, AAD_payload, payload_ct)` → otherwise `decryption_failed`.
7. Remove the padding (§5.3), read the JSON (§5.4); `id` ≠ `item_id` → `item_id_mismatch`.

### 5.7 Metadata visible to the relay and authoritative values

`created_at`, `expires_at` and the emitting `device_id` are provided to the relay in clear, next
to the envelope, for purge and sorting. These copies are **not authenticated**.
Clients MUST use the values **from the decrypted payload**. At worst, the relay may
delay or advance its own purge (availability, §12).

**Two distinct dates** (decision D1). A single duration remains visible to the user
(decision D27), but the format distinguishes two:

| Date | Where | Who uses it | Value |
|------|----|---------------|--------|
| User expiration | `expires_at` of the encrypted payload (§5.4), optional, authentic | devices: erasure of the local copy | **absent by default** (never), secrets included; otherwise explicitly chosen at capture |
| Relay deadline | `expires_at` in clear in `POST /v1/items` (`api-v1.md` §4.8), always present, not authenticated | the relay only: purge of an **undelivered** item | `min(user expiration, estimated_server_time + max_ttl_secs − margin)` |

The margin is `min(60 s, max_ttl_secs / 2)` (`relay_expires_at_ms` in `crates/core`);
it absorbs an estimation error of the relay's clock. The relay therefore does not learn the
actual expiration when it is absent or exceeds `max_ttl_secs`. Clients MUST NOT erase
a local copy at the relay deadline. This is data hygiene, not a cryptographic guarantee of erasure.

### 5.8 Reserved extension points for multi-recipient sharing (unused in v1)

Format 1 remains single-user (§1): every item goes to the whole space. So that a future
sharing with other people, or with a subset of the devices, can keep the
envelope layout and the payload JSON of `format = 1`, two extension points
are reserved now. They are **not used** in v1: no recipient, no access control list,
no notion of a person, no per-recipient encryption.
They change **no existing byte**: no published vector is modified; the additive file
`reservations.json` fixes the behavior of a v1 reader (§11). The document
`docs/protocol-extensibility.md` explains these choices and what cannot be reserved.

#### 5.8.1 `audience` field of the payload

**Name.** `audience` says "who the item is intended for" without promising who can read it
(same meaning as the `aud` claim of JWT tokens, RFC 7519 §4.1.3). Rejected:
`recipients`, which already designates the devices receiving a key sealed in a
`KeyRotation` (§8.3) and suggests per-recipient encryption; `scope`, which is already the
scope of a tombstone (§8.5).

**Reserved form** (for future versions; a v1 reader does not verify it):

```json
"audience": {
  "to": [
    {"type": "device", "id": "d2daf28457ef3d14c9c58d6f488e1844"},
    {"type": "member", "id": "0f1e2d3c4b5a69788796a5b4c3d2e1f0"}
  ]
}
```

| Field | Type | Reserved meaning |
|-------|------|--------------|
| `audience` | object or `null` | absent or `null`: **the whole space**, i.e. exactly the v1 behavior. This meaning will never change. |
| `audience.to` | array of entries | one or more recipients (hence the link with multi-recipient sharing). Absent or empty: same as absent `audience`. |
| `to[i].type` | string | `device`: a device, `id` = `device_id` (§3.2). `member`: a person, `id` defined together with the future person model (reserved). Any other value is reserved for future versions. |
| `to[i].id` | string | identifier in the namespace of `type` |

An object rather than a bare array, so that fields can be added next to `to` (delivery
mode, linked key, etc.) without changing the type of `audience`. Typed entries rather than
prefixed strings, so that nothing has to be split and an entry can
gain fields.

**v1 rules (normative).**

1. A v1 emitter MUST NOT write `audience`: its semantics are not defined, and a value
   written today would be interpreted by future versions.
2. A v1 reader MUST ignore `audience`, whatever its value (object, empty list,
   `null`, unexpected JSON type, entry of an unknown type, unknown fields): it MUST NOT
   reject the payload because of this field (no `malformed_payload`) and treats the item
   exactly as if it were absent: broadcast to the whole space, same display, same
   notifications, same masking of a secret and its title (§5.4.5), same history
   transfer (§8.7). This is the unknown-field rule (§5.4.4), applied by the
   reference parser (`WireItem` without `deny_unknown_fields`, `crates/core/src/item.rs`);
   vectors `payload` and `envelope` of `reservations.json`. The field is also subject to the
   repeated-key rule (§5.4): two `audience` keys in the same object yield
   `malformed_payload`, and a v1 device **preserves** it (§5.4.4) when it re-seals or
   exports the item.
3. A future version that gives meaning to `audience` keeps: absent = whole space; retains only the entries whose `type` it understands and, if none remain, treats
   the item as without `audience`; extends the form only by adding optional fields.

**No cryptographic effect in v1: this is NOT a confidentiality boundary.**
The payload, `audience` included, is encrypted under the space key (§5.5) like
everything else: **every member device decrypts and reads the content, whether or not it appears in
`audience`**. Moreover, a v1 reader ignores the field; the field is not authenticated
(a member can forge an item, §12); and a v1 device does not understand it: it preserves it
without interpreting it when it re-seals the item locally (§7.2) or exports it in a history
transfer (§8.7.3), but takes no account of it for display or for
notifications. An emitter, even a future one, MUST NOT
therefore use `audience` to restrict the reading of an item encrypted under the space
key. A read restriction requires a key that non-recipients do not have
(§5.8.2); `audience` then only describes, inside the ciphertext, who the item is
intended for (a "Shared with …" display, the choice of the device that rings). The relay does not see
this field: it is inside the ciphertext (§1).

#### 5.8.2 Reserved `key_epoch` range: identifiers of out-of-space keys

The envelope (§5.2) contains only one wrapped CEK, under the key designated by `key_epoch`.
Without changing this layout, the only way to restrict reading of an item to a
subset of the devices is therefore to seal it under **another key** than the space key
(a group or recipient key), designated by a `key_epoch` value that cannot be
a space epoch. These values are reserved now:

| `key_epoch` values | Meaning |
|------------------------|------|
| `0` | invalid (unchanged) |
| `1` to `2^31 − 1` (`0x7FFFFFFF`, `SPACE_KEY_EPOCH_MAX`) | epochs of the space key (§4.1, §8.3) |
| `2^31` to `2^32 − 1` (`0x80000000` to `0xFFFFFFFF`) | **reserved**: identifiers of out-of-space keys, not defined in v1 |

v1 rules (normative):

1. A v1 device MUST NOT create, sign, accept or keep a space key whose epoch is
   reserved. A `KeyRotation` (§8.3) or a `MemberList` (§8.2) carrying a reserved
   epoch is refused **at decoding**, before signature verification
   (`malformed_statement`, vectors `statements` of `reservations.json`). With +1 per
   rotation, a space epoch never reaches `2^31`: the rule removes nothing useful.
2. An item (§5.6) or a state event (§8.8.2) whose header announces a reserved value
   is never opened with a space key. The value is greater than the current epoch:
   the item is **KEY_PENDING** (`KeyPending`, §9.4), kept raw, never deleted, and never
   `UNREADABLE_SUSPECT_EPOCH` (a reserved value is never an epoch replaced by a
   rotation). This is the existing behavior of `SpaceKeyRing::key_for_item`; vectors
   `items` and `item_states` of `reservations.json`.
3. A v1 relay refuses such a deposit: the value is not the current epoch
   (`409 stale_key_epoch`, `api-v1.md` §4.8). A v1 device therefore does not receive it from
   an honest v1 relay.
4. Format 1 defines neither these keys, nor their derivation, nor their transport, nor their
   revocation. A version that defines them follows the registry rule (§4.2) and must also
   know which devices can read them (announced capabilities, `api-v1.md` §3.3): a
   recipient v1 device would keep the item KEY_PENDING until it is updated.

The header bytes remain those of §5.2: an item sealed under such a key would remain an
envelope of `format = 1`.

#### 5.8.3 Other reserved names

| Where | Reserved name | v1 behavior |
|----|-------------|-----------------|
| History transfer index (§8.7.3) | line `{"kind": "member", …}`: people known to the sender, modeled on the `device` lines | ignored (unknown `kind`); never written |
| `item` line of the index | nothing: `audience` travels in `payload` | — |
| API `ClientInfo` and `DeviceRecord` | field `capabilities` (list of strings) | not sent; ignored by a v1 relay (`api-v1.md` §3.3, §4.3) |

#### 5.8.4 What does not need reservation

- **Labels** (§4.2): the registry only grows. A new statement
  (person certificate, rotation of a group key) receives a new label without
  touching the existing ones.
- **Item state types** (§8.8.2): a content of an unknown type has no effect (kept raw and replayed after an update, §8.8.2 step 6), and the room
  (up to 2,048 padded bytes) is already provided.
- **Event log types and API error codes**: an unknown type is ignored,
  an unknown code is handled according to its HTTP class (`api-v1.md` §2.5, §4.10).
- **`HistoryTransfer` manifest (§8.7.1) and other binary statements**: their layout
  is fixed and excess bytes are refused. A reserved field would serve no purpose there; an
  extension goes through a new statement type (new label) or through the
  encrypted index, which is additive JSON.

---

## 6. Attachments: STREAM construction

### 6.1 Parameters

- Plaintext chunk size: **65 536 bytes** (64 KiB), fixed in format 1.
- For a file of `size` bytes: `N = max(1, ceil(size / 65536))` chunks.
  Chunk `i` (0 ≤ i < N) contains the bytes `[i·65536, min((i+1)·65536, size))`.
  All chunks except the last are exactly 65 536 bytes; the last is from 1 to
  65 536 bytes, or 0 if and only if the file is empty (`N = 1`). A file whose
  size is a multiple of 65 536 has **no** final empty chunk.
- `FK` (32) and `nonce_prefix` (19): fresh randomness per attachment, transmitted in the payload.

### 6.2 Nonce

```
nonce_i = nonce_prefix (19) || u32be(i) || flag      flag = 0x01 if i = N-1, otherwise 0x00
```

Example (`item-attachment.json`, attachment 2, prefix `3d3e…4f`, N = 2):

```
chunk 0 : 3d3e3f404142434445464748494a4b4c4d4e4f 00000000 00
chunk 1 : 3d3e3f404142434445464748494a4b4c4d4e4f 00000001 01   ← last
```

The counter prevents reordering and duplication; the final flag prevents silent
truncation and extension: an attacker who removes the last chunk leaves a
stream with no chunk marked final; a final chunk re-encrypted without the flag does not authenticate
(vectors `truncated_last_chunk`, `last_flag_missing`, `counter_mismatch`).

### 6.3 Chunk AAD

```
AAD_chunk = "tomi/v1/aad/chunk" || 0x00 || u32be(format) || uuid16(item_id) || u32be(attachment_index)
```

The same AAD is used for all chunks of an attachment. It binds the file to its item
and to its position in `attachments`. `key_epoch` deliberately does not appear: `FK` is
transmitted in the payload, which itself is bound to the epoch; and a file encrypted just before a
rotation thus remains valid if the item is sealed just after.

### 6.4 Blob

```
ct_i = XC_Seal(FK, nonce_i, AAD_chunk, chunk_i)            (len(chunk_i) + 16 bytes)
blob = ct_0 || ct_1 || ... || ct_{N-1}
len(blob) = size + 16·N
```

The relay stores the blob under `blob_id`; it sees its size, and therefore the approximate
size of the file (no padding of files in v1, §12).

### 6.5 Reading

The reader knows `size` (authenticated payload), hence `N` and the expected length of
each `ct_i` (65 552 except the last). It decrypts the chunks in order with
`nonce_i`. It MUST:

- fail (`decryption_failed`) as soon as a chunk does not authenticate;
- fail (`stream_trailing_data`) if bytes remain after chunk `N-1`;
- fail (`stream_truncated`) if the stream ends before chunk `N-1`;
- write the plaintext to a **temporary file** and expose it to the user
  (or to another app) only after the last chunk has been verified.

Memory required: one chunk (64 KiB) plus the read buffer, whatever the
size of the file.

### 6.6 Limits

The format admits up to `2^32` chunks. The product limit (25 MB per file,
PROJECT.md §5) is a client setting, not a format setting.

---

## 7. Device identity and transport of the space key

### 7.1 Keys and identifier

At installation, each device draws at random an Ed25519 seed (32 bytes) and an
X25519 secret (32 bytes). These are two **independent** keys (no Ed25519 → X25519
conversion).

```
device_id = hex( SHA-256("tomi/v1/device-id" || 0x00 || ed_pk)[0..16] )
```

Example (device A of `device.json`): `ed_pk = 4fd099cc…65c4` →
`device_id = e983270f3ea7baa8688d15c08d447559`.

### 7.2 Storage

The secrets (`ed` seed, `x_sk`, space keys, `LSK`, `HK` of a transfer task in
progress) MUST be kept in the OS keystore (Android Keystore, DPAPI /
Windows Credential Manager). They MUST never appear in a log, a URL,
a notification or an FCM message. Outside the binary scope of this document; recalled here
because the API exposes these bytes for persistence.

**Local storage independent of epochs (decision D1).** An item received and successfully
opened is re-sealed under the local key `LSK` (random, specific to the device, never
derived from a space key, never transmitted). The raw envelope is kept only for an
unreadable item (§9.4: `UpdateRequired`, `KeyPending`, `SuspectEpoch`, `Undecryptable`),
so that it can be reopened. Files remain encrypted under their `FK`, which is in the
re-sealed payload. Reference layout of a local record (specific to the
device, not covered by the vectors):

```
AAD_local = "tomi/v1/aad/local-item" || 0x00 || u8(version = 1) || uuid16(item_id)
record    = u8(1) || nonce (24) || XC_Seal(LSK, nonce, AAD_local, payload JSON)   // 41 bytes + JSON
```

Thus a device no longer needs to keep its old space keys in order to reread its
feed (§8.3, end).

The "payload JSON" of the record is the payload **received, byte for byte** (§5.4.4),
not a serialization of its known fields: an unknown field (for example an additive field
from a more recent version) survives re-sealing. An item created by the device itself
has no original bytes and is serialized from its fields.

**Files at rest.** An attachment is kept on disk only **encrypted**: the blob as it is on
the relay, under its `FK` (in the re-sealed payload), including at the sender, which keeps no
plaintext copy; for an imported item, under the transfer key `FK'` (§8.7), kept sealed in the
database. Decryption happens on demand, into a local cache purged when the item is deleted, when it
expires, when the application goes to the background, and at startup. A file is stored only once
it is fully authenticated (§6.5). Reference layout in `tomi-core`:
`files/{item_id}_{idx}.enc` (encrypted), `files/cache/{item_id}_{idx}` (plaintext, temporary).

**Plaintext metadata accepted in the local database.** Without the OS envelope key,
the database reveals no secret (device keys, `LSK`, space keys, `HK`, `FK'` are
sealed) and no content (title, text, URL, file names and contents are under `LSK` or
`FK`). What remains readable, because the feed, the outgoing queue and maintenance need it
without decrypting: the identifiers (items, blobs, events, transfers, devices); the
dates (creation, reception, relay deadline, chosen expiration, reading, pin,
deletion); the `secret` flag, the `read_policy`, the read / kept / pinned / sending states, the key
epochs, the size and rank of attachments; the names, platforms
and public keys of devices, the trust store (public) and the cursor; the bytes
of a pin statement waiting to be sent (`PinStatement`: item, decision, date,
signer, signature); the clear headers of pending envelopes and tombstones;
diagnostic codes without content. Part of this (identifiers, dates, sizes, epochs, device
names) is already visible to the relay (§5.7); the rest (`secret` flag, read policy,
read / pinned states, pin statements) does not leave the private storage of the
application, and is not protected against whoever reads that storage.

### 7.3 Sealed space key

To transmit `SK_e` to a device R (`R_edpk`, `R_xpk`):

```
e, E_pub          = ephemeral X25519 pair, fresh randomness
shared            = X25519(e, R_xpk)              rejected if 32 zero bytes
K_transport       = HKDF(E_pub || R_xpk, shared, "tomi/v1/hkdf/key-transport", 32)
AAD_transport     = "tomi/v1/aad/key-transport" || 0x00 || uuid16(space_id) || u32be(e) || R_edpk
nonce             = random (24)
sealed            = E_pub (32) || nonce (24) || XC_Seal(K_transport, nonce, AAD_transport, SK_e) (48)
```

`sealed` is **104 bytes**. The AAD binds the sealed key to the space, to the epoch and to the
**signature identity** of the recipient. The recipient recomputes `shared` with
`x_sk`, derives `K_transport`, and opens it with its own `ed_pk` in the AAD.

Decision D38: the device that seals MUST take `R_xpk` from **its own trust store**
(§8.4), never from a relay response.

### 7.4 Sealed history transfer key

For a history transfer (§8.7) to R, the sender draws `HK` (32 random bytes,
one transfer only) and seals it as in §7.3, with other labels and the
`transfer_id` in place of the epoch:

```
shared    = X25519(e, R_xpk)                       rejected if 32 zero bytes
K_hist    = HKDF(E_pub || R_xpk, shared, "tomi/v1/hkdf/history-transport", 32)
AAD_hist_transport = "tomi/v1/aad/history-transport" || 0x00 || uuid16(space_id)
                     || uuid16(transfer_id) || R_edpk
sealed_hk = E_pub (32) || nonce (24) || XC_Seal(K_hist, nonce, AAD_hist_transport, HK) (48)   // 104 bytes
```

`AAD_hist_transport` is 30 + 16 + 16 + 32 = 94 bytes. Decision D38 applies: `R_xpk`
comes from the sender's trust store. No space key, old or current, is
transmitted this way.

---

## 8. Signed statements and chain of trust

### 8.1 Common rules

- Wire form: `TBS || signature` (64 bytes), where `TBS` (to-be-signed) starts with
  the label of §4.2 followed by `0x00`, then the fields in the order indicated.
- `signature = Sign(ed_sk_signer, TBS)`. The signer's public key is **inside**
  the TBS: the statement is verifiable on its own.
- Lists are preceded by `u32be(n)`, `n ≤ 1024`, sorted by increasing Ed25519 key
  (byte-by-byte comparison), with no duplicates. A reader MUST reject an unsorted or
  duplicated list (`malformed_statement`) and excess bytes.
- Verifying a signature is not enough: the trust decision is in §8.4.

### 8.2 Pairing: device certificate and member list

**DeviceCert** (the issuer vouches for a new device):

```
TBS = "tomi/v1/sig/device-cert" || 0x00 || uuid16(space_id)
      || subject_ed_pk (32) || subject_x_pk (32) || u64be(issued_at_ms) || issuer_ed_pk (32)
```

208 bytes on the wire. A self-signed certificate (`issuer = subject`) only proves
possession of the key (useful to the relay for the founding device with the
installation token); it gives **no** trust to a device.

**MemberList** (snapshot of the members for a new device):

```
TBS = "tomi/v1/sig/member-list" || 0x00 || uuid16(space_id) || u32be(key_epoch)
      || u64be(issued_at_ms) || issuer_ed_pk || u32be(n) || n × (ed_pk (32) || x_pk (32))
```

A reserved `key_epoch` (> 2^31 − 1, §5.8.2) is refused at decoding (`malformed_statement`).

**Pairing sequence** (A, a member, invites N) — the QR / short code + PAKE channel is
specified separately (§13); it gives each side the **authentic** public keys of the other:

1. A and N exchange `(ed_pk, x_pk)` over the PAKE channel. Each adds the other as a
   peer verified face to face.
2. A sends to N: `space_id`, `key_epoch`, `sealed` (§7.3, for N), `DeviceCert(N)` signed
   by A, `MemberList` signed by A.
3. N creates its store (`space_id`, epoch, itself), adds A, accepts the `MemberList`
   (signed by A, whom it trusts), opens `sealed`.
4. A publishes `DeviceCert(N)` via the relay; the relay registers `N` as an
   authorized device; the other members accept the certificate because they trust A.

### 8.3 Space key rotation

**KeyRotation**:

```
TBS = "tomi/v1/sig/key-rotation" || 0x00 || uuid16(space_id) || u32be(new_epoch)
      || u64be(issued_at_ms) || issuer_ed_pk
      || u32be(n_rev) || n_rev × revoked_ed_pk (32)
      || u32be(n_rec) || n_rec × (recipient_ed_pk (32) || sealed (104))
```

`new_epoch` is a space epoch: a reserved value (> 2^31 − 1, §5.8.2) is refused
at decoding (`malformed_statement`), before signature verification.

**Creation** (device I, e.g. to revoke a lost device):

1. `new_epoch = key_epoch + 1`; draw `SK_new` at random.
2. Recipients = all members of I's store **minus** the revoked ones, I included.
   I cannot revoke itself; only a member is revoked.
3. For each recipient, seal `SK_new` (§7.3) with the `x_pk` **taken from I's store**.
4. Sort, sign, publish via the relay. The relay MUST refuse a rotation whose
   `new_epoch` ≠ its current epoch + 1 (one rotation per epoch) and remove the
   revoked devices from its authorized devices.

**Verification by a device M** (all steps, in order; on failure the
store is not modified):

1. `space_id` = M's → otherwise `space_mismatch`.
2. `issuer_ed_pk` is a trusted member of M and not revoked → otherwise `untrusted_signer`
   / `revoked_device`.
3. Valid signature (§2.4) → otherwise `invalid_signature`.
4. `new_epoch = M's current epoch + 1` → otherwise `epoch_out_of_order`. An offline device
   replays the missed rotations in `seq` order.
5. The issuer is not in `revoked`; no recipient is in `revoked`
   → otherwise `malformed_statement`.
6. If M is in `revoked`: M has left the space; it MUST erase its space keys
   and inform the user.
7. Otherwise M MUST find its entry (otherwise `not_a_recipient`) and open `sealed` with
   `space_id`, `new_epoch` and its own `ed_pk`.
8. Remove the revoked devices from the members, add them to the revoked set, move to `new_epoch`.
   Recipients unknown to M do **not** become members through this rule.

M writes only with the most recent key. It keeps `SK_e` to read the items of epoch
`e` still in transit, then **erases it `max_ttl_secs + 1 day` after applying the
rotation `e → e+1`** (31 days with the default values: beyond that, an honest relay
holds no item of epoch `e` any longer), unless a local item kept raw (§7.2, §9.4) still
depends on it. Items already opened no longer depend on `SK_e`: they are re-sealed
under `LSK` (§7.2). M also retains the `seq` of each applied rotation, even after
erasing the key, for the epoch ordering rule (§9.4, §12). Reference
implementation: `SpaceKeyRing` (`add_rotation_key`, `discard_expired`, `key_for_item`).

### 8.4 Trust store (rules)

Each device locally maintains: `space_id`, its own identity, the current epoch, the set of
**members** `(ed_pk → x_pk)` and the set of **revoked** devices. A key
enters the members only by:

1. face-to-face verification (pairing channel);
2. a valid `DeviceCert` signed by a non-revoked member (never self-signed);
3. a valid `MemberList` signed by a non-revoked member, at the current epoch.

Rules: a revoked key never becomes a member again (a reinstalled device generates
new keys); an `ed_pk` already known with another `x_pk` is refused
(`key_conflict`); a statement signed by a revoked device is refused, whatever its
date (signed dates are never used for the decision, because a stolen device can
backdate).

### 8.5 Deletion tombstone

```
TBS = "tomi/v1/sig/tombstone" || 0x00 || uuid16(space_id) || uuid16(item_id)
      || u64be(deleted_at_ms) || u8(scope) || signer_ed_pk
scope : 0x01 = local, 0x02 = everywhere
```

159 bytes on the wire. Example (`tombstone.json`, signed by B, scope everywhere):

```
746f6d692f76312f7369672f746f6d6273746f6e65 00   "tomi/v1/sig/tombstone"
5e1f0c2a7b3d4e8f9a610c2d4e6f8a10                       space_id
0e7b5a3c1d2f4a6b8c9d0e1f2a3b4c5d                       item_id
000001a1a411ea80                                       deleted_at_ms = 1793754000000
02                                                     scope = everywhere
dde3bccec7f3a66a1115f45d720f4dc135c3ae7c4e22dca38fdb1efd6a495ff8   signer_ed_pk (B)
8a613140…221c8400                                      signature (64)
```

- `everywhere`: sent to the relay, which verifies the signature against its authorized
  devices, purges the item and propagates the event; each device verifies (§8.4:
  signer is a non-revoked member) then erases its copy.
- `local`: is **not** sent. A `local` tombstone received from elsewhere is ignored.
- The scope is signed: the relay cannot turn a deletion everywhere into a local
  deletion (vector `tombstone_scope_altered`).
- A tombstone targets an `item_id`; it does not prove that the item existed and cannot be
  "cancelled".

### 8.6 What the relay verifies

The relay has no space key but can verify all signatures. It SHOULD
refuse: a `DeviceCert` whose issuer is not an authorized device; a rotation
from an unauthorized device or out of sequence; a tombstone from an unauthorized device;
an attachment removal (§8.10) from an unauthorized device; a `HistoryTransfer` manifest
(§8.7.1) that is not signed by the device depositing it, or whose recipient is not another
authorized member (`api-v1.md` §4.13). It cannot open `sealed_hk`. It cannot verify a pin
(§8.8), which is encrypted: it only checks the clear header of the state envelope and the
current epoch. These relay checks are defense in depth: the security of the content relies
on the device checks (§8.3, §8.4), not on the relay.

### 8.7 History transfer (decision D2)

The relay does not replay the history to a new device (`joined_seq`, `api-v1.md`
§4.10). A member A can however **transfer its whole feed** to a member R: at
pairing, or later from the Devices screen. The transfer carries **everything** A
can read, **secrets and files included**. The format provides no partial inclusion
option (no "secrets" flag, no "files" flag): the user only chooses whether to transfer or not.
The transfer is readable by R alone (key sealed for its `x_pk`, §7.4); no space key is
transmitted; the relay keeps the blobs only temporarily (`transfer_ttl_secs`, 7 days,
`api-v1.md` §4.13).

Elements: the sealed `HK` key (§7.4), the signed manifest (§8.7.1), the encrypted index in
parts (§8.7.2, §8.7.3), and a new blob per transferred file (§8.7.3).

#### 8.7.1 `HistoryTransfer` manifest

The common rules of §8.1 apply, with one exception: the list of parts follows the **order of the
parts** (it is not sorted). Duplicate `blob_id` or `nonce_prefix` values are refused
(`malformed_statement`).

```
TBS = "tomi/v1/sig/history-transfer" || 0x00 || uuid16(space_id) || uuid16(transfer_id)
      || u64be(as_of_seq) || u64be(created_at_ms) || sender_ed_pk (32) || recipient_ed_pk (32)
      || sealed_hk (104)
      || u32be(n_parts) || n_parts × ( uuid16(blob_id) || nonce_prefix (19) )
wire = TBS || signature (64)
```

Length on the wire: **313 + 35 · n_parts** bytes (29 + 16 + 16 + 8 + 8 + 32 + 32 +
104 + 4 = 249 bytes of fixed TBS, 64 of signature), with `1 ≤ n_parts ≤ 1024`. The
2-part manifest of `history-transfer.json` is 383 bytes. `sender_ed_pk` ≠
`recipient_ed_pk` (otherwise `malformed_statement`).

Start of the TBS of the vector (A → B):

```
746f6d692f76312f7369672f686973746f72792d7472616e73666572 00   "tomi/v1/sig/history-transfer"
5e1f0c2a7b3d4e8f9a610c2d4e6f8a10                             space_id
c1a7e0d25b3f4e8a9c612d4f6a8b0c1e                             transfer_id
000001a1a411eafb                                             as_of_seq = 1793754000123
000001a1a47fc780                                             created_at_ms = 1793761200000
4fd099cc…65c4                                                sender_ed_pk (A)
dde3bcce…5ff8                                                recipient_ed_pk (B)
ba193836…fbd8f4                                              sealed_hk (104)
00000002                                                     n_parts
b0000000000040008000000000000000 5152…63                     part 0 : blob_id, nonce_prefix
b1000000000040008000000000000001 6566…77                     part 1
```

`as_of_seq` is the `seq` of the snapshot: A has synchronized up to `cursor ≥
joined_seq(R)` and exports the local state at `cursor`. Everything that follows reaches R through the
journal.

#### 8.7.2 Index parts

The plaintext index (§8.7.3) is split into `n_parts` consecutive parts; part `p` is
encrypted with the STREAM construction of §6 (64 KiB chunks, nonce `nonce_prefix_p ||
u32be(i) || flag`), under the key `HK`, with the `nonce_prefix` of part `p` (random and
distinct for each part) and:

```
AAD_hist = "tomi/v1/aad/history-chunk" || 0x00 || u32be(format) || uuid16(transfer_id)
           || u32be(p) || u32be(n_parts)                                        // 54 bytes
```

Each encrypted part fits in a blob (`≤ max_blob_bytes`). The split is free (the
reader concatenates the parts); the reference implementation fills each part with the
largest number of whole chunks that fits in `max_blob_bytes`
(`history_part_capacity`, `history_part_count`). No plaintext size is
authenticated: the number of chunks of a part is derived from the blob size,
`N = max(1, ceil(len(blob) / 65552))`, and the **final flag is authoritative**. The reader
decrypts as a stream: any complete chunk followed by other bytes is non-final; the remainder,
at the end of the blob, MUST authenticate as a final chunk. A remainder that authenticates as a
**non-final** chunk signals a blob missing its last chunk (`stream_truncated`);
otherwise the failure is `decryption_failed`.

The AAD binds each part to its transfer, to its position and to the number of parts: the relay cannot
swap, remove or duplicate parts. The number of parts and their nonces
are signed in the manifest.

#### 8.7.3 Index (JSON Lines)

Concatenation of the decrypted parts = UTF-8 **JSON Lines**: one JSON object per line,
each line terminated by `\n` (`0x0A`). The JSON never contains a raw line break
(a `\n` inside a string is escaped). Read as a stream, without loading everything in memory; a
line is at most 2 097 152 bytes (`malformed_payload` beyond that).

```json
{"kind":"header","version":1,"space_id":"…","transfer_id":"…","as_of_seq":1793754000123,"source_device":"e983270f…"}
{"kind":"device","device_id":"6896f283…","name":"Old phone","platform":"android","revoked":true}
{"kind":"item","payload":{ …object §5.4 as decrypted… },"origin_seq":1793750400120,"received_at":1793750402000,"read":true,"kept":true,"files":[{"blob_id":"…","key":"<b64 32>","nonce_prefix":"<b64 19>"},null,{…}]}
{"kind":"end","items":3,"skipped":{"unreadable":1,"files_unavailable":1}}
```

| Line | Fields | Rules |
|-------|--------|--------|
| `header` | `version` (= 1), `space_id`, `transfer_id`, `as_of_seq`, `source_device` | MUST be the **first** line and match the manifest (`source_device` = `device_id` of the sender), otherwise `malformed_payload`. Only one. |
| `device` | `device_id`, `name`, `platform`, `revoked` (default `false`) | devices known to the sender, to name the author of an item even if revoked (which `GET /v1/devices` no longer returns) |
| `item` | `payload` (object §5.4), `origin_seq` (seq of the original `item` event), `received_at` (ms), `read` (default `false`), `kept` (default `false`, "Keep", D33), `files` (default `[]`), `pin` (optional, `{pinned, at}`, §8.8.5), `archive` (optional, `{archived, at}`, §8.9.4), `removed_attachments` (optional, array of slots, §8.10.6) | `payload` is re-read with the rules of §5.4 (repeated keys refused); the exporter puts the payload in **exactly as it received it** (unknown fields included, §5.4.4), except removed entries, which are exported emptied (§8.10.6), with no raw line break outside strings (removed if any), and the importer keeps it as is; two items with the same `id` → `malformed_payload`; `files.length` = `payload.attachments.length`, otherwise `malformed_payload` |
| `end` | `items`, `skipped`: `{unreadable, files_unavailable}` (defaults 0) | MUST be the last line; missing → `history_incomplete`; `items` ≠ number of `item` lines → `malformed_payload`; any data after → `malformed_payload` |
| other `kind` | | ignored (additive change, §5.4.4); `member` is reserved (§5.8.3) |

`files[i]` corresponds to `payload.attachments[i]`: it is a **new blob**, encrypted in
STREAM with a key `FK'` and a **fresh** prefix (never the `FK` nor the original prefix,
to rule out any nonce reuse), and the `AAD_chunk` of §6.3 (same `item_id`, same
index). The reader decrypts with the payload metadata (`size`, `name`, `mime`) and
the `blob_id`, key and prefix of `files[i]` (`HistoryItem::import_attachment`).
`null` means that the file is **not available on the sender** (never
downloaded, or erased): this is **not** an exclusion option. It also applies to a slot
removed (`removed_attachments`, §8.10.6), which is then not counted as unavailable. All files
present on the sender are transferred; the thumbnail, in the payload, remains
available.

**Not** exported: items that the sender cannot read (§9.4; counted in
`skipped.unreadable`), items still in its outgoing queue (they will arrive through the
journal), deleted or consumed items ("after reading"), acknowledgements and delivery
statuses. Secrets are exported like the other items, title included, in the
encrypted form.

#### 8.7.4 Verification and import (recipient)

R verifies the manifest, in order: `space_id` (`space_mismatch`);
sender is a trusted, non-revoked member (`untrusted_signer` / `revoked_device`);
signature (`invalid_signature`); `recipient_ed_pk` = its own (`not_a_recipient`);
opening of `sealed_hk` (`decryption_failed`). Reference implementation:
`TrustStore::verify_history_transfer`, which returns `HK`. Then each part is decrypted
in order (`HistoryPartDecryptor`) and the index is read as a stream (`HistoryIndexReader`), which
applies the rules of §8.7.3.

Import rules (normative on the client side, `api-v1.md` §7.8): insertion in batches, only
if the `item_id` is absent from the local items and from the known tombstones (idempotent resumption);
no `ack`, `read` or `release` is sent to the relay for an imported item; the local expiration
follows the payload; an unread `after_first_read` remains single-use; an imported secret
stays masked, and its title is never reused in a notification. The pin state of an `item` line
(field `pin`, §8.8.5) is merged into the local state
(last-writer-wins, §8.8.3), **even if the item is already present**; it is ignored if the item
is in the known tombstones. The slots of `removed_attachments` (§8.10.6) are emptied before
local sealing, their files are never downloaded, and the removal register
(§8.10.3) is also consulted: a removal received during the import applies to the imported item.

### 8.8 Pin: shared, encrypted item state (decision "Pin")

An item can be **pinned**: it moves to the top of the feed, and each device keeps its
local copy beyond the relay purge (§8.8.4). The pin is a **protocol state**, shared
by all devices, and not a local setting. It travels in an **item state event**: a signed
statement (§8.8.1) encrypted under the space key (§8.8.2). Devices converge by
last-writer-wins on the signed timestamp (§8.8.3), as for a
deletion tombstone.

The intended difference from the tombstone (§8.5): the tombstone is in clear because the relay
must purge the item. The pin, for its part, changes **nothing** in the relay purge (decision (a),
§8.8.4): the relay therefore does not need to read it. Knowing which items matter to the user
is sensitive metadata (especially for a secret). The relay sees neither the `item_id` nor
the state, only a random `event_id`, the emitting device, the epoch, the time and a
constant size. The `item_id` is not in clear either: otherwise the parity of the events
for a single item (pin, unpin, pin…) would reveal the state to the relay.

#### 8.8.1 `PinStatement` statement

```
TBS = "tomi/v1/sig/pin" || 0x00 || uuid16(space_id) || uuid16(item_id)
      || u64be(at_ms) || u8(state) || signer_ed_pk (32)
state : 0x00 = unpinned, 0x01 = pinned; any other value → malformed_statement
at_ms : 0 ≤ at_ms ≤ 2^53 − 1 = 9007199254740991; beyond → malformed_statement
wire  = TBS (89) || signature (64)                                        // 153 bytes
```

**Bound of `at_ms`.** A device MUST NOT sign a statement whose `at_ms` exceeds
`2^53 − 1`, and a reader MUST refuse it (`malformed_statement`) **before** verifying the
signature, even if the signature is valid (vector `at_ms_above_2_53_minus_1`). Reason:
the order of convergence (§8.8.3) must be the same everywhere; beyond `2^53`, a signed Kotlin `Long`
(beyond `2^63`) or a JavaScript number (from `2^53`) no longer compares like a Rust `u64`.
The same bound applies to the `pin.at` field of the transfer index
(§8.8.5) and to the persisted local state. Reference implementation: `PIN_AT_MS_MAX`.

Example (`pin.json`, `pin_by_b`, B pins the item `text_simple`):

```
746f6d692f76312f7369672f70696e 00   "tomi/v1/sig/pin"
5e1f0c2a7b3d4e8f9a610c2d4e6f8a10      space_id
0e7b5a3c1d2f4a6b8c9d0e1f2a3b4c5d      item_id
000001a1a42d61c0                      at_ms = 1793755800000
01                                    state = pinned
dde3bccec7f3a66a1115f45d720f4dc135c3ae7c4e22dca38fdb1efd6a495ff8   signer_ed_pk (B)
9ee55472…497c8109                     signature (64)
```

The signature identifies the authoring device; a statement from a device that is not a member or
is revoked is refused (§8.4), as for a tombstone. The statement is **never** sent in
clear: only inside a state envelope (§8.8.2), or, reduced to `{pinned, at}`, inside a
sealed history transfer (§8.8.5).

#### 8.8.2 Item state envelope

```
K_state_e = HKDF(∅, SK_e, "tomi/v1/hkdf/item-state", 32)
AAD_state = "tomi/v1/aad/item-state" || 0x00 || u32be(format) || u32be(key_epoch)
            || uuid16(event_id)                                               // 47 bytes
padded    = padding of §5.3 applied to wire(PinStatement)                    // 256 bytes
nonce     = random (24)
state_env = u32be(format) || u32be(key_epoch) || uuid16(event_id) || nonce
            || XC_Seal(K_state_e, nonce, AAD_state, padded)
```

| Offset | Length | Field |
|----------|----------|-------|
| 0 | 4 | `u32be(format)` = `00000001` (invariant §3.1) |
| 4 | 4 | `u32be(key_epoch)`: the emitter's **current** epoch |
| 8 | 16 | `uuid16(event_id)`: random UUID v4, the relay's idempotency key; **not** the `item_id` |
| 24 | 24 | `nonce` |
| 48 | 272 | encryption of the padded content (256) + tag (16) |

A pin is **320 bytes**. The padded content is a multiple of 256 bytes, from 256 to
2048 (room for other state types later; the size does not distinguish the types).
The decrypted content starts with the label of its type: `"tomi/v1/sig/pin" || 0x00`
for a pin. Content of an **unknown type** has no effect and produces no error (additive change,
§5.4.4), but the envelope is **kept raw** and replayed after an update
(step 6).

`K_state_e` is a new use of `SK_e`, hence a new label (§4.2); the
key differs from `K_wrap_e`. Example (`pin.json`):
`K_state_1 = 4cc1cf9910b1ebababd5106a1c2721614af121e00d1405c0be86b11c1cbd9737`, and

```
AAD_state = 746f6d692f76312f6161642f6974656d2d7374617465   "tomi/v1/aad/item-state"
            00 00000001 00000001 e0000000000040008000000000000001
state_env = 00000001 00000001 e0000000000040008000000000000001
            303132333435363738393a3b3c3d3e3f4041424344454647   nonce
            …                                                  ciphertext + tag (272 bytes)
```

**Opening** (reader), inputs: the envelope, the `event_id` under which the relay listed it,
its `seq`, the ring of space keys:

1. Fewer than 4 bytes → `malformed_envelope`; `format` out of range → `unsupported_format`
   (event kept raw, retried after an update).
2. Length ≠ `48 + 256·k + 16`, `1 ≤ k ≤ 8` (320 to 2 112 bytes) → `malformed_envelope`.
   The relay applies the same rule on deposit (`api-v1.md` §4.8). Steps 1 and 2:
   `read_item_state_header`.
3. Epoch ordering rule and presence of the key, as for an item (§9.4):
   `SuspectEpoch` → ignored; `KeyPending` → kept raw, retried after the rotation.
4. `event_id` in the header ≠ announced `event_id` → `event_id_mismatch`. The comparison
   covers the 16 bytes (an `event_id` announced in uppercase designates the same event).
5. `XC_Open` → otherwise `decryption_failed`; removal of the padding (§5.3) → otherwise
   `malformed_envelope`.
6. Content starting with the pin label: read `PinStatement` (exactly 153 bytes,
   otherwise `malformed_statement`). Content starting with the archive label: read
   `ArchiveStatement` (§8.9.1, 157 bytes). Other content: **unknown type**.
   Content of an unknown type is neither applied nor discarded: the envelope is kept raw (like
   an `UpdateRequired` state, step 1) and reopened after each application update
   (`api-v1.md` §7.3). A type that has become known then follows steps 6 and 7; a type that is
   still unknown remains kept, without effect, without error and without a log entry. A device that
   is behind thus catches up on the decisions (pin, archive, future states) issued before its
   update, instead of losing them with its cursor. Convergence (§8.8.3) makes the replay safe:
   an old or repeated state has no effect. Kept events are discarded only at steps 3
   (`SuspectEpoch`) and 5 (opening failure or invalid padding) during a replay.
7. Verify the statement (§8.4): `space_id` (`space_mismatch`), signer is a non-revoked
   member (`untrusted_signer` / `revoked_device`), signature (`invalid_signature`).
   Reference implementation: `TrustStore::verify_pin`.

**Revocation and kept raw events (accepted exception).** As for any signed
statement (§8.4), revocation is evaluated against the trust store **at the time of
verification**, never at the signed date nor at the `seq` of the event. A
tombstone is in clear and always verified in journal order: it is therefore verified
before any rotation with a higher `seq`. A pin, however, may be kept raw
(`KeyPending`, `UpdateRequired`) and reopened later. If the rotation that revokes its
signer has been applied in the meantime, the pin is refused (`revoked_device`), even if
its `seq` precedes that rotation and other devices have already applied it. This is a
choice: evaluating at the `seq` would require dating each revocation (by the `seq` of its
rotation) in the trust store and would make the pin the only statement judged
by a `seq` supplied by the relay; the single rule of §8.4 remains simpler and more
cautious. Consequences and mitigation:

- the discrepancy only affects the display and local retention of an item, never its
  content; it disappears at the next decision on this item (`current_at_ms + 1`, §8.8.3)
  or at the next history transfer (§8.8.5);
- a client MUST retry its `KeyPending` states **right after each applied rotation,
  before the next event** (`api-v1.md` §7.3, §7.5). A pin of epoch
  `e+1` is then verified before any later rotation, and therefore before the revocation of its
  signer: in practice, the exception only affects `UpdateRequired` states (future
  format) reopened after an application update.

A state event is never displayed: on failure it is ignored and logged without
content (only the item is subject to invariant 7).

#### 8.8.3 Convergence: last-writer-wins

The pin state of an item is the pair `(pinned, at_ms)`. Each device keeps, per
`item_id`, the **maximal** state according to the total order:

```
(at_ms, pinned) compared lexicographically; pinned = true > false
```

The larger `at_ms` wins; on a tie, **pinned** wins (keeping is the safer choice). The order
is total: whatever the order of reception (journal, history transfer, replay by
the relay), all devices reach the same state. A replayed or old event has no
effect. The default state (no known decision) is `(false, 0)`.

**Timestamp of a local decision**: `at_ms = max(now_ms, current_at_ms + 1)`. A
user decision thus always prevails over the state they saw, even if
the device clock is behind. Pinning an already pinned item (or unpinning an
unpinned item) emits nothing. If this computation exceeds `2^53 − 1` (§8.8.1), the decision is
refused (`malformed_statement`): a state already at the bound can no longer change; only a
tombstone removes it. An honest clock never comes close (2^53 ms ≈ 285 000 years).

The state is kept **independently** of the item: a decision may arrive before the item (item
received later through a history transfer) or for an item already purged from the journal. A
tombstone erases the item and its state; a state event for an `item_id` of the known
tombstones is ignored. The set of states may therefore name items absent from the device:
the "Pinned" section is the intersection with the local items. Reference
implementation: `PinState`, `PinBook` (`merge`, `apply_statement`, `set_pinned`, `forget`,
`pinned_items_present`).

#### 8.8.4 Effects of a pin: local retention, expiration, relay purge

- **Relay purge: unchanged (decision (a)).** The relay purges a pinned item like the
  others, as soon as it is delivered to all active devices (`api-v1.md` §8.1). It keeps nothing
  for a pin and does not read the state. Each device keeps its local copy beyond the
  purge.
- **Files: a pin triggers no download.** A device MUST NOT
  download the files of an item because it is pinned, neither on receiving a
  state event, nor after its own decision, nor later in batch: otherwise the relay,
  which sees blob downloads per item, would learn the targeted item **and** the meaning
  (only pinning would trigger a download). Files are downloaded under the ordinary policy, the same for
  all items (D20: on opening, or automatically on
  receiving the item under a threshold on Wi-Fi). The set of "wanted" files, which
  governs `release` (`api-v1.md` §4.8, §7.3), never depends on the pin state:
  downloads and `release` are identical, pinned or not.
  Consequence: the pin keeps what the device already has (item, thumbnails,
  downloaded files). A file not yet downloaded remains on the relay until the device
  has sent `release`, hence at most until the relay deadline (`expires_at`, at most
  `max_ttl_secs`); beyond that it is unavailable, as for an unpinned item. The app
  SHOULD flag this on a pinned item ("File not downloaded, available until …")
  so that the user opens it in time; opening is an ordinary action.
- **Explicit expiration**: a pin never extends it and revives nothing. An item
  with `expires_at` (payload) is erased on that date, pinned or not; an
  `after_first_read` is consumed by its tombstone, pinned or not.
- **Local cleanup** ("Clean up files older than N months", batch deletion from the
  secret review): a pinned item is **not** eligible for it. Only a targeted action
  (unpin, delete) removes it.
- **Sorting**: pinned items move to the top of the feed ("Pinned" section), from the
  most recently pinned to the oldest.

Reference implementation: `local_retention(user expiration, pinned, received_at,
now, cleanup_before)`.

#### 8.8.5 History transfer

The pin is part of the protocol: it is part of the transfer. An `item` line of the index
(§8.7.3) carries the optional additive field `pin`:

```json
"pin": {"pinned": true, "at": 1793755800000}
```

`pinned` (boolean) and `at` (ms, the signed timestamp of the winning decision) are both
required in the object; `at > 2^53 − 1` (§8.8.1) → `malformed_payload`. The sender writes it as soon as it knows a decision for the item,
**pinned or unpinned**: the timestamp lets the recipient converge with the
events that also arrive through the journal (between `joined_seq` and `as_of_seq`). Absent field =
no known decision. An old reader ignores it (§5.4.4); the index does not contain
the signed statement (its author may have been revoked since): the index is already
authenticated as coming from the sender (§8.7.2). The recipient merges the state
(§8.8.3) even if the item is already present on its side.

### 8.9 Archive: shared, encrypted item state (decision "Archive")

An item can be **archived**: it leaves the "Recent" list on all devices and remains
intact (content, files already downloaded, read or unread status), browsable in "Archives" and
by search. The archive is the **twin of the pin** (§8.8): a protocol state, not a
local setting, carried by the same **item state event** mechanism (§8.8.2) with its own
statement type. The "Recent" and "Archives" filters are an application concern: the
core never removes an archived item from the list it returns.

A **purely additive** change (§5.4.4): an archive statement is state content of an
**unknown type** for a device that does not know it, with no effect and no error (§8.8.2, step 6); it
keeps the raw envelope and catches up on this decision after its update. The pin statement is **not**
reused with another `state` value (any value other than `0x00` / `0x01` is refused there,
§8.8.1): the archive has its own label.

#### 8.9.1 `ArchiveStatement` statement

```
TBS = "tomi/v1/sig/archive" || 0x00 || uuid16(space_id) || uuid16(item_id)
      || u64be(at_ms) || u8(state) || signer_ed_pk (32)
state : 0x00 = unarchived, 0x01 = archived; any other value → malformed_statement
at_ms : 0 ≤ at_ms ≤ 2^53 − 1 = 9007199254740991; beyond → malformed_statement
wire  = TBS (93) || signature (64)                                        // 157 bytes
```

Same layout as `PinStatement` (§8.8.1), only the label differs (`tomi/v1/sig/archive`, 19
bytes, registered in §4.2); the state byte is at offset 60 and `at_ms` at offset 52.
The bound on `at_ms` and the check that precedes signature verification are those of §8.8.1.
A pin statement is not read as an archive statement, nor the reverse
(`malformed_statement`, vector `pin_statement_is_not_an_archive`): the label is signed, so a
pin signature can never be replayed as an archive. Reference implementation:
`ArchiveStatement`, `DeviceKeys::sign_archive`, `ARCHIVE_AT_MS_MAX`.

#### 8.9.2 Item state envelope

That of §8.8.2, unchanged: `K_state_e`, `AAD_state`, padding of §5.3 applied to
`wire(ArchiveStatement)` (157 bytes, one block of 256), envelope of **320 bytes**, identical in
size and shape to that of a pin. The relay sees neither the `item_id`, nor the state, **nor the
type** (pin or archive). The decrypted content starts with the label of its type:
`"tomi/v1/sig/archive" || 0x00` for an archive; the two labels are never a prefix of
each other once terminated by `0x00`. Opening and verification: steps 1 to 7 of
§8.8.2 with `ArchiveStatement` (exactly 157 bytes, otherwise `malformed_statement`) and
`TrustStore::verify_archive`; the rule on revocation of kept raw events applies
as is.

#### 8.9.3 Convergence: last-writer-wins

As in §8.8.3, in a register **independent** of the pin register: the archive state of an item
is `(archived, at_ms)`, total order `(at_ms, archived)` lexicographic, `archived = true >
false` (on a tie, **archived** wins; the order is total, so all devices converge).
A replayed or old event has no effect. Default state `(false, 0)`. Timestamp of a local
decision: `at_ms = max(now_ms, current_at_ms + 1)`; archiving an already archived item (or
unarchiving an item that is not) emits nothing; beyond `2^53 − 1` the decision is refused.
The state is kept independently of the item (it may arrive before it). A tombstone erases the item
**and** its archive state; an archive event for an `item_id` of the known tombstones is
ignored. Reference implementation: `ArchiveState`, `ArchiveBook` (`merge`, `apply_statement`,
`set_archived`, `forget`).

**Mutual exclusion with the pin.** Two registers, but three places for the user
(Pinned, Recent, Archives), mutually exclusive in the display:

- **archiving a pinned item** emits "unpin" **then** "archive" (two events, each signed
  with the timestamp of its own register);
- **pinning an archived item** emits "unarchive" **then** "pin";
- if two devices cross and both registers end up true, **the pin wins in the display**
  (keeping it visible is the safer choice): the item is "pinned" and is not "archived"
  for the application; the archive state stays in its register. As soon as it is unpinned, it
  appears archived.

#### 8.9.4 Effects of the archive

The archive **hides** an item from "Recent"; it does nothing else:

- **It deletes nothing**, **never** marks the item as read (a secret "after first reading" is
  therefore never consumed by a filing action), emits no acknowledgement, no `read`, no `release`
  (`api-v1.md` §4.8, §7.3), **downloads** nothing (same reason as §8.8.4: nothing the
  relay sees depends on the archive state), does not change the relay purge (decision (a)), nor
  an explicit expiration: an archived item expires on its date, an archived `after_first_read` is
  consumed at its first reading.
- **It does not protect against local cleanups** (unlike the pin, §8.8.4): an archived item
  is even a natural candidate for "Clean up files older than N months" and for the
  secret review (otherwise the archive would be a hiding place).
- An archived **secret** remains masked, title included; none of its content leaves either in the state
  or in the index. An **unreadable** item is archivable (the state is kept independently of the item).
  An item still in the outgoing queue or failed to send is not archivable on the application side
  (the failure must remain visible); the core, for its part, accepts the decision.
- **Deletion** (local or everywhere, or erasure at expiration): the item and its archive
  disappear.

**History transfer.** An `item` line of the index (§8.7.3) carries the optional additive field
`archive`, merged like `pin` (§8.8.5), **even if the item is already present**:

```json
"archive": {"archived": true, "at": 1793755800000}
```

`archived` (boolean) and `at` (ms, signed timestamp of the winning decision) are both
required; `at > 2^53 − 1` → `malformed_payload`. The sender writes it as soon as it knows a
decision for the item, **archived or unarchived**; absent field = no known decision. An
old reader ignores it (§5.4.4). The transfer keeps **everything**, secrets included: the new device
gets the archive state too. The recipient merges the state as last-writer-wins (§8.9.3); if both
the `pin` and `archive` states of a line are true, the pin wins in the display.

Vectors: `spec/vectors/v1/archive.json` (see §11). Contract for the apps: `archive-contract.md`.

### 8.10 Attachment removal (decision "Delete an image / a file")

Removes **one** attachment from an item without touching the others or the item. The removal is its own
signed statement (like the item tombstone, §8.5, but one level lower),
propagated by a journal event; it combines with a local register independent of
the item (§8.10.3).

Guiding principle: the removed attachment is erased **as if it had never existed**
on every up-to-date device: no file, no relay blob (when it still exists), no
thumbnail, no name, size or type readable in the local payload.

**A slot is never renumbered.** The attachment's index is part of the `AAD_chunk`
(§6.3) and of `files[i]` (§8.7.3): the entry in `payload.attachments` is not deleted
and the following ones are not shifted. A removed attachment keeps its **place** (its *slot*, its
original position in `payload.attachments`); the non-removed attachments keep their slot.

#### 8.10.1 `AttachmentTombstone` statement

```
TBS = "tomi/v1/sig/attachment-tombstone" || 0x00 || uuid16(space_id) || uuid16(item_id)
      || uuid16(blob_id) || u32be(slot) || u64be(deleted_at_ms) || signer_ed_pk
```

189 bytes on the wire (`TBS || signature(64)`, TBS of 125). An **exact** length is required
(`malformed_statement`). `slot` is the position of the attachment in `payload.attachments` of the
**original** item; `blob_id` is its `blob_id` in this payload. For an item imported through a
history transfer (§8.7), it is the `blob_id` of the **payload** that counts, not that of the transfer
blob (`files[i]`, which is another blob). `deleted_at_ms ≤ 2^53 − 1`, otherwise
`malformed_statement` (like `at_ms`, §8.8.1). There is **no** scope byte: only the
"everywhere" scope circulates.

The label is its own (§4.2): its length and prefix differ from those of the item
tombstone; the signature of one can never be replayed as the other (vectors
`item_tombstone_is_not_an_attachment_removal` and `attachment_removal_is_not_an_item_tombstone`).
Verification (`TrustStore::verify_attachment_tombstone`), in order: parsing (length,
label, bound of `deleted_at_ms`: `malformed_statement`), `space_id` (`space_mismatch`),
non-revoked member signer (`untrusted_signer` / `revoked_device`), signature
(`invalid_signature`). Vectors: `attachment-tombstone.json` (§11).

#### 8.10.2 Scopes

- **This device only**: the removal applies to the local copy only (§8.10.4); nothing
  is signed or sent. The other devices keep the attachment.
- **Everywhere**: the device signs the statement, applies it locally (§8.10.4) and sends it
  to the relay (`api-v1.md` §4.8 bis), which deletes the blob when it still exists and propagates
  the `attachment_tombstone` event. The send is replayed until success; a relay that does not
  know the route (`404` response) leaves the operation queued with a long delay, without
  ever abandoning it. The core offers "everywhere" only if the relay announces the
  `attachment_tombstones` capability (`features` of `hello` and of the space state, `api-v1.md` §4.1).
- An item still in the outgoing queue (never delivered) refuses the removal, **whatever the scope**
  (`invalid_input`): its envelope and its blob would leave anyway, and a rotation before sending
  re-seals the envelope from the local copy (§7.2): a "this device" removal would appear there and
  would be broadcast. The removal becomes possible once the item has left.
- If, after the removal, **no attachment**, **no text** and **no URL** remain (the
  title is ignored, secret or not), the core instead deletes the whole item with the same
  scope (§8.5) and tells the caller.
- Idempotent: unknown or already removed slot → no effect. The core never downloads the
  attachment in order to remove it. No biometrics (as for deleting an item).

#### 8.10.3 `attachment_removals` register

Local table **independent of the item** `(item_id, idx, blob_id, scope, at_ms)`, like the
tombstones and the pins: a removal can arrive **before** the item, during a history
import, for an item waiting for a key or an update, or during the download
of the attachment. It is consulted when an item is opened (creation of the file rows), at
history import (§8.7.4), at the end of a download and at export. An entry
applies only to the attachment whose payload `blob_id` equals its own. It is kept
as long as the item exists (it leaves with it); without an item, it is forgotten after 45 days
(like a tombstone). An "everywhere" scope is never downgraded to "local". We do **not**
add a "removed" state to the file rows: the row is deleted and the
register is authoritative.

**Race with the download.** The end of a download checks the register in the
same transaction as the row write: if the attachment was removed in the meantime, the row
is not written and the downloaded file (encrypted and decrypted) is erased.

#### 8.10.4 Erasure of the local copy

On removal (local, everywhere, or received), the device:

1. deletes the file row and, **after** the SQL validation (as for erasing an
   item), the encrypted copy at rest and the decrypted copy in the cache;
2. **empties the local payload entry** for this slot and re-seals the item under the local key
   (§7.2). The entry becomes `{ "blob_id": <kept>, "name": "", "mime": "", "size": 0,
   "key": <32 zero bytes>, "nonce_prefix": <19 zero bytes>, "removed": true }`: all the
   neutral values are valid for a reader that does not know `removed` (which will see
   an empty attachment, never an error); the thumbnail and any other field of the entry
   disappear; the `blob_id` is kept to verify later removals. All the
   other fields of the payload, known or unknown (§5.4.4), and the other entries are kept
   as they are. This is a **targeted** change to the local copy: it changes neither the
   envelope received from the relay nor the other devices;
3. no longer exposes the attachment: an up-to-date reader lists only the non-removed attachments,
   each with its original slot; access to a file is done by slot, never by position in
   the displayed list;
4. forgets the attachment in the list of blobs announced by the relay and kept with the item (neither the `blob_id`
   nor the encrypted size remain outside the `blob_id` kept in the payload).

On a **received** item, if the removal leaves no attachment to retrieve, the device
queues the `release` of the item (`api-v1.md` §7.3), as after a last download. The same applies
when opening an item all of whose attachments had already been removed (register filled before the item,
item waiting for a key or an update): the `release` goes out as soon as it is opened.

#### 8.10.5 Receiving the event

A device that reads the `attachment_tombstone` event from the journal:

- verifies the statement (§8.10.1, non-revoked member signer); a statement that does not
  verify is a diagnostic (`attachment_tombstone_refused`), never a synchronization error: the cursor
  always advances;
- ignores the event if the item is in the known tombstones; otherwise records it in the
  register (§8.10.3) when the item is not (yet) readable;
- if the item is present and readable, compares `payload.attachments[slot].blob_id` with the signed
  `blob_id`: different → ignored with the diagnostic `attachment_tombstone_refused`; equal → records it
  and applies the erasure of §8.10.4. An out-of-bounds slot is ignored;
- is idempotent: replaying the journal or a second event for the same `(item, slot)` changes
  nothing.

Documented case, not handled: two concurrent removals (two devices each remove the last
attachment) may leave an item with no content.

#### 8.10.6 History transfer

The `item` line of the index (§8.7.3) carries the optional additive field `removed_attachments`,
an array of integers: the slots removed on the sender (local **and** everywhere removals: the
attachment is also removed for the new device).

- `files[i]` stays of the **same length** as `payload.attachments` (rule of §8.7.3: otherwise
  the old reader would reject the whole transfer); for a removed slot, `files[i]` is
  `null`;
- the sender writes the field only if it is non-empty; the exported payload is the local
  copy (entry emptied, §8.10.4);
- the up-to-date reader requires a **sorted, duplicate-free** list, each value
  `< payload.attachments.length`, otherwise `malformed_payload`; it never downloads the
  file of a removed slot (even if `files[i]` is not `null`, it is ignored); these slots do not
  count in `skipped.files_unavailable`; the importer empties the removed slots before
  sealing the item and creates no file row for them;
- an old reader ignores the field (§5.4.4): it sees an empty attachment with no file available.

#### 8.10.7 Limits

See also §12. The `item` event in the relay journal keeps the old list of blobs (it
is not rewritten). The blobs of a history transfer already deposited still contain the
attachment (masked on the up-to-date recipient, not deletable by the relay). A device
**not yet up to date** keeps indefinitely an attachment already downloaded, and its thumbnail:
"everywhere" only applies to up-to-date devices. The relay must be deployed **before** the
apps (`api-v1.md` §4.2 and annex B.2).

---

## 9. Versioning and errors

### 9.1 Unknown format

An item whose `format` is not in `[FORMAT_MIN, FORMAT_MAX]`:

- MUST NOT be deleted, silently ignored, or "repaired";
- MUST be kept (encrypted) and displayed as "Unreadable content, update the app";
- MUST be reopened after an app update.

A reader keeps the ability to read an old format as long as items of that format
may be on the relay or kept raw locally (§7.2), before withdrawing support
(PROJECT.md §6). Items already opened are not concerned: they are re-sealed under
`LSK`.

### 9.2 Error codes

Stable codes (`CoreError::code()`), used by `expected_error` in the vectors:

| Code | Case |
|------|-----|
| `unsupported_format` | format out of range (§9.1) |
| `malformed_envelope` | length, padding |
| `malformed_payload` | invalid JSON, required field missing or of the wrong type, non-canonical base64/UUID |
| `malformed_statement` | malformed signed statement, unsorted lists |
| `decryption_failed` | AEAD failure (wrong key, AAD, altered ciphertext) |
| `key_epoch_mismatch` | key provided from a different epoch than the envelope |
| `item_id_mismatch` | announced id differs from the header and the payload |
| `invalid_signature` | invalid Ed25519 signature |
| `space_mismatch` | different space |
| `untrusted_signer` | signer is not a member |
| `revoked_device` | signer or subject revoked |
| `key_conflict` | different X25519 key for a known device |
| `epoch_out_of_order` | out-of-sequence rotation |
| `not_a_recipient` | device absent from a rotation that does not revoke it |
| `stream_truncated`, `stream_trailing_data`, `stream_finalized` | attachments (§6.5), index parts (§8.7.2) |
| `history_incomplete` | transfer index without an `end` line (§8.7.3) |
| `event_id_mismatch` | announced `event_id` ≠ `event_id` in the header of a state envelope (§8.8.2) |
| `invalid_input`, `randomness`, `internal` | call / environment errors |

Error messages never contain plaintext or keys.

### 9.3 Decryption error

Whatever the error, a received item is never deleted because of it: it is
displayed as "unreadable" (DEV.md invariant 7). Only the **user** expiration (§5.7) and
a valid tombstone delete an item.

### 9.4 States of a received item: readable or unreadable

The model of a received item distinguishes **readable** and **unreadable**; an unreadable item keeps
its raw envelope and a stable reason. Reference implementation:
`SpaceKeyRing::open_received(envelope, item_id, seq) → ReceivedItem`, which never returns
an error.

| State (`api-v1.md` §7.3) | `UnreadableReason` (`code()`) | Cause | Retry |
|-------------------------|-------------------------------|-------|---------|
| readable | — | opened successfully; re-sealed under `LSK` (§7.2) | — |
| `UNREADABLE_UPDATE_REQUIRED` | `UpdateRequired` (`update_required`) | `unsupported_format` | after an app update |
| `KEY_PENDING` | `KeyPending` (`key_pending`) | epoch higher than the device's current key, including a reserved value (§5.8.2) | after the next rotation is applied |
| `UNREADABLE_SUSPECT_EPOCH` | `SuspectEpoch` (`suspect_epoch`) | epoch `e` already replaced by a rotation `e → e+1` whose `seq` is **lower** than that of the item (epoch ordering rule, §12) | never |
| `UNDECRYPTABLE` | `Undecryptable` (`undecryptable`) | any other error (AEAD, AAD, JSON, id) or epoch key already erased (§8.3) | never automatically |

The order of checks is: header (format), epoch ordering rule, presence of the
key, opening. An unreadable item is never exported in a history transfer
(§8.7.3) and is never "consumed" by a read.

An item state event (§8.8) follows the same order of checks
(`SpaceKeyRing::open_received_state` → `ReceivedItemState`); `UpdateRequired` and
`KeyPending` cause it to be kept raw for a retry, the other reasons cause it to be ignored. It
is never displayed.

---

## 10. Fixed parameters of format 1

| Parameter | Value |
|-----------|--------|
| `format` | 1 |
| Space key epoch (`key_epoch`) | 1 to 2^31 − 1; `2^31` to `2^32 − 1` reserved (§5.8.2) |
| Envelope header | 120 bytes |
| Payload padding block | 256 bytes |
| Maximum padded payload | 1 048 576 bytes |
| Chunk size | 65 536 bytes |
| STREAM nonce prefix | 19 bytes |
| Sealed key | 104 bytes |
| Tombstone | 159 bytes |
| `AttachmentTombstone` | 189 bytes (TBS 125 + signature 64); `deleted_at_ms ≤ 2^53 − 1` |
| DeviceCert | 208 bytes |
| Max entries per signed list | 1024 |
| Sealed transfer key (`sealed_hk`) | 104 bytes |
| `HistoryTransfer` manifest | 313 + 35 · n_parts bytes, 1 ≤ n_parts ≤ 1024 |
| `AAD_hist_transport` / `AAD_hist` | 94 / 54 bytes |
| Transfer index line | ≤ 2 097 152 bytes |
| Retention of an old space key | `max_ttl_secs` + 1 day after the rotation (31 days by default) |
| Local record (`LSK`) | 41 bytes + payload JSON |
| `PinStatement` | 153 bytes (TBS 89 + signature 64) |
| Item state envelope | 48 + 256·k + 16 bytes, 1 ≤ k ≤ 8; a pin: 320 bytes |
| `AAD_state` | 47 bytes |

---

## 11. Test vectors

`spec/vectors/v1/` (see its `README.md`). Contents: external vectors for the primitives,
HKDF, AAD, three text envelopes, an item with three attachments (0 bytes, 64 KiB,
64 KiB + 1000 bytes over 2 chunks), identities and certificates, sealed key, tombstones,
rotation, JSON reading, failure cases, and history transfer
(`history-transfer.json`: `sealed_hk`, 2-part manifest, STREAM parts one of which spans
2 chunks, JSON Lines index, files re-encrypted under `FK'`, secret without expiration nor
`read_policy`, masked secret title, and their failure cases), and pin (`pin.json`:
`K_state`, pin/unpin statements, state envelopes at epochs 1 and 2, last-writer-wins
convergence, epoch ordering rule, `pin` field of the transfer index, unknown state
type, failure cases including `at_ms` above `2^53 − 1`), and multi-recipient reservations
(`reservations.json`, §5.8: payloads carrying `audience` read as if without
it, complete envelope carrying `audience`, item and state event of a reserved `key_epoch`
kept KEY_PENDING, `KeyRotation` and `MemberList` with a reserved epoch refused),
and payload extensibility (`payload-extensions.json`, §5.4 and §5.4.4: repeated keys
refused at any depth, unknown fields preserved byte for byte on local re-sealing
and on history export), and archive (`archive.json`, §8.9: archive/unarchive statements
of 157 bytes, state envelopes of 320 bytes, convergence, `archive` field of the transfer index,
failure cases including a pin read as an archive), and attachment removal
(`attachment-tombstone.json`, §8.10: 189-byte statement, bounds of `slot` and of
`deleted_at_ms`, failure cases including an item tombstone read as an attachment removal and vice versa, and a revoked signer).
`SHA256SUMS` freezes the bytes. A published file is never modified; a **new** file may be added
(with its line in `SHA256SUMS`) for a compatible addition to format 1.

CI safeguard: `cargo test -p tomi-core --test spec_vectors`. The test rebuilds
each cryptographic value from the primitives and the layouts of this document,
without going through the code of `tomi-core`, then checks the public API on the same data
(opening, streams, trust decisions, JSON reading, failure cases).

---

## 12. Non-goals and known limits (v1)

- **No forward secrecy.** Whoever obtains a space key reads all the items of that epoch that are still available, including the old encrypted copies
  kept by the relay or its backups. Rotation only protects the future.
- **No automatic post-compromise security.** A compromised device remains a reader
  until it is explicitly revoked.
- **No authentication of the emitter between members.** Items are encrypted with the
  shared space key, not signed: a member can forge an item "coming" from another
  member (`source_device` is only an indication). Acceptable for a single-user product.
- **No recipient and no partitioning.** The reserved field `audience` (§5.8.1) has no
  effect in v1 and will never, on its own, be a confidentiality boundary: every member of
  the space reads every item encrypted under the space key.
- **Revoked device + compromised relay**: as long as devices keep the old space key
  to read items in transit, a revoked device colluding with a
  compromised relay could inject items at the old epoch. Mitigations:
  write only with the latest epoch; **epoch ordering rule**: a client MUST
  refuse an `item` event of epoch `e` whose `seq` is higher than the `seq` of the
  rotation `e → e+1` it has already applied, and display it as "unreadable (suspect)" without
  ever deleting it (§9.4). An honest relay cannot produce this case, because it
  checks `stale_key_epoch` in the transaction that assigns the `seq` (`api-v1.md`
  §4.8). Finally, old keys are discarded `max_ttl_secs + 1 day` after the rotation
  (§8.3). A window remains: an item injected **before** the client applies the
  rotation, with a `seq` lower than that of the rotation.
- **History transfer**: a transfer deliberately copies the whole feed, secrets
  and files included, to the recipient (decision D2). Its confidentiality relies on the
  recipient's `x_pk`, taken from the sender's trust store. The ciphertext
  remains on the relay for at most 7 days; whoever later obtains the recipient's `x_sk` and a
  copy of these blobs can read the transfer. No forward secrecy here either.
- **Availability is out of scope.** The relay may refuse to serve, delay, replay
  the order of events, withhold a tombstone or a rotation, or purge too early.
  The format detects alterations, not omissions. A relay that withholds a rotation
  effectively keeps a revoked device in the old epoch on non-up-to-date emitters.
- **Event ordering.** The relay assigns `seq`. With a stolen device, it could
  present a `DeviceCert` signed by that device before its revocation; the list of
  devices visible to the user is the countermeasure (the user revokes the intruder).
- **Metadata.** The relay sees the number of items, their dates, the emitting device, the
  size of the payloads to within the 256-byte block and the exact size (to within 16 bytes per
  chunk) of files, as well as the number of attachments. No padding of
  files and no dummy traffic.
- **No guaranteed erasure.** Expiration and deletion are best-effort (exported
  copies, backups, offline devices, copies transferred to another device).
  Without a default expiration (D1), an item remains indefinitely on the devices until
  it is deleted.
- **Pin**: the relay sees neither the item nor the state, but it sees that a device emitted
  a state event, and when. An event emitted just after receiving or reading an
  item lets one guess the item concerned (same limit as the tombstone after a `read`),
  but not the meaning. **Side channel of downloads: closed by construction.** The pin
  triggers no file download and does not change the timing of the `release`
  (§8.8.4): what the relay sees of the blobs is identical, pinned or not. Accepted cost:
  a file never opened from a pinned item is not fetched automatically and becomes
  unavailable at the relay deadline. Convergence relies on the clocks of the devices:
  a device whose clock runs well ahead imposes its decisions until
  a later decision (timestamped `current_at_ms + 1`) replaces them (`at_ms` bounded by
  `2^53 − 1`, §8.8.1). A pin kept raw and reopened after the revocation of its
  signer is refused, even if other devices have applied it (§8.8.2, accepted
  exception).
- **Removal of an attachment** (§8.10): the `item` event in the relay journal keeps the old
  list of blobs (it is not rewritten); the blobs of a history transfer already deposited
  still contain the attachment (masked on the up-to-date recipient, not deletable by the relay,
  which purges them at the latest at the 7-day deadline); a device not yet up to date keeps
  indefinitely an attachment already downloaded, and its thumbnail ("everywhere" only applies to
  up-to-date devices); two concurrent removals of the last attachment may leave an
  empty item; the relay sees that a device removes an attachment (item, position, instant), not
  its content.
- **Device member compromised and active**: out of scope.
- **Recovery if all devices are lost**: outside the format (`tomi-relay admin`,
  new space).

---

## 13. Open points (outside this document)

1. ~~Pairing channel~~ **closed**: specified in [`pairing-v1.md`](pairing-v1.md)
   (CPace on ristretto255, short code `<channel>-<word>-<word>`, 6-digit verification
   code, frames sealed under keys derived from the PAKE session). It gives the
   two devices the other's authentic public keys (§8.2 step 1). Its final encrypted message
   carries, as an additive field, the `history` transfer announcement (`transfer_id`,
   `items`, `bytes`, §8.7); it never carries the history itself. Its labels
   (`tomi/v1/pair-…`, `tomi/v1/hkdf/pair-…`, `tomi/v1/aad/pair`, `pairing-v1.md` §13)
   complement the registry of §4.2; no byte of format 1 changes.
2. **Relay authentication** (Ed25519 signed challenge, reserved label
   `tomi/v1/sig/relay-auth`): defined in `api-v1.md` §3.6.
3. ~~Encrypted local storage~~ **closed**: local key `LSK` **random, not derived**
   from the space key (no HKDF label), re-sealing of open items (§7.2),
   abandonment of old space keys (§8.3).
4. **Encrypted backup** (the "all devices lost" case, later): could reuse the index of §8.7.3 under a printable recovery key, together with the
   list of known tombstones.
