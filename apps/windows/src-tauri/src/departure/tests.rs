//! The rules of the departure screens, tested against the demo core (which keeps the rules of
//! the real one) and a scripted Windows Hello.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::fake_core::{FakeCore, RelayMode, Reply, Step};
use crate::strong_auth::{AuthOutcome, GatedAuth, ScriptedAuth, StrongAuth};

#[derive(Default)]
struct TestHost {
    changed: AtomicUsize,
    erased: AtomicUsize,
    notices: Mutex<Vec<String>>,
    prompts_opened: AtomicUsize,
    prompts_closed: AtomicUsize,
    /// Windows the test declared hidden (`window_visible` says no for them).
    hidden: Mutex<Vec<String>>,
}

impl TestHost {
    /// Windows Hello prompts that are on screen right now.
    fn prompts_open(&self) -> usize {
        self.prompts_opened.load(Ordering::SeqCst) - self.prompts_closed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DepartureHost for TestHost {
    fn changed(&self) {
        self.changed.fetch_add(1, Ordering::SeqCst);
    }
    fn notice(&self, error: AppError) {
        self.notices.lock().unwrap().push(error.code);
    }
    async fn erased(&self) {
        self.erased.fetch_add(1, Ordering::SeqCst);
    }
    fn window_visible(&self, window: &str) -> bool {
        !self.hidden.lock().unwrap().iter().any(|w| w == window)
    }
    fn prompt_open(&self) {
        self.prompts_opened.fetch_add(1, Ordering::SeqCst);
    }
    fn prompt_closed(&self) {
        self.prompts_closed.fetch_add(1, Ordering::SeqCst);
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    core: Arc<FakeCore>,
    flow: Arc<LeaveFlow>,
    host: Arc<TestHost>,
    auth: Arc<ScriptedAuth>,
    now: Arc<AtomicI64>,
    kicked: Arc<AtomicUsize>,
}

impl Rig {
    fn new(auth: &[AuthOutcome]) -> Self {
        Self::build(auth, false)
    }

    /// `run_series`: the spawned series really run (on the test runtime).
    fn build(auth: &[AuthOutcome], run_series: bool) -> Self {
        let scripted = ScriptedAuth::new(auth);
        Self::build_with(scripted.clone(), scripted, run_series)
    }

    /// A Windows Hello that the test holds on screen as long as it wants (`GatedAuth`).
    fn with_prompt(prompt: Arc<dyn StrongAuth>) -> Self {
        Self::build_with(prompt, ScriptedAuth::new(&[]), false)
    }

    fn build_with(prompt: Arc<dyn StrongAuth>, auth: Arc<ScriptedAuth>, run_series: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let core = Arc::new(FakeCore::new(dir.path().join("store")).without_delay());
        let host = Arc::new(TestHost::default());
        let now = Arc::new(AtomicI64::new(1_000_000));
        let kicked = Arc::new(AtomicUsize::new(0));
        let clock_now = now.clone();
        let counted = kicked.clone();
        let flow = LeaveFlow::new(
            core.clone(),
            prompt,
            host.clone(),
            Arc::new(move || clock_now.load(Ordering::SeqCst)),
            Arc::new(move |task| {
                counted.fetch_add(1, Ordering::SeqCst);
                if run_series {
                    tokio::spawn(task);
                }
            }),
            Arc::new(|| 0.0),
        );
        Rig {
            _dir: dir,
            core,
            flow,
            host,
            auth,
            now,
            kicked,
        }
    }

    fn pass(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn erased(&self) -> bool {
        !self.core.status().paired
    }

    fn screen(&self) -> LeaveScreen {
        self.flow.snapshot().ui.screen
    }
}

use crate::core_api::TomiCore;

fn code(r: Result<DepartureSnapshot, AppError>) -> String {
    r.unwrap_err().code
}

// ------------------------------------------------------------------ the entry and "Se déconnecter"

#[tokio::test]
async fn the_entry_is_refused_when_the_core_cannot_do_a_departure() {
    let rig = Rig::new(&[]);
    rig.core.simulate_unavailable();
    assert_eq!(code(rig.flow.open_leave()), "error.departure_unavailable");
    assert!(!rig.flow.snapshot().available);
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
}

#[tokio::test]
async fn leaving_asks_then_erases_on_a_proof_and_cleans_up() {
    let rig = Rig::new(&[]);
    rig.flow.open_leave().unwrap();
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
    let snap = rig.flow.confirm_leave().await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::Hidden);
    assert!(rig.erased());
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 1);
    // Leaving does not ask Windows Hello: it is reversible by pairing again.
    assert_eq!(rig.auth.times_asked(), 0);
}

#[tokio::test]
async fn the_entry_does_nothing_while_a_departure_is_pending() {
    let rig = Rig::new(&[]);
    rig.core.simulate_relay(RelayMode::Offline);
    rig.flow.open_leave().unwrap();
    rig.flow.confirm_leave().await.unwrap();
    assert!(rig.flow.snapshot().departure.is_some());
    rig.flow.open_leave().unwrap();
    assert_eq!(rig.screen(), LeaveScreen::Hidden, "the banner owns it");
}

#[tokio::test]
async fn a_pending_departure_closes_the_dialog_shows_the_banner_and_starts_the_back_off() {
    let rig = Rig::new(&[]);
    rig.core.simulate_relay(RelayMode::Offline);
    rig.flow.open_leave().unwrap();
    let snap = rig.flow.confirm_leave().await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::Hidden);
    let view = snap.departure.expect("the banner has something to show");
    assert_eq!(view.kind, DepartureKind::Leave);
    assert_eq!(view.last, Some(PendingReason::Network));
    assert!(!rig.erased());
    assert_eq!(rig.kicked.load(Ordering::SeqCst), 1, "one series started");
}

#[tokio::test]
async fn the_last_device_leads_to_the_deletion_dialog_without_an_error() {
    let rig = Rig::new(&[]);
    rig.core.simulate_last_device();
    rig.flow.open_leave().unwrap();
    let snap = rig.flow.confirm_leave().await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::ConfirmDelete);
    assert_eq!(
        snap.ui.error, None,
        "last_device is the reason to ask, not an error"
    );
    assert_eq!(
        snap.ui.armed_at_ms,
        Some(rig.now.load(Ordering::SeqCst) + ARM_MS)
    );
    assert!(snap.departure.is_none(), "the refusal removed the marker");
    assert!(!rig.erased());
}

