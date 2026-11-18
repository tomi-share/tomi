//! Relay addresses and the links that carry them (`spec/pairing-v1.md` §4.3, §4.4, §10;
//! `spec/api-v1.md` §2.1, §3.1).
//!
//! Pure, always compiled (no network, no storage): the three platforms share this reading instead
//! of re-implementing it.
//!
//! * [`parse_pair_link`]: `https://<relay>/l/pair#<code>` for **any** valid https relay, or the
//!   fallback `tomi-share://pair#<code>` (no relay);
//! * [`parse_setup_link`]: `https://<relay>/create/<token>` or `https://<relay>/create#<token>`;
//! * [`is_install_token_shape`]: the light shape check of a bare installation token.
//!
//! **A relay read from a link, a QR code or a field is not trustworthy by itself**: whoever shows
//! the QR code chooses it. These functions only say what the text *contains*; the application
//! shows the relay in full to the user and nothing is stored before the pairing or the creation is
//! confirmed (`pairing-v1.md` §4.4). Neither the code nor the token is ever logged: their types
//! print `<redacted>`.

use crate::error::{CoreError, Result};
use crate::pairing::PairingCode;

/// Path of the pairing link on the relay host.
const PAIR_PATH: &str = "/l/pair";
/// Host of the `tomi-share://` form of the pairing link.
const DEEP_LINK_HOST: &str = "pair";
/// Path of the space creation link on the relay host.
const CREATE_PATH: &str = "/create";

/// Alphabet of the installation token (`api-v1.md` §3.1): digits and lowercase letters without
/// `i`, `l`, `o`, `u`.
const TOKEN_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";
/// Shortest and longest installation token, separators removed. A token is 32 of them.
const TOKEN_MIN_LEN: usize = 24;
const TOKEN_MAX_LEN: usize = 40;
/// Longest raw token looked at: a token with its dashes is 39 characters, the rest is a margin.
const TOKEN_MAX_RAW: usize = 64;

/// A relay base URL, parsed (`https://host[:port][/prefix]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaseUrl {
    /// Normalised URL, prefix path included, without the trailing slash.
    pub url: String,
    pub tls: bool,
    /// Host name or IP address, without the IPv6 brackets.
    pub host: String,
    pub port: u16,
    /// `host[:port]` as written (IPv6 brackets kept).
    pub authority: String,
    /// Prefix path: empty or `/a/b`.
    pub path: String,
}

impl BaseUrl {
    /// URL of the doorbell (`api-v1.md` §5), under the same prefix.
    pub fn ws_url(&self) -> String {
        let scheme = if self.tls { "wss" } else { "ws" };
        format!("{scheme}://{}{}/v1/ws", self.authority, self.path)
    }

