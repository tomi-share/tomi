//! User-visible strings produced on the Rust side (tray, tooltip, toasts, Windows Hello and
//! account-password prompts). Everything shown in the webview lives in `src/lib/i18n/`.
//!
//! One [`Table`] per language (`strings/<lang>.rs`), picked once from the display language of
//! Windows; English is the reference and the fallback. To add a language: write its table and
//! add it to [`TABLES`]. `{n}` and `{device}` are replaced when the string is built.

mod en;
mod fr;

#[cfg(not(test))]
use std::sync::OnceLock;

pub const APP_NAME: &str = "Tomi";

/// Every string of one language.
pub struct Table {
    pub tray_open: &'static str,
    pub tray_history: &'static str,
    pub tray_settings: &'static str,
    pub tray_quit: &'static str,
    pub toast_copy: &'static str,
    pub toast_open: &'static str,
    pub tooltip_sending: &'static str,
    pub tooltip_unread_one: &'static str,
    pub tooltip_unread_other: &'static str,
    pub toast_title_link: &'static str,
    pub toast_title_text: &'static str,
    pub toast_title_image: &'static str,
    pub toast_title_file: &'static str,
    pub toast_title_files: &'static str,
    pub toast_title_secret: &'static str,
    pub toast_title_unreadable: &'static str,
    pub toast_body_unreadable_update: &'static str,
    pub toast_body_unreadable_other: &'static str,
    pub toast_title_offer: &'static str,
    pub toast_from: &'static str,
    pub toast_group_title: &'static str,
    pub toast_group_body: &'static str,
    pub toast_title_new_device: &'static str,
    pub toast_body_new_device: &'static str,
    pub hello_delete_space: &'static str,
    pub hello_erase: &'static str,
    pub hello_revoke: &'static str,
    pub hello_reveal: &'static str,
    pub hello_add_device: &'static str,
    pub account_delete_space: &'static str,
    pub account_erase: &'static str,
    pub account_revoke: &'static str,
    pub account_reveal: &'static str,
    pub account_add_device: &'static str,
    pub account_generic: &'static str,
}

/// The languages of the app, by BCP 47 tag.
const TABLES: &[(&str, &Table)] = &[("en", &en::EN), ("fr", &fr::FR)];

/// The table for someone whose preferred languages are `preferred` (most wanted first): the
/// first one the app has, exactly or by its language alone (`fr-CA` -> `fr`), else English.
fn pick(preferred: &[String]) -> &'static Table {
    for tag in preferred {
        let wanted = tag.replace('_', "-").to_ascii_lowercase();
        let language = wanted.split('-').next().unwrap_or_default();
        for candidate in [wanted.as_str(), language] {
            if let Some((_, table)) = TABLES
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(candidate))
            {
                return table;
            }
        }
    }
    &en::EN
}

