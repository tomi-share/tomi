# Test vectors — format v1

> **RULE: these files are FROZEN as of the first public release. Once published, they are
> NEVER modified, corrected or reformatted again.** A change of behavior goes through a new
> format (`spec/vectors/v2/`); the v1 vectors stay readable as long as format 1 is supported.
>
> The `SHA256SUMS` file pins the exact content of every `.json`. The test
> `cargo test -p tomi-core --test spec_vectors` fails if a file is modified,
> added or removed without being verified.
>
> A **compatible** addition to format 1 may bring a **new** file, with one more line
> in `SHA256SUMS`; existing files and their lines do not change
> (as for `history-transfer.json`, decision D2, `pin.json`, the "Pin" decision,
> `archive.json`, the "Archive" decision, `reservations.json`, multi-recipient
> reservations, `payload-extensions.json`, repeated keys and unknown fields, and
> `attachment-tombstone.json`, removal of a single attachment).

Normative specification: [`../../format-v1.md`](../../format-v1.md) (the section numbers
`§` cited in the files refer to that document).

## Conventions

- Bytes in lowercase hexadecimal, without separator. Empty string = zero bytes.
- Identifiers (`item_id`, `space_id`, `blob_id`): canonical UUID; encoded as 16 raw
  bytes in the AADs and the signed structures.
- Integers: decimal in the JSON; big-endian in the bytes.
- Devices A, B, C (members) and D (non-member) are defined in `device.json`;
  the other files refer to them by name.
- The "randomness" (CEK, nonces, ephemeral keys) consists of fixed values chosen for the
  tests. **A real implementation draws them at random every time.**
- The JSON keys of these files are sorted alphabetically; the order has no meaning.
- The descriptive texts (`description`, `note`, ...) are in English. The test data themselves
  (item titles and contents, device names, pairing words) are inputs and are not translated:
  some are in French or contain non-ASCII characters on purpose.

## Files

| File | Content |
|------|---------|
| `primitives.json` | External vectors (XChaCha20-Poly1305 draft, RFC 5869, RFC 7748, RFC 8032) |
| `hkdf.json` | Derivation of `K_wrap` and `K_transport` |
| `aad.json` | Canonical encoding of the four AADs |
| `item-text.json` | Complete envelopes: plain text, Unicode secret, composite HTML item (epoch 2) |
| `item-attachment.json` | Item with three attachments (0 bytes, 64 KiB, 64 KiB + 1000) and their STREAM blobs |
| `device.json` | Identities, device certificate, member list, sealed space key |
| `tombstone.json` | Signed tombstones (everywhere and local scope) |
| `rotation.json` | Rotation 1 -> 2: A revokes C; expected result for A, B, C |
| `payload-json.json` | Reading the JSON: unknown fields ignored, unknown values, errors |
| `failures.json` | Expected failures: altered AAD, unknown format, truncation, invalid signatures... |
| `history-transfer.json` | History transfer A -> B (§7.4, §8.7): `sealed_hk` and intermediate steps, 2-part manifest (383 bytes), STREAM parts (part 0 over 2 chunks), JSON Lines index and expected entries, files re-encrypted under `FK'`, payloads (secret without expiry or `read_policy`, masked secret title), and failure cases (`manifest`, `part`, `index`) |
| `pin.json` | Shared, encrypted pin (§8.8): `K_state` (epochs 1 and 2), signed pin/unpin statements (153 bytes), state envelopes (320 bytes), last-writer-wins convergence and `next_at`, epoch ordering rule, `pin` field of the transfer index (with the manifest of `history-transfer.json`), unknown state type ignored, failure cases (`envelope`, `statement`, including `at_ms_above_2_53_minus_1`: `at_ms` = 2^53 correctly signed, refused `malformed_statement`) |
| `archive.json` | Shared, encrypted archive (§8.9), twin of `pin.json` (same devices, same keys, same item): `K_state` (epochs 1 and 2), signed archive/unarchive statements under the label `tomi/v1/sig/archive` (157 bytes, 93-byte TBS), state envelopes (320 bytes), last-writer-wins convergence (tie: archived) and `next_at`, epoch ordering rule (`epoch_rule`: open, `suspect_epoch`, `key_pending`), `kinds` (a pin is not an archive, unknown state type), `archive` field of the transfer index (with the manifest of `history-transfer.json`), failure cases (`envelope`, `statement`, including `at_ms_above_2_53_minus_1`, `unknown_state_byte`, `pin_statement_is_not_an_archive`) |
| `attachment-tombstone.json` | Removal of ONE attachment (§8.10): statement signed under the label `tomi/v1/sig/attachment-tombstone` (189 bytes, 125-byte TBS: `space_id`, `item_id`, `blob_id`, `slot` u32, `deleted_at_ms` u64, signer), edge cases (`slot` = 2^32 - 1, `deleted_at_ms` = 2^53 - 1) and failure cases in the same file (`failures`, including `item_tombstone_is_not_an_attachment_removal`, `attachment_removal_is_not_an_item_tombstone`, `slot_altered`, `blob_id_altered`, `deleted_at_above_2_53_minus_1`, `one_byte_too_many`, `untrusted_signer`, `revoked_signer`: C revoked by `rotation.json` -> `revoked_device`) |
| `payload-extensions.json` | Payload extensibility (§5.4, §5.4.4): `duplicate_keys` (key repeated at any depth, known or not, including through an escape: `malformed_payload`), `accepted` (same key in different objects, different case: read normally), `preserved` (payloads with unknown fields that must survive byte for byte the local re-sealing and the history export) |
| `reservations.json` | Reservations for future multi-recipient sharing (§5.8), without effect in v1: payloads carrying the reserved `audience` field (one device, several recipients, secret, `null`, empty list, unexpected type), each read exactly as its `equivalent_json` without `audience`; complete envelope carrying `audience`, opened under the space key; item and state event whose `key_epoch` is reserved (`0x80000001`): KEY_PENDING, before and after a rotation; correctly signed `KeyRotation` and `MemberList` with a reserved epoch: `malformed_statement` |

