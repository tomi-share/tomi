//! Space keys kept by a device, readability of received items, relay retention
//! (spec §5.7, §8.3, §9.4; decision D1).
//!
//! With items that may live forever (no expiration by default), a device must not keep
//! every old space key: received items are re-sealed under the device-local key
//! ([`crate::LocalKey`]) once opened, and an old epoch key is discarded
//! [`space_key_retention_ms`] after the rotation that replaced it.

use std::collections::BTreeMap;

use crate::crypto::SpaceKey;
use crate::envelope::{open_item, read_envelope_header, OpenedItem};
use crate::error::{CoreError, Result};

/// Default relay `max_ttl_secs` (30 days, `api-v1.md` §2.8).
pub const DEFAULT_MAX_TTL_SECS: u64 = 2_592_000;
/// Extra time an old space key is kept after the relay retention bound (1 day).
pub const SPACE_KEY_GRACE_MS: u64 = 86_400_000;
/// Margin subtracted from `server_now + max_ttl` so that a slightly wrong estimate of
/// the server clock does not get the item refused (`validation_failed`).
pub const RELAY_EXPIRY_MARGIN_MS: i64 = 60_000;

/// Relay retention deadline to send in `POST /v1/items` (`expires_at`, spec §5.7):
/// `min(user expiration, server_now + max_ttl − margin)`. The user expiration (inside the
/// payload) is usually absent: the relay then keeps an undelivered item for `max_ttl`.
///
/// `server_now_ms` is the estimated relay clock (`hello.server_time` + elapsed time). The
/// margin is at most half of `max_ttl`. A result `<= server_now_ms` means the item already
/// expired: do not send it.
pub fn relay_expires_at_ms(
    user_expires_at_ms: Option<i64>,
    server_now_ms: i64,
    max_ttl_secs: u64,
) -> i64 {
    let ttl_ms = i64::try_from(max_ttl_secs.saturating_mul(1000)).unwrap_or(i64::MAX);
    let margin = RELAY_EXPIRY_MARGIN_MS.min(ttl_ms / 2);
    let cap = server_now_ms.saturating_add(ttl_ms).saturating_sub(margin);
    match user_expires_at_ms {
        Some(t) => t.min(cap),
        None => cap,
    }
}

/// Margin added to the relay retention to bound the age of any item a relay may honestly
/// deliver (60 days): room for an item that waited in the sender's outbox, and for clock skew.
pub const ITEM_AGE_MARGIN_MS: i64 = 60 * 86_400_000;

/// Oldest `created_at` an item can have when it first reaches a device, relative to that moment:
/// `max_ttl + 60 days` (90 days by default). An honest relay purges an undelivered item at
/// `max_ttl`, so an older one (by its authenticated `created_at`) can only be a replay of something
/// deleted or consumed long ago, served again once the local tombstone is gone: it is refused
/// ([`UnreadableReason::SuspectEpoch`], the one "suspect" state). Tombstones are kept for the same length of time
/// ([`tombstone_retention_ms`]), so that one of the two always covers an item.
pub fn item_max_age_ms(max_ttl_secs: u64) -> i64 {
    i64::try_from(trusted_ttl_secs(max_ttl_secs).saturating_mul(1000))
        .unwrap_or(i64::MAX)
        .saturating_add(ITEM_AGE_MARGIN_MS)
}

/// Highest `max_ttl_secs` that the retention rules below accept from a relay (one year). The
/// value is the relay's own announcement: without a bound, a relay could announce ten years,
/// then replay an item whose tombstone was pruned, and no age check would stop it. A relay
/// configured for a longer retention than this is still served, but the windows stay at this size.
pub const MAX_TRUSTED_TTL_SECS: u64 = 365 * 86_400;

fn trusted_ttl_secs(max_ttl_secs: u64) -> u64 {
    max_ttl_secs.min(MAX_TRUSTED_TTL_SECS)
}

