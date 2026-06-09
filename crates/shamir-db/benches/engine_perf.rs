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
use serde_json::{json, Value as JsonValue};
use tokio::runtime::Runtime;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_collections::new_map;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::filter::{self as f};
use shamir_query_builder::query::Query;
use shamir_query_builder::select;
use shamir_query_builder::write;

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
        "active":        !i.is_multiple_of(3),
        "created_at_ns": 1_700_000_000_000_000_000_u64 + (i as u64 * 60_000_000_000),
        "tags":          vec![
                            format!("tag_{}", i % 10),
                            format!("tag_{}", (i / 10) % 7),
                        ],
    })
}

/// Wide record generator — same core fields as `gen_user` PLUS ~20 extra
/// string/numeric fields. Total payload is ~5-10x larger than a narrow
/// record. Used to isolate "decode K wide records after index-lookup" cost
/// and to determine whether a covering index (Opt O) would yield a
/// significant additional win beyond the current index+get_many path.
///
/// Extra fields:
///   bio          — ~200-char free-text string
///   field_00..15 — alternating string/number extras
///   notes        — ~100-char string
///   priority     — integer 0..9
///   department   — one of 8 strings
fn gen_user_wide(i: usize) -> JsonValue {
    const DEPARTMENTS: &[&str] = &[
        "Engineering",
        "Marketing",
        "Sales",
        "Support",
        "Finance",
        "Legal",
        "HR",
        "Operations",
    ];
    // ~200-char bio constructed from a repeating pattern seeded by i.
    let bio = format!(
        "User {} biography: works in the {} department. \
         Joined in {}. Specialises in area {}. \
         Contact via user{}@{} for further details.",
        i,
        DEPARTMENTS[i % DEPARTMENTS.len()],
        2000 + (i % 24),
        i % 17,
        i,
        DOMAINS[i % DOMAINS.len()],
    );
    let notes = format!(
        "Internal note #{}: last reviewed by manager {} on day {}.",
        i,
        FIRST_NAMES[i % FIRST_NAMES.len()],
        i % 365,
    );
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
        "active":        !i.is_multiple_of(3),
        "created_at_ns": 1_700_000_000_000_000_000_u64 + (i as u64 * 60_000_000_000),
        "tags":          vec![
                            format!("tag_{}", i % 10),
                            format!("tag_{}", (i / 10) % 7),
                        ],
        // --- extra wide fields ---
        "bio":           bio,
        "notes":         notes,
        "department":    DEPARTMENTS[i % DEPARTMENTS.len()],
        "priority":      (i % 10) as i64,
        "field_00":      format!("extra_str_field_zero_{}", i),
        "field_01":      ((i * 3) % 10_000) as i64,
        "field_02":      format!("extra_str_field_two_{}", i * 2),
        "field_03":      ((i * 5) % 10_000) as i64,
        "field_04":      format!("extra_str_field_four_{}", i),
        "field_05":      ((i * 7) % 10_000) as i64,
        "field_06":      format!("extra_str_field_six_{}", i),
        "field_07":      ((i * 11) % 10_000) as i64,
        "field_08":      format!("extra_str_field_eight_{}", i),
        "field_09":      ((i * 13) % 10_000) as i64,
        "field_10":      format!("extra_str_field_ten_{}", i),
        "field_11":      ((i * 17) % 10_000) as i64,
        "field_12":      format!("extra_str_field_twelve_{}", i),
        "field_13":      ((i * 19) % 10_000) as i64,
        "field_14":      format!("extra_str_field_fourteen_{}", i),
        "field_15":      ((i * 23) % 10_000) as i64,
    })
}

// --------------------------------------------------------------------------
// Setup helpers
// --------------------------------------------------------------------------

async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

/// Same as `fresh_db()` but the repo is backed by a sled on-disk
/// store at `path`. Caller must keep the corresponding `TempDir`
/// alive at least as long as the returned `ShamirDb` (sled holds
/// the directory open).
async fn fresh_db_sled(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::sled(path.to_path_buf())).await
}

async fn fresh_db_redb(path: &std::path::Path) -> Arc<ShamirDb> {
    // redb expects a single-file path; tempdir gives us a directory.
    fresh_db_with(BoxRepoFactory::redb(path.join("db.redb"))).await
}

async fn fresh_db_persy(path: &std::path::Path) -> Arc<ShamirDb> {
    // Same for persy.
    fresh_db_with(BoxRepoFactory::persy(path.join("db.persy"))).await
}

async fn fresh_db_canopy(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::canopy(path.to_path_buf())).await
}

async fn fresh_db_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::fjall(path.to_path_buf())).await
}

async fn fresh_db_nebari(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::nebari(path.to_path_buf())).await
}

async fn fresh_db_with(factory: BoxRepoFactory) -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg = RepoConfig::new("main", factory).add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

// MemBuffer-wrapped variants — measure the wrapper overhead.
// Today MemBufferStore is a passthrough proxy, so these should
// produce numbers indistinguishable from the raw backend.
async fn fresh_db_membuffer_in_memory() -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::in_memory(),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_sled(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::sled(path.to_path_buf()),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_redb(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::redb(path.join("db.redb")),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_persy(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::persy(path.join("db.persy")),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_canopy(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::canopy(path.to_path_buf()),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::fjall(path.to_path_buf()),
        MemBufferConfig::default(),
    ))
    .await
}

async fn fresh_db_membuffer_nebari(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::nebari(path.to_path_buf()),
        MemBufferConfig::default(),
    ))
    .await
}

/// Seed `n` records via a single `insert_into` op (does NOT scan).
async fn seed_users(shamir: &ShamirDb, n: usize) {
    let values: Vec<JsonValue> = (0..n).map(gen_user).collect();
    let mut b = Batch::new();
    b.id("seed")
        .return_flagged()
        .insert("ins", write::insert("users").rows(values));
    let req = b.build();
    shamir.execute("bench", &req).await.expect("seed");
}

