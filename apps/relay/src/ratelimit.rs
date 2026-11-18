//! Token-bucket rate limits (`api-v1.md` §8.5). Counters live in memory and are lost at a
//! restart, which the specification accepts.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};

use crate::clock::Clock;
use crate::config::Rate;
use crate::db::lock;
use crate::error::ApiError;

/// The key under which a caller is counted, from its address. An IPv4 address counts for itself.
/// An IPv6 address counts for its whole `/64`: one subscriber normally gets at least that much,
/// so counting each `/128` would let a single host mint as many callers as it likes.
pub fn ip_key(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            octets[8..].fill(0);
            format!("{}/64", Ipv6Addr::from(octets))
        }
    }
}

/// Whether a table key (`name|caller`) is a counter shared by all callers (caller `*`).
fn is_global(key: &str) -> bool {
    key.ends_with("|*")
}

/// One counter. It carries its own rate, so that the cleanup judges it by that rate, not by the
/// one of whichever limit happens to be checked when the cleanup runs.
struct Bucket {
    tokens: f64,
    last_ms: i64,
    capacity: f64,
    per_ms: f64,
}

impl Bucket {
    /// Whether it would be full again by `now`: dropping it then changes nothing.
    fn is_full_at(&self, now: i64) -> bool {
        let idle = (now - self.last_ms).max(0) as f64;
        self.tokens + idle * self.per_ms >= self.capacity
    }
}

struct Table {
    buckets: HashMap<String, Bucket>,
    /// Earliest time of the next sweep of idle buckets.
    next_sweep_ms: i64,
}

/// A set of buckets keyed by (limit name, caller).
pub struct Limiter {
    clock: Arc<dyn Clock>,
    table: Mutex<Table>,
}

/// Above this many buckets, idle ones are swept (at most once per [`SWEEP_EVERY_MS`]).
const GC_THRESHOLD: usize = 20_000;
/// The table never holds more than this many buckets: past it, the least recently used quarter
/// is evicted (an evicted caller starts again with a full bucket, which is the lesser evil
/// against unbounded memory).
const MAX_BUCKETS: usize = 40_000;
/// Minimum delay between two sweeps, so the whole table is not walked under the lock at every
/// request while it sits above the threshold.
const SWEEP_EVERY_MS: i64 = 1_000;

