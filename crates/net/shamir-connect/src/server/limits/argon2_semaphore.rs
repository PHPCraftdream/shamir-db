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
//! Argon2 callers via blocking `acquire()` / `acquire_until()` or non-blocking
//! `try_acquire()`). Async callers must run blocking acquisition in
//! `tokio::task::spawn_blocking`, not on an async executor worker.
//!
//! Production servers pair this with [`crate::server::rate_limit`] (which
//! caps `auth_init` arrival rate) so the queue depth in front of the
//! Argon2 semaphore stays bounded.

use crate::common::error::EnvironmentError;
use crate::common::time::UnixNanos;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
    /// Returns `Some(Permit)` on success, `None` on timeout — and also `None`
    /// if the system clock cannot be read (fail closed: a clock failure is
    /// indistinguishable from a timeout, so the caller reports `server_busy`;
    /// RV-CONNECT-387EAA05, P2-10: never fabricate a timestamp).
    ///
    /// Use `deadline_ns = u64::MAX` for indefinite wait.
    pub fn acquire_until(&self, deadline_ns: u64) -> Option<Argon2Permit<'_>> {
        self.acquire_until_with_clock(deadline_ns, UnixNanos::try_now)
    }

    /// Clock-injected core of [`Self::acquire_until`] (private; tests inject
    /// a failing clock to prove the fail-closed path).
    pub(crate) fn acquire_until_with_clock(
        &self,
        deadline_ns: u64,
        now: impl Fn() -> Result<UnixNanos, EnvironmentError>,
    ) -> Option<Argon2Permit<'_>> {
        let started = Instant::now();
        self.acquire_until_with_sources(
            deadline_ns,
            now,
            || started.elapsed(),
            || self.try_acquire(),
            |guard, _lock, cvar, wait_dur| {
                cvar.wait_timeout(guard, wait_dur)
                    .map(|(guard, _)| guard)
                    .map_err(|_| ())
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn acquire_until_with_test_sources<'a, N, E, A, W>(
        &'a self,
        deadline_ns: u64,
        now: N,
        elapsed: E,
        attempt: A,
        wait: W,
    ) -> Option<Argon2Permit<'a>>
    where
        N: FnMut() -> Result<UnixNanos, EnvironmentError>,
        E: FnMut() -> Duration,
        A: FnMut() -> Option<Argon2Permit<'a>>,
        W: FnMut(
            MutexGuard<'a, ()>,
            &'a Mutex<()>,
            &'a Condvar,
            Duration,
        ) -> Result<MutexGuard<'a, ()>, ()>,
    {
        self.acquire_until_with_sources(deadline_ns, now, elapsed, attempt, wait)
    }

    fn acquire_until_with_sources<'a, N, E, A, W>(
        &'a self,
        deadline_ns: u64,
        mut now: N,
        mut elapsed: E,
        mut attempt: A,
        mut wait: W,
    ) -> Option<Argon2Permit<'a>>
    where
        N: FnMut() -> Result<UnixNanos, EnvironmentError>,
        E: FnMut() -> Duration,
        A: FnMut() -> Option<Argon2Permit<'a>>,
        W: FnMut(
            MutexGuard<'a, ()>,
            &'a Mutex<()>,
            &'a Condvar,
            Duration,
        ) -> Result<MutexGuard<'a, ()>, ()>,
    {
        // Project the wall-clock budget at entry onto a monotonic elapsed time.
        // `u64::MAX` remains the existing explicit indefinite-wait sentinel.
        let monotonic_origin = elapsed();
        let entry_ns = now().ok()?.as_u64();
        let projected_budget = if deadline_ns == u64::MAX {
            Duration::MAX
        } else {
            if entry_ns >= deadline_ns {
                return None;
            }
            Duration::from_nanos(deadline_ns - entry_ns)
        };

        let (lock, cvar) = &self.notify;
        let mut guard = lock.lock().ok()?;
        loop {
            // Check wall and monotonic budgets before each attempt, including
            // the first free-permit attempt and every wake from the Condvar.
            let now_ns = now().ok()?.as_u64();
            let monotonic_elapsed = elapsed().saturating_sub(monotonic_origin);
            let remaining =
                remaining_budget(deadline_ns, projected_budget, monotonic_elapsed, now_ns)?;

            // Serialize timed acquisitions with releases. The production timed
            // path holds the waiter mutex around the optimistic attempt.
            if let Some(permit) = attempt() {
                let post_acquire = now().ok().and_then(|time| {
                    remaining_budget(
                        deadline_ns,
                        projected_budget,
                        elapsed().saturating_sub(monotonic_origin),
                        time.as_u64(),
                    )
                });
                if post_acquire.is_some() {
                    drop(guard);
                    return Some(permit);
                }
                drop(guard);
                drop(permit);
                return None;
            }

            let wait_dur = remaining.min(Duration::from_secs(1));
            guard = wait(guard, lock, cvar, wait_dur).ok()?;
        }
    }

    /// Block forever until a permit becomes available.
    pub fn acquire(&self) -> Argon2Permit<'_> {
        self.acquire_until(u64::MAX)
            .expect("indefinite acquire returns Some")
    }

    /// Release one permit (called automatically by `Argon2Permit::drop`).
    fn release(&self) {
        self.release_with_notify(|_, cvar| cvar.notify_one());
    }

    fn release_with_notify(&self, notify: impl FnOnce(&Mutex<()>, &Condvar)) {
        let (lock, cvar) = &self.notify;
        // The permit increment and notification stay under the waiters mutex.
        let _guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.available.fetch_add(1, Ordering::Release);
        notify(lock, cvar);
    }

    #[cfg(test)]
    pub(crate) fn release_with_test_notify(&self, notify: impl FnOnce(&Mutex<()>, &Condvar)) {
        self.release_with_notify(notify);
    }
}

fn remaining_budget(
    deadline_ns: u64,
    projected_budget: Duration,
    elapsed: Duration,
    now_ns: u64,
) -> Option<Duration> {
    if deadline_ns == u64::MAX {
        return Some(Duration::MAX);
    }
    if now_ns >= deadline_ns || elapsed >= projected_budget {
        return None;
    }
    let wall_remaining = Duration::from_nanos(deadline_ns - now_ns);
    Some(wall_remaining.min(projected_budget - elapsed))
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

// Tests live in crate::server::tests::security::argon2_semaphore_tests.
