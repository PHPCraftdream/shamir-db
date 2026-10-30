//! Isolated tests of the exact gate and accounting path used by argon2id.
use super::{with_argon2_permit, CountingSemaphore, ARGON2ID_CONCURRENCY_CAP};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

#[test]
fn argon2id_concurrency_cap_bounds_parallel_calls() {
    let cap = ARGON2ID_CONCURRENCY_CAP as usize;
    let gate = Arc::new(CountingSemaphore::with_capacity(cap as u32));
    let current = Arc::new(AtomicU32::new(0));
    let peak = Arc::new(AtomicU32::new(0));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..cap {
        let gate = Arc::clone(&gate);
        let current = Arc::clone(&current);
        let peak = Arc::clone(&peak);
        let release = Arc::clone(&release);
        let entered = entered_tx.clone();
        workers.push(thread::spawn(move || {
            with_argon2_permit(&gate, &current, &peak, || {
                entered.send(()).unwrap();
                let (lock, condvar) = &*release;
                let mut ready = lock.lock().unwrap();
                while !*ready {
                    ready = condvar.wait(ready).unwrap();
                }
            });
        }));
    }
    for _ in 0..cap {
        entered_rx.recv().unwrap();
    }
    let saturated = current.load(Ordering::Relaxed) == cap as u32 && gate.test_available() == 0;
    let (event_tx, event_rx) = mpsc::channel();
    *gate.waiting_observer.lock().unwrap() = Some(event_tx.clone());
    let contender_gate = Arc::clone(&gate);
    let contender_current = Arc::clone(&current);
    let contender_peak = Arc::clone(&peak);
    let contender = thread::spawn(move || {
        with_argon2_permit(&contender_gate, &contender_current, &contender_peak, || {
            let _ = event_tx.send(false); // entered instead of waiting
        });
    });
    let waited = event_rx.recv() == Ok(true);
    // Acquiring this mutex after the waiting event proves the waiter has
    // atomically registered with the condvar before permits are released.
    if waited {
        drop(gate.notify.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
    {
        let (lock, condvar) = &*release;
        *lock.lock().unwrap() = true;
        condvar.notify_all();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    contender.join().unwrap();
    assert!(saturated, "holders did not saturate the gate");
    assert!(waited, "contender entered without waiting for a permit");
    assert_eq!(peak.load(Ordering::Relaxed), cap as u32);
    assert_eq!(current.load(Ordering::Relaxed), 0);
    assert_eq!(gate.test_available(), cap as i64);
}

#[test]
fn panic_releases_permit_and_in_flight_count() {
    let gate = CountingSemaphore::with_capacity(1);
    let current = AtomicU32::new(0);
    let peak = AtomicU32::new(0);
    let result = catch_unwind(AssertUnwindSafe(|| {
        with_argon2_permit(&gate, &current, &peak, || panic!("injected work panic"));
    }));
    assert!(result.is_err());
    assert_eq!(current.load(Ordering::Relaxed), 0);
    assert_eq!(peak.load(Ordering::Relaxed), 1);
    assert_eq!(gate.test_available(), 1);
}

#[test]
fn poisoned_notification_mutex_recovers_on_release_and_wait() {
    let gate = Arc::new(CountingSemaphore::with_capacity(1));
    gate.test_acquire();
    let copy = Arc::clone(&gate);
    assert!(thread::spawn(move || {
        let _guard = copy.notify.0.lock().unwrap();
        panic!("poison notification lock");
    })
    .join()
    .is_err());
    let (event_tx, event_rx) = mpsc::channel();
    *gate.waiting_observer.lock().unwrap() = Some(event_tx.clone());
    let copy = Arc::clone(&gate);
    let waiter = thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            copy.test_acquire();
            let _ = event_tx.send(false);
            copy.test_release();
        }));
        if result.is_err() {
            // A poisoned-lock panic is an explicit event rather than a hang.
            let _ = copy
                .waiting_observer
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .send(false);
        }
        result
    });
    let waited = event_rx.recv() == Ok(true);
    if waited {
        drop(gate.notify.0.lock().unwrap_or_else(|e| e.into_inner()));
    }
    let release_result = catch_unwind(AssertUnwindSafe(|| gate.test_release()));
    if release_result.is_err() {
        // A mutant that panics after returning the permit must still wake the
        // waiter, so the oracle reports failure instead of hanging on join.
        let _guard = gate.notify.0.lock().unwrap_or_else(|e| e.into_inner());
        gate.notify.1.notify_all();
    }
    let result = waiter.join().unwrap();
    assert!(release_result.is_ok(), "poisoned release panicked");
    assert!(waited, "waiter bypassed or failed the poisoned lock");
    assert!(result.is_ok(), "poisoned wait or release panicked");
    assert!(gate.poisoned_waits.load(Ordering::Relaxed) > 0);
    assert_eq!(gate.test_available(), 1);
}
