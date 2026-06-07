use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::tx::commit_tx;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

// C6 empty-tx fast-path: an empty tx no longer assigns a version or writes
// the WAL. It commits as a pure in-memory no-op, reporting
// `commit_version == snapshot_version` and `Complete`. (Was: asserted a
// fresh `commit_version > 0`; that pre-C6 contract is now covered by the
// non-empty commit tests, e.g. `commit_phase5_applies_write_set_to_base_store`.)
#[tokio::test]
async fn commit_empty_tx_succeeds() {
    let repo = make_repo();
    let tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.tx_id, 1);
    assert_eq!(
        outcome.commit_version, outcome.snapshot_version,
        "empty fast-path pins commit_version to the snapshot version"
    );
    assert!(
        outcome.materialized(),
        "empty fast-path has nothing to materialize → Complete"
    );
}

#[tokio::test]
async fn commit_advances_last_committed() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();
    let before = gate.last_committed();

    // A non-empty tx (one staged write) so it crosses the commit point and
    // advances `last_committed`. An empty tx now takes the C6 fast-path and
    // intentionally does NOT advance the version (see `commit_empty_tx_succeeds`).
    let mut tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(Arc::clone(&data_store));
    staging
        .set(Bytes::from_static(b"k"), Bytes::from_static(b"v"))
        .await;
    tx.write_set.insert(2, staging);
    let outcome = commit_tx(tx, &repo).await.unwrap();

    let after = gate.last_committed();
    assert!(
        outcome.commit_version > before,
        "non-empty commit advances version"
    );
    assert!(after >= outcome.commit_version);
    assert!(after >= before);
}

#[tokio::test]
async fn commit_writes_then_clears_wal_entry() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    // Non-empty: an empty tx now takes the C6 fast-path and never writes the
    // WAL at all, so it could not exercise Phase 7 cleanup. Stage a write so
    // Phase 4 writes the marker and Phase 7 must remove it.
    let mut tx = TxContext::new(TxId::new(3), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(Arc::clone(&data_store));
    staging
        .set(Bytes::from_static(b"k"), Bytes::from_static(b"v"))
        .await;
    tx.write_set.insert(3, staging);
    let _ = commit_tx(tx, &repo).await.unwrap();

    let inflight = wal.list_inflight().await.unwrap();
    assert!(
        inflight.is_empty(),
        "phase 7 must remove the WAL entry after commit"
    );
}

#[tokio::test]
async fn commit_two_txs_monotonic_versions() {
    let repo = make_repo();

    // Both txs stage a write so each crosses the commit point and assigns a
    // version (empty txs now fast-path without consuming a version — C6).
    let staged = |k: &'static [u8]| {
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let s = StagingStore::new(data_store);
        let kb = Bytes::from_static(k);
        async move {
            s.set(kb, Bytes::from_static(b"v")).await;
            s
        }
    };

    let mut tx1 = TxContext::new(TxId::new(10), 0, 0, IsolationLevel::Snapshot);
    tx1.write_set.insert(10, staged(b"k1").await);
    let o1 = commit_tx(tx1, &repo).await.unwrap();

    let mut tx2 = TxContext::new(TxId::new(11), 0, 0, IsolationLevel::Snapshot);
    tx2.write_set.insert(11, staged(b"k2").await);
    let o2 = commit_tx(tx2, &repo).await.unwrap();

    assert!(o2.commit_version > o1.commit_version);
}

