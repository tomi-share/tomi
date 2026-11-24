//! WebAuthn verification for the web administration panel (`admin_web.rs`), by hand, in pure Rust.
//!
//! `webauthn-rs` is not used: it needs OpenSSL, which the static musl binary of the relay does not
//! have. The scope is deliberately narrow, and everything outside it is refused:
//!
//! * algorithm **ES256** only (COSE `alg` -7, curve P-256), verified with `p256`;
//! * attestation **`none`** only: the relay does not check any attestation chain, so it does not
//!   learn what authenticator made the key (it does not need to: one administrator, one key);
//! * **user verification required** (flag UV) at registration and at sign-in, as well as user
//!   presence (UP);
//! * one credential; nothing here stores anything: the functions are pure, the caller decides what
//!   a challenge is, whether it is still valid and what is kept.
//!
//! The CBOR decoder below reads only what a `none` attestation and an ES256 COSE key need:
//! definite lengths, no tag, no float. It rejects what it does not understand.

use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::wire::{b64_decode, sha256};

/// COSE algorithm identifier of ES256.
pub const COSE_ALG_ES256: i128 = -7;

const FLAG_UP: u8 = 0x01;
const FLAG_UV: u8 = 0x04;
const FLAG_BE: u8 = 0x08;
const FLAG_BS: u8 = 0x10;
const FLAG_AT: u8 = 0x40;
const FLAG_ED: u8 = 0x80;

/// `rpIdHash` (32) + flags (1) + signCount (4).
const AUTH_DATA_HEADER: usize = 37;

/// Why a registration or an assertion was refused. Only for the logs and the tests: the client is
/// never told (a generic failure leaks nothing about the cause).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// `clientDataJSON` is not a JSON object with the fields we read.
    ClientData,
    /// `type` is not the expected `webauthn.create` / `webauthn.get`.
    WrongType,
    /// The challenge is not the one that was issued.
    Challenge,
    /// The origin is not the origin of `public_url`.
    Origin,
    /// The ceremony ran in a cross-origin frame.
    CrossOrigin,
    /// `rpIdHash` is not the SHA-256 of the relying party id.
    RpIdHash,
    /// Flag UP (user present) is not set.
    UserPresence,
    /// Flag UV (user verified) is not set.
    UserVerification,
    /// The authenticator data is malformed.
    AuthData,
    /// The attestation object is malformed or its format is not `none`.
    Attestation,
    /// The COSE algorithm is not ES256.
    Algorithm,
    /// The COSE key is not an EC2 P-256 key, or its point is not on the curve.
    PublicKey,
    /// The signature does not verify.
    Signature,
    /// The signature counter did not increase.
    SignCount,
}

impl Reject {
    /// A fixed word for the logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Reject::ClientData => "client_data",
            Reject::WrongType => "wrong_type",
            Reject::Challenge => "challenge",
            Reject::Origin => "origin",
            Reject::CrossOrigin => "cross_origin",
            Reject::RpIdHash => "rp_id_hash",
            Reject::UserPresence => "user_presence",
            Reject::UserVerification => "user_verification",
            Reject::AuthData => "auth_data",
            Reject::Attestation => "attestation",
            Reject::Algorithm => "algorithm",
            Reject::PublicKey => "public_key",
            Reject::Signature => "signature",
            Reject::SignCount => "sign_count",
        }
    }
}

// ===== CBOR ===================================================================================

/// A decoded CBOR item (the subset of RFC 8949 that WebAuthn needs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cbor<'a> {
    Int(i128),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Cbor<'a>>),
    Map(Vec<(Cbor<'a>, Cbor<'a>)>),
    Bool(bool),
    Null,
}

