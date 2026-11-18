//! Native notifications (Windows toasts).
//!
//! Toasts go through the WinRT toast API directly (`windows` crate): the generic notification
//! plugin cannot report which button was pressed on desktop, nor tag a toast so that it can be
//! taken back when its item is deleted, expired or archived.
//! A secret never appears in a toast: no preview, only "Secret reçu · de <appareil>"
//! (invariant 3).

use crate::detect::ContentKind;
use crate::model::{Item, UnreadableReason};
use crate::strings;
use crate::view::item_kind;

pub const AUMID: &str = "eu.shaffe.tomi";
const BODY_PREVIEW_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastContent {
    pub title: String,
    pub body: String,
    /// `(label, action id)` buttons.
    pub buttons: Vec<(&'static str, &'static str)>,
}

fn short(s: &str, max: usize) -> String {
    let one_line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        one_line.chars().take(max).collect::<String>() + "…"
    }
}

/// Content of the toast for one received item.
pub fn single_toast(item: &Item) -> ToastContent {
    let from = strings::toast_from(&item.source_device.name);
    let buttons = vec![
        (strings::toast_copy(), "copy"),
        (strings::toast_open(), "open"),
    ];
    if item.unreadable {
        // Nothing of the item is known (and it may well be a hidden one): only the sender and
        // the reason, and nothing to copy.
        let body = match item.unreadable_reason {
            Some(UnreadableReason::UpdateRequired) => strings::toast_body_unreadable_update(),
            _ => strings::toast_body_unreadable_other(),
        };
        return ToastContent {
            title: strings::toast_title_unreadable(&item.source_device.name),
            body: body.to_string(),
            buttons: vec![(strings::toast_open(), "open")],
        };
    }
    if item.secret {
        // Nothing of the item shows (no title, no preview, no file name): the generic
        // label and the sender. Nothing is copied from the toast either: one click must not
        // hand over a hidden value. The only button opens the popover, where "Afficher" and
        // "Copier" ask Windows Hello.
        return ToastContent {
            title: strings::toast_title_secret().to_string(),
            body: from,
            buttons: vec![(strings::toast_open(), "open")],
        };
    }
    let (title, detail) = match item_kind(item) {
        ContentKind::Link => (
            strings::toast_title_link(),
            item.title.clone().or_else(|| item.url.clone()),
        ),
        ContentKind::Image => (
            strings::toast_title_image(),
            item.attachments.first().map(|a| a.name.clone()),
        ),
        ContentKind::File => (
            strings::toast_title_file(),
            item.attachments.first().map(|a| a.name.clone()),
        ),
        ContentKind::Files => (
            strings::toast_title_files(),
            Some(format!("{} fichiers", item.attachments.len())),
        ),
        ContentKind::Text | ContentKind::RichText => (
            strings::toast_title_text(),
            item.text.as_ref().map(|t| t.plain.clone()),
        ),
    };
    let body = match detail {
        Some(d) if !d.trim().is_empty() => format!("{}\n{}", short(&d, BODY_PREVIEW_CHARS), from),
        _ => from,
    };
    ToastContent {
        title: title.to_string(),
        body,
        buttons,
    }
}

/// Another device offers its history: the answer (yes or no) is given in the Devices tab.
pub fn offer_toast(peer: &str) -> ToastContent {
    ToastContent {
        title: strings::toast_title_offer().to_string(),
        body: strings::toast_from(peer),
        buttons: vec![(strings::toast_open(), "open")],
    }
}

/// Another device joined the space. Only its name (chosen by that device, cut short): never any
/// content.
pub fn new_device_toast(name: &str) -> ToastContent {
    ToastContent {
        title: strings::toast_title_new_device().to_string(),
        body: strings::toast_body_new_device(&short(name, 40)),
        buttons: vec![(strings::toast_open(), "open")],
    }
}

