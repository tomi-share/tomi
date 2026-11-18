//! Sealed bodies of the pairing frames: a list of TLVs (`spec/pairing-v1.md` §6.2-§6.6).
//!
//! ```text
//! body = tlv*              tlv = u8(tag) || u16be(len) || value(len)
//! ```
//!
//! Reader rules (§6.2):
//! 1. the tags are strictly increasing (no duplicate, no disorder) and `0x00` is not a tag;
//! 2. a truncated TLV is an error;
//! 3. an unknown tag in `0x01..=0x7f` is ignored (additive extension); an unknown tag in
//!    `0x80..=0xff` is critical: `pairing_unsupported`;
//! 4. a missing required field, or one whose length differs from the table, is an error.
//!
//! The structure of the whole body is checked first (rules 1 and 2), then the critical tags
//! (rule 3), then the fields (rule 4). A writer of version 1 writes only the tags of the
//! tables, in increasing order, each once.

use super::info::HistoryOffer;
use crate::crypto::KEY_LEN;
use crate::device::SEALED_SPACE_KEY_LEN;
use crate::error::{CoreError, Result};
use crate::format::SPACE_KEY_EPOCH_MAX;

/// Tags of `InviterHello` (§6.3) and `JoinerHello` (§6.4).
const T_ED_PK: u8 = 0x01;
const T_X_PK: u8 = 0x02;
const T_NAME: u8 = 0x03;
const T_PLATFORM: u8 = 0x04;
const T_MEMBER_COUNT: u8 = 0x05;
const T_HISTORY_OFFER: u8 = 0x06;
/// Tag of `JoinerConfirm` (§6.5).
const T_CONFIRM_HISTORY: u8 = 0x01;
/// Tags of `Provision` (§6.6).
const T_SPACE_ID: u8 = 0x01;
const T_KEY_EPOCH: u8 = 0x02;
const T_SEALED: u8 = 0x03;
const T_DEVICE_CERT: u8 = 0x04;
const T_MEMBER_LIST: u8 = 0x05;
const T_PROVISION_HISTORY: u8 = 0x06;

/// First tag of the critical range.
const CRITICAL_TAG_MIN: u8 = 0x80;

/// Wire length of a `DeviceCert` (`format-v1.md` §10).
pub(super) const DEVICE_CERT_LEN: usize = 208;
/// Wire length of a `MemberList` without its entries: TBS header (88) and signature (64).
pub(super) const MEMBER_LIST_BASE_LEN: usize = 152;
/// Wire length of one `MemberList` entry: `ed_pk || x_pk`.
pub(super) const MEMBER_ENTRY_LEN: usize = 64;

/// `name`: UTF-8, 1 to 256 bytes (§6.2), and the `name` type of `api-v1.md` §2.3 as is: 1 to 64
/// characters, no blank at either end, no control character (category Cc).
pub(super) const NAME_MAX_BYTES: usize = 256;
pub(super) const NAME_MAX_CHARS: usize = 64;
/// `platform`: ASCII `^[a-z0-9-]{1,32}$`.
pub(super) const PLATFORM_MAX_LEN: usize = 32;

pub(super) fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= NAME_MAX_BYTES
        && s.chars().count() <= NAME_MAX_CHARS
        && s.trim() == s
        && !s.chars().any(char::is_control)
}