/// Deepest nesting accepted. An attestation object is 3 levels deep.
const CBOR_MAX_DEPTH: usize = 8;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    /// The argument of an initial byte; `None` for the indefinite length and the reserved values.
    fn argument(&mut self, info: u8) -> Option<u64> {
        match info {
            0..=23 => Some(u64::from(info)),
            24 => self.byte().map(u64::from),
            25 => {
                let b = <[u8; 2]>::try_from(self.take(2)?).ok()?;
                Some(u64::from(u16::from_be_bytes(b)))
            }
            26 => {
                let b = <[u8; 4]>::try_from(self.take(4)?).ok()?;
                Some(u64::from(u32::from_be_bytes(b)))
            }
            27 => {
                let b = <[u8; 8]>::try_from(self.take(8)?).ok()?;
                Some(u64::from_be_bytes(b))
            }
            _ => None,
        }
    }

    fn item(&mut self, depth: usize) -> Option<Cbor<'a>> {
        if depth > CBOR_MAX_DEPTH {
            return None;
        }
        let head = self.byte()?;
        let (major, info) = (head >> 5, head & 0x1f);
        if major == 7 {
            return match info {
                20 => Some(Cbor::Bool(false)),
                21 => Some(Cbor::Bool(true)),
                22 => Some(Cbor::Null),
                _ => None,
            };
        }
        let arg = self.argument(info)?;
        match major {
            0 => Some(Cbor::Int(i128::from(arg))),
            1 => Some(Cbor::Int(-1 - i128::from(arg))),
            2 => {
                let n = usize::try_from(arg).ok()?;
                self.take(n).map(Cbor::Bytes)
            }
            3 => {
                let n = usize::try_from(arg).ok()?;
                std::str::from_utf8(self.take(n)?).ok().map(Cbor::Text)
            }
            4 => {
                let n = usize::try_from(arg).ok()?;
                // An item takes at least one byte: a count above what is left is a lie.
                if n > self.remaining() {
                    return None;
                }
                let mut items = Vec::with_capacity(n.min(16));
                for _ in 0..n {
                    items.push(self.item(depth + 1)?);
                }
                Some(Cbor::Array(items))
            }
            5 => {
                let n = usize::try_from(arg).ok()?;
                if n > self.remaining() / 2 {
                    return None;
                }
                let mut entries = Vec::with_capacity(n.min(16));
                for _ in 0..n {
                    let k = self.item(depth + 1)?;
                    let v = self.item(depth + 1)?;
                    entries.push((k, v));
                }
                Some(Cbor::Map(entries))
            }
            // 6: tags, not needed.
            _ => None,
        }
    }
}

/// Decodes the first item of `data` and says how many bytes it took.
pub fn cbor_decode_prefix(data: &[u8]) -> Option<(Cbor<'_>, usize)> {
    let mut r = Reader { data, pos: 0 };
    let item = r.item(0)?;
    Some((item, r.pos))
}

/// Decodes `data`, which must be exactly one item.
pub fn cbor_decode_exact(data: &[u8]) -> Option<Cbor<'_>> {
    let (item, used) = cbor_decode_prefix(data)?;
    (used == data.len()).then_some(item)
}

// ===== shared checks ==========================================================================

/// What a ceremony must match. The caller took `challenge` out of its store of pending
/// challenges (single use, 5 minutes): this module only compares.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    /// The origin of `public_url`, exactly as a browser writes it (`https://host[:port]`).
    pub origin: &'a str,
    /// The relying party id: the host of `public_url`.
    pub rp_id: &'a str,
    /// The 32 bytes issued for this ceremony.
    pub challenge: &'a [u8],
}

/// Equality of two byte strings in constant time (the length is not a secret).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// The challenge a `clientDataJSON` claims to answer, decoded. The caller uses it to find the
/// pending challenge it designates; [`verify_registration`] and [`verify_assertion`] then check it
/// again against the expected one.
pub fn claimed_challenge(client_data_json: &[u8]) -> Option<Vec<u8>> {
    let v: Value = serde_json::from_slice(client_data_json).ok()?;
    b64_decode(v.as_object()?.get("challenge")?.as_str()?)
}

