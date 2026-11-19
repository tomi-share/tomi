//! Strong verification of the user before an irreversible action: Windows Hello, else the
//! password of the Windows account.
//!
//! "Supprimer l'espace" and "Effacer quand même" destroy the only copy of the data (or cut this
//! PC off a space it cannot prove is gone). Android asks for strong biometrics there; this PC
//! asks Windows Hello (fingerprint, face or PIN of the account) through the system's
//! `UserConsentVerifier`, the prompt Windows itself draws, owned by the Tomi window.
//!
//! Rules:
//! * the check happens in Rust, before the core is called, and the answer never leaves this
//!   module as anything but [`AuthOutcome`]: the webview cannot say "verified";
//! * Windows Hello comes first and alone whenever it is usable. ONLY when it is unavailable
//!   (not set up, disabled by policy, no device: [`AuthOutcome::Unavailable`]) the check falls
//!   back on the password of the current Windows account (`account_auth.rs`), so that a PC
//!   without Hello (no sensor, no PIN) is not locked out. That fallback is weaker than Hello
//!   (no biometric factor) but needs no setup; it never replaces a Hello that failed, was
//!   cancelled or is busy. An account without a password cannot verify
//!   ([`AuthOutcome::NoPassword`]): there is no bypass. A device that is only busy
//!   ([`AuthOutcome::Busy`]) is not "not set up": it gets its own message and can be retried;
//! * a prompt nobody answers is not waited for forever: after [`PROMPT_TIMEOUT`] the prompt is
//!   closed (the `cancel` flag below) and the action is refused as cancelled. A late answer is
//!   never read: the refusal has already been given;
//! * nothing about the prompt is logged.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// The user proved who they are.
    Verified,
    /// The user dismissed the prompt.
    Cancelled,
    /// No verification can be used on this PC (Windows Hello unavailable and the account
    /// fallback not possible, e.g. outside Windows).
    Unavailable,
    /// Windows Hello is unavailable and the Windows account has no password: nothing to check.
    NoPassword,
    /// The device is busy (another prompt, a sensor in use): not a setup problem, try again.
    Busy,
    /// The prompt failed (too many wrong tries, system error).
    Failed,
    /// Wrong account passwords cumulated across actions: refused for a minute, without asking.
    TooManyTries,
    /// Windows reports the account as locked out.
    AccountLocked,
}

/// How long a prompt may stay on screen unanswered. Generous (a PIN, a finger that needs a
/// second try) but not endless: a forgotten prompt must not freeze the departure screens.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(150);

