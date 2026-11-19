//! What this app does around the core once the departure erased this PC
//! (departure plan, section 8, "Windows"): the core has erased its database and
//! its files; everything the app itself holds is cleaned here, then the window goes back to the
//! welcome / pairing screen with a clean state.
//!
//! | What | Action |
//! |------|--------|
//! | Protected clipboard (a copied secret waiting to be wiped) | wiped first (tried again while another program holds the clipboard), forgotten after |
//! | Clipboard holding any copy of Tomi (the `TomiClipboardOrigin` tag, like Android's clip label "Tomi") | emptied, content never read |
//! | Sends in flight or failed, their staged copies | cancelled and deleted |
//! | Decrypted copies of attachments (`open/`, the core's cache) | purged |
//! | Staged and pasted files (`staging/`) | deleted (tried again while a program holds a file) |
//! | A prefill waiting, a pairing link waiting (not at start-up), the history offers already announced | forgotten |
//! | Toast notifications of Tomi in the notification centre | cleared |
//! | Welcome screen (`onboarding_done`) | shown again |
//! | `belonged_to_space` (this PC was part of a space) | cleared, the very last of all |
//! | Windows | popover hidden, full window on the welcome / pairing tab |
//!
//! Kept: display settings (theme, text mode), the shortcut, autostart, the download folder,
//! and the key that wraps the database (DPAPI): they hold nothing of the space.
//!
//! The cleanup is idempotent and also runs at start-up when this PC is no longer paired but the
//! persistent `belonged_to_space` is still set ([`needs_startup_cleanup`]): the process that
//! erased may have died between the core's erasure and this cleanup. That setting is written when
//! the PC is provisioned ([`remember_membership`]) and cleared last; `onboarding_done` is not
//! proof of anything (the welcome screen can be finished without pairing).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tauri::{AppHandle, Emitter, Manager};

use crate::clip_clears::{ClipboardClears, PendingClear};
use crate::clipboard::ClearResult;
use crate::departure::DepartureHost;
use crate::error::AppError;
use crate::state::AppState;
use crate::{actions, clipboard, popover};

/// How many times a busy clipboard, or a file another program holds, is tried before giving up.
const TRIES: usize = 6;
const PAUSE: Duration = Duration::from_millis(250);

/// Empties the clipboard on behalf of the cleanup (the real one, or a script in the tests).
pub trait ClipboardWiper: Send + Sync + 'static {
    /// Wipes the clipboard if it still holds the pending copy of a secret.
    fn wipe_pending(&self, pending: &PendingClear) -> ClearResult;
    /// Empties the clipboard if it holds any copy made by Tomi (the tag, never the content).
    fn clear_tagged(&self) -> ClearResult;
}

pub struct SystemClipboard;

impl ClipboardWiper for SystemClipboard {
    fn wipe_pending(&self, pending: &PendingClear) -> ClearResult {
        actions::wipe_checked(pending)
    }

    fn clear_tagged(&self) -> ClearResult {
        clipboard::clear_tagged()
    }
}

/// Runs a clipboard attempt (blocking, on a thread of its own) again while the clipboard is busy.
async fn until_not_busy(
    tries: usize,
    pause: Duration,
    attempt: impl Fn() -> ClearResult + Send + Sync + 'static,
) -> ClearResult {
    let attempt = Arc::new(attempt);
    let mut last = ClearResult::Busy;
    for i in 0..tries {
        let a = attempt.clone();
        last = actions::blocking(move || a())
            .await
            .unwrap_or(ClearResult::Busy);
        if last != ClearResult::Busy {
            break;
        }
        if i + 1 < tries {
            tokio::time::sleep(pause).await;
        }
    }
    last
}

/// The clipboard of an erased PC: the copied secret is wiped first and forgotten only after, so
/// that a clipboard another program kept busy is still wiped by the 30 s timer; then whatever
/// else Tomi put there goes too.
pub async fn clear_clipboard(
    clears: &ClipboardClears,
    wiper: Arc<dyn ClipboardWiper>,
    tries: usize,
    pause: Duration,
) {
    if let Some(pending) = clears.peek() {
        let generation = pending.generation;
        let w = wiper.clone();
        let result = until_not_busy(tries, pause, move || w.wipe_pending(&pending)).await;
        if result != ClearResult::Busy {
            clears.forget(generation);
        }
    }
    until_not_busy(tries, pause, move || wiper.clear_tagged()).await;
}

