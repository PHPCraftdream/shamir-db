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
//!   - **Concurrency:** {8} writers (was {8, 32, 128} — collapsed to the
//!     smallest variant when migrated to the fixed-iteration harness).
//!   - **Batch size:** {1} rows per commit (was {1, 10, 100} — collapsed
//!     to the smallest variant).
//!
//! Each cell builds ONE repo upfront (untimed), then runs many iterations
//! against the SAME repo. The table grows monotonically — that's
//! intentional, it reflects steady-state DB behavior.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): the repo
//! is built once at registration time and shared across every iteration
//! (matches the original Criterion setup, which built it once outside
//! `b.iter`) → `bench_async`.

use std::sync::Arc;

use bench_scale_tool::Harness;
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

/// Spawns `writers` concurrent tasks, each performing one transaction with
/// `batch_size` row inserts and one commit.
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
    for hd in handles {
        hd.await.unwrap();
    }
}

fn main() {
    let mut h = Harness::new("backend_matrix", env!("CARGO_MANIFEST_DIR"));

    let backends: &[&'static str] = &["in_memory", "fjall"];
    // This IS a matrix bench — its whole purpose is comparing throughput
    // across concurrency (writers) × batch-size × backend, a genuine
    // structural comparison, not an artificial per-op loop the harness's
    // own repetition count already covers. Default sweep keeps the
    // smallest cell (writers=8, batch=1, ~0.2-0.8ms/call); the full
    // `[8,32,128] × [1,10,100]` matrix (some cells up to ~25ms/call —
    // fjall/w128/b100 is also partially I/O-bound/fsync-dominated) is
    // opt-in via BENCH_BACKEND_MATRIX_SCALING=1 so the cross-axis
    // comparison isn't lost, just not in the default fast path.
    let wide = std::env::var("BENCH_BACKEND_MATRIX_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let writers_levels: &[usize] = if wide { &[8, 32, 128] } else { &[8] };
    let batch_sizes: &[usize] = if wide { &[1, 10, 100] } else { &[1] };

    // Persistent tempdirs must outlive their repo's lifetime across every
    // iteration — kept alive by leaking into a Vec held by main's stack
    // frame (the harness runs everything before returning).
    let mut _dirs: Vec<Option<tempfile::TempDir>> = Vec::new();
    let rt = rt();
    let iter_ctr = Arc::new(std::sync::atomic::AtomicU64::new(0));

    for &backend in backends {
        for &writers in writers_levels {
            for &batch_size in batch_sizes {
                let (repo, dir) = rt.block_on(make_repo(backend));
                _dirs.push(dir);
                let repo = Arc::new(repo);
                let id_name = format!("{backend}/w{writers}/b{batch_size}");
                let iter_ctr = Arc::clone(&iter_ctr);
                h.bench_async(&id_name, move || {
                    let repo = repo.clone();
                    let iter_i = iter_ctr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    async move {
                        run_burst(repo, writers, batch_size, iter_i).await;
                    }
                });
            }
        }
    }

    h.run();

    // Keep tempdirs alive until after the run completes.
    drop(_dirs);
}
