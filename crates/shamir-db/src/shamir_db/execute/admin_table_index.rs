//! Admin handlers: CreateTable, DropTable, CreateIndex, DropIndex.

use crate::access::{Action, ResourcePath};
use crate::query::batch::{BatchError, BatchOp};
use crate::query::read::QueryResult;
use crate::shamir_db::shamir_db::schema_management::SCHEMA_FIELD;
use crate::types::value::QueryValue;
use shamir_types::mpack;

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
        let err_access =
            |e: shamir_types::access::AccessError| err_code("access_denied", e.to_string());

        // Check existence for if_not_exists / duplicate guard.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            if db.has_table(&op.repo, &op.create_table) {
                if op.if_not_exists {
                    return Ok(admin_result(mpack!({
                        "created_table": @(QueryValue::Str(op.create_table.clone())),
                        "repo": @(QueryValue::Str(op.repo.clone())),
                        "created": false,
                        "existed": true,
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
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::store(self.db_name.clone(), op.repo.clone()),
                Action::Create,
            )
            .await
            .map_err(err_access)?;
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

        Ok(admin_result(mpack!({
            "created_table": @(QueryValue::Str(op.create_table.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
            "created": true,
            "existed": false,
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

        // if_exists early-exit: missing db or missing table → no-op.
        if op.if_exists {
            let exists = self
                .shamir
                .get_db(&self.db_name)
                .is_some_and(|db| db.has_table(&op.repo, &op.drop_table));
            if !exists {
                return Ok(admin_result(mpack!({
                    "dropped_table": @(QueryValue::Str(op.drop_table.clone())),
                    "existed": false,
                })));
            }
        }

        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(self.db_name.clone(), op.repo.clone(), op.drop_table.clone()),
                Action::Delete,
            )
            .await
            .map_err(err_access)?;

        // Phase D.3 — reverse-FK drop guard.
        //
        // Refuse to drop a table that is still referenced by another table's
        // foreign key (any action — Restrict, Cascade, SetNull, NoAction).
        // Dropping a referenced parent would orphan the child FK and leave
        // dangling references.
        //
        // Discovery reads the PERSISTED catalogue schema from the system-store
        // (not the in-memory `TableManager` binding cache). The admin path's
        // `DbInstance` and the engine execute-path instance keep independent
        // in-memory buffers, so a validator binding just written through the
        // execute-path compile step is not reliably visible on the admin
        // handle. The catalogue is the coherent source of truth — every
        // `set_table_schema` commits the FK there before this guard runs.
        if let Some(db) = self.shamir.get_db(&self.db_name) {
            let table_names = db.list_tables(&op.repo).unwrap_or_default();
            for name in &table_names {
                if name == &op.drop_table {
                    continue;
                }
                let rec = match self
                    .shamir
                    .system_store()
                    .load_table_record(&self.db_name, &op.repo, name)
                    .await
                {
                    Ok(Some(r)) => r,
                    _ => continue,
                };
                let rules = match rec.get(SCHEMA_FIELD) {
                    Some(QueryValue::List(rules)) => rules,
                    _ => continue,
                };
                for rule in rules {
                    let refs_drop = rule
                        .get("foreign_key")
                        .and_then(|fk| fk.get("ref_table"))
                        .and_then(|v| v.as_str())
                        .is_some_and(|rt| rt == op.drop_table);
                    if refs_drop {
                        return Err(err_code(
                            "drop_refused_fk",
                            format!(
                                "cannot drop table '{}': still referenced by a foreign key in '{}'",
                                op.drop_table, name
                            ),
                        ));
                    }
                }
            }
        }

        // cascade: explicitly drop the table's own indexes (regular,
        // unique, sorted, index2) before removing validators and the
        // table itself.  Without cascade, indexes are orphaned in
        // storage (harmless — the catalogue entry is gone so they
        // will never be loaded again).
        if op.cascade {
            if let Some(db) = self.shamir.get_db(&self.db_name) {
                if let Ok(table) = db.get_table(&op.repo, &op.drop_table).await {
                    // Legacy regular indexes.
                    let regular_ids: Vec<u64> = table
                        .index_manager_ref()
                        .iter_indexes()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in regular_ids {
                        let _ = table.index_manager_ref().drop_index(id).await;
                    }
                    // Legacy unique indexes.
                    let unique_ids: Vec<u64> = table
                        .index_manager_ref()
                        .iter_unique_indexes()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in unique_ids {
                        let _ = table.index_manager_ref().drop_unique_index(id).await;
                    }
                    // Sorted indexes.
                    let sorted_ids: Vec<u64> = table
                        .sorted_indexes()
                        .iter_indexes()
                        .iter()
                        .map(|d| d.name_interned)
                        .collect();
                    for id in sorted_ids {
                        let _ = table.sorted_indexes().drop_index(id).await;
                    }
                    // index2 registry — remove all backends.
                    let backends = table.index2_registry().all_backends().await;
                    for b in &backends {
                        let _ = table
                            .index2_registry()
                            .remove_by_id(b.descriptor().id)
                            .await;
                    }
                }
            }
        }

        let removed = self
            .shamir
            .drop_table_cleaning_validators(&self.db_name, &op.repo, &op.drop_table)
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "dropped_table": @(QueryValue::Str(op.drop_table.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_rename_table(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameTable(op) = batch_op else {
            unreachable!("handle_rename_table called with non-RenameTable op");
        };

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

        // Auth: Write on the source table (rename mutates the table's
        // identity). Mirrors the function/validator rename auth path.
        self.shamir
            .authorize_access(
                &self.actor,
                &ResourcePath::table(
                    self.db_name.clone(),
                    op.repo.clone(),
                    op.rename_table.clone(),
                ),
                Action::Write,
            )
            .await
            .map_err(err_access)?;

        self.shamir
            .rename_table_as(
                &self.db_name,
                &op.repo,
                &op.rename_table,
                &op.to,
                self.actor.clone(),
            )
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "renamed_table": @(QueryValue::Str(op.rename_table.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
        })))
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
        // Inject the per-DB scalar resolver so create_index_v2 can validate
        // user-registered trusted_pure scalars for functional indexes.
        table
            .set_scalar_resolver(shamir_funclib::scalar_resolver::ScalarResolver::new(
                std::sync::Arc::clone(db.scalars()),
            ))
            .await;

        // Check if the index already exists (for if_not_exists / dup guard).
        let already_exists = if op.unique {
            table.unique_index_exists(&op.create_index).await
        } else {
            table.index_exists(&op.create_index).await
        };
        if already_exists {
            if op.if_not_exists {
                return Ok(admin_result(mpack!({
                    "created_index": @(QueryValue::Str(op.create_index.clone())),
                    "table": @(QueryValue::Str(op.table.clone())),
                    "created": false,
                    "existed": true,
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
            return Ok(admin_result(mpack!({
                "created_index": @(QueryValue::Str(op.create_index.clone())),
                "table": @(QueryValue::Str(op.table.clone())),
                "index_type": @(op.index_type.as_deref().map(|t| QueryValue::Str(t.to_string())).unwrap_or(QueryValue::Null)),
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

        Ok(admin_result(mpack!({
            "created_index": @(QueryValue::Str(op.create_index.clone())),
            "table": @(QueryValue::Str(op.table.clone())),
            "unique": @(QueryValue::Bool(op.unique)),
            "sorted": @(QueryValue::Bool(op.sorted)),
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

        // if_exists early-exit: missing db, table, or index → no-op.
        if op.if_exists {
            let db_opt = self.shamir.get_db(&self.db_name);
            let table_opt = match &db_opt {
                Some(db) => db.get_table(&op.repo, &op.table).await.ok(),
                None => None,
            };
            let index_exists = match &table_opt {
                Some(table) => {
                    if op.unique {
                        table.unique_index_exists(&op.drop_index).await
                    } else {
                        table.index_exists(&op.drop_index).await
                    }
                }
                None => false,
            };
            if !index_exists {
                return Ok(admin_result(mpack!({
                    "dropped_index": @(QueryValue::Str(op.drop_index.clone())),
                    "existed": false,
                })));
            }
        }

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

        Ok(admin_result(mpack!({
            "dropped_index": @(QueryValue::Str(op.drop_index.clone())),
            "existed": @(QueryValue::Bool(removed)),
        })))
    }

    pub(super) async fn handle_rename_index(
        &self,
        batch_op: &BatchOp,
    ) -> Result<QueryResult, BatchError> {
        let BatchOp::RenameIndex(op) = batch_op else {
            unreachable!("handle_rename_index called with non-RenameIndex op");
        };

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

        // Auth: Write on the parent table (rename mutates the index's
        // identity). Mirrors the index create/drop auth path.
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

        table
            .rename_index(&op.rename_index, &op.to)
            .await
            .map_err(|e| err_code("rename_index_failed", e.to_string()))?;

        Ok(admin_result(mpack!({
            "renamed_index": @(QueryValue::Str(op.rename_index.clone())),
            "to": @(QueryValue::Str(op.to.clone())),
            "table": @(QueryValue::Str(op.table.clone())),
            "repo": @(QueryValue::Str(op.repo.clone())),
        })))
    }
}
