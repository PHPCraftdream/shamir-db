//! Record value-size axis benchmarks.
//!
//! Existing benches all use narrow records (~20 fields of small values),
//! which leaves the behaviour of S.H.A.M.I.R. on records carrying real
//! blobs (article text, base64 images, serialised state) completely unmeasured.
//!
//! Three concrete questions this bench answers:
//!
//! 1. **Insert cost** vs value size — does cost scale linearly with bytes,
//!    or is there a step (mmap fault, msgpack chunking, allocator churn)?
//!    Migrated to `bench_scale_tool`: a fresh DB + unique key is required
//!    every iteration (inserting the same key twice would silently become
//!    an update), so this uses `bench_batched_async` — only the `execute`
//!    call is timed.
//!
//! 2. **Read cost** vs value size — same axis, fetching one record by key.
//!    Setup (one seeded record) is shared across every iteration — a read
//!    never mutates state — so this uses `bench_async`.
//!
//! 3. **De-intern cost** (`ShamirDb::decode_record_value_query_value`) on
//!    large records. This path backs the subscription bridge. The existing
//!    `decode_record_value_query_value` bench in `shamir-server` only covers
//!    5- and 20-field narrow records. Question: does a single 100KB
//!    string field stay O(1) at the interner level (one key, one big
//!    value), and how does that compare to a 10KB object spread across
//!    ~50 fields (many interner lookups)? Setup (captured changefeed
//!    bytes) is shared across iterations — `bench_async`.
//!
//! Methodology notes:
//!   * In-memory backend — removes disk variance; we want the engine /
//!     msgpack / interner curve, not fsync noise.
//!   * Deterministic value content (`"x".repeat(N)`) so msgpack length
//!     prefixes and allocator behaviour are reproducible run-to-run.
//!
//! Run:
//!   cargo bench -p shamir-db --bench record_size_axis

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

include!("bench_allocator.rs");

use bench_scale_tool::Harness;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::time::timeout;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_engine::ChangeOp;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter as f;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;

const DB: &str = "bench";
const REPO: &str = "main";
const TABLE: &str = "blobs";

/// Sizes for the single-string-field variants. 1MB is the upper bound:
/// some backends configure max-record limits below this; the in-memory
/// backend used here has no such cap, but if a future change introduces
/// one this size should be the first to drop.
const SIZES: &[(usize, &str)] = &[
    (1_024, "1kb"),
    (10 * 1_024, "10kb"),
    (100 * 1_024, "100kb"),
    (1_024 * 1_024, "1mb"),
];

/// Fresh in-memory ShamirDb with one repo + one table named `TABLE`.
async fn fresh_db() -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db(DB).await;
    let cfg = RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE));
    shamir.add_repo(DB, cfg).await.expect("add_repo");
    shamir
}

/// One record carrying a single big `data` string of the given length.
fn one_big_row(id: &str, size: usize) -> QueryValue {
    mpack!({
        "id":   @(QueryValue::from(id)),
        "data": @(QueryValue::from("x".repeat(size))),
    })
}

/// ~`size` byte object split across ~50 string fields — exercises the
/// interner path (many distinct keys) at a comparable total payload.
fn one_wide_row(id: &str, total_size: usize) -> QueryValue {
    const N_FIELDS: usize = 50;
    let per_field = total_size / N_FIELDS;
    let chunk = "x".repeat(per_field);
    let mut obj = new_map();
    obj.insert("id".to_string(), QueryValue::from(id));
    for i in 0..N_FIELDS {
        obj.insert(format!("f{i:02}"), QueryValue::from(chunk.as_str()));
    }
    QueryValue::Map(obj)
}

/// Insert one record carrying the given QueryValue, tap the changefeed
/// before the insert, return the Put change's value bytes.
async fn capture_change_bytes(row: QueryValue) -> (Arc<ShamirDb>, Bytes) {
    let shamir = fresh_db().await;
    let mut rx = shamir
        .subscribe_changelog(DB, REPO)
        .await
        .expect("subscribe_changelog");

    let mut bch = Batch::new();
    bch.id("ins").insert("ins", write::insert(TABLE).row(row));
    shamir.execute(DB, &bch.build()).await.unwrap();

    let evt = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("changefeed recv timeout")
        .expect("changefeed broadcast closed");
    let value_bytes = evt
        .changes
        .iter()
        .find_map(|c| {
            if matches!(c.op, ChangeOp::Put) {
                c.value.clone()
            } else {
                None
            }
        })
        .expect("no Put change in event");
    (shamir, value_bytes)
}

