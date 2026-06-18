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
//!    Reported as `Throughput::Bytes` → MB/sec curve across
//!    1KB / 10KB / 100KB / 1MB single-field records.
//!
//! 2. **Read cost** vs value size — same axis, fetching one record by key.
//!    Expected shape: roughly linear in N (memcpy + msgpack decode); a
//!    super-linear step would indicate something pathological.
//!
//! 3. **De-intern cost** (`ShamirDb::decode_record_value_query_value`) on
//!    large records. This path backs the subscription bridge. The existing
//!    `decode_record_value_query_value` bench in `shamir-server` only covers
//!    5- and 20-field narrow records. Question: does a single 100KB
//!    string field stay O(1) at the interner level (one key, one big
//!    value), and how does that compare to a 10KB object spread across
//!    ~50 fields (many interner lookups)?
//!
//! Methodology notes:
//!   * In-memory backend — removes disk variance; we want the engine /
//!     msgpack / interner curve, not fsync noise.
//!   * Setup once per bench function; only the measured op is timed.
//!   * Deterministic value content (`"x".repeat(N)`) so msgpack length
//!     prefixes and allocator behaviour are reproducible run-to-run.
//!   * 1MB sample size dropped to 20 to keep wall-clock sane.
//!   * `Throughput::Bytes(N)` → criterion prints MB/sec directly.
//!
//! Run:
//!   cargo bench -p shamir-db --bench record_size_axis
//!   cargo bench -p shamir-db --bench record_size_axis -- \
//!       --sample-size 10 --measurement-time 3

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Runtime;
use tokio::time::timeout;

use shamir_bench_utils as bu;
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

// --------------------------------------------------------------------------
// Group 1: insert_by_size
// --------------------------------------------------------------------------

fn bench_insert_by_size(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("insert_by_size");

    for &(size, label) in SIZES {
        group.throughput(Throughput::Bytes(size as u64));
        if label == "1mb" {
            group.sample_size(bu::sample_size(20));
        }
        group.bench_function(format!("value_{label}"), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let shamir = fresh_db().await;
                // Pre-build one row's worth of payload outside the timed
                // region; `id` will vary per-iter via a counter so we
                // never collide with an existing key.
                let payload = "x".repeat(size);
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let id = format!("k{i:010}");
                    let row = mpack!({ "id": @(QueryValue::from(id.as_str())), "data": @(QueryValue::from(payload.as_str())) });
                    let mut bch = Batch::new();
                    bch.id("ins").insert("ins", write::insert(TABLE).row(row));
                    let req = bch.build();
                    let start = Instant::now();
                    let r = shamir.execute(DB, &req).await.unwrap();
                    total += start.elapsed();
                    black_box(r);
                }
                total
            });
        });
    }
    group.finish();
}

// --------------------------------------------------------------------------
// Group 2: read_by_size
// --------------------------------------------------------------------------

fn bench_read_by_size(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("read_by_size");

    for &(size, label) in SIZES {
        // Setup once: create a fresh DB with exactly one record at this
        // size; the read iter pulls it back by primary-key equality.
        let shamir = rt.block_on(async {
            let s = fresh_db().await;
            let mut bch = Batch::new();
            let id = format!("k_{label}");
            bch.id("seed")
                .insert("ins", write::insert(TABLE).row(one_big_row(&id, size)));
            s.execute(DB, &bch.build()).await.unwrap();
            s
        });
        let id = format!("k_{label}");

        group.throughput(Throughput::Bytes(size as u64));
        if label == "1mb" {
            group.sample_size(bu::sample_size(20));
        }
        group.bench_function(format!("value_{label}"), |b| {
            let shamir = Arc::clone(&shamir);
            let id = id.clone();
            b.to_async(&rt).iter(move || {
                let shamir = Arc::clone(&shamir);
                let id = id.clone();
                async move {
                    let mut bch = Batch::new();
                    bch.id("r").query(
                        "r",
                        Query::from(TABLE).where_(f::eq("id", id.clone())).limit(1),
                    );
                    let req = bch.build();
                    let r = shamir.execute(DB, &req).await.unwrap();
                    black_box(r);
                }
            });
        });
    }
    group.finish();
}

// --------------------------------------------------------------------------
// Group 3: decode_record_value_qv_large — subscription bridge de-intern
//
// Captures real msgpack `RecordChange.value` bytes (same path as the
// subscription hot-path) at the large-value end of the spectrum.
// --------------------------------------------------------------------------

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

fn bench_decode_record_value_qv_large(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("decode_record_value_qv_large");

    // Scenario A: one big string field, 10KB.
    let (db_a, bytes_a) = rt.block_on(capture_change_bytes(one_big_row("k", 10 * 1_024)));
    group.throughput(Throughput::Bytes(bytes_a.len() as u64));
    group.bench_function("string_10kb", |b| {
        b.to_async(&rt).iter(|| {
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
        });
    });

    // Scenario B: one big string field, 100KB.
    let (db_b, bytes_b) = rt.block_on(capture_change_bytes(one_big_row("k", 100 * 1_024)));
    group.throughput(Throughput::Bytes(bytes_b.len() as u64));
    group.bench_function("string_100kb", |b| {
        b.to_async(&rt).iter(|| {
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
        });
    });

    // Scenario C: ~10KB total spread across ~50 fields — distinct
    // interner pressure (many string-key lookups vs one big value).
    let (db_c, bytes_c) = rt.block_on(capture_change_bytes(one_wide_row("k", 10 * 1_024)));
    group.throughput(Throughput::Bytes(bytes_c.len() as u64));
    group.bench_function("nested_object_10kb", |b| {
        b.to_async(&rt).iter(|| {
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
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_insert_by_size,
    bench_read_by_size,
    bench_decode_record_value_qv_large,
);
criterion_main!(benches);
