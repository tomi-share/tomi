//! `tomi-share://` URL parsing and validation.
//!
//! Any web page can trigger a `tomi-share://` URL, so the only effects are *prefilling the
//! popover*, *opening the app or an item* and *showing a pairing code on the pairing
//! screen*. This module returns a [`DeepLinkAction`] that cannot express "send", "delete",
//! "revoke", "join" or "change a setting". Hidden contents ("masqués") are never accepted in
//! a URL; the one secret a link may carry is the single-use pairing code of
//! `tomi-share://pair#<code>` (`spec/pairing-v1.md` §4.3), which only fills the pairing screen:
//! joining is the user's click. The links that carry a relay are read with the core
//! (`parse_pair_link`, `parse_setup_link`): `https://<relay>/l/pair#<code>` and the creation link
//! `https://<relay>/create#<token>` (or `/create/<token>`) only fill the pairing or the creation
//! screen. **A relay read from a link is not trusted**: the screen shows it in full and nothing
//! is stored before the user confirms (`spec/pairing-v1.md` §4.4).

use serde::Serialize;
use url::Url;

use crate::secret::SecretText;

/// Whole raw URL, before any parsing.
pub const MAX_RAW_LEN: usize = 16 * 1024;
/// The code of a pairing link, once percent-decoded. A real one is about 20 characters.
pub const MAX_PAIR_CODE_LEN: usize = 128;
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_TITLE_CHARS: usize = 300;
/// Text parameter, in bytes ("quelques Ko").
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_ITEM_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefillData {
    pub url: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    /// A parameter was cut to its maximum size.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkAction {
    Open,
    OpenItem {
        id: String,
    },
    Prefill(PrefillData),
    /// `tomi-share://pair#<code>`: the code of a pairing, in its canonical form (`4821-crayon-renard`).
    /// A secret. It only fills the pairing screen: nothing starts until the user presses
    /// "Rejoindre" (`spec/pairing-v1.md` §4.4).
    Pair {
        code: SecretText,
        /// The relay of an `https://<relay>/l/pair#<code>` link (`https://host[:port]`),
        /// `None` for the `tomi-share://` form, which names none. Not trusted.
        relay_url: Option<String>,
    },
    /// `https://<relay>/create#<token>` or `/create/<token>`: the installation token and the
    /// relay of a space creation link. A secret. It only fills the creation screen: nothing is
    /// sent until the user presses "Créer".
    Create {
        token: SecretText,
        relay_url: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeepLinkError {
    #[error("not a tomi-share:// URL")]
    WrongScheme,
    #[error("unknown command")]
    UnknownCommand,
    #[error("URL too long")]
    TooLong,
    #[error("secrets are not accepted in URLs")]
    SecretParam,
    #[error("invalid url parameter")]
    InvalidUrl,
    #[error("invalid item id")]
    InvalidItemId,
    #[error("nothing to prefill")]
    Empty,
    #[error("not a pairing code")]
    InvalidPairCode,
}

impl DeepLinkError {
    /// i18n key shown to the user.
    pub fn code(&self) -> &'static str {
        match self {
            Self::WrongScheme | Self::UnknownCommand => "deeplink.unknown",
            Self::TooLong => "deeplink.too_long",
            Self::SecretParam => "deeplink.secret",
            Self::InvalidUrl => "deeplink.invalid_url",
            Self::InvalidItemId => "deeplink.invalid_item",
            Self::Empty => "deeplink.empty",
            Self::InvalidPairCode => "deeplink.pair_invalid",
        }
    }
}

const FORBIDDEN_PARAMS: &[&str] = &["secret", "password", "passwd", "pwd", "token"];

pub fn parse(raw: &str) -> Result<DeepLinkAction, DeepLinkError> {
    if raw.len() > MAX_RAW_LEN {
        return Err(DeepLinkError::TooLong);
    }
    let parsed = Url::parse(raw.trim()).map_err(|_| DeepLinkError::WrongScheme)?;
    if parsed.scheme() == "https" {
        return parse_relay_link(raw.trim());
    }
    if parsed.scheme() != "tomi-share" {
        return Err(DeepLinkError::WrongScheme);
    }
    let command = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let segments: Vec<&str> = parsed
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();

    match command.as_str() {
        "open" => Ok(DeepLinkAction::Open),
        "item" => match segments.as_slice() {
            [id] if valid_item_id(id) => Ok(DeepLinkAction::OpenItem {
                id: (*id).to_string(),
            }),
            _ => Err(DeepLinkError::InvalidItemId),
        },
        "send" => parse_send(&parsed),
        "pair" => parse_pair(&parsed),
        _ => Err(DeepLinkError::UnknownCommand),
    }
}

/// `tomi-share://pair#<code>` (`spec/pairing-v1.md` §4.4): the code is the fragment, and nothing
/// but the fragment is read (a code in the query string is refused). The core reads it the way
/// the spec says (accents, blanks, the first letters of a word...); the screen gets the
/// canonical form, so it shows what will really be used.
fn parse_pair(parsed: &Url) -> Result<DeepLinkAction, DeepLinkError> {
    let fragment = parsed.fragment().unwrap_or_default();
    let decoded = percent_decode_utf8(fragment).ok_or(DeepLinkError::InvalidPairCode)?;
    if decoded.len() > MAX_PAIR_CODE_LEN {
        return Err(DeepLinkError::InvalidPairCode);
    }
    // The core reads the code (accents, blanks, the first letters of a word...) and gives its
    // canonical form: the screen shows what will really be used.
    let link = tomi_core::parse_pair_link(&format!("tomi-share://pair#{decoded}"))
        .ok_or(DeepLinkError::InvalidPairCode)?;
    Ok(DeepLinkAction::Pair {
        code: SecretText::new(link.code),
        relay_url: link.relay_url,
    })
}

/// A link that names its relay: a pairing link (`https://<relay>/l/pair#<code>`) or a creation
/// link (`https://<relay>/create#<token>`). Any other `https` URL is not a Tomi link.
fn parse_relay_link(raw: &str) -> Result<DeepLinkAction, DeepLinkError> {
    if let Some(link) = tomi_core::parse_pair_link(raw) {
        return Ok(DeepLinkAction::Pair {
            code: SecretText::new(link.code),
            relay_url: link.relay_url,
        });
    }
    if let Some(link) = tomi_core::parse_setup_link(raw) {
        return Ok(DeepLinkAction::Create {
            token: SecretText::new(link.token),
            relay_url: link.relay_url,
        });
    }
    Err(DeepLinkError::WrongScheme)
}

/// `%XX` sequences decoded; `None` if the result is not UTF-8.
fn percent_decode_utf8(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hex {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn valid_item_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ITEM_ID_LEN
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn parse_send(parsed: &Url) -> Result<DeepLinkAction, DeepLinkError> {
    let mut url = None;
    let mut title = None;
    let mut text = None;
    let mut truncated = false;

    for (key, value) in parsed.query_pairs() {
        let key = key.to_ascii_lowercase();
        if FORBIDDEN_PARAMS.contains(&key.as_str()) {
            return Err(DeepLinkError::SecretParam);
        }
        match key.as_str() {
            "url" if url.is_none() => url = Some(validate_http_url(&value)?),
            "title" if title.is_none() => {
                let (v, cut) = clean(&value, false, MAX_TITLE_CHARS, usize::MAX);
                truncated |= cut;
                if !v.is_empty() {
                    title = Some(v);
                }
            }
            "text" if text.is_none() => {
                let (v, cut) = clean(&value, true, usize::MAX, MAX_TEXT_BYTES);
                truncated |= cut;
                if !v.is_empty() {
                    text = Some(v);
                }
            }
            _ => {} // unknown or repeated parameters are ignored
        }
    }

    if url.is_none() && text.is_none() {
        return Err(DeepLinkError::Empty);
    }
    Ok(DeepLinkAction::Prefill(PrefillData {
        url,
        title,
        text,
        truncated,
    }))
}

fn validate_http_url(value: &str) -> Result<String, DeepLinkError> {
    let v = value.trim();
    if v.len() > MAX_URL_LEN {
        return Err(DeepLinkError::TooLong);
    }
    if v.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(DeepLinkError::InvalidUrl);
    }
    let u = Url::parse(v).map_err(|_| DeepLinkError::InvalidUrl)?;
    // http(s) only, with a host and no embedded credentials.
    if !matches!(u.scheme(), "http" | "https")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
    {
        return Err(DeepLinkError::InvalidUrl);
    }
    Ok(u.to_string())
}

/// Removes control characters (keeping newlines/tabs when `multiline`), trims, and cuts
/// to the limits on a character boundary. Returns the value and whether it was cut.
fn clean(value: &str, multiline: bool, max_chars: usize, max_bytes: usize) -> (String, bool) {
    let mut out = String::new();
    let mut chars = 0usize;
    let mut cut = false;
    for c in value.chars() {
        let c = if c == '\r' {
            continue;
        } else if c.is_control() && !(multiline && (c == '\n' || c == '\t')) {
            ' '
        } else {
            c
        };
        if chars + 1 > max_chars || out.len() + c.len_utf8() > max_bytes {
            cut = true;
            break;
        }
        out.push(c);
        chars += 1;
    }
    (out.trim().to_string(), cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefill(raw: &str) -> PrefillData {
        match parse(raw).unwrap() {
            DeepLinkAction::Prefill(p) => p,
            other => panic!("expected prefill, got {other:?}"),
        }
    }

    #[test]
    fn send_url_and_title() {
        let p = prefill(
            "tomi-share://send?url=https%3A%2F%2Fexample.org%2Fa%3Fb%3D1&title=Mon%20article",
        );
        assert_eq!(p.url.as_deref(), Some("https://example.org/a?b=1"));
        assert_eq!(p.title.as_deref(), Some("Mon article"));
        assert!(!p.truncated);
    }

    #[test]
    fn send_text() {
        let p = prefill("tomi-share://send?text=bonjour%0Amonde");
        assert_eq!(p.text.as_deref(), Some("bonjour\nmonde"));
        assert_eq!(p.url, None);
    }

    #[test]
    fn trailing_slash_and_case_are_tolerated() {
        assert!(parse("tomi-share://send/?text=a").is_ok());
        assert_eq!(parse("TOMI-SHARE://OPEN").unwrap(), DeepLinkAction::Open);
    }

    #[test]
    fn open_and_item() {
        assert_eq!(parse("tomi-share://open").unwrap(), DeepLinkAction::Open);
        assert_eq!(
            parse("tomi-share://item/abc-123_X").unwrap(),
            DeepLinkAction::OpenItem {
                id: "abc-123_X".into()
            }
        );
    }

    #[test]
    fn item_id_is_validated() {
        for bad in [
            "tomi-share://item/",
            "tomi-share://item",
            "tomi-share://item/a/b",
            "tomi-share://item/a%20b",
            "tomi-share://item/..%2F..",
            &format!("tomi-share://item/{}", "a".repeat(65)),
        ] {
            assert_eq!(parse(bad), Err(DeepLinkError::InvalidItemId), "{bad}");
        }
    }

    #[test]
    fn only_http_and_https_urls() {
        for bad in [
            "tomi-share://send?url=javascript%3Aalert(1)",
            "tomi-share://send?url=file%3A%2F%2F%2FC%3A%2Fx",
            "tomi-share://send?url=ftp%3A%2F%2Fa.org",
            "tomi-share://send?url=data%3Atext%2Fhtml%2Chi",
            "tomi-share://send?url=https%3A%2F%2Fuser%3Apass%40a.org",
            "tomi-share://send?url=not%20a%20url",
            "tomi-share://send?url=",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn secret_like_parameters_are_rejected() {
        for bad in [
            "tomi-share://send?text=a&secret=1",
            "tomi-share://send?text=a&password=hunter2",
            "tomi-share://send?text=a&TOKEN=x",
        ] {
            assert_eq!(parse(bad), Err(DeepLinkError::SecretParam), "{bad}");
        }
    }

    #[test]
    fn oversized_url_is_rejected() {
        let long = format!("https://a.org/{}", "x".repeat(MAX_URL_LEN));
        let raw = format!("tomi-share://send?url={}", long);
        assert_eq!(parse(&raw), Err(DeepLinkError::TooLong));
        let huge = format!("tomi-share://send?text={}", "a".repeat(MAX_RAW_LEN));
        assert_eq!(parse(&huge), Err(DeepLinkError::TooLong));
    }

    #[test]
    fn long_text_and_title_are_truncated_not_refused() {
        let raw = format!(
            "tomi-share://send?text={}&title={}",
            "é".repeat(5000),
            "t".repeat(1000)
        );
        let p = prefill(&raw);
        assert!(p.truncated);
        let text = p.text.unwrap();
        assert!(text.len() <= MAX_TEXT_BYTES);
        assert!(text.chars().all(|c| c == 'é'));
        assert_eq!(p.title.unwrap().chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn control_characters_are_stripped() {
        let p = prefill("tomi-share://send?title=a%00b%07c&text=x%00y");
        assert_eq!(p.title.as_deref(), Some("a b c"));
        assert_eq!(p.text.as_deref(), Some("x y"));
    }

    #[test]
    fn empty_send_is_refused() {
        assert_eq!(parse("tomi-share://send"), Err(DeepLinkError::Empty));
        assert_eq!(
            parse("tomi-share://send?title=seul"),
            Err(DeepLinkError::Empty)
        );
    }

    #[test]
    fn unknown_commands_and_schemes() {
        for bad in [
            "tomi-share://delete?id=1",
            "tomi-share://revoke/abc",
            "tomi-share://settings?shortcut=x",
            "tomi-share://pairing#4821-crayon-renard",
            "tomi-share://unpair#4821-crayon-renard",
        ] {
            assert_eq!(parse(bad), Err(DeepLinkError::UnknownCommand), "{bad}");
        }
        assert_eq!(parse("https://a.org"), Err(DeepLinkError::WrongScheme));
        assert_eq!(parse("garbage"), Err(DeepLinkError::WrongScheme));
    }

    #[test]
    fn unknown_and_repeated_params_are_ignored() {
        let p = prefill("tomi-share://send?text=first&text=second&foo=bar");
        assert_eq!(p.text.as_deref(), Some("first"));
    }

    fn pair(raw: &str) -> Result<String, DeepLinkError> {
        match parse(raw)? {
            DeepLinkAction::Pair { code, .. } => Ok(code.expose().to_owned()),
            other => panic!("expected a pairing link, got {other:?}"),
        }
    }

    #[test]
    fn a_pairing_link_gives_the_canonical_code() {
        for ok in [
            "tomi-share://pair#4821-crayon-renard",
            "TOMI-SHARE://PAIR#4821-Crayon-RENARD",
            "tomi-share://pair/#4821-crayon-renard",
            // The first letters of a word are enough, as when the code is typed.
            "tomi-share://pair#4821-cray-rena",
            "tomi-share://pair#4821%20crayon%20renard",
            "tomi-share://pair#4821_cr%C3%A2yon.renard",
            // Nothing but the fragment is read.
            "tomi-share://pair?utm=x&code=9999-abaisser-abaisser#4821-crayon-renard",
        ] {
            assert_eq!(pair(ok).as_deref(), Ok("4821-crayon-renard"), "{ok}");
        }
    }

    #[test]
    fn a_pairing_link_that_is_not_a_code_is_refused() {
        for bad in [
            "tomi-share://pair",
            "tomi-share://pair#",
            "tomi-share://pair#garbage",
            "tomi-share://pair#4821-crayon",
            "tomi-share://pair#4821-crayon-renard-tomate",
            "tomi-share://pair#0-crayon-renard",
            // The unknown word of the old examples: `lune` is not in the list.
            "tomi-share://pair#7-crayon-lune",
            // A code in the query string is not a code.
            "tomi-share://pair?code=4821-crayon-renard",
            // Not UTF-8 once decoded.
            "tomi-share://pair#%ff%fe",
            &format!(
                "tomi-share://pair#4821-crayon-renard{}",
                "-x".repeat(MAX_PAIR_CODE_LEN)
            ),
        ] {
            assert_eq!(pair(bad), Err(DeepLinkError::InvalidPairCode), "{bad}");
        }
        assert_eq!(
            DeepLinkError::InvalidPairCode.code(),
            "deeplink.pair_invalid"
        );
    }

    const TOKEN: &str = "h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns";

    #[test]
    fn a_link_with_a_relay_gives_the_code_and_the_relay_to_show() {
        let read = |raw: &str| match parse(raw).unwrap() {
            DeepLinkAction::Pair { code, relay_url } => (code.expose().to_owned(), relay_url),
            other => panic!("expected a pairing link, got {other:?}"),
        };
        assert_eq!(
            read("https://relay.example.org/l/pair#4821-cray-rena"),
            (
                "4821-crayon-renard".to_owned(),
                Some("https://relay.example.org".to_owned())
            )
        );
        // The fallback form names no relay: the relay of the device is used.
        assert_eq!(
            read("tomi-share://pair#4821-crayon-renard"),
            ("4821-crayon-renard".to_owned(), None)
        );
        // Not a relay, not a Tomi link: refused, whatever the host.
        for bad in [
            "http://relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/l/other#4821-crayon-renard",
            "https://user@relay.example.org/l/pair#4821-crayon-renard",
            "https://relay.example.org/l/pair",
            "https://example.org/",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_creation_link_gives_the_token_and_the_relay_to_show() {
        for raw in [
            format!("https://relay.example.org/create#{TOKEN}"),
            format!("https://Relay.Example.org/create/{TOKEN}"),
        ] {
            match parse(&raw).unwrap() {
                DeepLinkAction::Create { token, relay_url } => {
                    assert_eq!(token.expose(), TOKEN);
                    assert_eq!(relay_url, "https://relay.example.org");
                }
                other => panic!("expected a creation link, got {other:?}"),
            }
        }
        for bad in [
            "https://relay.example.org/create#abc".to_owned(),
            format!("http://relay.example.org/create#{TOKEN}"),
            format!("https://relay.example.org/make#{TOKEN}"),
            // `tomi-share://` has no creation form: the relay would be missing.
            format!("tomi-share://create#{TOKEN}"),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_token_of_a_creation_link_is_never_printed() {
        let action = parse(&format!("https://relay.example.org/create#{TOKEN}")).unwrap();
        let shown = format!("{action:?}");
        assert!(
            !shown.contains("h7k2") && !shown.contains("q2ns"),
            "{shown}"
        );
    }

    #[test]
    fn the_code_of_a_pairing_link_is_never_printed() {
        let action = parse("tomi-share://pair#4821-crayon-renard").unwrap();
        let shown = format!("{action:?}");
        assert!(
            !shown.contains("crayon") && !shown.contains("renard"),
            "{shown}"
        );
    }
}