/// Checks `clientDataJSON`: ceremony type, challenge, origin, same-origin.
fn check_client_data(exp: &Expected, raw: &[u8], ceremony: &str) -> Result<(), Reject> {
    let v: Value = serde_json::from_slice(raw).map_err(|_| Reject::ClientData)?;
    let obj = v.as_object().ok_or(Reject::ClientData)?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or(Reject::ClientData)?;
    if kind != ceremony {
        return Err(Reject::WrongType);
    }
    let challenge = obj
        .get("challenge")
        .and_then(Value::as_str)
        .ok_or(Reject::ClientData)?;
    let origin = obj
        .get("origin")
        .and_then(Value::as_str)
        .ok_or(Reject::ClientData)?;
    // Both are evaluated before the answer: the time taken does not tell which one was wrong.
    let challenge_ok = b64_decode(challenge).is_some_and(|c| same(&c, exp.challenge));
    let origin_ok = origin == exp.origin;
    if !challenge_ok {
        return Err(Reject::Challenge);
    }
    if !origin_ok {
        return Err(Reject::Origin);
    }
    if obj.get("crossOrigin").and_then(Value::as_bool) == Some(true)
        || obj.contains_key("topOrigin")
    {
        return Err(Reject::CrossOrigin);
    }
    Ok(())
}

struct AuthData<'a> {
    rp_id_hash: &'a [u8],
    flags: u8,
    sign_count: u32,
    /// Everything after the 37-byte header.
    rest: &'a [u8],
}

fn split_auth_data(data: &[u8]) -> Result<AuthData<'_>, Reject> {
    if data.len() < AUTH_DATA_HEADER {
        return Err(Reject::AuthData);
    }
    let count = <[u8; 4]>::try_from(&data[33..37]).map_err(|_| Reject::AuthData)?;
    Ok(AuthData {
        rp_id_hash: &data[..32],
        flags: data[32],
        sign_count: u32::from_be_bytes(count),
        rest: &data[AUTH_DATA_HEADER..],
    })
}

/// `rpIdHash`, user presence, user verification. A "backup state" without "backup eligible" is
/// not a state an authenticator can be in.
fn check_auth_data(exp: &Expected, auth: &AuthData) -> Result<(), Reject> {
    if !same(auth.rp_id_hash, &sha256(exp.rp_id.as_bytes())) {
        return Err(Reject::RpIdHash);
    }
    if auth.flags & FLAG_UP == 0 {
        return Err(Reject::UserPresence);
    }
    if auth.flags & FLAG_UV == 0 {
        return Err(Reject::UserVerification);
    }
    if auth.flags & FLAG_BS != 0 && auth.flags & FLAG_BE == 0 {
        return Err(Reject::AuthData);
    }
    Ok(())
}

// ===== registration ===========================================================================

/// A credential accepted at registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub credential_id: Vec<u8>,
    /// The public key as an uncompressed SEC1 point (`0x04 || x || y`, 65 bytes).
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    /// The BE flag (backup eligible): `false` for a key bound to its device. Declared by the
    /// authenticator, not attested.
    pub backup_eligible: bool,
}

/// Looks up the integer-keyed entries of a COSE key; `None` if a key is not an integer or is
/// repeated.
fn cose_entries<'a, 'b>(entries: &'b [(Cbor<'a>, Cbor<'a>)]) -> Option<Vec<(i128, &'b Cbor<'a>)>> {
    let mut out: Vec<(i128, &Cbor)> = Vec::with_capacity(entries.len());
    for (k, v) in entries {
        let Cbor::Int(label) = k else { return None };
        if out.iter().any(|(l, _)| l == label) {
            return None;
        }
        out.push((*label, v));
    }
    Some(out)
}

