# Tomi — Client ↔ Relay API Specification, version 1

Status: **normative**. API version: `1` (prefix `/v1`).
Related documents: `PROJECT.md` (§5, §6, §7 are authoritative), `docs/DEV.md` (security invariants),
`spec/format-v1.md` (format of encrypted envelopes, written separately).

This document is sufficient to implement `apps/relay` and the network part of `crates/core`.
Annexes A (§10, SQLite schema) and B (§12, decisions made here, trade-offs, open points)
are not normative. §11 (acceptance checks) is normative.

## 0. Reading this document

### 0.1 Normative keywords

**MUST** / **MUST NOT**: absolute requirement. **SHOULD** / **SHOULD NOT**: recommended;
deviating from it requires a written reason. **MAY**: optional. The RFC 2119 terms
(MUST, SHOULD, MAY) have the same meaning.

### 0.2 Glossary

| Term | Definition |
|-------|------------|
| Space | A group of devices that is **independent**: its space key, its log, its items, its blobs, its devices. A relay hosts **one or more spaces** (a few people, each with their own) that share only the server: the same URL, the same process, the per-IP-address limits (§8.5) and a shared total of bytes (§8.4). No data, identifier or right passes from one space to another. |
| Invitation | Permission to create a space, designated by a single-use installation token (§3.1): **pending** as long as no space has used it, then **consumed** (the row remains, with the `space_id` that was created). The relay keeps only its fingerprint. |
| Device | An installation of Tomi (Windows, Android, CLI) that is a member of the space. Identified by `device_id`. |
| **Member** device | Registered device that has not been revoked. |
| **Active** device | Member whose `last_seen_at` is less than `inactive_after_days` old (default 30 days). Used to compute the effective version (§3.5) and for purging (§8.1). A device that has never opened a session is not active. |
| **Present** device | Member that has reported recent user activity (§6.2). Used only to decide which devices ring. Do not confuse with "active". |
| Item | A shipment: an encrypted envelope (small, sent in one request) plus 0 to N blobs. |
| Envelope | Opaque bytes produced by `crates/core` according to `spec/format-v1.md`. The relay never parses them. |
| Blob | Opaque encrypted file attached to an item, sent in chunks (*parts*). |
| Event | Entry in the space's log, numbered by `seq`. |
| `seq` | Strictly increasing integer assigned by the relay to each event of the space (a counter **per space**), never reused, **even after restoring a backup**: `seq = max(last_assigned_seq + 1, relay_time_in_ms)` (§8.2). The examples in this document use small numbers for readability. |
| Cursor | The largest `seq` a device has applied. Stored by the device. |
| Doorbell | Signal "there is something new, read the log": WebSocket message `wake` or an empty FCM message. Carries no data. |
| Session | Short opaque token obtained through a signed challenge-response (§3.3). |
| `key_epoch` | Number of the space key in effect. Increases by 1 at each rotation. |

### 0.3 Dependencies on `spec/format-v1.md`

`spec/format-v1.md` was published while this document was being written; this document is aligned
with it and with `crates/core` (`device.rs`, `tombstone.rs`, `envelope.rs`, `format.rs`).
**`format-v1.md` is authoritative** for every signed or encrypted byte; this document only carries
them, and must be aligned with it in case of discrepancy.

1. **Envelope**: bytes produced by `core`. For **all** formats, bytes 0–3 equal
   `u32be(format)` (the only cross-format invariant, `format-v1.md` §3.1). In format 1, the cleartext
   header continues with `key_epoch` (u32 BE, bytes 4–7) and `item_id` (16 bytes, 8–23); the rest is opaque.
   Maximum size in format 1: 120-byte header + payload padded ≤ 1 MiB + 16 bytes of tag.
2. **Blobs**: an attachment = one blob, encrypted in chunks of 64 KiB (STREAM construction);
   encrypted size = plaintext size + 16 bytes per chunk. Independent of the HTTP *parts* of §4.9.
3. **`device_id`**: 32 lowercase hexadecimal characters,
   `hex(SHA-256("tomi/v1/device-id" || 0x00 || sign_pk)[0..16])`. It is **derived** from the signing key.
4. **Signed declarations** (binary form `TBS || signature`, encoded as `b64` in the API):
   device certificate `DeviceCert`, key rotation `KeyRotation` (which also carries the list of
   revoked devices), tombstone `Tombstone`, history transfer manifest `HistoryTransfer`
   (`format-v1.md` §8.7.1, 313 + 35 · n_parts bytes). They designate devices by their Ed25519 key.
   The relay decodes and verifies them **with `tomi-core`** (a dependency of the `tomi-relay` crate),
   without reimplementing their encoding. The pin (`PinStatement`, `format-v1.md` §8.8) and the archive
   (`ArchiveStatement`, §8.9) are the exception: each is **encrypted** inside an opaque **item-state envelope**
   (320 bytes in format 1; in cleartext only `format`, `key_epoch` and a random `event_id` at bytes 0–23)
   that the relay can neither read nor verify.
5. `key_epoch` is a u32 ≥ 1; the first epoch of a space is **1**.
6. `space_id`, `item_id`, `blob_id` are UUIDs; their binary form is 16 bytes.
7. The only signature specific to the API is that of the authentication challenge (§3.6), under the label
   `tomi/v1/sig/relay-auth` that `core` reserved for this use.

## 1. Overview and threat model

### 1.1 Architecture

```
 Device A ──HTTPS REST (data)───────►┐
          ◄──WSS /v1/ws (doorbell)───┤  Caddy (TLS) ──► tomi-relay ──► per space: SQLite (WAL) + blobs
 Device B ◄──FCM (empty doorbell)────┘                       │            global: routing.db
                                                             └──► FCM HTTP v1 (Google)
```

- **Data always goes through REST.** Sending: `POST /v1/items` (after the blobs are sent).
  Receiving: `GET /v1/events?after=<cursor>`.
- **WebSocket and FCM are only a doorbell.** A lost doorbell loses nothing: the device re-reads the
  log at startup, on reconnection and periodically.
- The relay listens for HTTP on a local address; **Caddy** terminates TLS. The relay never serves
  plaintext traffic on a public interface.
- **One URL, several spaces.** All clients use the same relay URL (compiled into the application).
  The relay finds each request's space from what the request already carries (§1.4): no route, no
  header, no extra field for the client.

### 1.2 What the relay knows (and only that)

For **each** space it hosts, plus some data specific to the relay (last row of the table). The relay
sees the metadata of all its spaces; no space sees that of another (§1.3).

| Data | Why |
|--------|----------|
| `space_id`, `created_at` of the space | identity of the space |
| Per device: `device_id`, `name`, `platform`, Ed25519 signing public key `sign_pk`, X25519 agreement public key `kx_pk`, signed pairing certificate, `app_version`, `format_min`, `format_max`, last activity and presence dates, FCM push token | access control, versioning, doorbell |
| Per item: `item_id`, issuing `device_id`, `created_at` (relay time), `expires_at`, `key_epoch`, envelope size, `seq`; the `format` is readable in the cleartext header of the envelope (§0.3) | storage and purge |
| Per blob: `blob_id`, encrypted size, item or attached transfer | storage, quota |
| Per history transfer: `transfer_id`, sending and receiving devices, signed manifest (public keys, `as_of_seq`, **sealed** `HK` key, `blob_id` and nonce prefixes of the parts), number and size of blobs, dates | temporary storage (≤ 7 days, §4.13) |
| Acknowledgements of receipt and of reading, tombstones (with their time) | delivery status, "read" state, deletion |
| Per item-state event (pin, archive): random `event_id`, issuing `device_id`, `key_epoch`, size (constant), time, `seq`; **neither `item_id`, nor state, nor even the state type** | encrypted propagation of pin and archive (§4.8) |
| Space keys **encrypted for each device** (`wrapped`), unreadable by the relay | rotation |
| Opaque messages of pairing channels, for ~2 minutes | pairing rendezvous |
| For the whole relay: the list of its spaces; pending **invitations** (SHA-256 fingerprint of the installation token and its date, never the token); the quota override of each space; the default space quota, if it has been set from the web admin panel (§8.7.1); the health status of FCM sending | space creation (§3.1), quotas (§8.4), supervision (§8.7) |

The relay **does not know**: the content type, the secret flag, the "after first read" policy, the
expiration the user wants (it only sees the retention deadline, §4.8), names, MIME types and the
logical number of files (beyond the number of blobs), titles (including that of a secret), previews,
thumbnails, the rendering format; for a history transfer, neither the number of items, nor their types,
nor the share of secrets; **which items are pinned or archived** (neither the item targeted by a state
event, nor its type, nor the meaning pin/unpin or archive/unarchive). **It holds no encryption key.**

### 1.3 Transport threat model

| Adversary | What it sees | What it can do | What prevents it |
|------------|---------------|---------------------|---------------------|
| **Compromised relay** (EC2 taken over, database copied, backup stolen) | All of §1.2: temporal graph of sends, sizes, number of blobs per item, reads, device presence, device names | Withhold, delete, delay items or events; invent an unsigned revocation ("by the administrator" or "voluntary departure"); rename a device; refuse service | E2EE (`format-v1.md`): no reading of content. AAD binding `item_id` and `key_epoch`: no swapping of envelopes. Device certificates, tombstones and rotations (which carry the revocations) **signed** by a device and **verified by clients** with `core`: the relay cannot slip its own key into the device list (D38) nor forge a deletion. |
| **Network attacker** (public Wi-Fi, ISP) | IP address of the relay, volumes and timing of TLS traffic | Cut the connection | TLS 1.2+ (Caddy, Let's Encrypt). No secret in a URL. |
| **Google / FCM** | The device's FCM token, the time of each doorbell, its priority (`HIGH` = a device must ring, `NORMAL` = silent update), the package name | Delay or lose a doorbell | FCM message with constant content (§6.3); catch-up by cursor. |
| **Unknown person who discovers the URL** | `GET /v1/hello`, `/healthz`, `/updates/` | Try to create a space, guess a pairing channel, fill the disk | Single-use installation token, issued by the operator (one invitation per space to create); all writes require a session of a member device; rate limits and quotas. |
| **Another person on the same relay** (they have their own space) | The same public responses as the unknown person (`hello` only says that at least one space exists); everything a member of **their** space sees | Try an identifier (session token, `device_id`, channel number) to reach another's space; randomly claim an open pairing channel (the other pairing fails, CPace prevents any takeover); exhaust the shared limits (`rl_global_claim`, same IP address, §8.5); fill the relay's total (§8.4) | The space of a request is that of its identifier, and only that space's database is consulted (§1.4): reading, writing, revoking, purging or counting in another's space is impossible. An identifier that designates no space receives the unknown-identifier response: one cannot know whether it exists elsewhere. A signing key or `device_id` already known in another space is refused at registration (`device_exists`): this reveals the existence of the key only to whoever already knows it. A quota per space (set by the operator) bounds each; cryptography remains the barrier for content (each space has its key, which the relay has never had). |
| **The relay operator** (who hosts other people) | The metadata of **all** spaces (§1.2): devices, times, sizes, presence | Revoke a device, erase a space (`tomi-relay admin`, §8.7), shut down the service | They read nothing and cannot forge a device accepted by the others (like the compromised relay, first line). It is no longer "their own relay" for other people: they SHOULD know what it sees. |
| **Thief of a revoked device** | The device's local content (out of scope for this document) | Connect to the relay | Revocation: key deleted from the relay, sessions and WebSockets cut immediately (§3.4). |
| **Thief of a device not yet revoked, unlocked** | Everything the device sees | Act as the device, including revoking others | Out of the API's scope (strong biometrics on the client side, D39); recourse: `tomi-relay admin revoke` over SSH. |

Out of scope: an actively compromised member device; fine-grained traffic analysis.

### 1.4 Multiple spaces on one relay: how a request designates its own

The relay receives **no** space name from the client. It finds the space from an identifier that the
request already carries and that belongs to only one space:

| Route | Identifier | Resolution |
|-------|-------------|------------|
| `Bearer` routes (§2.4) and `GET /v1/ws` | session token | `SHA-256(token)` → space |
| `POST /v1/auth/challenge`, `POST /v1/auth/session` | `device_id` from the body | `device_id` → space |
| `POST /v1/space` | `install_token` from the body | token → invitation, pending or already consumed (§3.1, §3.2); the space does not exist yet |
| `POST /v1/pairing/channels` | session of the inviting device | space of the session |
| `POST …/claim`, `Pairing <token>` routes of channels | `channel_id` in the URL | `channel_id` → space |
| Channel routes with `Bearer` | session **and** `channel_id` | space of the session; the channel MUST belong to that space, otherwise it is unknown (`404`) |
| `GET /healthz`, `GET /v1/hello`, `/updates/…`, `/.well-known/assetlinks.json`, `/l/…` | none | global |

Rules (normative):

1. **One database per space.** Once the space is resolved, all processing uses only that space's
   database and blob folder (§10). No request touches two spaces, except the computation of the total
   bytes (§8.4), which reads only sizes.
2. **The identifier is unique across the whole relay.** `device_id`, signing key `sign_pk` (active **or
   revoked**) and `channel_id` exist only in one space. The relay MUST refuse to register them if they
   exist in another: `409 device_exists` for a key or a `device_id` (when registering a device, §4.3,
   and when creating a space, §3.2), `409 channel_limit` if no channel number is free (§4.6). Without
   this, the routing of a challenge or of a channel would be ambiguous: a member of Bob's space could
   certify the **public** key of an Alice device without holding its private key.
3. **An unknown identifier gets the unknown-identifier response**, never a response that says
   "it exists, but elsewhere": `401 unauthorized` for a session token or a challenge, `404 not_found`
   for a channel. An unknown `device_id` receives a challenge just like a known `device_id` (§3.3).
4. **`409 space_not_initialized` comes only from a relay that hosts no space.** As soon as a space
   exists, an identifier that designates no space is an unknown identifier (rule 3). Accepted
   consequence in v1: after `admin reset` of a space, or its deletion by its last device (§4.4.1), on a
   relay that hosts other spaces, the devices of that space receive `401 unauthorized` (never
   `space_not_initialized`) and display "space not found"; they do not know that their space was
   deleted (see B.4, point 7). **Neither `401` nor `409 space_not_initialized` proves that a space was
   deleted**: a relay whose volume is not mounted, started with a wrong `data_dir`, or restored without
   that space responds exactly the same way. The only proof of deletion is `200 { "space_deleted": true }`,
   returned by `DELETE /v1/space` or by the receipt `POST /v1/space/receipt` (§4.4.1, §4.4.2); the
   client deletes nothing on any other response (§7.9).
5. **Routing is derived from the space databases.** Nothing used for routing is a source of truth: the
   space's database decides (a stale routing entry gives the same "unknown" response). Routing tables
   are rebuilt at startup (§10). If two spaces carry the same `device_id`, the same key, the same token
   or the same channel (a failed manual restore), the relay REFUSES to start.
6. **The order of checks** (§2.5) stays that of the list, with step 3 becoming "the relay hosts at least
   one space". For `auth/challenge`, `auth/session` and `…/claim`, the space is resolved once the body
   or URL identifier has been validated (step 7), which changes no observable response.

## 2. Conventions

### 2.1 Transport

- Clients ↔ relay: **HTTPS only** (`https://relay.example.org`, `wss://relay.example.org`).
  Clients MUST refuse `http://` and `ws://`, except in a debug build targeting `localhost`,
  `127.0.0.1` or `10.0.2.2` (Android emulator).
- By default the relay listens on `127.0.0.1:8080` (key `listen`). It MUST refuse to start on a
  non-loopback address unless `allow_public_listen = true` (development use only).
- No CORS: the relay sends no `Access-Control-*` header. No client is a browser.
- Every response carries an `X-Request-Id` header (a UUID generated by the relay), repeated in errors and logs.

### 2.2 JSON

- JSON request and response bodies: `Content-Type: application/json`, UTF-8 encoding, an object at the root.
  A JSON request with another `Content-Type` receives `415 unsupported_media_type`.
- Field names in `snake_case`.
- **Unknown fields: ignored**, in both directions (additive compatibility, `PROJECT.md` §6).
- A field marked *optional* may be absent. On input, `null` is equivalent to absent. On output, the relay
  **omits** an optional field that has no value (it never writes `null`), unless explicitly stated otherwise.
- Integers: JSON integer numbers, `0 ≤ n ≤ 2^53 − 1`. A non-integer number or one out of bounds is a validation error.
- Maximum size of a JSON body: `max_json_body_bytes` (default 2,097,152 bytes, 2 MiB, to hold a maximal envelope in base64); otherwise `413 payload_too_large`.

### 2.3 Value types

| Type | JSON form | Rule |
|------|------------|-------|
| `uuid` | string, 36 characters | RFC 9562 form in lowercase with hyphens, e.g. `"308339b6-1358-4bed-8328-23cc68b7f02b"`. Any other form (uppercase, without hyphens, braces) is refused (`422`), as in `format-v1.md` §3.2. Binary form: the 16 bytes in text order. Used for `space_id`, `item_id`, `blob_id`, `request_id`. |
| `device_id` | string, 32 characters | Lowercase hexadecimal, derived from `sign_pk` (§0.3, point 3), e.g. `"3f9a0c6e1b7d4e2a8c5f0b9d7e6a1c34"`. The relay recomputes it and refuses any value that does not match the key. |
| `statement` | `b64` | Signed declaration of `core` (`DeviceCert`, `KeyRotation`, `Tombstone`), binary form `TBS || signature`. |
| `timestamp` | string | RFC 3339 in UTC, **exactly** `YYYY-MM-DDTHH:MM:SS.mmmZ` (milliseconds on 3 digits, `Z` suffix). Any other form is refused (`422 validation_failed`). Binary form: milliseconds since the Unix epoch, a big-endian signed 64-bit integer. |
| `b64` | string | base64url (RFC 4648 §5) **without** `=` padding. The relay refuses padding, characters outside the alphabet, and non-canonical encodings (non-zero trailing bits). |
| `pubkey` | `b64` of 32 bytes (43 characters) | Raw Ed25519 or X25519 public key. |
| `signature` | `b64` of 64 bytes (86 characters) | Ed25519 signature (RFC 8032, "pure", not Ed25519ph). Strict verification (`verify_strict` of `ed25519-dalek`). |
| `seq` | integer ≥ 0 | `0` means "before the first event". |
| `name` | string | Device name: 1 to 64 Unicode characters after trimming leading and trailing spaces, no control character (category Cc). |
| `platform` | string | `^[a-z0-9-]{1,32}$`. Known values: `windows`, `android`, `cli`. An unknown value is accepted and stored. |
| `app_version` | string | 1 to 32 printable ASCII characters (SemVer version recommended). Informational: the relay never compares it. |

### 2.4 HTTP authentication

- Device session: `Authorization: Bearer <session_token>`.
- On the "new device" side of a pairing channel: `Authorization: Pairing <pairing_token>`.
- **No token, secret, signature or challenge ever appears in a URL or in a query string.**
  The only query string parameters of the API are `after`, `limit` and `wait` (§4).
- The relay MUST NOT set a cookie on the API (`/v1/*`). The only exception, outside the API and
  **disabled by default**: the session cookie of the web admin panel (`__Host-tomi-admin`, §8.7.1).

### 2.5 Error model (RFC 9457)

Every 4xx/5xx error response of `/v1/*` has the following `application/problem+json` body:

| Field | Type | Req. | Role |
|-------|------|--------|------|
| `type` | string | yes | `"urn:tomi:problem:<code>"` |
| `title` | string | yes | Short English label, stable per `code` |
| `status` | integer | yes | HTTP code, identical to that of the response |
| `code` | string | yes | **Stable machine code** (table below). Clients decide on `code`, never on `status` or on the text. |
| `detail` | string | no | Technical explanation in English. Never contains request data (body, token). |
| `request_id` | uuid | yes | Same value as `X-Request-Id` |
| `errors` | array | no | For `validation_failed`: `[{ "field": "<JSON pointer path>", "reason": "<English text>" }]` |
| extensions | | no | Fields specific to the code, listed below |

The text shown to the user is chosen by the client from `code` (in French, the default locale).

Example:

```json
{
  "type": "urn:tomi:problem:update_required",
  "title": "Update required",
  "status": 409,
  "code": "update_required",
  "detail": "Space effective format is 2, client format_max is 1.",
  "request_id": "358e7b4a-ca97-49a1-a89d-85a69096e034",
  "effective_format": 2,
  "update_url": "https://relay.example.org/updates/android/latest.json"
}
```

#### Code table

| `code` | HTTP | Meaning | Extensions |
|--------|------|------|------------|
| `bad_request` | 400 | Unreadable JSON, malformed header | |
| `validation_failed` | 422 | A field violates its type or bounds | `errors` |
| `unauthorized` | 401 | Token absent, unknown or malformed; invalid signature; challenge unknown, expired or already used; unknown device. An identifier that designates no space is an unknown identifier (§1.4) | |
| `session_expired` | 401 | Session token known but expired | |
| `install_token_invalid` | 403 | Invalid installation token (or one forgotten by `admin reset`, by the deletion of its space or by `invite --rotate`): no invitation designates it (§3.1). A token that has already created a space receives `space_exists` | |
| `revoked` | 403 | The device has been revoked | `revoked_at` |
| `forbidden` | 403 | Action not permitted for this device (e.g. deleting another device's blob) | |
| `not_found` | 404 | Resource unknown or not visible to this device | |
| `method_not_allowed` | 405 | | |
| `space_exists` | 409 | `POST /v1/space`: the `space_id` already exists on this relay (token pending, not consumed), **or** the token has already created a space, whatever `space_id` is sent (§3.2) | |
| `space_not_initialized` | 409 | The relay hosts **no** space (never created, or the last space deleted by `admin reset` or by `DELETE /v1/space`, §4.4.1). Never returned as long as a space exists (§1.4) | |
| `update_required` | 409 (410 for a retired API version) | The client cannot read the space's effective version, or speaks a retired API version | `effective_format`, `update_url` |
| `format_unsupported` | 409 | The client can no longer write the effective version (its `format_min` is too high) | `effective_format`, `limited_by` (list of `device_id`) |
| `item_id_conflict` | 409 | `item_id` already used with different content | |
| `event_id_conflict` | 409 | `event_id` of a state event already used with another envelope or by another device | |
| `blob_id_conflict` | 409 | `blob_id` already used with another size or by another device | |
| `blob_incomplete` | 409 | Blob not fully sent | `upload_offset` |
| `blob_attached` | 409 | Blob already attached to an item or to a history transfer | |
| `transfer_id_conflict` | 409 | `transfer_id` already used with another manifest or another set of blobs | |
| `transfer_limit` | 409 | Too many open transfers to this recipient | `transfer_max_open` |
| `upload_offset_mismatch` | 409 | `Upload-Offset` differs from the relay's state | `upload_offset` |
| `stale_key_epoch` | 409 | The item is encrypted with an epoch that is no longer the current one | `key_epoch` (current) |
| `epoch_conflict` | 409 | Rotation or registration with an epoch different from the expected one | `key_epoch` (current) |
| `device_set_mismatch` | 409 | The rotation does not cover exactly the members | `expected_device_ids` |
| `device_exists` | 409 | `device_id` or `sign_pk` already registered (even if revoked), in this space **or in another space of the relay** (§1.4) | |
| `last_device` | 409 | Revocation of the last member (`DELETE /v1/me`, `admin revoke`): to delete it together with its space, use `DELETE /v1/space` (§4.4.1) | |
| `not_last_device` | 409 | `DELETE /v1/space` while another active member remains in the space (§4.4.1): nothing is deleted or written | |
| `too_many_devices` | 409 | `max_devices` reached | `max_devices` |
| `channel_claimed` | 409 | Pairing channel already joined | |
| `channel_closed` | 409 | Channel finished (`completed` or `closed`) | |
| `channel_limit` | 409 | Too many open channels or too many messages | |
| `cursor_expired` | 410 | `after` is earlier than the compaction horizon | `retention_floor_seq` |
| `cursor_ahead` | 409 | `after` is greater than the last `seq` (relay restored from a backup) | `head_seq` |
| `already_expired` | 422 | `expires_at` has already passed | `server_time` |
| `length_required` | 411 | `Content-Length` absent on a part upload | |
| `payload_too_large` | 413 | Body, envelope, blob or item too large | `limit` (bytes) |
| `unsupported_media_type` | 415 | | |
| `range_not_satisfiable` | 416 | `Range` invalid or multiple | |
| `rate_limited` | 429 | Rate limit reached. `Retry-After` header (integer seconds ≥ 1). | `retry_after` (seconds) |
| `internal` | 500 | Internal error (details only in the logs) | |
| `unavailable` | 503 | Relay shutting down or database unavailable, or a token, `device_id` (`auth/challenge`, `auth/session`), channel or `space_id` (`POST /v1/space`) belonging to a "pending" space: its deletion is in progress, or its state is unknown until the next startup (§4.4.1). This is neither "deleted" nor "unknown". A `Retry-After` header MAY be present. | |
| `relay_full` | 507 | Space quota, relay total (§8.4) or disk full | `used_bytes`: usage **of this space**; `max_bytes`: effective limit of this space (§8.4); never the usage of another space |

A client that receives an unknown `code` MUST handle it according to the class of the HTTP status
(4xx: permanent failure of this request; 5xx and 429: retry after a delay). This is why no code is
reserved for a future multi-recipient sharing (`format-v1.md` §5.8.4): a code added later is already
understood, down to the HTTP class, by a v1 client.

**Order of checks** (determines which error wins): (1) API version of the path; (2) rate limit per IP;
(3) the relay hosts at least one space (`space_not_initialized`), except for `hello`, `POST /v1/space`
and the update distribution; the request's space is then the one designated by its identifier
(§1.4); (4) authentication (`unauthorized`, `session_expired`, `revoked`); (5) rate limit per device;
(6) content type and body size; (7) validation; (8) business rules.

For an `Authorization: Pairing` request (§4.6), see the exception in §8.5: the token is judged before
step (2); the per-IP limit (`rl_ip_public`) applies only to an **invalid** token, and a **valid** token
is counted as a device at step (5). The observable order for an invalid token remains the one above.

### 2.6 Idempotence

- Item identifiers (`item_id`) and blob identifiers (`blob_id`) are **chosen by the client** (UUID v4).
  Replaying a creation request with the same identifier and the same content returns the initial
  result (`200` instead of `201`) without creating a duplicate or a new event. Different content
  returns `409 item_id_conflict` / `blob_id_conflict`.
- Acknowledgements (`ack`, `read`), tombstones, revocations, push key registration, presence: idempotent.
  A replay returns the same success and does not create a second event.
- Any write request can therefore be replayed after a network interruption without side effects.

### 2.7 Cursors and pagination

Only the log (`GET /v1/events`) is paginated. The cursor is a `seq`: `after=N` returns the events of
`seq > N` in increasing order. The response gives `next_after`, to be passed back unchanged.
`seq` values may show **gaps** (compacted events): the client MUST NOT infer a loss from them.
Pairing channels use a message index on the same principle.

### 2.8 Default size limits

All configurable (§8.6). The configuration's default values are published in `GET /v1/hello`
(`limits`), except `max_relay_bytes` and `max_space_bytes`: a space's usage and effective limit are
given only to that space (`quota` of `GET /v1/space`).

| Key | Default | Scope |
|-----|--------|--------|
| `max_envelope_bytes` | 1 114 112 (1 MiB + 64 KiB; format v1 produces at most 1 048 712 bytes) | size of an item's envelope, decoded bytes |
| `max_blob_bytes` | 26 279 936 (25 MiB + 64 KiB margin for the encryption overhead) | one blob |
| `max_item_blob_bytes` | 105 119 744 (100 MiB + 256 KiB) | sum of an item's blobs |
| `max_blobs_per_item` | 32 | |
| `max_upload_part_bytes` | 1 048 576 (1 MiB) | body of a blob `PATCH` |
| `max_ttl_secs` | 2 592 000 (30 days) | maximum `expires_at − now` at sending: maximum retention time of an **undelivered** item on the relay, even without an expiration wanted by the user (§4.8) |
| `max_devices` | 20 | simultaneous members |
| `max_relay_bytes` | 1 073 741 824 (1 GiB) | total of the `used_bytes` of **all** the relay's spaces (§8.4) |
| `max_space_bytes` | *absent* (no per-space limit) | optional; quota of a space, default for spaces without an `admin quota` override and without a default set from the web panel (§8.4). Absent or `0`: no per-space limit; at most 2^53−1 |
| `max_transfer_bytes` | 1 073 741 824 (1 GiB) | sum of the blobs of a history transfer (§4.13) |
| `max_blobs_per_transfer` | 10 000 | index parts and files of a transfer |
| `transfer_ttl_secs` | 604 800 (7 days) | lifetime of a transfer on the relay |
| `transfer_max_open` | 2 | simultaneous open transfers to the same recipient |
| `max_item_state_bytes` | 4 096 (format v1 produces 320 bytes for a pin, at most 2 112) | envelope of an item-state event (§4.8), decoded bytes |

The limits "25 MiB per file, 100 MiB per item" are limits on the **plaintext**, applied first by the
client (immediate refusal at upload, D18). The relay applies the limits above to the ciphertext.

## 3. Authentication, space, versioning

### 3.1 Installation token and invitations

Creating a space requires an **invitation**, issued by the relay's operator. An invitation is a
**single-use** permission, designated by an installation token.

- The token: 160 random bits (CSPRNG), encoded in **lowercase Crockford base32** (alphabet
  `0123456789abcdefghjkmnpqrstvwxyz`), 32 characters, displayed as 8 groups of 4 separated by `-`,
  e.g. `h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns`.
- **Creation link** (additive, `pairing-v1.md` §10.3): `https://<relay>/create#<token>` (the token in the
  fragment, which the browser does not send to the server; the form `https://<relay>/create/<token>` is also
  read by the clients). The relay never receives the token through this link: the `/create` page is static and
  only offers the download of the application, which reads the link, shows the relay to the user and sends
  the token to `POST /v1/space` (§3.2) of that relay only. This changes no route and no exchange.
- **The relay keeps only the fingerprint**: `SHA-256` of the normalized token (without `-` or spaces, in
  lowercase), the creation date and, once the invitation is used, the consumption date and the created
  `space_id`, in `routing.db` (§10). The token exists in clear only at the moment it is created and displayed;
  it cannot be displayed again. A copy of `routing.db` (backup) yields no token.
- One invitation per space to create: `tomi-relay admin invite --new` creates an invitation and displays its
  token; `admin invite` without options **lists** pending invitations (short fingerprint and date, never the
  token); `--rotate` replaces all pending invitations with a single one (§8.7). Invitations do not expire.
  There is no self-service: without an invitation, nobody creates a space or fills the disk.
- **First startup.** If the relay hosts no space **and** has no invitation (fresh installation), it creates one
  at startup and **displays its token in its logs** (level `WARN`, a single line `install token: <token>`).
  This is the only exception to the rule "never a token in the logs" (§9), acceptable because the token is
  single-use. On later startups, and as soon as a space exists or an invitation is pending, the relay writes
  **no** token: the information line invites the use of `admin invite --new`.
- **Compatibility.** A file `<data_dir>/install-token` left by a single-space relay is read at startup: its
  token becomes an invitation (it remains valid) if no space exists, then the file is deleted.
- Comparison: the relay removes the `-` and spaces, lowercases, computes the fingerprint and looks for the
  invitation that carries it (a token cannot be guessed from the fingerprint: 160 bits).
- When a space is created successfully, **the invitation is consumed** (§3.2): the token never creates a second
  space. The row **remains** in `routing.db`, marked as consumed with the created `space_id`, so that the founder
  whose `201` response was lost, and who resends the same request, receives `409 space_exists` and not "invalid
  token" (§3.2, `pairing-v1.md` §10.2). It disappears with the deletion of that space (`admin reset`, or
  `DELETE /v1/space` by its last device, §4.4.1): the old token then becomes an invalid token (`403`).
  `admin invite --rotate` removes only **pending** invitations. `admin reset` of the **last** space creates a new
  invitation (§8.7); deletion by the last device does not create one (§4.4.1).

### 3.2 Space creation — `POST /v1/space`

No session. The first device generates by itself the `space_id` (UUID v4), its Ed25519 and X25519 keys,
the space key (epoch 1, never sent to the relay) and a **self-signed `DeviceCert` certificate**
(issuer = subject). A self-signed certificate only proves possession of the key; it is the installation
token that authorizes creation. The created space is **one more** on the relay: the others are not touched.

Request:

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `install_token` | string | yes | token from §3.1 |
| `space_id` | uuid | yes | MUST equal the `space_id` of the certificate |
| `name` | name | yes | device name |
| `platform` | platform | yes | |
| `certificate` | statement | yes | self-signed `DeviceCert` (issuer = subject), valid signature |

```json
{
  "install_token": "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns",
  "space_id": "6b5167fa-6c43-43da-a770-bb43fad022e2",
  "name": "Office-PC",
  "platform": "windows",
  "certificate": "CuWt_Gl8zIS7mG_CSqPItfKehWQs7BLknkf3ycw5JfbR90wyAwX5jnHoZTNvpJCHjMEEd6DLhqOnVnuUeX30g2bw1ArZonY-6GkbjqaWS4m1WhNqHgDZAqgQsWmdpmcmiw_321WeCPTLUfaRzlz1hsYg_f9JapBCzlVKoRaCfIm4s29nf5DyE-9TuhUCyCz23MIab6dvAvONO2O8ZWyaAu3KZEDVMU-zobBpW3z31SmeGmxKNaxA7JPcYITsBhSZdQgA2LZaKZnPqWUFVMz6Cr0P"
}
```

(In this whole document, the `statement` and `envelope` values in the examples are illustrative: they have a
plausible length but do not decode.)

Processing (under the creation lock, which serializes space creation, device registration and channel
opening: they check, then insert identifiers that must be unique across the whole relay), in order:
1. no invitation designates the token (invalid, or forgotten by `admin reset` / `--rotate`) →
   `403 install_token_invalid`. The token is judged **before anything else**, so that nobody without a
   token learns which `space_id` values exist;
2. the token's invitation is **already consumed** → `409 space_exists`, **whatever `space_id` is sent**.
   This is the resumption of a founder whose `201` was lost: they resend the same request, receive `409`,
   perform the challenge-response with their pending keys (§3.3), which succeeds, and provision themselves
   (`pairing-v1.md` §10.2). Another device holding the same token also receives `409`, but its
   challenge-response fails with `401` (the relay does not know its `device_id`): it offers the
   "Rejoindre" (Join) action. Nothing leaks to an unknown person: the token is needed to obtain this `409`;
3. the invitation is pending and a space with this `space_id` already exists → `409 space_exists`
   (the invitation is not consumed);
4. decode the certificate (`DeviceCert::from_bytes`); on failure, a different `space_id`, a certificate that
   is not self-signed, or an invalid signature → `422 validation_failed` (`field = "/certificate"`);
5. the founder's signing key or `device_id` already exists in another space of the relay, active or
   revoked → `409 device_exists` (the invitation is not consumed);
6. **consume the invitation** (in `routing.db`: the row receives the date and the `space_id`), then create the
   space's folder and database (§10): `key_epoch = 1`; register the device (`device_id` derived from
   `sign_pk`, `sign_pk` and `kx_pk` read from the certificate, `joined_seq = 0`); add the `device_added` event
   (first event of the log), all within one transaction of the space's database.
   These are **two databases, hence two transactions**: they are not atomic together. If creating the space
   fails, the folder is deleted and the invitation is **restored** (pending again). If the relay stops
   between the two, the invitation remains consumed while the space does not exist: the invitation is lost
   (the founder receives `409 space_exists`, their challenge-response fails with `401`, and the operator
   creates another invitation), but never a space without an invitation.

