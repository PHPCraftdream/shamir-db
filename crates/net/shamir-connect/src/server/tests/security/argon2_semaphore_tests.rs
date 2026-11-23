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

/// Bound on the in-gap release probe in
/// [`release_after_failed_check_wakes_waiter`]: long enough that a lock-free
/// release (which completes in microseconds) is observed inside the gap in
/// every run where the releaser is actually scheduled, far short enough to
/// keep the passing path well under the per-test kill.
///
/// This bound IS one of the two oracles. Its outcome is stored as
/// `completed_in_gap`: `Ok(())` means the release completed inside the
/// check-to-park gap and the test FAILS ("the release did not serialize with
/// the waiter mutex"); a timeout means the release serialized with the waiter
/// mutex, which is required but not sufficient. The timeout is also the
/// EXPECTED path of every green run — a correct releaser is blocked on the
/// waiter mutex the waiter holds — so every green run pays this 5 s.
///
/// The second oracle is the post-gap `acquired` assertion together with the
/// wake-reason flag: a timeout here never fails a correct run, but a correct
/// run still has to WAKE the parked waiter rather than leave it to its slice.
const GAP_PROBE: Duration = Duration::from_secs(5);

/// Bound on the test thread's handshake receives of
/// [`acquire_until_refuses_preexpired_entry_reading_without_the_waiter_mutex`],
/// whose red path is the probe blocking on the holder's waiter mutex: waiting
/// the full [`WATCHDOG`] there would let a red run burn 120 s before the
/// assertion could fire, so this fails it at 20 s instead, well inside the
/// 180 s default per-test kill. A green run's handshakes are immediate, so
/// this only shortens the failing path, and [`WATCHDOG`] stays the bound for
/// the joins.
const PROBE_BOUND: Duration = Duration::from_secs(20);

/// Outer hang detector for a worker parked until the test closes a handshake.
/// Strictly longer than [`WATCHDOG`], because the step that closes such a
/// handshake is itself a `WATCHDOG`-bounded receive: the worker must outlive
/// it. It is a bound, never a sequencer and never an oracle.
const GAP_HOLD: Duration = Duration::from_secs(WATCHDOG.as_secs() * 2);

/// Join a worker without an unbounded wait: a reaper thread owns the handle
/// and the test only waits, bounded, for its report.
///
/// The worker's panic is discarded here, so a panicking worker cannot fail
/// the test; tests that need that guarantee use [`join_checked`] instead.
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