    /// The authority the pairing PAKE binds to (`pairing-v1.md` §3.3): the host in lowercase
    /// ASCII (IPv6 in brackets), and `:port` unless it is the default of the scheme.
    pub fn relay_authority(&self) -> String {
        let host = self.host.to_ascii_lowercase();
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        let default_port = if self.tls { 443 } else { 80 };
        if self.port == default_port {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

fn parse_base_url(url: &str) -> Result<BaseUrl> {
    let bad = |why: &str| CoreError::input(format!("relay URL: {why}"));
    let trimmed = url.trim().trim_end_matches('/');
    let (tls, rest) = if let Some(r) = trimmed.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = trimmed.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(CoreError::input("the relay URL must be https://"));
    };
    if rest
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '?' | '#' | '\\'))
    {
        return Err(bad("no query, fragment, space or backslash"));
    }
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    // `http://localhost:1@evil.com` would reach evil.com: never accept user information.
    if authority.contains('@') {
        return Err(bad("no user information"));
    }
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        let end = after
            .find(']')
            .ok_or_else(|| bad("unclosed IPv6 bracket"))?;
        let host = &after[..end];
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(bad("invalid IPv6 address"));
        }
        let tail = &after[end + 1..];
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p),
            None if tail.is_empty() => None,
            None => return Err(bad("unexpected text after the IPv6 address")),
        };
        (host, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        if host.is_empty() || host.contains(['[', ']', ':']) {
            return Err(bad("invalid host"));
        }
        (host, port)
    };
    // A port is written with digits only, without a sign or a leading zero (`u16::from_str`
    // accepts `+8443`, which would be shown as `:8443` and kept as `:+8443`).
    let explicit_port = match port {
        Some(p) => {
            let canonical = !p.starts_with('0') && p.bytes().all(|b| b.is_ascii_digit());
            Some(
                p.parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0 && canonical)
                    .ok_or_else(|| bad("invalid port"))?,
            )
        }
        None => None,
    };
    let default_port = if tls { 443 } else { 80 };
    let port = explicit_port.unwrap_or(default_port);
    // The default port is not written: `https://host:443` and `https://host` are one relay, with
    // one spelling (compared as text by the sessions, `set_relay` and the pending record).
    let explicit_port = explicit_port.filter(|p| *p != default_port);
    // The URL and the authority are rebuilt from the parts that were checked, never copied from
    // the text: what is stored and used is what the PAKE authority shows. The host is not case
    // sensitive: one spelling, so that two spellings of a relay compare equal.
    let host = host.to_ascii_lowercase();
    let authority = {
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        match explicit_port {
            Some(p) => format!("{host}:{p}"),
            None => host,
        }
    };
    Ok(BaseUrl {
        url: format!("{}://{authority}{path}", if tls { "https" } else { "http" },),
        tls,
        host,
        port,
        authority,
        path: path.to_owned(),
    })
}

/// `api-v1.md` §2.1: HTTPS only; plain HTTP is accepted in debug builds towards the local
/// machine and the Android emulator host.
pub(crate) fn check_base_url(url: &str) -> Result<BaseUrl> {
    let b = parse_base_url(url)?;
    if b.tls
        || (cfg!(debug_assertions)
            && matches!(
                b.host.as_str(),
                "localhost" | "127.0.0.1" | "10.0.2.2" | "::1"
            ))
    {
        return Ok(b);
    }
    Err(CoreError::input("the relay URL must be https://"))
}

/// A relay typed, scanned or read from a link, before it is shown to the user: the rules of
/// [`check_base_url`], and an authority the pairing PAKE can bind to (lowercase ASCII, no
/// look-alike characters: what the user is shown is what is used).
pub(crate) fn check_relay_url(url: &str) -> Result<BaseUrl> {
    let b = check_base_url(url)?;
    crate::pairing::check_relay_authority(&b.relay_authority())?;
    Ok(b)
}

/// The authority (`host[:port]`, lowercase) of a relay URL that passes [`check_relay_url`].
#[cfg_attr(not(feature = "demo"), allow(dead_code))]
pub(crate) fn relay_authority_of(url: &str) -> Result<String> {
    Ok(check_relay_url(url)?.relay_authority())
}

/// The `https` origin of an authority read from a link, normalised (`https://host[:port]`), or
/// `None` when it is not a valid relay.
fn https_origin(authority: &str) -> Option<String> {
    if authority.is_empty() {
        return None;
    }
    let b = check_relay_url(&format!("https://{authority}")).ok()?;
    Some(format!("https://{}", b.relay_authority()))
}

/// What a pairing link contains.
#[derive(Clone, PartialEq, Eq)]
pub struct PairLink {
    /// The relay of the link, `https://<authority>`, normalised (lowercase host, default port
    /// dropped). `None` for the fallback form `tomi-share://pair#<code>`, which names no relay.
    /// **Not trusted**: the user must see it and confirm it.
    pub relay_url: Option<String>,
    /// The code in its canonical form (`4821-crayon-renard`). Secret.
    pub code: String,
}

