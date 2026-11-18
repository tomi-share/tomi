//! Pending clipboard wipes of copied secrets.
//!
//! A secret copied by the app is wiped 30 s later if the clipboard still holds it. That
//! timer is not enough on its own:
//!
//! - **Two copies in a row.** The first timer would wipe the clipboard early, while the
//!   second copy (same value, so "still held") has just been made. Each copy takes a
//!   new *generation*; a timer only acts when its generation is still the current one.
//! - **The item disappears first** (tombstone from another device, deletion, expiry):
//!   the clipboard must not keep the value of an item that no longer exists.
//! - **The app quits before 30 s**: nobody would wipe it any more.
//!
//! So the app remembers *what* is pending (value and sequence number) and *for which
//! item*, and wipes from whichever of the three events comes first. The clipboard is
//! unique, so only the latest copy can still be there: registering replaces the
//! previous entry.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

/// What to wipe: the value copied and the clipboard sequence number right after our
/// own write (see `clipboard::clear_if_still`).
#[derive(Clone)]
pub struct PendingClear {
    pub generation: u64,
    /// The item the copy came from, `None` when that item was consumed by the copy itself.
    pub item_id: Option<String>,
    value: String,
    pub seq: Option<u32>,
}

impl PendingClear {
    pub fn value(&self) -> &str {
        &self.value
    }
}

// Holds a secret value: never printed.
impl std::fmt::Debug for PendingClear {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingClear")
            .field("generation", &self.generation)
            .field("item_id", &self.item_id)
            .field("value", &crate::model::REDACTED)
            .field("seq", &self.seq)
            .finish()
    }
}

#[derive(Default)]
pub struct ClipboardClears {
    counter: AtomicU64,
    pending: Mutex<Option<PendingClear>>,
}

impl ClipboardClears {
    fn lock(&self) -> MutexGuard<'_, Option<PendingClear>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Records a fresh copy and returns its generation (strictly increasing). Any earlier
    /// entry is superseded: the clipboard no longer holds it.
    pub fn register(&self, item_id: Option<&str>, value: &str, seq: Option<u32>) -> u64 {
        let generation = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        *self.lock() = Some(PendingClear {
            generation,
            item_id: item_id.map(str::to_string),
            value: value.to_string(),
            seq,
        });
        generation
    }

    /// The 30 s timer of `generation` fired. Returns the wipe to perform, or `None` when
    /// a later copy (or another event) has superseded or already handled it.
    pub fn take_if_current(&self, generation: u64) -> Option<PendingClear> {
        let mut pending = self.lock();
        if pending.as_ref().is_some_and(|p| p.generation == generation) {
            pending.take()
        } else {
            None
        }
    }

    /// `item_id` was removed: returns the wipe to perform when it is the one pending.
    pub fn take_for_item(&self, item_id: &str) -> Option<PendingClear> {
        let mut pending = self.lock();
        if pending
            .as_ref()
            .is_some_and(|p| p.item_id.as_deref() == Some(item_id))
        {
            pending.take()
        } else {
            None
        }
    }

    /// The app is exiting: whatever is pending must be wiped now.
    pub fn take_any(&self) -> Option<PendingClear> {
        self.lock().take()
    }

    /// A copy of what is pending, which stays pending: the caller wipes first and forgets
    /// afterwards ([`ClipboardClears::forget`]), so that a clipboard that could not be wiped is
    /// still wiped by its 30 s timer.
    pub fn peek(&self) -> Option<PendingClear> {
        self.lock().clone()
    }

    /// Forgets the pending entry `generation`, if it is still the current one.
    pub fn forget(&self, generation: u64) {
        let _ = self.take_if_current(generation);
    }

    /// Reconciliation after a missed `ItemRemoved` (the event channel lagged): returns the
    /// wipe to perform when the pending clear is tied to an item that is no longer present.
    /// A clear with no item (a copy that consumed its own item) keeps its 30 s timer.
    pub fn take_if_item_absent(&self, is_present: impl Fn(&str) -> bool) -> Option<PendingClear> {
        let mut pending = self.lock();
        let gone = pending
            .as_ref()
            .and_then(|p| p.item_id.as_deref())
            .is_some_and(|id| !is_present(id));
        if gone {
            pending.take()
        } else {
            None
        }
    }

