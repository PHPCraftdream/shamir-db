//! Batch execution entry point for ShamirDb.

use base64::Engine;
use serde_json::json;

use std::sync::Arc;

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::{TableConfig, TableManager};
use crate::query::batch::{
    commit_interactive_tx, execute_batch, execute_in_open_tx, open_interactive_tx, AdminExecutor,
    BatchError, BatchOp, BatchRequest, BatchResponse, FunctionInvoker, TableResolver,
    TransactionInfo,
};
use crate::query::read::{QueryResult, QueryStats};
use crate::query::TableRef;
use crate::DbResult;

use crate::engine::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState};

use super::shamir_db::ShamirDb;
use crate::access::{Action, Actor, ResourcePath};

/// TableResolver that resolves TableRef within a DbInstance.
///
/// Injects the global `ValidatorRegistry` (S3) into every resolved
/// `TableManager` so the write path can run validators.
struct DbTableResolver {
    db: DbInstance,
    validators: std::sync::Arc<crate::engine::validator::ValidatorRegistry>,
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

/// Rejects path-traversal characters in database and repository names.
///
/// Only `[A-Za-z0-9_-]` is allowed — no `/`, `\`, `:`, `.`, or any
/// non-ASCII byte. Empty strings are also rejected.
fn validate_name_component(s: &str, label: &str) -> Result<(), BatchError> {
    if s.is_empty() {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: format!("{} must not be empty", label),
            code: None,
        });
    }
    if s == "." || s == ".." {
        return Err(BatchError::QueryError {
            alias: String::new(),
            message: format!("{} must not be '.' or '..'", label),
            code: None,
        });
    }
    for ch in s.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' {
            return Err(BatchError::QueryError {
                alias: String::new(),
                message: format!(
                    "{} contains disallowed character '{}': \
                     only [A-Za-z0-9_-] are permitted",
                    label, ch
                ),
                code: None,
            });
        }
    }
    Ok(())
}

/// AdminExecutor that operates on ShamirDb.
struct ShamirAdminExecutor {
    shamir: ShamirDb,
    db_name: String,
    actor: Actor,
}

