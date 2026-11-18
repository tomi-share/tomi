//! Content-type detection and the "looks like a secret" heuristic.
//!
//! Pure functions, no I/O. The clipboard-format signal for secrets lives in
//! [`crate::clipboard`]; this module only holds the fallback heuristic.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

use crate::model::SourceFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    Link,
    Text,
    RichText,
    Image,
    File,
    Files,
}

/// Result of analysing a pasted payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub kind: ContentKind,
    pub source_format: Option<SourceFormat>,
    /// Set for links (alone or with a title).
    pub url: Option<String>,
    /// Title extracted next to a link ("Page title https://...").
    pub title: Option<String>,
}

const MAX_TITLE_PREFIX_CHARS: usize = 300;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    // The patterns are constants verified by the unit tests below.
    cell.get_or_init(|| Regex::new(pattern).unwrap_or_else(|_| unreachable!("static regex")))
}

/// `Some(url)` when the whole trimmed text is one http(s) URL.
pub fn single_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.chars().any(char::is_whitespace) {
        return None;
    }
    let u = url::Url::parse(t).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return None;
    }
    Some(t.to_string())
}

/// Recognises "Some title https://example.org" (what Chrome's share sheet produces)
/// and "Title\nhttps://example.org": a link preceded by a short title, nothing else.
pub fn link_with_title(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    let idx = t.rfind(char::is_whitespace)?;
    let (prefix, last) = t.split_at(idx);
    let url = single_url(last)?;
    let title = prefix.trim();
    if title.is_empty()
        || title.chars().count() > MAX_TITLE_PREFIX_CHARS
        || title.contains("://")
        || title.lines().count() > 1
    {
        return None;
    }
    Some((title.to_string(), url))
}

pub fn looks_like_markdown(text: &str) -> bool {
    static FENCE: OnceLock<Regex> = OnceLock::new();
    static HEADING: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static BOLD: OnceLock<Regex> = OnceLock::new();
    static LIST: OnceLock<Regex> = OnceLock::new();

    if re(&FENCE, r"(?m)^\s*```").is_match(text)
        || re(&HEADING, r"(?m)^#{1,6}[ \t]+\S").is_match(text)
        || re(&LINK, r"\[[^\]\n]+\]\([^)\s]+\)").is_match(text)
        || re(&BOLD, r"\*\*[^*\n]+\*\*").is_match(text)
    {
        return true;
    }
    re(&LIST, r"(?m)^\s*(?:[-*+]|\d+\.)[ \t]+\S")
        .find_iter(text)
        .count()
        >= 2
}