#[tokio::test]
async fn an_update_required_refusal_stays_in_the_leave_dialog() {
    let rig = Rig::new(&[]);
    rig.core.script(Step::Session, Reply::UpdateRequired);
    rig.flow.open_leave().unwrap();
    let snap = rig.flow.confirm_leave().await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::ConfirmLeave);
    assert_eq!(snap.ui.error, Some(RefusalReason::UpdateRequired));
    assert!(!snap.ui.busy);
    assert!(!rig.erased());
}

#[tokio::test]
async fn a_storage_failure_keeps_the_dialog_open_and_erases_nothing() {
    // The core answers a local failure: the dialog says "failed", nothing was erased.
    let rig = Rig::new(&[]);
    rig.flow.open_leave().unwrap();
    rig.flow
        .apply(
            Ctx::Dialog(DepartureKind::Leave),
            Err(CoreError::Storage("x".into())),
        )
        .await;
    let snap = rig.flow.snapshot();
    assert!(snap.ui.failed);
    assert!(!snap.ui.busy);
    assert!(!rig.erased());
}

#[tokio::test]
async fn the_dialog_cannot_be_dismissed_while_busy() {
    let rig = Rig::new(&[]);
    rig.flow.open_leave().unwrap();
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    assert_eq!(rig.flow.dismiss().ui.screen, LeaveScreen::ConfirmLeave);
    // Nor started twice.
    assert_eq!(
        rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap_err().code,
        "error.departure_busy"
    );
    rig.flow.edit_ui(|ui| ui.busy = false);
    assert_eq!(rig.flow.dismiss().ui.screen, LeaveScreen::Hidden);
}

