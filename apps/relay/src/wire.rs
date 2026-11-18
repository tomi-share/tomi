//! Wire formats of `api-v1.md` §2.3: canonical UUIDs, `b64` (base64url without padding,
//! canonical), RFC 3339 timestamps with exactly three decimals, device ids, names.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

/// Largest integer a JSON number may carry in this API: 2^53 - 1 (§2.2).
pub const JSON_INT_MAX: u64 = (1 << 53) - 1;

/// Latest millisecond that `AAAA-MM-JJTHH:MM:SS.mmmZ` can show (year 9999).
pub const TS_MAX_MS: i64 = 253_402_300_799_999;

/// base64url without padding.
pub fn b64_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

/// Strict base64url: no padding, no character outside the alphabet, no non-canonical
/// trailing bits (§2.3).
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

/// SHA-256 digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-256 of several parts concatenated.
pub fn sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Random bytes from the OS CSPRNG. A failure of the generator is fatal for the request.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], getrandom::Error> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out)?;
    Ok(out)
}

/// Canonical UUID (lowercase, hyphenated, 36 characters) to its 16 bytes.
pub fn uuid_parse(s: &str) -> Option<[u8; 16]> {
    if s.len() != 36 {
        return None;
    }
    let u = uuid::Uuid::parse_str(s).ok()?;
    (u.hyphenated().to_string() == s).then(|| *u.as_bytes())
}

/// 16 bytes to the canonical UUID text.
pub fn uuid_format(bytes: &[u8]) -> String {
    match <[u8; 16]>::try_from(bytes) {
        Ok(b) => uuid::Uuid::from_bytes(b).hyphenated().to_string(),
        Err(_) => "00000000-0000-0000-0000-000000000000".to_owned(),
    }
}

/// `AAAA-MM-JJTHH:MM:SS.mmmZ` from milliseconds since the epoch. Out-of-range values are
/// clamped to the representable interval.
pub fn ts_format(ms: i64) -> String {
    let ms = ms.clamp(0, TS_MAX_MS);
    let nanos = i128::from(ms) * 1_000_000;
    match time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
        Ok(t) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
            t.millisecond()
        ),
        Err(_) => "1970-01-01T00:00:00.000Z".to_owned(),
    }
}

/// Strict timestamp parser: only the exact form of [`ts_format`] is accepted (§2.3).
pub fn ts_parse(s: &str) -> Option<i64> {
    if s.len() != 24 || !s.is_ascii() {
        return None;
    }
    let t = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    let ms = i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()?;
    (ts_format(ms) == s).then_some(ms)
}

/// `device_id`: 32 lowercase hexadecimal characters.
pub fn is_device_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Device name: 1 to 64 Unicode characters once trimmed, no control character. Returns the
/// trimmed name.
pub fn clean_name(s: &str) -> Option<String> {
    let t = s.trim();
    let n = t.chars().count();
    ((1..=64).contains(&n) && !t.chars().any(char::is_control)).then(|| t.to_owned())
}

/// `^[a-z0-9-]{1,32}$`.
pub fn is_platform(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// 1 to 32 printable ASCII characters.
pub fn is_app_version(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Host part of a public URL (`https://relay.example.org/` gives `relay.example.org`).
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_is_strict() {
        let data = [1u8, 2, 3, 4];
        let enc = b64_encode(&data);
        assert_eq!(enc, "AQIDBA");
        assert_eq!(b64_decode(&enc), Some(data.to_vec()));
        assert_eq!(b64_decode("AQIDBA=="), None, "padding refused");
        assert_eq!(b64_decode("AQ+D"), None, "standard alphabet refused");
        assert_eq!(
            b64_decode("AQIDBB"),
            None,
            "non canonical trailing bits refused"
        );
    }

    #[test]
    fn uuid_is_canonical_only() {
        let u = "308339b6-1358-4bed-8328-23cc68b7f02b";
        let b = uuid_parse(u).unwrap();
        assert_eq!(uuid_format(&b), u);
        assert!(uuid_parse(&u.to_uppercase()).is_none());
        assert!(uuid_parse(&u.replace('-', "")).is_none());
        assert!(uuid_parse(&format!("{{{u}}}")).is_none());
    }

    #[test]
    fn timestamps_round_trip_and_reject_other_forms() {
        let ms = 1_793_836_800_123;
        let s = ts_format(ms);
        assert_eq!(s, "2026-11-05T00:00:00.123Z");
        assert_eq!(ts_parse(&s), Some(ms));
        assert_eq!(ts_parse("2026-11-05T00:00:00Z"), None);
        assert_eq!(ts_parse("2026-11-05T00:00:00.123+00:00"), None);
        assert_eq!(ts_parse("2026-11-05T00:00:00.1234Z"), None);
        assert_eq!(ts_parse("2026-11-05t00:00:00.123Z"), None);
        assert_eq!(ts_format(TS_MAX_MS + 5), "9999-12-31T23:59:59.999Z");
    }

    #[test]
    fn small_validators() {
        assert!(is_device_id("3f9a0c6e1b7d4e2a8c5f0b9d7e6a1c34"));
        assert!(!is_device_id("3F9A0C6E1B7D4E2A8C5F0B9D7E6A1C34"));
        assert!(!is_device_id("3f9a"));
        assert_eq!(clean_name("  PC-Bureau \n").as_deref(), Some("PC-Bureau"));
        assert_eq!(clean_name("bad\u{7}name"), None);
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name(&"x".repeat(65)), None);
        assert!(is_platform("windows") && !is_platform("Windows") && !is_platform(""));
        assert!(is_app_version("0.1.0") && !is_app_version(""));
        assert_eq!(host_of("https://relay.example.org/"), "relay.example.org");
    }
}
