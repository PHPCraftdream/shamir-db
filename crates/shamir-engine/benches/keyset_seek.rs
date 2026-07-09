//! Keyset-seek (`Pagination::After`) page-cost benchmark — audit finding 1.2.
//!
//! Measures the cost of ONE keyset page (`ORDER BY y ASC AFTER [seek] LIMIT k`)
//! over a sorted index on `y`, as the table size N grows, with the seek key
//! pinned near the START of the value range (the worst case: the entire
//! remaining half-plane lies past the seek).
//!
//! Before the fix, `read_keyset_seek` collected the ENTIRE half-plane past the
//! seek key into a `BTreeSet<RecordId>`, fetched+decoded+projected EVERY such
//! record, fully sorted them, and only THEN truncated to `k` — O(N) fetch +
//! decode + sort per page, so per-page cost grew linearly with N (a full
//! scroll degrades to O(N²)).
//!
//! After the fix, the sorted index is walked in value order with an early stop
//! after `k` survivors — per-page cost is O(k + |rows == seek|), i.e. roughly
//! FLAT as N grows. This bench pins that flatness: `page_cost/{N}` should stay
//! ~constant across N instead of scaling with it.
//!
//! Uses the fixed-iteration harness (`bench_scale_tool`): the table + sorted
//! index fixture is built ONCE per N at registration time and shared read-only
//! (a keyset read never mutates it) → `bench_async`.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::ReadQuery;
use shamir_engine::table::table_manager::TableManager;
use shamir_query_builder::query::Query;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

fn parse_bool_env(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Table sizes. The whole point of the bench is to watch per-page cost as N
/// grows — a widening spread that stayed flat after the fix and grew linearly
/// before it. Default tiers stay small enough for the ~10ms/call budget; large
/// N (where the pre-fix O(N) cliff is dramatic) is opt-in.
fn table_sizes() -> Vec<usize> {
    let mut sizes = vec![1_000, 5_000, 20_000];
    if parse_bool_env("BENCH_KEYSET_HUGE") {
        sizes.push(100_000);
        sizes.push(500_000);
    }
    sizes
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Build a table of `n` records `{ y: i }` for `i in 0..n` (unique, dense `y`)
/// with a sorted index on `y`.
async fn build_table(n: usize) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("bench_table".into(), data, info)
        .await
        .unwrap();

    let interner = mgr.interner().get().await.unwrap();
    let k_y = match interner.touch_ind("y").unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };

    let chunk = 10_000;
    let mut batch = Vec::with_capacity(chunk);
    for i in 0..n {
        let mut m = new_map_wc(1);
        m.insert(k_y.clone(), InnerValue::Int(i as i64));
        batch.push(InnerValue::Map(m));
        if batch.len() == chunk || i == n - 1 {
            mgr.insert_many(&batch).await.unwrap();
            batch.clear();
        }
    }

    mgr.create_sorted_index("y_sorted", &["y"]).await.unwrap();
    mgr
}

/// `ORDER BY y ASC AFTER [seek] LIMIT k` — one keyset page. `seek = 5` pins
/// the seek near the start so the remaining half-plane is ~N (worst case).
fn query_page(k: u64) -> ReadQuery {
    Query::from("bench_table")
        .order_by_asc("y")
        .after(vec![QueryValue::Int(5)], Some(k))
        .build()
}

fn main() {
    let mut h = Harness::new("keyset_seek", env!("CARGO_MANIFEST_DIR"));
    let rt = rt();

    for &n in &table_sizes() {
        let mgr = rt.block_on(build_table(n));
        let q = query_page(10);
        h.bench_async(&format!("page_cost/{n}"), move || {
            let mgr = mgr.clone();
            let q = q.clone();
            async move {
                let interner = mgr.interner().get().await.unwrap();
                let refs = new_map();
                let ctx = FilterContext::new(interner, &refs);
                std::hint::black_box(mgr.read(&q, &ctx).await.unwrap());
            }
        });
    }

    h.run();
}
