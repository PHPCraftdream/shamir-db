use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::{IsolationLevel, TxContext, TxId};

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