impl std::fmt::Debug for PairLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairLink")
            .field("relay_url", &self.relay_url)
            .field("code", &"<redacted>")
            .finish()
    }
}

/// What a space creation link contains.
#[derive(Clone, PartialEq, Eq)]
pub struct SetupLink {
    /// The relay of the link, `https://<authority>`, normalised. **Not trusted** (see
    /// [`PairLink::relay_url`]).
    pub relay_url: String,
    /// The installation token as written in the link, trimmed. Secret.
    pub token: String,
}

impl std::fmt::Debug for SetupLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupLink")
            .field("relay_url", &self.relay_url)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Cuts `scheme://location[#fragment]` into the lowercase-insensitive scheme, the authority, the
/// path (query dropped: no parameter is ever read) and the fragment.
fn split_link(text: &str) -> Option<(&str, &str, &str, Option<&str>)> {
    let s = text.trim();
    let (scheme, rest) = s.split_once("://")?;
    let (location, fragment) = match rest.split_once('#') {
        Some((l, f)) => (l, Some(f)),
        None => (rest, None),
    };
    let (authority, path_and_query) = match location.find(['/', '?']) {
        Some(i) => location.split_at(i),
        None => (location, ""),
    };
    let path = path_and_query.split('?').next().unwrap_or_default();
    Some((scheme, authority, path, fragment))
}

/// Reads a pairing link (`pairing-v1.md` §4.4 rule 1): `https://<relay>/l/pair#<code>` for any
/// valid https relay, or `tomi-share://pair#<code>`. `None` for anything else, a code that cannot be
/// read, a host that is not a valid relay (user information, port 0, non-ASCII host...), another
/// scheme (`http`), or another path. Nothing but the fragment is read; the query is ignored.
pub fn parse_pair_link(text: &str) -> Option<PairLink> {
    let (scheme, authority, path, fragment) = split_link(text)?;
    let relay_url = if scheme.eq_ignore_ascii_case("https") {
        if path != PAIR_PATH {
            return None;
        }
        Some(https_origin(authority)?)
    } else if scheme.eq_ignore_ascii_case("tomi-share") {
        if !(authority.eq_ignore_ascii_case(DEEP_LINK_HOST) && (path.is_empty() || path == "/")) {
            return None;
        }
        None
    } else {
        return None;
    };
    let code = PairingCode::parse(fragment?).ok()?.to_code_string();
    Some(PairLink { relay_url, code })
}

/// Reads a space creation link (`pairing-v1.md` §10.2): `https://<relay>/create/<token>` or
/// `https://<relay>/create#<token>` (the form a relay shows: the fragment never reaches a
/// server). `None` when the host is not a valid relay, the scheme is not `https`, the path is
/// another one, both forms are mixed, or the token has not the shape of an installation token
/// ([`is_install_token_shape`]). Nothing else is read.
pub fn parse_setup_link(text: &str) -> Option<SetupLink> {
    let (scheme, authority, path, fragment) = split_link(text)?;
    if !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let token = match (path.strip_prefix(CREATE_PATH)?, fragment) {
        // `/create#<token>`, with or without the trailing slash.
        ("" | "/", Some(f)) => f,
        // `/create/<token>`, no fragment.
        (p, None) => p.strip_prefix('/').filter(|t| !t.contains('/'))?,
        _ => return None,
    };
    let token = token.trim();
    if !is_install_token_shape(token) {
        return None;
    }
    Some(SetupLink {
        relay_url: https_origin(authority)?,
        token: token.to_owned(),
    })
}

