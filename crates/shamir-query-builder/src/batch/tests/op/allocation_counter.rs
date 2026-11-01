//! Test-only allocation-byte oracle for borrowed batch encoding.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;
impl CountingAllocator {
    fn record(size: usize) {
        let _ = ACTIVE.try_with(|active| {
            if active.get() {
                let _ = BYTES.try_with(|bytes| bytes.set(bytes.get().saturating_add(size)));
            }
        });
    }
}

// SAFETY: forwards unchanged to System; thread-local Cell bookkeeping neither allocates nor panics.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::record(layout.size());
        // SAFETY: forwarded unchanged to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::record(layout.size());
        // SAFETY: forwarded unchanged to System.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged to System.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::record(new_size);
        // SAFETY: forwarded unchanged to System.
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

pub(crate) fn count_allocated_bytes<T>(f: impl FnOnce() -> T) -> (usize, T) {
    ACTIVE.with(|active| {
        assert!(!active.get(), "nested allocation window");
        BYTES.with(|bytes| bytes.set(0));
        active.set(true);
    });
    let _guard = WindowGuard;
    let result = f();
    (BYTES.with(Cell::get), result)
}
