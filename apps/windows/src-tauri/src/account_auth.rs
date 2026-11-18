//! Fallback of the strong verification: the password of the current Windows account.
//!
//! Windows Hello is the first choice (see `strong_auth.rs`). On a PC where it cannot be used
//! (no sensor and no PIN set up, disabled by policy) the user would be locked out of every
//! action that needs a verification. This module then asks for the password of the Windows
//! account the app runs under, through the system's own credential dialog. It is WEAKER than
//! Hello (a password can be typed by anyone who knows it, no biometric factor), but usable
//! everywhere; golden rule 3 (secrets) is unchanged: it only gates the same actions.
//!
//! Rules (the logic below is platform independent and tested; the Windows part is a thin
//! [`AccountPort`]):
//! * only used when Hello says [`AuthOutcome::Unavailable`] (never after a Hello failure or a
//!   cancellation: see [`WithFallback`]). Unavailable also covers a Hello that declares itself
//!   so at the moment of the request (remote desktop session, sensor unplugged);
//! * the account that answers MUST be the current user: the check compares the user SID of the
//!   logon token with the SID of the running process, whatever name was typed; any other
//!   account is refused;
//! * an empty password never verifies and counts as a failed try (a new prompt with the error).
//!   [`AuthOutcome::NoPassword`] is only answered when the system itself says the account is
//!   restricted (`ERROR_ACCOUNT_RESTRICTION`, which is what a blank-password account gives) or
//!   when the field was empty at every try;
//! * [`MAX_TRIES`] wrong passwords end in [`AuthOutcome::Failed`]; and [`MAX_TRIES`] wrong
//!   passwords cumulated ACROSS actions (in-memory counter, reset by a success) refuse every
//!   action for [`LOCK_FOR`] without calling the system at all ([`AuthOutcome::TooManyTries`]);
//!   a locked-out account is reported as such ([`AuthOutcome::AccountLocked`]);
//! * the password lives in a `Zeroizing` buffer, wiped as soon as the check is done; it is never
//!   logged, never returned: only an [`AuthOutcome`] leaves;
//! * the prompt obeys the same `cancel` flag as Hello. The prompt is drawn by the system, perhaps
//!   by another process (`CredentialUIBroker.exe`): closing it is attempted on a best-effort
//!   basis and may not work. What is guaranteed is that after the timeout the action is refused
//!   and a later answer is ignored; the prompt itself may stay on screen.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::strong_auth::{AuthOutcome, StrongAuth};

/// Wrong passwords tolerated before the action is refused (and, cumulated across actions,
/// before the lock of [`LOCK_FOR`]).
pub const MAX_TRIES: usize = 3;

/// How long every action is refused after [`MAX_TRIES`] cumulated wrong passwords.
pub const LOCK_FOR: Duration = Duration::from_secs(60);

/// What the credential dialog returned.
pub enum Ask {
    /// The user closed the dialog (or it was closed because nobody was waiting).
    Cancelled,
    /// The dialog could not be shown or read.
    Error,
    Entered(Credentials),
}

/// What was typed. The password is wiped when this is dropped.
pub struct Credentials {
    pub user: String,
    pub domain: String,
    /// UTF-16, without a trailing NUL.
    pub password: Zeroizing<Vec<u16>>,
}

/// Result of presenting credentials to the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logon {
    /// Right password, and the account is the current user.
    Correct,
    /// Wrong name or password.
    Wrong,
    /// The credentials are valid but belong to another account.
    OtherAccount,
    /// `ERROR_ACCOUNT_RESTRICTION`: what a blank-password account answers.
    Restricted,
    /// `ERROR_ACCOUNT_LOCKED_OUT`.
    LockedOut,
    /// The system could not decide.
    Error,
}

/// The two system calls, behind a trait so that the logic is tested without Windows.
pub trait AccountPort: Send + Sync {
    /// Shows the dialog. `retry` is true after a wrong password (the dialog says so).
    fn ask(&self, owner: Option<isize>, message: &str, cancel: &AtomicBool, retry: bool) -> Ask;
    fn logon(&self, credentials: &Credentials) -> Logon;
}

