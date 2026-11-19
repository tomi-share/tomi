//! The Windows Hello gate in front of hidden ("masqué") content.
//!
//! Showing, copying or opening a hidden item asks Windows Hello first (like the biometric gate of
//! Android, `BiometricGate`), then stays open for [`WINDOW`]: a second action within two minutes
//! does not ask again. The window runs from the verification, it is not extended by use.
//!
//! Rules:
//! * the check is in Rust, before the value is read from the core: the webview cannot say "open";
//! * Windows Hello when it is usable; only when it is unavailable (not set up, disabled, absent)
//!   the password of the Windows account is asked instead (`account_auth`, weaker, so that a PC
//!   without Hello is not locked out). An account without a password gets no access, with the
//!   message of `strong_auth`; a Hello that failed or was cancelled never falls back;
//! * two actions that arrive together share one prompt: the second waits for the first;
//! * the window is closed when the space is left or erased (`close`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::popover;
use crate::state::AppState;
use crate::strong_auth::{self, StrongAuth};

/// How long a verification lets hidden content through.
pub const WINDOW: Duration = Duration::from_secs(120);

pub struct SecretGate {
    auth: Arc<dyn StrongAuth>,
    until: Mutex<Option<Instant>>,
    /// Raised by every `close`: a prompt that began before a `close` must not open the window
    /// when it ends (the space was left, the session locked...).
    epoch: AtomicU64,
    /// One prompt at a time: a second request waits, then finds the window open.
    asking: tokio::sync::Mutex<()>,
}

impl SecretGate {
    pub fn new(auth: Arc<dyn StrongAuth>) -> Self {
        Self {
            auth,
            until: Mutex::new(None),
            epoch: AtomicU64::new(0),
            asking: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.until.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn is_open_at(&self, now: Instant) -> bool {
        self.lock().is_some_and(|until| now < until)
    }

    /// The window of a past verification is still open.
    pub fn is_open(&self) -> bool {
        self.is_open_at(Instant::now())
    }

    /// Forgets the verification: the next access asks again.
    pub fn close(&self) {
        // Under the lock, so that `require` never sees the epoch moved without the window closed.
        let mut until = self.lock();
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *until = None;
    }

    /// Lets the caller through inside the window, else asks Windows Hello. `on_prompt` is called
    /// only when a prompt is shown and its result is held until the prompt is over (it guards
    /// the popover's focus).
    pub async fn require<G>(
        &self,
        owner: Option<isize>,
        message: &'static str,
        on_prompt: impl FnOnce() -> G,
    ) -> AppResult<()> {
        if self.is_open() {
            return Ok(());
        }
        let _one_at_a_time = self.asking.lock().await;
        // Answered while this call waited its turn.
        if self.is_open() {
            return Ok(());
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let _prompt = on_prompt();
        strong_auth::require(&self.auth, owner, message).await?;
        let mut until = self.lock();
        // Closed while the prompt was on screen: this verification is for a state that is gone.
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(AppError::new("error.auth_cancelled"));
        }
        *until = Some(Instant::now() + WINDOW);
        Ok(())
    }
}

/// Windows Hello is on screen over a Tomi window: the popover must not hide under it (a file
/// dialog's guard) nor stay in front of it (it is always on top). Released on every path.
pub struct PromptGuard {
    app: AppHandle,
}

impl PromptGuard {
    pub fn open(app: &AppHandle) -> Self {
        app.state::<AppState>()
            .suppress_blur
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        popover::set_on_top(app, false);
        Self { app: app.clone() }
    }
}

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .suppress_blur
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        popover::set_on_top(&self.app, true);
    }
}

/// Windows Hello for hidden content (`strings::HELLO_REVEAL`), through the gate of the app.
pub async fn unlock(app: &AppHandle, owner: Option<isize>) -> AppResult<()> {
    let state = app.state::<AppState>();
    state
        .secret_gate
        .require(owner, crate::strings::HELLO_REVEAL, || {
            PromptGuard::open(app)
        })
        .await
}

/// Windows Hello before a revocation: always asked, never remembered.
pub async fn confirm_revoke(app: &AppHandle, owner: Option<isize>) -> AppResult<()> {
    let state = app.state::<AppState>();
    let _prompt = PromptGuard::open(app);
    strong_auth::require(&state.auth, owner, crate::strings::HELLO_REVOKE).await
}

