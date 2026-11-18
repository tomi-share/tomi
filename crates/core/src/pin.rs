//! Pinned items, shared by every device of the space (spec §8.8; decision "Pin").
//!
//! A pin is **protocol state**, not a local preference. It travels as an encrypted
//! *item-state event*:
//!
//! * the device signs a [`PinStatement`] `{ space_id, item_id, at_ms, pinned }` with its
//!   Ed25519 key (author identity, revoked devices refused, like a tombstone);
//! * the statement is padded to 256 bytes and sealed under a subkey of the current space
//!   key ([`seal_item_state`]). The relay only sees a random `event_id`, the sender, the
//!   key epoch and a constant size: **neither the `item_id` nor the pinned flag**;
//! * every device converges with last-writer-wins on the signed `at_ms` ([`PinState`],
//!   [`PinBook`]), the same kind of timestamped event as a deletion tombstone.
//!
//! A pin never changes the relay purge (decision (a)): it only makes each device keep and
//! show its local copy ([`local_retention`]). It never overrides an explicit expiration.
//! It never triggers a download either (spec §8.8.4): which files a device fetches, and
//! when, does not depend on the pin state, so blob downloads reveal nothing about it.
//!
//! `at_ms` is bounded by [`PIN_AT_MS_MAX`] (2^53 - 1) so that it compares the same way
//! in Rust, in a signed Kotlin `Long` and in a JavaScript number.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::archive::ArchiveStatement;
use crate::crypto::{
    aead_open, aead_seal, format_uuid, labels, new_uuid, parse_uuid, random_array, to_array,
    Reader, SpaceKey, Writer, NONCE_LEN,
};
use crate::device::{device_id_from_ed25519, verify_signature, DeviceKeys, TrustStore};
use crate::device::{PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::envelope::{pad_payload, unpad_payload};
use crate::error::{CoreError, Result};
use crate::format::{is_format_supported, FORMAT_MAX, FORMAT_MIN, PAYLOAD_PAD_BLOCK, TAG_LEN};
use crate::keyring::{SpaceKeyRing, UnreadableReason};

/// Wire length of a [`PinStatement`]: `TBS (89) || signature (64)`.
pub const PIN_STATEMENT_LEN: usize =
    labels::SIG_PIN.len() + 1 + 16 + 16 + 8 + 1 + PUBLIC_KEY_LEN + SIGNATURE_LEN;
/// Clear header of an item-state envelope: `format || key_epoch || event_id || nonce`.
pub const ITEM_STATE_HEADER_LEN: usize = 4 + 4 + 16 + NONCE_LEN;
/// Largest padded plaintext of an item-state envelope (room for future state kinds).
pub const MAX_ITEM_STATE_PADDED_LEN: usize = 2048;
/// Length of the envelope of a pin event (one 256-byte padding block).
pub const ITEM_STATE_ENVELOPE_LEN: usize = ITEM_STATE_HEADER_LEN + PAYLOAD_PAD_BLOCK + TAG_LEN;

/// Largest valid `at_ms` of a pin decision: 2^53 - 1 (spec §8.8.1). A larger value is
/// refused with `malformed_statement` when signing and when reading, so that every
/// platform (signed 64-bit integers, JavaScript numbers) orders states identically.
pub const PIN_AT_MS_MAX: u64 = (1 << 53) - 1;

const STATE_UNPINNED: u8 = 0x00;
const STATE_PINNED: u8 = 0x01;

/// `malformed_statement` if `at_ms` exceeds [`PIN_AT_MS_MAX`].
fn check_at_ms(at_ms: u64) -> Result<()> {
    if at_ms > PIN_AT_MS_MAX {
        return Err(CoreError::statement("pin: at_ms above 2^53 - 1"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Converging state (spec §8.8.3)
// ---------------------------------------------------------------------------

/// Pin state of one item: the value and the signed time of the decision that set it.
///
/// Order (last-writer-wins): the larger `at_ms` wins; on a tie, `pinned = true` wins
/// (keeping is the safer outcome). `(at_ms, pinned)` is a total order, so every device
/// reaches the same state whatever the order in which it receives the events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PinState {
    pub pinned: bool,
    /// Signed decision time, ms since the Unix epoch. `0` = never pinned (default).
    pub at_ms: u64,
}

impl PinState {
    fn key(self) -> (u64, bool) {
        (self.at_ms, self.pinned)
    }

    /// `true` if `self` replaces `current` under last-writer-wins.
    pub fn supersedes(&self, current: &PinState) -> bool {
        self.key() > current.key()
    }

    /// The winner of `self` and `other`.
    pub fn merge(self, other: PinState) -> PinState {
        if other.supersedes(&self) {
            other
        } else {
            self
        }
    }

    /// Time to sign for a new user decision when the current state is `current`:
    /// `max(now_ms, current.at_ms + 1)`. A decision therefore always supersedes the
    /// state the user saw, even if this device's clock is behind the one that set it.
    ///
    /// Fails with `malformed_statement` if the result would exceed [`PIN_AT_MS_MAX`]
    /// (a state already at the bound can then only be removed by a tombstone).
    pub fn next_at_ms(current: Option<PinState>, now_ms: u64) -> Result<u64> {
        let at_ms = match current {
            Some(c) => now_ms.max(c.at_ms.saturating_add(1)),
            None => now_ms,
        };
        check_at_ms(at_ms)?;
        Ok(at_ms)
    }
}

// ---------------------------------------------------------------------------
// Signed statement (spec §8.8.1)
// ---------------------------------------------------------------------------

/// `{ space_id, item_id, at_ms, pinned }` signed by a member device. Never sent in clear:
/// it only travels inside an item-state envelope ([`seal_item_state`]) or, as a bare
/// [`PinState`], inside a sealed history transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinStatement {
    pub space_id: String,
    pub item_id: String,
    pub at_ms: u64,
    pub pinned: bool,
    pub signer_ed25519: Vec<u8>,
    pub signature: Vec<u8>,
}

impl PinStatement {
    fn tbs(&self) -> Result<Vec<u8>> {
        check_at_ms(self.at_ms)?;
        let sid = parse_uuid(&self.space_id, "space id")?;
        let iid = parse_uuid(&self.item_id, "item id")?;
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&self.signer_ed25519, "signer")?;
        let mut w = Writer::labeled(labels::SIG_PIN);
        w.bytes(&sid)
            .bytes(&iid)
            .u64(self.at_ms)
            .u8(if self.pinned {
                STATE_PINNED
            } else {
                STATE_UNPINNED
            })
            .bytes(&signer);
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature` ([`PIN_STATEMENT_LEN`] bytes).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.signature.len() != SIGNATURE_LEN {
            return Err(CoreError::input("signature: expected 64 bytes"));
        }
        let mut out = self.tbs()?;
        out.extend_from_slice(&self.signature);
        Ok(out)
    }

    /// Parses the wire form (does not verify the signature).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PIN_STATEMENT_LEN {
            return Err(CoreError::statement("pin: wrong length"));
        }
        let mut r = Reader::new(bytes);
        r.expect_label(labels::SIG_PIN)?;
        let sid: [u8; 16] = r.array()?;
        let iid: [u8; 16] = r.array()?;
        let at_ms = r.u64()?;
        check_at_ms(at_ms)?;
        let pinned = match r.u8()? {
            STATE_UNPINNED => false,
            STATE_PINNED => true,
            _ => return Err(CoreError::statement("pin: unknown state")),
        };
        let signer: [u8; PUBLIC_KEY_LEN] = r.array()?;
        let sig = r.take(SIGNATURE_LEN)?;
        Ok(Self {
            space_id: format_uuid(&sid),
            item_id: format_uuid(&iid),
            at_ms,
            pinned,
            signer_ed25519: signer.to_vec(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `signer_ed25519` only. Devices must use
    /// [`TrustStore::verify_pin`] (signer trusted and not revoked).
    pub fn verify_signature(&self) -> Result<()> {
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&self.signer_ed25519, "signer")?;
        verify_signature(&signer, &self.tbs()?, &self.signature)
    }

    /// The state this statement sets.
    pub fn state(&self) -> PinState {
        PinState {
            pinned: self.pinned,
            at_ms: self.at_ms,
        }
    }

    /// `device_id` of the signer.
    pub fn signer_device_id(&self) -> Result<String> {
        device_id_from_ed25519(&self.signer_ed25519)
    }
}

impl DeviceKeys {
    /// Signs a pin (`pinned = true`) or unpin (`false`) decision for `item_id`.
    /// `at_ms` above [`PIN_AT_MS_MAX`] → `malformed_statement`.
    pub fn sign_pin(
        &self,
        space_id: &str,
        item_id: &str,
        at_ms: u64,
        pinned: bool,
    ) -> Result<PinStatement> {
        let mut s = PinStatement {
            space_id: space_id.to_owned(),
            item_id: item_id.to_owned(),
            at_ms,
            pinned,
            signer_ed25519: self.ed_public().to_vec(),
            signature: Vec::new(),
        };
        s.signature = self.sign(&s.tbs()?);
        Ok(s)
    }
}

impl TrustStore {
    /// Verifies a pin statement: same space (`space_mismatch`), signer trusted and not
    /// revoked (`untrusted_signer` / `revoked_device`), signature (`invalid_signature`).
    ///
    /// Revocation is evaluated against this store **now**, as for every signed statement
    /// (spec §8.4), never at the `seq` of the event. A pin kept raw in `pending_states`
    /// and reopened after the rotation that revokes its signer is therefore refused, even
    /// if its `seq` precedes that rotation: a known, accepted exception (spec §8.8.2).
    pub fn verify_pin(&self, statement: &PinStatement) -> Result<()> {
        if parse_uuid(&statement.space_id, "space id")? != self.space_id_bytes() {
            return Err(CoreError::SpaceMismatch);
        }
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&statement.signer_ed25519, "signer")?;
        self.require_signer(&signer)?;
        statement.verify_signature()
    }
}

// ---------------------------------------------------------------------------
// Encrypted item-state envelope (spec §8.8.2)
// ---------------------------------------------------------------------------

/// `AAD_state` (spec §8.8.2).
pub(crate) fn aad_item_state(format: u32, key_epoch: u32, event_id: &[u8; 16]) -> Vec<u8> {
    let mut w = Writer::labeled(labels::AAD_ITEM_STATE);
    w.u32(format).u32(key_epoch).bytes(event_id);
    w.finish()
}

/// Clear header of an item-state envelope (what the relay sees).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStateHeader {
    pub format: u32,
    pub key_epoch: u32,
    /// Random id chosen by the sender (idempotence on the relay). Not the item id.
    pub event_id: String,
}

/// Content of an opened item-state envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemStateKind {
    /// A pin / unpin decision (still to be checked with [`TrustStore::verify_pin`]).
    Pin(PinStatement),
    /// An archive / unarchive decision (still to be checked with
    /// [`TrustStore::verify_archive`], spec §8.9).
    Archive(ArchiveStatement),
    /// A state kind added by a later version: it changes nothing here (additive change), but
    /// the engine keeps the raw envelope in `pending_states` and replays it after an update
    /// (spec §8.8.2, step 6).
    Unknown,
}

/// A signed statement that can be sealed in an item-state envelope.
pub trait ItemStateWire {
    /// The wire form (`TBS || signature`), before padding.
    fn wire_bytes(&self) -> Result<Vec<u8>>;
}

impl ItemStateWire for PinStatement {
    fn wire_bytes(&self) -> Result<Vec<u8>> {
        self.to_bytes()
    }
}

impl ItemStateWire for ArchiveStatement {
    fn wire_bytes(&self) -> Result<Vec<u8>> {
        self.to_bytes()
    }
}

/// A statement of any known kind, as kept in the outbox and in [`OutgoingItemState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateStatement {
    Pin(PinStatement),
    Archive(ArchiveStatement),
}

impl StateStatement {
    /// Parses a wire form, dispatching on its label. Unknown label → `malformed_statement`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if starts_with_label(bytes, labels::SIG_PIN) {
            Ok(Self::Pin(PinStatement::from_bytes(bytes)?))
        } else if starts_with_label(bytes, labels::SIG_ARCHIVE) {
            Ok(Self::Archive(ArchiveStatement::from_bytes(bytes)?))
        } else {
            Err(CoreError::statement("unknown item state kind"))
        }
    }

    /// Signed decision time.
    pub fn at_ms(&self) -> u64 {
        match self {
            Self::Pin(s) => s.at_ms,
            Self::Archive(s) => s.at_ms,
        }
    }

    /// The item this decision is about.
    pub fn item_id(&self) -> &str {
        match self {
            Self::Pin(s) => &s.item_id,
            Self::Archive(s) => &s.item_id,
        }
    }
}

impl ItemStateWire for StateStatement {
    fn wire_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Pin(s) => s.to_bytes(),
            Self::Archive(s) => s.to_bytes(),
        }
    }
}