// ------------------------------------------------------------------ "Supprimer l'espace"

async fn open_delete(rig: &Rig) {
    rig.core.simulate_last_device();
    rig.flow.open_leave().unwrap();
    rig.flow.confirm_leave().await.unwrap();
    assert_eq!(rig.screen(), LeaveScreen::ConfirmDelete);
}

#[tokio::test]
async fn deleting_the_space_is_refused_before_the_two_seconds() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    rig.pass(ARM_MS - 1);
    assert_eq!(
        code(rig.flow.confirm_delete(Some(1)).await),
        "error.departure_not_armed"
    );
    assert_eq!(rig.auth.times_asked(), 0, "Windows Hello is not even asked");
    assert!(!rig.erased());
}

#[tokio::test]
async fn deleting_the_space_without_windows_hello_is_refused_and_nothing_is_sent() {
    let rig = Rig::new(&[AuthOutcome::Unavailable]);
    open_delete(&rig).await;
    rig.pass(ARM_MS);
    assert_eq!(
        code(rig.flow.confirm_delete(Some(1)).await),
        "error.strong_auth_unavailable"
    );
    assert!(!rig.erased());
    assert!(
        rig.flow.snapshot().departure.is_none(),
        "no request, no marker"
    );
    assert_eq!(rig.core.relay_state(), (false, false));
    assert!(!rig.flow.snapshot().ui.busy, "the dialog can be used again");
    assert_eq!(rig.screen(), LeaveScreen::ConfirmDelete);
}

#[tokio::test]
async fn deleting_the_space_cancelled_or_failed_in_windows_hello_does_nothing() {
    for outcome in [AuthOutcome::Cancelled, AuthOutcome::Failed] {
        let rig = Rig::new(&[outcome]);
        open_delete(&rig).await;
        rig.pass(ARM_MS);
        assert!(rig.flow.confirm_delete(Some(1)).await.is_err());
        assert!(!rig.erased());
        assert_eq!(rig.core.relay_state(), (false, false));
    }
}

#[tokio::test]
async fn deleting_the_space_with_windows_hello_erases_on_the_receipt_of_the_relay() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    rig.pass(ARM_MS);
    let snap = rig.flow.confirm_delete(Some(1)).await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::Hidden);
    assert!(rig.erased());
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 1);
    assert_eq!(rig.core.relay_state(), (false, true));
    assert_eq!(rig.auth.times_asked(), 1);
    assert_eq!(
        rig.auth.asked.lock().unwrap()[0],
        strings::hello_delete_space()
    );
}

#[tokio::test]
async fn a_relay_too_old_to_delete_refuses_and_offers_erasing_anyway() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    rig.core.simulate_relay(RelayMode::OldRelay);
    rig.pass(ARM_MS);
    let snap = rig.flow.confirm_delete(Some(1)).await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::ConfirmDelete);
    assert_eq!(snap.ui.error, Some(RefusalReason::RelayTooOld));
    assert!(
        snap.can_erase_without_proof,
        "the core allows D4 after this verdict"
    );
    assert!(!rig.erased());
    assert!(snap.departure.is_none());
}

#[tokio::test]
async fn a_lost_answer_of_the_deletion_is_settled_by_the_receipt() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    rig.core.script(Step::DeleteSpace, Reply::LostAfterEffect);
    rig.pass(ARM_MS);
    let snap = rig.flow.confirm_delete(Some(1)).await.unwrap();
    assert!(!rig.erased(), "the answer was lost: no proof yet");
    let view = snap.departure.expect("visible, waiting");
    assert_eq!(view.kind, DepartureKind::DeleteSpace);
    // The back-off asks again: the receipt is the proof, no second DELETE is needed.
    assert!(rig.flow.auto_attempt().await || rig.erased());
    assert!(rig.erased());
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn not_the_last_device_after_all_goes_back_to_a_normal_departure() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    // A device joined meanwhile.
    rig.core.script(Step::DeleteSpace, Reply::NotLastDevice);
    rig.pass(ARM_MS);
    let snap = rig.flow.confirm_delete(Some(1)).await.unwrap();
    // The retry as a normal departure is refused again with `last_device` (this demo space has
    // only this PC): back to the deletion dialog, armed again, nothing erased.
    assert_eq!(snap.ui.screen, LeaveScreen::ConfirmDelete);
    assert_eq!(snap.ui.error, None);
    assert!(!rig.erased());
}

