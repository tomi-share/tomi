//! CPace on ristretto255 with SHA-512 (`CPACE-RISTR255-SHA512`), initiator-responder mode.
//!
//! draft-irtf-cfrg-cpace-21 §6-§8.3, instantiated exactly as in `spec/pairing-v1.md` §3.3.
//! **No primitive is implemented here.** The ristretto255 group (canonical decode / encode,
//! the RFC 9496 element derivation, scalar multiplication) comes from `curve25519-dalek`,
//! SHA-512 from `sha2`. This file only composes them, and `tests` checks the composition
//! against the vectors of the draft (appendix B.3, A.1).
//!
//! `K` never leaves this layer: [`Share::finish`] returns the session key `ISK` only, and `K`
//! is erased as soon as `ISK` is computed. (`shared_point` is `pub(super)` for the
//! known-answer tests of `pairing::kat`, which are not part of the application surface.)

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};
use zeroize::{Zeroize, Zeroizing};

use super::entropy::Entropy;
use crate::error::{CoreError, Result};

/// `G.DSI` of the ristretto255 group environment.
pub(super) const DSI: &[u8] = b"CPaceRistretto255";
/// `G.DSI || "_ISK"`: domain separator of the session key hash.
pub(super) const DSI_ISK: &[u8] = b"CPaceRistretto255_ISK";
/// `H.s_in_bytes` of SHA-512: the input block size that the generator string fills.
pub(super) const S_IN_BYTES: usize = 128;
/// Length of an encoded ristretto255 element.
pub(super) const POINT_LEN: usize = 32;
/// Length of `ISK` (one SHA-512 output).
pub(super) const ISK_LEN: usize = 64;
/// Public encoding `G.I` of the neutral element.
pub(super) const IDENTITY: [u8; POINT_LEN] = [0u8; POINT_LEN];

/// `prepend_len(data)`: the length of `data` in LEB128, then `data` (draft A.1.1).
pub(super) fn prepend_len(out: &mut Vec<u8>, data: &[u8]) {
    let mut len = data.len();
    loop {
        // `len & 0x7f` is below 128: the cast never truncates.
        let low = (len & 0x7f) as u8;
        len >>= 7;
        if len == 0 {
            out.push(low);
            break;
        }
        out.push(low | 0x80);
    }
    out.extend_from_slice(data);
}

/// `lv_cat(a0, a1, ...) = prepend_len(a0) || prepend_len(a1) || ...` (draft A.1.3).
pub(super) fn lv_cat(parts: &[&[u8]]) -> Vec<u8> {
    // Room for the data and a LEB128 length of up to 5 bytes each: no reallocation, so no stale
    // copy of a secret input (the PRS) is left in freed memory.
    let mut out = Vec::with_capacity(parts.iter().map(|p| p.len() + 5).sum());
    for p in parts {
        prepend_len(&mut out, p);
    }
    out
}

/// `generator_string(DSI, PRS, CI, sid, s_in_bytes)` of draft A.2:
/// `lv_cat(DSI, PRS, zero_bytes(len_zpad), CI, sid)`.
pub(super) fn generator_string(prs: &[u8], ci: &[u8], sid: &[u8]) -> Vec<u8> {
    // Only its length is used, but the buffer holds a copy of the PRS: erased on drop.
    let mut prs_lv = Zeroizing::new(Vec::new());
    prepend_len(&mut prs_lv, prs);
    let mut dsi_lv = Vec::new();
    prepend_len(&mut dsi_lv, DSI);
    // len_zpad = max(0, s_in_bytes - 1 - len(prepend_len(PRS)) - len(prepend_len(DSI)))
    let zpad = S_IN_BYTES.saturating_sub(1 + prs_lv.len() + dsi_lv.len());
    lv_cat(&[DSI, prs, &vec![0u8; zpad], ci, sid])
}

/// `G.calculate_generator`: `g = element_derivation(SHA-512(gen_str))`.
pub(super) fn generator(prs: &[u8], ci: &[u8], sid: &[u8]) -> RistrettoPoint {
    let gen_str = Zeroizing::new(generator_string(prs, ci, sid));
    // H.hash(gen_str, 2 * field_size_bytes) is the whole 64-byte SHA-512 output.
    let hash: Zeroizing<[u8; 64]> = Zeroizing::new(Sha512::digest(gen_str.as_slice()).into());
    RistrettoPoint::from_uniform_bytes(&hash)
}