/// `label || 0x00` prefix test.
fn starts_with_label(bytes: &[u8], label: &[u8]) -> bool {
    bytes.len() > label.len() && bytes.starts_with(label) && bytes[label.len()] == 0
}

/// Result of [`open_item_state`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedItemState {
    pub format: u32,
    pub key_epoch: u32,
    pub event_id: String,
    pub state: ItemStateKind,
}

pub(crate) fn seal_item_state_raw(
    statement: &[u8],
    event_id: &[u8; 16],
    space_key: &SpaceKey,
    nonce: &[u8; NONCE_LEN],
) -> Result<Vec<u8>> {
    let padded = Zeroizing::new(pad_payload(statement));
    if padded.len() > MAX_ITEM_STATE_PADDED_LEN {
        return Err(CoreError::input("item state too long"));
    }
    let format = FORMAT_MAX;
    let epoch = space_key.epoch();
    let key = space_key.item_state_key()?;
    let ct = aead_seal(
        &key,
        nonce,
        &aad_item_state(format, epoch, event_id),
        &padded,
    )?;
    let mut out = Vec::with_capacity(ITEM_STATE_HEADER_LEN + ct.len());
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&epoch.to_be_bytes());
    out.extend_from_slice(event_id);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Seals a pin or archive statement as the item-state event `event_id` under the current
