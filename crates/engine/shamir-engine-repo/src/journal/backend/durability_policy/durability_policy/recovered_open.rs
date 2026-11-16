//! Exclusive-open certification and coverage installation, never live replay.

use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_wal::WalGroupCommit;

use super::RecoverabilityCoverage;

/// Live first touch installs EMPTY coverage. Page-cache replay is not fsync
/// evidence. Only the existing successful cold recovery seam may establish H.
// Cross-crate seam: the tx commit core (shamir-engine) installs the empty
// coverage on reopen.
pub fn install_empty_coverage(group: &WalGroupCommit) -> DbResult<()> {
    group.install_coverage(Arc::new(RecoverabilityCoverage::new(0)))
}

/// A durable clean checkpoint includes abort-only allocations absent from WAL.
/// Read its store directly: initializing changefeed here would recurse into gate.
#[doc(hidden)]
pub async fn checkpoint_allocation_floor(store: Arc<dyn Store>) -> DbResult<u64> {
    crate::changelog_store::StoreChangelog::new(store)
        .load_clean_horizon()
        .await
}
