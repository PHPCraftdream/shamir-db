use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_engine::query::filter::FilterContext;
use shamir_engine::table::TableManager;
use shamir_query_builder::val::lit;
use shamir_query_builder::{query::Query, select};
use shamir_storage::{error::DbError, storage_in_memory::InMemoryStore};
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn record_growth(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::SeqCst) + size;
    if COUNTING.load(Ordering::SeqCst) {
        PEAK.fetch_max(live, Ordering::SeqCst);
    }
}

fn record(size: usize) {
    if COUNTING.load(Ordering::Relaxed) {
        LARGEST.fetch_max(size, Ordering::Relaxed);
    }
}

// SAFETY: every operation forwards unchanged pointers, layouts and sizes to
// System; bookkeeping uses only non-allocating atomics and cannot unwind.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: GlobalAlloc's caller supplies a valid nonzero layout;
        // the identical layout is forwarded to System.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_growth(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: the valid nonzero caller layout is forwarded unchanged.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_growth(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr was allocated by System through this forwarding
        // allocator and the caller provides its original layout.
        unsafe { System.dealloc(ptr, layout) }
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        // SAFETY: ptr and layout identify a live System allocation; the
        // caller guarantees a valid nonzero new_size with the same alignment.
        let resized = unsafe { System.realloc(ptr, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                record_growth(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        resized
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct CountingGuard;

impl Drop for CountingGuard {
    fn drop(&mut self) {
        COUNTING.store(false, Ordering::SeqCst);
    }
}

// One test in this binary keeps allocator sampling isolated.
// Reverting the cap joins 200 rows into a 203,976-byte allocation;
// retaining only a length check lets amortized growth exceed 100 KiB.
#[tokio::test]
async fn string_agg_200_rows_stays_within_100_kib_allocation_cap() {
    let table = TableManager::create(
        "t".into(),
        Arc::new(InMemoryStore::new()),
        Arc::new(InMemoryStore::new()),
    )
    .await
    .unwrap();
    let mvcc = Arc::new(MvccStore::new(
        Arc::new(InMemoryStore::new()),
        Arc::new(RepoTxGate::fresh()),
    ));
    let table = table.with_mvcc_store(mvcc);
    for _ in 0..200 {
        table.insert(&InnerValue::Map(new_map())).await.unwrap();
    }
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs).with_max_accumulator_bytes(100 * 1024);
    let query = Query::from("t")
        .select([select::agg_fn_with_args_of(
            "string_agg",
            lit("x"),
            "joined",
            [lit("s".repeat(1024))],
        )])
        .build();
    LARGEST.store(0, Ordering::Relaxed);
    let baseline = LIVE.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    COUNTING.store(true, Ordering::SeqCst);
    let guard = CountingGuard;
    let result = table.read(&query, &ctx).await;
    drop(guard);
    assert!(
        matches!(&result, Err(DbError::Validation(message))
            if message == "result_too_large: ResultBytes actual=102501 max=102400"),
        "{result:?}",
    );
    let largest = LARGEST.load(Ordering::Relaxed);
    assert!(
        largest <= 100 * 1024 + 4096,
        "largest allocation: {largest}"
    );
    // The raw row Vec grows to 256 (RecordId, Bytes) slots (12 KiB on
    // 64-bit targets); 4 KiB covers the separator, slots, caches and error.
    let peak_growth = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    assert!(
        peak_growth <= 100 * 1024 + 16 * 1024,
        "peak requested live growth: {peak_growth}, baseline: {baseline}",
    );
}
