//! A virtual WebAuthn authenticator for the tests of the web administration panel: a real P-256
//! key that builds `clientDataJSON`, `attestationObject` (format `none`) and assertions the way
//! a browser hands them over, and can be told to get one thing wrong at a time (`Tweak`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{json, Value};
use tomi_relay::wire::{b64_encode, random_bytes, sha256};

pub const FLAG_UP: u8 = 0x01;
pub const FLAG_UV: u8 = 0x04;
pub const FLAG_AT: u8 = 0x40;

// ----- a minimal CBOR encoder -----------------------------------------------------------

fn head(major: u8, n: u64) -> Vec<u8> {
    let m = major << 5;
    if n < 24 {
        vec![m | n as u8]
    } else if n < 256 {
        vec![m | 24, n as u8]
    } else if n < 65_536 {
        let mut v = vec![m | 25];
        v.extend_from_slice(&(n as u16).to_be_bytes());
        v
    } else {
        let mut v = vec![m | 26];
        v.extend_from_slice(&(n as u32).to_be_bytes());
        v
    }
}

pub fn cbor_int(v: i64) -> Vec<u8> {
    if v >= 0 {
        head(0, v as u64)
    } else {
        head(1, (-1 - v) as u64)
    }
}

pub fn cbor_bytes(b: &[u8]) -> Vec<u8> {
    let mut v = head(2, b.len() as u64);
    v.extend_from_slice(b);
    v
}

pub fn cbor_text(s: &str) -> Vec<u8> {
    let mut v = head(3, s.len() as u64);
    v.extend_from_slice(s.as_bytes());
    v
}

pub fn cbor_map(entries: Vec<(Vec<u8>, Vec<u8>)>) -> Vec<u8> {
    let mut v = head(5, entries.len() as u64);
    for (k, val) in entries {
        v.extend(k);
        v.extend(val);
    }
    v
}

// ----- the authenticator ------------------------------------------------------------------

/// What to get wrong, one thing at a time. The default is a perfectly valid answer.
#[derive(Clone, Default)]
pub struct Tweak {
    /// Origin written in `clientDataJSON`.
    pub origin: Option<String>,
    /// Relying party id whose hash goes in the authenticator data.
    pub rp_id: Option<String>,
    /// `type` written in `clientDataJSON`.
    pub kind: Option<&'static str>,
    /// Challenge written in `clientDataJSON` (instead of the one given).
    pub challenge: Option<Vec<u8>>,
    pub flags: Option<u8>,
    pub count: Option<u32>,
    pub alg: Option<i64>,
    pub kty: Option<i64>,
    pub crv: Option<i64>,
    pub off_curve: bool,
    pub fmt: Option<&'static str>,
    pub att_stmt_entry: bool,
    pub cross_origin: bool,
    pub short_id: bool,
    pub trailing: bool,
    pub bad_signature: bool,
    pub garbage_signature: bool,
    pub sign_other_client_data: bool,
    pub sign_with: Option<SigningKey>,
}

pub struct Registration {
    pub client_data_json: Vec<u8>,
    pub attestation_object: Vec<u8>,
}

impl Registration {
    pub fn json(&self) -> Value {
        json!({
            "clientDataJSON": b64_encode(&self.client_data_json),
            "attestationObject": b64_encode(&self.attestation_object),
        })
    }
}

pub struct Assertion {
    pub id: Vec<u8>,
    pub client_data_json: Vec<u8>,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Assertion {
    pub fn json(&self) -> Value {
        json!({
            "id": b64_encode(&self.id),
            "clientDataJSON": b64_encode(&self.client_data_json),
            "authenticatorData": b64_encode(&self.authenticator_data),
            "signature": b64_encode(&self.signature),
        })
    }
}

pub struct Authenticator {
    pub key: SigningKey,
    pub credential_id: Vec<u8>,
    pub rp_id: String,
    pub origin: String,
    /// The counter of the next assertion is this plus one.
    pub sign_count: u32,
}

impl Authenticator {
    pub fn new(rp_id: &str, origin: &str) -> Self {
        Self {
            key: new_key(),
            credential_id: random_bytes::<32>().unwrap().to_vec(),
            rp_id: rp_id.to_owned(),
            origin: origin.to_owned(),
            sign_count: 0,
        }
    }

    /// The authenticator of the test relay (`https://relay.test`).
    pub fn for_test_relay() -> Self {
        Self::new("relay.test", "https://relay.test")
    }

