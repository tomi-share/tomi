//! Rich-text rendering: "Aperçu" / "Markdown", like Android (`ui/render/RenderMode.kt`).
//!
//! * [`RenderMode::Preview`] ("Aperçu", the default) is the styled rendering. What it is made of
//!   ([`PreviewKind`]) depends on the format the text was captured in: Markdown goes through
//!   `pulldown-cmark`, HTML through the sanitiser, anything else is shown as text.
//! * [`RenderMode::Source`] ("Markdown") is the Markdown source, literally. It only exists for
//!   an item that carries a Markdown representation ([`available_modes`]).
//!
//! HTML is never shown as source: there is no mode in which markup is displayed as text.
//!
//! HTML (received or derived from Markdown) is always sanitised here, in Rust, with
//! `ammonia`, and then wrapped in a document carrying a strict CSP. The front end shows
//! that document in an `<iframe sandbox>` without scripts and without network access
//! (invariant 6).

use std::borrow::Cow;
use std::collections::HashSet;

use pulldown_cmark::{html, Options, Parser};
use serde::{Deserialize, Serialize};

use crate::model::{SourceFormat, TextBody};

pub const DOC_CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderMode {
    /// "Aperçu": the styled rendering. The three old names (`markdown`, `html`, `plain`) of a
    /// `settings.json` written by a previous version are read as this one, so that an old file
    /// is not thrown away because of a mode that no longer exists.
    #[serde(alias = "markdown", alias = "html", alias = "plain")]
    Preview,
    /// "Markdown": the source of the text, literally.
    Source,
}

/// The renderer behind [`RenderMode::Preview`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Html,
    Plain,
}

/// Modes a card can offer, in display order. "Markdown" only exists for an item that has a
/// Markdown representation.
pub fn available_modes(text: &TextBody, format: Option<SourceFormat>) -> Vec<RenderMode> {
    let mut modes = vec![RenderMode::Preview];
    // A sender may omit `markdown` when it equals the plain text and only declare the format.
    if text.markdown.is_some() || format == Some(SourceFormat::Markdown) {
        modes.push(RenderMode::Source);
    }
    modes
}

/// The mode asked for if this item supports it, otherwise the styled preview.
pub fn resolve(
    text: &TextBody,
    format: Option<SourceFormat>,
    preferred: Option<RenderMode>,
) -> RenderMode {
    preferred
        .filter(|m| available_modes(text, format).contains(m))
        .unwrap_or(RenderMode::Preview)
}

