use crate::common::error::EnvironmentError;
use crate::common::time::UnixNanos;
use crate::server::argon2_semaphore::Argon2Semaphore;
use std::cell::Cell;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, TryLockError};
use std::thread;
use std::time::Duration;

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
fn release_after_failed_check_wakes_waiter() {
    let semaphore = Argon2Semaphore::with_capacity(1);
    let held_permit = semaphore.try_acquire().unwrap();
    let (waiter_ready_tx, waiter_ready_rx) = mpsc::channel();
    let (release_attempt_tx, release_attempt_rx) = mpsc::channel();

    let semaphore_ref = &semaphore;
    thread::scope(|scope| {
        let mut waiter_ready_tx = Some(waiter_ready_tx);
        let mut release_attempt_rx = Some(release_attempt_rx);
        let waiter = scope.spawn(move || {
            let semaphore = semaphore_ref;
            semaphore
                .acquire_until_with_test_sources(
                    10_000_000_100,
                    || Ok(UnixNanos(100)),
                    || Duration::ZERO,
                    || semaphore.try_acquire(),
                    move |guard, _lock, cvar, wait_dur| {
                        if let (Some(ready_tx), Some(release_rx)) =
                            (waiter_ready_tx.take(), release_attempt_rx.take())
                        {
                            ready_tx.send(()).unwrap();
                            release_rx
                                .recv_timeout(Duration::from_secs(120))
                                .expect("releaser must reach the wait boundary");
                        }
                        cvar.wait_timeout(guard, wait_dur)
                            .map(|(guard, _)| guard)
                            .map_err(|_| ())
                    },
                )
                .is_some()
        });
        waiter_ready_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("waiter must reach the failed-check/wait boundary");

        let releaser = scope.spawn(move || {
            release_attempt_tx.send(()).unwrap();
            drop(held_permit);
        });
        assert!(waiter.join().unwrap(), "released permit must be acquired");
        releaser.join().unwrap();
    });

    assert_eq!(semaphore.available(), 1);
}

#[test]
fn release_updates_availability_and_notifies_under_waiters_mutex() {
    let semaphore = Argon2Semaphore::with_capacity(0);
    semaphore.release_with_test_notify(|lock, _cvar| {
        assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    });
    assert_eq!(semaphore.available(), 1);
}

#[test]
fn acquire_until_rejects_preexpired_deadline_even_when_free() {
    let semaphore = Argon2Semaphore::with_capacity(1);
    let result = semaphore.acquire_until_with_clock(100, || Ok(UnixNanos(100)));
    assert!(
        result.is_none(),
        "expired deadline must reject a free permit"
    );
    assert_eq!(
        semaphore.available(),
        1,
        "expired check must not consume permit"
    );
}

#[test]
fn acquire_until_rechecks_deadline_after_release_before_retry() {
    let semaphore = Argon2Semaphore::with_capacity(1);
    let held_permit = semaphore.try_acquire().unwrap();
    let deadline_ns = Cell::new(100);
    let mut held_permit = Some(held_permit);
    let attempts = Cell::new(0);

    let result = semaphore.acquire_until_with_test_sources(
        101,
        || Ok(UnixNanos(deadline_ns.get())),
        || Duration::ZERO,
        || {
            attempts.set(attempts.get() + 1);
            semaphore.try_acquire()
        },
        |guard, lock, _cvar, _wait_dur| {
            drop(guard);
            deadline_ns.set(101);
            drop(held_permit.take().expect("held permit is released once"));
            lock.lock().map_err(|_| ())
        },
    );
    assert!(
        result.is_none(),
        "deadline reached during release must time out"
    );
    assert_eq!(attempts.get(), 1, "expiry must be checked before retry");
    assert_eq!(semaphore.available(), 1, "late retry leaves permit free");
}

#[test]
fn acquire_until_bounds_wait_by_monotonic_remaining_when_wall_clock_moves_back() {
    let semaphore = Argon2Semaphore::with_capacity(0);
    let wall_reads = AtomicUsize::new(0);
    let elapsed_ns = Cell::new(0_u64);
    let mut waits = Vec::new();

    let result = semaphore.acquire_until_with_test_sources(
        2_500_000_100,
        || {
            if wall_reads.fetch_add(1, Ordering::Relaxed) < 2 {
                Ok(UnixNanos(100))
            } else {
                Ok(UnixNanos(0))
            }
        },
        || Duration::from_nanos(elapsed_ns.get()),
        || semaphore.try_acquire(),
        |guard, _lock, _cvar, wait_dur| {
            assert!(waits.len() < 2, "backward clock must not add a wait slice");
            waits.push(wait_dur);
            let elapsed = if waits.len() == 1 {
                1_750_000_000
            } else {
                2_500_000_000
            };
            elapsed_ns.set(elapsed);
            Ok(guard)
        },
    );

    assert!(result.is_none(), "monotonic budget must eventually expire");
    assert_eq!(
        waits,
        [Duration::from_secs(1), Duration::from_millis(750)],
        "backward wall-clock movement must not restore elapsed budget"
    );
}

#[test]
fn acquire_until_returns_none_on_timeout() {
    let semaphore = Argon2Semaphore::with_capacity(0);
    let now_ns = UnixNanos::now().as_u64();
    let deadline_ns = now_ns + 50_000_000;
    let result = semaphore.acquire_until(deadline_ns);
    assert!(result.is_none(), "must time out");
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

/// RV-CONNECT-387EAA05 (P2-10 second tranche): a failing clock must make
/// `acquire_until` return `None` without panicking — fail closed, exactly
/// like a timeout, so callers report `server_busy` instead of crashing.
/// The pre-fix code called `UnixNanos::now()` (panicking wrapper) and the
/// test panicked.
#[test]
fn rv_connect_387eaa05_acquire_until_failing_clock_returns_none_without_panic() {
    let s = Argon2Semaphore::with_capacity(0);
    let result = s.acquire_until_with_clock(u64::MAX, || Err(EnvironmentError::ClockBeforeEpoch));
    assert!(result.is_none(), "failing clock must fail closed to None");
    // The permit accounting must be untouched by the failed acquisition.
    assert_eq!(s.available(), 0);
}
