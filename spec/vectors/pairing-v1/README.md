# Test vectors — pairing v1

> **RULE: these files are FROZEN as of the first public release. Once published, they are
> NEVER modified, corrected or reformatted again.** A breaking change of the pairing
> protocol (`PAIR_VERSION = 2`) opens `spec/vectors/pairing-v2/`; these vectors stay
> readable as long as version 1 is supported.
>
> The `SHA256SUMS` file pins the exact content of every `.json`. The test
> `cargo test -p tomi-core --test pairing_vectors` fails if a file is modified, added or
> removed without being verified. A **compatible** addition may bring a **new** file, with one
> more line in `SHA256SUMS`; existing files and their lines do not change.

Normative specification: [`../../pairing-v1.md`](../../pairing-v1.md) (the `§` cited in the
files refer to that document; `format-v1.md` is authoritative for the reused signed and sealed
bytes). These vectors do not touch `spec/vectors/v1/`: format 1 does not change
(`pairing-v1.md` §13), and devices A, B, C are those of `../v1/device.json`.

## Conventions

- Bytes in lowercase hexadecimal, without separator. Empty string = zero bytes.
- Integers: decimal in the JSON; big-endian in the bytes.
- The "randomness" (`sid`, `ya`, `yb`, ephemeral secret and nonce of the sealing, transfer
  identifier) consists of fixed values chosen for the tests. **A real implementation draws them at
  random for each session** (`ya` and `yb`: 32 bytes whose 4 high bits of the last byte
  are set to zero, `pairing-v1.md` §3.3).
- The JSON keys of these files are sorted alphabetically; the order has no meaning.
- All text inputs (`input`) are also given as hexadecimal UTF-8 (`input_utf8`): the
  normalization form (NFC or NFD) is part of the vector.
- The descriptive texts (`description`, `note`, `why`, ...) are in English. The test data themselves
  (pairing words, device names, text inputs with accents) are inputs and are not translated.

## Files

| File | Content |
|------|---------|
| `cpace-draft21.json` | The vectors of **draft-irtf-cfrg-cpace-21**, appendix B.3 (ristretto255, SHA-512, initiator-responder mode), reproduced byte for byte: `generator_string` (170 bytes), `g`, `Ya`, `Yb`, `K`, `ISK`, the transcript `transcript_ir`. Invalid inputs of B.3.11 (`Y_i1` that does not decode, neutral element): `scalar_mult_vfy` yields `I` and the protocol aborts. `prepend_len` (A.1.2) and `lv_cat` (A.1.4) vectors. |
| `wordlist.json` | List of 2048 words (appendix B): fingerprints of the original list and of its folded form (16,001 bytes), ranks (`abaisser` 0, `crayon` 465, `école` 616, `élève` 642, `renard` 1633, `tomate` 1891, `zoologie` 2047), PRS, **reading** of a typed code (§4.4) and of a link (§4.3), display of a code. |
| `pairing.json` | A complete, deterministic session (`relay_authority = relay.example.org`, channel `4821`, words `crayon` / `renard`, `sid = 000102…0f`, `ya` / `yb` of the draft). Inputs, intermediate values (`PRS`, `CI`, `ADa`, `ADb`, `gen_str`, `g`, `Ya`, `Yb`, `commit`, `K`, `ISK`, `K_I`, `K_J`, `SAS`), the 7 frames in hexadecimal with their AAD, nonce and clear body, the decoded batch (`DeviceCert`, `MemberList`, sealed key, history announcement), the `TrustStore`s of N and A, a second session for the replay (P14), the scenario of the relay that knows the code (P11, `mitm`) and **143 failure cases** P9 to P16 (`failures`). |

### Inputs of `pairing.json`

A (`PC-Bureau`, `windows`) invites; B (`Pixel`, `android`) is the new device; C is already a member
of A's space (the member list therefore holds A, B and C, as in `device.json`). A offers a
history of 812 items (356,515,840 bytes) and B's user accepts it: `Provision`
carries the `history` announcement. `member_count = 2`. `now_ms = 1793750460000`.

## Failure cases (`failures`)

A case drives **one** machine, the *receiver* (`inviter` = A, `joiner` = N), with frames read
from the file: all the frames of the other side are fixed bytes, as for an
implementation in another language.