/// Removes a directory, again while another program (an antivirus, a viewer) holds a file in it.
/// `true` once it is gone.
pub fn remove_dir_retrying(dir: &Path, tries: usize, pause: Duration) -> bool {
    for i in 0..tries {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(_) if i + 1 < tries => std::thread::sleep(pause),
            Err(_) => {}
        }
    }
    false
}

/// Why the cleanup runs. After an erasure in this process, whatever was waiting belonged to the
/// space that is gone. At start-up, a pairing link or a prefill that opened the app is the
/// user's NEW input and is never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    AfterErasure,
    AtStartup,
}

/// The part of the cleanup that does not need a window: testable on its own.
pub async fn cleanup_after_erasure(state: &AppState, mode: Cleanup) {
    cleanup_with(state, Arc::new(SystemClipboard), TRIES, PAUSE, mode).await;
}

pub async fn cleanup_with(
    state: &AppState,
    wiper: Arc<dyn ClipboardWiper>,
    tries: usize,
    pause: Duration,
    mode: Cleanup,
) {
    // The Windows Hello window of the hidden content is over with the space.
    state.secret_gate.close();
    // A copied secret must not stay on the clipboard of a PC that no longer has the space.
    clear_clipboard(&state.clip_clears, wiper, tries, pause).await;
    // Nothing in flight goes out for a space that is gone; the staged copies go with it.
    for job in state.queue.active() {
        state.queue.cancel(&job.id);
        state.queue.dismiss(&job.id);
    }
    let _ = state.core.purge_plaintext_cache().await;
    let staging = state.paths.staging_dir().to_path_buf();
    let _ = actions::blocking(move || {
        remove_dir_retrying(&staging, tries, pause);
        let _ = std::fs::create_dir_all(&staging);
    })
    .await;

    if mode == Cleanup::AfterErasure {
        *state
            .pending_prefill
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        *state.pending_pair.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *state
            .pending_setup
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
    }
    state
        .seen_offers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clear();
    // The welcome screen is the way back to a pairing.
    let _ = state.update_settings(|s| s.onboarding_done = false);
    // Last of all: "this PC belonged to a space" is the proof that makes the next launch finish
    // a cleanup that died before this line (`needs_startup_cleanup`). Cleared only now.
    let _ = state.update_settings(|s| s.belonged_to_space = false);
}

/// This PC is part of a space: remembered, so that a departure that dies half-way is finished
/// at the next launch. Called when the pairing ends and at every start-up (a PC paired before
/// this setting existed learns it then). Idempotent; writes the file only when it changes.
pub fn remember_membership(state: &AppState) {
    // The demo space is no membership: after a crash in the demo, nothing is left to clean.
    if state.core.status().paired
        && !state.core.is_demo()
        && !state.settings_snapshot().belonged_to_space
    {
        let _ = state.update_settings(|s| s.belonged_to_space = true);
    }
}

/// This PC belonged to a space and no longer does: a departure erased the core and the process
/// died before the cleanup above ended. The proof is the persistent `belonged_to_space`, never
/// `onboarding_done`: the welcome screen can be finished without ever pairing, and such a PC
/// has nothing to clean (a pairing link that opened the app must stay). Not a store that cannot
/// be opened (`unavailable`).
pub fn needs_startup_cleanup(state: &AppState) -> bool {
    let status = state.core.status();
    !status.paired && status.unavailable.is_none() && state.settings_snapshot().belonged_to_space
}

/// Removes the toasts Tomi left in the notification centre. Best effort.
#[cfg(windows)]
pub fn clear_toasts() {
    use windows::core::HSTRING;
    use windows::UI::Notifications::ToastNotificationManager;
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.ClearWithId(&HSTRING::from(crate::notify::toast_app_id()));
    }
}

#[cfg(not(windows))]
pub fn clear_toasts() {}

/// Everything after the core erased this PC: the app's own cleanup, then the windows.
pub async fn finish_erasure(app: &AppHandle, mode: Cleanup) {
    let state = app.state::<AppState>();
    cleanup_after_erasure(&state, mode).await;
    let _ = actions::blocking(clear_toasts).await;
    // The windows read the (now unpaired) state again, and the tray forgets the unread dot.
    let _ = app.emit("space-changed", ());
    let _ = app.emit("pairing-changed", ());
    actions::refresh_badge(app).await;
    let _ = app.emit("departure-changed", ());
    popover::hide_popover(app);
    popover::open_history(app, Some("onboarding"));
}

pub struct TauriHost {
    pub app: AppHandle,
}

