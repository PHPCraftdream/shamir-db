//! End-to-end performance benchmarks for ShamirDb.
//!
//! All benchmarks go through the public `ShamirDb::execute(BatchRequest)`
//! path — same code that the wire dispatcher runs, just without the
//! TLS / SCRAM / msgpack envelope cost. Backend is in-memory to remove
//! disk variance; we measure engine + planner + interner + index logic.
//!
//! Each scenario where an index can apply is benchmarked **twice** —
//! once against a table without indexes (full scan / current default
//! `set` path) and once against a table with the relevant index
//! pre-created. The delta exposes both today's hot spots and the
//! ceiling that the planned optimisations should approach.
//!
//! Run:
//!   cargo bench -p shamir-db
//!   cargo bench -p shamir-db -- 'set_existing'   # filter by name

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use tokio::runtime::Runtime;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

// --------------------------------------------------------------------------
// Test-data generator — realistic-ish user records
// --------------------------------------------------------------------------
//
// Each generated record carries: stable `id`, `name` (mixed first+last
// from small pools), `email`, `age` 18..=77, `city` from 8-pool,
// pseudo-random `score` 0..1000, `active` boolean, `created_at_ns`,
// `tags` array of two strings. Enough variation to exercise interner
// growth and to make filter selectivity non-trivial.

const FIRST_NAMES: &[&str] = &[
    "Alice", "Bob", "Carol", "David", "Eve", "Frank", "Grace", "Henry",
];
const LAST_NAMES: &[&str] = &["Smith", "Jones", "Brown", "Davis", "Miller", "Wilson"];
const CITIES: &[&str] = &[
    "NYC", "LA", "SF", "Chicago", "Boston", "Seattle", "Austin", "Miami",
];
const DOMAINS: &[&str] = &["example.com", "test.org", "demo.io"];

fn gen_user(i: usize) -> JsonValue {
    json!({
        "id":            format!("u{:08}", i),
        "name":          format!(
                            "{} {}",
                            FIRST_NAMES[i % FIRST_NAMES.len()],
                            LAST_NAMES[(i / FIRST_NAMES.len()) % LAST_NAMES.len()]
                        ),
        "email":         format!("user{}@{}", i, DOMAINS[i % DOMAINS.len()]),
        "age":           18 + ((i * 37) % 60) as i64,
        "city":          CITIES[i % CITIES.len()],
        "score":         ((i * 7919) % 1000) as i64,
        "active":        i % 3 != 0,
        "created_at_ns": 1_700_000_000_000_000_000_u64 + (i as u64 * 60_000_000_000),
        "tags":          vec![
                            format!("tag_{}", i % 10),
                            format!("tag_{}", (i / 10) % 7),
                        ],
    })
}

// --------------------------------------------------------------------------
// Setup helpers
// --------------------------------------------------------------------------

async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

/// Same as `fresh_db()` but the repo is backed by a sled on-disk
/// store at `path`. Caller must keep the corresponding `TempDir`
/// alive at least as long as the returned `ShamirDb` (sled holds
/// the directory open).
async fn fresh_db_sled(path: &std::path::Path) -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg = RepoConfig::new("main", BoxRepoFactory::sled(path.to_path_buf()))
        .add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

/// Seed `n` records via a single `insert_into` op (does NOT scan).
async fn seed_users(shamir: &ShamirDb, n: usize) {
    let values: Vec<JsonValue> = (0..n).map(gen_user).collect();
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "seed",
        "queries": {
            "ins": { "insert_into": "users", "values": values }
        },
        "return_all": false
    }))
    .expect("parse seed batch");
    shamir.execute("bench", &req).await.expect("seed");
}

async fn create_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str, unique: bool) {
    create_index_inner(shamir, table, index_name, field, unique, false).await
}

async fn create_sorted_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str) {
    create_index_inner(shamir, table, index_name, field, false, true).await
}