/// What the preview of `text` is made of: the format the content was captured in, with the
/// other representations as fallback (same order as Android).
pub fn preview_kind(text: &TextBody, format: Option<SourceFormat>) -> PreviewKind {
    let markdown_first = || {
        if text.markdown.is_some() {
            PreviewKind::Markdown
        } else if text.html.is_some() {
            PreviewKind::Html
        } else {
            PreviewKind::Plain
        }
    };
    let html_first = || {
        if text.html.is_some() {
            PreviewKind::Html
        } else if text.markdown.is_some() {
            PreviewKind::Markdown
        } else {
            PreviewKind::Plain
        }
    };
    match format {
        // Declared Markdown without a separate field: the plain text is the Markdown.
        Some(SourceFormat::Markdown) => match markdown_first() {
            PreviewKind::Plain => PreviewKind::Markdown,
            other => other,
        },
        Some(SourceFormat::Html) => html_first(),
        Some(SourceFormat::Plain) => PreviewKind::Plain,
        // No declared format: whatever rich flavour the text carries.
        None => markdown_first(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Rendered {
    /// Shown as text by the front end (monospace for the Markdown source).
    Plain { text: String },
    /// A complete, sanitised HTML document for a sandboxed iframe (`srcdoc`).
    Document { srcdoc: String },
}

fn is_safe_data_image(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    ["png", "jpeg", "jpg", "gif", "webp", "bmp"]
        .iter()
        .any(|t| v.starts_with(&format!("data:image/{t};base64,")))
}

fn is_safe_href(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    v.starts_with("http://")
        || v.starts_with("https://")
        || v.starts_with("mailto:")
        || v.starts_with('#')
}

/// Sanitises an HTML fragment: no scripts, no event handlers, no inline styles,
/// links limited to http/https/mailto, images limited to inline `data:` rasters.
pub fn sanitize_html(input: &str) -> String {
    let schemes: HashSet<&'static str> = ["http", "https", "mailto", "data"].into_iter().collect();
    ammonia::Builder::default()
        .url_schemes(schemes)
        .attribute_filter(|element, attribute, value| match (element, attribute) {
            ("img", "src") if !is_safe_data_image(value) => None,
            ("a", "href") if !is_safe_href(value) => None,
            _ => Some(Cow::Borrowed(value)),
        })
        .clean(input)
        .to_string()
}

pub fn markdown_to_html(md: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, opts));
    out
}

/// Colours of the document, from `design/tomi/tokens.json` (text, brand, surface-alt, border).
struct DocPalette {
    scheme: &'static str,
    text: &'static str,
    link: &'static str,
    code_bg: &'static str,
    border: &'static str,
}

const LIGHT: DocPalette = DocPalette {
    scheme: "light",
    text: "#1B2130",
    link: "#2E4C80",
    code_bg: "#ECE9E2",
    border: "#D5D1C7",
};
const DARK: DocPalette = DocPalette {
    scheme: "dark",
    text: "#E9E7E1",
    link: "#A9BFEA",
    code_bg: "#242A38",
    border: "#383E4C",
};

/// Wraps an *already sanitised* fragment in the final document. `dark` follows the theme of
/// the window (the document cannot read it: it is sandboxed).
pub fn wrap_document(sanitized_body: &str, dark: bool) -> String {
    let p = if dark { DARK } else { LIGHT };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{DOC_CSP}\">\
<style>:root{{color-scheme:{scheme}}}\
body{{margin:0;padding:2px 2px 8px;background:transparent;color:{text};\
font:15px/1.5 \"Inter\",\"Segoe UI Variable Text\",\"Segoe UI\",system-ui,sans-serif;overflow-wrap:anywhere}}\
body>:first-child{{margin-top:0}}\
img{{max-width:100%;height:auto}}\
pre,code{{font-family:\"JetBrains Mono\",\"Cascadia Mono\",Consolas,monospace;font-size:13px}}\
code{{background:{code_bg};border-radius:4px;padding:1px 4px}}\
pre{{white-space:pre-wrap;background:{code_bg};border-radius:4px;padding:8px 10px}}\
pre code{{background:none;padding:0}}\
table{{border-collapse:collapse}}\
td,th{{border:1px solid {border};padding:2px 6px}}\
blockquote{{margin:0 0 0 4px;padding-left:10px;border-left:3px solid {border}}}\
a{{color:{link}}}</style></head><body>{sanitized_body}</body></html>",
        scheme = p.scheme,
        text = p.text,
        code_bg = p.code_bg,
        border = p.border,
        link = p.link,
    )
}

/// The sanitised HTML of the item: its own HTML when it has one, otherwise the Markdown.
/// Used for the "Aperçu" of an HTML text and for the HTML flavour of the clipboard.
pub fn html_view(text: &TextBody) -> String {
    match &text.html {
        Some(h) => sanitize_html(h),
        None => sanitize_html(&markdown_to_html(
            text.markdown.as_deref().unwrap_or(&text.plain),
        )),
    }
}

pub fn render(
    text: &TextBody,
    format: Option<SourceFormat>,
    mode: RenderMode,
    dark: bool,
) -> Rendered {
    match mode {
        RenderMode::Source => Rendered::Plain {
            text: text.markdown.clone().unwrap_or_else(|| text.plain.clone()),
        },
        RenderMode::Preview => match preview_kind(text, format) {
            PreviewKind::Plain => Rendered::Plain {
                text: text.plain.clone(),
            },
            PreviewKind::Markdown => {
                let md = text.markdown.as_deref().unwrap_or(&text.plain);
                Rendered::Document {
                    srcdoc: wrap_document(&sanitize_html(&markdown_to_html(md)), dark),
                }
            }
            PreviewKind::Html => Rendered::Document {
                srcdoc: wrap_document(&html_view(text), dark),
            },
        },
    }
}