#[async_trait::async_trait]
impl AdminExecutor for ShamirAdminExecutor {
    async fn execute_admin(&self, op: &BatchOp) -> Result<QueryResult, BatchError> {
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

        match op {
            BatchOp::CreateDb(op) => {
                validate_name_component(&op.create_db, "db_name")?;
                if self.shamir.has_db(&op.create_db) {
                    if op.if_not_exists {
                        return Ok(admin_result(json!({
                            "created": false,
                            "existed": true,
                            "db": op.create_db
                        })));
                    }
                    return Err(err_code(
                        "exists",
                        format!("Database '{}' already exists", op.create_db),
                    ));
                }
                self.shamir
                    .create_db_as(&op.create_db, self.actor.clone())
                    .await;
                Ok(admin_result(json!({
                    "created": true,
                    "existed": false,
                    "db": op.create_db
                })))
            }

            BatchOp::DropDb(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::database(op.drop_db.clone()),
                        Action::Delete,
                    )
                    .await
                    .map_err(err_access)?;
                // Referential integrity: check for child repositories.
                if let Some(db) = self.shamir.get_db(&op.drop_db) {
                    let repos = db.list_repos();
                    if !repos.is_empty() {
                        if !op.cascade {
                            return Err(err_code(
                                "still_referenced",
                                format!(
                                    "cannot drop database '{}': still has repositories: {:?}",
                                    op.drop_db, repos
                                ),
                            ));
                        }
                        // Cascade: remove every repo (and its tables) first.
                        for repo_name in &repos {
                            // Remove tables within each repo.
                            if let Ok(tables) = db.list_tables(repo_name) {
                                for table_name in &tables {
                                    let _ = self
                                        .shamir
                                        .drop_table_cleaning_validators(
                                            &self.db_name,
                                            repo_name,
                                            table_name,
                                        )
                                        .await;
                                }
                            }
                            self.shamir.remove_repo(&op.drop_db, repo_name).await;
                        }
                    }
                }
                let removed = self.shamir.remove_db(&op.drop_db).await;
                Ok(admin_result(
                    json!({"dropped": op.drop_db, "existed": removed}),
                ))
            }

            BatchOp::CreateRepo(op) => {
                validate_name_component(&self.db_name, "db_name")?;
                validate_name_component(&op.create_repo, "repo_name")?;

                // Check existence for if_not_exists / duplicate guard.
                if let Some(db) = self.shamir.get_db(&self.db_name) {
                    if db.has_repo(&op.create_repo) {
                        if op.if_not_exists {
                            return Ok(admin_result(json!({
                                "created": false,
                                "existed": true,
                                "repo": op.create_repo
                            })));
                        }
                        return Err(err_code(
                            "exists",
                            format!(
                                "Repository '{}' already exists in database '{}'",
                                op.create_repo, self.db_name
                            ),
                        ));
                    }
                }

                let factory = match op.engine.as_deref() {
                    Some("in_memory") => BoxRepoFactory::in_memory(),
                    Some("redb") | None => {
                        // Durable default: if the home has a data_root,
                        // use a redb file under data_root/<db>/<repo>.redb.
                        // In-memory home (tests) falls back to in_memory.
                        match self.shamir.data_root() {
                            Some(root) => {
                                let db_dir = root.join(&self.db_name);
                                tokio::fs::create_dir_all(&db_dir).await.map_err(|e| {
                                    err(format!(
                                        "failed to create repo directory '{}': {}",
                                        db_dir.display(),
                                        e
                                    ))
                                })?;
                                let path = db_dir.join(format!("{}.redb", op.create_repo));
                                BoxRepoFactory::redb_raw(path)
                            }
                            None => BoxRepoFactory::in_memory(),
                        }
                    }
                    Some(other) => {
                        return Err(err(format!(
                            "Unsupported engine '{}'. Supported: in_memory, redb.",
                            other
                        )));
                    }
                };

                let mut config = RepoConfig::new(&op.create_repo, factory);
                for table_name in &op.tables {
                    config = config.add_table(TableConfig::new(table_name));
                }

                self.shamir
                    .add_repo_as(&self.db_name, config, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "created_repo": op.create_repo,
                    "created": true,
                    "existed": false
                })))
            }

            BatchOp::DropRepo(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::store(self.db_name.clone(), op.drop_repo.clone()),
                        Action::Delete,
                    )
                    .await
                    .map_err(err_access)?;
                // Referential integrity: check for child tables.
                if let Some(db) = self.shamir.get_db(&self.db_name) {
                    if let Ok(tables) = db.list_tables(&op.drop_repo) {
                        if !tables.is_empty() {
                            if !op.cascade {
                                return Err(err_code(
                                    "still_referenced",
                                    format!(
                                        "cannot drop repository '{}': still has tables: {:?}",
                                        op.drop_repo, tables
                                    ),
                                ));
                            }
                            // Cascade: remove every table first.
                            for table_name in &tables {
                                let _ = self
                                    .shamir
                                    .drop_table_cleaning_validators(
                                        &self.db_name,
                                        &op.drop_repo,
                                        table_name,
                                    )
                                    .await;
                            }
                        }
                    }
                }
                // Route through ShamirDb so the repo's catalogue record is
                // removed from the system store and the repo does not
                // resurrect on the next open (symmetry with CreateRepo).
                let removed = self.shamir.remove_repo(&self.db_name, &op.drop_repo).await;
                Ok(admin_result(
                    json!({"dropped_repo": op.drop_repo, "existed": removed}),
                ))
            }

            BatchOp::CreateTable(op) => {
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
                Ok(admin_result(json!({
                    "created_table": op.create_table,
                    "repo": op.repo,
                    "created": true,
                    "existed": false
                })))
            }

            BatchOp::DropTable(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.drop_table.clone(),
                        ),
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

            BatchOp::CreateIndex(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.table.clone(),
                        ),
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
                        .create_sorted_index_with_include(
                            &op.create_index,
                            &path_refs,
                            op.include.clone(),
                        )
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

            BatchOp::DropIndex(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.table.clone(),
                        ),
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

            BatchOp::GetBufferConfig(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.get_buffer_config.clone(),
                        ),
                        Action::Read,
                    )
                    .await
                    .map_err(err_access)?;
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db
                    .get_table(&op.repo, &op.get_buffer_config)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let cfg = table
                    .get_buffer_config()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let payload = match cfg {
                    Some(c) => json!({
                        "table": op.get_buffer_config,
                        "repo": op.repo,
                        "config": dto_from_storage(&c),
                    }),
                    None => json!({
                        "table": op.get_buffer_config,
                        "repo": op.repo,
                        "config": serde_json::Value::Null,
                    }),
                };
                Ok(admin_result(payload))
            }

            BatchOp::SetBufferConfig(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.set_buffer_config.clone(),
                        ),
                        Action::Manage,
                    )
                    .await
                    .map_err(err_access)?;
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db
                    .get_table(&op.repo, &op.set_buffer_config)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let storage_cfg = storage_from_dto(&op.config);
                table
                    .set_buffer_config(&storage_cfg)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "set_buffer_config": op.set_buffer_config,
                    "repo": op.repo,
                    "config": dto_from_storage(&storage_cfg),
                })))
            }

            BatchOp::AlterBufferConfig(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.alter_buffer_config.clone(),
                        ),
                        Action::Manage,
                    )
                    .await
                    .map_err(err_access)?;
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db
                    .get_table(&op.repo, &op.alter_buffer_config)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let patch = op.patch.clone();
                let updated = table
                    .alter_buffer_config(|c| apply_patch(c, &patch))
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "alter_buffer_config": op.alter_buffer_config,
                    "repo": op.repo,
                    "config": dto_from_storage(&updated),
                })))
            }

            BatchOp::List(list_op) => {
                use crate::query::admin::ListOp;
                match list_op {
                    ListOp::Databases => {
                        self.shamir
                            .authorize_access(&self.actor, &ResourcePath::Root, Action::List)
                            .await
                            .map_err(err_access)?;
                        let dbs = self.shamir.list_dbs();
                        Ok(admin_result(json!({"databases": dbs})))
                    }
                    ListOp::Repos => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::database(self.db_name.clone()),
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let db = self
                            .shamir
                            .get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let repos = db.list_repos();
                        Ok(admin_result(json!({"repos": repos})))
                    }
                    ListOp::Tables { repo } => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::store(self.db_name.clone(), repo.clone()),
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let db = self
                            .shamir
                            .get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let tables = db.list_tables(repo).map_err(|e| err(e.to_string()))?;
                        Ok(admin_result(json!({"tables": tables, "repo": repo})))
                    }
                    ListOp::Users => {
                        self.shamir
                            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                            .await
                            .map_err(err_access)?;
                        let table = self
                            .shamir
                            .system_store()
                            .users_table()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = table
                            .interner()
                            .get()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let refs = crate::types::common::new_map();
                        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                        let query = crate::query::read::ReadQuery::new("users");
                        let result = table
                            .read(&query, &ctx)
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        // Strip password_hash from output
                        let users: Vec<serde_json::Value> = result
                            .records
                            .into_iter()
                            .map(|mut r| {
                                if let Some(obj) = r.as_object_mut() {
                                    obj.remove("password_hash");
                                }
                                r
                            })
                            .collect();
                        Ok(admin_result(json!({"users": users})))
                    }
                    ListOp::Roles => {
                        self.shamir
                            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                            .await
                            .map_err(err_access)?;
                        let table = self
                            .shamir
                            .system_store()
                            .roles_table()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = table
                            .interner()
                            .get()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let refs = crate::types::common::new_map();
                        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                        let query = crate::query::read::ReadQuery::new("roles");
                        let result = table
                            .read(&query, &ctx)
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        Ok(admin_result(json!({"roles": result.records})))
                    }
                    ListOp::Indexes { table, repo } => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::table(
                                    self.db_name.clone(),
                                    repo.clone(),
                                    table.clone(),
                                ),
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let db = self
                            .shamir
                            .get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let tm = db
                            .get_table(repo, table)
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = tm.interner().get().await.map_err(|e| err(e.to_string()))?;

                        let mut indexes = Vec::new();
                        for def in tm.index_manager_ref().iter_indexes() {
                            let name = interner
                                .get_str(&crate::core::interner::InternerKey::new(
                                    def.name_interned,
                                ))
                                .map(|k| k.as_str().to_string())
                                .unwrap_or_else(|| def.name_interned.to_string());
                            indexes.push(json!({"name": name, "unique": false}));
                        }
                        for def in tm.index_manager_ref().iter_unique_indexes() {
                            let name = interner
                                .get_str(&crate::core::interner::InternerKey::new(
                                    def.name_interned,
                                ))
                                .map(|k| k.as_str().to_string())
                                .unwrap_or_else(|| def.name_interned.to_string());
                            indexes.push(json!({"name": name, "unique": true}));
                        }

                        Ok(admin_result(
                            json!({"indexes": indexes, "table": table, "repo": repo}),
                        ))
                    }
                    ListOp::Functions { folder } => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::FunctionNamespace,
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let mut names = self
                            .shamir
                            .list_functions()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        if let Some(prefix) = folder {
                            let prefix_slash = if prefix.ends_with('/') {
                                prefix.clone()
                            } else {
                                format!("{}/", prefix)
                            };
                            names.retain(|n| n.starts_with(&prefix_slash));
                        }
                        Ok(admin_result(json!({"functions": names})))
                    }
                    ListOp::Validators => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::FunctionNamespace,
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let validators = self.shamir.list_validators();
                        let items: Vec<serde_json::Value> = validators
                            .iter()
                            .map(|(id, name)| {
                                let bound = self.shamir.validators().bound_tables(id);
                                json!({
                                    "id": id.to_string(),
                                    "name": name,
                                    "bound_in": bound,
                                })
                            })
                            .collect();
                        Ok(admin_result(json!({"validators": items})))
                    }
                    ListOp::FunctionFolders { parent } => {
                        self.shamir
                            .authorize_access(
                                &self.actor,
                                &ResourcePath::FunctionNamespace,
                                Action::List,
                            )
                            .await
                            .map_err(err_access)?;
                        let mut folders = self
                            .shamir
                            .list_function_folders()
                            .await
                            .map_err(|e| err(e.to_string()))?;
                        if let Some(prefix) = parent {
                            let prefix_slash = if prefix.ends_with('/') {
                                prefix.clone()
                            } else {
                                format!("{}/", prefix)
                            };
                            folders.retain(|f| f.starts_with(&prefix_slash));
                        }
                        Ok(admin_result(json!({"function_folders": folders})))
                    }
                }
            }

            BatchOp::CreateUser(op) => {
                // Authorization (owner-delegation): a global admin (Manage on
                // root) may create any user; a database owner may create users
                // scoped to their own database. System bypasses.
                self.authorize_user_lifecycle(op.database.as_deref())
                    .await
                    .map_err(err_access)?;
                // Hash the password at rest with Argon2id (PHC string).
                // This `users.password_hash` field is RBAC/admin metadata,
                // NOT a live-auth credential — the wire login path is
                // SCRAM-Argon2id in `shamir-connect` over StoredKey /
                // ServerKey, which never reads this field. Hashing here is
                // defense-in-depth for the at-rest secret; no verify-side
                // change is required.
                let password_hash = crate::query::auth::SecretString::new(
                    hash_password(op.password.reveal()).map_err(|e| err(e.to_string()))?,
                );
                let user = crate::query::auth::User {
                    name: op.create_user.clone(),
                    password_hash,
                    roles: op.roles.clone(),
                    profile: op.profile.clone(),
                    database: op.database.clone(),
                };
                let user_json = serde_json::to_value(&user).map_err(|e| err(e.to_string()))?;
                let table = self
                    .shamir
                    .system_store()
                    .users_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let set_op = crate::query::write::SetOp {
                    set: crate::query::TableRef::new("users"),
                    key: json!({"name": op.create_user}),
                    value: user_json,
                };
                table
                    .execute_set(&set_op)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                table
                    .interner()
                    .persist()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_user": op.create_user})))
            }

            BatchOp::DropUser(op) => {
                let table = self
                    .shamir
                    .system_store()
                    .users_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table
                    .interner()
                    .get()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);

                // Authorization (owner-delegation): resolve the target user's
                // stored database scope so a database owner can only drop users
                // bound to their own database. A non-existent user resolves to
                // `None` scope → only a global admin (or System) may proceed.
                let scope = {
                    let lookup = crate::query::read::ReadQuery::new("users").filter(
                        crate::query::filter::Filter::Eq {
                            field: vec!["name".to_string()],
                            value: crate::query::filter::FilterValue::String(op.drop_user.clone()),
                        },
                    );
                    let existing = table
                        .read(&lookup, &ctx)
                        .await
                        .map_err(|e| err(e.to_string()))?;
                    existing.records.first().and_then(|rec| {
                        rec.get("database")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    })
                };
                self.authorize_user_lifecycle(scope.as_deref())
                    .await
                    .map_err(err_access)?;

                let del_op = crate::query::write::DeleteOp {
                    delete_from: crate::query::TableRef::new("users"),
                    where_clause: crate::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::query::filter::FilterValue::String(op.drop_user.clone()),
                    },
                };
                let result = table
                    .execute_delete(&del_op, &ctx)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"dropped_user": op.drop_user, "existed": result.affected > 0}),
                ))
            }

            BatchOp::CreateRole(op) => {
                // Role management is global-admin only (Manage on the root).
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let role = crate::query::auth::Role {
                    name: op.create_role.clone(),
                    permissions: op.permissions.clone(),
                };
                let role_json = serde_json::to_value(&role).map_err(|e| err(e.to_string()))?;
                let table = self
                    .shamir
                    .system_store()
                    .roles_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let set_op = crate::query::write::SetOp {
                    set: crate::query::TableRef::new("roles"),
                    key: json!({"name": op.create_role}),
                    value: role_json,
                };
                table
                    .execute_set(&set_op)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                table
                    .interner()
                    .persist()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_role": op.create_role})))
            }

            BatchOp::DropRole(op) => {
                // Role management is global-admin only (Manage on the root).
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let table = self
                    .shamir
                    .system_store()
                    .roles_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table
                    .interner()
                    .get()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                let del_op = crate::query::write::DeleteOp {
                    delete_from: crate::query::TableRef::new("roles"),
                    where_clause: crate::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::query::filter::FilterValue::String(op.drop_role.clone()),
                    },
                };
                let result = table
                    .execute_delete(&del_op, &ctx)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"dropped_role": op.drop_role, "existed": result.affected > 0}),
                ))
            }

            BatchOp::GrantRole(op) => {
                // Role grants are global-admin only (Manage on the root).
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let user_lock = self
                    .shamir
                    .admin_user_locks()
                    .entry(op.user.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone();
                let _user_guard = user_lock.lock().await;

                // Read user, add role, write back
                let table = self
                    .shamir
                    .system_store()
                    .users_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table
                    .interner()
                    .get()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                let query = crate::query::read::ReadQuery::new("users").filter(
                    crate::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::query::filter::FilterValue::String(op.user.clone()),
                    },
                );
                let result = table
                    .read(&query, &ctx)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                if result.records.is_empty() {
                    return Err(err_code(
                        "not_found",
                        format!("User '{}' not found", op.user),
                    ));
                }
                let mut user_json = result.records[0].clone();
                if let Some(roles) = user_json.get_mut("roles").and_then(|r| r.as_array_mut()) {
                    if !roles.contains(&json!(op.grant_role)) {
                        roles.push(json!(op.grant_role));
                    }
                }
                let set_op = crate::query::write::SetOp {
                    set: crate::query::TableRef::new("users"),
                    key: json!({"name": op.user}),
                    value: user_json,
                };
                table
                    .execute_set(&set_op)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                table
                    .interner()
                    .persist()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"granted_role": op.grant_role, "user": op.user}),
                ))
            }

            BatchOp::RevokeRole(op) => {
                // Role revokes are global-admin only (Manage on the root).
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let user_lock = self
                    .shamir
                    .admin_user_locks()
                    .entry(op.user.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone();
                let _user_guard = user_lock.lock().await;

                let table = self
                    .shamir
                    .system_store()
                    .users_table()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table
                    .interner()
                    .get()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                let query = crate::query::read::ReadQuery::new("users").filter(
                    crate::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::query::filter::FilterValue::String(op.user.clone()),
                    },
                );
                let result = table
                    .read(&query, &ctx)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                if result.records.is_empty() {
                    return Err(err_code(
                        "not_found",
                        format!("User '{}' not found", op.user),
                    ));
                }
                let mut user_json = result.records[0].clone();
                if let Some(roles) = user_json.get_mut("roles").and_then(|r| r.as_array_mut()) {
                    roles.retain(|r| r != &json!(op.revoke_role));
                }
                let set_op = crate::query::write::SetOp {
                    set: crate::query::TableRef::new("users"),
                    key: json!({"name": op.user}),
                    value: user_json,
                };
                table
                    .execute_set(&set_op)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                table
                    .interner()
                    .persist()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"revoked_role": op.revoke_role, "user": op.user}),
                ))
            }

            BatchOp::StartMigration(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::table(
                            self.db_name.clone(),
                            op.repo.clone(),
                            op.start_migration.clone(),
                        ),
                        Action::Manage,
                    )
                    .await
                    .map_err(err_access)?;
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;

                let table_name = &op.start_migration;
                // Atomic counter + ns timestamp + random suffix — collision-free
                // even under concurrent start_migration on same table within
                // the same nanosecond.
                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let rand_suffix: u32 = rand::random();
                let migration_id = format!("mig_{}_{}_{:08x}", table_name, now_ns, rand_suffix);

                // Reject if any active migration already targets this table.
                let table_already_migrating = self
                    .shamir
                    .active_migrations()
                    .iter()
                    .any(|e| e.value().targets_table(&op.repo, table_name));
                if table_already_migrating {
                    return Err(err(format!(
                        "migration already in progress for table '{}/{}'",
                        op.repo, table_name
                    )));
                }

                // Get source table's data_store + info_store
                let src_table = db
                    .get_table(&op.repo, table_name)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let src_data = Arc::clone(src_table.table().data_store());
                let info_store = Arc::clone(src_table.info_store());

                // Resolve dst engine factory
                let dst_factory = match op.dst_engine.as_str() {
                    "in_memory" => BoxRepoFactory::in_memory(),
                    engine => {
                        return Err(err(format!(
                            "Migration dst_engine '{}' not yet supported. Supported: in_memory",
                            engine
                        )))
                    }
                };
                let dst_repo_name = &op.dst_repo;
                let dst_config = RepoConfig::new(dst_repo_name, dst_factory)
                    .add_table(TableConfig::new(table_name));
                db.add_repo(dst_config)
                    .await
                    .map_err(|e| err(e.to_string()))?;

                // From here on, any error must clean up dst repo
                // (rollback-on-failure). We pull dst_data + run snapshot/drain;
                // a `?` aborts the whole batch, but the dst repo would leak.
                // So unwind explicitly on failure.
                let run = async {
                    let dst_table = db.get_table(dst_repo_name, table_name).await?;
                    let dst_data = Arc::clone(dst_table.table().data_store());

                    // Step 1: replicate src's interner state into dst's
                    // info_store so the data_store bytes copied below
                    // decode with the same field-name → id mappings.
                    // Must precede any `.interner().get()` on dst.
                    dst_table.replicate_interner_from(&src_table).await?;

                    // Step 2: replicate index2 descriptors (FTS / Functional
                    // / Vector) from src → dst. Creates empty backends on
                    // dst so that bulk_populate_index2 (called later in
                    // CommitMigration) can fill them. Must happen before
                    // any data lands on dst.
                    dst_table
                        .replicate_index2_descriptors_from(&src_table)
                        .await?;

                    let shadow =
                        Arc::new(MigrationShadowLog::new(migration_id.clone(), info_store));
                    let state = MigrationState::new(
                        migration_id.clone(),
                        table_name.to_string(),
                        op.repo.clone(),
                        op.dst_repo.clone(),
                        op.dst_engine.clone(),
                        op.dst_path.clone(),
                    );
                    let coord =
                        Arc::new(MigrationCoordinator::new(state, shadow, src_data, dst_data));

                    coord.run_snapshot().await?;
                    coord.drain_until_caught_up(0).await?;
                    coord.mark_cutover_ready().await?;
                    Ok::<_, shamir_storage::error::DbError>(coord)
                }
                .await;

                let coord = match run {
                    Ok(c) => c,
                    Err(e) => {
                        // Roll back: remove the orphan dst repo.
                        db.remove_repo(dst_repo_name).await;
                        return Err(err(e.to_string()));
                    }
                };

                self.shamir
                    .active_migrations()
                    .insert(migration_id.clone(), coord);

                Ok(admin_result(json!({
                    "migration_id": migration_id,
                    "phase": "cutover_ready",
                    "table": table_name,
                    "src_repo": op.repo,
                    "dst_repo": op.dst_repo,
                    "dst_engine": op.dst_engine,
                })))
            }

            BatchOp::CommitMigration(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::database(self.db_name.clone()),
                        Action::Manage,
                    )
                    .await
                    .map_err(err_access)?;
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.commit_migration)
                    .ok_or_else(|| {
                        err_code(
                            "not_found",
                            format!("migration '{}' not found", op.commit_migration),
                        )
                    })?
                    .clone();
                let tail = coord
                    .final_drain_and_commit()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let (src_count, dst_count) = coord
                    .verify_record_count()
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let state = coord.state().await;

                // Bulk-populate index2 backends on dst.
                //
                // Order of operations:
                //   1. replicate_index2_descriptors_from (StartMigration) — empty backends
                //   2. run_snapshot + drain_until_caught_up — data_store only
                //   3. final_drain_and_commit — drains remaining shadow log
                //      entries into dst data_store (NO index2 hooks)
                //   4. bulk_populate_index2 ← we are here — streams ALL
                //      dst data_store records into on_batch_insert, creating
                //      postings in info_store + in-memory state.
                //
                // After this point the migration is committed. New writes
                // go through `insert()` → `index2_on_insert` automatically.
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let dst_table = db
                    .get_table(&state.dst_repo, &state.table_name)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                dst_table
                    .bulk_populate_index2()
                    .await
                    .map_err(|e| err(e.to_string()))?;

                // Remove from active map — committed migrations are
                // terminal, no further state changes possible. Status
                // queries on a committed id will now return 404, which
                // is the correct semantics (migration is done; query the
                // dst table directly).
                self.shamir.active_migrations().remove(&op.commit_migration);

                Ok(admin_result(json!({
                    "migration_id": op.commit_migration,
                    "phase": "committed",
                    "tail_drained": tail,
                    "src_records": src_count,
                    "dst_records": dst_count,
                    "records_copied": state.records_copied,
                })))
            }

            BatchOp::RollbackMigration(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::database(self.db_name.clone()),
                        Action::Manage,
                    )
                    .await
                    .map_err(err_access)?;
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.rollback_migration)
                    .ok_or_else(|| {
                        err_code(
                            "not_found",
                            format!("migration '{}' not found", op.rollback_migration),
                        )
                    })?
                    .clone();
                coord.rollback().await.map_err(|e| err(e.to_string()))?;
                self.shamir
                    .active_migrations()
                    .remove(&op.rollback_migration);

                Ok(admin_result(json!({
                    "migration_id": op.rollback_migration,
                    "phase": "rolled_back",
                })))
            }

            BatchOp::MigrationStatus(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::database(self.db_name.clone()),
                        Action::Read,
                    )
                    .await
                    .map_err(err_access)?;
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.migration_status)
                    .ok_or_else(|| {
                        err_code(
                            "not_found",
                            format!("migration '{}' not found", op.migration_status),
                        )
                    })?
                    .clone();
                let state = coord.state().await;
                let shadow_lag = coord.shadow_lag().await;

                Ok(admin_result(json!({
                    "migration_id": state.id,
                    "phase": state.phase.to_string(),
                    "table": state.table_name,
                    "src_repo": state.src_repo,
                    "dst_repo": state.dst_repo,
                    "dst_engine": state.dst_engine,
                    "snapshot_lsn": state.snapshot_lsn,
                    "last_lsn_applied": state.last_lsn_applied,
                    "records_copied": state.records_copied,
                    "shadow_lag": shadow_lag,
                })))
            }

            // ── Access-control DDL (S3) ─────────────────────────────────
            BatchOp::Chmod(op) => {
                let path = op
                    .chmod
                    .to_path()
                    .ok_or_else(|| err("invalid resource reference".to_string()))?;
                self.shamir
                    .authorize_access(&self.actor, &path, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let mut meta = self.shamir.resource_meta(&path).await;
                meta.mode = op.mode;
                self.shamir
                    .set_resource_meta(&path, &meta)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "chmod": serde_json::to_value(&op.chmod).map_err(|e| err(e.to_string()))?,
                    "mode": op.mode,
                })))
            }

            BatchOp::Chown(op) => {
                let path = op
                    .chown
                    .to_path()
                    .ok_or_else(|| err("invalid resource reference".to_string()))?;
                self.shamir
                    .authorize_access(&self.actor, &path, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let mut meta = self.shamir.resource_meta(&path).await;
                meta.owner = Actor::from_owner_id(op.owner);
                self.shamir
                    .set_resource_meta(&path, &meta)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "chown": serde_json::to_value(&op.chown).map_err(|e| err(e.to_string()))?,
                    "owner": op.owner,
                })))
            }

            BatchOp::Chgrp(op) => {
                let path = op
                    .chgrp
                    .to_path()
                    .ok_or_else(|| err("invalid resource reference".to_string()))?;
                self.shamir
                    .authorize_access(&self.actor, &path, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let mut meta = self.shamir.resource_meta(&path).await;
                meta.group = op.group;
                self.shamir
                    .set_resource_meta(&path, &meta)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "chgrp": serde_json::to_value(&op.chgrp).map_err(|e| err(e.to_string()))?,
                    "group": op.group,
                })))
            }

            BatchOp::CreateGroup(op) => {
                // Groups are global; managing them requires Manage on the root.
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let group_id = self
                    .shamir
                    .create_group(&op.create_group)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "created_group": op.create_group,
                    "group_id": group_id,
                })))
            }

            BatchOp::DropGroup(op) => {
                // Groups are global; managing them requires Manage on the root.
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let group_id = self
                    .shamir
                    .resolve_group_id(&op.drop_group)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                self.shamir
                    .drop_group(group_id)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "dropped_group_id": group_id,
                })))
            }

            BatchOp::AddGroupMember(op) => {
                // Groups are global; managing them requires Manage on the root.
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let group_id = self
                    .shamir
                    .resolve_group_id(&op.add_group_member)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                self.shamir
                    .add_group_member(group_id, op.user)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "added_to_group": group_id,
                    "user": op.user,
                })))
            }

            BatchOp::RemoveGroupMember(op) => {
                // Groups are global; managing them requires Manage on the root.
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let group_id = self
                    .shamir
                    .resolve_group_id(&op.remove_group_member)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                self.shamir
                    .remove_group_member(group_id, op.user)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "removed_from_group": group_id,
                    "user": op.user,
                })))
            }

            BatchOp::AccessTree(op) => {
                // Admin-only: reading the whole access fabric requires
                // `Manage` on the root. `System` bypasses; a non-admin
                // `User` actor is denied here.
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
                    .await
                    .map_err(err_access)?;
                let tree = self
                    .shamir
                    .access_tree(op.depth, op.db.as_deref())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({ "access_tree": tree })))
            }

            // ── Function DDL (DDL-A) ──────────────────────────────────
            BatchOp::CreateFunction(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::FunctionNamespace,
                        Action::Create,
                    )
                    .await
                    .map_err(err_access)?;
                if let Some(ref source) = op.source {
                    self.shamir
                        .create_function_from_source_as(
                            &op.create_function,
                            source,
                            op.replace,
                            self.actor.clone(),
                        )
                        .await
                        .map_err(|e| err(e.to_string()))?;
                } else if let Some(ref wasm_b64) = op.wasm {
                    let wasm_bytes = base64::engine::general_purpose::STANDARD
                        .decode(wasm_b64)
                        .map_err(|e| err(format!("invalid base64 wasm: {}", e)))?;
                    self.shamir
                        .create_function_from_wasm_as(
                            &op.create_function,
                            &wasm_bytes,
                            op.replace,
                            self.actor.clone(),
                        )
                        .await
                        .map_err(|e| err(e.to_string()))?;
                } else {
                    return Err(err(
                        "create_function requires either 'source' or 'wasm'".to_string()
                    ));
                }
                Ok(admin_result(
                    json!({"created_function": op.create_function}),
                ))
            }

            BatchOp::DropFunction(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::Function {
                            name: op.drop_function.clone(),
                        },
                        Action::Delete,
                    )
                    .await
                    .map_err(err_access)?;
                let existed = self
                    .shamir
                    .drop_function_as(&op.drop_function, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"dropped_function": op.drop_function, "existed": existed}),
                ))
            }

            BatchOp::RenameFunction(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::Function {
                            name: op.rename_function.clone(),
                        },
                        Action::Write,
                    )
                    .await
                    .map_err(err_access)?;
                self.shamir
                    .rename_function_as(&op.rename_function, &op.to, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"renamed_function": op.rename_function, "to": op.to}),
                ))
            }

            // ── Validator DDL (DDL-A) ─────────────────────────────────
            BatchOp::CreateValidator(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::FunctionNamespace,
                        Action::Create,
                    )
                    .await
                    .map_err(err_access)?;
                let id = if let Some(ref source) = op.source {
                    self.shamir
                        .create_validator_from_source_as(
                            &op.create_validator,
                            source,
                            op.replace,
                            self.actor.clone(),
                        )
                        .await
                        .map_err(|e| err(e.to_string()))?
                } else if let Some(ref wasm_b64) = op.wasm {
                    let wasm_bytes = base64::engine::general_purpose::STANDARD
                        .decode(wasm_b64)
                        .map_err(|e| err(format!("invalid base64 wasm: {}", e)))?;
                    self.shamir
                        .create_validator_from_wasm_as(
                            &op.create_validator,
                            &wasm_bytes,
                            op.replace,
                            self.actor.clone(),
                        )
                        .await
                        .map_err(|e| err(e.to_string()))?
                } else {
                    return Err(err(
                        "create_validator requires either 'source' or 'wasm'".to_string()
                    ));
                };
                Ok(admin_result(json!({
                    "created_validator": op.create_validator,
                    "id": id.to_string(),
                })))
            }

            BatchOp::DropValidator(op) => {
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::FunctionNamespace,
                        Action::Delete,
                    )
                    .await
                    .map_err(err_access)?;
                let existed = self
                    .shamir
                    .drop_validator_as(&op.drop_validator, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"dropped_validator": op.drop_validator, "existed": existed}),
                ))
            }

            BatchOp::RenameValidator(op) => {
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::FunctionNamespace, Action::Write)
                    .await
                    .map_err(err_access)?;
                self.shamir
                    .rename_validator_as(&op.rename_validator, &op.to, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"renamed_validator": op.rename_validator, "to": op.to}),
                ))
            }

            BatchOp::BindValidator(op) => {
                // Auth: Write on the target Table (binding changes the
                // table's write behaviour).
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::Table {
                            db: op.db.clone(),
                            store: op.repo.clone(),
                            table: op.table.clone(),
                        },
                        Action::Write,
                    )
                    .await
                    .map_err(err_access)?;
                self.shamir
                    .bind_validator_as(
                        &op.db,
                        &op.repo,
                        &op.table,
                        &op.bind_validator,
                        op.ops.clone(),
                        op.priority,
                        self.actor.clone(),
                    )
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "bound_validator": op.bind_validator,
                    "table": op.table,
                })))
            }

            BatchOp::UnbindValidator(op) => {
                // Auth: Write on the target Table.
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::Table {
                            db: op.db.clone(),
                            store: op.repo.clone(),
                            table: op.table.clone(),
                        },
                        Action::Write,
                    )
                    .await
                    .map_err(err_access)?;
                let removed = self
                    .shamir
                    .unbind_validator_as(
                        &op.db,
                        &op.repo,
                        &op.table,
                        &op.unbind_validator,
                        self.actor.clone(),
                    )
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({
                    "unbound_validator": op.unbind_validator,
                    "table": op.table,
                    "existed": removed,
                })))
            }

            BatchOp::ListValidators(op) => {
                // Auth: Read on the target Table.
                self.shamir
                    .authorize_access(
                        &self.actor,
                        &ResourcePath::Table {
                            db: op.db.clone(),
                            store: op.repo.clone(),
                            table: op.list_validators.clone(),
                        },
                        Action::Read,
                    )
                    .await
                    .map_err(err_access)?;
                let bindings = self
                    .shamir
                    .list_validator_bindings(&op.db, &op.repo, &op.list_validators)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                let bindings_json: Vec<serde_json::Value> = bindings
                    .iter()
                    .map(|b| {
                        json!({
                            "validator_id": b.validator_id.to_string(),
                            "priority": b.priority,
                        })
                    })
                    .collect();
                Ok(admin_result(json!({
                    "validators": bindings_json,
                    "table": op.list_validators,
                })))
            }

            // ── Function folder DDL ───────────────────────────────────
            BatchOp::CreateFunctionFolder(op) => {
                // Validate path segments.
                if op.create_function_folder.is_empty() {
                    return Err(err("function folder path must not be empty".to_string()));
                }
                for segment in &op.create_function_folder {
                    validate_name_component(segment, "folder segment")?;
                }

                // Auth: Create on the parent folder or FunctionNamespace
                // (if only one segment).
                let parent_path = if op.create_function_folder.len() == 1 {
                    ResourcePath::FunctionNamespace
                } else {
                    ResourcePath::FunctionFolder {
                        path: op.create_function_folder[..op.create_function_folder.len() - 1]
                            .to_vec(),
                    }
                };
                self.shamir
                    .authorize_access(&self.actor, &parent_path, Action::Create)
                    .await
                    .map_err(err_access)?;

                // mkdir -p: create all prefix folders that don't yet exist.
                let created = self
                    .shamir
                    .create_function_folder_as(&op.create_function_folder, self.actor.clone())
                    .await
                    .map_err(|e| err(e.to_string()))?;

                Ok(admin_result(json!({
                    "created_function_folder": op.create_function_folder,
                    "created": created,
                })))
            }

            _ => Err(err("Not an admin operation".to_string())),
        }
    }
}

