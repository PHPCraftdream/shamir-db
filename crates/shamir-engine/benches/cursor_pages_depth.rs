//! Cursor pages 1/10/100 deep — keyset (`Pagination::After`) walk-cost
//! benchmark. F-53d (#877): this workload category (cursor pagination
//! depth) had no dedicated bench before this file — `keyset_seek.rs`
//! measures a single page's cost as table size N grows, and
//! `shamir-db/benches/changelog_read.rs` measures changelog backfill depth,
//! but neither walks a chain of consecutive keyset pages the way a client
//! actually pages through a cursor (`FetchNext` × depth).
//!
//! Measures the cost of walking `depth` consecutive keyset pages
//! (`ORDER BY y ASC AFTER [seek] LIMIT k`, `k` = 50), each page's seek key
//! derived from the previous page's last row — the same "wire" sequence a
//! `cursor_handlers.rs` `FetchNext` loop drives, minus the batch/session
//! layer. Table is fixed at 50,000 rows with a sorted index on `y` so pages
//! deep into the range (100 pages × 50 rows = 5,000 rows in) still land well
//! inside the fixture.
//!
//! Three cells, one per requested depth:
//!   * `pages_deep/1`   — a single page (first-page cost only).
//!   * `pages_deep/10`  — 10 consecutive pages walked from the start.
//!   * `pages_deep/100` — 100 consecutive pages walked from the start.
//!
//! Because each page's seek key depends on the previous page's result, the
//! whole `depth`-page walk is one indivisible unit of work per iteration —
//! there is no shared-fixture split point inside a single cell the way
//! `keyset_seek.rs` splits "build once, read many". The table fixture
//! itself IS shared read-only across iterations (`bench_async`, plan 1):
//! walking a keyset cursor never mutates the table.
//!
//! Uses the fixed-iteration harness (`bench_scale_tool`), never Criterion.
//!
//! Run:
//!   cargo bench -p shamir-engine --bench cursor_pages_depth

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::table::table_manager::TableManager;
use shamir_query_builder::query::Query;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

/// Table size. Large enough that a 100-page-deep walk (100 * PAGE_SIZE =
/// 5,000 rows) stays comfortably inside the fixture with headroom to spare.
const TABLE_ROWS: usize = 50_000;

/// Rows per page — a realistic client page size.
const PAGE_SIZE: u64 = 50;

/// Cursor depths under test — the three named cells from the F-53d brief.
const DEPTHS: &[usize] = &[1, 10, 100];

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Build a table of `n` records `{ y: i }` for `i in 0..n` (unique, dense
/// `y`) with a sorted index on `y` — same fixture shape as `keyset_seek.rs`.
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

/// Walk `depth` consecutive keyset pages starting from the very beginning
/// of the `y` range, deriving each page's seek key from the last row of the
/// previous page (the "next cursor" a real client would resend). Returns
/// the total row count actually seen, purely so `black_box` has something
/// real to hold onto and the loop can't be trivially optimized away.
async fn walk_pages(mgr: &TableManager, depth: usize) -> usize {
    // Start below the smallest `y` (0) so the first page's seek is
    // exclusive-start and picks up row 0.
    let mut seek = QueryValue::Int(-1);
    let mut total_rows = 0usize;

    for _ in 0..depth {
        let q = Query::from("bench_table")
            .order_by_asc("y")
            .after(vec![seek.clone()], Some(PAGE_SIZE))
            .build();

        let interner = mgr.interner().get().await.unwrap();
        let refs = new_map();
        let ctx = FilterContext::new(interner, &refs);
        let result = mgr.read(&q, &ctx).await.unwrap();

        total_rows += result.records.len();

        // Advance the seek to the last row's `y` value for the next page.
        // `get_value_i64` reads through either `QueryRecord` variant the
        // read path may return (`Direct` for a plain projection, `Inserted`
        // when the planner's fast keyset-seek path attaches the record via
        // the write-result shape) — an empty page (walked past the end of
        // the fixture) simply stops advancing. depth=100 stays well inside
        // TABLE_ROWS so this is not expected to trigger in practice, but a
        // truncated tail must not panic.
        match result.records.last().and_then(|r| r.get_value_i64("y")) {
            Some(y) => seek = QueryValue::Int(y),
            None => break,
        }
    }

    total_rows
}

fn main() {
    let mut h = Harness::new("cursor_pages_depth", env!("CARGO_MANIFEST_DIR"));
    let rt = rt();

    let mgr = rt.block_on(build_table(TABLE_ROWS));

    for &depth in DEPTHS {
        let mgr = mgr.clone();
        h.bench_async(&format!("pages_deep/{depth}"), move || {
            let mgr = mgr.clone();
            async move {
                std::hint::black_box(walk_pages(&mgr, depth).await);
            }
        });
    }

    h.run();
}
