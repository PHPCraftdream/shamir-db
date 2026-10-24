use std::sync::Arc;

use shamir_collections::new_map;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::filter::{self as f};
use shamir_query_builder::query::Query;
use shamir_query_builder::select;
use shamir_query_builder::write;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Runtime;

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

pub(super) fn gen_user(i: usize) -> QueryValue {
    let mut m = new_map();
    m.insert("id".into(), QueryValue::from(format!("u{:08}", i)));
    m.insert(
        "name".into(),
        QueryValue::from(format!(
            "{} {}",
            FIRST_NAMES[i % FIRST_NAMES.len()],
            LAST_NAMES[(i / FIRST_NAMES.len()) % LAST_NAMES.len()]
        )),
    );
    m.insert(
        "email".into(),
        QueryValue::from(format!("user{}@{}", i, DOMAINS[i % DOMAINS.len()])),
    );
    m.insert("age".into(), QueryValue::from(18 + ((i * 37) % 60) as i64));
    m.insert("city".into(), QueryValue::from(CITIES[i % CITIES.len()]));
    m.insert("score".into(), QueryValue::from(((i * 7919) % 1000) as i64));
    m.insert("active".into(), QueryValue::Bool(!i.is_multiple_of(3)));
    m.insert(
        "created_at_ns".into(),
        QueryValue::from(1_700_000_000_000_000_000_i64 + (i as i64 * 60_000_000_000)),
    );
    m.insert(
        "tags".into(),
        QueryValue::List(vec![
            QueryValue::from(format!("tag_{}", i % 10)),
            QueryValue::from(format!("tag_{}", (i / 10) % 7)),
        ]),
    );
    QueryValue::Map(m)
}

// --------------------------------------------------------------------------
// Setup helpers
// --------------------------------------------------------------------------

/// Run an async setup future on a worker thread with an 8 MiB stack. Bench
/// fixtures (`seeded`, `seed_users`, `create_*_index`) generate deep async
/// state machines that, under `profile.bench` (opt-level=0), overflow the
/// ~1 MiB Windows main-thread stack — see `docs/dev-artifacts/perf/sefer-alloc-rollout-…`.
/// All bench-setup `rt.block_on(...)` calls should route through this helper.
pub(super) fn block_on_setup<F>(rt: &Runtime, fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = rt.handle().clone();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || handle.block_on(fut))
        .expect("spawn fat-stack setup thread")
        .join()
        .expect("setup thread panicked")
}

pub(super) async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await.expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

pub(super) async fn fresh_db_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::fjall(path.to_path_buf())).await
}

async fn fresh_db_with(factory: BoxRepoFactory) -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await.expect("create_db");
    let cfg = RepoConfig::new("main", factory).add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

// MemBuffer-wrapped variants — measure the wrapper overhead.
// Today MemBufferStore is a passthrough proxy, so these should
// produce numbers indistinguishable from the raw backend.
pub(super) async fn fresh_db_membuffer_in_memory() -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::in_memory(),
        MemBufferConfig::default(),
    ))
    .await
}

pub(super) async fn fresh_db_membuffer_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
    use shamir_storage::storage_membuffer::MemBufferConfig;
    fresh_db_with(BoxRepoFactory::membuffer(
        BoxRepoFactory::fjall(path.to_path_buf()),
        MemBufferConfig::default(),
    ))
    .await
}

/// Seed `n` records via a single `insert_into` op (does NOT scan).
async fn seed_users(shamir: &ShamirDb, n: usize) {
    let values: Vec<QueryValue> = (0..n).map(gen_user).collect();
    let mut b = Batch::new();
    b.id("seed")
        .return_flagged()
        .insert("ins", write::insert("users").rows(values));
    let req = b.build();
    shamir.execute("bench", &req).await.expect("seed");
}

pub(super) async fn create_index(
    shamir: &ShamirDb,
    table: &str,
    index_name: &str,
    field: &str,
    unique: bool,
) {
    create_index_inner(shamir, table, index_name, field, unique, false).await
}

pub(super) async fn create_sorted_index(
    shamir: &ShamirDb,
    table: &str,
    index_name: &str,
    field: &str,
) {
    create_index_inner(shamir, table, index_name, field, false, true).await
}

/// Like `create_sorted_index` but also sets `include: [[include_field]]` so
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
pub(super) async fn seeded(n: usize, with_id_index: bool) -> Arc<ShamirDb> {
    let shamir = fresh_db().await;
    seed_users(&shamir, n).await;
    if with_id_index {
        create_index(&shamir, "users", "by_id", "id", false).await;
    }
    shamir
}

