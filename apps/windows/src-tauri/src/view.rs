//! What the webview gets to see of an item.
//!
//! A hidden ("masqué") item never crosses to the webview in a list or notification
//! payload: no title, no preview, no link, no file name (only how many files there are).
//! Everything is fetched on explicit action (`reveal_secret`), or copied entirely inside
//! Rust (`copy_secret`). The title of a hidden item is part of the secret
//! (`spec/format-v1.md` §5.4.5): it is not shown without an explicit reveal, and not
//! searched.

use serde::{Deserialize, Serialize};

use crate::detect::ContentKind;
use crate::model::{Delivery, DeviceRef, Item, ReadPolicy, SourceFormat, UnreadableReason};

pub const PREVIEW_CHARS: usize = 400;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentView {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub thumbnail: Option<String>,
    /// `false` when the file cannot be fetched any more.
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub id: String,
    pub kind: ContentKind,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub url: Option<String>,
    /// Has an HTML or Markdown flavour.
    pub rich: bool,
    /// Has a Markdown representation: the card then offers the "Aperçu" / "Markdown" switch.
    pub has_markdown: bool,
    /// The text is longer than preview: the card cuts it and offers "Lire la suite".
    pub preview_truncated: bool,
    pub source_format: Option<SourceFormat>,
    /// Empty for a hidden item: see `attachment_count`.
    pub attachments: Vec<AttachmentView>,
    /// Number of files, also for a hidden item (whose file names stay hidden).
    pub attachment_count: usize,
    pub secret: bool,
    pub source_device: DeviceRef,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub read_policy: ReadPolicy,
    pub read: bool,
    pub outgoing: bool,
    pub delivery: Vec<Delivery>,
    pub unreadable: bool,
    /// Why the item is unreadable: `"updateRequired"` (the sentence that says an update of Tomi
    /// will show it) or `"other"` (unreadable for now, a synchronisation or an update may help);
    /// `null` for a readable item. Always set when `unreadable` is `true`.
    pub unreadable_reason: Option<UnreadableReason>,
    /// Pinned on all devices; the UI builds the "Épinglés" section from it.
    pub pinned: bool,
    /// Time of the decision to pin (ms), to sort the section, most recent first.
    pub pinned_at: Option<i64>,
    /// Archived on all devices and not pinned (the pin wins): the item is only in the
    /// "Archives" filter, and in a search under "Tous" (badge "Archivé").
    pub archived: bool,
    /// Time of the decision to archive (ms), `null` when not (shown) archived.
    pub archived_at: Option<i64>,
}

pub fn item_kind(item: &Item) -> ContentKind {
    match item.attachments.as_slice() {
        [] => {}
        [a] => {
            return if a.mime.starts_with("image/") {
                ContentKind::Image
            } else {
                ContentKind::File
            }
        }
        _ => return ContentKind::Files,
    }
    if item.url.is_some() {
        return ContentKind::Link;
    }
    if is_rich(item) {
        return ContentKind::RichText;
    }
    ContentKind::Text
}