impl ShamirAdminExecutor {
    /// Authorize a user-lifecycle op (`CreateUser` / `DropUser`) under the
    /// owner-delegation model.
    ///
    /// Two acceptance paths (either suffices):
    ///   1. **Global admin** — `Manage` on [`ResourcePath::Root`]. `System`
    ///      bypasses inside [`authorize_access`]. A global admin may manage
    ///      any user, scoped or not.
    ///   2. **Database owner** — when `scope == Some(db)` and the actor holds
    ///      `Manage` on [`ResourcePath::Database`] for that `db`. Lets a
    ///      database owner manage users bound to *their* database without
    ///      global-admin rights.
    ///
    /// Returns the original root-level [`AccessError`] when neither path
    /// admits, so the denial message reflects the admin domain.
    async fn authorize_user_lifecycle(
        &self,
        scope: Option<&str>,
    ) -> Result<(), shamir_types::access::AccessError> {
        // Path 1: global admin (Manage on the root). System bypasses here.
        let root_decision = self
            .shamir
            .authorize_access(&self.actor, &ResourcePath::Root, Action::Manage)
            .await;
        if root_decision.is_ok() {
            return Ok(());
        }

        // Path 2: database owner of the user's scope.
        if let Some(db) = scope {
            if self
                .shamir
                .authorize_access(
                    &self.actor,
                    &ResourcePath::Database { db: db.to_string() },
                    Action::Manage,
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
        }

        // Neither path admits — surface the root-level denial.
        root_decision
    }
}

// ============================================================================
// FunctionInvoker — stored procedure / callable function invocation
// ============================================================================

/// FunctionInvoker that invokes functions via `ShamirDb::invoke_function_in_db_as`.
struct ShamirFunctionInvoker {
    shamir: ShamirDb,
    db_name: String,
}

#[async_trait::async_trait]
impl FunctionInvoker for ShamirFunctionInvoker {
    async fn invoke_call(
        &self,
        op: &crate::query::CallOp,
        actor: &Actor,
        resolved_refs: &crate::types::common::TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        // Convert positional Vec<FilterValue> params into Params, resolving
        // `$query` references against `resolved_refs` (Phase 2). Literals
        // pass through unchanged.
        //
        // Layout:
        //   - Each param at index i is stored under key "i" (positional access).
        //   - The full array is stored under key "args" as QueryValue::List.
        //
        // Guest SDK reads: `params.get("0")` for first arg, or
        // `params.get("args")` for the whole array.
        let mut params = crate::engine::function::Params::new();
        let mut args_list = Vec::with_capacity(op.params.len());
        for (i, fv) in op.params.iter().enumerate() {
            let qv = filter_value_to_query_value(fv, resolved_refs);
            let key: String = i.to_string();
            params.set(key, qv.clone());
            args_list.push(qv);
        }
        params.set("args", crate::types::value::QueryValue::List(args_list));

        let qv = self
            .shamir
            .invoke_function_in_db_as(&self.db_name, &op.repo, &op.call, params, actor.clone())
            .await
            .map_err(|e| BatchError::QueryError {
                alias: String::new(),
                message: e.to_string(),
                code: None,
            })?;

        // Map QueryValue -> QueryResult with `value` field.
        let json_value = serde_json::to_value(&qv).unwrap_or(serde_json::Value::Null);
        Ok(QueryResult {
            records: vec![],
            stats: None,
            pagination: None,
            value: Some(json_value),
        })
    }
}

/// Convert a `FilterValue` literal to a `QueryValue`.
///
/// Literals (Null / Bool / Int / Float / String / Binary / Array) are mapped
/// directly. `$query` / `QueryRef` variants are resolved against
/// `resolved_refs` — the same value-first / records-second rules as the
/// filter evaluator (Phase 2). Other dynamic variants (`$ref`, `$fn`, `$expr`,
/// `$cond`) collapse to `Null` here; they are not meaningful as Call params.
fn filter_value_to_query_value(
    fv: &crate::query::FilterValue,
    resolved_refs: &crate::types::common::TMap<String, QueryResult>,
) -> crate::types::value::QueryValue {
    use crate::query::FilterValue;
    use crate::types::value::QueryValue;

    match fv {
        FilterValue::Null => QueryValue::Null,
        FilterValue::Bool(b) => QueryValue::Bool(*b),
        FilterValue::Int(i) => QueryValue::Int(*i),
        FilterValue::Float(f) => QueryValue::F64(*f),
        FilterValue::String(s) => QueryValue::Str(s.clone()),
        FilterValue::Binary(b) => QueryValue::Bin(b.clone()),
        FilterValue::Array(arr) => QueryValue::List(
            arr.iter()
                .map(|v| filter_value_to_query_value(v, resolved_refs))
                .collect(),
        ),
        FilterValue::QueryRef { alias, path } => {
            let key = alias.strip_prefix('@').unwrap_or(alias.as_str());
            let Some(qr) = resolved_refs.get(key) else {
                return QueryValue::Null;
            };
            // Same value-first / records-second rule as the filter evaluator:
            // a Call result lives in `value`; a Read result lives in `records`.
            if let Some(value) = &qr.value {
                json_value_to_query_value(value, path.as_deref())
            } else if path.is_none() {
                // No path + Read result: synthesize from the records array.
                let arr: Vec<QueryValue> = qr
                    .records
                    .iter()
                    .map(|r| json_value_to_query_value(r, None))
                    .collect();
                QueryValue::List(arr)
            } else {
                // Indexed/field path into records. Only the `[n]` form is
                // meaningful without a record context; walk to the record
                // then serialise it.
                let path = path.as_deref().unwrap_or("");
                if let Some(rest) = path.strip_prefix('[') {
                    if let Some(end) = rest.find(']') {
                        if let Ok(idx) = rest[..end].parse::<usize>() {
                            if let Some(record) = qr.records.get(idx) {
                                let after = &rest[end + 1..];
                                if let Some(field_path) = after.strip_prefix('.') {
                                    if let Some(field_val) = record.get(field_path) {
                                        return json_value_to_query_value(field_val, None);
                                    }
                                    return QueryValue::Null;
                                }
                                return json_value_to_query_value(record, None);
                            }
                        }
                    }
                }
                QueryValue::Null
            }
        }
        // $ref / $fn / $expr / $cond — not meaningful as positional params.
        _ => QueryValue::Null,
    }
}

/// Convert a `serde_json::Value` (the wire representation used in
/// `QueryResult.value` / `QueryResult.records`) into a `QueryValue`, with
/// optional path navigation. Used to resolve `$query` refs inside Call
/// params.
fn json_value_to_query_value(
    v: &serde_json::Value,
    path: Option<&str>,
) -> crate::types::value::QueryValue {
    use crate::types::value::QueryValue;

    let Some(target) = navigate_json_value(v, path) else {
        return QueryValue::Null;
    };
    match target {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                QueryValue::Int(i)
            } else {
                QueryValue::F64(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => QueryValue::Str(s.clone()),
        serde_json::Value::Array(arr) => QueryValue::List(
            arr.iter()
                .map(|v| json_value_to_query_value(v, None))
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut out = crate::types::common::new_map();
            for (k, vv) in map {
                out.insert(k.clone(), json_value_to_query_value(vv, None));
            }
            QueryValue::Map(out)
        }
    }
}

/// Walk a path like `.field`, `[0]`, `[0].name` through a `serde_json::Value`.
/// Mirrors `resolve_json_path` in the filter evaluator — duplicated here to
/// keep `shamir-db` independent of `shamir-engine::query::filter::eval`
/// (which is crate-private). Returns `None` on any miss / unsupported syntax.
fn navigate_json_value<'a>(
    mut cur: &'a serde_json::Value,
    path: Option<&str>,
) -> Option<&'a serde_json::Value> {
    let Some(path) = path else {
        return Some(cur);
    };
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            cur = cur.get(&after_dot[..end])?;
            rest = &after_dot[end..];
        } else if rest.starts_with('[') {
            let bracket_end = rest.find(']')?;
            let idx: usize = rest[1..bracket_end].parse().ok()?;
            cur = cur.get(idx)?;
            rest = &rest[bracket_end + 1..];
        } else {
            return None;
        }
    }
    Some(cur)
}