/// The display languages of the user, most wanted first.
#[cfg(all(windows, not(test)))]
fn system_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut len) = (0u32, 0u32);
    // SAFETY: a null buffer with a zero length asks for the size only.
    let sized = unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if sized == 0 || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` holds `len` UTF-16 units, the size the call above asked for.
    let filled = unsafe {
        GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buffer.as_mut_ptr(), &mut len)
    };
    if filled == 0 {
        return Vec::new();
    }
    // A list of NUL-terminated names, ended by an empty one.
    buffer
        .split(|unit| *unit == 0)
        .filter(|name| !name.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// Outside Windows (a development build): the `LANG` of the environment.
#[cfg(all(not(windows), not(test)))]
fn system_languages() -> Vec<String> {
    std::env::var("LANG")
        .ok()
        .and_then(|lang| lang.split('.').next().map(str::to_owned))
        .into_iter()
        .collect()
}

#[cfg(not(test))]
fn table() -> &'static Table {
    static TABLE: OnceLock<&'static Table> = OnceLock::new();
    TABLE.get_or_init(|| pick(&system_languages()))
}

/// The tests of the other modules assert the French texts, whatever the machine they run on.
#[cfg(test)]
fn table() -> &'static Table {
    &fr::FR
}

pub fn tray_open() -> &'static str {
    table().tray_open
}
pub fn tray_history() -> &'static str {
    table().tray_history
}
pub fn tray_settings() -> &'static str {
    table().tray_settings
}
pub fn tray_quit() -> &'static str {
    table().tray_quit
}
pub fn toast_copy() -> &'static str {
    table().toast_copy
}
pub fn toast_open() -> &'static str {
    table().toast_open
}

fn tooltip_in(t: &Table, unread: usize, sending: bool) -> String {
    let mut s = APP_NAME.to_string();
    if sending {
        s.push_str(" · ");
        s.push_str(t.tooltip_sending);
    }
    if unread > 0 {
        let form = if unread == 1 {
            t.tooltip_unread_one
        } else {
            t.tooltip_unread_other
        };
        s.push_str(" · ");
        s.push_str(&form.replace("{n}", &unread.to_string()));
    }
    s
}

pub fn tooltip(unread: usize, sending: bool) -> String {
    tooltip_in(table(), unread, sending)
}

pub fn toast_title_link() -> &'static str {
    table().toast_title_link
}
pub fn toast_title_text() -> &'static str {
    table().toast_title_text
}
pub fn toast_title_image() -> &'static str {
    table().toast_title_image
}
pub fn toast_title_file() -> &'static str {
    table().toast_title_file
}
pub fn toast_title_files() -> &'static str {
    table().toast_title_files
}
pub fn toast_title_secret() -> &'static str {
    table().toast_title_secret
}
/// An item that cannot be read: the title names only the sender (as on Android).
pub fn toast_title_unreadable(device: &str) -> String {
    table().toast_title_unreadable.replace("{device}", device)
}
/// ... and the body says why, with the advice that fits (nothing of the item is known).
pub fn toast_body_unreadable_update() -> &'static str {
    table().toast_body_unreadable_update
}
pub fn toast_body_unreadable_other() -> &'static str {
    table().toast_body_unreadable_other
}
pub fn toast_title_offer() -> &'static str {
    table().toast_title_offer
}
pub fn toast_from(device: &str) -> String {
    table().toast_from.replace("{device}", device)
}
pub fn toast_group_title(n: usize) -> String {
    table().toast_group_title.replace("{n}", &n.to_string())
}
pub fn toast_group_body() -> &'static str {
    table().toast_group_body
}
/// Another device just joined the space: a security notice, so the user can react if it is not
/// theirs (the Devices tab revokes it).
pub fn toast_title_new_device() -> &'static str {
    table().toast_title_new_device
}
pub fn toast_body_new_device(device: &str) -> String {
    table().toast_body_new_device.replace("{device}", device)
}

// Verification prompts (see `strong_auth.rs`): the sentence Windows shows above the PIN,
// fingerprint or face check of Windows Hello. Same wording as the Android biometric prompts.
// When Hello is unavailable the password of the Windows account is asked instead
// (`account_auth.rs`) with its own sentence (`account_prompt`): a cancelled Hello never opens
// that fallback, so the Hello sentences do not mention it.
pub fn hello_delete_space() -> &'static str {
    table().hello_delete_space
}
pub fn hello_erase() -> &'static str {
    table().hello_erase
}
pub fn hello_revoke() -> &'static str {
    table().hello_revoke
}
pub fn hello_reveal() -> &'static str {
    table().hello_reveal
}
pub fn hello_add_device() -> &'static str {
    table().hello_add_device
}

fn account_prompt_in(t: &'static Table, hello: &str) -> &'static str {
    if hello == t.hello_delete_space {
        t.account_delete_space
    } else if hello == t.hello_erase {
        t.account_erase
    } else if hello == t.hello_revoke {
        t.account_revoke
    } else if hello == t.hello_reveal {
        t.account_reveal
    } else if hello == t.hello_add_device {
        t.account_add_device
    } else {
        t.account_generic
    }
}

/// The sentence of the account-password prompt that goes with a Hello sentence.
pub fn account_prompt(hello: &str) -> &'static str {
    account_prompt_in(table(), hello)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hello_sentence_has_its_account_sentence_and_neither_mentions_the_other() {
        for (_, t) in TABLES {
            for (hello, account) in [
                (t.hello_delete_space, t.account_delete_space),
                (t.hello_erase, t.account_erase),
                (t.hello_revoke, t.account_revoke),
                (t.hello_reveal, t.account_reveal),
                (t.hello_add_device, t.account_add_device),
            ] {
                assert_eq!(account_prompt_in(t, hello), account);
                assert!(hello.contains("Hello"));
                assert!(!account.contains("Hello"));
                assert!(account.contains("Windows"));
            }
            assert_eq!(account_prompt_in(t, "other"), t.account_generic);
        }
    }

    #[test]
    fn the_language_is_the_first_wanted_one_the_app_has_else_english() {
        let pick_of =
            |tags: &[&str]| pick(&tags.iter().map(|t| t.to_string()).collect::<Vec<_>>()).tray_quit;
        assert_eq!(pick_of(&["fr-CA", "en-US"]), fr::FR.tray_quit);
        assert_eq!(pick_of(&["fr_FR"]), fr::FR.tray_quit);
        assert_eq!(pick_of(&["xx-YY", "fr"]), fr::FR.tray_quit);
        assert_eq!(pick_of(&["xx-YY"]), en::EN.tray_quit);
        assert_eq!(pick_of(&[]), en::EN.tray_quit);
    }

    #[test]
    fn every_table_is_complete_and_keeps_its_placeholders() {
        for (name, t) in TABLES {
            for (field, text) in [
                ("tooltip_unread_one", t.tooltip_unread_one),
                ("tooltip_unread_other", t.tooltip_unread_other),
                ("toast_group_title", t.toast_group_title),
            ] {
                assert!(text.contains("{n}"), "{name}.{field}");
            }
            for (field, text) in [
                ("toast_title_unreadable", t.toast_title_unreadable),
                ("toast_from", t.toast_from),
                ("toast_body_new_device", t.toast_body_new_device),
            ] {
                assert!(text.contains("{device}"), "{name}.{field}");
            }
            assert!(
                !t.tray_open.is_empty() && !t.account_generic.is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn the_tooltip_says_what_is_going_on() {
        assert_eq!(tooltip_in(&en::EN, 0, false), "Tomi");
        assert_eq!(tooltip_in(&en::EN, 1, true), "Tomi · sending… · 1 unread");
        assert_eq!(tooltip_in(&fr::FR, 3, false), "Tomi · 3 non lus");
    }
}
