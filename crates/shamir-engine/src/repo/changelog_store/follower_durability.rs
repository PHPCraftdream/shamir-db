use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::{Repo, Store};

use crate::repo::RepoInstance;

impl RepoInstance {
    pub(crate) async fn follower_journal_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }
}