/// Map the wire DTO into the storage struct without dragging the
/// storage crate's serde-compatible-by-coincidence layout into
/// the API contract — the two types are intentionally distinct.
fn storage_from_dto(
    dto: &crate::query::admin::BufferConfigDto,
) -> crate::storage::storage_membuffer::MemBufferConfig {
    crate::storage::storage_membuffer::MemBufferConfig {
        max_bytes: dto.max_bytes,
        max_entries: dto.max_entries,
        ttl_ms: dto.ttl_ms,
        flush_interval_ms: dto.flush_interval_ms,
        flush_batch_size: dto.flush_batch_size,
    }
}

fn dto_from_storage(
    cfg: &crate::storage::storage_membuffer::MemBufferConfig,
) -> crate::query::admin::BufferConfigDto {
    crate::query::admin::BufferConfigDto {
        max_bytes: cfg.max_bytes,
        max_entries: cfg.max_entries,
        ttl_ms: cfg.ttl_ms,
        flush_interval_ms: cfg.flush_interval_ms,
        flush_batch_size: cfg.flush_batch_size,
    }
}

/// Apply only the fields the patch actually set; leave the rest
/// alone. Double-option semantics for `ttl_ms`: `Some(None)` ↔
/// "clear TTL"; `Some(Some(v))` ↔ "set TTL"; `None` ↔ "untouched".
fn apply_patch(
    cfg: &mut crate::storage::storage_membuffer::MemBufferConfig,
    patch: &crate::query::admin::BufferConfigPatch,
) {
    if let Some(v) = patch.max_bytes {
        cfg.max_bytes = v;
    }
    if let Some(v) = patch.max_entries {
        cfg.max_entries = v;
    }
    if let Some(v) = patch.ttl_ms {
        cfg.ttl_ms = v;
    }
    if let Some(v) = patch.flush_interval_ms {
        cfg.flush_interval_ms = v;
    }
    if let Some(v) = patch.flush_batch_size {
        cfg.flush_batch_size = v;
    }
}

