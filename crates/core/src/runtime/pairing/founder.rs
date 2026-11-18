//! Creation of the space by the first device (`spec/pairing-v1.md` §10).
//!
//! The founder uses no PAKE: nobody exists yet to establish trust with. Its authorisation is the
//! install token of the relay (`api-v1.md` §3.1) and its proof of possession a self-signed
//! `DeviceCert`, which gives no trust to anyone else.
//!
//! Its keys, the space id and the space key are sealed in the store **before** the request
//! leaves: if the answer is lost, the space exists on the relay and only these keys can ever
//! reach it. The install token is never stored: it is typed or pasted, used, and dropped.

use zeroize::Zeroizing;

use super::pending::{Stored, FOUNDER_PENDING, JOINER_PENDING};
use crate::crypto::Reader;
use crate::device::{DeviceKeys, TrustStore};
use crate::error::{CoreError, Result};
use crate::new_space_id;
use crate::runtime::engine::{Engine, Provision, ProvisionExtras};
use crate::runtime::relay::RelayHold;
use crate::runtime::transport::{NewSpace, RelayError};
use crate::SpaceKey;

const FOUNDER_VERSION: u8 = 1;
/// Longest install token accepted (the relay's is 39 characters with its separators).
const TOKEN_MAX_LEN: usize = 128;

/// What a founder keeps pending: the identity and the space it is about to create.
pub(super) struct FounderMaterial {
    keys: DeviceKeys,
    space_id: String,
    space_key: SpaceKey,
    device_name: String,
}

impl FounderMaterial {
    /// Id of the device the founder will be.
    #[cfg(test)]
    pub(super) fn device_id(&self) -> String {
        self.keys.device_id()
    }
}

fn corrupt() -> CoreError {
    CoreError::Storage {
        reason: "corrupt pairing record".into(),
    }
}

