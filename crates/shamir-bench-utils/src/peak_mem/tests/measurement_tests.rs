use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;

use crate::peak_mem::{current_peak, measure, measure_async, PeakTracker};

#[cfg(test)]
pub(crate) fn test_window_lock() -> std::sync::MutexGuard<'static, ()> {
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

struct ThreadWake(thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn poll_once<F: std::future::Future>(mut future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    future.as_mut().poll(&mut Context::from_waker(&waker))
}

#[test]
fn rv_bench_utils_c5422902_async_window_excludes_competitors_and_cleans_up_cancel() {
    let _test_window = crate::peak_mem::tests::measurement_tests::test_window_lock();
    let entered = Arc::new((Mutex::new(false), Condvar::new()));
    let entered_in_future = Arc::clone(&entered);
    let mut future = Box::pin(measure_async(async move {
        let (lock, ready) = &*entered_in_future;
        *lock.lock().unwrap() = true;
        ready.notify_one();
        std::future::pending::<()>().await;
    }));
    assert!(matches!(poll_once(future.as_mut()), Poll::Pending));
    let (lock, ready) = &*entered;
    let mut flag = lock.lock().unwrap();
    while !*flag {
        flag = ready.wait(flag).unwrap();
    }
    // A refused window must not run the workload, sync or async.
    let ran = AtomicBool::new(false);
    assert!(measure(|| ran.store(true, Ordering::SeqCst)).is_none());
    assert!(!ran.load(Ordering::SeqCst));
    let mut second = Box::pin(measure_async(async {
        ran.store(true, Ordering::SeqCst);
    }));
    assert!(matches!(poll_once(second.as_mut()), Poll::Ready(None)));
    assert!(!ran.load(Ordering::SeqCst));
    drop(second);
    drop(flag);
    drop(future);
    assert!(measure(|| 17).is_some());
}

#[test]
fn rv_bench_utils_77eb8d3e_measure_tracks_explicit_tracker_allocations() {
    let _test_window = crate::peak_mem::tests::measurement_tests::test_window_lock();
    let tracker = PeakTracker::new();
    let (result, peak) = measure(|| {
        let layout = std::alloc::Layout::from_size_align(3072, 8).unwrap();
        // SAFETY: `layout` is valid and deallocated with the same allocator.
        let ptr = unsafe { std::alloc::GlobalAlloc::alloc(&tracker, layout) };
        assert!(!ptr.is_null());
        let captured = current_peak();
        // SAFETY: ptr was allocated by this allocator with this layout.
        unsafe { std::alloc::GlobalAlloc::dealloc(&tracker, ptr, layout) };
        (captured, layout.size())
    })
    .expect("exclusive test window");
    assert!(peak >= result.1);
    assert!(result.0 >= result.1);
}

#[test]
fn rv_bench_utils_77eb8d3e_measure_resets_peak_after_large_allocation_is_freed() {
    let _test_window = crate::peak_mem::tests::measurement_tests::test_window_lock();
    let tracker = PeakTracker::new();
    let large_layout = std::alloc::Layout::from_size_align(1 << 20, 8).unwrap();
    let small_layout = std::alloc::Layout::from_size_align(128, 8).unwrap();
    let base = crate::peak_mem::current_allocated();
    let large = measure(|| {
        // SAFETY: `large_layout` is valid, and the returned pointer is checked
        // then deallocated by the same tracker using the identical layout.
        let ptr = unsafe { std::alloc::GlobalAlloc::alloc(&tracker, large_layout) };
        assert!(!ptr.is_null());
        // SAFETY: `ptr` was returned by this tracker for `large_layout`.
        unsafe { std::alloc::GlobalAlloc::dealloc(&tracker, ptr, large_layout) };
    })
    .unwrap();
    assert!(large.1 >= base + large_layout.size());
    let small = measure(|| {
        // SAFETY: `small_layout` is valid, and the returned pointer is checked
        // then deallocated by the same tracker using the identical layout.
        let ptr = unsafe { std::alloc::GlobalAlloc::alloc(&tracker, small_layout) };
        assert!(!ptr.is_null());
        // SAFETY: `ptr` was returned by this tracker for `small_layout`.
        unsafe { std::alloc::GlobalAlloc::dealloc(&tracker, ptr, small_layout) };
    })
    .unwrap();
    assert!(small.1 < base + large_layout.size());
}
