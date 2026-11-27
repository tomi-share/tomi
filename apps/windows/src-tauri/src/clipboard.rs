//! Windows clipboard access (clipboard-win).
//!
//! Rules (docs/DEV.md invariants 2 and 4):
//! - The app never reads clipboard *content* on its own. The only read helper in use is
//!   the equality check of the 30 s auto-clear of a secret the app itself copied.
//!   [`flagged_sensitive`] (format *presence* only) is reserved for the explicit paste
//!   handler but is not called yet (see its doc comment).
//! - Copying a secret, in any shape (text, link, rich text, files, image), marks the
//!   clipboard as sensitive and schedules a clear (see clip_clears).

use crate::error::{AppError, AppResult};

/// Formats understood by Windows clipboard history / cloud clipboard and by clipboard
/// managers (documented by Microsoft).
pub const FORMAT_EXCLUDE_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
pub const FORMAT_HISTORY: &str = "CanIncludeInClipboardHistory";
pub const FORMAT_CLOUD: &str = "CanUploadToCloudClipboard";

/// Every copy Tomi makes (secret or not) carries this format, empty of meaning: it plays the part
/// of the clip label "Tomi" on Android. After the space is erased, a clipboard that still holds
/// it is emptied without ever reading its content (only the presence of the format is tested).
pub const FORMAT_ORIGIN: &str = "TomiClipboardOrigin";

/// Carried, besides [`FORMAT_ORIGIN`], by every copy of a secret (and only by those). It lets the
/// next start-up empty a clipboard that still holds a secret Tomi copied before it crashed
/// (`panic = "abort"`: no exit handler runs). Like the origin tag, only its presence is tested.
pub const FORMAT_SECRET_ORIGIN: &str = "TomiSecretClipboardOrigin";

/// Delay before a copied secret is wiped from the clipboard.
pub const SECRET_CLEAR_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// What an attempt to empty the clipboard concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearResult {
    /// The clipboard held what Tomi put there and was emptied.
    Cleared,
    /// The clipboard holds something else (or nothing): there is nothing to do.
    NotOurs,
    /// The clipboard could not be opened (another program has it): try again.
    Busy,
}

