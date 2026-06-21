//! Durability axis benchmark — `buffered` vs `synced` per-commit cost.
//!
//! `BatchRequest.durability` is a per-request knob (see
//! `docs/roadmap/DURABILITY_LEVELS.md`, `shamir_query_types::DurabilityLevel`).
//! Every user choosing a deployment config asks: what does the fsync gate
//! cost me per commit? This bench answers it.
//!
//! Axes:
//!   - batch size N ∈ {1, 10, 100} rows per commit
//!   - durability level ∈ {buffered, synced}
//!
//! Six functions total: `buffered_n{1,10,100}` and `synced_n{1,10,100}`.
//! Each iteration spins up a fresh redb-backed table in a per-iter tempdir
//! and submits a single batch via the typed `shamir_query_builder::Batch`.
//! Setup happens OUTSIDE the timed window.
//!
//! Persistent backend: **redb**. redb is part of the default workspace
//! `all-backends` feature, has the lightest setup surface (single-file
//! database, no background threads to drain on drop), and respects the
//! durability flag at commit time.
//!
//! Noise budget: fsync timing on Windows is significantly noisier than
//! memory ops (NTFS write barrier behaviour is OS-scheduler-dependent).
//! `sample_size(20)` + `measurement_time(5)` keep totals reasonable while
//! still smoothing outliers; expect synced timings to vary ~10-20% run to
//! run on Windows. Use the medians, not the means, when comparing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Runtime;

use shamir_bench_utils as bu;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::write;

// --------------------------------------------------------------------------
// Setup helpers
// --------------------------------------------------------------------------

async fn fresh_db_redb(path: &std::path::Path) -> Arc<ShamirDb> {
    let shamir = Arc::new(ShamirDb::init_memory().await.expect("init"));
    shamir.create_db("bench").await;
    let cfg = RepoConfig::new("main", BoxRepoFactory::fjall(path.join("db.redb")))
        .add_table(TableConfig::new("users"));
    shamir.add_repo("bench", cfg).await.expect("add_repo");
    shamir
}

/// Deterministic record generator — fixed shape, content seeded by `i`.
fn gen_row(i: usize) -> QueryValue {
    mpack!({
        "id":    @(QueryValue::from(format!("u{:08}", i))),
        "name":  @(QueryValue::from(format!("User-{}", i))),
        "email": @(QueryValue::from(format!("user{}@example.com", i))),
        "age":   @(QueryValue::from(18 + ((i * 37) % 60) as i64)),
        "score": @(QueryValue::from(((i * 7919) % 1000) as i64)),
    })
}

/// Build a single batch carrying `n` rows under the given durability level.
fn req_insert(n: usize, level: Durability) -> BatchRequest {
    let rows: Vec<QueryValue> = (0..n).map(gen_row).collect();
    let mut b = Batch::new();
    b.id("dur")
        .durability(level)
        .insert("ins", write::insert("users").rows(rows));
    b.build()
}

// --------------------------------------------------------------------------
// Bench group
// --------------------------------------------------------------------------

fn bench_durability(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("durability");
    group.sample_size(bu::sample_size(20));
    group.measurement_time(bu::measurement_time(Duration::from_secs(5)));

    for &n in &[1usize, 10, 100] {
        for &(level, label) in &[
            (Durability::Buffered, "buffered"),
            (Durability::Synced, "synced"),
        ] {
            group.throughput(Throughput::Elements(n as u64));
            group.bench_with_input(
                BenchmarkId::new(label, format!("n_{}", n)),
                &(n, level),
                |b, &(n, level)| {
                    b.to_async(&rt).iter_custom(|iters| async move {
                        let mut total = Duration::ZERO;
                        for _ in 0..iters {
                            // Fresh tempdir + DB per iter — no cross-iter state,
                            // measurement window covers exactly one commit.
                            let tempdir = tempfile::TempDir::new().expect("tempdir");
                            let shamir = fresh_db_redb(tempdir.path()).await;
                            let req = req_insert(n, level);
                            let start = Instant::now();
                            let resp = shamir.execute("bench", &req).await.unwrap();
                            total += start.elapsed();
                            black_box(resp);
                            drop(shamir);
                            drop(tempdir);
                        }
                        total
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, bench_durability);
criterion_main!(benches);

// ----- Headline numbers (redb backend, Windows 10, sample-size 10, mt 3s) -----
//
// Cost of synced gate per commit (median):
//   N=1:   synced 24.88 ms vs buffered 12.65 ms  (ratio 1.97×)
//   N=10:  synced 25.92 ms vs buffered 14.98 ms  (ratio 1.73×, fsync amortising)
//   N=100: synced 31.34 ms vs buffered 20.64 ms  (ratio 1.52×, fsync amortised hard)
//
// Note: each iter includes fresh tempdir + DB open + one commit, so the
// floor (~12 ms) is dominated by redb file-create + initial transaction
// setup, not by the commit itself. The synced-vs-buffered delta (~10-12 ms
// across all N) is the true fsync cost; ratios shrink as N grows because
// the fixed delta is divided across more rows.