pub(super) fn is_valid_platform(s: &str) -> bool {
    (1..=PLATFORM_MAX_LEN).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Builds a body with strictly increasing tags.
pub(super) struct Writer {
    buf: Vec<u8>,
    last: u8,
}

impl Writer {
    pub(super) fn new() -> Self {
        Self {
            buf: Vec::new(),
            last: 0,
        }
    }

    /// Appends one TLV. A tag that does not increase, or a value longer than `u16::MAX`, is
    /// refused (the latter can only be a space far too large for a pairing).
    pub(super) fn put(&mut self, tag: u8, value: &[u8]) -> Result<()> {
        if tag <= self.last {
            return Err(CoreError::Internal {
                reason: "tlv tags must increase".into(),
            });
        }
        let len = u16::try_from(value.len()).map_err(|_| CoreError::PairingFailed)?;
        self.buf.push(tag);
        self.buf.extend_from_slice(&len.to_be_bytes());
        self.buf.extend_from_slice(value);
        self.last = tag;
        Ok(())
    }

    pub(super) fn finish(self) -> Vec<u8> {
        self.buf
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// The TLVs of a body, structurally valid.
pub(super) struct Fields<'a> {
    entries: Vec<(u8, &'a [u8])>,
}

impl<'a> Fields<'a> {
    /// Rules 1 to 3. `PairingFailed` for a structural error, `PairingUnsupported` for an
    /// unknown critical tag in an otherwise well-formed body.
    pub(super) fn parse(body: &'a [u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut rest = body;
        let mut last = 0u8;
        let mut critical = false;
        while !rest.is_empty() {
            let [tag, l0, l1, tail @ ..] = rest else {
                return Err(CoreError::PairingFailed);
            };
            let len = usize::from(u16::from_be_bytes([*l0, *l1]));
            if tail.len() < len {
                return Err(CoreError::PairingFailed);
            }
            let (value, next) = tail.split_at(len);
            if *tag <= last {
                // Covers tag 0, duplicates and disorder.
                return Err(CoreError::PairingFailed);
            }
            last = *tag;
            // No tag of this version lies in the critical range: any such tag is unknown.
            critical |= *tag >= CRITICAL_TAG_MIN;
            entries.push((*tag, value));
            rest = next;
        }
        if critical {
            return Err(CoreError::PairingUnsupported);
        }
        Ok(Self { entries })
    }

    fn get(&self, tag: u8) -> Option<&'a [u8]> {
        self.entries
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| *v)
    }

    /// A required field of exactly `N` bytes.
    fn required<const N: usize>(&self, tag: u8) -> Result<[u8; N]> {
        let v = self.get(tag).ok_or(CoreError::PairingFailed)?;
        <[u8; N]>::try_from(v).map_err(|_| CoreError::PairingFailed)
    }

    /// An optional field of exactly `N` bytes (present with another length: an error).
    fn optional<const N: usize>(&self, tag: u8) -> Result<Option<[u8; N]>> {
        match self.get(tag) {
            None => Ok(None),
            Some(v) => <[u8; N]>::try_from(v)
                .map(Some)
                .map_err(|_| CoreError::PairingFailed),
        }
    }

    /// A required UTF-8 text accepted by `valid`.
    fn text(&self, tag: u8, valid: fn(&str) -> bool) -> Result<String> {
        let v = self.get(tag).ok_or(CoreError::PairingFailed)?;
        let s = std::str::from_utf8(v).map_err(|_| CoreError::PairingFailed)?;
        if valid(s) {
            Ok(s.to_owned())
        } else {
            Err(CoreError::PairingFailed)
        }
    }

    /// A required variable-length field.
    fn bytes(&self, tag: u8) -> Result<&'a [u8]> {
        self.get(tag).ok_or(CoreError::PairingFailed)
    }
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

fn offer_to_bytes(o: HistoryOffer) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&o.items.to_be_bytes());
    out[8..].copy_from_slice(&o.bytes.to_be_bytes());
    out
}

fn split_counts(v: &[u8; 16]) -> (u64, u64) {
    let (a, b) = v.split_at(8);
    (be64(a), be64(b))
}

fn be64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    u64::from_be_bytes(a)
}

fn check_own_text(name: &str, platform: &str) -> Result<()> {
    if !is_valid_name(name) {
        return Err(CoreError::input("invalid device name"));
    }
    if !is_valid_platform(platform) {
        return Err(CoreError::input("invalid device platform"));
    }
    Ok(())
}

/// `InviterHello` (§6.3), inside the `reveal` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InviterHello {
    pub(super) ed_pk: [u8; KEY_LEN],
    pub(super) x_pk: [u8; KEY_LEN],
    pub(super) name: String,
    pub(super) platform: String,
    pub(super) member_count: Option<u16>,
    pub(super) history_offer: Option<HistoryOffer>,
}

