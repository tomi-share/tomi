//! Validation of what the user composed, and conversion into the core's [`ItemDraft`].

use serde::Deserialize;

use crate::deeplink::MAX_URL_LEN;
use crate::detect;
use crate::error::{AppError, AppResult};
use crate::expiry::{self, ExpiryChoice};
use crate::limits::{self, MAX_FILE_BYTES, MAX_ITEM_BYTES};
use crate::model::{Attachment, ItemDraft, SourceFormat, TextBody, REDACTED};

pub const MAX_TITLE_CHARS: usize = 300;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextInput {
    pub plain: String,
    #[serde(default)]
    pub html: Option<String>,
    #[serde(default)]
    pub markdown: Option<String>,
}

impl std::fmt::Debug for TextInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextInput")
            .field("plain", &REDACTED)
            .field("has_html", &self.html.is_some())
            .field("has_markdown", &self.markdown.is_some())
            .finish()
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftInput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub text: Option<TextInput>,
    /// Attachments as described by Rust (`describe_files` / `stage_blob`).
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Hidden ("masqué") item: its title, text, link and file names are part of the
    /// secret. The value to hide is the text.
    #[serde(default)]
    pub secret: bool,
    pub expiry: ExpiryChoice,
    #[serde(default)]
    pub source_format: Option<SourceFormat>,
}

// The input holds the text, which may be a secret value: never print it.
impl std::fmt::Debug for DraftInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftInput")
            .field("secret", &self.secret)
            .field("has_title", &self.title.is_some())
            .field("has_url", &self.url.is_some())
            .field("text", &self.text)
            .field("attachments", &self.attachments.len())
            .field("expiry", &self.expiry)
            .finish_non_exhaustive()
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Length of `s` once JSON-encoded (quotes and escapes included).
fn json_len(s: &str) -> usize {
    serde_json::to_string(s)
        .map(|j| j.len())
        .unwrap_or_else(|_| s.len() * 6 + 2)
}

/// Fixed part of the payload: braces, the keys, `id` (36), `source_device` (32),
/// two timestamps, `read_policy`, `secret`. Rounded up.
const PAYLOAD_BASE: usize = 320;
/// The `text` object without its strings: keys, `source_format`.
const TEXT_OVERHEAD: usize = 100;
/// One attachment without its strings: keys, `blob_id` (36), `size`, `key` (44 base64),
/// `nonce_prefix` (28 base64). Rounded up.
const ATTACHMENT_OVERHEAD: usize = 256;
/// Thumbnail object keys and mime.
const THUMBNAIL_OVERHEAD: usize = 60;

/// Upper bound of the serialized payload (`spec/format-v1.md` section 5.4) of `d`, before
/// padding. Everything but file bodies counts: the title, the link, every text flavour
/// and the metadata (and thumbnails) of the attachments.
pub fn estimate_payload_bytes(d: &ItemDraft) -> usize {
    let mut n = PAYLOAD_BASE;
    if let Some(t) = &d.title {
        n += 9 + json_len(t);
    }
    if let Some(u) = &d.url {
        n += 7 + json_len(u);
    }
    if let Some(t) = &d.text {
        n += TEXT_OVERHEAD + json_len(&t.plain);
        n += t.html.as_deref().map_or(0, json_len);
        n += t.markdown.as_deref().map_or(0, json_len);
    }
    for a in &d.attachments {
        n += ATTACHMENT_OVERHEAD + json_len(&a.name) + json_len(&a.mime);
        // `thumbnail` is a `data:` URL; the payload holds its base64 part. Counting the
        // whole URL only overestimates by a few bytes.
        n += a
            .thumbnail
            .as_deref()
            .map_or(0, |t| THUMBNAIL_OVERHEAD + t.len());
    }
    n
}

/// All the thumbnails of one item, in bytes (base64 adds a third): well under the ~1 MB the
/// relay takes for an item. Same budget as Android (`THUMBNAILS_BUDGET_BYTES`).
const THUMBNAILS_BUDGET_BYTES: usize = 384 * 1024;