// ------------------------------------------------------------------ "Effacer quand même"

async fn waiting_for(rig: &Rig, mode: RelayMode) {
    rig.core.simulate_relay(mode);
    rig.flow.open_leave().unwrap();
    rig.flow.confirm_leave().await.unwrap();
    assert!(rig.flow.snapshot().departure.is_some());
}

#[tokio::test]
async fn erasing_anyway_is_never_allowed_after_a_network_error_a_5xx_or_a_429() {
    for mode in [RelayMode::Offline, RelayMode::Server] {
        let rig = Rig::new(&[AuthOutcome::Verified]);
        waiting_for(&rig, mode).await;
        let snap = rig.flow.open_erase().await.unwrap();
        assert_eq!(snap.ui.screen, LeaveScreen::EraseWithoutProof);
        assert!(snap.ui.not_allowed, "{mode:?}");
        assert!(!snap.ui.checking);
        rig.flow.set_erase_checked(true);
        rig.pass(ARM_MS);
        assert_eq!(
            code(rig.flow.confirm_erase(Some(1)).await),
            "error.erase_not_allowed"
        );
        assert!(!rig.erased(), "{mode:?}");
        assert_eq!(rig.auth.times_asked(), 0, "{mode:?}");
        // Even a direct call to the core is refused.
        assert!(rig.core.erase_without_proof().await.is_err());
    }
}

#[tokio::test]
async fn erasing_anyway_is_refused_after_a_429_scripted_on_the_session() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    // The attempt of the gesture and the fresh look of the dialog both meet a 429/5xx.
    rig.core.script(Step::Session, Reply::Server);
    rig.core.script(Step::Session, Reply::Server);
    rig.flow.open_leave().unwrap();
    rig.flow.confirm_leave().await.unwrap();
    let snap = rig.flow.open_erase().await.unwrap();
    assert!(snap.ui.not_allowed);
    assert!(!rig.core.can_erase_without_proof());
}

#[tokio::test]
async fn erasing_anyway_after_not_found_needs_the_checkbox_two_seconds_and_windows_hello() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    waiting_for(&rig, RelayMode::Reset).await;
    let snap = rig.flow.snapshot();
    assert_eq!(snap.departure.unwrap().last, Some(PendingReason::NotFound));

    let snap = rig.flow.open_erase().await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::EraseWithoutProof);
    assert!(!snap.ui.not_allowed);
    assert!(!snap.ui.checked);

    // No checkbox.
    rig.pass(ARM_MS);
    assert_eq!(
        code(rig.flow.confirm_erase(Some(1)).await),
        "error.erase_not_checked"
    );
    rig.flow.set_erase_checked(true);

    // Before 2 s.
    rig.flow.open_erase().await.unwrap();
    let armed = rig.flow.snapshot().ui.armed_at_ms.unwrap();
    rig.now.store(armed - 1, Ordering::SeqCst);
    assert_eq!(
        code(rig.flow.confirm_erase(Some(1)).await),
        "error.departure_not_armed"
    );
    assert_eq!(rig.auth.times_asked(), 0);
    assert!(!rig.erased());

    // All three: the core erases.
    rig.now.store(armed, Ordering::SeqCst);
    let snap = rig.flow.confirm_erase(Some(1)).await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::Hidden);
    assert!(rig.erased());
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 1);
    assert_eq!(rig.auth.asked.lock().unwrap()[0], strings::hello_erase());
    // Never a request to delete or leave: nothing reached the relay.
    assert_eq!(rig.core.relay_state(), (false, false));
}

