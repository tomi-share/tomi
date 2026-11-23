# Apple push (APNs): implementation

Reference for whoever picks up this work. The normative rule is in `spec/api-v1.md` (§4.5, §6.3, §6.4, §1.3); this
document says **where the code is, why it is built this way, how to put it into service and what is left to do**.

**Status on November 18, 2026: written, tested with test doubles, never tried against Apple.** What is missing is an
APNs key from the developer account on a deployed relay. As long as it is not configured, nothing changes: the relay
sends nothing to Apple and iPhones receive as before (when the app is opened, or when iOS chooses to wake it).

## 1. The problem

iOS stops an app that leaves the screen. The core's WebSocket doorbell closes with it: an item sent from another
device only reaches the iPhone when Tomi is reopened, or during a background refresh (`BGAppRefreshTask`, every 15
minutes at best, often much less frequently, never if the app was closed by swiping it away). Without push, there is
no notification on arrival.

The Mac is not affected: its app stays in the menu bar with the doorbell open. Android has the same need, covered on
the relay side by FCM (§6.3) but not yet on the app side (see 8).

## 2. The choice: a generic alert, then the real notification

The relay is blind: it sees neither the content nor the type of an item. It can only ask Apple for two things, both
**with constant content**:

| Case | What the relay sends | What the person sees |
|---|---|---|
| The item must ring on this device (`ring = true`, §6.1) | an **alert** whose two texts are app keys (`ios_push_title` = "Tomi", `ios_push_body` = "New item"), with `content-available` | the notification "Tomi · New item", immediately, even if iOS has stopped the app |
| Other wake-up (item received silently, update) | a `background` push (`content-available` only) | nothing; the app is woken when iOS decides to |

When iOS wakes the app (it is not obliged to), the app syncs, shows **its own** notifications (type, sender,
preview; never anything from a Hidden item), without their sound since the alert has just rung, then removes the
generic alert. If the app is not woken, the generic alert stays, and goes away when the person opens Tomi.

Ruled out: the silent wake-up alone, like Android's FCM message. iOS limits these wake-ups to a few per hour, can
delay them by several minutes and delivers none to an app closed by swiping it away: the notification would not
arrive reliably. Also ruled out for now: a "Notification Service" extension that would decrypt the item to write the
real notification on arrival; it would have to open the core (keychain key, database in the app group), which the
app does not do outside its process today.

What Apple learns (§1.3): the device token, the time of each doorbell, whether it must ring, the app identifier.
Nothing about the item.

## 3. The path of a push

```
iPhone                         relay                          Apple (APNs)
  │ 1. registerForRemoteNotifications ──────────────────────────▶│
  │◀──────────────────────────────── device token (hexadecimal) │
  │ 2. PUT /v1/me/push {provider: "apns" | "apns-sandbox", token}│
  │──────────────────────────────▶│ push_registrations (space database)
  │        (app stopped by iOS)   │
  │                               │ 3. another device sends an item
  │                               │    → PushDispatcher.notify (no open WebSocket)
  │                               │ 4. POST /3/device/<token> (HTTP/2, JWT ES256)
  │                               │──────────────────────────────▶│
  │◀──────────────────────────────────────────── 5. generic alert │
  │ 6. iOS wakes the app (if possible): sync, own                │
  │    notifications, removal of the generic alert               │
```

## 4. Relay (`apps/relay`)

| File | Role |
|---|---|
| `src/apns.rs` | All of APNs: `ApnsCredentials` (`.p8` key, identifiers), provider token, `payload`, `headers`, `classify`, `ApnsSender`, `Http2Poster` client. |
| `src/push.rs` | `PushSender` (common interface), `PushDispatcher` (2 s per-device burst guard, merging of urgencies, removal of dead tokens), **`ProviderSenders`** (picks the sender according to the provider), `PushError::Unsupported`. |
| `src/fcm.rs` | The existing FCM sender; it also provides the `HttpPost` and `HttpReply` interface reused by APNs. |
| `src/app.rs` | Wiring from the configuration (`PushChoice::FromConfig`): one, two or no sender. |
| `src/config.rs` | `apns_key_file`, `apns_key_id`, `apns_team_id`, `apns_topic` (all optional). |
| `src/routes/devices.rs` | `PUT /v1/me/push` accepts `fcm`, `apns`, `apns-sandbox`. |

Things to know:

- **Two providers for Apple.** A token is only valid on the endpoint of the environment the app was signed for:
  `apns` → `api.push.apple.com` (App Store, TestFlight), `apns-sandbox` → `api.sandbox.push.apple.com` (app
  installed from Xcode). The app says which (see 6).
