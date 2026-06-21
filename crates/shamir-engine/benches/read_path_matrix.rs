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

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use shamir_bench_utils as bu;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::ReadQuery;
use shamir_engine::table::table_manager::TableManager;
use shamir_query_builder::ddl;
use shamir_query_builder::filter::{and, eq, gt, gte};
use shamir_query_builder::query::Query;
use shamir_query_types::batch::BatchOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ── Configuration ───────────────────────────────────────────────────────

fn table_sizes() -> Vec<usize> {
    let mut sizes = vec![10_000, 100_000, 1_000_000];
    if std::env::var("BENCH_READ_PATH_HUGE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
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
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("bench_table".into(), data, info)
        .await
        .unwrap();

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
async fn create_sorted_index(mgr: &TableManager, name: &str, field: &str) {
    let op = ddl::create_index(name, "bench_table")
        .field(field)
        .sorted()
        .build();
    let create_op = match op {
        BatchOp::CreateIndex(o) => o,
        other => panic!("expected BatchOp::CreateIndex, got {other:?}"),
    };
    mgr.create_index_v2(&create_op).await.unwrap();
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
/// Planner currently ignores range-in-AND -> full scan (S3 trigger).
fn query_s3_range_and() -> ReadQuery {
    Query::from("bench_table")
        .where_(and([gte("x", 10i64), eq("name", "foo")]))
        .build()
}

/// Shape 5: AsOf(Timestamp) point read. Triggers version_at_or_before_ts
/// full history scan (S1).
fn query_s1_asof(ts_millis: u64) -> ReadQuery {
    Query::from("bench_table")
        .as_of_timestamp(ts_millis)
        .build()
}

// ── Bench runner ────────────────────────────────────────────────────────

fn bench_read_path_matrix(c: &mut Criterion) {
    let rt = rt();
    let sizes = table_sizes();

    let mut group = c.benchmark_group("read_path_matrix");
    bu::tune(&mut group, 10, 1, 1);

    for &n in &sizes {
        // ── Shape 1: fast_path (with sorted index on y) ─────────────
        {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                create_sorted_index(&f.mgr, "y_sorted", "y").await;
                f
            });
            let q = query_fast_path();

            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(BenchmarkId::new("fast_path", n), &n, |b, _| {
                b.to_async(&rt).iter(|| {
                    let mgr = fixture.mgr.clone();
                    let q = q.clone();
                    async move {
                        let interner = mgr.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        black_box(mgr.read(&q, &ctx).await.unwrap());
                    }
                });
            });
        }

        // ── Shape 2: s2_no_index (no index on y) ───────────────────
        {
            let fixture = rt.block_on(build_table(n));
            let q = query_s2_no_index();

            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(BenchmarkId::new("s2_no_index", n), &n, |b, _| {
                b.to_async(&rt).iter(|| {
                    let mgr = fixture.mgr.clone();
                    let q = q.clone();
                    async move {
                        let interner = mgr.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        black_box(mgr.read(&q, &ctx).await.unwrap());
                    }
                });
            });
        }

        // ── Shape 3: s2_s3_combo (WHERE + ORDER BY, no index) ──────
        {
            let fixture = rt.block_on(build_table(n));
            let q = query_s2_s3_combo();

            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(BenchmarkId::new("s2_s3_combo", n), &n, |b, _| {
                b.to_async(&rt).iter(|| {
                    let mgr = fixture.mgr.clone();
                    let q = q.clone();
                    async move {
                        let interner = mgr.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        black_box(mgr.read(&q, &ctx).await.unwrap());
                    }
                });
            });
        }

        // ── Shape 4: s3_range_and (sorted index on x, range-in-AND) ─
        {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                create_sorted_index(&f.mgr, "x_sorted", "x").await;
                f
            });
            let q = query_s3_range_and();

            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(BenchmarkId::new("s3_range_and", n), &n, |b, _| {
                b.to_async(&rt).iter(|| {
                    let mgr = fixture.mgr.clone();
                    let q = q.clone();
                    async move {
                        let interner = mgr.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        black_box(mgr.read(&q, &ctx).await.unwrap());
                    }
                });
            });
        }

        // ── Shape 5: s1_asof (AsOf timestamp, history scan) ────────
        {
            let fixture = rt.block_on(async {
                let f = build_table(n).await;
                accumulate_history(&f.mgr, &f.record_ids).await;
                f
            });
            // Use a timestamp slightly in the past to force history scan.
            // We pick "now - 1 second" to ensure it's before the updates.
            let ts_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
                - 1000;
            let q = query_s1_asof(ts_millis);

            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(BenchmarkId::new("s1_asof", n), &n, |b, _| {
                b.to_async(&rt).iter(|| {
                    let mgr = fixture.mgr.clone();
                    let q = q.clone();
                    async move {
                        let interner = mgr.interner().get().await.unwrap();
                        let refs = new_map();
                        let ctx = FilterContext::new(interner, &refs);
                        black_box(mgr.read(&q, &ctx).await.unwrap());
                    }
                });
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_read_path_matrix);
criterion_main!(benches);
