//! Hidden-O(N) Stage 0 probe: realistic overlay depth on a live repo.
//!
//! The companion bench `overlay_gc_cost_vs_depth` (shamir-tx) shows
//! `gc_upto` is O(total entries) in the small-slice case (14× at 20k vs
//! 1k). Whether Stage 1 (version-major index) is justified depends on
//! whether realistic overlay depth ever reaches the regime where that
//! cliff bites.
//!
//! This file is MEASUREMENT, not correctness. Each probe drives a live
//! `InMemory` repo and prints the observed overlay depth(s) to stderr —
//! the Stage 0 verdict reads these numbers from the test run.

use shamir_collections::TMap;
use shamir_engine::repo::repo_types::BoxRepoFactory;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::table_manager::table_token_for;
use shamir_engine::table::{TableConfig, TableManager};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

const TABLE: &str = "probe";

async fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let r = RepoInstance::new("probe".into(), BoxRepo::InMemory(repo), Vec::new());
    r.add_table(TableConfig::new(TABLE));
    r
}

async fn make_fjall_repo(tempdir: &tempfile::TempDir) -> RepoInstance {
    let factory = BoxRepoFactory::fjall_raw(tempdir.path().join("probe.fjall"));
    RepoInstance::from_factory("probe".into(), factory, vec![TableConfig::new(TABLE)])
        .await
        .expect("open fjall repo")
}

async fn field_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn text_record(body_key_id: u64, text: &str) -> InnerValue {
    let mut m: TMap<InternerKey, InnerValue> = TMap::default();
    m.insert(InternerKey::new(body_key_id), InnerValue::Str(text.into()));
    InnerValue::Map(m)
}

async fn overlay_len(repo: &RepoInstance) -> usize {
    repo.per_table_mvcc()
        .read_async(&table_token_for(TABLE), |_, m| m.overlay_len())
        .await
        .unwrap_or(0)
}

/// Probe A: steady-state — sequential commits with the background
/// drainer enabled. Measures peak overlay depth across a 10K-tx burst.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_steady_state_overlay_depth() {
    let repo = make_repo().await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;

    let mut peak = 0usize;
    let mut samples: Vec<(usize, usize)> = Vec::new();

    for i in 0..10_000u64 {
        let rec = text_record(body_id, &format!("steady-{i}"));
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tbl.insert_tx(&rec, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(guard);

        if i % 100 == 0 {
            let d = overlay_len(&repo).await;
            if d > peak {
                peak = d;
            }
            if i < 1000 || i % 1000 == 0 {
                samples.push((i as usize, d));
            }
        }
    }

    let pre_drain = overlay_len(&repo).await;
    repo.drainer().drain_all(&repo).await.unwrap();
    let post_drain = overlay_len(&repo).await;

    eprintln!(
        "STAGE0_PROBE_A steady_state: peak_during_burst={peak} \
         pre_drain={pre_drain} post_drain={post_drain} samples={samples:?}"
    );
    assert!(
        peak > 0 || pre_drain > 0,
        "probe drove no overlay state at all — fixture is broken"
    );
}

/// Probe B: drain-lag pathology — tight burst with no waits beyond
/// the commit path itself. Equilibrium of "drainer keeps up vs not".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_drain_lag_overlay_depth() {
    let repo = make_repo().await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;

    let mut peak = 0usize;
    let mut samples: Vec<(usize, usize)> = Vec::new();

    for i in 0..10_000u64 {
        let rec = text_record(body_id, &format!("burst-{i}"));
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tbl.insert_tx(&rec, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(guard);

        if i % 50 == 0 {
            let d = overlay_len(&repo).await;
            if d > peak {
                peak = d;
            }
            if i % 500 == 0 {
                samples.push((i as usize, d));
            }
        }
    }

    let pre_drain = overlay_len(&repo).await;
    repo.drainer().drain_all(&repo).await.unwrap();
    let post_drain = overlay_len(&repo).await;

    eprintln!(
        "STAGE0_PROBE_B drain_lag: peak={peak} \
         pre_drain={pre_drain} post_drain={post_drain} samples={samples:?}"
    );
}

/// Probe C: pinned snapshot — hold an open `begin_tx` Snapshot for the
/// whole burst (its guard pins `min_alive` to the snapshot floor).
/// Overlay GC can drop entries up to the durable watermark, but if the
/// pinned snapshot is at version 0, the floor stays low and overlay
/// reclamation is bounded only by `durable_watermark`, not `min_alive`
/// — measure both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_pinned_snapshot_overlay_depth() {
    let repo = make_repo().await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;

    // Hold a Snapshot-isolation tx open through the burst; its guard
    // keeps `min_alive` pinned at the snapshot's version.
    let (_pinned_tx, pinned_guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    for i in 0..5_000u64 {
        let rec = text_record(body_id, &format!("pinned-{i}"));
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tbl.insert_tx(&rec, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(guard);
    }

    repo.drainer().drain_all(&repo).await.unwrap();
    let depth_with_pinned = overlay_len(&repo).await;

    drop(pinned_guard);
    // Another drain pass — gc_overlay_to fires at the tail with the now-
    // higher floor.
    repo.drainer().drain_all(&repo).await.unwrap();
    let depth_after_release = overlay_len(&repo).await;

    eprintln!(
        "STAGE0_PROBE_C pinned_snapshot: depth_with_pinned={depth_with_pinned} \
         depth_after_release={depth_after_release}"
    );
}

/// Probe D (load-bearing): same burst as Probe B but on the REAL durable
/// backend (fjall). InMemory drain has zero I/O cost so the drainer
/// always keeps up at depth 1; fjall drain pays disk write + fsync per
/// batch, so under sustained writes the overlay window can actually
/// grow. This is the measurement that decides whether the O(total) cliff
/// in `gc_upto` bites in production.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probe_drain_lag_overlay_depth_fjall() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let repo = make_fjall_repo(&tempdir).await;
    let tbl = repo.get_table(TABLE).await.unwrap();
    let body_id = field_id(&tbl, "body").await;

    let mut peak = 0usize;
    let mut samples: Vec<(usize, usize)> = Vec::new();

    // 10K burst is overkill on fjall (each commit ~ms). Use 2K to keep
    // wall-clock bounded under nextest's per-test 180s slow-timeout.
    const N: u64 = 2_000;
    for i in 0..N {
        let rec = text_record(body_id, &format!("burst-{i}"));
        let (mut tx, guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
        tbl.insert_tx(&rec, Some(&mut tx)).await.unwrap();
        repo.commit_tx(tx).await.unwrap();
        drop(guard);

        if i % 25 == 0 {
            let d = overlay_len(&repo).await;
            if d > peak {
                peak = d;
            }
            if i % 200 == 0 {
                samples.push((i as usize, d));
            }
        }
    }

    let pre_drain = overlay_len(&repo).await;
    repo.drainer().drain_all(&repo).await.unwrap();
    let post_drain = overlay_len(&repo).await;

    eprintln!(
        "STAGE0_PROBE_D drain_lag_fjall N={N}: peak={peak} \
         pre_drain={pre_drain} post_drain={post_drain} samples={samples:?}"
    );
}
