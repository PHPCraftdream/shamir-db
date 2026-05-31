//! Argon2id concurrency limit per spec §8 + §8.1 NORMATIVE.
//!
//! Argon2id allocates ~128 MB of RAM per concurrent invocation by default.
//! Without a cap an attacker can OOM the server with a few hundred parallel
//! `auth_init` requests. Spec §8 mandates `MAX_CONCURRENT_ARGON2 = 64`
//! permits; spec §8.1 clarifies the permit is held ONLY for the duration of
//! the actual KDF call (NOT the surrounding state machine — pre-state
//! occupies a 1KB slot, not a permit).
//!
//! This module provides a counting semaphore using `std::sync::atomic`
//! (no async runtime dependency — it works for both sync and async
//! Argon2 callers via a blocking `wait()` or non-blocking `try_acquire()`).
//!
//! Production servers pair this with [`crate::server::rate_limit`] (which
//! caps `auth_init` arrival rate) so the queue depth in front of the
//! Argon2 semaphore stays bounded.

use crate::common::time::{ns, UnixNanos};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;

/// Spec §8 table: 64 concurrent Argon2id permits.
pub const MAX_CONCURRENT_ARGON2: u32 = 64;

/// Counting semaphore for Argon2id permits.
///
/// Threading model: blocking — [`Argon2Semaphore::acquire`] blocks the
/// calling thread until a permit is available or the deadline expires.
/// Returns a [`Argon2Permit`] RAII guard that releases the permit on drop.
pub struct Argon2Semaphore {
    /// Available permits (signed because we use fetch_sub then check).
    available: AtomicI64,
    /// Mutex+Condvar pair for blocking until a permit becomes available.
    notify: (Mutex<()>, Condvar),
    /// Capacity (for metrics).
    capacity: u32,
}

impl Argon2Semaphore {
    /// Create with default capacity `MAX_CONCURRENT_ARGON2 = 64`.
    pub fn default_capacity() -> Self {
        Self::with_capacity(MAX_CONCURRENT_ARGON2)
    }

    /// Create with custom capacity (for tests / tuning).
    pub fn with_capacity(capacity: u32) -> Self {
        Self {
            available: AtomicI64::new(capacity as i64),
            notify: (Mutex::new(()), Condvar::new()),
            capacity,
        }
    }

    /// Capacity (= total permits).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Currently-available permits (non-blocking peek).
    pub fn available(&self) -> i64 {
        self.available.load(Ordering::Relaxed)
    }

    /// Try to acquire a permit without blocking.
    /// Returns `Some(Permit)` if a permit was available, `None` otherwise.
    pub fn try_acquire(&self) -> Option<Argon2Permit<'_>> {
        // Optimistically decrement.
        let prev = self.available.fetch_sub(1, Ordering::Acquire);
        if prev > 0 {
            Some(Argon2Permit { sem: self })
        } else {
            // Restore — we didn't actually take a permit.
            self.available.fetch_add(1, Ordering::Release);
            None
        }
    }

    /// Block until a permit becomes available OR the deadline expires.
    /// Returns `Some(Permit)` on success, `None` on timeout.
    ///
    /// Use `deadline_ns = u64::MAX` for indefinite wait.
    pub fn acquire_until(&self, deadline_ns: u64) -> Option<Argon2Permit<'_>> {
        if let Some(p) = self.try_acquire() {
            return Some(p);
        }

        let (lock, cvar) = &self.notify;
        let mut guard = lock.lock().ok()?;
        loop {
            if let Some(p) = self.try_acquire() {
                return Some(p);
            }
            let now_ns = UnixNanos::now().as_u64();
            if now_ns >= deadline_ns {
                return None;
            }
            let remaining_ns = deadline_ns - now_ns;
            let wait_dur = Duration::from_nanos(remaining_ns.min(ns::SECOND));
            let (g, _) = cvar.wait_timeout(guard, wait_dur).ok()?;
            guard = g;
        }
    }

    /// Block forever until a permit becomes available.
    pub fn acquire(&self) -> Argon2Permit<'_> {
        self.acquire_until(u64::MAX)
            .expect("indefinite acquire returns Some")
    }

    /// Release one permit (called automatically by `Argon2Permit::drop`).
    fn release(&self) {
        self.available.fetch_add(1, Ordering::Release);
        let (_lock, cvar) = &self.notify;
        cvar.notify_one();
    }
}

