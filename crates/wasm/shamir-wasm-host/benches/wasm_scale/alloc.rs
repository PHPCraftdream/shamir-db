//! A counting allocator that records heap use only while a measurement window is open.
//!
//! Outside a window the cost is one relaxed atomic load per allocation. It sees the heap
//! only: compiled code and linear memories live in mapped memory that the system
//! allocator never hands out, so the memory report states those through the engine's own
//! numbers (code size, pooling ceiling) and not through this counter.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct Counting;

static OPEN: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    if OPEN.load(Ordering::Relaxed) {
        let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
}

fn shrink(bytes: usize) {
    if OPEN.load(Ordering::Relaxed) {
        // Memory allocated before the window and freed inside it must not underflow.
        let _ = LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
            Some(live.saturating_sub(bytes))
        });
    }
}

// SAFETY: every operation is delegated to `System` with the caller's layout; the counters
// are plain atomics and never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            grow(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
        shrink(layout.size());
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = System.realloc(pointer, layout, new_size);
        if !moved.is_null() {
            shrink(layout.size());
            grow(new_size);
        }
        moved
    }
}

/// Runs `work` in a window and returns its result with the bytes still live when it
/// returned and the peak live bytes inside the window.
pub fn measure<R>(work: impl FnOnce() -> R) -> (R, usize, usize) {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    OPEN.store(true, Ordering::Relaxed);
    let result = work();
    OPEN.store(false, Ordering::Relaxed);
    (
        result,
        LIVE.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed),
    )
}
