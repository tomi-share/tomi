//! The demo core keeps the rules of tables T1 to T4 of `spec/api-v1.md` 7.9.3: one test per
//! family of rows. The window's tests rely on this.

use std::sync::Arc;

use super::*;
use crate::core_api::TomiCore;

fn core() -> (tempfile::TempDir, Arc<FakeCore>) {
    let dir = tempfile::tempdir().unwrap();
    let core = Arc::new(FakeCore::new(dir.path().join("store")).without_delay());
    (dir, core)
}

fn solo() -> (tempfile::TempDir, Arc<FakeCore>) {
    let (d, c) = core();
    c.simulate_last_device();
    (d, c)
}

fn is_erased(c: &FakeCore) -> bool {
    !c.status().paired
}

fn pending(reason: PendingReason) -> DepartureOutcome {
    DepartureOutcome::Pending { reason }
}

fn refused(reason: RefusalReason) -> DepartureOutcome {
    DepartureOutcome::Refused { reason }
}

#[tokio::test]
async fn t1_t2_a_proof_erases_and_only_a_proof() {
    // 200 with the proof, or 403 revoked: erased.
    for reply in [Reply::Ok, Reply::Revoked] {
        let (_d, c) = core();
        c.script(Step::LeaveMe, reply);
        assert_eq!(c.leave_space().await.unwrap(), DepartureOutcome::Erased);
        assert!(is_erased(&c));
        assert_eq!(c.pending_marker(), None);
        assert!(!c.can_erase_without_proof());
    }
    // A 2xx that is not the proof, a network error, a lost answer: waiting, nothing erased,
    // the marker stays and D4 is not allowed.
    for (reply, why) in [
        (Reply::OkNoProof, PendingReason::Network),
        (Reply::Network, PendingReason::Network),
        (Reply::LostAfterEffect, PendingReason::Network),
        (Reply::Server, PendingReason::Server),
    ] {
        let (_d, c) = core();
        c.script(Step::LeaveMe, reply);
        assert_eq!(c.leave_space().await.unwrap(), pending(why), "{reply:?}");
        assert!(!is_erased(&c), "{reply:?}");
        assert_eq!(c.pending_marker(), Some("requested"), "{reply:?}");
        assert!(!c.can_erase_without_proof(), "{reply:?}");
    }
}

#[tokio::test]
async fn t1_a_lost_answer_is_settled_by_the_403_of_the_retry() {
    let (_d, c) = core();
    c.script(Step::LeaveMe, Reply::LostAfterEffect);
    assert_eq!(
        c.leave_space().await.unwrap(),
        pending(PendingReason::Network)
    );
    assert_eq!(
        c.relay_state(),
        (true, false),
        "the relay did remove this PC"
    );
    assert_eq!(c.retry_departure().await.unwrap(), DepartureOutcome::Erased);
    assert!(is_erased(&c));
}

#[tokio::test]
async fn t1_a_401_or_a_missing_space_is_not_found_and_never_a_proof() {
    for reply in [Reply::Unauthorized, Reply::SpaceNotInitialized] {
        let (_d, c) = core();
        c.script(Step::Session, reply);
        assert_eq!(
            c.leave_space().await.unwrap(),
            pending(PendingReason::NotFound),
            "{reply:?}"
        );
        assert!(!is_erased(&c));
        assert_eq!(c.status().space, SpaceState::NotFound);
        assert!(c.can_erase_without_proof(), "D4 may be offered");
        assert_eq!(c.pending_marker(), Some("requested"), "kept, not removed");
    }
}

#[tokio::test]
async fn t1_a_session_that_opens_again_clears_the_not_found_state() {
    let (_d, c) = core();
    c.simulate_relay(RelayMode::Reset);
    c.leave_space().await.unwrap();
    assert_eq!(c.status().space, SpaceState::NotFound);
    c.simulate_relay(RelayMode::Offline);
    c.retry_departure().await.unwrap();
    assert_eq!(
        c.status().space,
        SpaceState::NotFound,
        "a network error proves nothing"
    );
    c.simulate_relay(RelayMode::Normal);
    assert_eq!(c.retry_departure().await.unwrap(), DepartureOutcome::Erased);
}