impl core::fmt::Debug for Argon2Semaphore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Argon2Semaphore")
            .field("capacity", &self.capacity)
            .field("available", &self.available.load(Ordering::Relaxed))
            .finish()
    }
}

/// RAII permit — release on drop. Hold across the Argon2id call only.
#[must_use = "permit is released as soon as it's dropped"]
pub struct Argon2Permit<'a> {
    sem: &'a Argon2Semaphore,
}

impl<'a> Drop for Argon2Permit<'a> {
    fn drop(&mut self) {
        self.sem.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn try_acquire_succeeds_when_capacity_available() {
        let s = Argon2Semaphore::with_capacity(3);
        let _p1 = s.try_acquire().unwrap();
        let _p2 = s.try_acquire().unwrap();
        let _p3 = s.try_acquire().unwrap();
        assert!(s.try_acquire().is_none(), "4th must fail");
    }

    #[test]
    fn permit_drop_releases_back() {
        let s = Argon2Semaphore::with_capacity(1);
        {
            let _p = s.try_acquire().unwrap();
            assert!(s.try_acquire().is_none());
        }
        // After drop the permit should be available again.
        assert!(s.try_acquire().is_some());
    }

    #[test]
    fn acquire_blocks_until_permit_available() {
        let s = Arc::new(Argon2Semaphore::with_capacity(1));
        let p1 = s.try_acquire().unwrap();

        let s2 = s.clone();
        let handle = thread::spawn(move || {
            let _p = s2.acquire(); // blocks until p1 dropped
            42
        });

        // Give the thread a moment to start blocking.
        thread::sleep(Duration::from_millis(50));
        drop(p1); // release → wakes the waiter

        assert_eq!(handle.join().unwrap(), 42);
    }

    #[test]
    fn acquire_until_returns_none_on_timeout() {
        let s = Argon2Semaphore::with_capacity(0);
        let now = UnixNanos::now().as_u64();
        let deadline = now + 50 * 1_000_000; // 50ms in ns
        let r = s.acquire_until(deadline);
        assert!(r.is_none(), "must time out");
    }

    #[test]
    fn capacity_and_available_metrics() {
        let s = Argon2Semaphore::with_capacity(10);
        assert_eq!(s.capacity(), 10);
        assert_eq!(s.available(), 10);
        let _p = s.try_acquire().unwrap();
        assert_eq!(s.available(), 9);
    }

    #[test]
    fn many_concurrent_threads_respect_cap() {
        let s = Arc::new(Argon2Semaphore::with_capacity(4));

        // Count permits ACTUALLY held — NOT `capacity - available()`. The
        // latter is corrupted by the optimistic `fetch_sub`-then-restore in
        // `try_acquire`: under contention several threads decrement
        // `available` past zero before the losers restore it, so `available`
        // dips transiently NEGATIVE and `capacity - available()` spuriously
        // reports more than `capacity` "in use" — a flaky false failure, not
        // a real cap violation. Instead increment a dedicated counter only
        // AFTER a successful `acquire` and decrement it BEFORE releasing the
        // permit: that interval lies strictly inside [acquire, release], and
        // the semaphore guarantees at most `capacity` permits are held at
        // once, so the counter never exceeds the cap — deterministically.
        let held = Arc::new(AtomicI64::new(0));
        let max_observed = Arc::new(AtomicI64::new(0));
        let handles: Vec<_> = (0..20)
            .map(|_| {
                let s = s.clone();
                let held = held.clone();
                let max = max_observed.clone();
                thread::spawn(move || {
                    let _p = s.acquire();
                    let now_held = held.fetch_add(1, Ordering::AcqRel) + 1;
                    max.fetch_max(now_held, Ordering::AcqRel);
                    thread::sleep(Duration::from_millis(10));
                    // Decrement while still holding the permit so the counted
                    // interval stays within the permit's lifetime.
                    held.fetch_sub(1, Ordering::AcqRel);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(
            max_observed.load(Ordering::Acquire) <= 4,
            "at most `capacity` permits may be held at once; observed {}",
            max_observed.load(Ordering::Acquire)
        );
    }
}
