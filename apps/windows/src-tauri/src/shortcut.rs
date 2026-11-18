//! Global shortcut: validation of the user-chosen combination and (un)registration.
//!
//! `Ctrl+Alt+<character>` is refused: on AZERTY keyboards `Ctrl+Alt` is `AltGr`, and the
//! combination is "Paste special" in Word/Excel.

use crate::error::{AppError, AppResult};

pub const DEFAULT_SHORTCUT: &str = "Ctrl+Shift+Space";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShortcutError {
    #[error("empty shortcut")]
    Empty,
    #[error("a shortcut needs one non-modifier key")]
    NoKey,
    #[error("only one non-modifier key is allowed")]
    TooManyKeys,
    #[error("unsupported key")]
    UnsupportedKey,
    #[error("a modifier is required")]
    NeedsModifier,
    #[error("Ctrl+Alt is AltGr on many keyboards")]
    AltGr,
}

impl ShortcutError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Empty | Self::NoKey | Self::TooManyKeys | Self::UnsupportedKey => {
                "shortcut.invalid"
            }
            Self::NeedsModifier => "shortcut.needs_modifier",
            Self::AltGr => "shortcut.altgr",
        }
    }
}

const NAMED_KEYS: &[&str] = &[
    "Space", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "Insert", "Delete",
];

/// Normalises a combination such as `ctrl + shift + space` to `Ctrl+Shift+Space`.
pub fn normalize(spec: &str) -> Result<String, ShortcutError> {
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut meta = false;
    let mut key: Option<String> = None;

    let spec = spec.trim();
    if spec.is_empty() {
        return Err(ShortcutError::Empty);
    }
    for raw in spec.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return Err(ShortcutError::UnsupportedKey);
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "commandorcontrol" | "cmdorctrl" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "super" | "win" | "windows" | "meta" | "cmd" | "command" => meta = true,
            _ => {
                if key.is_some() {
                    return Err(ShortcutError::TooManyKeys);
                }
                key = Some(normalize_key(token)?);
            }
        }
    }
    let key = key.ok_or(ShortcutError::NoKey)?;
    let is_function_key = key.starts_with('F') && key.len() > 1;
    if !(ctrl || alt || shift || meta) && !is_function_key {
        return Err(ShortcutError::NeedsModifier);
    }
    let printable = key.len() == 1 || key == "Space";
    if ctrl && alt && printable {
        return Err(ShortcutError::AltGr);
    }

    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if meta {
        parts.push("Super");
    }
    parts.push(&key);
    Ok(parts.join("+"))
}

fn normalize_key(token: &str) -> Result<String, ShortcutError> {
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Ok(c.to_ascii_uppercase().to_string());
        }
        return Err(ShortcutError::UnsupportedKey);
    }
    let lower = token.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Ok(format!("F{n}"));
        }
    }
    NAMED_KEYS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(token))
        .map(|k| (*k).to_string())
        .ok_or(ShortcutError::UnsupportedKey)
}

/// Registers `new` and then drops `old`, so a conflict leaves the previous shortcut
/// working. The handler is installed once, in the plugin builder.
pub fn rebind<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    old: Option<&str>,
    new: &str,
) -> AppResult<String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let normalized = normalize(new).map_err(|e| AppError::new(e.code()))?;
    if old == Some(normalized.as_str()) {
        return Ok(normalized);
    }
    let shortcut: Shortcut = normalized
        .parse()
        .map_err(|_| AppError::new("shortcut.invalid"))?;
    let gs = app.global_shortcut();
    // Another app (or Windows) already owns it: RegisterHotKey fails.
    gs.register(shortcut).map_err(|_| {
        AppError::with(
            "shortcut.conflict",
            serde_json::json!({ "shortcut": normalized }),
        )
    })?;
    if let Some(old) = old {
        if let Ok(old_shortcut) = old.parse::<Shortcut>() {
            let _ = gs.unregister(old_shortcut);
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid_and_stable() {
        assert_eq!(normalize(DEFAULT_SHORTCUT).unwrap(), DEFAULT_SHORTCUT);
    }

    #[test]
    fn normalises_case_spacing_and_order() {
        assert_eq!(
            normalize("shift + ctrl + space").unwrap(),
            "Ctrl+Shift+Space"
        );
        assert_eq!(normalize("CommandOrControl+k").unwrap(), "Ctrl+K");
        assert_eq!(normalize("win+shift+n").unwrap(), "Shift+Super+N");
        assert_eq!(normalize("f9").unwrap(), "F9");
    }

    #[test]
    fn ctrl_alt_with_a_character_is_refused() {
        assert_eq!(normalize("Ctrl+Alt+V"), Err(ShortcutError::AltGr));
        assert_eq!(normalize("Alt+Ctrl+Shift+4"), Err(ShortcutError::AltGr));
        assert_eq!(normalize("Ctrl+Alt+Space"), Err(ShortcutError::AltGr));
        assert!(normalize("Ctrl+Alt+F5").is_ok());
        assert!(normalize("Ctrl+Alt+Left").is_ok());
    }

    #[test]
    fn requires_a_modifier_except_for_function_keys() {
        assert_eq!(normalize("V"), Err(ShortcutError::NeedsModifier));
        assert_eq!(normalize("Space"), Err(ShortcutError::NeedsModifier));
        assert!(normalize("F12").is_ok());
    }

    #[test]
    fn rejects_malformed_specs() {
        assert_eq!(normalize(""), Err(ShortcutError::Empty));
        assert_eq!(normalize("Ctrl+Shift"), Err(ShortcutError::NoKey));
        assert_eq!(normalize("Ctrl+A+B"), Err(ShortcutError::TooManyKeys));
        assert_eq!(normalize("Ctrl++"), Err(ShortcutError::UnsupportedKey));
        assert_eq!(normalize("Ctrl+é"), Err(ShortcutError::UnsupportedKey));
        assert_eq!(normalize("Ctrl+F25"), Err(ShortcutError::UnsupportedKey));
    }

    #[test]
    fn normalised_output_is_accepted_by_the_plugin_parser() {
        use tauri_plugin_global_shortcut::Shortcut;
        for spec in [
            "Ctrl+Shift+Space",
            "Ctrl+K",
            "Ctrl+Shift+7",
            "Shift+Super+N",
            "F9",
            "Ctrl+Alt+F5",
            "Ctrl+Left",
            "Alt+PageUp",
            "Ctrl+Shift+Delete",
            "Ctrl+Insert",
            "Ctrl+Home",
        ] {
            let n = normalize(spec).unwrap();
            assert!(n.parse::<Shortcut>().is_ok(), "{n} not parsed");
        }
    }
}