impl InviterHello {
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        check_own_text(&self.name, &self.platform)?;
        let mut w = Writer::new();
        w.put(T_ED_PK, &self.ed_pk)?;
        w.put(T_X_PK, &self.x_pk)?;
        w.put(T_NAME, self.name.as_bytes())?;
        w.put(T_PLATFORM, self.platform.as_bytes())?;
        if let Some(n) = self.member_count {
            w.put(T_MEMBER_COUNT, &n.to_be_bytes())?;
        }
        // "Absent, or items = 0: nothing to offer."
        if let Some(o) = self.history_offer.filter(|o| o.items > 0) {
            w.put(T_HISTORY_OFFER, &offer_to_bytes(o))?;
        }
        Ok(w.finish())
    }

    pub(super) fn decode(body: &[u8]) -> Result<Self> {
        let f = Fields::parse(body)?;
        let history_offer = f
            .optional::<16>(T_HISTORY_OFFER)?
            .map(|v| split_counts(&v))
            .filter(|(items, _)| *items > 0)
            .map(|(items, bytes)| HistoryOffer { items, bytes });
        Ok(Self {
            ed_pk: f.required(T_ED_PK)?,
            x_pk: f.required(T_X_PK)?,
            name: f.text(T_NAME, is_valid_name)?,
            platform: f.text(T_PLATFORM, is_valid_platform)?,
            member_count: f.optional::<2>(T_MEMBER_COUNT)?.map(u16::from_be_bytes),
            history_offer,
        })
    }
}

/// `JoinerHello` (§6.4), inside the `joiner_hello` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JoinerHello {
    pub(super) ed_pk: [u8; KEY_LEN],
    pub(super) x_pk: [u8; KEY_LEN],
    pub(super) name: String,
    pub(super) platform: String,
}

impl JoinerHello {
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        check_own_text(&self.name, &self.platform)?;
        let mut w = Writer::new();
        w.put(T_ED_PK, &self.ed_pk)?;
        w.put(T_X_PK, &self.x_pk)?;
        w.put(T_NAME, self.name.as_bytes())?;
        w.put(T_PLATFORM, self.platform.as_bytes())?;
        Ok(w.finish())
    }

    pub(super) fn decode(body: &[u8]) -> Result<Self> {
        let f = Fields::parse(body)?;
        Ok(Self {
            ed_pk: f.required(T_ED_PK)?,
            x_pk: f.required(T_X_PK)?,
            name: f.text(T_NAME, is_valid_name)?,
            platform: f.text(T_PLATFORM, is_valid_platform)?,
        })
    }
}

/// `JoinerConfirm` (§6.5): the answer of the user on N. The frame exists only if the user
/// accepted; its size does not depend on the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JoinerConfirm {
    /// `true`: the user asked for the history.
    pub(super) history: bool,
}

impl JoinerConfirm {
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        w.put(T_CONFIRM_HISTORY, &[u8::from(self.history)])?;
        Ok(w.finish())
    }

    pub(super) fn decode(body: &[u8]) -> Result<Self> {
        let f = Fields::parse(body)?;
        match f.required::<1>(T_CONFIRM_HISTORY)? {
            [0x00] => Ok(Self { history: false }),
            [0x01] => Ok(Self { history: true }),
            _ => Err(CoreError::PairingFailed),
        }
    }
}

/// The history announcement as it travels (`Provision.history`, §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HistoryWire {
    pub(super) transfer_id: [u8; 16],
    pub(super) items: u64,
    pub(super) bytes: u64,
}

/// `Provision` (§6.6), inside the `provision` frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProvisionBody {
    pub(super) space_id: [u8; 16],
    pub(super) key_epoch: u32,
    pub(super) sealed: Vec<u8>,
    pub(super) device_cert: Vec<u8>,
    pub(super) member_list: Vec<u8>,
    pub(super) history: Option<HistoryWire>,
}

fn valid_member_list_len(len: usize) -> bool {
    len >= MEMBER_LIST_BASE_LEN && (len - MEMBER_LIST_BASE_LEN).is_multiple_of(MEMBER_ENTRY_LEN)
}

fn valid_epoch(e: u32) -> bool {
    (1..=SPACE_KEY_EPOCH_MAX).contains(&e)
}

