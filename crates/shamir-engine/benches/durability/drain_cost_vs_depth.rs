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
//! The W ladder is a genuine scaling curve (the whole point is to see
//! whether drain cost is O(W) or O(W²) — see "cost vs depth" in the
//! title), so the larger tiers are NOT deleted. Instead the smallest tier
//! (W=1000) is the default in the fast sweep, and the full ladder is
//! available on demand via `BENCH_DRAIN_DEPTH_SCALING=1`. Note: even the
//! smallest tier calibrates low (1-2 iters at 0.05s) because each
//! iteration provisions a FRESH tempdir + fjall backend AND seeds W WAL
//! entries — that fs-I/O setup floor is a legitimate per-call cost that
//! can't be shrunk without changing the workload's semantics, so it is
//! accepted as a documented I/O-bound exception rather than forced under
//! the ~10ms/call target.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): each
//! iteration provisions a FRESH tempdir + fjall backend and seeds W
//! inflight WAL entries — that seeded state is consumed by `drain_all`
//! (drained entries can't be redrained), so setup must be fresh every
//! iteration → `bench_batched_async`.

use bench_scale_tool::Harness;
use shamir_engine::repo::{repo_token, BoxRepoFactory, RepoInstance};
use shamir_engine::table::table_manager::table_token_for;
use shamir_engine::table::TableConfig;
use shamir_engine::tx::drainer::Drainer;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

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

        wal.begin_grouped(&entry, WalDurability::Buffered)
            .await
            .unwrap();
    }

    // Set the gate so visibility = W, durable = 0.
    gate.publish_committed_max(w);
}

fn main() {
    let mut h = Harness::new("drain_cost_vs_depth", env!("CARGO_MANIFEST_DIR"));

    // The W ladder is a genuine scaling curve (O(W) vs O(W²) — see the
    // module docs). The smallest tier is the default so the fast sweep
    // stays bounded; the full ladder is opt-in via
    // `BENCH_DRAIN_DEPTH_SCALING`. Even the smallest tier is fs-I/O-bound
    // (fresh tempdir + fjall + W seeded entries per call) — see the
    // documented I/O exception in the module docs.
    let wide = std::env::var("BENCH_DRAIN_DEPTH_SCALING")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    let depths: &[u64] = if wide {
        &[1_000, 5_000, 20_000]
    } else {
        &[1_000]
    };

    for &w in depths {
        h.bench_batched_async(
            &format!("fjall/{w}"),
            move || async move {
                let (repo, dir) = make_fjall_repo().await;
                seed_inflight_entries(&repo, w).await;
                (repo, dir)
            },
            move |(repo, dir)| async move {
                // The drainer starts with an empty window — drain_step
                // will hit the gap-reseed path (wal.recover()) once, then
                // drain from the window.
                let drainer = Drainer::new();
                let drained: usize = drainer.drain_all(&repo).await.unwrap();

                assert_eq!(
                    drained as u64, w,
                    "expected to drain {w} entries, got {drained}"
                );

                drop(repo);
                drop(dir);
            },
        );
    }

    h.run();
}
