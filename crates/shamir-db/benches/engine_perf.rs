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
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): every
//! group below chooses `bench_async` (shared setup, read-only or
//! same-key-upsert workloads that don't invalidate the fixture) or
//! `bench_batched_async` (setup must be fresh every iteration — deletes
//! that shrink the table, or a DB that must start from a clean tempdir/
//! empty state) per the module docs' plan 1 / plan 2 split.
//!
//! Run:
//!   cargo bench -p shamir-db --bench engine_perf

use std::hint::black_box;
use std::sync::Arc;

include!("bench_allocator.rs");

use bench_scale_tool::Harness;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
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

fn gen_user(i: usize) -> QueryValue {
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
/// ~1 MiB Windows main-thread stack — see `docs/perf/sefer-alloc-rollout-…`.
/// All bench-setup `rt.block_on(...)` calls should route through this helper.
fn block_on_setup<F>(rt: &Runtime, fut: F) -> F::Output
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

async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

async fn fresh_db_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
    fresh_db_with(BoxRepoFactory::fjall(path.to_path_buf())).await
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

async fn fresh_db_membuffer_fjall(path: &std::path::Path) -> Arc<ShamirDb> {
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

async fn create_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str, unique: bool) {
    create_index_inner(shamir, table, index_name, field, unique, false).await
}

async fn create_sorted_index(shamir: &ShamirDb, table: &str, index_name: &str, field: &str) {
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
async fn seeded(n: usize, with_id_index: bool) -> Arc<ShamirDb> {
    let shamir = fresh_db().await;
    seed_users(&shamir, n).await;
    if with_id_index {
        create_index(&shamir, "users", "by_id", "id", false).await;
    }
    shamir
}

async fn seed_users_inner(shamir: &ShamirDb, n: usize, table: &str) {
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

fn req_set_one(target_id: &str, score: i64) -> BatchRequest {
    let mut b = Batch::new();
    b.id("s").return_flagged().upsert(
        "s",
        write::upsert("users")
            .key(mpack!({ "id": @(QueryValue::from(target_id)) }))
            .value(mpack!({ "id": @(QueryValue::from(target_id)), "score": @(QueryValue::from(score)), "name": "Updated", "active": true })),
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
            .set(mpack!({ "score": 1234 })),
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

fn req_bulk_insert(start: usize, count: usize) -> BatchRequest {
    let values: Vec<QueryValue> = (start..start + count).map(gen_user).collect();
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
// Sweep constants
// --------------------------------------------------------------------------

// Capped at 2_000 (was 10_000): the harness now owns iteration count
// externally, so a single call should stay a small, cheap "unit" (target
// ≲10ms) rather than a multi-hundred-ms operation on its own — a large-N
// no-index scan at 10_000 records cost 60-150ms per single call, which
// piles up fast across dozens of size-swept workloads in one bench binary.
const SIZES: &[usize] = &[100, 1_000, 2_000];
// A single bulk-insert call carrying N rows in ONE Batch/execute() request
// is a genuine feature to measure at realistic N — not an artificial
// per-op loop the harness's own repetition count already covers. Default
// sweep keeps /100 (~6ms/call, fast); /1000 (~25ms/call, over the fast-
// sweep budget but real bulk-throughput signal) is opt-in via
// BENCH_BULK_INSERT_SCALING=1 so it isn't lost, just not in the default
// fast path.
fn bulk_counts() -> Vec<usize> {
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
const BULK_COUNT: usize = 100;

fn main() {
    let mut h = Harness::new("engine_perf", env!("CARGO_MANIFEST_DIR"));
    let rt = Runtime::new().unwrap();

    // ── bulk_insert — pure write throughput, fresh empty table per iter ──
    for count in bulk_counts() {
        let id = format!("bulk_insert/{count}");
        h.bench_batched_async(
            &id,
            || async move { fresh_db().await },
            move |shamir| async move {
                let req = req_bulk_insert(0, count);
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ── set_existing_no_index / with_index — shared seeded fixture ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let target = format!("u{:08}", n - 1);
        let id = format!("set_existing_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_set_one(&target, 42);
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let target = format!("u{:08}", n - 1);
        let id = format!("set_existing_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_set_one(&target, 42);
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── read_by_id_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("read_by_id_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let id = format!("read_by_id_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── read_by_city_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("read_by_city_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_city("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        let id = format!("read_by_city_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_city("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── update_by_id_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("update_by_id_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_update_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let id = format!("update_by_id_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_update_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── delete_by_id_no_index / with_index — table shrinks, fresh per iter ──
    for &n in SIZES {
        let id = format!("delete_by_id_no_index/{n}");
        h.bench_batched_async(
            &id,
            move || async move { seeded(n, false).await },
            move |shamir| {
                let req = req_delete_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            },
        );
    }
    for &n in SIZES {
        let id = format!("delete_by_id_with_index/{n}");
        h.bench_batched_async(
            &id,
            move || async move { seeded(n, true).await },
            move |shamir| {
                let req = req_delete_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            },
        );
    }

    // ── complex_filter ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("complex_filter/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_complex_filter();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10 (full scan + sort, no index) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("order_limit_top10/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── count_all_no_filter ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("count_all_no_filter/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_all();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── count_with_filter_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("count_with_filter_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_with_filter("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        let id = format!("count_with_filter_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_with_filter("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── min_max_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("min_max_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_max_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        let id = format!("min_max_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_max_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── min_only_no_index / with_index (sorted) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("min_only_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("min_only_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_desc_sorted (sorted index, reverse-iter fast path) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("order_limit_top10_desc_sorted/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_asc_sorted (sorted index, forward-iter fast path) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("order_limit_top10_asc_sorted/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit_asc();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_with_index (regular index) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        let id = format!("order_limit_top10_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── range_query_no_index / with_index (sorted) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("range_query_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_range_age();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            // Sorted index for range queries — equality (hash) index
            // wouldn't help here.
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        let id = format!("range_query_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_range_age();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // --------------------------------------------------------------------
    // Sled/disk-backed range bench — exercises the native
    // `iter_range_stream` path on a real disk backend. Fresh tempdir per
    // iter, so `bench_batched_async`.
    //
    // I/O-bound exception: fjall is a persistent LSM-tree — every commit
    // does real disk I/O, so even the /100 cell costs well over the
    // ≲10ms-per-call target (calibrated N=1). Reduced to the smallest
    // cell only (BULK_COUNT); the disk cost is the point of the bench,
    // not something tunable by shrinking N further.
    // --------------------------------------------------------------------

    {
        let count = BULK_COUNT;
        let id = format!("bulk_insert_fjall/{count}");
        h.bench_batched_async(
            &id,
            || async move {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let shamir = fresh_db_fjall(tempdir.path()).await;
                (shamir, tempdir)
            },
            move |(shamir, tempdir)| {
                let req = req_bulk_insert(0, count);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                    drop(shamir);
                    drop(tempdir);
                }
            },
        );
    }

    // ── bulk_insert_membuffer_in_memory — fresh DB per iter (pure wrapper overhead) ──
    for count in bulk_counts() {
        let id = format!("bulk_insert_membuffer_in_memory/{count}");
        h.bench_batched_async(
            &id,
            || async move { fresh_db_membuffer_in_memory().await },
            move |shamir| async move {
                let req = req_bulk_insert(0, count);
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ── bulk_insert_membuffer_fjall — fresh tempdir + DB per iter ──
    // I/O-bound exception: same fjall persistent-backend disk cost as
    // `bulk_insert_fjall` above — every commit hits disk, so calibrated N=1
    // is expected and accepted. Kept at the smallest cell only (BULK_COUNT).
    {
        let count = BULK_COUNT;
        let id = format!("bulk_insert_membuffer_fjall/{count}");
        h.bench_batched_async(
            &id,
            || async move {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let shamir = fresh_db_membuffer_fjall(tempdir.path()).await;
                (shamir, tempdir)
            },
            move |(shamir, tempdir)| {
                let req = req_bulk_insert(0, count);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                    drop(shamir);
                    drop(tempdir);
                }
            },
        );
    }

    // ── steady_state_insert — inserts into a fresh MemBuffer DB ──
    h.bench_batched_async(
        "steady_state_insert_500/membuffer_in_memory",
        || async move { fresh_db_membuffer_in_memory().await },
        |shamir| async move {
            let req = req_bulk_insert(0, 500);
            shamir.execute("bench", &req).await.unwrap();
            drop(shamir);
        },
    );

    // ── ttl_sweep — no_ttl vs short-TTL 50k-seed cost ──
    {
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::{RecordKey, Store};
        use shamir_types::types::record_id::RecordId;

        h.bench_batched_async(
            "ttl_sweep/no_ttl_seed_2k",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                (store, v)
            },
            |(store, v)| async move {
                for _ in 0..2_000 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );

        h.bench_batched_async(
            "ttl_sweep/ttl_50ms_seed_2k_flush_300ms",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: Some(50),
                    flush_interval_ms: 300,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                (store, v)
            },
            |(store, v)| async move {
                for _ in 0..2_000 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );
    }

    // ── eviction_byte_pressure — seed 8k near cap, time next 1k writes ──
    {
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::{RecordKey, Store};
        use shamir_types::types::record_id::RecordId;

        h.bench_batched_async(
            "eviction_byte_pressure/seed_1800_then_insert_200",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                // ~100 bytes/value × 1800 = ~180k cache footprint.
                // max_bytes 200_000 leaves ~20k headroom (~200 entries) —
                // the first new inserts put us over; eviction kicks in
                // immediately. Scaled down from seed_8k/insert_1k (was
                // ~25ms per single call) so the timed routine stays
                // ≲10ms while still exercising the eviction path on
                // every insert.
                let cfg = MemBufferConfig {
                    max_bytes: 200_000,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000, // effectively no auto-flush
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                for _ in 0..1_800 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                (store, v)
            },
            |(store, v)| async move {
                // Now seeded near cap. Time the next 200 writes — each
                // pushes past the byte cap and forces an eviction.
                for _ in 0..200 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );
    }

    // ── wal_high_qps — 200 single-record batches into fresh MemBuffer DB ──
    h.bench_batched_async(
        "wal_high_qps/200_single_record_batches",
        || async move { fresh_db_membuffer_in_memory().await },
        |shamir| async move {
            let req = req_bulk_insert(0, 1);
            for _ in 0..200 {
                shamir.execute("bench", &req).await.unwrap();
            }
            drop(shamir);
        },
    );

    // ── cache_hit_get — warm MemBuffer cache, 100% hit, random key ──
    {
        use rand::seq::SliceRandom;
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::Store;
        use shamir_types::types::record_id::RecordId;

        // Collapsed to the smallest cell (was &[1_000, 2_000]): both
        // calibrated cheaply (~30k iters), but the harness owns repetition
        // count now, so a single scaled axis keeps only its smallest unit.
        let n: usize = 1_000;
        {
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
                    let store: Arc<dyn Store> =
                        Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                    let mut keys = Vec::with_capacity(n);
                    for i in 0..n {
                        let id = RecordId::new();
                        let key = shamir_db::storage::types::RecordKey::from_slice(id.as_bytes());
                        let value = bytes::Bytes::from(format!("v{i}"));
                        store.set(key.clone(), value).await.unwrap();
                        keys.push(key);
                    }
                    (store, keys)
                });

            // Shuffle keys for uniform-random access pattern.
            let mut rng = rand::thread_rng();
            let mut shuffled = keys.clone();
            shuffled.shuffle(&mut rng);
            let cursor = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

            let id = format!("cache_hit_get/{n}");
            h.bench_async(&id, move || {
                let store = Arc::clone(&store);
                let cursor = Arc::clone(&cursor);
                let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let key = shuffled[i % shuffled.len()].clone();
                async move {
                    let _ = store.get(key).await.unwrap();
                }
            });
        }
    }

    // ── batch_multi_read_8 — 8 independent reads in one batch ──
    // Collapsed + tuned down (was &[1_000, 2_000]): each batch fires 8
    // full-table scans, so a single call at n=1_000 cost ~12ms and n=2_000
    // ~25ms — both over the ≲10ms-per-call target. n=500 keeps one full
    // batch (all 8 cities) under budget while preserving the multi-read
    // shape that's the point of this workload.
    let n: usize = 500;
    {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("batch_multi_read_8/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_batch_independent_reads();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ═══════════════════════════════════════════════════════════════════
    // Concurrent write contention
    // ═══════════════════════════════════════════════════════════════════

    {
        let shamir = rt.block_on(async {
            let s = Arc::new(ShamirDb::init_memory().await.unwrap());
            s.create_db("bench").await;
            let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("users"));
            s.add_repo("bench", cfg).await.unwrap();
            s
        });
        for n_writers in [1usize, 2, 4, 8] {
            let shamir = Arc::clone(&shamir);
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let id = format!("concurrent_inserts/writers_{n_writers}");
            h.bench_async(&id, move || {
                let shamir = Arc::clone(&shamir);
                let counter = Arc::clone(&counter);
                async move {
                    let mut handles = Vec::new();
                    for _w in 0..n_writers {
                        let s = Arc::clone(&shamir);
                        let id = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        handles.push(tokio::spawn(async move {
                            let mut b = Batch::new();
                            b.id(id).upsert(
                                "ups",
                                write::upsert("users").key(mpack!({ "id": @(QueryValue::from(id)) })).value(
                                    mpack!({ "id": @(QueryValue::from(id)), "name": @(QueryValue::from(format!("w{id}"))), "score": @(QueryValue::from(id)) }),
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
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // DDL: create_index on a seeded table (rebuild cost) — fresh table per iter
    // ═══════════════════════════════════════════════════════════════════

    for n_records in [50usize, 300] {
        let id = format!("ddl_create_index/records_{n_records}");
        h.bench_batched_async(
            &id,
            move || async move {
                let s = Arc::new(ShamirDb::init_memory().await.unwrap());
                s.create_db("bench").await;
                let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
                    .add_table(TableConfig::new("items"));
                s.add_repo("bench", cfg).await.unwrap();
                seed_users_inner(&s, n_records, "items").await;
                s
            },
            move |shamir| async move {
                let idx = ddl::create_index("by_score", "items")
                    .field("score")
                    .build();
                let mut b = Batch::new();
                b.id(1).create_index("idx", idx);
                let req = b.build();
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // GROUP BY + aggregation e2e
    // ═══════════════════════════════════════════════════════════════════

    {
        let shamir = block_on_setup(&rt, seeded(1000, false));

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

        {
            let shamir = Arc::clone(&shamir);
            let req = req_group_sum.clone();
            h.bench_async("group_by_e2e/sum_count_by_city_1000", move || {
                let s = Arc::clone(&shamir);
                let req = req.clone();
                async move {
                    black_box(s.execute("bench", &req).await.unwrap());
                }
            });
        }
        {
            let shamir = Arc::clone(&shamir);
            let req = req_group_avg.clone();
            h.bench_async("group_by_e2e/avg_by_city_1000", move || {
                let s = Arc::clone(&shamir);
                let req = req.clone();
                async move {
                    black_box(s.execute("bench", &req).await.unwrap());
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // B1a — changefeed emission overhead
    //
    // Regression guard: overhead of `emit_nontx_changefeed` on a single
    // insert when (a) no subscriber is attached, and (b) one subscriber
    // is holding a `broadcast::Receiver`.
    // ═══════════════════════════════════════════════════════════════════

    {
        let mut b_ins = Batch::new();
        b_ins
            .id("b1")
            .return_flagged()
            .insert("ins", write::insert("users").row(gen_user(999)));
        let req_insert = b_ins.build();

        // (a) no_subscribers.
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let req = req_insert.clone();
            h.bench_async("changefeed_overhead/no_subscribers", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }

        // (b) with_subscriber — one live broadcast::Receiver held for the
        //     duration; never polled (measures send-side cost only).
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let _subscriber = rt.block_on(shamir.subscribe_changelog("bench", "main"));
            let req = req_insert.clone();
            h.bench_async("changefeed_overhead/with_subscriber", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // B1b — validator dispatch overhead
    // ═══════════════════════════════════════════════════════════════════

    {
        use shamir_db::engine::validator::WriteOp;

        let mut b_ins = Batch::new();
        b_ins
            .id("b1")
            .return_flagged()
            .insert("ins", write::insert("users").row(gen_user(999)));
        let req_insert = b_ins.build();

        // (a) no_validators.
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let req = req_insert.clone();
            h.bench_async("validator_overhead/no_validators", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }

        // (b) one_validator — minimal WASM module (no exported functions,
        //     vacuous accept) bound to `users` for Insert ops.
        {
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
            let req = req_insert.clone();
            h.bench_async("validator_overhead/one_validator", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // P7 — nested-batch overhead
    //
    // Measures the overhead introduced by `BatchOp::Batch` (sub_batch) —
    // bind-resolution, recursive execution, and result-wrapping — versus
    // the same logical work expressed as sibling ops in a flat batch.
    // ═══════════════════════════════════════════════════════════════════

    {
        const N: usize = 1_000;
        const TARGET: &str = "u00000999";
        const SCORE: i64 = 42;

        let shamir = block_on_setup(&rt, seeded(N, false));

        // ── flat request ───────────────────────────────────────────────
        let req_flat = {
            let mut b = Batch::new();
            b.id("flat_nb").return_flagged();
            b.query("user", Query::from("users").where_eq("id", TARGET));
            b.upsert(
                "write",
                write::upsert("users")
                    .key(mpack!({ "id": @(QueryValue::from(TARGET)) }))
                    .value(mpack!({ "id": @(QueryValue::from(TARGET)), "score": @(QueryValue::from(SCORE)), "name": "Bench", "active": true })),
            );
            b.build()
        };

        // ── nested request ─────────────────────────────────────────────
        let req_nested = {
            let inner = {
                let mut ib = Batch::new();
                ib.id("inner_nb").return_flagged();
                ib.upsert(
                    "write",
                    write::upsert("users").key(mpack!({ "id": @(QueryValue::from(TARGET)) })).value(
                        mpack!({ "id": @(QueryValue::from(TARGET)), "score": @(QueryValue::from(SCORE)), "name": "Bench", "active": true }),
                    ),
                );
                ib.build()
            };

            let mut ob = Batch::new();
            ob.id("nested_nb").return_flagged();
            let user_handle = ob.query("user", Query::from("users").where_eq("id", TARGET));
            let uid_ref = user_handle.first().field("id");
            let mut bind = new_map();
            bind.insert("uid".to_string(), uid_ref);
            ob.sub_batch("proc", inner, bind);
            ob.build()
        };

        {
            let shamir = Arc::clone(&shamir);
            let req = req_flat.clone();
            h.bench_async("nested_batch/flat", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }

        {
            let shamir = Arc::clone(&shamir);
            let req = req_nested.clone();
            h.bench_async("nested_batch/nested", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }

    h.run();
}