/// Burst of several items: one grouped toast, no previews at all.
pub fn group_toast(count: usize) -> ToastContent {
    ToastContent {
        title: strings::toast_group_title(count),
        body: strings::toast_group_body().to_string(),
        buttons: vec![(strings::toast_open(), "open")],
    }
}

/// What a tap on the toast body does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    OpenLink,
    CopyText,
    OpenFile,
    ShowInPopover,
}

const SHORT_TEXT_CHARS: usize = 280;

pub fn tap_action(item: &Item) -> TapAction {
    if item.secret || item.unreadable {
        return TapAction::ShowInPopover;
    }
    match item_kind(item) {
        ContentKind::Link => TapAction::OpenLink,
        ContentKind::Text => {
            let short_text = item.text.as_ref().is_some_and(|t| {
                t.plain.chars().count() <= SHORT_TEXT_CHARS && !t.plain.contains('\n')
            });
            if short_text {
                TapAction::CopyText
            } else {
                TapAction::ShowInPopover
            }
        }
        ContentKind::File => TapAction::OpenFile,
        ContentKind::RichText | ContentKind::Image | ContentKind::Files => TapAction::ShowInPopover,
    }
}

/// A tag Windows accepts (64 characters at most). The id of an item (a UUID) always fits; if it
/// does not, the toast is shown without a tag and simply stays until the user dismisses it.
fn usable_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.len() <= 64
}

/// Escapes a text for the toast's XML (text nodes and single-quoted attributes).
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The toast as Windows reads it: a generic binding with the title and the body, and the buttons.
fn toast_xml(content: &ToastContent) -> String {
    let mut actions = String::new();
    if !content.buttons.is_empty() {
        actions.push_str("<actions>");
        for (label, action) in &content.buttons {
            actions.push_str(&format!(
                "<action content='{}' arguments='{}'/>",
                xml_escape(label),
                xml_escape(action)
            ));
        }
        actions.push_str("</actions>");
    }
    format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text id=\"1\">{}</text><text id=\"2\">{}</text></binding></visual>{}</toast>",
        xml_escape(&content.title),
        xml_escape(&content.body),
        actions
    )
}

/// Group of every toast Tomi tags (with the id of the item as tag): `remove` needs both.
#[cfg(windows)]
const TOAST_GROUP: &str = "tomi";

/// The identity the toasts are shown under. A development or portable build has no Start-menu
/// identity: Windows would drop the toast, so it borrows PowerShell's (icon and name are then
/// wrong).
#[cfg(windows)]
pub fn toast_app_id() -> &'static str {
    const POWERSHELL_APP_ID: &str =
        r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
    if tauri::is_dev() {
        POWERSHELL_APP_ID
    } else {
        AUMID
    }
}

/// Shows a toast. `tag` (the id of the item it announces) lets [`remove`] take it back when the
/// item is deleted, expired or archived. `on_action` gets the id of the button pressed (`None`
/// for a click on the toast itself).
#[cfg(windows)]
pub fn show(
    content: ToastContent,
    tag: Option<&str>,
    on_action: impl Fn(Option<String>) + Send + 'static,
) {
    use windows::core::{IInspectable, Interface, HSTRING};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{
        ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
    };

    fn activated_action(insp: &Option<IInspectable>) -> Option<String> {
        let args = insp.as_ref()?.cast::<ToastActivatedEventArgs>().ok()?;
        let arguments = args.Arguments().ok()?;
        (!arguments.is_empty()).then(|| arguments.to_string())
    }

    let build = || -> windows::core::Result<()> {
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(toast_xml(&content)))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        if let Some(tag) = tag.filter(|t| usable_tag(t)) {
            toast.SetTag(&HSTRING::from(tag))?;
            toast.SetGroup(&HSTRING::from(TOAST_GROUP))?;
        }
        toast.Activated(&TypedEventHandler::new(move |_, insp| {
            on_action(activated_action(&insp).filter(|a| !a.is_empty()));
            Ok(())
        }))?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(toast_app_id()))?;
        notifier.Show(&toast)
    };
    // Notification centre disabled or unavailable: nothing else to do.
    let _ = build();
}

