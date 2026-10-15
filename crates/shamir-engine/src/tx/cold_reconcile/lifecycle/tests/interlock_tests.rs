//! Deterministic tests for synchronous publication/termination serialization.
use crate::tx::cold_reconcile::lifecycle::PubTermInterlock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

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
    run_inflight_termination(true);
}

#[test]
fn nonwaiting_negative_control_returns_before_publication_finishes() {
    run_inflight_termination(false);
}

fn run_inflight_termination(protected: bool) {
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
    let term_lock = Arc::clone(&lock);
    let term_value = Arc::clone(&published);
    let (sample_tx, sample_rx) = std::sync::mpsc::channel();
    let terminator = thread::spawn(move || {
        let result = if protected {
            term_lock.terminate_after_observed_contention_for_test(|| sample_tx.send(()).unwrap())
        } else {
            let result = term_lock.nonwaiting_termination_for_test();
            sample_tx.send(()).unwrap();
            result
        };
        let at_return = term_value.load(Ordering::SeqCst);
        (result, at_return)
    });
    if protected {
        // Wait until try_lock has observed the callback-owned gate, then let publication finish.
        sample_rx.recv().unwrap();
        release.wait();
        worker.join().unwrap().unwrap();
        let (result, sample) = terminator.join().unwrap();
        assert!(result);
        assert_eq!(sample, 42);
    } else {
        // The negative-control return snapshot is observed before releasing callback.
        sample_rx.recv().unwrap();
        let (result, sample) = terminator.join().unwrap();
        assert!(result);
        assert_eq!(sample, 0);
        release.wait();
        worker.join().unwrap().unwrap();
        assert_eq!(published.load(Ordering::SeqCst), 42);
    }
    assert_eq!(lock.is_terminated(), protected);
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