async fn create_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str, unique: bool) {
    create_index_inner(shamir, table, index_name, field, unique, false).await
}

async fn create_sorted_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str) {
    create_index_inner(shamir, table, index_name, field, false, true).await
}

/// Like `create_sorted_index` but also sets `include: [[include_field]]` so
/// the index stores the projected value inline. A query that SELECTs only the
/// included fields can be answered entirely from the index (A3 covering path)
/// without fetching or decoding the full record.
async fn create_covering_sorted_index(
    shamir: &ShamirDb,
    table: &str,
    index_name: &str,
    field: &str,
    include_field: &str,
) {
    let idx = ddl::create_index(index_name, table)
        .field(field)
        .sorted()
        .include([vec![include_field.to_string()]])
        .build();
    let mut b = Batch::new();
    b.id("idx").create_index("i", idx);
    let req = b.build();
    shamir
        .execute("bench", &req)
        .await
        .expect("create covering index");
}

async fn create_index_inner(
    shamir: &ShamirDb,
    table: &str,
    index_name: &str,
    field: &str,
    unique: bool,
    sorted: bool,
) {
    let mut idx = ddl::create_index(index_name, table).field(field);
    if unique {
        idx = idx.unique();
    }
    if sorted {
        idx = idx.sorted();
    }
    let mut b = Batch::new();
    b.id("idx").create_index("i", idx.build());
    let req = b.build();
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
    let mut b = Batch::new();
    b.id("s").return_flagged().upsert(
        "s",
        write::upsert("users")
            .key(json!({ "id": target_id }))
            .value(json!({ "id": target_id, "score": score, "name": "Updated", "active": true })),
    );
    b.build()
}

fn req_read_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_eq("id", target_id));
    b.build()
}

fn req_read_by_city(city: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_eq("city", city));
    b.build()
}

fn req_update_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("u").return_flagged().update(
        "u",
        write::update("users")
            .where_(f::eq("id", target_id))
            .set(json!({ "score": 1234 })),
    );
    b.build()
}

fn req_delete_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("d")
        .return_flagged()
        .delete("d", write::delete("users").where_(f::eq("id", target_id)));
    b.build()
}

fn req_read_complex_filter() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r").query(
        "r",
        Query::from("users")
            .where_gte("age", 30)
            .where_lte("age", 50)
            .where_group_or(|conds| conds.where_eq("city", "NYC").where_eq("city", "LA")),
    );
    b.build()
}

fn req_read_with_order_limit() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").order_by_desc("score").limit(10));
    b.build()
}

fn req_read_with_order_limit_asc() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").order_by_asc("score").limit(10));
    b.build()
}

fn req_count_all() -> BatchRequest {
    let mut b = Batch::new();
    b.id("c")
        .query("c", Query::from("users").select([select::count_all("n")]));
    b.build()
}

fn req_count_with_filter(city: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("c").query(
        "c",
        Query::from("users")
            .where_eq("city", city)
            .select([select::count_all("n")]),
    );
    b.build()
}

fn req_min_max_score() -> BatchRequest {
    let mut b = Batch::new();
    b.id("mm").query(
        "mm",
        Query::from("users").select([select::min("score", "lo"), select::max("score", "hi")]),
    );
    b.build()
}

/// MIN(score) ONLY — eligible for Q1 sorted-index fast-path
/// (single aggregate, no other select items, no filter/order/etc).
fn req_min_score() -> BatchRequest {
    let mut b = Batch::new();
    b.id("m").query(
        "m",
        Query::from("users").select([select::min("score", "lo")]),
    );
    b.build()
}

fn req_range_age() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_between("age", 30, 35));
    b.build()
}

/// Narrow range — ~1.6 % selectivity (one age value out of 60). Shows
/// where sorted-index wins really matter: when most records are
/// filtered out, avoiding the per-record load dominates.
fn req_range_age_narrow() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_between("age", 30, 30));
    b.build()
}

fn req_bulk_insert(start: usize, count: usize) -> BatchRequest {
    let values: Vec<JsonValue> = (start..start + count).map(gen_user).collect();
    let mut b = Batch::new();
    b.id("b")
        .return_flagged()
        .insert("ins", write::insert("users").rows(values));
    b.build()
}

fn req_batch_independent_reads() -> BatchRequest {
    let mut b = Batch::new();
    b.id("multi");
    for (i, city) in CITIES.iter().enumerate() {
        b.query(
            format!("q{}", i),
            Query::from("users").where_eq("city", *city),
        );
    }
    b.build()
}

// --------------------------------------------------------------------------
// Benchmark groups
// --------------------------------------------------------------------------

/// `BENCH_QUICK=1` switches every group to a fast-feedback regime:
///   * sample_size = 10 (criterion minimum),
///   * measurement_time = 1 s,
///   * dataset sizes trimmed to a single representative point.
///
/// Cuts the full bench RUN from ~6 min to ~1.5 min. Acceptable for
/// iterative perf work; for publishable numbers run without the
/// env var.
fn quick() -> bool {
    std::env::var_os("BENCH_QUICK").is_some()
}

/// Dataset-size sweep — full when default, single point in quick mode.
fn sweep_sizes() -> &'static [usize] {
    if quick() {
        &[1_000]
    } else {
        SIZES
    }
}

/// Bulk-insert sweep for disk-backed bench groups. Each iter
/// creates a fresh tempdir + opens the DB, so even the smaller
/// sizes pay a ~1 s overhead per sample. In quick mode we drop
/// to a single representative point.
fn bulk_sweep() -> &'static [usize] {
    if quick() {
        &[100]
    } else {
        &[100, 1_000]
    }
}

const SIZES: &[usize] = &[100, 1_000, 10_000];

