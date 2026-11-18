//! Popover and history windows.
//!
//! The popover is created hidden at startup (see `tauri.conf.json`), so showing it is
//! instant. It hides itself when it loses focus unless it is pinned.

use std::sync::atomic::Ordering;
use std::time::Instant;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

use crate::core_api::CoreStatus;
use crate::placement::{self, Area};
use crate::state::AppState;

pub const POPOVER: &str = "popover";
pub const HISTORY: &str = "history";

const TABS: &[&str] = &["history", "settings", "devices", "onboarding"];

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn remember_anchor(app: &AppHandle, anchor: Area) {
    *lock(&app.state::<AppState>().tray_anchor) = Some(anchor);
}

pub fn is_popover_visible(app: &AppHandle) -> bool {
    app.get_webview_window(POPOVER)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// Windows Hello is on screen over the popover: it must not stay in front of the prompt (it is
/// always on top), nor hide under it (see `suppress_blur`).
pub fn set_on_top(app: &AppHandle, on_top: bool) {
    if let Some(w) = app.get_webview_window(POPOVER) {
        let _ = w.set_always_on_top(on_top);
    }
}

/// A confirmation that waited in a window is closed whenever that window goes away or (the
/// popover) comes back: its anti-tap delay would be long over, and an irreversible question must
/// not wait for a stray key (`LeaveFlow::window_hidden`). Nothing under way is touched.
fn close_stale_departure_dialog(app: &AppHandle, window: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.departure.window_hidden(window);
    }
}

/// The tab of the full window to show instead of the popover, when this PC is not in a space.
/// The popover has nothing to offer then (no composer, an empty feed), and it cannot host the
/// pairing: it hides as soon as it loses focus, which happens the moment the user looks at the
/// other device. `None`: show the popover. A store that cannot be opened (`unavailable`) keeps the
/// popover, whose banner says why.
fn pairing_tab(status: &CoreStatus, onboarding_done: bool) -> Option<&'static str> {
    if status.paired || status.unavailable.is_some() {
        return None;
    }
    Some(if onboarding_done {
        "devices"
    } else {
        "onboarding"
    })
}

pub fn show_popover(app: &AppHandle) {
    let Some(w) = app.get_webview_window(POPOVER) else {
        return;
    };
    // Every way to open the popover (tray, shortcut, second launch, toast, link) comes through
    // here. `open_history` never comes back to this function.
    let state = app.state::<AppState>();
    let tab = pairing_tab(&state.core.status(), state.settings().onboarding_done);
    if let Some(tab) = tab {
        open_history(app, Some(tab));
        return;
    }
    close_stale_departure_dialog(app, POPOVER);
    let anchor = *lock(&app.state::<AppState>().tray_anchor);
    let monitor = anchor
        .and_then(|a| {
            app.monitor_from_point((a.x + a.w / 2) as f64, (a.y + a.h / 2) as f64)
                .ok()
                .flatten()
        })
        .or_else(|| w.primary_monitor().ok().flatten());
    if let (Some(m), Ok(size)) = (monitor, w.outer_size()) {
        let wa = m.work_area();
        let work = Area {
            x: wa.position.x,
            y: wa.position.y,
            w: wa.size.width as i32,
            h: wa.size.height as i32,
        };
        let (x, y) =
            placement::popover_position(anchor, (size.width as i32, size.height as i32), work);
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    let _ = w.show();
    let _ = w.set_focus();
    let _ = w.emit("popover-shown", ());
}

pub fn hide_popover(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(POPOVER) {
        let _ = w.hide();
    }
    close_stale_departure_dialog(app, POPOVER);
}

/// Left click on the tray icon.
pub fn toggle_from_tray(app: &AppHandle) {
    let state = app.state::<AppState>();
    let hidden_at = *lock(&state.hidden_by_blur_at);
    if placement::ignore_tray_click(hidden_at, Instant::now()) {
        return;
    }
    if is_popover_visible(app) {
        hide_popover(app);
    } else {
        show_popover(app);
    }
}

/// Global shortcut: open, or close when already open and focused.
pub fn toggle_from_shortcut(app: &AppHandle) {
    let focused = app
        .get_webview_window(POPOVER)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if is_popover_visible(app) && focused {
        hide_popover(app);
    } else {
        show_popover(app);
    }
}

/// Called when the popover loses focus.
pub fn on_popover_blur(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.pinned.load(Ordering::Relaxed) || state.suppress_blur.load(Ordering::Relaxed) > 0 {
        return;
    }
    if is_popover_visible(app) {
        hide_popover(app);
        *lock(&state.hidden_by_blur_at) = Some(Instant::now());
    }
}

/// Opens (or focuses) the full window on a tab: history, settings, devices, onboarding.
/// Window creation happens off the main thread, as Windows requires.
pub fn open_history(app: &AppHandle, tab: Option<&str>) {
    let tab = tab.filter(|t| TABS.contains(t)).map(str::to_owned);
    hide_popover(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(w) = app.get_webview_window(HISTORY) {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            if let Some(tab) = &tab {
                let _ = w.emit("navigate", tab);
            }
            return;
        }
        // A question left for a previous instance of this window (a refusal that came while it
        // was closed, an attempt still running when it was closed) must not come back already
        // armed in the new page, without a gesture.
        close_stale_departure_dialog(&app, HISTORY);
        let url = match &tab {
            Some(t) => format!("index.html?view=history&tab={t}"),
            None => "index.html?view=history".to_string(),
        };
        let built = WebviewWindowBuilder::new(&app, HISTORY, WebviewUrl::App(url.into()))
            .title("Tomi")
            .inner_size(980.0, 700.0)
            .min_inner_size(680.0, 480.0)
            .center()
            // Revealed secrets can be on screen here: keep the window out of screenshots
            // and screen sharing.
            .content_protected(true)
            .build();
        // Closing the big window closes the question that waited in it (the state lives in
        // Rust, not in the page that is gone).
        if let Ok(window) = built {
            let handle = app.clone();
            window.on_window_event(move |e| {
                if matches!(e, WindowEvent::Destroyed) {
                    close_stale_departure_dialog(&handle, HISTORY);
                }
            });
        }
    });
}