`201` response:

```json
{
  "space_id": "6b5167fa-6c43-43da-a770-bb43fad022e2",
  "created_at": "2026-11-04T09:00:01.250Z",
  "device_id": "2e2ad3e872079a9cbce60917f275457f",
  "key_epoch": 1,
  "head_seq": 1
}
```

The device then opens a session (§3.3) like any other member.
Other errors: `422 validation_failed` (fields), `429 rate_limited`.
Two simultaneous creations with the same token: exactly one succeeds (`201`), the other finds the
invitation consumed (`409 space_exists`). Two different tokens for the same `space_id`: the second receives
`409 space_exists`. An invalid token consumes nothing.

### 3.3 Session by challenge-response

#### Step 1 — `POST /v1/auth/challenge`

No session. Request: `{ "device_id": device_id }` (required).

`200` response:

```json
{
  "challenge": "B-Qt16lHbSZDnTm5UPmxvpOIUM6mtzT3TKTucuNhx2s",
  "expires_at": "2026-11-04T09:01:01.250Z"
}
```

- `challenge`: 32 random bytes. Lifetime `challenge_ttl_secs` (default 60 s). Single use.
- The relay responds the same way whether the `device_id` exists or not (no existence oracle). The
  `device_id` designates the space (§1.4); a `device_id` that no space knows receives a challenge of
  identical appearance, **which is stored nowhere** (it would serve no purpose: the session for this
  `device_id` will be refused).
  At most 5 pending challenges per `device_id` and 200 per space: beyond that, the oldest is deleted.
- **Accepted trade-off (no content oracle, a residual timing oracle).** The *response* is identical, but
  the *timing* is not quite: a known `device_id` costs a SQLite write with `synchronous = FULL` (an
  `fsync`), an unknown `device_id` costs nothing. An attacker who measures a few milliseconds of difference
  can therefore guess whether a `device_id` **that they already know** exists on this relay. Its value is
  low: a `device_id` is derived from a 256-bit public key, which cannot be guessed; the attacker must already
  hold it (and learns only "this device is here", without the space, a name or a key). v1 does not fix this.
  A possible fix: perform an equivalent write for an unknown identifier (a disposable challenge in
  `routing.db`, purged on expiry).
- Errors: `409 space_not_initialized` (the relay hosts no space), `422 validation_failed`,
  `429 rate_limited`.

#### Step 2 — `POST /v1/auth/session`

No session.

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `device_id` | device_id | yes | |
| `challenge` | b64 (32 bytes) | yes | received in step 1 for this same `device_id` |
| `signature` | signature | yes | over `auth` (§3.6) |
| `client` | ClientInfo | yes | |

ClientInfo:

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `format_min` | integer | yes | ≥ 1 |
| `format_max` | integer | yes | ≥ `format_min` |
| `app_version` | app_version | yes | |
| `platform` | platform | yes | replaces the registered value |
| `capabilities` | list of strings | **reserved** | not sent in v1 and ignored by a v1 relay (§2.2). Intended to announce features added **without** changing `format` (e.g. reading an item under a key outside the space, `format-v1.md` §5.8), which the `format` negotiation (§3.5) cannot signal |

```json
{
  "device_id": "2e2ad3e872079a9cbce60917f275457f",
  "challenge": "B-Qt16lHbSZDnTm5UPmxvpOIUM6mtzT3TKTucuNhx2s",
  "signature": "gx4alllX7-CXbpmVagHBrcAgtuMeRi_LVBOyGHxybzhLRHFmSm8Sqbp04_I4XEM5IJoNiaf19DDi3S5zJKxm_Q",
  "client": { "format_min": 1, "format_max": 1, "app_version": "0.1.0", "platform": "windows" }
}
```

Processing, in order:

0. Find the space of the `device_id` (§1.4); no space knows it → `401 unauthorized`. The following steps
   take place in that space's database.
1. Find the challenge by (`device_id`, `challenge`). **Delete it whatever the result** (no second attempt
   with the same challenge). Missing or expired → `401 unauthorized`.
2. Unknown device → `401 unauthorized`. Revoked device → `403 revoked`.
3. Verify the signature with `sign_pk` → otherwise `401 unauthorized`.
4. Version negotiation (§3.5) → possibly `409 update_required` or `409 format_unsupported`.
   In both cases the device is **not marked active** and no session is created.
5. Record `format_min`, `format_max`, `app_version`, `platform`; `last_seen_at = now`.
6. Create the session.

`200` response:

```json
{
  "session_token": "tms1.10o-uHd2mmD8K1MEX1H7e9MmKldBQ2kaLh_Wyjy4DGY",
  "expires_at": "2026-11-04T10:00:02.000Z",
  "device_id": "2e2ad3e872079a9cbce60917f275457f",
  "space": {
    "space_id": "6b5167fa-6c43-43da-a770-bb43fad022e2",
    "created_at": "2026-11-04T09:00:01.250Z",
    "effective_format": 1,
    "effective_format_limited_by": ["2e2ad3e872079a9cbce60917f275457f"],
    "key_epoch": 1,
    "rotation_required": false,
    "head_seq": 1,
    "server_time": "2026-11-04T09:00:02.000Z",
    "quota": { "used_bytes": 0, "max_bytes": 1073741824 },
    "limits": {
      "max_envelope_bytes": 1114112,
      "max_blob_bytes": 26279936,
      "max_item_blob_bytes": 105119744,
      "max_blobs_per_item": 32,
      "max_upload_part_bytes": 1048576,
      "max_ttl_secs": 2592000,
      "max_devices": 20
    }
  }
}
```

The `space` object (**SpaceState**) is also returned by `GET /v1/space`. Fields, all required:
`space_id` uuid, `created_at` timestamp, `effective_format` integer, `effective_format_limited_by`
list of device_id (active devices whose `format_max` equals `effective_format`), `key_epoch` integer,
`rotation_required` boolean (§3.4), `head_seq` seq, `server_time` timestamp,
`quota` `{used_bytes, max_bytes}` integers, `limits` Limits object (keys of §2.8 except `max_relay_bytes` and
`max_space_bytes`, which are not included). `quota.used_bytes` is the usage **of this space** and `quota.max_bytes`
its effective limit: the smallest of its quota (if it has one) and of `used_bytes` + the room left in the
relay total (§8.4). Never the usage of another space.

SpaceState also carries, as an **additive** field, `features`: list of strings, the relay's capabilities
(identical to that of `GET /v1/hello`, §4.2). A client MUST tolerate its absence (older relay: no
capability) and ignore strings it does not know. A client that never calls `hello` reads the list here,
at each session opening (§7.3).

Session token:
- Form `tms1.` + base64url of 32 random bytes. The prefix lets secret-detection tools recognize it.
- The relay stores only `SHA-256(token)`. Lifetime `session_ttl_secs` (default **3600 s**), **not extendable**.
  Renewal = new challenge-response. Clients SHOULD renew when less than 5 minutes remain, and MUST renew
  on `401 session_expired`.
- At most 5 valid sessions per device; the 6th deletes the oldest.
- `DELETE /v1/auth/session` (with a session) invalidates the current token → `204`.

Each authenticated request re-reads the session in its space's database (no decision cache): a
revocation takes effect on the next request. The token's fingerprint is also used to find the space
(§1.4); this routing entry is added when the session is created, removed at expiry (after the one-day
grace period of §8.3) or at `DELETE /v1/auth/session`, and rebuilt at startup. `last_seen_at` is updated
at most once per minute per device.

### 3.4 Revocation

Three paths lead to revoking a device on the relay:

| Path | Who | Request | Rotation |
|--------|-----|---------|----------|
| (a) Normal | a member revokes one or more other devices | `POST /v1/rotations` with a `KeyRotation` whose `revoked` list is not empty (§4.7) | done in the same request |
| (b) Voluntary departure | the device itself ("leave the space") | `DELETE /v1/me` (§4.4) | to be done by another member |
| (c) Administration | the administrator over SSH | `tomi-relay admin revoke` (§8.7) | to be done by a member |

Common effects, in one transaction, for each revoked device: delete `sign_pk` and `kx_pk` from the members
table (keeping `sign_pk` in the list of forbidden keys), the push token and the presence; mark `revoked_at`;
mark its sessions revoked (kept until they expire, so as to answer `403 revoked` rather than `401`); close
its pairing channels; purge the history transfers of which it is the sender or the recipient (§4.13: blobs
deleted after the commit, `history_transfer_closed` event with `outcome: "revoked"`); add the `device_revoked`
event. For (b) and (c): `rotation_required = true`. For (a): the rotation of the same request is applied,
`rotation_required = false`. Then, **after the commit and before responding**: close all WebSocket
connections of the revoked device with code `4003` (§5.4).

Any later request from this device receives `403 revoked`, including `POST /v1/auth/session`.
A revoked `device_id` or `sign_pk` can never be registered again (`409 device_exists`).

`rotation_required` tells members that they MUST rotate the space key by listing the revoked device in
`revoked` (algorithm §7.6). The relay does not prevent sending while waiting. In cases (b) and (c), the
revocation is **not signed** by a member: clients accept it (the worst effect is a denial of service) and
display it as coming from the device itself or from the administrator.

### 3.5 Version negotiation

Three numbers (`PROJECT.md` §6): `api` (the `/v1` path), `format`, `key_epoch`.

**API version.** `GET /v1/hello` publishes `api_versions` (list of supported integers, `[1]` here).
A request to `/v<N>/...` with an integer `N` lower than the smallest supported version receives
`410 update_required` with `update_url` (§4.12), derived from the `User-Agent` header if it starts with
`tomi-<platform>/`, otherwise `https://<public_host>/updates/`. `N` greater → `404 not_found`.
Clients SHOULD send `User-Agent: tomi-<platform>/<app_version>`.

**Effective version of the space**, computed at each `POST /v1/auth/session` for device `D`:

```
A        = active members (last_seen_at ≥ now − inactive_after_days), D excluded
E_others = min(format_max of the devices in A)        // undefined if A is empty
if D is active:             E = min(E_others, D.format_max)   // D holds the version, never refused for that
else if A is empty:         E = D.format_max
else if D.format_max < E_others: refuse → 409 update_required (effective_format = E_others)
else:                       E = E_others
if D.format_min > E: refuse → 409 format_unsupported (effective_format = E, limited_by = devices of A with format_max = E)
```

"D is active" is judged **before** `last_seen_at` is updated, using the `format_max` that it announces
now. `effective_format` is returned in SpaceState. A sender always writes in `effective_format`; it
re-reads it at each new session (at least once an hour).

The relay never reads the `format` of an envelope: it cannot verify that a sender respects the effective
version. An item of an unknown format is handled by the client (§7.4).

`update_url` is `https://<public_host>/updates/<platform>/latest.json` (platform of the request).

### 3.6 Signatures

#### Challenge signature (specific to the API)

```
msg = "tomi/v1/sig/relay-auth" || 0x00 || space_id (16 bytes) || sign_pk (32 bytes) || challenge (32 bytes)
signature = Ed25519(sign_sk, msg)        // RFC 8032 "pure", 64 bytes
```

Same convention as the declarations of `core`: ASCII label, null byte, fixed-size fields concatenated
without a length prefix. `msg` is 22 + 1 + 16 + 32 + 32 = 103 bytes. The label is reserved for this use
by `core` (`labels::SIG_RELAY_AUTH_RESERVED`): no other declaration can be replayed as an authentication
proof, and vice versa. `core` SHOULD expose `sign_relay_auth(space_id, challenge)` so that clients do not
build `msg` themselves.

#### Declarations of `core` verified by the relay

The relay decodes and verifies these declarations with `tomi-core` (functions `from_bytes` and
`verify_signature`, strict Ed25519 verification). Any violation → `422 validation_failed` with
`field` = the JSON field concerned, unless a more specific code is mentioned.

| Declaration | Received by | Relay checks |
|-------------|-----------|--------------------|
| `DeviceCert` | `POST /v1/space`, `POST /v1/devices` | decoding; `space_id` of the space; signature; at creation: self-signed; at pairing: issuer = `sign_pk` of the caller; subject (`sign_pk`) never seen, member or revoked (otherwise `409 device_exists`) |
| `KeyRotation` | `POST /v1/rotations` | decoding; `space_id`; `new_epoch` = current epoch + 1 (otherwise `409 epoch_conflict`); issuer = `sign_pk` of the caller; signature; each key in `revoked` is that of a known device (member or already revoked) and differs from the caller's; the set of keys in `recipients` equals exactly the current members minus those in `revoked` (otherwise `409 device_set_mismatch`) |
| `Tombstone` | `POST /v1/tombstones` | decoding; `space_id`; "everywhere" scope (a "local" scope is never sent); signer = `sign_pk` of the caller; signature |
| `AttachmentTombstone` | `POST /v1/attachment-tombstones` | decoding (exact length, label, `deleted_at ≤ 2^53 − 1`); `space_id`; signer = `sign_pk` of the caller; signature; blob read at `(item_id, slot)` and `blob_id` consistent with what the relay holds (§4.8 bis) |
| `HistoryTransfer` | `POST /v1/transfers` | decoding (`HistoryTransfer::from_bytes`); `space_id`; `transfer_id` = that of the body; sender = `sign_pk` of the caller; signature (`verify_signature`); recipient = non-revoked member different from the caller; `joined_seq(recipient) ≤ as_of_seq ≤ head_seq`; parts ⊆ `blob_ids` of the body. The relay cannot open `sealed_hk`. |

Clients ALSO verify these declarations, independently, when receiving events, with their own list of
trusted devices (`TrustStore` of `core`): the relay is not a point of trust. Certificates form a chain
starting from the founding device; a device never encrypts the space key for a key that is not in its
trust list (D38).

Replay protection: the challenge is single-use and lives 60 s; a `DeviceCert` can register its `sign_pk`
only once across the whole relay (all spaces, §1.4); a `KeyRotation` is bound to `new_epoch = current + 1`;
a `Tombstone` is idempotent; a `HistoryTransfer` is bound to its `transfer_id` (idempotent,
`transfer_id_conflict` otherwise) and to its recipient, and its blobs disappear after 7 days. Everything is
bound to `space_id`: a signature is not valid on another space, nor after a `reset` (the new space has a
new `space_id`).

## 4. Endpoints

### 4.1 Summary table

Auth: **—** none; **B** `Bearer` session of a member device; **P** `Pairing` (new device);
**B/P** either one; **I** installation token in the body.

**Common** errors, not repeated in the table:
all `/v1` routes: `400 bad_request`, `410 update_required` (API version), `429 rate_limited`,
`500 internal`, `503 unavailable`; routes with a JSON body: `413 payload_too_large`,
`415 unsupported_media_type`, `422 validation_failed`; **B** routes: `401 unauthorized`,
`401 session_expired`, `403 revoked`; all `/v1` routes except `hello`, `POST /v1/space` and
`POST /v1/space/receipt`: `409 space_not_initialized` (only if the relay hosts no space, §1.4).

| # | Method and path | Auth | Success | Specific errors | § |
|---|-------------------|------|--------|-----------------|---|
| 1 | `GET /healthz` | — | 200 | 503 | 4.12 |
| 2 | `GET /v1/hello` | — | 200 | | 4.2 |
| 3 | `POST /v1/space` | I | 201 | 403 `install_token_invalid`, 409 `space_exists`, `device_exists` | 3.2 |
| 4 | `POST /v1/auth/challenge` | — | 200 | | 3.3 |
| 5 | `POST /v1/auth/session` | — | 200 | 401 `unauthorized`, 403 `revoked`, 409 `update_required`, 409 `format_unsupported` | 3.3 |
| 6 | `DELETE /v1/auth/session` | B | 204 | | 3.3 |
| 7 | `GET /v1/space` | B | 200 | | 4.2 |
| 8 | `GET /v1/devices` | B | 200 | | 4.3 |
| 9 | `POST /v1/devices` | B | 201 | 404 (channel), 409 `channel_closed`, `device_exists` (also for a key of another space), `too_many_devices`, `epoch_conflict`; 422 (certificate) | 4.3 |
| 10 | `PATCH /v1/devices/{device_id}` | B | 200 | 404 | 4.3 |
| 11 | `DELETE /v1/me` | B | 200 | 409 `last_device` | 4.4 |
| 12 | `PUT /v1/me/push` | B | 204 | | 4.5 |
| 13 | `DELETE /v1/me/push` | B | 204 | | 4.5 |
| 14 | `PUT /v1/me/presence` | B | 204 | | 4.5 |
| 15 | `POST /v1/pairing/channels` | B | 201 | 409 `channel_limit` | 4.6 |
| 16 | `POST /v1/pairing/channels/{channel_id}/claim` | — | 200 | 404, 409 `channel_claimed`, `channel_closed` | 4.6 |
| 17 | `POST /v1/pairing/channels/{channel_id}/messages` | B/P | 201 | 401, 404, 409 `channel_closed`, `channel_limit`; 413 | 4.6 |
| 18 | `GET /v1/pairing/channels/{channel_id}/messages` | B/P | 200 | 401, 404 | 4.6 |
| 19 | `DELETE /v1/pairing/channels/{channel_id}` | B/P | 204 | 401, 404 | 4.6 |
| 20 | `POST /v1/rotations` | B | 201 / 200 | 409 `epoch_conflict`, `device_set_mismatch`; 422 (declaration) | 4.7 |
| 21 | `GET /v1/keys` | B | 200 | | 4.7 |
| 22 | `POST /v1/items` | B | 201 / 200 | 409 `item_id_conflict`, `stale_key_epoch`, `blob_incomplete`, `blob_attached`; 404 (blob); 413; 422 `already_expired`; 507 `relay_full` | 4.8 |
| 23 | `GET /v1/items/{item_id}` | B | 200 | 404 | 4.8 |
| 24 | `POST /v1/items/{item_id}/ack` | B | 200 | 404 | 4.8 |
| 25 | `POST /v1/items/{item_id}/read` | B | 200 | 404 | 4.8 |
| 26 | `POST /v1/tombstones` | B | 201 / 200 | 422 (declaration) | 4.8 |
| 27 | `POST /v1/blobs` | B | 201 / 200 | 409 `blob_id_conflict`; 413; 507 `relay_full` | 4.9 |
| 28 | `GET /v1/blobs/{blob_id}/status` | B | 200 | 404 | 4.9 |
| 29 | `PATCH /v1/blobs/{blob_id}` | B | 204 | 404, 409 `upload_offset_mismatch`, `blob_attached`, 411, 413, 415 | 4.9 |
| 30 | `GET` / `HEAD /v1/blobs/{blob_id}` | B | 200 / 206 | 404, 409 `blob_incomplete`, 416 | 4.9 |
| 31 | `DELETE /v1/blobs/{blob_id}` | B | 204 | 403 `forbidden`, 404, 409 `blob_attached` | 4.9 |
| 32 | `GET /v1/events` | B | 200 | 409 `cursor_ahead`, 410 `cursor_expired` | 4.10 |
| 33 | `GET /v1/ws` | (1st message) | 101 | close codes §5.4 | 5 |
| 34 | `GET /updates/{chemin}` | — | 200 / 206 / 304 | 404, 416 | 4.12 |
| 35 | `GET /.well-known/assetlinks.json` | — | 200 | 404 | 4.12 |
| 36 | `GET /l/{chemin}` | — | 200 | 404 | 4.12 |
| 37 | `POST /v1/transfers` | B | 201 / 200 | 404 (blob), 409 `transfer_id_conflict`, `blob_incomplete`, `blob_attached`, `transfer_limit`; 413; 422 (manifest); 507 `relay_full` | 4.13 |
| 38 | `GET /v1/transfers/{transfer_id}` | B | 200 | 404 | 4.13 |
| 39 | `DELETE /v1/transfers/{transfer_id}` | B | 204 | 404 | 4.13 |
| 40 | `POST /v1/item-states` | B | 201 / 200 | 409 `event_id_conflict`, `stale_key_epoch`; 413; 507 `relay_full` | 4.8 |
| 41 | `DELETE /v1/space` | B | 200 | 409 `not_last_device`; 422 (`deletion_id`) | 4.4.1 |
| 42 | `POST /v1/space/receipt` | — | 200 | 404 `not_found` | 4.4.2 |
| 43 | `POST /v1/attachment-tombstones` | B | 201 / 200 | 422 (declaration, blob) | 4.8 bis |

Any other known method/path combination → `405 method_not_allowed` (with an `Allow` header); unknown path → `404 not_found`.

### 4.2 Hello and space state

#### `GET /v1/hello` — no auth, no space check (global to the relay)

```json
{
  "service": "tomi-relay",
  "relay_version": "0.1.0",
  "api_versions": [1],
  "space_initialized": true,
  "server_time": "2026-11-04T09:00:02.000Z",
  "limits": {
    "max_envelope_bytes": 1114112,
    "max_blob_bytes": 26279936,
    "max_item_blob_bytes": 105119744,
    "max_blobs_per_item": 32,
    "max_upload_part_bytes": 1048576,
    "max_ttl_secs": 2592000,
    "max_devices": 20,
    "max_transfer_bytes": 1073741824,
    "max_blobs_per_transfer": 10000,
    "transfer_ttl_secs": 604800,
    "transfer_max_open": 2
  },
  "features": ["attachment_tombstones"]
}
```

`space_initialized` is `true` as soon as the relay hosts **at least one space**. It is **informational** and
says nothing about the caller's space, which this route cannot identify (it has no identifier). A client
MUST NOT conclude from it that its space exists, nor refrain from creating a space (which only an invitation
authorizes, §3.1). `limits` gives the default configuration values, never the usage or quota of a space:
those are read with a session, in `GET /v1/space`.

All fields are required for a relay that conforms to this document. The last four limits (§4.13) are an
additive addition: a client MUST tolerate their absence (older relay) and then does not offer history
transfer. `server_time` lets the client estimate the offset of its clock (it takes it into account for
`expires_at`).

`features` (additive) is the list of the relay's capabilities; a client MUST tolerate its absence (older
relay: no capability) and ignore unknown strings. Defined capability: `attachment_tombstones`, that is, the
route `POST /v1/attachment-tombstones` (§4.8 bis). A client offers the function only if the relay lists it;
**the relay must therefore be deployed before the apps** (§12, B.2). The same list is in SpaceState (§3.3).

#### `GET /v1/space` — B

`200` response: SpaceState (§3.3), for the session's space (§1.4).

### 4.3 Devices

#### DeviceRecord (returned object)

| Field | Type | Always present | Meaning |
|-------|------|------------------|------|
| `device_id` | device_id | yes | |
| `name` | name | yes | |
| `platform` | platform | yes | |
| `sign_pk` | pubkey | yes | |
| `kx_pk` | pubkey | yes | |
| `certificate` | statement | yes | `DeviceCert` received at registration (self-signed for the founder) |
| `issuer_device_id` | device_id | yes | issuer of the certificate (equal to `device_id` for the founder) |
| `joined_seq` | seq | yes | the device only sees events with `seq > joined_seq` |
| `registered_at` | timestamp | yes | |
| `last_seen_at` | timestamp | no | absent if never connected; rounded down to the minute |
| `active` | boolean | yes | §0.2 |
| `present` | boolean | yes | §6.2 |
| `push` | boolean | yes | a push token is registered |
| `format_min`, `format_max` | integer | no | absent before the first session |
| `app_version` | app_version | no | same |
| `capabilities` | list of strings | no | **reserved** (ClientInfo, §3.3): never returned by a v1 relay |

#### `GET /v1/devices` — B

`200` response: `{ "devices": [DeviceRecord, ...] }`, non-revoked members only, sorted by
`registered_at`. Revocations are read from the log (`device_revoked`).

#### `POST /v1/devices` — B (registration after pairing)

Called by the **inviting device**, after the pairing protocol (outside this document) has passed the
identity of the new device to it over the encrypted channel, and it has passed the current space key to it.

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `channel_id` | string | yes | channel created by the caller, in state `claimed` |
| `key_epoch` | integer | yes | epoch of the space key passed to the new device; MUST equal the current epoch |
| `name` | name | yes | name of the new device (passed over the pairing channel) |
| `platform` | platform | yes | |
| `certificate` | statement | yes | `DeviceCert` of the new device, issued and signed by the caller |

```json
{
  "channel_id": "4821",
  "key_epoch": 1,
  "name": "Pixel",
  "platform": "android",
  "certificate": "CuWt_Gl8zIS7mG_CSqPItfKehWQs7BLknkf3ycw5JfbR90wyAwX5jnHoZTNvpJCHjMEEd6DLhqOnVnuUeX30g2bw1ArZonY-6GkbjqaWS4m1WhNqHgDZAqgQsWmdpmcmiw_321WeCPTLUfaRzlz1hsYg_f9JapBCzlVKoRaCfIm4s29nf5DyE-9TuhUCyCz23MIab6dvAvONO2O8ZWyaAu3KZEDVMU-zobBpW3z31SmeGmxKNaxA7JPcYITsBhSZdQgA2LZaKZnPqWUFVMz6Cr0P"
}
```

Checks and errors, in order: unknown, expired or another creator's channel → `404 not_found`;
channel in state `open`, `completed` or `closed` (not `claimed`) → `409 channel_closed`; `key_epoch` ≠ current → `409 epoch_conflict`;
certificate (§3.6: decoding, `space_id`, issuer = caller, signature) → `422 validation_failed`
(`field = "/certificate"`); subject already known, member or revoked, **in this space or in another space of the
relay** (§1.4) → `409 device_exists` (the same error in both cases);
members = `max_devices` → `409 too_many_devices`.

Registration takes place under the creation lock (§3.2): two spaces that receive the same subject at the same
instant do not both accept it.

Effects (transaction): register the device (`device_id` derived from the subject's `sign_pk`,
`joined_seq` = `head_seq` before the addition, `last_seen_at` absent), add `device_added`, move the channel
to `completed`. `201` response: DeviceRecord. Doorbell (without ringing) to the other members.

The new device learns the list of the other members through a signed `MemberList` that the inviter passes
to it over the encrypted channel (`core`), not through the relay.

#### `PATCH /v1/devices/{device_id}` — B

Request: `{ "name": name }` (required). Any member can rename any member of its space (single-user: the
devices of a space all belong to the same person). Effects: update; a `device_updated` event if the name
changes (otherwise none). `200` response: DeviceRecord. `404` if the device is not a member.

The name is **not signed**: a compromised relay can rename a device. Clients MUST also identify devices on
sensitive screens (revocation) by a fingerprint of `sign_pk`.

### 4.4 Voluntary departure — `DELETE /v1/me` — B

Empty body. The calling device removes itself from the space (path (b) of §3.4). To revoke **another**
device, a member uses `POST /v1/rotations` with a `revoked` list (§4.7): revocation and rotation are then atomic.

Error: the caller is the last member → `409 last_device` (delete the space with `DELETE /v1/space`,
§4.4.1, or use `tomi-relay admin reset`).
Effects: §3.4, with `revoked_by` = the caller. `200` response:
`{ "seq": <seq of device_revoked>, "rotation_required": true }`. The session used becomes invalid
immediately (`403 revoked`); the device then erases its local keys. Client algorithm: §7.9.

*Idempotence.* Departure is also proven to a retry: a departed device receives `403 revoked` for any
authenticated request and for `POST /v1/auth/session`, for as long as the space exists (its `devices` row and
its key in `revoked_sign_pks` are kept, §3.4). Two simultaneous `DELETE /v1/me` from the same device: one
receives `200`, the other `403 revoked` (the relay re-reads the device within the transaction; it never
answers `401` to a device it has just revoked).

### 4.4.1 Deletion of the space by its last device — `DELETE /v1/space` — B

Body: empty, or JSON `{ "deletion_id": b64 }` (exactly 32 bytes; **optional, additive** field: an earlier
client sends an empty body). The caller deletes the **entire space** if, and only if, it is the **only active
member**. This is the departure of the last device: no one can read the space any more, and the relay has no
reason to keep encrypted data for which no device holds the key. The interface offers it behind an explicit
confirmation, since deletion is irreversible.

`deletion_id` is drawn at random by the client on its first request, kept locally and sent back **identically**
on each retry (§7.9). The relay keeps only `SHA-256(deletion_id)`, and only if the deletion succeeded: it is the
**receipt** that lets a client whose `200` response was lost obtain the proof anyway (§4.4.2). A body that is
present requires `Content-Type: application/json` (otherwise `415`) and must not exceed 1 KiB (otherwise `413`).
A `deletion_id` that is present but badly encoded or of another length gives `422 validation_failed`, and
nothing is deleted; absent, `null`, empty body or `{}`: accepted, without a receipt.

The client draws a **new `deletion_id` for each deletion** and never reuses it for another space: the relay
cannot verify it (it keeps only its fingerprint, and only after a successful deletion). A `deletion_id` already
used would wrongly prove the deletion of another space (§4.4.2).

*Path.* `/v1/space` is already the resource of `POST /v1/space` (creation) and `GET /v1/space` (state):
`DELETE` removes it. `DELETE /v1/me` remains the departure of **one** device and requires that another member
remain.

*Who can.* The request first passes the gate of any **B** route (§2.5): an absent or unknown token, an expired
one, or that of a revoked device receives `401` or `403 revoked`. Then, in the critical section below:

- the caller is still an active member (otherwise `403 revoked` if it has just been revoked, `401` if it is unknown);
- it is the **only** active member (revoked devices do not count), otherwise `409 not_last_device`:
  nothing is deleted and nothing is written (no event, no doorbell).

*Atomicity.* The check and the deletion form a single critical section. The relay takes the lock of the
identifier registry (the one used by `POST /v1/devices`, channel creation and `admin reset`), reads the members
in a write transaction of the space's database (serialized with all the others, including `DELETE /v1/me`),
then deletes **without releasing** the lock. No one can therefore join the space between the check and the
deletion: a simultaneous `POST /v1/devices` either came before (the deletion sees two members and answers
`409`), or is refused afterwards (the space no longer exists). A simultaneous `DELETE /v1/me` by another member
is ordered the same way: leaving first leaves the caller alone (`200`), being judged first gives `409`. Two
simultaneous calls from the same device: only one `200`. The deletion runs in a task that holds the lock: a
client that cuts the connection does not interrupt the deletion, and does not release the lock before it ends.

*Effects.* The same as `admin reset` of this space (§8.7), the relay reading no content. **Fail-closed**: the
relay answers `200` only if it has itself succeeded in a positive action on disk, made it durable, and recorded
the receipt. In this order:

1. **removal from routing**: the space becomes "pending". No request reaches it any more; its tokens, its
   `device_id` values (`auth/challenge`, `auth/session`), its channels and its `space_id` (`POST /v1/space`)
   receive `503 unavailable`; a WebSocket that opens for it is closed with `1013`; the relay does not count as
   empty because of it (`hello.space_initialized` stays true). WebSockets already open **remain open** at this
   stage;
2. **presence verified**: `spaces/<id>/` must be a folder and `spaces/<id>/relay.db` an ordinary file, read just
   before acting (without following a symbolic link), **and these must be the ones the space was loaded from**:
   the relay records the identity `(st_dev, st_ino)` of the folder and of `relay.db` when it loads, creates or
   re-registers the space, and any difference counts as absence. A path alone is not enough: after a `umount -l`
   under a running relay, the mount point may hide an old copy of `spaces/<id>/` that the rename and the `fsync`
   would accept, whereas the real space returns on remount. This identity protection exists under Unix
   (production); under Windows (development) the check remains that of the path. Otherwise (volume not mounted,
   wrong `data_dir`, folder moved or replaced by hand), **nothing is deleted**: *Failure*, case (a);
3. **positive action, point of no return**: rename `spaces/<id>/` to `<data_dir>/spaces.deleted-<id>-<random>/`
   (outside `spaces/`); if the rename is refused, delete `spaces/<id>/relay.db` in place, then its `-wal` and
   `-shm` files. **Only the success of one of these two system calls counts.** "The file does not exist"
   (`ENOENT`) is never a success, neither for the rename nor for the deletion; such a result after step 2 is an
   unknown state: *Failure*, case (c);
