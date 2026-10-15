//! Deterministic tests for synchronous publication/termination serialization.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use crate::tx::cold_reconcile::lifecycle::PubTermInterlock;

#[test]
fn publication_runs_callback_and_termination_is_sticky() {
    let lock = PubTermInterlock::new();
    let value = AtomicUsize::new(0);
    assert_eq!(lock.publish(|| value.fetch_add(1, Ordering::SeqCst)), Ok(0));
    assert_eq!(value.load(Ordering::SeqCst), 1);
    assert!(lock.terminate());
    assert!(!lock.terminate());
    assert!(lock.is_terminated());
    assert!(lock
        .publish(|| value.fetch_add(1, Ordering::SeqCst))
        .is_err());
    assert_eq!(value.load(Ordering::SeqCst), 1);
}

#[test]
fn termination_waits_until_inflight_publication_callback_finishes() {
    let lock = Arc::new(PubTermInterlock::new());
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let published = Arc::new(AtomicUsize::new(0));
    let (worker_lock, worker_entered, worker_release, worker_value) = (
        Arc::clone(&lock),
        Arc::clone(&entered),
        Arc::clone(&release),
        Arc::clone(&published),
    );
    let worker = thread::spawn(move || {
        worker_lock.publish(|| {
            worker_entered.wait();
            worker_release.wait();
            worker_value.store(42, Ordering::SeqCst);
        })
    });
    entered.wait();
    let (term_entered, term_release) = (Arc::new(Barrier::new(2)), Arc::new(Barrier::new(2)));
    let (te, tr, term_lock) = (
        Arc::clone(&term_entered),
        Arc::clone(&term_release),
        Arc::clone(&lock),
    );
    let terminator = thread::spawn(move || {
        te.wait();
        let result = term_lock.terminate();
        tr.wait();
        result
    });
    term_entered.wait();
    assert_eq!(published.load(Ordering::SeqCst), 0);
    release.wait();
    worker.join().unwrap().unwrap();
    term_release.wait();
    assert!(terminator.join().unwrap());
    assert_eq!(published.load(Ordering::SeqCst), 42);
    assert!(lock.is_terminated());
}

#[test]
fn termination_first_prevents_publication_callback() {
    let lock = PubTermInterlock::new();
    assert!(lock.terminate());
    let called = AtomicUsize::new(0);
    assert!(lock
        .publish(|| called.fetch_add(1, Ordering::SeqCst))
        .is_err());
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn unprotected_test_seam_demonstrates_stale_admission_race() {
    let lock = PubTermInterlock::new();
    let passed_check = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let (p, r, l) = (
        Arc::clone(&passed_check),
        Arc::clone(&resume),
        Arc::new(lock),
    );
    let worker_lock = Arc::clone(&l);
    let worker = thread::spawn(move || {
        worker_lock.unprotected_publish_for_test(
            || {
                p.wait();
                r.wait();
            },
            || 1,
        )
    });
    passed_check.wait();
    assert!(l.terminate());
    resume.wait();
    assert_eq!(worker.join().unwrap(), Ok(1));
    assert!(l.is_terminated());
}
