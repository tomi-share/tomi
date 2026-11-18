//! Tray icon: left click toggles the popover, right click opens the menu, a red dot
//! marks unread items.

use std::sync::Mutex;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Position, Size};

use crate::placement::Area;
use crate::{popover, strings};

pub const TRAY_ID: &str = "main";

/// How often the taskbar theme and the display scale are re-read (they have no event
/// the tray can subscribe to: the app theme is not the taskbar theme).
const LOOK_POLL: Duration = Duration::from_secs(3);

/// Which of the charter's tray icons to show: `tray-light` (black symbol, for a light
/// taskbar) or `tray-dark` (white symbol), at 16 / 20 / 24 / 32 px (charter, section 8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Look {
    light_taskbar: bool,
    size: u32,
}

/// What the tray currently shows, so that a theme or scale change can redraw it.
#[derive(Clone, Copy)]
struct Shown {
    unread: usize,
    sending: bool,
    look: Look,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

fn tray_png(look: Look) -> &'static [u8] {
    match (look.light_taskbar, look.size) {
        (true, 16) => include_bytes!("../icons/tray/tray-light-16.png"),
        (true, 20) => include_bytes!("../icons/tray/tray-light-20.png"),
        (true, 24) => include_bytes!("../icons/tray/tray-light-24.png"),
        (true, _) => include_bytes!("../icons/tray/tray-light-32.png"),
        (false, 16) => include_bytes!("../icons/tray/tray-dark-16.png"),
        (false, 20) => include_bytes!("../icons/tray/tray-dark-20.png"),
        (false, 24) => include_bytes!("../icons/tray/tray-dark-24.png"),
        (false, _) => include_bytes!("../icons/tray/tray-dark-32.png"),
    }
}

/// 16 px x the display scale, rounded up to a drawn size: 100 % -> 16, 125 % -> 20,
/// 150 % -> 24, 175 % and above -> 32.
fn size_for_scale(scale: f64) -> u32 {
    if scale <= 1.0 {
        16
    } else if scale <= 1.25 {
        20
    } else if scale <= 1.5 {
        24
    } else {
        32
    }
}

/// `SystemUsesLightTheme` is the taskbar theme (`AppsUseLightTheme` is the apps').
/// Missing value: older Windows, whose taskbar is dark.
#[cfg(windows)]
fn taskbar_is_light() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|v| v != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn taskbar_is_light() -> bool {
    false
}

fn current_look(app: &AppHandle) -> Look {
    let scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    Look {
        light_taskbar: taskbar_is_light(),
        size: size_for_scale(scale),
    }
}

/// Decoded tray icon for this look, with the unread dot when there is something unread.
fn icon_for(look: Look, unread: usize) -> Option<Image<'static>> {
    let img = image::load_from_memory(tray_png(look)).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let rgba = img.into_raw();
    let rgba = if unread > 0 {
        with_badge(&rgba, w, h)
    } else {
        rgba
    };
    Some(Image::new_owned(rgba, w, h))
}
/// Draws the "unread" dot (red disc, white ring) in the top-right corner.
pub fn with_badge(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let radius = (w.min(h) as f32) * 0.24;
    let ring = (w.min(h) as f32 / 32.0).max(1.0);
    let cx = w as f32 - radius - 1.0;
    let cy = radius + 1.0;
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let px = ((y * w + x) * 4) as usize;
            if d <= radius - ring {
                out[px..px + 4].copy_from_slice(&[220, 38, 38, 255]);
            } else if d <= radius {
                out[px..px + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    out
}

fn to_i32(p: Position, s: Size) -> Area {
    let (x, y) = match p {
        Position::Physical(p) => (p.x, p.y),
        Position::Logical(p) => (p.x as i32, p.y as i32),
    };
    let (w, h) = match s {
        Size::Physical(s) => (s.width as i32, s.height as i32),
        Size::Logical(s) => (s.width as i32, s.height as i32),
    };
    Area { x, y, w, h }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .item(&MenuItemBuilder::with_id("open", strings::TRAY_OPEN).build(app)?)
        .item(&MenuItemBuilder::with_id("history", strings::TRAY_HISTORY).build(app)?)
        .item(&MenuItemBuilder::with_id("settings", strings::TRAY_SETTINGS).build(app)?)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&MenuItemBuilder::with_id("quit", strings::TRAY_QUIT).build(app)?)
        .build()?;

    let look = current_look(app);
    let icon = match icon_for(look, 0) {
        Some(i) => i,
        None => app
            .default_window_icon()
            .cloned()
            .ok_or_else(|| tauri::Error::InvalidIcon(std::io::Error::other("no tray icon")))?,
    };
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Shown {
        unread: 0,
        sending: false,
        look,
    });
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(strings::APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => popover::show_popover(app),
            "history" => popover::open_history(app, Some("history")),
            "settings" => popover::open_history(app, Some("settings")),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                popover::remember_anchor(app, to_i32(rect.position, rect.size));
                popover::toggle_from_tray(app);
            }
        })
        .build(app)?;
    Ok(())
}

/// Refreshes the unread dot and the tooltip.
pub fn update(app: &AppHandle, unread: usize, sending: bool) {
    apply(app, unread, sending, current_look(app));
}

fn apply(app: &AppHandle, unread: usize, sending: bool, look: Look) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.set_tooltip(Some(strings::tooltip(unread, sending)));
    if let Some(icon) = icon_for(look, unread) {
        let _ = tray.set_icon(Some(icon));
    }
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Shown {
        unread,
        sending,
        look,
    });
}

/// Swaps the icon when the taskbar goes light or dark, or when the display scale changes.
pub async fn watch_look(app: AppHandle) {
    let mut tick = tokio::time::interval(LOOK_POLL);
    loop {
        tick.tick().await;
        let shown = *SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = shown {
            let look = current_look(&app);
            if look != s.look {
                apply(&app, s.unread, s.sending, look);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_tray_icon_decodes_at_its_size() {
        for light_taskbar in [true, false] {
            for size in [16, 20, 24, 32] {
                let look = Look {
                    light_taskbar,
                    size,
                };
                let img = image::load_from_memory(tray_png(look)).expect("tray icon decodes");
                assert_eq!((img.width(), img.height()), (size, size));
                assert!(icon_for(look, 1).is_some());
            }
        }
    }

    #[test]
    fn the_size_follows_the_display_scale() {
        assert_eq!(size_for_scale(1.0), 16);
        assert_eq!(size_for_scale(1.25), 20);
        assert_eq!(size_for_scale(1.5), 24);
        assert_eq!(size_for_scale(1.75), 32);
        assert_eq!(size_for_scale(2.0), 32);
        assert_eq!(size_for_scale(3.0), 32);
    }

    #[test]
    fn badge_paints_a_red_dot_top_right_and_leaves_the_rest() {
        let (w, h) = (32u32, 32u32);
        let base = vec![10u8; (w * h * 4) as usize];
        let out = with_badge(&base, w, h);
        assert_eq!(out.len(), base.len());
        // Centre of the dot.
        let radius = 32.0 * 0.24;
        let (cx, cy) = ((w as f32 - radius - 1.0) as u32, (radius + 1.0) as u32);
        let i = ((cy * w + cx) * 4) as usize;
        assert_eq!(&out[i..i + 4], &[220, 38, 38, 255]);
        // Bottom-left corner untouched.
        let j = (((h - 1) * w) * 4) as usize;
        assert_eq!(&out[j..j + 4], &base[j..j + 4]);
    }
}
