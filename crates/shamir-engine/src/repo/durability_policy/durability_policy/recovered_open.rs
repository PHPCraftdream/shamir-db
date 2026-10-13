//! Exclusive-open certification and coverage installation, never live replay.

use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::Repo;
use shamir_wal::WalGroupCommit;

use super::RecoverabilityCoverage;
use crate::repo::RepoInstance;

/// Live first touch installs EMPTY coverage. Page-cache replay is not fsync
/// evidence. Only the existing successful cold recovery seam may establish H.
pub(crate) fn install_empty_coverage(group: &WalGroupCommit) -> DbResult<()> {
    group.install_coverage(Arc::new(RecoverabilityCoverage::new(0)))
}

/// A durable clean checkpoint includes abort-only allocations absent from WAL.
/// Read its store directly: initializing changefeed here would recurse into gate.
pub(crate) async fn checkpoint_allocation_floor(repo: &RepoInstance) -> DbResult<u64> {
    let store = repo.repo.store_get("__changelog__").await?;
    crate::repo::changelog_store::StoreChangelog::new(store)
        .load_clean_horizon()
        .await
}

/// Called by recover_inflight_v2 after replay/interner persistence/finalization,
/// exclusively before factory publication. H starts as an allocation floor;
/// flushing recovered history and certifying unknown (J,H] as gaps turns it
/// into a safe baseline, NOT a claim that every historical version committed.
/// Already-checkpointed J needs no second gap; still flush replayed history.
/// No reconciliation/projection of historical events is performed here (PR4).
pub(crate) async fn certify_recovered_open(repo: &RepoInstance) -> DbResult<()> {
    let gate = repo.tx_gate().await?;
    let h = gate.version_allocation_high_water_mark();
    repo.flush_all_history().await?;
    let handle = repo.changefeed().await?;
    let j = handle.feed.clean_horizon();
    if h > j {
        handle.feed.record_recovery_gap(j + 1, h).await?;
        handle.feed.record_clean_horizon(h).await?;
    }
    // Only AFTER every recovery/certification I/O succeeded. Missing WAL
    // predecessors may be aborted, in durable history, or unknown: the gap
    // prevents serving them as a complete journal, while R permits new writes.
    if let Some(coverage) = repo.wal_group().await?.coverage_observer() {
        coverage.certify_exclusive_open(h);
    }
    Ok(())
}