#[tokio::test]
async fn erasing_anyway_without_windows_hello_is_refused_and_never_bypassed() {
    let rig = Rig::new(&[AuthOutcome::Unavailable]);
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    rig.flow.set_erase_checked(true);
    rig.pass(ARM_MS);
    assert_eq!(
        code(rig.flow.confirm_erase(Some(1)).await),
        "error.strong_auth_unavailable"
    );
    assert!(!rig.erased());
    assert!(!rig.flow.snapshot().ui.busy);
    assert_eq!(rig.screen(), LeaveScreen::EraseWithoutProof);
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_verdict_expiring_while_the_dialog_is_open_refuses_the_erasure() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    rig.flow.set_erase_checked(true);
    rig.pass(ARM_MS);
    rig.core.advance_clock(11 * 60_000);
    assert_eq!(
        code(rig.flow.confirm_erase(Some(1)).await),
        "error.erase_not_allowed"
    );
    assert!(!rig.erased());
    assert!(rig.flow.snapshot().ui.not_allowed);
}

#[tokio::test]
async fn the_state_not_found_without_a_departure_offers_erasing_after_a_fresh_look() {
    let rig = Rig::new(&[]);
    // Not found, no departure: the core must verify before the dialog says "allowed".
    rig.core.simulate_relay(RelayMode::Reset);
    rig.flow.open_leave().unwrap();
    rig.flow.confirm_leave().await.unwrap();
    rig.flow.cancel().await.unwrap();
    assert!(rig.flow.snapshot().departure.is_none());
    assert_eq!(
        rig.core.status().space,
        crate::core_api::SpaceState::NotFound
    );
    let snap = rig.flow.open_erase().await.unwrap();
    assert!(!snap.ui.not_allowed);
}

#[tokio::test]
async fn opening_the_erase_dialog_while_busy_or_already_open_changes_nothing() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    rig.flow.set_erase_checked(true);
    rig.flow.open_erase().await.unwrap();
    assert!(
        rig.flow.snapshot().ui.checked,
        "reopening does not reset the box"
    );
}

// ------------------------------------------------------------------ the banner

#[tokio::test]
async fn cancel_removes_the_marker_immediately_and_never_erases() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    let cancelled = rig.flow.cancel().await.unwrap();
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.kind, Some(DepartureKind::Leave));
    assert!(rig.flow.snapshot().departure.is_none());
    assert!(!rig.erased());
    // Nothing to cancel the second time.
    let again = rig.flow.cancel().await.unwrap();
    assert!(!again.cancelled);
    assert_eq!(again.kind, None);
}

#[tokio::test]
async fn retry_resends_and_erases_once_the_network_is_back() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    let snap = rig.flow.retry(Some(1)).await.unwrap();
    assert!(rig.erased());
    assert!(snap.departure.is_none());
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retry_met_with_a_refusal_reopens_the_dialog_it_belongs_to() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.simulate_last_device();
    let snap = rig.flow.retry(Some(1)).await.unwrap();
    assert_eq!(snap.ui.screen, LeaveScreen::ConfirmDelete);
    assert!(!rig.erased());
}

#[tokio::test]
async fn retry_without_a_departure_does_nothing() {
    let rig = Rig::new(&[]);
    let snap = rig.flow.retry(Some(1)).await.unwrap();
    assert!(snap.departure.is_none());
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
}

#[tokio::test]
async fn an_automatic_refusal_only_tells_and_opens_nothing() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.simulate_last_device();
    rig.flow.auto_attempt().await;
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
    assert_eq!(
        rig.host.notices.lock().unwrap().as_slice(),
        ["error.departure_refused_last_device"]
    );
}

// ------------------------------------------------------------------ back-off and restart

