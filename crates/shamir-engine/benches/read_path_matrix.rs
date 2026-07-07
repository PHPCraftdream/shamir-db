//! Read-path matrix benchmark — measurement-only (Phase 0b).
//!
//! Turns S1/S2/S3 performance cliffs into N->{time, peak_mem} curves.
//!
//! **Axes:**
//! - N (table size): {10K, 100K, 1M}. 10M opt-in via `BENCH_READ_PATH_HUGE=1`.
//! - Query shape: 5 forms targeting specific read-path weaknesses.
//! - Backend: in-memory (engine-layer is the bottleneck, not storage).
//!
//! **Query shapes:**
//! 1. fast_path — ORDER BY y LIMIT 10, sorted index on y (baseline).
//! 2. s2_no_index — ORDER BY y LIMIT 10, no index (full materialize+sort).
//! 3. s2_s3_combo — WHERE x > 5 ORDER BY y LIMIT 10, no index (worst case).
//! 4. s3_range_and — WHERE x >= 10 AND name = "foo", sorted index on x.
//! 5. s1_asof — AsOf(Timestamp(t)) point read (version_at_or_before_ts scan).
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each query
//! shape's fixture (table + index) is built ONCE per N at registration time
//! and shared read-only across every iteration (reads never mutate the
//! fixture) → `bench_async`.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::ReadQuery;
use shamir_engine::table::table_manager::TableManager;
use shamir_query_builder::filter::{and, eq, gt, gte};
use shamir_query_builder::query::Query;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ── Configuration ───────────────────────────────────────────────────────