/// What "Copier" must place in the clipboard for the displayed mode, `(plain, html)`: exactly
/// what the card shows. The styled HTML travels with its plain text; a Markdown or plain
/// preview copies the plain text; the Markdown source copies the source.
pub fn copy_payload(
    text: &TextBody,
    format: Option<SourceFormat>,
    mode: RenderMode,
) -> (String, Option<String>) {
    match mode {
        RenderMode::Source => (
            text.markdown.clone().unwrap_or_else(|| text.plain.clone()),
            None,
        ),
        RenderMode::Preview => {
            let html = (preview_kind(text, format) == PreviewKind::Html).then(|| html_view(text));
            (text.plain.clone(), html)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(plain: &str, html: Option<&str>, md: Option<&str>) -> TextBody {
        TextBody {
            plain: plain.into(),
            html: html.map(Into::into),
            markdown: md.map(Into::into),
        }
    }

    #[test]
    fn scripts_and_handlers_are_removed() {
        let out = sanitize_html(
            "<p onclick=\"x()\">a</p><script>alert(1)</script><img src=x onerror=alert(1)>\
<iframe src=\"https://evil\"></iframe><style>p{}</style><form action=\"x\"><input></form>",
        );
        for bad in [
            "script", "onclick", "onerror", "iframe", "<style", "<form", "<input", "alert",
        ] {
            assert!(!out.contains(bad), "{bad} survived in {out}");
        }
        assert!(out.contains("<p>a</p>"));
    }

    #[test]
    fn dangerous_link_schemes_are_removed() {
        let out = sanitize_html(
            "<a href=\"javascript:alert(1)\">x</a><a href=\"data:text/html,hi\">y</a>\
<a href=\"file:///C:/x\">z</a><a href=\"https://ok.org\">ok</a>",
        );
        assert!(!out.contains("javascript"));
        assert!(!out.contains("data:"));
        assert!(!out.contains("file:"));
        assert!(out.contains("href=\"https://ok.org\""));
    }

    #[test]
    fn remote_images_are_removed_but_inline_rasters_stay() {
        let out = sanitize_html(
            "<img src=\"https://tracker.example/p.png\"><img src=\"data:image/png;base64,AAAA\">\
<img src=\"data:image/svg+xml;base64,AAAA\">",
        );
        assert!(!out.contains("tracker"));
        assert!(!out.contains("svg"));
        assert!(out.contains("data:image/png;base64,AAAA"));
    }

    #[test]
    fn inline_styles_are_removed() {
        let out = sanitize_html("<p style=\"background:url(https://x/y)\">a</p>");
        assert!(!out.contains("style"));
        assert!(!out.contains("url("));
    }

    #[test]
    fn raw_html_inside_markdown_is_sanitised() {
        let out = render(
            &body(
                "x",
                None,
                Some("# T\n\n<script>alert(1)</script>\n\n[a](javascript:alert(1))"),
            ),
            Some(SourceFormat::Markdown),
            RenderMode::Preview,
            false,
        );
        let Rendered::Document { srcdoc } = out else {
            panic!("expected a document");
        };
        assert!(srcdoc.contains("<h1>T</h1>"));
        assert!(!srcdoc.contains("<script"));
        assert!(!srcdoc.contains("javascript:"));
    }

    #[test]
    fn document_carries_the_strict_csp() {
        let Rendered::Document { srcdoc } = render(
            &body("a", Some("<b>a</b>"), None),
            Some(SourceFormat::Html),
            RenderMode::Preview,
            false,
        ) else {
            panic!("expected a document");
        };
        assert!(srcdoc
            .contains("content=\"default-src 'none'; img-src data:; style-src 'unsafe-inline'\""));
        assert!(srcdoc.contains("<b>a</b>"));
    }

    #[test]
    fn the_document_follows_the_theme() {
        let light = wrap_document("<p>a</p>", false);
        let dark = wrap_document("<p>a</p>", true);
        assert!(light.contains("color-scheme:light") && light.contains("#1B2130"));
        assert!(dark.contains("color-scheme:dark") && dark.contains("#E9E7E1"));
    }

    #[test]
    fn plain_preview_is_untouched_text() {
        let r = render(
            &body("<b>a</b>", None, None),
            Some(SourceFormat::Plain),
            RenderMode::Preview,
            false,
        );
        assert_eq!(
            r,
            Rendered::Plain {
                text: "<b>a</b>".into()
            }
        );
    }

    #[test]
    fn html_is_never_shown_as_source() {
        // An HTML text has no "Markdown" mode, and asking for it falls back to the preview.
        let t = body("brut", Some("<b>riche</b>"), None);
        assert_eq!(
            available_modes(&t, Some(SourceFormat::Html)),
            vec![RenderMode::Preview]
        );
        assert_eq!(
            resolve(&t, Some(SourceFormat::Html), Some(RenderMode::Source)),
            RenderMode::Preview
        );
        // Even when the source mode is forced, the markup is not what comes out.
        let Rendered::Plain { text } =
            render(&t, Some(SourceFormat::Html), RenderMode::Source, false)
        else {
            panic!("the source is text");
        };
        assert!(!text.contains("<b>"));
    }

    #[test]
    fn markdown_source_is_literal_and_only_offered_with_markdown() {
        let t = body("# T", None, Some("# T\n- a"));
        assert_eq!(
            available_modes(&t, Some(SourceFormat::Markdown)),
            vec![RenderMode::Preview, RenderMode::Source]
        );
        assert_eq!(
            resolve(&t, Some(SourceFormat::Markdown), Some(RenderMode::Source)),
            RenderMode::Source
        );
        assert_eq!(
            resolve(&t, Some(SourceFormat::Markdown), None),
            RenderMode::Preview
        );
        let r = render(&t, Some(SourceFormat::Markdown), RenderMode::Source, false);
        assert_eq!(
            r,
            Rendered::Plain {
                text: "# T\n- a".into()
            }
        );
    }

    #[test]
    fn preview_kind_follows_the_source_format_with_fallbacks() {
        let both = body("p", Some("<b>h</b>"), Some("**m**"));
        assert_eq!(
            preview_kind(&both, Some(SourceFormat::Markdown)),
            PreviewKind::Markdown
        );
        assert_eq!(
            preview_kind(&both, Some(SourceFormat::Html)),
            PreviewKind::Html
        );
        assert_eq!(
            preview_kind(&both, Some(SourceFormat::Plain)),
            PreviewKind::Plain
        );
        let only_html = body("p", Some("<b>h</b>"), None);
        assert_eq!(
            preview_kind(&only_html, Some(SourceFormat::Markdown)),
            PreviewKind::Html
        );
        // Declared Markdown with no separate field: the plain text is the Markdown.
        let nothing = body("p", None, None);
        assert_eq!(
            preview_kind(&nothing, Some(SourceFormat::Markdown)),
            PreviewKind::Markdown
        );
        assert_eq!(
            available_modes(&nothing, Some(SourceFormat::Markdown)),
            vec![RenderMode::Preview, RenderMode::Source]
        );
        assert_eq!(preview_kind(&nothing, None), PreviewKind::Plain);
    }

    #[test]
    fn copy_payload_follows_the_displayed_mode() {
        let t = body("brut", Some("<b>riche</b>"), Some("**md**"));
        // Styled HTML travels with its plain text.
        let (plain, html) = copy_payload(&t, Some(SourceFormat::Html), RenderMode::Preview);
        assert_eq!(plain, "brut");
        assert_eq!(html.as_deref(), Some("<b>riche</b>"));
        // A Markdown preview copies the plain text.
        assert_eq!(
            copy_payload(&t, Some(SourceFormat::Markdown), RenderMode::Preview),
            ("brut".into(), None)
        );
        // The source copies the source.
        assert_eq!(
            copy_payload(&t, Some(SourceFormat::Markdown), RenderMode::Source),
            ("**md**".into(), None)
        );
    }

    #[test]
    fn the_old_render_modes_of_a_settings_file_are_still_read() {
        for old in ["markdown", "html", "plain", "preview"] {
            let m: RenderMode = serde_json::from_str(&format!("\"{old}\"")).unwrap();
            assert_eq!(m, RenderMode::Preview, "{old}");
        }
        let m: RenderMode = serde_json::from_str("\"source\"").unwrap();
        assert_eq!(m, RenderMode::Source);
        assert_eq!(
            serde_json::to_string(&RenderMode::Preview).unwrap(),
            "\"preview\""
        );
    }
}
