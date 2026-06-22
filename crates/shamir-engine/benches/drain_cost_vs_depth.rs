//! Op #2 Stage 5 — drain cost vs WAL depth bench.
//!
//! Measures the wall-clock cost of `Drainer::drain_all(&repo)` over a
//! backlog of W inflight WAL entries on a fjall-backed repo.
//!
//! The bench seeds W entries into the WAL (via `begin_grouped`) WITHOUT
//! running the background drainer, then times one full `drain_all` pass.
//! This captures the O(W) vs O(W²) difference between the new window-based
//! drainer (Op #2 Stages 1-4) and the old `wal.recover()`-per-step drainer.
//!
//! Cells: `drain_cost_vs_depth/W={1000,5000,20000}/fjall`.
//!
//! Each Criterion iteration provisions a FRESH tempdir + fjall backend
//! (no inter-iter state bleed).

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_engine::repo::{repo_token, BoxRepoFactory, RepoInstance};
use shamir_engine::table::table_manager::table_token_for;
use shamir_engine::table::TableConfig;
use shamir_engine::tx::drainer::Drainer;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Create a fjall-backed RepoInstance with one table. Returns (instance, tempdir).
/// Does NOT access repo.drainer() — the background loop is never spawned.
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
    // Eagerly open the table manager so the timed window doesn't pay DDL.
    repo.get_table("tbl_0").await.expect("get_table tbl_0");
    (repo, tempdir)
}

/// Seed W inflight WAL entries (Put ops) on the repo WITHOUT triggering the
/// background drainer. Each entry gets a unique commit_version 1..=W and a
/// unique record id.
async fn seed_inflight_entries(repo: &RepoInstance, w: u64) {
    let wal = repo.repo_wal().await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let repo_id = repo_token(repo.name());
    let table_id = table_token_for("tbl_0");

    // Pre-compute a body once (content doesn't matter for drain cost).
    let body = InnerValue::Str("bench_payload".into()).to_bytes().unwrap();

    for v in 1..=w {
        let mut rid_bytes = [0u8; 16];
        rid_bytes[0..8].copy_from_slice(&v.to_le_bytes());
        let rid = RecordId(rid_bytes);

        let entry = WalEntryV2::new(
            wal.fresh_txn_id(),
            repo_id,
            vec![WalOpV2::Put {
                table_id_interned: table_id,
                rid,
                body: body.clone(),
            }],
        )
        .with_commit_version(v);

        wal.begin_grouped(entry, WalDurability::Buffered)
            .await
            .unwrap();
    }

    // Set the gate so visibility = W, durable = 0.
    gate.publish_committed_max(w);
}

fn bench_drain_cost_vs_depth(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("drain_cost_vs_depth");
    // QUICK mode: sample=10, measurement=500ms, warm_up=500ms.
    // max_wall_secs=600 per cell (10 min budget).
    bu::tune_tiered(&mut group, 10, 5, 3, 600);

    let depths: &[u64] = &[1_000, 5_000, 20_000];

    for &w in depths {
        group.throughput(Throughput::Elements(w));
        group.bench_function(BenchmarkId::new("fjall", w), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let mut total = Duration::ZERO;
                for _iter_i in 0..iters {
                    // Setup: fresh repo + seed W inflight entries.
                    let (repo, _dir) = make_fjall_repo().await;
                    seed_inflight_entries(&repo, w).await;

                    // Timed: create a standalone drainer and drain_all.
                    // The drainer starts with an empty window — drain_step
                    // will hit the gap-reseed path (wal.recover()) once,
                    // then drain from the window.
                    let drainer = Drainer::new();

                    let start = Instant::now();
                    let drained: usize = drainer.drain_all(&repo).await.unwrap();
                    total += start.elapsed();

                    assert_eq!(
                        drained as u64, w,
                        "expected to drain {w} entries, got {drained}"
                    );

                    drop(repo);
                    // _dir drops here, cleaning up the tempdir.
                }
                total
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_drain_cost_vs_depth);
criterion_main!(benches);