/// space key ([`ITEM_STATE_ENVELOPE_LEN`] bytes, fresh random nonce).
pub fn seal_item_state<S: ItemStateWire + ?Sized>(
    statement: &S,
    event_id: &str,
    space_key: &SpaceKey,
) -> Result<Vec<u8>> {
    let eid = parse_uuid(event_id, "event id")?;
    seal_item_state_raw(&statement.wire_bytes()?, &eid, space_key, &random_array()?)
}

/// Reads the clear header. Fails with `unsupported_format` for an unknown format (the
/// client then keeps the raw event and retries after an update), and with
/// `malformed_envelope` unless the length is exactly `48 + 256·k + 16`, `1 ≤ k ≤ 8`.
///
/// The relay uses it as its format-1 check of `POST /v1/item-states` (api-v1.md §4.8):
/// `malformed_envelope` → `422 validation_failed`; `unsupported_format` → no other check.
pub fn read_item_state_header(envelope: &[u8]) -> Result<ItemStateHeader> {
    let format_bytes: [u8; 4] = envelope
        .get(..4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| CoreError::envelope("shorter than 4 bytes"))?;
    let format = u32::from_be_bytes(format_bytes);
    if !is_format_supported(format) {
        return Err(CoreError::UnsupportedFormat {
            format,
            min: FORMAT_MIN,
            max: FORMAT_MAX,
        });
    }
    let body = envelope
        .len()
        .checked_sub(ITEM_STATE_HEADER_LEN + TAG_LEN)
        .ok_or_else(|| CoreError::envelope("too short"))?;
    if body < PAYLOAD_PAD_BLOCK
        || !body.is_multiple_of(PAYLOAD_PAD_BLOCK)
        || body > MAX_ITEM_STATE_PADDED_LEN
    {
        return Err(CoreError::envelope("item state: invalid length"));
    }
    let key_epoch = u32::from_be_bytes(to_array(&envelope[4..8], "key_epoch")?);
    let event_id: [u8; 16] = to_array(&envelope[8..24], "event_id")?;
    Ok(ItemStateHeader {
        format,
        key_epoch,
        event_id: format_uuid(&event_id),
    })
}

