# Archive: core contract for the apps (Android, Windows)

Status: **frozen contract** for clients. Twin of [`pin-contract.md`](pin-contract.md).
Format and protocol: `format-v1.md` §8.9 (and §8.8 for the item state envelope),
`api-v1.md` §1.2, §4.8 (`POST /v1/item-states`), §7.3. Vectors: `vectors/v1/archive.json`.

Archiving means **the user is done with this item**: it leaves "Recent" on all of the user's devices
and stays intact, browsable in "Archives" and by search. Apps **never** handle the signed statement,
the encrypted envelope or the timestamp: they call `archive` / `unarchive` and read `archived` on the
item. The "Recent" / "Archives" filters are **UI-side**: the core always returns all items.

## 1. What the apps call

### Android (`apps/android/.../core/TomiCore.kt`, `Model.kt`)

UniFFI (`RustTomiCore`, `FfiItem`) exposes, in camelCase on the Kotlin side:

```kotlin
// Model.kt: two fields added to TomiItem (default values = not archived)
data class TomiItem(
    // … existing fields unchanged …
    val archived: Boolean = false,
    /** Time of the decision to archive (ms), null if not archived. */
    val archivedAt: Instant? = null,
)

interface TomiCore {
    /** Archives the item on all devices. Idempotent; immediate local effect (items re-emitted). */
    suspend fun archive(id: String)
    /** Unarchives the item on all devices. Idempotent; immediate local effect. */
    suspend fun unarchive(id: String)
}
```

### Windows (`apps/windows/src-tauri/src/core_api.rs`, `model.rs`)

```rust
// model.rs: fields added to Item
#[serde(default)]
pub archived: bool,
/// ms since the Unix epoch; None if not archived.
#[serde(default)]
pub archived_at: Option<i64>,

// core_api.rs: methods added to the TomiCore trait
async fn archive(&self, id: &str) -> Result<(), CoreError>;
async fn unarchive(&self, id: &str) -> Result<(), CoreError>;
```

Rust core side: `Engine::archive(&self, item_id: &str) -> Result<()>`,
`Engine::unarchive(&self, item_id: &str) -> Result<()>`, `ItemView.archived: bool`,
`ItemView.archived_at_ms: Option<i64>`. FFI: `RustTomiCore::archive(id: String)`,
`RustTomiCore::unarchive(id: String)`, `FfiItem.archived: bool`, `FfiItem.archived_at_ms:
Option<i64>`.

## 2. Semantics guaranteed by the core

| Case | Behavior |
|-----|--------------|
| `archive` on an already archived item (or `unarchive` on a non-archived item) | nothing is sent, normal return |
| `id` unknown locally (or deleted) | no effect, normal return |
| Offline | immediate local effect; the decision leaves through the outgoing queue as soon as the network is back |
| Concurrent decisions | all devices converge to the most recent one (last-writer-wins, tie → archived) |
| `items` stream | the modified item is re-emitted (change event) with `archived` / `archivedAt` up to date; it stays in the list |
| **Marked as read?** | **Never.** Archiving sends neither `read`, `ack` nor `release`, and does not consume a "After reading" secret |
| Download, relay purge, expiration | unchanged: archiving downloads nothing, changes neither the relay purge nor `expiresAt`; an archived item expires on its date |
| Local cleanup (old files, secret review) | **Planned, not wired up**: local cleanup (`LocalRetention::Cleanup`) is not called anywhere in the engine today (maintenance does not enable it), so nothing is erased or proposed for that reason. The planned rule, already expressed by the pure function `local_retention`: archiving **does not protect** (unlike pinning); archived secrets will be included in the review |
| Pinned → archive | the core unpins first (a "unpin" event then an "archive" event, a single local step) |
| Archived → pin | the core unarchives first ("unarchive" then "pin") |
| Conflict (pinned elsewhere while archiving) | if both registers are true, `pinned = true` and **`archived = false`**: the pin wins in the display (the view never reports an item as both pinned and archived). The archive register stays true internally: the item reappears in "Archives" as soon as it is unpinned |
| Unreadable item (`Unreadable`) | archivable |
| Item pending in the queue or failed to send | the core accepts; **the UI does not offer it** (a send failure must remain visible) |
| Secret | archivable without biometrics; stays masked (title included); none of its content leaves |
| Timestamp (`archivedAt`) | integer of milliseconds **≤ 2^53 − 1**: `Long` in Kotlin, `i64` in Rust, `number` in TypeScript without loss |
| Deletion (local or everywhere, expiration) | the item and its archive state disappear; an archive event received for a deleted item is ignored |
| History transfer | the archive state is transferred, secrets included, with the item (or merged if the item is already present) |
| Local notification | the app must remove it without marking as read (the core does not touch notifications) |

