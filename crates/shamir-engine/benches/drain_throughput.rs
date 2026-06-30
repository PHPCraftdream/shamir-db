//! Phase 2 — Backend-matrix drain-throughput bench.
//!
//! Measures sustained ack-throughput of a RepoInstance under N concurrent
//! committers, parametrised by backend (fjall). Durability is
//! NOT owned by the backend in our architecture (the WAL is the single
//! durable owner), so each backend runs with its cheapest persist mode:
//!
//!   - **fjall:** `PersistMode::Buffered` (set in storage_fjall by default).
//!
//! Concurrency levels: {8, 32, 128} writers on a single shared table.
//!
//! Metric: `Throughput::Elements(writers)` — total commits per sample.
//!
//! Each Criterion iteration provisions a **fresh** tempdir + backend so
//! inter-iter state bleed is impossible and the table never grows across
//! samples.

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

async fn make_fjall_repo() -> (RepoInstance, tempfile::TempDir) {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
    let repo = RepoInstance::from_factory(
        "bench".into(),
        factory,
        vec![TableConfig::new("tbl_0".to_string())],
    )
    .await
    .expect("RepoInstance::from_factory (fjall)");
    repo.get_table("tbl_0").await.expect("get_table tbl_0");
    (repo, tempdir)
}

// ── Core bench loop ───────────────────────────────────────────────────────

/// Runs the concurrent-commit workload for a single (backend, writers) cell.
/// `make_repo` is the backend-specific factory.
fn bench_backend<F, Fut>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    backend_name: &str,
    writers: usize,
    make_repo: F,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = (RepoInstance, tempfile::TempDir)> + Send,
{
    let rt = rt();
    group.throughput(Throughput::Elements(writers as u64));
    group.bench_function(BenchmarkId::new(backend_name, writers), |b| {
        b.to_async(&rt).iter_custom(|iters| {
            let make = &make_repo;
            async move {
                let mut total = Duration::ZERO;
                for iter_i in 0..iters {
                    let (repo, _dir) = make().await;

                    let start = Instant::now();
                    let mut handles = Vec::with_capacity(writers);
                    for w in 0..writers {
                        let repo = repo.clone();
                        let val = format!("v_{}_{}", iter_i, w);
                        handles.push(tokio::spawn(async move {
                            let (mut tx, _g) =
                                repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                            let tbl = repo.get_table("tbl_0").await.unwrap();
                            tbl.insert_tx(&InnerValue::Str(val), Some(&mut tx))
                                .await
                                .unwrap();
                            repo.commit_tx(tx).await.unwrap();
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
                    total += start.elapsed();

                    drop(repo);
                    // _dir drops here, cleaning up the tempdir.
                }
                total
            }
        });
    });
}

// ── Benchmark functions ───────────────────────────────────────────────────

fn bench_drain_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("drain_throughput");
    // QUICK mode: sample=10, measurement=1s, warm_up=1s.
    bu::tune_tiered(&mut group, 10, 1, 1, 30);

    let concurrency_levels: &[usize] = &[8, 32, 128];

    for &writers in concurrency_levels {
        bench_backend(&mut group, "fjall", writers, make_fjall_repo);
    }

    group.finish();
}

criterion_group!(benches, bench_drain_throughput);
criterion_main!(benches);
