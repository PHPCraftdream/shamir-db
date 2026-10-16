//! Test-only commit-window park for the repository-wide atomic batch.
//!
//! Parks the `spawn_blocking` thread inside [`crate::storage_fjall`]'s
//! `exec_atomic_commit` — after the batch is fully staged and BEFORE the
//! physical `batch.commit()` — so engine-level cancellation tests can abort
//! the caller while the commit is in flight and watch the detached worker
//! complete it anyway.
//!
//! Why not `cfg(test)`: the seam is driven from the `shamir-engine` test
//! binary, which links `shamir-storage` WITHOUT `cfg(test)`. It is therefore
//! `pub` + `#[doc(hidden)]` and always compiled; when not armed the only
//! cost on the commit path is ONE unordered (`Relaxed`) atomic load.
//!
//! Contention model: unarmed — a single `Relaxed` load per atomic commit
//! (no lock, no allocation, effectively free next to a batch commit +
//! `persist(SyncAll)`). Armed (tests only) — the parking thread blocks on a
//! `std::sync::Condvar` while holding no storage locks, and the test thread
//! spins on an `Acquire` load between scheduler yields (deterministic,
//! no sleeps). Exactly one committer parks at a time in practice; the
//! `Mutex` guards only the condvar wait itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

pub(crate) static ARMED: AtomicBool = AtomicBool::new(false);
static REACHED: AtomicBool = AtomicBool::new(false);
static RELEASED: AtomicBool = AtomicBool::new(false);
static CONDVAR: Condvar = Condvar::new();
static WAIT_MUTEX: Mutex<()> = Mutex::new(());
/// Async test-side arrival signal (round 5): the parking thread fires
/// `notify_waiters()` right after setting `REACHED`, so engine tests can
/// await `wait_reached()` instead of spinning on yield budgets.
static NOTIFY: tokio::sync::Notify = tokio::sync::Notify::const_new();

fn lock() -> MutexGuard<'static, ()> {
    WAIT_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Arm (or disarm) the park. Disarming releases any parked committer and
/// clears the reached flag, leaving the seam inert.
pub fn arm(on: bool) {
    ARMED.store(on, Ordering::Release);
    if !on {
        release();
    }
}

/// `true` once a committer is parked inside the commit window.
pub fn reached() -> bool {
    REACHED.load(Ordering::Acquire)
}

/// Deterministic async wait for arrival (round 5): replaces the fixed
/// `for _ in 0..2000 { yield; }` seam-arrival loops. Arm-then-check
/// pattern (mirrors `ReceiptWorker::wait_idle`): the waker is registered
/// via `enable()` BEFORE re-checking `reached()`, so a notification fired
/// between the check and the registration cannot be missed; the parking
/// thread stores `REACHED` (Release) BEFORE `notify_waiters()`, so waking
/// implies `reached() == true`. No sleeps, no yield budgets.
pub async fn wait_reached() {
    // A fresh `Notified` per round: re-awaiting a completed one is Ready at
    // once and would spin if `REACHED` was already reset by a release.
    loop {
        let notified = NOTIFY.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if reached() {
            return;
        }
        notified.await;
    }
}

/// Release the parked committer; the physical commit proceeds.
pub fn release() {
    RELEASED.store(true, Ordering::Release);
    drop(lock()); // acquire+drop so the store above is visible to wait()
    CONDVAR.notify_all();
}

/// Blocking park — runs on the `spawn_blocking` thread, so a blocking
/// primitive (Condvar, NOT tokio Notify) is required. Sets `reached`,
/// then blocks until [`release`].
pub(crate) fn park() {
    REACHED.store(true, Ordering::Release);
    NOTIFY.notify_waiters();
    let mut guard = lock();
    while !RELEASED.load(Ordering::Acquire) {
        guard = CONDVAR
            .wait(guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    REACHED.store(false, Ordering::Release);
    RELEASED.store(false, Ordering::Release);
}
