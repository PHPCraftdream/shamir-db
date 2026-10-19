use crate::common::time::UnixNanos;
use crate::server::argon2_semaphore::Argon2Semaphore;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
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
fn acquire_blocks_until_permit_available() {
    let s = Arc::new(Argon2Semaphore::with_capacity(1));
    let p1 = s.try_acquire().unwrap();

    let s2 = s.clone();
    let handle = thread::spawn(move || {
        let _p = s2.acquire(); // blocks until p1 dropped
        42
    });

    // Give the thread a moment to start blocking.
    thread::sleep(Duration::from_millis(50));
    drop(p1); // release → wakes the waiter

    assert_eq!(handle.join().unwrap(), 42);
}

#[test]
fn acquire_until_returns_none_on_timeout() {
    let s = Argon2Semaphore::with_capacity(0);
    let now = UnixNanos::now().as_u64();
    let deadline = now + 50 * 1_000_000; // 50ms in ns
    let r = s.acquire_until(deadline);
    assert!(r.is_none(), "must time out");
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