fn parse_bool_env(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn table_sizes() -> Vec<usize> {
    // Default tiers capped at 3_000 (were 10_000/100_000): full-table
    // no-index scans in this matrix are O(n), and at 100_000 a single
    // scan call already cost multiple seconds — far past the ~10ms/call
    // budget the harness now expects (it owns repetition count
    // externally). Large-N behavior is still available on demand via
    // `BENCH_READ_PATH_HUGE`, just not part of the default fast sweep.
    let mut sizes = vec![1_000, 3_000];
    if parse_bool_env("BENCH_READ_PATH_HUGE") {
        sizes.push(10_000);
        sizes.push(100_000);
        sizes.push(1_000_000);
        sizes.push(10_000_000);
    }
    sizes
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// ── Table setup ─────────────────────────────────────────────────────────

/// Schema: records with fields `x` (i64, [0,1000)), `y` (i64, [0,10000)),
/// `name` (Str, random 8-char).
struct TableFixture {
    mgr: TableManager,
    /// RecordIds of inserted records (needed for s1_asof updates).
    record_ids: Vec<RecordId>,
}

async fn build_table(n: usize) -> TableFixture {
    build_table_inner(n, false).await
}

/// Build table with MvccStore attached — needed for Shape 5 (AsOf temporal
/// reads). Other shapes don't need it.
async fn build_table_mvcc(n: usize) -> TableFixture {
    build_table_inner(n, true).await
}

async fn build_table_inner(n: usize, with_mvcc: bool) -> TableFixture {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mut mgr = TableManager::create("bench_table".into(), data, info)
        .await
        .unwrap();
    if with_mvcc {
        let gate = Arc::new(RepoTxGate::fresh());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mvcc = Arc::new(MvccStore::new(history, gate));
        mgr = mgr.with_mvcc_store(mvcc);
    }

    let interner = mgr.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_x = touch("x");
    let k_y = touch("y");
    let k_name = touch("name");

    let mut record_ids = Vec::with_capacity(n);

    // Batch insert in chunks to avoid extreme single-call overhead.
    let chunk_size = 10_000;
    let mut batch = Vec::with_capacity(chunk_size);
    for i in 0..n {
        let mut m = new_map_wc(3);
        m.insert(k_x.clone(), InnerValue::Int((i % 1000) as i64));
        m.insert(k_y.clone(), InnerValue::Int((i % 10_000) as i64));
        // Deterministic "random" 8-char name based on index.
        let name = format!("{:08x}", i.wrapping_mul(0x9E3779B9) & 0xFFFFFFFF);
        m.insert(k_name.clone(), InnerValue::Str(name));
        batch.push(InnerValue::Map(m));

        if batch.len() == chunk_size || i == n - 1 {
            let ids = mgr.insert_many(&batch).await.unwrap();
            record_ids.extend(ids);
            batch.clear();
        }
    }

    TableFixture { mgr, record_ids }
}

/// Create a sorted index on `field_name` for the given table.
///
/// Uses `TableManager::create_sorted_index` directly — `create_index_v2`
/// routes `sorted: true` through the hash-index path (ignores the flag),
/// so the sorted index was never actually created in Phase 0 benchmarks.
async fn create_sorted_index(mgr: &TableManager, _name: &str, field: &str) {
    mgr.create_sorted_index(&format!("{field}_sorted"), &[field])
        .await
        .unwrap();
}

/// Update ~10% of records to accumulate ts-history for AsOf queries.
async fn accumulate_history(mgr: &TableManager, record_ids: &[RecordId]) {
    let interner = mgr.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_x = touch("x");
    let k_y = touch("y");
    let k_name = touch("name");

    // Update every 10th record.
    for (idx, rid) in record_ids.iter().enumerate() {
        if idx % 10 != 0 {
            continue;
        }
        let mut m = new_map_wc(3);
        m.insert(k_x.clone(), InnerValue::Int(((idx + 500) % 1000) as i64));
        m.insert(k_y.clone(), InnerValue::Int(((idx + 5000) % 10_000) as i64));
        m.insert(
            k_name.clone(),
            InnerValue::Str(format!("{:08x}", idx.wrapping_mul(0xDEADBEEF) & 0xFFFFFFFF)),
        );
        mgr.set(*rid, &InnerValue::Map(m)).await.unwrap();
    }
}

// ── Query builders ──────────────────────────────────────────────────────

/// Shape 1: ORDER BY y LIMIT 10, sorted index on y. Fast-path baseline.
fn query_fast_path() -> ReadQuery {
    Query::from("bench_table")
        .order_by_asc("y")
        .limit(10)
        .build()
}

/// Shape 2: ORDER BY y LIMIT 10, NO index on y. Full materialize+sort.
fn query_s2_no_index() -> ReadQuery {
    Query::from("bench_table")
        .order_by_asc("y")
        .limit(10)
        .build()
}

/// Shape 3: WHERE x > 5 ORDER BY y LIMIT 10, no index. Full scan + sort.
fn query_s2_s3_combo() -> ReadQuery {
    Query::from("bench_table")
        .where_(gt("x", 5i64))
        .order_by_asc("y")
        .limit(10)
        .build()
}

/// Shape 4: WHERE x >= 10 AND name = "foo", sorted index on x.
/// `x = i % 1000` -> `x >= 10` keeps 99% of records (non-selective range).
/// On non-selective ranges, sorted-index scan + per-rid record fetch
/// (2 reads/row) is SLOWER than full table scan (1 read/row). This shape
/// is a tipping-point regression guard, NOT an optimization target.
fn query_s3_range_and() -> ReadQuery {
    Query::from("bench_table")
        .where_(and([gte("x", 10i64), eq("name", "foo")]))
        .build()
}

/// Shape 4b: WHERE x >= 990 AND name = "foo", sorted index on x.
/// `x = i % 1000` -> `x >= 990` keeps 1% of records (selective range).
/// This is the realistic case Phase 2 (S3 range-AND extraction) targets:
/// sorted-index scan reads ~1% of records vs full table scan reading 100%.
fn query_s3_range_and_selective() -> ReadQuery {
    Query::from("bench_table")
        .where_(and([gte("x", 990i64), eq("name", "foo")]))
        .build()
}

/// Shape 5: AsOf(Timestamp) point read. Triggers version_at_or_before_ts
/// resolution (S1 ts-index lookup). LIMIT 1 isolates the ts resolution
/// cost from full-snapshot materialization.
fn query_s1_asof(ts_millis: u64) -> ReadQuery {
    Query::from("bench_table")
        .as_of_timestamp(ts_millis)
        .limit(1)
        .build()
}

fn main() {
    let mut h = Harness::new("read_path_matrix", env!("CARGO_MANIFEST_DIR"));

    let rt = rt();
    let sizes = table_sizes();

    for &n in &sizes {
        // ── Shape 1: fast_path (with sorted index on y) ─────────────
        {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                create_sorted_index(&f.mgr, "y_sorted", "y").await;
                f
            });
            let q = query_fast_path();
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("fast_path/{n}"), move || {
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

        // ── Shape 2: s2_no_index (no index on y) ───────────────────
        {
            let fixture = rt.block_on(build_table(n));
            let q = query_s2_no_index();
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("s2_no_index/{n}"), move || {
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

        // ── Shape 3: s2_s3_combo (WHERE + ORDER BY, no index) ──────
        {
            let fixture = rt.block_on(build_table(n));
            let q = query_s2_s3_combo();
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("s2_s3_combo/{n}"), move || {
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

        // ── Shape 4: s3_range_and (sorted index on x, range-in-AND) ─
        //
        // This is the NON-SELECTIVE case (x >= 10, 99% pass) — kept here
        // intentionally to document the "tipping point" where sorted-index
        // scan visits virtually all rows and a full-table scan would win.
        // Capped at N <= 500K so even worst-case stays wall-clock bounded;
        // the companion `s3_range_and_selective` cell (1% selectivity)
        // continues to run at the full N range — that's where Phase 2 win
        // is demonstrated.
        if n <= 500_000 {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                create_sorted_index(&f.mgr, "x_sorted", "x").await;
                f
            });
            let q = query_s3_range_and();
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("s3_range_and/{n}"), move || {
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

        // ── Shape 4b: s3_range_and_selective (1% selectivity) ──────
        {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                create_sorted_index(&f.mgr, "x_sorted", "x").await;
                f
            });
            let q = query_s3_range_and_selective();
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("s3_range_and_selective/{n}"), move || {
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

        // ── Shape 5: s1_asof (AsOf timestamp, history scan) ────────
        //
        // NOTE: this shape measures end-to-end AsOf query cost — which
        // includes full snapshot table scan after ts resolution. The
        // ts-index O(log N) lookup itself is proven by 4 unit tests in
        // `shamir-tx/src/tests/mvcc_store_tests/ts_index_tests.rs`. Skipped
        // at N >= 100K unless BENCH_READ_PATH_HUGE=1 — per-N runs are
        // minutes-scale.
        if n < 100_000 || parse_bool_env("BENCH_READ_PATH_HUGE") {
            // Capture ts BETWEEN initial inserts and history accumulation:
            // initial versions have ts < ts_millis, updated versions have
            // ts > ts_millis. AsOf(ts_millis) should return the initial
            // version for any record.
            let (fixture, ts_millis) = rt.block_on(async {
                let f = build_table_mvcc(n).await;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                accumulate_history(&f.mgr, &f.record_ids).await;
                (f, ts)
            });
            let q = query_s1_asof(ts_millis);
            let mgr = fixture.mgr.clone();
            h.bench_async(&format!("s1_asof/{n}"), move || {
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
    }

    h.run();
}