/// Bulk insert — measures pure write throughput (no scan). Each iter
/// gets a fresh empty table so insert cost is constant.
fn bench_bulk_insert(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bulk_insert");

    for &count in bulk_sweep() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
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

    for &n in sweep_sizes() {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_all();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_with_filter("NYC");
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_count_with_filter_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("count_with_filter_with_index");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_count_with_filter("NYC");
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_max_score();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_min_max_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("min_max_with_index");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_max_score();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Q1 baseline: MIN alone WITHOUT sorted index — falls through to
/// full scan + aggregate.
fn bench_min_only_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("min_only_no_index");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_score();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Q1 fast-path: MIN alone with sorted index on `score`. Should hit
/// the lookup_min path in read(), O(log n).
fn bench_min_only_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("min_only_with_index");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_min_score();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

/// Same DESC ORDER BY LIMIT 10 path, but on a SLED-backed repo.
/// Exercises sled's native `iter_range_stream_reverse` cursor —
/// O(log N + K) — vs the default in-memory impl which collects
/// forward and reverses in memory (O(N)).
fn bench_order_limit_desc_with_sorted_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("order_limit_top10_desc_sorted_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users(&s, n).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_with_order_limit();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// `order_by score DESC + LIMIT 10` with a SORTED index on `score`.
/// Hits the reverse-iter fast path — `lookup_last_k(index, 10)`
/// using `Store::iter_range_stream_reverse` instead of full scan
/// + sort.
fn bench_order_limit_desc_with_sorted_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("order_limit_top10_desc_sorted");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
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

/// `order_by score ASC + LIMIT 10` with a SORTED index on `score`.
/// Hits the Opt #6 fast path — `lookup_first_k(index, 10)` instead
/// of full scan + sort. Companion to the existing
/// `order_limit_top10` (which is DESC and falls through to full
/// scan + sort by design).
fn bench_order_limit_asc_with_sorted_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("order_limit_top10_asc_sorted");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_read_with_order_limit_asc();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
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

/// `where age between 30 AND 35` — narrow range, ~5 % selectivity.
/// Opt #5 should make this O(log N + K) via sorted-index range scan.
fn bench_range_query_no_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_no_index");

    for &n in sweep_sizes() {
        let shamir = rt.block_on(seeded(n, false));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
    }
    group.finish();
}

fn bench_range_query_with_index(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_with_index");

    for &n in sweep_sizes() {
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
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

/// Bulk insert on sled — exercises the write-path of a real disk
/// backend. Sample counts kept low (each iter creates a fresh
/// tempdir and does N inserts on a disk-backed tree, which is slow
/// when every write fsyncs).
fn bench_bulk_insert_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bulk_insert_sled");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(8));

    for &count in bulk_sweep() {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let tempdir = tempfile::TempDir::new().expect("tempdir");
                    let shamir = fresh_db_sled(tempdir.path()).await;
                    let req = req_bulk_insert(0, count);
                    let start = Instant::now();
                    shamir.execute("bench", &req).await.unwrap();
                    total += start.elapsed();
                    drop(shamir);
                    drop(tempdir);
                }
                total
            });
        });
    }
    group.finish();
}

/// Same `bulk_insert` for every disk backend. Used as a parity
/// check — each backend should converge to a similar
/// "amortised-fsync, no per-write commit" cost. Sample counts kept
/// low (each iter spawns a fresh tempdir + DB).
macro_rules! bench_bulk_insert_for_backend {
    ($fn_name:ident, $group:literal, $fresh:ident) => {
        fn $fn_name(c: &mut Criterion) {
            let rt = Runtime::new().unwrap();
            let mut group = c.benchmark_group($group);
            group.sample_size(10);
            group.measurement_time(Duration::from_secs(10));
            for &count in bulk_sweep() {
                group.throughput(Throughput::Elements(count as u64));
                group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
                    b.to_async(&rt).iter_custom(|iters| async move {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            let tempdir = tempfile::TempDir::new().expect("tempdir");
                            let shamir = $fresh(tempdir.path()).await;
                            let req = req_bulk_insert(0, count);
                            let start = Instant::now();
                            shamir.execute("bench", &req).await.unwrap();
                            total += start.elapsed();
                            drop(shamir);
                            drop(tempdir);
                        }
                        total
                    });
                });
            }
            group.finish();
        }
    };
}

bench_bulk_insert_for_backend!(bench_bulk_insert_redb, "bulk_insert_redb", fresh_db_redb);
bench_bulk_insert_for_backend!(bench_bulk_insert_persy, "bulk_insert_persy", fresh_db_persy);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_canopy,
    "bulk_insert_canopy",
    fresh_db_canopy
);
bench_bulk_insert_for_backend!(bench_bulk_insert_fjall, "bulk_insert_fjall", fresh_db_fjall);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_nebari,
    "bulk_insert_nebari",
    fresh_db_nebari
);

// MemBuffer-wrapped variants. Same backends, same numbers as raw
// for the passthrough proxy phase. Once the LRU + flusher ship,
// expect: persy/nebari/canopy biggest win; sled/redb/fjall
// near-noise.
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_sled,
    "bulk_insert_membuffer_sled",
    fresh_db_membuffer_sled
);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_redb,
    "bulk_insert_membuffer_redb",
    fresh_db_membuffer_redb
);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_persy,
    "bulk_insert_membuffer_persy",
    fresh_db_membuffer_persy
);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_canopy,
    "bulk_insert_membuffer_canopy",
    fresh_db_membuffer_canopy
);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_fjall,
    "bulk_insert_membuffer_fjall",
    fresh_db_membuffer_fjall
);
bench_bulk_insert_for_backend!(
    bench_bulk_insert_membuffer_nebari,
    "bulk_insert_membuffer_nebari",
    fresh_db_membuffer_nebari
);

