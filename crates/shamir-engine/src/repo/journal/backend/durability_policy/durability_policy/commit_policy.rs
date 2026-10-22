//! Commit policy and typed WAL outcome classification.
use super::*;

#[cfg(test)]
pub(crate) static TEST_SYNCED_WAL_REPO: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(crate) fn commit_wal_durability(repo: &RepoInstance) -> shamir_wal::WalDurability {
    #[cfg(test)]
    if TEST_SYNCED_WAL_REPO
        .get()
        .is_some_and(|name| name == repo.name())
    {
        return shamir_wal::WalDurability::Synced;
    }
    let policy = repo.durability().load();
    match policy.effective_wal_commit() {
        crate::repo::WalCommitTier::Synced => shamir_wal::WalDurability::Synced,
        crate::repo::WalCommitTier::Buffered => shamir_wal::WalDurability::Buffered,
    }
}

/// `Ok(bool)` is the `durability_failed` flag (`WrittenNotDurable` = written
/// at level 2 but not fsynced). The second tuple element is TRUE only when
/// the ticket outcome PROVES the frame was never written (`NotWritten`) —
/// the caller settles the version for R-contiguity at that point (B3).
pub(crate) fn wal_completion(
    gate: &shamir_tx::RepoTxGate,
    result: Result<(), WalAppendError>,
) -> (Result<bool, TxError>, bool) {
    match result {
        Ok(()) => (Ok(false), false),
        Err(WalAppendError::WrittenNotDurable) => (Ok(true), false),
        Err(WalAppendError::NotWritten) => (
            Err(TxError::Storage(WalAppendError::NotWritten.into())),
            true,
        ),
        Err(WalAppendError::Unknown | WalAppendError::Indeterminate) => {
            gate.set_requires_reopen();
            (Err(TxError::WalRequiresReopen), false)
        }
        Err(_) => {
            gate.set_requires_reopen();
            (Err(TxError::WalRequiresReopen), false)
        }
    }
}