#[cfg(not(windows))]
pub fn show(
    _content: ToastContent,
    _tag: Option<&str>,
    _on_action: impl Fn(Option<String>) + Send + 'static,
) {
}

/// Takes back the toast of an item (on screen or in the notification centre): the item is gone
/// or archived, and the toast must not keep announcing it. Best effort, blocking: call it off the
/// async threads.
#[cfg(windows)]
pub fn remove(tag: &str) {
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;
    if !usable_tag(tag) {
        return;
    }
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(
            &HSTRING::from(tag),
            &HSTRING::from(TOAST_GROUP),
            &HSTRING::from(toast_app_id()),
        );
    }
}

#[cfg(not(windows))]
pub fn remove(_tag: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, DeviceRef, ReadPolicy, TextBody};

    fn base() -> Item {
        Item {
            id: "i".into(),
            title: None,
            text: None,
            url: None,
            attachments: vec![],
            secret: false,
            source_device: DeviceRef {
                id: "d".into(),
                name: "Pixel".into(),
            },
            created_at: 0,
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

    #[test]
    fn secret_toast_has_no_preview_whatsoever() {
        let mut i = base();
        i.secret = true;
        i.title = Some("Banque".into());
        i.url = Some("https://banque.example/login".into());
        i.attachments = vec![attachment("releve.pdf", "application/pdf")];
        i.text = Some(TextBody {
            plain: "Tr0ub4dor&3".into(),
            html: None,
            markdown: None,
        });
        let t = single_toast(&i);
        let all = format!("{} {}", t.title, t.body);
        for leaked in ["Tr0ub4dor", "Banque", "banque", "releve"] {
            assert!(!all.contains(leaked), "{leaked} leaked: {all}");
        }
        assert_eq!(t.title, "Contenu masqué reçu");
        assert!(t.body.contains("Pixel"));
        // Nothing is copied from a toast: the only button opens the popover (Windows Hello there).
        assert_eq!(t.buttons, vec![("Ouvrir", "open")]);
    }

    #[test]
    fn no_hidden_toast_ever_offers_to_copy() {
        let mut i = base();
        i.secret = true;
        for (text, url) in [
            (Some("x"), None),
            (None, Some("https://a.org")),
            (None, None),
        ] {
            i.text = text.map(|t| TextBody {
                plain: t.into(),
                html: None,
                markdown: None,
            });
            i.url = url.map(str::to_string);
            assert!(single_toast(&i).buttons.iter().all(|(_, a)| *a != "copy"));
        }
    }

    #[test]
    fn the_toast_xml_escapes_what_it_carries() {
        let t = ToastContent {
            title: "a & <b>".into(),
            body: "l'1\nl\"2\"".into(),
            buttons: vec![("Ouvrir", "open")],
        };
        let xml = toast_xml(&t);
        assert!(xml.contains("a &amp; &lt;b&gt;"));
        assert!(xml.contains("l&apos;1\nl&quot;2&quot;"));
        assert!(xml.contains("<action content='Ouvrir' arguments='open'/>"));
        assert!(!xml.contains("<b>"));
        // No button, no actions node.
        let none = ToastContent {
            buttons: vec![],
            ..t
        };
        assert!(!toast_xml(&none).contains("<actions>"));
    }

    #[test]
    fn a_tag_must_fit_what_windows_accepts() {
        assert!(usable_tag("0b7a3f12-6c58-4a39-9d2e-1c4f5a6b7c8d"));
        assert!(!usable_tag(""));
        assert!(!usable_tag(&"x".repeat(65)));
        assert!(usable_tag(&"x".repeat(64)));
    }

    #[test]
    fn an_unreadable_item_names_only_its_sender_and_the_reason() {
        let mut i = base();
        i.unreadable = true;
        i.unreadable_reason = Some(UnreadableReason::UpdateRequired);
        let t = single_toast(&i);
        assert_eq!(t.title, "Élément de Pixel");
        assert_eq!(
            t.body,
            "Illisible pour le moment : mise à jour de Tomi nécessaire"
        );
        assert_eq!(t.buttons, vec![("Ouvrir", "open")], "nothing to copy");

        // Any other reason does not promise an update.
        i.unreadable_reason = Some(UnreadableReason::Other);
        let t = single_toast(&i);
        assert_eq!(t.title, "Élément de Pixel");
        assert_eq!(t.body, "Illisible pour le moment");
        // A core that gave no reason says no more than that.
        i.unreadable_reason = None;
        assert_eq!(single_toast(&i).body, "Illisible pour le moment");
        assert_eq!(tap_action(&i), TapAction::ShowInPopover);
    }

    #[test]
    fn a_hidden_file_has_no_copy_button() {
        let mut i = base();
        i.secret = true;
        i.attachments = vec![attachment("cle.pem", "application/x-pem-file")];
        assert_eq!(single_toast(&i).buttons, vec![("Ouvrir", "open")]);
    }

    #[test]
    fn the_offer_of_a_history_names_the_sender_and_nothing_else() {
        let t = offer_toast("PC-Bureau");
        assert_eq!(t.title, "Historique proposé");
        assert!(t.body.contains("PC-Bureau"));
        assert_eq!(t.buttons, vec![("Ouvrir", "open")]);
    }

    #[test]
    fn the_new_device_toast_names_the_device_and_nothing_else() {
        let t = new_device_toast("Pixel 9");
        assert_eq!(t.title, "Nouvel appareil ajouté");
        assert!(t.body.starts_with("Pixel 9 a rejoint"));
        assert_eq!(t.buttons, vec![("Ouvrir", "open")]);
        // A very long (or multi-line) name stays short.
        let t = new_device_toast(&format!("a\n{}", "x".repeat(200)));
        assert!(!t.body.contains('\n'));
        assert!(t.body.chars().count() < 120);
    }

    #[test]
    fn link_toast_shows_title_or_url() {
        let mut i = base();
        i.url = Some("https://example.org/x".into());
        assert!(single_toast(&i).body.starts_with("https://example.org/x"));
        i.title = Some("Un article".into());
        assert!(single_toast(&i).body.starts_with("Un article"));
    }

    #[test]
    fn long_text_preview_is_cut_and_single_line() {
        let mut i = base();
        i.text = Some(TextBody {
            plain: format!("a\nb {}", "x".repeat(300)),
            html: None,
            markdown: None,
        });
        let body = single_toast(&i).body;
        let first = body.lines().next().unwrap();
        assert!(first.chars().count() <= BODY_PREVIEW_CHARS + 1);
        assert!(first.starts_with("a b "));
    }

    #[test]
    fn group_toast_has_no_content() {
        let t = group_toast(3);
        assert_eq!(t.title, "3 nouveaux éléments");
    }

    #[test]
    fn tap_actions_follow_the_reception_table() {
        let mut link = base();
        link.url = Some("https://a.org".into());
        assert_eq!(tap_action(&link), TapAction::OpenLink);

        let mut short = base();
        short.text = Some(TextBody {
            plain: "42 rue des Lilas".into(),
            html: None,
            markdown: None,
        });
        assert_eq!(tap_action(&short), TapAction::CopyText);

        let mut long = base();
        long.text = Some(TextBody {
            plain: "a\nb".into(),
            html: None,
            markdown: None,
        });
        assert_eq!(tap_action(&long), TapAction::ShowInPopover);

        let mut file = base();
        file.attachments = vec![attachment("a.pdf", "application/pdf")];
        assert_eq!(tap_action(&file), TapAction::OpenFile);

        let mut img = base();
        img.attachments = vec![attachment("a.png", "image/png")];
        assert_eq!(tap_action(&img), TapAction::ShowInPopover);

        let mut secret = base();
        secret.secret = true;
        assert_eq!(tap_action(&secret), TapAction::ShowInPopover);
    }
}