#[async_trait]
impl DepartureHost for TauriHost {
    fn changed(&self) {
        let _ = self.app.emit("departure-changed", ());
    }

    fn notice(&self, error: AppError) {
        let _ = self.app.emit("notice", &error);
    }

    async fn erased(&self) {
        finish_erasure(&self.app, Cleanup::AfterErasure).await;
    }

    fn window_visible(&self, window: &str) -> bool {
        self.app
            .get_webview_window(window)
            .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
    }

    // Windows Hello takes the focus: the popover must not hide under it (the same guard as a
    // file dialog) and must not stay in front of it (it is always on top).
    fn prompt_open(&self) {
        self.app
            .state::<AppState>()
            .suppress_blur
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        popover::set_on_top(&self.app, false);
    }

    fn prompt_closed(&self) {
        self.app
            .state::<AppState>()
            .suppress_blur
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        popover::set_on_top(&self.app, true);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashSet, VecDeque};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::clip_clears::ClipboardClears;
    use crate::core_api::TomiCore;
    use crate::fake_core::FakeCore;
    use crate::paths::PathRegistry;
    use crate::queue::{JobSink, JobView, SendQueue};
    use crate::secret::SecretText;
    use crate::settings::Settings;
    use crate::state::PrefillPayload;

    struct NoSink;
    impl JobSink for NoSink {
        fn job_changed(&self, _job: &JobView) {}
    }

    struct NoHost;
    #[async_trait]
    impl DepartureHost for NoHost {
        fn changed(&self) {}
        fn notice(&self, _error: AppError) {}
        async fn erased(&self) {}
    }

    /// A clipboard that answers what it is told, and counts.
    struct ScriptedClipboard {
        pending: Mutex<VecDeque<ClearResult>>,
        tagged: Mutex<VecDeque<ClearResult>>,
        pending_calls: AtomicUsize,
        tagged_calls: AtomicUsize,
    }

    impl ScriptedClipboard {
        fn new(pending: &[ClearResult], tagged: &[ClearResult]) -> Arc<Self> {
            Arc::new(Self {
                pending: Mutex::new(pending.iter().copied().collect()),
                tagged: Mutex::new(tagged.iter().copied().collect()),
                pending_calls: AtomicUsize::new(0),
                tagged_calls: AtomicUsize::new(0),
            })
        }
    }

    impl ClipboardWiper for ScriptedClipboard {
        fn wipe_pending(&self, _pending: &PendingClear) -> ClearResult {
            self.pending_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.pending
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ClearResult::NotOurs)
        }

