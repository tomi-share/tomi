# Tomi — Pairing specification, version 1

Status: **normative** for version 1 of the pairing protocol (`PAIR_VERSION = 1`).
Closes open point 1 of `spec/format-v1.md` §13.
Related documents: `spec/format-v1.md` (authoritative for every signed or sealed byte reused here:
`DeviceCert`, `MemberList`, sealed space key), `spec/api-v1.md` (authoritative for the
relay routes, in particular §4.6 "Pairing rendezvous", §4.3 `POST /v1/devices`, §3.2 `POST /v1/space`,
§4.13 and §7.8 history transfer).
Reference implementation: to be written in `crates/core` (module `pairing`, §15).
Vectors: to be created in `spec/vectors/pairing-v1/` (§14), frozen once published.

The words **MUST**, **MUST NOT**, **SHOULD**, **MAY** have the meaning of MUST, MUST NOT, SHOULD, MAY
in RFC 2119. Annexes A and B are not normative.

---

## 0. Notation and vocabulary

Notation of `format-v1.md` §0 (`||`, `u8`, `u16be`, `u32be`, `u64be`, `"text"`, `XC_Seal`,
`XC_Open`, `HKDF(sel, ikm, info, L)`, `uuid16`, `hex`), plus:

- `SHA-256(m)`, `SHA-512(m)`: FIPS 180-4.
- `prepend_len(s)`: the length of `s` encoded as LEB128, followed by `s`
  (draft-irtf-cfrg-cpace-21 §6.3 and annex A.1). A length < 128 fits in one byte.
- `lv_cat(a0, a1, …)`: `prepend_len(a0) || prepend_len(a1) || …`.
- `zero_bytes(n)`: `n` zero bytes.
- ristretto255: the prime-order group of RFC 9496. `encode` / `decode` are its canonical
  32-byte encoding functions; `decode` fails on any non-canonical encoding.
  `element_derivation(b)` is the 64-byte element derivation (RFC 9496 §4.3.4).
  `I` is the encoding of the identity element (32 zero bytes).

| Term | Meaning |
|-------|------|
| **A** (inviter) | a device already a member of the space, which creates the channel and displays the code. CPace *initiator* role. |
| **N** (new device) | an unprovisioned device joining the space. CPace *responder* role. |
| **channel** | an ephemeral mailbox on the relay (`api-v1.md` §4.6), identified by `channel_id`. |
| **pairing code** | `<channel_id>-<word1>-<word2>` (§4), displayed on A, typed or scanned on N. |
| **code secret** | the two words. The only secret part of the code. |
| **frame** | an opaque message deposited in the channel (`body` of `api-v1.md` §4.6), defined in §6. |
| **verification code** | 6 digits derived from the session (§5.3), shown on A and N and compared by eye. |
| **pairing batch** | the content of the `provision` frame (§8): `space_id`, `key_epoch`, sealed key, `DeviceCert(N)`, `MemberList`, history announcement. |

Constants of version 1:

| Constant | Value |
|-----------|-----------|
| `PAIR_VERSION` | `1` |
| CPace suite | `CPACE-RISTR255-SHA512` |
| Words per code | 2, drawn from the 2048-word list of annex B (11 bits each, 22 bits of secret) |
| Verification code | 6 decimal digits |
| Maximum frame size | 8192 bytes (= default `pairing_max_message_bytes`) |
| Frames written by A / by N | 3 / 4 (≤ `pairing_max_messages` = 8) |

---

## 1. Scope

Pairing brings a new device N into the space, on the invitation of a member A. It performs
step 1 of the sequence of `format-v1.md` §8.2 (**authenticated** exchange of the public keys
`(ed_pk, x_pk)`) and carries step 2 (the pairing batch); steps 3 and 4 (creation of N's
store, publication of `DeviceCert(N)` by A) are repeated here with the exact calls of the core
and of the relay.

This document defines: the short code and the QR (§4), the PAKE suite and the derivation of the
session keys (§3, §5), the frame format (§6), their sequence on the relay channel (§7), the construction
and verification of the batch (§8), the history transfer announcement (§9), space creation by the
first device (§10), errors (§11) and secret hygiene (§12).

Out of scope: the history transfer itself (`format-v1.md` §8.7, `api-v1.md` §4.13, §7.8),
authentication to the relay (`api-v1.md` §3.3), key rotation.

---

## 2. Threat model of the channel

### 2.1 Stakes

Pairing is the only moment when trust between devices is created outside the certificate chain
(`format-v1.md` §8.4, rule 1 "face-to-face verification"). Afterwards, the confidentiality of the
**content** no longer depends on the channel: everything is encrypted end to end under the space key. But an
error here compromises everything: if A seals the space key for an attacker's `x_pk`, the attacker reads
the entire space until its revocation; if N accepts an attacker's `MemberList`, the user sends
their secrets into the attacker's space.

### 2.2 Adversaries

