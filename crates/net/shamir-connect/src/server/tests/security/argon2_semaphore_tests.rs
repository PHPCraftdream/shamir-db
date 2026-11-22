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

/// Bounded wait for a release that is attempted while the waiter is held in
/// its check-to-sleep gap. It is a sequencer, not an oracle: the assertion
/// reads the wake reason.
const RELEASE_GRACE: Duration = Duration::from_millis(500);

/// Join a worker without an unbounded wait: a reaper thread owns the handle
/// and the test only waits, bounded, for its report.
fn join_bounded(handle: thread::JoinHandle<()>) {
    let (finished_tx, finished_rx) = mpsc::channel();
    let reaper = thread::spawn(move || {
        let _ = handle.join();
        let _ = finished_tx.send(());
    });
    finished_rx
        .recv_timeout(WATCHDOG)
        .expect("worker thread finishes");
    drop(reaper);
}

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

/// A release racing the waiter's check-to-sleep gap must not lose its
/// notification: the waiter has to be NOTIFIED, not woken by its slice
/// timeout.
///
/// The interleaving is forced, not hoped for. The waiter holds the waiter
/// mutex across the gap (exactly as production does: it re-enters `wait`
/// with the guard after a failed attempt) and parks there on a handshake, so
/// the release is attempted strictly between the failed attempt and the
/// registration inside `wait_timeout`. The mutex-serialized release cannot
/// proceed there — it needs the waiter mutex — so its increment and its
/// notification happen only after the waiter registered (registration is
/// atomic with releasing the mutex). A release that incremented and notified
/// without the mutex instead completes inside the gap and its notification
/// reaches nobody, so the waiter could only time out.
#[test]
fn release_after_failed_check_wakes_waiter() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let (holding_tx, holding_rx) = mpsc::channel();
    let (drop_tx, drop_rx) = mpsc::channel();
    let (released_tx, released_rx) = mpsc::channel();
    let (parked_tx, parked_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let notified = Arc::new(AtomicBool::new(false));

    // The releaser owns the only permit until the waiter is parked in the gap.
    let releaser = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            let permit = semaphore.try_acquire().expect("hold the only permit");
            holding_tx.send(()).expect("test observes the held permit");
            drop_rx
                .recv_timeout(WATCHDOG)
                .expect("test drops the permit");
            drop(permit);
            released_tx
                .send(())
                .expect("test observes the completed release");
        })
    };
    holding_rx
        .recv_timeout(WATCHDOG)
        .expect("the only permit is held by the releaser");

    let waiter = {
        let semaphore = Arc::clone(&semaphore);
        let notified = Arc::clone(&notified);
        let mut parked_tx = Some(parked_tx);
        let mut go_rx = Some(go_rx);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            let acquired = semaphore
                .acquire_until_with_test_sources(
                    Argon2PermitBudget::at_admission(10_000_000_100, 100),
                    || Ok(UnixNanos(100)),
                    || Duration::ZERO,
                    || semaphore.try_acquire(),
                    move |guard, _lock, cvar, _wait_dur| {
                        // The gap: the attempt above failed and nothing is
                        // registered yet. The guard keeps the waiter mutex
                        // held, so the release stays stuck until this closure
                        // calls `wait_timeout`.
                        if let Some(tx) = parked_tx.take() {
                            tx.send(()).expect("test observes the gap");
                        }
                        if let Some(rx) = go_rx.take() {
                            rx.recv_timeout(WATCHDOG).expect("test closes the gap");
                        }
                        // A long slice: only a notification ends this wait in
                        // time, so a lost wake-up surfaces as a timeout.
                        let (guard, outcome) = cvar
                            .wait_timeout(guard, Duration::from_secs(10))
                            .map_err(|_| ())?;
                        if outcome.timed_out() {
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
        .expect("waiter must reach the failed-check gap");
    drop_tx.send(()).expect("releaser still waiting");
    // Sequencing only: a correct release cannot complete while the gap is
    // open, so this receive is expected to time out and the wake reason
    // below stays the oracle. A release that skipped the waiter mutex would
    // complete here instead — before the waiter registered — and lose its
    // notification.
    let completed_early = released_rx.recv_timeout(RELEASE_GRACE).is_ok();
    go_tx.send(()).expect("waiter still in the gap");

    let acquired = done_rx
        .recv_timeout(WATCHDOG)
        .expect("released permit must be acquired");
    join_bounded(waiter);
    join_bounded(releaser);
    assert!(
        !completed_early,
        "the release must serialize with the waiter mutex, not notify inside the gap"
    );
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

/// A live wait slice must stay bounded by the MONOTONIC remainder when the
/// wall clock steps back by more than that remainder: a backward step can
/// lengthen the wall-limited remainder relative to the previous sample, never
/// the budget fixed at admission.
#[test]
fn acquire_until_bounds_wait_by_monotonic_remaining_when_wall_clock_moves_back() {
    let semaphore = Argon2Semaphore::with_capacity(0);
    let wall_reads = AtomicUsize::new(0);
    let elapsed_ns = Cell::new(0_u64);
    let mut waits = Vec::new();

    let result = semaphore.acquire_until_with_test_sources(
        // 2.5 s of wall budget fixed at admission.
        Argon2PermitBudget::at_admission(2_500_000_100, 100),
        || {
            if wall_reads.fetch_add(1, Ordering::Relaxed) < 1 {
                Ok(UnixNanos(100))
            } else {
                // The wall clock stepped back past the admission reading, so
                // a wall-only remainder would grow back to a full 2.5 s.
                Ok(UnixNanos(0))
            }
        },
        || Duration::from_nanos(elapsed_ns.get()),
        || semaphore.try_acquire(),
        |guard, _lock, _cvar, wait_dur| {
            assert!(waits.len() < 2, "backward clock must not add a wait slice");
            waits.push(wait_dur);
            // 1.75 s spent at the first wake-up, so the monotonic remainder is
            // 750 ms even though the wall clock now claims 2.5 s.
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
        "a backward wall-clock step must not lengthen a live slice past the monotonic remainder"
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
    let first_held = semaphore.try_acquire().expect("hold the first permit");
    let second_held = semaphore.try_acquire().expect("hold the second permit");
    let second_held = Cell::new(Some(second_held));
    let (parked_tx, parked_rx) = mpsc::channel();
    let (woken_tx, woken_rx) = mpsc::channel();
    let (hold_tx, hold_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
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
            dropped_tx.send(()).expect("test observes the permit drop");
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
            // One permit is released WITHOUT notifying: this models the
            // release whose single notification this waiter already consumed.
            // Dropping that permit's guard normally would notify the parked
            // waiter directly and hide a missing handoff, so the guard is
            // forgotten instead of dropped: exactly one permit leaves the
            // accounting and the fixture still ends at capacity.
            std::mem::forget(
                second_held
                    .take()
                    .expect("the released permit is accounted once"),
            );
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

    // The join follows the permit drop, not a message sent before it.
    hold_tx.send(()).expect("waiter still waiting");
    dropped_rx
        .recv_timeout(WATCHDOG)
        .expect("the parked waiter drops its permit");
    join_bounded(waiter);
    assert_eq!(
        semaphore.available(),
        1,
        "the parked waiter released its permit again"
    );
    drop(first_held);
    assert_eq!(
        semaphore.available(),
        2,
        "capacity accounting ends exactly at capacity"
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

/// The pre-expired entry reading must be refused before the waiter mutex is
/// taken: while a probe keeps the waiter mutex locked, a pre-expired call
/// still returns at once and reads the clock exactly once.
#[test]
fn acquire_until_refuses_preexpired_entry_reading_without_the_waiter_mutex() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let (locked_tx, locked_rx) = mpsc::channel();
    let (unlock_tx, unlock_rx) = mpsc::channel();

    // `release_with_test_notify` runs with the waiter mutex held; parking in
    // its notify closure keeps that mutex locked for the whole probe.
    let holder = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            semaphore.release_with_test_notify(|_lock, _cvar| {
                locked_tx.send(()).expect("test observes the held mutex");
                unlock_rx
                    .recv_timeout(WATCHDOG)
                    .expect("test unlocks the mutex");
            });
        })
    };
    locked_rx
        .recv_timeout(WATCHDOG)
        .expect("the waiter mutex is held by the probe");

    let (done_tx, done_rx) = mpsc::channel();
    let probe = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let reads = Cell::new(0_usize);
            let result = semaphore.acquire_until_with_clock(100, || {
                reads.set(reads.get() + 1);
                Ok(UnixNanos(100))
            });
            done_tx
                .send((result.is_none(), reads.get()))
                .expect("test observes the pre-expired refusal");
        })
    };

    let (refused, reads) = done_rx
        .recv_timeout(WATCHDOG)
        .expect("a pre-expired entry reading must not block on the waiter mutex");
    assert!(refused, "a pre-expired entry reading must refuse");
    assert_eq!(
        reads, 1,
        "the refusal must skip the acquisition loop, so only the entry read happens"
    );
    // The probe above repaid one permit that no guard owns, so this
    // capacity-one semaphore holds two: neither the attempt nor the wait ran.
    assert_eq!(semaphore.available(), 2, "no permit may be consumed");

    unlock_tx.send(()).expect("probe still parked");
    join_bounded(probe);
    join_bounded(holder);
}

/// `u64::MAX` is the indefinite-wait sentinel, never an expired deadline.
#[test]
fn acquire_until_indefinite_deadline_is_not_expired() {
    let semaphore = Argon2Semaphore::with_capacity(1);
    let result = semaphore.acquire_until_with_clock(u64::MAX, || Ok(UnixNanos(u64::MAX)));
    assert!(
        result.is_some(),
        "an indefinite deadline must not be treated as expired"
    );
    assert_eq!(semaphore.available(), 0, "the free permit is taken");
}