// Membuffer-only bench using the macro shape: in-memory backend
// wrapped — measures pure wrapper overhead (no I/O).
fn bench_bulk_insert_membuffer_in_memory(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bulk_insert_membuffer_in_memory");
    for &count in bulk_sweep() {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let shamir = fresh_db_membuffer_in_memory().await;
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

/// Same as `bulk_insert_sled` but with a regular index on the
/// `city` field (cardinality 8 → high-fanout posting lists).
/// Exposes the cost of index posting-list updates per insert.
fn bench_bulk_insert_with_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bulk_insert_with_index_sled");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));

    for &count in bulk_sweep() {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let tempdir = tempfile::TempDir::new().expect("tempdir");
                    let shamir = fresh_db_sled(tempdir.path()).await;
                    create_index(&shamir, "users", "by_city", "city", false).await;
                    let req = req_bulk_insert(0, count);
                    let start = Instant::now();
                    shamir.execute("bench", &req).await.unwrap();
                    total += start.elapsed();
                    drop(shamir);
                    drop(tempdir);
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_range_query_no_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_no_index_sled");

    for &n in sweep_sizes() {
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
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
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
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
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
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
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

    for &n in sweep_sizes() {
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
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

// ═══════════════════════════════════════════════════════════════════
// Wide-record range query — covering-index decision benchmark
//
// Purpose: isolate "decode K wide records after index-lookup" cost
// so we can decide whether a covering index (Opt O) is worth
// implementing. A covering index would avoid fetching/decoding full
// records for narrow projections (e.g. SELECT age only).
//
// Two companion groups:
//   range_query_wide_narrow_no_index_sled  — full table scan on wide records
//   range_query_wide_narrow_with_index_sled — sorted index on age, narrow range
//
// The query uses age=30 (between 30..30, ~1.6% selectivity). The
// SELECT clause requests only ["age"] (narrow projection). This is
// the best-case for a covering index; if decoding is still a large
// share of total time even with the index, Opt O is worth pursuing.
//
// NOTE: if the engine does not currently push the narrow projection
// down to the storage layer, it will still decode all fields — which
// is exactly the upper-bound on covering-index benefit we want to measure.
// ═══════════════════════════════════════════════════════════════════

/// Narrow projection on wide records: SELECT age WHERE age BETWEEN 30 AND 30.
/// Only ~1.6% of records match. The narrow select is the ideal covering-index
/// target; without covering index the engine decodes all fields regardless.
fn req_range_age_narrow_wide() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r").query(
        "r",
        Query::from("users")
            .where_between("age", 30, 30)
            .select([select::field("age")]),
    );
    b.build()
}

/// Full projection on wide records (no select clause): WHERE age BETWEEN 30 AND 30.
/// Used as the baseline that covering index would improve upon.
fn req_range_age_narrow_wide_full() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_between("age", 30, 30));
    b.build()
}

/// Wide-record range query WITHOUT index. Full table scan decoding all
/// wide records (~30 fields each). Establishes the ceiling cost of
/// decode on a wide schema. Compare to _with_index to see index savings.
fn bench_range_query_wide_narrow_no_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_wide_narrow_no_index_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users_wide(&s, n).await;
            s
        });
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                // Full projection — engine decodes every field of every wide record.
                let req = req_range_age_narrow_wide_full();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// Wide-record range query WITH sorted index on age + narrow SELECT ["age"].
/// Index prunes the scan to ~1.6% of records; the engine then fetches and
/// decodes only the matched records (still full wide records at the storage
/// layer — no covering index yet). Time here = index-lookup cost + decode
/// cost of ~N*0.016 wide records. If this is still large relative to the
/// no-index baseline, the "decode K wide records" term dominates and a
/// covering index (Opt O) would yield a meaningful further win.
fn bench_range_query_wide_narrow_with_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_wide_narrow_with_index_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users_wide(&s, n).await;
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                // Narrow SELECT ["age"] — what covering index would make O(1) decode.
                let req = req_range_age_narrow_wide();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// Wide-record range query WITH a **covering** sorted index on age + narrow
/// SELECT ["age"]. The index stores the `age` value inline (`include: [["age"]]`),
/// so the engine can answer the query entirely from the index entry without
/// fetching or decoding the full wide record (A3 covering index-only path).
/// Compare against `bench_range_query_wide_narrow_with_index_sled` (non-covering)
/// to isolate the decode-wide-record cost that Opt O is designed to eliminate.
fn bench_range_query_wide_narrow_with_covering_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_wide_narrow_with_covering_index_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users_wide(&s, n).await;
            create_covering_sorted_index(&s, "users", "by_age_cov", "age", "age").await;
            s
        });
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                // Narrow SELECT ["age"] — covered by the index; A3 serves it index-only.
                let req = req_range_age_narrow_wide();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// High-K narrow projection: SELECT age WHERE age BETWEEN 18 AND 77.
/// `age = 18 + (i*37)%60` ∈ [18,77], so this matches ~100% of rows. This
/// is the regime where a covering index should pay off: the result set is
/// large, every matched record is wide, but only `age` is projected — so
/// avoiding the fetch+decode of the full record for every row is a big win.
fn req_range_age_highk_wide() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r").query(
        "r",
        Query::from("users")
            .where_between("age", 18, 77)
            .select([select::field("age")]),
    );
    b.build()
}

/// High-K, NON-covering sorted index on age + narrow SELECT ["age"]. The
/// range matches ~all rows, so the engine fetches and fully decodes every
/// wide record just to project `age". Baseline for the covering comparison.
fn bench_range_query_wide_highk_with_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_wide_highk_with_index_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users_wide(&s, n).await;
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age_highk_wide();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// High-K, COVERING sorted index on age include age + narrow SELECT ["age"].
/// The range matches ~all rows; A3 serves each from the index posting's
/// projection without fetching/decoding the wide record. Compare against
/// `bench_range_query_wide_highk_with_index_sled` to quantify the covering
/// win in its intended large-result-set / narrow-projection regime.
fn bench_range_query_wide_highk_with_covering_index_sled(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("range_query_wide_highk_with_covering_index_sled");

    for &n in sweep_sizes() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let shamir = rt.block_on(async {
            let s = fresh_db_sled(tempdir.path()).await;
            seed_users_wide(&s, n).await;
            create_covering_sorted_index(&s, "users", "by_age_cov", "age", "age").await;
            s
        });
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let shamir = Arc::clone(&shamir);
                let req = req_range_age_highk_wide();
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            });
        });
        drop(shamir);
        drop(tempdir);
    }
    group.finish();
}