impl ProvisionBody {
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        if !valid_epoch(self.key_epoch)
            || self.sealed.len() != SEALED_SPACE_KEY_LEN
            || self.device_cert.len() != DEVICE_CERT_LEN
            || !valid_member_list_len(self.member_list.len())
        {
            return Err(CoreError::input("provision body: invalid field"));
        }
        let mut w = Writer::new();
        w.put(T_SPACE_ID, &self.space_id)?;
        w.put(T_KEY_EPOCH, &self.key_epoch.to_be_bytes())?;
        w.put(T_SEALED, &self.sealed)?;
        w.put(T_DEVICE_CERT, &self.device_cert)?;
        w.put(T_MEMBER_LIST, &self.member_list)?;
        if let Some(h) = &self.history {
            let mut v = [0u8; 32];
            v[..16].copy_from_slice(&h.transfer_id);
            v[16..24].copy_from_slice(&h.items.to_be_bytes());
            v[24..].copy_from_slice(&h.bytes.to_be_bytes());
            w.put(T_PROVISION_HISTORY, &v)?;
        }
        Ok(w.finish())
    }

    pub(super) fn decode(body: &[u8]) -> Result<Self> {
        let f = Fields::parse(body)?;
        let key_epoch = u32::from_be_bytes(f.required::<4>(T_KEY_EPOCH)?);
        if !valid_epoch(key_epoch) {
            return Err(CoreError::PairingFailed);
        }
        let sealed = f.bytes(T_SEALED)?;
        let device_cert = f.bytes(T_DEVICE_CERT)?;
        let member_list = f.bytes(T_MEMBER_LIST)?;
        if sealed.len() != SEALED_SPACE_KEY_LEN
            || device_cert.len() != DEVICE_CERT_LEN
            || !valid_member_list_len(member_list.len())
        {
            return Err(CoreError::PairingFailed);
        }
        let history = f.optional::<32>(T_PROVISION_HISTORY)?.map(|v| {
            let mut transfer_id = [0u8; 16];
            transfer_id.copy_from_slice(&v[..16]);
            HistoryWire {
                transfer_id,
                items: be64(&v[16..24]),
                bytes: be64(&v[24..]),
            }
        });
        Ok(Self {
            space_id: f.required(T_SPACE_ID)?,
            key_epoch,
            sealed: sealed.to_vec(),
            device_cert: device_cert.to_vec(),
            member_list: member_list.to_vec(),
            history,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inviter_hello() -> InviterHello {
        InviterHello {
            ed_pk: [1; 32],
            x_pk: [2; 32],
            name: "PC-Bureau".into(),
            platform: "windows".into(),
            member_count: Some(3),
            history_offer: Some(HistoryOffer {
                items: 812,
                bytes: 356_515_840,
            }),
        }
    }

    fn joiner_hello() -> JoinerHello {
        JoinerHello {
            ed_pk: [3; 32],
            x_pk: [4; 32],
            name: "Pixel".into(),
            platform: "android".into(),
        }
    }

    fn provision(history: bool, members: usize) -> ProvisionBody {
        ProvisionBody {
            space_id: [5; 16],
            key_epoch: 1,
            sealed: vec![6; SEALED_SPACE_KEY_LEN],
            device_cert: vec![7; DEVICE_CERT_LEN],
            member_list: vec![8; MEMBER_LIST_BASE_LEN + MEMBER_ENTRY_LEN * members],
            history: history.then_some(HistoryWire {
                transfer_id: [9; 16],
                items: 10,
                bytes: 11,
            }),
        }
    }

    /// A body made of raw TLVs.
    fn raw(tlvs: &[(u8, &[u8])]) -> Vec<u8> {
        let mut v = Vec::new();
        for (t, val) in tlvs {
            v.push(*t);
            v.extend_from_slice(&(val.len() as u16).to_be_bytes());
            v.extend_from_slice(val);
        }
        v
    }

    fn hello_tlvs<'a>(name: &'a [u8], platform: &'a [u8]) -> Vec<(u8, &'a [u8])> {
        vec![(1, &[1; 32]), (2, &[2; 32]), (3, name), (4, platform)]
    }

    #[test]
    fn round_trips() {
        let h = inviter_hello();
        assert_eq!(InviterHello::decode(&h.encode().unwrap()).unwrap(), h);
        let j = joiner_hello();
        assert_eq!(JoinerHello::decode(&j.encode().unwrap()).unwrap(), j);
        for history in [false, true] {
            let c = JoinerConfirm { history };
            let body = c.encode().unwrap();
            assert_eq!(body, [0x01, 0x00, 0x01, u8::from(history)]);
            assert_eq!(JoinerConfirm::decode(&body).unwrap(), c);
        }
        for (history, members) in [(false, 1), (true, 1), (false, 20), (true, 119)] {
            let p = provision(history, members);
            assert_eq!(ProvisionBody::decode(&p.encode().unwrap()).unwrap(), p);
        }
    }

    #[test]
    fn optional_fields_may_be_absent_and_a_zero_offer_means_nothing() {
        let mut h = inviter_hello();
        h.member_count = None;
        h.history_offer = None;
        assert_eq!(InviterHello::decode(&h.encode().unwrap()).unwrap(), h);
        h.history_offer = Some(HistoryOffer {
            items: 0,
            bytes: 99,
        });
        let body = h.encode().unwrap();
        assert!(Fields::parse(&body).unwrap().get(T_HISTORY_OFFER).is_none());
        // A zero offer received from another implementation reads as no offer.
        let mut tlvs = hello_tlvs(b"A", b"windows");
        let zero = [0u8; 16];
        tlvs.push((6, &zero));
        assert_eq!(
            InviterHello::decode(&raw(&tlvs)).unwrap().history_offer,
            None
        );
    }

    #[test]
    fn sizes_match_the_spec() {
        // §6.1: reveal <= 438 (header 2 + Ya 32 + tag 16 + body), joiner_hello <= 382
        // (header 2 + tag 16 + body), joiner_confirm 22, provision 517 / 552 + 64 m.
        // 64 phone emoji are 64 characters and 256 bytes: the longest valid name.
        let longest = "\u{1f4f1}".repeat(64);
        assert_eq!(longest.len(), 256);
        assert!(is_valid_name(&longest));
        let h = InviterHello {
            name: longest.clone(),
            platform: "p".repeat(32),
            ..inviter_hello()
        };
        assert_eq!(2 + 32 + 16 + h.encode().unwrap().len(), 438);
        let j = JoinerHello {
            name: longest,
            platform: "p".repeat(32),
            ..joiner_hello()
        };
        assert_eq!(2 + 16 + j.encode().unwrap().len(), 382);
        assert_eq!(
            2 + 16 + JoinerConfirm { history: true }.encode().unwrap().len(),
            22
        );
        for m in [1usize, 3, 20, 119] {
            let without = 2 + 16 + provision(false, m).encode().unwrap().len();
            let with = 2 + 16 + provision(true, m).encode().unwrap().len();
            assert_eq!(without, 517 + 64 * m);
            assert_eq!(with, 552 + 64 * m);
        }
        // m = 119 fits the 8192-byte frame, m = 120 does not (§6.6).
        let frame_len =
            |history: bool, m: usize| 2 + 16 + provision(history, m).encode().unwrap().len();
        assert!(frame_len(true, 119) <= crate::pairing::MAX_FRAME_LEN);
        assert!(frame_len(false, 119) <= crate::pairing::MAX_FRAME_LEN);
        assert!(frame_len(false, 120) > crate::pairing::MAX_FRAME_LEN);
    }

    #[test]
    fn p15_unknown_tags_are_ignored_below_0x80_and_critical_above() {
        // JoinerConfirm knows tag 1 only: 0x02..0x7f are extensions to ignore.
        let ext = raw(&[(1, &[1]), (0x02, b"future"), (0x40, b"x"), (0x7f, b"")]);
        assert_eq!(
            JoinerConfirm::decode(&ext).unwrap(),
            JoinerConfirm { history: true }
        );
        // An unknown tag after the known ones of a hello is ignored too.
        let mut hello = hello_tlvs(b"Pixel", b"android");
        hello.push((0x40, b"future"));
        let j = JoinerHello::decode(&raw(&hello)).unwrap();
        assert_eq!((j.name.as_str(), j.platform.as_str()), ("Pixel", "android"));
        // Tag 0 is not a tag.
        let zero = raw(&[(0, b""), (1, &[1])]);
        assert_eq!(
            JoinerConfirm::decode(&zero).unwrap_err(),
            CoreError::PairingFailed
        );
        // A critical tag: unsupported, whatever the other fields are.
        let mut crit = hello_tlvs(b"Pixel", b"android");
        crit.push((0x90, b"x"));
        assert_eq!(
            JoinerHello::decode(&raw(&crit)).unwrap_err(),
            CoreError::PairingUnsupported
        );
        assert_eq!(
            JoinerHello::decode(&raw(&[(0x80, b"")])).unwrap_err(),
            CoreError::PairingUnsupported
        );
        assert_eq!(
            JoinerConfirm::decode(&raw(&[(0xff, b"")])).unwrap_err(),
            CoreError::PairingUnsupported
        );
        // ... but a body that is also malformed is malformed first.
        let mut crit_then_junk = raw(&crit);
        crit_then_junk.push(0xff);
        assert_eq!(
            JoinerHello::decode(&crit_then_junk).unwrap_err(),
            CoreError::PairingFailed
        );
        let crit_disorder = raw(&[(0x90, b""), (0x91, b""), (0x91, b"")]);
        assert_eq!(
            JoinerHello::decode(&crit_disorder).unwrap_err(),
            CoreError::PairingFailed
        );
    }

    #[test]
    fn p15_structure_errors() {
        let good = raw(&hello_tlvs(b"Pixel", b"android"));
        assert!(JoinerHello::decode(&good).is_ok());
        // Truncation at every length.
        for cut in 1..good.len() {
            let r = JoinerHello::decode(&good[..cut]);
            // Cutting exactly after a TLV leaves a body that misses later fields.
            assert_eq!(r.unwrap_err(), CoreError::PairingFailed, "cut at {cut}");
        }
        // Duplicate, disorder.
        let dup = raw(&[
            (1, &[1; 32]),
            (1, &[1; 32]),
            (2, &[2; 32]),
            (3, b"P"),
            (4, b"a"),
        ]);
        assert_eq!(
            JoinerHello::decode(&dup).unwrap_err(),
            CoreError::PairingFailed
        );
        let disorder = raw(&[(2, &[2; 32]), (1, &[1; 32]), (3, b"P"), (4, b"a")]);
        assert_eq!(
            JoinerHello::decode(&disorder).unwrap_err(),
            CoreError::PairingFailed
        );
        // Length field beyond the body.
        let mut long = good.clone();
        long[2] = 0xff;
        assert_eq!(
            JoinerHello::decode(&long).unwrap_err(),
            CoreError::PairingFailed
        );
        // Empty body: every required field is missing.
        assert_eq!(
            JoinerHello::decode(&[]).unwrap_err(),
            CoreError::PairingFailed
        );
    }

    #[test]
    fn p15_required_fields_and_lengths() {
        let full = hello_tlvs(b"Pixel", b"android");
        for skip in 0..4 {
            let missing: Vec<_> = full
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, t)| *t)
                .collect();
            assert_eq!(
                JoinerHello::decode(&raw(&missing)).unwrap_err(),
                CoreError::PairingFailed,
                "missing field {skip}"
            );
        }
        // Wrong key lengths.
        for (tag, len) in [(1u8, 31usize), (1, 33), (2, 31), (2, 0)] {
            let key = vec![1u8; len];
            let tlvs: Vec<(u8, &[u8])> = full
                .iter()
                .map(|(t, v)| {
                    if *t == tag {
                        (*t, key.as_slice())
                    } else {
                        (*t, *v)
                    }
                })
                .collect();
            assert!(JoinerHello::decode(&raw(&tlvs)).is_err());
        }
        // Optional fields with a wrong length are errors, not ignored.
        let mut inv = hello_tlvs(b"PC", b"windows");
        inv.push((5, &[0, 0, 1]));
        assert!(InviterHello::decode(&raw(&inv)).is_err());
        let mut inv = hello_tlvs(b"PC", b"windows");
        inv.push((6, &[0; 15]));
        assert!(InviterHello::decode(&raw(&inv)).is_err());
        // JoinerConfirm: only 0x00 and 0x01.
        for v in [2u8, 0x80, 0xff] {
            assert!(JoinerConfirm::decode(&raw(&[(1, &[v])])).is_err());
        }
        assert!(JoinerConfirm::decode(&raw(&[(1, &[0, 0])])).is_err());
        assert!(JoinerConfirm::decode(&[]).is_err());
    }

    #[test]
    fn p15_name_and_platform_rules() {
        let decode =
            |name: &[u8], platform: &[u8]| JoinerHello::decode(&raw(&hello_tlvs(name, platform)));
        assert!(decode("Pixel 8 Pro".as_bytes(), b"android").is_ok());
        assert!(decode("Alice's phone 📱".as_bytes(), b"android").is_ok());
        assert!(decode("a".repeat(64).as_bytes(), b"cli").is_ok());
        let bad_names: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            "a".repeat(65).into_bytes(),
            "é".repeat(65).into_bytes(),
            b"bell\x07".to_vec(),
            b"tab\there".to_vec(),
            b"line\nbreak".to_vec(),
            b"nul\0".to_vec(),
            b"trailing ".to_vec(),
            b" leading".to_vec(),
            "nbsp\u{a0}".as_bytes().to_vec(),
            "ideographic\u{3000}".as_bytes().to_vec(),
            vec![0xff, 0xfe],
            "c1\u{85}".as_bytes().to_vec(),
        ];
        for n in &bad_names {
            assert_eq!(
                decode(n, b"android").unwrap_err(),
                CoreError::PairingFailed,
                "{n:?}"
            );
        }
        for p in [
            &b""[..],
            b"Android",
            b"and roid",
            b"and_roid",
            b"android\n",
            &[b'a'; 33],
            "andrøid".as_bytes(),
        ] {
            assert_eq!(
                decode(b"Pixel", p).unwrap_err(),
                CoreError::PairingFailed,
                "{p:?}"
            );
        }
        assert!(decode(b"Pixel", &[b'a'; 32]).is_ok());
        assert!(decode(b"Pixel", b"x86-64").is_ok());
        // Own text is checked before it is sent.
        let mut j = joiner_hello();
        j.name = " ".into();
        assert_eq!(j.encode().unwrap_err().code(), "invalid_input");
        let mut j = joiner_hello();
        j.platform = "Windows".into();
        assert_eq!(j.encode().unwrap_err().code(), "invalid_input");
    }

    #[test]
    fn provision_field_rules() {
        let p = provision(false, 2);
        let tlvs = |epoch: u32, sealed: usize, cert: usize, list: usize| {
            raw(&[
                (1, &[5; 16]),
                (2, &epoch.to_be_bytes()),
                (3, &vec![6; sealed]),
                (4, &vec![7; cert]),
                (5, &vec![8; list]),
            ])
        };
        let ok = tlvs(1, 104, 208, 152 + 128);
        assert!(ProvisionBody::decode(&ok).is_ok());
        assert_eq!(
            ProvisionBody::decode(&ok).unwrap().member_list.len(),
            p.member_list.len()
        );
        assert!(ProvisionBody::decode(&tlvs(SPACE_KEY_EPOCH_MAX, 104, 208, 152)).is_ok());
        // Reserved or null epoch, wrong lengths.
        for bad in [
            tlvs(0, 104, 208, 152),
            tlvs(SPACE_KEY_EPOCH_MAX + 1, 104, 208, 152),
            tlvs(u32::MAX, 104, 208, 152),
            tlvs(1, 103, 208, 152),
            tlvs(1, 105, 208, 152),
            tlvs(1, 104, 207, 152),
            tlvs(1, 104, 209, 152),
            tlvs(1, 104, 208, 151),
            tlvs(1, 104, 208, 152 + 63),
            tlvs(1, 104, 208, 152 + 65),
            tlvs(1, 104, 208, 0),
        ] {
            assert_eq!(
                ProvisionBody::decode(&bad).unwrap_err(),
                CoreError::PairingFailed
            );
        }
        // History: 32 bytes or an error.
        let short_history = raw(&[
            (1, &[5; 16]),
            (2, &1u32.to_be_bytes()),
            (3, &[6; 104]),
            (4, &[7; 208]),
            (5, &[8; 152]),
            (6, &[9; 31]),
        ]);
        assert_eq!(
            ProvisionBody::decode(&short_history).unwrap_err(),
            CoreError::PairingFailed
        );
        // Encoding refuses what a reader would refuse.
        let mut bad = provision(false, 1);
        bad.key_epoch = 0;
        assert!(bad.encode().is_err());
        let mut bad = provision(false, 1);
        bad.sealed.pop();
        assert!(bad.encode().is_err());
        let mut bad = provision(false, 1);
        bad.member_list.push(0);
        assert!(bad.encode().is_err());
    }

    #[test]
    fn writer_refuses_disorder_and_oversize_values() {
        let mut w = Writer::new();
        w.put(2, b"a").unwrap();
        assert!(w.put(2, b"b").is_err());
        assert!(w.put(1, b"b").is_err());
        assert_eq!(
            w.put(3, &vec![0u8; 70_000]).unwrap_err(),
            CoreError::PairingFailed
        );
        w.put(3, &vec![0u8; 65_535]).unwrap();
    }
}