        fn clear_tagged(&self) -> ClearResult {
            self.tagged_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.tagged
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ClearResult::NotOurs)
        }
    }

    fn state(dir: &Path) -> AppState {
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let paths = Arc::new(PathRegistry::new(&staging));
        let core: Arc<dyn TomiCore> = Arc::new(FakeCore::new(dir.join("store")).without_delay());
        let auth = crate::strong_auth::system();
        let departure = crate::departure::LeaveFlow::new(
            core.clone(),
            auth.clone(),
            Arc::new(NoHost),
            Arc::new(crate::model::now_ms),
            Arc::new(|_task| {}),
            Arc::new(|| 0.0),
        );
        AppState {
            core: core.clone(),
            secret_gate: crate::unlock::SecretGate::new(auth.clone()),
            auth,
            departure,
            #[cfg(debug_assertions)]
            fake: None,
            queue: SendQueue::new(
                core,
                Arc::new(NoSink),
                std::time::Duration::from_secs(60),
                paths.clone(),
            ),
            settings: Mutex::new(Settings {
                onboarding_done: true,
                belonged_to_space: true,
                download_dir: Some("D:\\Tomi".into()),
                ..Settings::default()
            }),
            settings_path: dir.join("config").join("settings.json"),
            paths,
            clip_clears: Arc::new(ClipboardClears::default()),
            seen_offers: Mutex::new(HashSet::from(["offer-1".to_string()])),
            relay_notices: crate::relay_notices::RelayNotices::default(),
            pending_prefill: Mutex::new(Some(PrefillPayload::default())),
            pending_pair: Mutex::new(Some(crate::state::PendingPair {
                code: SecretText::new("4821-crayon-renard"),
                relay_url: Some("https://relay.example.org".into()),
            })),
            pending_setup: Mutex::new(Some(crate::state::PendingSetup {
                token: SecretText::new("h7k2-m9qv-d4xr-8tbn-c3wz-p6fj-a5ye-q2ns"),
                relay_url: "https://relay.example.org".into(),
            })),
            pinned: AtomicBool::new(false),
            suppress_blur: AtomicUsize::new(0),
            hidden_by_blur_at: Mutex::new(None),
            tray_anchor: Mutex::new(None),
        }
    }

    const NO_PAUSE: Duration = Duration::from_millis(1);

    #[tokio::test]
    async fn an_erasure_leaves_a_clean_app_that_shows_the_welcome_screen() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        std::fs::write(dir.path().join("staging").join("pasted.png"), b"x").unwrap();
        // A copied secret is waiting for its 30 s wipe (no sequence number: the system
        // clipboard is not touched by this test).
        st.clip_clears.register(Some("item-1"), "s3cret", None);
        let clipboard = ScriptedClipboard::new(&[ClearResult::Cleared], &[]);

        cleanup_with(
            &st,
            clipboard.clone(),
            TRIES,
            NO_PAUSE,
            Cleanup::AfterErasure,
        )
        .await;

        assert!(
            !st.clip_clears.is_pending(),
            "the pending clipboard wipe was done, then forgotten"
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("staging"))
                .unwrap()
                .count(),
            0,
            "staged files are gone and the directory is back"
        );
        assert!(st.pending_prefill.lock().unwrap().is_none());
        assert!(st.pending_pair.lock().unwrap().is_none());
        assert!(st.pending_setup.lock().unwrap().is_none());
        assert!(st.seen_offers.lock().unwrap().is_empty());
        let settings = st.settings_snapshot();
        assert!(!settings.onboarding_done, "the welcome screen comes back");
        assert!(
            !settings.belonged_to_space,
            "and the proof is cleared, last"
        );
        // What belongs to the machine, not to the space, is kept.
        assert_eq!(settings.download_dir.as_deref(), Some("D:\\Tomi"));
        assert!(
            Settings::load(&st.settings_path) == settings,
            "and it is saved"
        );
    }

    #[tokio::test]
    async fn cleaning_twice_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let clipboard = ScriptedClipboard::new(&[], &[]);
        cleanup_with(
            &st,
            clipboard.clone(),
            TRIES,
            NO_PAUSE,
            Cleanup::AfterErasure,
        )
        .await;
        cleanup_with(&st, clipboard, TRIES, NO_PAUSE, Cleanup::AfterErasure).await;
        assert!(st.paths.staging_dir().is_dir());
    }

    // ---------------------------------------------------------------- the clipboard

    #[tokio::test]
    async fn a_busy_clipboard_is_tried_again_and_the_secret_is_forgotten_only_once_wiped() {
        let clears = ClipboardClears::default();
        clears.register(Some("item-1"), "s3cret", Some(3));
        let clipboard = ScriptedClipboard::new(
            &[ClearResult::Busy, ClearResult::Busy, ClearResult::Cleared],
            &[ClearResult::NotOurs],
        );
        clear_clipboard(&clears, clipboard.clone(), TRIES, NO_PAUSE).await;
        assert_eq!(
            clipboard
                .pending_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert!(!clears.is_pending(), "wiped, then forgotten");
    }

    #[tokio::test]
    async fn a_clipboard_that_stays_busy_keeps_the_entry_for_its_timer() {
        let clears = ClipboardClears::default();
        clears.register(Some("item-1"), "s3cret", Some(3));
        let clipboard = ScriptedClipboard::new(&[ClearResult::Busy; 10], &[ClearResult::Busy; 10]);
        clear_clipboard(&clears, clipboard.clone(), 3, NO_PAUSE).await;
        assert_eq!(
            clipboard
                .pending_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            3,
            "no more than the tries"
        );
        assert!(
            clears.is_pending(),
            "not wiped: the 30 s timer still knows what to wipe"
        );
    }

    #[tokio::test]
    async fn a_clipboard_that_holds_something_else_is_left_alone_and_the_entry_forgotten() {
        let clears = ClipboardClears::default();
        clears.register(Some("item-1"), "s3cret", Some(3));
        let clipboard = ScriptedClipboard::new(&[ClearResult::NotOurs], &[ClearResult::NotOurs]);
        clear_clipboard(&clears, clipboard.clone(), TRIES, NO_PAUSE).await;
        assert!(!clears.is_pending());
        assert_eq!(
            clipboard
                .tagged_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn a_non_secret_copy_of_tomi_is_emptied_through_its_tag_even_with_nothing_pending() {
        let clears = ClipboardClears::default();
        let clipboard = ScriptedClipboard::new(&[], &[ClearResult::Busy, ClearResult::Cleared]);
        clear_clipboard(&clears, clipboard.clone(), TRIES, NO_PAUSE).await;
        assert_eq!(
            clipboard
                .tagged_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2,
            "tried again once while busy"
        );
        assert_eq!(
            clipboard
                .pending_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    // ---------------------------------------------------------------- the staging directory

    #[test]
    fn a_missing_directory_counts_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_dir_retrying(
            &dir.path().join("nothing"),
            2,
            NO_PAUSE
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_file_another_program_holds_is_removed_once_it_lets_go() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let file = staging.join("held.bin");
        std::fs::write(&file, b"x").unwrap();
        // Opened without FILE_SHARE_DELETE: Windows refuses to delete it meanwhile.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&file)
            .unwrap();

        // Refused while held, however many times it is tried.
        assert!(!remove_dir_retrying(&staging, 2, NO_PAUSE));
        assert!(file.exists());

        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(held);
        });
        assert!(
            remove_dir_retrying(&staging, 40, Duration::from_millis(25)),
            "tried again until the file is free"
        );
        release.join().unwrap();
        assert!(!staging.exists());
    }

    // ---------------------------------------------------------------- start-up

    #[tokio::test]
    async fn an_interrupted_erasure_is_cleaned_at_the_next_start_and_only_then() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        // Paired and set up: nothing to do.
        assert!(!needs_startup_cleanup(&st));

        // The core erased this PC and the process died before the cleanup: no longer paired,
        // but this PC did belong to a space (the persistent setting is still there).
        st.core.leave_space().await.unwrap();
        assert!(!st.core.status().paired);
        assert!(needs_startup_cleanup(&st));

        // The cleanup (idempotent) ends by clearing the setting: not needed again.
        let scripted = ScriptedClipboard::new(&[], &[]);
        cleanup_with(&st, scripted, TRIES, NO_PAUSE, Cleanup::AtStartup).await;
        let settings = st.settings_snapshot();
        assert!(!settings.onboarding_done, "the welcome screen is back");
        assert!(!settings.belonged_to_space, "cleared, and cleared last");
        assert!(Settings::load(&st.settings_path) == settings, "and saved");
        assert!(!needs_startup_cleanup(&st));
        cleanup_with(
            &st,
            ScriptedClipboard::new(&[], &[]),
            TRIES,
            NO_PAUSE,
            Cleanup::AtStartup,
        )
        .await;
        assert!(!needs_startup_cleanup(&st));
    }

    #[tokio::test]
    async fn a_pc_never_paired_is_not_cleaned_even_with_the_welcome_screen_finished() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        st.core.leave_space().await.unwrap();
        // Never paired: the welcome screen was finished without pairing (`onboarding_done`),
        // and the persistent proof was never written.
        st.update_settings(|s| s.belonged_to_space = false).unwrap();
        assert!(st.settings_snapshot().onboarding_done);
        assert!(!needs_startup_cleanup(&st), "nothing to clean");
        // A pairing link that opened the app, a prefill and the welcome screen stay.
        assert!(st.pending_pair.lock().unwrap().is_some());
        assert!(st.pending_prefill.lock().unwrap().is_some());
    }

    #[tokio::test]
    async fn the_start_up_cleanup_keeps_a_pairing_link_and_a_prefill_that_opened_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        st.core.leave_space().await.unwrap();
        assert!(needs_startup_cleanup(&st));
        cleanup_with(
            &st,
            ScriptedClipboard::new(&[], &[]),
            TRIES,
            NO_PAUSE,
            Cleanup::AtStartup,
        )
        .await;
        assert!(st.pending_pair.lock().unwrap().is_some(), "link kept");
        assert!(st.pending_prefill.lock().unwrap().is_some(), "prefill kept");
        assert!(!st.settings_snapshot().belonged_to_space);
    }

    #[tokio::test]
    async fn membership_is_remembered_when_paired_and_not_invented_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        // An install from before the setting existed, paired: it learns it at start-up.
        st.update_settings(|s| s.belonged_to_space = false).unwrap();
        remember_membership(&st);
        assert!(st.settings_snapshot().belonged_to_space);
        assert!(Settings::load(&st.settings_path).belonged_to_space);

        // Not paired: nothing is written.
        st.core.leave_space().await.unwrap();
        st.update_settings(|s| s.belonged_to_space = false).unwrap();
        remember_membership(&st);
        assert!(!st.settings_snapshot().belonged_to_space);
    }
}
