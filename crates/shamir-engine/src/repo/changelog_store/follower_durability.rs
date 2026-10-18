use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::{AtomicBatch, Repo as _, Store};

use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::repo::RepoInstance;

impl RepoInstance {
    pub(crate) async fn follower_journal_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    /// Backend capability for the S5c-X PR6b-2 receipt apply protocol.
    pub(crate) fn storage_supports_atomic_batch(&self) -> bool {
        self.repo.supports_atomic_batch()
    }

    /// Raw named-store access, test-only: every caller lives under `tests/`,
    /// so this would be dead code in the non-test build.
    #[cfg(test)]
    pub(crate) async fn raw_store(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get(name).await
    }

    /// Begin a repository-wide atomic batch over named stores (PR6b-2).
    /// The permit is the admitted-API capability: callers must already hold
    /// a `RepoWritePermit` from this repo's write domain; there is NO second
    /// admission mechanism here.
    pub(crate) async fn begin_repo_atomic_batch(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<Box<dyn AtomicBatch>> {
        self.write_domain.debug_check_permit(permit);
        self.repo.begin_atomic_batch().await
    }
}