pub trait StrongAuth: Send + Sync {
    /// Shows the verification and blocks until it ends. `message` is the sentence of the
    /// prompt. `owner` is the raw handle of the window the prompt belongs to (`None` when
    /// unknown). `cancel` is set when nobody is waiting any more: the prompt must be closed and
    /// the call must return. Never call this on an async worker: use [`require`].
    fn verify(&self, owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome;
}

/// Asks for the verification on a thread of its own and turns anything but success into the
/// error the window shows. The only way the commands ask for it.
pub async fn require(
    auth: &Arc<dyn StrongAuth>,
    owner: Option<isize>,
    message: &'static str,
) -> Result<(), AppError> {
    require_within(auth, owner, message, PROMPT_TIMEOUT).await
}

/// [`require`] with the delay as a parameter (the tests do not wait minutes).
pub async fn require_within(
    auth: &Arc<dyn StrongAuth>,
    owner: Option<isize>,
    message: &'static str,
    timeout: Duration,
) -> Result<(), AppError> {
    let auth = auth.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let task = tauri::async_runtime::spawn_blocking(move || auth.verify(owner, message, &flag));
    let outcome = match tokio::time::timeout(timeout, task).await {
        Ok(joined) => joined.unwrap_or(AuthOutcome::Failed),
        Err(_) => {
            // Nobody answered: the prompt is closed, and whatever it says from now on is
            // thrown away (the task's result is never read).
            cancel.store(true, Ordering::SeqCst);
            AuthOutcome::Cancelled
        }
    };
    outcome_to_result(outcome)
}

pub fn outcome_to_result(outcome: AuthOutcome) -> Result<(), AppError> {
    match outcome {
        AuthOutcome::Verified => Ok(()),
        AuthOutcome::Cancelled => Err(AppError::new("error.auth_cancelled")),
        AuthOutcome::Unavailable => Err(AppError::new("error.strong_auth_unavailable")),
        AuthOutcome::NoPassword => Err(AppError::new("error.strong_auth_no_password")),
        AuthOutcome::Busy => Err(AppError::new("error.auth_busy")),
        AuthOutcome::Failed => Err(AppError::new("error.auth_failed")),
        AuthOutcome::TooManyTries => Err(AppError::new("error.strong_auth_too_many")),
        AuthOutcome::AccountLocked => Err(AppError::new("error.strong_auth_locked")),
    }
}

/// The verification of this machine: Windows Hello, and on Windows the password of the
/// account when Hello is unavailable.
#[cfg(windows)]
pub fn system() -> Arc<dyn StrongAuth> {
    Arc::new(crate::account_auth::WithFallback {
        primary: Arc::new(WindowsHello),
        fallback: Arc::new(crate::account_auth::AccountAuth::new(
            crate::account_auth::WindowsAccount,
        )),
    })
}

/// Off Windows (the tests) nothing is available.
#[cfg(not(windows))]
pub fn system() -> Arc<dyn StrongAuth> {
    Arc::new(WindowsHello)
}

/// Windows Hello through `Windows.Security.Credentials.UI.UserConsentVerifier`. On any other
/// platform (the tests run there too) it is never available.
pub struct WindowsHello;

#[cfg(windows)]
impl StrongAuth for WindowsHello {
    fn verify(&self, owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome {
        hello::verify(owner, message, cancel)
    }
}

#[cfg(not(windows))]
impl StrongAuth for WindowsHello {
    fn verify(&self, _owner: Option<isize>, _message: &str, _cancel: &AtomicBool) -> AuthOutcome {
        AuthOutcome::Unavailable
    }
}

#[cfg(windows)]
mod hello {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use windows::core::{factory, HSTRING};
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::{
        IUserConsentVerifierInterop, RoInitialize, RO_INIT_MULTITHREADED,
    };
    use windows_future::{AsyncStatus, IAsyncOperation};

    use super::AuthOutcome;

    /// How often the open prompt is looked at (to close it when the delay ran out).
    const WATCH_EVERY: Duration = Duration::from_millis(100);

    /// A desktop (non-packaged) app must go through the interop interface to give the prompt
    /// an owner window: `UserConsentVerifier::RequestVerificationAsync` alone has none.
    pub fn verify(owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome {
        // A thread of its own, in the multithreaded apartment: the prompt is awaited by
        // blocking, which an apartment with a message loop must never do.
        std::thread::scope(|scope| {
            let handle = std::thread::Builder::new()
                .name("tomi-hello".into())
                .spawn_scoped(scope, || run(owner, message, cancel));
            match handle.map(|h| h.join()) {
                Ok(Ok(outcome)) => outcome,
                _ => AuthOutcome::Failed,
            }
        })
    }

    fn run(owner: Option<isize>, message: &str, cancel: &AtomicBool) -> AuthOutcome {
        // `S_FALSE` / `RPC_E_CHANGED_MODE` only say the thread was already initialised.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        match availability() {
            Some(UserConsentVerifierAvailability::Available) => {}
            Some(UserConsentVerifierAvailability::DeviceBusy) => return AuthOutcome::Busy,
            Some(_) => return AuthOutcome::Unavailable,
            None => return AuthOutcome::Failed,
        }
        let Some(owner) = owner.filter(|h| *h != 0) else {
            // No window to own the prompt: refuse rather than show it ownerless.
            return AuthOutcome::Failed;
        };
        let hwnd = HWND(owner as *mut c_void);
        let request = || -> windows::core::Result<UserConsentVerificationResult> {
            let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
            let op: IAsyncOperation<UserConsentVerificationResult> = unsafe {
                interop.RequestVerificationForWindowAsync(hwnd, &HSTRING::from(message))?
            };
            // Not `op.join()`: that waits for ever. The prompt is watched, and closed when
            // nobody is waiting for it any more (the delay of `require` ran out).
            while op.Status()? == AsyncStatus::Started {
                if cancel.load(Ordering::SeqCst) {
                    let _ = op.Cancel();
                    break;
                }
                std::thread::sleep(WATCH_EVERY);
            }
            op.GetResults()
        };
        match request() {
            Ok(result) => map_result(result),
            Err(_) => AuthOutcome::Failed,
        }
    }

