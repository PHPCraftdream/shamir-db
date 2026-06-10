use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::decode_version_key;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use std::sync::Arc;

pub(super) fn make_gate() -> Arc<RepoTxGate> {
    Arc::new(RepoTxGate::fresh())
}

pub(super) fn make_mvcc() -> MvccStore {
    let gate = make_gate();
    MvccStore::new(Arc::new(InMemoryStore::new()), gate)
}

pub(super) fn make_mvcc_with_gate(gate: Arc<RepoTxGate>) -> MvccStore {
    MvccStore::new(Arc::new(InMemoryStore::new()), gate)
}

pub(super) async fn count_history_entries(mvcc: &MvccStore) -> usize {
    // T1c: count only version-keys (decode_version_key succeeds). ts-keys
    // ([TS_TAG][version_be]) are skipped — decode returns None for them.
    let stream = mvcc.history_store().iter_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0;
    while let Some(batch) = stream.next().await {
        for (phys_key, _val) in batch.unwrap() {
            if decode_version_key(&phys_key).is_some() {
                count += 1;
            }
        }
    }
    count
}

/// Helper: count this key's prior (old) log versions — the entries
/// `history_of` returns minus the current (latest) entry. The current
/// entry is always in the log, so `history_of` includes it.
/// Prior versions = total − (1 if a current version is cached else 0).
pub(super) async fn archived_count(mvcc: &MvccStore, key: &[u8]) -> usize {
    let timeline = mvcc.history_of(key).await.unwrap();
    let cur_v = mvcc.current_version(key);
    if cur_v > 0 {
        timeline.len().saturating_sub(1)
    } else {
        timeline.len()
    }
}