/// Decrypts an item-state envelope listed by the relay under `expected_event_id`, with
/// the space key of its epoch. The returned [`PinStatement`] is **not yet trusted**: check
/// it with [`TrustStore::verify_pin`] (or use [`PinBook::apply_statement`]).
pub fn open_item_state(
    envelope: &[u8],
    expected_event_id: &str,
    space_key: &SpaceKey,
) -> Result<OpenedItemState> {
    let header = read_item_state_header(envelope)?;
    let eid: [u8; 16] = to_array(&envelope[8..24], "event_id")?;
    // Compare the 16 bytes, not the text: the announced id comes from the caller (API
    // JSON, local storage) and may differ in case from the canonical form.
    let announced = uuid::Uuid::parse_str(expected_event_id).map(|u| *u.as_bytes());
    if announced != Ok(eid) {
        return Err(CoreError::EventIdMismatch);
    }
    if header.key_epoch != space_key.epoch() {
        return Err(CoreError::KeyEpochMismatch {
            envelope: header.key_epoch,
            key: space_key.epoch(),
        });
    }
    let nonce: [u8; NONCE_LEN] = to_array(&envelope[24..ITEM_STATE_HEADER_LEN], "nonce")?;
    let key = space_key.item_state_key()?;
    let padded = Zeroizing::new(aead_open(
        &key,
        &nonce,
        &aad_item_state(header.format, header.key_epoch, &eid),
        &envelope[ITEM_STATE_HEADER_LEN..],
    )?);
    let inner = unpad_payload(&padded)?;
    let state = if starts_with_label(inner, labels::SIG_PIN) {
        ItemStateKind::Pin(PinStatement::from_bytes(inner)?)
    } else if starts_with_label(inner, labels::SIG_ARCHIVE) {
        ItemStateKind::Archive(ArchiveStatement::from_bytes(inner)?)
    } else {
        ItemStateKind::Unknown
    };
    Ok(OpenedItemState {
        format: header.format,
        key_epoch: header.key_epoch,
        event_id: header.event_id,
        state,
    })
}