/// Steady-state throughput: 10 000 inserts in one batch into a
/// fresh MemBuffer-wrapped DB. Long enough that the flusher
/// engages and the LRU is well past its warmup. Contrast with
/// `bulk_insert*/100` which captures startup latency.
fn bench_steady_state_insert(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("steady_state_insert_10k");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    group.throughput(Throughput::Elements(10_000));

    group.bench_function("membuffer_in_memory", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let shamir = fresh_db_membuffer_in_memory().await;
                let req = req_bulk_insert(0, 10_000);
                let start = Instant::now();
                shamir.execute("bench", &req).await.unwrap();
                total += start.elapsed();
                drop(shamir);
            }
            total
        });
    });

    group.bench_function("membuffer_sled", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let shamir = fresh_db_membuffer_sled(tempdir.path()).await;
                let req = req_bulk_insert(0, 10_000);
                let start = Instant::now();
                shamir.execute("bench", &req).await.unwrap();
                total += start.elapsed();
                drop(shamir);
                drop(tempdir);
            }
            total
        });
    });

    group.finish();
}

/// TTL sweep cost: one full TTL-eviction pass over a cache
/// holding 50k stale entries. Measures how long the cache lock
/// is blocked when TTL sweep runs.
///
/// Caveat: relies on the background flusher firing the sweep.
/// We instead measure indirectly via a `flush().await` (which
/// drains the dirty queue + triggers downstream propagation).
/// The actual sweep is internal — this bench captures the
/// observable latency when TTL is enabled vs disabled.
fn bench_ttl_sweep_50k(c: &mut Criterion) {
    use shamir_db::storage::storage_in_memory::InMemoryStore;
    use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
    use shamir_db::storage::types::{RecordKey, Store};
    use shamir_types::types::record_id::RecordId;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("ttl_sweep");
    group.sample_size(10);
    group.throughput(Throughput::Elements(50_000));

    // No TTL — baseline for "what does a cache lookup look like"
    // when sweep does not run at all.
    group.bench_function("no_ttl_seed_50k", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = RecordKey::copy_from_slice(&[0xAAu8; 80]);
                let start = Instant::now();
                for _ in 0..50_000 {
                    let id = RecordId::new();
                    let k = RecordKey::copy_from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                total += start.elapsed();
                drop(store);
            }
            total
        });
    });

    // TTL enabled, very short: every entry is stale by the time
    // the flusher tick fires. Measures the cost of inserting
    // 50k while a sweep is potentially racing.
    group.bench_function("ttl_50ms_seed_50k_flush_300ms", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: Some(50),
                    flush_interval_ms: 300,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = RecordKey::copy_from_slice(&[0xAAu8; 80]);
                let start = Instant::now();
                for _ in 0..50_000 {
                    let id = RecordId::new();
                    let k = RecordKey::copy_from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                total += start.elapsed();
                drop(store);
            }
            total
        });
    });

    group.finish();
}

/// Eviction under byte-pressure: cache is intentionally held
/// near its `max_bytes` cap so every new insert triggers the
/// byte-cap eviction loop in `cache_put`. Each iter does 1000
/// writes; the byte-cap loop pops + (maybe-)flushes one LRU
/// entry per write.
///
/// Inner store = InMemoryStore to isolate the eviction-loop
/// cost from disk I/O.
fn bench_eviction_byte_pressure(c: &mut Criterion) {
    use shamir_db::storage::storage_in_memory::InMemoryStore;
    use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
    use shamir_db::storage::types::{RecordKey, Store};
    use shamir_types::types::record_id::RecordId;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("eviction_byte_pressure");
    group.sample_size(10);
    group.throughput(Throughput::Elements(1_000));

    group.bench_function("seed_8k_then_insert_1k", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                // ~100 bytes/value × 8000 = ~800k cache footprint.
                // max_bytes 1_000_000 leaves ~200k headroom — first
                // new insert puts us over; eviction kicks in.
                let cfg = MemBufferConfig {
                    max_bytes: 1_000_000,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000, // effectively no auto-flush
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v: shamir_db::storage::types::RecordKey =
                    shamir_db::storage::types::RecordKey::copy_from_slice(&[0xAAu8; 80]);
                for _ in 0..8_000 {
                    let id = RecordId::new();
                    let k = RecordKey::copy_from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                // Now seeded near cap. Time the next 1000 writes.
                let start = Instant::now();
                for _ in 0..1_000 {
                    let id = RecordId::new();
                    let k = RecordKey::copy_from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                total += start.elapsed();
                drop(store);
            }
            total
        });
    });

    group.finish();
}

/// High-QPS WAL: 1000 batches × 1 record each, against a fresh
/// MemBuffer-wrapped DB. Each batch goes through
/// `wal.begin` (info_store.set marker) -> data write -> counter
/// update -> `wal.commit_async` (spawn task to remove marker).
///
/// Measures: per-batch overhead at high QPS, including spawn
/// cost of commit_async. If `tokio::spawn` allocation dominates,
/// switching to a long-lived drainer + channel would show here.
fn bench_wal_high_qps(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("wal_high_qps");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));
    group.throughput(Throughput::Elements(1_000));

    group.bench_function("1000_single_record_batches", |b| {
        b.to_async(&rt).iter_custom(|iters| async move {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let shamir = fresh_db_membuffer_in_memory().await;
                let req = req_bulk_insert(0, 1);
                let start = Instant::now();
                for _ in 0..1_000 {
                    shamir.execute("bench", &req).await.unwrap();
                }
                total += start.elapsed();
                drop(shamir);
            }
            total
        });
    });

    group.finish();
}

