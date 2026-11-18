//! More rules of the departure screens: "Annuler" during an attempt, Windows Hello and the
//! focus of the popover, which window draws which dialog, the small gaps.

use super::*;

// ------------------------------------------------------------------ "Annuler" during an attempt

type Attempt = tokio::task::JoinHandle<Result<DepartureSnapshot, AppError>>;

/// The attempt of a gesture has written its marker and now waits for the network: the dialog is
/// busy and the core has not answered.
async fn leave_attempt_in_flight(rig: &Rig) -> (Arc<crate::fake_core::Hold>, Attempt) {
    let hold = rig.core.hold_next_attempt();
    rig.flow.open_leave().unwrap();
    let flow = rig.flow.clone();
    let task = tokio::spawn(async move { flow.confirm_leave().await });
    hold.entered.notified().await;
    (hold, task)
}

#[tokio::test]
async fn cancel_is_available_during_a_leave_attempt_and_does_not_wait_for_it() {
    let rig = Rig::new(&[]);
    let (hold, task) = leave_attempt_in_flight(&rig).await;
    let snap = rig.flow.snapshot();
    assert!(snap.ui.busy, "the attempt is running");
    assert!(snap.working);
    assert!(!snap.cancelling);

    // The attempt is still blocked in the core; "Annuler" answers at once.
    let done = rig.flow.cancel_running().await.unwrap();
    assert!(done.cancelled);
    assert_eq!(done.kind, Some(DepartureKind::Leave));
    let snap = rig.flow.snapshot();
    assert_eq!(
        snap.ui.screen,
        LeaveScreen::Hidden,
        "the dialog closed at once"
    );
    assert!(snap.departure.is_none(), "the marker is gone");
    assert!(snap.cancelling, "the attempt that left is still running");
    assert!(snap.working);
    assert!(!rig.erased());

    // The attempt ends: it erases nothing, reopens nothing, starts no series.
    hold.release.notify_one();
    task.await.unwrap().unwrap();
    let snap = rig.flow.snapshot();
    assert_eq!(snap.ui.screen, LeaveScreen::Hidden);
    assert!(!snap.cancelling && !snap.working);
    assert!(!rig.erased());
    assert_eq!(rig.core.relay_state(), (false, false));
    assert_eq!(rig.kicked.load(Ordering::SeqCst), 0);
    assert_eq!(rig.host.erased.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_refusal_that_arrives_after_cancel_reopens_nothing_and_says_nothing() {
    let rig = Rig::new(&[]);
    rig.core.simulate_last_device();
    let (hold, task) = leave_attempt_in_flight(&rig).await;
    rig.flow.cancel_running().await.unwrap();
    hold.release.notify_one();
    task.await.unwrap().unwrap();
    // The attempt found nothing pending (the marker was cancelled): no `last_device` dialog.
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
    assert!(rig.host.notices.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_is_available_during_a_deletion_attempt_after_windows_hello() {
    let rig = Rig::new(&[AuthOutcome::Verified]);
    open_delete(&rig).await;
    rig.pass(ARM_MS);
    let hold = rig.core.hold_next_attempt();
    let flow = rig.flow.clone();
    let task = tokio::spawn(async move { flow.confirm_delete(Some(1)).await });
    hold.entered.notified().await;
    assert_eq!(
        rig.host.prompts_open(),
        0,
        "Hello is over, the request is out"
    );
    assert!(!rig.flow.snapshot().authenticating);

    let done = rig.flow.cancel_running().await.unwrap();
    assert_eq!(done.kind, Some(DepartureKind::DeleteSpace));
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
    assert_eq!(rig.auth.times_asked(), 1, "Hello is not asked to cancel");

    hold.release.notify_one();
    task.await.unwrap().unwrap();
    assert!(!rig.erased());
    assert_eq!(rig.core.relay_state(), (false, false));
}

#[tokio::test]
async fn a_cancel_before_the_marker_exists_still_says_which_gesture_was_cancelled() {
    // "Annuler" pressed before the engine wrote its marker: the core has no kind to give, the
    // dialog on screen does ("Déconnexion annulée" must not be said for a deletion).
    let rig = Rig::new(&[]);
    rig.core.promise_cancel_without_marker();
    rig.flow.open_leave().unwrap();
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    let done = rig.flow.cancel_running().await.unwrap();
    assert!(done.cancelled);
    assert_eq!(done.kind, Some(DepartureKind::Leave));

    let rig = Rig::new(&[]);
    open_delete(&rig).await;
    rig.core.promise_cancel_without_marker();
    rig.flow.begin(LeaveScreen::ConfirmDelete).unwrap();
    assert!(rig.core.departure().is_none(), "no marker yet");
    let done = rig.flow.cancel_running().await.unwrap();
    assert!(done.cancelled);
    assert_eq!(done.kind, Some(DepartureKind::DeleteSpace));
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
}

#[tokio::test]
async fn cancel_running_does_nothing_for_an_idle_dialog_the_prompt_or_the_erasure() {
    let rig = Rig::new(&[]);
    // No dialog.
    assert!(!rig.flow.cancel_running().await.unwrap().cancelled);
    // An idle dialog stays (its own "Annuler" closes it, `dismiss`).
    rig.flow.open_leave().unwrap();
    assert!(!rig.flow.cancel_running().await.unwrap().cancelled);
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);

    // Windows Hello on screen: the prompt has its own Cancel.
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    rig.flow.authenticating.store(true, Ordering::SeqCst);
    assert!(!rig.flow.cancel_running().await.unwrap().cancelled);
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
    rig.flow.authenticating.store(false, Ordering::SeqCst);

    // The erasure without proof is not cancellable from the dialog either.
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    rig.flow.begin(LeaveScreen::EraseWithoutProof).unwrap();
    assert!(!rig.flow.cancel_running().await.unwrap().cancelled);
    assert_eq!(rig.screen(), LeaveScreen::EraseWithoutProof);
}

#[tokio::test]
async fn the_erasure_itself_is_never_cancelled() {
    let rig = Rig::new(&[]);
    rig.core.interrupt_in_wiping();
    let done = rig.flow.cancel().await.unwrap();
    assert!(!done.cancelled);
    assert!(
        rig.flow.snapshot().departure.unwrap().wiping,
        "still wiping"
    );
    rig.flow.resume_at_startup().await;
    assert!(rig.erased());
}

#[tokio::test]
async fn cancel_from_the_banner_works_while_a_retry_runs_and_the_retry_changes_nothing_after() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    let kicks = rig.kicked.load(Ordering::SeqCst);
    let hold = rig.core.hold_next_attempt();
    let flow = rig.flow.clone();
    let task = tokio::spawn(async move { flow.retry(Some(1)).await });
    hold.entered.notified().await;
    assert!(rig.flow.snapshot().working, "Réessayer is off");
    // A second tap on Réessayer does nothing.
    assert!(rig.flow.retry(Some(1)).await.unwrap().working);

    let done = rig.flow.cancel().await.unwrap();
    assert!(done.cancelled);
    assert!(rig.flow.snapshot().cancelling);
    hold.release.notify_one();
    task.await.unwrap().unwrap();

    assert!(!rig.erased(), "the cancelled retry erased nothing");
    assert_eq!(rig.core.relay_state(), (false, false));
    assert_eq!(rig.kicked.load(Ordering::SeqCst), kicks, "no new series");
    assert!(!rig.flow.snapshot().cancelling);
}

#[tokio::test]
async fn cancel_ends_the_series_flag() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    assert!(rig.flow.series_running.load(Ordering::SeqCst), "kicked");
    rig.flow.cancel().await.unwrap();
    assert!(
        !rig.flow.series_running.load(Ordering::SeqCst),
        "a cancelled series no longer blocks the pokes"
    );
}

#[tokio::test]
async fn no_new_leave_dialog_while_a_cancelled_attempt_still_runs() {
    let rig = Rig::new(&[]);
    let (hold, task) = leave_attempt_in_flight(&rig).await;
    rig.flow.cancel_running().await.unwrap();
    rig.flow.open_leave().unwrap();
    assert_eq!(
        rig.screen(),
        LeaveScreen::Hidden,
        "wait for the attempt to end"
    );
    hold.release.notify_one();
    task.await.unwrap().unwrap();
    rig.flow.open_leave().unwrap();
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
}

// ------------------------------------------------------------------ Windows Hello

#[tokio::test]
async fn retrying_a_pending_deletion_asks_windows_hello_every_time() {
    let rig = Rig::new(&[
        AuthOutcome::Verified,
        AuthOutcome::Verified,
        AuthOutcome::Verified,
    ]);
    open_delete(&rig).await;
    rig.core.simulate_relay(RelayMode::Offline);
    rig.pass(ARM_MS);
    // The first gesture (with Hello): the request does not get through, the banner takes over.
    rig.flow.confirm_delete(Some(1)).await.unwrap();
    let view = rig.flow.snapshot().departure.expect("waiting");
    assert_eq!(view.kind, DepartureKind::DeleteSpace);
    assert_eq!(rig.auth.times_asked(), 1);

    // "Réessayer" on that banner: Hello again, every time.
    rig.flow.retry(Some(1)).await.unwrap();
    assert_eq!(rig.auth.times_asked(), 2);
    assert_eq!(
        rig.auth.asked.lock().unwrap()[1],
        strings::hello_delete_space()
    );
    rig.flow.retry(Some(1)).await.unwrap();
    assert_eq!(rig.auth.times_asked(), 3);
    assert!(!rig.erased());
    assert_eq!(rig.host.prompts_open(), 0);
}

#[tokio::test]
async fn retrying_a_pending_deletion_without_windows_hello_sends_nothing() {
    for outcome in [
        AuthOutcome::Cancelled,
        AuthOutcome::Unavailable,
        AuthOutcome::Failed,
        AuthOutcome::Busy,
    ] {
        let rig = Rig::new(&[AuthOutcome::Verified, outcome]);
        open_delete(&rig).await;
        rig.core.simulate_relay(RelayMode::Offline);
        rig.pass(ARM_MS);
        rig.flow.confirm_delete(Some(1)).await.unwrap();
        // The relay is back, but the retry needs Hello: refused, the departure keeps waiting.
        rig.core.simulate_relay(RelayMode::Normal);
        assert!(rig.flow.retry(Some(1)).await.is_err(), "{outcome:?}");
        assert!(!rig.erased(), "{outcome:?}");
        assert_eq!(rig.core.relay_state(), (false, false), "{outcome:?}");
        let snap = rig.flow.snapshot();
        assert!(snap.departure.is_some(), "{outcome:?}");
        assert!(!snap.working && !snap.authenticating, "{outcome:?}");
    }
}

#[tokio::test]
async fn retrying_a_pending_leave_needs_no_windows_hello() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.flow.retry(Some(1)).await.unwrap();
    assert!(rig.erased());
    assert_eq!(rig.auth.times_asked(), 0);
}

