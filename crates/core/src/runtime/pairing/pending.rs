//! The verified batch kept pending, and the resumption after a stop (`spec/pairing-v1.md`
//! §8.3, §11.4).
//!
//! Between the moment the new device verifies the batch (N10) and the moment the relay says the
//! channel is completed (N12), it holds the keys of a device that is not registered yet and the
//! space key. If the app stops there, nothing may be lost, and nothing may stay in the clear: the
//! batch is sealed under the wrapping key of the OS secure storage, in the `secrets` table of
//! the store (the same protection as the device keys it becomes), **before** the acknowledgement
//! that lets the inviter register the device leaves. Provisioning then reads that stored copy and
//! erases it in the same transaction, so a normal completion and a resumption after a stop are
//! the same code.

use std::sync::atomic::Ordering;

use zeroize::Zeroizing;

use super::history::AwaitedHistory;
use super::join::JoinOutcome;
use crate::crypto::Reader;
use crate::device::{DeviceKeys, DevicePublic, TrustStore};
use crate::error::{CoreError, Result};
use crate::pairing::{HistoryAnnouncement, JoinerProvision, PeerInfo};
use crate::runtime::engine::{Engine, KnownMember, Provision, ProvisionExtras};
use crate::runtime::transport::{ClientInfo, RelayError, SessionRequest};
use crate::{SpaceKey, FORMAT_MAX, FORMAT_MIN};

/// Sealed record of the verified batch of a new device.
pub(crate) const JOINER_PENDING: &str = "pairing_pending";
/// Sealed record of the keys of a founder whose space may be created (`pairing-v1.md` §10.2).
pub(crate) const FOUNDER_PENDING: &str = "founder_pending";

const LOT_VERSION: u8 = 1;

/// What [`Engine::resume_pairing`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// Nothing was pending (or a founder whose space the relay does not know yet: it can still
    /// call [`Engine::create_space`]).
    Nothing,
    /// The relay knows this device: it was provisioned, a session opened and a sync done.
    Provisioned,
    /// The relay does not know this device (the inviter did not register it): the pending
    /// batch was erased.
    Discarded,
}

/// What a settled pending batch became (the public [`ResumeOutcome`], with the outcome of the
/// provisioning for the session that needs it).
pub(super) enum Resumed {
    Nothing,
    Provisioned(JoinOutcome),
    Discarded,
}

/// What the store holds under a pending name.
pub(super) enum Stored<T> {
    Absent,
    /// A record that cannot be opened (another wrapping key) or read: it is of no use.
    Unreadable,
    Found(T),
}

// ---------------------------------------------------------------------------
// Encoding of the batch
// ---------------------------------------------------------------------------

fn put(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(data);
}

fn put_lv(out: &mut Vec<u8>, data: &[u8]) -> Result<()> {
    let len = u32::try_from(data.len()).map_err(|_| CoreError::input("pending record: field"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
    Ok(())
}

fn read_lv<'a>(r: &mut Reader<'a>) -> Result<&'a [u8]> {
    let len = usize::try_from(r.u32().map_err(|_| corrupt())?).map_err(|_| corrupt())?;
    r.take(len).map_err(|_| corrupt())
}

fn read_string(r: &mut Reader<'_>) -> Result<String> {
    String::from_utf8(read_lv(r)?.to_vec()).map_err(|_| corrupt())
}

fn corrupt() -> CoreError {
    CoreError::Storage {
        reason: "corrupt pairing record".into(),
    }
}

/// The batch as stored: version, device keys, space key and epoch, trust store, own name, the
/// inviter (name, platform, keys), the announced history. Secrets live in a zeroized buffer.
pub(super) fn encode_lot(p: &JoinerProvision) -> Result<Zeroizing<Vec<u8>>> {
    let seed = Zeroizing::new(p.keys.ed25519_seed());
    let x = Zeroizing::new(p.keys.x25519_secret());
    let key = Zeroizing::new(p.space_key.to_bytes());
    let trust = p.trust.to_json()?;
    let history = p.history.as_ref();
    let size = 1
        + seed.len()
        + x.len()
        + 4
        + key.len()
        + 4
        + trust.len()
        + 4
        + p.device_name.len()
        + 4
        + p.inviter.name.len()
        + 4
        + p.inviter.platform.len()
        + 4
        + p.inviter.public.ed25519.len()
        + 4
        + p.inviter.public.x25519.len()
        + 1
        + history.map_or(0, |h| 4 + h.transfer_id.len() + 16);
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    out.push(LOT_VERSION);
    put(&mut out, &seed);
    put(&mut out, &x);
    out.extend_from_slice(&p.space_key.epoch().to_be_bytes());
    put(&mut out, &key);
    put_lv(&mut out, trust.as_bytes())?;
    put_lv(&mut out, p.device_name.as_bytes())?;
    put_lv(&mut out, p.inviter.name.as_bytes())?;
    put_lv(&mut out, p.inviter.platform.as_bytes())?;
    put_lv(&mut out, &p.inviter.public.ed25519)?;
    put_lv(&mut out, &p.inviter.public.x25519)?;
    match history {
        Some(h) => {
            out.push(1);
            put_lv(&mut out, h.transfer_id.as_bytes())?;
            out.extend_from_slice(&h.items.to_be_bytes());
            out.extend_from_slice(&h.bytes.to_be_bytes());
        }
        None => out.push(0),
    }
    Ok(out)
}

