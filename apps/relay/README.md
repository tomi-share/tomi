# tomi-relay

The **blind** server of Tomi: a *store-and-forward* HTTP relay that routes encrypted bytes
between devices without ever decrypting anything. It hosts **one or more independent spaces**
(one per person, fewer than five) behind **a single URL**. It implements
`spec/api-v1.md` (the normative document).

## What the relay does, and does not do

- It **stores** encrypted envelopes, blobs and an event log, **routes** them between member
  devices, **purges** them when due, and enforces quotas and rate limits.
- It **verifies signatures** (device certificates, key rotations, tombstones, transfer manifests),
  delegating all decoding and cryptography to `tomi-core`. It does not reimplement any primitive.
- It **never decrypts anything**: no content, no keys, no state. For a pin, it only sees an opaque
  random `event_id`, never the targeted item nor whether the action is pin or unpin.
- It **never logs a secret** (token, challenge, signature, envelope, part body), with the single
  documented exception of the installation token, shown at the first start of a fresh installation (§3.1).
- It **isolates spaces**: one SQLite database and one blob directory per space. The space of a
  request is the one designated by its identifier (session token, `device_id`, channel number,
  installation token, §1.4). An identifier that designates no space gets the unknown-identifier response.

It depends on `tomi-core` with **`default-features = false`**: no second SQLite, rustls or UniFFI
is pulled in from the core. The relay's SQLite databases (WAL) and on-disk blobs are **server-side**
storage, distinct from the client store of `tomi-core`.

Layout of `data_dir` (§10):

```
routing.db                        invitations (pending and consumed), quota overrides, default quota set from the panel, FCM health
spaces/<space_id>/relay.db        the database of one space
spaces/<space_id>/blobs/…         its blobs
public/  relay.lock  admin.sock   global
```

