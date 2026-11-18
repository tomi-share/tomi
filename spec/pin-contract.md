# Pin: core contract for the apps (Android, Windows)

Status: **frozen contract** for clients. Format and protocol: `format-v1.md` §8.8,
`api-v1.md` §4.8 (`POST /v1/item-states`), §7.3, §7.7, §7.8, §8.1. Vectors: `vectors/v1/pin.json`.

Apps **never** handle the signed statement, the encrypted envelope or the convergence timestamp:
they call `pin` / `unpin` and read `pinned` on the item. Everything else (signature, encryption,
outgoing queue, convergence, history transfer) lives in the core.

## 1. What the apps call

### Android (`apps/android/.../core/TomiCore.kt`, `Model.kt`)

```kotlin
// Model.kt: two fields added to TomiItem (default values = not pinned)
data class TomiItem(
    // … existing fields unchanged …
    val pinned: Boolean = false,
    /** Time of the decision to pin (used to sort the "Pinned" section), null if not pinned. */
    val pinnedAt: Instant? = null,
)

// TomiCore.kt: two methods added
interface TomiCore {
    /** Pins the item on all devices. Idempotent; immediate local effect (items re-emitted). */
    suspend fun pin(id: String)
    /** Unpins the item on all devices. Idempotent; immediate local effect. */
    suspend fun unpin(id: String)
}
```

### Windows (`apps/windows/src-tauri/src/core_api.rs`, `model.rs`)

```rust
// model.rs: fields added to Item
#[serde(default)]
pub pinned: bool,
/// ms since the Unix epoch; None if not pinned.
#[serde(default)]
pub pinned_at: Option<i64>,

// core_api.rs: methods added to the TomiCore trait
async fn pin(&self, id: &str) -> Result<(), CoreError>;
async fn unpin(&self, id: &str) -> Result<(), CoreError>;
```

On the TypeScript side (`api.ts`), two Tauri commands `pin_item(id)` / `unpin_item(id)`; the name
`set_pinned` is already taken by pinning the popover window, which is unrelated.

## 2. Semantics guaranteed by the core

| Case | Behavior |
|-----|--------------|
| `pin` on an already pinned item (or `unpin` on a non-pinned item) | nothing is sent, normal return |
| Exclusion with archive | pinning an **archived** item unarchives it first (an "unarchive" event then a "pin" event, a single local step), and archiving a pinned item unpins it first. If both states are true after a cross-device conflict, the pin wins in the display (`pinned = true`, `archived = false`). Details: `archive-contract.md` §2 |
| `id` unknown locally | no effect, normal return |
| Offline | immediate local effect; the decision leaves through the outgoing queue as soon as the network is back |
| Concurrent decisions on two devices | all devices converge to the most recent one (last-writer-wins, tie → pinned) |
| `items` stream | stays sorted from most recent to oldest; the modified item is re-emitted with `pinned` up to date. The "Pinned" section is built by the UI (sorted by `pinnedAt` descending) |
| Item with an explicit expiration (`expiresAt`, "after reading") | pinning never extends it: the item disappears at `expiresAt`, or at the first read, pinned or not |
| Relay purge | unchanged; a pinned item stays in the local feed after the purge |
| Files of a pinned item | pinning triggers **no** download (neither on `pin`, nor on receiving a pin) and does not change the `release`: otherwise the relay would learn which item is pinned. Files follow the usual rule (opening, or automatic on Wi-Fi below the threshold). A file never downloaded remains available until the relay expiry; the UI SHOULD indicate it on a pinned item ("File not downloaded, available until …") |
| Timestamp (`pinnedAt`) | integer of milliseconds **≤ 2^53 − 1** (9 007 199 254 740 991): fits a Kotlin `Long`, a Rust `i64` and a TypeScript `number` without loss. The core refuses to sign or read beyond it (`malformed_statement`) |
| Local cleanup (old files, secret review) | a pinned item is never proposed or erased |
| Deletion (local or everywhere) | the item and its pin disappear |
| History transfer | pins are transferred; the new device gets them |
| Unreadable item (`ItemContent.Unreadable`) | pinnable; the pin applies once the item becomes readable |
| Secret | pinnable; it stays masked (title included). The relay does not know that it is pinned |

Errors: a network error is never an error of `pin` / `unpin` (the decision leaves through
the outgoing queue). The following can be raised (`TomiCoreException`): a local storage error
(`Storage`); `InvalidInput` if the device is not yet in a space; a refusal to sign the decision
(`Crypto` with the code `malformed_statement` when the timestamp would exceed 2^53 − 1 ms, or a trust
code if this device is no longer a recognized signer).

## 3. What the Rust core already provides (`crates/core`, module `pin`)

The facade called by the apps (local database and sync layer, still to be written) relies on these
public types:

```rust
pub struct PinState { pub pinned: bool, pub at_ms: u64 }       // converged state of an item
impl PinState {
    pub fn supersedes(&self, current: &PinState) -> bool;      // order (at_ms, pinned)
    pub fn merge(self, other: PinState) -> PinState;
    pub fn next_at_ms(current: Option<PinState>, now_ms: u64) -> Result<u64>; // > PIN_AT_MS_MAX → malformed_statement
}
pub const PIN_AT_MS_MAX: u64 = (1 << 53) - 1;                   // bound of at_ms (reading and signing)

pub struct PinBook;                                             // item_id -> PinState, to be persisted
impl PinBook {
    pub fn new() -> Self;
    pub fn state(&self, item_id: &str) -> Option<PinState>;     // TomiItem.pinned / pinnedAt
    pub fn is_pinned(&self, item_id: &str) -> bool;
    pub fn pinned_items(&self) -> Vec<String>;                  // most recently pinned first; may name
                                                                // absent items: intersect
    pub fn pinned_items_present(&self, is_present: impl Fn(&str) -> bool) -> Vec<String>; // already intersected
    pub fn set_pinned(&mut self, keys: &DeviceKeys, trust: &TrustStore, current_key: &SpaceKey,
                      item_id: &str, pinned: bool, now_ms: u64)
        -> Result<Option<OutgoingItemState>>;                   // pin(id) / unpin(id)
    pub fn apply_statement(&mut self, trust: &TrustStore, s: &PinStatement) -> Result<bool>;
    pub fn merge(&mut self, item_id: &str, state: PinState) -> Result<bool>;   // history import
    pub fn forget(&mut self, item_id: &str) -> Option<PinState>;              // tombstone
    pub fn to_json(&self) -> Result<String>;
    pub fn from_json(json: &str) -> Result<Self>;
}

pub struct OutgoingItemState {                                  // to be placed in the outgoing queue
    pub event_id: String, pub key_epoch: u32, pub envelope: Vec<u8>, pub statement: PinStatement,
}
impl OutgoingItemState {
    pub fn reissue(&self, current_key: &SpaceKey) -> Result<OutgoingItemState>; // after 409 event_id_conflict:
}                                                               // same statement, new event_id

impl SpaceKeyRing {                                             // received `item_state` event
    pub fn open_received_state(&self, envelope: &[u8], event_id: &str, seq: u64) -> ReceivedItemState;
}
pub fn seal_item_state(s: &PinStatement, event_id: &str, key: &SpaceKey) -> Result<Vec<u8>>; // re-sealing after stale_key_epoch

pub fn local_retention(user_expires_at_ms: Option<i64>, pinned: bool, received_at_ms: i64,
                       now_ms: i64, cleanup_before_ms: Option<i64>) -> LocalRetention;

pub struct HistoryItem { /* … */ pub pin: Option<PinState> }   // history export / import
```