pub(super) async fn seed_users_inner(shamir: &ShamirDb, n: usize, table: &str) {
    for chunk_start in (0..n).step_by(50) {
        let chunk_end = (chunk_start + 50).min(n);
        let values: Vec<QueryValue> = (chunk_start..chunk_end).map(gen_user).collect();
        let mut b = Batch::new();
        b.id(chunk_start)
            .insert("s", write::insert(table).rows(values));
        let req = b.build();
        shamir.execute("bench", &req).await.unwrap();
    }
}

// --------------------------------------------------------------------------
// Op factories — keeps the bench loops short
// --------------------------------------------------------------------------

pub(super) fn req_set_one(target_id: &str, score: i64) -> BatchRequest {
    let mut b = Batch::new();
    b.id("s").return_flagged().try_upsert(
        "s",
        write::upsert("users")
            .key(mpack!({ "id": @(QueryValue::from(target_id)) }))
            .value(mpack!({ "id": @(QueryValue::from(target_id)), "score": @(QueryValue::from(score)), "name": "Updated", "active": true })),
    ).unwrap();
    b.build()
}

pub(super) fn req_read_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_eq("id", target_id));
    b.build()
}

pub(super) fn req_read_by_city(city: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_eq("city", city));
    b.build()
}

pub(super) fn req_update_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("u")
        .return_flagged()
        .try_update(
            "u",
            write::update("users")
                .where_(f::eq("id", target_id))
                .set(mpack!({ "score": 1234 })),
        )
        .unwrap();
    b.build()
}

pub(super) fn req_delete_by_id(target_id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("d")
        .return_flagged()
        .try_delete("d", write::delete("users").where_(f::eq("id", target_id)))
        .unwrap();
    b.build()
}

pub(super) fn req_read_complex_filter() -> BatchRequest {
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

pub(super) fn req_read_with_order_limit() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").order_by_desc("score").limit(10));
    b.build()
}

pub(super) fn req_read_with_order_limit_asc() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").order_by_asc("score").limit(10));
    b.build()
}

pub(super) fn req_count_all() -> BatchRequest {
    let mut b = Batch::new();
    b.id("c")
        .query("c", Query::from("users").select([select::count_all("n")]));
    b.build()
}

pub(super) fn req_count_with_filter(city: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id("c").query(
        "c",
        Query::from("users")
            .where_eq("city", city)
            .select([select::count_all("n")]),
    );
    b.build()
}

pub(super) fn req_min_max_score() -> BatchRequest {
    let mut b = Batch::new();
    b.id("mm").query(
        "mm",
        Query::from("users").select([select::min("score", "lo"), select::max("score", "hi")]),
    );
    b.build()
}

/// MIN(score) ONLY — eligible for Q1 sorted-index fast-path
/// (single aggregate, no other select items, no filter/order/etc).
pub(super) fn req_min_score() -> BatchRequest {
    let mut b = Batch::new();
    b.id("m").query(
        "m",
        Query::from("users").select([select::min("score", "lo")]),
    );
    b.build()
}

pub(super) fn req_range_age() -> BatchRequest {
    let mut b = Batch::new();
    b.id("r")
        .query("r", Query::from("users").where_between("age", 30, 35));
    b.build()
}

pub(super) fn req_bulk_insert(start: usize, count: usize) -> BatchRequest {
    let values: Vec<QueryValue> = (start..start + count).map(gen_user).collect();
    let mut b = Batch::new();
    b.id("b")
        .return_flagged()
        .insert("ins", write::insert("users").rows(values));
    b.build()
}

pub(super) fn req_batch_independent_reads() -> BatchRequest {
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
// Sweep constants
// --------------------------------------------------------------------------

// Capped at 2_000 (was 10_000): the harness now owns iteration count
// externally, so a single call should stay a small, cheap "unit" (target
// ≲10ms) rather than a multi-hundred-ms operation on its own — a large-N
// no-index scan at 10_000 records cost 60-150ms per single call, which
// piles up fast across dozens of size-swept workloads in one bench binary.
pub(super) const SIZES: &[usize] = &[100, 1_000, 2_000];
// A single bulk-insert call carrying N rows in ONE Batch/execute() request
// is a genuine feature to measure at realistic N — not an artificial
// per-op loop the harness's own repetition count already covers. Default
// sweep keeps /100 (~6ms/call, fast); /1000 (~25ms/call, over the fast-
// sweep budget but real bulk-throughput signal) is opt-in via
// BENCH_BULK_INSERT_SCALING=1 so it isn't lost, just not in the default
// fast path.
pub(super) fn bulk_counts() -> Vec<usize> {
    let mut counts = vec![100usize];
    let wide = std::env::var("BENCH_BULK_INSERT_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    if wide {
        counts.push(1_000);
    }
    counts
}

/// Fixed single count for the fjall-backed bulk-insert variants below —
/// these are I/O-bound (every commit hits real disk), so scaling N
/// doesn't add a meaningful signal the way it does for the in-memory
/// variants above; kept at the smallest cell only.
pub(super) const BULK_COUNT: usize = 100;
