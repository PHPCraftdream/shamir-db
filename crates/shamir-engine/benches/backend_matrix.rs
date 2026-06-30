//! Backend-matrix steady-state throughput bench.
//!
//! Measures sustained ack-throughput of a RepoInstance under N concurrent
//! committers with a **fixed steady-state repo** (NOT fresh-per-iteration).
//! This is the honest measurement of what the DB can sustain — fresh-repo
//! setup overhead (fjall keyspace init, redb page table, etc.) was dominating
//! the original `drain_throughput` numbers (it included ~10-50ms of cold-start
//! setup per criterion iter on top of the actual commit work).
//!
//! Axes:
//!   - **Backend:** fjall / in_memory
//!     (durability OFF or default on each — our WAL is the single owner).
//!   - **Concurrency:** {8, 32, 128} writers.
//!   - **Batch size:** {1, 10, 100} rows per commit — single-row is worst case,
//!     batch is the realistic high-throughput path.
//!
//! Each `bench_function` creates ONE repo upfront, then criterion runs many
//! iterations against the SAME repo. The table grows monotonically — that's
//! intentional, it reflects steady-state DB behavior.

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_engine::repo::{BoxRepoFactory, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

// ── Backend repo factories ────────────────────────────────────────────────

/// Build a fresh `(RepoInstance, optional tempdir)` for the given backend.
/// The tempdir lives until the repo is dropped.
async fn make_repo(backend: &str) -> (RepoInstance, Option<tempfile::TempDir>) {
    let tempdir = if backend == "in_memory" {
        None
    } else {
        Some(tempfile::TempDir::new().expect("tempdir"))
    };
    let factory = match backend {
        "in_memory" => BoxRepoFactory::in_memory(),
        "fjall" => BoxRepoFactory::fjall_raw(tempdir.as_ref().unwrap().path().to_path_buf()),
        other => panic!("unknown backend: {}", other),
    };
    let repo = RepoInstance::from_factory(
        "bench".into(),
        factory,
        vec![TableConfig::new("tbl_0".to_string())],
    )
    .await
    .unwrap_or_else(|e| panic!("RepoInstance::from_factory ({backend}): {e:?}"));
    repo.get_table("tbl_0").await.expect("get_table tbl_0");
    (repo, tempdir)
}

// ── Bench cell — concurrent commits over a long-lived repo ────────────────

/// Spawns `writers` concurrent tasks, each performing one transaction with
/// `batch_size` row inserts and one commit. Returns the wall-clock duration.
async fn run_burst(repo: Arc<RepoInstance>, writers: usize, batch_size: usize, iter_i: u64) {
    let mut handles = Vec::with_capacity(writers);
    for w in 0..writers {
        let repo = repo.clone();
        handles.push(tokio::spawn(async move {
            let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
            let tbl = repo.get_table("tbl_0").await.unwrap();
            let values: Vec<InnerValue> = (0..batch_size)
                .map(|row| InnerValue::Str(format!("v_{}_{}_{}", iter_i, w, row)))
                .collect();
            tbl.insert_tx_many(&values, &mut tx).await.unwrap();
            (*repo).commit_tx(tx).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

fn bench_cell(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    backend: &'static str,
    writers: usize,
    batch_size: usize,
) {
    let rt = rt();
    // Build the repo ONCE; reuse across all criterion samples.
    let (repo, _dir) = rt.block_on(make_repo(backend));
    let repo = Arc::new(repo);
    // Total rows committed per sample = writers * batch_size.
    let total_per_sample = (writers * batch_size) as u64;
    group.throughput(Throughput::Elements(total_per_sample));
    let id_name = format!("{backend}/w{writers}/b{batch_size}");
    group.bench_function(BenchmarkId::from_parameter(id_name), |b| {
        b.to_async(&rt).iter_custom(|iters| {
            let repo = repo.clone();
            async move {
                let mut total = Duration::ZERO;
                for iter_i in 0..iters {
                    let repo = repo.clone();
                    let start = Instant::now();
                    run_burst(repo, writers, batch_size, iter_i).await;
                    total += start.elapsed();
                }
                total
            }
        });
    });
    // Keep _dir alive across the bench by binding it to a local; drop AFTER
    // criterion finishes the cell.
    drop(_dir);
}

fn bench_backend_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("backend_matrix");
    // QUICK: sample=15, measurement=2s, warm_up=1s — enough for warm caches.
    // FULL: sample=50, measurement=5s, warm_up=3s.
    bu::tune_tiered(&mut group, 50, 5, 3, 120);

    // 3 backends × 3 concurrency × 3 batch sizes = 27 cells.
    let backends: &[&'static str] = &["in_memory", "fjall"];
    let writers_levels: &[usize] = &[8, 32, 128];
    let batch_sizes: &[usize] = &[1, 10, 100];

    for &backend in backends {
        for &writers in writers_levels {
            for &batch_size in batch_sizes {
                bench_cell(&mut group, backend, writers, batch_size);
            }
        }
    }

    group.finish();
}

criterion_group!(benches, bench_backend_matrix);
criterion_main!(benches);