4. **durability**: `fsync` of the modified folders (`data_dir` and `spaces/` after a rename, `spaces/<id>/`
   after an in-place deletion; Unix only, no effect elsewhere). On failure: *Failure*, case (b);
5. **receipt, in a transaction of `routing.db`**: forget the space's quota override and the invitation it had
   consumed (its token becomes an invalid token, `403`), and, if `deletion_id` is supplied, insert
   `(SHA-256(deletion_id), time)` into `space_deletions` (§4.4.2, §10.2). On failure of the transaction:
   *Failure*, case (b);
6. permanent removal of the space's identifiers (`device_id`, keys, tokens, channels), which become free again;
   closing of **all** the space's WebSockets, the caller's included, with `4004` (`space_reset`, §5.4), **before**
   the response, and only now;
7. removal of what remains on disk (renamed folder, or the remainder of the folder after an in-place deletion):
   best effort, without making the call fail. A `spaces.deleted-*` that resists is deleted at the next startup;
   the remainder of an in-place deletion no longer has a `relay.db` and is ignored at startup (§10.1).

Nothing else is touched: the other spaces, their connections, `public/`, the pending invitations.

*Response.* `200` `{ "space_deleted": true }`, only after steps 2 to 6 have succeeded. This field, with the
boolean value `true`, is the proof of deletion, like `seq` for `DELETE /v1/me` (§7.9): a `2xx` response without
it, or with another value, proves nothing. Specific error: `409 not_last_device`.

