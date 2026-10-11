use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::order::TopKHeap;
use shamir_engine::repo::changelog_store::StoreChangelog;
use shamir_engine::table::TableManager;
use shamir_query_builder::query::Query;
use shamir_query_types::read::{OrderBy, Pagination};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{ChangelogStore, MvccStore, RepoTxGate};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static LARGEST: AtomicUsize = AtomicUsize::new(0);

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
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: the valid nonzero caller layout is forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr was allocated by System through this forwarding
        // allocator and the caller provides its original layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        // SAFETY: ptr and layout identify a live System allocation; the
        // caller guarantees a valid nonzero new_size with the same alignment.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct CountingGuard;

impl Drop for CountingGuard {
    fn drop(&mut self) {
        COUNTING.store(false, Ordering::Relaxed);
    }
}

// One test in this binary: no other test can overlap the measured section.
// Red before the fix: empty TopKHeap and range_from reserve the hostile count;
// Page multiplication and pagination metadata addition can also overflow.
#[tokio::test]
async fn empty_pagination_and_journal_preallocations_are_bounded() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let table = TableManager::create("t".into(), data, info).await.unwrap();
    let mvcc = Arc::new(MvccStore::new(history, Arc::new(RepoTxGate::fresh())));
    let table = table.with_mvcc_store(mvcc);
    let journal = StoreChangelog::new(Arc::new(InMemoryStore::new()));
    let order = OrderBy::asc("score");
    let refs = new_map();

    LARGEST.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let guard = CountingGuard;
    for (skip, take) in [(0, usize::MAX), (usize::MAX, 1), (usize::MAX, usize::MAX)] {
        let heap = TopKHeap::new(&order, skip, take, false);
        assert!(heap.is_empty());
        assert!(heap.into_sorted().0.is_empty());
    }
    let empty = journal.range_from(Bytes::new(), usize::MAX).await.unwrap();
    assert!(empty.is_empty());
    let overflow = Pagination::page(u64::MAX, u64::MAX).try_resolve();
    assert_eq!(overflow.unwrap_err().code(), Some("pagination_too_large"));
    assert_eq!(
        Pagination::page(3, 10).try_resolve().unwrap(),
        (20, Some(10))
    );

    for indexed in [false, true] {
        if indexed {
            table
                .create_sorted_index("score_idx", &["score"])
                .await
                .unwrap();
            table.create_index("base_idx", &["status"]).await.unwrap();
        }
        let interner = table.interner().get().await.unwrap();
        let ctx = FilterContext::new(interner, &refs);
        let queries = [
            Query::from("t")
                .order_by_asc("score")
                .limit(u64::MAX)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .limit(1)
                .offset(u64::MAX)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .page(u64::MAX, u64::MAX)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .after(vec![QueryValue::Int(0)], Some(u64::MAX))
                .build(),
            Query::from("t")
                .where_eq("status", 1_i64)
                .limit(u64::MAX)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .limit(u64::MAX)
                .as_of_version(0)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .page(u64::MAX, u64::MAX)
                .as_of_version(0)
                .build(),
            Query::from("t")
                .order_by_asc("score")
                .limit(u64::MAX)
                .offset(u64::MAX)
                .count_total(true)
                .build(),
        ];
        for query in queries {
            match table.read(&query, &ctx).await {
                Ok(result) => assert!(result.records.is_empty(), "{query:?}"),
                Err(DbError::Validation(_)) => {}
                Err(error) => panic!("unexpected error for {query:?}: {error}"),
            }
        }
    }
    drop(guard);
    let largest = LARGEST.load(Ordering::Relaxed);
    assert!(largest < 64 * 1024 * 1024, "largest allocation: {largest}");
}
