//! Batch execution entry point for ShamirDb.

use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::table::TableManager;
use crate::db::query::batch::{execute_batch, BatchError, BatchRequest, BatchResponse, TableResolver};
use crate::db::query::TableRef;
use crate::db::DbResult;

use super::shamir_db::ShamirDb;

/// TableResolver that resolves TableRef (repo + table) within a DbInstance.
struct DbTableResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for DbTableResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&table_ref.repo, &table_ref.table).await
    }
}

impl ShamirDb {
    /// Execute a batch request against a specific database.
    ///
    /// Each operation specifies its target table (and optionally repo)
    /// via `from`, `insert_into`, `update`, `delete_from` fields.
    /// Default repo is "main".
    ///
    /// # Arguments
    /// * `db_name` — database name (connection context)
    /// * `request` — batch request with one or more operations
    pub async fn execute(
        &self,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
        })?;

        let resolver = DbTableResolver { db };

        execute_batch(request, &resolver).await
    }
}