async fn create_index_inner(
    shamir: &ShamirDb,
    table: &str,
    index_name: &str,
    field: &str,
    unique: bool,
    sorted: bool,
) {
    let req: BatchRequest = serde_json::from_value(json!({
        "id": "idx",
        "queries": {
            "i": {
                "create_index": index_name,
                "table": table,
                "fields": [[field]],
                "unique": unique,
                "sorted": sorted
            }
        }
    }))
    .expect("parse idx batch");
    shamir.execute("bench", &req).await.expect("create index");
}

/// Build a populated table; optionally with a regular (non-unique)
/// index on `id`. Regular indexes are what the read planner consumes
/// today (`try_plan_index_scan`) — unique indexes are stored separately
/// and the read planner doesn't currently look at them, so to get a
/// fair "with index" baseline we create a regular one.
async fn seeded(n: usize, with_id_index: bool) -> Arc<ShamirDb> {
    let shamir = fresh_db().await;
    seed_users(&shamir, n).await;
    if with_id_index {
        create_index(&shamir, "users", "by_id", "id", false).await;
    }
    shamir
}

// --------------------------------------------------------------------------
// Op factories — keeps the bench loops short
// --------------------------------------------------------------------------

fn req_set_one(target_id: &str, score: i64) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "s",
        "queries": {
            "s": {
                "set": "users",
                "key": { "id": target_id },
                "value": { "id": target_id, "score": score, "name": "Updated", "active": true }
            }
        },
        "return_all": false
    }))
    .unwrap()
}

fn req_read_by_id(target_id: &str) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": { "from": "users", "where": { "op": "eq", "field": ["id"], "value": target_id } }
        }
    }))
    .unwrap()
}

fn req_read_by_city(city: &str) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": { "from": "users", "where": { "op": "eq", "field": ["city"], "value": city } }
        }
    }))
    .unwrap()
}

fn req_update_by_id(target_id: &str) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "u",
        "queries": {
            "u": {
                "update": "users",
                "where": { "op": "eq", "field": ["id"], "value": target_id },
                "set": { "score": 1234 }
            }
        },
        "return_all": false
    }))
    .unwrap()
}

fn req_delete_by_id(target_id: &str) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "d",
        "queries": {
            "d": {
                "delete_from": "users",
                "where": { "op": "eq", "field": ["id"], "value": target_id }
            }
        },
        "return_all": false
    }))
    .unwrap()
}

fn req_read_complex_filter() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": {
                "from": "users",
                "where": {
                    "op": "and",
                    "filters": [
                        { "op": "gte", "field": ["age"], "value": 30 },
                        { "op": "lte", "field": ["age"], "value": 50 },
                        {
                            "op": "or",
                            "filters": [
                                { "op": "eq", "field": ["city"], "value": "NYC" },
                                { "op": "eq", "field": ["city"], "value": "LA" }
                            ]
                        }
                    ]
                }
            }
        }
    }))
    .unwrap()
}

fn req_read_with_order_limit() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": {
                "from": "users",
                "order_by": { "items": [{ "field": ["score"], "direction": "desc" }] },
                "pagination": { "mode": "LimitOffset", "limit": 10, "offset": 0 }
            }
        }
    }))
    .unwrap()
}

fn req_count_all() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "c",
        "queries": {
            "c": {
                "from": "users",
                "select": { "items": [{ "type": "count_all", "alias": "n" }] }
            }
        }
    }))
    .unwrap()
}

fn req_count_with_filter(city: &str) -> BatchRequest {
    serde_json::from_value(json!({
        "id": "c",
        "queries": {
            "c": {
                "from": "users",
                "where": { "op": "eq", "field": ["city"], "value": city },
                "select": { "items": [{ "type": "count_all", "alias": "n" }] }
            }
        }
    }))
    .unwrap()
}

fn req_min_max_score() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "mm",
        "queries": {
            "mm": {
                "from": "users",
                "select": {
                    "items": [
                        { "type": "aggregate", "func": "min", "field": ["score"], "alias": "lo" },
                        { "type": "aggregate", "func": "max", "field": ["score"], "alias": "hi" }
                    ]
                }
            }
        }
    }))
    .unwrap()
}