/// Join a worker with the same bounded reaper wait as [`join_bounded`], but
/// carry the `join` result back and re-raise the worker's panic on the test
/// thread: a panicking worker (a poisoned mutex, a broken handshake) FAILS
/// the test instead of passing silently.
fn join_checked(handle: thread::JoinHandle<()>) {
    let (finished_tx, finished_rx) = mpsc::channel();
    let reaper = thread::spawn(move || {
        let _ = finished_tx.send(handle.join());
    });
    let result = finished_rx
        .recv_timeout(WATCHDOG)
        .expect("worker thread finishes");
    drop(reaper);
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
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
/// registration inside `wait_timeout`.
///
/// ORACLE: `release_with_notify` takes the waiter mutex, so while the gap is
/// open the release can neither increment nor notify, and the flag its
/// guarded notify closure raises (`notify_ran`) cannot arrive. The release is
/// therefore REQUESTED while the gap is open and `notify_ran` is awaited with
/// only the short [`GAP_PROBE`] as a bound: the passing run is the one where
/// that receive times out, so no grace duration decides the outcome
/// (a grace would be both a false-failure source for a correct but delayed
/// releaser and an escape hatch for a lock-free mutant), and because the pass
/// condition is the post-gap acquisition, a probe timeout can never fail a
/// correct run. A release that increments and notifies without the mutex
/// instead finishes the gap's occupancy at once and `notify_ran` arrives
/// immediately. Once the test closes the gap, the released permit must still
/// WAKE the parked waiter — not leave it to its 10 s slice — and the waiter
/// must acquire it.
///
/// RESIDUAL LIMIT: a releaser that is descheduled between entering the
/// release and its actual mutation can still let a lock-free mutant escape a
/// given run; this test observes that the mutant did not escape, it cannot
/// make escaping impossible.
#[test]
fn release_after_failed_check_wakes_waiter() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let (holding_tx, holding_rx) = mpsc::channel();
    let (drop_tx, drop_rx) = mpsc::channel();
    let (notify_ran_tx, notify_ran_rx) = mpsc::channel();
    let (parked_tx, parked_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let notified = Arc::new(AtomicBool::new(false));

    // The releaser owns the only permit until the waiter is parked in the gap.
    // It releases through `release_with_test_notify`, whose closure runs with
    // the waiter mutex held right after the increment, so `notify_ran` proves
    // the release reached the mutex (and therefore serialized with the gap).
    //
    // ACCOUNTING: `release_with_test_notify` repays a permit WITHOUT consuming
    // the releaser's `Argon2Permit`, so that guard must not also be dropped —
    // it is forgotten, and the synthetic repayment stands in for exactly the
    // drop it replaces. At capacity 1 the sequence is: `try_acquire` takes the
    // only permit (available 0) -> the synthetic repayment credits it back
    // (1) -> the woken waiter takes the credited permit (0) -> the waiter
    // drops it (1). Nothing is outstanding at the end, so the final
    // `available()` is the capacity; without the `forget` the run would end at
    // 2, one loan credited twice.
    let releaser = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            let permit = semaphore.try_acquire().expect("hold the only permit");
            holding_tx.send(()).expect("test observes the held permit");
            drop_rx
                .recv_timeout(WATCHDOG)
                .expect("test requests the release");
            semaphore.release_with_test_notify(|_lock, cvar| {
                cvar.notify_one();
                notify_ran_tx
                    .send(())
                    .expect("test observes that the release reached the mutex");
            });
            std::mem::forget(permit);
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
                        // held, so a correct release stays blocked in
                        // `lock.lock()` until this closure calls
                        // `wait_timeout` (registration is atomic with
                        // releasing the mutex).
                        if let Some(tx) = parked_tx.take() {
                            tx.send(()).expect("test observes the gap");
                        }
                        if let Some(rx) = go_rx.take() {
                            // Held open until the test has proven the release
                            // is blocked (its `notify_ran` wait timed out).
                            // `GAP_HOLD` is an outer hang detector only, so it
                            // outlives that `GAP_PROBE`-bounded test step.
                            rx.recv_timeout(GAP_HOLD).expect("test closes the gap");
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
    // Request the release only now, with the gap provably open, then await
    // its arrival for one short `GAP_PROBE`. This receive is the first of the
    // two oracles: the correct releaser is blocked on the waiter mutex the
    // waiter is holding, so the expected outcome is a TIMEOUT (the gap is
    // closed next), while a lock-free release answers at once and fails the
    // `completed_in_gap` assertion below.
    drop_tx.send(()).expect("releaser still waiting");
    let completed_in_gap = notify_ran_rx.recv_timeout(GAP_PROBE).is_ok();
    go_tx.send(()).expect("waiter still in the gap");

    let acquired = done_rx
        .recv_timeout(WATCHDOG)
        .expect("released permit must be acquired");
    join_checked(waiter);
    join_checked(releaser);
    assert!(
        !completed_in_gap,
        "the release did not serialize with the waiter mutex"
    );
    assert!(acquired, "released permit must be acquired");
    assert!(
        notified.load(Ordering::Relaxed),
        "the release must notify the parked waiter, not leave it to a slice timeout"
    );
    assert_eq!(
        semaphore.available(),
        1,
        "capacity is intact: one claim, one synthetic repayment, one waiter drop"
    );
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
/// wall clock steps back: the wall-limited remainder can only grow relative
/// to the previous sample, never the budget fixed at admission.
///
/// The injected step is tiny — admission reads the wall clock at 100 ns and
/// the second sample reads 0 ns, a backward step of exactly 100 ns — and its
/// size is irrelevant to the result. What matters is that the wall term never
/// shrinks: the wall remainder stays `deadline - 0 = 2.5000001 s`, while
/// 1.75 s of monotonic time since admission leaves the monotonic remainder at
/// `2.5 s - 1.75 s = 750 ms`. `remaining_budget` takes the smaller term, so
/// the slice is the 750 ms monotonic remainder, not the 1 s cap a wall-only
/// remainder would allow. (Had the wall clock instead jumped forward, the wall
/// term would shrink and could bind; a backward step only ever frees budget
/// above the monotonic term, which is why the slice stays 750 ms.)
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
                // The wall clock stepped back 100 ns, below the admission
                // reading: the wall remainder is now `deadline - 0` =
                // 2.5000001 s, never smaller than before, so a wall-only
                // implementation would keep handing out full 1 s slices.
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
/// taken. The oracle is that the caller completes while the waiter mutex is
/// PROVABLY live: the holder runs inside `release_with_test_notify` (whose
/// closure runs with that mutex held) and does not leave it until it has
/// received the caller's result over a channel, so the caller's completion
/// there cannot have touched the mutex. An implementation that takes the mutex
/// instead deadlocks against the holder and fails; a panicking holder can no
/// longer masquerade as a non-blocking return.
#[test]
fn acquire_until_refuses_preexpired_entry_reading_without_the_waiter_mutex() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let (locked_tx, locked_rx) = mpsc::channel();
    let (probe_tx, probe_rx) = mpsc::channel();
    let (observed_tx, observed_rx) = mpsc::channel();

    // ACCOUNTING: the holder CLAIMS the only permit and then repays it through
    // `release_with_test_notify`, whose closure runs with the waiter mutex held.
    // That synthetic repayment replaces exactly the drop of the real guard, so
    // the guard is forgotten afterwards instead of being dropped as well.
    // Capacity 1: `try_acquire` takes the permit (available 0) -> the synthetic
    // repayment credits it back (1) -> the caller consumes nothing -> 1. One
    // claim, one repayment, no debt counted as a collection, and the semaphore
    // never exceeds its capacity (the previous fixture credited a repayment
    // that stood in for no permit at all and ended at 2).
    let holder = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let permit = semaphore.try_acquire().expect("hold the only permit");
            semaphore.release_with_test_notify(|_lock, _cvar| {
                locked_tx.send(()).expect("test observes the held mutex");
                let observed = probe_rx
                    .recv_timeout(WATCHDOG)
                    .expect("the caller must complete while the waiter mutex is held");
                observed_tx
                    .send(observed)
                    .expect("test observes the caller's result");
            });
            std::mem::forget(permit);
        })
    };
    locked_rx
        .recv_timeout(PROBE_BOUND)
        .expect("the waiter mutex is held by the probe");

    let probe = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let reads = Cell::new(0_usize);
            let result = semaphore.acquire_until_with_clock(100, || {
                reads.set(reads.get() + 1);
                Ok(UnixNanos(100))
            });
            probe_tx
                .send((result.is_none(), reads.get()))
                .expect("test observes the pre-expired refusal");
        })
    };

    // The result can only be forwarded from inside the guarded section, so
    // observing it here proves the mutex was live at the moment the caller
    // returned.
    let (refused, reads) = observed_rx
        .recv_timeout(PROBE_BOUND)
        .expect("a pre-expired entry reading must not block on the waiter mutex");
    join_checked(probe);
    join_checked(holder);
    assert!(refused, "a pre-expired entry reading must refuse");
    assert_eq!(
        reads, 1,
        "the refusal must skip the acquisition loop, so only the entry read happens"
    );
    assert_eq!(
        semaphore.available(),
        1,
        "no permit may be consumed: capacity with nothing outstanding"
    );
}