#[tokio::test]
async fn t2_final_refusals_remove_the_marker() {
    for (reply, reason) in [
        (Reply::LastDevice, RefusalReason::LastDevice),
        (Reply::UpdateRequired, RefusalReason::UpdateRequired),
        (Reply::Unexpected4xx, RefusalReason::Unexpected),
        (Reply::NotFound404, RefusalReason::Unexpected),
    ] {
        let (_d, c) = core();
        c.script(Step::LeaveMe, reply);
        assert_eq!(c.leave_space().await.unwrap(), refused(reason), "{reply:?}");
        assert_eq!(c.pending_marker(), None, "{reply:?}");
        assert!(!is_erased(&c), "{reply:?}");
        assert!(c.departure().is_none(), "{reply:?}");
    }
    // The session itself refused for an update.
    let (_d, c) = core();
    c.script(Step::Session, Reply::UpdateRequired);
    assert_eq!(
        c.leave_space().await.unwrap(),
        refused(RefusalReason::UpdateRequired)
    );
}

#[tokio::test]
async fn t3_the_last_device_deletes_the_space_on_the_proof() {
    let (_d, c) = solo();
    assert_eq!(
        c.leave_space().await.unwrap(),
        refused(RefusalReason::LastDevice)
    );
    assert_eq!(c.delete_space().await.unwrap(), DepartureOutcome::Erased);
    assert!(is_erased(&c));
    assert_eq!(c.relay_state(), (false, true));
    assert!(!c.has_deletion_id(), "removed with the erasure");
}

#[tokio::test]
async fn t3_a_lost_answer_is_settled_by_the_receipt_without_a_second_delete() {
    let (_d, c) = solo();
    c.script(Step::DeleteSpace, Reply::LostAfterEffect);
    assert_eq!(
        c.delete_space().await.unwrap(),
        pending(PendingReason::Network)
    );
    assert!(c.has_deletion_id());
    assert_eq!(c.pending_marker(), Some("deleting"));
    // The retry asks the receipt first: 200, erased; nothing is left scripted or sent again.
    assert_eq!(c.retry_departure().await.unwrap(), DepartureOutcome::Erased);
    assert_eq!(c.scripted_left(), 0);
}

#[tokio::test]
async fn t3_failures_wait_and_a_missing_route_means_a_relay_too_old() {
    for (reply, why) in [
        (Reply::Network, PendingReason::Network),
        (Reply::OkNoProof, PendingReason::Network),
        (Reply::Server, PendingReason::Server),
    ] {
        let (_d, c) = solo();
        c.script(Step::DeleteSpace, reply);
        assert_eq!(c.delete_space().await.unwrap(), pending(why), "{reply:?}");
        assert!(!c.can_erase_without_proof(), "{reply:?}");
        assert_eq!(c.pending_marker(), Some("deleting"));
    }
    let (_d, c) = solo();
    c.simulate_relay(RelayMode::OldRelay);
    assert_eq!(
        c.delete_space().await.unwrap(),
        refused(RefusalReason::RelayTooOld)
    );
    assert!(c.can_erase_without_proof(), "D4 after a relay too old");
    assert_eq!(c.pending_marker(), None);
    assert!(c.has_deletion_id(), "kept: a later deletion reuses it");
}

#[tokio::test]
async fn t3_not_the_last_device_goes_back_to_a_normal_departure() {
    let (_d, c) = core();
    // Two devices: DELETE answers 409 not_last_device, then the departure continues as a leave.
    assert_eq!(c.delete_space().await.unwrap(), DepartureOutcome::Erased);
    assert!(is_erased(&c));
    assert_eq!(
        c.relay_state(),
        (true, false),
        "left, did not delete the space"
    );
}