/// How long a local item tombstone is kept: [`item_max_age_ms`].
pub fn tombstone_retention_ms(max_ttl_secs: u64) -> i64 {
    item_max_age_ms(max_ttl_secs)
}

/// How long an old space key is kept after the rotation that replaced it:
/// `max_ttl + 1 day` (31 days by default). Past that, no honest relay still holds an
/// item of that epoch.
pub fn space_key_retention_ms(max_ttl_secs: u64) -> u64 {
    trusted_ttl_secs(max_ttl_secs)
        .saturating_mul(1000)
        .saturating_add(SPACE_KEY_GRACE_MS)
}

/// Why a received item cannot be shown. The item is **never deleted** for that reason
/// (DEV.md invariant 7); the raw envelope is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnreadableReason {
    /// Unknown `format`. Retry after an app update.
    UpdateRequired,
    /// Epoch newer than the keys of this device. Retry after the next
    /// rotation is applied.
    KeyPending,
    /// Sealed with an epoch already replaced by a rotation whose
    /// `seq` is lower than the item's (spec §12), or older (authenticated `created_at`) than a
    /// relay can honestly hold it ([`item_max_age_ms`]): an honest relay cannot produce either.
    /// Never retried.
    SuspectEpoch,
    /// Authentication or decoding failed, or the epoch key was discarded.
    /// Never retried automatically.
    Undecryptable,
}

impl UnreadableReason {
    /// Stable code for clients (localisation, persistence).
    pub fn code(self) -> &'static str {
        match self {
            UnreadableReason::UpdateRequired => "update_required",
            UnreadableReason::KeyPending => "key_pending",
            UnreadableReason::SuspectEpoch => "suspect_epoch",
            UnreadableReason::Undecryptable => "undecryptable",
        }
    }

    /// Classification of an [`open_item`] error.
    pub fn from_error(e: &CoreError) -> Self {
        match e {
            CoreError::UnsupportedFormat { .. } => UnreadableReason::UpdateRequired,
            _ => UnreadableReason::Undecryptable,
        }
    }
}

/// Outcome of opening a received item: readable or unreadable, never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivedItem {
    Readable(Box<OpenedItem>),
    Unreadable(UnreadableReason),
}

impl ReceivedItem {
    pub fn is_readable(&self) -> bool {
        matches!(self, ReceivedItem::Readable(_))
    }
}

/// Rotation that replaced an epoch, as applied by this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supersession {
    /// `seq` of the `rotation` event `e -> e+1`.
    pub rotation_seq: u64,
    /// Local time the rotation was applied (ms since the Unix epoch).
    pub applied_at_ms: u64,
}

/// Persisted form of one epoch of the ring. Store `key` in the OS secure storage.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyRingRecord {
    pub epoch: u32,
    /// `None` once the key was discarded (the supersession is kept for the epoch rule).
    pub key: Option<Vec<u8>>,
    /// `None` only for the current epoch.
    pub superseded: Option<Supersession>,
}

impl std::fmt::Debug for KeyRingRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRingRecord")
            .field("epoch", &self.epoch)
            .field("has_key", &self.key.is_some())
            .field("superseded", &self.superseded)
            .finish()
    }
}

/// The space keys of this device, by epoch, with the rotation that replaced each one.
#[derive(Debug, Clone)]
pub struct SpaceKeyRing {
    keys: BTreeMap<u32, SpaceKey>,
    superseded: BTreeMap<u32, Supersession>,
    /// The epoch that was current when [`erase_keys`](Self::erase_keys) wiped every key (this
    /// device was revoked): the ring then holds no key at all.
    erased_at: Option<u32>,
}

impl SpaceKeyRing {
    /// Ring holding only the key received at pairing or created with the space.
    pub fn new(current: SpaceKey) -> Self {
        let mut keys = BTreeMap::new();
        keys.insert(current.epoch(), current);
        Self {
            keys,
            superseded: BTreeMap::new(),
            erased_at: None,
        }
    }