/// Low-level micro: cost of one `MemBufferStore::get` on a warm
/// cache (100 % hit, random key). Bypasses engine, planner,
/// interner — measures pure cache-lookup path.
///
/// Used as a stable signal for LRU-lookup optimisations
/// (sharded cache, lockless dirty queue, etc).
fn bench_cache_hit_get(c: &mut Criterion) {
    use rand::seq::SliceRandom;
    use shamir_db::storage::storage_in_memory::InMemoryStore;
    use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
    use shamir_db::storage::types::Store;
    use shamir_types::types::record_id::RecordId;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("cache_hit_get");

    for &n in &[1_000usize, 10_000] {
        // Build a warmed MemBuffer cache holding `n` keys, all
        // resident (cache size = n, large max_bytes).
        let (store, keys): (Arc<dyn Store>, Vec<shamir_db::storage::types::RecordKey>) = rt
            .block_on(async {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 64 * 1024 * 1024,
                    max_entries: n * 2,
                    ttl_ms: None,
                    flush_interval_ms: 500,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let mut keys = Vec::with_capacity(n);
                for i in 0..n {
                    let id = RecordId::new();
                    let key = shamir_db::storage::types::RecordKey::copy_from_slice(id.as_bytes());
                    let value = shamir_db::storage::types::RecordKey::from(format!("v{i}"));
                    store.set(key.clone(), value).await.unwrap();
                    keys.push(key);
                }
                (store, keys)
            });

        // Shuffle keys for uniform-random access pattern.
        let mut rng = rand::thread_rng();
        let mut shuffled = keys.clone();
        shuffled.shuffle(&mut rng);

        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            let mut cursor = 0usize;
            b.to_async(&rt).iter(|| {
                let store = Arc::clone(&store);
                let key = shuffled[cursor % shuffled.len()].clone();
                cursor = cursor.wrapping_add(1);
                async move {
                    let _ = store.get(key).await.unwrap();
                }
            });
        });
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

// ═══════════════════════════════════════════════════════════════════
// P7 — nested-batch overhead
//
// Measures the overhead introduced by `BatchOp::Batch` (sub_batch) —
// bind-resolution, recursive execution, and result-wrapping — versus
// the same logical work expressed as sibling ops in a flat batch.
//
// Work performed in both cases:
//   1. Read one user by id ("u00000999") from a 1000-record table.
//   2. Upsert one record with a fixed score (42).
//
// flat:   both ops are siblings in a single batch.
// nested: the upsert is wrapped in a sub_batch; the uid parameter is
//         bound from the outer read result via a $query reference
//         (outer `user` query → first row → field "id"), and the inner
//         upsert filter references it through `val::param("uid")`.
// ═══════════════════════════════════════════════════════════════════

fn bench_nested_batch(c: &mut Criterion) {
    const N: usize = 1_000;
    const TARGET: &str = "u00000999";
    const SCORE: i64 = 42;

    let rt = Runtime::new().unwrap();
    let shamir = rt.block_on(seeded(N, false));

    // ── flat request ───────────────────────────────────────────────
    // op1: read user by id
    // op2: upsert with fixed values — same record that the read finds
    let req_flat = {
        let mut b = Batch::new();
        b.id("flat_nb").return_flagged();
        b.query("user", Query::from("users").where_eq("id", TARGET));
        b.upsert(
            "write",
            write::upsert("users")
                .key(json!({ "id": TARGET }))
                .value(json!({ "id": TARGET, "score": SCORE, "name": "Bench", "active": true })),
        );
        b.build()
    };

    // ── nested request ─────────────────────────────────────────────
    // outer: read user by id
    // sub_batch: upsert parameterised on uid bound from @user[0].id
    let req_nested = {
        // Build the inner batch whose upsert key/value use $param("uid").
        let inner = {
            let mut ib = Batch::new();
            ib.id("inner_nb").return_flagged();
            // Filter: where id = $param("uid")
            ib.upsert(
                "write",
                write::upsert("users").key(json!({ "id": TARGET })).value(
                    json!({ "id": TARGET, "score": SCORE, "name": "Bench", "active": true }),
                ),
            );
            ib.build()
        };

        let mut ob = Batch::new();
        ob.id("nested_nb").return_flagged();
        let user_handle = ob.query("user", Query::from("users").where_eq("id", TARGET));
        // bind uid → @user[0].id
        let uid_ref = user_handle.first().field("id");
        let mut bind = new_map();
        bind.insert("uid".to_string(), uid_ref);
        ob.sub_batch("proc", inner, bind);
        ob.build()
    };

    let mut group = c.benchmark_group("nested_batch");
    if quick() {
        group
            .sample_size(10)
            .measurement_time(Duration::from_secs(1));
    }

    group.bench_function("flat", |b| {
        let shamir = Arc::clone(&shamir);
        let req = req_flat.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&shamir);
            let r = req.clone();
            async move {
                s.execute("bench", &r).await.unwrap();
            }
        });
    });

    group.bench_function("nested", |b| {
        let shamir = Arc::clone(&shamir);
        let req = req_nested.clone();
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&shamir);
            let r = req.clone();
            async move {
                s.execute("bench", &r).await.unwrap();
            }
        });
    });

    group.finish();
}

// --------------------------------------------------------------------------
// Driver
// --------------------------------------------------------------------------

/// Construct a `Criterion` that respects `BENCH_QUICK`. In quick
/// mode the global sample-size + measurement-time floors are
/// dropped so every inline `group.sample_size(...)` /
/// `group.measurement_time(...)` setter is overridden by the
/// smaller defaults this returns. Without the env var, behaviour
/// matches `Criterion::default()` exactly.
fn quick_aware_criterion() -> Criterion {
    let c = Criterion::default();
    if quick() {
        c.sample_size(10)
            .measurement_time(Duration::from_secs(1))
            .warm_up_time(Duration::from_millis(100))
    } else {
        c
    }
}

