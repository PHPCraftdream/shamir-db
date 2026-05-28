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
    RepoInstance::new(BoxRepo::InMemory(repo), Vec::new())
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
    assert_eq!(tx.repo_id, 0);
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