    /// Latest key: the only one used to write.
    pub fn current(&self) -> Result<&SpaceKey> {
        self.keys
            .values()
            .next_back()
            .ok_or_else(|| CoreError::Internal {
                reason: "empty key ring".into(),
            })
    }

    pub fn current_epoch(&self) -> u32 {
        self.keys
            .keys()
            .next_back()
            .copied()
            .or(self.erased_at)
            .unwrap_or(0)
    }

    /// Wipes every space key (the ring keeps the `seq` of each rotation, which is not secret).
    /// For a device that learned from a **verified signed rotation** that it was revoked
    /// (`format-v1.md` §8.3 step 6). What was already opened is re-sealed under `LSK` and
    /// stays readable; what is still raw can no longer be opened. Returns whether a key was
    /// erased.
    pub fn erase_keys(&mut self) -> bool {
        if self.keys.is_empty() {
            return false;
        }
        self.erased_at = Some(self.current_epoch());
        self.keys.clear();
        true
    }

    /// Whether [`erase_keys`](Self::erase_keys) wiped the ring.
    pub fn is_erased(&self) -> bool {
        self.keys.is_empty() && self.erased_at.is_some()
    }

    /// Key of `epoch`, if still held.
    pub fn key(&self, epoch: u32) -> Option<&SpaceKey> {
        self.keys.get(&epoch)
    }

    /// Epochs whose key is still held, ascending.
    pub fn epochs(&self) -> Vec<u32> {
        self.keys.keys().copied().collect()
    }

    /// Records the key of an applied rotation (`TrustStore::apply_rotation`): the
    /// current epoch becomes superseded at `rotation_seq`.
    pub fn add_rotation_key(
        &mut self,
        new_key: SpaceKey,
        rotation_seq: u64,
        applied_at_ms: u64,
    ) -> Result<()> {
        if self.is_erased() {
            return Err(CoreError::Internal {
                reason: "the space keys of this device were erased".into(),
            });
        }
        let current = self.current_epoch();
        let expected = current.saturating_add(1);
        if new_key.epoch() != expected {
            return Err(CoreError::EpochOutOfOrder {
                expected,
                got: new_key.epoch(),
            });
        }
        self.superseded.insert(
            current,
            Supersession {
                rotation_seq,
                applied_at_ms,
            },
        );
        self.keys.insert(new_key.epoch(), new_key);
        Ok(())
    }

    /// Lowers the recorded `rotation_seq` of the rotation `epoch -> epoch+1` to `seq`, when the
    /// log position of that rotation turns out to be lower than what was recorded (the `seq`
    /// a relay answered to `POST /v1/rotations` is not proof of the position of the event in
    /// the log; the event itself, met in order, is). Never raises it. Returns whether
    /// anything changed.
    pub fn tighten_supersession(&mut self, epoch: u32, seq: u64) -> bool {
        match self.superseded.get_mut(&epoch) {
            Some(s) if seq < s.rotation_seq => {
                s.rotation_seq = seq;
                true
            }
            _ => false,
        }
    }

    /// Makes every superseded epoch final: from now on **any** item sealed under one of them
    /// that is first seen is [`UnreadableReason::SuspectEpoch`], whatever its `seq`. Used when
    /// the relay's `seq` numbering can no longer be trusted to order events against the
    /// rotations (the log went backwards: `cursor_ahead`, spec api §7.5). Returns whether
    /// anything changed.
    pub fn freeze_supersessions(&mut self) -> bool {
        let mut changed = false;
        for s in self.superseded.values_mut() {
            if s.rotation_seq != 0 {
                s.rotation_seq = 0;
                changed = true;
            }
        }
        changed
    }