/// Wrong passwords cumulated across actions, in memory only.
#[derive(Default)]
pub struct Throttle {
    state: Mutex<ThrottleState>,
}

#[derive(Default)]
struct ThrottleState {
    failures: usize,
    locked_until: Option<Instant>,
}

impl Throttle {
    /// Is the fallback locked at `now`? An expired lock is lifted (the count restarts).
    pub fn is_locked(&self, now: Instant) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match s.locked_until {
            Some(until) if now < until => true,
            Some(_) => {
                s.locked_until = None;
                s.failures = 0;
                false
            }
            None => false,
        }
    }

    /// Records a wrong password; true when this locks the fallback.
    fn failed(&self, now: Instant) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.failures += 1;
        if s.failures >= MAX_TRIES {
            s.locked_until = Some(now + LOCK_FOR);
            true
        } else {
            false
        }
    }

    fn succeeded(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.failures = 0;
        s.locked_until = None;
    }
}

/// Runs the whole fallback with `port`. `now` is the clock (injected by the tests).
pub fn verify_account(
    port: &dyn AccountPort,
    owner: Option<isize>,
    message: &str,
    cancel: &AtomicBool,
    throttle: &Throttle,
    now: &dyn Fn() -> Instant,
) -> AuthOutcome {
    if throttle.is_locked(now()) {
        // Neither the dialog nor the system is called.
        return AuthOutcome::TooManyTries;
    }
    let mut only_empty = true;
    for attempt in 0..MAX_TRIES {
        if cancel.load(Ordering::SeqCst) {
            return AuthOutcome::Cancelled;
        }
        let credentials = match port.ask(owner, message, cancel, attempt > 0) {
            Ask::Cancelled => return AuthOutcome::Cancelled,
            Ask::Error => return AuthOutcome::Failed,
            Ask::Entered(c) => c,
        };
        if cancel.load(Ordering::SeqCst) {
            return AuthOutcome::Cancelled;
        }
        let logon = if credentials.password.is_empty() {
            // An empty password proves nothing and the system is not asked: a failed try.
            Logon::Wrong
        } else {
            only_empty = false;
            port.logon(&credentials)
        };
        // Wipe the password now, before anything else happens.
        drop(credentials);
        // A late answer is never read: the refusal has already been given.
        if cancel.load(Ordering::SeqCst) {
            return AuthOutcome::Cancelled;
        }
        match logon {
            Logon::Correct => {
                throttle.succeeded();
                return AuthOutcome::Verified;
            }
            Logon::Restricted => return AuthOutcome::NoPassword,
            Logon::LockedOut => return AuthOutcome::AccountLocked,
            Logon::Error => return AuthOutcome::Failed,
            Logon::Wrong | Logon::OtherAccount => {
                if throttle.failed(now()) && attempt + 1 < MAX_TRIES {
                    return AuthOutcome::TooManyTries;
                }
            }
        }
    }
    if only_empty {
        AuthOutcome::NoPassword
    } else {
        AuthOutcome::Failed
    }
}

/// The fallback as a [`StrongAuth`].
pub struct AccountAuth<P: AccountPort> {
    port: P,
    throttle: Throttle,
    clock: fn() -> Instant,
}

impl<P: AccountPort> AccountAuth<P> {
    pub fn new(port: P) -> Self {
        Self {
            port,
            throttle: Throttle::default(),
            clock: Instant::now,
        }
    }
}

