use std::collections::HashSet;

use shamir_collections::THasher;
use shamir_tx::{IsolationLevel, PendingCommit, RepoTxGate, TxContext};

#[tokio::test]
async fn enqueue_and_drain_pending() {
    let gate = RepoTxGate::fresh();
    let tx_id = gate.fresh_tx_id();

    let tx = TxContext::new(tx_id, 1, 0, IsolationLevel::Snapshot);
    let write_set_keys: HashSet<(u64, bytes::Bytes), THasher> = HashSet::default();
    let (result_tx, _rx) = tokio::sync::oneshot::channel();

    let pending = PendingCommit::new(tx, write_set_keys, Vec::new(), result_tx);
    gate.enqueue_pending(pending);

    let batch = gate.drain_pending();
    assert_eq!(batch.len(), 1);

    // Second drain returns empty.
    let empty = gate.drain_pending();
    assert!(empty.is_empty());
}
