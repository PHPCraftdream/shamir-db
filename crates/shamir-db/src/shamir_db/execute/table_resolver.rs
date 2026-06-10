//! `TableResolver` implementation for `ShamirDb`.

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::table::TableManager;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::DbResult;

/// TableResolver that resolves TableRef within a DbInstance.
///
/// Injects the global `ValidatorRegistry` (S3) into every resolved
/// `TableManager` so the write path can run validators.
pub(super) struct DbTableResolver {
    pub(super) db: DbInstance,
    pub(super) validators: std::sync::Arc<crate::engine::validator::ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for DbTableResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        let mut table = self.db.get_table(&table_ref.repo, &table_ref.table).await?;
        table.set_validator_registry(self.validators.clone());
        Ok(table)
    }

    async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::engine::repo::RepoInstance> {
        self.db.get_repo(repo_name).ok_or_else(|| {
            crate::DbError::NotFound(format!("Repository '{}' not found", repo_name))
        })
    }
}
