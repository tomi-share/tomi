//! Injectable clock (`api-v1.md` §8.6): every time-dependent rule of the relay reads the
//! time through [`Clock`], so the tests can move it by days without waiting.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Source of the relay time, in milliseconds since the Unix epoch.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// The wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

/// A clock moved by hand (tests and the `admin` commands of a stopped relay).
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    pub fn new(start_ms: i64) -> Self {
        Self(Arc::new(AtomicI64::new(start_ms)))
    }

    /// Moves the clock forward by `ms` milliseconds.
    pub fn advance_ms(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    /// Moves the clock forward by `secs` seconds.
    pub fn advance_secs(&self, secs: i64) {
        self.advance_ms(secs.saturating_mul(1000));
    }

    /// Sets the clock (it may go backwards: the `seq` rule must survive that, §8.2).
    pub fn set_ms(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_by_hand() {
        let c = ManualClock::new(1_000);
        c.advance_secs(2);
        assert_eq!(c.now_ms(), 3_000);
        c.set_ms(5);
        assert_eq!(c.now_ms(), 5);
    }

    #[test]
    fn system_clock_is_after_2025() {
        assert!(SystemClock.now_ms() > 1_735_689_600_000);
    }
}