/// Decides whether the auto-clear must wipe the clipboard.
///
/// `seq_unchanged`: the clipboard sequence number is the one recorded right after our
/// own write (nobody copied anything since). Otherwise the content is compared with the
/// secret, and only an exact match is wiped.
pub fn should_clear(seq_unchanged: bool, current_text: Option<&str>, value: &str) -> bool {
    seq_unchanged || current_text.is_some_and(|t| t == value)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use clipboard_win::{formats, raw, Clipboard};

    fn sys<T>(r: clipboard_win::SysResult<T>) -> AppResult<T> {
        r.map_err(|_| AppError::new("error.clipboard"))
    }

    fn open() -> AppResult<Clipboard> {
        sys(Clipboard::new_attempts(10))
    }

    /// True when the clipboard currently carries `ExcludeClipboardContentFromMonitorProcessing`
    /// (set by password managers). Only the presence of the format is tested.
    /// Not called yet: reserved for the explicit "paste a secret" flow.
    #[allow(dead_code)]
    pub fn flagged_sensitive() -> bool {
        clipboard_win::register_format(FORMAT_EXCLUDE_MONITOR)
            .is_some_and(|f| clipboard_win::is_format_avail(f.get()))
    }

    /// Adds the three "do not keep / do not sync" formats to the content just written.
    /// Must be called while the clipboard is open, after the content is set.
    fn add_sensitive_formats() -> AppResult<()> {
        let zero = 0u32.to_le_bytes();
        for name in [FORMAT_EXCLUDE_MONITOR, FORMAT_HISTORY, FORMAT_CLOUD] {
            let fmt = clipboard_win::register_format(name)
                .ok_or_else(|| AppError::new("error.clipboard"))?;
            sys(raw::set_without_clear(fmt.get(), &zero))?;
        }
        Ok(())
    }

    /// Finishes a copy: flags it sensitive when asked and returns the clipboard sequence
    /// number right after the write (to recognise our own content later).
    fn finish(sensitive: bool) -> AppResult<Option<u32>> {
        if sensitive {
            add_sensitive_formats()?;
            // Not best effort: a secret that cannot be recognised after a crash is not copied.
            let fmt = clipboard_win::register_format(FORMAT_SECRET_ORIGIN)
                .ok_or_else(|| AppError::new("error.clipboard"))?;
            sys(raw::set_without_clear(fmt.get(), &0u32.to_le_bytes()))?;
        }
        // Best effort: the copy itself must not fail for a missing tag.
        if let Some(fmt) = clipboard_win::register_format(FORMAT_ORIGIN) {
            let _ = raw::set_without_clear(fmt.get(), &0u32.to_le_bytes());
        }
        Ok(clipboard_win::seq_num().map(|n| n.get()))
    }

    /// Every copy of a secret, whatever its shape (text, link, rich text, files, image),
    /// goes through `sensitive = true`.
    pub fn set_text(text: &str, sensitive: bool) -> AppResult<Option<u32>> {
        let _c = open()?;
        sys(raw::set_string(text))?;
        finish(sensitive)
    }

    pub fn set_rich(plain: &str, html: &str, sensitive: bool) -> AppResult<Option<u32>> {
        let _c = open()?;
        sys(raw::set_string(plain))?;
        let fmt = formats::Html::new().ok_or_else(|| AppError::new("error.clipboard"))?;
        sys(raw::set_html(fmt.code(), html))?;
        finish(sensitive)
    }

    /// Copies an image file as PNG (modern apps) and as a bitmap (legacy apps).
    pub fn set_image(path: &std::path::Path, sensitive: bool) -> AppResult<Option<u32>> {
        let img = image::open(path).map_err(|_| AppError::new("error.image"))?;
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|_| AppError::new("error.image"))?;
        let mut bmp = Vec::new();
        image::DynamicImage::ImageRgb8(img.to_rgb8())
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .map_err(|_| AppError::new("error.image"))?;

        let _c = open()?;
        sys(raw::empty())?;
        if let Some(png_fmt) = clipboard_win::register_format("PNG") {
            sys(raw::set_without_clear(png_fmt.get(), &png))?;
        }
        // Best effort: some bitmaps are refused, PNG alone is enough for most apps.
        let _ = raw::set_bitmap(&bmp);
        finish(sensitive)
    }

    pub fn set_files(paths: &[String], sensitive: bool) -> AppResult<Option<u32>> {
        let _c = open()?;
        sys(raw::empty())?;
        sys(raw::set_file_list(paths))?;
        finish(sensitive)
    }

    /// Wipes the clipboard if it still holds `value` (see [`should_clear`]).
    /// Returns true when it was cleared.
    pub fn clear_if_still(value: &str, seq_after_write: Option<u32>) -> bool {
        clear_if_still_checked(value, seq_after_write) == ClearResult::Cleared
    }

    /// Same, telling "not ours" from "the clipboard is busy" (the caller tries again).
    pub fn clear_if_still_checked(value: &str, seq_after_write: Option<u32>) -> ClearResult {
        let current_seq = clipboard_win::seq_num().map(|n| n.get());
        let unchanged = seq_after_write.is_some() && current_seq == seq_after_write;
        let current_text = if unchanged {
            None
        } else {
            clipboard_win::get_clipboard_string().ok()
        };
        if !should_clear(unchanged, current_text.as_deref(), value) {
            return ClearResult::NotOurs;
        }
        empty_it()
    }

    /// Empties the clipboard if it carries the tag of Tomi's own copies ([`FORMAT_ORIGIN`]).
    /// Only the presence of the format is tested, never the content.
    pub fn clear_tagged() -> ClearResult {
        let tagged = clipboard_win::register_format(FORMAT_ORIGIN)
            .is_some_and(|f| clipboard_win::is_format_avail(f.get()));
        if !tagged {
            return ClearResult::NotOurs;
        }
        empty_it()
    }

    /// Empties the clipboard only if it carries the marker of a secret copied by Tomi
    /// ([`FORMAT_SECRET_ORIGIN`]); a plain copy of Tomi, or anything from another program, stays.
    /// Only the presence of the format is tested, never the content.
    pub fn clear_tagged_secret() -> ClearResult {
        let tagged = clipboard_win::register_format(FORMAT_SECRET_ORIGIN)
            .is_some_and(|f| clipboard_win::is_format_avail(f.get()));
        if !tagged {
            return ClearResult::NotOurs;
        }
        empty_it()
    }

    fn empty_it() -> ClearResult {
        match open() {
            Ok(_c) if raw::empty().is_ok() => ClearResult::Cleared,
            _ => ClearResult::Busy,
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    fn unsupported<T>() -> AppResult<T> {
        Err(AppError::new("error.unsupported"))
    }
    pub fn flagged_sensitive() -> bool {
        false
    }
    pub fn set_text(_: &str, _: bool) -> AppResult<Option<u32>> {
        unsupported()
    }
    pub fn set_rich(_: &str, _: &str, _: bool) -> AppResult<Option<u32>> {
        unsupported()
    }
    pub fn set_image(_: &std::path::Path, _: bool) -> AppResult<Option<u32>> {
        unsupported()
    }
    pub fn set_files(_: &[String], _: bool) -> AppResult<Option<u32>> {
        unsupported()
    }
    pub fn clear_if_still(_: &str, _: Option<u32>) -> bool {
        false
    }
    pub fn clear_if_still_checked(_: &str, _: Option<u32>) -> ClearResult {
        ClearResult::NotOurs
    }
    pub fn clear_tagged() -> ClearResult {
        ClearResult::NotOurs
    }
    pub fn clear_tagged_secret() -> ClearResult {
        ClearResult::NotOurs
    }
}

pub use imp::*;

/// How many times the start-up clear tries a clipboard another program keeps open.
const LEFTOVER_TRIES: usize = 8;
const LEFTOVER_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// At start-up: a secret that a previous run copied and never got to wipe (a crash: the release
/// build aborts on panic, so no exit handler ran) is emptied from the clipboard. Only a clipboard
/// that still carries Tomi's secret marker is touched; whatever the user copied since is left
/// alone. Blocking: call it off the async threads.
pub fn clear_leftover_secret() -> ClearResult {
    let mut last = ClearResult::Busy;
    for i in 0..LEFTOVER_TRIES {
        last = clear_tagged_secret();
        if last != ClearResult::Busy {
            break;
        }
        if i + 1 < LEFTOVER_TRIES {
            std::thread::sleep(LEFTOVER_PAUSE);
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_when_nothing_was_copied_since() {
        assert!(should_clear(true, None, "secret"));
    }

    #[test]
    fn clears_when_clipboard_still_holds_the_value() {
        assert!(should_clear(false, Some("secret"), "secret"));
    }

    #[test]
    fn keeps_something_the_user_copied_afterwards() {
        assert!(!should_clear(false, Some("autre chose"), "secret"));
        assert!(!should_clear(false, None, "secret"));
    }

    #[test]
    fn format_names_are_the_documented_ones() {
        assert_eq!(
            FORMAT_EXCLUDE_MONITOR,
            "ExcludeClipboardContentFromMonitorProcessing"
        );
        assert_eq!(FORMAT_HISTORY, "CanIncludeInClipboardHistory");
        assert_eq!(FORMAT_CLOUD, "CanUploadToCloudClipboard");
        assert_eq!(SECRET_CLEAR_DELAY.as_secs(), 30);
        assert_eq!(FORMAT_ORIGIN, "TomiClipboardOrigin");
        assert_eq!(FORMAT_SECRET_ORIGIN, "TomiSecretClipboardOrigin");
    }
}

/// Real Win32 round trip. Not run by default: it overwrites, then empties, the system
/// clipboard and needs an interactive desktop session. Run it by hand on Windows:
///
/// `cargo test --lib win32_clipboard -- --ignored --test-threads=1`
#[cfg(all(test, windows))]
mod win32_clipboard {
    use super::*;
    use clipboard_win::{raw, Clipboard};

    fn available(name: &str) -> bool {
        clipboard_win::register_format(name).is_some_and(|f| raw::is_format_avail(f.get()))
    }

    /// The DWORD stored under a registered format, read back from the clipboard.
    fn dword(name: &str) -> Vec<u8> {
        let fmt = clipboard_win::register_format(name).expect("format registers");
        let _c = Clipboard::new_attempts(10).expect("clipboard opens");
        let mut buf = Vec::new();
        raw::get_vec(fmt.get(), &mut buf).expect("format readable");
        buf
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn a_sensitive_copy_really_carries_the_three_formats_and_is_wiped() {
        let seq = set_text("tomi-test-secret", true).expect("copy works");
        assert!(seq.is_some(), "a sequence number is returned");

        assert_eq!(
            clipboard_win::get_clipboard_string().unwrap(),
            "tomi-test-secret"
        );
        for name in [FORMAT_EXCLUDE_MONITOR, FORMAT_HISTORY, FORMAT_CLOUD] {
            assert!(available(name), "{name} must be on the clipboard");
            assert_eq!(dword(name), 0u32.to_le_bytes(), "{name} must hold DWORD 0");
        }

        assert!(clear_if_still("tomi-test-secret", seq), "wiped");
        assert!(!raw::is_format_avail(formats_unicode_text()));
        assert!(!available(FORMAT_CLOUD));
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn rich_text_and_files_of_a_secret_are_flagged_too() {
        set_rich("plain", "<b>html</b>", true).expect("rich copy works");
        for name in [FORMAT_EXCLUDE_MONITOR, FORMAT_HISTORY, FORMAT_CLOUD] {
            assert!(available(name), "{name} (rich)");
        }
        let file = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        set_files(&[file], true).expect("file copy works");
        for name in [FORMAT_EXCLUDE_MONITOR, FORMAT_HISTORY, FORMAT_CLOUD] {
            assert!(available(name), "{name} (files)");
        }
        let _c = Clipboard::new_attempts(10).unwrap();
        raw::empty().unwrap();
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn an_ordinary_copy_carries_none_of_the_sensitive_formats() {
        set_text("pas un secret", false).expect("copy works");
        for name in [FORMAT_EXCLUDE_MONITOR, FORMAT_HISTORY, FORMAT_CLOUD] {
            assert!(!available(name), "{name} must be absent");
        }
        let _c = Clipboard::new_attempts(10).unwrap();
        raw::empty().unwrap();
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn an_ordinary_copy_of_tomi_is_emptied_by_its_tag_and_a_foreign_one_is_not() {
        set_text("pas un secret", false).expect("copy works");
        assert!(available(FORMAT_ORIGIN), "every copy of Tomi is tagged");
        assert_eq!(clear_tagged(), ClearResult::Cleared);
        assert_eq!(clear_tagged(), ClearResult::NotOurs, "nothing left");

        // Something copied by another program carries no tag: left alone.
        {
            let _c = Clipboard::new_attempts(10).unwrap();
            raw::set_string("venu d'ailleurs").unwrap();
        }
        assert_eq!(clear_tagged(), ClearResult::NotOurs);
        assert_eq!(
            clipboard_win::get_clipboard_string().unwrap(),
            "venu d'ailleurs"
        );
        let _c = Clipboard::new_attempts(10).unwrap();
        raw::empty().unwrap();
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn the_start_up_clear_empties_a_leftover_secret_only() {
        // A secret left by a crashed run: emptied, without reading it.
        set_text("tomi-test-secret", true).unwrap();
        assert!(available(FORMAT_SECRET_ORIGIN));
        assert_eq!(clear_tagged_secret(), ClearResult::Cleared);
        assert_eq!(clear_tagged_secret(), ClearResult::NotOurs, "nothing left");

        // An ordinary copy of Tomi is not a secret: left alone.
        set_text("pas un secret", false).unwrap();
        assert!(!available(FORMAT_SECRET_ORIGIN));
        assert_eq!(clear_tagged_secret(), ClearResult::NotOurs);
        assert_eq!(
            clipboard_win::get_clipboard_string().unwrap(),
            "pas un secret"
        );

        // So is something copied by another program.
        {
            let _c = Clipboard::new_attempts(10).unwrap();
            raw::set_string("venu d'ailleurs").unwrap();
        }
        assert_eq!(clear_tagged_secret(), ClearResult::NotOurs);
        let _c = Clipboard::new_attempts(10).unwrap();
        raw::empty().unwrap();
    }

    #[test]
    #[ignore = "writes to the real Windows clipboard; run manually"]
    fn the_wipe_leaves_something_the_user_copied_afterwards() {
        let seq = set_text("tomi-test-secret", true).unwrap();
        set_text("autre chose", false).unwrap();
        assert!(!clear_if_still("tomi-test-secret", seq));
        assert_eq!(
            clipboard_win::get_clipboard_string().unwrap(),
            "autre chose"
        );
        let _c = Clipboard::new_attempts(10).unwrap();
        raw::empty().unwrap();
    }

    fn formats_unicode_text() -> u32 {
        clipboard_win::formats::CF_UNICODETEXT
    }
}