/// Keeps the thumbnails, in order, while together they stay under [`THUMBNAILS_BUDGET_BYTES`];
/// the next ones are left out (the card then shows the file name only). A hidden item's files
/// never carry one: the file name and look of a secret's attachment is part of the secret.
fn budget_thumbnails(d: &mut ItemDraft) {
    let mut budget = THUMBNAILS_BUDGET_BYTES;
    for a in &mut d.attachments {
        if d.secret {
            a.thumbnail = None;
            continue;
        }
        // `data:<mime>;base64,<data>`: the data is ~3/4 of the URL.
        let bytes = a.thumbnail.as_deref().map_or(0, |t| t.len() / 4 * 3);
        if bytes > budget {
            a.thumbnail = None;
        } else {
            budget -= bytes;
        }
    }
}

/// Makes the draft fit the payload budget, or refuses it. Optional material goes first,
/// largest first: the HTML flavour, the Markdown flavour, then the thumbnails. The text,
/// the title and the link are never cut.
fn fit_payload(d: &mut ItemDraft) -> AppResult<()> {
    let fits = |d: &ItemDraft| limits::payload_fits(estimate_payload_bytes(d));
    if fits(d) {
        return Ok(());
    }
    if let Some(t) = &mut d.text {
        t.html = None;
    }
    if fits(d) {
        return Ok(());
    }
    if let Some(t) = &mut d.text {
        t.markdown = None;
    }
    if fits(d) {
        return Ok(());
    }
    for a in &mut d.attachments {
        a.thumbnail = None;
    }
    if fits(d) {
        return Ok(());
    }
    Err(AppError::new("error.payload_too_large"))
}