/// Hash a plaintext password into an Argon2id PHC string for at-rest
/// storage in the `users` table. Salt is drawn from the OS CSPRNG
/// (`OsRng`) per a fresh 16-byte `SaltString`; params are the `argon2`
/// crate defaults (Argon2id, v0x13). Returns the self-describing PHC
/// string (`$argon2id$v=19$m=...$<salt>$<hash>`), which embeds the salt
/// and params so verification needs no side-channel state.
///
/// NOTE: this field is admin/RBAC metadata, not the live-auth
/// credential — wire login is SCRAM-Argon2id in `shamir-connect`. No
/// verify site reads `users.password_hash`, so hashing here is purely
/// defense-in-depth at rest.
fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
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
        value: None,
    }
}

impl ShamirDb {
    /// Execute a batch request against a specific database.
    pub async fn execute(
        &self,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        self.execute_as(Actor::System, db_name, request).await
    }

    /// Execute a batch request with an explicit [`Actor`] for access control.
    ///
    /// This is the principal-aware entry point called by the server with the
    /// authenticated session's actor. The convenience [`execute`] delegates
    /// here with `Actor::System` (admin bypass) for backward compatibility.
    pub async fn execute_as(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
    ) -> Result<BatchResponse, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;

        // Per-op authorization: each data op is checked against its TARGET
        // table (admin/DDL ops carry no table_ref and are authorized in
        // execute_admin). authorize_access traverses the db/store ancestors,
        // so the table path covers the whole chain. System bypasses.
        for entry in request.queries.values() {
            if let Some(tref) = entry.op.table_ref() {
                let action = match &entry.op {
                    BatchOp::Read(_) => Action::Read,
                    BatchOp::Insert(_) => Action::Create,
                    BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
                    BatchOp::Delete(_) => Action::Delete,
                    _ => Action::Write,
                };
                let path = ResourcePath::Table {
                    db: db_name.to_string(),
                    store: tref.repo.clone(),
                    table: tref.table.clone(),
                };
                self.authorize_access(&actor, &path, action)
                    .await
                    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
            }
        }