#[tokio::test]
async fn repo_begin_tx_returns_valid_context() {
    let repo = make_repo();
    let (tx, guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    assert_ne!(tx.repo_id, 0, "repo_id must be populated from repo_token");
    assert!(tx.tx_id.0 > 0, "fresh_tx_id must allocate");
    drop(guard);
}

#[tokio::test]
async fn repo_begin_then_commit_succeeds() {
    let repo = make_repo();
    let (tx, _guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    // An empty begin→commit takes the C6 fast-path: it commits as a no-op
    // with `commit_version == snapshot_version` (no version burned).
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn repo_two_concurrent_begin_tx_get_distinct_tx_ids() {
    let repo = make_repo();
    let (t1, _g1) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    let (t2, _g2) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    assert_ne!(t1.tx_id, t2.tx_id, "fresh_tx_id must be monotonic");
}

#[tokio::test]
async fn commit_phase5_applies_write_set_to_base_store() {
    let repo = make_repo();

    let mut tx = TxContext::new(TxId::new(100), 0, 0, IsolationLevel::Snapshot);
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(Arc::clone(&data_store));
    staging
        .set(Bytes::from_static(b"rid_1"), Bytes::from_static(b"payload"))
        .await;
    tx.write_set.insert(42, staging);

    assert!(
        data_store.get(Bytes::from_static(b"rid_1")).await.is_err(),
        "data_store must not have the key before commit"
    );

    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);

    let got = data_store.get(Bytes::from_static(b"rid_1")).await.unwrap();
    assert_eq!(got, Bytes::from_static(b"payload"));
}

#[tokio::test]
async fn commit_applies_multiple_tables_atomically() {
    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(200), 0, 0, IsolationLevel::Snapshot);

    let s1: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let s2: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let st1 = StagingStore::new(Arc::clone(&s1));
    st1.set(Bytes::from_static(b"a"), Bytes::from_static(b"1"))
        .await;
    tx.write_set.insert(1, st1);

    let st2 = StagingStore::new(Arc::clone(&s2));
    st2.set(Bytes::from_static(b"b"), Bytes::from_static(b"2"))
        .await;
    tx.write_set.insert(2, st2);

    let _ = commit_tx(tx, &repo).await.unwrap();

    assert_eq!(
        s1.get(Bytes::from_static(b"a")).await.unwrap(),
        Bytes::from_static(b"1")
    );
    assert_eq!(
        s2.get(Bytes::from_static(b"b")).await.unwrap(),
        Bytes::from_static(b"2")
    );
}

#[tokio::test]
async fn commit_empty_write_set_still_succeeds() {
    let repo = make_repo();
    let tx = TxContext::new(TxId::new(300), 0, 0, IsolationLevel::Snapshot);
    // C6: empty tx commits via the fast-path (no version, no WAL).
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn commit_serializable_with_empty_read_set_succeeds() {
    use shamir_tx::{IsolationLevel, TxContext, TxId};
    let repo = make_repo();
    let tx = TxContext::new(TxId::new(500), 0, 0, IsolationLevel::Serializable);
    // empty read_set + zero provider → passes Phase 2, then the empty
    // op-set takes the C6 fast-path (no version, no WAL).
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn commit_serializable_with_read_set_passes_zero_provider_scaffold() {
    // Until Stage 4.D.6 plugs in a real version provider, the scaffold
    // uses `|_, _| 0`. A tx with a non-empty read_set but NO writes still
    // passes Phase 2 (0 ≤ version_seen trivially), then takes the C6
    // read-only fast-path (read_set is not gated by `is_empty`).
    use shamir_tx::{IsolationLevel, TxContext, TxId};
    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(501), 0, 0, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"key"), 5);
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn commit_serializable_real_provider_detects_conflict() {
    use bytes::Bytes;
    use shamir_tx::{IsolationLevel, TxContext, TxId, VersionProvider};
    use std::sync::Arc;

    struct ConflictProvider;
    impl VersionProvider for ConflictProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            Some(999)
        }
    }

    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(700), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k"), 5);
    tx.set_version_provider(Arc::new(ConflictProvider));

    let result = commit_tx(tx, &repo).await;
    assert!(result.is_err(), "real provider with conflict must abort");
    match result.unwrap_err() {
        crate::tx::CommitError::SsiConflict { key } => {
            assert_eq!(key, Bytes::from_static(b"k"));
        }
        e => panic!("expected SsiConflict, got {:?}", e),
    }
}

