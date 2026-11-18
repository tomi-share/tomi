//! Persistent user settings (JSON in the app config directory).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::render::RenderMode;
use crate::shortcut::DEFAULT_SHORTCUT;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub shortcut: String,
    /// Last text mode chosen ("Aperçu" or "Markdown"); `None` until the user picks one.
    pub render_mode: Option<RenderMode>,
    pub autostart: bool,
    pub notifications: bool,
    pub onboarding_done: bool,
    /// Where "Enregistrer" writes; `None` = Downloads\Tomi.
    pub download_dir: Option<String>,
    /// Executable the SendTo shortcut was last written for.
    pub sendto_exe: Option<String>,
    /// Creation time (ms since the Unix epoch) of that executable when the shortcut was written:
    /// a new file at the same path (reinstall after an uninstall, update) is told apart from the
    /// same one, without rewriting a shortcut the user removed on purpose at every launch.
    pub sendto_stamp: Option<u64>,
    /// Autostart was enabled once by default (the user may disable it afterwards).
    pub autostart_initialised: bool,
    /// This PC has belonged to a space (written when it is provisioned, cleared last by the
    /// cleanup that follows a departure). The only proof the start-up cleanup trusts: a PC that
    /// was never paired has nothing to clean, whatever `onboarding_done` says.
    pub belonged_to_space: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shortcut: DEFAULT_SHORTCUT.to_string(),
            render_mode: None,
            autostart: true,
            notifications: true,
            onboarding_done: false,
            download_dir: None,
            sendto_exe: None,
            sendto_stamp: None,
            autostart_initialised: false,
            belonged_to_space: false,
        }
    }
}

impl Settings {
    /// Missing or unreadable file gives the defaults (never a startup failure).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic write: temp file then rename.
    pub fn save(&self, path: &Path) -> AppResult<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp: PathBuf = path.with_extension("json.tmp");
        let json =
            serde_json::to_vec_pretty(self).map_err(|_| crate::error::AppError::new("error.io"))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!(s.shortcut, "Ctrl+Shift+Space");
        assert!(s.autostart);
        assert!(s.notifications);
        assert!(!s.onboarding_done);
        assert!(!s.belonged_to_space);
        assert_eq!(s.render_mode, None);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        let s = Settings {
            render_mode: Some(RenderMode::Source),
            shortcut: "Ctrl+Shift+K".into(),
            ..Default::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);
    }

    #[test]
    fn corrupt_or_missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn a_file_written_with_an_old_render_mode_keeps_its_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"shortcut":"Ctrl+K","renderMode":"html"}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.shortcut, "Ctrl+K");
        assert_eq!(s.render_mode, Some(RenderMode::Preview));
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"shortcut":"Ctrl+K","futureField":1}"#).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.shortcut, "Ctrl+K");
        assert!(s.autostart);
    }
}
