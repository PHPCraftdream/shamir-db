//! SNAP-2: the contiguous completion watermark is an always-live GC pin.

use std::sync::Arc;

use bytes::Bytes;
use shamir_collections::TFxMap;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::RecordKey;

use crate::mvcc_store::{MvccStore, Retention};
use crate::predicate_set::PredicateDep;
use crate::repo_tx_gate::{CommitWriteRecord, RepoTxGate, TableWriteFootprint};
use crate::version_codec::encode_version_key;

#[test]
fn min_alive_without_pins_uses_completion_watermark() {
    let gate = RepoTxGate::new(9, 1);
    let parked = gate.assign_next_version_guarded();
    let later = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    assert_eq!(later.version(), 11);
    later.commit();
    gate.publish_committed_max(11);

    assert!(gate.active_snapshots_empty());
    assert_eq!(gate.completion().watermark(), 9);
    // Red before SNAP-2: eager last_committed (11) was the empty-set floor.
    assert_eq!(gate.min_alive(), 9);
    drop(parked);
    assert_eq!(gate.min_alive(), 11);
}

#[tokio::test]
async fn min_alive_keeps_pin_older_than_completion_watermark() {
    let gate = RepoTxGate::new(5, 1);
    let older = gate.open_snapshot().await;
    for expected in 6..=9 {
        let guard = gate.assign_next_version_guarded();
        assert_eq!(guard.version(), expected);
        guard.commit();
    }
    gate.publish_committed_max(12);

    assert_eq!(older.version(), 5);
    assert_eq!(gate.completion().watermark(), 9);
    assert_eq!(gate.min_alive(), 5);
    drop(older);
    // Red before SNAP-2 after releasing the older pin: returned 12, not W.
    assert_eq!(gate.min_alive(), 9);
}

#[tokio::test]
async fn min_alive_eager_pin_above_watermark_does_not_raise_floor() {
    let gate = RepoTxGate::new(9, 1);
    let parked = gate.assign_next_version_guarded();
    gate.publish_committed_max(11);
    // Index-build-style eager pin; snapshot assignment is not changed here.
    let eager = gate.open_snapshot().await;

    assert_eq!(eager.version(), 11);
    assert_eq!(gate.completion().watermark(), 9);
    // Red before SNAP-2: the sole active pin (11) displaced W (9).
    assert_eq!(gate.min_alive(), 9);
    drop(eager);
    drop(parked);
}

#[test]
fn min_alive_opening_barrier_still_protects_watermark_floor() {
    let gate = RepoTxGate::new(9, 1);
    gate.publish_committed_max(12);
    let opening = gate.test_hold_opening_barrier();
    assert_eq!(gate.min_alive(), 0);
    drop(opening);
    // Red before SNAP-2 after the barrier clears: returned eager 12.
    assert_eq!(gate.min_alive(), 9);
}

fn make_store(gate: &Arc<RepoTxGate>) -> MvccStore {
    let mvcc = MvccStore::new(Arc::new(InMemoryStore::new()), Arc::clone(gate));
    mvcc.set_test_now(1_000);
    mvcc
}

async fn seed_key_at_five(mvcc: &MvccStore, gate: &RepoTxGate) {
    assert_eq!(
        mvcc.set_versioned(
            RecordKey::from(Bytes::from_static(b"K")),
            Bytes::from("K@5")
        )
        .await
        .unwrap(),
        5,
    );
    // No writes to K while advancing the contiguous floor to 9.
    for expected in 6..=9 {
        let guard = gate.assign_next_version_guarded();
        assert_eq!(guard.version(), expected);
        guard.commit();
        gate.mark_durable(expected);
    }
    assert_eq!(gate.completion().watermark(), 9);
}

#[tokio::test]
async fn parked_v10_nontx_v11_v12_preserve_key_at_watermark() {
    let gate = Arc::new(RepoTxGate::new(4, 1));
    let mvcc = make_store(&gate);
    seed_key_at_five(&mvcc, &gate).await;
    let parked = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    assert!(gate.active_snapshots_empty());

    for version in 11..=12 {
        assert_eq!(
            mvcc.set_versioned(
                RecordKey::from(Bytes::from_static(b"K")),
                Bytes::from("new")
            )
            .await
            .unwrap(),
            version,
        );
    }
    assert_eq!(gate.completion().watermark(), 9);
    assert_eq!(mvcc.version_of(b"K"), 12);
    mvcc.gc().await.unwrap();

    // Explicitly register the old snapshot at W using the existing plain
    // publisher seam. Restore the eager publisher after registration; do not
    // change production register_snapshot as part of this GC-only leaf.
    let eager = gate.last_committed();
    let watermark = gate.completion().watermark();
    gate.publish_committed(watermark);
    let snapshot = gate.open_snapshot().await;
    gate.publish_committed_max(eager);
    assert_eq!(snapshot.version(), 9);
    // Red before SNAP-2: the v12 L6 vacuum deleted deferred K@5.
    assert_eq!(
        mvcc.get_at(b"K", snapshot.version()).await.unwrap(),
        Some(Bytes::from("K@5")),
    );
    drop(snapshot);
    drop(parked);
}