| Adversary | Capabilities | What stops it |
|------------|-----------|-----------------|
| **Hostile relay** (compromised EC2) | sees and withholds all messages; invents, replays, reorders, deletes; lies about the channel state; plays A to N and N to A | CPace: without the code secret, a single attempt per session, no offline attack; commitment + verification code (§5.3) against a relay that knows the code; all useful bytes are sealed and bound to the session (§5.4). |
| **Network attacker** | between the devices and the relay | TLS (`api-v1.md` §2.1), then the same protections as against the relay. |
| **Attacker who joins the channel** | knows or guesses `channel_id` (not secret, 1 to 9999), makes the `claim` before N | a single `claim` per channel (`api-v1.md` §4.6); a code guess succeeds with probability 2^-22; failure ⇒ channel closed by A; the legitimate N receives `channel_claimed` and the user does not confirm on A (§7.4). |
| **Attacker who has seen the code** (photo of the QR, screen sharing) | knows the code secret | must win the race to the `claim` (then the legitimate N fails visibly) or also be the relay; in that case, the commitment makes the verification code unpredictable to them: P(both codes coincide) = 10^-6. |
| **Malicious web page** | opens `tomi-share://` or an App Link | no pairing starts without a user action (§4.4 rule 1). |
| **Unexpected relay for an installation token** (trapped creation link, token pasted or typed next to an unexpected "Relay" field) | receives the installation token the user sends it, and can create *its own* space with it | the displayed relay is the one that receives the token (§10.3); the user MUST read it before confirming. **Accepted limit: a token pasted or typed for an unexpected relay is exposed to that relay**: if it was the token of the right relay, whoever runs the other relay can replay it to the right relay and create the space there in place of the user; the right to create the space on the right relay is therefore lost and the token must be replaced on the legitimate relay (`admin invite --rotate`, `api-v1.md` §8.7). No key is exposed: the token only authorizes the creation of a space, and the relay never sees the founder's keys (§10.2). |
| **Unexpected relay** (trapped QR or link: `https://relay-of-the-attacker/l/pair#…`) | has their QR scanned; hosts a relay and a space that are theirs | the link only **proposes** a relay: it is displayed in full with the verification code, and it is kept durably only once pairing succeeds (§4.4 rules 6 and 7; before that, only the pending record carries it: it is written with the sealed batch, **after** the user's confirmation, so that a resumption goes to the right relay, and it is forgotten with that batch), and the verification code binds the session to this relay (`relay_authority` enters `CI`, §3.3): the attacker only obtains the right to present *their own* relay and space, never to enter the victim's or read anything in it. The user MUST read the relay and compare the code with the inviting device. Accepted limit: a user who confirms without reading joins the attacker's space (and then entrusts their future items to it); this is the same gesture as confirming a verification code without comparing it (out of scope below). |

Out of scope: an already compromised A or N device; a user who confirms a verification code that
differs or is absent on the other device.

### 2.3 Required properties

1. **Authenticity of the keys**: at the end, N holds A's `(ed_pk, x_pk)` and A holds N's,
   as each sent them, or pairing fails.
2. **A single attempt**: an active adversary tests at most one code secret per session; a
   failed session consumes the channel (both sides close it, §11.2).
3. **No offline attack**: a passive adversary, relay included, learns nothing that lets them
   test codes offline.
4. **Confidentiality of the batch**: the batch is readable only by N (sealed channel, and the space key
   additionally sealed for N's `x_pk`, `format-v1.md` §7.3).
5. **Consent on both sides**: A sends the space key only after the user's confirmation
   on A **and** on N; N is registered on the relay only after having verified and accepted the batch.
6. **Blind relay**: the relay sees only `channel_id`, sizes, times and opaque bytes.
   The sizes let one guess the length of the device names and the number of members, which the relay
   already knows (`api-v1.md` §1.2).

---

## 3. Choice of PAKE: CPace on ristretto255

### 3.1 Decision (recommendation, to be validated by the maintainers, annex A D-A1)

Pairing uses **CPace**, suite `CPACE-RISTR255-SHA512` of draft-irtf-cfrg-cpace-21 (April 2026)
§8.3, in *initiator-responder* mode, A as initiator, N as responder. The CPace layer (≈ 150 lines) is
written in `tomi-core` **on top of the primitives already present**:

| CPace need | Provided by | Already in `Cargo.lock` |
|--------------|-----------|------------------------|
| ristretto255 group, canonical `decode`/`encode`, `element_derivation`, scalar multiplication | `curve25519-dalek` 5.0.0 (`RistrettoPoint::from_uniform_bytes`, `CompressedRistretto::decompress`, `Scalar`) | yes (dependency of `x25519-dalek` 3 and `ed25519-dalek` 3) |
| SHA-512 | `sha2` 0.11 | yes |
| HKDF-SHA256, SHA-256 | `hkdf` 0.13, `sha2` 0.11 | yes |
| channel AEAD | `chacha20poly1305` 0.11 (XChaCha20-Poly1305) | yes |
| randomness | `getrandom` 0.4 | yes |

`curve25519-dalek` becomes a **direct** dependency of `tomi-core`, at the same version already
resolved: no new crate enters the tree, on either the clients or the relay side.

### 3.2 Justification

- **Maturity of the protocol.** CPace is the balanced PAKE retained by the CFRG at the end of its
  2020 selection, with a security proof in the UC model (AHH21, cited by the draft §10) and test vectors for each
  suite (annex B of the draft). It has no parameters of "initial trust": the generator is derived from the
  password by hashing. SPAKE2 (RFC 9382, informational) depends on two fixed points M and N whose discrete
  logarithm nobody must know.
- **Libraries.** No Rust PAKE library is audited. `spake2` (RustCrypto) states it has
  "never received an independent third party audit"; its published version targets
  `curve25519-dalek` 4 (a second copy of dalek in the tree), its branch for dalek 5 is a
  pre-release, and its encoding is that of magic-wormhole, not that of RFC 9382.
  `pake-cpace` follows an old draft. Writing the CPace layer on `curve25519-dalek` (Quarkslab audit
  2019, `format-v1.md` §2.1) keeps audited primitives and does not reimplement **any
  primitive**: only the composition is written, and it is verified byte for byte against the
  B.3 vectors of the draft (§14.1).
- **Why ristretto255 rather than X25519.** The X25519 suite of CPace requires the Elligator2 map,
  which `x25519-dalek` does not expose: it would have to be written (a primitive). ristretto255 provides a
  prime-order group without cofactor, a `decode` that rejects invalid encodings, and the
  element derivation of RFC 9496 in dalek.
- **Honest caveat.** CPace is still an Internet-Draft. The suite inputs (§3.3) are frozen
  by this document and by our vectors: a change in the draft changes nothing for Tomi as long as
  `PAIR_VERSION = 1`. The CPace layer is sensitive code: independent review is mandatory (§15).

Discarded alternatives: annex A.2.

### 3.3 Normative instantiation

```
H            = SHA-512                               s_in_bytes = 128
G.DSI        = "CPaceRistretto255"                   (17 bytes)
PRS          = u16be(i1) || u16be(i2)                (4 bytes; i1, i2 = ranks of the two words, annex B)
CI           = lv_cat("tomi/v1/pair-ci", relay_authority, channel_id)
sid          = 16 random bytes drawn by A, sent in clear in the `commit` frame
ADa          = "tomi/v1/pair-inviter" || 0x00 || u8(PAIR_VERSION)       (22 bytes)
ADb          = "tomi/v1/pair-joiner"  || 0x00 || u8(PAIR_VERSION)       (21 bytes)

gen_str      = lv_cat(G.DSI, PRS, zero_bytes(104), CI, sid)
g            = element_derivation(SHA-512(gen_str))
sample_scalar() : 32 random bytes b; b[31] &= 0x0f; little-endian integer (< 2^252 < group order)

A : ya = sample_scalar() ; Ya = encode(ya · g)
N : yb = sample_scalar() ; Yb = encode(yb · g)

scalar_mult_vfy(y, X) = I if decode(X) fails, otherwise encode(y · decode(X))
A : K = scalar_mult_vfy(ya, Yb) ;  N : K = scalar_mult_vfy(yb, Ya) ;  K = I ⇒ abort (§11)

ISK          = SHA-512( lv_cat("CPaceRistretto255_ISK", sid, K) || lv_cat(Ya, ADa) || lv_cat(Yb, ADb) )   (64 bytes)
```

- `relay_authority`: the authority (host, and port if not 443) of the relay **retained by the device**
  for this session, in lowercase ASCII (e.g. `relay.example.org`). For A, it is the relay of its space.
  For N, it is the one from the link or QR, or the one typed next to the code, or the one the device
  already had; it may have been **read** from a link or a field, in which case it is not reliable by itself and
  the user confirms it (§4.4 rules 6 and 7). This change of origin modifies neither the computation of
  `CI` nor the vectors: only the value of `relay_authority` as input changes source, and a
  device whose relay differs from that of the inviter obtains a different verification code
  (the session fails, §11.1).
- `channel_id`: the canonical decimal form (1 to 4 digits, no leading zero), in ASCII.
- The padding length 104 equals `128 − len(prepend_len(PRS)) − len(prepend_len(G.DSI)) − 1`
  (draft §8.1) = 128 − 5 − 18 − 1; it is constant because `PRS` is always 4 bytes long.
- The role identities are in `ADa`/`ADb` and are **not** transmitted: each side knows them.
  A different `PAIR_VERSION` changes `ADa`/`ADb`, hence `ISK`: a forced downgrade by
  the relay makes the session fail.
- A device plays only one role per channel: A never acts as responder on a channel it created
  (protection against reflection, draft §10.1.2). Structurally, N is not provisioned and A is.
- `K` MUST never leave the CPace layer nor be logged (draft §10.3).

Example, `relay_authority = "relay.example.org"`, `channel_id = "4821"`, words `crayon` (rank 465) and
`renard` (rank 1633), `sid = 000102…0f`:

```
PRS = 01d1 0661
CI  = 0f 746f6d692f76312f706169722d6369              "tomi/v1/pair-ci"
      11 72656c61792e6578616d706c652e6f7267          "relay.example.org"
      04 34383231                                    "4821"                       (39 bytes)
ADa = 746f6d692f76312f706169722d696e7669746572 00 01
ADb = 746f6d692f76312f706169722d6a6f696e6572 00 01
gen_str = 11 435061636552697374726574746f323535      "CPaceRistretto255"
          04 01d10661                                PRS
          68 00…00 (104 bytes)                       padding
          27 <CI, 39 bytes>
          10 000102030405060708090a0b0c0d0e0f        sid                          (185 bytes)
```

---

## 4. Pairing code, link and QR

### 4.1 Form

```
code = channel_id "-" word1 "-" word2          e.g. 4821-crayon-renard
```

- `channel_id`: the one returned by `POST /v1/pairing/channels` (not secret).
- `word1`, `word2`: two ranks `i1`, `i2` drawn **independently** and uniformly in `[0, 2047]`
  (11 bits of CSPRNG randomness each, without rejection), displayed in the **folded** form of annex B:
  lowercase ASCII, without accents. The two words MAY be equal.
- Secret: 22 bits. Justification: an adversary has only one attempt per channel (§2.3); a channel can
  be created only by a member and lives 120 s; each failed attempt is visible (the channel closes, A
  displays a new code). Probability of success per attempted pairing: 2^-22 ≈ 2.4·10^-7, even before
  the comparison of the verification code. 3-word variant (33 bits): annex A, D-A2.

The code is **single-use** and dies with its channel (expiration, failure or success).

### 4.2 Display (A)

A displays the full code, in a fixed-width font, words separated by hyphens
(`4821-crayon-renard`), and next to it the QR of the link (§4.3). It MUST NOT display the code before having
deposited the `commit` frame (§7.2, step A3). It SHOULD display the remaining time (`expires_at` of the channel)
and automatically produce a new channel and a new code at expiration as long as the screen stays
open (within the limit `rl_device_pairing`, `api-v1.md` §8.5).

### 4.3 Link and QR

```
link = "https://" relay_authority "/l/pair#" code         e.g. https://relay.example.org/l/pair#4821-crayon-renard
```

- The QR encodes **exactly** this link (byte mode, error correction M recommended). It carries the same
  information as the typed code: there is only one secret per channel, whatever the input mode.
- The code is in the **fragment**: a browser never sends it to the server. It may remain in the
  browser history if the application is not installed (decision D7); this is accepted because the
  code is single-use and expires in 2 minutes.
- The page served by the relay at `/l/pair` MUST be static, with no script that reads `location.hash`,
  and no third-party resource; it only offers the download of the application.
- Fallback form `tomi-share://pair#<code>`: a client MAY accept it (Windows deep link). A QR
  produced by Tomi always uses the `https` form. It names no relay: N then uses the one
  it already has (or the one typed next to the code).
- `relay_authority` of the link is that of A (the relay of its space): A has no other choice.
- **No path prefix in a link.** The link (`/l/pair`, and `/create` in §10.3) carries only
  the authority of the relay: the path is fixed, and a reader rebuilds from `https://<authority>` only
  the origin. A relay served under a prefix (`https://host/prefix`) therefore works **only** through the
  "Relay" field (« Relais » in the French UI; full URL, prefix included); the QR and the link of such a relay, which would designate the origin
  without the prefix, do not lead there.

### 4.4 Reading (N)

Input: a typed code, or a link (QR scan, App Link, `tomi-share://pair`).

1. **Link**: `https` scheme and an authority that is that of a valid relay (ASCII host, valid port,
   no user information; otherwise refusal: "This QR code is not a Tomi pairing code"),
   path `/l/pair`, then code = fragment; the authority of the link is the **candidate relay** (rules 6 and 7);
   or scheme `tomi-share`, host
   `pair`, code = fragment. No other parameter is read. A link **never** starts pairing
   by itself: the application displays the pairing screen and waits for "Join" (`DeepLink.Pair`).
2. **Normalization**: remove leading and trailing whitespace; normalize to NFKD; remove combining
   marks (category Mn); convert to lowercase; split on any sequence of characters that are neither
   ASCII letters nor ASCII digits. A reader MAY, instead of implementing NFKD in full,
   locally refuse the code if it contains a letter or digit (Unicode) outside the ranges it can fold
   exactly (at minimum ASCII, Latin-1 Supplement and Latin Extended-A
   `U+00A0`–`U+017F`, combining marks `U+0300`–`U+036F`, full-width forms `U+FF10`–`U+FF5A`):
   otherwise folding such a character into a separator could make it read as a **different** valid word
   (a removed ligature leaves a prefix of another word). This refusal is local (§4.4 rule 5).
3. **Tokens**: exactly 3. The first: 1 to 4 digits, no leading zero, value 1 to 9999. The next two:
   letters `a`–`z` only.
4. **Words**: a token equal to a folded word of annex B gives its rank. Otherwise, if it is at least
   4 letters long and is the prefix of only one word, it gives the rank of that word (the first 4 letters
   are enough to distinguish the 2048 words, annex B); otherwise: invalid code.
5. An invalid code is refused **locally**, before any `claim`: the channel is not consumed by
   a format typo. (A typo that lands on another valid word, however, consumes the attempt.)

6. **Candidate relay.** N's relay is, in order: the authority of the `https` link; the relay typed
   next to the code (field "Relay", URL `https://host[:port]`); the relay the device already has. A link
   and a field that name two different relays are refused. Without any relay, reading fails
   locally (`relay_not_configured`), nothing is sent. The format of the typed code does not change; the relay
   is a separate parameter.
7. **A relay that was read is not reliable.** Whoever shows the QR chooses the relay (§2.2). The application MUST
   display the authority of the candidate relay **in full**, on the verification screen, with the
   6-digit code, and MUST **store nothing** (no relay, no keys, no batch) before the user's
   confirmation; the relay is kept durably only once pairing succeeds (it is written
   in the provisioning transaction, §8.4). The client switches to the candidate for the duration of the
   session; if it fails, is cancelled or expires, it reverts to the previous relay (or "not configured").
   The pending sealed batch (§8.3) keeps the relay in the pending record, so that resumption
   (§11.4) goes to the right relay after a stop.
8. **Provisioned device.** Its relay is that of its space and does not change: no link or field
   can modify it, and a link to another relay is refused. **Exception, only once**: a device
   provisioned by a version that did not keep its relay, and opened by a build without a default relay,
   has no relay (it can neither synchronize nor be woken up); it then accepts the relay from the "Relay" field
   (`set_relay`), which becomes that of its space, permanently: once kept, it no longer changes. A
   provisioned device that has a relay always refuses it.
9. **Only one switch at a time.** Decisions about the relay (start a session, `set_relay`, end a
   session and return to the kept relay) are atomic. Sessions to the **same** relay may
   overlap; a session that requests another relay, `set_relay`, or the resumption of a founder whose keys await another
   relay (§10.3) are refused (`InvalidInput`) as long as another session is in progress:
   the transport is never repointed under a session that has already sent its `pairing_token` on it.

The interface SHOULD offer word autocompletion from 4 letters.

---

## 5. Session keys, AEAD and verification code

### 5.1 Commitment of A

```
commit = SHA-256("tomi/v1/pair-commit" || 0x00 || sid || Ya)        (32 bytes; input of 68 bytes)
```

A publishes `commit` before `Ya`, and reveals `Ya` only after having received `Yb`. Reason: against a relay
that would know the code (§2.2), the verification code must not be chosen by anyone. On A's side, the
relay answers `Yb'` without knowing `Ya`; on N's side, it has committed to its `Ya''` before seeing `Yb`.
Neither of the two codes can be "searched" offline to coincide with the other.
`Ya` is a random element of the group: `commit` reveals nothing about the code (`Ya` is independent of the
secret as long as `ya` is unknown).

### 5.2 Derivation

```
K_I  = HKDF(∅, ISK, "tomi/v1/hkdf/pair-inviter", 32)       seals the frames A → N
K_J  = HKDF(∅, ISK, "tomi/v1/hkdf/pair-joiner", 32)        seals the frames N → A
s8   = HKDF(∅, ISK, "tomi/v1/hkdf/pair-sas", 8)
SAS  = u64be(s8) mod 1 000 000                             6 digits, leading zeros included
```

`ISK` already binds `sid`, `CI` (relay and channel), `PRS`, `Ya`, `ADa`, `Yb`, `ADb`: no other value enters
the derivation. The bias of the reduction modulo 10^6 is below 10^-13.

### 5.3 Verification code

The SAS is displayed on A and on N, grouped by 3 (`482 913`). The user confirms on **each**
device that the two codes are identical. On A, the screen also says that the new device must
display the same code: if the other device displays an error (channel already taken), the user
refuses. The SAS is **never** typed on the keyboard, transmitted, logged, nor shown in a
notification. It protects against a code seen by a third party (photo of the QR); without a leak of the code, the PAKE
already suffices. Decision to be validated: annex A, D-A3.

### 5.4 Sealing of frames

```
nonce(n) = zero_bytes(20) || u32be(n)                                         (24 bytes)
AAD(t)   = "tomi/v1/aad/pair" || 0x00 || u8(PAIR_VERSION) || u8(t) || sid      (35 bytes)
box      = XC_Seal(K_dir, nonce(n), AAD(t), body)
```

| Frame (type `t`) | Key | `n` |
|------------------|------|-----|
| `reveal` (0x03) | `K_I` | 0 |
| `provision` (0x06) | `K_I` | 1 |
| `joiner_hello` (0x04) | `K_J` | 0 |
| `joiner_confirm` (0x05) | `K_J` | 1 |
| `joiner_ack` (0x07) | `K_J` | 2 |

The nonces are **deterministic** (an accepted exception to `format-v1.md` §2.2): `K_I` and `K_J` are
specific to a session (fresh randomness `ya`, `yb`) and each seals at most 3 frames of distinct types;
no pair (key, nonce) repeats. The directions have distinct keys; the type is in the AAD:
a frame can neither be reordered, nor replayed in the other direction, nor mistaken for another.

The first `box` received on each side serves as **explicit key confirmation** (draft §10.4): opening it
proves that the other side derived the same `ISK`, hence that it has the same code and saw the same
transcript. Its failure is the failure of the code (§11.1).

Example: `AAD(0x03)` with `sid = 000102…0f`:
`746f6d692f76312f6161642f70616972 00 01 03 000102030405060708090a0b0c0d0e0f`.

---

## 6. Frames

### 6.1 Common header

```
frame = u8(PAIR_VERSION) || u8(type) || payload
```

A frame is 2 to 8192 bytes long. The relay carries it as `b64` in `body` (`api-v1.md` §4.6);
it has no meaning for the relay.

| Type | Name | Meaning | Payload | Frame size |
|------|-----|------|--------|--------------------|
| `0x01` | `commit` | A → N | `sid (16) || commit (32)` | 50 |
| `0x02` | `share` | N → A | `Yb (32)` | 34 |
| `0x03` | `reveal` | A → N | `Ya (32) || box(K_I, 0, InviterHello)` | 50 + body, ≤ 438 |
| `0x04` | `joiner_hello` | N → A | `box(K_J, 0, JoinerHello)` | 18 + body, ≤ 382 |
| `0x05` | `joiner_confirm` | N → A | `box(K_J, 1, JoinerConfirm)` | 22 |
| `0x06` | `provision` | A → N | `box(K_I, 1, Provision)` | 18 + body = 552 + 64·m with history announcement (517 + 64·m without), m = members of the `MemberList` |
| `0x07` | `joiner_ack` | N → A | `box(K_J, 2, empty body)` | 18 |

The sizes of the clear frames (`commit`, `share`) are exact: one byte more or less ⇒
`pairing_failed`. Other types: reserved; received in v1 ⇒ `pairing_failed`.

### 6.2 Sealed bodies: TLV list

```
body = tlv*              tlv = u8(tag) || u16be(len) || value(len)
```

Rules (reader):

1. The `tag`s are **strictly increasing**: a duplicate or a disorder ⇒ `pairing_failed`.
2. A truncated TLV (the body ends in the middle) ⇒ `pairing_failed`.
3. An unknown `tag` from `0x01` to `0x7F` is **ignored** (additive extension). An unknown `tag` from `0x80` to
   `0xFF` is **critical**: ⇒ `pairing_unsupported` ("update Tomi").
4. A mandatory field missing, or of a length different from that of the table ⇒ `pairing_failed`.
5. A v1 emitter writes only the tags of this document, in increasing order, each once.

Value types: `ed_pk`, `x_pk`: 32 bytes; `name`: UTF-8, 1 to 256 bytes, conforming as is to the `name` type
of `api-v1.md` §2.3 (1 to 64 characters, no leading or trailing whitespace, no Cc character);
`platform`: ASCII `^[a-z0-9-]{1,32}$`.

### 6.3 `InviterHello` (in `reveal`)

| Tag | Field | Length | Req. | Meaning |
|-----|-------|----------|--------|------|
| `0x01` | `ed_pk` | 32 | yes | A's Ed25519 key |
| `0x02` | `x_pk` | 32 | yes | A's X25519 key |
| `0x03` | `name` | 1–256 | yes | A's name (displayed on N) |
| `0x04` | `platform` | 1–32 | yes | A's platform |
| `0x05` | `member_count` | 2 (`u16be`) | no | current number of members, A included ("Your space has 3 devices") |
| `0x06` | `history_offer` | 16 (`u64be(items) || u64be(bytes)`) | no | estimate of what A can transfer (§9). Absent, or `items = 0`: nothing to offer. |

### 6.4 `JoinerHello` (in `joiner_hello`)

| Tag | Field | Length | Req. | Meaning |
|-----|-------|----------|--------|------|
| `0x01` | `ed_pk` | 32 | yes | N's Ed25519 key (generated before this frame) |
| `0x02` | `x_pk` | 32 | yes | N's X25519 key |
| `0x03` | `name` | 1–256 | yes | N's name; A reuses it for `POST /v1/devices` |
| `0x04` | `platform` | 1–32 | yes | N's platform; idem |

### 6.5 `JoinerConfirm` (in `joiner_confirm`)

| Tag | Field | Length | Req. | Meaning |
|-----|-------|----------|--------|------|
| `0x01` | `history` | 1 | yes | `0x00`: no; `0x01`: yes (§9); any other value ⇒ `pairing_failed` |

This frame exists only if the user **accepted** on N. A refusal on N closes the channel (§11.2).
Its size is constant: the relay does not learn the answer from it.

### 6.6 `Provision` (in `provision`)

| Tag | Field | Length | Req. | Meaning |
|-----|-------|----------|--------|------|
| `0x01` | `space_id` | 16 | yes | `uuid16(space_id)` |
| `0x02` | `key_epoch` | 4 (`u32be`) | yes | current epoch, `1 ≤ e ≤ 2^31 − 1` (`format-v1.md` §5.8.2) |
| `0x03` | `sealed` | 104 | yes | space key `SK_e` sealed for N (`format-v1.md` §7.3) |
| `0x04` | `device_cert` | 208 | yes | `DeviceCert(N)` signed by A (`format-v1.md` §8.2) |
| `0x05` | `member_list` | 152 + 64·m | yes | `MemberList` signed by A |
| `0x06` | `history` | 32 (`uuid16(transfer_id) || u64be(items) || u64be(bytes)`) | no | history transfer announcement (§9; `format-v1.md` §13 point 1) |

Size: the frame fits in 8192 bytes as long as `m ≤ 119`. If the `MemberList` does not fit, A MUST NOT
split it: it fails before sending anything (`pairing_failed`, "space too large for a pairing").
With `max_devices = 20` (default), the frame is at most 1832 bytes.

### 6.7 `joiner_ack`

Empty body: the frame contains only the AEAD tag. It means "N has verified the batch and kept it
pending" (§8.3).

---

## 7. Sequence on the relay channel

### 7.1 Overview

The numbers in square brackets are the order of the frames; the actual `index` of the relay is common to both sides
and depends on the writes.

```
A (inviter)                                  relay §4.6                              N (new device)
A1 sync, local checks
A2 POST /v1/pairing/channels ─────────────► open, channel_id
A3 POST messages [1] commit(sid, commit)
A4 displays code + QR  · · · · · · · · · · · · · · (code) · · · · · · · · · · · · · · ► N1 typed or scanned
                                            ◄──────────────── N2 POST …/claim (no auth) → pairing_token
                                            ──────── N3 GET messages ─────────────► [1] commit
                                            ◄──────────────── N4 POST [2] share(Yb)
A5 GET messages ◄── [2] share
A6 POST [3] reveal(Ya, InviterHello)
                                            ──────── N5 GET ──────────────────────► [3]: commitment, ISK, opens
                                            ◄──────────────── N6 POST [4] joiner_hello(JoinerHello)
A7 GET ◄── [4]: opens; shows SAS and N                                             N7 shows SAS and A
A8 the user confirms on A                                                          N8 the user confirms on N
                                            ◄──────────────── N9 POST [5] joiner_confirm(history)
A9 GET ◄── [5]
A10 builds the batch, POST [6] provision
                                            ──────── N10 GET ─────────────────────► [6]: verifies, keeps pending
                                            ◄──────────────── N11 POST [7] joiner_ack
A11 GET ◄── [7]
A12 POST /v1/devices ─────────────────────► channel completed, device_added
A13 updates its store; optional history export (§9)
                                            ──────── N12 GET (state = completed) ──► N13 engine.provision, session, sync
```

A writes 3 frames, N writes 4 (limit: 8 per side). Each wait is a repeated `GET
/v1/pairing/channels/{channel_id}/messages?after=<last index read>&wait=25`; received messages
are processed in the order of their `index`. A authenticates with its session (`Bearer`), N with
`Authorization: Pairing <pairing_token>`.

### 7.2 A's side (normative)

| Step | Action | Failure |
|-------|--------|-------|
| A1 | A is provisioned, not blocked (`Blocked` absent), synchronized (`sync()`): it knows the current epoch. It SHOULD check that the space has fewer than `max_devices` members. | suitable message, no channel |
| A2 | `POST /v1/pairing/channels` → `channel_id`, `expires_at`. | `channel_limit`: "A pairing is already in progress" |
| A3 | Draw `i1`, `i2`, `sid`, `ya`; compute `Ya`, `commit`; deposit `commit`. | network: retry as long as the channel lives |
| A4 | Display code and QR (§4.2). | — |
| A5 | Wait for N's first frame. It MUST be `share`. Check `decode(Yb)` and `K ≠ I`. | §11 |
| A6 | Compute `ISK`, `K_I`, `K_J`, `SAS`; deposit `reveal` with `InviterHello`. | §11 |
| A7 | Wait for `joiner_hello`, open it. Opening failure = **wrong code** (§11.1). Read `JoinerHello`. If `ed_pk` is already a member or revoked in A's store: failure `pairing_failed`. (A dedicated code "This device is already part of your space, or has been removed from it: reinstall Tomi on it" would be an additive addition to §11.5, to come; it would not be an oracle, since A7 takes place after the key confirmation.) Display the SAS, N's name and platform. | §11 |
| A8 | Wait for the user's confirmation on A ("Add") **and** for the `joiner_confirm` frame, in any order. Refusal on A ⇒ close the channel. | §11.2 |
| A9 | Read `history` from `JoinerConfirm`. | §11 |
| A10 | Build the batch (§8.1) on a **copy** of its store, deposit `provision`. | §11 |
| A11 | Wait for `joiner_ack`, open it. | expiration ⇒ nothing is recorded |
| A12 | `POST /v1/devices` (§8.2). | §8.2 |
| A13 | On `201`: record the store copy, N's device row, start the export if `history = 1` (§9). Display "Pixel is part of your space". | — |

A releases **no** space key before A10, and A10 takes place only after the two human confirmations
(property 5 of §2.3). The `provision` frame is written **before** `POST /v1/devices`, because a
`completed` channel no longer accepts any message (`api-v1.md` §4.6). If the recording fails after A10,
N has received a space key without being registered: this is an operational failure, not a leak (N has passed
the PAKE and both confirmations; it discards the batch when the channel closes, §8.3).

### 7.3 N's side (normative)

| Step | Action | Failure |
|-------|--------|-------|
| N0 | N is not provisioned (`engine.is_provisioned() = false`). It MAY read `GET /v1/hello` (`space_initialized = true`, `api_versions` contains 1). | "This device is already part of a space" |
| N1 | Read the code (§4.4). | local refusal, nothing is consumed |
| N2 | `POST /v1/pairing/channels/{channel_id}/claim` → `pairing_token` (kept in memory only). | `404`: "Code expired"; `409 channel_claimed`: "This code has already been used"; `409 channel_closed`: "Code expired" |
| N3 | Wait for A's first frame. It MUST be `commit`. | §11 |
| N4 | Compute `g` (with the received `sid`), `yb`, `Yb`; deposit `share`. | §11 |
| N5 | Wait for `reveal`. Check `commit = SHA-256("tomi/v1/pair-commit" || 0x00 || sid || Ya)`, `decode(Ya)`, `K ≠ I`; compute `ISK`, the keys, the SAS; open the `box`. Any failure here = **wrong code** (§11.1). Read `InviterHello`. | §11.1 |
| N6 | Generate `DeviceKeys` (if not already done), deposit `joiner_hello`. | §11 |
| N7 | Display the SAS, A's name, `member_count`. | — |
| N8 | The user confirms. If `history_offer` is present with `items > 0`, ask the history question, a single yes / no (§9). Refusal or going back ⇒ close the channel. | §11.2 |
| N9 | Deposit `joiner_confirm`. | §11 |
| N10 | Wait for `provision`, open it, **verify the batch** (§8.3). Keep it pending (§8.3). | failure ⇒ close the channel, nothing is provisioned |
| N11 | Deposit `joiner_ack`. | §11 |
| N12 | Wait for `state = "completed"` (the `GET` returns the state even without a message). `closed` ⇒ A has given up: discard the batch. `404` or expiration ⇒ §11.4. | §11.4 |
| N13 | `engine.provision(Provision { .. })` (§8.4), then session (`ensure_session`) and `sync()`. Display "Device added". | — |

### 7.4 Strict order, unexpected frames

Each side waits for one frame at a time, of a known type (tables above). A frame of another
type, of another version, a second copy of an already received type, or a frame of invalid size
⇒ failure (§11), except:

- unknown version in the **first** frame received ⇒ `pairing_unsupported`;
- frames received after the last expected frame (A after `joiner_ack`, N after `provision`):
  ignored.

No frame is replayed from one session to another: `sid` is new and enters `ISK` and the AAD.

---

## 8. Pairing batch and correspondence with `tomi-core`

All the operations below already exist in `tomi-core` (`crates/core/src/device.rs`,
`crates/core/src/runtime/engine.rs`). Pairing adds **no** new cryptographic construction
to the batch: it reuses `DeviceCert`, `MemberList` and the sealed key as `format-v1.md`
defines them.

### 8.1 Construction of the batch (A, step A10)

```
peer      = DevicePublic { ed25519: JoinerHello.ed_pk, x25519: JoinerHello.x_pk }   // verified source: PAKE channel (D38)
space_id  = trust.space_id()
key       = current space key (SpaceKeyRing), epoch e = trust.key_epoch()
sealed    = seal_space_key(&peer, &space_id, &key)                                  // 104 bytes
cert      = keys.sign_device_cert(&space_id, &peer, now_ms)                         // DeviceCert(N), issuer A
staged    = trust.clone() ; staged.add_verified_peer(&peer)                         // copy: nothing is committed yet
list      = staged.issue_member_list(&keys, now_ms)                                 // contains A, N and the other members
history   = if JoinerConfirm.history = 1 and there is something to transfer:
              (transfer_id = fresh UUID v4, items, bytes)                          // §9
Provision = TLV { space_id, e, sealed, cert.to_bytes(), list.to_bytes(), history? }
```

`staged` is recorded only after the `201` of `POST /v1/devices` (A13), in the same local transaction
as N's device row (`DeviceRow { state: Verified, name, platform, joined_seq }`, `joined_seq`
read from the response). On failure, `staged` is discarded: A's store has not changed.

### 8.2 Registration on the relay (A, step A12)

```
POST /v1/devices
{ "channel_id": "<channel_id>", "key_epoch": e, "name": JoinerHello.name,
  "platform": JoinerHello.platform, "certificate": b64(cert.to_bytes()) }
```

| Response | A's action |
|---------|-------------|
| `201` DeviceRecord | A13. The channel moves to `completed`, which releases N. |
| `409 epoch_conflict` | a rotation happened between A1 and A12: close the channel, `sync()`, offer to start again. |
| `409 too_many_devices`, `409 device_exists`, `422`, `404`, `409 channel_closed` (outside resumption) | close the channel; suitable message; nothing is recorded locally. |
| response lost (network) | replay; if the response is `409 channel_closed` or `409 device_exists`, read `GET /v1/devices`: the presence of N's `device_id` counts as success. |

The other members learn of N through the `device_added` event and verify the certificate with
`TrustStore::accept_device_cert` (`format-v1.md` §8.2 step 4): nothing new.

### 8.3 Verification of the batch (N, step N10), in order

Any failure ⇒ `pairing_failed`, channel closed, batch and session keys erased.

1. TLV (§6.2); mandatory fields present; `key_epoch` in `1..=2^31 − 1`.
2. `cert = DeviceCert::from_bytes(device_cert)`; `cert.space_id = space_id`; `cert.subject` = N's
   own public keys; `cert.issuer_ed25519 = InviterHello.ed_pk`; not self-signed;
   `cert.verify_signature()`.
3. `list = MemberList::from_bytes(member_list)`; `list.space_id = space_id`; `list.key_epoch = e`;
   `list.issuer_ed25519 = InviterHello.ed_pk`; the list contains the entry
   `(InviterHello.ed_pk, InviterHello.x_pk)`. If it contains `ed_pk(N)`, it MUST be with `x_pk(N)`.
4. `space_key = keys.open_space_key(space_id, e, sealed)`.
5. `trust = TrustStore::new(space_id, &keys.public(), e)`;
   `trust.add_verified_peer(&inviter)` (`inviter` = keys from `InviterHello`);
   `trust.accept_member_list(&list)` (verifies the signature, the epoch and the key conflicts).
6. Read `history` if present (§9).

Then N keeps the result **pending**: it SHOULD place it in the OS protected storage
(device keys, `trust.to_json()`, space key, name, `history`), so that it survives a stop between
N11 and N13 (§11.4). Nothing enters the Tomi database before N13.

### 8.4 Provisioning (N, step N13)

```
engine.provision(Provision {
    keys,                        // DeviceKeys generated in N6
    trust,                       // §8.3 step 5
    space_key,                   // §8.3 step 4
    device_name,                 // JoinerHello.name
})
```

Then `ensure_session()` (challenge-response, `api-v1.md` §3.3) and `sync()`; the relay brings the cursor back to
`joined_seq` (`api-v1.md` §4.10). The pairing orchestration runs **in the core**: the secrets
(keys, space key, `ISK`) never pass through the application. The existing FFI function
`provision(ProvisionData)` remains reserved for tests and tools; applications SHOULD NOT
pass a pairing batch through it.

---

## 9. History transfer at pairing (decision D2)

A single choice, yes or no; everything is transferred, secrets and files included; no partial inclusion
(`format-v1.md` §8.7, `api-v1.md` §4.13). The channel **never** carries the history: it
carries only an offer and an announcement.

1. **Offer (A → N, `InviterHello.history_offer`)**: A's local estimate,
   `items` = exportable items (readable, outside the outbox), `bytes` = approximate size (payloads and
   files present locally). A omits it if `items = 0` or if the relay does not publish the transfer
   limits (`api-v1.md` §4.2, `transfer_max_open` absent).
2. **Question (on N, step N8)**: "Retrieve the history? 812 items, 340 MB", answers "Yes,
   retrieve the history" / "No, start with an empty feed". Without `history_offer` (or `items = 0`), the
   question is not asked and N sends `history = 0`. Where the question is placed: decision to be validated,
   annex A D-A4.
3. **Announcement (A → N, `Provision.history`)**: present if and only if `history = 1` and A has
   offered something. `transfer_id` is drawn by A **at that moment** and MUST be that of the transfer
   that will follow; `items` and `bytes` repeat the estimate. N displays "History being prepared on
   Desktop PC".
4. **Export (A, after the `201` of `POST /v1/devices`)**: `start_export(N, transfer_id)` (persistent,
   resumable task, `api-v1.md` §7.8): synchronization up to `cursor ≥ joined_seq(N)`,
   index, parts, re-encrypted files, `POST /v1/transfers` with this `transfer_id`.
5. **Import (N)**: on the `history_transfer` event carrying the announced `transfer_id`, N imports without
   asking again (the user has already answered yes), according to `api-v1.md` §7.8; it verifies the manifest with
   its store (`TrustStore::verify_history_transfer`) as for any transfer. A transfer that was not
   announced follows the usual rule (`auto_import` or `accept_import`).

`items` and `bytes` are only used for display: N MUST NOT rely on them for anything else. An N that
ignores the `history` tag (a future version without this tag, unknown tag ignored) still imports the
transfer through the journal; it simply does not display "being prepared".

---

## 10. Space creation (first device) and the "create" / "join" boundary

### 10.1 Boundary

| Device situation | Choices offered |
|-------------------------|----------------|
| provisioned | neither create nor join; it can **invite** (§7.2). To change space, it first leaves its own (`DELETE /v1/me`, local erasure). |
| not provisioned, `GET /v1/hello` → `space_initialized = true` | **Create your space** (installation token, another person may have theirs on the same relay) and **Join** (§7.3): both choices remain offered. |
| not provisioned, `space_initialized = false` | **Create your space** (installation token). "Join" is useless: there is no member to invite. |
| not provisioned, relay unreachable | "Join" by default; "Create" appears as soon as `hello` answers `false`. |

The founder does **not** use a PAKE: there is no one yet with whom to establish trust. Its authorization
comes from the installation token (`api-v1.md` §3.1), its proof of key possession is a self-signed
`DeviceCert`, which gives no trust to another device (`format-v1.md` §8.2).

### 10.2 Sequence (normative)

```
keys      = DeviceKeys::generate()
space_id  = new_space_id()
space_key = SpaceKey::generate(1)
cert      = keys.sign_device_cert(&space_id, &keys.public(), now_ms)      // self-signed
→ keep (keys, space_id, space_key) pending in the OS protected storage
POST /v1/space { install_token, space_id, name, platform, certificate: b64(cert.to_bytes()) }
201 → trust = TrustStore::new(&space_id, &keys.public(), 1)
      engine.provision(Provision { keys, trust, space_key, device_name: name })
      ensure_session(), sync() ; erase the pending record
```

| Response | Action |
|---------|--------|
| `201` | provision as above |
| `403 install_token_invalid` | "Incorrect installation token"; the pending record remains, a new attempt is possible (the token is not consumed) |
| `409 space_exists` after a lost response | try a challenge-response with `keys` (`POST /v1/auth/challenge` then `session`): success ⇒ it was this creation, provision; `401` ⇒ another device created the space: erase the pending record, offer "Join" |
| `409 space_exists` without a lost response | erase the pending record, offer "Join" |

The installation token is a secret (§12): typed or pasted, never logged nor kept after use.
The founder starts with an empty feed; no history question.

### 10.3 Creation link and the founder's relay

The founder's relay is not compiled into the application: it comes from the creation link, from the "Relay"
field typed next to the token, or from the relay the device already has.

```
creation link = "https://" relay_authority "/create#" install_token
```

- Issued form (by the relay and its administration panel): token in the **fragment**, which a browser
  never sends to the server. A reader also accepts `https://<relay>/create/<token>` (token in the
  path); the two forms are not mixed. `https` scheme only, authority of a valid relay (§4.4
  rule 1), token of the form of `api-v1.md` §3.1 (24 to 40 characters of the relay's base32 alphabet, hyphens
  and whitespace removed); no other parameter is read. A link creates nothing by itself: the application waits for the
  user's action.
- **The relay of a link is not reliable** (§2.2, §4.4 rule 7): the application displays it in full
  before sending the token, keeps this relay only upon success, switches to it for the duration of the
  creation and otherwise reverts to the previous relay. **One precise exception**: the founder's keys are
  sealed **before** the request (§10.2), and `pending_relay_url` is written in the same transaction, so that
  the resumption (`resume_pairing`) goes to the right relay after a stop or a lost response; this relay is
  therefore recorded before success, but only as the *pending* relay of these keys, and it is forgotten
  with them (success, or keys erased). The token is sent only to the displayed relay; it is never
  kept, but a token sent to an unexpected relay is exposed to it (§2.2).
- **Keys pending for another relay.** If founder keys are pending for a relay other than
  that of this creation, the core first queries that relay (a creation whose response was lost is
  recognized there: the device is then provisioned to that relay and the new creation is refused), then
  **erases** them. Accepted trade-off: if that relay does not answer, the creation is abandoned there without a response: the
  keys are destroyed, and a space possibly created there without a response is no longer reachable by this
  device (it contains only its empty feed). This exchange moves the shared transport: it takes place only
  when **no other session** is in progress (§4.4 rule 9), otherwise the creation is refused
  (`InvalidInput`) and the keys remain as they are.
- A provisioned device creates or joins nothing (§10.1); its relay does not change, except through the
  exception of a device that keeps none (§4.4 rule 8). A creation link, like a pairing link,
  carries no path prefix (§4.3): a relay under a prefix is chosen through the "Relay" field.

---

## 11. Errors, abandonment, expiration, resumption

### 11.1 Wrong code: one attempt, no oracle

These failures are **indistinguishable** for the user and in the logs: `decode` of `Ya`/`Yb`
impossible, `K = I`, wrong commitment, opening of the first `box` impossible. They all give
`pairing_failed`. The device that notices it:

1. closes the channel (`DELETE /v1/pairing/channels/{channel_id}`, on a best-effort basis);
2. erases `ya`/`yb`, `K`, `ISK`, `K_I`, `K_J`, the SAS, the code, the `pairing_token`;
3. **never** retries another code on the same channel.

On N: "Incorrect code. Generate a new code on the other device." On A: "Pairing did not succeed.
A new code is ready." (A creates a new channel, §4.2).

### 11.2 Abandonment

User refusal (different verification codes, "Cancel", going back), closing of the screen,
failure to verify the batch: the concerned side closes the channel and erases its session secrets. The other
side sees `state = "closed"` (or `404`) and displays "Pairing was cancelled on the other device.".
A refusal on N after N9 is still possible as long as N13 has not taken place: N closes the channel; if A
has already registered N (race with A12), the user removes N from the Devices screen.

### 11.3 Expiration
The relay deletes a channel that has not been joined 120 s after its creation, and a channel that has been joined 120 s after the
`claim` (`pairing_ttl_secs`, one extension). The whole sequence N2 → A12, human comparison
included, must fit within the second window. A client MUST also abandon by itself 600 s after
the start of a session, whatever the relay says (local bound, independent of `pairing_ttl_secs`). Expiration ⇒ `pairing_expired`, "Code expired.
Generate a new one."; nothing is provisioned, A's store does not change. Duration to be validated:
annex A, D-A5.

### 11.4 Resumption of N after a stop

If N stops between N10 and N13 and a verified batch is pending (§8.3): on restart, N tries a
challenge-response with the pending keys. Success ⇒ A has registered it: N13. `401 unauthorized` ⇒ A did not
register it: discard the batch. The same rule applies if N sees `404` at N12 (channel purged before N reads
`completed`). Without a pending batch, N starts again from zero; an N that is registered but has no keys remains visible
in Devices and is removed like a lost device.

### 11.5 Error codes added to the core

Stable codes (`CoreError::code()`), additive to those of `format-v1.md` §9.2:

| Code | Case | Message (N / A) |
|------|-----------------|-----------------|
| `pairing_failed` | §11.1, unexpected or malformed frame, batch refused | "Incorrect code" / "Pairing did not succeed" |
| `pairing_expired` | §11.3, `404` at `claim` | "Code expired" |
| `pairing_taken` | `409 channel_claimed` at `claim` | "This code has already been used. Generate a new code on the other device." |
| `pairing_cancelled` | channel closed by the other side, user refusal | "Pairing was cancelled" |
| `pairing_unsupported` | unknown version, unknown critical tag | "Update Tomi on both devices to pair them." |

Network errors keep their existing code (`network`). No message contains the code, the SAS
nor frame bytes.

### 11.6 Joining a space after a voluntary departure

A device that has left its space (`api-v1.md` §7.9) no longer has any identity, key or data: it pairs
like a new device (§7.3), with new device keys, and may join the same space or another.
The relay never recognizes it under its former identity (`device_id` and `sign_pk` revoked, `409 device_exists`).
Before provisioning (N13), N completes an erasure that is still pending (`leave_state = "wiping"`, otherwise a
later startup would delete the files of the new space); provisioning also clears `blocked`
and `leave_state` (with `departure_since` and `deletion_id`) in its transaction. At startup,
`continue_departure` is called before `resume_pairing`: a pending departure is verified (and, within the
15 minutes following the gesture, sent again) before any pairing resumption (`api-v1.md` §7.9.4).

Erasure without proof (`erase_without_proof`, "Erase anyway", `api-v1.md` §7.9.5) leads to the same fresh
state without leaving anything on the relay side: if the space still exists, the device remains registered there. The core
allows it only after a recent "not found" or "relay too old" verdict, never after a network error.
To start over on the server side: `tomi-relay admin reset` (`api-v1.md` §8.7).

---

## 12. Secret hygiene

Pairing secrets: the code (its two words), `ya`, `yb`, `K`, `ISK`, `K_I`, `K_J`, the SAS, the
`pairing_token`, the installation token, and of course the device keys and the space key.

- They MUST NOT appear in any log, crash report, notification, FCM message, request URL, nor in the
  Tomi database in clear. The relay never sees them (the code is in a URL fragment;
  the `pairing_token` in a header). MAY be logged: `channel_id`, states, error codes.
- They live in memory, in buffers erased (`zeroize`) at the end of the session, whether it succeeds or
  not. Only the verified pending batch (§8.3) and the founder's material (§10.2) touch the OS
  protected storage, and they are erased from it after provisioning.
- The screen that displays the code and the QR SHOULD prevent screenshots (`FLAG_SECURE` on
  Android). The code is never copied to the clipboard by the application.
- The SAS is never typed: it is compared by eye.

---

## 13. Compatibility and versioning

- **No byte of format 1 changes.** Pairing frames form a separate protocol, versioned
  by `PAIR_VERSION`, never stored nor kept by the relay for more than a few minutes. The batch reuses
  without change `DeviceCert`, `MemberList` and the sealed key of `format-v1.md`. There is therefore **no**
  `vN` directory to open and the vectors `spec/vectors/v1/` are not touched.
- **New labels**, which complement the registry of `format-v1.md` §4.2 (pure addition, one label
  per use):

  | Label | Type | Section |
  |-----------|------|---------|
  | `tomi/v1/pair-ci` | CPace CI prefix | §3.3 |
  | `tomi/v1/pair-inviter` | CPace ADa | §3.3 |
  | `tomi/v1/pair-joiner` | CPace ADb | §3.3 |
  | `tomi/v1/pair-commit` | hash prefix (commitment) | §5.1 |
  | `tomi/v1/hkdf/pair-inviter` | HKDF info | §5.2 |
  | `tomi/v1/hkdf/pair-joiner` | HKDF info | §5.2 |
  | `tomi/v1/hkdf/pair-sas` | HKDF info | §5.2 |
  | `tomi/v1/aad/pair` | AAD | §5.4 |

  `CPaceRistretto255` and `CPaceRistretto255_ISK` are the domain separators of the CPace draft.
- **Additive extensions** within `PAIR_VERSION = 1`: new TLV tags from `0x01` to `0x7F` (ignored by
  a v1 reader), as `history` is today. A tag from `0x80` to `0xFF` forces an update.
- **Breaking change** (another PAKE suite, another sequence, another code): `PAIR_VERSION = 2`. Both
  devices must then speak the same version; there is no negotiation in v1: A's version
  prevails, and N fails with `pairing_unsupported` if it does not speak it. The form of the code (§4) and of the link
  carries no version; a future different code will use another path (`/l/pair2`).
- **Relay**: no change. It does not read the frames and applies `api-v1.md` §4.6 as is.

---

## 14. Vectors and acceptance cases

### 14.1 Vectors

New directory `spec/vectors/pairing-v1/` (with its `SHA256SUMS`), separate from `spec/vectors/v1/`.
The values are produced by a generator (like `gen_spec_vectors`), then verified by a test which
**recomputes them from the primitives and from this document**, without going through the `pairing` module.

1. `cpace-draft21.json`: reuse of the B.3 vectors of draft-irtf-cfrg-cpace-21 (PRS `Password`,
   CI `0b415f696e69746961746f720b425f726573706f6e646572`, sid `7e4b4791…2c57`, ADa `ADa`, ADb `ADb`,
   scalars `ya`, `yb` of the draft): `gen_str`, `g = 222b6b19…7e56`, `Ya`, `Yb`, `K = 80b69a8a…b40f`,
   `ISK` (initiator-responder mode) `b69effbf…0447`; invalid inputs B.3.11 (`Y_i1`, identity element)
   ⇒ `I` and abort; `prepend_len` vectors of A.1.2.
2. `wordlist.json`: fingerprints of annex B, ranks of a few words (`abaisser` 0, `crayon` 465,
   `renard` 1633, `zoologie` 2047), reading cases of §4.4.
3. `pairing.json`: a complete deterministic session. Inputs: `relay_authority = "relay.example.org"`,
   `channel_id = "4821"`, words `crayon`/`renard`, `sid = 000102…0f`, `ya`/`yb` = those of the draft,
   devices A and B of `spec/vectors/v1/device.json` (A inviter, B new device), `space_id`
   `5e1f0c2a-7b3d-4e8f-9a61-0c2d4e6f8a10`, epoch 1, space key and ephemeral key of the sealing fixed
   (API `kat`), `now_ms` fixed, names "Desktop PC" / "Pixel". Outputs: `PRS`, `CI`, `gen_str`, `g`, `Ya`,
   `Yb`, `commit`, `K`, `ISK`, `K_I`, `K_J`, `SAS`, the 7 frames in hexadecimal, the decoded batch, the
   resulting `TrustStore` of N (`to_json`). Plus the failure cases of §14.2 (P9 to P17) in the form of altered
   frames and expected error.

### 14.2 Acceptance cases

**Pure layer (`tomi-core`, no network)**

- **P1** The vectors `cpace-draft21.json` are found byte for byte; `Y_i1` and `I` cause an abort.
- **P2** `prepend_len` and `lv_cat` find the A.1.2 vectors of the draft.
- **P3** The embedded word list has the fingerprints of annex B; 2048 distinct folded words, of
  5 to 8 letters `a`–`z`, distinct on their first 4 letters.
- **P4** `generate(channel_id)` gives an unchanged `channel_id` and two ranks in `[0, 2047]`; over 10^5
  draws, each rank appears (coarse statistical test, without a realistic false positive).
- **P5** Reading: `4821-crayon-renard`, `4821 Crayon RENARD`, `4821-crayo-rena`, `4821_crayon.renard`,
  `4821-crâyon-renard` give the same code; refusal of `0-…`, `04821-…`, `10000-…`, `4821-cra-renard`
  (fewer than 4 letters), `4821-crayon` (2 tokens), `4821-crayon-renard-tomate` (4 tokens), of an
  unknown word, of non-ASCII digits.
- **P6** Link: `https://relay.example.org/l/pair#4821-crayon-renard` and `tomi-share://pair#…` read; `http`, another
  path, code in the query string: refused. Another host is refused by a reader whose authority is fixed
  (frozen vectors) and read as a **candidate relay** by a reader according to §4.4 rule 1 (which displays it, rule 7);
  user information (`https://user@host/…`), port 0, non-ASCII host: refused everywhere.
- **P7** Nominal session A ↔ N (pure state machines): SAS equal; N obtains A's keys, A obtains N's;
  the batch passes §8.3; N's `TrustStore` contains A, N and the other members; the space key
  opens an item sealed by A; `cert` verifies with `TrustStore::accept_device_cert` on a third
  member C.
- **P8** Size: `provision` with 119 members ≤ 8192 bytes; with 120, A fails before writing.
- **P9** Wrong code (a neighbouring word): N fails at N5 with `pairing_failed`; A never opens a
  `joiner_hello`; both errors are identical whatever the cause (commitment, invalid point,
  AEAD).
- **P10** Betrayed commitment: `reveal` with a `Ya` different from the committed one ⇒ `pairing_failed`.
- **P11** Man in the middle who knows the code (simulated relay holding two sessions): it can reveal
  only a committed `Ya`; both SAS are computed on independent `ISK`; deterministic test: with the
  attacker's randomness fixed, changing `yb` of N changes N's SAS without the attacker being able to
  recompute it before having committed.
- **P12** A modified bit in each frame (header, payload, tag) ⇒ failure; `PAIR_VERSION = 2` in the
  first frame ⇒ `pairing_unsupported`; type `0x08` ⇒ `pairing_failed`; truncated or too long
  frame ⇒ `pairing_failed`.
- **P13** Disorder: `reveal` before `share`; two `commit`s; `provision` before `joiner_confirm`;
  `joiner_confirm` before `joiner_hello`; A's frame replayed to A ⇒ failure. Frame received after the
  last expected one ⇒ ignored.
- **P14** Replay of a frame from another session (other `sid`) ⇒ failure.
- **P15** TLV: unknown tag `0x40` ignored; unknown tag `0x90` ⇒ `pairing_unsupported`; duplicate, disorder,
  missing mandatory field, wrong length, invalid `name` (empty, 65 characters, control character,
  trailing whitespace), invalid `platform` ⇒ `pairing_failed`.
- **P16** Batch refused: certificate subject ≠ N; issuer ≠ A; self-signed certificate; list signed by
  another; list of another epoch; list without A; A's `x_pk` different in the list
  (`key_conflict`); sealed key for another device or another `space_id` (`decryption_failed`);
  reserved epoch; inconsistent `space_id`.
- **P17** No secret in the logs: over a complete session and a failed session, the captured logs contain
  neither the words of the code, nor the SAS, nor `ISK`, nor the `pairing_token`.

**End to end (runtime against the relay's in-memory double, then against `apps/relay`)**

- **P18** Nominal: A invites, N joins; `POST /v1/devices` → `201`; channel `completed`; N, provisioned,
  opens a session; A sees N in `devices()`; C (third member) accepts `device_added`; an item of
  C is read by N.
- **P19** Wrong code: N closes the channel; A sees `closed` and offers a new code; a second `claim`
  on the old channel ⇒ `404` or `409`.
- **P20** Channel taken: a third party makes the `claim` before N ⇒ N receives `pairing_taken`; the third party fails
  with a wrong code. With the right code (simulated leak), A displays a SAS but **does not write** `provision`
  until A's user has confirmed; without `joiner_confirm`, nothing is sent.
- **P21** Expiration: channel not joined for 121 s ⇒ A creates another; channel joined but no
  confirmation within 120 s ⇒ `pairing_expired` on both sides, no device registered, A's store
  unchanged.
- **P22** Refusal on N ("They are different") ⇒ channel closed, A displays "cancelled", nothing recorded.
- **P23** Refusal on A ⇒ channel closed, N displays "cancelled", N not provisioned.
- **P24** `POST /v1/devices` answering `409 epoch_conflict` or `too_many_devices` ⇒ A closes the channel; N discards the
  batch; A's store unchanged.
- **P25** Response to `POST /v1/devices` lost ⇒ replay, `GET /v1/devices` contains N ⇒ success.
- **P26** N stopped between N11 and N13: on restart, successful challenge-response ⇒ provisioned; if A did not
  register N, `401` ⇒ batch discarded.
- **P27** History "yes": `provision` carries `history`; A exports with this `transfer_id`; N imports
  without a new question; `DELETE /v1/transfers` with `outcome = "imported"`.
- **P28** History "no", or `history_offer` absent: no `history` tag, no `POST /v1/transfers`.
- **P29** N ignoring the `history` tag (tag removed from the reader in the test): pairing succeeds and the
  transfer arrives through the journal.
- **P30** Founder: `POST /v1/space` then provisioning; response lost then `409 space_exists` ⇒
  successful challenge-response ⇒ provisioned; wrong token ⇒ nothing provisioned, a new attempt is possible.
- **P31** Relay: checks A100 to A107 of `api-v1.md` §11.10 pass on `apps/relay` with real frames
  of this protocol.

---

## 15. Implementation plan (non-normative)

Sequential batches on `crates/` (one contributor at a time on the core), independent review before each commit.

1. **Pure core** (`crates/core/src/pairing/`, no network): embedded word list (`include_str!`,
   fingerprint test); code (`generate`, `parse`, `link`, `prs`); CPace layer (private) and the draft's
   vectors; frames and TLVs; state machines `Inviter` and `Joiner` (inputs: received frames and user
   decisions; outputs: frames to deposit, SAS, peer information, verified batch); construction and
   verification of the batch with the existing functions (`seal_space_key`,
   `DeviceKeys::sign_device_cert`, `TrustStore::{clone, add_verified_peer, issue_member_list,
   accept_member_list}`, `DeviceCert::from_bytes`, `MemberList::from_bytes`, `open_space_key`);
   labels in `crypto::labels`; error codes of §11.5; `curve25519-dalek` as a direct dependency
   (version 5, `default-features = false`, `zeroize`). Generator and test of `spec/vectors/pairing-v1/`.
   Tests P1 to P17. **Independent review** (crypto).
2. **Runtime**: methods of `Transport` (+ `http.rs` + `mock.rs` double) for `POST /v1/space`,
   the five routes of `api-v1.md` §4.6 (with `Authorization: Pairing`), `POST /v1/devices`;
   orchestration in `Engine`: `create_space`, inviting session (code, link, SAS, peer name,
   confirm, cancel, done), joining session (join by code or link, SAS and A's information,
   confirm with `history`, cancel), pending batch and resumption (§11.4); `start_export` with
   an imposed `transfer_id`; automatic acceptance of the announced transfer. Tests P18 to P30 against the
   in-memory double. **Independent review** (network, states).
3. **Relay**: no change expected. Make P31 pass; check that `/l/pair` is a static page
   with no script.
4. **Clients** (can progress in parallel with batches 1 and 2, disjoint areas): UniFFI exposure of
   step 2; Android: wire `pairWithCode` / `pairWithQr` / `confirmPairing(importHistory)` /
   `cancelPairing` to the real core, add the "Add a device" screen (code, QR, SAS, confirm),
   complete `PairingFailure` (`Taken`, `Cancelled`, `UpdateRequired`), fix the texts (§A.3);
   Windows: same screens.

---

## Annex A (non-normative): decisions, alternatives, discrepancies

### A.1 Decisions to be validated by the maintainers

| # | Decision | Recommendation | Alternative |
|---|----------|----------------|-------------|
| D-A1 | PAKE suite | CPace `CPACE-RISTR255-SHA512`, layer written on the `curve25519-dalek` already present (§3) | SPAKE2 via the `spake2` crate (not audited, pre-release for dalek 5) |
| D-A2 | Code secret | 2 words of the French BIP-39 list (22 bits), `4821-crayon-renard` | 3 words (33 bits, `4821-crayon-renard-tomate`): longer to type, marginal gain given the single attempt and the SAS; digits only (`4821-583-201`): no list, but less readable aloud |
| D-A3 | 6-digit verification code | keep it (the validated Android UI already has it) together with the commitment of §5.1: it stops a photographed code | remove it: one screen fewer, but a QR seen by a third party is then enough to join if the third party wins the race to `claim` and the user confirms on A |
| D-A4 | Where to ask the history question | on N (validated Android UI: `PairingPhase.History`) | on A (`api-v1.md` §7.8, `docs/history-and-expiry-decision.md` §2.4); the protocol allows it: N would always send `history = 1` and A would decide |
| D-A5 | Duration after `claim` | set `pairing_ttl_secs` to 300 s in the relay configuration (no protocol change): 120 s for two human comparisons and one question is tight | keep 120 s |
| D-A6 | Words displayed without accents | yes (folded form); input accepts accents | display the accents of the list |

### A.2 Discarded alternatives

- **SPAKE2** (RFC 9382): fixed points M and N with initial trust; `spake2` crate not audited, compatible with
  magic-wormhole and not RFC 9382, stable version on dalek 4 (second copy of dalek in the tree).
- **CPace on X25519**: requires Elligator2, absent from `x25519-dalek`: a primitive would have to be written.
- **`pake-cpace`**: old draft, different encodings, little maintained.
- **OPAQUE**: augmented PAKE for client / server; here the two peers are symmetric and nothing is
  stored.
- **No PAKE, code comparison alone** (Bluetooth style): security would rest entirely on the user's
  attention, and the typed code (PC without a camera) would have no role left.
- **JSON bodies**: 4/3 base64 overhead (the `MemberList` would fit ~85 members instead of 119)
  and one more parser to harden; TLV keeps additive extensibility.
- **Home-made word list**: the French BIP-39 list exists, under MIT licence, designed for
  human transcription, and verified here (annex B).

### A.3 Discrepancies with other documents (to be aligned by the maintainers)

1. `api-v1.md` §4.6 and B.1 no. 6, the Android help (`pair_code_hint`) and `FakeTomiCore` (`DEMO_CODE`) give
   the example `7-crayon-lune`: `lune` is not in the list (4 letters). Compliant example: `7-crayon-renard`.
2. `api-v1.md` §7.8 and `docs/history-and-expiry-decision.md` §2.4 place the history question
   on A (and the latter still speaks of checkboxes); the Android UI and D-A4 place it on N.
3. Earlier internal reviews proposed removing the 6-digit verification code; this document keeps it, with a
   new reason (the commitment makes it useful against a code seen by a third party), subject to D-A3.
4. Android `pair_verify_validity` displays "Code valid for 5 min"; the channel lives 120 s after the `claim`
   (D-A5).
5. `docs/ux-flows.md` describes a QR carrying an X25519 key and a typed 6-digit code: obsolete.

---

## Annex B (normative): word list

The list is the French BIP-39 list (`bip-0039/french.txt` in the `bitcoin/bips` repository, MIT licence):
2048 lines in UTF-8 (NFD: accents are decomposed there), separated by `\n`, terminated by `\n`,
16 777 bytes, SHA-256 `ebc3959ab7801a1df6bac4fa7d970652f1df76b683cd2f4003c941c63d517e59`. The **rank**
of a word is
its line number, starting from 0.

**Folded form** of a word: NFKD, removal of combining marks (Mn), lowercase. The 2048 folded forms
are distinct, are 5 to 8 letters `a`–`z`, and are distinct on their first 4 letters;
they are in the same order as the original list. Concatenated with `\n` and a final `\n`, they make
16 001 bytes, SHA-256 `cf8db448ad5ca0b68c3c8f88b51458f20362a5939fef236fac64a303f1485e87`.
The implementation MAY embed the folded form directly; a test MUST verify this fingerprint.

Examples: rank 0 `abaisser`, 465 `crayon`, 616 `ecole` (`école`), 642 `eleve` (`élève`), 1633 `renard`,
1891 `tomate`, 2047 `zoologie`.

Sources: [draft-irtf-cfrg-cpace-21](https://www.ietf.org/archive/id/draft-irtf-cfrg-cpace-21.txt)
(§6 to §10, annexes A and B.3); [README of the `spake2` crate](https://github.com/RustCrypto/PAKEs/tree/master/spake2);
[BIP-39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki) and its
[French list](https://github.com/bitcoin/bips/blob/master/bip-0039/french.txt).
