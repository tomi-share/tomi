//! Archived items, shared by every device of the space (spec §8.9; decision "Archiver").
//!
//! Archiving is the twin of pinning ([`crate::pin`]): **protocol state**, not a local
//! preference. It travels as an encrypted *item-state event* of its own kind:
//!
//! * the device signs an [`ArchiveStatement`] `{ space_id, item_id, at_ms, archived }`
//!   under its own label `tomi/v1/sig/archive` (a pin statement can never be replayed as an
//!   archive one, nor the other way round);
//! * the statement is padded and sealed exactly like a pin ([`seal_item_state`]): the same
//!   256-byte block, the same 320-byte envelope, so the relay can tell neither the item,
//!   nor the flag, nor even the kind of state;
//! * every device converges with last-writer-wins on the signed `at_ms`
//!   ([`ArchiveState`], [`ArchiveBook`]).
//!
//! Archiving only *hides* an item from the "Recents" list. It never erases anything, never
//! marks an item read, never sends an ack / read / release, never downloads a file, never
//! changes the relay purge or an expiration, and does not protect against local cleanups
//! (unlike a pin). `at_ms` is bounded by [`ARCHIVE_AT_MS_MAX`] (2^53 - 1).

use std::collections::BTreeMap;

use crate::crypto::{
    format_uuid, labels, new_uuid, parse_uuid, to_array, Reader, SpaceKey, Writer,
};
use crate::device::{device_id_from_ed25519, verify_signature, DeviceKeys, TrustStore};
use crate::device::{PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::error::{CoreError, Result};
use crate::pin::{seal_item_state, OutgoingItemState, StateStatement, PIN_AT_MS_MAX};

/// Wire length of an [`ArchiveStatement`]: `TBS (93) || signature (64)`.
pub const ARCHIVE_STATEMENT_LEN: usize =
    labels::SIG_ARCHIVE.len() + 1 + 16 + 16 + 8 + 1 + PUBLIC_KEY_LEN + SIGNATURE_LEN;

/// Largest valid `at_ms` of an archive decision: 2^53 - 1 (spec §8.9.1), the same bound as
/// a pin decision.
pub const ARCHIVE_AT_MS_MAX: u64 = PIN_AT_MS_MAX;

const STATE_UNARCHIVED: u8 = 0x00;
const STATE_ARCHIVED: u8 = 0x01;

/// `malformed_statement` if `at_ms` exceeds [`ARCHIVE_AT_MS_MAX`].
fn check_at_ms(at_ms: u64) -> Result<()> {
    if at_ms > ARCHIVE_AT_MS_MAX {
        return Err(CoreError::statement("archive: at_ms above 2^53 - 1"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Converging state (spec §8.9.3)
// ---------------------------------------------------------------------------

/// Archive state of one item: the value and the signed time of the decision that set it.
///
/// Order (last-writer-wins): the larger `at_ms` wins; on a tie, `archived = true` wins.
/// `(at_ms, archived)` is a total order, so every device reaches the same state whatever
/// the order in which it receives the events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ArchiveState {
    pub archived: bool,
    /// Signed decision time, ms since the Unix epoch. `0` = never archived (default).
    pub at_ms: u64,
}

impl ArchiveState {
    fn key(self) -> (u64, bool) {
        (self.at_ms, self.archived)
    }

    /// `true` if `self` replaces `current` under last-writer-wins.
    pub fn supersedes(&self, current: &ArchiveState) -> bool {
        self.key() > current.key()
    }

    /// The winner of `self` and `other`.
    pub fn merge(self, other: ArchiveState) -> ArchiveState {
        if other.supersedes(&self) {
            other
        } else {
            self
        }
    }

    /// Time to sign for a new user decision when the current state is `current`:
    /// `max(now_ms, current.at_ms + 1)`. Fails with `malformed_statement` beyond
    /// [`ARCHIVE_AT_MS_MAX`].
    pub fn next_at_ms(current: Option<ArchiveState>, now_ms: u64) -> Result<u64> {
        let at_ms = match current {
            Some(c) => now_ms.max(c.at_ms.saturating_add(1)),
            None => now_ms,
        };
        check_at_ms(at_ms)?;
        Ok(at_ms)
    }
}

// ---------------------------------------------------------------------------
// Signed statement (spec §8.9.1)
// ---------------------------------------------------------------------------

/// `{ space_id, item_id, at_ms, archived }` signed by a member device. Never sent in clear:
/// it only travels inside an item-state envelope ([`seal_item_state`]) or, as a bare
/// [`ArchiveState`], inside a sealed history transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveStatement {
    pub space_id: String,
    pub item_id: String,
    pub at_ms: u64,
    pub archived: bool,
    pub signer_ed25519: Vec<u8>,
    pub signature: Vec<u8>,
}

impl ArchiveStatement {
    fn tbs(&self) -> Result<Vec<u8>> {
        check_at_ms(self.at_ms)?;
        let sid = parse_uuid(&self.space_id, "space id")?;
        let iid = parse_uuid(&self.item_id, "item id")?;
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&self.signer_ed25519, "signer")?;
        let mut w = Writer::labeled(labels::SIG_ARCHIVE);
        w.bytes(&sid)
            .bytes(&iid)
            .u64(self.at_ms)
            .u8(if self.archived {
                STATE_ARCHIVED
            } else {
                STATE_UNARCHIVED
            })
            .bytes(&signer);
        Ok(w.finish())
    }

    /// Bytes to sign.
    pub fn signed_bytes(&self) -> Result<Vec<u8>> {
        self.tbs()
    }

    /// Wire form `TBS || signature` ([`ARCHIVE_STATEMENT_LEN`] bytes).
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
        if bytes.len() != ARCHIVE_STATEMENT_LEN {
            return Err(CoreError::statement("archive: wrong length"));
        }
        let mut r = Reader::new(bytes);
        r.expect_label(labels::SIG_ARCHIVE)?;
        let sid: [u8; 16] = r.array()?;
        let iid: [u8; 16] = r.array()?;
        let at_ms = r.u64()?;
        check_at_ms(at_ms)?;
        let archived = match r.u8()? {
            STATE_UNARCHIVED => false,
            STATE_ARCHIVED => true,
            _ => return Err(CoreError::statement("archive: unknown state")),
        };
        let signer: [u8; PUBLIC_KEY_LEN] = r.array()?;
        let sig = r.take(SIGNATURE_LEN)?;
        Ok(Self {
            space_id: format_uuid(&sid),
            item_id: format_uuid(&iid),
            at_ms,
            archived,
            signer_ed25519: signer.to_vec(),
            signature: sig.to_vec(),
        })
    }

    /// Verifies the signature against `signer_ed25519` only. Devices must use
    /// [`TrustStore::verify_archive`] (signer trusted and not revoked).
    pub fn verify_signature(&self) -> Result<()> {
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&self.signer_ed25519, "signer")?;
        verify_signature(&signer, &self.tbs()?, &self.signature)
    }

    /// The state this statement sets.
    pub fn state(&self) -> ArchiveState {
        ArchiveState {
            archived: self.archived,
            at_ms: self.at_ms,
        }
    }

    /// `device_id` of the signer.
    pub fn signer_device_id(&self) -> Result<String> {
        device_id_from_ed25519(&self.signer_ed25519)
    }
}

