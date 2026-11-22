use crate::common::error::EnvironmentError;
use crate::common::time::UnixNanos;
use crate::server::argon2_semaphore::{Argon2PermitBudget, Argon2Semaphore};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, TryLockError};
use std::thread;
use std::time::Duration;

/// Generous hang detector for thread handshakes; never the only
/// synchronisation.
const WATCHDOG: Duration = Duration::from_secs(120);

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
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let held_permit = semaphore.try_acquire().expect("hold the only permit");
    let (parked_tx, parked_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let notified = Arc::new(AtomicBool::new(false));

    let waiter = {
        let semaphore = Arc::clone(&semaphore);
        let notified = Arc::clone(&notified);
        let mut parked_tx = Some(parked_tx);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            let acquired = semaphore
                .acquire_until_with_test_sources(
                    Argon2PermitBudget::at_admission(10_000_000_100, 100),
                    || Ok(UnixNanos(100)),
                    || Duration::ZERO,
                    || semaphore.try_acquire(),
                    move |guard, _lock, cvar, wait_dur| {
                        // Signalled while the waiter mutex is still held, so
                        // the release below has to wait for this waiter to
                        // register on the condvar before it can notify.
                        if let Some(tx) = parked_tx.take() {
                            tx.send(()).expect("test observes the wait boundary");
                        }
                        let (guard, outcome) =
                            cvar.wait_timeout(guard, wait_dur).map_err(|_| ())?;
                        if outcome.timed_out() {
                            // Nothing notified this waiter: abort instead of
                            // sleeping through another slice, so a lost
                            // wake-up fails fast.
                            return Err(());
                        }
                        notified.store(true, Ordering::Relaxed);
                        Ok(guard)
                    },
                )
                .is_some();
            done_tx
                .send(acquired)
                .expect("test observes the acquisition result");
        })
    };

    parked_rx
        .recv_timeout(WATCHDOG)
        .expect("waiter must reach the failed-check/wait boundary");
    // The waiter is now inside `wait_timeout` (it released the waiter mutex
    // only to register), so this release cannot miss it.
    drop(held_permit);

    let acquired = done_rx
        .recv_timeout(WATCHDOG)
        .expect("released permit must be acquired");
    waiter.join().expect("waiter thread finishes");
    assert!(acquired, "released permit must be acquired");
    assert!(
        notified.load(Ordering::Relaxed),
        "the release must notify the parked waiter, not leave it to a slice timeout"
    );
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
    let now_ns = Cell::new(100);
    let mut held_permit = Some(held_permit);
    let attempts = Cell::new(0);

    let result = semaphore.acquire_until_with_test_sources(
        Argon2PermitBudget::at_admission(101, 100),
        || Ok(UnixNanos(now_ns.get())),
        || Duration::ZERO,
        || {
            attempts.set(attempts.get() + 1);
            semaphore.try_acquire()
        },
        |guard, lock, _cvar, _wait_dur| {
            drop(guard);
            now_ns.set(101);
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
        Argon2PermitBudget::at_admission(2_500_000_100, 100),
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

/// A backward wall-clock step while the job is queued must not restore budget:
/// the monotonic remainder is measured from the admission instant, which is
/// already spent by the time the job reaches the semaphore.
#[test]
fn admitted_waiter_does_not_regain_budget_from_a_backward_wall_clock_step() {
    let semaphore = Argon2Semaphore::with_capacity(1);
    let attempts = Cell::new(0);

    let result = semaphore.acquire_until_with_test_sources(
        // Three-second budget fixed at admission (10s of wall time).
        Argon2PermitBudget::at_admission(13_000_000_000, 10_000_000_000),
        // The wall clock stepped BACK four seconds while the job was queued.
        || Ok(UnixNanos(6_000_000_000)),
        // Four seconds really passed since admission: the budget is spent.
        || Duration::from_secs(4),
        || {
            attempts.set(attempts.get() + 1);
            semaphore.try_acquire()
        },
        |_guard, _lock, _cvar, _wait_dur| panic!("a spent budget must not wait"),
    );

    assert!(
        result.is_none(),
        "spent budget must refuse even a free permit"
    );
    assert_eq!(
        attempts.get(),
        0,
        "an expired budget must not attempt an acquisition"
    );
    assert_eq!(semaphore.available(), 1, "no permit may be consumed");
}

/// A waiter woken by a release that finds its own budget spent must pass the
/// wake-up on: the waiter parked behind it has to be woken by that handoff,
/// not by its own slice timeout.
#[test]
fn expired_notified_waiter_hands_the_wake_up_on() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(2));
    let _first_held = semaphore.try_acquire().expect("hold the first permit");
    let second_held = semaphore.try_acquire().expect("hold the second permit");
    let (parked_tx, parked_rx) = mpsc::channel();
    let (woken_tx, woken_rx) = mpsc::channel();
    let (hold_tx, hold_rx) = mpsc::channel();
    let notified = Arc::new(AtomicBool::new(false));

    let waiter = {
        let semaphore = Arc::clone(&semaphore);
        let notified = Arc::clone(&notified);
        let mut parked_tx = Some(parked_tx);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            let permit = semaphore.acquire_until_with_test_sources(
                Argon2PermitBudget::at_admission(60_000_000_000, 0),
                || Ok(UnixNanos(0)),
                || Duration::ZERO,
                || semaphore.try_acquire(),
                move |guard, _lock, cvar, _wait_dur| {
                    if let Some(tx) = parked_tx.take() {
                        tx.send(()).expect("test observes the wait boundary");
                    }
                    // A long slice: only a notification can end this wait in
                    // time, so a missing handoff surfaces as a timeout.
                    let (guard, outcome) = cvar
                        .wait_timeout(guard, Duration::from_secs(10))
                        .map_err(|_| ())?;
                    if outcome.timed_out() {
                        return Err(());
                    }
                    notified.store(true, Ordering::Relaxed);
                    Ok(guard)
                },
            );
            let acquired = permit.is_some();
            woken_tx
                .send(acquired)
                .expect("test observes the acquisition result");
            hold_rx
                .recv_timeout(WATCHDOG)
                .expect("test inspects accounting before release");
            drop(permit);
        })
    };

    parked_rx
        .recv_timeout(WATCHDOG)
        .expect("waiter reaches its wait boundary");

    // The expiring waiter: admitted with a three-second budget and woken by a
    // release whose single notification it consumed. It regains the waiter
    // mutex after that release, past its own deadline, so it must hand the
    // wake-up on instead of swallowing it. Locking the waiter mutex also
    // proves the parked waiter has released it (i.e. is registered).
    let elapsed_ns = Cell::new(0_u64);
    let expired = semaphore.acquire_until_with_test_sources(
        Argon2PermitBudget::at_admission(3_000_000_000, 0),
        || Ok(UnixNanos(0)),
        || Duration::from_nanos(elapsed_ns.get()),
        || semaphore.try_acquire(),
        |guard, lock, _cvar, _wait_dur| {
            drop(guard);
            // Release one permit WITHOUT notifying: this models the release
            // whose single notification this waiter already consumed. Dropping
            // a real permit here would notify and hide a missing handoff.
            semaphore.release_with_test_notify(|_lock, _cvar| {});
            elapsed_ns.set(3_000_000_000);
            lock.lock().map_err(|_| ())
        },
    );
    assert!(
        expired.is_none(),
        "a waiter that regains the mutex past its deadline takes no permit"
    );

    let acquired = woken_rx
        .recv_timeout(WATCHDOG)
        .expect("the parked waiter must be woken by the handoff");
    assert!(
        acquired,
        "the handoff must let the parked waiter take the free permit"
    );
    assert!(
        notified.load(Ordering::Relaxed),
        "the parked waiter must be notified, not timed out"
    );
    assert_eq!(
        semaphore.available(),
        0,
        "exactly one released permit is accounted: the parked waiter holds it"
    );

    hold_tx.send(()).expect("waiter still waiting");
    waiter.join().expect("waiter thread finishes");
    assert_eq!(
        semaphore.available(),
        1,
        "the parked waiter released its permit again"
    );
    drop(second_held);
    drop(_first_held);
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
