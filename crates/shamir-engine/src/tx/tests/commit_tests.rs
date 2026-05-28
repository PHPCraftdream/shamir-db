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

#[tokio::test]
async fn commit_empty_tx_succeeds() {
    let repo = make_repo();
    let tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert_eq!(outcome.tx_id, 1);
    assert!(outcome.commit_version > 0, "version must be assigned");
}

#[tokio::test]
async fn commit_advances_last_committed() {
    let repo = make_repo();
    let gate = repo.tx_gate().await.unwrap();
    let before = gate.last_committed();

    let tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);
    let outcome = commit_tx(tx, &repo).await.unwrap();

    let after = gate.last_committed();
    assert!(after >= outcome.commit_version);
    assert!(after >= before);
}

#[tokio::test]
async fn commit_writes_then_clears_wal_entry() {
    let repo = make_repo();
    let wal = repo.repo_wal().await.unwrap();

    let tx = TxContext::new(TxId::new(3), 0, 0, IsolationLevel::Snapshot);
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

    let tx1 = TxContext::new(TxId::new(10), 0, 0, IsolationLevel::Snapshot);
    let o1 = commit_tx(tx1, &repo).await.unwrap();

    let tx2 = TxContext::new(TxId::new(11), 0, 0, IsolationLevel::Snapshot);
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
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);
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
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);
}

#[tokio::test]
async fn commit_serializable_with_empty_read_set_succeeds() {
    use shamir_tx::{IsolationLevel, TxContext, TxId};
    let repo = make_repo();
    let tx = TxContext::new(TxId::new(500), 0, 0, IsolationLevel::Serializable);
    // empty read_set + zero provider → passes Phase 2.
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);
}

#[tokio::test]
async fn commit_serializable_with_read_set_passes_zero_provider_scaffold() {
    // Until Stage 4.D.6 plugs in a real version provider, the scaffold
    // uses `|_, _| 0`. Any tx with non-empty read_set still passes
    // commit because 0 ≤ version_seen trivially.
    use shamir_tx::{IsolationLevel, TxContext, TxId};
    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(501), 0, 0, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"key"), 5);
    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);
}

#[tokio::test]
async fn commit_serializable_real_provider_detects_conflict() {
    use bytes::Bytes;
    use shamir_tx::{IsolationLevel, TxContext, TxId, VersionProvider};
    use std::sync::Arc;

    struct ConflictProvider;
    impl VersionProvider for ConflictProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> u64 {
            999
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
        fn version_of(&self, _t: u64, _k: &Bytes) -> u64 {
            5
        }
    }

    let repo = make_repo();
    let mut tx = TxContext::new(TxId::new(701), 0, 10, IsolationLevel::Serializable);
    tx.record_read(7, Bytes::from_static(b"k"), 5);
    tx.set_version_provider(Arc::new(OkProvider));

    let outcome = commit_tx(tx, &repo).await.unwrap();
    assert!(outcome.commit_version > 0);
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
    let outcome = repo.commit_tx(tx).await.unwrap();
    assert!(outcome.commit_version > 0);
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

    let put_found = ops
        .iter()
        .any(|op| matches!(op, WalOpV2::Put { rid, .. } if *rid == rid_set));
    let del_found = ops
        .iter()
        .any(|op| matches!(op, WalOpV2::Delete { rid } if *rid == rid_del));
    assert!(put_found, "expected WalOpV2::Put for staged Set");
    assert!(del_found, "expected WalOpV2::Delete for staged Remove");
}
