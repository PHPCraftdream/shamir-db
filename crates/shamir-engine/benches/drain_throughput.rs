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
//! The writers ladder is a genuine concurrency/contention scaling curve
//! (sustained ack-throughput under N concurrent committers), so the
//! larger tiers are NOT deleted. Instead the smallest tier (8 writers)
//! is the default in the fast sweep, and the full ladder is available
//! on demand via `BENCH_DRAIN_THROUGHPUT_SCALING=1`. Note: even the
//! smallest tier calibrates low (1-2 iters at 0.05s) because each
//! iteration provisions a FRESH tempdir + fjall backend — that fs-I/O
//! setup floor is a legitimate per-call cost that can't be shrunk
//! without changing the workload's semantics, so it is accepted as a
//! documented I/O-bound exception rather than forced under the
//! ~10ms/call target.
//!
//! Each iteration provisions a **fresh** tempdir + backend so inter-iter
//! state bleed is impossible and the table never grows across samples.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): a fresh
//! backend must be built every iteration (a shared, growing table would
//! change the workload's semantics across iterations), so every cell uses
//! `bench_batched_async` — setup builds the fresh repo, the routine spawns
//! and joins the N committers.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepoFactory, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_tx::IsolationLevel;
use shamir_types::types::value::InnerValue;

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

fn main() {
    let mut h = Harness::new("drain_throughput", env!("CARGO_MANIFEST_DIR"));

    // The writers ladder is a genuine concurrency/contention scaling
    // curve (see module docs). The smallest tier (8 writers) is the
    // default so the fast sweep stays bounded; the full ladder is
    // opt-in via `BENCH_DRAIN_THROUGHPUT_SCALING`. Even the smallest
    // tier is fs-I/O-bound (fresh tempdir + fjall per call) — see the
    // documented I/O exception in the module docs.
    let wide = std::env::var("BENCH_DRAIN_THROUGHPUT_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let levels: &[usize] = if wide { &[8, 32, 128] } else { &[8] };

    let ctr = Arc::new(std::sync::atomic::AtomicU64::new(0));

    for &writers in levels {
        let ctr = Arc::clone(&ctr);
        h.bench_batched_async(
            &format!("fjall/{writers}"),
            move || {
                let iter_i = ctr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async move {
                    let (repo, dir) = make_fjall_repo().await;
                    (repo, dir, iter_i)
                }
            },
            move |(repo, dir, iter_i)| async move {
                let mut handles = Vec::with_capacity(writers);
                for w in 0..writers {
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

    h.run();
}