/// `u64::MAX` is the indefinite-wait sentinel, never an expired deadline, and
/// a pre-expired FINITE deadline is refused at entry — with the waiter mutex
/// FREE — instead of by the acquisition loop. The two halves of the entry
/// guard are covered separately: the sentinel catches an unconditional
/// `entry >= deadline` check, and the read counter catches deletion of the
/// early return, because such a mutant has to re-read the clock inside
/// `acquire_with_budget_sources` (and would lock the mutex). Both acquisitions
/// run under the watchdog, so a mutant that blocks forever fails instead of
/// hanging the suite.
#[test]
fn acquire_until_indefinite_deadline_is_not_expired() {
    let semaphore = Arc::new(Argon2Semaphore::with_capacity(1));
    let (result_tx, result_rx) = mpsc::channel();

    let worker = {
        let semaphore = Arc::clone(&semaphore);
        thread::spawn(move || {
            let semaphore = &*semaphore;
            // Sentinel: `u64::MAX` is an indefinite wait, never expired.
            let indefinite =
                semaphore.acquire_until_with_clock(u64::MAX, || Ok(UnixNanos(u64::MAX)));
            let indefinite_taken = indefinite.is_some();
            let taken_available = semaphore.available();
            drop(indefinite);
            // Fast path: a finite, pre-expired deadline with the mutex FREE
            // must still be refused by the ENTRY read alone. The counter is
            // the oracle: the early return reads the clock once, while a
            // mutant that drops it enters the loop and reads again.
            let reads = Cell::new(0_usize);
            let expired = semaphore.acquire_until_with_clock(100, || {
                reads.set(reads.get() + 1);
                Ok(UnixNanos(100))
            });
            result_tx
                .send((
                    indefinite_taken,
                    taken_available,
                    expired.is_none(),
                    reads.get(),
                ))
                .expect("test observes both entry-guard outcomes");
        })
    };

    let (indefinite_taken, taken_available, refused, reads) = result_rx
        .recv_timeout(WATCHDOG)
        .expect("both entry-guard calls must return under the watchdog");
    join_checked(worker);
    assert!(
        indefinite_taken,
        "an indefinite deadline must not be treated as expired"
    );
    assert_eq!(taken_available, 0, "the free permit is taken");
    assert!(
        refused,
        "a pre-expired finite deadline must be refused even with a free permit"
    );
    assert_eq!(
        reads, 1,
        "the refusal must come from the entry read, not from the acquisition loop"
    );
    assert_eq!(
        semaphore.available(),
        1,
        "neither refusal may consume a permit"
    );
}
