//! Devices the relay removed on its own authority, waiting to be told to the user once.
//!
//! The core hands each such revocation over exactly once (`TomiCore::take_relay_revocations`
//! clears its persisted list), so it is taken only when a window that is on screen asks
//! (`take_relay_notices`, which calls [`collect`]): taking again returns nothing, so the popover
//! and the main window never both show the same notice. A window that is hidden does not ask (the
//! front checks that it is visible), so the notice stays in the core, which keeps it across a
//! restart, until somebody can read it. A device name and an id: nothing secret, nothing to wipe.

use std::sync::Mutex;

use crate::core_api::{CoreError, RelayRevocation, TomiCore};

/// More than this is a flood, not news: the oldest ones are dropped.
const MAX_PENDING: usize = 16;

#[derive(Default)]
pub struct RelayNotices {
    pending: Mutex<Vec<RelayRevocation>>,
}

impl RelayNotices {
    /// Adds what the core just handed over. The same device removed the same way twice (a core
    /// that told it again after a restart) is kept once. Returns whether something is pending.
    pub fn push(&self, fresh: Vec<RelayRevocation>) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        for r in fresh {
            if !pending
                .iter()
                .any(|p| p.device_id == r.device_id && p.via == r.via)
            {
                pending.push(r);
            }
        }
        if pending.len() > MAX_PENDING {
            let excess = pending.len() - MAX_PENDING;
            pending.drain(..excess);
        }
        !pending.is_empty()
    }

    /// Hands the pending notices over, once.
    pub fn take(&self) -> Vec<RelayRevocation> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// Reads what the core has to tell and queues it. Returns whether something is pending. Called
/// only on behalf of a window that is on screen, never on a timer: a failure to read leaves the
/// core's list untouched, so the next look tries again.
pub fn collect(core: &dyn TomiCore, notices: &RelayNotices) -> Result<bool, CoreError> {
    let fresh = core.take_relay_revocations()?;
    if fresh.is_empty() {
        return Ok(false);
    }
    Ok(notices.push(fresh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_api::RelayRevocationVia;
    use crate::fake_core::FakeCore;

    fn revocation(id: &str, name: &str, via: RelayRevocationVia) -> RelayRevocation {
        RelayRevocation {
            device_id: id.into(),
            name: name.into(),
            via,
        }
    }

    #[test]
    fn a_notice_is_taken_once() {
        let notices = RelayNotices::default();
        assert!(notices.push(vec![revocation("a", "Pixel", RelayRevocationVia::Admin)]));
        assert_eq!(notices.take().len(), 1);
        assert!(notices.take().is_empty(), "once");
    }

    #[test]
    fn the_same_device_removed_the_same_way_is_kept_once() {
        let notices = RelayNotices::default();
        notices.push(vec![revocation("a", "Pixel", RelayRevocationVia::Admin)]);
        notices.push(vec![
            revocation("a", "Pixel", RelayRevocationVia::Admin),
            revocation("a", "Pixel", RelayRevocationVia::SelfDeparture),
        ]);
        assert_eq!(notices.take().len(), 2);
    }

    #[test]
    fn a_flood_keeps_the_latest() {
        let notices = RelayNotices::default();
        let many = (0..MAX_PENDING + 4)
            .map(|i| revocation(&format!("d{i}"), "X", RelayRevocationVia::Admin))
            .collect();
        notices.push(many);
        let taken = notices.take();
        assert_eq!(taken.len(), MAX_PENDING);
        assert_eq!(taken[0].device_id, "d4");
    }

    #[test]
    fn serialises_for_the_webview_without_anything_else() {
        let json =
            serde_json::to_value(revocation("a", "Pixel", RelayRevocationVia::SelfDeparture))
                .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"deviceId": "a", "name": "Pixel", "via": "self"})
        );
        let admin = serde_json::to_value(RelayRevocationVia::Admin).unwrap();
        assert_eq!(admin, serde_json::json!("admin"));
    }

    #[test]
    fn the_via_of_the_core_is_mapped_and_unknown_ones_stay_neutral() {
        assert_eq!(
            RelayRevocationVia::from_core("admin"),
            RelayRevocationVia::Admin
        );
        assert_eq!(
            RelayRevocationVia::from_core("self"),
            RelayRevocationVia::SelfDeparture
        );
        assert_eq!(
            RelayRevocationVia::from_core("later"),
            RelayRevocationVia::Other
        );
    }

    #[test]
    fn collect_queues_what_the_core_tells_once() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().join("store")).without_delay();
        let notices = RelayNotices::default();
        assert!(!collect(&core, &notices).unwrap(), "nothing to tell");

        core.relay_revokes(revocation("a", "Pixel", RelayRevocationVia::Admin));
        assert!(collect(&core, &notices).unwrap());
        // The core handed it over once: a second look finds nothing new, the notice is queued.
        assert!(!collect(&core, &notices).unwrap());
        assert_eq!(
            notices.take(),
            vec![revocation("a", "Pixel", RelayRevocationVia::Admin)]
        );
    }

    #[test]
    fn the_stalled_flag_follows_the_core() {
        let dir = tempfile::tempdir().unwrap();
        let core = FakeCore::new(dir.path().join("store")).without_delay();
        assert!(!core.rotation_stalled());
        core.set_rotation_stalled(true);
        assert!(core.rotation_stalled());
        core.set_rotation_stalled(false);
        assert!(!core.rotation_stalled());
    }
}