/// Light shape check of an installation token (`api-v1.md` §3.1) before it is taken for one, so
/// that no QR code is mistaken for a token: once dashes and blanks are removed and the text is
/// lowercased, 24 to 40 characters of the relay's base32 alphabet (a token is 32). The relay
/// decides whether the token is valid.
pub fn is_install_token_shape(text: &str) -> bool {
    let raw = text.trim();
    if raw.is_empty() || raw.len() > TOKEN_MAX_RAW {
        return false;
    }
    let mut len = 0usize;
    for c in raw.chars() {
        if c == '-' || c.is_whitespace() {
            continue;
        }
        if !TOKEN_ALPHABET.contains(c.to_ascii_lowercase()) {
            return false;
        }
        len += 1;
    }
    (TOKEN_MIN_LEN..=TOKEN_MAX_LEN).contains(&len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "4821-crayon-renard";
    const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";

    fn pair(text: &str) -> Option<(Option<String>, String)> {
        parse_pair_link(text).map(|p| (p.relay_url, p.code))
    }

    #[test]
    fn a_pairing_link_of_any_valid_https_relay_is_read() {
        let want = |relay: &str| Some((Some(relay.to_owned()), CODE.to_owned()));
        assert_eq!(
            pair("https://relay.example.org/l/pair#4821-crayon-renard"),
            want("https://relay.example.org")
        );
        // Case of scheme and host, default port, blanks around, a non-default port, a query.
        assert_eq!(
            pair("  HTTPS://Relay.Example.ORG:443/l/pair#4821-crayon-renard\n"),
            want("https://relay.example.org")
        );
        assert_eq!(
            pair("https://relay.example.org:8443/l/pair#4821-crayon-renard"),
            want("https://relay.example.org:8443")
        );
        assert_eq!(
            pair("https://relay.example.org/l/pair?utm=1#4821-crayon-renard"),
            want("https://relay.example.org")
        );
        assert_eq!(
            pair("https://[2001:db8::1]/l/pair#4821-crayon-renard"),
            want("https://[2001:db8::1]")
        );
        // The code is canonical whatever way it was written.
        assert_eq!(
            pair("https://relay.example.org/l/pair#4821 Crayon-RENA"),
            want("https://relay.example.org")
        );
        // The fallback form names no relay.
        assert_eq!(
            pair("tomi-share://pair#4821-crayon-renard"),
            Some((None, CODE.to_owned()))
        );
        assert_eq!(
            pair("TOMI-SHARE://Pair/#4821-crayon-renard"),
            Some((None, CODE.to_owned()))
        );
    }

    #[test]
    fn a_pairing_link_with_a_trap_is_refused() {
        for bad in [
            // Not https.
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "ftp://relay.example.org/l/pair#4821-crayon-renard",
            // User information: `relay.example.org` would be the user, evil the host.
            "https://relay.example.org@evil.example/l/pair#4821-crayon-renard",
            "https://relay.example.org:443@evil.example/l/pair#4821-crayon-renard",
            // Invalid hosts and ports.
            "https:///l/pair#4821-crayon-renard",
            "https://relay.example.org:0/l/pair#4821-crayon-renard",
            "https://relay.example.org:99999/l/pair#4821-crayon-renard",
            "https://re lay.example.org/l/pair#4821-crayon-renard",
            // A look-alike host is refused: what is shown is what is used.
            "https://r\u{435}lay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org\u{202e}/l/pair#4821-crayon-renard",
            // Other paths.
            "https://relay.example.org/l/pair/#4821-crayon-renard",
            "https://relay.example.org/l/pairs#4821-crayon-renard",
            "https://relay.example.org/l/pair/x#4821-crayon-renard",
            "https://relay.example.org/create#4821-crayon-renard",
            "https://relay.example.org/#4821-crayon-renard",
            "https://relay.example.org?x=/l/pair#4821-crayon-renard",
            // Fallback form with another host or path.
            "tomi-share://create#4821-crayon-renard",
            "tomi-share://pair/x#4821-crayon-renard",
            "tomi-share://relay.example.org/l/pair#4821-crayon-renard",
            // No or bad code.
            "https://relay.example.org/l/pair",
            "https://relay.example.org/l/pair#",
            "https://relay.example.org/l/pair#4821-crayon",
            "https://relay.example.org/l/pair#0-crayon-renard",
            "https://relay.example.org/l/pair?c=4821-crayon-renard",
            // Not a link at all: a bare code is read by the engine, not here.
            "4821-crayon-renard",
            "",
        ] {
            assert_eq!(pair(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_host_that_looks_like_another_relay_is_reported_as_it_is() {
        // Valid: it is another relay, and the application shows it in full for the user to read.
        assert_eq!(
            pair("https://relay.example.org.evil.example/l/pair#4821-crayon-renard"),
            Some((
                Some("https://relay.example.org.evil.example".to_owned()),
                CODE.to_owned()
            ))
        );
    }

    fn setup(text: &str) -> Option<(String, String)> {
        parse_setup_link(text).map(|s| (s.relay_url, s.token))
    }

    #[test]
    fn a_creation_link_is_read_in_both_forms() {
        let want = Some(("https://relay.example.org".to_owned(), TOKEN.to_owned()));
        assert_eq!(
            setup(&format!("https://relay.example.org/create#{TOKEN}")),
            want
        );
        assert_eq!(
            setup(&format!("https://relay.example.org/create/#{TOKEN}")),
            want
        );
        assert_eq!(
            setup(&format!("https://relay.example.org/create/{TOKEN}")),
            want
        );
        assert_eq!(
            setup(&format!(
                "  https://Relay.Example.org:443/create/{TOKEN}?a=1\n"
            )),
            want
        );
        assert_eq!(
            setup(&format!("https://relay.example.org:8443/create#{TOKEN}")),
            Some((
                "https://relay.example.org:8443".to_owned(),
                TOKEN.to_owned()
            ))
        );
        // Without dashes, in capitals: the shape check is the one of the bare token.
        let bare = TOKEN.replace('-', "").to_uppercase();
        assert_eq!(
            setup(&format!("https://relay.example.org/create#{bare}")),
            Some(("https://relay.example.org".to_owned(), bare))
        );
    }

    #[test]
    fn a_creation_link_with_a_trap_is_refused() {
        let short = "h7k2-m9qv-d4xr-8tbn-c3wz-p6f"; // 23 characters
        let long = format!("{TOKEN}-h7k2-m9qv-d"); // 41
        for bad in [
            format!("http://relay.example.org/create#{TOKEN}"),
            format!("https://relay.example.org@evil.example/create#{TOKEN}"),
            format!("https://relay.example.org:0/create#{TOKEN}"),
            format!("https://relay.example.org/creates#{TOKEN}"),
            format!("https://relay.example.org/create/x/{TOKEN}"),
            format!("https://relay.example.org/create/{TOKEN}/"),
            format!("https://relay.example.org/l/pair#{TOKEN}"),
            format!("https://relay.example.org/#{TOKEN}"),
            // Both forms at once.
            format!("https://relay.example.org/create/{TOKEN}#{TOKEN}"),
            // No token, or not the shape of one.
            "https://relay.example.org/create".to_owned(),
            "https://relay.example.org/create/".to_owned(),
            "https://relay.example.org/create#".to_owned(),
            format!("https://relay.example.org/create#{short}"),
            format!("https://relay.example.org/create#{long}"),
            // `i`, `l`, `o`, `u` are not in the alphabet.
            "https://relay.example.org/create#h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2nu".to_owned(),
            // Not a link.
            TOKEN.to_owned(),
            String::new(),
        ] {
            assert_eq!(setup(&bad), None, "{bad}");
        }
    }

    #[test]
    fn the_token_shape_is_the_one_of_the_relay() {
        assert!(is_install_token_shape(TOKEN));
        assert!(is_install_token_shape(&format!(
            "  {}\n",
            TOKEN.to_uppercase()
        )));
        assert!(is_install_token_shape(&"a".repeat(24)));
        assert!(is_install_token_shape(&"a".repeat(40)));
        assert!(!is_install_token_shape(&"a".repeat(23)));
        assert!(!is_install_token_shape(&"a".repeat(41)));
        assert!(!is_install_token_shape(&"a".repeat(65)));
        assert!(!is_install_token_shape(""));
        assert!(!is_install_token_shape("https://relay.example.org/create"));
        for banned in ['i', 'l', 'o', 'u'] {
            assert!(!is_install_token_shape(&format!(
                "{}{banned}",
                "a".repeat(31)
            )));
        }
    }

    #[test]
    fn what_a_link_carries_never_prints() {
        let p = parse_pair_link("https://relay.example.org/l/pair#4821-crayon-renard").unwrap();
        let s = parse_setup_link(&format!("https://relay.example.org/create#{TOKEN}")).unwrap();
        for dbg in [format!("{p:?}"), format!("{p:#?}")] {
            assert!(
                !dbg.contains("crayon") && dbg.contains("relay.example.org"),
                "{dbg}"
            );
        }
        for dbg in [format!("{s:?}"), format!("{s:#?}")] {
            assert!(
                !dbg.contains("h7k2") && dbg.contains("relay.example.org"),
                "{dbg}"
            );
        }
    }

    #[test]
    fn a_port_is_plain_digits_and_the_url_is_rebuilt_from_what_was_checked() {
        for bad in [
            "https://relay.example.org:+8443",
            "https://relay.example.org:-8443",
            "https://relay.example.org:0443",
            "https://relay.example.org:00",
            "https://relay.example.org:",
            "https://relay.example.org:8 443",
            "https://relay.example.org:8443x",
            "https://[2001:db8::1]:+8443",
            "https://[2001:db8::1]:0443",
        ] {
            assert!(check_relay_url(bad).is_err(), "{bad}");
            assert!(check_base_url(bad).is_err(), "{bad}");
        }
        for link in [
            "https://relay.example.org:+8443/l/pair#4821-crayon-renard",
            "https://relay.example.org:0443/l/pair#4821-crayon-renard",
        ] {
            assert_eq!(parse_pair_link(link), None, "{link}");
        }
        // What is kept is what the PAKE shows: same host, same port, lowercase.
        let b = check_relay_url("https://Relay.Example.ORG:8443/Prefix/").unwrap();
        assert_eq!(b.url, "https://relay.example.org:8443/Prefix");
        assert_eq!(b.authority, "relay.example.org:8443");
        assert_eq!(b.relay_authority(), "relay.example.org:8443");
        assert_eq!(b.url, format!("https://{}{}", b.authority, b.path));
        let b = check_relay_url("https://[2001:DB8::1]:8443").unwrap();
        assert_eq!(b.url, "https://[2001:db8::1]:8443");
        assert_eq!(b.relay_authority(), "[2001:db8::1]:8443");
        let b = check_relay_url("https://relay.example.org:443").unwrap();
        // The default port has no spelling of its own: one relay, one text.
        assert_eq!((b.url.as_str(), b.port), ("https://relay.example.org", 443));
        assert_eq!(b.authority, "relay.example.org");
        assert_eq!(b.relay_authority(), "relay.example.org");
        assert_eq!(
            check_relay_url("https://relay.example.org:443/p")
                .unwrap()
                .url,
            check_relay_url("https://RELAY.example.org/p/").unwrap().url
        );
    }

    #[test]
    fn the_relay_a_user_may_name_is_checked() {
        for ok in [
            "https://relay.example.org",
            "https://relay.example.org/",
            "https://relay.example.org:8443/prefix",
            "https://[2001:db8::1]:8443",
        ] {
            assert!(check_relay_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://relay.example.org",
            "ftp://relay.example.org",
            "relay.example.org",
            "https://user:pw@relay.example.org",
            "https://relay.example.org?x=1",
            "https://relay.example.org#f",
            "https://relay.example.org:0",
            "https://relay.example.org:99999",
            "https://r\u{435}lay.example.org",
            "https://",
            "",
        ] {
            assert!(check_relay_url(bad).is_err(), "{bad}");
        }
    }
}