/// True when the HTML carries real formatting. Browsers put an HTML flavour on every
/// copy, even plain selections (`<meta><span>text</span>`); those stay plain text.
pub fn html_is_rich(html: &str) -> bool {
    const RICH: &[&str] = &[
        "a",
        "b",
        "strong",
        "i",
        "em",
        "u",
        "ul",
        "ol",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "table",
        "pre",
        "code",
        "blockquote",
        "img",
        "hr",
    ];
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    let mut paragraphs = 0;
    while let Some(pos) = rest.find('<') {
        rest = &rest[pos + 1..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if RICH.contains(&name.as_str()) {
            return true;
        }
        if name == "p" || name == "div" || name == "br" {
            paragraphs += 1;
            if paragraphs >= 3 {
                return true;
            }
        }
    }
    false
}

/// Detects the kind of a pasted or prefilled payload.
/// `file_mimes` is the MIME type of each attached file.
pub fn detect(plain: &str, html: Option<&str>, file_mimes: &[&str]) -> Detection {
    match file_mimes {
        [] => {}
        [mime] => {
            let kind = if mime.starts_with("image/") {
                ContentKind::Image
            } else {
                ContentKind::File
            };
            return Detection {
                kind,
                source_format: None,
                url: None,
                title: None,
            };
        }
        _ => {
            return Detection {
                kind: ContentKind::Files,
                source_format: None,
                url: None,
                title: None,
            }
        }
    }

    if let Some(url) = single_url(plain) {
        return Detection {
            kind: ContentKind::Link,
            source_format: None,
            url: Some(url),
            title: None,
        };
    }
    if let Some((title, url)) = link_with_title(plain) {
        return Detection {
            kind: ContentKind::Link,
            source_format: None,
            url: Some(url),
            title: Some(title),
        };
    }
    if html.is_some_and(html_is_rich) {
        return Detection {
            kind: ContentKind::RichText,
            source_format: Some(SourceFormat::Html),
            url: None,
            title: None,
        };
    }
    if looks_like_markdown(plain) {
        return Detection {
            kind: ContentKind::RichText,
            source_format: Some(SourceFormat::Markdown),
            url: None,
            title: None,
        };
    }
    Detection {
        kind: ContentKind::Text,
        source_format: Some(SourceFormat::Plain),
        url: None,
        title: None,
    }
}

const SECRET_MIN_CHARS: usize = 8;
const SECRET_MAX_CHARS: usize = 128;

/// Fallback heuristic when the clipboard does not carry the "sensitive" flag:
/// one word, no whitespace, a mix of at least three character classes.
/// Never a certainty: the UI shows it as a pre-checked, editable box.
pub fn looks_like_secret(text: &str) -> bool {
    let t = text.trim();
    let n = t.chars().count();
    if !(SECRET_MIN_CHARS..=SECRET_MAX_CHARS).contains(&n) {
        return false;
    }
    if t.chars().any(char::is_whitespace) || t.contains("://") {
        return false;
    }
    // Windows paths ("C:\..." / "C:/...").
    let mut chars = t.chars();
    if let (Some(d), Some(':'), Some(sep)) = (chars.next(), chars.next(), chars.next()) {
        if d.is_ascii_alphabetic() && (sep == '\\' || sep == '/') {
            return false;
        }
    }
    // UUIDs and hex digests with dashes are identifiers, not passwords.
    if t.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    let lower = t.chars().any(char::is_lowercase);
    let upper = t.chars().any(char::is_uppercase);
    let digit = t.chars().any(|c| c.is_ascii_digit());
    let symbol = t.chars().any(|c| !c.is_alphanumeric());
    [lower, upper, digit, symbol].iter().filter(|b| **b).count() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(plain: &str, html: Option<&str>, files: &[&str]) -> ContentKind {
        detect(plain, html, files).kind
    }

    #[test]
    fn static_regexes_compile() {
        // Forces every lazily-built regex once.
        assert!(looks_like_markdown("# t"));
        assert!(looks_like_markdown("```\nx\n```"));
        assert!(looks_like_markdown("[a](b)"));
        assert!(looks_like_markdown("**a**"));
        assert!(looks_like_markdown("- a\n- b"));
    }

    #[test]
    fn single_http_line_is_a_link() {
        let d = detect("https://example.org/a?b=1\n", None, &[]);
        assert_eq!(d.kind, ContentKind::Link);
        assert_eq!(d.url.as_deref(), Some("https://example.org/a?b=1"));
        assert_eq!(kind("http://localhost:8080", None, &[]), ContentKind::Link);
    }

    #[test]
    fn non_http_or_multiword_is_not_a_link() {
        assert_eq!(kind("ftp://example.org/x", None, &[]), ContentKind::Text);
        assert_eq!(kind("javascript:alert(1)", None, &[]), ContentKind::Text);
        assert_eq!(
            kind("voir https://a.org et https://b.org", None, &[]),
            ContentKind::Text
        );
        assert_eq!(kind("example.org", None, &[]), ContentKind::Text);
    }

    #[test]
    fn title_plus_url_is_a_link_with_title() {
        let d = detect("Un bel article https://example.org/x", None, &[]);
        assert_eq!(d.kind, ContentKind::Link);
        assert_eq!(d.title.as_deref(), Some("Un bel article"));
        assert_eq!(d.url.as_deref(), Some("https://example.org/x"));

        let d = detect("Un bel article\nhttps://example.org/x", None, &[]);
        assert_eq!(d.title.as_deref(), Some("Un bel article"));
    }

    #[test]
    fn html_flavour_of_a_plain_copy_stays_plain() {
        let html = "<meta charset='utf-8'><span style=\"color: red\">bonjour</span>";
        assert_eq!(kind("bonjour", Some(html), &[]), ContentKind::Text);
    }

    #[test]
    fn formatted_html_is_rich_text() {
        let d = detect("Titre\n- a", Some("<h1>Titre</h1><ul><li>a</li></ul>"), &[]);
        assert_eq!(d.kind, ContentKind::RichText);
        assert_eq!(d.source_format, Some(SourceFormat::Html));
        assert_eq!(
            kind(
                "x",
                Some("<p>un <a href=\"https://a.org\">lien</a></p>"),
                &[]
            ),
            ContentKind::RichText
        );
    }

    #[test]
    fn markdown_syntax_is_rich_text_with_markdown_format() {
        let d = detect("# Titre\n\ntexte", None, &[]);
        assert_eq!(d.kind, ContentKind::RichText);
        assert_eq!(d.source_format, Some(SourceFormat::Markdown));
        assert_eq!(
            kind("voir [la doc](https://a.org/doc)", None, &[]),
            ContentKind::RichText
        );
        assert_eq!(kind("- un\n- deux", None, &[]), ContentKind::RichText);
    }

    #[test]
    fn ordinary_text_is_not_markdown() {
        let d = detect("Rendez-vous à 14h - salle 3", None, &[]);
        assert_eq!(d.kind, ContentKind::Text);
        assert_eq!(d.source_format, Some(SourceFormat::Plain));
        assert_eq!(kind("#hashtag", None, &[]), ContentKind::Text);
        assert_eq!(kind("- une seule puce", None, &[]), ContentKind::Text);
    }

    #[test]
    fn files_are_detected_by_count_and_mime() {
        assert_eq!(kind("", None, &["image/png"]), ContentKind::Image);
        assert_eq!(kind("", None, &["application/pdf"]), ContentKind::File);
        assert_eq!(
            kind("", None, &["image/png", "image/png"]),
            ContentKind::Files
        );
        // Files win over any text flavour that came with them.
        assert_eq!(kind("photo.png", None, &["image/png"]), ContentKind::Image);
    }

    #[test]
    fn secret_heuristic_flags_password_like_words() {
        assert!(looks_like_secret("Tr0ub4dor&3"));
        assert!(looks_like_secret("p@ssw0rd"));
        assert!(looks_like_secret("  ghp_AbCdEf0123456789xyz  "));
        assert!(looks_like_secret("kJ8#mQ2$vL9!"));
    }

    #[test]
    fn secret_heuristic_ignores_ordinary_content() {
        assert!(!looks_like_secret("bonjour"));
        assert!(!looks_like_secret("Password"));
        assert!(!looks_like_secret("correct horse battery staple"));
        assert!(!looks_like_secret("Hello everyone 42!"));
        assert!(!looks_like_secret("https://example.org/a?b=1&c=2"));
        assert!(!looks_like_secret(r"C:\Users\alice\file.txt"));
        assert!(!looks_like_secret("123e4567-e89b-12d3-a456-426614174000"));
        assert!(!looks_like_secret("1234567890"));
        assert!(!looks_like_secret("a1!"));
        assert!(!looks_like_secret(""));
    }

    #[test]
    fn secret_heuristic_is_bounded() {
        let long = format!("Aa1!{}", "x".repeat(200));
        assert!(!looks_like_secret(&long));
    }
}