/// The EC2 P-256 ES256 key a COSE_Key map holds, as an uncompressed SEC1 point. Extra labels
/// are ignored; the five that matter must be present and right.
fn cose_to_sec1(key: &Cbor) -> Result<Vec<u8>, Reject> {
    let Cbor::Map(entries) = key else {
        return Err(Reject::PublicKey);
    };
    let labels = cose_entries(entries).ok_or(Reject::PublicKey)?;
    let get = |label: i128| labels.iter().find(|(l, _)| *l == label).map(|(_, v)| *v);
    // `alg` first: a key made for another algorithm is "wrong algorithm", not "wrong key".
    match get(3) {
        Some(Cbor::Int(a)) if *a == COSE_ALG_ES256 => {}
        Some(Cbor::Int(_)) => return Err(Reject::Algorithm),
        _ => return Err(Reject::PublicKey),
    }
    // kty 2 = EC2, crv 1 = P-256.
    if !matches!(get(1), Some(Cbor::Int(2))) || !matches!(get(-1), Some(Cbor::Int(1))) {
        return Err(Reject::PublicKey);
    }
    let (Some(Cbor::Bytes(x)), Some(Cbor::Bytes(y))) = (get(-2), get(-3)) else {
        return Err(Reject::PublicKey);
    };
    if x.len() != 32 || y.len() != 32 {
        return Err(Reject::PublicKey);
    }
    let mut point = Vec::with_capacity(65);
    point.push(0x04);
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    // Refuses a point that is not on the curve.
    VerifyingKey::from_sec1_bytes(&point).map_err(|_| Reject::PublicKey)?;
    Ok(point)
}

/// Verifies the answer of `navigator.credentials.create`: `clientDataJSON` and
/// `attestationObject` (both raw bytes).
pub fn verify_registration(
    exp: &Expected,
    client_data_json: &[u8],
    attestation_object: &[u8],
) -> Result<Registered, Reject> {
    check_client_data(exp, client_data_json, "webauthn.create")?;

    // The attestation object: exactly `fmt`, `attStmt` and `authData`; format `none`, so an
    // empty statement.
    let Some(Cbor::Map(entries)) = cbor_decode_exact(attestation_object) else {
        return Err(Reject::Attestation);
    };
    if entries.len() != 3 {
        return Err(Reject::Attestation);
    }
    let (mut fmt, mut stmt, mut auth_data) = (None, None, None);
    for (k, v) in &entries {
        let Cbor::Text(name) = k else {
            return Err(Reject::Attestation);
        };
        let slot = match *name {
            "fmt" => &mut fmt,
            "attStmt" => &mut stmt,
            "authData" => &mut auth_data,
            _ => return Err(Reject::Attestation),
        };
        if slot.replace(v).is_some() {
            return Err(Reject::Attestation);
        }
    }
    if !matches!(fmt, Some(Cbor::Text("none"))) {
        return Err(Reject::Attestation);
    }
    if !matches!(stmt, Some(Cbor::Map(m)) if m.is_empty()) {
        return Err(Reject::Attestation);
    }
    let Some(Cbor::Bytes(auth_data)) = auth_data else {
        return Err(Reject::Attestation);
    };

    let auth = split_auth_data(auth_data)?;
    check_auth_data(exp, &auth)?;
    if auth.flags & FLAG_AT == 0 || auth.flags & FLAG_ED != 0 {
        // No attested credential data, or extension data we did not ask for.
        return Err(Reject::AuthData);
    }

    // Attested credential data: AAGUID (16), credential id length (2), id, COSE key.
    let rest = auth.rest;
    if rest.len() < 18 {
        return Err(Reject::AuthData);
    }
    let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
    if !(16..=1023).contains(&id_len) {
        return Err(Reject::AuthData);
    }
    let id_end = 18 + id_len;
    let Some(credential_id) = rest.get(18..id_end) else {
        return Err(Reject::AuthData);
    };
    let key_bytes = &rest[id_end..];
    let Some((key, used)) = cbor_decode_prefix(key_bytes) else {
        return Err(Reject::PublicKey);
    };
    if used != key_bytes.len() {
        return Err(Reject::AuthData);
    }
    let public_key = cose_to_sec1(&key)?;
    Ok(Registered {
        credential_id: credential_id.to_vec(),
        public_key,
        sign_count: auth.sign_count,
        backup_eligible: auth.flags & FLAG_BE != 0,
    })
}

// ===== assertion ==============================================================================

