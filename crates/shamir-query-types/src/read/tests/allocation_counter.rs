//! Shared allocation-count oracle for regression tests (P2 carry-overs).
//! The allocator tracks only allocations on the measuring thread; unrelated
//! parallel test threads cannot affect a count. Nested windows are rejected.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

impl CountingAllocator {
    fn record_allocation() {
        // TLS teardown can invoke allocator callbacks; try_with deliberately
        // ignores access failure during teardown rather than panicking there.
        let _ = ACTIVE.try_with(|active| {
            if active.get() {
                let _ = ALLOCS.try_with(|allocs| allocs.set(allocs.get() + 1));
            }
        });
    }
}

// SAFETY: every operation forwards its arguments unchanged to `System`;
// callback bookkeeping uses non-allocating TLS Cells via `try_with` and does
// not panic, including when TLS is being destroyed.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record_allocation();
        // SAFETY: the caller supplies a valid nonzero layout, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record_allocation();
        // SAFETY: the valid nonzero caller layout is forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr was allocated by System through this allocator and the
        // caller provides its original layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::record_allocation();
        // SAFETY: ptr/layout identify a live System allocation; new_size and
        // alignment satisfy the GlobalAlloc caller's contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct WindowGuard;

impl Drop for WindowGuard {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|active| active.set(false));
    }
}

/// Count allocations made on this thread while `f` runs. Nested windows
/// panic before changing state; the guard disables counting during unwinding.
pub(crate) fn count_allocations<T>(f: impl FnOnce() -> T) -> (usize, T) {
    ACTIVE.with(|active| {
        assert!(!active.get(), "nested allocation-counting window");
        ALLOCS.with(|allocs| allocs.set(0));
        active.set(true);
    });
    let _guard = WindowGuard;
    let result = f();
    let count = ALLOCS.with(Cell::get);
    (count, result)
}