/// Windows Hello before a new device is added to the space (it receives the space key and, if it
/// asks, the whole history): always asked, never remembered, like a revocation.
pub async fn confirm_add_device(app: &AppHandle, owner: Option<isize>) -> AppResult<()> {
    let state = app.state::<AppState>();
    let _prompt = PromptGuard::open(app);
    strong_auth::require(&state.auth, owner, crate::strings::HELLO_ADD_DEVICE).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strong_auth::{AuthOutcome, ScriptedAuth};

    fn gate(answers: &[AuthOutcome]) -> (SecretGate, Arc<ScriptedAuth>) {
        let auth = ScriptedAuth::new(answers);
        (SecretGate::new(auth.clone()), auth)
    }

    #[tokio::test]
    async fn the_first_access_asks_and_the_next_ones_within_the_window_do_not() {
        let (gate, auth) = gate(&[AuthOutcome::Verified]);
        assert!(!gate.is_open());
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert!(gate.is_open());
        // Nothing left in the script: a second prompt would fail closed.
        gate.require(Some(1), "m", || ()).await.unwrap();
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert_eq!(auth.times_asked(), 1);
    }

    #[tokio::test]
    async fn a_refusal_opens_nothing_and_is_asked_again() {
        let (gate, auth) = gate(&[
            AuthOutcome::Cancelled,
            AuthOutcome::Unavailable,
            AuthOutcome::Verified,
        ]);
        assert_eq!(
            gate.require(Some(1), "m", || ()).await.unwrap_err().code,
            "error.auth_cancelled"
        );
        assert!(!gate.is_open());
        assert_eq!(
            gate.require(Some(1), "m", || ()).await.unwrap_err().code,
            "error.strong_auth_unavailable"
        );
        assert!(!gate.is_open());
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert_eq!(auth.times_asked(), 3);
    }

    #[test]
    fn the_window_is_two_minutes_and_ends() {
        let (gate, _) = gate(&[]);
        let t0 = Instant::now();
        *gate.lock() = Some(t0 + WINDOW);
        assert!(gate.is_open_at(t0));
        assert!(gate.is_open_at(t0 + WINDOW - Duration::from_millis(1)));
        assert!(!gate.is_open_at(t0 + WINDOW));
        assert_eq!(WINDOW, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn closing_the_gate_asks_again() {
        let (gate, auth) = gate(&[AuthOutcome::Verified, AuthOutcome::Verified]);
        gate.require(Some(1), "m", || ()).await.unwrap();
        gate.close();
        assert!(!gate.is_open());
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert_eq!(auth.times_asked(), 2);
    }

    #[tokio::test]
    async fn a_close_during_the_prompt_keeps_the_window_shut() {
        let (gate, auth) = gate(&[AuthOutcome::Verified, AuthOutcome::Verified]);
        // The space is left (or the session locked) while Windows Hello is on screen.
        let err = gate
            .require(Some(1), "m", || gate.close())
            .await
            .unwrap_err();
        assert_eq!(err.code, "error.auth_cancelled");
        assert!(!gate.is_open());
        // The next access asks again and, with no close in between, opens it.
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert!(gate.is_open());
        assert_eq!(auth.times_asked(), 2);
    }

    #[tokio::test]
    async fn an_expired_window_asks_again() {
        let (gate, auth) = gate(&[AuthOutcome::Verified]);
        *gate.lock() = Some(Instant::now() - Duration::from_secs(1));
        gate.require(Some(1), "m", || ()).await.unwrap();
        assert_eq!(auth.times_asked(), 1);
    }

    #[tokio::test]
    async fn simultaneous_accesses_share_one_prompt() {
        let (gate, auth) = gate(&[AuthOutcome::Verified]);
        let gate = Arc::new(gate);
        let calls: Vec<_> = (0..4)
            .map(|_| {
                let gate = gate.clone();
                tokio::spawn(async move { gate.require(Some(1), "m", || ()).await })
            })
            .collect();
        for c in calls {
            c.await.unwrap().unwrap();
        }
        assert_eq!(auth.times_asked(), 1);
    }

    #[tokio::test]
    async fn the_prompt_guard_is_held_only_while_a_prompt_is_shown() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Guard<'a>(&'a AtomicUsize);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let open = AtomicUsize::new(0);
        let (gate, _) = gate(&[AuthOutcome::Failed, AuthOutcome::Verified]);
        let ask = || {
            open.fetch_add(1, Ordering::SeqCst);
            Guard(&open)
        };
        assert!(gate.require(Some(1), "m", ask).await.is_err());
        assert_eq!(open.load(Ordering::SeqCst), 0, "released after a failure");
        gate.require(Some(1), "m", ask).await.unwrap();
        assert_eq!(open.load(Ordering::SeqCst), 0, "released after a success");
        // Inside the window no prompt is shown at all.
        gate.require(Some(1), "m", || -> Guard<'_> {
            panic!("no prompt expected")
        })
        .await
        .unwrap();
    }
}