fn main() {
    let mut h = Harness::new("record_size_axis", env!("CARGO_MANIFEST_DIR"));
    let rt = tokio::runtime::Runtime::new().unwrap();

    // ── Group 1: insert_by_size ─────────────────────────────────────────
    // Fresh DB + unique key required every iteration — `bench_batched_async`.
    for &(size, label) in SIZES {
        let id = format!("insert_by_size/value_{label}");
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        h.bench_batched_async(
            &id,
            move || async move { fresh_db().await },
            move |shamir| {
                let counter = Arc::clone(&counter);
                async move {
                    let i = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let payload = "x".repeat(size);
                    let key = format!("k{i:010}");
                    let row = mpack!({ "id": @(QueryValue::from(key.as_str())), "data": @(QueryValue::from(payload.as_str())) });
                    let mut bch = Batch::new();
                    bch.id("ins").insert("ins", write::insert(TABLE).row(row));
                    let req = bch.build();
                    let r = shamir.execute(DB, &req).await.unwrap();
                    black_box(r);
                }
            },
        );
    }

    // ── Group 2: read_by_size ───────────────────────────────────────────
    // Setup (one seeded record) shared across iterations — `bench_async`.
    for &(size, label) in SIZES {
        let shamir = rt.block_on(async {
            let s = fresh_db().await;
            let mut bch = Batch::new();
            let key = format!("k_{label}");
            bch.id("seed")
                .insert("ins", write::insert(TABLE).row(one_big_row(&key, size)));
            s.execute(DB, &bch.build()).await.unwrap();
            s
        });
        let key = format!("k_{label}");
        let id = format!("read_by_size/value_{label}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let key = key.clone();
            async move {
                let mut bch = Batch::new();
                bch.id("r").query(
                    "r",
                    Query::from(TABLE).where_(f::eq("id", key.clone())).limit(1),
                );
                let req = bch.build();
                let r = shamir.execute(DB, &req).await.unwrap();
                black_box(r);
            }
        });
    }

    // ── Group 3: decode_record_value_qv_large — subscription bridge de-intern ──

    // Scenario A: one big string field, 10KB.
    let (db_a, bytes_a) = rt.block_on(capture_change_bytes(one_big_row("k", 10 * 1_024)));
    h.bench_async(
        "decode_record_value_qv_large/string_10kb",
        move || {
            let db = Arc::clone(&db_a);
            let bytes = bytes_a.clone();
            async move {
                let v = db
                    .decode_record_value_query_value(
                        black_box(DB),
                        black_box(REPO),
                        black_box(TABLE),
                        black_box(&bytes),
                    )
                    .await;
                black_box(v);
            }
        },
    );

    // Scenario B: one big string field, 100KB.
    let (db_b, bytes_b) = rt.block_on(capture_change_bytes(one_big_row("k", 100 * 1_024)));
    h.bench_async(
        "decode_record_value_qv_large/string_100kb",
        move || {
            let db = Arc::clone(&db_b);
            let bytes = bytes_b.clone();
            async move {
                let v = db
                    .decode_record_value_query_value(
                        black_box(DB),
                        black_box(REPO),
                        black_box(TABLE),
                        black_box(&bytes),
                    )
                    .await;
                black_box(v);
            }
        },
    );

    // Scenario C: ~10KB total spread across ~50 fields — distinct
    // interner pressure (many string-key lookups vs one big value).
    let (db_c, bytes_c) = rt.block_on(capture_change_bytes(one_wide_row("k", 10 * 1_024)));
    h.bench_async(
        "decode_record_value_qv_large/nested_object_10kb",
        move || {
            let db = Arc::clone(&db_c);
            let bytes = bytes_c.clone();
            async move {
                let v = db
                    .decode_record_value_query_value(
                        black_box(DB),
                        black_box(REPO),
                        black_box(TABLE),
                        black_box(&bytes),
                    )
                    .await;
                black_box(v);
            }
        },
    );

    h.run();
}
