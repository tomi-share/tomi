//! Closes the Windows Hello window (`unlock::SecretGate`, 2 minutes) when the user's session is
//! locked, disconnected or logged off, or when the PC goes to sleep: whoever sits down at the PC
//! afterwards must verify again before a hidden item is shown, copied or opened.
//!
//! Tauri does not expose these events, so a hidden top-level window (never shown) lives on a
//! thread of its own with its own message loop. It registers for the session notifications of
//! its session (`WTSRegisterSessionNotification`) and receives the power broadcast
//! (`WM_POWERBROADCAST`), which only top-level windows get. The window procedure only calls the
//! callback it was started with, and decides what closes the gate in [`closes_gate`]. The callback
//! also tells the webviews to mask what they show ([`HIDE_EVENT`]).

/// Event sent to every webview when the gate closes: each one masks what it shows of hidden
/// content (`events.sessionLocked` in `api.ts`).
pub const HIDE_EVENT: &str = "session-locked";

/// `WM_WTSSESSION_CHANGE` (`wParam` = the event below).
pub const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
/// `WM_POWERBROADCAST` (`wParam` = the event below).
pub const WM_POWERBROADCAST: u32 = 0x0218;

const WTS_CONSOLE_DISCONNECT: usize = 0x2;
const WTS_REMOTE_DISCONNECT: usize = 0x4;
const WTS_SESSION_LOGOFF: usize = 0x6;
const WTS_SESSION_LOCK: usize = 0x7;
const PBT_APMSUSPEND: usize = 0x4;

/// True for the messages after which the next hidden-content access must ask Windows Hello again:
/// lock, disconnect (console or remote), logoff, suspend. Unlock and resume do not reopen
/// anything, and every other message is none of our business.
pub fn closes_gate(msg: u32, wparam: usize) -> bool {
    match msg {
        WM_WTSSESSION_CHANGE => matches!(
            wparam,
            WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT | WTS_SESSION_LOGOFF
        ),
        WM_POWERBROADCAST => wparam == PBT_APMSUSPEND,
        _ => false,
    }
}

/// Starts watching; `on_close` runs on the watcher thread each time the gate must close. Best
/// effort: when the window cannot be created the 2-minute window simply stays as it was.
#[cfg(windows)]
pub fn start(on_close: impl Fn() + Send + Sync + 'static) {
    imp::start(Box::new(on_close));
}

#[cfg(not(windows))]
pub fn start(_on_close: impl Fn() + Send + Sync + 'static) {}

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, CW_USEDEFAULT, MSG, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPED,
    };

    use super::closes_gate;

    type Callback = Box<dyn Fn() + Send + Sync>;
    static ON_CLOSE: OnceLock<Callback> = OnceLock::new();

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if closes_gate(msg, wparam.0) {
            if let Some(f) = ON_CLOSE.get() {
                f();
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    pub fn start(on_close: Callback) {
        // One watcher per process.
        if ON_CLOSE.set(on_close).is_err() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("tomi-session-watch".into())
            .spawn(run);
    }

    fn run() {
        // SAFETY: plain Win32 calls on this thread, which owns the window and its message loop
        // for the life of the process.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else {
                return;
            };
            let instance = HINSTANCE(module.0);
            let class = w!("TomiSessionWatch");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return;
            }
            // Never shown: only there to receive messages (a top-level window, because the
            // power broadcast is not sent to message-only windows).
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Tomi"),
                WS_OVERLAPPED,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            ) else {
                return;
            };
            // Without this registration the power events still work; only lock and
            // disconnect are missed.
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_disconnect_logoff_and_suspend_close_the_gate() {
        for event in [0x7, 0x2, 0x4, 0x6] {
            assert!(closes_gate(WM_WTSSESSION_CHANGE, event), "{event:#x}");
        }
        assert!(closes_gate(WM_POWERBROADCAST, 0x4));
    }

    #[test]
    fn the_hide_event_is_the_one_the_webviews_listen_to() {
        // `events.sessionLocked` in `src/lib/api.ts` listens to this exact name.
        assert_eq!(HIDE_EVENT, "session-locked");
        let api = include_str!("../../src/lib/api.ts");
        assert!(api.contains(&format!("listen(\"{HIDE_EVENT}\"")));
    }

    #[test]
    fn unlock_resume_and_other_messages_do_not() {
        // WTS_SESSION_UNLOCK, WTS_CONSOLE_CONNECT, WTS_SESSION_LOGON.
        for event in [0x8, 0x1, 0x5] {
            assert!(!closes_gate(WM_WTSSESSION_CHANGE, event), "{event:#x}");
        }
        // PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND.
        for event in [0x12, 0x7] {
            assert!(!closes_gate(WM_POWERBROADCAST, event), "{event:#x}");
        }
        // The event numbers mean nothing for another message.
        assert!(!closes_gate(0x0010, 0x7));
    }
}
