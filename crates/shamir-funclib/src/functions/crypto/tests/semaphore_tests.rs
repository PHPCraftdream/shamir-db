//! Regression test for audit card RV-WASM-HOST-D2732518:
//! `CountingSemaphore::release` must hold the notify mutex across
//! `notify_one`, so a waiter between its predicate check and condvar
//! registration cannot miss the wakeup.
//!
//! Deterministic by construction: the release runs synchronously on THIS
//! thread, and the property is observed on the actual release path — the
//! `release` implementation records, while the guard is still live, that a
//! `try_lock` on the notify mutex fails (WouldBlock). No second thread, no
//! sleeps, no scheduler-timeout oracles.
//!
//! Mutation sensitivity: reverting ONLY the production guard acquisition
//! in `release` (keeping the test observation) makes `try_lock` succeed —
//! the flag records `false` and this test fails.

use crate::functions::crypto::core::CountingSemaphore;

#[test]
fn rv_wasm_host_d2732518_release_records_notify_lock_still_held_after_wakeup() {
    let sem = CountingSemaphore::with_capacity(1);
    sem.test_acquire();
    assert_eq!(sem.test_available(), 0, "acquire must take the permit");

    // Synchronous release on this thread: observes the real release path.
    sem.test_release();

    assert_eq!(
        sem.test_available(),
        1,
        "release must return the permit to the pool"
    );
    assert!(
        sem.test_released_with_notify_lock_held(),
        "release must notify while STILL holding the notify mutex \
         (try_lock must fail immediately after notify_one)"
    );
}