impl DeviceKeys {
    /// Signs an archive (`archived = true`) or unarchive (`false`) decision for `item_id`.
    /// `at_ms` above [`ARCHIVE_AT_MS_MAX`] → `malformed_statement`.
    pub fn sign_archive(
        &self,
        space_id: &str,
        item_id: &str,
        at_ms: u64,
        archived: bool,
    ) -> Result<ArchiveStatement> {
        let mut s = ArchiveStatement {
            space_id: space_id.to_owned(),
            item_id: item_id.to_owned(),
            at_ms,
            archived,
            signer_ed25519: self.ed_public().to_vec(),
            signature: Vec::new(),
        };
        s.signature = self.sign(&s.tbs()?);
        Ok(s)
    }
}

impl TrustStore {
    /// Verifies an archive statement: same space (`space_mismatch`), signer trusted and not
    /// revoked (`untrusted_signer` / `revoked_device`), signature (`invalid_signature`).
    /// Revocation is evaluated now, like for a pin (spec §8.8.2).
    pub fn verify_archive(&self, statement: &ArchiveStatement) -> Result<()> {
        if parse_uuid(&statement.space_id, "space id")? != self.space_id_bytes() {
            return Err(CoreError::SpaceMismatch);
        }
        let signer: [u8; PUBLIC_KEY_LEN] = to_array(&statement.signer_ed25519, "signer")?;
        self.require_signer(&signer)?;
        statement.verify_signature()
    }
}

