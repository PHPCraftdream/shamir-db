//! Argon2id concurrency limit per spec §8 + §8.1 NORMATIVE.
//!
//! Argon2id allocates ~128 MB of RAM per concurrent invocation by default.
//! Without a cap an attacker can OOM the server with a few hundred parallel
//! derivations. Spec §8 sets `MAX_CONCURRENT_ARGON2 = 64` permits as the
//! default (the server's `argon2_concurrent_max` configures it); spec
//! §8.1 clarifies the permit is held ONLY for the duration of the actual KDF
//! call (NOT the surrounding state machine — pre-state occupies a 1KB slot,
//! not a permit).
//!
//! This module provides a counting semaphore using `std::sync::atomic`
//! (no async runtime dependency — it works for both sync and async
//! Argon2 callers via blocking `acquire()` / `acquire_until()` or non-blocking
//! `try_acquire()`). Async callers must run blocking acquisition in
//! `tokio::task::spawn_blocking`, not on an async executor worker.
//!
//! Production servers hold this semaphore behind the server-side SCRAM
//! derivation gate (`shamir-server-security`'s `ScramDerivationGate`) and take
//! its permits only for privileged account creation (`CreateScramUser` /
//! `UserAdminPort::create_user`). Ordinary authentication derives no
//! server-side password key and never reaches this semaphore: native SCRAM
//! `auth_init` + proof verification, session resume and the HTTP login proof
//! check all happen without a permit, and the inbound `auth_init` rate limit
//! bounds handshake attempts, not this queue. The queue in front of the
//! semaphore is bounded by that gate's admission cap, not by the rate limit.

use crate::common::error::EnvironmentError;
use crate::common::time::UnixNanos;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Spec §8 table: 64 concurrent Argon2id permits.
pub const MAX_CONCURRENT_ARGON2: u32 = 64;

/// Longest single blocking slice of a bounded permit wait.
const MAX_WAIT_SLICE: Duration = Duration::from_secs(1);

/// Permit budget fixed at admission, on both the wall clock and the monotonic
/// clock.
///
/// The gate that admits the job fixes this once, before the work reaches the
/// blocking pool, and passes it by value; there is no global state. Both
/// halves are measured **from admission**, not from semaphore entry, so time
/// spent queued behind other blocking work and a backward wall-clock step
/// before entry cannot restore budget that is already spent.
#[derive(Debug, Clone, Copy)]
pub struct Argon2PermitBudget {
    /// Absolute wall-clock deadline in Unix nanoseconds.
    deadline_ns: u64,
    /// Wall-clock reading taken at the same instant as `admitted_at`.
    admitted_ns: u64,
    /// Monotonic anchor captured at admission.
    admitted_at: Instant,
}

impl Argon2PermitBudget {
    /// Fix a budget at admission: `admitted_ns` is the wall-clock reading
    /// taken now and `deadline_ns` the absolute wall deadline derived from
    /// that same reading. `u64::MAX` stays the indefinite-wait sentinel.
    pub fn at_admission(deadline_ns: u64, admitted_ns: u64) -> Self {
        Self {
            deadline_ns,
            admitted_ns,
            admitted_at: Instant::now(),
        }
    }

    /// Fix a budget with an explicit monotonic anchor instead of
    /// `Instant::now()`.
    ///
    /// Test seam: the SCRAM gate's queue tests live in `shamir-server-security`
    /// and must hand the semaphore a budget whose monotonic remainder is
    /// already spent while its wall deadline is still in the future. Such a
    /// budget cannot be produced by [`Self::at_admission`], which anchors the
    /// monotonic clock at the call, and it cannot be waited for in the test
    /// without spending real seconds.
    ///
    /// Gated on `debug_assertions` rather than `cfg(test)` on purpose: a
    /// dependency is compiled WITHOUT `cfg(test)`, so a `cfg(test)` item here
    /// would be invisible to the tests of the crate that depends on this one.
    /// Release builds elide the item; production callers only ever see
    /// [`Self::at_admission`]. The profile split is therefore: the item exists
    /// in any build with `debug_assertions` on (all test builds) and is elided
    /// from release builds, so a dependent crate's tests must run in a
    /// debug-assertions profile — this crate has no `test-hooks` feature to
    /// forward, and one crate's `test-hooks` cannot enable an item in its
    /// dependency.
    #[cfg(any(test, debug_assertions))]
    pub fn at_admission_with_anchor(
        deadline_ns: u64,
        admitted_ns: u64,
        admitted_at: Instant,
    ) -> Self {
        Self {
            deadline_ns,
            admitted_ns,
            admitted_at,
        }
    }

