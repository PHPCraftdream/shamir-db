use std::sync::Arc;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::table::TableManager;
use crate::validator::ValidatorRegistry;

pub(super) struct FkTestResolver {
    pub(super) db: DbInstance,
    pub(super) repo: String,
    pub(super) registry: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for FkTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        let mut table = self.db.get_table(&self.repo, &table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}
