//! Batch execution entry point for ShamirDb.

use serde_json::json;

use std::sync::Arc;

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::{TableConfig, TableManager};
use crate::query::batch::{
    commit_interactive_tx, execute_batch, execute_in_open_tx, open_interactive_tx, AdminExecutor,
    BatchError, BatchOp, BatchRequest, BatchResponse, TableResolver, TransactionInfo,
};
use crate::query::read::{QueryResult, QueryStats};
use crate::query::TableRef;
use crate::DbResult;

use crate::engine::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState};

use super::shamir_db::ShamirDb;
use crate::access::{Action, Actor, ResourcePath};

/// TableResolver that resolves TableRef within a DbInstance.
struct DbTableResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for DbTableResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&table_ref.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, repo_name: &str) -> DbResult<crate::engine::repo::RepoInstance> {
        self.db.get_repo(repo_name).ok_or_else(|| {
            crate::DbError::NotFound(format!("Repository '{}' not found", repo_name))
        })
    }
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
        };

        match op {
            BatchOp::CreateDb(op) => {
                self.shamir.create_db(&op.create_db).await;
                Ok(admin_result(json!({"created": op.create_db})))
            }

            BatchOp::DropDb(op) => {
                let removed = self.shamir.remove_db(&op.drop_db).await;
                Ok(admin_result(
                    json!({"dropped": op.drop_db, "existed": removed}),
                ))
            }

            BatchOp::CreateRepo(op) => {
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

                // Route through ShamirDb so the repo record and its inline
                // table catalogue are persisted to the system store and
                // survive a restart (symmetry with CreateTable, I.2). For an
                // in-memory engine only the catalogue record is durable — the
                // repo's data legitimately does not survive a process restart;
                // a re-attach on the next open creates a fresh empty repo.
                self.shamir
                    .add_repo(&self.db_name, config)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_repo": op.create_repo})))
            }

            BatchOp::DropRepo(op) => {
                // Route through ShamirDb so the repo's catalogue record is
                // removed from the system store and the repo does not
                // resurrect on the next open (symmetry with CreateRepo).
                let removed = self.shamir.remove_repo(&self.db_name, &op.drop_repo).await;
                Ok(admin_result(
                    json!({"dropped_repo": op.drop_repo, "existed": removed}),
                ))
            }

            BatchOp::CreateTable(op) => {
                // Route through ShamirDb so the table is persisted to the
                // catalogue and survives a restart (I.2).
                self.shamir
                    .add_table(&self.db_name, &op.repo, &op.create_table, false)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"created_table": op.create_table, "repo": op.repo}),
                ))
            }

            BatchOp::DropTable(op) => {
                let removed = self
                    .shamir
                    .drop_table(&self.db_name, &op.repo, &op.drop_table)
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"dropped_table": op.drop_table, "existed": removed}),
                ))
            }

            BatchOp::CreateIndex(op) => {
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let table = db
                    .get_table(&op.repo, &op.table)
                    .await
                    .map_err(|e| err(e.to_string()))?;

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
                if op.sorted {
                    if op.fields.len() != 1 {
                        return Err(err(
                            "Sorted index requires exactly one field (composite TBD)".to_string(),
                        ));
                    }
                    table
                        .create_sorted_index(&op.create_index, &path_refs)
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
                        let dbs = self.shamir.list_dbs();
                        Ok(admin_result(json!({"databases": dbs})))
                    }
                    ListOp::Repos => {
                        let db = self
                            .shamir
                            .get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let repos = db.list_repos();
                        Ok(admin_result(json!({"repos": repos})))
                    }
                    ListOp::Tables { repo } => {
                        let db = self
                            .shamir
                            .get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let tables = db.list_tables(repo).map_err(|e| err(e.to_string()))?;
                        Ok(admin_result(json!({"tables": tables, "repo": repo})))
                    }
                    ListOp::Users => {
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
                }
            }

            BatchOp::CreateUser(op) => {
                // Hash the password at rest with Argon2id (PHC string).
                // This `users.password_hash` field is RBAC/admin metadata,
                // NOT a live-auth credential — the wire login path is
                // SCRAM-Argon2id in `shamir-connect` over StoredKey /
                // ServerKey, which never reads this field. Hashing here is
                // defense-in-depth for the at-rest secret; no verify-side
                // change is required.
                let password_hash = hash_password(&op.password).map_err(|e| err(e.to_string()))?;
                let user = crate::query::auth::User {
                    name: op.create_user.clone(),
                    password_hash,
                    roles: op.roles.clone(),
                    profile: op.profile.clone(),
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
                    return Err(err(format!("User '{}' not found", op.user)));
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
                    return Err(err(format!("User '{}' not found", op.user)));
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
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.commit_migration)
                    .ok_or_else(|| err(format!("migration '{}' not found", op.commit_migration)))?
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
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.rollback_migration)
                    .ok_or_else(|| err(format!("migration '{}' not found", op.rollback_migration)))?
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
                let coord = self
                    .shamir
                    .active_migrations()
                    .get(&op.migration_status)
                    .ok_or_else(|| err(format!("migration '{}' not found", op.migration_status)))?
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
                    .map_err(|e| err(e.to_string()))?;
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
                    .map_err(|e| err(e.to_string()))?;
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
                    .map_err(|e| err(e.to_string()))?;
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
                    .map_err(|e| err(e.to_string()))?;
                let tree = self
                    .shamir
                    .access_tree(op.depth, op.db.as_deref())
                    .await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({ "access_tree": tree })))
            }

            _ => Err(err("Not an admin operation".to_string())),
        }
    }
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
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: e.to_string(),
        })?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
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
                    .map_err(|e| BatchError::QueryError {
                        alias: String::new(),
                        message: e.to_string(),
                    })?;
            }
        }

        let resolver = DbTableResolver { db };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        };

        execute_batch(request, &resolver, Some(&admin), actor, db_name).await
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
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: e.to_string(),
        })?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
        })?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
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
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: e.to_string(),
        })?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
        })?;
        let resolver = DbTableResolver { db };
        let admin = ShamirAdminExecutor {
            shamir: self.clone(),
            db_name: db_name.to_string(),
            actor: actor.clone(),
        };
        execute_in_open_tx(request, &resolver, Some(&admin), &actor, db_name, tx).await
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
        .map_err(|e| BatchError::QueryError {
            alias: String::new(),
            message: e.to_string(),
        })?;
        let db = self.get_db(db_name).ok_or_else(|| BatchError::QueryError {
            alias: String::new(),
            message: format!("Database '{}' not found", db_name),
        })?;
        let repo = db
            .get_repo(repo_name)
            .ok_or_else(|| BatchError::QueryError {
                alias: String::new(),
                message: format!("Repository '{}' not found", repo_name),
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