/// `G.scalar_mult_vfy(y, X)`: the neutral element if `decode(X)` fails, else `encode(y * decode(X))`.
pub(super) fn scalar_mult_vfy(y: &Scalar, x: &[u8; POINT_LEN]) -> [u8; POINT_LEN] {
    match CompressedRistretto(*x).decompress() {
        Some(p) => (y * p).compress().to_bytes(),
        None => IDENTITY,
    }
}

/// The role of a party in the initiator-responder transcript.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Role {
    /// A, the inviter: `Ya`, `ADa`.
    Initiator,
    /// N, the new device: `Yb`, `ADb`.
    Responder,
}

/// One party's secret scalar and public share.
pub(super) struct Share {
    y: Scalar,
    public: [u8; POINT_LEN],
}

impl Share {
    /// Samples a share with `G.sample_scalar()` (draft §8.3): 32 random bytes, the bits above
    /// the 252nd cleared (`b[31] &= 0x0f`), read as a little-endian integer (< 2^252 < the group
    /// order, so no reduction takes place).
    pub(super) fn sample(g: &RistrettoPoint, entropy: &mut Entropy) -> Result<Self> {
        let mut b = Zeroizing::new([0u8; 32]);
        entropy.fill(b.as_mut())?;
        b[31] &= 0x0f;
        Self::from_scalar_bytes(g, &b)
    }

    /// Share for explicit scalar bytes (little endian, already below 2^252).
    pub(super) fn from_scalar_bytes(g: &RistrettoPoint, bytes: &[u8; 32]) -> Result<Self> {
        let y = Scalar::from_bytes_mod_order(*bytes);
        if y == Scalar::ZERO {
            // 2^-252 with a working generator: report a broken RNG instead of sending `I`.
            return Err(CoreError::Randomness);
        }
        let public = (y * g).compress().to_bytes();
        Ok(Self { y, public })
    }

    /// `Ya` or `Yb`: the encoded public share.
    pub(super) fn public(&self) -> &[u8; POINT_LEN] {
        &self.public
    }

    /// `K = scalar_mult_vfy(y, peer)`; `None` if it is the neutral element (the protocol aborts).
    pub(super) fn shared_point(
        &self,
        peer: &[u8; POINT_LEN],
    ) -> Option<Zeroizing<[u8; POINT_LEN]>> {
        let k = Zeroizing::new(scalar_mult_vfy(&self.y, peer));
        if *k == IDENTITY {
            None
        } else {
            Some(k)
        }
    }

    /// Completes the run: aborts if `K` is the neutral element, else returns
    /// `ISK = SHA-512(lv_cat(DSI_ISK, sid, K) || lv_cat(Ya, ADa) || lv_cat(Yb, ADb))`.
    pub(super) fn finish(
        &self,
        role: Role,
        peer: &[u8; POINT_LEN],
        sid: &[u8],
        ada: &[u8],
        adb: &[u8],
    ) -> Result<Zeroizing<[u8; ISK_LEN]>> {
        let k = self.shared_point(peer).ok_or(CoreError::PairingFailed)?;
        let (ya, yb) = match role {
            Role::Initiator => (&self.public, peer),
            Role::Responder => (peer, &self.public),
        };
        Ok(isk(sid, &k, ya, ada, yb, adb))
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        self.y.zeroize();
    }
}

impl std::fmt::Debug for Share {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Share(<redacted>)")
    }
}

