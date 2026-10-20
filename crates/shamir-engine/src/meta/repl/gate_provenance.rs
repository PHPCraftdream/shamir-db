//! Tx-gate construction with durable follower provenance (S5c-X PR6b-3),
//! extracted from `repo/repo_instance.rs` to keep that file within the
//! layout ratchet.

use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::Store;

/// Construct the [`shamir_tx::RepoTxGate`] from the given allocation floor
/// and tx-id counter, then consult the durable follower bookmark: a valid
/// v3 record enables receipt-native visibility before any read; a legacy or
/// malformed record refuses gate initialization (fail closed — operator
/// reseeds). Durable follower provenance is live from open.
pub(crate) async fn with_follower_provenance(
    last_committed: u64,
    next_tx_id: u64,
    info: &Arc<dyn Store>,
) -> DbResult<Arc<shamir_tx::RepoTxGate>> {
    let gate = shamir_tx::RepoTxGate::new(last_committed, next_tx_id);
    if let crate::meta::repl::bookmark_v3::FollowerBookmarkState::Native(_) =
        crate::meta::repl::bookmark_v3::classify_follower_bookmark(info).await?
    {
        gate.enable_receipt_native_visibility();
    }
    Ok(Arc::new(gate))
}