    /// Key to open an `item` event of `key_epoch` at `item_seq`, or why not (spec §12,
    /// epoch order rule). A `key_epoch` in the reserved range (above
    /// [`crate::SPACE_KEY_EPOCH_MAX`], spec §5.8.2) is never a space key of this ring and
    /// is always newer than the current epoch: `KeyPending`.
    pub fn key_for_item(
        &self,
        key_epoch: u32,
        item_seq: u64,
    ) -> std::result::Result<&SpaceKey, UnreadableReason> {
        if let Some(s) = self.superseded.get(&key_epoch) {
            if item_seq > s.rotation_seq {
                return Err(UnreadableReason::SuspectEpoch);
            }
        }
        match self.keys.get(&key_epoch) {
            Some(k) => Ok(k),
            None if key_epoch > self.current_epoch() => Err(UnreadableReason::KeyPending),
            None => Err(UnreadableReason::Undecryptable),
        }
    }

    /// Opens a received envelope listed by the relay under `announced_item_id` at
    /// `item_seq`. Never fails: an unreadable item is kept with its reason.
    pub fn open_received(
        &self,
        envelope: &[u8],
        announced_item_id: &str,
        item_seq: u64,
    ) -> ReceivedItem {
        let header = match read_envelope_header(envelope) {
            Ok(h) => h,
            Err(e) => return ReceivedItem::Unreadable(UnreadableReason::from_error(&e)),
        };
        let key = match self.key_for_item(header.key_epoch, item_seq) {
            Ok(k) => k,
            Err(reason) => return ReceivedItem::Unreadable(reason),
        };
        match open_item(envelope, announced_item_id, key) {
            Ok(opened) => ReceivedItem::Readable(Box::new(opened)),
            Err(e) => ReceivedItem::Unreadable(UnreadableReason::from_error(&e)),
        }
    }

    /// Discards the superseded keys older than `retention_ms` (see
    /// [`space_key_retention_ms`]) unless their epoch is in `still_needed` (raw
    /// envelopes kept locally in state `KeyPending`/`UpdateRequired` of that epoch).
    /// The current key is never discarded. Returns the discarded epochs.
    pub fn discard_expired(
        &mut self,
        now_ms: u64,
        retention_ms: u64,
        still_needed: &[u32],
    ) -> Vec<u32> {
        let current = self.current_epoch();
        let drop: Vec<u32> = self
            .keys
            .keys()
            .copied()
            .filter(|e| *e != current && !still_needed.contains(e))
            .filter(|e| {
                self.superseded
                    .get(e)
                    .is_some_and(|s| now_ms >= s.applied_at_ms.saturating_add(retention_ms))
            })
            .collect();
        for e in &drop {
            self.keys.remove(e);
        }
        drop
    }

    /// Persisted form (keys go to secure storage).
    pub fn to_records(&self) -> Vec<KeyRingRecord> {
        let mut epochs: Vec<u32> = self.keys.keys().copied().collect();
        epochs.extend(self.superseded.keys().copied());
        epochs.extend(self.erased_at);
        epochs.sort_unstable();
        epochs.dedup();
        epochs
            .into_iter()
            .map(|e| KeyRingRecord {
                epoch: e,
                key: self.keys.get(&e).map(SpaceKey::to_bytes),
                superseded: self.superseded.get(&e).copied(),
            })
            .collect()
    }