```json
{ "id": "…", "pxx": "P12", "description": "…", "receiver": "joiner" | "inviter",
  "setup": { "joiner_ranks": [466, 1633], "joiner_yb": "hex", "offer": false },
  "steps": [
    { "do": "receive", "frame": FRAME, "expect": "ok" | "ignored" | CODE, "produces": FRAME },
    { "do": "user_confirm" },
    { "do": "joiner_confirm", "history": true, "produces": FRAME },
    { "do": "provision", "produces": FRAME } ],
  "final_error": CODE | null }
FRAME = { "nominal": k }  frame k of `frames` (0 commit … 6 joiner_ack)
      | { "other": k }    frame k of `other_session.frames`
      | { "hex": "…" }    bytes
      | { "padded": { "prefix": "hex", "len": n } }   the prefix followed by zero bytes up to n bytes
```

- `setup` (optional): words typed by N (`joiner_ranks`), N's scalar `yb`, `offer = false` if A
  does not offer a history. The other inputs are those of `inputs`.
- `receive`: gives the frame to the receiver. `expect` is `ok`, `ignored` (frame after the last
  expected frame) or a core error code (`pairing_failed`, `pairing_unsupported`).
  `produces`, if present, is the frame the receiver sends back.
- `user_confirm`: A's user presses "Add" (after the `joiner_hello`).
  `joiner_confirm`: N's user accepts, with or without the history. `provision`: A builds
  the batch (both humans have confirmed) and sends back the frame.
- The **first error** ends the case: its code is the `expect` of the step (when exact) and
  the `final_error` of the case. A case with `final_error: null` ends without error.
- The frames of a step that fails after an `ok` (for example a `commit` whose `sid` byte is
  altered) show that **a clear frame is not authenticated when it is read**: the session fails at
  the next frame, which is the authentic one.

Families: **P9** wrong code (neighboring word, invalid point, neutral element); **P10** betrayed
commitment (with the control where the commitment matches); **P12** bit flipped in each frame
(header, start, middle, end, label), type `0x08`, truncated frame, too long, of 8193 bytes,
version 2; **P13** disorder, replay towards the sender, frames after the last expected one; **P14**
replay of a frame from another session; **P15** TLV bodies (unknown tag `0x40` ignored, `0x90`
critical, duplicate, disorder, missing field, lengths, invalid `name` and `platform`); **P16**
rejected batch (certificate, list, sealed key, epoch, `space_id`), with the core error code
behind `pairing_failed` in `core_error` (informational: the machine only returns `pairing_failed`).
P11 is the `mitm` block; P17 (no secret in the logs) is a core test, not a vector.

For the P15 and P16 cases, `body` is the clear body and the frame is that body sealed with the keys of
the nominal session (`K_I` or `K_J`, counter of §5.4).

## Conformance of another implementation (e.g. Kotlin)

An implementation is conformant if, for each file, it:

1. recomputes all the intermediate and final values from the inputs (byte for byte),
   without going through a PAKE library;
2. opens each box (`box`) of `frames` with the indicated key, nonce and AAD and recovers the
   clear body, verifies the signatures of the batch (strict Ed25519) and opens the sealed key with the keys
   of B;
3. produces the 7 frames of `pairing.json` from its inputs alone (`sid`, `ya`, `yb`, fixed randomness of
   the sealing and of the transfer identifier), then replays each case of `failures` and obtains
   the same errors at the same steps;
4. reads the codes and links of `wordlist.json` as indicated, including the accented or
   decomposed forms and the full-width characters.

For `cpace-draft21.json`, the implementation must also find `scalar_mult_vfy = I` (and abort)
for `Y_i1` and for the neutral element.

## Generator

`crates/core/examples/gen_pairing_vectors.rs`
(`cargo run -p tomi-core --example gen_pairing_vectors -- spec/vectors/pairing-v1`). It never
overwrites anything: it checks that each existing file is regenerated byte for byte, creates the missing
files, and only adds lines to `SHA256SUMS`. Each value is computed by
the independent reference implementation `crates/core/tests/common/pairing_reference.rs` (primitives
and text of the specification only), compared with the state machines of `tomi-core`
(`pairing::kat`), and each failure case is replayed before being written.