        let resolver = DbTableResolver {
            db,
            validators: self.validators().clone(),
        };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        };

        let invoker = ShamirFunctionInvoker {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        };
        execute_batch(
            request,
            &resolver,
            Some(&admin),
            Some(&invoker),
            actor,
            db_name,
        )
        .await
    }
}

// ===========================================================================
// Phase B — interactive (multi-call) transactions
//
// These facade methods expose the engine's interactive-tx glue
// (`open_interactive_tx` / `execute_in_open_tx` / `commit_interactive_tx`)
// to the server, which owns the live-tx registry (it depends on `shamir-tx`
// directly). The facade resolves the db/repo and builds the same
// resolver + admin pair `execute` uses, then drives one lifecycle step. The
// `TxContext` / `SnapshotGuard` flow back to the server registry via the
// engine re-export (`crate::engine::tx::*`) — the same concrete `shamir-tx`
// types the server names. See `docs/roadmap/PHASE_B_INTERACTIVE_TX.md` §5.
// ===========================================================================

impl ShamirDb {
    /// BEGIN: open an interactive tx against `db_name`/`repo_name`. Returns
    /// the live `TxContext` + its `SnapshotGuard` for the caller (the server
    /// registry) to park between round-trips.
    pub async fn tx_begin(
        &self,
        db_name: &str,
        repo_name: &str,
        isolation: &str,
    ) -> Result<
        (
            crate::engine::tx::TxContext,
            crate::engine::tx::SnapshotGuard,
        ),
        BatchError,
    > {
        self.tx_begin_as(Actor::System, db_name, repo_name, isolation)
            .await
    }

