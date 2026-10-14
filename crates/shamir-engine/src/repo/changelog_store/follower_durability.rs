use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::{AtomicBatch, Repo as _, Store};

use crate::repo::RepoInstance;

impl RepoInstance {
    pub(crate) async fn follower_journal_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    /// Backend capability for the S5c-X PR6b-2 receipt apply protocol.
    pub(crate) fn storage_supports_atomic_batch(&self) -> bool {
        self.repo.supports_atomic_batch()
    }

    /// Begin a repository-wide atomic batch over named stores (PR6b-2).
    pub(crate) async fn begin_repo_atomic_batch(&self) -> DbResult<Box<dyn AtomicBatch>> {
        self.repo.begin_atomic_batch().await
    }
}