## Conformance of another implementation (e.g. Kotlin)

An implementation is conformant if, for each file, it:
1. recomputes all the intermediate and final values from the inputs
   (byte for byte);
2. decrypts / verifies the outputs and obtains `expected_item` or the expected result;
3. fails on each case of `failures.json` (and of `failures` in
   `history-transfer.json`, `pin.json` and `archive.json`) with the `expected_error` category (the codes are those of
   `CoreError::code()`, spec §9.2).

For `history-transfer.json`: `trust_setup.apply_rotation` = first apply the
rotation of `rotation.json` with the keys of `self`; a `part` case decrypts the blob of
`blob_of_part` with the parameters of `decrypt_as_part` after `transform`; an `index`
case reads the `index` text with the valid manifest.

For `pin.json`: an `envelope` case opens `envelope` under `announced_event_id` with the
space key of `open_with_key_epoch` (`item_state_keys`); a `statement` case reads
`wire` then verifies it with the store of `trust_setup` (same `apply_rotation` rule);
`epoch_rule` applies the 1 -> 2 rotation at `seq` `rotation_seq`; `history_index` is read
with the manifest of `history-transfer.json`.

For `archive.json` (same rules as `pin.json`, with the label `tomi/v1/sig/archive` and
the 157-byte statement of §8.9.1): an `envelope` case opens `envelope` under
`announced_event_id` with the space key of `open_with_key_epoch` (`item_state_keys`) and
obtains `expected_error`; a `statement` case reads `wire` with `ArchiveStatement` then
verifies it with the store of `trust_setup` (same `apply_rotation` rule as for `pin.json`);
`epoch_rule` applies the 1 -> 2 rotation of `rotation.json` at `seq` `rotation_seq` and
obtains `expected`; `kinds.unknown_kind` opens as an unknown type (without error); `convergence`
replays `events` in the given order and obtains `expected`; `history_index` is read with the
manifest of `history-transfer.json` and MUST yield `expected_states` (`archive` and
`pin` fields of each `item` line).

For `attachment-tombstone.json`: a case of `vectors` is rebuilt like one of
`tombstone.json` (TBS of §8.10.1, signature of `signer`, `wire` = TBS || signature, exact
length 189) and verified with the store of devices A, B, C; a case of `failures` reads
`wire` with the type `kind` (`attachment_tombstone` or `tombstone`) then, if parsing succeeds,
verifies it with `trust_setup` (including `apply_rotation`, which applies the indicated rotation file), and MUST fail with `expected_error`.

For `payload-extensions.json`: a `duplicate_keys` case reads `json_hex` and MUST fail with
`expected_error`; an `accepted` case MUST yield `expected_item`; a `preserved` case MUST
yield `expected_item`, then the payload re-sealed locally (§7.2) and the `payload` value
of the `item` line of a history index (§8.7.3) MUST be exactly `json_hex`
(compact JSON, no line break).

For `reservations.json`: a `payload` case reads `json_hex` and `equivalent_json_hex` and MUST
obtain two identical items, equal to `expected_item`; an `envelope` case is verified as
in `item-text.json`; an `items` / `item_states` case rebuilds `envelope` from
`inputs` (layout of §5.2 or §8.8.2, arbitrary key), builds the `ring` keyring
(keys, then rotations at `rotation_seq`) and obtains `expected_reason` without decrypting
anything; a `statements` case rebuilds `tbs`, verifies the signature of `signer`, then
MUST fail when decoding `wire` with `expected_error`.

Generator: `crates/core/examples/gen_spec_vectors.rs`
(`cargo run -p tomi-core --example gen_spec_vectors -- spec/vectors/v1`). It never
overwrites anything: it checks that each existing file is regenerated byte for byte, creates
the missing files, and only adds lines to `SHA256SUMS`.