/// A Windows Hello that records whether the popover was protected while it was on screen.
struct ProbeAuth {
    host: Arc<TestHost>,
    open_during: Mutex<Vec<usize>>,
    answer: AuthOutcome,
}

impl StrongAuth for ProbeAuth {
    fn verify(
        &self,
        _owner: Option<isize>,
        _message: &str,
        _cancel: &std::sync::atomic::AtomicBool,
    ) -> AuthOutcome {
        self.open_during
            .lock()
            .unwrap()
            .push(self.host.prompts_open());
        self.answer
    }
}

#[tokio::test]
async fn the_popover_is_held_while_windows_hello_is_on_screen_and_released_on_every_path() {
    for answer in [
        AuthOutcome::Verified,
        AuthOutcome::Cancelled,
        AuthOutcome::Unavailable,
        AuthOutcome::Busy,
        AuthOutcome::Failed,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let core = Arc::new(FakeCore::new(dir.path().join("store")).without_delay());
        core.simulate_last_device();
        let host = Arc::new(TestHost::default());
        let probe = Arc::new(ProbeAuth {
            host: host.clone(),
            open_during: Mutex::new(Vec::new()),
            answer,
        });
        let now = Arc::new(AtomicI64::new(1_000_000));
        let clock = now.clone();
        let flow = LeaveFlow::new(
            core,
            probe.clone(),
            host.clone(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
            Arc::new(|_task| {}),
            Arc::new(|| 0.0),
        );
        flow.open_leave().unwrap();
        flow.confirm_leave().await.unwrap();
        now.fetch_add(ARM_MS, Ordering::SeqCst);

        let _ = flow.confirm_delete(Some(1)).await;

        assert_eq!(
            probe.open_during.lock().unwrap().as_slice(),
            [1],
            "{answer:?}: the guard was up while the prompt was"
        );
        assert_eq!(host.prompts_open(), 0, "{answer:?}: and down afterwards");
        assert_eq!(host.prompts_opened.load(Ordering::SeqCst), 1, "{answer:?}");
        assert!(!flow.snapshot().authenticating, "{answer:?}");
    }
}

#[tokio::test]
async fn the_dialog_shows_authenticating_apart_from_the_request_under_way() {
    // `authenticating` is true only while Hello is on screen, and the dialog is busy then too.
    let rig = Rig::new(&[]);
    rig.flow.open_leave().unwrap();
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    let before = rig.flow.snapshot();
    assert!(before.ui.busy && !before.authenticating);
    {
        let _prompt = Prompt::open(&rig.flow);
        let during = rig.flow.snapshot();
        assert!(during.ui.busy && during.authenticating);
        assert_eq!(rig.host.prompts_open(), 1);
    }
    assert!(!rig.flow.snapshot().authenticating);
    assert_eq!(rig.host.prompts_open(), 0);
}

// ------------------------------------------------------------------ which window draws what

#[tokio::test]
async fn a_dialog_belongs_to_the_window_of_the_gesture_that_opened_it() {
    let rig = Rig::new(&[]);
    rig.flow.note_origin("popover");
    rig.flow.open_leave().unwrap();
    assert_eq!(rig.flow.snapshot().ui.window.as_deref(), Some("popover"));

    // A refusal met after "Réessayer" in the other window reopens the dialog there.
    rig.flow.dismiss();
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.simulate_last_device();
    rig.flow.note_origin("history");
    rig.flow.retry(Some(1)).await.unwrap();
    let ui = rig.flow.snapshot().ui;
    assert_eq!(ui.screen, LeaveScreen::ConfirmDelete);
    assert_eq!(ui.window.as_deref(), Some("history"));
}

#[tokio::test]
async fn the_popover_going_away_closes_its_waiting_dialog_but_not_one_under_way_or_elsewhere() {
    let rig = Rig::new(&[]);
    rig.flow.note_origin("popover");
    rig.flow.open_leave().unwrap();
    // The other window's coming and going does not touch it.
    rig.flow.window_hidden("history");
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
    // The popover hides (or is shown again): the question is gone, not left waiting.
    rig.flow.window_hidden("popover");
    assert_eq!(rig.screen(), LeaveScreen::Hidden);

    // Under way: untouched.
    rig.flow.open_leave().unwrap();
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    rig.flow.window_hidden("popover");
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
    assert!(rig.flow.snapshot().ui.busy);
}

#[tokio::test]
async fn the_deletion_dialog_reopened_in_a_hidden_popover_is_closed_when_it_comes_back() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.simulate_last_device();
    rig.flow.note_origin("popover");
    rig.host.hidden.lock().unwrap().push("popover".into());
    rig.flow.retry(Some(1)).await.unwrap();
    assert_eq!(rig.screen(), LeaveScreen::ConfirmDelete);
    // Nobody saw that question: the refusal is told as a notice, not lost.
    assert_eq!(
        rig.host.notices.lock().unwrap().as_slice(),
        ["error.departure_refused_last_device"]
    );
    // The popover is shown again later: it does not greet the user with an armed question.
    rig.flow.window_hidden("popover");
    assert_eq!(rig.screen(), LeaveScreen::Hidden);
    assert!(!rig.erased());
}