A `data_dir` in the old layout (`tomi.db`, `blobs/`) is migrated at startup (renames, the
switch point `spaces.migrating/` → `spaces/`, §10.1.1), and **only** by a process holding
`relay.lock`. A read-only `admin` command that finds the old layout refuses ("start the
relay") instead of migrating.

## Deployment

TLS is **terminated by a reverse proxy (Caddy)** in front of the relay. The relay listens over HTTP on
a loopback address (`127.0.0.1:8080` by default) and **refuses to start** on a non-loopback
address unless `allow_public_listen = true` (development only). It never serves plaintext traffic
on a public interface and emits no CORS headers.

```
Devices ──HTTPS/WSS──► Caddy (TLS, Let's Encrypt) ──local HTTP──► tomi-relay ──► per space: SQLite (WAL) + blobs
                                                                          └──► FCM HTTP v1 (ring notifications)
```

## Running

```sh
# Server (default command):
tomi-relay --config /etc/tomi-relay/config.toml
# or simply: tomi-relay   (default file /etc/tomi-relay/config.toml, optional)
```

At the first start of a fresh installation (no space, no invitation), the relay creates an
**invitation** and logs its token at `WARN` level (`install token: <token>`), once only. It keeps
only a fingerprint of the token. This single-use token allows a first device to create a space
(`POST /v1/space`); the invitation is then consumed. A consumed invitation stays in `routing.db`
(with the `space_id` that was created): if the `201` response is lost, the founder resends the
request, gets `409 space_exists`, and proves with its keys that the space is its own. `admin reset`
of the space forgets it; `admin invite --rotate` only removes pending invitations. One invitation per
person to welcome: `tomi-relay admin invite --new`.

## Configuration (§8.6)

A TOML file, where each key can be overridden by an environment variable
`TOMI_RELAY_<KEY_IN_UPPERCASE>`. An unknown key (in the file or the environment) makes startup
fail: a typo never silently runs with a default value.

Main keys (default values in `src/config.rs`):

| Key | Default | Role |
|-----|---------|------|
| `listen` | `127.0.0.1:8080` | listen address (loopback unless `allow_public_listen`) |
| `allow_public_listen` | `false` | allows a non-loopback address (development only) |
| `public_url` | `https://relay.example.org` (example: **to be set**) | public URL of the relay, without a trailing slash: update URLs, short links, the space-creation link of the panel (`<public_url>/create#<token>`) and the origin of the web panel |
| `data_dir` | `/var/lib/tomi-relay` | `routing.db`, `spaces/` (database and blobs per space), `public/` |
| `trusted_proxies` | `127.0.0.1/32`, `::1/128` | proxies whose `X-Forwarded-For` is trusted |
| `session_ttl_secs` | `3600` | lifetime of a session token (not extendable) |
| `max_relay_bytes` | `1 GiB` | total bytes of **all** spaces |
| `max_space_bytes` | *absent* | default quota of a space: absent or `0` = **no per-space limit**; the web panel, then `admin quota` (per space), override it (see "Per-space quota") |
| `admin_web` | `false` | serves the web administration panel (see below); requires an https `public_url` on a domain name |
| `fcm_service_account_file`, `fcm_project_id` | — | FCM service account (otherwise Android push is disabled) |
| `apns_key_file`, `apns_key_id`, `apns_team_id`, `apns_topic` | — | `.p8` key of the Apple developer account, its identifier, the team, the app identifier (default `eu.shaffe.tomi`); otherwise iPhone push is disabled (`docs/push-apns.md`) |
| `log_format` | `json` | `json` or `text` |

The size limits (§2.8) and the rate-limit buckets (`rl_*`, §8.5) are also
configurable; see `src/config.rs`.

```sh
# Example of an override through the environment:
TOMI_RELAY_DATA_DIR=/srv/tomi TOMI_RELAY_MAX_DEVICES=10 tomi-relay
```

## Local administration (§8.7)

Never reachable from the network. On Unix, a command talks to the running relay through a
local socket; otherwise it acts on the databases directly, and anything that changes state
requires the relay to be stopped (the `relay.lock` lock proves it). Reading is always allowed
(SQLite WAL) and writes nothing: no migration, no `routing.db` created, no row deleted.

`--space <id>` designates a space by its `space_id` or by a prefix of at least 8 characters. Without
`--space`, a relay that has only one space uses it; with several, the command refuses
(exit code 2) and lists them.

```sh
tomi-relay admin status [--json]                  # the relay: number of spaces, total, disk, push, invitations
tomi-relay admin status --space <id> [--json]     # one space: members, usage, quota
tomi-relay admin spaces [--json]                  # the spaces: id, date, members, usage, quota
tomi-relay admin devices [--space <id>] [--json]  # the member devices of a space
tomi-relay admin quota [--space <id>] <bytes|none|default>   # quota override of ONE space (the default for all is set from the web panel)
tomi-relay admin invite [--new | --rotate]        # lists pending invitations; --new: creates one and shows its token; --rotate: replaces the pending invitations
tomi-relay admin revoke [--space <id>] <id|prefix>  # revokes a device (prefix ≥ 8 characters)
tomi-relay admin reset [--space <id>] [--confirm <8 chars of the space_id>]  # deletes ONE space, nothing else
```

The **last device** of a space can also delete it itself, from the application
(`DELETE /v1/space`, spec §4.4.1): the same deletion as `admin reset` (WebSockets closed with `4004`,
folder, blobs, routing, quota, consumed invitation), refused with `409 not_last_device` while another
active member remains. Unlike `admin reset`, the relay **does not create an invitation** on this
occasion (it keeps no plaintext token and logs none outside the first start). To recreate a space,
run `tomi-relay admin invite --new` (or restart a relay left without space or invitation, which
announces one as at installation).

## Per-space quota (default that can be managed)

A space can have a **ceiling** (`used_bytes`: live envelopes, item states, declared blobs, §8.4),
decided in this order, the first one that exists winning:

1. **the space override**: `tomi-relay admin quota --space <id> <bytes|none|default>` (`none` = no
   limit for this space, `default` = removes the override);
2. **the default set from the web panel** (see below), stored in `routing.db`;
3. **`max_space_bytes`** from the configuration file (`0` = no per-space limit);
4. **otherwise: NO per-space limit.** This is the historical behaviour of production, kept: there is no
   built-in value. Only the total `max_relay_bytes` then bounds the spaces together.

Without any setting anywhere, a deployment or an update therefore changes nothing in the relay's behaviour.

The effective ceiling announced to a space (`quota.max_bytes`) remains the smaller of this ceiling (if it exists) and
the remaining room in the `max_relay_bytes` total (which only reveals the sum of the other spaces). Exceeding it
answers `507 relay_full`.

The default is set from the panel (`GET`/`POST api/quota`, with the same guard rails as the other actions: `__Host-`
cookie, `Origin` of `public_url`, `X-Tomi-Admin: 1` header): body `{"default_bytes": N}` with `N` an integer between
1 MiB (`1048576`) and `2^53 - 1`; `0` = no per-space limit (only the total applies); `null` = forget the panel setting
(back to `max_space_bytes`, then to "no limit"). Any other value: `400 bad_request`,
without repeating it. The change applies **immediately**, without restart, and survives a restart. Nothing
is deleted: a space already above the new ceiling can only free space (its writes are refused). The panel shows the default
("no limit per space" as long as nothing is set), its origin (`panel`, `config`, `none`) and, per space, the usage, the quota and whether
it is specific to the space; it does NOT change the quota of a single space (that stays with the CLI). `admin status --json` gives the same object
`default_space_quota` (`bytes`: `null` = no limit; `source`), `admin spaces --json` gives `max_bytes`
(`null` = no limit) and `quota_source` (`space` or `default`). The log only says that the default
changed (and the number of bytes), never anything about a space.

**Rollback to an older binary**: `max_space_bytes = 0` is accepted by this binary (same meaning as an absent
key: no limit), but the older binary refuses it ("must be > 0") and would not start.
Before a rollback, remove the key (the behaviour is identical) or replace it with an explicit limit; the
`default_space_quota` table of `routing.db` is ignored by the older binary.

## Device revoked during a request

The gate (`gate.rs`) reads the session before the request body, which may take several minutes to
arrive. Every write transaction made on behalf of a device therefore starts with
`store::require_caller`: within the **same** transaction, the session exists and is not revoked, the device
is not revoked, and it still holds the key the gate saw. A device revoked (rotation, `DELETE /v1/me`,
`admin revoke`) between the access check and the transaction therefore achieves nothing and receives the response of a
device revoked before the request (`403 revoked`, `401 unauthorized` if the session has disappeared): no oracle.
The session's `expires_at` is not rechecked within the transaction: a session that expires during a long upload completes its write, as the gate already allows.
Out of scope, by choice: reads (`GET`), `DELETE /v1/auth/session` (which only removes a right) and the indicative `cursor_hint` written by `GET /v1/events` (a hint for `admin devices`, never used for purging; it only touches the calling device's own row).

## API response headers

Every `/v1/*` response, errors included, carries `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`
(`middleware.rs`), in particular `POST /v1/auth/session`, which contains the token.

## Space-creation links (`/create`)

`GET /create`, `GET /create/` and `GET /create/<anything>` return **the same static page** (embedded:
`routes/create.html`), which says to open the link with the Tomi app and points to `/l/` (the install page at
`public/l/index.html`) to download it. The creation link is `https://<public host>/create#<token>`: the
installation token is in the **fragment**, which the browser never sends. The form `/create/<token>`
(accepted by the apps) puts the token in the path, which the route **does not read** and which the relay's
access log does not contain either (it only logs the route pattern, here `/create/{*path}`). The route validates nothing
(no database access), decrypts nothing, runs no script (`Content-Security-Policy: default-src 'none';
style-src 'unsafe-inline'`, `Referrer-Policy: no-referrer`) and loads no third-party resource. It is counted
in the `rl_ip_static` bucket. **Beware of reverse proxy logs**: an access log that records the path
would see the token of `/create/<token>` (see `infra/caddy/Caddyfile`, `infra/forge/nginx-site.conf`). App
links (Android App Links) are declared in the app (`pathPrefix`); `assetlinks.json` is a file placed
in `public/.well-known/` without a list of paths, and the relay does not serve an `apple-app-site-association`.

## Web administration panel

In addition to the local socket, the relay can serve a web panel: **read-only + invitations**,
protected by a **passkey** (WebAuthn). A single administrator per relay, a single key.
The panel never displays or carries item content, file names, keys or secrets:
the relay stays blind.

### Enabling

Disabled by default. `admin_web = true` (or `TOMI_RELAY_ADMIN_WEB=true`); the `public_url` must be an
https URL on a domain name (startup fails otherwise): the WebAuthn `rpId` is its host, the expected origin
is its own, and the cookie is `Secure`. **Without activation, every panel path answers
exactly the 404 of any unknown resource** (same body, same headers; only the `X-Request-Id`
changes, as everywhere): nothing is routed, only the router's "fallback" looks at the first segment.

### Path

The panel lives under `/<segment>/`, where `<segment>` = 128 bits drawn from the CSPRNG, in base64url (22 characters),
created at the first start with the panel enabled and kept in `routing.db`. **This is not an authentication mechanism**
(the passkey is): it is defence in depth against scanners, nothing more. A wrong
segment answers like an unknown resource. The segment appears in no log: the route label of the access log
is the constant `admin-web`.

```sh
tomi-relay admin web                 # shows the full URL (public_url + segment) and the state of the key
tomi-relay admin web --rotate-path   # new segment: the old one and all sessions stop working
tomi-relay admin web enroll          # single-use enrolment code (shown ONCE)
tomi-relay admin web reset [--confirm <first 8 chars of the segment>]   # deletes the key and the sessions, reopens enrolment
```

Like the other `admin` commands, those that write go through the socket of the running relay (Unix),
otherwise they require the relay to be stopped.

### Enrolling the key

A public relay reachable before its owner has enrolled their key could be claimed by an
unknown party. Enrolment therefore requires an **enrolment code** created on the server by `admin web enroll`: 60 random bits
(`xxxx-xxxx-xxxx`), **valid for 15 minutes, 5 attempts** (the fifth failure invalidates the code),
compared in constant time; only its SHA-256 fingerprint is kept. The first holder of the code
enrols their key; **as soon as a key is registered, enrolment is closed for good** (its endpoints
answer 404). Adding or replacing a key is only possible through the CLI: `admin web reset`, then a new code.

### Passkey (WebAuthn, in pure Rust)

`webauthn-rs` is not used: it depends on OpenSSL, which would break the static musl binary. The
verification is written by hand (`src/webauthn.rs`), with a narrow, deliberate scope:
**ES256 only** (COSE `alg` -7, curve P-256), attestation **`none` only** (no attestation chain
is verified), **user verification required** (UV, in addition to UP), a single key.
At enrolment: `clientDataJSON` (type, challenge, exact origin), `rpIdHash`, UP+UV+AT flags, a COSE
EC2 P-256 key whose point must be on the curve. At sign-in: type, challenge, origin, `rpIdHash`,
UP+UV, ECDSA signature over `authData || SHA-256(clientDataJSON)`, signature counter strictly
increasing as soon as either counter is non-zero (two zeros: authenticator without a counter). The
challenges are 32 bytes, single-use, expire after 5 minutes, live in memory and are compared in
constant time. Storage (`routing.db`): credential identifier, raw public key (SEC1, 65 bytes),
counter, date. Nothing else.

### Session

After a valid assertion: a 256-bit token (CSPRNG) in a `__Host-tomi-admin` cookie (`Secure`,
`HttpOnly`, `SameSite=Strict`, `Path=/`, no `Domain`); only its fingerprint is kept, in memory (lost
at restart: you sign in again). **30 minutes of inactivity, 8 hours at most**, explicit sign-out.
Every `POST` request must carry the `Origin` header of the `public_url` **and** `X-Tomi-Admin: 1`.
Sensitive operations (enrolment code, challenge, sign-in) share a global rate bucket (burst of 10, then
6 per minute, `ratelimit.rs`); failure is generic: never a hint about the cause (the exact cause, a fixed word,
goes to the log, without a value).

### What the panel does

The same functions as `admin.rs` (refactored to be shared): relay state (version, disk, FCM,
spaces), list of spaces (8-character short identifier, quota, usage, devices), devices of a
space (metadata only, as with `admin devices`), pending invitations (list, creation, replacement;
the token is shown only **once**, in the creation response, together with the **creation link** `<public_url>/create#<token>`: the panel shows the bare token (for manual entry), the copyable link and the QR code **of the link**; the link's host comes from `public_url`, never from the `Host` header), **default space quota** (read and edit). **No** revocation, reset, or change of the quota of a single space from the web (that remains `admin quota`): a space's detail view shows the matching server commands instead, to copy (the one for `reset` without `--confirm`).

Added reads (additive, under the same session, metadata only):

* `GET api/spaces`: each row additionally gives `present`, `last_activity_at` (latest activity of a member),
  `items` (live envelopes), `blobs`, `events` (retained events), `head_seq`, `retention_floor_seq` and
  `rotation_required` (key change due after a revocation). The same fields appear in `admin spaces --json`.
* `GET api/devices`: additionally `registered_at` (date the device arrived; also in `admin devices --json`).
* `GET api/about`: version, `started_at`/`uptime_secs`, `server_time` (the page counts its "3 days ago" from
  the server clock), end of the session (`session.idle_ends_at`, `absolute_ends_at`, `ends_at`), `push_configured`
  and the configuration limits (total, configured quota, sizes, retention, inactivity, purge…). No path,
  URL or file.
* `GET api/journal`: the panel's **administration journal** since startup, in memory only (lost
  at restart, 100 lines at most): sign-in, refused sign-in, refused enrolment code, sign-out, key
  created, invitation created/replaced, default quota changed (with its number of bytes), and, from the CLI, path
  changed and key deleted. Fixed words, dates and a count: never a value coming from a request, an
  address, a token, a code or anything about a space. Two consecutive actions of the same type, without an associated value, count as one, with
  a count (a flood of refusals does not drive out the rest); the quota change, which carries a value, is never folded.
  The journal keeps 100 lines: the count of refused sign-ins shown by the panel covers these lines only.

The page: an overview with tiles, gauges and **attention points** (storage or quota of a space above
90%, disk near the threshold, FCM in error, pending key change, refused sign-ins), relative dates,
sortable tables, a space detail view, light/dark/automatic theme (the only preference kept in the browser),
in-page confirmation (`<dialog>`) to replace invitations or remove the per-space ceiling, token hidden
automatically after 10 minutes or when leaving the page, remaining session time, and a copyable **diagnostic** for
a bug report (versions, counts, sizes, states; no space identifier, device name or token).
No automatic refresh: it would keep the session open forever. The interface is an HTML page, a JS file and a CSS file embedded
(`include_str!`), same origin, no framework, no CDN, no inline script or style, with
`Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
`Cache-Control: no-store`, `Referrer-Policy: no-referrer` and `X-Content-Type-Options: nosniff` on all responses.

### Threat model

* **A scanner looking for `/admin`**: nothing (identical 404). **One that finds the segment**: a page that asks
  for a passkey; it gains nothing beyond being able to try. Rate limiting is applied **per address**
  (10 in burst, then 6/min), with a wider global safety net: a flood from a single address therefore does not
  lock out the legitimate administrator, who connects from another one.
* **Unknown party before enrolment**: they need the enrolment code (15 min, 5 attempts, 60 bits).
* **Cookie theft**: session of 8 h at most, 30 min of inactivity, `HttpOnly`/`Secure`/`SameSite=Strict`.
  `admin web reset` or `--rotate-path` cuts everything off.
* **CSRF**: `SameSite=Strict`, exact `Origin` and a custom header on every `POST`; no CORS.
* **Phishing**: the key is bound to the `rpId`; another origin cannot get it to sign.
* **Out of scope**: a compromised server machine (which can read `routing.db` anyway); a reverse proxy that logs
  URLs (Caddy: the segment would appear there; do not log panel paths, or rotate the segment with
  `--rotate-path`); attestation (we do not know which authenticator produced the key).
* The panel is a deliberate exception to "no administration API on the network" (`spec/api-v1.md` §8.7):
  it offers reads and invitations, under a passkey, disabled by default.

The three panel tables (`admin_web_credential`, `admin_web_enrollment`, `admin_web_session` in
`routing.db`) are created by an additive migration as soon as `routing.db` is opened for writing, even when
`admin_web = false`: they stay empty and inert while the panel is not enabled, and a binary of the
previous version opens the same database without seeing anything in it.

### Added crates

* `p256` **`=0.14.0`** (`default-features = false`, `features = ["ecdsa"]`): ECDSA P-256 /
  SHA-256 verification and point validation on the curve. Pure Rust (RustCrypto), same generation as `sha2` 0.11
  already in use. Pulls in `ecdsa`, `elliptic-curve`, `primefield`, `primeorder`, `crypto-bigint`, `sec1`, `der`,
  `rfc6979`, `base16ct`, `cpubits`, `ff`, `group`, `wnaf`, `num-traits`. **No additional OpenSSL or `ring`**
  (`cargo tree -p tomi-relay -i openssl` is empty; `ring` was already there for FCM).
* CBOR decoder: written by hand (about 150 lines, `src/webauthn.rs`) rather than `ciborium`. It only reads the
  required subset (definite lengths, no tags or floats, bounded depth, lengths
  checked before any allocation) and refuses the rest.

### Migration of `routing.db`

Additive: four tables (`admin_web_path`, `admin_web_credential`, `admin_web_enrollment`, `default_space_quota`) created with
`CREATE TABLE IF NOT EXISTS` at each write-mode open, **without changing `user_version`** (still 1):
a relay of the previous version opens the same database without breaking anything (it ignores these tables), so a
rollback remains possible. A read command writes nothing and reads an older database as "nothing recorded".

## Development

From the root of the repository (the relay is part of the Cargo workspace):

```sh
cargo check -p tomi-relay
cargo test  -p tomi-relay
cargo fmt --all -- --check
cargo clippy -p tomi-relay --all-targets -- -D warnings
```

The integration tests (`tests/`) start a real relay in a temporary directory with an
injectable clock and a test HTTP/WebSocket client, and cover the acceptance
checks A1–A173 of `spec/api-v1.md` §11 that can be verified on the server side. `tests/multispace.rs`
(A160–A173) starts two spaces on the same relay: isolation, identifier uniqueness, quotas,
`admin --space`, migration. The signed objects used by the tests are built with `tomi-core`
(no dummy bytes where validity is checked).

## Structure

```
src/
  main.rs         binary: server + `admin` subcommands
  app.rs          state opening, startup, service loop, startup cleanup
  config.rs       TOML configuration + environment overrides (§8.6)
  db.rs store.rs  SQLite access (WAL): per-space database, `routing.db`, space/log queries, budget
  spaces.rs       spaces, routing (token/device/channel → space), per-space quotas, creation/deletion
  migrate.rs      migration from the single-space layout to `spaces/<space_id>/`
  install.rs      installation tokens and invitations (§3.1)
  error.rs wire.rs input.rs   error model RFC 9457 (§2.5), encoding, input validation
  gate.rs ratelimit.rs middleware.rs   auth, rate limits (§8.5), X-Request-Id
  hub.rs push.rs fcm.rs apns.rs   WebSocket notification hub, ring decision, FCM HTTP v1, APNs (§5, §6)
  blobstore.rs disk.rs        blobs on disk, free space
  purge.rs maintenance.rs revoke.rs   purge/compaction/quotas (§8), revocation (§3.4)
  logging.rs      logging policy (§9): no secret, even under RUST_LOG=trace
  admin.rs        local administration (§8.7)
  admin_web.rs webauthn.rs admin_web/   web panel: state, session, enrolment; WebAuthn verification; page, script, style
  routes/         one module per endpoint family (§4)
tests/            acceptance tests (§11)
```