#[tokio::test]
async fn t3_a_401_after_the_delete_is_settled_by_a_receipt_or_is_not_found() {
    let (_d, c) = solo();
    c.script(Step::DeleteSpace, Reply::Unauthorized);
    c.script(Step::Receipt, Reply::NotFound404); // before the delete
    c.script(Step::Receipt, Reply::Ok); // after the 401
    assert_eq!(c.delete_space().await.unwrap(), DepartureOutcome::Erased);

    let (_d, c) = solo();
    c.script(Step::DeleteSpace, Reply::SpaceNotInitialized);
    assert_eq!(
        c.delete_space().await.unwrap(),
        pending(PendingReason::NotFound)
    );
    assert!(!is_erased(&c));
}

#[tokio::test]
async fn t4_a_found_receipt_erases_before_anything_else() {
    let (_d, c) = solo();
    c.script(Step::Receipt, Reply::Ok);
    assert_eq!(c.delete_space().await.unwrap(), DepartureOutcome::Erased);
    assert_eq!(c.relay_state(), (false, false), "no DELETE was needed");
}

#[tokio::test]
async fn a_window_of_fifteen_minutes_resends_then_only_verifies() {
    let (_d, c) = core();
    c.simulate_relay(RelayMode::Offline);
    c.leave_space().await.unwrap();
    c.simulate_relay(RelayMode::Normal);
    // Inside the window: the request is sent again and erases.
    c.advance_clock(14 * 60_000);
    assert_eq!(
        c.continue_departure().await.unwrap(),
        DepartureOutcome::Erased
    );

    let (_d, c) = core();
    c.simulate_relay(RelayMode::Offline);
    c.leave_space().await.unwrap();
    c.simulate_relay(RelayMode::Normal);
    c.advance_clock(16 * 60_000);
    assert_eq!(
        c.continue_departure().await.unwrap(),
        pending(PendingReason::NotSent)
    );
    assert_eq!(
        c.relay_state(),
        (false, false),
        "verification only: nothing was sent"
    );
    assert!(!is_erased(&c));
    // "Réessayer" is a gesture: it refreshes the time and resends.
    assert_eq!(c.retry_departure().await.unwrap(), DepartureOutcome::Erased);
}

#[tokio::test]
async fn an_old_marker_without_a_time_only_verifies() {
    let (_d, c) = core();
    c.simulate_relay(RelayMode::Offline);
    c.leave_space().await.unwrap();
    c.sim().since_ms = None;
    c.simulate_relay(RelayMode::Normal);
    assert_eq!(
        c.continue_departure().await.unwrap(),
        pending(PendingReason::NotSent)
    );
}

#[tokio::test]
async fn nothing_pending_is_said_so() {
    let (_d, c) = core();
    assert_eq!(
        c.continue_departure().await.unwrap(),
        DepartureOutcome::NothingPending
    );
    assert!(c.retry_departure().await.is_err());
    assert!(!c.cancel_departure().await.unwrap());
}

#[tokio::test]
async fn cancelling_during_an_attempt_erases_nothing() {
    let (_d, c) = core();
    let again = c.clone();
    c.during_next_request(move || {
        again.dep_cancel().unwrap();
    });
    // The request reached the relay (this PC was removed there) and "Annuler" came before the
    // answer: nothing is erased, the data stays readable, the state is "revoked".
    assert_eq!(c.leave_space().await.unwrap(), DepartureOutcome::Cancelled);
    assert!(!is_erased(&c));
    assert_eq!(c.status().space, SpaceState::Revoked);
    assert_eq!(c.pending_marker(), None);

    let (_d, c) = solo();
    let again = c.clone();
    c.during_next_request(move || {
        again.dep_cancel().unwrap();
    });
    assert_eq!(c.delete_space().await.unwrap(), DepartureOutcome::Cancelled);
    assert!(!is_erased(&c));
    assert_eq!(c.status().space, SpaceState::NotFound);
    assert!(c.has_deletion_id(), "kept: a new deletion reuses it");
    assert!(c.can_erase_without_proof() || c.status().space == SpaceState::NotFound);
}