- **The token goes in the request path** (`/3/device/<token>`). It is therefore validated twice as hexadecimal of 64
  to 200 characters: at registration (`422` otherwise) and again in `ApnsSender::wake`.
- **Provider token.** JWT ES256 `{"alg":"ES256","kid":<apns_key_id>}.{"iss":<apns_team_id>,"iat":<s>}`, signed with
  `ring` (`ECDSA_P256_SHA256_FIXED_SIGNING`: raw r‖s signature, which is what JWS wants). Kept for 50 minutes (Apple
  refuses more than one hour). On a `403`, it is dropped so it gets re-signed, unless it is less than 20 minutes old:
  Apple refuses renewals closer together than that, and a token that young is not refused for its age (wrong key or
  identifiers).
- **HTTP/2.** APNs only speaks HTTP/2 and `ureq` (used for FCM) does not: `Http2Poster` uses `hyper` (already there
  for the server) with `hyper-rustls`. Same TLS stack as the rest of the relay (`rustls` with `ring`, `webpki-roots`
  roots), `https_only`. One connection is kept open, monitored by a ping every 60 s; overall timeout of 20 s per send.
- **Apple's responses** (`classify`): `200` → sent; `410`, or `400` with `BadDeviceToken`, `DeviceTokenNotForTopic`
  or `Unregistered` → token deleted; `403` → `auth_error`; `429` and `5xx` → retry (1 s, 4 s, then give up: cursor
  catch-up covers the loss); other `400` → relay bug, logged.
- **Provider not configured.** An `apns` registration on a relay without an Apple key is not served
  (`PushError::Unsupported`): nothing is sent, the token is kept, health is not affected.
- **Secrets.** Neither the device token, nor the provider token, nor the key appear in a log, an error or a `Debug`
  (`PushTarget` and `ApnsCredentials` have a hand-written `Debug`).
- **Health** (`admin status`, web panel): a single state for FCM and APNs. Known limitation, see 8.

## 5. Core (`crates/core`)

| File | Addition |
|---|---|
| `src/runtime/transport.rs` | `Transport::register_push(provider, token)` and `unregister_push()`, no-ops by default (demo, tests). |
| `src/runtime/http.rs` | `PUT /v1/me/push` and `DELETE /v1/me/push`. |
| `src/runtime/actions.rs` | `Engine::register_push` / `unregister_push`: require an active space and a session. |
| `src/ffi.rs` | `register_push(provider, token)` and `unregister_push()` exposed to the apps. |

The core does not keep the token: the app hands it over again at every launch and when the system renews it
(registration is idempotent on the relay side). The `push_token` parameter of `sync` is still accepted and ignored:
it did not say which push service the token was for.

`unregister_push` has no caller yet: the relay itself removes the registration of a device that is revoked or that
leaves the space (`revoke.rs`).

## 6. iOS app (`apps/apple`)

| File | Role |
|---|---|
| `Tomi/Notify/PushRegistration.swift` | Token request, provider choice, wake-up by push, removal of the generic alert. |
| `Tomi/Notify/IncomingNotifications.swift` | `TomiAppDelegate`: receiving the token and pushes; `quietRuns` (notifications without sound). |
| `Tomi/App/AppContainer.swift` | `registerPush()` and `runInBackground(sendOnly:push:)`. |
| `Packages/TomiKit/.../TomiCore.swift`, `Packages/TomiRuntime/.../RustTomiCoreAdapter.swift` | `registerPush(provider:token:)` (nothing on the demo core). |
| `project.yml`, `Tomi/Tomi.entitlements` | `aps-environment` entitlement. |
| `Tomi/Info.plist` | `remote-notification` background mode. |
| `tools/strings-apple/<lang>.json` | The two fixed texts `ios_push_title` and `ios_push_body`. |

Things to know:

- **Token request** at launch (`PushRegistration.start`). It asks for no permission: the permission to show
  notifications is separate, and without it a push still wakes the app.
- **Which provider.** `PushRegistration.provider` reads `aps-environment` in the embedded signing profile
  (`embedded.mobileprovision`): `development` → `apns-sandbox`, otherwise `apns`. An App Store or TestFlight build
  embeds no profile → `apns`. A simulator → `apns-sandbox`. The entitlement in `project.yml` is `development`; Xcode
  signs a distribution build with `production` on its own.
- **When the token goes to the relay** (`AppContainer.registerPush`): when the core is loaded, when the token
  arrives, at the end of a pairing, on every feed change as long as it has not succeeded, and on every return to the
  foreground. Once it has succeeded for a (core, token) pair, it is not sent again. Never on the demo core nor
  outside a space.