// ------------------------------------------------------------------ the small gaps

#[tokio::test]
async fn of_several_windows_poking_at_once_only_one_attempt_is_made() {
    let rig = Rig::new(&[]);
    let flow = rig.flow.clone();
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let flow = flow.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                flow.claim_auto_slot()
            })
        })
        .collect();
    let winners: usize = threads
        .into_iter()
        .map(|t| usize::from(t.join().unwrap()))
        .sum();
    assert_eq!(winners, 1);
    assert!(!flow.claim_auto_slot(), "and not again within the minute");
    rig.pass(POKE_MIN_INTERVAL_MS);
    assert!(flow.claim_auto_slot(), "the next minute opens again");
}

#[tokio::test]
async fn opening_erase_after_a_refusal_says_why_the_departure_is_gone() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Offline).await;
    // The relay is back but this is the last device: the fresh look is refused for good.
    rig.core.simulate_relay(RelayMode::Normal);
    rig.core.simulate_last_device();
    let snap = rig.flow.open_erase().await.unwrap();
    assert!(snap.departure.is_none(), "the marker is gone");
    assert!(snap.ui.not_allowed);
    assert_eq!(
        rig.host.notices.lock().unwrap().as_slice(),
        ["error.departure_refused_last_device"]
    );
}

#[tokio::test(start_paused = true)]
async fn after_the_six_delays_the_series_goes_on_once_a_minute_until_the_relay_answers() {
    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    // 2 + 5 + 15 + 30 + 60 + 120 s: the six delays of A4, all met offline.
    tokio::time::sleep(Duration::from_millis(232_500)).await;
    assert!(
        rig.flow.series_running.load(Ordering::SeqCst),
        "still waiting, still polling"
    );
    assert!(!rig.erased());
    // The network is back: the next minute finds the proof.
    rig.core.simulate_relay(RelayMode::Normal);
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert!(rig.erased());
    assert!(!rig.flow.series_running.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn the_slow_tail_stops_on_a_server_that_does_not_find_the_space() {
    let rig = Rig::build(&[], true);
    waiting_for(&rig, RelayMode::Offline).await;
    tokio::time::sleep(Duration::from_secs(240)).await;
    rig.core.simulate_relay(RelayMode::Reset);
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert!(
        !rig.flow.series_running.load(Ordering::SeqCst),
        "'not found' ends the tail: the banner offers 'Effacer quand même'"
    );
    assert!(!rig.erased());
}

// ------------------------------------------------------------------ "Annuler" is locked while the user is being asked

async fn hello_is_on_screen(gate: &GatedAuth) {
    for _ in 0..500 {
        if gate.entered.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("Windows Hello never showed");
}

#[tokio::test]
async fn cancel_does_nothing_while_windows_hello_is_on_screen_and_the_erasure_then_goes_through() {
    let gate = GatedAuth::new(AuthOutcome::Verified);
    let rig = Rig::with_prompt(gate.clone());
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    rig.flow.set_erase_checked(true);
    rig.pass(ARM_MS);
    let flow = rig.flow.clone();
    let task = tokio::spawn(async move { flow.confirm_erase(Some(1)).await });
    hello_is_on_screen(&gate).await;
    assert!(rig.flow.snapshot().authenticating);

    // The banner of the other window: "Annuler" answers "nothing", the marker stays readable.
    let done = rig.flow.cancel().await.unwrap();
    assert!(!done.cancelled && done.kind.is_none());
    let done = rig.flow.cancel_running().await.unwrap();
    assert!(!done.cancelled);
    assert!(
        rig.flow.snapshot().departure.is_some(),
        "the marker is still there"
    );
    assert!(!rig.erased());

    gate.release();
    task.await.unwrap().unwrap();
    assert!(
        rig.erased(),
        "and Windows Hello then lets the erasure go through"
    );
}

#[tokio::test]
async fn cancel_does_nothing_once_erasing_anyway_is_confirmed() {
    let rig = Rig::new(&[]);
    waiting_for(&rig, RelayMode::Reset).await;
    rig.flow.open_erase().await.unwrap();
    // The confirmation is under way (past the checkbox and the 2 s; Hello, then the erasure).
    rig.flow.begin(LeaveScreen::EraseWithoutProof).unwrap();
    let done = rig.flow.cancel().await.unwrap();
    assert!(!done.cancelled);
    assert!(rig.flow.snapshot().departure.is_some());
    assert!(rig.flow.snapshot().ui.busy, "the dialog is untouched");
    // Once nothing is under way, "Annuler" works again.
    rig.flow.edit_ui(|ui| ui.busy = false);
    assert!(rig.flow.cancel().await.unwrap().cancelled);
}

#[tokio::test]
async fn the_cancelling_state_is_on_the_snapshot_without_a_marker_until_the_attempt_ends() {
    // What the banner draws as "Annulation en cours": no marker any more, an attempt still out.
    let rig = Rig::new(&[]);
    let (hold, task) = leave_attempt_in_flight(&rig).await;
    rig.flow.cancel_running().await.unwrap();
    let snap = rig.flow.snapshot();
    assert!(snap.departure.is_none(), "the marker is gone");
    assert!(
        snap.cancelling && snap.working,
        "yet the banner has something to say"
    );
    hold.release.notify_one();
    task.await.unwrap().unwrap();
    let snap = rig.flow.snapshot();
    assert!(!snap.cancelling && !snap.working, "and it stops saying it");
}

// ------------------------------------------------------------------ windows going away

#[tokio::test]
async fn closing_the_big_window_closes_its_waiting_dialog() {
    let rig = Rig::new(&[]);
    rig.flow.note_origin("history");
    rig.flow.open_leave().unwrap();
    assert_eq!(rig.flow.snapshot().ui.window.as_deref(), Some("history"));
    // The popover coming and going does not touch it.
    rig.flow.window_hidden("popover");
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
    // The window is closed (popover.rs calls this on `Destroyed`).
    rig.flow.window_hidden("history");
    assert_eq!(rig.screen(), LeaveScreen::Hidden);

    // One that is under way is left to finish.
    rig.flow.open_leave().unwrap();
    rig.flow.begin(LeaveScreen::ConfirmLeave).unwrap();
    rig.flow.window_hidden("history");
    assert_eq!(rig.screen(), LeaveScreen::ConfirmLeave);
}

#[tokio::test]
async fn a_refusal_for_a_window_nobody_sees_is_told_and_one_for_a_visible_window_is_not() {
    for hidden in [true, false] {
        let rig = Rig::new(&[]);
        waiting_for(&rig, RelayMode::Offline).await;
        rig.core.simulate_relay(RelayMode::Normal);
        rig.core.simulate_last_device();
        rig.flow.note_origin("popover");
        if hidden {
            rig.host.hidden.lock().unwrap().push("popover".into());
        }
        rig.flow.retry(Some(1)).await.unwrap();
        assert_eq!(rig.screen(), LeaveScreen::ConfirmDelete);
        let notices = rig.host.notices.lock().unwrap().clone();
        if hidden {
            assert_eq!(
                notices,
                ["error.departure_refused_last_device"],
                "the refusal does not vanish unread"
            );
        } else {
            assert!(notices.is_empty(), "the dialog says it");
        }
    }
}