impl Limiter {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            table: Mutex::new(Table {
                buckets: HashMap::new(),
                next_sweep_ms: 0,
            }),
        }
    }

    /// Takes one token from the bucket `name` of `key`, or answers `429 rate_limited` with
    /// the time to wait.
    pub fn check(&self, name: &str, key: &str, rate: Rate) -> Result<(), ApiError> {
        let now = self.clock.now_ms();
        let capacity = f64::from(rate.burst.max(1));
        let per_ms = rate.per_minute.max(0.0) / 60_000.0;
        let mut table = lock(&self.table);
        let k = format!("{name}|{key}");
        if !table.buckets.contains_key(&k) {
            Self::make_room(&mut table, now);
        }
        let b = table.buckets.entry(k).or_insert(Bucket {
            tokens: capacity,
            last_ms: now,
            capacity,
            per_ms,
        });
        b.capacity = capacity;
        b.per_ms = per_ms;
        let elapsed = (now - b.last_ms).max(0) as f64;
        b.tokens = (b.tokens + elapsed * per_ms).min(capacity);
        b.last_ms = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            return Ok(());
        }
        let wait_ms = if per_ms > 0.0 {
            ((1.0 - b.tokens) / per_ms).ceil()
        } else {
            3_600_000.0
        };
        let secs = (wait_ms / 1000.0).ceil().clamp(1.0, 86_400.0) as u64;
        Err(ApiError::rate_limited(secs))
    }

    /// Called before a new bucket is inserted: bounds the table.
    fn make_room(table: &mut Table, now: i64) {
        if table.buckets.len() > GC_THRESHOLD && now >= table.next_sweep_ms {
            table.buckets.retain(|_, b| !b.is_full_at(now));
            table.next_sweep_ms = now + SWEEP_EVERY_MS;
        }
        if table.buckets.len() >= MAX_BUCKETS {
            // Evict the least recently used quarter, the global counters excepted: they hold
            // a limit shared by all callers (`GlobalClaim`, `admin-web-all`), and evicting one
            // would give everybody a fresh budget. Residual: the caller key of an IPv6 address
            // is its `/64`, so the holder of a `/48` has 65 536 distinct "callers" and can still
            // push the table to its bound, which makes the others start again with a full
            // bucket. Bounded memory is preferred over that.
            let mut ages: Vec<i64> = table
                .buckets
                .iter()
                .filter(|(k, _)| !is_global(k))
                .map(|(_, b)| b.last_ms)
                .collect();
            if ages.is_empty() {
                return;
            }
            let cut = ages.len() / 4;
            let (_, threshold, _) = ages.select_nth_unstable(cut);
            let threshold = *threshold;
            let mut budget = cut + 1;
            table.buckets.retain(|k, b| {
                if !is_global(k) && b.last_ms <= threshold && budget > 0 {
                    budget -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        lock(&self.table).buckets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;

    #[test]
    fn burst_then_refill() {
        let clock = ManualClock::new(1_000_000);
        let l = Limiter::new(Arc::new(clock.clone()));
        let rate = Rate {
            per_minute: 60.0,
            burst: 3,
        };
        for _ in 0..3 {
            l.check("t", "ip", rate).unwrap();
        }
        let e = l.check("t", "ip", rate).unwrap_err();
        assert_eq!(e.status.as_u16(), 429);
        assert_eq!(e.ext["retry_after"], 1);
        // Another caller has its own bucket.
        l.check("t", "other", rate).unwrap();
        clock.advance_ms(1_000);
        l.check("t", "ip", rate).unwrap();
        assert!(l.check("t", "ip", rate).is_err());
    }

    #[test]
    fn slow_refill_gives_a_long_retry_after() {
        let clock = ManualClock::new(0);
        let l = Limiter::new(Arc::new(clock));
        let rate = Rate {
            per_minute: 1.0,
            burst: 1,
        };
        l.check("t", "k", rate).unwrap();
        let e = l.check("t", "k", rate).unwrap_err();
        assert_eq!(e.ext["retry_after"], 60);
    }

    #[test]
    fn ipv6_counts_per_64_and_ipv4_per_address() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(ip_key(a), ip_key(b));
        assert_ne!(ip_key(a), ip_key(c));
        assert_eq!(ip_key(a), "2001:db8:1:2::/64");
        let v4: IpAddr = "192.0.2.7".parse().unwrap();
        assert_eq!(ip_key(v4), "192.0.2.7");
        // An IPv4-mapped IPv6 address is the IPv4 one.
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(ip_key(mapped), "192.0.2.7");

        // Several /128 of one /64 share one bucket.
        let clock = ManualClock::new(0);
        let l = Limiter::new(Arc::new(clock));
        let rate = Rate {
            per_minute: 0.0,
            burst: 2,
        };
        l.check("t", &ip_key(a), rate).unwrap();
        l.check("t", &ip_key(b), rate).unwrap();
        assert!(l.check("t", &ip_key(a), rate).is_err());
    }

    #[test]
    fn cleanup_judges_each_bucket_by_its_own_rate() {
        let clock = ManualClock::new(0);
        let l = Limiter::new(Arc::new(clock.clone()));
        let slow = Rate {
            per_minute: 1.0,
            burst: 1,
        };
        let fast = Rate {
            per_minute: 600_000.0,
            burst: 1,
        };
        // A slow bucket, emptied: it needs a full minute to refill.
        l.check("slow", "k", slow).unwrap();
        assert!(l.check("slow", "k", slow).is_err());
        // Fill the table past the threshold with fast buckets, then let a few seconds pass.
        clock.advance_ms(2_000);
        for i in 0..=GC_THRESHOLD {
            l.check("fast", &i.to_string(), fast).unwrap();
        }
        clock.advance_ms(2_000);
        // A new key triggers the sweep, with a fast rate in hand.
        l.check("fast", "new", fast).unwrap();
        // The slow bucket was kept: its limit has not been reset.
        assert!(l.check("slow", "k", slow).is_err());
        // The idle fast buckets were swept.
        assert!(l.len() < GC_THRESHOLD / 2, "len = {}", l.len());
    }

    #[test]
    fn table_is_bounded_even_when_nothing_is_idle() {
        let clock = ManualClock::new(0);
        let l = Limiter::new(Arc::new(clock));
        let slow = Rate {
            per_minute: 1.0,
            burst: 1,
        };
        for i in 0..(MAX_BUCKETS + 5_000) {
            let _ = l.check("t", &i.to_string(), slow);
        }
        assert!(l.len() <= MAX_BUCKETS, "len = {}", l.len());
    }

    #[test]
    fn eviction_spares_the_global_counters() {
        let clock = ManualClock::new(0);
        let l = Limiter::new(Arc::new(clock.clone()));
        let slow = Rate {
            per_minute: 0.0,
            burst: 1,
        };
        // A global counter, spent, and the least recently used of the table.
        l.check("admin-web-all", "*", slow).unwrap();
        assert!(l.check("admin-web-all", "*", slow).is_err());
        clock.advance_ms(1);
        for i in 0..(MAX_BUCKETS + 5_000) {
            let _ = l.check("t", &i.to_string(), slow);
        }
        assert!(l.len() <= MAX_BUCKETS, "len = {}", l.len());
        assert!(
            l.check("admin-web-all", "*", slow).is_err(),
            "the shared budget was not reset by an eviction"
        );
    }
}
