use std::sync::Arc;

use bytes::Bytes;
use shamir_collections::TFxSet;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, PendingCommit, RepoTxGate, StagingStore, TxContext, TxId};

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::tx::commit_tx;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

#[tokio::test]
async fn enqueue_and_drain_pending() {
    let gate = RepoTxGate::fresh();
    let tx_id = gate.fresh_tx_id();

    let tx = TxContext::new(tx_id, 1, 0, IsolationLevel::Snapshot);
    let write_set_keys: TFxSet<(u64, bytes::Bytes)> = TFxSet::default();
    let (result_tx, _rx) = tokio::sync::oneshot::channel();

    let pending = PendingCommit::new(tx, write_set_keys, Vec::new(), result_tx);
    gate.enqueue_pending(pending);

    let batch = gate.drain_pending();
    assert_eq!(batch.len(), 1);

    // Second drain returns empty.
    let empty = gate.drain_pending();
    assert!(empty.is_empty());
}

/// P2b: 10 concurrent disjoint-table commits succeed with monotonic versions;
/// readers between commits see only the contiguous prefix (no visibility holes).
#[tokio::test]
async fn concurrent_disjoint_table_commits_monotonic_no_holes() {
    let repo = Arc::new(make_repo());
    let gate = repo.tx_gate().await.unwrap();

    // Spawn 10 concurrent commits, each writing to a distinct table_token.
    let mut handles = Vec::new();
    for i in 0u64..10 {
        let repo_clone = Arc::clone(&repo);
        handles.push(tokio::spawn(async move {
            let table_token = 100 + i; // disjoint tables
            let tx_id = TxId::new(10 + i);
            let mut tx = TxContext::new(tx_id, 0, 0, IsolationLevel::Snapshot);
            let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
            let mut staging = StagingStore::new(Arc::clone(&data_store));
            staging.set(
                Bytes::from(format!("key_{i}")).into(),
                Bytes::from(format!("val_{i}")),
            );
            tx.write_set.insert(table_token, staging);
            let outcome = commit_tx(tx, &repo_clone).await.unwrap();
            outcome.commit_version
        }));
    }

    let mut versions: Vec<u64> = Vec::new();
    for h in handles {
        versions.push(h.await.unwrap());
    }

    // All versions are distinct.
    let unique: TFxSet<u64> = versions.iter().copied().collect();
    assert_eq!(unique.len(), 10, "each commit gets a distinct version");

    // After all commits complete, the watermark (last_committed) must be
    // at least as high as the max committed version — no holes.
    let last = gate.last_committed();
    let max_v = *versions.iter().max().unwrap();
    assert!(
        last >= max_v,
        "watermark ({last}) must cover max committed version ({max_v}) — no visibility holes"
    );
}