pub(super) fn decode_lot(bytes: &[u8]) -> Result<JoinerProvision> {
    let mut r = Reader::new(bytes);
    if r.u8().map_err(|_| corrupt())? != LOT_VERSION {
        return Err(corrupt());
    }
    let seed = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let x = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let epoch = r.u32().map_err(|_| corrupt())?;
    let key = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let trust = TrustStore::from_json(&read_string(&mut r)?)?;
    let device_name = read_string(&mut r)?;
    let name = read_string(&mut r)?;
    let platform = read_string(&mut r)?;
    let ed25519 = read_lv(&mut r)?.to_vec();
    let x25519 = read_lv(&mut r)?.to_vec();
    let history = match r.u8().map_err(|_| corrupt())? {
        0 => None,
        1 => Some(HistoryAnnouncement {
            transfer_id: read_string(&mut r)?,
            items: r.u64().map_err(|_| corrupt())?,
            bytes: r.u64().map_err(|_| corrupt())?,
        }),
        _ => return Err(corrupt()),
    };
    if r.remaining() != 0 {
        return Err(corrupt());
    }
    Ok(JoinerProvision {
        keys: DeviceKeys::from_secrets(seed.as_slice(), x.as_slice())?,
        trust,
        space_key: SpaceKey::from_bytes(epoch, key.as_slice())?,
        device_name,
        inviter: PeerInfo {
            name,
            platform,
            public: DevicePublic { ed25519, x25519 },
        },
        history,
    })
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

impl Engine {
    /// Whether a pairing was interrupted and left something sealed in the store (the verified
    /// batch of a new device, or the keys of a founder): call
    /// [`resume_pairing`](Self::resume_pairing) when the device starts and is not provisioned.
    pub fn has_pending_pairing(&self) -> Result<bool> {
        self.store
            .read(|db| Ok(db.secret_exists(JOINER_PENDING)? || db.secret_exists(FOUNDER_PENDING)?))
    }

    /// Settles a pairing that a stop interrupted (`pairing-v1.md` §11.4, §10.2).
    ///
    /// * A verified batch is pending: the device tries a challenge/response with the pending
    ///   keys. If the relay knows the device (the inviter registered it), the device is
    ///   provisioned, a session is opened and a sync run; if it answers `401` (the inviter did
    ///   not register it), the batch is erased.
    /// * The keys of a founder are pending: same test; if the space was created and the answer
    ///   lost, the device is provisioned; otherwise the keys are kept for the next
    ///   [`create_space`](Self::create_space).
    ///
    /// A network failure is an error and changes nothing: call again later.
    pub fn resume_pairing(&self) -> Result<ResumeOutcome> {
        if self.is_provisioned() {
            // Both records are erased with the provisioning; a leftover is of no use.
            self.store.tx(|db| {
                db.secret_delete(JOINER_PENDING)?;
                db.secret_delete(FOUNDER_PENDING)
            })?;
            return Ok(ResumeOutcome::Nothing);
        }
        match self.resume_joiner()? {
            Resumed::Nothing => self.resume_founder(),
            Resumed::Provisioned(_) => Ok(ResumeOutcome::Provisioned),
            Resumed::Discarded => Ok(ResumeOutcome::Discarded),
        }
    }

    /// The verified batch of a new device, if one is pending and no live session owns it:
    /// provisioned when the relay knows the device, erased when it does not.
    ///
    /// A live session is waiting for the inviter, so the relay does not know the device *yet*:
    /// settling the batch from under it would erase the keys of a pairing that is about to
    /// succeed. The session settles its own batch ([`resolve_lot`](Self::resolve_lot)).
    pub(super) fn resume_joiner(&self) -> Result<Resumed> {
        if self.lot_held.load(Ordering::SeqCst) {
            return Ok(Resumed::Nothing);
        }
        self.resolve_lot()
    }

    /// Asks the relay whether it registered the device of the pending batch, and acts on it.
    pub(super) fn resolve_lot(&self) -> Result<Resumed> {
        match self.load_lot()? {
            Stored::Absent => Ok(Resumed::Nothing),
            Stored::Unreadable => {
                self.discard_lot()?;
                Ok(Resumed::Discarded)
            }
            Stored::Found(lot) => {
                if self.probe_registration(&lot.keys, &lot.trust.space_id())? {
                    Ok(Resumed::Provisioned(self.complete_lot(lot)?))
                } else {
                    self.discard_lot()?;
                    Ok(Resumed::Discarded)
                }
            }
        }
    }

    pub(super) fn load_lot(&self) -> Result<Stored<JoinerProvision>> {
        let raw = match self.store.read(|db| db.secret_get(JOINER_PENDING)) {
            Ok(Some(raw)) => raw,
            Ok(None) => return Ok(Stored::Absent),
            // Another wrapping key (restored backup): the record can never be opened.
            Err(CoreError::DecryptionFailed) => return Ok(Stored::Unreadable),
            Err(e) => return Err(e),
        };
        Ok(decode_lot(&raw).map_or(Stored::Unreadable, Stored::Found))
    }

    /// Seals the verified batch of a joining session in the store, and marks it as owned by that
    /// session ([`release_lot`](Self::release_lot) when it ends). One batch at a time: a second
    /// session that reaches this point fails instead of overwriting the first one's keys.
    pub(super) fn save_lot(&self, lot: &JoinerProvision) -> Result<()> {
        if self.lot_held.swap(true, Ordering::SeqCst) {
            return Err(CoreError::input(
                "another pairing is in progress on this device",
            ));
        }
        let saved = encode_lot(lot).and_then(|bytes| {
            self.store.tx(|db| {
                if db.secret_exists(JOINER_PENDING)? {
                    return Err(CoreError::input(
                        "another pairing is pending on this device",
                    ));
                }
                db.secret_put(JOINER_PENDING, &bytes)?;
                // The relay this batch belongs to, so that a stop does not lose it.
                self.stage_pending_relay(db)
            })
        });
        if saved.is_err() {
            self.lot_held.store(false, Ordering::SeqCst);
        }
        saved
    }

    /// The session that owns the sealed batch ended (or was dropped): a resumption may settle it.
    pub(super) fn release_lot(&self) {
        self.lot_held.store(false, Ordering::SeqCst);
    }

    /// Replaces the sealed batch (tests: a reader that dropped a field).
    #[cfg(test)]
    pub(super) fn replace_lot(&self, lot: &JoinerProvision) -> Result<()> {
        let bytes = encode_lot(lot)?;
        self.store.tx(|db| db.secret_put(JOINER_PENDING, &bytes))
    }

    pub(super) fn discard_lot(&self) -> Result<()> {
        self.store.tx(|db| db.secret_delete(JOINER_PENDING))?;
        // Nothing pending any more: the relay of that batch is forgotten (unless a session still
        // holds it, which puts it back itself).
        self.settle_relay();
        Ok(())
    }

    /// N13 from the stored copy (`pairing-v1.md` §8.4): provisions the device, erases the
    /// pending record in the same transaction, opens a session, syncs, and arranges for the
    /// announced history to be accepted without a question.
    pub(super) fn complete_pending(&self) -> Result<JoinOutcome> {
        match self.load_lot()? {
            Stored::Found(lot) => self.complete_lot(lot),
            Stored::Absent | Stored::Unreadable => Err(CoreError::Storage {
                reason: "the pending pairing record is gone".into(),
            }),
        }
    }

    pub(super) fn complete_lot(&self, lot: JoinerProvision) -> Result<JoinOutcome> {
        let JoinerProvision {
            keys,
            trust,
            space_key,
            device_name,
            inviter,
            history,
        } = lot;
        let space_id = trust.space_id();
        let device_id = keys.device_id();
        let extras = ProvisionExtras {
            known_members: vec![KnownMember {
                ed25519: inviter.public.ed25519.clone(),
                name: inviter.name.clone(),
                platform: inviter.platform.clone(),
            }],
            awaited_history: match &history {
                Some(h) => Some(AwaitedHistory {
                    transfer_id: h.transfer_id.clone(),
                    from_device_id: inviter.public.device_id()?,
                    items: h.items,
                    bytes: h.bytes,
                    announced_at_ms: self.clock.now_ms(),
                }),
                None => None,
            },
            // Provisioning erases both pending secrets (§12): the joiner's lot, and any founder
            // lot left sealed by an earlier failed `create_space` on this device.
            drop_secrets: vec![JOINER_PENDING, FOUNDER_PENDING],
        };
        self.provision_with(
            Provision {
                keys,
                trust,
                space_key,
                device_name,
            },
            extras,
        )?;
        self.first_contact();
        Ok(JoinOutcome {
            device_id,
            space_id,
            history,
        })
    }

    /// The first session and sync of a device that was just provisioned. The device is
    /// provisioned whatever happens here: a relay that cannot be reached right now, or an
    /// update that is required, is seen through [`space_status`](Self::space_status) and by the
    /// next sync.
    pub(super) fn first_contact(&self) {
        if self.ensure_session().is_ok() {
            let _ = self.sync();
        }
    }

    /// Whether the relay knows the device of `keys` in the space `space_id`, by the
    /// challenge/response of `api-v1.md` §3.3 (`pairing-v1.md` §10.2, §11.4).
    ///
    /// `Ok(false)` means the relay does not know it (`401`), or knows it only as revoked, or has
    /// no space: what is pending is of no use. A version problem (`update_required`,
    /// `format_unsupported`) is raised after the signature is verified, so it means "known".
    /// Any other failure, the network above all, is an error: nothing may be decided on it.
    ///
    /// Each attempt asks for a fresh challenge (a challenge is single-use, so an attempt whose
    /// answer was lost can never be repeated with it). The session it obtains is closed at once;
    /// the engine opens its own.
    pub(super) fn probe_registration(&self, keys: &DeviceKeys, space_id: &str) -> Result<bool> {
        // A device that is not provisioned has no session: do not leave one behind.
        self.transport.set_session_token(None);
        let device_id = keys.device_id();
        let answer = super::retrying(None, || {
            let challenge = self.transport.auth_challenge(&device_id)?;
            let signature = keys
                .sign_relay_auth(space_id, &challenge.challenge)
                .map_err(|e| RelayError::Network(format!("cannot sign challenge: {e}")))?;
            self.transport.auth_session(&SessionRequest {
                device_id: device_id.clone(),
                challenge: challenge.challenge,
                signature,
                client: ClientInfo {
                    format_min: FORMAT_MIN,
                    format_max: FORMAT_MAX,
                    app_version: self.cfg.app_version.clone(),
                    platform: self.cfg.platform.clone(),
                },
            })
        });
        match answer {
            Ok(grant) => {
                self.transport.set_session_token(Some(grant.token));
                let _ = self.transport.delete_session();
                self.transport.set_session_token(None);
                Ok(true)
            }
            Err(RelayError::Api(p)) => match (p.status, p.code.as_str()) {
                (401, _) | (403, "revoked") | (409, "space_not_initialized") => Ok(false),
                (409, "update_required" | "format_unsupported") => Ok(true),
                _ => Err(RelayError::Api(p).into()),
            },
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_space_id;

    fn sample() -> JoinerProvision {
        let me = DeviceKeys::generate().unwrap();
        let inviter = DeviceKeys::generate().unwrap();
        let space_id = new_space_id().unwrap();
        let mut trust = TrustStore::new(&space_id, &me.public(), 3).unwrap();
        trust.add_verified_peer(&inviter.public()).unwrap();
        JoinerProvision {
            keys: me,
            trust,
            space_key: SpaceKey::generate(3).unwrap(),
            device_name: "Pixel".into(),
            inviter: PeerInfo {
                name: "PC-Bureau".into(),
                platform: "windows".into(),
                public: inviter.public(),
            },
            history: Some(HistoryAnnouncement {
                transfer_id: "c1a7e0d2-5b3f-4e8a-9c61-2d4f6a8b0c1e".into(),
                items: 812,
                bytes: 356_515_840,
            }),
        }
    }

    #[test]
    fn the_pending_batch_round_trips() {
        let lot = sample();
        let bytes = encode_lot(&lot).unwrap();
        let back = decode_lot(&bytes).unwrap();
        assert_eq!(back.keys.device_id(), lot.keys.device_id());
        assert_eq!(back.keys.ed25519_seed(), lot.keys.ed25519_seed());
        assert_eq!(back.keys.x25519_secret(), lot.keys.x25519_secret());
        assert_eq!(back.trust, lot.trust);
        assert_eq!(back.space_key.to_bytes(), lot.space_key.to_bytes());
        assert_eq!(back.space_key.epoch(), 3);
        assert_eq!(back.device_name, "Pixel");
        assert_eq!(back.inviter, lot.inviter);
        assert_eq!(back.history, lot.history);
        let mut without = sample();
        without.history = None;
        let back = decode_lot(&encode_lot(&without).unwrap()).unwrap();
        assert!(back.history.is_none());
    }

    #[test]
    fn a_damaged_pending_batch_is_refused_without_panicking() {
        let bytes = encode_lot(&sample()).unwrap();
        // Every truncation, a trailing byte, a wrong version, and a flipped length.
        for cut in 0..bytes.len() {
            assert!(decode_lot(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert!(decode_lot(&longer).is_err());
        let mut other = bytes.to_vec();
        other[0] = 2;
        assert!(decode_lot(&other).is_err());
        let mut huge = bytes.to_vec();
        // The length of the trust store JSON sits right after the key material.
        huge[1 + 64 + 4 + 32..1 + 64 + 4 + 32 + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode_lot(&huge).is_err());
    }
}