#[tokio::test]
async fn l6_retains_deferred_anchor_when_old_version_exceeds_watermark() {
    let gate = Arc::new(RepoTxGate::new(4, 1));
    let mvcc = make_store(&gate);
    seed_key_at_five(&mvcc, &gate).await;
    let parked = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    assert_eq!(
        mvcc.set_versioned(
            RecordKey::from(Bytes::from_static(b"K")),
            Bytes::from("K@11")
        )
        .await
        .unwrap(),
        11,
    );
    assert_eq!(
        mvcc.set_versioned(
            RecordKey::from(Bytes::from_static(b"K")),
            Bytes::from("K@12")
        )
        .await
        .unwrap(),
        12,
    );
    // Red before SNAP-2: old_v=11 > W=9 still swapped/deleted anchor 5.
    // Check durable history, not merely an overlay copy of the value.
    assert_eq!(
        mvcc.history_store()
            .get(encode_version_key(b"K", 5).into())
            .await
            .unwrap(),
        Bytes::from("K@5"),
    );
    // A subsequent write must not use an unsafe fast path after flag clearing.
    assert_eq!(
        mvcc.set_versioned(
            RecordKey::from(Bytes::from_static(b"K")),
            Bytes::from("K@13")
        )
        .await
        .unwrap(),
        13,
    );
    assert_eq!(
        mvcc.get_at(b"K", 9).await.unwrap(),
        Some(Bytes::from("K@5")),
    );
    drop(parked);
}

#[tokio::test]
async fn scan_vacuum_keeps_watermark_anchor_without_live_snapshots() {
    let gate = Arc::new(RepoTxGate::new(4, 1));
    let mvcc = make_store(&gate);
    seed_key_at_five(&mvcc, &gate).await;
    let parked = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    // A non-CurrentOnly policy forces the scan path independently of L6.
    mvcc.set_retention(Retention {
        max_age_secs: None,
        max_count: Some(1),
        min_count: None,
    })
    .unwrap();
    gate.publish_committed_max(12);
    for expected in 11..=12 {
        assert_eq!(
            mvcc.set_versioned(
                RecordKey::from(Bytes::from_static(b"K")),
                Bytes::from("new")
            )
            .await
            .unwrap(),
            expected,
        );
    }
    assert!(gate.active_snapshots_empty());
    assert_eq!(gate.completion().watermark(), 9);
    // Red before SNAP-2: no live snapshots meant no anchor and eager floor 12.
    assert_eq!(
        mvcc.get_at(b"K", 9).await.unwrap(),
        Some(Bytes::from("K@5")),
    );
    drop(parked);
}

#[tokio::test]
async fn ssi_v11_footprint_survives_gc_without_snapshots_and_conflicts_at_watermark() {
    let gate = Arc::new(RepoTxGate::new(9, 1));
    let mvcc = make_store(&gate);
    let parked = gate.assign_next_version_guarded();
    let later = gate.assign_next_version_guarded();
    assert_eq!(parked.version(), 10);
    assert_eq!(later.version(), 11);
    gate.record_commit_writes(CommitWriteRecord {
        commit_version: 11,
        per_table: TFxMap::from_iter([(
            42,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    });
    later.commit();
    gate.publish_committed_max(11);
    assert_eq!(gate.completion().watermark(), 9);
    assert!(gate.active_snapshots_empty());

    mvcc.gc().await.unwrap();
    // Mirror the engine GC tick's SSI prune call, with no registered snapshot.
    gate.prune_commit_log_below(gate.min_alive());
    gate.publish_committed(9);
    let transaction = gate.open_snapshot_serializable().await;
    gate.publish_committed_max(11);
    assert_eq!(transaction.version(), 9);
    let deps = [PredicateDep::TableScan { table_token: 42 }];
    // Red before SNAP-2: floor 11 pruned B's entire conflict footprint.
    assert_eq!(
        gate.predicate_conflicts_batch(&deps, transaction.version()),
        Some(0),
    );
    assert_eq!(gate.commit_log_len(), 1);
    assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 7 }, 9));
    drop(transaction);
    drop(parked);
}