/// Verifies the answer of `navigator.credentials.get` against the stored public key and signature
/// counter. Returns the new counter to store.
///
/// The counter rule: if either the stored or the new value is not zero, the new value must be
/// strictly greater. Two zeros are accepted (authenticators that keep no counter).
pub fn verify_assertion(
    exp: &Expected,
    public_key: &[u8],
    stored_count: u32,
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature_der: &[u8],
) -> Result<u32, Reject> {
    check_client_data(exp, client_data_json, "webauthn.get")?;
    let auth = split_auth_data(authenticator_data)?;
    check_auth_data(exp, &auth)?;

    let key = VerifyingKey::from_sec1_bytes(public_key).map_err(|_| Reject::PublicKey)?;
    let sig = Signature::from_der(signature_der).map_err(|_| Reject::Signature)?;
    let mut signed = Vec::with_capacity(authenticator_data.len() + 32);
    signed.extend_from_slice(authenticator_data);
    signed.extend_from_slice(&sha256(client_data_json));
    key.verify(&signed, &sig).map_err(|_| Reject::Signature)?;

    if (auth.sign_count != 0 || stored_count != 0) && auth.sign_count <= stored_count {
        return Err(Reject::SignCount);
    }
    Ok(auth.sign_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbor_reads_the_subset() {
        // {1: 2, -1: h'AB', "a": [true, null]}
        let data = [
            0xa3, 0x01, 0x02, 0x20, 0x41, 0xab, 0x61, b'a', 0x82, 0xf5, 0xf6,
        ];
        let Some(Cbor::Map(m)) = cbor_decode_exact(&data) else {
            panic!("not a map")
        };
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], (Cbor::Int(1), Cbor::Int(2)));
        assert_eq!(m[1], (Cbor::Int(-1), Cbor::Bytes(&[0xab])));
        assert_eq!(
            m[2],
            (
                Cbor::Text("a"),
                Cbor::Array(vec![Cbor::Bool(true), Cbor::Null])
            )
        );
        // Negative integers: -7 is 0x26, -1 - 255 = -256 is 0x38 0xff.
        assert_eq!(cbor_decode_exact(&[0x26]), Some(Cbor::Int(-7)));
        assert_eq!(cbor_decode_exact(&[0x38, 0xff]), Some(Cbor::Int(-256)));
    }

    #[test]
    fn cbor_refuses_what_it_does_not_understand() {
        // Trailing bytes.
        assert!(cbor_decode_exact(&[0x01, 0x02]).is_none());
        assert_eq!(cbor_decode_prefix(&[0x01, 0x02]), Some((Cbor::Int(1), 1)));
        // Indefinite length (byte string, array, map), break, reserved additional information.
        assert!(cbor_decode_exact(&[0x5f, 0x41, 0x00, 0xff]).is_none());
        assert!(cbor_decode_exact(&[0x9f, 0xff]).is_none());
        assert!(cbor_decode_exact(&[0xbf, 0xff]).is_none());
        assert!(cbor_decode_exact(&[0x1c]).is_none());
        // Tags, floats, undefined, other simple values.
        assert!(cbor_decode_exact(&[0xc1, 0x00]).is_none());
        assert!(cbor_decode_exact(&[0xfb, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(cbor_decode_exact(&[0xf7]).is_none());
        assert!(cbor_decode_exact(&[0xf0]).is_none());
        // Truncated, and lengths that promise more than there is (no allocation by length).
        assert!(cbor_decode_exact(&[0x42, 0x00]).is_none());
        assert!(
            cbor_decode_exact(&[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_none()
        );
        assert!(
            cbor_decode_exact(&[0x5b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).is_none()
        );
        assert!(cbor_decode_exact(&[0xa1, 0x01]).is_none());
        assert!(cbor_decode_exact(&[]).is_none());
        // Text that is not UTF-8.
        assert!(cbor_decode_exact(&[0x61, 0xff]).is_none());
        // Nesting too deep.
        let deep = vec![0x81u8; CBOR_MAX_DEPTH + 3];
        assert!(cbor_decode_exact(&deep).is_none());
        let mut ok = vec![0x81u8; CBOR_MAX_DEPTH];
        ok.push(0x00);
        assert!(cbor_decode_exact(&ok).is_some());
    }

    #[test]
    fn same_is_an_equality_of_bytes() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