impl<P: AccountPort> StrongAuth for AccountAuth<P> {
    fn verify(&self, owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome {
        verify_account(
            &self.port,
            owner,
            crate::strings::account_prompt(message),
            cancel,
            &self.throttle,
            &self.clock,
        )
    }
}

/// `primary` (Windows Hello) first; `fallback` only when `primary` says it is unavailable. A
/// cancellation, a failure or a busy device of the primary never reaches the fallback. The
/// fallback maps the Hello sentence to its own wording (`strings::account_prompt`).
pub struct WithFallback {
    pub primary: Arc<dyn StrongAuth>,
    pub fallback: Arc<dyn StrongAuth>,
}

impl StrongAuth for WithFallback {
    fn verify(&self, owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome {
        match self.primary.verify(owner, message, cancel) {
            AuthOutcome::Unavailable => self.fallback.verify(owner, message, cancel),
            other => other,
        }
    }
}

#[cfg(windows)]
pub use win::WindowsAccount;

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    use windows::core::{BOOL, HRESULT, HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_CANCELLED, ERROR_LOGON_FAILURE, HANDLE, HWND, LPARAM, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::HBITMAP;
    use windows::Win32::Security::Credentials::{
        CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
        CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW, CRED_PACK_FLAGS,
    };
    use windows::Win32::Security::{
        EqualSid, GetTokenInformation, LogonUserW, TokenUser, LOGON32_LOGON_INTERACTIVE,
        LOGON32_PROVIDER_DEFAULT, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, OpenProcessToken,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumThreadWindows, EnumWindows, GetWindow, GetWindowThreadProcessId, PostMessageW,
        GW_OWNER, WM_CLOSE,
    };
    use zeroize::Zeroizing;

    use super::{AccountPort, Ask, Credentials, Logon};

    /// How often the open dialog is looked at (to close it when nobody waits any more).
    const WATCH_EVERY: Duration = Duration::from_millis(100);
    /// How long a closing dialog gets to answer before the call gives up on it.
    const CLOSE_GRACE: Duration = Duration::from_secs(3);

    /// `ERROR_ACCOUNT_RESTRICTION`: a blank-password account (or a logon restriction).
    const ERROR_ACCOUNT_RESTRICTION_CODE: u32 = 1327;
    /// `ERROR_ACCOUNT_LOCKED_OUT`.
    const ERROR_ACCOUNT_LOCKED_OUT_CODE: u32 = 1909;

    // `lmcons.h` / `wincred.h` maxima, with a margin.
    const USER_MAX: usize = 520;
    const DOMAIN_MAX: usize = 260;
    const PASSWORD_MAX: usize = 520;

    /// The credential dialog of Windows and `LogonUserW`.
    pub struct WindowsAccount;

    enum Msg {
        Started(u32),
        Done(Ask),
    }

    impl AccountPort for WindowsAccount {
        fn ask(
            &self,
            owner: Option<isize>,
            message: &str,
            cancel: &AtomicBool,
            retry: bool,
        ) -> Ask {
            let Some(owner) = owner.filter(|h| *h != 0) else {
                // No window to own the dialog: refuse rather than show it ownerless.
                return Ask::Error;
            };
            let (tx, rx) = mpsc::channel::<Msg>();
            let text = message.to_owned();
            let spawned = std::thread::Builder::new()
                .name("tomi-account".into())
                .spawn(move || {
                    let _ = tx.send(Msg::Started(unsafe { GetCurrentThreadId() }));
                    let answer = prompt(owner, &text, retry);
                    // If nobody listens any more, the credentials are dropped (wiped) here.
                    let _ = tx.send(Msg::Done(answer));
                });
            if spawned.is_err() {
                return Ask::Error;
            }
            let mut thread_id = None;
            let mut closing_since = None;
            loop {
                match rx.recv_timeout(WATCH_EVERY) {
                    Ok(Msg::Started(id)) => thread_id = Some(id),
                    Ok(Msg::Done(answer)) => return answer,
                    Err(RecvTimeoutError::Timeout) => {
                        if cancel.load(Ordering::SeqCst) {
                            if let Some(id) = thread_id {
                                close_windows_of(id);
                            }
                            close_foreign_windows_owned_by(owner);
                            let since = closing_since.get_or_insert_with(std::time::Instant::now);
                            if since.elapsed() >= CLOSE_GRACE {
                                // The dialog would not close (it may be drawn by another
                                // process and stay on screen): give up on it. Whatever it
                                // returns later is dropped by the thread, never read.
                                return Ask::Cancelled;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return Ask::Error,
                }
            }
        }

        fn logon(&self, credentials: &Credentials) -> Logon {
            logon(credentials)
        }
    }

    /// Best effort, NOT known to work: the credential dialog is probably drawn by
    /// `CredentialUIBroker.exe`, a process of its own, so the thread enumeration below cannot
    /// reach it. This also asks to close the top-level windows of OTHER processes whose owner is
    /// the Tomi window. If it does not work, the prompt stays on screen and its answer is ignored.
    fn close_foreign_windows_owned_by(owner: isize) {
        unsafe extern "system" fn close(hwnd: HWND, owner: LPARAM) -> BOOL {
            let owned = unsafe { GetWindow(hwnd, GW_OWNER) }
                .map(|w| w.0 as isize == owner.0)
                .unwrap_or(false);
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            // Windows of this process are not touched here (the thread enumeration covers the
            // dialog's own, and Tomi's other windows must stay).
            if owned && pid != unsafe { GetCurrentProcessId() } {
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
            BOOL(1)
        }
        let _ = unsafe { EnumWindows(Some(close), LPARAM(owner)) };
    }

    /// Asks every window the dialog thread owns to close.
    fn close_windows_of(thread_id: u32) {
        unsafe extern "system" fn close(hwnd: HWND, _: LPARAM) -> BOOL {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            BOOL(1)
        }
        let _ = unsafe { EnumThreadWindows(thread_id, Some(close), LPARAM(0)) };
    }

    /// The dialog itself (blocks). Only the tile of the current user is offered.
    fn prompt(owner: isize, message: &str, retry: bool) -> Ask {
        // A thread that is already initialised is fine; every successful call (S_OK or S_FALSE)
        // is balanced by a CoUninitialize when `_com` drops, a failed one (e.g. another
        // apartment model) is not.
        let _com = ComGuard(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok());
        let caption = HSTRING::from("Tomi");
        let text = HSTRING::from(message);
        let info = CREDUI_INFOW {
            cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
            hwndParent: HWND(owner as *mut c_void),
            pszMessageText: PCWSTR(text.as_ptr()),
            pszCaptionText: PCWSTR(caption.as_ptr()),
            hbmBanner: HBITMAP::default(),
        };
        let mut package = 0u32;
        let mut out: *mut c_void = std::ptr::null_mut();
        let mut out_size = 0u32;
        let auth_error = if retry { ERROR_LOGON_FAILURE.0 } else { 0 };
        let rc = unsafe {
            CredUIPromptForWindowsCredentialsW(
                Some(&info),
                auth_error,
                &mut package,
                None,
                0,
                &mut out,
                &mut out_size,
                None,
                CREDUIWIN_ENUMERATE_CURRENT_USER,
            )
        };
        if rc == ERROR_CANCELLED.0 {
            return Ask::Cancelled;
        }
        if rc != 0 || out.is_null() {
            // Even on an error the system may have handed a buffer back: wipe and free it.
            if !out.is_null() {
                wipe_and_free(out, out_size);
            }
            return Ask::Error;
        }
        let answer = unpack(out, out_size);
        wipe_and_free(out, out_size);
        answer
    }

    /// The buffer holds the typed password: wipe it before giving it back to the system.
    fn wipe_and_free(buffer: *mut c_void, size: u32) {
        unsafe {
            std::ptr::write_bytes(buffer as *mut u8, 0, size as usize);
            CoTaskMemFree(Some(buffer));
        }
    }

    /// Balances a successful `CoInitializeEx` on the thread.
    struct ComGuard(bool);
    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn unpack(buffer: *mut c_void, size: u32) -> Ask {
        let mut user = Zeroizing::new(vec![0u16; USER_MAX]);
        let mut domain = Zeroizing::new(vec![0u16; DOMAIN_MAX]);
        let mut password = Zeroizing::new(vec![0u16; PASSWORD_MAX]);
        let (mut user_len, mut domain_len, mut password_len) =
            (USER_MAX as u32, DOMAIN_MAX as u32, PASSWORD_MAX as u32);
        let ok = unsafe {
            CredUnPackAuthenticationBufferW(
                CRED_PACK_FLAGS(0),
                buffer,
                size,
                Some(PWSTR(user.as_mut_ptr())),
                &mut user_len,
                Some(PWSTR(domain.as_mut_ptr())),
                Some(&mut domain_len),
                Some(PWSTR(password.as_mut_ptr())),
                &mut password_len,
            )
        };
        if ok.is_err() {
            return Ask::Error;
        }
        let until_nul = |v: &[u16]| v.iter().position(|c| *c == 0).unwrap_or(v.len());
        let user_text = String::from_utf16_lossy(&user[..until_nul(&user)]);
        let domain_text = String::from_utf16_lossy(&domain[..until_nul(&domain)]);
        let password_len = until_nul(&password);
        password.truncate(password_len);
        Ask::Entered(Credentials {
            user: user_text,
            domain: domain_text,
            password,
        })
    }

    /// Splits what was typed into the (user, domain) `LogonUserW` wants: `DOMAIN\user` is split,
    /// a UPN (`user@domain`) keeps a null domain, a bare name is a local account (`.`).
    pub(super) fn split_account(user: &str, domain: &str) -> (String, Option<String>) {
        if !domain.is_empty() {
            return (user.to_owned(), Some(domain.to_owned()));
        }
        if let Some((d, u)) = user.split_once('\\') {
            return (u.to_owned(), Some(d.to_owned()));
        }
        if user.contains('@') {
            return (user.to_owned(), None);
        }
        (user.to_owned(), Some(".".to_owned()))
    }

    fn logon(credentials: &Credentials) -> Logon {
        let (user, domain) = split_account(&credentials.user, &credentials.domain);
        let user = HSTRING::from(user);
        let domain = domain.map(HSTRING::from);
        // Built with its final size: no reallocation, so no stale clear copy left behind. This
        // is the only copy of the password besides `Credentials` (wiped by its own owner).
        let mut password: Zeroizing<Vec<u16>> =
            Zeroizing::new(Vec::with_capacity(credentials.password.len() + 1));
        password.extend_from_slice(&credentials.password);
        password.push(0);
        let mut token = HANDLE::default();
        let result = unsafe {
            LogonUserW(
                PCWSTR(user.as_ptr()),
                domain
                    .as_ref()
                    .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
                PCWSTR(password.as_ptr()),
                // INTERACTIVE (not NETWORK): also accepts the cached password of a Microsoft or
                // domain account while offline. The SID check below is unchanged.
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        };
        drop(password);
        match result {
            Ok(()) => {
                let token = Handle(token);
                match same_user_as_process(&token) {
                    Some(true) => Logon::Correct,
                    Some(false) => Logon::OtherAccount,
                    None => Logon::Error,
                }
            }
            Err(e) if e.code() == HRESULT::from_win32(ERROR_LOGON_FAILURE.0) => Logon::Wrong,
            Err(e) if e.code() == HRESULT::from_win32(ERROR_ACCOUNT_RESTRICTION_CODE) => {
                Logon::Restricted
            }
            Err(e) if e.code() == HRESULT::from_win32(ERROR_ACCOUNT_LOCKED_OUT_CODE) => {
                Logon::LockedOut
            }
            Err(_) => Logon::Error,
        }
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// The `TOKEN_USER` of a token. The SID inside points into the returned buffer (the heap
    /// block does not move when the `Vec` does), so the buffer must outlive the SID.
    fn token_user(token: HANDLE) -> Option<Vec<u64>> {
        let mut needed = 0u32;
        // First call: only the size.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        // u64 elements: the buffer is aligned for TOKEN_USER.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut c_void),
                needed,
                &mut needed,
            )
        }
        .ok()?;
        Some(buf)
    }

    fn same_user_as_process(logon_token: &Handle) -> Option<bool> {
        let mut process = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process) }.ok()?;
        let process = Handle(process);
        let a = token_user(logon_token.0)?;
        let b = token_user(process.0)?;
        // The SIDs point into `a` and `b`, which are alive here.
        let sid_a = unsafe { (*(a.as_ptr() as *const TOKEN_USER)).User.Sid };
        let sid_b = unsafe { (*(b.as_ptr() as *const TOKEN_USER)).User.Sid };
        Some(unsafe { EqualSid(sid_a, sid_b) }.is_ok())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_token_of_this_process_is_the_current_user() {
            let mut token = HANDLE::default();
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.unwrap();
            let token = Handle(token);
            assert_eq!(same_user_as_process(&token), Some(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strong_auth::{require_within, ScriptedAuth};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn typed(password: &str) -> Ask {
        Ask::Entered(Credentials {
            user: r"PC\moi".into(),
            domain: String::new(),
            password: Zeroizing::new(password.encode_utf16().collect()),
        })
    }

    /// Scripted dialog + logon. `block` makes the dialog wait for `cancel`; `raise` is a flag the
    /// logon sets (a cancellation arriving while the system checks the password).
    struct FakePort {
        asks: Mutex<VecDeque<Ask>>,
        logons: Mutex<VecDeque<Logon>>,
        asked: Mutex<Vec<bool>>,
        logon_calls: Mutex<usize>,
        block: bool,
        raise: Option<Arc<AtomicBool>>,
    }

    impl FakePort {
        fn new(asks: Vec<Ask>, logons: &[Logon]) -> Self {
            Self {
                asks: Mutex::new(asks.into()),
                logons: Mutex::new(logons.iter().copied().collect()),
                asked: Mutex::new(Vec::new()),
                logon_calls: Mutex::new(0),
                block: false,
                raise: None,
            }
        }
    }

    impl AccountPort for FakePort {
        fn ask(&self, _o: Option<isize>, _m: &str, cancel: &AtomicBool, retry: bool) -> Ask {
            self.asked.lock().unwrap().push(retry);
            if self.block {
                while !cancel.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Ask::Cancelled;
            }
            self.asks.lock().unwrap().pop_front().unwrap_or(Ask::Error)
        }
        fn logon(&self, _c: &Credentials) -> Logon {
            *self.logon_calls.lock().unwrap() += 1;
            if let Some(flag) = &self.raise {
                flag.store(true, Ordering::SeqCst);
            }
            self.logons
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Logon::Error)
        }
    }

    fn run(port: &FakePort) -> AuthOutcome {
        run_with(port, &Throttle::default(), &Instant::now)
    }

    fn run_with(port: &FakePort, throttle: &Throttle, now: &dyn Fn() -> Instant) -> AuthOutcome {
        verify_account(port, Some(1), "m", &AtomicBool::new(false), throttle, now)
    }

    #[test]
    fn the_right_password_of_the_current_account_verifies() {
        let p = FakePort::new(vec![typed("secret")], &[Logon::Correct]);
        assert_eq!(run(&p), AuthOutcome::Verified);
    }

    #[test]
    fn another_account_is_refused() {
        let p = FakePort::new(
            vec![typed("a"), typed("a"), typed("a")],
            &[Logon::OtherAccount; 3],
        );
        assert_eq!(run(&p), AuthOutcome::Failed);
    }

    #[test]
    fn an_empty_password_is_a_failed_try_and_the_system_is_not_asked() {
        // Empty, then right: the empty one only cost a try (the dialog is shown again).
        let p = FakePort::new(vec![typed(""), typed("ok")], &[Logon::Correct]);
        assert_eq!(run(&p), AuthOutcome::Verified);
        assert_eq!(*p.logon_calls.lock().unwrap(), 1);
        assert_eq!(*p.asked.lock().unwrap(), vec![false, true]);
    }

    #[test]
    fn an_empty_field_at_every_try_means_no_password() {
        let p = FakePort::new(vec![typed(""), typed(""), typed("")], &[Logon::Correct]);
        assert_eq!(run(&p), AuthOutcome::NoPassword);
        assert_eq!(*p.logon_calls.lock().unwrap(), 0);
        // Empty then wrong is a plain failure, not "no password".
        let p = FakePort::new(vec![typed(""), typed("x"), typed("")], &[Logon::Wrong]);
        assert_eq!(run(&p), AuthOutcome::Failed);
    }

    #[test]
    fn only_the_system_saying_restricted_means_no_password() {
        let p = FakePort::new(vec![typed("x")], &[Logon::Restricted]);
        assert_eq!(run(&p), AuthOutcome::NoPassword);
    }

    #[test]
    fn a_locked_out_account_has_its_own_outcome() {
        let p = FakePort::new(vec![typed("x")], &[Logon::LockedOut]);
        assert_eq!(run(&p), AuthOutcome::AccountLocked);
    }

    #[test]
    fn three_cumulated_failures_lock_for_a_minute_without_asking_anything() {
        let t0 = Instant::now();
        let throttle = Throttle::default();
        // Two actions: 2 failures then a cancel, then 1 failure (3 cumulated).
        let p = FakePort::new(
            vec![typed("a"), typed("b"), Ask::Cancelled],
            &[Logon::Wrong; 2],
        );
        assert_eq!(run_with(&p, &throttle, &|| t0), AuthOutcome::Cancelled);
        let p = FakePort::new(vec![typed("c"), typed("d")], &[Logon::Wrong; 2]);
        assert_eq!(
            run_with(&p, &throttle, &|| t0),
            AuthOutcome::TooManyTries,
            "the third cumulated failure locks"
        );
        // Locked: no dialog, no system call.
        let p = FakePort::new(vec![typed("right")], &[Logon::Correct]);
        let at = t0 + Duration::from_secs(59);
        assert_eq!(run_with(&p, &throttle, &|| at), AuthOutcome::TooManyTries);
        assert!(p.asked.lock().unwrap().is_empty());
        assert_eq!(*p.logon_calls.lock().unwrap(), 0);
        // After a minute the fallback works again.
        let later = t0 + LOCK_FOR + Duration::from_millis(1);
        assert_eq!(run_with(&p, &throttle, &|| later), AuthOutcome::Verified);
    }

    #[test]
    fn a_success_resets_the_cumulated_failures() {
        let t0 = Instant::now();
        let throttle = Throttle::default();
        let p = FakePort::new(
            vec![typed("a"), typed("b"), typed("ok")],
            &[Logon::Wrong, Logon::Wrong, Logon::Correct],
        );
        assert_eq!(run_with(&p, &throttle, &|| t0), AuthOutcome::Verified);
        let p = FakePort::new(vec![typed("a"), typed("b"), typed("c")], &[Logon::Wrong; 3]);
        assert_eq!(run_with(&p, &throttle, &|| t0), AuthOutcome::Failed);
        assert!(throttle.is_locked(t0), "3 new failures lock again");
    }

    #[test]
    fn three_wrong_passwords_fail_and_the_dialog_says_retry_after_the_first() {
        let p = FakePort::new(
            vec![typed("a"), typed("b"), typed("c"), typed("d")],
            &[Logon::Wrong; 4],
        );
        assert_eq!(run(&p), AuthOutcome::Failed);
        assert_eq!(*p.asked.lock().unwrap(), vec![false, true, true]);
        assert_eq!(*p.logon_calls.lock().unwrap(), MAX_TRIES);
    }

    #[test]
    fn a_wrong_then_a_right_password_verifies() {
        let p = FakePort::new(
            vec![typed("a"), typed("b")],
            &[Logon::Wrong, Logon::Correct],
        );
        assert_eq!(run(&p), AuthOutcome::Verified);
    }

    #[test]
    fn cancellation_and_errors_are_not_verified() {
        assert_eq!(
            run(&FakePort::new(vec![Ask::Cancelled], &[])),
            AuthOutcome::Cancelled
        );
        assert_eq!(
            run(&FakePort::new(vec![Ask::Error], &[])),
            AuthOutcome::Failed
        );
        let p = FakePort::new(vec![typed("a")], &[Logon::Error]);
        assert_eq!(run(&p), AuthOutcome::Failed);
    }

    #[test]
    fn a_cancel_flag_set_before_the_answer_wins_over_a_correct_password() {
        let p = FakePort::new(vec![typed("a")], &[Logon::Correct]);
        let cancel = AtomicBool::new(true);
        assert_eq!(
            verify_account(
                &p,
                Some(1),
                "m",
                &cancel,
                &Throttle::default(),
                &Instant::now
            ),
            AuthOutcome::Cancelled
        );
        assert_eq!(*p.logon_calls.lock().unwrap(), 0);
    }

    #[test]
    fn a_cancel_raised_while_the_system_checks_wins_over_a_correct_password() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut p = FakePort::new(vec![typed("a")], &[Logon::Correct]);
        p.raise = Some(cancel.clone());
        let r = verify_account(
            &p,
            Some(1),
            "m",
            &cancel,
            &Throttle::default(),
            &Instant::now,
        );
        assert_eq!(r, AuthOutcome::Cancelled);
        assert_eq!(*p.logon_calls.lock().unwrap(), 1);
    }

    /// Only the refusal and the return are checked here (the fake dialog obeys `cancel`): whether
    /// Windows really closes its prompt is NOT tested and must be checked by hand.
    #[tokio::test]
    async fn a_dialog_nobody_answers_is_refused_after_the_timeout() {
        let mut port = FakePort::new(vec![], &[]);
        port.block = true;
        let auth: Arc<dyn StrongAuth> = Arc::new(AccountAuth::new(port));
        let r = require_within(&auth, Some(1), "m", Duration::from_millis(60)).await;
        assert_eq!(r.unwrap_err().code, "error.auth_cancelled");
    }

    fn fallback(
        primary: &[AuthOutcome],
        secondary: &[AuthOutcome],
    ) -> (WithFallback, Arc<ScriptedAuth>, Arc<ScriptedAuth>) {
        let a = ScriptedAuth::new(primary);
        let b = ScriptedAuth::new(secondary);
        (
            WithFallback {
                primary: a.clone(),
                fallback: b.clone(),
            },
            a,
            b,
        )
    }

    #[test]
    fn hello_available_means_no_fallback_whatever_it_answers() {
        for hello in [
            AuthOutcome::Verified,
            AuthOutcome::Cancelled,
            AuthOutcome::Failed,
            AuthOutcome::Busy,
        ] {
            let (auth, a, b) = fallback(&[hello], &[AuthOutcome::Verified]);
            assert_eq!(auth.verify(Some(1), "m", &AtomicBool::new(false)), hello);
            assert_eq!(a.times_asked(), 1);
            assert_eq!(b.times_asked(), 0, "{hello:?} must not fall back");
        }
    }

    #[test]
    fn hello_unavailable_uses_the_account_fallback_and_its_answer() {
        for second in [
            AuthOutcome::Verified,
            AuthOutcome::Cancelled,
            AuthOutcome::NoPassword,
            AuthOutcome::Failed,
        ] {
            let (auth, a, b) = fallback(&[AuthOutcome::Unavailable], &[second]);
            assert_eq!(auth.verify(Some(1), "m", &AtomicBool::new(false)), second);
            assert_eq!(a.times_asked(), 1);
            assert_eq!(b.times_asked(), 1);
        }
    }

    #[cfg(windows)]
    #[test]
    fn typed_names_are_split_for_logon() {
        use super::win::split_account;
        assert_eq!(
            split_account(r"PC\moi", ""),
            ("moi".into(), Some("PC".into()))
        );
        assert_eq!(
            split_account("user@example.org", ""),
            ("user@example.org".into(), None)
        );
        assert_eq!(split_account("moi", ""), ("moi".into(), Some(".".into())));
        assert_eq!(
            split_account("moi", "PC"),
            ("moi".into(), Some("PC".into()))
        );
    }
}
