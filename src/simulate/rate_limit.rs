//! Hand-rolled token bucket. One bucket per rule, shared across reloads
//! when the (rps, burst) parameters are unchanged so token debt survives a
//! config edit. A `Mutex` is fine here: contention is per-route and the
//! critical section is a few arithmetic ops.

use std::sync::Mutex;
use std::time::Instant;

pub struct TokenBucket {
    state: Mutex<BucketState>,
    capacity: f64,
    refill_per_sec: f64,
    rps: f64,
    burst: u32,
}

struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(rps: f64, burst: u32) -> Self {
        Self {
            state: Mutex::new(BucketState {
                tokens: f64::from(burst),
                last_refill: Instant::now(),
            }),
            capacity: f64::from(burst),
            refill_per_sec: rps,
            rps,
            burst,
        }
    }

    /// Consume one token if available.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_at(Instant::now())
    }

    fn try_acquire_at(&self, now: Instant) -> bool {
        let Ok(mut state) = self.state.lock() else {
            // A poisoned mutex only happens if a holder panicked; failing
            // open (allow the request) is the safer behavior for a mock.
            return true;
        };
        let elapsed = now.saturating_duration_since(state.last_refill);
        state.tokens =
            (state.tokens + elapsed.as_secs_f64() * self.refill_per_sec).min(self.capacity);
        state.last_refill = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Refill to full capacity (admin reset).
    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.tokens = self.capacity;
            state.last_refill = Instant::now();
        }
    }

    pub fn params(&self) -> (f64, u32) {
        (self.rps, self.burst)
    }

    /// Suggested Retry-After in whole seconds (at least 1).
    pub fn retry_after_secs(&self) -> u64 {
        (1.0 / self.refill_per_sec).ceil().max(1.0) as u64
    }

    pub fn available(&self) -> f64 {
        self.state.lock().map_or(0.0, |s| s.tokens)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::time::Duration;

    #[test]
    fn burst_then_empty() {
        let b = TokenBucket::new(10.0, 3);
        assert!(b.try_acquire());
        assert!(b.try_acquire());
        assert!(b.try_acquire());
        assert!(!b.try_acquire());
    }

    #[test]
    fn refills_over_time() {
        let b = TokenBucket::new(10.0, 1);
        let start = Instant::now();
        assert!(b.try_acquire_at(start));
        assert!(!b.try_acquire_at(start));
        // 10 rps → one token back after 100ms
        assert!(b.try_acquire_at(start + Duration::from_millis(150)));
    }

    #[test]
    fn refill_caps_at_capacity() {
        let b = TokenBucket::new(1000.0, 2);
        let start = Instant::now();
        assert!(b.try_acquire_at(start));
        let later = start + Duration::from_secs(60);
        assert!(b.try_acquire_at(later));
        assert!(b.try_acquire_at(later));
        assert!(!b.try_acquire_at(later)); // capped at burst=2
    }

    #[test]
    fn reset_refills() {
        let b = TokenBucket::new(0.1, 1);
        assert!(b.try_acquire());
        assert!(!b.try_acquire());
        b.reset();
        assert!(b.try_acquire());
    }

    #[test]
    fn concurrent_grants_do_not_exceed_capacity() {
        let b = std::sync::Arc::new(TokenBucket::new(0.000001, 50));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = b.clone();
            handles.push(std::thread::spawn(move || {
                (0..100).filter(|_| b.try_acquire()).count()
            }));
        }
        let granted: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(granted <= 50);
    }

    #[test]
    fn retry_after_floor_is_one() {
        assert_eq!(TokenBucket::new(100.0, 1).retry_after_secs(), 1);
        assert_eq!(TokenBucket::new(0.5, 1).retry_after_secs(), 2);
    }
}