- **Wake-up by push** (`PushRegistration.woken`): `runInBackground(push:)` syncs, **re-reads the state right away**
  (`RustTomiCoreAdapter.refresh`, then `itemsChanged`) and returns. Without this explicit re-read, iOS could suspend
  the app before the state caught up with the core: the generic alert was removed and no notification replaced it.
  Queued sends are not processed here (a push gets 30 seconds).
- **No double ring.** After an alert (`aps.alert` present), the app's notifications are shown without sound
  (`quietRuns`, a counter: two wake-ups can overlap). After a silent wake-up, they ring.
- **Removal of the generic alert** (identifier `tomi`, the one from `apns-collapse-id`): after a wake-up that managed
  to sync, and on every return of the app to the foreground. Never if the sync failed: it is then all the person has.
- **App in the foreground.** `NotificationRouter.willPresent` shows nothing on iOS: the feed already shows the item.
  In practice the relay does not send a push to a device whose WebSocket is open.
- **Tapping the generic alert** simply opens the app (no item identifier in it).
- **Mac.** Everything is under `#if os(iOS)`; the Mac target has neither the entitlement nor the background mode.

## 7. Putting it into service

In this order: **the relay first**. An older relay refuses the `apns` provider (`422`); the app keeps working,
without push, and retries on every return to the foreground.

1. **APNs key.** developer.apple.com › Certificates, Identifiers & Profiles › Keys: create a key with "Apple Push
   Notifications service (APNs)". Download `AuthKey_<id>.p8` (possible only once), note the key identifier and the
   team identifier (Membership), ten characters each. One key serves both environments.
2. **Relay.** Upload the key and fill in `apns_key_file`, `apns_key_id`, `apns_team_id` (`apns_topic` if the app
   identifier is not `eu.shaffe.tomi`): recipe in `infra/forge/recipes.md`, "5 bis", example in
   `infra/forge/config.toml`. Restart. `admin status` must show push as configured.
3. **iOS app.** Build and install. The app identifier must have the "Push Notifications" capability: Xcode adds it
   by itself with automatic signing (`-allowProvisioningUpdates` on the command line).
4. **Trial.** iPhone in the space, notifications allowed, Tomi left (not swiped away). Send from another device:
   "Tomi · New item" must appear within a few seconds, then be replaced.

If nothing arrives, check in this order: is `push` `true` for the iPhone in `tomi-relay admin devices` (the token is
registered); the relay log at the time of sending (`auth_error`: key, key identifier or team; token deleted: wrong
environment or wrong `apns_topic`); did the iPhone have an open WebSocket (app on screen: no push, this is
intended).

## 8. What is left to do

1. **Try against Apple.** Nothing has run against the real service. To confirm in particular: reading a real `.p8`
   key with `ring` (the tests sign with a generated key), the remote notification's identifier being equal to
   `apns-collapse-id` (this is what allows it to be removed), the absence of an embedded profile on TestFlight.
2. **Health per provider.** `push_health` is shared: a failing Apple key can be hidden by a successful Android send,
   and vice versa.
3. **"Reset Tomi" without leaving the space** leaves the iPhone registered on the relay: the generic alert can still
   arrive on a device that no longer has anything to show. Call `unregister_push` before erasing.
4. **Android.** The relay already sends FCM and the core can register a token; what is left is to add Firebase to
   the app and call `registerPush("fcm", token)`.
5. **Missing tests**: end-to-end wake-up by push on the app side (`woken`, `quietRuns`, registration after pairing);
   on the relay side, the wiring with one failing sender and the other healthy.
6. **Notification Service extension** (see 2) if we want the real notification on arrival, without depending on the
   app being woken.

## 9. Existing tests

| Where | What |
|---|---|
| `apps/relay/src/apns.rs` (9 tests) | Constant body, headers per urgency, classification of responses, provider token signed and verified, renewal at 50 minutes, refused token re-signed without going under 20 minutes, endpoint per environment, non-hexadecimal token never sent, unusable keys and identifiers refused. |
| `apps/relay/src/push.rs` | Each registration goes to the sender of its provider; a provider that is not configured sends nothing. |
| `apps/relay/tests/realtime.rs` | `PUT /v1/me/push` with `apns` and `apns-sandbox`, refusal of a non-hexadecimal token. |
| `apps/apple/TomiTests/PushRegistrationTests.swift` | Environment read from a signing profile, fixed texts present in the app. |

Commands: `cargo test -p tomi-relay`, `cargo test -p tomi-core --features demo`, iOS app tests on the simulator
(`apps/apple/README.md`).
