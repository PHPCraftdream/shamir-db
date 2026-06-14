//! D0b — Durable-backend concurrent-commit baseline.
//!
//! Measures the wall-clock cost of N concurrent committers against a
//! RAW (un-MemBuffer-wrapped) sled backend, which fsyncs on every commit.
//! This is the "before GroupFsync" baseline that D1c/D1d will later
//! compare against.
//!
//! Two access patterns × three concurrency levels:
//!   - `same_table/n_{1,8,32}` — all N writers commit to the SAME table.
//!   - `disjoint_tables/n_{1,8,32}` — each writer commits to its own table.
//!
//! fsync-count: sled does not expose a public fsync counter. Wall-clock
//! alone is the signal. At N=1 the per-commit cost is dominated by a
//! single fsync (~0.5–2 ms on spinning rust, <0.1 ms on NVMe). As N
//! grows, same-table serialises on the commit gate AND under the
//! unique-write-lock during materialize (the D2 bottleneck documented in
//! durability-model.md); disjoint-tables exposes the pure fsync fan-out
//! overhead without intra-table serialisation. The ratio
//! `same[N] / disjoint[N]` is the empirical weight of the D2 bottleneck;
//! the ratio `disjoint[N] / (N * disjoint[1])` is the fsync-fan-out
//! overhead (ideal = 1.0, >1.0 means OS queue saturation).
//!
//! Each Criterion iteration:
//!   * provisions a fresh tempdir + sled-raw RepoInstance (no MemBuffer),
//!   * spawns N tasks that each run one insert_tx + commit_tx,
//!   * joins all tasks and records wall-clock.
//!
//! NOTE: Engine-internal APIs (`insert_tx` / `begin_tx` / `commit_tx`)
//! are used directly, following the same pattern as `tx_concurrent.rs`.
//! The query-builder exception applies: these are the engine's typed entry
//! points, not user-facing query construction.

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_engine::repo::{BoxRepoFactory, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

fn rt() -> tokio::runtime::Runtime {
    // Multi-thread: concurrent tasks must actually run in parallel.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Build a sled-raw (no MemBuffer) RepoInstance with `table_count` tables,
/// inside a freshly created tempdir. Returns (instance, _tempdir) — the
/// tempdir is kept alive via the returned guard; drop it after the repo.
async fn make_durable_repo(table_count: usize) -> (RepoInstance, tempfile::TempDir) {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let tables: Vec<TableConfig> = (0..table_count)
        .map(|i| TableConfig::new(format!("tbl_{}", i)))
        .collect();
    let factory = BoxRepoFactory::sled_raw(tempdir.path().to_path_buf());
    let repo = RepoInstance::from_factory("bench".into(), factory, tables)
        .await
        .expect("RepoInstance::from_factory");
    // Eagerly open all table managers so the timed window doesn't pay DDL.
    for i in 0..table_count {
        repo.get_table(&format!("tbl_{}", i))
            .await
            .expect("get_table");
    }
    (repo, tempdir)
}

// ── same-table: all N writers commit to `tbl_0` ────────────────────────────

fn bench_same_table(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("durable_concurrent_commit/same_table");
    // QUICK: sample_size=10, measurement=1 s, warm_up=1 s.
    bu::tune(&mut group, 10, 1, 1);

    for &n in &[1usize, 8, 32] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("n", n), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for iter_i in 0..iters {
                    // Fresh durable repo per iter — avoids inter-iter
                    // state bleed on the sled tree.
                    let (repo, _dir) = make_durable_repo(1).await;

                    let start = Instant::now();
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let repo = repo.clone();
                        // Unique value per (iter, writer) → no key collisions.
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
            });
        });
    }

    group.finish();
}

// ── disjoint-tables: writer w commits to `tbl_{w}` ─────────────────────────

fn bench_disjoint_tables(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("durable_concurrent_commit/disjoint_tables");
    bu::tune(&mut group, 10, 1, 1);

    for &n in &[1usize, 8, 32] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("n", n), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for iter_i in 0..iters {
                    let (repo, _dir) = make_durable_repo(n).await;

                    let start = Instant::now();
                    let mut handles = Vec::with_capacity(n);
                    for w in 0..n {
                        let repo = repo.clone();
                        let val = format!("v_{}_{}", iter_i, w);
                        let tbl_name = format!("tbl_{}", w);
                        handles.push(tokio::spawn(async move {
                            let (mut tx, _g) =
                                repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                            let tbl = repo.get_table(&tbl_name).await.unwrap();
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
                }
                total
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_same_table, bench_disjoint_tables);
criterion_main!(benches);