#[tokio::test]
async fn commit_serializable_real_provider_no_conflict_succeeds() {
    use bytes::Bytes;
    use shamir_tx::{IsolationLevel, TxContext, TxId, VersionProvider};
    use std::sync::Arc;

    struct OkProvider;
    impl VersionProvider for OkProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            Some(5)
        }
    }

    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(701), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k"), 5);
    tx.set_version_provider(Arc::new(OkProvider));

    // Read-only Serializable tx, SSI validation passes (no conflict) → C6
    // fast-path. `commit_version` pins to the snapshot version (10).
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert_eq!(outcome.commit_version, 10);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn begin_tx_populates_repo_id_from_repo_token() {
    let repo = make_repo();
    let (tx, _guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    assert_ne!(tx.repo_id, 0, "repo_id should be populated from repo_token");
}

#[tokio::test]
async fn commit_runs_apply_id_remap_phase_1_with_empty_overlay() {
    // Sanity: commit with empty interner_overlay (default state)
    // succeeds — Phase 1 is wired but no-op.
    let repo = make_repo();
    let (tx, _guard) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    // Verify the overlay is empty (precondition).
    assert!(tx.interner_overlay.is_empty());
    // Empty tx → C6 fast-path (no version assigned).
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert_eq!(outcome.commit_version, outcome.snapshot_version);
    assert!(outcome.materialized());
}

#[tokio::test]
async fn commit_with_non_empty_overlay_proceeds_with_warning() {
    // Until Stage 5 wires LayeredInterner, a non-empty overlay
    // triggers the warning path but commit still succeeds with an
    // empty remap (overlay entries are ignored).
    use shamir_tx::{IsolationLevel, TxContext, TxId};

    let repo = make_repo();
    let tx = TxContext::new(TxId::new(900), 0, 0, IsolationLevel::Snapshot);
    let _ = tx.interner_overlay.insert("foo".to_string(), 12345);

    // Commit succeeds despite non-empty overlay (warning-only path).
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);
}

#[tokio::test]
async fn wal_ops_from_tx_emits_put_for_set_remove_for_remove() {
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::Store;
    use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId};
    use shamir_types::types::record_id::RecordId;
    use shamir_wal::WalOpV2;

    let mut tx = TxContext::new(TxId::new(801), 0, 0, IsolationLevel::Snapshot);
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = StagingStore::new(data);

    let rid_set = RecordId::new();
    let rid_del = RecordId::new();
    staging
        .set(rid_set.to_bytes(), Bytes::from_static(b"v"))
        .await;
    staging.remove(rid_del.to_bytes()).await;
    tx.write_set.insert(7, staging);

    let ops = crate::tx::commit::wal_ops_from_tx(&tx).await;

    let put_found = ops.iter().any(|op| {
        matches!(
            op,
            WalOpV2::Put {
                rid,
                table_id_interned,
                ..
            } if *rid == rid_set && *table_id_interned == 7
        )
    });
    let del_found = ops.iter().any(|op| {
        matches!(
            op,
            WalOpV2::Delete {
                rid,
                table_id_interned,
            } if *rid == rid_del && *table_id_interned == 7
        )
    });
    assert!(put_found, "expected WalOpV2::Put for staged Set");
    assert!(del_found, "expected WalOpV2::Delete for staged Remove");
}

// Originally named `ssi_conflict_detected_via_repo_version_provider` — the
// previous name implied conflict detection but the assertion (and reality)
// was the opposite under the OLD model where a non-tx insert did NOT bump the
// per-key version_cache.
//
// T1a reversed the HIGH-4 assumption: a non-tx insert NOW bumps the MVCC
// version and advances `last_committed`. The no-conflict expectation still
// holds, but for the RIGHT reason: the non-tx write PREDATES the tx's snapshot
// (the seed version ≤ tx.snapshot_version), so `version_of(key) ≤ snapshot`
// and `validate_read_set` sees no advance. The read is recorded at the real
// snapshot version (not a hardcoded 0).
#[tokio::test]
async fn ssi_no_conflict_when_only_non_tx_writes_predate_snapshot() {
    use crate::table::TableConfig;
    use shamir_tx::IsolationLevel;

    let repo = make_repo();
    repo.add_table(TableConfig::new("users"));
    let tbl = repo.get_table("users").await.unwrap();
    let rid = tbl
        .insert(&shamir_types::types::value::InnerValue::Str("v".into()))
        .await
        .unwrap();

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();

    let token = crate::table::table_manager::table_token_for("users");
    // T1a: non-tx writes now bump the MVCC version + advance last_committed (the HIGH-4 "non-tx doesn't bump" assumption is intentionally reversed), so record the read at the actual snapshot version, not a hardcoded 0.
    tx.record_read(token, rid.to_bytes(), tx.snapshot_version);

    let outcome = repo.commit_tx(tx).await;
    assert!(
        outcome.is_ok(),
        "no conflict expected — the non-tx insert predates the snapshot (version ≤ snapshot), so SSI sees no advance"
    );
}

