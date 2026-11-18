//! "Envoyer vers > Tomi": a `Tomi.lnk` in `shell:sendto` launching the app, which
//! receives the selected file paths as command-line arguments.

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

pub const SHORTCUT_NAME: &str = "Tomi.lnk";

/// `%APPDATA%\Microsoft\Windows\SendTo`
pub fn sendto_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| {
        PathBuf::from(a)
            .join("Microsoft")
            .join("Windows")
            .join("SendTo")
    })
}

pub fn shortcut_path(dir: &Path) -> PathBuf {
    dir.join(SHORTCUT_NAME)
}

/// What to do with the shortcut at launch.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Leave it alone: written for this very executable, and if the user removed it since, that
    /// was on purpose.
    Nothing,
    /// Write it, then remember `(exe, stamp)`.
    Write,
    /// Only remember the stamp (settings from before the stamp existed): no write.
    Remember,
}

/// Creation time of the executable in ms since the Unix epoch (modification time if the file
/// system has no creation time): changes when the file is installed again.
pub fn exe_stamp(exe: &Path) -> Option<u64> {
    let meta = std::fs::metadata(exe).ok()?;
    let time = meta.created().or_else(|_| meta.modified()).ok()?;
    let ms = time.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    u64::try_from(ms).ok()
}

/// The shortcut is (re)written only when it can be missing for a reason other than the user:
/// first launch, the app moved, or the executable is a new file (install, uninstall then
/// reinstall, update). Not at every launch: someone who deleted « Envoyer vers › Tomi » keeps it
/// deleted.
pub fn decide(
    saved_exe: Option<&str>,
    saved_stamp: Option<u64>,
    exe: &str,
    stamp: Option<u64>,
) -> Action {
    if saved_exe != Some(exe) {
        return Action::Write;
    }
    match (saved_stamp, stamp) {
        (Some(a), Some(b)) if a != b => Action::Write,
        (None, Some(_)) => Action::Remember,
        _ => Action::Nothing,
    }
}

/// Writes (or rewrites) the shortcut so that it points at `exe`.
#[cfg(windows)]
pub fn write_shortcut(exe: &Path, dir: &Path) -> AppResult<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut link = mslnk::ShellLink::new(exe).map_err(|_| AppError::new("error.sendto"))?;
    link.set_name(Some("Tomi".to_string()));
    link.set_icon_location(Some(exe.to_string_lossy().into_owned()));
    if let Some(parent) = exe.parent() {
        link.set_working_dir(Some(parent.to_string_lossy().into_owned()));
    }
    let path = shortcut_path(dir);
    link.create_lnk(&path)
        .map_err(|_| AppError::new("error.sendto"))?;
    Ok(path)
}

#[cfg(not(windows))]
pub fn write_shortcut(_exe: &Path, _dir: &Path) -> AppResult<PathBuf> {
    Err(AppError::new("error.unsupported"))
}

#[cfg(test)]
mod decision_tests {
    use super::*;

    #[test]
    fn written_on_first_launch_and_when_the_app_moved() {
        assert_eq!(
            decide(None, None, "C:\\a\\tomi.exe", Some(1)),
            Action::Write
        );
        assert_eq!(
            decide(
                Some("C:\\old\\tomi.exe"),
                Some(1),
                "C:\\a\\tomi.exe",
                Some(1)
            ),
            Action::Write
        );
    }

    #[test]
    fn not_rewritten_when_nothing_changed_even_if_the_user_deleted_it() {
        // The shortcut is not even looked at: a deliberate deletion stays.
        assert_eq!(
            decide(Some("C:\\a\\tomi.exe"), Some(5), "C:\\a\\tomi.exe", Some(5)),
            Action::Nothing
        );
        assert_eq!(
            decide(Some("C:\\a\\tomi.exe"), None, "C:\\a\\tomi.exe", None),
            Action::Nothing
        );
    }

    #[test]
    fn written_again_for_a_new_file_at_the_same_path() {
        // Uninstall (the NSIS hook removes the shortcut) then reinstall, or an update.
        assert_eq!(
            decide(Some("C:\\a\\tomi.exe"), Some(5), "C:\\a\\tomi.exe", Some(9)),
            Action::Write
        );
    }

    #[test]
    fn settings_from_before_the_stamp_only_learn_it() {
        assert_eq!(
            decide(Some("C:\\a\\tomi.exe"), None, "C:\\a\\tomi.exe", Some(9)),
            Action::Remember
        );
    }

    #[test]
    fn the_stamp_of_a_file_is_read_and_changes_with_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tomi.exe");
        assert_eq!(exe_stamp(&p), None);
        std::fs::write(&p, b"1").unwrap();
        assert!(exe_stamp(&p).is_some());
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn writes_a_shortcut_file() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let path = write_shortcut(&exe, dir.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), SHORTCUT_NAME);
        let bytes = std::fs::read(&path).unwrap();
        // Shell link header: size 0x4C then the link CLSID.
        assert_eq!(&bytes[..4], &[0x4C, 0, 0, 0]);
        // Rewriting is idempotent.
        write_shortcut(&exe, dir.path()).unwrap();
    }
}