    /// BEGIN with an explicit [`Actor`].
    pub async fn tx_begin_as(
        &self,
        actor: Actor,
        db_name: &str,
        repo_name: &str,
        isolation: &str,
    ) -> Result<
        (
            crate::engine::tx::TxContext,
            crate::engine::tx::SnapshotGuard,
        ),
        BatchError,
    > {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
                code: None,
            })?;
        let iso = match isolation {
            "serializable" => crate::engine::tx::IsolationLevel::Serializable,
            _ => crate::engine::tx::IsolationLevel::Snapshot,
        };
        let (mut tx, guard) =
            open_interactive_tx(&repo, iso)
                .await
                .map_err(|e| BatchError::QueryError {
                    alias: String::new(),
                    message: format!("begin_tx: {}", e),
                    code: None,
                })?;
        tx.set_actor(actor);
        Ok((tx, guard))
    }

    /// EXECUTE: run one batch inside an already-open interactive tx, WITHOUT
    /// committing. The `BatchResponse` carries `transaction: None` (the tx is
    /// still open). The single-repo guard is enforced inside the engine glue;
    /// the caller additionally asserts the batch targets the handle's repo.
    pub async fn tx_execute(
        &self,
        db_name: &str,
        request: &BatchRequest,
        tx: &mut crate::engine::tx::TxContext,
    ) -> Result<BatchResponse, BatchError> {
        self.tx_execute_as(Actor::System, db_name, request, tx)
            .await
    }

    /// EXECUTE with an explicit [`Actor`].
    pub async fn tx_execute_as(
        &self,
        actor: Actor,
        db_name: &str,
        request: &BatchRequest,
        tx: &mut crate::engine::tx::TxContext,
    ) -> Result<BatchResponse, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Read,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;

        // Per-op DML authorization (mirrors execute_as).
        for entry in request.queries.values() {
            if let Some(tref) = entry.op.table_ref() {
                let action = match &entry.op {
                    BatchOp::Read(_) => Action::Read,
                    BatchOp::Insert(_) => Action::Create,
                    BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
                    BatchOp::Delete(_) => Action::Delete,
                    _ => Action::Write,
                };
                let path = ResourcePath::Table {
                    db: db_name.to_string(),
                    store: tref.repo.clone(),
                    table: tref.table.clone(),
                };
                self.authorize_access(&actor, &path, action)
                    .await
                    .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
            }
        }

        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;
        let resolver = DbTableResolver {
            db,
            validators: self.validators().clone(),
        };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        };
        let invoker = ShamirFunctionInvoker {
            shamir: self.clone(),
            db_name: db_name.to_string(),
        };
        execute_in_open_tx(
            request,
            &resolver,
            Some(&admin),
            Some(&invoker),
            &actor,
            db_name,
            tx,
        )
        .await
    }

    /// COMMIT: run the Phase-A commit pipeline on a parked interactive tx and
    /// map the outcome to a wire [`TransactionInfo`] — `committed` (with the
    /// inherited `materialized` flag) on success, `aborted` with a reason on
    /// a commit-time conflict/violation. Mirrors the mapping the single-batch
    /// `execute_transactional` performs.
    pub async fn tx_commit(
        &self,
        db_name: &str,
        repo_name: &str,
        tx: crate::engine::tx::TxContext,
    ) -> Result<TransactionInfo, BatchError> {
        self.tx_commit_as(Actor::System, db_name, repo_name, tx)
            .await
    }

    /// COMMIT with an explicit [`Actor`].
    pub async fn tx_commit_as(
        &self,
        actor: Actor,
        db_name: &str,
        repo_name: &str,
        tx: crate::engine::tx::TxContext,
    ) -> Result<TransactionInfo, BatchError> {
        self.authorize_access(
            &actor,
            &ResourcePath::Database {
                db: db_name.to_string(),
            },
            Action::Write,
        )
        .await
        .map_err(|e| BatchError::query_coded("", "access_denied", e.to_string()))?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
            code: None,
        })?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
                code: None,
            })?;
        let tx_id = tx.tx_id.0;
        match commit_interactive_tx(&repo, tx).await {
            Ok(outcome) => Ok(TransactionInfo::committed(
                outcome.tx_id,
                outcome.snapshot_version,
                outcome.commit_version,
                outcome.materialized(),
            )),
            Err(commit_err) => {
                let reason = match commit_err {
                    crate::engine::tx::CommitError::SsiConflict { .. } => "tx_conflict".to_string(),
                    crate::engine::tx::CommitError::PhantomConflict { .. } => {
                        "tx_conflict".to_string()
                    }
                    crate::engine::tx::CommitError::Wounded { .. } => "tx_conflict".to_string(),
                    crate::engine::tx::CommitError::UniqueViolation { .. } => {
                        "unique_violation".to_string()
                    }
                    crate::engine::tx::CommitError::Storage(e) => format!("storage: {}", e),
                    crate::engine::tx::CommitError::Expired { elapsed, max } => {
                        format!("tx expired: elapsed {:?} > max {:?}", elapsed, max)
                    }
                };
                Ok(TransactionInfo::aborted(tx_id, reason))
            }
        }
    }
}