    /// Wall budget fixed at admission (empty when admission was already past
    /// its deadline).
    fn wall_budget(&self) -> Duration {
        Duration::from_nanos(self.deadline_ns.saturating_sub(self.admitted_ns))
    }
}

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
    /// The budget is projected onto the monotonic clock at entry. A caller
    /// whose budget was fixed earlier — before the blocking pool queued its
    /// job — must pass it to [`Self::acquire_until_admitted`] instead, so the
    /// queue time is charged.
    ///
    /// Use `deadline_ns = u64::MAX` for indefinite wait.
    pub fn acquire_until(&self, deadline_ns: u64) -> Option<Argon2Permit<'_>> {
        self.acquire_until_with_clock(deadline_ns, UnixNanos::try_now)
    }

    /// Block until a permit becomes available OR `budget` is spent, with both
    /// the wall-clock deadline and the monotonic anchor taken from admission.
    ///
    /// The remaining budget is the SMALLER of the wall remainder and the
    /// monotonic remainder measured from the admission instant, so a backward
    /// wall-clock step cannot extend the wait beyond the monotonic budget
    /// (it can lengthen the wall-limited remainder relative to the previous
    /// sample, never the original budget).
    pub fn acquire_until_admitted(&self, budget: Argon2PermitBudget) -> Option<Argon2Permit<'_>> {
        self.acquire_with_budget_sources(
            budget,
            UnixNanos::try_now,
            move || budget.admitted_at.elapsed(),
            || self.try_acquire(),
            Self::default_wait,
        )
    }

    /// Clock-injected core of [`Self::acquire_until`] (private; tests inject
    /// a failing clock to prove the fail-closed path). The budget is projected
    /// onto the monotonic clock from the entry reading.
    pub(crate) fn acquire_until_with_clock(
        &self,
        deadline_ns: u64,
        now: impl Fn() -> Result<UnixNanos, EnvironmentError>,
    ) -> Option<Argon2Permit<'_>> {
        let entry = now().ok()?;
        if deadline_ns != u64::MAX && entry.as_u64() >= deadline_ns {
            // Already expired at entry: refuse without touching the waiter mutex.
            return None;
        }
        let budget = Argon2PermitBudget::at_admission(deadline_ns, entry.as_u64());
        self.acquire_with_budget_sources(
            budget,
            now,
            move || budget.admitted_at.elapsed(),
            || self.try_acquire(),
            Self::default_wait,
        )
    }

    #[cfg(test)]
    pub(crate) fn acquire_until_with_test_sources<'a, N, E, A, W>(
        &'a self,
        budget: Argon2PermitBudget,
        now: N,
        elapsed_since_admission: E,
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
        self.acquire_with_budget_sources(budget, now, elapsed_since_admission, attempt, wait)
    }

    fn default_wait<'a>(
        guard: MutexGuard<'a, ()>,
        _lock: &'a Mutex<()>,
        cvar: &'a Condvar,
        wait_dur: Duration,
    ) -> Result<MutexGuard<'a, ()>, ()> {
        cvar.wait_timeout(guard, wait_dur)
            .map(|(guard, _)| guard)
            .map_err(|_| ())
    }

    fn acquire_with_budget_sources<'a, N, E, A, W>(
        &'a self,
        budget: Argon2PermitBudget,
        mut now: N,
        mut elapsed_since_admission: E,
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
        let (lock, cvar) = &self.notify;
        let mut guard = lock.lock().ok()?;
        loop {
            // Both clocks are re-read before every attempt — the first
            // free-permit attempt and every wake-up included. Every exit that
            // does not take the permit passes the notification on (see
            // `handoff` and the permit drop) so a waiter that consumed the
            // wake-up and then expired cannot strand the next waiter next to a
            // free permit.
            let now_ns = match now() {
                Ok(time) => time.as_u64(),
                Err(_) => {
                    // Unreadable clock: fail closed and hand the wake-up on.
                    handoff(cvar);
                    return None;
                }
            };
            let Some(remaining) = remaining_budget(&budget, now_ns, elapsed_since_admission())
            else {
                // Budget spent before attempting: the caller's queue time, and
                // any backward wall-clock step, are charged on the monotonic
                // clock anchored at admission.
                handoff(cvar);
                return None;
            };

            // Serialize timed acquisitions with releases. The production timed
            // path holds the waiter mutex around the optimistic attempt.
            if let Some(permit) = attempt() {
                let post_acquire = now().ok().and_then(|time| {
                    remaining_budget(&budget, time.as_u64(), elapsed_since_admission())
                });
                if post_acquire.is_some() {
                    drop(guard);
                    return Some(permit);
                }
                // Acquired past the budget: dropping the permit increments and
                // notifies under the waiter mutex, which is this waiter's
                // handoff.
                drop(guard);
                drop(permit);
                return None;
            }

            let wait_dur = remaining.min(MAX_WAIT_SLICE);
            guard = match wait(guard, lock, cvar, wait_dur) {
                Ok(guard) => guard,
                // Poisoned waiter mutex: the loop cannot continue, so wake one
                // other waiter best-effort before leaving without a permit.
                // Notifying without the mutex cannot lose a wake-up that the
                // poison has not already lost.
                Err(_) => {
                    cvar.notify_one();
                    return None;
                }
            };
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
        // The permit increment and notification stay under the waiters mutex:
        // a waiter can therefore never be notified between its failed attempt
        // and its registration on the condvar.
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

/// Pass the wake-up on to the next waiter. The caller still holds the waiter
/// mutex, exactly like the release path, so the notification cannot be lost.
fn handoff(cvar: &Condvar) {
    cvar.notify_one();
}

/// Remaining budget, or `None` when it is spent. The monotonic remainder is
/// measured from the admission instant and wins whenever it is smaller, so a
/// backward wall-clock step cannot extend the wait beyond the monotonic budget.
fn remaining_budget(
    budget: &Argon2PermitBudget,
    now_ns: u64,
    elapsed_since_admission: Duration,
) -> Option<Duration> {
    if budget.deadline_ns == u64::MAX {
        return Some(Duration::MAX);
    }
    if now_ns >= budget.deadline_ns {
        return None;
    }
    let wall_budget = budget.wall_budget();
    if elapsed_since_admission >= wall_budget {
        return None;
    }
    Some(
        Duration::from_nanos(budget.deadline_ns - now_ns)
            .min(wall_budget - elapsed_since_admission),
    )
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