    #[cfg(test)]
    pub fn is_pending(&self) -> bool {
        self.lock().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_timer_of_the_latest_copy_wipes() {
        let c = ClipboardClears::default();
        let g = c.register(Some("item1"), "s3cret", Some(7));
        let p = c.take_if_current(g).expect("current generation wipes");
        assert_eq!(p.value(), "s3cret");
        assert_eq!(p.item_id.as_deref(), Some("item1"));
        assert_eq!(p.seq, Some(7));
        assert!(!c.is_pending());
    }

    #[test]
    fn two_copies_of_the_same_secret_only_the_second_timer_wipes() {
        let c = ClipboardClears::default();
        let first = c.register(Some("item1"), "s3cret", Some(1));
        let second = c.register(Some("item1"), "s3cret", Some(2));
        assert!(second > first);
        // 30 s after the first copy: the second copy is only a few seconds old.
        assert!(c.take_if_current(first).is_none(), "early wipe prevented");
        assert!(c.is_pending());
        // 30 s after the second one.
        let p = c.take_if_current(second).expect("second timer wipes");
        assert_eq!(p.seq, Some(2));
    }

    #[test]
    fn a_timer_that_already_ran_does_not_wipe_again() {
        let c = ClipboardClears::default();
        let g = c.register(Some("item1"), "v", None);
        assert!(c.take_if_current(g).is_some());
        assert!(c.take_if_current(g).is_none());
    }

    #[test]
    fn removing_the_item_wipes_and_cancels_the_timer() {
        let c = ClipboardClears::default();
        let g = c.register(Some("item1"), "s3cret", Some(3));
        assert!(c.take_for_item("other").is_none(), "unrelated item ignored");
        let p = c.take_for_item("item1").expect("tombstone wipes");
        assert_eq!(p.value(), "s3cret");
        assert!(c.take_if_current(g).is_none(), "timer finds nothing left");
    }

    #[test]
    fn removing_an_earlier_item_does_not_touch_a_later_copy() {
        let c = ClipboardClears::default();
        c.register(Some("item1"), "a", None);
        c.register(Some("item2"), "b", None);
        assert!(c.take_for_item("item1").is_none());
        assert_eq!(c.take_for_item("item2").unwrap().value(), "b");
    }

    #[test]
    fn a_copy_that_consumed_its_item_survives_the_removal_of_that_item() {
        let c = ClipboardClears::default();
        let g = c.register(None, "s3cret", Some(4));
        // The item read once is erased by the copy itself: its removal must not wipe.
        assert!(c.take_for_item("item1").is_none());
        assert!(c.is_pending(), "still waiting for its 30 s");
        assert!(c.take_if_current(g).is_some(), "the timer still wipes it");
    }

    #[test]
    fn exit_takes_whatever_is_pending() {
        let c = ClipboardClears::default();
        assert!(c.take_any().is_none());
        c.register(Some("item1"), "s3cret", None);
        assert_eq!(c.take_any().unwrap().value(), "s3cret");
        assert!(c.take_any().is_none());
    }

    #[test]
    fn peeking_leaves_the_entry_for_its_timer_until_it_is_forgotten() {
        let c = ClipboardClears::default();
        let g = c.register(Some("item1"), "s3cret", Some(5));
        let seen = c.peek().expect("pending");
        assert_eq!(seen.value(), "s3cret");
        assert!(c.is_pending(), "peeking removes nothing");
        // A later copy replaced it: forgetting the old generation does not touch the new one.
        let newer = c.register(Some("item2"), "other", None);
        c.forget(g);
        assert!(c.is_pending());
        c.forget(newer);
        assert!(!c.is_pending());
    }

    #[test]
    fn debug_hides_the_value() {
        let c = ClipboardClears::default();
        c.register(Some("item1"), "s3cret-value", None);
        let dump = format!("{:?}", c.take_any().unwrap());
        assert!(!dump.contains("s3cret-value"));
    }
}
