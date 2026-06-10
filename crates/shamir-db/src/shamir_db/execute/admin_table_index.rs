//! Admin handlers: CreateTable, DropTable, CreateIndex, DropIndex.

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::{admin_result, apply_table_retention};

impl ShamirAdminExecutor {
    pub(super) async fn handle_create_table(
        &self,
        op: &crate::query::admin::CreateTableOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };

        // Check existence for if_not_exists / duplicate guard.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            if db.has_table(&op.repo, &op.create_table) {
                if op.if_not_exists {
                    return Ok(admin_result(json!({
                        "created_table": op.create_table,
                        "repo": op.repo,
                        "created": false,
                        "existed": true
                    })));
                }
                return Err(err_code(
                    "exists",
                    format!(
                        "Table '{}' already exists in repository '{}'",
                        op.create_table, op.repo
                    ),
                ));
            }
        }
        // Route through ShamirDb so the table is persisted to the
        // catalogue and survives a restart (I.2).
        self.shamir
            .add_table_as(
                &self.db_name,
                &op.repo,
                &op.create_table,
                false,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;

        // T3: apply per-table history retention at creation time.
        if let Some(ref dto) = op.retention {
            dto.validate().map_err(err)?;
            apply_table_retention(
                &self.shamir,
                &self.db_name,
                &op.repo,
                &op.create_table,
                crate::engine::repo::to_mvcc_retention(dto),
            )
            .await?;
        }

        Ok(admin_result(json!({
            "created_table": op.create_table,
            "repo": op.repo,
            "created": true,
            "existed": false
        })))
    }

    pub(super) async fn handle_drop_table(
        &self,
        op: &crate::query::admin::DropTableOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.drop_table.clone()),
                Action::Delete,
            )
            .await
            .map_err(err_access)?;
        let removed = self
            .shamir
            .drop_table_cleaning_validators(&self.db_name, &op.repo, &op.drop_table)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(
            json!({"dropped_table": op.drop_table, "existed": removed}),
        ))
    }

    pub(super) async fn handle_create_index(
        &self,
        op: &crate::query::admin::CreateIndexOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.table)
            .await
            .map_err(|e| err(e.to_string()))?;

        // Check if the index already exists (for if_not_exists / dup guard).
        let already_exists = if op.unique {
            table.unique_index_exists(&op.create_index).await
        } else {
            table.index_exists(&op.create_index).await
        };
        if already_exists {
            if op.if_not_exists {
                return Ok(admin_result(json!({
                    "created_index": op.create_index,
                    "table": op.table,
                    "created": false,
                    "existed": true
                })));
            }
            return Err(err_code(
                "exists",
                format!(
                    "Index '{}' already exists on table '{}'",
                    op.create_index, op.table
                ),
            ));
        }

        let field_strs: Vec<Vec<&str>> = op
            .fields
            .iter()
            .map(|f| f.iter().map(|s| s.as_str()).collect())
            .collect();
        // For single-segment paths, join as dot-separated for create_index API
        let paths: Vec<String> = field_strs
            .iter()
            .map(|segments| segments.join("."))
            .collect();
        let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();

        if op.index_type.as_deref().is_some_and(|t| t != "btree") {
            table
                .create_index_v2(op)
                .await
                .map_err(|e| err(e.to_string()))?;
            return Ok(admin_result(json!({
                "created_index": op.create_index,
                "table": op.table,
                "index_type": op.index_type,
            })));
        }

        if op.sorted && op.unique {
            return Err(err("Index cannot be both sorted and unique".to_string()));
        }
        if !op.include.is_empty() && !op.sorted {
            return Err(err("include is only valid for sorted indexes".to_string()));
        }
        if op.sorted {
            if op.fields.len() != 1 {
                return Err(err(
                    "Sorted index requires exactly one field (composite TBD)".to_string(),
                ));
            }
            table
                .create_sorted_index_with_include(&op.create_index, &path_refs, op.include.clone())
                .await
                .map_err(|e| err(e.to_string()))?;
        } else if op.unique {
            table
                .create_unique_index(&op.create_index, &path_refs)
                .await
                .map_err(|e| err(e.to_string()))?;
        } else {
            table
                .create_index(&op.create_index, &path_refs)
                .await
                .map_err(|e| err(e.to_string()))?;
        }

        Ok(admin_result(json!({
            "created_index": op.create_index,
            "table": op.table,
            "unique": op.unique,
            "sorted": op.sorted
        })))
    }

    pub(super) async fn handle_drop_index(
        &self,
        op: &crate::query::admin::DropIndexOp,
    ) -> Result<QueryResult, BatchError> {
        let err = |msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: None,
        };
        let err_code = |code: &str, msg: String| BatchError::QueryError {
            alias: String::new(),
            message: msg,
            code: Some(code.to_string()),
        };
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.table.clone()),
                Action::Write,
            )
            .await
            .map_err(err_access)?;
        let db = self
            .shamir
            .get_db(&self.db_name)
            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
        let table = db
            .get_table(&op.repo, &op.table)
            .await
            .map_err(|e| err(e.to_string()))?;

        let removed = if op.unique {
            table
                .drop_unique_index(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        } else {
            table
                .drop_index(&op.drop_index)
                .await
                .map_err(|e| err(e.to_string()))?
        };

        Ok(admin_result(json!({
            "dropped_index": op.drop_index,
            "existed": removed
        })))
    }
}