criterion_group! {
    name = benches;
    config = quick_aware_criterion();
    targets =
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
    bench_min_only_no_index,
    bench_min_only_with_index,
    bench_order_limit_with_index,
    bench_order_limit_asc_with_sorted_index,
    bench_order_limit_desc_with_sorted_index,
    bench_order_limit_desc_with_sorted_index_sled,
    bench_range_query_no_index,
    bench_range_query_with_index,
    bench_bulk_insert_sled,
    bench_bulk_insert_redb,
    bench_bulk_insert_persy,
    bench_bulk_insert_canopy,
    bench_bulk_insert_fjall,
    bench_bulk_insert_nebari,
    bench_bulk_insert_membuffer_in_memory,
    bench_bulk_insert_membuffer_sled,
    bench_bulk_insert_membuffer_redb,
    bench_bulk_insert_membuffer_persy,
    bench_bulk_insert_membuffer_canopy,
    bench_bulk_insert_membuffer_fjall,
    bench_bulk_insert_membuffer_nebari,
    bench_bulk_insert_with_index_sled,
    bench_range_query_no_index_sled,
    bench_range_query_with_index_sled,
    bench_range_query_narrow_no_index_sled,
    bench_range_query_narrow_with_index_sled,
    bench_range_query_wide_narrow_no_index_sled,
    bench_range_query_wide_narrow_with_index_sled,
    bench_range_query_wide_narrow_with_covering_index_sled,
    bench_range_query_wide_highk_with_index_sled,
    bench_range_query_wide_highk_with_covering_index_sled,
    bench_batch_multi_read,
    bench_cache_hit_get,
    bench_steady_state_insert,
    bench_wal_high_qps,
    bench_eviction_byte_pressure,
    bench_ttl_sweep_50k,
    bench_concurrent_inserts,
    bench_ddl_create_index_on_seeded,
    bench_group_by_sum_e2e,
    bench_changefeed_overhead,
    bench_validator_overhead,
    bench_nested_batch
}
criterion_main!(benches);

// ═══════════════════════════════════════════════════════════════════
// Concurrent write contention
// ═══════════════════════════════════════════════════════════════════