// ---------------------------------------------------------------------------
// Local archive book (spec §8.9.3, api-v1.md §7.3)
// ---------------------------------------------------------------------------

/// Archive states known by this device, by `item_id`, independent of the items themselves
/// (a state may arrive before its item, e.g. history transfer). The twin of
/// [`crate::PinBook`]; the two registers are independent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveBook {
    states: BTreeMap<String, ArchiveState>,
}

impl ArchiveBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state of `item_id`, if any decision is known.
    pub fn state(&self, item_id: &str) -> Option<ArchiveState> {
        self.states.get(item_id).copied()
    }

    /// `true` if `item_id` is currently archived.
    pub fn is_archived(&self, item_id: &str) -> bool {
        self.state(item_id).is_some_and(|s| s.archived)
    }

    /// Applies a state under last-writer-wins. Returns `true` if the state changed.
    /// Callers skip tombstoned items and call [`forget`](Self::forget) when a tombstone
    /// arrives. `state.at_ms` above [`ARCHIVE_AT_MS_MAX`] → `malformed_statement`.
    pub fn merge(&mut self, item_id: &str, state: ArchiveState) -> Result<bool> {
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

    /// Verifies a received statement ([`TrustStore::verify_archive`]) and merges it.
    pub fn apply_statement(
        &mut self,
        trust: &TrustStore,
        statement: &ArchiveStatement,
    ) -> Result<bool> {
        trust.verify_archive(statement)?;
        self.merge(&statement.item_id, statement.state())
    }

    /// Drops the state of a deleted item (tombstone). Returns the former state.
    pub fn forget(&mut self, item_id: &str) -> Option<ArchiveState> {
        self.states.remove(item_id)
    }

    /// User decision on this device: archives (`archived = true`) or unarchives `item_id`.
    ///
    /// Returns `None` if the item is already in that state (nothing to send). Otherwise
    /// signs the decision at [`ArchiveState::next_at_ms`], applies it locally at once, and
    /// returns the sealed event to upload under the current space key.
    pub fn set_archived(
        &mut self,
        keys: &DeviceKeys,
        trust: &TrustStore,
        current_key: &SpaceKey,
        item_id: &str,
        archived: bool,
        now_ms: u64,
    ) -> Result<Option<OutgoingItemState>> {
        parse_uuid(item_id, "item id")?;
        let me = keys.ed_public();
        if me != trust.own_ed() {
            return Err(CoreError::input("keys do not belong to this store"));
        }
        trust.require_signer(&me)?;
        let current = self.state(item_id);
        if current.unwrap_or_default().archived == archived {
            return Ok(None);
        }
        let at_ms = ArchiveState::next_at_ms(current, now_ms)?;
        let statement = keys.sign_archive(&trust.space_id(), item_id, at_ms, archived)?;
        let event_id = new_uuid()?;
        let envelope = seal_item_state(&statement, &event_id, current_key)?;
        self.merge(item_id, statement.state())?;
        Ok(Some(OutgoingItemState {
            event_id,
            key_epoch: current_key.epoch(),
            envelope,
            statement: StateStatement::Archive(statement),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyring::SpaceKeyRing;
    use crate::pin::{
        aad_item_state, open_item_state, seal_item_state_raw, ItemStateKind, PinStatement,
        ReceivedItemState, ITEM_STATE_ENVELOPE_LEN, PIN_STATEMENT_LEN,
    };

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
    fn lengths_and_envelope_size_match_a_pin() {
        assert_eq!(labels::SIG_ARCHIVE.len(), 19);
        assert_eq!(ARCHIVE_STATEMENT_LEN, 157);
        const { assert!(ARCHIVE_STATEMENT_LEN < crate::format::PAYLOAD_PAD_BLOCK) };
        assert_ne!(ARCHIVE_STATEMENT_LEN, PIN_STATEMENT_LEN);
        assert_eq!(aad_item_state(1, 1, &[0; 16]).len(), 47);
        // Neither label is a prefix of the other once terminated by 0x00.
        let mut a = labels::SIG_ARCHIVE.to_vec();
        a.push(0);
        let mut p = labels::SIG_PIN.to_vec();
        p.push(0);
        assert!(!a.starts_with(&p) && !p.starts_with(&a));
    }

    #[test]
    fn last_writer_wins() {
        let s = |archived, at_ms| ArchiveState { archived, at_ms };
        assert!(s(true, 2).supersedes(&s(false, 1)));
        assert!(!s(false, 1).supersedes(&s(true, 2)));
        assert!(s(true, 5).supersedes(&s(false, 5)));
        assert!(!s(false, 5).supersedes(&s(true, 5)));
        assert!(!s(true, 5).supersedes(&s(true, 5)));
        let evs = [s(true, 10), s(false, 30), s(true, 20), s(true, 30)];
        let fold = |order: &[usize]| {
            order
                .iter()
                .fold(ArchiveState::default(), |acc, i| acc.merge(evs[*i]))
        };
        assert_eq!(fold(&[0, 1, 2, 3]), s(true, 30));
        assert_eq!(fold(&[3, 2, 1, 0]), s(true, 30));
        assert_eq!(fold(&[1, 0, 3, 2]), s(true, 30));
        assert_eq!(ArchiveState::next_at_ms(None, 100).unwrap(), 100);
        assert_eq!(
            ArchiveState::next_at_ms(Some(s(true, 500)), 100).unwrap(),
            501
        );
        assert_eq!(
            ArchiveState::next_at_ms(Some(s(true, 50)), 100).unwrap(),
            100
        );
    }

    #[test]
    fn at_ms_bound() {
        let max = ARCHIVE_AT_MS_MAX;
        assert_eq!(max, 9_007_199_254_740_991);
        let s = |archived, at_ms| ArchiveState { archived, at_ms };
        let code = |r: Result<u64>| r.unwrap_err().code();
        assert_eq!(ArchiveState::next_at_ms(None, max).unwrap(), max);
        assert_eq!(
            code(ArchiveState::next_at_ms(None, max + 1)),
            "malformed_statement"
        );
        assert_eq!(
            code(ArchiveState::next_at_ms(Some(s(true, max)), 0)),
            "malformed_statement"
        );
        let a = DeviceKeys::generate().unwrap();
        assert!(a.sign_archive(SPACE, ITEM, max, true).is_ok());
        assert_eq!(
            a.sign_archive(SPACE, ITEM, max + 1, true)
                .unwrap_err()
                .code(),
            "malformed_statement"
        );
        let mut book = ArchiveBook::new();
        assert!(book.merge(ITEM, s(true, u64::MAX)).is_err());
        assert!(book.state(ITEM).is_none());
    }

    #[test]
    fn archive_roundtrip_between_devices() {
        let (a, b, ta, tb) = stores();
        let key = SpaceKey::generate(1).unwrap();
        let mut book_a = ArchiveBook::new();
        let out = book_a
            .set_archived(&a, &ta, &key, ITEM, true, 1_000)
            .unwrap()
            .unwrap();
        assert!(book_a.is_archived(ITEM));
        // Same envelope size as a pin: the relay cannot tell the kinds apart.
        assert_eq!(out.envelope.len(), ITEM_STATE_ENVELOPE_LEN);
        assert_eq!(out.key_epoch, 1);
        assert!(book_a
            .set_archived(&a, &ta, &key, ITEM, true, 2_000)
            .unwrap()
            .is_none());
        let iid = parse_uuid(ITEM, "id").unwrap();
        assert!(!out.envelope.windows(16).any(|w| w == iid));

        let ring = SpaceKeyRing::new(key.clone());
        let mut book_b = ArchiveBook::new();
        match ring.open_received_state(&out.envelope, &out.event_id, 7) {
            ReceivedItemState::Opened(o) => match &o.state {
                ItemStateKind::Archive(s) => {
                    assert!(book_b.apply_statement(&tb, s).unwrap());
                    assert!(!book_b.apply_statement(&tb, s).unwrap(), "replay");
                }
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        }
        assert!(book_b.is_archived(ITEM));

        let un = book_b
            .set_archived(&b, &tb, &key, ITEM, false, 500)
            .unwrap()
            .unwrap();
        assert_eq!(un.statement.at_ms(), 1_001);
        let opened = open_item_state(&un.envelope, &un.event_id, &key).unwrap();
        let ItemStateKind::Archive(s) = opened.state else {
            panic!("archive expected")
        };
        assert!(book_a.apply_statement(&ta, &s).unwrap());
        assert!(!book_a.is_archived(ITEM));
        assert_eq!(book_a, book_b);
        assert_eq!(book_a.forget(ITEM).map(|s| s.archived), Some(false));
        assert!(book_a.state(ITEM).is_none());
    }

    #[test]
    fn rejections() {
        let (a, _b, ta, _tb) = stores();
        let key = SpaceKey::generate(1).unwrap();
        let s = a.sign_archive(SPACE, ITEM, 5, true).unwrap();
        let eid = crate::new_item_id().unwrap();
        let env = seal_item_state(&s, &eid, &key).unwrap();
        let other_id = crate::new_item_id().unwrap();
        assert_eq!(
            open_item_state(&env, &other_id, &key).unwrap_err(),
            CoreError::EventIdMismatch
        );
        let mut forged = s.clone();
        forged.archived = false;
        assert_eq!(
            ta.verify_archive(&forged).unwrap_err(),
            CoreError::InvalidSignature
        );
        let d = DeviceKeys::generate().unwrap();
        let sd = d.sign_archive(SPACE, ITEM, 5, true).unwrap();
        assert_eq!(
            ta.verify_archive(&sd).unwrap_err(),
            CoreError::UntrustedSigner
        );
        let mut book = ArchiveBook::new();
        assert!(book.apply_statement(&ta, &sd).is_err());
        assert!(!book.is_archived(ITEM));
        // Another space.
        let other_space = "33333333-3333-4333-8333-333333333333";
        let so = a.sign_archive(other_space, ITEM, 5, true).unwrap();
        assert_eq!(
            ta.verify_archive(&so).unwrap_err(),
            CoreError::SpaceMismatch
        );
        // Unknown state byte (validly signed): malformed_statement.
        let mut wire = s.to_bytes().unwrap();
        wire[labels::SIG_ARCHIVE.len() + 1 + 16 + 16 + 8] = 2;
        assert_eq!(
            ArchiveStatement::from_bytes(&wire).unwrap_err().code(),
            "malformed_statement"
        );
        // Wrong length.
        assert_eq!(
            ArchiveStatement::from_bytes(&wire[..wire.len() - 1])
                .unwrap_err()
                .code(),
            "malformed_statement"
        );
    }

    #[test]
    fn a_pin_and_an_archive_are_not_interchangeable() {
        let (a, _b, ta, _tb) = stores();
        let key = SpaceKey::generate(1).unwrap();
        // A signed pin never parses as an archive statement, nor the reverse.
        let pin = a.sign_pin(SPACE, ITEM, 5, true).unwrap();
        assert!(ArchiveStatement::from_bytes(&pin.to_bytes().unwrap()).is_err());
        let arc = a.sign_archive(SPACE, ITEM, 5, true).unwrap();
        assert!(PinStatement::from_bytes(&arc.to_bytes().unwrap()).is_err());
        // Open dispatches on the label.
        let eid = crate::new_item_id().unwrap();
        let env = seal_item_state(&arc, &eid, &key).unwrap();
        assert_eq!(
            open_item_state(&env, &eid, &key).unwrap().state,
            ItemStateKind::Archive(arc.clone())
        );
        ta.verify_archive(&arc).unwrap();
    }

    #[test]
    fn a_reader_without_archive_support_ignores_the_kind() {
        // What a device built before the archive does: the inner content starts with the pin
        // label or it is an unknown kind, ignored without error (spec §8.8.2, step 6).
        let (a, ..) = stores();
        let key = SpaceKey::generate(1).unwrap();
        let arc = a.sign_archive(SPACE, ITEM, 5, true).unwrap();
        let eid = crate::new_item_id().unwrap();
        let env = seal_item_state(&arc, &eid, &key).unwrap();
        let raw = crate::crypto::aead_open(
            &key.item_state_key().unwrap(),
            &crate::crypto::to_array(&env[24..crate::pin::ITEM_STATE_HEADER_LEN], "nonce").unwrap(),
            &aad_item_state(1, 1, &parse_uuid(&eid, "e").unwrap()),
            &env[crate::pin::ITEM_STATE_HEADER_LEN..],
        )
        .unwrap();
        let inner = crate::envelope::unpad_payload(&raw).unwrap();
        let mut pin_prefix = labels::SIG_PIN.to_vec();
        pin_prefix.push(0);
        assert!(!inner.starts_with(&pin_prefix));
        // And the unknown-kind path itself still works.
        let env =
            seal_item_state_raw(b"tomi/v1/sig/future\0xyz", &[3; 16], &key, &[4; 24]).unwrap();
        let o = open_item_state(&env, &format_uuid(&[3; 16]), &key).unwrap();
        assert_eq!(o.state, ItemStateKind::Unknown);
    }
}