    fn availability() -> Option<UserConsentVerifierAvailability> {
        UserConsentVerifier::CheckAvailabilityAsync()
            .ok()?
            .join()
            .ok()
    }

    pub(super) fn map_result(result: UserConsentVerificationResult) -> AuthOutcome {
        match result {
            UserConsentVerificationResult::Verified => AuthOutcome::Verified,
            UserConsentVerificationResult::Canceled => AuthOutcome::Cancelled,
            UserConsentVerificationResult::DeviceNotPresent
            | UserConsentVerificationResult::NotConfiguredForUser
            | UserConsentVerificationResult::DisabledByPolicy => AuthOutcome::Unavailable,
            UserConsentVerificationResult::DeviceBusy => AuthOutcome::Busy,
            // Too many wrong tries, and anything a later Windows adds.
            _ => AuthOutcome::Failed,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_a_verified_result_verifies() {
            assert_eq!(
                map_result(UserConsentVerificationResult::Verified),
                AuthOutcome::Verified
            );
            for r in [
                UserConsentVerificationResult::Canceled,
                UserConsentVerificationResult::RetriesExhausted,
                UserConsentVerificationResult::DeviceBusy,
                UserConsentVerificationResult::DeviceNotPresent,
                UserConsentVerificationResult::NotConfiguredForUser,
                UserConsentVerificationResult::DisabledByPolicy,
            ] {
                assert_ne!(map_result(r), AuthOutcome::Verified, "{r:?}");
            }
        }

        #[test]
        fn a_missing_hello_setup_is_unavailable_not_a_failure_to_retry() {
            for r in [
                UserConsentVerificationResult::DeviceNotPresent,
                UserConsentVerificationResult::NotConfiguredForUser,
                UserConsentVerificationResult::DisabledByPolicy,
            ] {
                assert_eq!(map_result(r), AuthOutcome::Unavailable, "{r:?}");
            }
            assert_eq!(
                map_result(UserConsentVerificationResult::Canceled),
                AuthOutcome::Cancelled
            );
        }

        #[test]
        fn a_busy_device_is_not_a_missing_setup() {
            assert_eq!(
                map_result(UserConsentVerificationResult::DeviceBusy),
                AuthOutcome::Busy
            );
            assert_ne!(
                map_result(UserConsentVerificationResult::DeviceBusy),
                AuthOutcome::Unavailable
            );
        }

        #[test]
        fn without_an_owner_window_the_prompt_is_refused() {
            // The check of availability may itself say "unavailable" on a CI machine; what
            // must never happen is `Verified` without a window and a user.
            let cancel = AtomicBool::new(false);
            assert_ne!(run(None, "test", &cancel), AuthOutcome::Verified);
            assert_ne!(run(Some(0), "test", &cancel), AuthOutcome::Verified);
        }
    }
}

/// A scripted verification, for the tests of everything that asks for one.
#[cfg(test)]
pub struct ScriptedAuth {
    answers: std::sync::Mutex<std::collections::VecDeque<AuthOutcome>>,
    /// The messages the prompt was given, in order.
    pub asked: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl ScriptedAuth {
    pub fn new(answers: &[AuthOutcome]) -> Arc<Self> {
        Arc::new(Self {
            answers: std::sync::Mutex::new(answers.iter().copied().collect()),
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn times_asked(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

#[cfg(test)]
impl StrongAuth for ScriptedAuth {
    /// With nothing left in the script the answer is "unavailable": a test that forgot to
    /// script a verification fails closed.
    fn verify(&self, _owner: Option<isize>, message: &str, _cancel: &AtomicBool) -> AuthOutcome {
        self.asked.lock().unwrap().push(message.to_owned());
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(AuthOutcome::Unavailable)
    }
}

/// A prompt that stays on screen until the test lets it go (or until nobody waits for it any
/// more, in which case it answers `Verified` all the same: a late answer must stay refused).
#[cfg(test)]
pub struct GatedAuth {
    released: AtomicBool,
    pub entered: AtomicBool,
    pub saw_cancel: AtomicBool,
    answer: AuthOutcome,
}

#[cfg(test)]
impl GatedAuth {
    pub fn new(answer: AuthOutcome) -> Arc<Self> {
        Arc::new(Self {
            released: AtomicBool::new(false),
            entered: AtomicBool::new(false),
            saw_cancel: AtomicBool::new(false),
            answer,
        })
    }

    pub fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
impl StrongAuth for GatedAuth {
    fn verify(&self, _owner: Option<isize>, _message: &str, cancel: &AtomicBool) -> AuthOutcome {
        self.entered.store(true, Ordering::SeqCst);
        loop {
            if cancel.load(Ordering::SeqCst) {
                self.saw_cancel.store(true, Ordering::SeqCst);
                return AuthOutcome::Verified;
            }
            if self.released.load(Ordering::SeqCst) {
                return self.answer;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_has_its_own_code_and_only_success_passes() {
        assert!(outcome_to_result(AuthOutcome::Verified).is_ok());
        assert_eq!(
            outcome_to_result(AuthOutcome::Cancelled).unwrap_err().code,
            "error.auth_cancelled"
        );
        assert_eq!(
            outcome_to_result(AuthOutcome::Unavailable)
                .unwrap_err()
                .code,
            "error.strong_auth_unavailable"
        );
        assert_eq!(
            outcome_to_result(AuthOutcome::Failed).unwrap_err().code,
            "error.auth_failed"
        );
        assert_eq!(
            outcome_to_result(AuthOutcome::NoPassword).unwrap_err().code,
            "error.strong_auth_no_password"
        );
        assert_eq!(
            outcome_to_result(AuthOutcome::TooManyTries)
                .unwrap_err()
                .code,
            "error.strong_auth_too_many"
        );
        assert_eq!(
            outcome_to_result(AuthOutcome::AccountLocked)
                .unwrap_err()
                .code,
            "error.strong_auth_locked"
        );
        // Busy is retried, not "set Windows Hello up".
        assert_eq!(
            outcome_to_result(AuthOutcome::Busy).unwrap_err().code,
            "error.auth_busy"
        );
    }

    #[tokio::test]
    async fn require_passes_only_a_verified_answer() {
        let auth: Arc<dyn StrongAuth> = ScriptedAuth::new(&[
            AuthOutcome::Verified,
            AuthOutcome::Cancelled,
            AuthOutcome::Unavailable,
        ]);
        assert!(require(&auth, Some(1), "m").await.is_ok());
        assert_eq!(
            require(&auth, Some(1), "m").await.unwrap_err().code,
            "error.auth_cancelled"
        );
        assert_eq!(
            require(&auth, Some(1), "m").await.unwrap_err().code,
            "error.strong_auth_unavailable"
        );
        // Nothing scripted left: closed, never open.
        assert!(require(&auth, Some(1), "m").await.is_err());
    }

    #[tokio::test]
    async fn a_prompt_nobody_answers_is_closed_and_refused_and_a_late_answer_stays_refused() {
        let gate = GatedAuth::new(AuthOutcome::Verified);
        let auth: Arc<dyn StrongAuth> = gate.clone();
        let result = require_within(&auth, Some(1), "m", Duration::from_millis(60)).await;
        assert_eq!(result.unwrap_err().code, "error.auth_cancelled");
        // The prompt was told to close, and its (late) `Verified` changed nothing.
        for _ in 0..200 {
            if gate.saw_cancel.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(gate.entered.load(Ordering::SeqCst));
        assert!(
            gate.saw_cancel.load(Ordering::SeqCst),
            "the prompt is closed"
        );
    }

    #[tokio::test]
    async fn an_answer_in_time_passes_and_the_delay_is_a_few_minutes() {
        let gate = GatedAuth::new(AuthOutcome::Verified);
        gate.release();
        let auth: Arc<dyn StrongAuth> = gate.clone();
        assert!(require_within(&auth, Some(1), "m", Duration::from_secs(5))
            .await
            .is_ok());
        assert!(!gate.saw_cancel.load(Ordering::SeqCst));
        assert!(
            PROMPT_TIMEOUT >= Duration::from_secs(120)
                && PROMPT_TIMEOUT <= Duration::from_secs(180)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn off_windows_the_system_verification_is_never_available() {
        assert_eq!(
            system().verify(Some(1), "m", &AtomicBool::new(false)),
            AuthOutcome::Unavailable
        );
    }
}