#[tokio::test(start_paused = true)]
async fn the_back_off_follows_its_delays_and_stops_on_a_proof() {
    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    // 2 s, 5 s, 15 s: still offline, still waiting.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    assert!(rig.flow.snapshot().departure.is_some());
    assert!(rig.flow.series_running.load(Ordering::SeqCst));
    // The relay is back; the next step (5 s later) finds the proof.
    rig.core.simulate_relay(RelayMode::Normal);
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert!(rig.erased());
    assert!(!rig.flow.series_running.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn the_back_off_stops_on_not_found_and_on_cancel() {
    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Reset);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert!(
        !rig.flow.series_running.load(Ordering::SeqCst),
        "not found ends the series"
    );
    assert!(!rig.erased());

    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.flow.cancel().await.unwrap();
    rig.core.simulate_relay(RelayMode::Normal);
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert!(
        !rig.erased(),
        "a cancelled departure is never resumed by its series"
    );
}

#[tokio::test(start_paused = true)]
async fn a_new_gesture_replaces_the_running_series() {
    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.flow.kick();
    rig.flow.kick();
    assert_eq!(rig.flow.generation.load(Ordering::SeqCst), 3);
    rig.core.simulate_relay(RelayMode::Normal);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(rig.erased());
}

#[tokio::test]
async fn the_window_coming_back_retries_at_most_once_a_minute() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    // The series has not been run (the spawner drops it) but is flagged as running: pokes
    // leave the attempts to it.
    rig.flow.poke().await;
    assert_eq!(rig.core.relay_state(), (false, false));
    rig.flow.series_running.store(false, Ordering::SeqCst);
    rig.flow.poke().await;
    let first = rig.flow.last_auto_ms.load(Ordering::SeqCst);
    rig.pass(30_000);
    rig.core.simulate_relay(RelayMode::Normal);
    rig.flow.poke().await;
    assert!(!rig.erased(), "30 s later is too soon");
    assert_eq!(rig.flow.last_auto_ms.load(Ordering::SeqCst), first);
    rig.pass(30_000);
    rig.flow.poke().await;
    assert!(rig.erased(), "a minute later it tries again");
}

#[tokio::test]
async fn a_departure_left_by_the_last_run_is_settled_first_at_start_up() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.reopen();
    rig.flow.resume_at_startup().await;
    assert!(rig.erased());
}

#[tokio::test]
async fn a_start_up_with_an_old_departure_only_verifies_and_says_so() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.advance_clock(16 * 60_000);
    rig.core.reopen();
    rig.flow.resume_at_startup().await;
    assert!(
        !rig.erased(),
        "still a member: the request is not sent again by itself"
    );
    let view = rig.flow.snapshot().departure.unwrap();
    assert_eq!(view.last, Some(PendingReason::NotSent));
    assert_eq!(rig.core.relay_state(), (false, false));
    // A gesture does resend it.
    rig.flow.retry(Some(1)).await.unwrap();
    assert!(rig.erased());
}

#[tokio::test]
async fn start_up_with_nothing_pending_or_without_the_feature_does_nothing() {
    let rig = Rig::new(&[]);
    rig.flow.resume_at_startup().await;
    assert_eq!(rig.kicked.load(Ordering::SeqCst), 0);
    let rig = Rig::new(&[]);
    rig.core.simulate_unavailable();
    rig.flow.resume_at_startup().await;
    rig.flow.poke().await;
    assert_eq!(rig.kicked.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_wiping_departure_is_finished_and_has_no_buttons() {
    let rig = Rig::new(&[]);
    rig.core.interrupt_in_wiping();
    let snap = rig.flow.snapshot();
    assert!(snap.departure.unwrap().wiping);
    // The banner's "Réessayer" is ignored while wiping; the start-up finishes the erasure.
    rig.flow.retry(Some(1)).await.unwrap();
    assert!(!rig.erased());
    rig.flow.resume_at_startup().await;
    assert!(rig.erased());
}

#[test]
fn the_jitter_stays_between_zero_and_twenty_percent() {
    for _ in 0..200 {
        let j = random_jitter();
        assert!((0.0..=0.2).contains(&j), "{j}");
    }
}

#[test]
fn the_delays_are_the_ones_of_the_android_retrier() {
    assert_eq!(
        RETRY_DELAYS_MS,
        [2_000, 5_000, 15_000, 30_000, 60_000, 120_000]
    );
    assert_eq!(ARM_MS, 2_000);
}

mod more;