/// Validates `input` and builds the draft. `attachments` must already carry the real
/// on-disk sizes (the command layer re-reads them; the front end is not trusted).
///
/// The whole serialized payload is estimated here, at the click, so that an oversized
/// item is refused immediately and never fails after the cancellation delay.
pub fn build_draft(input: DraftInput, now_ms: i64) -> AppResult<ItemDraft> {
    let title = non_empty(input.title);
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > MAX_TITLE_CHARS)
    {
        return Err(AppError::new("error.title_too_long"));
    }

    let url = non_empty(input.url);
    if let Some(u) = &url {
        if u.len() > MAX_URL_LEN || detect::single_url(u).is_none() {
            return Err(AppError::new("error.invalid_url"));
        }
    }

    let mut text_body = input
        .text
        .filter(|t| !t.plain.trim().is_empty())
        .map(|t| TextBody {
            // The composer sends the plain text again as Markdown when the text is
            // Markdown. `source_format` already says so: sending it twice would only
            // double the payload.
            markdown: t.markdown.filter(|m| *m != t.plain),
            plain: t.plain,
            html: t.html,
        });

    // Attachments: per-file and per-item limits, checked again on real sizes.
    let mut total = 0u64;
    for a in &input.attachments {
        if a.size > MAX_FILE_BYTES {
            return Err(AppError::with(
                "error.file_too_large",
                serde_json::json!({ "name": a.name }),
            ));
        }
        total += a.size;
    }
    if total > MAX_ITEM_BYTES || input.attachments.len() > limits::MAX_FILES_PER_ITEM {
        return Err(AppError::new("error.item_too_large"));
    }

    let mut source_format = text_body.as_ref().and(input.source_format);
    // Markdown is its own source: an HTML flavour sent next to it would win on the receivers
    // (Android and Windows preview the HTML first when `markdown` is omitted) and show the raw
    // `# Title` / `- item` lines laid out as HTML instead of rendering them.
    if source_format == Some(SourceFormat::Markdown) {
        if let Some(t) = &mut text_body {
            t.html = None;
        }
    }
    // A hidden item is its plain text: no rich flavour (it would be a second, unmasked
    // copy of the value in the preview of a renderer) and no source format.
    if input.secret {
        if let Some(t) = &mut text_body {
            t.html = None;
            t.markdown = None;
        }
        source_format = None;
    }

    if title.is_none() && url.is_none() && text_body.is_none() && input.attachments.is_empty() {
        return Err(AppError::new("error.empty_draft"));
    }

    // "Après lecture" is only honoured for a hidden item (see `expiry`), and not when it carries
    // files: the first read would erase them everywhere before they could be opened or saved
    // (same rule as Android). Refused, not silently turned into "Jamais".
    let has_files = !input.attachments.is_empty();
    if input.secret && has_files && input.expiry == ExpiryChoice::AfterFirstRead {
        return Err(AppError::new("error.after_read_with_files"));
    }
    let (expires_at, read_policy) = expiry::resolve(input.expiry, input.secret, has_files, now_ms);

    let mut draft = ItemDraft {
        id: None,
        title,
        text: text_body,
        url,
        attachments: input.attachments,
        secret: input.secret,
        expires_at,
        read_policy,
        source_format,
    };
    budget_thumbnails(&mut draft);
    fit_payload(&mut draft)?;
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReadPolicy;

    const NOW: i64 = 5_000_000;
    const MB: u64 = 1024 * 1024;

    fn input() -> DraftInput {
        DraftInput {
            title: None,
            url: None,
            text: Some(TextInput {
                plain: "bonjour".into(),
                html: None,
                markdown: None,
            }),
            attachments: vec![],
            secret: false,
            expiry: ExpiryChoice::Never,
            source_format: Some(SourceFormat::Plain),
        }
    }

    fn file(size: u64) -> Attachment {
        Attachment {
            name: "f.bin".into(),
            mime: "application/octet-stream".into(),
            size,
            path: Some("C:/x/f.bin".into()),
            thumbnail: None,
            available: true,
            slot: None,
        }
    }

    #[test]
    fn plain_text_never_expires_by_default() {
        let d = build_draft(input(), NOW).unwrap();
        assert_eq!(d.expires_at, None);
        assert_eq!(d.read_policy, ReadPolicy::Normal);
        assert_eq!(d.text.unwrap().plain, "bonjour");
        assert!(!d.secret);
    }

    #[test]
    fn an_explicit_duration_is_honoured() {
        let mut i = input();
        i.expiry = ExpiryChoice::Day7;
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.expires_at, Some(NOW + 7 * 86_400_000));
    }

    #[test]
    fn a_secret_may_never_expire() {
        let mut i = input();
        i.secret = true;
        i.expiry = ExpiryChoice::Never;
        let d = build_draft(i, NOW).unwrap();
        assert!(d.secret);
        assert_eq!(d.expires_at, None);
        assert_eq!(d.read_policy, ReadPolicy::Normal);
    }

    #[test]
    fn a_hidden_item_keeps_its_value_as_text_with_a_duration() {
        let mut i = input();
        i.secret = true;
        i.expiry = ExpiryChoice::Hour1;
        let d = build_draft(i, NOW).unwrap();
        assert!(d.secret);
        assert_eq!(d.text.unwrap().plain, "bonjour");
        assert_eq!(d.expires_at, Some(NOW + 3_600_000));
    }

    #[test]
    fn a_hidden_item_is_plain_text_only() {
        let mut i = input();
        i.secret = true;
        i.title = Some("Wi-Fi".into());
        i.source_format = Some(SourceFormat::Markdown);
        i.text = Some(TextInput {
            plain: "mot-de-passe".into(),
            html: Some("<b>mot-de-passe</b>".into()),
            markdown: Some("**mot-de-passe**".into()),
        });
        let d = build_draft(i, NOW).unwrap();
        let t = d.text.unwrap();
        assert_eq!((t.html, t.markdown), (None, None));
        assert_eq!(d.source_format, None);
        // The title travels with the item: it is encrypted with the rest (spec 5.4.5).
        assert_eq!(d.title.as_deref(), Some("Wi-Fi"));
    }

    #[test]
    fn a_hidden_item_read_once() {
        let mut i = input();
        i.secret = true;
        i.expiry = ExpiryChoice::AfterFirstRead;
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.read_policy, ReadPolicy::AfterFirstRead);
        assert_eq!(d.expires_at, None);
    }

    #[test]
    fn a_hidden_item_with_files_cannot_be_read_once() {
        let mut i = input();
        i.secret = true;
        i.expiry = ExpiryChoice::AfterFirstRead;
        i.attachments = vec![file(10)];
        assert_eq!(
            build_draft(i.clone(), NOW).unwrap_err().code,
            "error.after_read_with_files"
        );
        // Without the files it is fine, and with the files a duration or "Jamais" is.
        i.attachments = vec![];
        assert!(build_draft(i.clone(), NOW).is_ok());
        i.attachments = vec![file(10)];
        for ok in [
            ExpiryChoice::Never,
            ExpiryChoice::Hour1,
            ExpiryChoice::Day30,
        ] {
            i.expiry = ok;
            let d = build_draft(i.clone(), NOW).unwrap();
            assert_eq!(d.read_policy, ReadPolicy::Normal, "{ok:?}");
        }
        // A plain item with files never carried the choice anyway.
        let mut plain = input();
        plain.expiry = ExpiryChoice::AfterFirstRead;
        plain.attachments = vec![file(10)];
        assert_eq!(
            build_draft(plain, NOW).unwrap().read_policy,
            ReadPolicy::Normal
        );
    }

    #[test]
    fn after_first_read_is_ignored_for_a_plain_item() {
        // Opening the card of a plain item marks it read: honouring the choice would
        // erase the item as soon as it is seen (same rule as Android).
        let mut i = input();
        i.expiry = ExpiryChoice::AfterFirstRead;
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.read_policy, ReadPolicy::Normal);
        assert_eq!(d.expires_at, None);
    }

    #[test]
    fn a_hidden_link_or_file_alone_is_a_valid_item() {
        let mut i = input();
        i.secret = true;
        i.text = None;
        i.url = Some("https://example.org/prive".into());
        assert!(build_draft(i, NOW).unwrap().url.is_some());
        let mut i = input();
        i.secret = true;
        i.text = None;
        i.attachments = vec![file(10)];
        assert!(build_draft(i, NOW).unwrap().text.is_none());
    }

    #[test]
    fn empty_draft_is_refused() {
        let mut i = input();
        i.text = Some(TextInput {
            plain: "  \n".into(),
            html: None,
            markdown: None,
        });
        assert_eq!(build_draft(i, NOW).unwrap_err().code, "error.empty_draft");
    }

    #[test]
    fn file_and_item_limits_are_enforced() {
        let mut i = input();
        i.attachments = vec![file(25 * MB + 1)];
        assert_eq!(
            build_draft(i, NOW).unwrap_err().code,
            "error.file_too_large"
        );

        let mut i = input();
        i.attachments = vec![file(25 * MB); 5];
        assert_eq!(
            build_draft(i, NOW).unwrap_err().code,
            "error.item_too_large"
        );

        let mut i = input();
        i.attachments = vec![file(25 * MB); 4];
        assert!(build_draft(i, NOW).is_ok());
    }

    #[test]
    fn link_must_be_http_or_https() {
        let mut i = input();
        i.text = None;
        i.url = Some("javascript:alert(1)".into());
        assert_eq!(build_draft(i, NOW).unwrap_err().code, "error.invalid_url");
        let mut i = input();
        i.text = None;
        i.url = Some("https://example.org/x".into());
        i.title = Some("Titre".into());
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.url.as_deref(), Some("https://example.org/x"));
    }

    #[test]
    fn oversized_html_degrades_to_plain() {
        let mut i = input();
        i.text = Some(TextInput {
            plain: "a".into(),
            html: Some("x".repeat(limits::MAX_PAYLOAD_BYTES + 1)),
            markdown: None,
        });
        let d = build_draft(i, NOW).unwrap();
        assert!(d.text.unwrap().html.is_none());
    }

    #[test]
    fn files_only_item_is_valid() {
        let mut i = input();
        i.text = None;
        i.attachments = vec![file(10), file(20)];
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.attachments.len(), 2);
        assert!(d.source_format.is_none());
    }

    // ---- payload budget (1 MiB for the whole serialized payload) ----

    fn text_input(plain: String, markdown: Option<String>) -> DraftInput {
        let mut i = input();
        i.text = Some(TextInput {
            plain,
            html: None,
            markdown,
        });
        i
    }

    #[test]
    fn markdown_copy_of_the_plain_text_is_not_sent_twice() {
        // 600 KB sent as plain + the same 600 KB as Markdown would be 1.2 MB.
        let md = "# titre\n".to_string() + &"a".repeat(600_000);
        let mut i = text_input(md.clone(), Some(md.clone()));
        i.source_format = Some(SourceFormat::Markdown);
        let d = build_draft(i, NOW).unwrap();
        let t = d.text.unwrap();
        assert_eq!(t.plain, md);
        assert!(t.markdown.is_none(), "duplicate flavour must be dropped");
        assert_eq!(d.source_format, Some(SourceFormat::Markdown));
    }

    #[test]
    fn markdown_drops_the_html_flavour_so_receivers_render_the_markdown() {
        let md = "# titre\n\n- a\n- b";
        let mut i = text_input(md.into(), Some(md.into()));
        i.text.as_mut().unwrap().html = Some("<div># titre</div><div>- a</div>".into());
        i.source_format = Some(SourceFormat::Markdown);
        let t = build_draft(i, NOW).unwrap().text.unwrap();
        assert!(t.html.is_none());
        assert_eq!(t.plain, md);
    }

    #[test]
    fn a_distinct_flavour_that_blows_the_budget_is_dropped_not_the_text() {
        let i = text_input("a".repeat(600_000), Some("b".repeat(600_000)));
        let d = build_draft(i, NOW).unwrap();
        let t = d.text.unwrap();
        assert_eq!(t.plain.len(), 600_000);
        assert!(t.markdown.is_none());
    }

    #[test]
    fn plain_text_alone_over_the_budget_is_refused_immediately() {
        let i = text_input("a".repeat(limits::MAX_PAYLOAD_BYTES), None);
        assert_eq!(
            build_draft(i, NOW).unwrap_err().code,
            "error.payload_too_large"
        );
    }

    #[test]
    fn the_whole_payload_counts_not_only_the_text() {
        // Text just under the budget, but a long title and a link push it over.
        let mut i = text_input("a".repeat(limits::MAX_PAYLOAD_BYTES - 600), None);
        assert!(build_draft(i.clone(), NOW).is_ok(), "fits on its own");
        i.title = Some("t".repeat(MAX_TITLE_CHARS));
        i.url = Some(format!("https://example.org/{}", "u".repeat(1000)));
        assert_eq!(
            build_draft(i, NOW).unwrap_err().code,
            "error.payload_too_large"
        );
    }

    #[test]
    fn json_escaping_is_counted() {
        // Every `"` becomes `\"`: the JSON is twice the text.
        let i = text_input("\"".repeat(600_000), None);
        assert_eq!(
            build_draft(i, NOW).unwrap_err().code,
            "error.payload_too_large"
        );
    }

    fn images_with_thumbnails(count: usize, url_chars: usize) -> Vec<Attachment> {
        (0..count)
            .map(|n| Attachment {
                name: format!("img{n}.png"),
                mime: "image/png".into(),
                size: 10,
                path: Some(format!("C:/x/img{n}.png")),
                thumbnail: Some(format!("data:image/png;base64,{}", "A".repeat(url_chars))),
                available: true,
                slot: None,
            })
            .collect()
    }

    #[test]
    fn thumbnails_are_dropped_before_the_item_is_refused() {
        // The text alone fits, but not together with the thumbnails.
        let mut i = text_input("a".repeat(800_000), None);
        i.attachments = images_with_thumbnails(4, 80_000);
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.attachments.len(), 4);
        assert!(d.attachments.iter().all(|a| a.thumbnail.is_none()));
        assert_eq!(d.text.unwrap().plain.len(), 800_000);
    }

    #[test]
    fn the_thumbnails_of_one_item_share_a_budget_and_the_first_ones_win() {
        let mut i = input();
        i.text = None;
        // 20 x 60 000 bytes: only the first 6 fit in 384 KiB.
        i.attachments = images_with_thumbnails(20, 80_000);
        let d = build_draft(i, NOW).unwrap();
        assert_eq!(d.attachments.len(), 20);
        let kept = d
            .attachments
            .iter()
            .filter(|a| a.thumbnail.is_some())
            .count();
        assert_eq!(kept, 6);
        assert!(d.attachments[..6].iter().all(|a| a.thumbnail.is_some()));
    }

    #[test]
    fn a_hidden_item_carries_no_thumbnail() {
        let mut i = input();
        i.secret = true;
        i.attachments = images_with_thumbnails(2, 1_000);
        let d = build_draft(i, NOW).unwrap();
        assert!(d.attachments.iter().all(|a| a.thumbnail.is_none()));
    }

    #[test]
    fn the_estimate_is_not_below_the_real_serialization_of_a_typical_item() {
        let d = build_draft(text_input("bonjour".into(), None), NOW).unwrap();
        // A real payload for this item is about 250 bytes.
        let est = estimate_payload_bytes(&d);
        assert!((250..2048).contains(&est), "{est}");
    }

    #[test]
    fn debug_of_the_input_hides_the_secret_values() {
        let mut i = input();
        i.secret = true;
        i.title = Some("titre-prive".into());
        i.text = Some(TextInput {
            plain: "texte-prive".into(),
            html: None,
            markdown: None,
        });
        let dump = format!("{i:?}");
        assert!(!dump.contains("texte-prive"));
        assert!(!dump.contains("titre-prive"));
    }
}