fn encode(m: &FounderMaterial) -> Result<Zeroizing<Vec<u8>>> {
    let seed = Zeroizing::new(m.keys.ed25519_seed());
    let x = Zeroizing::new(m.keys.x25519_secret());
    let key = Zeroizing::new(m.space_key.to_bytes());
    let name = m.device_name.as_bytes();
    let name_len = u32::try_from(name.len()).map_err(|_| corrupt())?;
    let space_id = crate::crypto::parse_uuid(&m.space_id, "space id")?;
    let mut out = Zeroizing::new(Vec::with_capacity(
        1 + seed.len() + x.len() + space_id.len() + key.len() + 4 + name.len(),
    ));
    out.push(FOUNDER_VERSION);
    out.extend_from_slice(&seed);
    out.extend_from_slice(&x);
    out.extend_from_slice(&space_id);
    out.extend_from_slice(&key);
    out.extend_from_slice(&name_len.to_be_bytes());
    out.extend_from_slice(name);
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<FounderMaterial> {
    let mut r = Reader::new(bytes);
    if r.u8().map_err(|_| corrupt())? != FOUNDER_VERSION {
        return Err(corrupt());
    }
    let seed = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let x = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let space_id = r.array::<16>().map_err(|_| corrupt())?;
    let key = Zeroizing::new(r.array::<32>().map_err(|_| corrupt())?);
    let len = usize::try_from(r.u32().map_err(|_| corrupt())?).map_err(|_| corrupt())?;
    let name =
        String::from_utf8(r.take(len).map_err(|_| corrupt())?.to_vec()).map_err(|_| corrupt())?;
    if r.remaining() != 0 {
        return Err(corrupt());
    }
    Ok(FounderMaterial {
        keys: DeviceKeys::from_secrets(seed.as_slice(), x.as_slice())?,
        space_id: crate::crypto::format_uuid(&space_id),
        space_key: SpaceKey::from_bytes(1, key.as_slice())?,
        device_name: name,
    })
}

impl Engine {
    /// Creates the space from this device, which becomes its first member
    /// (`pairing-v1.md` §10.2).
    ///
    /// `install_token` is the token the relay prints at start (`api-v1.md` §3.1), typed or
    /// pasted by the user; it is used for this call only. On success the device is provisioned,
    /// a session is open and the first sync done.
    ///
    /// | answer | effect |
    /// |--------|--------|
    /// | `201` | provisioned |
    /// | `403 install_token_invalid` | `Relay { 403, "install_token_invalid" }`; the keys stay sealed, the token is not consumed: call again with the right one |
    /// | `409 space_exists` | a challenge/response with the pending keys: if it succeeds the answer of an earlier call was lost and this device is provisioned; otherwise another device created the space, the keys are erased and `Relay { 409, "space_exists" }` tells the app to offer "Rejoindre" |
    /// | no answer | `Network`; the keys stay sealed; call again |
    ///
    /// A device that has a verified batch pending is settled first
    /// ([`resume_pairing`](Self::resume_pairing)).
    pub fn create_space(&self, install_token: &str, device_name: &str) -> Result<()> {
        self.create_space_at(install_token, device_name, None)
    }

    /// [`create_space`](Self::create_space) at a relay the user may have named: `relay_url` is the
    /// relay of a creation link (`https://<relay>/create#<token>`) or the "Relais" field
    /// (`https://host[:port][/prefix]`); absent, the device uses the relay it has. Same principle
    /// as [`join_space_at`](Self::join_space_at): **a relay that was read is not trusted**, the
    /// application shows it before the call; the transport is pointed at it for this call only and
    /// the relay is kept when the space is created (the keys sealed before the request keep it
    /// as well, so that a stop does not lose it); any other end puts the relay back. An invalid
    /// URL is `InvalidInput`, no relay at all `RelayNotConfigured`, with nothing sent.
    pub fn create_space_at(
        &self,
        install_token: &str,
        device_name: &str,
        relay_url: Option<&str>,
    ) -> Result<()> {
        self.resume_joiner()?;
        if self.is_provisioned() {
            return Err(CoreError::input("device already provisioned"));
        }
        let name = super::check_device_name(device_name)?;
        let platform = self.cfg.platform.clone();
        if !crate::pairing::is_valid_platform_name(&platform) {
            return Err(CoreError::input("invalid platform"));
        }
        let token = install_token.trim();
        if token.is_empty() || token.len() > TOKEN_MAX_LEN {
            return Err(CoreError::input("invalid install token"));
        }
        // Held until the end of the call, whatever the way (an error, a panic).
        let _hold = RelayHold::begin(self, relay_url, None)?;
        self.found_space(token, name, platform)
    }

    /// The request of the creation, at the relay the call holds.
    fn found_space(&self, token: &str, name: String, platform: String) -> Result<()> {
        // Keys sealed by an attempt at another relay are of no use here: settled with that relay
        // first (a creation whose answer was lost is recognised), then dropped.
        self.drop_founder_of_another_relay()?;
        let material = self.founder_material(&name)?;
        let now = u64::try_from(self.clock.now_ms()).unwrap_or(0);
        let certificate = material
            .keys
            .sign_device_cert(&material.space_id, &material.keys.public(), now)?
            .to_bytes()?;
        let request = NewSpace {
            install_token: token.to_owned(),
            space_id: material.space_id.clone(),
            name,
            platform,
            certificate,
        };
        match super::retrying(None, || self.transport.post_space(&request)) {
            Ok(created) => {
                if created.space_id != material.space_id
                    || created.device_id != material.keys.device_id()
                {
                    return Err(CoreError::Network {
                        reason: "unexpected answer".into(),
                    });
                }
                self.complete_founder(material)
            }
            Err(RelayError::Api(p)) if p.status == 409 && p.code == "space_exists" => {
                // The answer of an earlier call may have been lost: is this device the founder?
                if self.probe_registration(&material.keys, &material.space_id)? {
                    self.complete_founder(material)
                } else {
                    self.store.tx(|db| db.secret_delete(FOUNDER_PENDING))?;
                    Err(RelayError::Api(p).into())
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Settles a founder whose keys are pending (§10.2), without the install token: if the
    /// relay knows the device, the space was created and this device is provisioned.
    pub(super) fn resume_founder(&self) -> Result<super::ResumeOutcome> {
        match self.load_founder()? {
            Stored::Absent => Ok(super::ResumeOutcome::Nothing),
            Stored::Unreadable => {
                self.store.tx(|db| db.secret_delete(FOUNDER_PENDING))?;
                self.settle_relay();
                Ok(super::ResumeOutcome::Discarded)
            }
            Stored::Found(m) => {
                if self.probe_registration(&m.keys, &m.space_id)? {
                    self.complete_founder(m)?;
                    Ok(super::ResumeOutcome::Provisioned)
                } else {
                    // Maybe the request never left: the keys stay for `create_space`.
                    Ok(super::ResumeOutcome::Nothing)
                }
            }
        }
    }

    /// The relay of sealed founder keys, if those keys were made for another relay than the
    /// current one.
    fn founder_relay_to_settle(&self) -> Result<Option<String>> {
        let pending = self
            .store
            .read(|db| db.meta_get_string(crate::runtime::relay::PENDING_RELAY_KEY))?;
        if pending == self.relay_url()
            || !self.store.read(|db| db.secret_exists(FOUNDER_PENDING))?
        {
            return Ok(None);
        }
        Ok(pending)
    }

    /// Sealed founder keys that were made for another relay than the current one: the relay they
    /// belong to is asked first (the creation may have gone through), then they are dropped.
    ///
    /// The transport is shared: it goes to the relay of the keys and back **only while this call
    /// holds the relay alone** ([`Engine::exclusive_relay`]). If another pairing is under way
    /// (it may be talking to the current relay with a secret of its own) the call is refused
    /// (`InvalidInput`) and the keys stay as they are: the user tries again when it is over.
    fn drop_founder_of_another_relay(&self) -> Result<()> {
        if self.founder_relay_to_settle()?.is_none() {
            return Ok(());
        }
        let _exclusive = self.exclusive_relay()?;
        // Asked again under the exclusive hold: what was seen may have changed meanwhile.
        let Some(pending) = self.founder_relay_to_settle()? else {
            return Ok(());
        };
        // The transport is on the current relay: point it at the pending one for the probe.
        // A relay that does not answer cannot say either: the user is moving on, and a space that
        // was created there without an answer holds nothing but its (empty) feed.
        let here = self.relay_url();
        self.point_transport_at(Some(&pending))?;
        let settled = self.resume_founder();
        if matches!(settled, Ok(super::ResumeOutcome::Provisioned)) {
            // The creation had gone through at that relay: the device belongs to it.
            return Err(CoreError::input("device already provisioned"));
        }
        self.point_transport_at(here.as_deref())?;
        self.store.tx(|db| db.secret_delete(FOUNDER_PENDING))
    }

    pub(super) fn load_founder(&self) -> Result<Stored<FounderMaterial>> {
        let raw = match self.store.read(|db| db.secret_get(FOUNDER_PENDING)) {
            Ok(Some(raw)) => raw,
            Ok(None) => return Ok(Stored::Absent),
            Err(CoreError::DecryptionFailed) => return Ok(Stored::Unreadable),
            Err(e) => return Err(e),
        };
        Ok(decode(&raw).map_or(Stored::Unreadable, Stored::Found))
    }

    /// The keys and space the founder is about to create: the ones already sealed (an earlier
    /// call whose outcome is unknown), or new ones, sealed now, before any request.
    fn founder_material(&self, device_name: &str) -> Result<FounderMaterial> {
        let mut material = match self.load_founder()? {
            Stored::Found(m) => m,
            Stored::Absent | Stored::Unreadable => FounderMaterial {
                keys: DeviceKeys::generate()?,
                space_id: new_space_id()?,
                space_key: SpaceKey::generate(1)?,
                device_name: device_name.to_owned(),
            },
        };
        material.device_name = device_name.to_owned();
        let bytes = encode(&material)?;
        self.store.tx(|db| {
            db.secret_put(FOUNDER_PENDING, &bytes)?;
            // The relay these keys belong to, so that a stop does not lose it.
            self.stage_pending_relay(db)
        })?;
        Ok(material)
    }

    /// `201` (or a founder recognised by the relay): provisions the device and erases the
    /// pending keys in the same transaction.
    fn complete_founder(&self, m: FounderMaterial) -> Result<()> {
        let trust = TrustStore::new(&m.space_id, &m.keys.public(), 1)?;
        self.provision_with(
            Provision {
                keys: m.keys,
                trust,
                space_key: m.space_key,
                device_name: m.device_name,
            },
            ProvisionExtras {
                drop_secrets: vec![FOUNDER_PENDING, JOINER_PENDING],
                ..ProvisionExtras::default()
            },
        )?;
        self.first_contact();
        Ok(())
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn the_founder_material_round_trips_and_refuses_damage() {
        let m = FounderMaterial {
            keys: DeviceKeys::generate().unwrap(),
            space_id: new_space_id().unwrap(),
            space_key: SpaceKey::generate(1).unwrap(),
            device_name: "PC-Bureau".into(),
        };
        let bytes = encode(&m).unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.keys.device_id(), m.keys.device_id());
        assert_eq!(back.space_id, m.space_id);
        assert_eq!(back.space_key.to_bytes(), m.space_key.to_bytes());
        assert_eq!(back.device_name, "PC-Bureau");
        for cut in 0..bytes.len() {
            assert!(decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert!(decode(&longer).is_err());
    }
}