fn is_rich(item: &Item) -> bool {
    item.text.as_ref().is_some_and(|t| {
        t.html.is_some()
            || t.markdown.is_some()
            || matches!(
                item.source_format,
                Some(SourceFormat::Html | SourceFormat::Markdown)
            )
    })
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

impl ItemView {
    pub fn from_item(item: &Item) -> Self {
        let secret = item.secret;
        let preview = if secret || item.unreadable {
            None
        } else {
            item.text
                .as_ref()
                .map(|t| truncate_chars(t.plain.trim(), PREVIEW_CHARS))
                .filter(|p| !p.is_empty())
        };
        Self {
            id: item.id.clone(),
            // The kind of a hidden item would tell what it holds: it is always "masqué".
            kind: if secret {
                ContentKind::Text
            } else {
                item_kind(item)
            },
            title: if secret { None } else { item.title.clone() },
            preview,
            preview_truncated: !secret
                && !item.unreadable
                && item
                    .text
                    .as_ref()
                    .is_some_and(|t| t.plain.trim().chars().count() > PREVIEW_CHARS),
            url: if secret { None } else { item.url.clone() },
            rich: !secret && is_rich(item),
            has_markdown: !secret && item.text.as_ref().is_some_and(|t| t.markdown.is_some()),
            source_format: if secret { None } else { item.source_format },
            attachments: if secret {
                Vec::new()
            } else {
                item.attachments
                    .iter()
                    .map(|a| AttachmentView {
                        name: a.name.clone(),
                        mime: a.mime.clone(),
                        size: a.size,
                        thumbnail: a.thumbnail.clone(),
                        available: a.available,
                    })
                    .collect()
            },
            attachment_count: item.attachments.len(),
            secret,
            source_device: item.source_device.clone(),
            created_at: item.created_at,
            expires_at: item.expires_at,
            read_policy: item.read_policy,
            read: item.read,
            outgoing: item.outgoing,
            delivery: item.delivery.clone(),
            unreadable: item.unreadable,
            // An unreadable item always has a reason (never `null` on the page): a core that
            // gave none counts as "other".
            unreadable_reason: item
                .unreadable
                .then(|| item.unreadable_reason.unwrap_or(UnreadableReason::Other)),
            pinned: item.pinned,
            pinned_at: item.pinned_at,
            archived: item.is_shown_archived(),
            archived_at: item
                .is_shown_archived()
                .then_some(item.archived_at)
                .flatten(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ItemFilter {
    #[default]
    All,
    Links,
    Text,
    Files,
    Secrets,
    /// The only filter that shows archived items; they are absent from every other one.
    Archives,
}

/// Archived items belong to [`ItemFilter::Archives`] only; every other filter ("Tous"
/// included) leaves them out, so they are neither in "Récents" nor in the typed filters
/// (`FeedFiltering.matches`). A pinned item is never shown archived.
pub fn matches_filter(item: &Item, filter: ItemFilter) -> bool {
    if filter == ItemFilter::Archives {
        return item.is_shown_archived();
    }
    if item.is_shown_archived() {
        return false;
    }
    match filter {
        ItemFilter::Archives => false,
        ItemFilter::All => true,
        ItemFilter::Secrets => item.secret,
        ItemFilter::Links => !item.secret && item.url.is_some(),
        ItemFilter::Files => !item.secret && !item.attachments.is_empty(),
        ItemFilter::Text => {
            !item.secret && item.attachments.is_empty() && item.url.is_none() && item.text.is_some()
        }
    }
}

/// Local search (case-insensitive). A hidden item is searched by its source device name
/// only: neither its title nor its content (text, link, file names) is ever searched, so
/// the search field cannot be used to guess or to confirm anything about it. Same rule as
/// Android (`FeedFiltering.kt`).
pub fn matches_query(item: &Item, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let has = |s: &str| s.to_lowercase().contains(&q);
    if has(&item.source_device.name) {
        return true;
    }
    if item.secret {
        return false;
    }
    item.title.as_deref().is_some_and(has)
        || item.url.as_deref().is_some_and(has)
        || item.text.as_ref().is_some_and(|t| has(&t.plain))
        || item.attachments.iter().any(|a| has(&a.name))
}

/// Filter then search (`FeedFiltering.apply`). An archived item is "findable": while the
/// user types a search under "Tous", archived items are searched too (the card carries the
/// "Archivé" badge). With an empty search they only show under "Archives". The search rules
/// of a hidden item stay those of [`matches_query`].
pub fn is_listed(item: &Item, filter: ItemFilter, query: &str) -> bool {
    let searching_everywhere = filter == ItemFilter::All && !query.trim().is_empty();
    (matches_filter(item, filter) || (searching_everywhere && item.is_shown_archived()))
        && matches_query(item, query)
}

/// Number of unread items received from other devices, as the tray announces it. An item that
/// came with a history transfer is not "new": it counts only if the other device had not read
/// it either. Two kinds never count, because nothing on screen could ever clear them: an item
/// the user archived (it is no longer in the feed, so it is never seen) and an unreadable one
/// (a card with no content to read). Archiving still does not mark read: the item keeps its
/// dot under "Archives".
pub fn unread_count(items: &[Item]) -> usize {
    items
        .iter()
        .filter(|i| !i.read && !i.outgoing && !i.is_shown_archived() && !i.unreadable)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, TextBody};

    pub(crate) fn base() -> Item {
        Item {
            id: "i1".into(),
            title: None,
            text: None,
            url: None,
            attachments: vec![],
            secret: false,
            source_device: DeviceRef {
                id: "d".into(),
                name: "Pixel".into(),
            },
            created_at: 1,
            expires_at: None,
            read_policy: ReadPolicy::Normal,
            read: false,
            outgoing: false,
            imported: false,
            delivery: vec![],
            source_format: None,
            unreadable: false,
            unreadable_reason: None,
            pinned: false,
            pinned_at: None,
            archived: false,
            archived_at: None,
        }
    }

    fn archived(mut i: Item) -> Item {
        i.archived = true;
        i.archived_at = Some(1_700_000_000_000);
        i
    }

    fn attachment(name: &str, mime: &str) -> Attachment {
        Attachment {
            name: name.into(),
            mime: mime.into(),
            size: 1,
            path: None,
            thumbnail: None,
            available: true,
            slot: None,
        }
    }

    fn text_item(t: &str) -> Item {
        let mut i = base();
        i.text = Some(TextBody {
            plain: t.into(),
            html: None,
            markdown: None,
        });
        i
    }

    fn secret_item() -> Item {
        let mut i = text_item("Tr0ub4dor&3");
        i.secret = true;
        i.title = Some("banque".into());
        i.url = Some("https://banque.example/login".into());
        i
    }

    #[test]
    fn a_hidden_item_reveals_nothing_to_the_feed() {
        let mut i = secret_item();
        i.attachments = vec![attachment("releve.png", "image/png")];
        i.attachments[0].thumbnail = Some("data:image/png;base64,AAA".into());
        i.source_format = Some(SourceFormat::Markdown);
        let view = ItemView::from_item(&i);
        let json = serde_json::to_string(&view).unwrap();
        for leaked in ["Tr0ub4dor", "banque", "releve", "base64", "markdown"] {
            assert!(!json.contains(leaked), "{leaked} leaked: {json}");
        }
        assert!(view.title.is_none() && view.preview.is_none() && view.url.is_none());
        assert!(view.attachments.is_empty());
        assert_eq!(view.attachment_count, 1, "only how many files");
        assert!(view.secret && !view.rich);
        assert_eq!(view.kind, ContentKind::Text, "no hint about what it holds");
    }

    #[test]
    fn the_title_of_a_hidden_item_is_never_in_the_view_but_a_plain_title_is() {
        let mut plain = text_item("hello");
        plain.title = Some("Un titre".into());
        assert_eq!(
            ItemView::from_item(&plain).title.as_deref(),
            Some("Un titre")
        );
        let mut hidden = plain.clone();
        hidden.secret = true;
        assert_eq!(ItemView::from_item(&hidden).title, None);
    }

    #[test]
    fn a_plain_attachment_keeps_its_thumbnail_and_availability() {
        let mut i = base();
        let mut a = attachment("a.png", "image/png");
        a.thumbnail = Some("data:image/png;base64,AAA".into());
        a.available = false;
        i.attachments = vec![a];
        let v = ItemView::from_item(&i);
        assert_eq!(
            v.attachments[0].thumbnail.as_deref(),
            Some("data:image/png;base64,AAA")
        );
        assert!(!v.attachments[0].available);
        assert_eq!(v.attachment_count, 1);
    }

    #[test]
    fn the_pin_reaches_the_view() {
        let mut i = text_item("a");
        i.pinned = true;
        i.pinned_at = Some(1_700_000_000_000);
        let v = ItemView::from_item(&i);
        assert!(v.pinned);
        assert_eq!(v.pinned_at, Some(1_700_000_000_000));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["pinned"], true);
        assert_eq!(json["pinnedAt"], 1_700_000_000_000i64);
        assert!(!ItemView::from_item(&text_item("b")).pinned);
    }

    #[test]
    fn the_reason_of_an_unreadable_item_reaches_the_page_as_camel_case_or_null() {
        let readable = serde_json::to_value(ItemView::from_item(&text_item("a"))).unwrap();
        assert_eq!(readable["unreadable"], false);
        assert!(readable["unreadableReason"].is_null());
        assert!(
            readable
                .as_object()
                .unwrap()
                .contains_key("unreadableReason"),
            "present even when null"
        );

        let mut i = base();
        i.unreadable = true;
        i.unreadable_reason = Some(UnreadableReason::UpdateRequired);
        let json = serde_json::to_value(ItemView::from_item(&i)).unwrap();
        assert_eq!(json["unreadable"], true);
        assert_eq!(json["unreadableReason"], "updateRequired");

        i.unreadable_reason = Some(UnreadableReason::Other);
        let json = serde_json::to_value(ItemView::from_item(&i)).unwrap();
        assert_eq!(json["unreadableReason"], "other");

        // An unreadable item never reaches the page without a reason.
        i.unreadable_reason = None;
        let json = serde_json::to_value(ItemView::from_item(&i)).unwrap();
        assert_eq!(json["unreadableReason"], "other");
        // And nothing of the item is in the view.
        let v = ItemView::from_item(&i);
        assert!(v.preview.is_none() && v.title.is_none() && v.url.is_none());
    }

    #[test]
    fn a_pin_time_up_to_2_pow_53_survives_the_json_round_trip() {
        // 2^53 - 1: the largest integer a JavaScript number holds exactly.
        const MAX: i64 = (1 << 53) - 1;
        let mut i = text_item("a");
        i.pinned = true;
        i.pinned_at = Some(MAX);
        let json = serde_json::to_string(&ItemView::from_item(&i)).unwrap();
        assert!(json.contains(&format!("\"pinnedAt\":{MAX}")));
    }

    #[test]
    fn kinds() {
        assert_eq!(item_kind(&text_item("a")), ContentKind::Text);
        let mut link = base();
        link.url = Some("https://a.org".into());
        assert_eq!(item_kind(&link), ContentKind::Link);
        let mut rich = text_item("a");
        rich.text.as_mut().unwrap().html = Some("<b>a</b>".into());
        assert_eq!(item_kind(&rich), ContentKind::RichText);
        let mut img = base();
        img.attachments = vec![attachment("a.png", "image/png")];
        assert_eq!(item_kind(&img), ContentKind::Image);
        img.attachments.push(img.attachments[0].clone());
        assert_eq!(item_kind(&img), ContentKind::Files);
    }

    #[test]
    fn preview_is_bounded() {
        let v = ItemView::from_item(&text_item(&"é".repeat(1000)));
        assert!(v.preview_truncated);
        assert_eq!(v.preview.unwrap().chars().count(), PREVIEW_CHARS + 1);
        assert!(!ItemView::from_item(&text_item("court")).preview_truncated);
    }

    #[test]
    fn the_markdown_mode_is_offered_only_with_a_markdown_representation() {
        let mut md = text_item("# T");
        md.text.as_mut().unwrap().markdown = Some("# T".into());
        assert!(ItemView::from_item(&md).has_markdown);
        let mut html = text_item("a");
        html.text.as_mut().unwrap().html = Some("<b>a</b>".into());
        let v = ItemView::from_item(&html);
        assert!(v.rich && !v.has_markdown);
        // Nothing of a hidden item, not even that it has Markdown.
        md.secret = true;
        assert!(!ItemView::from_item(&md).has_markdown);
    }

    #[test]
    fn filters() {
        let mut link = base();
        link.url = Some("https://a.org".into());
        let text = text_item("hello");
        let mut file = base();
        file.attachments = vec![attachment("a.pdf", "application/pdf")];
        let secret = secret_item();
        assert!(matches_filter(&link, ItemFilter::Links));
        assert!(!matches_filter(&link, ItemFilter::Text));
        assert!(matches_filter(&text, ItemFilter::Text));
        assert!(matches_filter(&file, ItemFilter::Files));
        assert!(matches_filter(&secret, ItemFilter::Secrets));
        assert!(!matches_filter(&secret, ItemFilter::Text));
        assert!(!matches_filter(&secret, ItemFilter::Links));
        for i in [&link, &text, &file, &secret] {
            assert!(matches_filter(i, ItemFilter::All));
        }
    }

    const ALL_FILTERS: [ItemFilter; 6] = [
        ItemFilter::All,
        ItemFilter::Links,
        ItemFilter::Text,
        ItemFilter::Files,
        ItemFilter::Secrets,
        ItemFilter::Archives,
    ];

    #[test]
    fn an_archived_item_is_only_in_the_archives_filter() {
        let mut link = base();
        link.url = Some("https://a.org".into());
        let mut file = base();
        file.attachments = vec![attachment("a.pdf", "application/pdf")];
        for plain in [archived(link), archived(text_item("hello")), archived(file)] {
            for f in ALL_FILTERS {
                assert_eq!(
                    matches_filter(&plain, f),
                    f == ItemFilter::Archives,
                    "{f:?}"
                );
            }
        }
        // A hidden archived item is not under "Secrets" either, only under "Archives".
        let hidden = archived(secret_item());
        assert!(matches_filter(&hidden, ItemFilter::Archives));
        assert!(!matches_filter(&hidden, ItemFilter::Secrets));
        assert!(!matches_filter(&hidden, ItemFilter::All));
        // Not archived: never in "Archives".
        assert!(!matches_filter(&text_item("x"), ItemFilter::Archives));
        assert!(!matches_filter(&secret_item(), ItemFilter::Archives));
    }

    #[test]
    fn a_pin_wins_over_an_archive_at_display() {
        let mut both = archived(text_item("hello"));
        both.pinned = true;
        both.pinned_at = Some(2);
        assert!(!both.is_shown_archived());
        assert!(matches_filter(&both, ItemFilter::All));
        assert!(matches_filter(&both, ItemFilter::Text));
        assert!(!matches_filter(&both, ItemFilter::Archives));
        let v = ItemView::from_item(&both);
        assert!(v.pinned && !v.archived);
        assert_eq!(v.archived_at, None);
    }

    #[test]
    fn a_search_under_all_also_finds_archived_items_but_an_empty_one_does_not() {
        let kept = archived(text_item("Réunion Lundi"));
        let recent = text_item("Réunion Mardi");
        // Empty (or blank) search: "Tous" leaves archived items out.
        assert!(!is_listed(&kept, ItemFilter::All, ""));
        assert!(!is_listed(&kept, ItemFilter::All, "   "));
        assert!(is_listed(&recent, ItemFilter::All, ""));
        // Searching under "Tous": archived items are searched too.
        assert!(is_listed(&kept, ItemFilter::All, "lundi"));
        assert!(is_listed(&recent, ItemFilter::All, "réunion"));
        assert!(!is_listed(&kept, ItemFilter::All, "mardi"));
        // A typed filter does not widen the search: archived stay out.
        assert!(!is_listed(&kept, ItemFilter::Text, "lundi"));
        // Under "Archives" only archived items, searched like the others.
        assert!(is_listed(&kept, ItemFilter::Archives, ""));
        assert!(is_listed(&kept, ItemFilter::Archives, "lundi"));
        assert!(!is_listed(&kept, ItemFilter::Archives, "mardi"));
        assert!(!is_listed(&recent, ItemFilter::Archives, "réunion"));
    }

    #[test]
    fn an_archived_hidden_item_is_searched_by_its_device_only() {
        let hidden = archived(secret_item());
        for q in ["tr0ub4dor", "banque", "login"] {
            assert!(!is_listed(&hidden, ItemFilter::All, q), "{q}");
            assert!(!is_listed(&hidden, ItemFilter::Archives, q), "{q}");
        }
        assert!(is_listed(&hidden, ItemFilter::All, "pixel"));
        assert!(is_listed(&hidden, ItemFilter::Archives, "pixel"));
        // The view of an archived hidden item still shows nothing of its content.
        let json = serde_json::to_string(&ItemView::from_item(&hidden)).unwrap();
        for leaked in ["Tr0ub4dor", "banque"] {
            assert!(!json.contains(leaked), "{leaked} leaked: {json}");
        }
    }

    #[test]
    fn the_archive_reaches_the_view_as_camel_case() {
        let v = ItemView::from_item(&archived(text_item("a")));
        assert!(v.archived);
        assert_eq!(v.archived_at, Some(1_700_000_000_000));
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["archived"], true);
        assert_eq!(json["archivedAt"], 1_700_000_000_000i64);
        let plain = serde_json::to_value(ItemView::from_item(&text_item("b"))).unwrap();
        assert_eq!(plain["archived"], false);
        assert!(plain["archivedAt"].is_null());
        // 2^53 - 1 survives the JSON round trip (spec/archive-contract.md §2).
        const MAX: i64 = (1 << 53) - 1;
        let mut i = archived(text_item("c"));
        i.archived_at = Some(MAX);
        let json = serde_json::to_string(&ItemView::from_item(&i)).unwrap();
        assert!(json.contains(&format!("\"archivedAt\":{MAX}")));
    }

    #[test]
    fn archiving_never_marks_read_but_the_tray_stops_counting_it() {
        let mut a = archived(text_item("a"));
        a.id = "a".into();
        assert!(!a.read, "archiving does not mark read");
        assert_eq!(
            unread_count(&[a]),
            0,
            "an archived item is out of the feed: nothing could clear it"
        );
    }

    #[test]
    fn search_covers_content_but_never_what_is_hidden() {
        let text = text_item("Réunion Lundi");
        assert!(matches_query(&text, "lundi"));
        assert!(matches_query(&text, "pixel"));
        assert!(!matches_query(&text, "mardi"));
        let mut secret = secret_item();
        secret.attachments = vec![attachment("releve-2026.pdf", "application/pdf")];
        // Neither the value, nor the title, nor the link, nor a file name.
        for q in ["tr0ub4dor", "banque", "login", "releve"] {
            assert!(!matches_query(&secret, q), "{q}");
        }
        // Only the source device, which is not part of the secret.
        assert!(matches_query(&secret, "pixel"));
        assert!(matches_query(&text, "  "));
    }

    #[test]
    fn unread_counts_only_incoming_unread() {
        let mut a = base();
        let mut b = base();
        b.read = true;
        let mut c = base();
        c.outgoing = true;
        a.id = "a".into();
        assert_eq!(unread_count(&[a, b, c]), 1);
    }

    #[test]
    fn an_unreadable_item_or_an_archived_one_is_not_counted_but_a_pinned_archived_one_is() {
        let mut u = base();
        u.unreadable = true;
        let mut pinned = archived(base());
        pinned.pinned = true;
        assert_eq!(unread_count(&[u]), 0);
        assert_eq!(
            unread_count(&[pinned]),
            1,
            "a pinned item stays in the feed"
        );
    }
}