#[tokio::test]
async fn expired_tx_rejected_at_commit() {
    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));

    let (mut tx, _g) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Backdoor: set started_at to the past to simulate expiry.
    tx.started_at = std::time::Instant::now() - std::time::Duration::from_secs(600);

    let result = repo.commit_tx(tx).await;
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("expired"),
        "expected expired error, got: {}",
        err
    );
}

#[tokio::test]
async fn tx_metrics_track_commit_and_abort() {
    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));

    // Start + commit a tx.
    let (tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();

    let snap = repo.tx_metrics().snapshot();
    assert!(snap.txs_started >= 1);
    assert!(snap.txs_committed >= 1);
    assert_eq!(snap.txs_aborted_ssi, 0);
}

// CRIT-1 regression test: recovery markers must survive a repo restart.
//
// The previous implementation only bumped the in-memory
// `last_committed_version` AtomicU64 in Phase 6 — on reopen the gate
// seeded from `MetaKey::LastCommittedVersion` (unset) → 0, so MVCC
// version monotonicity broke. Phase 6.5 in `commit_tx_inner` now
// persists both markers; this test exercises the round trip across
// drop+rebuild of `RepoInstance` over the same underlying `InMemoryRepo`.
#[tokio::test]
async fn last_committed_version_persists_across_restart() {
    let underlying = Arc::new(InMemoryRepo::new());
    let repo1 = RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo1.add_table(crate::table::TableConfig::new("t"));

    // Commit a few txs so commit_version advances past 0.
    for i in 0..3i64 {
        let (mut tx, _g) = repo1
            .begin_tx(shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        let tbl = repo1.get_table("t").await.unwrap();
        tbl.insert_tx(
            &shamir_types::types::value::InnerValue::Int(i),
            Some(&mut tx),
        )
        .await
        .unwrap();
        repo1.commit_tx(tx).await.unwrap();
    }
    let last_v_pre = repo1.tx_gate().await.unwrap().last_committed();
    assert!(
        last_v_pre > 0,
        "pre-restart gate must have advanced past zero"
    );

    drop(repo1);

    let repo2 = RepoInstance::new("test".into(), BoxRepo::InMemory(underlying), Vec::new());
    repo2.add_table(crate::table::TableConfig::new("t"));
    let last_v_post = repo2.tx_gate().await.unwrap().last_committed();
    assert_eq!(
        last_v_post, last_v_pre,
        "last_committed_version must survive restart (CRIT-1)"
    );
}

// CRIT-3 regression test: the happy-path commit must advance the
// table counter in memory so callers see post-commit data without
// waiting for recovery. Previously Phase 5b was a TODO and the WAL
// `CounterDelta` op was only applied during crash replay.
#[tokio::test]
async fn commit_tx_advances_table_counter() {
    let repo = make_repo();
    repo.add_table(crate::table::TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let before = tbl.counter().get().await.unwrap();

    let (mut tx, _g) = repo
        .begin_tx(shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    tbl.insert_tx(
        &shamir_types::types::value::InnerValue::Int(1),
        Some(&mut tx),
    )
    .await
    .unwrap();
    tbl.insert_tx(
        &shamir_types::types::value::InnerValue::Int(2),
        Some(&mut tx),
    )
    .await
    .unwrap();
    repo.commit_tx(tx).await.unwrap();

    let after = tbl.counter().get().await.unwrap();
    assert_eq!(
        after - before,
        2,
        "counter must reflect committed inserts (CRIT-3)"
    );
}

// ===========================================================================
// C6 — empty-tx fast-path
//
// A tx that staged nothing durable (read-only Serializable txs, or any tx
// whose write_set / index_write_set / staged_vectors / counter_deltas /
// interner_overlay are all empty) commits as a pure in-memory no-op: it does
// NOT assign a new MVCC version and does NOT write the WAL. The fast-path
// sits AFTER Phase 2 SSI validation, so a read-only Serializable tx that read
// stale data still ABORTS with `SsiConflict` (the version-assign + WAL +
// publish are the only steps skipped, never the SSI check).
// ===========================================================================

/// An empty tx burns no MVCC version. Proven behaviourally: commit a
/// non-empty tx (version V), then an empty tx, then another non-empty tx —
/// the second non-empty tx must get exactly V+1, not V+2. If the empty tx
/// had gone through the full pipeline it would have consumed a version and
/// the delta would be 2.
#[tokio::test]
async fn empty_tx_fast_path_assigns_no_version_and_no_wal() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let staged = |k: &'static [u8]| {
        let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let s = StagingStore::new(data_store);
        async move {
            s.set(Bytes::from_static(k), Bytes::from_static(b"v")).await;
            s
        }
    };

    // Real tx #1 → assigns version V.
    let mut tx1 = TxContext::new(TxId::new(6001), 0, 0, IsolationLevel::Snapshot);
    tx1.write_set.insert(6001, staged(b"k1").await);
    let v1 = commit_tx(tx1, &repo).await.unwrap().commit_version;
    assert!(v1 > 0);

    // Empty tx → fast-path: commit_version pinned to snapshot (0), no WAL.
    let snap_before = repo.tx_gate().await.unwrap().last_committed();
    let empty = TxContext::new(TxId::new(6002), 0, snap_before, IsolationLevel::Snapshot);
    let out = commit_tx(empty, &repo).await.unwrap();
    assert_eq!(
        out.commit_version, snap_before,
        "empty fast-path pins commit_version to the snapshot version"
    );
    assert!(out.materialized(), "empty fast-path is Complete");
    assert_eq!(
        repo.tx_gate().await.unwrap().last_committed(),
        snap_before,
        "empty fast-path must NOT advance last_committed (nothing published)"
    );
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "empty fast-path must write no WAL entry"
    );

    // Real tx #2 → must get V+1 (the empty tx consumed nothing).
    let mut tx3 = TxContext::new(TxId::new(6003), 0, 0, IsolationLevel::Snapshot);
    tx3.write_set.insert(6003, staged(b"k3").await);
    let v2 = commit_tx(tx3, &repo).await.unwrap().commit_version;
    assert_eq!(
        v2,
        v1 + 1,
        "the empty tx must not have consumed a version (expected {}, got {})",
        v1 + 1,
        v2
    );
}