fn req_range_age() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": {
                "from": "users",
                "where": { "op": "between", "field": ["age"], "from": 30, "to": 35 }
            }
        }
    }))
    .unwrap()
}

/// Narrow range — ~1.6 % selectivity (one age value out of 60). Shows
/// where sorted-index wins really matter: when most records are
/// filtered out, avoiding the per-record load dominates.
fn req_range_age_narrow() -> BatchRequest {
    serde_json::from_value(json!({
        "id": "r",
        "queries": {
            "r": {
                "from": "users",
                "where": { "op": "between", "field": ["age"], "from": 30, "to": 30 }
            }
        }
    }))
    .unwrap()
}

fn req_bulk_insert(start: usize, count: usize) -> BatchRequest {
    let values: Vec<JsonValue> = (start..start + count).map(gen_user).collect();
    serde_json::from_value(json!({
        "id": "b",
        "queries": {
            "ins": { "insert_into": "users", "values": values }
        },
        "return_all": false
    }))
    .unwrap()
}

fn req_batch_independent_reads() -> BatchRequest {
    let mut queries = JsonMap::new();
    for (i, city) in CITIES.iter().enumerate() {
        queries.insert(
            format!("q{}", i),
            json!({ "from": "users", "where": { "op": "eq", "field": ["city"], "value": city } }),
        );
    }
    serde_json::from_value(json!({ "id": "multi", "queries": queries })).unwrap()
}

// --------------------------------------------------------------------------
// Benchmark groups
// --------------------------------------------------------------------------

const SIZES: &[usize] = &[100, 1_000, 10_000];

