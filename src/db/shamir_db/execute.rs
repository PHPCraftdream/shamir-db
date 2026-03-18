//! Batch execution entry point for ShamirDb.

use serde_json::json;

use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::repo::repo_types::BoxRepoFactory;
use crate::db::engine::repo::RepoConfig;
use crate::db::engine::table::{TableConfig, TableManager};
use crate::db::query::batch::{
    execute_batch, AdminExecutor, BatchError, BatchOp, BatchRequest, BatchResponse, TableResolver,
};
use crate::db::query::read::{QueryResult, QueryStats};
use crate::db::query::TableRef;
use crate::db::DbResult;

use super::shamir_db::ShamirDb;

/// TableResolver that resolves TableRef within a DbInstance.
struct DbTableResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for DbTableResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&table_ref.repo, &table_ref.table).await
    }
}

/// AdminExecutor that operates on ShamirDb.
struct ShamirAdminExecutor {
    shamir: ShamirDb,
    db_name: String,
}

#[async_trait::async_trait]
impl AdminExecutor for ShamirAdminExecutor {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
        };

        match op {
            BatchOp::CreateDb(op) => {
                self.shamir.create_db(&op.create_db).await;
                Ok(admin_result(json!({"created": op.create_db})))
            }

            BatchOp::DropDb(op) => {
                let removed = self.shamir.remove_db(&op.drop_db).await;
                Ok(admin_result(json!({"dropped": op.drop_db, "existed": removed})))
            }

            BatchOp::CreateRepo(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;

                let factory = match op.engine.as_str() {
                    "in_memory" => BoxRepoFactory::in_memory(),
                    engine => return Err(err(format!(
                        "Unsupported engine '{}'. Supported: in_memory. Disk engines require path config.",
                        engine
                    ))),
                };

                let mut config = RepoConfig::new(&op.create_repo, factory);
                for table_name in &op.tables {
                    config = config.add_table(TableConfig::new(table_name));
                }

                db.add_repo(config).await.map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_repo": op.create_repo})))
            }

            BatchOp::DropRepo(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let removed = db.remove_repo(&op.drop_repo).await;
                Ok(admin_result(json!({"dropped_repo": op.drop_repo, "existed": removed})))
            }

            BatchOp::CreateTable(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                // Tables are lazily created when accessed. We just need to ensure
                // the repo exists and the table config is registered.
                // For now, verify repo exists:
                if !db.has_repo(&op.repo) {
                    return Err(err(format!("Repository '{}' not found", op.repo)));
                }
                // Table will be created on first access via RepoInstance
                Ok(admin_result(json!({"created_table": op.create_table, "repo": op.repo})))
            }

            BatchOp::DropTable(_op) => {
                // Table drop not yet implemented at storage level
                Err(err("drop_table not yet implemented".to_string()))
            }

            BatchOp::CreateIndex(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db.get_table(&op.repo, &op.table).await
                    .map_err(|e| err(e.to_string()))?;

                let field_strs: Vec<Vec<&str>> = op.fields.iter()
                    .map(|f| f.iter().map(|s| s.as_str()).collect())
                    .collect();
                // For single-segment paths, join as dot-separated for create_index API
                let paths: Vec<String> = field_strs.iter()
                    .map(|segments| segments.join("."))
                    .collect();
                let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

                if op.unique {
                    table.create_unique_index(&op.create_index, &path_refs).await
                        .map_err(|e| err(e.to_string()))?;
                } else {
                    table.create_index(&op.create_index, &path_refs).await
                        .map_err(|e| err(e.to_string()))?;
                }

                Ok(admin_result(json!({
                    "created_index": op.create_index,
                    "table": op.table,
                    "unique": op.unique
                })))
            }

            BatchOp::DropIndex(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db.get_table(&op.repo, &op.table).await
                    .map_err(|e| err(e.to_string()))?;

                let removed = if op.unique {
                    table.drop_unique_index(&op.drop_index).await
                        .map_err(|e| err(e.to_string()))?
                } else {
                    table.drop_index(&op.drop_index).await
                        .map_err(|e| err(e.to_string()))?
                };

                Ok(admin_result(json!({
                    "dropped_index": op.drop_index,
                    "existed": removed
                })))
            }

            BatchOp::List(list_op) => {
                use crate::db::query::admin::ListOp;
                match list_op {
                    ListOp::Databases => {
                        let dbs = self.shamir.list_dbs();
                        Ok(admin_result(json!({"databases": dbs})))
                    }
                    ListOp::Repos => {
                        let db = self.shamir.get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let repos = db.list_repos();
                        Ok(admin_result(json!({"repos": repos})))
                    }
                    ListOp::Tables { repo } => {
                        let db = self.shamir.get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let tables = db.list_tables(repo).map_err(|e| err(e.to_string()))?;
                        Ok(admin_result(json!({"tables": tables, "repo": repo})))
                    }
                }
            }

            _ => Err(err("Not an admin operation".to_string())),
        }
    }
}

fn admin_result(data: serde_json::Value) -> QueryResult {
    QueryResult {
        records: vec![data],
        stats: Some(QueryStats {
            index_used: None,
            records_scanned: 0,
            records_returned: 1,
            execution_time_us: 0,
        }),
        pagination: None,
    }
}

impl ShamirDb {
    /// Execute a batch request against a specific database.
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
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        };

        execute_batch(request, &resolver, Some(&admin)).await
    }
}
