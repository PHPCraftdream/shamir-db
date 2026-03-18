//! Batch execution entry point for ShamirDb.

use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::table::TableManager;
use crate::db::query::batch::{execute_batch, BatchError, BatchRequest, BatchResponse, TableResolver};
use crate::db::DbResult;

use super::shamir_db::ShamirDb;

/// TableResolver implementation that resolves table names within a specific
/// DbInstance + repo context.
struct RepoTableResolver {
    db: DbInstance,
    repo_name: String,
}

#[async_trait::async_trait]
impl TableResolver for RepoTableResolver {
    async fn resolve(&self, table_name: &str) -> DbResult<TableManager> {
        self.db.get_table(&self.repo_name, table_name).await
    }
}

impl ShamirDb {
    /// Execute a batch request against a specific database and repository.
    ///
    /// This is the primary entry point for all query execution.
    /// A single read query is just a batch with one operation.
    ///
    /// # Arguments
    /// * `db_name` — database name
    /// * `repo_name` — repository name within the database
    /// * `request` — batch request with one or more operations
    ///
    /// # Example
    /// ```ignore
    /// let request = BatchRequest { queries: ..., .. };
    /// let response = shamir.execute("mydb", "default", &request).await?;
    /// ```
    pub async fn execute(
        &self,
        db_name: &str,
        repo_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        let db = self.get_db(db_name).ok_or_else(|| {
            BatchError::QueryError {
                alias: String::new(),
                message: format!("Database '{}' not found", db_name),
            }
        })?;

        // Verify repo exists
        if !db.has_repo(repo_name) {
            return Err(BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
            });
        }

        let resolver = RepoTableResolver {
            db,
            repo_name: repo_name.to_string(),
        };

        execute_batch(request, &resolver).await
    }
}