/// A read-only Serializable tx whose read-set does NOT conflict passes SSI
/// validation and then takes the empty fast-path: it commits without a WAL
/// write and pins `commit_version` to the snapshot version.
#[tokio::test]
async fn read_only_serializable_no_conflict_fast_paths() {
    use shamir_tx::VersionProvider;

    struct OkProvider;
    impl VersionProvider for OkProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            // Equal to the version the tx read → no conflict.
            Some(5)
        }
    }

    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let mut tx = TxContext::new(TxId::new(6100), 0, 7, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k"), 5);
    tx.set_version_provider(Arc::new(OkProvider));

    let out = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(
        out.commit_version, 7,
        "read-only fast-path pins commit_version to the snapshot version"
    );
    assert!(out.materialized());
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "read-only fast-path must write no WAL entry"
    );
}

/// CRITICAL C6 invariant: the empty fast-path must NOT swallow an SSI
/// conflict. A read-only Serializable tx with a STALE read (a concurrent
/// committer advanced the key past what the tx saw) must still ABORT —
/// proving the fast-path sits AFTER Phase 2 validation, not before it.
#[tokio::test]
async fn read_only_serializable_with_conflict_still_aborts() {
    use shamir_tx::VersionProvider;

    struct ConflictProvider;
    impl VersionProvider for ConflictProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            // Far above what the tx read → SSI conflict.
            Some(999)
        }
    }

    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let mut tx = TxContext::new(TxId::new(6200), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k"), 5);
    tx.set_version_provider(Arc::new(ConflictProvider));

    let result = commit_tx(tx, &repo).await;
    match result {
        Err(crate::tx::CommitError::SsiConflict { key }) => {
            assert_eq!(key, Bytes::from_static(b"k"));
        }
        other => panic!(
            "a read-only SSI tx with a stale read must abort with SsiConflict, \
             not fast-path to success; got {:?}",
            other.map(|o| o.commit_version).map_err(|_| "Err(other)")
        ),
    }

    // The abort happened in Phase 2, before any WAL write.
    assert!(
        wal.list_inflight().await.unwrap().is_empty(),
        "a pre-commit SSI abort leaves no WAL entry"
    );
}