*Failure.* No failure outcome gives the caller `200`, nor `401`, nor `409 space_not_initialized`. The body
contains neither `space_id` nor a path (the details go to the operator's logs, §9):

- (a) **nothing has been deleted** (step 2 refused, or rename and in-place deletion both refused):
  `routing.db` was not touched. The relay **re-registers the space** as at startup (database reopened, routing,
  registry), without touching the sessions or the open WebSockets: the space responds again, whole, and
  **the call can be retried without restarting the relay**. Response `500 internal`. If the database cannot be
  reopened (or step 2 found it absent), the space stays "pending" until the next startup, where it returns whole
  if it is there: response `503 unavailable`, and its tokens receive `503` in the meantime;
- (b) **the positive action took place but the relay cannot prove it durably** (step 4 or 5 failed):
  response `500 internal`, **without receipt**. After an `fsync` failure, the space stays "pending" (`503`) until
  the next startup, which finds it whole (rename lost) or does not find it. In this second case the deletion is
  real but **without receipt** (an `fsync` failure followed by a restart): startup cannot distinguish a deleted
  space from an unmounted volume (§10.1), and writes nothing. The client has no proof and goes through
  "Erase anyway" (§7.9, D4). After a failure of the receipt transaction alone, the space is deleted for good; its
  identifiers are freed as in step 6, and the quota or invitation not forgotten remain as orphan rows (§10.1).
  The client has no proof and erases nothing: it ends up in the "space not found" state (§7.9), from which the
  user can choose "Erase anyway". These two cases (`fsync` failure then restart, failure of the receipt
  transaction) are the only ones where a successful deletion cannot be proven;
- (c) **unknown state** (panic, unexpected `ENOENT`, I/O error during the check): the space stays "pending"
  until the next startup; response `503 unavailable`.

*After.* The relay responds as for a space that no longer exists (§1.4): `409 space_not_initialized` if it
hosts none any more, otherwise `401 unauthorized`. Neither of these two responses is a proof (§1.4, rule 4):
a client whose `200` response was lost obtains the proof through the receipt (§4.4.2). A repeated or
simultaneous call during the deletion receives `503 unavailable` (retry).

*Starting over.* The relay does **not** create an invitation on this occasion, unlike `admin reset` of the last
space, which displays its token on the operator's console. The relay keeps only the fingerprint of a token
(§3.1) and is allowed to log it only at first startup (§9): a token created here would have no safe recipient
(the HTTP response would carry it through the device and the network, which an installation secret must not do).
To create a new space, the operator runs `tomi-relay admin invite --new` on the server; a relay that has neither
a space nor a pending invitation creates one at its next startup and displays its token (§3.1). Pending
invitations are kept.

*Client reaction.* Full table in §7.9 (tables T2 and T3). In short: only `200 { "space_deleted": true }`
(or the same body returned by the receipt, §4.4.2) erases the device; `403 revoked` only proves that the device
is no longer a member (departure proof); `409 not_last_device` leads back to normal departure (`DELETE /v1/me`);
any other response leaves the device intact.

### 4.4.2 Deletion receipt — `POST /v1/space/receipt` — no auth

Body `{ "deletion_id": b64 }` (32 bytes, the same as the one sent to `DELETE /v1/space`; 1 KiB at most,
otherwise `413`). Global route, like `hello`: no session (the caller's session disappeared with the space), no
space check, never `409 space_not_initialized`. Rate limits `rl_ip_public` and `rl_ip_auth` (§8.5).

| The relay has… | Response |
|-------------|---------|
| deleted a space through a `DELETE /v1/space` that carried this `deletion_id`, less than `deletion_receipt_ttl_secs` ago (default 90 days) | `200 { "space_deleted": true }` |
| no receipt for this `deletion_id` (never seen, deletion not completed, receipt expired) | `404 not_found` |
| a `deletion_id` badly encoded or of another length | `422 validation_failed` |

Rules:

- The relay keeps only `SHA-256(deletion_id)` and the time of the deletion (table `space_deletions` of
  `routing.db`, §10.2): neither `space_id`, nor `device_id`, nor key. The receipt is written in the transaction
  that forgets the space's quota and invitation (step 5 of §4.4.1), **after** the positive action has been made
  durable: a receipt therefore exists only for a space that was really deleted.
- `deletion_id` is a weak client secret (32 random bytes): whoever does not know it learns nothing (`404`,
  identical for all unknown identifiers). It never appears in a URL or in a log.
- The response is idempotent: it can be requested several times as long as the receipt has not expired.
  Maintenance deletes receipts older than `deletion_receipt_ttl_secs`.
- `admin reset` writes **no** receipt: after a manual reset, the devices of the space have no proof and do not
  erase themselves (state "space not found", §7.9).
- A relay prior to this route answers `404` (unknown path): no proof, which is safe.

### 4.5 Push and presence

#### `PUT /v1/me/push` — B

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `provider` | string | yes | `"fcm"` in v1. Unknown value → `422`. (`"unifiedpush"` is reserved.) |
| `token` | string | yes | FCM registration token, 1 to 4096 printable ASCII characters |

Replaces the device's previous registration, if any. A same `token` already registered by another device
**of the same space** is removed from that device (reinstallation); two spaces may carry the same token (the
token is a registration of the space, and the doorbell of one does not depend on the other). `204`. The token
never appears in a response or a log.

#### `DELETE /v1/me/push` — B → `204` (even without a registration).

#### `PUT /v1/me/presence` — B

Request: `{ "present": boolean }` (required). `true` → `present_until = now + presence_ttl_secs`
(default 120 s); `false` → presence cleared. `204`. Semantics and frequency: §6.2. No event.

### 4.6 Pairing rendezvous

The relay offers an **ephemeral mailbox** where the inviting device and the new device exchange opaque
messages (PAKE messages, then messages encrypted with the key derived from the PAKE; their content is defined
elsewhere). The relay derives no trust from them: the identity of the new device enters the space only through
the signed certificate of `POST /v1/devices`.

Channel lifecycle: `open` → (`claim`) → `claimed` → (`POST /v1/devices`) → `completed`;
at any time `DELETE` → `closed`; at expiry → deleted (404).

| Parameter | Default | Key |
|-----------|--------|-----|
| Lifetime of an unjoined channel | 120 s after creation | `pairing_ttl_secs` |
| Lifetime after `claim` | 120 s after the `claim` (a single extension) | `pairing_ttl_secs` |
| Simultaneous `open`/`claimed` channels **per space** | 2 | `pairing_max_open` |
| Messages per side and per channel | 8 | `pairing_max_messages` |
| Size of a message (decoded bytes) | 8192 | `pairing_max_message_bytes` |
| Number of successful `claim`s per channel | **1** | — |

Rate limits for channel requests: §8.5. In particular, a request carrying the channel's `Pairing` token is not
counted per IP (`rl_ip_public`) but per channel (`rl_device`), whereas any other `Pairing` token is.

`channel_id`: a decimal string of 1 to 4 digits with no leading zero, drawn at random (CSPRNG) among the
values `1` to `9999` not used by an existing channel **of any space on the relay** (a closed channel keeps its
number until its expiry). The number designates the channel's space (§1.4): this is how the new device, which
has no identifier yet, reaches its inviter's space. With `pairing_max_open = 2` per space, 9,999 numbers suffice
for several thousand spaces; changing the form of the number would affect the `sid` of pairing
(`spec/pairing-v1.md`), which this version avoids. It is short so that it can appear in a code typed by hand
(e.g. `7-crayon-lune` → channel `7`). It is not secret: security comes from the PAKE, where an attacker who joins
the channel has only one attempt, after which the channel is consumed. Another person on the same relay can
therefore, like anyone, claim an open channel at random and make that pairing fail (denial of service, not
takeover): the risk is bounded by `rl_ip_claim` and `rl_global_claim` (§8.5).

#### `POST /v1/pairing/channels` — B (inviting device)

Body: `{}` (no field). `201` response:

```json
{ "channel_id": "4821", "state": "open", "expires_at": "2026-11-04T09:12:00.000Z" }
```

Error: `409 channel_limit` if `pairing_max_open` channels of **the space** are already open, or if no number
is free on the relay.

#### `POST /v1/pairing/channels/{channel_id}/claim` — no auth (new device)

Body: `{}`. `200` response:

```json
{ "pairing_token": "tmp1.uFJcfepRayMOiriiRhh-eL7yHvoUg3Ej", "expires_at": "2026-11-04T09:14:00.000Z" }
```

`pairing_token`: `tmp1.` + base64url of 24 random bytes; only its SHA-256 is stored; it is valid only for
this channel. Errors: unknown or expired → `404`; already joined → `409 channel_claimed` (the channel remains
usable by the first arrival); `completed`/`closed` → `409 channel_closed`.

#### `POST /v1/pairing/channels/{channel_id}/messages` — B (channel creator) or P (token holder)

Request: `{ "body": b64 }` (required, 1 to `pairing_max_message_bytes` decoded bytes).
The side is deduced from authentication: `inviter` (session of the creator) or `joiner` (Pairing token).
A B session from a device other than the creator, or from another space → `404`. `201` response: `{ "index": integer }`
(index of the message in the channel, increasing from 1, common to both sides).
The inviter MAY write as soon as the state is `open` (its first PAKE message waits for the new device);
the new device writes after its `claim`. Errors: channel `completed` or `closed` → `409 channel_closed`;
message quota of the side reached → `409 channel_limit`; message too large → `413`.
The inviter's messages written before the `claim` are kept and read by the new device.

#### `GET /v1/pairing/channels/{channel_id}/messages?after=<index>&wait=<seconds>` — B/P

- `after`: integer ≥ 0, default 0. `wait`: integer from 0 to 25, default 0 (long wait).
- Returns the messages **from the other side** with index `> after`. If there are none and `wait > 0`, the
  relay waits up to `wait` seconds for a message to arrive or for a state change, then responds.
- If the channel is **already** `completed` or `closed`, nothing can arrive any more: the relay responds
  **immediately**, without waiting `wait`, with the state and the remaining messages of the other side (none
  in `closed`, the messages having been erased). Waiting applies only to `open` and `claimed` channels.

```json
{
  "state": "claimed",
  "expires_at": "2026-11-04T09:14:00.000Z",
  "messages": [
    { "index": 1, "side": "inviter", "body": "8iHUlI5aErkNAkEI07A1I0no-vY8i14pD_2uV9aKaH6c5MsS1mcdM5RFBKkvJ6--" }
  ]
}
```

`state` ∈ `open`, `claimed`, `completed`, `closed`. Messages remain readable in state `completed` until expiry
(the new device must be able to read the inviter's last message). The new device knows it is registered when
`state = "completed"`; it then opens a session (§3.3). A read started after this state change therefore
responds without waiting; re-reading a finished channel gives the same response, and remains subject to
`rl_pairing_poll` (§8.5).

#### `DELETE /v1/pairing/channels/{channel_id}` — B/P

Sets the channel to `closed` (messages erased). `204`. Clients MUST close the channel after a PAKE failure.

### 4.7 Space key rotation (and revocation)

The `KeyRotation` declaration of `core` contains: `space_id`, `new_epoch`, `issued_at`, the issuer's key, the
sorted list of **revoked** Ed25519 keys, and, for each remaining member, its Ed25519 key and the new space key
sealed for its X25519 key (104 bytes). The relay cannot open these seals.

#### `POST /v1/rotations` — B

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `rotation` | statement | yes | `KeyRotation` issued and signed by the caller |

```json
{
  "rotation": "yj92F0upJDZg6-40DNvgHGFB2-Mlmaqpj9tfvOpb6UYLAHSglMVEYIi63_RZoLJ7TEa7mq29rP7Bru3YJ8TLOmQM6HP1Lpn6DPdbQdYNA-Z0tAL9WaUnGxCXuCGcr3quIa0WVxtH7G9a9KibUvksvIO6Y8zR-1vZ"
}
```

Checks: §3.6. Order: decoding and `space_id` (`422`); `new_epoch` ≠ current + 1 →
`409 epoch_conflict` (extension `key_epoch` = current); issuer ≠ caller, or invalid signature (`422`);
`revoked` contains an unknown key or the caller's own key (`422`); set of recipients ≠ current members minus
`revoked` → `409 device_set_mismatch` (extension `expected_device_ids` = the `device_id` values expected as
recipients, sorted).

Exact replay (same bytes) of an already accepted rotation → `200` with the same response.

Effects (one transaction): revoke each member listed in `revoked` (§3.4, with `revoked_by` = the caller);
current epoch = `new_epoch`; `rotation_required = false`; store the declaration; add the `rotation` event, then
a `device_revoked` event for each newly revoked device. After the commit: close the WebSockets of the revoked
devices (`4003`); doorbell (without ringing) to the other members.

`201` response:

```json
{
  "seq": 57,
  "key_epoch": 2,
  "revoked_device_ids": ["4a3d212940f91ecf984ee64acd54afb8"]
}
```

`revoked_device_ids`: devices **newly** revoked by this request (possibly an empty list).
A rotation without revocation is permitted at any time.

#### `GET /v1/keys` — B

`200` response:

```json
{
  "key_epoch": 2,
  "rotations": [
    {
      "key_epoch": 2,
      "seq": 57,
      "rotation": "yj92F0upJDZg6-40DNvgHGFB2-Mlmaqpj9tfvOpb6UYLAHSglMVEYIi63_RZoLJ7TEa7mq29rP7Bru3YJ8TLOmQM6HP1Lpn6DPdbQdYNA-Z0tAL9WaUnGxCXuCGcr3quIa0WVxtH7G9a9KibUvksvIO6Y8zR-1vZ"
    }
  ]
}
```

`rotations`: **all** the declarations of the space (§8.2), by increasing `key_epoch`; the caller applies in order
those whose epoch exceeds its own (it is a recipient only of those posterior to its arrival).
Used for full catch-up (§7.5). Epoch 1 has no declaration: the initial key is created by the founding device
and transmitted by pairing.

### 4.8 Items

#### `POST /v1/items` — B

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `item_id` | uuid | yes | chosen by the client |
| `expires_at` | timestamp | yes | **retention deadline on the relay** (not the expiration wanted by the user, which stays inside the ciphertext). `> now` otherwise `422 already_expired`; `≤ now + max_ttl_secs` otherwise `422 validation_failed` |
| `key_epoch` | integer | yes | MUST equal the current epoch, otherwise `409 stale_key_epoch`. This check MUST be done **in the write transaction that assigns the `seq`** of the `item` event (not before): thus no item of epoch `e` receives a `seq` greater than that of the rotation `e → e+1`, which lets clients apply the epoch ordering rule (§7.3). Values `≥ 2^31` are reserved for keys outside the space (`format-v1.md` §5.8.2): never being the current epoch, they receive `409 stale_key_epoch` in v1 |
| `envelope` | b64 | yes | 1 to `max_envelope_bytes` decoded bytes, otherwise `413` |
| `blob_ids` | list of uuid | no (default `[]`) | ≤ `max_blobs_per_item`, no duplicates |

```json
{
  "item_id": "308339b6-1358-4bed-8328-23cc68b7f02b",
  "expires_at": "2026-11-11T09:05:00.000Z",
  "key_epoch": 1,
  "envelope": "8iHUlI5aErkNAkEI07A1I0no-vY8i14pD_2uV9aKaH6c5MsS1mcdM5RFBKkvJ6--",
  "blob_ids": ["aa29c935-8ce3-4ab6-a443-d13cc3c73e5f"]
}
```

**Computation of `expires_at` by the client (decision D1).** There is no default expiration: most items have no
user expiration (`expires_at` absent from the encrypted payload, `format-v1.md` §5.7). The client sends
`expires_at = min(payload expiration, estimated server time + max_ttl_secs − margin)`, with
`margin = min(60 s, max_ttl_secs / 2)` (`relay_expires_at_ms` in `crates/core`) and
`estimated server time` derived from `hello.server_time`. An item without expiration is therefore kept at most
`max_ttl_secs` (30 days) on the relay if it is not delivered, and the relay does not learn the real expiration
when it is absent or further away. The relay changes nothing in its checks.

Plaintext header check (§0.3): the envelope MUST be at least 4 bytes long. If its bytes 0–3 equal `1`
(format 1), it MUST be at least 24 bytes long, its bytes 4–7 MUST equal `key_epoch`, and its bytes 8–23 MUST
equal `item_id`; otherwise `422 validation_failed` (`field = "/envelope"`). For another `format`, the relay
checks nothing more and does not refuse the item (a client newer than the relay must be able to write).
The relay decrypts nothing. This check prevents a buggy client from publishing an envelope under the wrong
identifier; it is not a protection (clients verify for themselves, `format-v1.md` §5.6).

Blob checks: each blob MUST exist, belong to the caller (otherwise `404 not_found`), be complete
(`409 blob_incomplete`), and not be attached to another item nor to a history transfer (`409 blob_attached`);
sum of sizes ≤ `max_item_blob_bytes` (`413`). Quota: the envelope MUST fit within the quota (`507 relay_full`).

**Idempotence.** If `item_id` already exists (even if purged, as long as its row is kept, §8.2): same sender,
same `SHA-256(envelope)`, same `expires_at`, same `key_epoch`, same set of `blob_ids` → `200` with the original
response (plus `"purged": true` if the item has been purged since); otherwise → `409 item_id_conflict`.
Idempotence is checked **before all other business checks** (`key_epoch`, `expires_at`, blobs, quota): a late
replay of an accepted item succeeds, even if its blobs have since been attached or purged.

Effects (transaction): `created_at = now`, store the envelope, attach the blobs, compute the doorbell decision
(§6.1), add the `item` event. `201` response:

```json
{
  "item_id": "308339b6-1358-4bed-8328-23cc68b7f02b",
  "seq": 42,
  "created_at": "2026-11-04T09:05:00.120Z",
  "expires_at": "2026-11-11T09:05:00.000Z"
}
```

#### `GET /v1/items/{item_id}` — B

Visible if the item is not purged and the caller is the sender or a member with `joined_seq < seq` of the item;
otherwise `404`. `200` response:

```json
{
  "item_id": "308339b6-1358-4bed-8328-23cc68b7f02b",
  "seq": 42,
  "sender_device_id": "2e2ad3e872079a9cbce60917f275457f",
  "created_at": "2026-11-04T09:05:00.120Z",
  "expires_at": "2026-11-11T09:05:00.000Z",
  "key_epoch": 1,
  "envelope": "8iHUlI5aErkNAkEI07A1I0no-vY8i14pD_2uV9aKaH6c5MsS1mcdM5RFBKkvJ6--",
  "envelope_size": 48,
  "blobs": [{ "blob_id": "aa29c935-8ce3-4ab6-a443-d13cc3c73e5f", "size": 3145728 }],
  "receipts": [
    {
      "device_id": "4a3d212940f91ecf984ee64acd54afb8",
      "received_at": "2026-11-04T09:05:02.000Z",
      "read_at": "2026-11-04T09:06:10.000Z"
    }
  ]
}
```

`receipts`: one entry per device that has acknowledged receipt; `received_at` required, `read_at` optional.
`blobs` and `receipts` are always present (possibly empty).

#### `POST /v1/items/{item_id}/ack` — B (acknowledgement of receipt)

Request: `{ "release": boolean }` (optional, default `false`).

- First `ack` from this device for this item: `received_at = now`, `ack` event.
- `release: true`: the device no longer needs anything from the relay for this item (envelope and all wanted
  blobs retrieved, or item deleted locally). Counts for purging (§8.1). No event. A `release` cannot be undone.
- **Visibility** (`ack` and `read`): the item's row exists (purged or not, §8.2) and the caller is its sender or
  a member with `joined_seq < seq` of the item. Otherwise → `404` (unknown item, row already deleted, or a device
  that arrived after the item).
- Sender of the item: `200` without effect, without event.
- Purged item: `200` without effect, without event, with `"purged": true`.

`200` response: `{ "received_at": timestamp, "released": boolean }`, plus `"purged": true` if the item is
purged. For the sender, or for a purged item without a prior acknowledgement from this device, `received_at` is
absent and `released` is `false`.

**What each acknowledgement means.** `ack` (received) = the envelope is stored on the device (readable or not):
this is what makes "delivered to X" display on the sender's side. `release` = the relay may erase it.
A device that downloads files on demand (D20) sends `ack` straight away, and `release` only after the download,
the local deletion, or never (purging will happen at `expires_at`).

#### `POST /v1/items/{item_id}/read` — B

Body `{}`. Marks the item as read by this device; implies `ack` if that has not taken place.
First call: `read_at = now`; an `ack` event if the device had not yet acknowledged receipt, then a `read` event.
Replay: `200` without event.
Same visibility, sender and `purged` rules as `ack`.
`200` response: `{ "read_at": timestamp }`, plus `"purged": true` if the item is purged; `read_at` is absent for
the sender and for a purged item not previously read by this device.

#### `POST /v1/tombstones` — B (delete everywhere)

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `tombstone` | statement | yes | `Tombstone` of `core`, "everywhere" scope, signed by the caller (§3.6) |

```json
{ "tombstone": "Jyb1knsO6CRMy_eP2MD_-27ABlSqInP-92t7wXvHkZk-1x49bWFFS5vN8UP327-ZLG4kLwWlvNk5Pib8fyZv2AgxXQ" }
```

- The relay reads `item_id` and `deleted_at` from the declaration (there is no duplicate JSON field).
- Accepted for **any** `item_id`, known to the relay or not: an item already purged from the relay may still
  exist in local copies and must be erased there.
- First tombstone for this `item_id`: immediate purge of the item and its blobs if they exist (§8.1), `tombstone`
  event, `201` response `{ "seq": integer, "item_id": uuid }`.
- Tombstone already present for this `item_id` (from any device): `200` with the existing `seq`, without a new event.
- Invalid declaration (decoding, `space_id`, local scope, signer ≠ caller, signature) → `422`
  (`field = "/tombstone"`).

**"After first read" (secrets).** The relay does not know which items have this policy (it is inside the
ciphertext). It is **the first device that reveals or copies the secret** that immediately sends a signed
tombstone. Two simultaneous devices produce two requests; only one creates the event. The other devices, the
sender included, erase their copy on receipt of the tombstone. An offline device that reveals the secret queues
the tombstone (§7.7) and immediately hides the item locally.
Known limit: the relay can **infer** that an item was of this type by observing a tombstone arriving just after a `read`.

#### `POST /v1/attachment-tombstones` — B (delete ONE attachment everywhere)

Removal of one attachment from an item, without touching the item or the other attachments (`format-v1.md` §8.10).
Capability `attachment_tombstones` (§4.2): a relay that does not list it answers `404` to this route.

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `attachment_tombstone` | statement | yes | `AttachmentTombstone` of `core` (189 bytes), signed by the caller (§3.6) |

```json
{ "attachment_tombstone": "Jyb1knsO6CRMy_eP2MD_-27ABlSqInP-92t7wXvHkZk-1x49bWFFS5vN8UP327-ZLG4kLwWlvNk5Pib8fyZv2AgxXQ" }
```

- The relay reads `item_id`, `slot`, `blob_id` and `deleted_at` from the declaration (there is no duplicate JSON
  field). It stays blind: it neither reads nor verifies anything of the item's content.
- Invalid declaration (decoding, `space_id`, signer ≠ caller, signature, `deleted_at` out of bounds) →
  `422 validation_failed` (`field = "/attachment_tombstone"`). Same membership check as `POST /v1/tombstones`.
  The client signs `deleted_at` up to `2^53 − 1` (`format-v1.md` §8.10.1), but the relay bounds it to `wire::TS_MAX_MS`
  (`9999-12-31T23:59:59.999Z`, like all its timestamps): beyond that, `422`. No practical effect (it is the device's clock).
- `(item_id, slot)` already registered (from any device): `200` `{ "seq": integer, "item_id": uuid,
  "slot": integer }` with the existing `seq`, without a new event.
- Otherwise: the relay records the declaration, emits the `attachment_tombstone` event (§4.10) and responds `201`
  with the same shape. There is no special case depending on the item's state: it is accepted for an unknown,
  already purged or already tombstoned `item_id` (clients ignore the event if the item is tombstoned), because
  local copies may survive the relay's copy.
- **Deletion of the blob.** The relay deletes the blob `blob_id` **only** if `blobs.item_id = item_id` AND
  `blobs.pos = slot` AND `blob_id` matches (file erased after validation, quota usage freed). A blob attached to
  another item or to another position, or another blob at this position of this item → `422`
  (`field = "/attachment_tombstone"`), nothing is recorded. Absent blob (item purged, never sent): accepted, nothing
  to delete, and the event is still emitted (it is the only way to propagate the removal to devices that already
  have the file).
- `purge_item` (§8.1) **does not touch** these records: items are purged shortly after their delivery, and a replay
  after the purge must not become a second event. Retention: like tombstones (§8.2).
- Rate limiter: `rl_device_tombstones` (§8.5), shared with `POST /v1/tombstones`.
- The `item` event in the log keeps the old `blobs` list: it is not rewritten (limit, `format-v1.md` §12).

#### `POST /v1/item-states` — B (pin, unpin, archive, unarchive, encrypted state)

Propagates an **item-state event** (`format-v1.md` §8.8): today the pin (§8.8) and the archive (§8.9),
indistinguishable to the relay (same size, same shape). The relay stores it in the log like a mailbox: it does
not know which item is targeted nor what changes, and **nothing in its purge depends on these events** (§8.1).

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `event_id` | uuid | yes | chosen by the client (random UUID v4, **never** the `item_id`); idempotency key |
| `key_epoch` | integer | yes | MUST equal the current epoch, otherwise `409 stale_key_epoch`; the check is done **in the transaction that assigns the `seq`**, as for `POST /v1/items` (epoch ordering rule). A reserved value (`≥ 2^31`) thus receives `409 stale_key_epoch` |
| `envelope` | b64 | yes | state envelope produced by `core` (`seal_item_state`), 4 to `max_item_state_bytes` decoded bytes, otherwise `413` |

```json
{
  "event_id": "e0000000-0000-4000-8000-000000000001",
  "key_epoch": 1,
  "envelope": "AAAAAQAAAAHgAAAAAABAAIAAAAAAAAABMDEyMzQ1Njc4OTo7PD0-P0BBQkNERUZH…"
}
```

Plaintext header check: if bytes 0–3 equal `1`, the envelope MUST be **exactly** `48 + 256·k + 16` bytes with
`1 ≤ k ≤ 8` (320 to 2,112 bytes; a pin is 320), its bytes 4–7 MUST equal `key_epoch`, and its bytes 8–23 MUST
equal `event_id`; otherwise `422 validation_failed` (`field = "/envelope"`). The relay performs this length check
with `tomi_core::read_item_state_header` (`malformed_envelope` → `422`; `unsupported_format` → unknown format, see
below), the same rule as the clients (`format-v1.md` §8.8.2, step 2): defence in depth, and a format 1 envelope of
invalid length, which all clients would reject, occupies neither the log nor the quota. For another `format`,
there is no check beyond `max_item_state_bytes`. The relay decrypts nothing and verifies no signature (it cannot:
the signed declaration is inside the ciphertext).

**Idempotence** (before the other business checks): a known `event_id` → same sender, same `SHA-256(envelope)`
and same `key_epoch` → `200` with the original response, without a new event; otherwise `409 event_id_conflict`.
After `stale_key_epoch`, nothing is recorded: the client re-seals the same declaration under the new key and
retries with the **same** `event_id`. After `event_id_conflict`, the client re-emits the same declaration under a
**new** `event_id` (§7.7): a decision is never abandoned.

Quota: the envelope counts towards `used_bytes` as long as the event is in the log (§8.4); otherwise `507 relay_full`.

Effects (transaction): add the `item_state` event. After the commit: `NORMAL` doorbell to the other members
(FCM and `wake`), never `HIGH`. `201` response:

```json
{ "event_id": "e0000000-0000-4000-8000-000000000001", "seq": 1793755800321 }
```

### 4.9 Blobs

A blob is sent **before** the item that references it, in sequential parts, each ≤ `max_upload_part_bytes`.
The relay writes each part directly to disk (never the whole blob in memory).

#### `POST /v1/blobs` — B (creation)

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `blob_id` | uuid | yes | chosen by the client |
| `size` | integer | yes | 1 to `max_blob_bytes`, otherwise `413` |

The size is **reserved** in the quota from creation (`507 relay_full` if it does not fit).
Same `blob_id`, same caller, same `size` → `200` with the current state; otherwise `409 blob_id_conflict`.
`201` (or `200`) response: BlobStatus.

BlobStatus:

```json
{
  "blob_id": "aa29c935-8ce3-4ab6-a443-d13cc3c73e5f",
  "size": 3145728,
  "upload_offset": 1048576,
  "complete": false,
  "attached": false,
  "expires_at": "2026-11-05T09:04:00.000Z"
}
```

`expires_at` (present only if `attached = false`): deadline for collecting orphan blobs
(`blob_orphan_ttl_secs` after the last part received, default 24 h).

#### `GET /v1/blobs/{blob_id}/status` — B

Owner only (otherwise `404`). `200` response: BlobStatus. Used to resume an upload.

#### `PATCH /v1/blobs/{blob_id}` — B (sending a part)

Required headers: `Content-Type: application/octet-stream` (`415`), `Content-Length` (`411 length_required`;
`Transfer-Encoding: chunked` refused), `Upload-Offset: <integer>`.

- Owner only (otherwise `404`). Blob complete or attached → `409 blob_attached`.
- `Upload-Offset` ≠ bytes already received → `409 upload_offset_mismatch` (extension `upload_offset`).
- `Content-Length` > `max_upload_part_bytes`, or `Upload-Offset + Content-Length > size` → `413`.
- `Content-Length = 0` → `422`.
- **Atomic part**: if the connection drops before the end of the body, the relay rolls the file back to
  `Upload-Offset`; progress advances only by whole parts.
- When `upload_offset` reaches `size`, the relay flushes its buffers to disk (`fsync`) and marks the blob complete.

`204` response with an `Upload-Offset: <new offset>` header.

#### `GET /v1/blobs/{blob_id}` and `HEAD` — B (download)

Visible to the owner; to any member with `joined_seq < seq` of the attaching item, if that item is not purged;
and, for a blob attached to a history transfer (§4.13) that is neither expired nor closed, to the **only
recipient** of that transfer (`joined_seq` does not apply). Otherwise `404` (including for a purged blob, and for
any member other than a recipient). Blob not complete → `409 blob_incomplete`.

- `Range`: a single range `bytes=a-b`, `bytes=a-` or `bytes=-n` → `206` with `Content-Range`.
  A multiple, malformed or out-of-size range → `416` with `Content-Range: bytes */<size>`.
- Headers: `Content-Type: application/octet-stream`, `Content-Length`, `Accept-Ranges: bytes`,
  `ETag: "<blob_id>"` (the content of a blob is immutable), `Cache-Control: no-store`.
- `If-Range` with the ETag: honoured.

#### `DELETE /v1/blobs/{blob_id}` — B

Abandons an upload. Owner only (`403 forbidden` for another member who can see it, `404` otherwise).
Attached to an item or to a transfer → `409 blob_attached` (an attached blob disappears together with its
item or transfer). `204`, quota freed.

The blobs of a history transfer (index parts and re-encrypted files) are created and sent exactly like those of
an item (`POST /v1/blobs`, `PATCH`), then attached by `POST /v1/transfers`.

### 4.10 Log — `GET /v1/events?after=<seq>&limit=<n>` — B

- `after`: seq, required. `limit`: 1 to 500, default 100.
- If `after < joined_seq` of the caller, it is replaced by `joined_seq` (a new device does not see the history in
  the log, `PROJECT.md` §7; it may receive it through a sealed transfer, §4.13).
- `after > head_seq` → `409 cursor_ahead` (relay restored from an older backup; see §7.5).
- `after < retention_floor_seq` (and `≥ joined_seq`) → `410 cursor_expired` (§8.2).
- Page: events with `seq > after`, increasing, at most `limit`. The relay stops the page **before** an `item`
  event that would push the sum of the `envelope_size` values of the page above 2,097,152 bytes, unless the page
  is still empty (a page always contains at least one event if one exists).
- `head_seq`, the page and `has_more` are read in **the same read transaction**.
- `next_after` = `seq` of the last event if `has_more`, otherwise `head_seq`.
- The relay records `after` as the device's indicative cursor (displayed by `admin devices`); it does not use it
  for purging.

```json
{
  "events": [
    {
      "seq": 42,
      "type": "item",
      "at": "2026-11-04T09:05:00.120Z",
      "data": {
        "item_id": "308339b6-1358-4bed-8328-23cc68b7f02b",
        "sender_device_id": "2e2ad3e872079a9cbce60917f275457f",
        "created_at": "2026-11-04T09:05:00.120Z",
        "expires_at": "2026-11-11T09:05:00.000Z",
        "key_epoch": 1,
        "envelope": "8iHUlI5aErkNAkEI07A1I0no-vY8i14pD_2uV9aKaH6c5MsS1mcdM5RFBKkvJ6--",
        "envelope_size": 48,
        "blobs": [{ "blob_id": "aa29c935-8ce3-4ab6-a443-d13cc3c73e5f", "size": 3145728 }],
        "ring": true
      }
    },
    {
      "seq": 43,
      "type": "ack",
      "at": "2026-11-04T09:05:02.000Z",
      "data": {
        "item_id": "308339b6-1358-4bed-8328-23cc68b7f02b",
        "device_id": "4a3d212940f91ecf984ee64acd54afb8",
        "received_at": "2026-11-04T09:05:02.000Z"
      }
    }
  ],
  "head_seq": 43,
  "has_more": false,
  "next_after": 43
}
```

Each event: `seq` (seq), `type` (string), `at` (timestamp of addition to the log), `data` (object).
Clients MUST ignore an unknown `type` (while advancing their cursor).

#### Event types (`data`, all fields required unless stated)

| `type` | `data` | Produced by | FCM doorbell |
|--------|--------|-------------|--------------|
| `item` | `item_id`, `sender_device_id`, `created_at`, `expires_at`, `key_epoch`, `envelope` (b64), `envelope_size`, `blobs` `[{blob_id, size}]`, `ring` (boolean **specific to the device that reads it**; always `false` for the sender) | `POST /v1/items` | yes, `HIGH` if `ring`, otherwise `NORMAL` |
| `ack` | `item_id`, `device_id`, `received_at` | first `ack` or `read` | no |
| `read` | `item_id`, `device_id`, `read_at` | first `read` | yes, `NORMAL` |
| `tombstone` | `item_id` (uuid), `device_id` (signer), `deleted_at`, `tombstone` (statement, the received declaration, to be verified by the client) | `POST /v1/tombstones` | yes, `NORMAL` |
| `attachment_tombstone` | `item_id` (uuid), `slot` (integer), `blob_id` (uuid), `device_id` (signer), `deleted_at`, `statement` (the received declaration, to be verified by the client; the other fields are only a copy by the relay) | `POST /v1/attachment-tombstones` | yes, `NORMAL` |
| `item_state` | `event_id` (uuid), `device_id` (sender), `key_epoch`, `envelope` (b64, encrypted state envelope, to be opened and verified by the client), `envelope_size` | `POST /v1/item-states` | yes, `NORMAL` (not for the sender) |
| `rotation` | `key_epoch` (new epoch), `device_id` (issuer), `created_at` (`issued_at` of the declaration), `revoked_device_ids` (list of device_id, possibly empty), `rotation` (complete statement, with the seals of **all** the recipients) | `POST /v1/rotations` | yes, `NORMAL` |
| `device_added` | `device`: DeviceRecord reduced to the fields `device_id`, `name`, `platform`, `sign_pk`, `kx_pk`, `certificate`, `issuer_device_id`, `joined_seq`, `registered_at` | space creation, `POST /v1/devices` | yes, `NORMAL` |
| `device_updated` | `device_id`, `name` | `PATCH /v1/devices/{id}` | no |
| `device_revoked` | `device_id`, `revoked_at`, `via` (`"rotation"`, `"self"` or `"admin"`), `revoked_by` (device_id, **or `null`** for `"admin"`), `rotation_seq` (seq of the `rotation` event, present only for `"rotation"`) | §3.4 | yes, `NORMAL` |
| `history_transfer` | `transfer_id`, `from_device_id`, `to_device_id`, `total_bytes` (sum of the blobs), `expires_at` | `POST /v1/transfers` (first deposit) | yes, `NORMAL`, **recipient only** |
| `history_transfer_closed` | `transfer_id`, `by_device_id` (**or `null`** if the relay closes it itself: deadline, revocation), `outcome` (string; known values `imported`, `declined`, `failed`, `cancelled`, `expired`, `revoked`; unknown value tolerated) | `DELETE /v1/transfers/{id}`, purge (§8.1) | no |

The `history_transfer` and `history_transfer_closed` events are in the common log, like the others; a member
who is neither the sender nor the recipient ignores them (and can neither read the transfer nor its blobs,
§4.13). The `joined_seq` filter applies normally: a transfer is always posterior to its recipient's arrival.
An old client ignores them (unknown type). The history itself never goes through the log.

`revoked_by` and `history_transfer_closed.by_device_id` are the only protocol fields that may be `null` in output.
Only a revocation with `via: "rotation"` is proven by a signed declaration (that of the `rotation_seq` event).
The `"self"` and `"admin"` revocations are not signed: clients apply them (at worst a denial of service) and
display them as such.

An `item` event disappears from the log when the item is purged (§8.2): a device that catches up after the relay
deadline sees nothing, which corresponds to "not delivered" on the sender's side (§7.3).

### 4.11 WebSocket — see §5.

### 4.12 Health and distribution

These routes are not under `/v1`, without auth, without space check, **read-only** (GET/HEAD).

#### `GET /healthz`

`200` `{"status":"ok"}` if **each** database (`routing.db` and that of each space) answers a trivial query in less
than 2 s in total, otherwise `503` `{"status":"unavailable"}`. No other information. Excluded from access logs at `INFO` level.

#### Static files

Root: `<data_dir>/public/`, deployed by the deployment (`infra` area). The relay:
- serves `/updates/<path>` from `public/updates/`, `/.well-known/assetlinks.json` from
  `public/.well-known/assetlinks.json`, and `/l/<anything>` always returning `public/l/index.html`;
- refuses any path containing `..`, a null byte, a backslash, or a segment starting with `.`
  (except `/.well-known/assetlinks.json`), and does not follow symbolic links leading out of `public/` → `404`;
- never lists a directory (`404`);
- types: `.json` → `application/json`, `.apk` → `application/vnd.android.package-archive`, `.html` →
  `text/html; charset=utf-8`, `.sig` → `text/plain`, others → `application/octet-stream`;
- sends `ETag` and `Last-Modified`, honors `If-None-Match` (`304`) and `Range` (one range);
- `Cache-Control: no-cache` for `latest.json` and `assetlinks.json`, `public, max-age=31536000, immutable` for the
  rest of `/updates/`;
- for `/l/*`: `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; img-src 'self'`,
  `Referrer-Policy: no-referrer`. The `#<secret>` fragment of pairing links is never sent to the server by the
  browser; the page runs no script.

Expected tree:

```
public/
  .well-known/assetlinks.json
  l/index.html                      page « Installer Tomi » (French UI label, kept), link to the APK
  updates/
    windows/latest.json             Tauri updater manifest (format defined by Tauri)
    windows/<installers and .sig>
    android/latest.json             manifest below
    android/tomi-<version>.apk
    cli/latest.json                 same format as Android, "url" field pointing to the binary
```

Manifest `updates/android/latest.json` (and `cli`), defined here to align the android and infra areas:

```json
{
  "version": "0.2.0",
  "version_code": 2,
  "url": "https://relay.example.org/updates/android/tomi-0.2.0.apk",
  "sha256": "b1946ac92492d2347c6235b4d2611184b1946ac92492d2347c6235b4d2611184",
  "size": 18350080,
  "released_at": "2026-11-04T08:00:00.000Z"
}
```

`version` (SemVer string), `version_code` (integer, Android only, absent for `cli`), `url`, `sha256`
(64 lowercase hexadecimal characters), `size` (integer), `released_at` (timestamp): required; `notes` (string):
optional. The relay neither reads nor validates these files.
These files are public: anyone who knows the URL can download the application, which is useless without a space.

### 4.13 History transfers (decision D2)

A member (the **sender**) deposits on the relay a sealed copy of **all** of its thread for **one** other member
(the **recipient**): at pairing, or later from the Devices screen. Format: `format-v1.md` §7.4 and §8.7. The
transfer carries everything, **secrets and files included**: the API has **no** partial-inclusion field (no
"secrets" option, no "files" option). The user's only choice is whether to transfer or not. The relay sees only
opaque blobs, their sizes, the two devices and the signed manifest; it holds no key.

Procedure: the sender syncs until `cursor ≥ joined_seq(recipient)`, takes the snapshot `as_of_seq = cursor`,
writes the index, encrypts the parts and re-encrypts the files, creates and sends the blobs (§4.9, resumable),
then calls `POST /v1/transfers`. The recipient receives the `history_transfer` event, reads the transfer,
downloads and imports it (§7.8), then calls `DELETE`.

#### `POST /v1/transfers` — B (sender)

| Field | Type | Req. | Rule |
|-------|------|--------|-------|
| `transfer_id` | uuid | yes | chosen by the sender; equal to that of the manifest |
| `manifest` | statement | yes | `HistoryTransfer` signed by the caller (`format-v1.md` §8.7.1) |
| `blob_ids` | list of uuid | yes | index parts **and** re-encrypted files; contains all the parts of the manifest; 1 to `max_blobs_per_transfer` elements; no duplicates |

No other field has any meaning; in particular there is no flag to include secrets or files (an unknown field is
ignored, §2.2).

```json
{
  "transfer_id": "c1a7e0d2-5b3f-4e8a-9c61-2d4f6a8b0c1e",
  "manifest": "dG9taS92MS9zaWcvaGlzdG9yeS10cmFuc2Zlcg…",
  "blob_ids": ["b0000000-0000-4000-8000-000000000000", "b1000000-0000-4000-8000-000000000001", "f0000000-0000-4000-8000-000000000000"]
}
```

Checks, in order:

1. **Idempotence**: `transfer_id` already known (open or closed, as long as its row exists, §8.3) → same
   manifest bytes, same caller and same set of `blob_ids` → `200` with the original response, without a new event;
   otherwise `409 transfer_id_conflict`.
2. **Manifest** (§3.6): decoding, `space_id`, `transfer_id` equal to the body's, sender = caller, signature,
   recipient = non-revoked member different from the caller,
   `joined_seq(recipient) ≤ as_of_seq ≤ head_seq`, parts of the manifest ⊆ `blob_ids` →
   otherwise `422 validation_failed`, `field = "/manifest"`.
3. **Blobs**: each exists and belongs to the caller (`404`), is complete (`409 blob_incomplete`), and is attached
   to no item nor transfer (`409 blob_attached`); sum of sizes ≤ `max_transfer_bytes` (`413`, extension `limit`).
4. **Limit**: open transfers to this recipient < `transfer_max_open` → otherwise `409 transfer_limit`.
5. **Quota**: the blobs are already counted from their creation (§8.4); the transfer's row MUST fit (`507 relay_full`).

Effects (one transaction): store the manifest, attach the blobs to the transfer,
`expires_at = now + transfer_ttl_secs`, add the `history_transfer` event. After the commit: doorbell to the
recipient only (FCM `NORMAL`, `wake` WebSocket). `201` response:

```json
{ "transfer_id": "c1a7e0d2-5b3f-4e8a-9c61-2d4f6a8b0c1e", "seq": 1793754000456, "expires_at": "2026-11-11T09:00:00.000Z" }
```

#### `GET /v1/transfers/{transfer_id}` — B (sender or recipient)

Visible to the sender and the recipient of an open transfer; `404` for any other device, for an unknown transfer,
or for one that is closed or expired. `200` response:

```json
{
  "transfer_id": "c1a7e0d2-5b3f-4e8a-9c61-2d4f6a8b0c1e",
  "from_device_id": "e983270f3ea7baa8688d15c08d447559",
  "to_device_id": "d2daf28457ef3d14c9c58d6f488e1844",
  "manifest": "dG9taS92MS9zaWcvaGlzdG9yeS10cmFuc2Zlcg…",
  "blobs": [{ "blob_id": "b0000000-0000-4000-8000-000000000000", "size": 69124 }],
  "created_at": "2026-11-04T09:00:00.000Z",
  "expires_at": "2026-11-11T09:00:00.000Z",
  "seq": 1793754000456
}
```

`blobs`: all the blobs of the transfer (parts and files), in the order of `blob_ids` at creation. The recipient
verifies the manifest with its own trust store (`TrustStore::verify_history_transfer`) before downloading anything;
the sizes are not authenticated.

#### `DELETE /v1/transfers/{transfer_id}` — B (sender: cancellation; recipient: end)

Optional body `{ "outcome": "imported" | "declined" | "failed" | "cancelled" }` (informational; default
`cancelled` for the sender, `imported` for the recipient; an unknown value is stored as is).
Sender or recipient only (otherwise `404`). Effects: transfer closed, blobs deleted (files after the commit) and
quota freed, `history_transfer_closed` event; the row is kept for `event_retention_days` (45 days) for idempotence.
`204` response. Idempotent: a transfer already closed or expired → `204` without event.

### 4.14 What the transfer does not change

- The log and the `joined_seq` filter remain as they are: history goes through the transfer, never through the log.
- No `ack`, `read` or `release` is sent for an imported item: the relay would answer `404`.
- An `after_first_read` item imported and revealed on the recipient emits a tombstone, accepted for any
  `item_id` (§4.8).

## 5. WebSocket `/v1/ws`: the doorbell

The WebSocket carries **no data**: only wake-ups. It serves the Windows, CLI (`watch` mode) and Android clients
**when the app is in the foreground**. Android in the background uses FCM (§6).

### 5.1 Opening and authentication

1. The client opens `wss://<host>/v1/ws` (HTTP/1.1 upgrade, RFC 6455). **No token in the URL**, no subprotocol
   required. The relay accepts the upgrade without prior authentication (IP checks §8.5).
2. Within **10 s**, the client sends a single text message:
   ```json
   { "type": "auth", "token": "tms1.10o-uHd2mmD8K1MEX1H7e9MmKldBQ2kaLh_Wyjy4DGY" }
   ```
3. The relay validates the token as for a REST request, then responds:
   ```json
   { "type": "ready", "head_seq": 43, "session_expires_at": "2026-11-04T10:00:02.000Z" }
   ```
   On failure: close with the code of §5.4 (no JSON error message).
4. The client then starts a REST catch-up (§7.2): `ready` never replaces reading the log.

Text messages only, UTF-8 JSON, ≤ 4096 bytes. A binary or too-large message → close `4000`.
After `ready`, the client's text messages are **ignored** (reserved for future versions).

### 5.2 Server → client messages

| `type` | Fields | Meaning |
|--------|--------|------|
| `ready` | `head_seq` (seq), `session_expires_at` (timestamp) | Authentication succeeded |
| `wake` | `seq` (seq), `ring` (boolean) | New events, up to and including the indicated `seq`, are available; `ring` says whether **this** device must ring |

```json
{ "type": "wake", "seq": 44, "ring": true }
```

Sending rules:
- A `wake` is sent to each connection of a device concerned by a new event (see the "Doorbell" column of §4.10:
  all types wake the device over WebSocket; `ack` only the sender of the item; never the author of the event).
- `ring` takes the decision of §6.1 for an `item` event, and `false` for any other event.
- The relay MAY group the wake-ups of one connection that occur within a 250 ms window into a single `wake`
  carrying the largest `seq` and `ring` = logical OR of the grouped `ring` values.
- `ring` is only a convenience hint: the reference decision is the `ring` field of the `item` event read over REST.

Clients MUST ignore an unknown `type`.

### 5.3 Heartbeat

- The relay sends a WebSocket **ping** frame every 25 s. Without a **pong** within the following 20 s, it
  considers the connection dead and closes the TCP connection without a closing handshake.
- The client considers the connection dead if it receives **no frame** (ping, message) for 60 s, and reconnects (§5.5).
- The client MAY send its own pings; the relay answers them.
- When the session expires, the relay closes with `4002`. The client opens a new session, then reconnects.

### 5.4 Close codes

| Code | Reason (close text) | Client reaction |
|------|-----------------------------|--------------------|
| `1001` | `going_away`: relay shutting down | reconnection, initial "restart" delay (§5.5) |
| `1012` | `restart`: relay restarting | same |
| `4000` | `protocol_error` | reconnection with normal delay; report a bug |
| `4001` | `unauthorized`: token invalid or absent within 10 s | new session, then reconnection |
| `4002` | `session_expired` | new session, then immediate reconnection |
| `4003` | `revoked` | **no reconnection**; display "This device has been removed from the space" |
| `4004` | `space_reset`: `admin reset` of **this** space, or its deletion by its last device (§4.4.1), sent only **after** the successful deletion (connections of other spaces are not affected) | **not a proof** (§7.9): do not reconnect straight away; open a new session (§7.2). Session opened → the space exists: reconnect with normal delay. `401` twice or `409 space_not_initialized` → state NOT_FOUND, stop the doorbell. `403 revoked` → REVOKED, stop |
| `4008` | `replaced`: more than 3 connections for this device, the oldest is closed | do not reconnect on this connection (another one is open) |
| `1013` | `try_again_later`: the session's space is "pending" (deletion in progress or unknown state, §4.4.1) | reconnection with normal delay; this is neither "deleted" nor "unknown". An earlier client, which does not know this code, already applies the normal delay |
| other / network cut | | reconnection with normal delay |

### 5.5 Reconnection with random exponential delay

```
base = 1 s ; cap = 60 s ; attempt = 0
on close:
  if code ∈ {4003, 4008}       : stop
  if code = 4004               : new session; member → normal delay; otherwise stop (§5.4)
  if code ∈ {4001, 4002}       : renew the session; delay = 0 for the first attempt
  if code ∈ {1001, 1012}       : delay = uniform(1 s, 15 s)        // spreads the rush after a restart
  else                         : delay = uniform(0, min(cap, base × 2^attempt))   // "full jitter"
  wait delay ; attempt += 1 ; reconnect
on "ready":
  start the REST catch-up
  if the connection holds for 60 s: attempt = 0
```

The client MUST also reconnect without waiting when the system signals that the network is back
(keeping a jitter of 0 to 1 s), and MUST NOT attempt more than one connection at a time.
On the relay side: at most 30 new WebSocket connections per minute and per IP (§8.5).

## 6. Push, presence and the ring decision

### 6.1 Who rings

When an `item` event sent by `S` is added, the relay computes, in the same transaction:

```
recipients = members \ {S}
present    = { d ∈ recipients : d.present_until > now }
for each d ∈ recipients:
  ring(d) = (present is empty) OR (d ∈ present)
```

`ring(d)` is **stored** with the item and returned as is in the `ring` field of the `item` event read by `d`
(and in the WebSocket `wake`). The sender's presence does not count.
Example: I send from the phone; the desktop had input 30 s ago, the laptop is asleep → only the desktop rings;
the laptop receives the item silently (an "unread" dot).

The client also applies its own rules (no sound for an item read in the meantime, nor for an item older than
15 minutes discovered during a catch-up, grouping of bursts, §7.3).

### 6.2 Presence

A device is **present** when the user is using it:
- **Windows**: keyboard or mouse input within the last 2 minutes and session not locked.
- **Android**: screen on and unlocked **while the application runs**. In practice, without a permanent service,
  the app only knows this reliably in the foreground (annex B.4, open point 1).
- **CLI**: never present (does not send presence).

Protocol: `PUT /v1/me/presence {"present": true}` on the transition to present, then every 60 s while the device
remains so (TTL 120 s); `{"present": false}` on the reverse transition when possible (locking, sleep). If the
device disappears without notice, presence expires on its own after 120 s.
At most 12 requests per minute (§8.5). Presence is never sent to FCM.

### 6.3 FCM (Android)

Internal interface of the relay (not normative in its form, normative in its behaviour):

```rust
enum Urgency { High, Normal }
#[async_trait]
trait PushSender: Send + Sync {
    /// Sends one content-free wake-up to `target`. Never logs the token.
    async fn wake(&self, target: &PushTarget, urgency: Urgency) -> Result<(), PushError>;
}
enum PushError { Unregistered, AuthFailure, InvalidArgument, Transient { retry_after: Option<Duration> } }
```

One `FcmSender` implementation in v1; `UnifiedPushSender` will be added without changing the caller
(`provider = "unifiedpush"`, token = endpoint URL).

When to send:
- only to devices that have a push token **and no open authenticated WebSocket connection**;
- for the events marked "yes" in the FCM doorbell column of §4.10, never to the author of the event;
- **anti-burst**: at most one send per device every 2 s; wake-ups that occur during this delay are merged into a
  single send (urgency = the highest) at the end of the delay.

Exact message (FCM HTTP v1 API, `POST https://fcm.googleapis.com/v1/projects/<project_id>/messages:send`):

```json
{
  "message": {
    "token": "<device FCM token>",
    "data": { "w": "1" },
    "android": {
      "priority": "HIGH",
      "collapse_key": "tomi",
      "ttl": "86400s"
    }
  }
}
```

- `data` is **constant** (`{"w": "1"}`, identical for all messages): no identifier, no type, no `seq`, no counter.
  No `notification` block.
- `priority`: `"HIGH"` when the urgency is high (an item with `ring = true` for this device), otherwise `"NORMAL"`.
  Android penalizes high-priority messages that do not display a notification; so it is used only for an item that
  must ring. Google learns this bit (§1.3).
- `collapse_key` constant: FCM keeps only the last pending message for an offline device.
- FCM responses: `UNREGISTERED` (404), or `INVALID_ARGUMENT` (400) whose detail designates the `message.token`
  field → delete the device's token (`push = false`); any other `INVALID_ARGUMENT` is a relay bug: log `ERROR`,
  token kept; 401/403 → `AuthFailure`: health status `auth_error` (§8.7), log `ERROR` at most once an hour;
  429/5xx → retry after `Retry-After` or an exponential delay (1 s → 60 s, 3 attempts), then give up
  (cursor catch-up covers the loss).
- Credentials: service account JSON file (`fcm_service_account_file`, mode `0600`). OAuth token
  (scope `https://www.googleapis.com/auth/firebase.messaging`) obtained at startup, then renewed before
  expiry. Failure at startup → `auth_error`; the relay keeps running without FCM.
- Missing key → FCM disabled (`disabled`), no sending.

Note: since late 2026, Firebase offers, in early access, `android_v2` (`remote_notification` /
`background_sync`). Tomi v1 stays on `android` + `priority`; any eventual migration changes only `FcmSender`.
See [the Firebase migration page](https://firebase.google.com/docs/cloud-messaging/send/v2-api).

## 7. Client-side synchronization algorithm (normative for `crates/core`)

### 7.1 Persistent state of the device

```
device_id, space_id, sign_sk, kx_sk          // protected keys (Keystore / DPAPI)
lsk        : random local key of the device (Keystore / DPAPI), never derived nor transmitted
space_keys : SpaceKeyRing : key_epoch -> key, + seq and application date of the rotation that replaced it;
             an old key is erased max_ttl_secs + 1 day after that date (format-v1.md §8.3)
cursor     : seq                             // 0 at start; the relay brings it back to joined_seq (§4.10)
devices    : verified local list (certificates, revocations)
items      : readable items re-sealed under lsk (format-v1.md §7.2); raw envelope kept
             only for unreadable items (UNREADABLE_UPDATE_REQUIRED, UNDECRYPTABLE, UNREADABLE_SUSPECT_EPOCH, KEY_PENDING)
tombstoned : set of item_id deleted everywhere (kept 45 days)
pins       : PinBook: item_id -> (pinned, at_ms), independent of items (format-v1.md §8.8.3)
archives   : ArchiveBook: item_id -> (archived, at_ms), independent of items and pins (format-v1.md §8.9.3)
pending_states : raw state envelopes kept (KeyPending, UpdateRequired), retried like items
outbox     : persistent queue of outgoing operations (§7.7)
transfers  : persistent history-transfer tasks, sending and receiving (§7.8)
session    : token + expiry (in memory, or protected storage)
```

> **Note on state labels.** State labels are written in English and mirror the core's enums (`UnreadableReason`, `Blocked`, `SpaceStatus`).

Any application of an event and the advancement of the cursor take place **in the same local transaction**.
A crash between two events therefore replays at most the event in progress, which is idempotent.

### 7.2 Synchronization loop

```
function ensure_session():
  if the session is still valid for more than 5 min: return
  c = POST /v1/auth/challenge { device_id }
  sig = ed25519_sign(sign_sk, "tomi/v1/sig/relay-auth" || 0x00 || space_id || sign_pk || c.challenge)
                                           // 103 bytes, §3.6; core: sign_relay_auth(space_id, challenge)
  r = POST /v1/auth/session { device_id, challenge: c.challenge, signature: sig, client: { format_min, format_max, app_version, platform } }
  depending on the error:
    update_required       → state BLOCKED_UPDATE_REQUIRED (show "Mise à jour requise", update_url link); stop the network except /updates
    format_unsupported    → state BLOCKED_FORMAT_UNSUPPORTED (show which devices limit it: limited_by)
    revoked               → state REVOKED (show "Cet appareil a été retiré de l'espace"); kept
    space_not_initialized → state NOT_FOUND       // the relay hosts no space (§1.4); NOT kept
    unauthorized          → retry once (new challenge); second failure → state NOT_FOUND; NOT kept
    unavailable (503)     → transient error (space "pending", §4.4.1): retry later
  session = r ; space_state = r.space
  if space_state.rotation_required: rotation due (§7.6, automatic)

// NOT_FOUND (UI label: « Ton serveur Tomi ne trouve plus ton espace ») replaces the former names UNRECOGNIZED and
// SPACE_DELETED. It is NOT a proof of deletion (§1.4 rule 4, §7.9): it is never written to disk (a
// "space_deleted" value left in meta.blocked by an earlier version is ignored and removed on opening), and it is
// re-checked at each network call, at most once a minute. Only REVOKED is kept; it too is re-checked before any
// erasure (§7.9).

function sync():                           // only one execution at a time
  if sync is running: mark dirty; return
  repeat:
    dirty = false
    ensure_session()
    loop:
      p = GET /v1/events?after=cursor&limit=200
        on cursor_expired or cursor_ahead: full_resync(); continue
        on session_expired: ensure_session(); continue
      for ev in p.events:
        local transaction { apply(ev) ; cursor = ev.seq }
      local transaction { cursor = p.next_after }
      if not p.has_more: exit
    drain_outbox()
    send pending acknowledgements (§7.3)
  while dirty

triggers of sync():
  app start; WebSocket ready; WebSocket wake (if wake.seq > cursor); FCM message;
  network back; return to the foreground; timer (every 15 min in the foreground, periodic WorkManager on Android);
  addition to the outbox
```

### 7.3 Applying events

```
apply(ev):
  depending on ev.type:
  "item" :
    if items contains ev.data.item_id or tombstoned contains ev.data.item_id: return   // idempotent
    store the raw envelope, the metadata and the list of blobs
    if ev.data.sender_device_id == me: mark the outbox item "sent"; return
    decode: space_keys.open_received(envelope, item_id, ev.seq) (format-v1.md §9.4):
      unknown format         → state UNREADABLE_UPDATE_REQUIRED ("Contenu non lisible, mettez l'app à jour" UI label), never deleted
      epoch e already replaced by a rotation with seq < ev.seq (epoch ordering rule)
                             → state UNREADABLE_SUSPECT_EPOCH ("Contenu suspect, non affiché" UI label), never deleted,
                               never retried
      unknown epoch key, epoch > current
                             → state KEY_PENDING (retried after the next rotation received)
      decryption failure, or epoch key already erased
                             → state UNDECRYPTABLE, never deleted (invariant 7)
      success                → re-seal under lsk and erase the raw envelope (format-v1.md §7.2)
    acknowledgement queue: ack(item_id, release = release_possible(item))
    notification: if ev.data.ring and unread and now − created_at < 15 min → ring; otherwise dot only
  "ack"            : update the delivery status of my sent items ("livré à X" UI label)
  "read"           : mark as read; remove the local notification
  "tombstone"      : trust_store.verify_tombstone(Tombstone::from_bytes(ev.data.tombstone)) ;
                     if invalid: ignore and log; otherwise erase the local copy and the files,
                     add to tombstoned, pins.forget(item_id), archives.forget(item_id), close the open view, clear the
                     clipboard if it contains the value
  "attachment_tombstone" : (format-v1.md §8.10.5) a = AttachmentTombstone::from_bytes(ev.data.statement) ;
                     trust_store.verify_attachment_tombstone(a); on failure (or malformed event): log
                     "attachment_tombstone_refused" and CONTINUE (the cursor always advances);
                     if a.item_id ∈ tombstoned: ignore; otherwise record (item_id, a.slot, a.blob_id) in the
                     attachment_removals registry; if the item is present and readable: compare
                     payload.attachments[a.slot].blob_id with a.blob_id (different: ignore + log), then erase the
                     attachment (file row, encrypted and decrypted copies, local payload entry emptied, §8.10.4) and,
                     on a received item with no attachment left to retrieve, queue the release. Idempotent.
                     No further network request, never a download.
  "item_state"     : if ev.data.device_id == me: mark the ITEM_STATE operation of the outbox "sent"; return
                     (the state is already applied locally)
                     depending on space_keys.open_received_state(envelope, event_id, ev.seq) (format-v1.md §8.8.2):
                       Unreadable(KeyPending | UpdateRequired) → keep raw in pending_states (with ev.seq),
                         retry just after the next applied rotation / after the app update
                       Unreadable(SuspectEpoch | Undecryptable) → ignore, log without content
                       Opened(Unknown)  → no effect, but keep the raw envelope in pending_states
                                          (state type of a newer version): it is replayed after an application
                                          update (format-v1.md §8.8.2, step 6); a type still unknown stays kept,
                                          without a log entry
                       Opened(Pin(p))   → if p.item_id ∈ tombstoned: ignore;
                                          otherwise pins.apply_statement(trust_store, p) (checks space, signer not
                                          revoked at that moment, signature; failure → ignore and log,
                                          format-v1.md §8.8.2);
                                          NO further network request: neither file download nor change of the
                                          release (format-v1.md §8.8.4)
                       Opened(Archive(a)) → like Pin: if a.item_id ∈ tombstoned: ignore; otherwise
                                          archives.apply_statement(trust_store, a) (format-v1.md §8.9.2);
                                          same guarantees: NO further network request, never a read, ack, release
                                          or download (§8.9.4)
  "rotation"       : trust_store.apply_rotation(KeyRotation::from_bytes(ev.data.rotation)) (checks the issuer,
                     the signature, marks the revoked, opens the new key); if I am revoked → state REVOKED;
                     otherwise space_keys.add_rotation_key(key, ev.seq, now); retry the KEY_PENDING items
                     and the KeyPending pending_states (open_received_state with their original seq), in this
                     same transaction, before the next event (format-v1.md §8.8.2)
  "history_transfer" : if ev.data.to_device_id == me: create the receiving task (§7.8); otherwise ignore
  "history_transfer_closed" : update the corresponding task (sending: "812 éléments transférés vers X" UI label
                     if outcome = imported); otherwise ignore
  "device_added"   : if the certificate is self-signed (founding device): nothing, the device is already known
                     through pairing or the MemberList (format-v1.md §8.4);
                     otherwise trust_store.accept_device_cert(DeviceCert::from_bytes(ev.data.device.certificate))
                     (trusted issuer); failure → mark the device "unverified", never seal for it
  "device_updated" : update the name
  "device_revoked" : via "rotation" → nothing more (already applied by the rotation event);
                     via "self" or "admin" → mark the device as "revoked by the relay" (no more seal for it),
                     if it is me → state REVOKED, otherwise schedule maybe_rotate(delay = uniform(0, 30 s))
  unknown type     : ignore

release_possible(item) =
  item.blobs is empty
  OR all the wanted blobs are downloaded      // "wanted": D20 policy, independent of the pin
  OR the item is deleted locally
  // For UNREADABLE_UPDATE_REQUIRED with blobs: do not release, so that it can be downloaded after the update.
  // The pin changes nothing here: neither ack, nor release, nor download, nor relay-side duration (§8.1).

File download (D20), a rule common to all items: when the user opens the item, or automatically on receipt of
the item under the configured threshold (Wi-Fi). It is NEVER triggered, advanced or grouped because of a pin,
received or sent: otherwise the relay, which sees downloads per blob, would learn the pinned item and the
direction (format-v1.md §8.8.4).

"Not delivered" status (local computation on the sender's side, without a call to the relay):
  at the relay deadline (expires_at sent to POST /v1/items), each member who was a recipient
  (member at created_at, joined_seq < seq) and for whom no ack was received is displayed with:
    "expiré avant livraison à <nom>" (UI label)                 if the payload expiration ≤ relay deadline
    "non livré à <nom> (conservé 30 jours sur le relay)" (UI label) otherwise (no expiration, or a later one)

Periodic task (at startup and once a day):
  locally erase items whose payload expiration has passed (never according to the relay deadline),
  pinned ones included: a pin never extends an explicit expiration (local_retention);
  a local cleanup (old files, review of secrets) never erases a pinned item;
  space_keys.discard_expired(now, max_ttl_secs + 1 d, epochs still needed by the raw items)
```

**Epoch ordering rule** (`format-v1.md` §12). An honest relay checks `stale_key_epoch` in the transaction that
assigns the `seq` (§4.8): no item of epoch `e` can therefore have a `seq` greater than that of the rotation
`e → e+1`. Such an item comes from a revoked device in collusion with a compromised relay; the client keeps it,
does not open it, and displays it as "illisible (suspect)" (UI label). This rule uses only the rotations already
applied; the `seq` of a rotation remains known after its key is erased.

Acknowledgements (`ack`, `read`) are sent after the current page, in series, and replayed until they succeed
(they are idempotent). `read` is sent when the user opens, copies or closes the item (D16).

### 7.4 Item of an unknown format

Neither deletion nor silent forgetting: the item stays in the thread with the update message; `ack` is sent (the
sender sees "delivered"); `release` only if it has no blob. After the app update, the client re-decodes all the
`UNREADABLE_UPDATE_REQUIRED` items and downloads their blobs if they are still on the relay.

### 7.5 Full catch-up (`cursor_expired`, `cursor_ahead`)

```
full_resync():
  s = GET /v1/space
  k = GET /v1/keys           → apply_rotation for each rotation of epoch > trust_store.key_epoch, in order,
                               then space_keys.add_rotation_key(key, rotation.seq, now), then retry the
                               KeyPending pending_states before the next rotation (§7.3)
  d = GET /v1/devices        → accept_device_cert for each unknown certificate (refused if it does not chain
                               to a trusted member); a device absent from the relay's list is marked
                               "revoked by the relay"
  cursor = s.head_seq
  display once: "Des éléments plus anciens n'ont pas pu être récupérés" (UI label)
```

Local items are not deleted. Items that stayed on the relay during the absence are not recovered through the log
(same rule as for a new device): that is the price of an absence of more than 45 days or of a backup restore.
Another member can send them to it through a history transfer (§7.8).

### 7.6 Rotation after revocation

```
revoke(targets):                                 // the user's "Révoquer" gesture (after strong biometrics, D39)
  sync()                                         // list and epoch up to date
  b = trust_store.create_rotation(me, revoke = keys of the targets ∪ devices "revoked by the relay")
  r = POST /v1/rotations { rotation: b64(b.rotation.to_bytes()) }
  201/200               → trust_store.apply_rotation(b.rotation) ; space_keys[b.new_space_key.epoch] = b.new_space_key
  epoch_conflict        → sync() ; retry if the targets are not yet revoked
  device_set_mismatch   → sync() ; retry once

maybe_rotate():                                  // after an unsigned revocation ("self", "admin")
  s = GET /v1/space ; if not s.rotation_required: return        // another member already did it
  revoke(targets = ∅)                            // the "revoked by the relay" list is included by default
```

The other members wait a random delay (0–30 s), re-check, then perform the rotation if it is still missing.

*Automatic trigger (normative, `crates/core`).* The rotation that follows a departure (`via: "self"`) or a
revocation by the administrator (`via: "admin"`) is performed **by the core itself**, on all platforms, without
the application having to call `maybe_rotate`:

- the deadline is drawn (`now + uniform(0, 30 s)`) as soon as `rotation_required` becomes true: on application of a
  `device_revoked` of **another** device (`via` ≠ `"rotation"`), or on opening a session whose
  `space.rotation_required` is true. A deadline already drawn is not postponed;
- `maybe_rotate` is launched when the deadline has passed: at the end of each `sync()` (including those triggered by
  the doorbell) and, while the doorbell is open, at the latest 5 s after the deadline (the doorbell loop wakes up at
  least every 5 s);
- only one rotation at a time; none during a pending departure of this device (§7.9), nor on a device that is
  REVOKED, NOT_FOUND, BLOCKED_UPDATE_REQUIRED or not paired;
- success, or `GET /v1/space` no longer requesting it (another member did it): `rotation_required = false`, deadline
  erased. Transient failure (network, `429`, `5xx`, repeated `epoch_conflict`): new deadline
  `now + uniform(0, min(15 min, 30 s × 2^failures))`. Persistent `device_set_mismatch`: alert, new deadline in 1 h;
- residual window: as long as no member is connected (apps closed), the rotation waits for the next startup of one of
  them. Items sent in this window are sealed under a key that the departed device knew; it erased that key if it left
  via « Se déconnecter » (UI label), and the relay refuses all its requests. A client MUST seal the new key only for
  keys present in its trust list;
if the relay counts a member that the client does not know (persistent `device_set_mismatch` after synchronization),
the client refuses and shows an alert instead of sealing for an unverified key.
Items sent before the rotation and still in transit remain readable with the old key. The device keeps that key
`max_ttl_secs + 1 d` (31 days) after applying the rotation, then erases it (unless a raw item depends on it): items
already opened are re-sealed under `lsk` and no longer depend on it.

### 7.7 Offline sending: the outgoing queue (outbox)

Each outgoing operation is recorded **before** any request, with everything needed to replay it identically:
`item_id` and `blob_id` already drawn, envelope and blobs already encrypted on disk.

```
drain_outbox():
  for op in outbox, in insertion order (a temporary failure of one operation does not block the following ones):
    depending on op:
    ENVOI_ITEM :
      expires_at = relay_expires_at_ms(payload expiration, estimated server time, max_ttl_secs)   // §4.8
      if expires_at ≤ estimated server time: mark "expired before sending"; remove; continue
      for b in op.blobs:
        POST /v1/blobs { blob_id, size }                       // 201 or 200: idempotent
        st = GET /v1/blobs/{id}/status
        send the parts from st.upload_offset (PATCH, Upload-Offset);
          on upload_offset_mismatch: re-read the status and resume
      r = POST /v1/items { item_id, expires_at, key_epoch, envelope, blob_ids }
        201 / 200        → mark "sent" (seq); remove
        stale_key_epoch  → sync(); re-encrypt the envelope with the key of r.key_epoch (same item_id); retry
        relay_full       → keep; display "Relay plein" (UI label); retry later (delay ≥ 5 min)
        already_expired  → mark "expired before sending"; remove
        item_id_conflict, payload_too_large, validation_failed, 404 on a blob → mark "failed"; remove
        429              → wait Retry-After
        network, 5xx     → exponential delay with jitter (1 s → 5 min)
    TOMBSTONE, ACK, READ, RENAME: replay until success (all idempotent)
    ATTACHMENT_TOMBSTONE (removal "everywhere" of an attachment; `payload` = signed declaration, without `item_id`:
    it survives the erasure of the item; already applied locally):
      r = POST /v1/attachment-tombstones { attachment_tombstone }
        201 / 200         → remove
        404 (unknown route: older relay) → KEEP, long delay (6 h), never give up silently
        422               → remove, log (inconsistent declaration or blob: client bug)
        429, network, 5xx → like ENVOI_ITEM
      The core reads `features` (SpaceState, §3.3) at each session opening: the application offers "everywhere"
      only if `attachment_tombstones` is listed there. An item still in the sending queue refuses the removal,
      whatever the scope (its envelope and its blob would be sent anyway, including re-sealed after a rotation).
    ITEM_STATE (pin / unpin / archive / unarchive; created by PinBook::set_pinned or ArchiveBook::set_archived,
    already applied locally; `aux` = PinStatement or ArchiveStatement, depending on the label):
      r = POST /v1/item-states { event_id, key_epoch, envelope }
        201 / 200         → remove
        stale_key_epoch   → sync() ; re-seal the same declaration (seal_item_state, same event_id)
                            under the key of r.key_epoch; retry
        event_id_conflict → replace op with op.reissue(current key): same signed declaration (same
                            at_ms), new random event_id; retry. Never a silent abandonment: the event_id is not
                            signed, so there is nothing to re-sign, and keeping the original at_ms avoids overwriting
                            a more recent decision taken elsewhere in the meantime. A duplicate (the first envelope
                            was in fact accepted) has no effect (last-writer-wins).
                            Second event_id_conflict for the same decision → remove, log (bug)
        relay_full, 429, network, 5xx → like ENVOI_ITEM
        validation_failed, payload_too_large → remove, log (client bug)
      Several decisions on the same item may be queued: all go, in order;
      convergence (last-writer-wins) makes their order of arrival irrelevant.
    ROTATION: is not queued; it requires the network at the time of the gesture (otherwise "Réessayer quand le réseau revient" UI label)
```

Cancellation within 5 s (F16) or deletion of an item still in the queue (D34): local removal of the operation,
and `DELETE /v1/blobs/{id}` for the blobs already sent. If `POST /v1/items` has already succeeded: tombstone.

In format v1, each attachment has its own key (stored in the item's payload), independent of the epoch: after a
`stale_key_epoch`, only the envelope is redone; blobs already sent remain valid.

`expires_at` is recalculated at each attempt (the relay deadline depends on the sending time); the payload
expiration, for its part, does not change.

### 7.8 History transfer: export and import (decision D2)

Offered at pairing (on the inviter's side: « Transférer l'historique vers Pixel ? 812 éléments, 340 Mo » UI label)
and from the Devices screen (« Envoyer mon historique » UI label). **A single choice, yes or no**: the whole thread
is transferred, secrets and files included; there is no "secrets" or "files" checkbox. Both sides are persistent,
resumable tasks, like the outbox.

```
export(recipient R):                             // sender A
  sync() until cursor ≥ joined_seq(R)            // R has just been registered, or already exists
  as_of_seq = cursor ; transfer_id = uuid v4 ; HK = HistoryKey::generate()   // HK persisted (Keystore/DPAPI)
  index (temporary file, encrypted locally):
    header ; one device line per known device (revoked ones included) ;
    one item line per readable item, in origin_seq order (secrets included, titles included;
    excluding items still in the outbox, which will arrive through the log) ; pin = pins.state(item_id)
    (pinned or unpinned, with its at_ms; absent if no decision is known, format-v1.md §8.8.5):
    archive = archives.state(item_id) (archived or unarchived, with its at_ms; absent if no decision
    known, format-v1.md §8.9.4):
      for each attachment present locally: re-encrypt under a new FK' and a new prefix
        (ChunkEncryptor::new(item_id, index)) → new blob ; files[i] = { blob_id, FK', prefix }
      attachment absent locally: files[i] = null
    end (skipped.unreadable = number of unreadable items not exported)
  n = history_part_count(index size, max_blob_bytes)
  for p in 0..n: HistoryPartEncryptor::new(HK, transfer_id, p, n) → blob of part p
  send all the blobs (POST /v1/blobs, PATCH; resumable)
  m = trust_store.create_history_transfer(me, R, transfer_id, as_of_seq, now, HK, parts)
  POST /v1/transfers { transfer_id, manifest: m, blob_ids: parts + files }
    201/200 → wait for history_transfer_closed ; erase HK and the temporary file
    transfer_limit → retry later ; 413 → display "Historique trop volumineux" (UI label) ; 422 → failure
  if a revocation event for R arrives: abandon (the relay has already purged)

import(history_transfer event for me):           // recipient R
  t = GET /v1/transfers/{id} ; m = HistoryTransfer::from_bytes(t.manifest)
  HK = trust_store.verify_history_transfer(m, me)        // failure → DELETE outcome=failed, display the error
  r = HistoryIndexReader::new(m)
  for each part p, in order: download (Range, resumable),
    HistoryPartDecryptor::new(HK, m, p) ; r.feed(plaintext) → entries
    in batches, in a local transaction: for each item entry:
      if item_id ∈ tombstoned: ignore
      if entry.pin is present: pins.merge(item_id, entry.pin)    // even if the item is already there
      if item_id ∈ items: ignore the rest                         // idempotent, resume after a crash
      else insert re-sealed under lsk, origin = imported (from A), read and kept taken from the index,
            local expiration = that of the payload
  r.finish()                                                     // history_incomplete → DELETE outcome=failed
  files: download each files[i] that is not null (ChunkDecryptor with HistoryItem::import_attachment(i)),
    at import or on demand while the transfer has not expired; files[i] = null → thumbnail and
    "Fichier non disponible sur <nom de A>" (UI label)
  DELETE /v1/transfers/{id} { outcome: "imported" } ; erase HK
```

Normative import rules:

- No `ack`, `read` or `release` is sent to the relay for an imported item; no notification nor "unread" dot is
  created (a single end notification: « 812 éléments importés depuis <appareil> » UI label, without any title or content).
- An unread `after_first_read` item remains single-use: its revelation on R emits the tombstone everywhere (§4.8).
- An imported secret stays masked, is not indexed for search, and its title is never displayed without revelation
  (`format-v1.md` §5.4.5).
- A tombstone received through the log after the snapshot erases the imported item (or prevents its import if it
  arrives before); items that arrived between `joined_seq` and `as_of_seq` arrive on both sides and are
  de-duplicated by `item_id`.
- The pin is part of the transfer: the `pin` field of an `item` line is merged, last-writer-wins, with the local state,
  whether the item is new or already present. An `item_state` event received through the log before or after the
  import converges to the same state (`format-v1.md` §8.8.3).
  The same goes for the `archive` field (`format-v1.md` §8.9.4), merged into the archive registry.

### 7.9 Device departure and space deletion, client side

Two user gestures: « Se déconnecter » (sign out) (the device leaves the space, `DELETE /v1/me`, §3.4 (b) and
§4.4) and, for the last device, « Supprimer l'espace » (delete the space) (`DELETE /v1/space`, §4.4.1). In both
cases the device then erases everything it holds. The relay is told first: a local erasure alone would leave a
ghost member (it counts in `max_devices` and remains a recipient of rotations).

#### 7.9.1 Positive proof (normative)

The client erases its local data **automatically** only on one of these three responses from the relay,
received in full during the current attempt:

| Proof | Returned by | Proves |
|--------|-----------|--------|
| `200` whose JSON body has an integer `seq` field | `DELETE /v1/me` | this device has left the space |
| `403 revoked` | `POST /v1/auth/session`, any **B** route | this device is no longer a member (departure proof) |
| `200` whose JSON body has `space_deleted` equal to the **boolean** `true` | `DELETE /v1/space`, `POST /v1/space/receipt` | the space is deleted |

**Nothing else is a proof**, in particular: `401` (even confirmed by a second challenge), `409
space_not_initialized`, `404`, `500`, `503` and any `5xx`, `429`, a cut connection, a timeout, a truncated,
unreadable or unexpected body (`2xx` without the field, `"space_deleted": "true"`, `1`, `null`), a `4003` or
`4004` close of the doorbell, a `device_revoked` event, a rotation that removes the device, and **any state kept
locally** (`meta.blocked`, verdict of a previous startup). A kept state is always re-verified by a new request
before erasing.

#### 7.9.2 Persistent states and state machine

Keys of `meta` (none contains a secret, so they are not sealed):

| Key | Values |
|-----|---------|
| `leave_state` | absent: nothing in progress; `requested`: departure requested, proof awaited; `deleting`: space deletion requested, proof awaited; `wiping`: proof received, erasure in progress |
| `departure_since` | milliseconds: time of the user's **last gesture** (« Se déconnecter », « Supprimer l'espace », « Réessayer »; UI labels). Absent for a marker written by an earlier version: treated as very old |
| `deletion_id` | 32 random bytes, created at the first deletion request, sent back identical at each retry, erased **only** by the erasure (a cancellation keeps them) |

In memory only: the **last verdict** of the relay and its time (`Member`, `NotFound`: confirmed `401` or
`409 space_not_initialized`; `RelayTooOld`; `Transient`), and the reason for the last wait.

```
                 « Se déconnecter »                         proof (T1, T2)
   (nothing) ─────────────────────────▶ requested ─────────────────────────────▶ wiping ──end──▶ (not paired)
     │  ▲                             │   │ ▲                                    ▲
     │  │ definitive refusal (T1)     │   │ │ 409 not_last_device (T3)          │ proof (T3, T4)
     │  └─────────────────────────────┘   │ │                                    │
     │  ▲          « Annuler »            │ │                                    │
     │  └─────────────────────────────────┘ │                                    │
     │                                      │                                    │
     │ « Supprimer l'espace » (after 409 last_device, irreversible confirmation)  │
     └──────────────────────────────────▶ deleting ─────────────────────────────┘
                                            │ definitive refusal (T3) or « Annuler » → (nothing), deletion_id kept
```

- `requested` and `deleting` are written **in a single transaction, before the first network call** of the attempt,
  with `departure_since` (and `deletion_id` if it is missing). As long as they are there, the device remains a
  member and works (synchronization, sending, doorbell); the interface **shows the state permanently** (thread
  banner) with « Réessayer » and « Annuler » (UI labels, "Retry" and "Cancel").
- `wiping` is written in the **same transaction** as the erasure of the tables, and only if `leave_state` still
  equals `requested` or `deleting` in that transaction (otherwise: cancelled in the meantime, nothing is erased).
  There is therefore never a "keys erased without marker" state. One leaves `wiping` only when the erasure is
  complete; it is resumed when the engine opens, if a stop interrupted it.
- « Annuler » (`cancel_departure`) removes `requested` or `deleting` **immediately**, without waiting for a relay
  response. It is refused in `wiping`. An attempt in progress whose proof arrives after the cancellation **erases
  nothing** (the erasure transaction re-checks the marker): a `403 revoked` leaves the device REVOKED with its data
  readable; a proven deletion leaves it NOT_FOUND.

#### 7.9.3 One attempt

An attempt follows the tables below in order: for `deleting`, T4 (receipt) then T1 (session) then T3; for
`requested`, T1 then T2. Attempts from the same device are serialized. The session is **always new** (a new
challenge; the kept state REVOKED or NOT_FOUND is ignored for this request). Timeouts: `10 s` to connect and
`20 s` in total for `DELETE /v1/me`, `DELETE /v1/space` and `POST /v1/space/receipt`; those of the session (§7.2)
for the challenge and the session.

Columns: **Erases** = immediate erasure; **Marker** = what becomes of `leave_state`; **Result** = outcome given to
the application; **D4** = the interface may offer "Effacer quand même" (Erase anyway) (§7.9.5).

*T1 — New session (`POST /v1/auth/challenge` then `POST /v1/auth/session`)*

| Response | Erases | Marker | Result | D4 |
|---------|--------|----------|-------|----|
| `200` (session opened) | — | unchanged | continue: T2 or T3 | — |
| `403 revoked` | **yes** (departure proof) | → `wiping` | Erased | — |
| `401`, then `401` on a second new challenge | no | kept | Pending (not found) | yes |
| `409 space_not_initialized` | no | kept | Pending (not found) | yes |
| `409 update_required`, `409 format_unsupported`, `410 update_required` | no | removed | Refused (update required) | no |
| `503`, other `5xx`, `429` | no | kept | Pending (server) | no |
| network error, timeout, unreadable body | no | kept | Pending (network) | no |
| other `4xx` | no | removed | Refused (unexpected) | no |

*T2 — `DELETE /v1/me` (marker `requested`)*

| Response | Erases | Marker | Result | D4 |
|---------|--------|----------|-------|----|
| `200` with integer `seq` | **yes** | → `wiping` | Erased | — |
| `2xx` without integer `seq` (empty, `{}`, `"seq": "12"`, non-JSON) | no | kept | Pending (network) | no |
| `403 revoked` | **yes** | → `wiping` | Erased | — |
| `409 last_device` | no | removed | Refused (last device): the interface offers "Supprimer l'espace" (D3) | no |
| `401` after a session renewal, `409 space_not_initialized` | no | kept | Pending (not found) | yes |
| `409 update_required`, `format_unsupported` | no | removed | Refused (update required) | no |
| `503`, other `5xx`, `429` | no | kept | Pending (server) | no |
| network error, timeout, truncated body | no | kept | Pending (network) | no |
| other `4xx` | no | removed | Refused (unexpected) | no |

*T3 — `DELETE /v1/space` with `{ "deletion_id": … }` (marker `deleting`)*

| Response | Erases | Marker | Result | D4 |
|---------|--------|----------|-------|----|
| `200` with `"space_deleted": true` (boolean) | **yes** | → `wiping` | Erased | — |
| `2xx` without this field or with another value | no | kept | Pending (network) | no |
| `403 revoked` | **yes** (departure proof: the space still exists for other members) | → `wiping` | Erased | — |
| `409 not_last_device` | no | → `requested` (same transaction), then **one** attempt T1 + T2 | that of T2; if it gives Refused (last device), the interface asks for confirmation again | — |
| `401` after a session renewal, `409 space_not_initialized` | no | kept | re-check the receipt (T4): proof → Erased; otherwise Pending (not found) | yes |
| `404`, `405 method_not_allowed` (relay without this route) | no | removed | Refused (relay too old) | yes |
| `409 update_required`, `format_unsupported` | no | removed | Refused (update required) | no |
| `500`, `503`, other `5xx`, `429` | no | kept | Pending (server) | no |
| network error, timeout, truncated body | no | kept | Pending (network) | no |
| other `4xx` (`400`, `415`, `422`) | no | removed | Refused (unexpected) | no |

*T4 — Receipt `POST /v1/space/receipt` (marker `deleting`, at the start of each attempt and after a `401` /
`409 space_not_initialized` from T3)*

| Response | Erases | Marker | Result |
|---------|--------|----------|-------|
| `200` with `"space_deleted": true` (boolean) | **yes** | → `wiping` | Erased |
| `404` | no | unchanged | continue (T1) |
| any other response (network, `5xx`, `429`, unexpected `2xx`, `405`) | no | unchanged | continue (T1): never a proof |

#### 7.9.4 Retries, resumption at startup and backoff

- **After a gesture** (« Se déconnecter », « Supprimer l'espace », « Réessayer »; UI labels), the application re-runs
  the attempt as long as it gives Pending (network | server), at the delays 2 s, 5 s, 15 s, 30 s, 60 s, 120 s (each
  increased by a random 0 to 20 %), then only on the return of the network and on return to the foreground, at most
  once a minute. The requests are idempotent: a departure accepted whose response was lost gives `403 revoked` on
  retry; a completed deletion gives the receipt.
- **Resend window**: the automatic attempt (`continue_departure`) **resends** `DELETE /v1/me` or `DELETE /v1/space`
  only within **15 minutes** after `departure_since`. Beyond that, or for a marker without `departure_since`, it
  **only checks**: receipt (T4) for `deleting`, then new session (T1); `403 revoked` or receipt → Erased; session
  opened → Pending (not sent: the device is still a member, the request had no effect); the rest as T1. Only
  "Réessayer" resends the request after 15 minutes. Thus a departure is never executed long after the gesture
  without the user having seen it on screen.
- **At startup**, before `resume_pairing` (`pairing-v1.md` §11.4): `continue_departure` once (same window rule),
  then the backoff above if the result is transient.
- **When the engine opens** (without network): `wiping` → finish deleting the files and the compaction, then remove
  the marker; `requested` or `deleting` on an unpaired device → remove them.

#### 7.9.5 Erasing without proof ("Effacer quand même", D4)

`erase_without_proof()` performs the same erasure **without any call to the relay**. It is the **only** recourse
when the relay cannot prove: it is allowed (otherwise `InvalidInput`) only if the last verdict of the relay,
obtained **within the last 10 minutes** by a request from this process, is `NotFound` (confirmed `401` or
`409 space_not_initialized`) or `RelayTooOld` (T3). Never after a mere network error, a timeout, a `5xx` or a
`429`. The device leaves nothing on the relay side: if the space still exists, the device remains registered there
(ghost member) and this phone no longer has access to it. The interface surrounds it with strong biometrics
requested each time (same scope as revoking another device), an irreversible confirmation with an honest text, and
a guard against accidental taps. After a manual `tomi-relay admin reset`, this is the expected path: the application
displays "espace introuvable" (UI label) and offers it (D5).

#### 7.9.6 The erasure

```
erase(guard):                       // guard: "requested|deleting marker present" (departure) or none (D4)
  0. take the sync, send and transfer locks, in this order; check that the device is still paired
  1. empty the decrypted cache (files/cache)
  2. one transaction, secure_delete active:
       if guard and meta.leave_state ∉ {requested, deleting}: cancel the transaction, return « Annulé »
       DELETE all the rows of ALL the tables of the schema (list read from sqlite_master,
       sqlite_sequence included), re-insert sync_state(1, 0), meta.leave_state = "wiping"
  3. forget in memory: identity, keys (zeroize), session, limits, flags, verdict; remove the token
  4. delete files/, outbox/, dl/, export/, import/ under the core's files folder (and only it),
     recreate them empty
  5. wal_checkpoint(TRUNCATE) ; VACUUM ; wal_checkpoint(TRUNCATE)
  6. remove meta.leave_state
```

No write from another operation (marked read, rename, pin, sending, event, rotation, `meta.blocked`, session
limits) can land after step 2: every write by the core checks, within its transaction and under the identity
lock, that the identity that launched it is still the engine's.

Normative rules:

- The `leave_state` marker is in `meta`, not in `secrets`: it contains no secret. Sealing it would protect nothing,
  since whoever can write the database can also erase the keys there. A forged marker triggers an erasure only if
  the relay returns a proof (§7.9.1).
- If the erasure transaction fails, nothing is erased and `requested` or `deleting` remains (the result is a storage
  error; the next attempt asks for the proof again).
- Before any new provisioning, a pending erasure completion is resumed (compaction is mandatory: it is what removes
  the sealed records; deleting the files is best-effort, so that a file still open under Windows does not block a
  new pairing), then `blocked` and `leave_state` are cleared in the provisioning transaction, together with
  `departure_since` and `deletion_id`: a device that joins a space after a departure starts again unblocked, with
  new device keys. Encrypted files left over from the former space are unreadable (their keys are erased) and are no
  longer deleted afterwards; the decrypted cache is purged at each opening.
- **A revocation received without a departure request erases nothing** (`device_revoked` event, `403 revoked`,
  `4003` close): the device becomes `REVOKED` and its data remain readable. Only a present `requested` or `deleting`
  marker allows an automatic erasure, and only on a proof (§7.9.1). The user of a REVOKED device can then
  « Se déconnecter »: the attempt gets `403 revoked` and erases.
- A `requested` or `deleting` marker remains as long as the proof has not come, the relay has not refused for good,
  or the user has not cancelled; it is **never** removed silently on a `401` or a `409 space_not_initialized` (the
  "not found" state stays visible, with "Effacer quand même").
- A `4003` close or a revocation arriving after the erasure writes nothing (no `meta.blocked` on an unpaired device),
  and the doorbell loop stops.
- During a pending departure, the device does not rotate the key (§7.6) and does not create a pairing channel;
  the rest works normally.
- The relay does not create an invitation after a space deletion: to create another space, the operator makes one with
  `tomi-relay admin invite --new`. The device, now new, can create a space or join one.
- Kept: the envelope key supplied by the host (Keystore, DPAPI), which no longer seals anything, and the database
  schema. The erased bytes are no longer in the database files; the physical medium (flash) may keep freed blocks.
  What the application keeps outside the core (notifications, clipboard, temporary copies of files to send, screens,
  space-related preferences) is its responsibility: it erases them when the device is no longer paired.
- The other members rotate the key automatically (§7.6): the departure sets
  `rotation_required = true`.

*Compatibility.* Nothing changes in format 1 or in the vectors. On the API side, everything is additive: the optional
`deletion_id` field, the route `POST /v1/space/receipt`, the close code `1013`, and `503` responses where the relay
used to answer `401` for a "pending" space. An earlier client that sends `DELETE /v1/space` without a body gets no
receipt. An earlier client that erases on `409 space_not_initialized` or on a kept `SPACE_DELETED` (rule removed by
this version) remains exposed to the false positive of an empty relay; on the relay side, only the presence check
(§4.4.1) and `require_existing_data_dir` (§8.6) reduce this risk.

## 8. Purge, compaction, quotas, rate limits, configuration, administration

### 8.1 Item purge

An item is **purged** (envelope erased, blob files deleted, `item` event removed from the log, quota freed, item
row kept without its envelope, §8.2) as soon as one of the conditions is true:

| Case | Condition | When |
|-----|-----------|--------|
| (a) Expiry | `now ≥ expires_at` | at the next pass of the purge task |
| (b) Deleted everywhere | tombstone received for `item_id` | immediately, in the tombstone's transaction |
| (c) Delivered to all | see below | `release_grace_secs` (default 300 s) after the last required `release` |

Condition (c), evaluated at each pass:

```
R = { d member: d ≠ sender, d active (last_seen_at ≥ now − inactive_after_days), d.joined_seq < item.seq }
t = max(item.created_at, max{ d.released_at : d ∈ R })      // t = item.created_at if R is empty
purge if  (∀ d ∈ R : d has released)   AND   now ≥ t + release_grace_secs
```

- If `R` is empty (a single-device space, or no other active device), the item is purged `release_grace_secs`
  after its creation.
- `R` is re-evaluated at each pass: an inactive device that returns before the purge becomes required again.
- A revoked device, or one that has become inactive, leaves `R`; it no longer delays the purge.
- The purge task runs every `purge_interval_secs` (default 30 s). Each item is purged in its own transaction; blob
  files are deleted **after** the commit (a residual orphan file is collected at the next startup, §8.3).

Effect on devices (D27, D1): a single duration visible to the user, the payload expiration, **absent by default**.
The relay follows its own deadline (`expires_at` of §4.8, at most `max_ttl_secs`), which bounds only the retention
of an **undelivered** item; devices erase their local copy according to the payload expiration only (client rule,
outside the API). An item without expiration is purged from the relay by (b) or (c) when all active devices have
it, otherwise at the latest at `created_at + max_ttl_secs` by (a): the relay's retention ceiling remains
`max_ttl_secs`, even without expiration.

**Archive.** Same rule as the pin: an archived item is purged from the relay exactly like the others; the archive
is an encrypted, opaque state (`format-v1.md` §8.9) and changes neither acknowledgements, nor `read`, nor `release`,
nor downloads.

**Pin (decision (a)).** A pinned item is purged from the relay **exactly like the others**: conditions (a), (b) and
(c) never look at `item_state` events, which the relay in any case cannot link to an item. The pin synchronizes the
**local** display and retention of each device, which keeps its copy beyond the relay purge; the relay retains
nothing for it (no archive; option C rejected). The pin does not change downloads or the timing of `release` either
(§7.3, `format-v1.md` §8.8.4): nothing the relay observes on the blobs depends on it. A file of a pinned item that the
device has never downloaded therefore follows the ordinary fate: available until this device's `release` or the
deadline (a).

**History transfers.** A transfer (its blobs, and its row moves to the closed state) is purged: at its `expires_at`
(`transfer_ttl_secs` after the deposit), on `DELETE`, or on revocation of its sender or recipient (in the transaction
of §3.4). The purge adds the `history_transfer_closed` event (`by_device_id: null`, `outcome: "expired"` or
`"revoked"`); blob files are deleted after the commit, as for an item.

### 8.2 Log retention and compaction

| Data | Retained | Key |
|--------|-----------|-----|
| `item` event | until the item is purged | — |
| Other events (`ack`, `read`, `tombstone`, `attachment_tombstone`, `item_state`, `rotation`, `device_*`) | `event_retention_days` (default **45 days**) after `at` | `event_retention_days` |
| State-event row (`event_id`, sender, `SHA-256` of the envelope), for idempotence | like its event | `event_retention_days` |
| `attachment_tombstones` row (removal of an attachment, §4.8 bis), for idempotence | like its event; **the purge of the item does not erase it** (compacted with the event, before it) | `event_retention_days` |
| Purged item row (metadata, `SHA-256` of the envelope, acknowledgements), for idempotence | 45 days after the purge | `event_retention_days` |
| `KeyRotation` declaration (table `rotations`) | **indefinitely** (a few hundred bytes per rotation): a device that returns must be able to apply the whole chain of epochs in order via `GET /v1/keys` | — |
| Revoked device row (`device_id`, `revoked_at`, `sign_pk` in the list of forbidden keys) | indefinitely (prevents key reuse) | — |

`event_retention_days` MUST be greater than `inactive_after_days` (checked at startup; otherwise the relay refuses
to start). With the defaults (45 > 30), an active device always catches up on all tombstones, rotations and revocations.

**Compaction horizon.** The relay keeps `retention_floor_seq` = the largest `seq` deleted by the retention above
(not by an item purge). `GET /v1/events?after=N` with `joined_seq ≤ N < retention_floor_seq` → `410 cursor_expired`.
Gaps left by item purges never trigger this error.

Compaction runs once an hour.

**Assignment of `seq`.** In the write transaction that adds the event:
`seq = max(SELECT max(seq) FROM events ∪ {space.last_seq}, now_ms − 1) + 1`, where `space.last_seq` records the last
assigned `seq` (compacted or purged events do not make it go back). The intended consequence: after restoring a
backup, new events receive `seq` values greater than any cursor distributed before the restore (time has advanced),
so no client silently skips new events, in particular tombstones. Events posted after the backup are lost, which
is unavoidable. If the relay's clock goes backwards, the `max(…)` rule keeps the growth strictly increasing.

### 8.3 Other cleanups

| Object | Deleted | Key (default) |
|-------|----------|--------------|
| Unattached blob | `blob_orphan_ttl_secs` after the last part received (or the creation) | 86 400 s |
| Open history transfer (blobs) | at its `expires_at` (§8.1) | `transfer_ttl_secs` = 604 800 s |
| Row of a closed transfer (manifest, outcome), for idempotence | `event_retention_days` after closing | 45 days |
| Pairing channel | at its `expires_at` (§4.6) | `pairing_ttl_secs` = 120 s |
| Authentication challenge | at its `expires_at` | `challenge_ttl_secs` = 60 s |
| Session | at its `expires_at` | `session_ttl_secs` = 3600 s |
| Presence | `present_until` passed = absent (no deletion needed) | `presence_ttl_secs` = 120 s |
| Files of `spaces/<space_id>/blobs/` without a row in **this space's** database | at startup | — |
| Routing entries of a deleted session or channel | at the same time as the row (§1.4, §3.3) | — |

Each cleanup, purge (§8.1) and compaction (§8.2) pass applies to **each space**, one after the other, in its own
database and its own blob folder.

Inactive devices are **never** deleted automatically: only a human revokes them.

### 8.4 Quotas

- `used_bytes` of a space = sum of the envelope sizes of non-purged items + sum of the envelope sizes of
  `item_state` events still in the log (§8.2) + sum of the **declared** `size` values of existing blobs (complete or
  not, attached to an item, to a history transfer, or to nothing). The size of a blob is reserved at
  `POST /v1/blobs`. An open transfer therefore counts for the sum of its blobs (at most `max_transfer_bytes`).
  The **relay total** is the sum of the `used_bytes` of all spaces.
- Three limits apply to each reservation (`POST /v1/blobs`, `POST /v1/items`, `POST /v1/item-states`,
  `POST /v1/transfers`). It passes if **all three** are respected, otherwise `507 relay_full`:
  1. the **space quota**, if it has one, decided in this order (the first that exists wins): its override
     (`admin quota`, stored in `routing.db`); failing that, the **default quota set from the web panel**
     (§8.7.1, stored in `routing.db`, effective immediately); failing that, `max_space_bytes` from the configuration
     (`0` = no limit); failing that, **no per-space limit**: there is no default per-space quota (key absent,
     nothing set);
  2. the **relay total**: `sum of used_bytes + new_size ≤ max_relay_bytes` (default 1 073 741 824, 1 GiB);
  3. the free disk space of `data_dir` minus `new_size` does not drop below `min_free_disk_bytes`
     (default 1 073 741 824, 1 GiB).
- **Reservations are serialized** by a lock common to all spaces, held from the reading of the total until the
  transaction is validated: two spaces that reserve at the same instant cannot together exceed the total. Releases
  (purge, deletion) do not take this lock. At the targeted scale (fewer than five spaces), this serialization is
  not noticeable.
- **What a space learns.** `507 relay_full` gives `used_bytes` = the usage **of this space** and `max_bytes` = its
  **effective limit**, and `quota` of `GET /v1/space` (§3.3) gives the same two numbers:

  `max_bytes = min( space quota (if it has one) ,  used_bytes + max(0, max_relay_bytes − relay total) )`.

  Never the usage of another space. (The room left in the total is shared: it can decrease without this space
  having written anything.)

  Known limit: since `max_bytes − used_bytes` is the room left in the total, a space can deduce the **cumulative**
  usage of the other spaces (never that of any one of them in particular). This is unavoidable with a shared total
  and is accepted: at the targeted scale, the spaces mutually trust each other on this point.
- **Alerts.** Beyond 90 % of a limit, the relay logs a `WARN`, at most once an hour per limit: for each space that
  has a quota (the message names the space by the first 8 characters of its `space_id`), and for the relay total
  (without a space name). `admin status` displays the usage and the limit (§8.7).
- The sum of the per-space quotas is not checked: the real ceiling is `max_relay_bytes`, which the operator sizes
  according to the disk (including local backups, `infra/backup.md`) and the 10 % that the quota does not count
  (journal, index, WAL).
### 8.5 Rate limits

Token buckets (*token bucket*): `rate` = refill per minute, `burst` = capacity. Exceeding → `429 rate_limited`
with `Retry-After`. The client IP is read from `X-Forwarded-For` **only** if the connection comes from an address
in `trusted_proxies` (default `127.0.0.1/32`, `::1/128`); otherwise the TCP address.

| Key | Scope | Rate / min | Burst |
|-----|--------|-------------|--------|
| `rl_ip_public` | per IP: all `/v1` routes without a session (except `hello`); a `Pairing` request is counted here only if its token is not the channel's own (see below) | 30 | 10 |
| `rl_ip_space_create` | per IP: `POST /v1/space` | 1 | 5 |
| `rl_ip_auth` | per IP: `auth/challenge` + `auth/session` | 20 | 10 |
| `rl_ip_claim` | per IP: `claim` | 5 | 5 |
| `rl_global_claim` | whole relay: `claim` | 60 | 20 |
| `rl_ip_ws` | per IP: openings of `/v1/ws` | 30 | 10 |
| `rl_device` | per device: all B routes (except blob parts and downloads); per channel for the holder of a valid `Pairing` token (see below) | 300 | 100 |
| `rl_device_items` | per device: `POST /v1/items` | 60 | 20 |
| `rl_device_tombstones` | per device: `POST /v1/tombstones` and `POST /v1/attachment-tombstones` (shared) | 120 | 30 |
| `rl_device_item_states` | per device: `POST /v1/item-states` | 60 | 30 |
| `rl_device_presence` | per device: `PUT /v1/me/presence` | 12 | 4 |
| `rl_device_pairing` | per device: `POST /v1/pairing/channels` | 10 / hour | 3 |
| `rl_device_transfers` | per device: `POST /v1/transfers` | 10 / hour | 3 |
| `rl_device_blob_parts` | per device: `PATCH` and `GET` of blobs | 1200 | 200 |
| `rl_pairing_poll` | per channel and per side: `GET .../messages` | 60 | 10 |

`/healthz`, `/updates/`, `/.well-known/` and `/l/` have a separate per-IP limit (`rl_ip_static`, 120/min, burst 60).
The counters are in memory (lost on restart, which is acceptable).

**Limits shared between spaces.** The counters are not per space. The **per-IP** limits (`rl_ip_*`) are common:
several households behind the same address (the operator's NAT) share theirs. `rl_global_claim` is a single limit
for the whole relay, under a single key. The **per-device** and **per-channel** limits are not shared: `device_id`
and `channel_id` are unique on the relay (§1.4), so the key of a counter cannot designate two spaces.

**`Pairing` requests.** `Authorization: Pairing <token>` carries no session: the per-device limit (step 5 of §2.5)
cannot apply to it by itself. The relay judges the token against the channel in the URL. The token is **valid for
the channel** if the channel exists, has not expired, and if the SHA-256 of the token is the one recorded at
`claim` (§4.6).

- **Token valid for the channel**: the request is **not** counted in `rl_ip_public`. A pairing brings together two
  devices of the same household behind the same address; it must not be slowed down by that (a dozen requests from
  the new device are enough to exhaust the burst). The request is counted, like that of a device, in `rl_device`
  (step 5), with the channel as key; a read is also counted in `rl_pairing_poll`. The token remains bounded: it can
  only be obtained through a `claim`, itself limited by `rl_ip_claim` and `rl_global_claim`; it dies with the channel
  (`pairing_ttl_secs`); messages are limited to `pairing_max_messages` per side; and `rl_device` also bounds refused
  requests (a 9th message, repeated `DELETE`), which nevertheless hit the database's single writer.
- **Any other `Pairing` token** (unknown, the token of another channel, unknown channel, expired or never joined,
  a string that does not have the shape of a token): the request is counted in `rl_ip_public`. Without this limit,
  anyone could saturate the relay with bogus `Pairing` requests, which have no session to be charged to. To decide,
  the relay reads the channel before the per-IP limit (step 2), for this type of request only. The order of responses
  for an invalid token remains that of §2.5: `429 rate_limited`, then `space_not_initialized`, then the route's
  response (§4.6).

For this, the relay reads only the channel's deadline (`expires_at`) and the hash of its token (in the database of
the space designated by the channel number), never a message.

Trade-off: `rl_global_claim` prevents sweeping channel numbers, but an attacker can exhaust it and hinder a
legitimate pairing for a minute. Pairing remains possible afterwards. On a relay that hosts several people, this
shared limit lets one of them hinder the others' pairings, and a random `claim` on an open channel of another person
makes that person's pairing fail (the channel moves to `claimed`; CPace prevents any takeover, not this denial of
service). Accepted for fewer than five spaces; to be monitored, and to be replaced by a per-space limit if one of
them abuses it.

### 8.6 Configuration

TOML file (`--config <path>`, default `/etc/tomi-relay/config.toml`). Each key can be overridden by an environment
variable `TOMI_RELAY_<KEY IN UPPERCASE>`. Any unknown key makes startup fail (typo).

| Key | Default | Role |
|-----|--------|------|
| `listen` | `"127.0.0.1:8080"` | local HTTP address (behind Caddy) |
| `allow_public_listen` | `false` | allows a non-loopback address (dev) |
| `public_url` | `"https://relay.example.org"` | base of the returned URLs (`update_url`) |
| `admin_web` | `false` | `true`: serves the web admin panel under a secret path (bounded exception of §8.7.1). Requires a `public_url` in https on a domain name, otherwise the relay refuses to start (exit code `1`). Variable: `TOMI_RELAY_ADMIN_WEB` |
| `data_dir` | `"/var/lib/tomi-relay"` | `routing.db`, `spaces/` (one database and blobs per space, §10), `public/`, `admin.sock`, `relay.lock` |
| `require_existing_data_dir` | `false` | `true`: the relay refuses to start (non-zero exit code, message to the operator) if `data_dir` contains neither `routing.db` nor `spaces/` nor the old layout (§10.1.1). Protects against an unmounted volume or a wrong `data_dir`, which would make the relay look new (`space_not_initialized` for everyone). Set it to `true` after the first installation. Also applies to read-only `admin` commands (§8.7): on a `data_dir` that looks empty, they refuse with the same message instead of showing an empty relay. Variable: `TOMI_RELAY_REQUIRE_EXISTING_DATA_DIR` |
| `deletion_receipt_ttl_secs` | 7776000 (90 days) | retention period of space deletion receipts (§4.4.2) |
| `trusted_proxies` | `["127.0.0.1/32", "::1/128"]` | §8.5 |
| `session_ttl_secs` | 3600 | §3.3 |
| `challenge_ttl_secs` | 60 | §3.3 |
| `inactive_after_days` | 30 | §0.2 |
| `presence_ttl_secs` | 120 | §6.2 |
| `release_grace_secs` | 300 | §8.1 |
| `purge_interval_secs` | 30 | §8.1 |
| `event_retention_days` | 45 | §8.2 |
| `blob_orphan_ttl_secs` | 86400 | §8.3 |
| `pairing_ttl_secs` | 120 | §4.6 |
| `pairing_max_open` | 2 | §4.6 |
| `pairing_max_messages` | 8 | §4.6 |
| `pairing_max_message_bytes` | 8192 | §4.6 |
| `max_json_body_bytes` | 2097152 | §2.2 |
| `max_envelope_bytes`, `max_blob_bytes`, `max_item_blob_bytes`, `max_blobs_per_item`, `max_upload_part_bytes`, `max_ttl_secs`, `max_devices` | §2.8 | per space (`max_devices`) or per request |
| `max_relay_bytes` | 1073741824 | total of the `used_bytes` of **all** spaces (§8.4); must be > 0 |
| `max_space_bytes` | **absent** | default quota of a space (§8.4); optional key: absent, or `0`, means no per-space limit; otherwise at most 2^53−1 (9,007,199,254,740,991). A space's `admin quota` override, then the default set from the web panel (§8.7.1), take precedence over it. Environment variable: `TOMI_RELAY_MAX_SPACE_BYTES` |
| `max_transfer_bytes`, `max_blobs_per_transfer`, `transfer_ttl_secs`, `transfer_max_open` | §2.8 | history transfers (§4.13) |
| `max_item_state_bytes` | §2.8 | item-state events (§4.8) |
| `min_free_disk_bytes` | 1073741824 | §8.4 |
| `fcm_service_account_file` | absent | FCM disabled if absent |
| `fcm_project_id` | read from the service account file | |
| `rl_*` | §8.5 | limits, in the form `{ per_minute = 30, burst = 10 }` |
| `log_format` | `"json"` | `json` or `text` |

*Pending space and quotas.* The total of `max_relay_bytes` (§8.4), `others_used` of a space's budget and the
`/healthz` check do not count a "pending" space (§4.4.1): it is no longer in the list of spaces. The gap is bounded
by the size of that space, and lasts until the deletion ends or until the next startup.

For integration tests, the implementation SHOULD accept an **injectable clock** and reduced values (e.g.
`inactive_after_days` expressible in seconds through a test option), so as to verify the 30- and 45-day rules
without waiting.

### 8.7 Local administration: `tomi-relay admin`

**No administration API is exposed on the network**, except the bounded exception of the web admin panel (§8.7.1),
disabled by default. The commands run on the relay machine (over SSH), under the service user.

Mechanism: the running server listens on a Unix socket `<data_dir>/admin.sock` (mode `0600`, owner = service user;
never TCP). The `admin` command connects to it if it exists. If the server is not running (no socket, `<data_dir>/relay.lock`
lock free), it acts directly on the database. Read commands can always read the databases directly (SQLite WAL),
without a lock and **without writing anything**: no migration, no creation of `routing.db`, no row deletion, no change
of journal mode; they open the databases read-only, and a missing `routing.db` counts as empty.
If the folder still has the old layout intact (§10.1.1: `tomi.db` alone, neither `spaces/` nor `spaces.migrating/`),
they refuse with a message saying it has not yet been migrated (the relay of this version migrates it at startup) and
exit with `1`, **not** `78`: this is not a refusal due to the data (a pre-migration binary reads it). A migration in
progress or interrupted (`spaces.migrating/`), or both layouts at once, exit with `78`. With
`require_existing_data_dir = true` (§8.6), they also refuse a `data_dir` that looks empty (unmounted volume), with the
startup message.
On Windows (development), the socket does not exist: `revoke`, `reset`, `quota`, `invite --new` and `invite --rotate`
require the server to be stopped. The socket protocol is internal to the binary (not specified here). The socket is
**unique** for the whole relay.

**Designating the space.** A relay hosts several spaces: `status`, `devices`, `revoke`, `reset` and `quota` take
`--space <id>`, the full `space_id` or a prefix of **at least 8 characters**. Without `--space`, if the relay has only
one space, that is the one; if it has several, the command **refuses (code 2) and lists the spaces**. A prefix that is
too short or ambiguous is refused (code 2); a prefix that designates nothing is an error (code 1). `spaces` and
`invite` concern the whole relay.

| Command | Effect | Output |
|----------|-------|--------|
| `admin status [--json]` | read | **relay state**: version; number of spaces, and separately the number of "pending" spaces (§4.4.1); `space_initialized` computed **exactly** as `GET /v1/hello` (a pending space counts); total usage / `max_relay_bytes`; free disk space; **FCM health**: `disabled` / `ok` / `auth_error` / `degraded` / `unknown`, date of the last success, last error (class, date, no token), number of tokens deleted for `UNREGISTERED` over 7 days; number of pending invitations |
| `admin status --space <id> [--json]` | read | **state of a space**: `space_id`, date; number of members, active, present; `head_seq`, `retention_floor_seq`; `key_epoch`, `rotation_required`; usage of the space, its quota (or "none"), its effective limit; free disk space; FCM health |
| `admin spaces [--json]` | read | one space per line: `space_id`, creation date, members (of which active), usage, quota |
| `admin devices [--space <id>] [--json]` | read | per member: `device_id`, name, platform, `app_version`, `format_min..max`, `last_seen_at`, active, present, push yes/no, indicative cursor, fingerprint of `sign_pk` (first 16 hexadecimal characters of the SHA-256) |
| `admin quota [--space <id>] <bytes>` \| `none` \| `default` | write (server stopped, or socket) | sets the space's **quota override**: a number of bytes (≥ 1); `none` = no limit for this space, even if the configuration gives a default; `default` = removes the override (back to the relay's default: the default quota set from the web panel, otherwise `max_space_bytes`, otherwise no limit). Stored in `routing.db`, effective immediately. Invalid value: code 2 |
| `admin invite` | read | lists pending invitations: short fingerprint (8 characters) and date. **Never the token**: the relay does not keep it (§3.1) |
| `admin invite --new` | write | creates an invitation and displays its **token**, once only |
| `admin invite --rotate` | write | replaces all **pending** invitations with a single one and displays its token. **Consumed** invitations are not touched (they serve to resume a founder, §3.2) |
| `admin revoke [--space <id>] <device_id or prefix ≥ 8 characters>` | revocation §3.4 (path (c): `via: "admin"`, `revoked_by: null`, `rotation_required = true`); closes the device's WebSockets (`4003`). Refuses (code 2) an ambiguous prefix and the last member (points to `reset`) | confirmation |
| `admin reset [--space <id>] [--confirm <first 8 characters>]` | asks you to retype the first 8 characters of **this space's** `space_id` (or use `--confirm`); closes the WebSockets **of this space** (`4004`); deletes its folder (database and blobs), its routing entries, its quota override and the invitations it consumed (their tokens become invalid tokens: `403`), **nothing else**: the other spaces, their connections and `public/` are not touched. If no space remains, it adds a new invitation and displays its token (invitations already pending are kept: they may have been made for other people); otherwise it displays `space <8 characters> deleted`. On a relay without a space, without `--space`, there is nothing to erase: it only adds a new invitation | new token, or confirmation |

Deleting a space by its last device (`DELETE /v1/space`, §4.4.1) does the same thing as `admin reset` of that space
(same relay function), with one difference: it **never** creates an invitation, even if no space remains. The token
would have no safe place to be displayed; the operator makes one with `admin invite --new` (or the relay announces
one at its next startup, §3.1).

Exit codes: `0` success, `1` error, `2` refusal or incorrect usage.
FCM health: `ok` = last send succeeded and OAuth token valid; `auth_error` = obtaining the OAuth token, or a send
refused with 401/403; `degraded` = more than half of the last 20 sends in temporary error; `unknown` = configured
but no send yet; `disabled` = not configured.

#### 8.7.1 Exception: the web admin panel

The relay MAY serve a small web panel for supervision and invitations (`apps/relay/src/admin_web.rs`). It is the
**only** exception to "no administration API on the network" (§8.7) and the **only** one to "no cookie" (§2.4);
it is bounded as follows.

- **Disabled by default** (`admin_web = false`, variable `TOMI_RELAY_ADMIN_WEB`). When disabled, no path of the panel
  is routed: everything answers exactly like an unknown resource (`404`). Enabling it requires a `public_url` in
  https on a domain name (a passkey does not bind to an IP address, and the cookie is `Secure`): otherwise the relay
  refuses to start (code `1`, §8.6).
- **Outside the API.** The panel lives under `/<segment>/`, where `<segment>` is a secret of 128 random bits
  (base64url), created at the first startup with the panel active and kept in `routing.db`. It is defence in depth
  against scanners, **not** authentication. Another first segment, or a request without it, receives the ordinary
  `404`. The logs (§9) label these requests with a constant (`admin-web`): never the segment. Nothing of the panel
  exists under `/v1/*`, and the relay sets **no** cookie on `/v1/*`.
- **Authentication: a passkey** (WebAuthn, ES256, `none` attestation, **user verification (UV) required**). A single
  administrator, a single key. The first key is registered with a **single-use enrollment code** generated on the
  machine (`tomi-relay admin web enroll`: 15 minutes, 5 attempts, only its SHA-256 is kept). Once the key is
  registered, the enrollment endpoints answer `404` permanently; replacing it is done locally (`admin web reset`).
- **Session: a cookie `__Host-tomi-admin`** (`Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`, without `Domain`),
  256 random bits, of which only the SHA-256 is kept, in memory. 30 minutes of inactivity, 8 hours at most. This
  cookie is set only by the panel.
- **A single administrative action.** (Passkey enrollment and sign-out also write, but they only modify the panel's
  passkey and sessions, not the relay.) `POST /<segment>/api/invitations` (`{"mode": "new" | "rotate"}`) does what
  `admin invite --new` / `--rotate` do (§8.7) and returns the installation token **once only**, in that response.
  Every `POST` of the panel MUST carry an `Origin` header equal to the origin of `public_url` **and** the header
  `X-Tomi-Admin: 1`, otherwise `403`; with `SameSite=Strict`, this is the protection against cross-site requests.
  There is **no** revocation, **no** space erasure, and **no** change of the quota of ONE space via the web: these
  commands remain local. Only the **default space quota** can be set there (`GET`/`POST api/quota`, with the same
  safeguards as `api/invitations`). `POST` carries `{"default_bytes": N}`: an integer of bytes between 1 MiB and
  2^53-1, `0` (no per-space limit) or `null` (forget the panel's value); the response
  `{"default": {"bytes", "source", ...}}` gives the effective value and its origin.
- **Reads**: the relay state, the spaces, the devices and the pending invitations (never the token), produced by the
  same functions as `admin status`, `spaces`, `devices` and `invite`. **Metadata** only: the relay remains blind, and
  the panel never shows or carries an item, a file name, a key or a secret. Added (additive) reads: more counts,
  dates and flags per space and per device (`api/spaces`, `api/devices`), `GET api/about` (version, startup, server
  clock, session end, configuration limits: neither path nor URL) and `GET api/journal` (what the panel has done since
  startup, kept in memory only: fixed words, dates and counts, never a value coming from a request, an address, or
  anything from a space).
- **Hardening**: a per-address rate limit for every sensitive request (code, challenge, sign-in: burst of 10, then 6
  per minute) and a wider global ceiling; a body of 16 KiB at most; page, script and stylesheet embedded in the
  binary, same origin, strict `Content-Security-Policy` (`default-src 'none'`), `Cache-Control: no-store`,
  `Referrer-Policy: no-referrer`.
- **Local commands**: `tomi-relay admin web` (panel URL and passkey state), `admin web --rotate-path` (new segment:
  the old one and all sessions stop working), `admin web enroll`, `admin web reset`.

An operator who does not want it leaves `admin_web = false`: the relay then behaves exactly as if this section did
not exist (annex B.3, point 6).

## 9. Relay logging policy

Output: `stdout` (journald), JSON by default. Default level `INFO`.

**Forbidden, at every level, including `DEBUG` and `TRACE`:**
- any request or response body (JSON, envelopes, blob parts, pairing messages);
- request headers, in particular `Authorization`: only those on the "allowed" list below may appear;
- session tokens, pairing tokens, challenges, signatures, FCM tokens, the Google OAuth token, the content of the
  service account file;
- the installation token, **except** the line of §3.1 (at the first startup of a fresh relay); nor its fingerprint;
- the full `space_id`: a message that must locate a space (creation, purge, quota, compaction, migration) gives only
  its **first 8 characters** (this is allowed, see below);
- the query string: the relay logs the **route template** (`/v1/blobs/{blob_id}`), never the raw URL;
- the text of deserialization errors that copies the value received (log only the field path).

**Allowed:** timestamp, `request_id`, method, route template, HTTP status, error `code`, duration, the first 8
characters of the `space_id` to locate a background message, body sizes (in bytes), authenticated `device_id`,
identifiers `item_id` / `blob_id` / `channel_id` in dedicated fields, `seq`, `User-Agent` truncated to 64 characters,
client IP address **only** for security events (authentication failure, `install_token_invalid`, `rate_limited`,
`revoked`), FCM error classes and the HTTP code of the Google response.

Background tasks (purge, compaction) log counters (e.g. "12 items purged, 340 MB freed"), never bulk identifiers.

Backups and Caddy's logs are outside this specification, but the API is designed so that a standard Caddy access log
(which includes the URL and the query string, and masks `Authorization` by default) contains no secret.

## 10. Annex A (non-normative): on-disk layout, SQLite schemas and blob storage

### 10.1 Layout of `data_dir`

```
<data_dir>/
  routing.db                          global database: invitations (pending and consumed), quota overrides, FCM health
  spaces/
    <space_id>/                       lowercase UUID text with hyphens
      relay.db                        the space's database (schema below), + relay.db-wal / -shm
      blobs/<2 first hex>/<blob_id hex>
  public/                             APK, assetlinks.json, /l/ page (global)
  relay.lock                          process lock (global)
  admin.sock                          administration socket (global, Unix)
```

- **One SQLite database per space.** The schema of §10.2 is that of a space: `WHERE id = 1` remains true in each
  database. No table has a `space_id` column, and no query can reach another space's data: a connection opens only
  its space's file. Each database has its single writer and its readers (4 connections per space; with fewer than
  five spaces, everything stays open).
- **`routing.db`** has its own `PRAGMA user_version` (1) and three tables: `invitations(token_hash, created_at, consumed_at, space_id)`
  (`consumed_at` NULL = pending; a consumed invitation keeps the date and the created `space_id`, §3.1),
  `space_quotas(space_id, max_bytes)` (`max_bytes` NULL = no limit, absent row = default), and the health of FCM sending
  (`push_health`, `push_events`), global since a single service account serves the whole relay.
- **Routing is not a source of truth.** The lookup tables of §1.4 (session token → space, `device_id` → space,
  active and revoked `sign_pk` → space, `channel_id` → space) live in memory and are **rebuilt at each startup** by
  reading the space databases (`devices`, `revoked_sign_pks`, `sessions`, `pairing_channels`). What cannot be derived
  from the space databases — pending invitations and per-space quotas — is what is stored in `routing.db`. Deleting
  `routing.db` breaks no session and no device: it loses the invitations (pending and consumed: a founder whose `201`
  was lost then receives `403` instead of `409`) and the quota overrides. Additions are made after the space's
  database has been validated (one more entry gives no right: the database decides); removals happen at the same time
  as the rows (§8.3). The quota overrides and consumed invitations of a space that startup does not find are **kept**
  (warning in the logs): startup cannot distinguish a deleted space from a badly mounted volume, and these rows have no
  effect. A successful deletion forgets them in the transaction of its receipt (§4.4.1, step 5). The `space_deletions`
  table keeps the deletion receipts (§4.4.2).
- **Deletion of a space** (`admin reset`, or `DELETE /v1/space` by its last device): normative algorithm in §4.4.1
  (steps 1 to 7). In summary: presence of `spaces/<id>/relay.db` verified just before; the folder is **renamed** to
  `<data_dir>/spaces.deleted-<id>-<random>/` (outside `spaces/`), or, if the rename is refused, `relay.db` is deleted
  in place; only the **success** of this system call counts (`ENOENT` is never one); `fsync` of the folders (Unix);
  then a `routing.db` transaction forgets the quota and the consumed invitation and records the receipt. A blob write
  still holding the space cannot recreate `spaces/<id>/blobs/` (the space folder is never recreated by a blob write:
  it fails). A `spaces.deleted-*` folder left by an abrupt stop is deleted at the next startup. If nothing could be
  deleted, the relay re-registers the space under the registry lock; if it cannot, the space stays "pending" (`503`,
  non-empty relay) until the next startup, where it returns whole.
- **A symbolic link is not a space, and startup is refused.** The relay never loads a space whose `spaces/<id>` or
  `spaces/<id>/relay.db` is a symbolic link, and it does not silently ignore it either: it **refuses to start**, with
  a message that says to replace the link with the real folder (or the real file), and gives only the beginning of the
  `space_id`, without a path. Ignoring this space would make all the devices of an entire space receive `401` or
  `409 space_not_initialized` (contrary to §1.4, rules 4 and 5, and to §4.4.1) and, if it were the only space, would
  create and log an installation token. Read-only `admin` commands are refused in the same way. The presence check of a
  deletion does not follow links (§4.4.1, step 2): a space mounted this way could never be deleted and would become
  "pending" at each attempt. Links higher in the path (`data_dir`, `spaces/`) are allowed **as long as their target
  exists**: what matters is the identity of the folder. A `spaces/` that exists (even as a link) but whose target is
  absent or is not a folder (unmounted volume) **refuses startup**: this is not "no space". Likewise, only "does not
  exist" (`ENOENT`) means "no space" (a space folder without
  `relay.db`): any other error when looking at the disk (`EACCES` after a restore done as root without `chown`, an
  I/O error) refuses startup, with a message that has no path and no full `space_id`.
  The relay keeps the identity `(st_dev, st_ino)` of the folder and of the `relay.db` of each loaded space, read
  **after** the database is opened and its `space` row is read (as at creation), and compares it at deletion (Unix
  only, §4.4.1). A link whose name is not a `space_id` is not a space: it is ignored with a warning.
- **`spaces/` on another filesystem.** If `spaces/` is a link to a filesystem other than `data_dir`, the rename to
  `<data_dir>/spaces.deleted-*` fails with `EXDEV`: deletion then always happens in place (`relay.db` deleted, blob
  folder left as debris), and the protection that the rename gives against a blob write in progress is lost. To avoid
  this, keep `spaces/` and `data_dir` on the same filesystem.
- **A space folder that is not a space** (a name that is not a `space_id`, a database that is absent or has no
  `space` row, a remainder of an interrupted creation) is ignored with a warning. It is erased at the creation of
  this `space_id` only if it contains no `relay.db` holding a `space` row (otherwise `POST /v1/space` answers
  `409 space_exists` and touches nothing), and never if this `space_id` is "pending" (`503`).
- **Startup refused** if the `spaces/<id>` folder contains the database of another space, if a `spaces/<id>` or a
  `spaces/<id>/relay.db` is a symbolic link (above), if `spaces/` exists but cannot be read as a folder (a link whose
  target is missing, unmounted volume), or if the disk cannot be read (other than "does not exist"), or if two spaces
  carry the same `device_id`, the same signing key, the same session token or the same channel (§1.4, rule 5).
- **Exit code 78 (`EX_CONFIG`): refusal to start because of the data.** Any refusal to start that comes from what the
  data contains (or from its absence) makes the process exit with code **78**, and no other failure uses this code (an
  invalid configuration, a port already taken, a lock really held by another relay, or `admin_web` without a `public_url`
  in https, exit with `1`). Concerned are: `data_dir` empty while `require_existing_data_dir` is true, a data folder that
  cannot be created, `relay.lock` that cannot be opened or locked (other than because another relay holds it), a migration
  that fails, `routing.db` or a space database that is unreadable or of an unknown version, all the refusals of the list
  above (links, `EACCES`, unreadable `spaces/`, another space's database, routing conflict), a startup cleanup that fails,
  and a write that fails in `routing.db` at startup (first invitation, panel secret path). The `admin` command that cannot
  open the data exits with the same code, except for the old, intact, not-yet-migrated layout (§8.7: `1`).
  **Consequence for operations:**
  an update script does NOT fall back to an older binary when the binary in place exits with 78, unless the old layout is
  intact (`tomi.db` alone, neither `spaces/` nor `spaces.migrating/`: a migration that failed cleanly exits with 78 without
  moving anything, and a pre-migration binary reads it very well). A binary from before multi-space support ignores these
  spaces (it only looks for `tomi.db`), starts "empty", answers `409 space_not_initialized` or `401` for a live space, and
  creates an installation token if there is no pending invitation: worse than the outage. `infra/update.sh` and
  `infra/forge/deploy.sh` run the binary in place (`admin status --json`, under the service's user and configuration)
  before any rollback, automatic or manual, and stop with what must be checked (volume, rights, links) if the code is 78.
  Any code other than 0 and 78 (1, 124 timeout exceeded, 137 killed) means the check could not conclude: the script warns,
  and the manual rollback stops unless `--force` is given, which never overrides an established 78.
  The message of a failed database opening gives neither a path nor the full `space_id`: only the SQLite result code and
  at most 8 characters of the `space_id`.
- **`spaces/` is a SUBfolder of the data volume, never the mount point itself** (nor a link to the root of the mount
  point). The "unmounted volume" guard (`require_existing_data_dir`, §8.6) and the refusal to start when `spaces/` is
  absent or unreadable rely on the fact that, with the volume unmounted, `spaces/` no longer exists. If `spaces/` were the
  mount point, the unmounted volume would leave an empty folder in its place: the relay would see "no space", start empty,
  and answer `401` / `409 space_not_initialized` for a live space. Mount the volume on the parent (`data_dir`, or above)
  and leave `spaces/` inside it.

### 10.1.1 Migration from the single-space layout

The versions that hosted a single space (relay 0.1.x in production) stored `<data_dir>/tomi.db`
(+ `-wal`, `-shm`), `<data_dir>/blobs/` and `<data_dir>/install-token`. **Only a process that holds
`relay.lock` migrates** (the relay at startup, or an `admin` command that writes); a read-only command that
finds the old layout refuses and touches nothing (§8.7). The working folder is **reserved** by its creation
(`create_dir`, which fails if it exists), and the process never deletes a `spaces.migrating/` that it did not
create. At startup, if `tomi.db` exists and `spaces/` does not:

1. the database is opened, its journal emptied (`wal_checkpoint(TRUNCATE)`) and its `space_id` read (the schema
   version MUST be `1`);
2. the new tree is assembled under `spaces.migrating/<space_id>/` by simple **renames** (the database and its
   companion files → `relay.db`, `blobs/` → `blobs/`), within the same folder: nothing is copied or rewritten;
3. the last rename, `spaces.migrating/` → `spaces/`, is the switchover point. If a step fails, the renames
   already done are undone;
4. `routing.db` is created empty and the routing is rebuilt as at every startup; the `install-token` file,
   if it exists, becomes an invitation (§3.1).

States refused at startup, with a message that says what to do: `spaces.migrating/` present (an interrupted
migration that a human must look at); `tomi.db` **and** `spaces/` both present (we do not guess which one is
authoritative); `blobs/` containing files without `tomi.db`; unknown schema version. An old database with no
space and no device at all is deleted (there is nothing to keep), along with its `-wal` and `-shm` files and an
empty `blobs/`; nothing else is erased (`public/`, `config.toml`, logs). A migrated database may keep the
`push_health` and `push_events` tables of the old schema: they are no longer read. `SCHEMA_VERSION`
stays `1`: a migrated database and a new database therefore have the same version number but not exactly the
same tables (no effect; to remember for the next schema migration).

### 10.2 Schema of a space (`relay.db`)

WAL mode, `foreign_keys = ON`, `synchronous = FULL` (an item accepted with `201` must not disappear after a
power cut: the client has already removed it from its queue). **A single writer** (one shared write connection, or a
mutex): this is what guarantees that a smaller `seq` is never committed after a larger `seq` already visible to a
reader. Timestamps are stored as Unix milliseconds (`INTEGER`), UUIDs as 16-byte `BLOB`, `device_id` as `TEXT` (32 hex), keys as `BLOB`.

```sql
CREATE TABLE space (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),     -- a single row in this database: the space
  space_id            BLOB NOT NULL,
  created_at          INTEGER NOT NULL,
  key_epoch           INTEGER NOT NULL,
  rotation_required   INTEGER NOT NULL DEFAULT 0,
  retention_floor_seq INTEGER NOT NULL DEFAULT 0,
  last_seq            INTEGER NOT NULL DEFAULT 0      -- last assigned seq (§8.2)
);

CREATE TABLE devices (
  device_id        TEXT PRIMARY KEY,         -- 32 hex, derived from sign_pk
  name             TEXT NOT NULL,
  platform         TEXT NOT NULL,
  sign_pk          BLOB UNIQUE,              -- NULL after revocation
  kx_pk            BLOB,                     -- NULL after revocation
  certificate      BLOB,                     -- DeviceCert (bytes), NULL after revocation
  issuer_device_id TEXT NOT NULL,
  joined_seq       INTEGER NOT NULL,
  registered_at    INTEGER NOT NULL,
  last_seen_at     INTEGER,
  present_until    INTEGER,
  format_min       INTEGER,
  format_max       INTEGER,
  app_version      TEXT,
  cursor_hint      INTEGER,
  revoked_at       INTEGER
);
CREATE TABLE revoked_sign_pks (sign_pk BLOB PRIMARY KEY);   -- forbids reuse of a key

CREATE TABLE sessions (
  token_hash   BLOB PRIMARY KEY,               -- SHA-256 of the token
  device_id    TEXT NOT NULL REFERENCES devices(device_id),
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  revoked      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sessions_device ON sessions(device_id);

CREATE TABLE challenges (
  challenge    BLOB PRIMARY KEY,
  device_id    TEXT NOT NULL,
  expires_at   INTEGER NOT NULL
);

CREATE TABLE events (
  seq          INTEGER PRIMARY KEY,                 -- assigned explicitly (§8.2), never reused
  type         TEXT NOT NULL,
  at           INTEGER NOT NULL,
  author       TEXT,                                -- author device_id (NULL = admin)
  item_id      BLOB,                                -- to find an item's events
  data         TEXT NOT NULL                        -- JSON of "data" (without the envelope for type=item)
);
CREATE INDEX events_item ON events(item_id);
CREATE INDEX events_at ON events(at);

CREATE TABLE items (
  item_id        BLOB PRIMARY KEY,
  seq            INTEGER NOT NULL,
  sender         TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  expires_at     INTEGER NOT NULL,
  key_epoch      INTEGER NOT NULL,
  envelope       BLOB,                       -- NULL after purge
  envelope_size  INTEGER NOT NULL,
  envelope_hash  BLOB NOT NULL,              -- SHA-256, for idempotence
  purged_at      INTEGER
);
CREATE INDEX items_expires ON items(expires_at) WHERE purged_at IS NULL;
CREATE INDEX items_purged ON items(purged_at) WHERE purged_at IS NOT NULL;

CREATE TABLE item_rings (                      -- ring decision per recipient (§6.1)
  item_id   BLOB NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  device_id TEXT NOT NULL,
  ring      INTEGER NOT NULL,
  PRIMARY KEY (item_id, device_id)
);

CREATE TABLE receipts (
  item_id      BLOB NOT NULL REFERENCES items(item_id) ON DELETE CASCADE,
  device_id    TEXT NOT NULL,
  received_at  INTEGER NOT NULL,
  read_at      INTEGER,
  released_at  INTEGER,
  PRIMARY KEY (item_id, device_id)
);

CREATE TABLE tombstones (
  item_id      BLOB PRIMARY KEY,
  seq          INTEGER NOT NULL,
  device_id    TEXT NOT NULL,
  deleted_at   INTEGER NOT NULL,
  statement    BLOB NOT NULL                -- Tombstone (bytes) as received
);

-- ADDITIVE migration (`CREATE TABLE IF NOT EXISTS` on each write-mode opening, `user_version` stays 1):
-- a relay of the previous version opens the database as is and ignores the table.
CREATE TABLE attachment_tombstones (          -- removal of ONE attachment (§4.8 bis)
  item_id      BLOB NOT NULL,                 -- no foreign key: the item may be purged or unknown
  slot         INTEGER NOT NULL,
  blob_id      BLOB NOT NULL,
  seq          INTEGER NOT NULL,
  device_id    TEXT NOT NULL,
  deleted_at   INTEGER NOT NULL,
  statement    BLOB NOT NULL,                 -- AttachmentTombstone (bytes) as received
  PRIMARY KEY (item_id, slot)
) WITHOUT ROWID;

CREATE TABLE item_states (                    -- encrypted state events (pin, §4.8)
  event_id       BLOB PRIMARY KEY,            -- random; no item_id (events.item_id stays NULL)
  seq            INTEGER NOT NULL,
  device_id      TEXT NOT NULL,
  key_epoch      INTEGER NOT NULL,
  envelope_size  INTEGER NOT NULL,
  envelope_hash  BLOB NOT NULL               -- SHA-256, for idempotence; deleted with the event
);

CREATE TABLE blobs (
  blob_id        BLOB PRIMARY KEY,
  owner          TEXT NOT NULL,
  size           INTEGER NOT NULL,
  upload_offset  INTEGER NOT NULL DEFAULT 0,
  complete       INTEGER NOT NULL DEFAULT 0,
  item_id        BLOB REFERENCES items(item_id),          -- NULL until attached
  transfer_id    BLOB REFERENCES transfers(transfer_id),  -- history transfer (§4.13)
  last_activity  INTEGER NOT NULL,
  CHECK (item_id IS NULL OR transfer_id IS NULL)          -- mutually exclusive attachments
);
CREATE INDEX blobs_item ON blobs(item_id);
CREATE INDEX blobs_transfer ON blobs(transfer_id);
CREATE INDEX blobs_orphan ON blobs(last_activity) WHERE item_id IS NULL AND transfer_id IS NULL;

CREATE TABLE transfers (                       -- history transfers (§4.13)
  transfer_id  BLOB PRIMARY KEY,
  from_device  TEXT NOT NULL,
  to_device    TEXT NOT NULL,
  manifest     BLOB NOT NULL,                  -- HistoryTransfer (bytes) as received
  blob_set_hash BLOB NOT NULL,                 -- SHA-256 of the sorted blob_ids, for idempotence
  as_of_seq    INTEGER NOT NULL,
  seq          INTEGER NOT NULL,               -- seq of the history_transfer event
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL,
  closed_at    INTEGER,                        -- NULL while open
  outcome      TEXT                            -- imported | declined | failed | cancelled | expired | revoked | other
);
CREATE INDEX transfers_open ON transfers(expires_at) WHERE closed_at IS NULL;

CREATE TABLE rotations (                       -- never deleted (§8.2)
  key_epoch    INTEGER PRIMARY KEY,
  device_id    TEXT NOT NULL,                  -- issuer
  created_at   INTEGER NOT NULL,
  seq          INTEGER NOT NULL,
  statement    BLOB NOT NULL                   -- KeyRotation (bytes) as received
);

CREATE TABLE pairing_channels (
  channel_id   TEXT PRIMARY KEY,
  creator      TEXT NOT NULL,
  state        TEXT NOT NULL,              -- open | claimed | completed | closed
  token_hash   BLOB,                       -- Pairing token
  created_at   INTEGER NOT NULL,
  expires_at   INTEGER NOT NULL
);
CREATE TABLE pairing_messages (
  channel_id   TEXT NOT NULL REFERENCES pairing_channels(channel_id) ON DELETE CASCADE,
  idx          INTEGER NOT NULL,
  side         TEXT NOT NULL,              -- inviter | joiner
  body         BLOB NOT NULL,
  PRIMARY KEY (channel_id, idx)
);

CREATE TABLE push_registrations (
  device_id    TEXT PRIMARY KEY REFERENCES devices(device_id),
  provider     TEXT NOT NULL,
  token        TEXT NOT NULL UNIQUE,
  updated_at   INTEGER NOT NULL
);

CREATE TABLE push_health (
  id              INTEGER PRIMARY KEY CHECK (id = 1),
  last_success_at INTEGER,
  last_error_at   INTEGER,
  last_error      TEXT,                    -- error class only
  state           TEXT NOT NULL
);
```

The `push_health` table of `routing.db` is this one (global, not in a space's database). Pairing channels MAY remain
in memory (losing them at restart is harmless); the `pairing_channels` table is optional, **but** routing by
`channel_id` (§1.4) then needs its own list in memory. `push_registrations` is in the space's database: a device's
token is looked up in its space's database.

`routing.db`:

```sql
CREATE TABLE invitations (
  token_hash   BLOB PRIMARY KEY,                 -- SHA-256 of the normalized token (§3.1); the token is not kept
  created_at   INTEGER NOT NULL
);
CREATE TABLE space_quotas (                      -- `admin quota` overrides (§8.4)
  space_id     TEXT PRIMARY KEY,                 -- UUID text
  max_bytes    INTEGER                           -- NULL = no limit for this space
);
CREATE TABLE IF NOT EXISTS space_deletions (     -- deletion receipts (§4.4.2); additive, without changing user_version
  receipt_hash BLOB PRIMARY KEY,                 -- SHA-256(deletion_id); neither space_id nor device
  deleted_at   INTEGER NOT NULL                  -- purged after deletion_receipt_ttl_secs
);
-- push_health (above) and push_events(at INTEGER, kind TEXT): FCM sending health, global.
```

**Disk storage of blobs**: `<data_dir>/spaces/<space_id>/blobs/<first 2 hex characters>/<blob_id in hex>` (the files of
two spaces are never in the same tree; deleting a space deletes its folder). Writing a part: open for writing, position
at `Upload-Offset`, copy the body stream through 64 KiB buffers, and on error `set_len(Upload-Offset)`. There is no
separate temporary file: atomicity is ensured by truncation. `fsync` of the file on completion. Deletion: `remove_file`
after commit.

**Backup** (D43): stop the service or use `sqlite3 .backup` **on each database** (`routing.db` and the `relay.db` of each
space), then copy `blobs/` of each space. The content is fully encrypted; the metadata (§1.2) remains readable. A
**single space can be restored** without touching the others (its database and its blobs), `routing.db` not being a
source of truth for it. A restore moves that space's `head_seq` backwards: its clients detect this through
`cursor_ahead` (§4.10, §7.5).

## 11. Acceptance checks

Each scenario becomes an integration test (real relay on a temporary database, HTTP/WS test client, injectable
clock). "Member" = a device with a valid session, unless stated otherwise.

### 11.1 Space and installation token

- **A01** Given a fresh relay (no space, no invitation), when it starts, then an invitation is created and a line `install token:` logs its token, once only; at the next startup (without a space), the invitation still exists, **no token** is logged, and this token still creates a space; on a relay that has a space, no `install token:` line is written.
- **A02** Given a fresh relay, when `POST /v1/space` arrives with a wrong token, then `403 install_token_invalid`, no space (nor folder) is created and the invitation remains valid.
- **A03** Given a fresh relay, when `POST /v1/space` arrives with the right token and a valid self-signed certificate, then `201`, the log contains a single event, of type `device_added`, whose `seq` equals `head_seq`, the invitation is consumed, and `spaces/<space_id>/relay.db` exists.
- **A04** Given a created space, when `POST /v1/space` arrives with the **already used** token, then `409 space_exists`, whatever `space_id` is sent (that of the space or another), and no second space exists; with a fresh invitation but the `space_id` of the existing space, then `409 space_exists` and the invitation is not consumed; with a wrong token and that same `space_id`, then `403 install_token_invalid` (we do not learn which spaces exist).
- **A05** Given a fresh relay, when two valid `POST /v1/space` requests arrive in parallel with the same token, then exactly one `201` and one `409 space_exists`, and only one space exists.
- **A06** Given a fresh relay, when `POST /v1/space` arrives with a `DeviceCert` whose signature is wrong, that is not self-signed, or that belongs to another `space_id`, then `422 validation_failed` with `field = "/certificate"`, and the invitation remains valid, with no space folder created.
- **A07** Given a relay **that hosts no space**, when `POST /v1/auth/challenge`, a B route with any token, or `…/claim` arrives, then `409 space_not_initialized`; `GET /v1/hello` answers `space_initialized: false`. Given **a created space**, the same requests with an unknown identifier receive the unknown-identifier response — session token: `401 unauthorized`; `device_id`: challenge `200`; channel: `404` — and `hello` answers `space_initialized: true` (§1.4).

- **A08** Given a created space whose `201` was lost, when the founder resends the same request, then `409 space_exists`; their challenge-response (`auth/challenge` then `auth/session`) with their keys succeeds; the same holds after `admin invite --rotate` and after a relay restart. Given two devices with the same token, the second receives `409 space_exists` and its challenge-response fails with `401` (`pairing-v1.md` §10.2, P30).
- **A09** Given two spaces A and B, when `admin reset --space A` runs, then the token that had created A gives `403 install_token_invalid`, and that of B still gives `409 space_exists`; `admin invite --rotate` never gives `403` for a consumed token.

### 11.2 Authentication

- **A10** Given a member, when it signs the challenge according to §3.6, then `200` with a `tms1.` token and `expires_at = now + 3600 s`.
- **A11** Given a challenge already used (success or failure), when it is presented again, then `401 unauthorized`.
- **A12** Given a challenge older than 60 s, then `401 unauthorized`.
- **A13** Given a signature computed without the label `tomi/v1/sig/relay-auth\0`, with the `sign_pk` of another device, or on another `space_id`, then `401 unauthorized`.
- **A14** Given an unknown `device_id`, then `POST /v1/auth/challenge` answers `200` (as for a known one) and `POST /v1/auth/session` answers `401 unauthorized`.
- **A15** Given an expired session, when a B route is called, then `401 session_expired`.
- **A16** Given a token in the query string (`?token=...`) and no header, then `401 unauthorized` (the relay never reads the query string for authentication).
- **A17** Given 6 sessions opened successively by a device, then the first is invalid (`401`) and the other 5 are valid.
- **A18** Given `DELETE /v1/auth/session`, then the token is refused afterwards (`401 unauthorized`).

### 11.3 Revocation

- **A20** Given members A, B and C, with B connected by WebSocket, when A sends `POST /v1/rotations` with a `KeyRotation` to epoch 2 revoking B and sealing the key for A and C, then (1) `201` with `revoked_device_ids = [B]`, (2) B's WebSocket is closed with `4003` before the response to A, (3) B's next REST request receives `403 revoked`, including `POST /v1/auth/session`, (4) `GET /v1/space` shows `key_epoch = 2` and `rotation_required: false`, (5) the log contains `rotation` then `device_revoked` (`via: "rotation"`, `revoked_by = A`, `rotation_seq` = seq of the rotation).
- **A21** Given B revoked, when `POST /v1/devices` is attempted with a certificate whose subject has the same `sign_pk`, then `409 device_exists`.
- **A22** Given a single member, when it calls `DELETE /v1/me`, then `409 last_device`; given two members, then `200`, `rotation_required: true` and `device_revoked` with `via: "self"`.
- **A23** Given an accepted rotation, when the same bytes are sent again, then `200` with the same `seq` and no new event.
- **A24** Given `tomi-relay admin revoke <B>` while the server runs, then B's WebSocket is closed with `4003`, the event carries `via: "admin"`, `revoked_by: null`, and `rotation_required` becomes `true`; a later rotation listing B in `revoked` is accepted.
- **A26** Given a rotation whose `revoked` contains the caller's own key, or an unknown key, then `422`.
- **A25** Given B revoked, when B calls `GET /v1/blobs/{id}` for a blob it could read, then `403 revoked`.

### 11.4 Versioning

- **A30** Given a space whose active members have `format_max = 2`, when a new device (never connected) announces `format_max = 1`, then `409 update_required` with `effective_format = 2` and `update_url = <public_url>/updates/<platform>/latest.json`, and the device remains non-active.
- **A31** Given A (`format_max = 1`, active) and B (`format_max = 2`), then B's session returns `effective_format = 1` and `effective_format_limited_by = [A]`.
- **A32** Given A (`format_max = 1`) inactive for 31 days and B (`1..2`), then B's session returns `effective_format = 2`; then A's session receives `409 update_required`.
- **A33** Given a space at `effective_format = 1`, when a device announces `format_min = 2`, then `409 format_unsupported` with `limited_by`.
- **A34** Given a request to `/v0/hello`, then `410 update_required`; to `/v9/hello`, then `404 not_found`.

### 11.5 Items, idempotence, acknowledgements

- **A40** Given a member, when it sends a valid item, then `201` with a `seq` equal to the new `head_seq`, and the other members see the `item` event with the envelope identical byte for byte.
- **A41** Given an item already accepted, when the same `POST /v1/items` is replayed, then `200`, the same `seq`, and no new event (`head_seq` unchanged).
- **A42** Given an item already accepted, when the same `item_id` arrives with another envelope, then `409 item_id_conflict`.
- **A43** Given an item accepted and then purged, when it is replayed identically, then `200` with `purged: true`, without a new event.
- **A44** Given a rotation to epoch 2, when an item arrives with `key_epoch = 1`, then `409 stale_key_epoch` with `key_epoch = 2`.
- **A45** Given an `expires_at` in the past, then `422 already_expired`; at `now + max_ttl_secs + 1 s`, then `422 validation_failed`.
- **A46** Given an envelope of `max_envelope_bytes + 1` bytes, then `413 payload_too_large`.
- **A47** Given a referenced blob that is not complete, then `409 blob_incomplete`; a blob of another device, then `404`; an already attached blob, then `409 blob_attached`.
- **A48** Given an item received by B, when B `ack`s twice, then a single `ack` event; when B does `read`, then a `read` event and no second `ack`.
- **A49** Given the sender of an item, when it does `ack` or `read` on its own item, then `200` without event.
- **A50** Given a device registered after the item (`joined_seq ≥ item.seq`), then `GET /v1/items/{id}`, `ack`, `read` and the blob downloads answer `404`, and `GET /v1/events?after=0` starts only after its `joined_seq`.

### 11.6 Log and synchronization

- **A60** Given 250 events, when a client reads with `limit=100` following `next_after`, then it obtains all 250 in order, without duplicates, in 3 pages, and `has_more = false` on the last one.
- **A61** Given a client connected by WebSocket, when the relay restarts while 3 items are being sent, then after reconnection and catch-up the client has the 3 items, each exactly once.
- **A62** Given an item purged between two reads, then its event does not appear and `next_after` moves past the gap without error.
- **A63** Given events older than 45 days that have been compacted, when a device reads with an `after` lower than `retention_floor_seq`, then `410 cursor_expired`; with `after ≥ retention_floor_seq`, then `200`.
- **A64** Given `after > head_seq` (restored database), then `409 cursor_ahead` with `head_seq`.
- **A65** Given 10 items with 409,600-byte envelopes, when reading with `limit=100`, then the first page contains exactly 5 `item` events (the 6th would push the sum beyond 2,097,152) and `has_more = true`.
- **A66** Given an unknown event type injected into the log, then the reference client (`core`) advances its cursor without error.

### 11.7 WebSocket and ring

- **A70** Given a WebSocket connection with no `auth` message for 10 s, then close `4001`.
- **A71** Given an `auth` message with a valid token, then `ready` with `head_seq`; then, for each new event from another device, a `wake` with a `seq` ≥ that of the event.
- **A72** Given a URL `/v1/ws?token=<valid token>` with no `auth` message, then the connection is not authenticated (close `4001` after 10 s).
- **A73** Given PC1 present and PC2 absent, when the phone sends an item, then the event read by PC1 has `ring: true` and the one read by PC2 has `ring: false`; the same applies in the `wake` messages.
- **A74** Given no recipient present, then `ring: true` for all recipients and `false` for the sender.
- **A75** Given a presence signalled 121 s ago with no refresh, then the device is no longer present.
- **A76** Given an Android device with no WebSocket and an FCM token, when an item that must make it ring arrives, then exactly one call to `PushSender::wake(High)`; for a `read`, `wake(Normal)`; for an `ack`, no call. With an open WebSocket, no FCM call.
- **A77** Given 5 items within 1 s for the same device, then at most 2 FCM calls (2 s anti-burst).
- **A78** Given an FCM `UNREGISTERED` response, then the device's token is deleted and `push = false` in `GET /v1/devices`.
- **A79** The body sent to FCM, captured by a fake server, contains `"data": {"w": "1"}` and no variable field other than `token` and `priority`.

### 11.8 Blobs

- **A80** Given a 3 MiB blob sent in 1 MiB parts, when the connection drops in the middle of the 2nd part, then `status` returns `upload_offset = 1048576`; resuming from that offset completes the blob, and the download yields exactly the bytes sent.
- **A81** Given an `Upload-Offset` that differs from the state, then `409 upload_offset_mismatch` with the correct `upload_offset`.
- **A82** Given `Range: bytes=100-199`, then `206`, 100 bytes, `Content-Range: bytes 100-199/<size>`; `Range: bytes=0-1,5-6`, then `416`.
- **A83** Given an unattached blob for 24 h, then it is deleted (database and disk) and the quota freed.
- **A84** Given `size > max_blob_bytes`, then `413`; given a remaining quota lower than `size`, then `507 relay_full` with `used_bytes` and `max_bytes`.
- **A85** Given a 25 MiB blob sent in 25 parts, then the downloaded blob is identical and the part handler streams the body to disk (verified by code review: no `Bytes`/`Vec` the size of the part or of the blob).

### 11.9 Purge

- **A90** Given A as sender and B and C active, when B and C `release`, then the item is purged `release_grace_secs` after the last `release` (not before): `GET /v1/items/{id}` → `404`, blob files absent, `item` event absent from the log.
- **A91** Given B active and C inactive for 31 days, when B `release`s, then the item is purged after the grace period without waiting for C.
- **A92** Given B who `ack`ed without `release`, then the item remains until `expires_at`, then is purged.
- **A93** Given a valid tombstone, then the item and its blobs are purged immediately and the `tombstone` event is in the log; a second tombstone for the same item answers `200` with the same `seq`.
- **A94** Given a tombstone for a never-seen `item_id`, then `201` and a `tombstone` event.
- **A95** Given a tombstone with an invalid signature, with local scope, or signed by a device other than the caller, then `422` and no purge.
- **A96** Given a single-device space, when it sends an item, then the item is purged `release_grace_secs` after its creation.
- **A97** Given a purged item, then `used_bytes` decreases by the size of the envelope and of the blobs.

### 11.10 Pairing

- **A100** Given a member A, when A creates a channel, then `channel_id` is a string of 1 to 4 digits and `state = open`.
- **A101** Given an `open` channel, when X `claim`s it, then `200` with a `tmp1.` token; when Y `claim`s it afterwards, then `409 channel_claimed`.
- **A102** Given a joined channel, when A and X exchange messages, then each receives only those of the other, in order; a `GET` with `wait=10` and no message answers after 10 s (± 1 s) with an empty list, or as soon as a message arrives.
- **A103** Given a channel not joined for 121 s, then `claim` → `404`.
- **A104** Given the 9th message from the same side, then `409 channel_limit`; a message of 8193 bytes, then `413`.
- **A105** Given a joined channel, when A does `POST /v1/devices` with a certificate signed by A, then `201`, a `device_added` event, channel `completed`, and X can open a session.
- **A106** Given a certificate issued by a device other than the caller, then `422`; a channel created by another member, then `404`; a `key_epoch` different from the current one, then `409 epoch_conflict`. Given a successful registration, the `device_id` returned equals `hex(SHA-256("tomi/v1/device-id" || 0x00 || sign_pk)[0..16])`.
- **A107** Given a Pairing token, when it is used on a B route (e.g. `GET /v1/events`), then `401 unauthorized`.
- **A108** Given a `completed` or `closed` channel, when one side does `GET .../messages?wait=10`, then the response arrives without waiting (well before 10 s): the state, and in `completed` the remaining messages of the other side, none in `closed`. On a `claimed` channel with no message, the same read lasts `wait` seconds (A102).

### 11.11 Rotation

- **A110** Given members A, B and C, when A sends a rotation to epoch 2 revoking C with seals for A and B, then `201`, `rotation_required = false`, and the events `rotation` then `device_revoked` (C).
- **A111** Given members A, B and C, when A sends a rotation revoking C but also sealing for C, or omitting B, then `409 device_set_mismatch` with `expected_device_ids = [A, B]` (sorted).
- **A112** Given two concurrent rotations to epoch 2, then only one succeeds, and the other receives `409 epoch_conflict` with `key_epoch = 2`.
- **A113** Given a rotation, then B's `GET /v1/keys` returns it, and that of C (revoked) answers `403 revoked`.

### 11.12 Logs, distribution, administration

- **A120** Given a complete test suite run with `RUST_LOG=trace`, then the logs contain none of the session tokens, Pairing tokens, challenges, signatures, FCM tokens, envelopes or part bodies used (textual search of each value, in base64url and in hexadecimal), nor the installation token outside the `install token:` line.
- **A121** `GET /updates/../config.toml` and `GET /updates/%2e%2e/x` → `404`; `GET /updates/android/` (directory) → `404`.
- **A122** `GET /l/pair` → the page `l/index.html` with the CSP header of §4.12.
- **A123** `GET /healthz` → `200 {"status":"ok"}`; database inaccessible → `503`.
- **A124** Given a single-space relay, `admin reset --confirm <8 characters>` → the space's WebSockets closed with `4004`, its folder deleted, `GET /v1/hello` → `space_initialized: false`, a new token is displayed (the other pending invitations are kept; those the deleted space had consumed are forgotten, A09), and `POST /v1/space` with this token succeeds. Without confirmation, or with a wrong confirmation → code 2, nothing is deleted.
- **A125** `admin invite` → lists the invitations (never a token); `--new` → displays a token that creates a space, including on a relay that already has one; `--rotate` → replaces all **pending** invitations with a single one; old pending tokens give `403 install_token_invalid` (a token already consumed always gives `409 space_exists`, A08).
- **A126** `admin status` with an invalid FCM service account file → health `auth_error`, and the relay serves other requests normally.
- **A127** No network route allows `revoke` without a member session, nor `reset` of a space that has other members, nor `quota`, nor reading or creating an invitation (verified by walking the routing table). The only space deletion possible over the network is `DELETE /v1/space`, reserved for the only active member of the space (A177–A183).

### 11.13 Rate limits

- **A130** Given 11 instant `POST /v1/auth/challenge` from one IP, then the 11th receives `429 rate_limited` with `Retry-After ≥ 1`.
- **A131** Given a request with `X-Forwarded-For: 1.2.3.4` coming from an address outside `trusted_proxies`, then the limit applies to the TCP address, not to `1.2.3.4`.
- **A132** Given a joined channel, when the holder of its Pairing token sends from one same IP more requests than the `rl_ip_public` burst (8 messages, a 9th refused, reads, a `DELETE`), then none is limited per IP: the IP's `rl_ip_public` budget is the same as before these requests.
- **A133** Given a Pairing token that is not the channel's (unknown, of another channel, unknown channel, expired or never joined, invalid shape), when 11 requests arrive from one IP on one of the routes `POST messages`, `GET messages` or `DELETE`, then the first ten receive the route's response (`401` or `404`) and the 11th receives `429 rate_limited`. Without a space, the first ten receive `409 space_not_initialized`: the per-IP limit precedes the space check (§2.5).
- **A134** Given an IP whose `rl_ip_public` is exhausted by bogus Pairing tokens, when the holder of a valid token sends a request from this IP, then it is not limited, and the bogus token still is.
- **A135** Given the holder of a valid Pairing token, when it exceeds `rl_device` (key: the channel), including through refused requests (a 9th message) or a `DELETE`, then `429 rate_limited`; the inviter and the other channels are not affected, and the bucket refills over time.
- **A136** Given a joined channel, when one side makes 11 instant reads, then the 11th receives `429 rate_limited` (`rl_pairing_poll`), without the other side being affected.

### 11.14 History transfers and relay deadline

Test manifests are built with `tomi-core` (`TrustStore::create_history_transfer`); those of
`spec/vectors/v1/history-transfer.json` may be used as they are if the test space reuses its `space_id`
and its devices.

- **A140** Given members A, B and C, when A deposits a valid transfer to B (`POST /v1/transfers`), then `201`, a `history_transfer` event (`to_device_id = B`) and a doorbell for B only; B reads `GET /v1/transfers/{id}` and downloads each of its blobs; C receives `404` on the transfer and on each of its blobs.
- **A141** Given a manifest signed by a device other than the caller, from another `space_id`, with a `transfer_id` different from the body's, or whose recipient is not a member or is the caller, then `422 validation_failed` with `field = "/manifest"`.
- **A142** Given `as_of_seq < joined_seq` of the recipient, or `as_of_seq > head_seq`, or a part of the manifest absent from `blob_ids`, then `422` (`/manifest`).
- **A143** Given an accepted transfer, when the same request is replayed, then `200` with the same `seq`, without a new event; when the same `transfer_id` arrives with another manifest or another set of blobs, then `409 transfer_id_conflict`.
- **A144** Given an open transfer, when B does `DELETE /v1/transfers/{id}` with `outcome: "imported"`, then `204`, the blobs have disappeared (database and disk), `used_bytes` has decreased by their size, a `history_transfer_closed` event (`by_device_id = B`, `outcome = "imported"`) is added, then `GET` → `404` and a second `DELETE` → `204` without event.
- **A145** Given a transfer deposited more than `transfer_ttl_secs` ago, then it is purged (blobs absent, `history_transfer_closed` event with `by_device_id: null`, `outcome: "expired"`). Given the revocation of B or of A, then the transfer is purged in the revocation transaction (`outcome: "revoked"`).
- **A146** Given a sum of blobs > `max_transfer_bytes`, then `413`; a quota exceeded at blob creation, then `507 relay_full`; a 3rd open transfer to B, then `409 transfer_limit` with `transfer_max_open = 2`.
- **A147** Given a blob attached to a transfer, when it is referenced by `POST /v1/items` or deleted by `DELETE /v1/blobs/{id}`, then `409 blob_attached`; given a blob attached to an item, when it is listed in `POST /v1/transfers`, then `409 blob_attached`.
- **A148** Given a `POST /v1/transfers` body with extra fields (for example `include_secrets: false`), then they are ignored: the relay has no notion of partial inclusion.
- **A149** Given a rotation to epoch 2 running concurrently with `POST /v1/items` at epoch 1, then no accepted epoch-1 item has a `seq` greater than that of the `rotation` event (`stale_key_epoch` check in the transaction that assigns the `seq`, §4.8).
- **A150** Given a reference client (`core`) that sends an item with no user expiration, then the `expires_at` sent equals `server time + max_ttl_secs − 60 s` (± 1 s) and the item is accepted; with a user expiration at 1 h, then the `expires_at` sent equals that expiration.

### 11.15 Pin (item-state events)

Test envelopes are built with `tomi-core` (`PinBook::set_pinned`, `seal_item_state`); those of
`spec/vectors/v1/pin.json` may be used as they are if the test space reuses its `space_id` and its devices.

- **A151** Given members A and B, when A deposits a valid pin (`POST /v1/item-states`), then `201` with a `seq`, an `item_state` event (`event_id`, `device_id = A`, `key_epoch`, `envelope` identical byte for byte) and a `NORMAL` doorbell for B only; the relay's database contains the targeted `item_id` nowhere (neither `events.item_id` nor `data`).
- **A152** Given an accepted pin, when the same request is replayed, then `200` with the same `seq`, without a new event; when the same `event_id` arrives with another envelope or from another device, then `409 event_id_conflict`.
- **A153** Given `key_epoch` ≠ the current epoch, then `409 stale_key_epoch` without recording anything; the same `event_id` is then accepted with the correct epoch. Given a format 1 envelope whose bytes 4–7 or 8–23 do not equal `key_epoch` or `event_id`, then `422` (`/envelope`); given a format 1 envelope of 24, 319, 321, 577 or 2,368 bytes (length ≠ `48 + 256·k + 16`, `1 ≤ k ≤ 8`; 2,368 = `k = 9`), then `422` (`/envelope`) without recording anything, while 320 or 2,112 bytes are accepted; more than `max_item_state_bytes` bytes, then `413`.
- **A154** Given a pinned item (`item_state` event present), when all active devices have `release`d, then the item is purged after `release_grace_secs` exactly like an unpinned item; the `item_state` event remains in the log until `event_retention_days`.
- **A155** Given a revoked device, then its `POST /v1/item-states` receive `403 revoked`; given a pin signed by a since-revoked device and replayed by a malicious relay, then the reference client ignores it (`TrustStore::verify_pin` → `revoked_device`) without changing its state. Accepted exception (`format-v1.md` §8.8.2): a legitimate pin kept raw (`pending_states`) and reopened after the rotation that revokes its signer is also refused, even if its `seq` precedes the rotation; a `KeyPending` pin retried just after the rotation that brings its key (§7.3) is however verified before any later revocation and applied.

### 11.16 Several spaces on one relay

Each scenario starts from a relay and two spaces A and B (two invitations, two founders).

- **A160** Given A and B, when a member of B calls `GET /v1/items/{id}`, `ack`, `read`, `GET`, `HEAD` or `DELETE /v1/blobs/{id}`, or attaches to an item the `blob_id` of a blob of A, using valid identifiers **of A**, then `404 not_found`; the log and the device list of B contain nothing from A; renaming a device of A from B → `404`. The same `item_id` and `blob_id` are free in B (`201`), without A knowing; a tombstone from B for an item of A is accepted (`201`) and purges nothing at A.
- **A161** Given A and B, when an item of A is sent, then a device of A receives `wake` on its WebSocket and a device of B receives nothing; the FCM doorbell of a device of A looks for its token only in A.
- **A162** Given A and B, when the challenge requested for a device of A is signed with the `space_id` of B, or when the challenge of a device of B is presented for a device of A, then `401 unauthorized`; a `device_id` unknown to all receives a challenge like a known one, then `401` at the session.
- **A163** Given a pairing channel of A, when a member of B writes to it, reads it, closes it, or registers a device there (`POST /v1/devices`), then `404`; the `Pairing` token obtained by `claim` opens only that channel (`401` on a channel of B); the channel number is never given twice on the relay, even when A occupies half of them (B's numbers are all outside A's).
- **A164** Given A and B, when B registers (`POST /v1/devices`) a device key of A — member or revoked — or when a third space is created with such a key, then `409 device_exists` (the same error as for a duplicate within the space), and the invitation is not consumed; two simultaneous registrations of the same key in A and in B: one `201` and one `409`.
- **A165** Given A and B, when `admin reset --space A` runs, then A's WebSockets are closed with `4004`, B's remain open, A's folder disappears, and B responds as before (session, items, blobs); A's tokens receive `401` (not `space_not_initialized`), a `device_id` of A receives a challenge then `401`; no invitation is created; A's keys become free again; the invitations consumed by A are forgotten (A09). Without `--space`, `admin reset` refuses (code 2) and lists both spaces; a confirmation that is B's does not delete A.
- **A166** Given `max_relay_bytes = 10 000`, A using 6,000 and B 3,000, when B reserves 2,000, then `507 relay_full` with `used_bytes = 3000` and `max_bytes = 4000`, and `GET /v1/space` gives the same numbers (`quota` of A: 6,000 and 7,000); when A reserves 1,000, then `201`, then one more byte → `507`; when B frees a blob, A can reserve again. The responses never contain the usage of another space.
- **A167** Given `max_relay_bytes = 10 000`, two empty spaces, when each reserves 6,000 at the same instant, then exactly one `201` and one `507` (repeated over several fresh relays).
- **A168** Given no configured quota, when A reserves 40 MB, then `201` (no per-space limit); after `admin quota --space A 30000000`, any reservation of A → `507` with `max_bytes = 30000000` and `used_bytes` = 40,000,000, and B is untouched; `quota none` and `quota default` lift it; an invalid value (`0`, `-5`, `lots`) → code 2, an unknown space → code 1, several spaces without `--space` → code 2; the override survives a restart. With `max_space_bytes = 5000` in the configuration, each space is limited to 5,000, `quota none` lifts the limit of one without touching the other, `quota <n>` replaces it, and `quota default` restores the configuration's value.
- **A169** Given A and B, when the purge runs (A: expired item, B: non-expired item), then only A's item is purged and only A's blob file is deleted; compaction applies to both logs; A's expired sessions and expired channels also leave the routing (§1.4); `DELETE /v1/auth/session` removes the token from the routing immediately.
- **A170** Given A (configured quota of 10,000 bytes) at 95 %, when the purge runs, then a `WARN` names A by the first 8 characters of its `space_id`, at most once an hour, and no space below 90 % is named; given a total above 90 % of `max_relay_bytes`, then a "total" `WARN` without a space name. Never the full `space_id` in the logs.
- **A171** Given two spaces, when the relay restarts, then each space's sessions, devices and open channels still route to their space; without `routing.db` (deleted), likewise (only the invitations and quota overrides are lost); if a space database contains a `device_id` of another space, the relay refuses to start, without writing a full identifier into the message.
- **A172** Given a `data_dir` in the old layout (`tomi.db`, `blobs/`) of a space with a session, an item and a blob, when the relay starts, then everything is under `spaces/<space_id>/`, `tomi.db` and `blobs/` have disappeared, the session, item and blob respond as before, and a second space can be created alongside; `spaces.migrating/` present, or `tomi.db` and `spaces/` together, or blobs without a database: startup is refused without changing anything; an `install-token` file becomes a valid invitation.
- **A173** Given `admin spaces`, `admin status`, `admin status --space`, `admin devices --space` and `admin revoke --space`, then: `spaces` lists both spaces (members, usage, quota); `status` gives the number of spaces, the total, the disk and the invitations, without space details; `status --space` gives the space; `devices` without `--space` refuses (code 2) and lists the spaces; `revoke` of a device of A with `--space B` → code 1 and nothing is revoked; a single-space relay does not require `--space`.
- **A174** Given the `data_dir` of relay 0.1.1 in production (`tomi.db` of schema 1 **without a `space` row**, with `-wal` and `-shm`, an empty `blobs/`, `install-token` with a pending token, `public/`, `relay.lock`, `admin.sock`, `config.toml`, `smoke.log`), when the new relay starts, then startup succeeds; `tomi.db`, `-wal`, `-shm`, `blobs/` and `install-token` have disappeared; `public/`, `config.toml`, `smoke.log`, `admin.sock` and `relay.lock` are intact; `GET /v1/hello` answers `space_initialized: false`, and `POST /v1/space` with the token of the old file answers `201`.
- **A175** Given the old layout, when a read-only `admin` command runs without holding `relay.lock`, then it refuses (with a message saying that the relay must be started, which performs the migration) and touches nothing; given the new layout, a read-only command does not create `routing.db`, does not delete any row (an orphan quota override remains), and does not change any byte of the databases.
- **A176** Given a space with a blob write in progress, when `admin reset --space` deletes it, then `spaces/<id>/` no longer exists and the write does not recreate it; a `spaces.deleted-*` left by an abrupt stop is deleted at the next startup.

### 11.17 Deletion of the space by its last device

- **A177** Given a space with a single member, with an item, a blob, an open WebSocket, a quota override and a pending invitation made for someone else, when this member calls `DELETE /v1/space`, then `200 {"space_deleted": true}`, the WebSocket is closed with `4004`, `spaces/<id>/` no longer exists (nor `spaces.deleted-*`), the quota override is forgotten, the invitation the space had consumed is forgotten (its token gives `403 install_token_invalid`), the pending invitation is kept, and **no invitation is created**. On a relay with no other space: `GET /v1/hello` → `space_initialized: false`, and the old session or its `device_id` receive `409 space_not_initialized`. The member's keys can found a new space with a pending invitation.
- **A178** Given a relay that has become space-less and without a pending invitation after A177, when it makes its startup announcement (or `admin invite --new`), then a token is created and `POST /v1/space` with it succeeds; a second announcement creates none (§3.1).
- **A179** Given two members, when either one calls `DELETE /v1/space`, then `409 not_last_device`, the log's `seq` does not move, and the space is intact. Given one of the two has departed (`DELETE /v1/me`) or been revoked (`admin revoke`), then it receives `403 revoked` at `DELETE /v1/space` and the space remains; the remaining member, alone, gets `200`, and `DELETE /v1/me` of the last member still answers `409 last_device`.
- **A180** Given `DELETE /v1/space` without a token, with a wrong token, with a token in the query string, with a `Pairing` token, or with an expired session, then `401` (`unauthorized` or `session_expired`) and nothing is deleted.
- **A181** Given two spaces A (one member) and B (two members), when A's member calls `DELETE /v1/space`, then A's WebSockets are closed with `4004`, B's remain open, B responds as before (devices, items, blobs, `seq`) and keeps its rule (`DELETE /v1/space` of a member of B → `409 not_last_device`); A's tokens receive `401` (not `space_not_initialized`); no invitation is created.
- **A182** Given a single member who calls `DELETE /v1/space` twice in parallel, then exactly one `200`, and the other receives `409 space_not_initialized` (or `503 unavailable` if it had already resolved the space); nothing is half-deleted. Given two members, one of whom calls `DELETE /v1/me` while the other calls `DELETE /v1/space`, then the departure always answers `200`, and the deletion answers `200` (the departure took place first) or `409 not_last_device` (it was judged first), never anything else; the space is deleted only in the first case.
- **A183** Given the only member calling `DELETE /v1/space` while it registers a new device (`POST /v1/devices`), then either the new device is registered (`201`) and the deletion is refused (`409 not_last_device`, the space counts two members), or the space is deleted (`200`) and the registration fails; never `200` and `201` together, never a deleted space that had gained a member.

### 11.18 Fail-closed deletion and receipt

- **A184** Given the only member calls `DELETE /v1/space` with `{"deletion_id": X}` and succeeds, then `POST /v1/space/receipt {"deletion_id": X}` without authentication → `200 {"space_deleted": true}`, twice in a row; another 32-byte identifier → `404 not_found`; a 31-byte identifier → `422`; after `deletion_receipt_ttl_secs` (test clock) → `404`. On a relay that no longer hosts any space, the route never answers `409 space_not_initialized`. `routing.db` contains only `SHA-256(X)` and the time.
- **A185** Given `DELETE /v1/space` with no body or with `{}`, then `200 {"space_deleted": true}` and no receipt is written. Given a body `{"deletion_id": "abc"}` or a `Content-Type` other than JSON, then `422` or `415` and nothing is deleted (the space responds as before).
- **A186** Given a space whose `spaces/<id>/` folder was moved out of `data_dir` by the test (simulated unmounted volume) after startup, when its last member calls `DELETE /v1/space`, then the response is **never** `200` (`500` or `503`), no receipt exists, `hello.space_initialized` stays true, and once the folder is put back and the relay restarted, the space is whole (members, items, blobs, quota, consumed invitation). The same expectation applies if `relay.db` was replaced by a folder, or deleted by the test just before the call.
- **A187** Given a fault injected on the `fsync` of the folders after the rename, then `500`, no receipt, no `4004` close, and the relay answers `200` to no retry during that startup. Given a fault injected on the receipt transaction, then `500`, the receipt answers `404`, the space's tokens then receive `401` (or `409 space_not_initialized` on an empty relay), and the consumed invitation and the quota remain in `routing.db` after a restart.
- **A188** Given a deletion in progress (the test holds the registry lock after the routing removal) or a space left "pending", then for its devices: `POST /v1/auth/challenge` and `POST /v1/auth/session` → `503 unavailable` (never `401`), any **B** route → `503`, and a WebSocket that opens → closed with `1013`; `POST /v1/space` with this `space_id` and a valid invitation → `503`, and `spaces/<id>/` is intact.
- **A189** Given a deletion that fails without deleting anything (rename and in-place deletion refused), then the space's open WebSockets do **not** receive `4004` and remain open; a successful deletion closes them with `4004` only after the receipt is written.
- **A190** Given the same device calling `DELETE /v1/me` twice in parallel in a two-member space, then exactly one `200` with `seq`, and the other `403 revoked` (never `401`); a third call and `POST /v1/auth/session` → `403 revoked`.
- **A191** Given a relay that starts while a space referenced by a consumed invitation and a quota override has no folder any more, then the relay starts, logs a warning, and keeps the invitation and the quota in `routing.db`.
- **A192** Given a pending space, then `admin status` counts one pending space and gives the same `space_initialized` as `GET /v1/hello`.
- **A193** Given `require_existing_data_dir = true` and an empty `data_dir`, then the relay refuses to start (non-zero exit code) without creating anything; with a `data_dir` containing `routing.db`, it starts.
- **A194** Given a space whose folder exists under `spaces/<id>/` with a `relay.db` holding a `space` row but which has not been loaded (simulated), when `POST /v1/space` arrives with this `space_id`, then `409 space_exists` and the folder is intact.
- **A195** Given a relay whose data cannot be loaded faithfully (`data_dir` empty while `require_existing_data_dir` is true, `spaces/<id>` or `relay.db` a symbolic link, an unreadable space folder, `spaces/` whose target is missing, an unreadable database, a database of another space), then it refuses to start with exit code **78**, without creating any space or token (`relay.lock` and `routing.db` may have been created before the refusal); any other startup failure (a lock really held by another relay, an invalid configuration) exits with `1`. The message of a database that does not open contains neither a path nor the full `space_id` (§10.1).

### 11.19 Removal of an attachment (`POST /v1/attachment-tombstones`)

Test declarations are built with `tomi-core` (`DeviceKeys::sign_attachment_tombstone`); those of
`spec/vectors/v1/attachment-tombstone.json` may be used as they are if the test space reuses its `space_id` and
its devices.

- **A196** Given an item of two blobs and a valid removal of attachment `slot = 1`, then `201` `{seq, item_id, slot}`; blob 1 is deleted (file erased, quota freed); the item and blob 0 remain; and an `attachment_tombstone` event (`item_id`, `slot`, `blob_id`, `device_id`, `deleted_at`, `statement`) is in the log. The same removal replayed, from any device, answers `200` with the same `seq`, without a new event; the `item` event keeps its list of blobs.
- **A197** Given a removal with an invalid signature, signed by a device other than the caller, from another space, a removal of an item tombstone, arbitrary or empty bytes, or a body without the field or with an extra field, then `422 validation_failed` (`field = "/attachment_tombstone"` for a declaration), no blob deleted, no event, no row.
- **A198** Given a blob well attached to the item but at another position, the blob of another item, another blob at the targeted position, or an unknown `blob_id` while another blob occupies that position, then `422`, and nothing is deleted or recorded.
- **A199** Given an unknown `item_id` or an absent blob, or an item already purged or tombstoned, then `201` and an `attachment_tombstone` event (no special case: clients ignore the event of a tombstoned item).
- **A200** Given an item purged from the relay (item tombstone, or delivery), then the `attachment_tombstones` rows of its attachments are kept and a replay answers `200` without a second event; after `event_retention_days`, compaction deletes the event and its row.
- **A201** Given an up-to-date relay, then `GET /v1/hello` and `SpaceState` (`GET /v1/space`) carry `features = ["attachment_tombstones"]`; given a space database at schema 1 without the table, then opening it for writing creates the table (additive migration, `user_version` unchanged), and a second opening changes nothing.
- **A202** Given a request without a session token, then `401 unauthorized` and nothing is deleted.

## 12. Annex B (non-normative): decisions made in this document, trade-offs, open points

### B.1 Decisions the documents did not settle

| # | Decision | Reason | Alternative rejected |
|---|----------|--------|---------------------|
| 1 | **Two acknowledgements**: `ack` (received, feeds "delivered to X") and `release` (the relay may erase). | With on-demand file download (D20), "received" and "no longer needs the relay" differ: purging on receipt would make the photo unavailable on tap. | A "delivered" status derived from the cursor (`PROJECT.md` §3): an advanced cursor does not prove that the envelope was stored. See B.3. |
| 2 | The `ring` decision is **stored per recipient** and read over REST in the `item` event; the FCM message stays constant. | FCM sees no variable field. The `ring` of the WebSocket `wake` is only a shortcut. | Putting `ring` in the FCM message: Google would learn the bit in clear in the content. It learns it anyway through the `HIGH` priority (an accepted trade-off for reliability under Doze). |
| 3 | FCM `data = {"w": "1"}` rather than an empty `data`. | It has not been verified that an HTTP v1 message with no payload at all is accepted; a constant value reveals nothing. | `data: {}`: to be tested, and adopted if FCM accepts it. |
| 4 | **1-hour session, not extendable**; renewal through a new challenge-response. | Signing is free; there is no long-lived refresh token to protect. | A long refresh token: one more secret to store. |
| 5 | WebSocket authentication by **first message**. | No token in the URL; the `Sec-WebSocket-Protocol` subprotocol is not masked by default in Caddy's logs. | An `Authorization` header on the upgrade: possible for our Rust clients, but the first message works everywhere. |
| 6 | **Short numeric** `channel_id` (1 to 9999), not secret. | A code typed by hand (`7-crayon-lune`) must contain the channel number. Security comes from the PAKE and the single claim. | An opaque 128-bit identifier: impossible to type. The original design asked for an "opaque" `channel_id`: it is opaque to the relay (meaningless), but not long. |
| 7 | Registration of the new device is done by **the inviter**, bound to the `claimed` channel and to the **current epoch**. | The relay checks that the inviter is a member and signs; binding the epoch prevents a device from joining with a key already replaced by a concurrent rotation. | Registration by the new device itself with a certificate: requires a second kind of token. |
| 8 | The **device name is in clear** for the relay. | `admin devices` and `admin revoke` over SSH (fallback D39) must show names; a device name is not very sensitive. | A name encrypted with the space key: the admin would only see UUIDs. **`docs/DEV.md` invariant 1 does not list the name**: see B.3. |
| 9 | An item's `created_at` = **relay time**. | Does not depend on the devices' clocks; the capture time stays inside the ciphertext. | |
| 10 | An item's `key_epoch` **equal** to the current epoch, otherwise `stale_key_epoch`. Rotation covers **all** members, inactive ones included. | After a revocation, no new item uses a key known to the revoked device. An inactive member must recover the key on its return. | Accepting the old epoch for a period: reopens the window that the rotation closes. |
| 11 | Tombstone accepted for **any** `item_id`. | Most "delete everywhere" operations arrive after the relay has purged the item; local copies must still be erased. | Requiring a known item: the deletion would no longer propagate after the purge. |
| 12 | The relay **hides** from a new device the events prior to its arrival (`joined_seq`). | History goes through a sealed transfer for the sole recipient (§4.13, decision D2), never through the log: the relay replays nothing and distributes no old key. | Replaying the log with old epoch keys: all the history at rest on the relay, readable by anyone holding an old key. |
| 13 | Log retention of **45 days** (> 30 days of inactivity); `cursor_expired` beyond that; `seq` timestamped and `cursor_ahead` for restores. | An active device never misses a tombstone; restoring a backup skips no new event. | `seq` as a simple counter: after a restore, a client whose cursor is ahead would skip tombstones. |
| 14 | `update_required` as **409** (format) or **410** (retired API version). | Clients decide on `code`. `426` requires an `Upgrade` header that has no purpose here. | |
| 15 | Default values: relay total 1 GiB (no per-space quota), max TTL 30 days, envelope 1 MiB + 64 KiB (format v1 allows a 1 MiB payload), 20 devices, session 1 h. | A small EC2 instance, personal use. All configurable. | |
| 16 | **One blob per attachment** (as `core` foresees). | File-by-file download, 25 MiB limit verifiable by the relay. | One blob per item: the relay would only see one size. Cost: the relay knows the number and encrypted size of each file. |
| 17 | **Revoking = publishing a `KeyRotation` whose `revoked` list is not empty** (`POST /v1/rotations`); `DELETE /v1/me` to leave; `admin revoke` for the administrator. No separate revocation declaration. | `core` already puts the list of revoked devices in the signed rotation: revocation and rotation become atomic, and the signed proof of revocation is the rotation itself. | A signed `revoke` endpoint followed by a rotation: two steps, with a window where a revocation happens without a rotation. |
| 18 | `admin invite` = **invite a person to create a space** (single-use token, only its fingerprint kept); it never adds a device to an existing space. | Without a member device, the space key cannot be handed over; adding a device goes through pairing. | An `invite` that adds a device to an existing space: impossible without breaking E2EE. |
| 19 | **Automatic** rotation by any member after a random delay if `rotation_required` stays true. | Covers revocation by the administrator and by a revoker that went down. `epoch_conflict` settles concurrent attempts. | |
| 20 | Purge: only devices **that have opened a session** within the last 30 days count; if nobody else counts, purge after the grace period. | A device that is registered but never connected (e.g. blocked by `update_required`) must not block the purge for 30 days. | |
| 21 | The relay **depends on `tomi-core`** to decode and verify `DeviceCert`, `KeyRotation`, `Tombstone`. Only the challenge signature is defined here, with `core`'s convention (label, null byte, fixed-size fields). | A single encoding of the declarations, verified by the same code on the relay and on the clients. | Defining API-specific signatures: two competing formats for the same facts. |
| 22 | `ack` messages do **not** trigger FCM. | Avoids waking the phone for every delivery of an item it sent; the status updates at the next sync. | |
| 23 | Format of the `updates/android/latest.json` manifest defined here. | A contract common to the android and infra areas was needed; the relay only serves it. | |
| 24 | A joined channel gets a **single extension** of 120 s. | Leaves time to finish the exchange after a slowly typed code. | |
| 25 | `expires_at` of `POST /v1/items` remains **required** and becomes the **relay retention deadline**; the client sends `min(payload expiration, server time + max_ttl_secs − margin)` (D1: no default expiration). | No change to the relay, to validation, nor to A45; the relay no longer learns the real expiration beyond 30 days. | Making the field optional with a relay-side default: same result, more changes. |
| 26 | History transfer **everything included, no partial option** (§4.13). | Decision D2 (project decision): pairing is already the moment of full trust (the new device receives the space key); checkboxes would weigh on the UX without any gain. | "Secrets" and "files" checkboxes (initial proposal); an archive on the relay; an encrypted backup (kept for later). |
| 27 | The `stale_key_epoch` check is done in the transaction that assigns the `seq`. | Lets clients refuse an item from an epoch already replaced (epoch ordering rule, `format-v1.md` §12) without false positives. | |
| 28 | Pin = **encrypted** `item_state` event, without a clear `item_id` (`POST /v1/item-states`), which does not touch the purge. | Project decision: the pinned state does not leak to the relay and the relay purge remains (a). Without a clear `item_id`, the parity of an item's events does not reveal its state. | `item_id` in clear in the event: the relay would infer the state (first event = pin); a tombstone-like event in clear: same; keeping the item on the relay (option (b)): rejected by the project decision. |
| 29 | Multi-recipient reservations **with no effect in v1**: `capabilities` field of ClientInfo / DeviceRecord (§3.3, §4.3); `key_epoch ≥ 2^31` reserved (§4.8, refused by `stale_key_epoch`); `audience` field in the encrypted payload (`format-v1.md` §5.8). | Project decision: be able to open multi-person sharing later without a new `format` for the envelope or the payload. The relay has nothing to change: unknown fields are ignored and the `stale_key_epoch` check already exists. | A clear recipients field in `POST /v1/items`: the relay would learn who each item is for; future routing will go through the key identifier, already in clear. Reserved error codes: useless (§2.5). |
| 30 | The **last device deletes the space** through `DELETE /v1/space` (§4.4.1), without the relay creating an invitation. | Project decision: when the last device leaves, nobody holds the key any more, and the encrypted data has no reason to remain. `/v1/space` is the resource already created by `POST` and read by `GET`. The deletion reuses that of `admin reset` (same function, same isolation of other spaces). No invitation: the relay keeps no clear token and cannot log one (§3.1, §9); the operator makes one with `admin invite --new`. | Returning an installation token to the device: one more secret on the network and in the app. Letting `DELETE /v1/me` of the last member delete the space: a gesture that removes a device would destroy everything without a separate confirmation. |
| 31 | **Positive proof only** (§7.9.1): a client erases its data only on `200` with `seq` (`DELETE /v1/me`), `403 revoked`, or `200 {"space_deleted": true}`; the relay answers `200 space_deleted` only after a durable positive action and the receipt write (§4.4.1). | Three reviews found the same class of defect: an intermediate relay state (`401`, `409 space_not_initialized`, a pending space, an unmounted volume, a partial failure) taken for a proof, leading to an irreversible erasure while the space existed. | Deducing deletion from absence (`401`, `space_not_initialized`, `relay.db` not found): a false positive on any badly mounted or restored relay. |
| 32 | **Deletion receipt** by `deletion_id` (§4.4.2) to prove a deletion on retry. | The client keeps 32 random bytes and sends them back; the relay keeps only their fingerprint and the time, nothing else. No signature to verify for a space that no longer exists, no device identifier kept, no change to `auth/session` nor to rule 4 of §1.4. | A tombstone `(device_id, sign_pk, space_id)` in `routing.db` and a challenge-response on a key of a deleted space, answered with `410`: more metadata kept, one more authentication branch, and `admin reset` would have had to choose whether to write one. `409 space_not_initialized` as proof: rejected by point 31. |
| 33 | **Automatic rotation** by the core after a departure or a revocation by the administrator (§7.6). | Android only rotated the key at startup or after a local action, and Windows never did: an open-ended window in which new items were sealed under a key known to the departed device. No crypto label nor format byte changes. | Letting each application call `maybe_rotate`: two implementations, and doorbell syncs did not relay the information. |
| 34 | **Removal of ONE attachment** (`POST /v1/attachment-tombstones`, §4.8 bis, `format-v1.md` §8.10): a dedicated signed declaration, an `attachment_tombstone` event, an `attachment_tombstones` table kept like the tombstones (the item's purge does not touch it), capability `features: ["attachment_tombstones"]` in `hello` and SpaceState. | Project decision: delete one image or file from a batch, "on this device" or "everywhere", erasing it as if it had never existed. Items are purged from the relay shortly after delivery: the event is then the only way to propagate the removal. | Rewriting the `item` event or the envelope (breaking the chunk AAD, §6.3, and the contract "the relay does not touch the ciphertext"); a scope byte in the declaration (the local scope never travels); a capability probed by `404` (indistinguishable from a broken route). |

### B.2 Accepted trade-offs

- **Removal of an attachment: the relay must be deployed before the apps.** An app that offers "everywhere" to an older
  relay would queue the removal for nothing; the app therefore reads `features` (§3.3, §4.2) and offers the "everywhere"
  scope only if `attachment_tombstones` is listed there. The relay sees that a device removes an attachment (item,
  position, instant), never its content; its `item` event keeps the old list of blobs.
- The relay sees the temporal graph (who sends when, what sizes, who reads when, who is present).
  This is the price of smart ringing, delivery status and fast purging. The relay belongs to you — or to the operator
  who hosts your space, who then sees the metadata of **all** spaces (§1.3).
- **Several spaces on one relay** (decisions of `docs/multi-person-shared-relay.md`): a "single relay URL" mode (the
  applications and the crypto and pairing specifications do not change) and **one database per space** rather than a
  shared database with a `space_id` column: isolation rests on the file, not on a filter that could be forgotten in a
  query, and the queries of a space remain those of the single-space relay. What it costs: 4 SQLite connections per
  space (everything stays open, fewer than five spaces), one writer per space, a total of bytes computed by reading the
  other databases under a global lock, and routing to rebuild at startup. A future sharing between people (links between
  spaces) would not go through the database: it requires cryptography (sealing for the devices of another space);
  nothing is prepared here.
- Outage, update and full disk are common to all the spaces of the relay.
- A tombstone just after a `read` lets the relay guess a secret "after first read".
- An `item_state` event just after receiving or reading an item lets the relay guess which item is concerned (not
  whether it is pinned or unpinned). The relay also sees how many pin decisions each device emits.
  With the archive (`format-v1.md` §8.9), it also sees, without being able to tell them apart from a pin, the archiving
  decisions; archiving a pinned item emits two almost simultaneous state events from the same device, and cancelling an
  archiving gesture emits a second one a few seconds later: the relay at most guesses "a filing gesture", never the item
  nor the direction.
- **Side channels of the pin: downloads.** Downloading an item's files on receipt of a pin would reveal to the relay the
  item and the direction (only pinning would trigger a download). Chosen mitigation: complete decoupling; the pin
  triggers, advances and groups no download, and does not change the timing of `release` (§7.3, `format-v1.md` §8.8.4);
  files follow the D20 policy, the same for all items. Cost: a never-opened file of a pinned item is not fetched
  automatically and becomes unavailable at the relay deadline (at most `max_ttl_secs`); the app flags it on the item.
  Rejected: a deferred or random batch download, which breaks temporal correlation but still reveals that a never-opened
  file was wanted, hence pinned.
- A pin kept raw (`pending_states`, future format) and reopened after the revocation of its signer is refused although
  other devices have applied it (`format-v1.md` §8.8.2); the gap resolves at the next decision on the item.
- A device absent for more than 45 days (`cursor_expired`) loses the compacted `item_state` events, like the tombstones;
  a history transfer from another member gives it the pins back.
- The relay can rename a device (the name is unsigned) and invent an unsigned revocation ("by the administrator" or
  "voluntary departure"). Neither gives access to the content.
- `rl_global_claim` can be exhausted by an attacker to hinder pairing for a minute; with several spaces, it hinders
  everyone's pairing, and a random `claim` makes another's pairing fail (§8.5).
- A device's signing key is unique on the relay: the `device_exists` refusal reveals to whoever knows a key that it is
  already in use (in another space or in one's own). This is information the relay already had.
- Items that stayed on the relay during an absence of more than 45 days, or were lost through a restore, are not
  recovered through the log (a history transfer from another member remains possible).
- With no default expiration, an item not delivered to an offline (non-revoked) device stays encrypted on the relay for
  up to 30 days (before D1, a "1 h" secret disappeared after one hour).
- A history transfer copies all kept secrets and all files to the recipient.

### B.3 Deviations or tensions with other documents

1. **"Delivered" status and cursor**: `PROJECT.md` §3 and §7 say that the "delivered" status comes from the cursor.
   This document uses an explicit acknowledgement (`ack`), which is more exact, and keeps the indicative cursor in
   `admin devices`. The spirit (the relay knows delivery per device) is respected; the letter is not. To be validated by the maintainers.
2. **Device name**: `docs/DEV.md` invariant 1 lists what the relay knows without naming the devices (the original design
   there adds "the identity (public keys) of devices and their presence"). This document adds the name, the platform, the
   app version, the acknowledgements, the reads and the FCM token, which are needed for the decided features (versioning,
   doorbell, "read" state). To be recorded in `DEV.md`.
3. **Pairing lifetime (~2 min) and D7**: the flow "QR → install the APK → rescan" exceeds 2 minutes. The inviting device
   must therefore regenerate a channel (and a QR) at expiry. Nothing changes on the API side.
4. An earlier UX note still mentions a tombstone `{item_id, deleted_at}` without `device_id` or signature, typing the
   relay URL, and a 6-digit code: this document follows `PROJECT.md`.
5. **Interpretation differences settled during the relay implementation** (reviewed and validated):
   (a) an empty envelope (item or state), or one shorter than 4 bytes, receives `422 validation_failed` rather than `413`:
   the minimum size is a shape constraint (the cleartext header is at least 4 bytes, §0.3), not an overrun of a limit;
   (b) `cursor_expired` (§4.10) is evaluated on `max(after, joined_seq)`, consistent with §8.2 (`joined_seq ≤ N < floor`):
   a device never "misses" an event prior to its arrival; (c) the `key_epoch` of a JSON body is accepted in
   `1..=u32::MAX`, and any other value (including `0`) is `422`; epochs `≥ 2^31` otherwise give `409 stale_key_epoch`
   (§4.8, keys outside the space); (d) a `read` (or a `release`) of an item never acknowledged also creates the receipt
   and the `ack` event, consistent with §4.8 (reading implies receipt).
6. **Web admin panel**: `apps/relay/src/admin_web.rs` sets a `__Host-` `SameSite=Strict` session cookie and exposes
   `POST /<segment>/api/invitations`, which contradicted "the relay MUST NOT set a cookie" (§2.4) and "no administration
   API is exposed on the network" (§8.7). Decision: **amend** the spec with a documented, bounded exception (§8.7.1)
   rather than remove the panel: disabled by default, outside `/v1/*`, passkey with user verification, `__Host-` cookie
   `SameSite=Strict`, check of the `Origin` and of the `X-Tomi-Admin` header on any write, a single write (invitations),
   metadata only. When disabled, the relay complies with the letter of §2.4 and §8.7.

### B.4 Open points

1. **Presence on Android**: detecting "screen on and unlocked" without a permanent service is reliable only while the app
   is in the foreground. In practice the phone will be "present" mainly when Tomi is open. To be validated on a test device.
2. **FCM `android_v2`** (early access, November 2026): `background_sync` for silent wake-ups and `remote_notification`
   with `mutable_content` for items that ring could replace `priority`. `remote_notification` requires a generic
   fallback text (e.g. « Nouvel élément », a UI string), without content. To be re-evaluated when the API is stable;
   only `FcmSender` will change.
3. **Alignment with `spec/format-v1.md`**: (a) `core` SHOULD expose `sign_relay_auth(space_id, challenge)` (§3.6), which
   closes open point 2 of `format-v1.md` §13; (b) the relay checks the envelope header only in format 1, because only the
   first 4 bytes are stable across formats; (c) `format-v1.md` §5.4.2 says that an `after_first_read` item is "erased
   from the device" after the first reveal; this document adds that the first device that reveals emits a tombstone
   "everywhere" (a requirement of the original design); to be recorded in `format-v1.md` or in the client specification.
4. **Minimum app version imposed by the relay** (e.g. after a security flaw): not planned. The only lever is the
   `format`. A possible later addition: a configuration key `min_app_version.<platform>` → `update_required`.
5. **TLS certificate pinning**: not planned (Let's Encrypt renews the keys). The model does not depend on TLS for the
   confidentiality of the content (E2EE), only for that of the metadata and the tokens.
6. **Administration socket protocol**: left to the implementation (internal to the binary).
7. **Clients and multi-space relay.** `space_initialized` of `hello` says nothing any more about the caller's space
   (§4.2): a client that uses it to hide « Créer » (Windows) must always offer it. A client whose space was deleted
   (`admin reset`, or `DELETE /v1/space` whose response it did not receive) while the relay hosts others receives
   `401 unauthorized` at the session, and not `space_not_initialized`: the loop of §7.2 leads it to the state NOT_FOUND
   (UI label: « Ton serveur Tomi ne trouve plus ton espace »), which replaces the former names UNRECOGNIZED and SPACE_DELETED.
   **v1 decision: no proof of a manual reset.** The devices of a space reset by `admin reset` receive `401` or
   `409 space_not_initialized`, display "espace introuvable" (UI label), do **not** erase themselves (it is not a proof,
   §7.9.1), and offer « Effacer quand même » behind biometrics (§7.9.5). The state is re-checked at each network call;
   it is never kept. The runbook must tell the operator to warn the person: their apps will display "espace
   introuvable", they can erase them from there, then use a new token (`admin invite --new`, since a `reset` of a space
   that is not the last one creates none). A deletion through `DELETE /v1/space`, for its part, is proven by the receipt
   (§4.4.2), including after a lost response.
8. **Per-space pairing limit**: `rl_global_claim` is common to all spaces (§8.5). A per-space limit is planned only if a
   space abuses it.
9. **On-demand SQLite connections**: beyond a few dozen spaces, open the databases on demand and close those that are
   idle. Not relevant for fewer than five spaces.
10. **Backup and restore per space**: `infra/backup.sh` must back up `routing.db` and each `relay.db` (one `.backup` copy
    per database); restoring a single space on its own is possible (§10).