/// Bulk insert — measures pure write throughput (no scan). Each iter
/// gets a fresh empty table so insert cost is constant.
fn bench_bulk_insert(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bulk_insert");

    for &count in &[100usize, 1_000] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let shamir = fresh_db().await;
                    let req = req_bulk_insert(0, count);
                    let start = Instant::now();
                    shamir.execute("bench", &req).await.unwrap();
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

/// `set` (upsert) on existing key — currently O(n) full scan regardless
/// of indexes (optimisation B will make this O(log n)). Without index
/// is the baseline we want to beat.
///
/// Target = LAST seeded record to measure worst-case scan; the executor
/// short-circuits on first match, so picking the first record would
/// hide the O(n) cost.
fn bench_set_existing_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("set_existing_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        let target = format!("u{:08}", n - 1);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_set_one(&target, 42);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Same `set` but with a unique index on `id` pre-created. With current
/// code this changes nothing (index isn't consulted on the write path),
/// so the numbers match no-index. After optimisation B + C the times
/// here should drop dramatically.
fn bench_set_existing_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("set_existing_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, true));
        let target = format!("u{:08}", n - 1);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_set_one(&target, 42);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Read by id: the read planner already uses indexes — this should
/// already be O(log n) when index exists, O(n) otherwise. Two groups
/// to confirm the gap.
fn bench_read_by_id_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_by_id_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_read_by_id_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_by_id_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, true));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Read by city — non-PK column, lower selectivity (~12.5% records per
/// city). Run with and without an index on `city`.
fn bench_read_by_city_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_by_city_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_by_city("NYC");
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_read_by_city_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_by_city_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_by_city("NYC");
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Update by id — write path that today scans regardless of index.
/// Optimisation C will make the indexed variant fast.
fn bench_update_by_id_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("update_by_id_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_update_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_update_by_id_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("update_by_id_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, true));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_update_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Delete by id — same story as update. Delete shrinks the table, so
/// we reset state per iteration via `iter_custom` to keep N constant.
fn bench_delete_by_id_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("delete_by_id_no_index");

    for &n in SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let shamir = seeded(n, false).await;
                    let req = req_delete_by_id(&format!("u{:08}", n - 1));
                    let start = Instant::now();
                    shamir.execute("bench", &req).await.unwrap();
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_delete_by_id_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("delete_by_id_with_index");

    for &n in SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let shamir = seeded(n, true).await;
                    let req = req_delete_by_id(&format!("u{:08}", n - 1));
                    let start = Instant::now();
                    shamir.execute("bench", &req).await.unwrap();
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

/// Complex filter (AND of nested OR over indexed + non-indexed
/// columns). Tests planner cost on real-shaped queries.
fn bench_complex_filter(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("complex_filter");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_complex_filter();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Order_by + limit — full scan + sort.
fn bench_order_limit(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("order_limit_top10");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_with_order_limit();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// COUNT(*) without filter — Opt #2: should fast-path through
/// RecordCounter (O(1)) instead of a full scan.
fn bench_count_all_no_filter(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("count_all_no_filter");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_all();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

/// COUNT(*) with eq filter — eligible for index-lookup fast path
/// (count = matched_set.len() without materialising records).
fn bench_count_with_filter_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("count_with_filter_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_with_filter("NYC");
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

fn bench_count_with_filter_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("count_with_filter_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_with_filter("NYC");
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

/// MIN(score) + MAX(score) over the whole table — Opt #4 should walk
/// the score index (first / last key) instead of scanning everything.
fn bench_min_max_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("min_max_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_max_score();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

fn bench_min_max_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("min_max_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_max_score();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

/// `order_by score desc + LIMIT 10` on indexed score field — Opt #1
/// can read the index in order and stop after K matches.
fn bench_order_limit_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("order_limit_top10_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_with_order_limit();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

/// `where age between 30 AND 35` — narrow range, ~5 % selectivity.
/// Opt #5 should make this O(log N + K) via sorted-index range scan.
fn bench_range_query_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_no_index");

    for &n in SIZES {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

fn bench_range_query_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_with_index");

    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            // Sorted index for range queries — equality (hash) index
            // wouldn't help here.
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
    }
    group.finish();
}

// --------------------------------------------------------------------------
// Sled-backed range bench — exercises the native `iter_range_stream`
// path on a real disk backend. Contrast with `range_query_*` (above)
// which run against `in_memory` where the default O(N) fallback is
// used. Same scenarios, different backend, fair before/after picture
// of what the sorted-index work actually buys in production.
// --------------------------------------------------------------------------

fn bench_range_query_no_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_no_index_sled");

    for &n in SIZES {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users(&s, n).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
        // Drop shamir before tempdir so sled releases the directory.
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

fn bench_range_query_with_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_with_index_sled");

    for &n in SIZES {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users(&s, n).await;
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// Narrow range on sled — shows where sorted-index gives the biggest
/// payoff: low selectivity means we avoid most per-record loads.
fn bench_range_query_narrow_no_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_narrow_no_index_sled");

    for &n in SIZES {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users(&s, n).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age_narrow();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

fn bench_range_query_narrow_with_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_narrow_with_index_sled");

    for &n in SIZES {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users(&s, n).await;
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age_narrow();
                async move { shamir.execute("bench", &req).await.unwrap(); }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// 8 independent reads in a single batch — exercises the parallel
/// stage planner.
fn bench_batch_multi_read(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("batch_multi_read_8");

    for &n in &[1_000usize, 10_000] {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_batch_independent_reads();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

// --------------------------------------------------------------------------
// Driver
// --------------------------------------------------------------------------

criterion_group!(
    benches,
    bench_bulk_insert,
    bench_set_existing_no_index,
    bench_set_existing_with_index,
    bench_read_by_id_no_index,
    bench_read_by_id_with_index,
    bench_read_by_city_no_index,
    bench_read_by_city_with_index,
    bench_update_by_id_no_index,
    bench_update_by_id_with_index,
    bench_delete_by_id_no_index,
    bench_delete_by_id_with_index,
    bench_complex_filter,
    bench_order_limit,
    bench_count_all_no_filter,
    bench_count_with_filter_no_index,
    bench_count_with_filter_with_index,
    bench_min_max_no_index,
    bench_min_max_with_index,
    bench_order_limit_with_index,
    bench_range_query_no_index,
    bench_range_query_with_index,
    bench_range_query_no_index_sled,
    bench_range_query_with_index_sled,
    bench_range_query_narrow_no_index_sled,
    bench_range_query_narrow_with_index_sled,
    bench_batch_multi_read,
);
criterion_main!(benches);