    /// Restores a ring saved with [`to_records`](Self::to_records).
    pub fn from_records(records: &[KeyRingRecord]) -> Result<Self> {
        let mut keys = BTreeMap::new();
        let mut superseded = BTreeMap::new();
        for r in records {
            if let Some(k) = &r.key {
                keys.insert(r.epoch, SpaceKey::from_bytes(r.epoch, k)?);
            }
            if let Some(s) = r.superseded {
                superseded.insert(r.epoch, s);
            }
        }
        // A ring whose keys were all erased (revoked device) has no key: its last record is
        // the epoch that was current, with neither key nor supersession.
        let erased_at = if keys.is_empty() {
            let epoch = records
                .iter()
                .filter(|r| r.key.is_none() && r.superseded.is_none())
                .map(|r| r.epoch)
                .max()
                .ok_or_else(|| CoreError::input("key ring without a key"))?;
            Some(epoch)
        } else {
            None
        };
        let current = keys
            .keys()
            .next_back()
            .copied()
            .or(erased_at)
            .ok_or_else(|| CoreError::input("key ring without a key"))?;
        if superseded.contains_key(&current)
            || superseded.keys().any(|e| *e > current)
            || keys
                .keys()
                .any(|e| *e != current && !superseded.contains_key(e))
        {
            return Err(CoreError::input("inconsistent key ring records"));
        }
        Ok(Self {
            keys,
            superseded,
            erased_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::seal_item;
    use crate::item::{Item, ReadPolicy};

    const DAY: u64 = 86_400_000;

    fn item() -> Item {
        Item {
            id: crate::new_item_id().unwrap(),
            source_device: "0123456789abcdef0123456789abcdef".into(),
            created_at_ms: 1,
            expires_at_ms: None,
            read_policy: ReadPolicy::UntilExpiry,
            secret: false,
            title: None,
            text: None,
            url: None,
            attachments: vec![],
            raw_payload: None,
        }
    }

    #[test]
    fn relay_deadline() {
        let now = 1_000_000_000_000;
        let ttl = DEFAULT_MAX_TTL_SECS;
        let cap = now + 2_592_000_000 - 60_000;
        assert_eq!(relay_expires_at_ms(None, now, ttl), cap);
        assert_eq!(
            relay_expires_at_ms(Some(now + 3_600_000), now, ttl),
            now + 3_600_000
        );
        assert_eq!(
            relay_expires_at_ms(Some(now + 90 * DAY as i64), now, ttl),
            cap
        );
        assert_eq!(relay_expires_at_ms(None, now, 10), now + 5_000);
        assert_eq!(space_key_retention_ms(ttl), 31 * DAY);
    }

    #[test]
    fn a_ttl_announced_by_the_relay_cannot_stretch_the_windows_past_the_bound() {
        let year = MAX_TRUSTED_TTL_SECS;
        let ten_years = 10 * year;
        assert_eq!(item_max_age_ms(ten_years), item_max_age_ms(year));
        assert_eq!(
            tombstone_retention_ms(ten_years),
            tombstone_retention_ms(year)
        );
        assert_eq!(
            space_key_retention_ms(ten_years),
            space_key_retention_ms(year)
        );
        assert_eq!(item_max_age_ms(u64::MAX), item_max_age_ms(year));
        // Below the bound the relay's value is used as before.
        assert_eq!(
            item_max_age_ms(DEFAULT_MAX_TTL_SECS),
            (30 + 60) * DAY as i64
        );
    }

    #[test]
    fn epoch_rule_and_discard() {
        let k1 = SpaceKey::generate(1).unwrap();
        let k2 = SpaceKey::generate(2).unwrap();
        let it = item();
        let env1 = seal_item(&it, &k1).unwrap();
        let env2 = seal_item(&it, &k2).unwrap();

        let mut ring = SpaceKeyRing::new(k1.clone());
        assert_eq!(
            ring.open_received(&env2, &it.id, 10),
            ReceivedItem::Unreadable(UnreadableReason::KeyPending)
        );
        ring.add_rotation_key(k2, 100, 5 * DAY).unwrap();
        assert!(ring.open_received(&env1, &it.id, 99).is_readable());
        assert_eq!(
            ring.open_received(&env1, &it.id, 101),
            ReceivedItem::Unreadable(UnreadableReason::SuspectEpoch)
        );
        assert!(ring.open_received(&env2, &it.id, 101).is_readable());
        let mut env_bad = env2.clone();
        env_bad[0..4].copy_from_slice(&9u32.to_be_bytes());
        assert_eq!(
            ring.open_received(&env_bad, &it.id, 101),
            ReceivedItem::Unreadable(UnreadableReason::UpdateRequired)
        );

        let retention = space_key_retention_ms(DEFAULT_MAX_TTL_SECS);
        assert!(ring
            .discard_expired(5 * DAY + retention - 1, retention, &[])
            .is_empty());
        assert!(ring
            .discard_expired(5 * DAY + retention, retention, &[1])
            .is_empty());
        assert_eq!(
            ring.discard_expired(5 * DAY + retention, retention, &[]),
            vec![1]
        );
        assert_eq!(ring.epochs(), vec![2]);
        assert_eq!(
            ring.open_received(&env1, &it.id, 99),
            ReceivedItem::Unreadable(UnreadableReason::Undecryptable)
        );
        assert_eq!(
            ring.open_received(&env1, &it.id, 101),
            ReceivedItem::Unreadable(UnreadableReason::SuspectEpoch)
        );

        let restored = SpaceKeyRing::from_records(&ring.to_records()).unwrap();
        assert_eq!(restored.to_records(), ring.to_records());
        assert!(!format!("{:?}", ring.to_records()).contains(&format!("{:?}", k1.to_bytes())));
    }

    #[test]
    fn erased_ring_holds_no_key_and_round_trips() {
        let k1 = SpaceKey::generate(1).unwrap();
        let k2 = SpaceKey::generate(2).unwrap();
        let it = item();
        let env1 = seal_item(&it, &k1).unwrap();
        let env2 = seal_item(&it, &k2).unwrap();
        let mut ring = SpaceKeyRing::new(k1);
        ring.add_rotation_key(k2.clone(), 100, DAY).unwrap();
        assert!(!ring.is_erased());
        assert!(ring.erase_keys());
        assert!(!ring.erase_keys(), "idempotent");
        assert!(ring.is_erased());
        assert!(ring.epochs().is_empty());
        assert!(ring.current().is_err());
        assert_eq!(ring.current_epoch(), 2);
        for env in [&env1, &env2] {
            assert!(!ring.open_received(env, &it.id, 5).is_readable());
        }
        assert!(ring
            .add_rotation_key(SpaceKey::generate(3).unwrap(), 200, DAY)
            .is_err());
        let restored = SpaceKeyRing::from_records(&ring.to_records()).unwrap();
        assert_eq!(restored.to_records(), ring.to_records());
        assert!(restored.is_erased());
        assert_eq!(restored.current_epoch(), 2);
        // Nothing of the keys is left in the persisted form.
        assert!(ring.to_records().iter().all(|r| r.key.is_none()));
        // A ring that was never rotated erases too.
        let mut fresh = SpaceKeyRing::new(SpaceKey::generate(1).unwrap());
        assert!(fresh.erase_keys());
        let again = SpaceKeyRing::from_records(&fresh.to_records()).unwrap();
        assert!(again.is_erased());
        assert_eq!(again.current_epoch(), 1);
    }

    #[test]
    fn frozen_supersessions_make_every_old_epoch_item_suspect() {
        let k1 = SpaceKey::generate(1).unwrap();
        let k2 = SpaceKey::generate(2).unwrap();
        let it = item();
        let env1 = seal_item(&it, &k1).unwrap();
        let mut ring = SpaceKeyRing::new(k1);
        ring.add_rotation_key(k2, 100, DAY).unwrap();
        assert!(ring.open_received(&env1, &it.id, 99).is_readable());
        assert!(ring.freeze_supersessions());
        assert!(!ring.freeze_supersessions(), "idempotent");
        for seq in [1, 99, 100, 101] {
            assert_eq!(
                ring.open_received(&env1, &it.id, seq),
                ReceivedItem::Unreadable(UnreadableReason::SuspectEpoch)
            );
        }
        let restored = SpaceKeyRing::from_records(&ring.to_records()).unwrap();
        assert_eq!(restored.to_records(), ring.to_records());
    }
}
