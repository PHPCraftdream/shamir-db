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

// Tests live in crate::server::tests::argon2_semaphore_tests.