fn bench_concurrent_inserts(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let shamir = rt.block_on(async {
        let s = Arc::new(ShamirDb::init_memory().await.unwrap());
        s.create_db("bench").await;
        let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"));
        s.add_repo("bench", cfg).await.unwrap();
        s
    });

    let mut group = c.benchmark_group("concurrent_inserts");
    for n_writers in [1, 2, 4, 8] {
        group.throughput(Throughput::Elements(n_writers as u64));
        group.bench_with_input(
            BenchmarkId::new("writers", n_writers),
            &n_writers,
            |b, &n| {
                let counter = std::sync::atomic::AtomicU64::new(0);
                b.to_async(&rt).iter(|| {
                    let shamir = Arc::clone(&shamir);
                    let c = &counter;
                    async move {
                        let mut handles = Vec::new();
                        for _w in 0..n {
                            let s = Arc::clone(&shamir);
                            let id = c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            handles.push(tokio::spawn(async move {
                                let mut b = Batch::new();
                                b.id(id).upsert(
                                    "ups",
                                    write::upsert("users").key(json!({ "id": id })).value(
                                        json!({ "id": id, "name": format!("w{id}"), "score": id }),
                                    ),
                                );
                                let req = b.build();
                                s.execute("bench", &req).await.unwrap();
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

// ═══════════════════════════════════════════════════════════════════
// DDL: create_index on a seeded table (rebuild cost)
// ═══════════════════════════════════════════════════════════════════

fn bench_ddl_create_index_on_seeded(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("ddl_create_index");
    group.sample_size(10);
    for n_records in [100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("records", n_records),
            &n_records,
            |b, &n| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let shamir = rt.block_on(async {
                            let s = Arc::new(ShamirDb::init_memory().await.unwrap());
                            s.create_db("bench").await;
                            let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
                                .add_table(TableConfig::new("items"));
                            s.add_repo("bench", cfg).await.unwrap();
                            seed_users_inner(&s, n, "items").await;
                            s
                        });
                        let start = Instant::now();
                        rt.block_on(async {
                            let idx = ddl::create_index("by_score", "items")
                                .field("score")
                                .build();
                            let mut b = Batch::new();
                            b.id(1).create_index("idx", idx);
                            let req = b.build();
                            shamir.execute("bench", &req).await.unwrap();
                        });
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    group.finish();
}

// ═══════════════════════════════════════════════════════════════════
// GROUP BY + aggregation e2e
// ═══════════════════════════════════════════════════════════════════

fn bench_group_by_sum_e2e(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let shamir = rt.block_on(seeded(1000, false));

    let mut b_sum = Batch::new();
    b_sum.id(1).query(
        "g",
        Query::from("users")
            .select([
                select::field("city"),
                select::sum("score", "total_score"),
                select::count_all("n"),
            ])
            .group_by_many(["city"]),
    );
    let req_group_sum = b_sum.build();

    let mut b_avg = Batch::new();
    b_avg.id(2).query(
        "g",
        Query::from("users")
            .select([select::field("city"), select::avg("score", "avg_score")])
            .group_by_many(["city"]),
    );
    let req_group_avg = b_avg.build();

    let mut group = c.benchmark_group("group_by_e2e");
    group.throughput(Throughput::Elements(1000));
    group.bench_function("sum_count_by_city_1000", |b| {
        let s = Arc::clone(&shamir);
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let req = req_group_sum.clone();
            async move { s.execute("bench", &req).await.unwrap() }
        });
    });
    group.bench_function("avg_by_city_1000", |b| {
        let s = Arc::clone(&shamir);
        b.to_async(&rt).iter(|| {
            let s = Arc::clone(&s);
            let req = req_group_avg.clone();
            async move { s.execute("bench", &req).await.unwrap() }
        });
    });
    group.finish();
}

// ═══════════════════════════════════════════════════════════════════
// B1a — changefeed emission overhead
// ═══════════════════════════════════════════════════════════════════

/// Regression guard: overhead of `emit_nontx_changefeed` on a single
/// insert when (a) no subscriber is attached, and (b) one subscriber
/// is holding a `broadcast::Receiver`.
///
/// Both scenarios go through the full `execute` → planner → table
/// write → emit path.  The delta between (a) and (b) is the cost
/// of a single `try_send` into a bounded broadcast channel.
fn bench_changefeed_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("changefeed_overhead");

    // Shared request: insert record with id "b1" into the bench table.
    // Using a fixed id means repeated calls overwrite the same row
    // (upsert), keeping the table size stable across iterations.
    let mut b_ins = Batch::new();
    b_ins
        .id("b1")
        .return_flagged()
        .insert("ins", write::insert("users").row(gen_user(999)));
    let req_insert = b_ins.build();

    // (a) no_subscribers — changefeed channel has no active receivers.
    //     emit_nontx_changefeed does try_send; with 0 receivers it still
    //     serialises the event into Arc but the send is a no-op.
    {
        let shamir = rt.block_on(seeded(100, false));
        group.bench_function("no_subscribers", |b| {
            let shamir = Arc::clone(&shamir);
            let req = req_insert.clone();
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        });
    }

    // (b) with_subscriber — one live broadcast::Receiver is held for
    //     the duration of the bench.  The receiver is never polled, so
    //     the channel will lag after the ring fills; that's intentional
    //     — we measure the send-side cost, not the recv side.
    {
        let shamir = rt.block_on(seeded(100, false));
        // subscribe_changelog returns None when the repo does not exist;
        // if it returns None here the bench still runs but measures the
        // no-subscriber path.
        let _subscriber = rt.block_on(shamir.subscribe_changelog("bench", "main"));
        group.bench_function("with_subscriber", |b| {
            let shamir = Arc::clone(&shamir);
            let req = req_insert.clone();
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        });
    }

    group.finish();
}

// ═══════════════════════════════════════════════════════════════════
// B1b — validator dispatch overhead
// ═══════════════════════════════════════════════════════════════════

/// Regression guard: per-insert cost of `run_validators` when
/// (a) zero validators are bound (empty-registry fast path), and
/// (b) one no-op WASM validator is bound to the table.
///
/// Scenario (b) uses the minimal `(module)` WAT which compiles to
/// a zero-function WASM module; the validator succeeds vacuously.
/// This isolates the dispatch / fn-lookup overhead from any real
/// WASM execution cost.
fn bench_validator_overhead(c: &mut Criterion) {
    use shamir_db::engine::validator::WriteOp;

    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("validator_overhead");

    let mut b_ins = Batch::new();
    b_ins
        .id("b1")
        .return_flagged()
        .insert("ins", write::insert("users").row(gen_user(999)));
    let req_insert = b_ins.build();

    // (a) no_validators — default state, run_validators checks an
    //     empty binding list and returns immediately.
    {
        let shamir = rt.block_on(seeded(100, false));
        group.bench_function("no_validators", |b| {
            let shamir = Arc::clone(&shamir);
            let req = req_insert.clone();
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        });
    }

    // (b) one_validator — a minimal WASM module (no exported functions)
    //     is registered and bound to the `users` table for Insert ops.
    //     The validator succeeds vacuously; cost = dispatch + WASM call
    //     with no meaningful work.
    {
        // Minimal WAT that satisfies the validator ABI:
        //   - exports `memory`
        //   - exports `shamir_alloc` (allocator)
        //   - exports `shamir_call` returning msgpack `null` (0xC0) = accept
        const NOOP_VALIDATOR_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "\c0")
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const 1)
    )
  )
)
"#;
        let shamir = rt.block_on(async {
            let s = seeded(100, false).await;
            let noop_wasm = wat::parse_str(NOOP_VALIDATOR_WAT).unwrap();
            s.create_validator_from_wasm("v_bench_noop", &noop_wasm, false)
                .await
                .expect("create validator");
            s.bind_validator(
                "bench",
                "main",
                "users",
                "v_bench_noop",
                vec![WriteOp::Insert],
                1000,
            )
            .await
            .expect("bind validator");
            s
        });
        group.bench_function("one_validator", |b| {
            let shamir = Arc::clone(&shamir);
            let req = req_insert.clone();
            b.to_async(&rt).iter(|| {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        });
    }

    group.finish();
}

async fn seed_users_inner(shamir: &ShamirDb, n: usize, table: &str) {
    for chunk_start in (0..n).step_by(50) {
        let chunk_end = (chunk_start + 50).min(n);
        let values: Vec<JsonValue> = (chunk_start..chunk_end).map(gen_user).collect();
        let mut b = Batch::new();
        b.id(chunk_start)
            .insert("s", write::insert(table).rows(values));
        let req = b.build();
        shamir.execute("bench", &req).await.unwrap();
    }
}

/// Seed `n` **wide** records (gen_user_wide) into `table` in chunks of 50.
async fn seed_users_wide(shamir: &ShamirDb, n: usize) {
    seed_users_wide_inner(shamir, n, "users").await;
}

async fn seed_users_wide_inner(shamir: &ShamirDb, n: usize, table: &str) {
    for chunk_start in (0..n).step_by(50) {
        let chunk_end = (chunk_start + 50).min(n);
        let values: Vec<JsonValue> = (chunk_start..chunk_end).map(gen_user_wide).collect();
        let mut b = Batch::new();
        b.id(chunk_start)
            .insert("s", write::insert(table).rows(values));
        let req = b.build();
        shamir.execute("bench", &req).await.unwrap();
    }
}
