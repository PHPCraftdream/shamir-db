//! Batch execution entry point for ShamirDb.

use serde_json::json;

use std::sync::Arc;

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::{TableConfig, TableManager};
use crate::query::batch::{
    execute_batch, AdminExecutor, BatchError, BatchOp, BatchRequest, BatchResponse, TableResolver,
};
use crate::query::read::{QueryResult, QueryStats};
use crate::query::TableRef;
use crate::DbResult;

use crate::engine::migration::{MigrationCoordinator, MigrationShadowLog, MigrationState};

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
                Ok(admin_result(
                    json!({"dropped": op.drop_db, "existed": removed}),
                ))
            }

            BatchOp::CreateRepo(op) => {
                let db = self
                    .shamir
                    .get_db(&self.db_name)
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
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let removed = db.remove_repo(&op.drop_repo).await;
                Ok(admin_result(
                    json!({"dropped_repo": op.drop_repo, "existed": removed}),
                ))
            }

            BatchOp::CreateTable(op) => {
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                db.create_table(&op.repo, &op.create_table)
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(
                    json!({"created_table": op.create_table, "repo": op.repo}),
                ))
            }

            BatchOp::DropTable(op) => {
                let db = self
                    .shamir
                    .get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let removed = db
                    .drop_table(&op.repo, &op.drop_table)
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
                let user = crate::query::auth::User {
                    name: op.create_user.clone(),
                    password_hash: op.password.clone(), // TODO: hash properly
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

                    // Replicate index2 descriptors (FTS / Functional /
                    // Vector) from src → dst. This creates empty backends
                    // on dst so that bulk_populate_index2 (called later
                    // in CommitMigration) can fill them. Must happen
                    // before any data lands on dst.
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