    fn client_data(&self, ceremony: &str, challenge: &[u8], t: &Tweak) -> Vec<u8> {
        let mut v = json!({
            "type": t.kind.unwrap_or(match ceremony {
                "create" => "webauthn.create",
                _ => "webauthn.get",
            }),
            "challenge": b64_encode(t.challenge.as_deref().unwrap_or(challenge)),
            "origin": t.origin.clone().unwrap_or_else(|| self.origin.clone()),
            "crossOrigin": t.cross_origin,
        });
        if t.cross_origin {
            v["topOrigin"] = json!("https://evil.test");
        }
        serde_json::to_vec(&v).unwrap()
    }

    fn rp_hash(&self, t: &Tweak) -> [u8; 32] {
        sha256(t.rp_id.as_deref().unwrap_or(&self.rp_id).as_bytes())
    }

    /// The COSE_Key of the public key (ES256 unless told otherwise).
    pub fn cose_key(&self, t: &Tweak) -> Vec<u8> {
        let point = self.key.verifying_key().to_sec1_point(false);
        let (x, mut y) = (point.x().unwrap().to_vec(), point.y().unwrap().to_vec());
        if t.off_curve {
            y[31] ^= 0x01;
        }
        cbor_map(vec![
            (cbor_int(1), cbor_int(t.kty.unwrap_or(2))),
            (cbor_int(3), cbor_int(t.alg.unwrap_or(-7))),
            (cbor_int(-1), cbor_int(t.crv.unwrap_or(1))),
            (cbor_int(-2), cbor_bytes(&x)),
            (cbor_int(-3), cbor_bytes(&y)),
        ])
    }

    /// The answer of `navigator.credentials.create`.
    pub fn create(&self, challenge: &[u8], t: &Tweak) -> Registration {
        let mut auth = Vec::new();
        auth.extend_from_slice(&self.rp_hash(t));
        auth.push(t.flags.unwrap_or(FLAG_UP | FLAG_UV | FLAG_AT));
        auth.extend_from_slice(&t.count.unwrap_or(0).to_be_bytes());
        auth.extend_from_slice(&[0u8; 16]);
        let id: &[u8] = if t.short_id {
            &self.credential_id[..8]
        } else {
            &self.credential_id
        };
        auth.extend_from_slice(&(id.len() as u16).to_be_bytes());
        auth.extend_from_slice(id);
        auth.extend(self.cose_key(t));
        if t.trailing {
            auth.push(0x00);
        }
        let stmt = if t.att_stmt_entry {
            cbor_map(vec![(cbor_text("x"), cbor_int(1))])
        } else {
            cbor_map(vec![])
        };
        let attestation_object = cbor_map(vec![
            (cbor_text("fmt"), cbor_text(t.fmt.unwrap_or("none"))),
            (cbor_text("attStmt"), stmt),
            (cbor_text("authData"), cbor_bytes(&auth)),
        ]);
        Registration {
            client_data_json: self.client_data("create", challenge, t),
            attestation_object,
        }
    }

    /// The answer of `navigator.credentials.get`. The counter goes up by one unless told.
    pub fn get(&mut self, challenge: &[u8], t: &Tweak) -> Assertion {
        self.sign_count += 1;
        let mut auth = Vec::new();
        auth.extend_from_slice(&self.rp_hash(t));
        auth.push(t.flags.unwrap_or(FLAG_UP | FLAG_UV));
        auth.extend_from_slice(&t.count.unwrap_or(self.sign_count).to_be_bytes());
        let client_data_json = self.client_data("get", challenge, t);
        let signed_client_data = if t.sign_other_client_data {
            self.client_data("get", &[9u8; 32], &Tweak::default())
        } else {
            client_data_json.clone()
        };
        let mut message = auth.clone();
        message.extend_from_slice(&sha256(&signed_client_data));
        let key = t.sign_with.as_ref().unwrap_or(&self.key);
        let sig: Signature = key.sign(&message);
        let mut signature = sig.to_der().as_bytes().to_vec();
        if t.bad_signature {
            let n = signature.len();
            signature[n - 1] ^= 0x01;
        }
        if t.garbage_signature {
            signature = vec![0x30, 0x03, 0x01, 0x02, 0x03];
        }
        Assertion {
            id: self.credential_id.clone(),
            client_data_json,
            authenticator_data: auth,
            signature,
        }
    }
}

pub fn new_key() -> SigningKey {
    loop {
        if let Ok(k) = SigningKey::from_slice(&random_bytes::<32>().unwrap()) {
            return k;
        }
    }
}