/// Outcome of opening a received item-state event: never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivedItemState {
    Opened(Box<OpenedItemState>),
    /// `UpdateRequired` / `KeyPending`: keep the raw envelope and retry (after an update,
    /// after the next rotation). `SuspectEpoch` / `Undecryptable`: drop it (log without
    /// content). Unlike an item, a state event is never shown to the user.
    Unreadable(UnreadableReason),
}

impl SpaceKeyRing {
    /// Opens an `item_state` event listed at `seq`, applying the same epoch order rule
    /// as for items (spec §9.4, §12).
    pub fn open_received_state(
        &self,
        envelope: &[u8],
        announced_event_id: &str,
        seq: u64,
    ) -> ReceivedItemState {
        let header = match read_item_state_header(envelope) {
            Ok(h) => h,
            Err(e) => return ReceivedItemState::Unreadable(UnreadableReason::from_error(&e)),
        };
        let key = match self.key_for_item(header.key_epoch, seq) {
            Ok(k) => k,
            Err(reason) => return ReceivedItemState::Unreadable(reason),
        };
        match open_item_state(envelope, announced_event_id, key) {
            Ok(opened) => ReceivedItemState::Opened(Box::new(opened)),
            Err(e) => ReceivedItemState::Unreadable(UnreadableReason::from_error(&e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Local pin book (spec §8.8.3, api-v1.md §7.3)
// ---------------------------------------------------------------------------

/// What to upload after a local pin decision (`POST /v1/item-states`, kept in the
/// outbox until accepted; replaying it is idempotent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingItemState {
    /// Random UUID of the event (relay idempotence key).
    pub event_id: String,
    pub key_epoch: u32,
    /// [`ITEM_STATE_ENVELOPE_LEN`] bytes.
    pub envelope: Vec<u8>,
    /// Signed statement (pin or archive), kept so that a `stale_key_epoch` answer can be
    /// re-sealed under the new key ([`seal_item_state`], same `event_id`) without
    /// re-signing.
    pub statement: StateStatement,
}

impl OutgoingItemState {
    /// The same signed decision under a **new** random `event_id`, sealed under
    /// `current_key`: what the outbox sends after `409 event_id_conflict` instead of
    /// dropping the decision (api-v1.md §7.7). The `event_id` is not signed, so the
    /// statement (and its `at_ms`) is unchanged: re-sending it can never override a
    /// later decision, and a duplicate is harmless under last-writer-wins.
    pub fn reissue(&self, current_key: &SpaceKey) -> Result<OutgoingItemState> {
        let event_id = new_uuid()?;
        let envelope = seal_item_state(&self.statement, &event_id, current_key)?;
        Ok(OutgoingItemState {
            event_id,
            key_epoch: current_key.epoch(),
            envelope,
            statement: self.statement.clone(),
        })
    }
}

/// Pin states known by this device, by `item_id`, independent of the items themselves:
/// a state may arrive before its item (history transfer, purged `item` event) and is
/// applied once the item exists. Persist it with [`to_json`](Self::to_json).
///
/// Wrap in a `Mutex`-holding object for UniFFI (methods take `&mut self`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinBook {
    states: BTreeMap<String, PinState>,
}

#[derive(Serialize, Deserialize)]
struct PersistedPins {
    version: u32,
    pins: Vec<PersistedPin>,
}

#[derive(Serialize, Deserialize)]
struct PersistedPin {
    item_id: String,
    pinned: bool,
    at: u64,
}

impl PinBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state of `item_id`, if any decision is known.
    pub fn state(&self, item_id: &str) -> Option<PinState> {
        self.states.get(item_id).copied()
    }

    /// `true` if `item_id` is currently pinned.
    pub fn is_pinned(&self, item_id: &str) -> bool {
        self.state(item_id).is_some_and(|s| s.pinned)
    }

    /// Pinned item ids, most recently pinned first (ties by item id).
    ///
    /// The book is independent of the items: it may hold states for items this device
    /// does not have (yet), e.g. a decision received before its item. Callers showing the
    /// "Pinned" section must intersect with their local items, or use
    /// [`pinned_items_present`](Self::pinned_items_present).
    pub fn pinned_items(&self) -> Vec<String> {
        self.pinned_items_present(|_| true)
    }

    /// [`pinned_items`](Self::pinned_items) restricted to the ids for which `is_present`
    /// returns `true` (typically: the item exists locally and is not tombstoned).
    pub fn pinned_items_present(&self, is_present: impl Fn(&str) -> bool) -> Vec<String> {
        let mut v: Vec<(&String, &PinState)> = self
            .states
            .iter()
            .filter(|(id, s)| s.pinned && is_present(id))
            .collect();
        v.sort_by(|a, b| b.1.at_ms.cmp(&a.1.at_ms).then_with(|| a.0.cmp(b.0)));
        v.into_iter().map(|(id, _)| id.clone()).collect()
    }

    /// Applies a state under last-writer-wins. Returns `true` if the state changed.
    /// Callers skip items already deleted (`tombstoned`) and call [`forget`](Self::forget)
    /// when a tombstone arrives. `state.at_ms` above [`PIN_AT_MS_MAX`] →
    /// `malformed_statement`.
    pub fn merge(&mut self, item_id: &str, state: PinState) -> Result<bool> {
        parse_uuid(item_id, "item id")?;
        check_at_ms(state.at_ms)?;
        match self.states.get_mut(item_id) {
            Some(cur) if !state.supersedes(cur) => Ok(false),
            Some(cur) => {
                *cur = state;
                Ok(true)
            }
            None => {
                self.states.insert(item_id.to_owned(), state);
                Ok(true)
            }
        }
    }

    /// Verifies a received statement ([`TrustStore::verify_pin`]) and merges it.
    pub fn apply_statement(
        &mut self,
        trust: &TrustStore,
        statement: &PinStatement,
    ) -> Result<bool> {
        trust.verify_pin(statement)?;
        self.merge(&statement.item_id, statement.state())
    }

    /// Drops the state of a deleted item (tombstone). Returns the former state.
    pub fn forget(&mut self, item_id: &str) -> Option<PinState> {
        self.states.remove(item_id)
    }

    /// User decision on this device: pins (`pinned = true`) or unpins `item_id`.
    ///
    /// Returns `None` if the item is already in that state (nothing to send). Otherwise
    /// signs the decision at [`PinState::next_at_ms`], applies it locally at once, and
    /// returns the sealed event to upload under the current space key.
    pub fn set_pinned(
        &mut self,
        keys: &DeviceKeys,
        trust: &TrustStore,
        current_key: &SpaceKey,
        item_id: &str,
        pinned: bool,
        now_ms: u64,
    ) -> Result<Option<OutgoingItemState>> {
        parse_uuid(item_id, "item id")?;
        let me = keys.ed_public();
        if me != trust.own_ed() {
            return Err(CoreError::input("keys do not belong to this store"));
        }
        trust.require_signer(&me)?;
        let current = self.state(item_id);
        if current.unwrap_or_default().pinned == pinned {
            return Ok(None);
        }
        let at_ms = PinState::next_at_ms(current, now_ms)?;
        let statement = keys.sign_pin(&trust.space_id(), item_id, at_ms, pinned)?;
        let event_id = new_uuid()?;
        let envelope = seal_item_state(&statement, &event_id, current_key)?;
        self.merge(item_id, statement.state())?;
        Ok(Some(OutgoingItemState {
            event_id,
            key_epoch: current_key.epoch(),
            envelope,
            statement: StateStatement::Pin(statement),
        }))
    }

    /// Serializes the book for local persistence (no secret, but private: it says which
    /// items matter; store it with the rest of the local database).
    pub fn to_json(&self) -> Result<String> {
        let p = PersistedPins {
            version: 1,
            pins: self
                .states
                .iter()
                .map(|(id, s)| PersistedPin {
                    item_id: id.clone(),
                    pinned: s.pinned,
                    at: s.at_ms,
                })
                .collect(),
        };
        serde_json::to_string(&p).map_err(|_| CoreError::Internal {
            reason: "json serialization failed".into(),
        })
    }

    /// Restores a book saved with [`to_json`](Self::to_json).
    pub fn from_json(json: &str) -> Result<Self> {
        let p: PersistedPins =
            serde_json::from_str(json).map_err(|_| CoreError::input("invalid pin book json"))?;
        if p.version != 1 {
            return Err(CoreError::input("unknown pin book version"));
        }
        let mut book = Self::new();
        for e in p.pins {
            book.merge(
                &e.item_id,
                PinState {
                    pinned: e.pinned,
                    at_ms: e.at,
                },
            )?;
        }
        Ok(book)
    }
}

// ---------------------------------------------------------------------------
// Local retention (spec §8.8.4)
// ---------------------------------------------------------------------------

/// What a device does with its local copy of an item during the periodic task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalRetention {
    /// Keep the item.
    Keep,
    /// The user expiration (payload `expires_at`) is reached: erase it, **pinned or not**.
    EraseExpired,
    /// Eligible to the automatic or batch cleanup the caller asked for (storage cleanup,
    /// review of old secrets). Never returned for a pinned item.
    Cleanup,
}

/// Local retention rule. A pin protects an item only against cleanups the user did not
/// target at it (`cleanup_before_ms`: items received before that date); it never
/// resurrects or extends an item with an explicit expiration. "After first read" is
/// consumed by a tombstone and is outside this function. The relay purge never erases a
/// local copy.
pub fn local_retention(
    user_expires_at_ms: Option<i64>,
    pinned: bool,
    received_at_ms: i64,
    now_ms: i64,
    cleanup_before_ms: Option<i64>,
) -> LocalRetention {
    if user_expires_at_ms.is_some_and(|t| now_ms >= t) {
        return LocalRetention::EraseExpired;
    }
    if pinned {
        return LocalRetention::Keep;
    }
    match cleanup_before_ms {
        Some(cutoff) if received_at_ms < cutoff => LocalRetention::Cleanup,
        _ => LocalRetention::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACE: &str = "11111111-1111-4111-8111-111111111111";
    const ITEM: &str = "22222222-2222-4222-8222-222222222222";

    fn stores() -> (DeviceKeys, DeviceKeys, TrustStore, TrustStore) {
        let a = DeviceKeys::generate().unwrap();
        let b = DeviceKeys::generate().unwrap();
        let mut ta = TrustStore::new(SPACE, &a.public(), 1).unwrap();
        ta.add_verified_peer(&b.public()).unwrap();
        let mut tb = TrustStore::new(SPACE, &b.public(), 1).unwrap();
        tb.add_verified_peer(&a.public()).unwrap();
        (a, b, ta, tb)
    }

    #[test]
    fn lengths() {
        assert_eq!(labels::SIG_PIN.len(), 15);
        assert_eq!(PIN_STATEMENT_LEN, 153);
        assert_eq!(ITEM_STATE_HEADER_LEN, 48);
        assert_eq!(ITEM_STATE_ENVELOPE_LEN, 320);
        assert_eq!(aad_item_state(1, 1, &[0; 16]).len(), 47);
    }

    #[test]
    fn last_writer_wins() {
        let p = |pinned, at_ms| PinState { pinned, at_ms };
        assert!(p(true, 2).supersedes(&p(false, 1)));
        assert!(!p(false, 1).supersedes(&p(true, 2)));
        assert!(p(true, 5).supersedes(&p(false, 5)));
        assert!(!p(false, 5).supersedes(&p(true, 5)));
        assert!(!p(true, 5).supersedes(&p(true, 5)));
        // Any order of the same events gives the same result.
        let evs = [p(true, 10), p(false, 30), p(true, 20), p(true, 30)];
        let fold = |order: &[usize]| {
            order
                .iter()
                .fold(PinState::default(), |acc, i| acc.merge(evs[*i]))
        };
        assert_eq!(fold(&[0, 1, 2, 3]), p(true, 30));
        assert_eq!(fold(&[3, 2, 1, 0]), p(true, 30));
        assert_eq!(fold(&[1, 0, 3, 2]), p(true, 30));
        assert_eq!(PinState::next_at_ms(None, 100).unwrap(), 100);
        assert_eq!(PinState::next_at_ms(Some(p(true, 500)), 100).unwrap(), 501);
        assert_eq!(PinState::next_at_ms(Some(p(true, 50)), 100).unwrap(), 100);
    }

    #[test]
    fn at_ms_bound() {
        let max = PIN_AT_MS_MAX;
        assert_eq!(max, 9_007_199_254_740_991);
        let p = |pinned, at_ms| PinState { pinned, at_ms };
        let malformed = |r: Result<u64>| r.unwrap_err().code();
        assert_eq!(PinState::next_at_ms(None, max).unwrap(), max);
        assert_eq!(
            malformed(PinState::next_at_ms(None, max + 1)),
            "malformed_statement"
        );
        assert_eq!(
            malformed(PinState::next_at_ms(Some(p(true, max)), 0)),
            "malformed_statement"
        );
        let a = DeviceKeys::generate().unwrap();
        assert!(a.sign_pin(SPACE, ITEM, max, true).is_ok());
        assert_eq!(
            a.sign_pin(SPACE, ITEM, max + 1, true).unwrap_err().code(),
            "malformed_statement"
        );
        let mut book = PinBook::new();
        assert!(book.merge(ITEM, p(true, u64::MAX)).is_err());
        assert!(book.state(ITEM).is_none());
    }

    #[test]
    fn pin_roundtrip_between_devices() {
        let (a, b, ta, tb) = stores();
        let key = SpaceKey::generate(1).unwrap();
        let mut book_a = PinBook::new();
        let out = book_a
            .set_pinned(&a, &ta, &key, ITEM, true, 1_000)
            .unwrap()
            .unwrap();
        assert!(book_a.is_pinned(ITEM));
        assert_eq!(out.envelope.len(), ITEM_STATE_ENVELOPE_LEN);
        assert_eq!(out.key_epoch, 1);
        // Already pinned: nothing to send.
        assert!(book_a
            .set_pinned(&a, &ta, &key, ITEM, true, 2_000)
            .unwrap()
            .is_none());
        // The relay sees neither the item id nor the flag.
        let iid = parse_uuid(ITEM, "id").unwrap();
        assert!(!out.envelope.windows(16).any(|w| w == iid));

        let ring = SpaceKeyRing::new(key.clone());
        let mut book_b = PinBook::new();
        match ring.open_received_state(&out.envelope, &out.event_id, 7) {
            ReceivedItemState::Opened(o) => match &o.state {
                ItemStateKind::Pin(s) => {
                    assert!(book_b.apply_statement(&tb, s).unwrap());
                    assert!(!book_b.apply_statement(&tb, s).unwrap());
                }
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
        assert!(book_b.is_pinned(ITEM));

        // B unpins later; A converges.
        let un = book_b
            .set_pinned(&b, &tb, &key, ITEM, false, 500)
            .unwrap()
            .unwrap();
        assert_eq!(un.statement.at_ms(), 1_001);
        let opened = open_item_state(&un.envelope, &un.event_id, &key).unwrap();
        let ItemStateKind::Pin(s) = opened.state else {
            panic!("pin expected")
        };
        assert!(book_a.apply_statement(&ta, &s).unwrap());
        assert!(!book_a.is_pinned(ITEM));
        assert_eq!(book_a, book_b);
        assert!(book_a.pinned_items().is_empty());

        let restored = PinBook::from_json(&book_a.to_json().unwrap()).unwrap();
        assert_eq!(restored, book_a);
        assert_eq!(book_a.forget(ITEM).map(|s| s.pinned), Some(false));
    }

    #[test]
    fn rejections() {
        let (a, _b, ta, _tb) = stores();
        let key = SpaceKey::generate(1).unwrap();
        let s = a.sign_pin(SPACE, ITEM, 5, true).unwrap();
        let eid = crate::new_item_id().unwrap();
        let env = seal_item_state(&s, &eid, &key).unwrap();
        let other_id = crate::new_item_id().unwrap();
        assert_eq!(
            open_item_state(&env, &other_id, &key).unwrap_err(),
            CoreError::EventIdMismatch
        );
        let other_key = SpaceKey::generate(1).unwrap();
        assert_eq!(
            open_item_state(&env, &eid, &other_key).unwrap_err(),
            CoreError::DecryptionFailed
        );
        let mut forged = s.clone();
        forged.pinned = false;
        assert_eq!(
            ta.verify_pin(&forged).unwrap_err(),
            CoreError::InvalidSignature
        );
        let d = DeviceKeys::generate().unwrap();
        let sd = d.sign_pin(SPACE, ITEM, 5, true).unwrap();
        assert_eq!(ta.verify_pin(&sd).unwrap_err(), CoreError::UntrustedSigner);
        let mut book = PinBook::new();
        assert!(book.apply_statement(&ta, &sd).is_err());
        assert!(!book.is_pinned(ITEM));
        // Unknown inner kind: ignored, not an error.
        let env =
            seal_item_state_raw(b"tomi/v1/sig/future\0xyz", &[3; 16], &key, &[4; 24]).unwrap();
        let o = open_item_state(&env, &format_uuid(&[3; 16]), &key).unwrap();
        assert_eq!(o.state, ItemStateKind::Unknown);
    }

    #[test]
    fn retention_rules() {
        use LocalRetention::*;
        // Explicit expiration wins over the pin.
        assert_eq!(local_retention(Some(100), true, 0, 100, None), EraseExpired);
        assert_eq!(local_retention(Some(100), false, 0, 99, None), Keep);
        // Cleanup spares pinned items only: the archive is not a parameter, so an archived
        // item is not protected (spec §8.9.4). Not wired into the engine yet.
        assert_eq!(local_retention(None, false, 10, 1_000, Some(50)), Cleanup);
        assert_eq!(local_retention(None, true, 10, 1_000, Some(50)), Keep);
        assert_eq!(local_retention(None, false, 60, 1_000, Some(50)), Keep);
        assert_eq!(local_retention(None, false, 10, 1_000, None), Keep);
    }
}
