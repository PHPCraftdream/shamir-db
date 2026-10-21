//! D0b — Durable-backend concurrent-commit baseline.
//!
//! Measures the wall-clock cost of N concurrent committers against a
//! RAW (un-MemBuffer-wrapped) fjall backend, which fsyncs on every commit.
//! This is the "before GroupFsync" baseline that D1c/D1d will later
//! compare against.
//!
//! Two access patterns × three concurrency levels:
//!   - `same_table/n_{1,8,32}` — all N writers commit to the SAME table.
//!   - `disjoint_tables/n_{1,8,32}` — each writer commits to its own table.
//!
//! The N ladder is a genuine concurrency/contention scaling curve (the
//! `same[N]/disjoint[N]` and `disjoint[N]/(N*disjoint[1])` ratios ARE the
//! measurement — see below), so the larger tiers are NOT deleted.
//! Instead the smallest tier (N=1) is the default in the fast sweep, and
//! the full ladder is available on demand via
//! `BENCH_DURABLE_COMMIT_SCALING=1`. Note: even N=1 calibrates low
//! (1-2 iters at 0.05s) because each iteration provisions a FRESH
//! tempdir + fjall backend AND pays a real fsync on commit — that
//! fs-I/O floor is a legitimate per-call cost (the bench's whole subject
//! is fsync behaviour) that can't be shrunk without changing the
//! workload's semantics, so it is accepted as a documented I/O-bound
//! exception rather than forced under the ~10ms/call target.
//!
//! fsync-count: fjall does not expose a public fsync counter. Wall-clock
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
//! NOTE: Engine-internal APIs (`insert_tx` / `begin_tx` / `commit_tx`)
//! are used directly, following the same pattern as `tx_concurrent.rs`.
//! The query-builder exception applies: these are the engine's typed entry
//! points, not user-facing query construction.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`). Each
//! iteration provisions a FRESH tempdir + fjall-raw RepoInstance (no
//! MemBuffer) — this cannot be shared across iterations without a real
//! fsync-cost being amortised away by state accumulation — so every cell
//! uses `bench_batched_async`: setup builds the fresh repo, the routine
//! spawns N committers and joins them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepoFactory, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

/// Build a fjall-raw (no MemBuffer) RepoInstance with `table_count` tables,
/// inside a freshly created tempdir. Returns (instance, _tempdir) — the
/// tempdir is kept alive via the returned guard; drop it after the repo.
async fn make_durable_repo(table_count: usize) -> (RepoInstance, tempfile::TempDir) {
    let tempdir = tempfile::TempDir::new().expect("tempdir");
    let tables: Vec<TableConfig> = (0..table_count)
        .map(|i| TableConfig::new(format!("tbl_{}", i)))
        .collect();
    let factory = BoxRepoFactory::fjall_raw(tempdir.path().to_path_buf());
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

fn main() {
    let mut h = Harness::new("durable_concurrent_commit", env!("CARGO_MANIFEST_DIR"));

    // The N ladder is a genuine concurrency/contention scaling curve (the
    // ratios in the module docs ARE the measurement). The smallest tier
    // (N=1) is the default so the fast sweep stays bounded; the full
    // ladder is opt-in via `BENCH_DURABLE_COMMIT_SCALING`. Even N=1 is
    // fs-I/O-bound (fresh tempdir + fjall + real fsync per call) — see
    // the documented I/O exception in the module docs.
    let wide = std::env::var("BENCH_DURABLE_COMMIT_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let levels: &[usize] = if wide { &[1, 8, 32] } else { &[1] };

    // ── same-table: all N writers commit to `tbl_0` ─────────────────────
    for &n in levels {
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("same_table/n_{n}"),
            move || {
                let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                async move {
                    let (repo, dir) = make_durable_repo(1).await;
                    (repo, dir, iter_i)
                }
            },
            move |(repo, dir, iter_i)| async move {
                let mut handles = Vec::with_capacity(n);
                for w in 0..n {
                    let repo = repo.clone();
                    let val = format!("v_{}_{}", iter_i, w);
                    handles.push(tokio::spawn(async move {
                        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                        let tbl = repo.get_table("tbl_0").await.unwrap();
                        tbl.insert_tx(&InnerValue::Str(val), Some(&mut tx))
                            .await
                            .unwrap();
                        repo.commit_tx(tx).await.unwrap();
                    }));
                }
                for hd in handles {
                    hd.await.unwrap();
                }
                drop(repo);
                drop(dir);
            },
        );
    }

    // ── disjoint-tables: writer w commits to `tbl_{w}` ──────────────────
    for &n in levels {
        let ctr = Arc::new(AtomicU64::new(0));
        h.bench_batched_async(
            &format!("disjoint_tables/n_{n}"),
            move || {
                let iter_i = ctr.fetch_add(1, Ordering::Relaxed);
                async move {
                    let (repo, dir) = make_durable_repo(n).await;
                    (repo, dir, iter_i)
                }
            },
            move |(repo, dir, iter_i)| async move {
                let mut handles = Vec::with_capacity(n);
                for w in 0..n {
                    let repo = repo.clone();
                    let val = format!("v_{}_{}", iter_i, w);
                    let tbl_name = format!("tbl_{}", w);
                    handles.push(tokio::spawn(async move {
                        let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
                        let tbl = repo.get_table(&tbl_name).await.unwrap();
                        tbl.insert_tx(&InnerValue::Str(val), Some(&mut tx))
                            .await
                            .unwrap();
                        repo.commit_tx(tx).await.unwrap();
                    }));
                }
                for hd in handles {
                    hd.await.unwrap();
                }
                drop(repo);
                drop(dir);
            },
        );
    }

    h.run();
}