Errors: a network error is never an error of `archive` / `unarchive`. The following can be raised
(`TomiCoreException`): `Storage`; `InvalidInput` if the device is not in a space; `Crypto` with the
code `malformed_statement` (timestamp beyond 2^53 − 1 ms) or a trust code (device plus an unrecognized
signer).

## 3. What the UI must do with these fields

- **Recent** = non-deleted items with `!pinned && !archived`; **Pinned** = `pinned`
  (sorted by `pinnedAt` descending); **Archives** = `archived` (sorted by `createdAt` descending,
  or by `archivedAt` if the UI prefers "last archived"). The three places are mutually exclusive.
- Search includes archived items (an "Archived" badge), following the usual "Masked" rules.
- "Undo" after an archive = `unarchive(id)`; if the item was pinned, call `pin(id)` again
  (which unarchives first).
- Never call `markRead` when archiving.

## 4. What the Rust core provides (`crates/core`, module `archive`)

```rust
pub struct ArchiveState { pub archived: bool, pub at_ms: u64 }
impl ArchiveState {
    pub fn supersedes(&self, current: &ArchiveState) -> bool;   // order (at_ms, archived)
    pub fn merge(self, other: ArchiveState) -> ArchiveState;
    pub fn next_at_ms(current: Option<ArchiveState>, now_ms: u64) -> Result<u64>;
}
pub const ARCHIVE_AT_MS_MAX: u64 = (1 << 53) - 1;
pub const ARCHIVE_STATEMENT_LEN: usize = 157;

pub struct ArchiveStatement { pub space_id, pub item_id, pub at_ms: u64, pub archived: bool,
                              pub signer_ed25519: Vec<u8>, pub signature: Vec<u8> }
impl DeviceKeys { pub fn sign_archive(&self, space_id, item_id, at_ms, archived) -> Result<ArchiveStatement>; }
impl TrustStore { pub fn verify_archive(&self, s: &ArchiveStatement) -> Result<()>; }

pub struct ArchiveBook;                                          // item_id -> ArchiveState
impl ArchiveBook {
    pub fn state(&self, item_id: &str) -> Option<ArchiveState>;
    pub fn is_archived(&self, item_id: &str) -> bool;
    pub fn set_archived(&mut self, keys, trust, current_key, item_id, archived: bool, now_ms: u64)
        -> Result<Option<OutgoingItemState>>;
    pub fn apply_statement(&mut self, trust, s: &ArchiveStatement) -> Result<bool>;
    pub fn merge(&mut self, item_id: &str, state: ArchiveState) -> Result<bool>;   // history import
    pub fn forget(&mut self, item_id: &str) -> Option<ArchiveState>;               // tombstone
}

pub enum ItemStateKind { Pin(PinStatement), Archive(ArchiveStatement), Unknown }  // open_item_state
pub enum StateStatement { Pin(PinStatement), Archive(ArchiveStatement) }          // OutgoingItemState.statement
pub struct HistoryItem { /* … */ pub pin: Option<PinState>, pub archive: Option<ArchiveState> }
```

`seal_item_state` accepts a `PinStatement`, an `ArchiveStatement` or a `StateStatement`
(trait `ItemStateWire`). SQLite persistence: table `archives` (schema v3, additive migration
from v1 and v2).