#[tokio::test]
async fn a_restart_finishes_an_interrupted_erasure_and_forgets_the_verdict() {
    let (_d, c) = core();
    c.interrupt_in_wiping();
    assert!(c.departure().unwrap().wiping);
    assert!(c.cancel_departure().await.is_ok());
    assert!(
        c.departure().unwrap().wiping,
        "never cancellable once wiping"
    );
    c.reopen();
    assert!(is_erased(&c));
    assert!(c.departure().is_none());

    let (_d, c) = core();
    c.simulate_relay(RelayMode::Reset);
    c.leave_space().await.unwrap();
    assert!(c.can_erase_without_proof());
    c.reopen();
    assert!(
        !c.can_erase_without_proof(),
        "the verdict lives in memory only"
    );
}

#[tokio::test]
async fn erasing_without_proof_needs_a_recent_verdict_and_calls_nothing() {
    let (_d, c) = core();
    assert!(c.erase_without_proof().await.is_err());
    c.simulate_relay(RelayMode::Reset);
    c.leave_space().await.unwrap();
    // 11 minutes later the verdict is too old.
    c.advance_clock(11 * 60_000);
    assert!(!c.can_erase_without_proof());
    assert!(c.erase_without_proof().await.is_err());
    assert!(!is_erased(&c));
    c.retry_departure().await.unwrap();
    assert!(c.can_erase_without_proof());
    c.erase_without_proof().await.unwrap();
    assert!(is_erased(&c));
    assert_eq!(c.relay_state(), (false, false));
    assert_eq!(c.pending_marker(), None);
}

#[tokio::test]
async fn an_unavailable_core_answers_the_dedicated_error_to_every_call() {
    let (_d, c) = core();
    c.simulate_unavailable();
    assert!(!c.departure_available());
    assert_eq!(c.leave_space().await, Err(CoreError::DepartureUnavailable));
    assert_eq!(c.delete_space().await, Err(CoreError::DepartureUnavailable));
    assert_eq!(
        c.retry_departure().await,
        Err(CoreError::DepartureUnavailable)
    );
    assert_eq!(
        c.continue_departure().await,
        Err(CoreError::DepartureUnavailable)
    );
    assert_eq!(
        c.cancel_departure().await,
        Err(CoreError::DepartureUnavailable)
    );
    assert_eq!(
        c.erase_without_proof().await,
        Err(CoreError::DepartureUnavailable)
    );
    assert!(!is_erased(&c));
}

#[tokio::test]
async fn a_second_departure_cannot_start_over_a_pending_one() {
    let (_d, c) = core();
    c.simulate_relay(RelayMode::Offline);
    c.leave_space().await.unwrap();
    assert!(c.leave_space().await.is_err());
    assert!(c.delete_space().await.is_err());
}

#[tokio::test]
async fn after_an_erasure_the_demo_pc_can_pair_again_with_a_clean_space() {
    let (_d, c) = core();
    c.leave_space().await.unwrap();
    assert!(is_erased(&c));
    // Pairing again (create or join) gives a fresh demo space: not erased, nothing pending,
    // the other devices are back, and the old verdicts are gone.
    c.pairing_create_space("token", None).await.unwrap();
    assert!(!is_erased(&c));
    assert!(c.departure().is_none());
    assert!(!c.can_erase_without_proof());
    assert_eq!(c.devices().await.unwrap().len(), 3);
    // And it can leave again.
    assert_eq!(c.leave_space().await.unwrap(), DepartureOutcome::Erased);
    assert!(is_erased(&c));
    c.pairing_join("4821-crayon-renard", None).await.unwrap();
    assert!(!is_erased(&c));
}

#[tokio::test]
async fn a_pc_that_is_not_erased_cannot_create_or_join_a_second_space() {
    let (_d, c) = core();
    assert!(c.pairing_create_space("token", None).await.is_err());
    assert!(c.pairing_join("4821-crayon-renard", None).await.is_err());
    assert!(!is_erased(&c));
}