/// The user is looking at Tomi: the popover is shown (and, kept open with the pin, has the focus:
/// a pinned popover left on the desktop must not keep the decrypted copies alive for ever), or the
/// full window has the focus.
pub fn is_user_active(app: &AppHandle) -> bool {
    let pinned = app.state::<AppState>().pinned.load(Ordering::Relaxed);
    let popover_focused = app
        .get_webview_window(POPOVER)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    popover_in_use(is_popover_visible(app), pinned, popover_focused)
        || app
            .get_webview_window(HISTORY)
            .and_then(|w| w.is_focused().ok())
            .unwrap_or(false)
}

/// A popover that hides itself on blur is in use as long as it is shown; a pinned one stays shown
/// without anyone, so it only counts while it has the focus.
fn popover_in_use(visible: bool, pinned: bool, focused: bool) -> bool {
    visible && (!pinned || focused)
}

/// Brings an item to the user's attention in the popover.
pub fn show_item(app: &AppHandle, id: &str) {
    show_popover(app);
    let _ = app.emit_to(POPOVER, "focus-item", id.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_api::SpaceState;

    fn status(paired: bool, unavailable: Option<&str>) -> CoreStatus {
        CoreStatus {
            paired,
            space: SpaceState::Active,
            unavailable: unavailable.map(str::to_owned),
        }
    }

    #[test]
    fn a_pinned_popover_without_focus_is_not_a_user_in_front_of_the_screen() {
        assert!(popover_in_use(true, false, false));
        assert!(popover_in_use(true, false, true));
        assert!(popover_in_use(true, true, true));
        assert!(!popover_in_use(true, true, false));
        assert!(!popover_in_use(false, true, true));
        assert!(!popover_in_use(false, false, false));
    }

    #[test]
    fn a_paired_pc_gets_the_popover() {
        assert_eq!(pairing_tab(&status(true, None), true), None);
        assert_eq!(pairing_tab(&status(true, None), false), None);
    }

    #[test]
    fn an_unpaired_pc_gets_the_pairing_screen() {
        assert_eq!(pairing_tab(&status(false, None), true), Some("devices"));
        assert_eq!(pairing_tab(&status(false, None), false), Some("onboarding"));
    }

    #[test]
    fn an_unavailable_store_keeps_the_popover_and_its_banner() {
        assert_eq!(pairing_tab(&status(false, Some("locked")), true), None);
    }
}