/// `ISK` for the initiator-responder transcript (draft §7.2 and A.3.4, `transcript_ir`).
pub(super) fn isk(
    sid: &[u8],
    k: &[u8; POINT_LEN],
    ya: &[u8; POINT_LEN],
    ada: &[u8],
    yb: &[u8; POINT_LEN],
    adb: &[u8],
) -> Zeroizing<[u8; ISK_LEN]> {
    // The buffer holds K: erased on drop, and sized up front so that it never moves.
    let parts: [&[u8]; 7] = [DSI_ISK, sid, k, ya, ada, yb, adb];
    let mut m = Zeroizing::new(Vec::with_capacity(parts.iter().map(|p| p.len() + 5).sum()));
    for p in parts {
        prepend_len(&mut m, p);
    }
    Zeroizing::new(Sha512::digest(m.as_slice()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    fn a32(s: &str) -> [u8; 32] {
        h(s).try_into().unwrap()
    }

    // draft-irtf-cfrg-cpace-21, appendix B.3 (ristretto255, SHA-512).
    const PRS: &[u8] = b"Password";
    const CI: &str = "0b415f696e69746961746f720b425f726573706f6e646572";
    const SID: &str = "7e4b4791d6a8ef019b936c79fb7f2c57";
    const G: &str = "222b6b195fe84b1652badb6f6a3ae3d24341e7306967f0b8115b40d5698c7e56";
    const HASH: &str = "da6d3ddc8802fca9058755ffd3ebde08a9c2c74945901a258482a288b6663af06bf645c93cd1c51512307199c80e84908916d983b34af77205f90851a657ee27";
    const YA: &str = "da3d23700a9e5699258aef94dc060dfda5ebb61f02a5ea77fad53f4ff0976d08";
    const YA_PUB: &str = "d6bac480f2c386c394efc7c47adb9925dcd2630b64f240c50f8d0eec482b9157";
    const YB: &str = "d2316b454718c35362d83d69df6320f38578ed5984651435e2949762d900b80d";
    const YB_PUB: &str = "3ea7e0b19560d7c0b0f5734f63b955286dfa8232b5ebe63324e2d9e7433f7258";
    const K: &str = "80b69a8a76457ab6a4d7f887a4bf6b55a2f80ac19c333f917a05fc9887c8b40f";
    const ISK_IR: &str = "b69effbf61b51d56401c0f65601abe428de8206feaaf0e32198896dcae7b35cd2b38950a39dfd5d4a79164614c2984f7daa460b588c1e80c3fa2068af7900447";

    #[test]
    fn p02_prepend_len_and_lv_cat_match_appendix_a1() {
        let enc = |d: &[u8]| {
            let mut v = Vec::new();
            prepend_len(&mut v, d);
            v
        };
        assert_eq!(enc(b""), h("00"));
        assert_eq!(enc(b"1234"), h("0431323334"));
        let r127: Vec<u8> = (0..127u8).collect();
        let mut want = vec![0x7f];
        want.extend(&r127);
        assert_eq!(enc(&r127), want);
        let r128: Vec<u8> = (0..128u8).collect();
        let mut want = vec![0x80, 0x01];
        want.extend(&r128);
        assert_eq!(enc(&r128), want);
        assert_eq!(
            lv_cat(&[b"1234", b"5", b"", b"678"]),
            h("043132333401350003363738")
        );
        // Two-byte and three-byte LEB128 lengths.
        assert_eq!(enc(&vec![0u8; 300])[..2], [0xac, 0x02]);
        assert_eq!(enc(&vec![0u8; 16_384])[..3], [0x80, 0x80, 0x01]);
    }

    #[test]
    fn p01_generator_matches_appendix_b3() {
        let gen_str = generator_string(PRS, &h(CI), &h(SID));
        // The draft prints the string as: DSI, PRS, 100 zero bytes (length 0x64), CI, sid.
        let mut want = String::new();
        want.push_str("11435061636552697374726574746f3235350850617373776f7264");
        want.push_str("64");
        want.push_str(&"00".repeat(100));
        want.push_str("180b415f696e69746961746f720b425f726573706f6e646572");
        want.push_str("107e4b4791d6a8ef019b936c79fb7f2c57");
        assert_eq!(gen_str.len(), 170);
        assert_eq!(hex::encode(&gen_str), want);
        let digest = Sha512::digest(&gen_str);
        assert_eq!(hex::encode(digest), HASH);
        let g = generator(PRS, &h(CI), &h(SID));
        assert_eq!(hex::encode(g.compress().to_bytes()), G);
    }

    #[test]
    fn p01_shares_shared_point_and_isk_match_appendix_b3() {
        let g = generator(PRS, &h(CI), &h(SID));
        let a = Share::from_scalar_bytes(&g, &a32(YA)).unwrap();
        let b = Share::from_scalar_bytes(&g, &a32(YB)).unwrap();
        assert_eq!(hex::encode(a.public()), YA_PUB);
        assert_eq!(hex::encode(b.public()), YB_PUB);
        assert_eq!(hex::encode(*a.shared_point(b.public()).unwrap()), K);
        assert_eq!(hex::encode(*b.shared_point(a.public()).unwrap()), K);
        let isk_a = a
            .finish(Role::Initiator, b.public(), &h(SID), b"ADa", b"ADb")
            .unwrap();
        let isk_b = b
            .finish(Role::Responder, a.public(), &h(SID), b"ADa", b"ADb")
            .unwrap();
        assert_eq!(hex::encode(isk_a.as_slice()), ISK_IR);
        assert_eq!(*isk_a, *isk_b);
        // A wrong associated data, sid or role gives another key.
        let other = a
            .finish(Role::Initiator, b.public(), &h(SID), b"ADa", b"ADc")
            .unwrap();
        assert_ne!(*other, *isk_a);
        let swapped = a
            .finish(Role::Responder, b.public(), &h(SID), b"ADa", b"ADb")
            .unwrap();
        assert_ne!(*swapped, *isk_a);
    }

    #[test]
    fn p01_scalar_mult_vfy_valid_and_invalid_inputs_appendix_b3() {
        let s = Scalar::from_bytes_mod_order(a32(
            "7cd0e075fa7955ba52c02759a6c90dbbfc10e6d40aea8d283e407d88cf538a05",
        ));
        let x = a32("2c3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c");
        let want = "7c13645fe790a468f62c39beb7388e541d8405d1ade69d1778c5fe3e7f6b600e";
        assert_eq!(hex::encode(scalar_mult_vfy(&s, &x)), want);
        let point = CompressedRistretto(x).decompress().unwrap();
        assert_eq!(hex::encode((s * point).compress().to_bytes()), want);
        // B.3.11: an invalid encoding and the neutral element both give G.I.
        let y_i1 = a32("2b3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c");
        assert!(CompressedRistretto(y_i1).decompress().is_none());
        assert_eq!(scalar_mult_vfy(&s, &y_i1), IDENTITY);
        assert_eq!(scalar_mult_vfy(&s, &IDENTITY), IDENTITY);
    }

    #[test]
    fn p01_invalid_peer_shares_abort_the_run() {
        let g = generator(PRS, &h(CI), &h(SID));
        let a = Share::from_scalar_bytes(&g, &a32(YA)).unwrap();
        let y_i1 = a32("2b3c6b8c4f3800e7aef6864025b4ed79bd599117e427c41bd47d93d654b4a51c");
        for bad in [y_i1, IDENTITY] {
            assert!(a.shared_point(&bad).is_none());
            for role in [Role::Initiator, Role::Responder] {
                assert_eq!(
                    a.finish(role, &bad, &h(SID), b"ADa", b"ADb").unwrap_err(),
                    CoreError::PairingFailed
                );
            }
        }
        // A non-canonical encoding (the field modulus itself) is rejected too.
        let mut non_canonical = [0xffu8; 32];
        non_canonical[0] = 0xed;
        non_canonical[31] = 0x7f;
        assert!(a.shared_point(&non_canonical).is_none());
    }

    #[test]
    fn sampling_clears_the_top_bits_and_never_yields_zero() {
        let g = generator(PRS, &h(CI), &h(SID));
        // Scripted all-ones bytes: b[31] is masked to 0x0f, the scalar is 2^252 - 1.
        let mut e = Entropy::scripted(vec![vec![0xff; 32]]);
        let s = Share::sample(&g, &mut e).unwrap();
        let mut want = [0xffu8; 32];
        want[31] = 0x0f;
        let direct = Share::from_scalar_bytes(&g, &want).unwrap();
        assert_eq!(s.public(), direct.public());
        // Zero bytes (or only masked-out bits) is a broken RNG, not a valid share.
        let mut zero = Entropy::scripted(vec![vec![0u8; 32]]);
        assert_eq!(
            Share::sample(&g, &mut zero).unwrap_err(),
            CoreError::Randomness
        );
        let mut masked = [0u8; 32];
        masked[31] = 0xf0;
        let mut only_high_bits = Entropy::scripted(vec![masked.to_vec()]);
        assert_eq!(
            Share::sample(&g, &mut only_high_bits).unwrap_err(),
            CoreError::Randomness
        );
    }

    #[test]
    fn debug_never_prints_the_scalar() {
        let g = generator(PRS, &h(CI), &h(SID));
        let a = Share::from_scalar_bytes(&g, &a32(YA)).unwrap();
        assert_eq!(format!("{a:?}"), "Share(<redacted>)");
    }

    #[test]
    fn dsi_lengths_match_the_spec() {
        assert_eq!(DSI.len(), 17);
        assert_eq!(DSI_ISK.len(), 21);
        // The padding of a 4-byte PRS: 128 - 1 - 5 - 18 = 104 (pairing-v1.md §3.3).
        let gen_str = generator_string(&[1, 2, 3, 4], &[7; 36], &[9; 16]);
        assert_eq!(gen_str.len(), 1 + 17 + 1 + 4 + 1 + 104 + 1 + 36 + 1 + 16);
        assert_eq!(gen_str[23], 104);
    }
}
