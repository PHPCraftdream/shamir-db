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
                db.create_table(&op.repo, &op.create_table)
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_table": op.create_table, "repo": op.repo})))
            }

            BatchOp::DropTable(op) => {
                let db = self.shamir.get_db(&self.db_name)
                    .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                let removed = db.drop_table(&op.repo, &op.drop_table)
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"dropped_table": op.drop_table, "existed": removed})))
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
                    ListOp::Users => {
                        let table = self.shamir.system_store().users_table().await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = table.interner().get().await
                            .map_err(|e| err(e.to_string()))?;
                        let refs = crate::types::common::new_map();
                        let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                        let query = crate::db::query::read::ReadQuery::new("users");
                        let result = table.read(&query, &ctx).await
                            .map_err(|e| err(e.to_string()))?;
                        // Strip password_hash from output
                        let users: Vec<serde_json::Value> = result.records.into_iter().map(|mut r| {
                            if let Some(obj) = r.as_object_mut() {
                                obj.remove("password_hash");
                            }
                            r
                        }).collect();
                        Ok(admin_result(json!({"users": users})))
                    }
                    ListOp::Roles => {
                        let table = self.shamir.system_store().roles_table().await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = table.interner().get().await
                            .map_err(|e| err(e.to_string()))?;
                        let refs = crate::types::common::new_map();
                        let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                        let query = crate::db::query::read::ReadQuery::new("roles");
                        let result = table.read(&query, &ctx).await
                            .map_err(|e| err(e.to_string()))?;
                        Ok(admin_result(json!({"roles": result.records})))
                    }
                    ListOp::Indexes { table, repo } => {
                        let db = self.shamir.get_db(&self.db_name)
                            .ok_or_else(|| err(format!("Database '{}' not found", self.db_name)))?;
                        let tm = db.get_table(repo, table).await
                            .map_err(|e| err(e.to_string()))?;
                        let interner = tm.interner().get().await
                            .map_err(|e| err(e.to_string()))?;

                        let mut indexes = Vec::new();
                        for def in tm.index_manager_ref().iter_indexes() {
                            let name = interner
                                .get_str(&crate::core::interner::InternerKey::new(def.name_interned))
                                .map(|k| k.as_str().to_string())
                                .unwrap_or_else(|| def.name_interned.to_string());
                            indexes.push(json!({"name": name, "unique": false}));
                        }
                        for def in tm.index_manager_ref().iter_unique_indexes() {
                            let name = interner
                                .get_str(&crate::core::interner::InternerKey::new(def.name_interned))
                                .map(|k| k.as_str().to_string())
                                .unwrap_or_else(|| def.name_interned.to_string());
                            indexes.push(json!({"name": name, "unique": true}));
                        }

                        Ok(admin_result(json!({"indexes": indexes, "table": table, "repo": repo})))
                    }
                }
            }

            BatchOp::CreateUser(op) => {
                let user = crate::db::query::auth::User {
                    name: op.create_user.clone(),
                    password_hash: op.password.clone(), // TODO: hash properly
                    roles: op.roles.clone(),
                    profile: op.profile.clone(),
                };
                let user_json = serde_json::to_value(&user).map_err(|e| err(e.to_string()))?;
                let table = self.shamir.system_store().users_table().await
                    .map_err(|e| err(e.to_string()))?;
                let set_op = crate::db::query::write::SetOp {
                    set: crate::db::query::TableRef::new("users"),
                    key: json!({"name": op.create_user}),
                    value: user_json,
                };
                table.execute_set(&set_op).await.map_err(|e| err(e.to_string()))?;
                table.interner().persist().await.map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_user": op.create_user})))
            }

            BatchOp::DropUser(op) => {
                let table = self.shamir.system_store().users_table().await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table.interner().get().await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                let del_op = crate::db::query::write::DeleteOp {
                    delete_from: crate::db::query::TableRef::new("users"),
                    where_clause: crate::db::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::db::query::filter::FilterValue::String(op.drop_user.clone()),
                    },
                };
                let result = table.execute_delete(&del_op, &ctx).await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"dropped_user": op.drop_user, "existed": result.affected > 0})))
            }

            BatchOp::CreateRole(op) => {
                let role = crate::db::query::auth::Role {
                    name: op.create_role.clone(),
                    permissions: op.permissions.clone(),
                };
                let role_json = serde_json::to_value(&role).map_err(|e| err(e.to_string()))?;
                let table = self.shamir.system_store().roles_table().await
                    .map_err(|e| err(e.to_string()))?;
                let set_op = crate::db::query::write::SetOp {
                    set: crate::db::query::TableRef::new("roles"),
                    key: json!({"name": op.create_role}),
                    value: role_json,
                };
                table.execute_set(&set_op).await.map_err(|e| err(e.to_string()))?;
                table.interner().persist().await.map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"created_role": op.create_role})))
            }

            BatchOp::DropRole(op) => {
                let table = self.shamir.system_store().roles_table().await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table.interner().get().await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                let del_op = crate::db::query::write::DeleteOp {
                    delete_from: crate::db::query::TableRef::new("roles"),
                    where_clause: crate::db::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::db::query::filter::FilterValue::String(op.drop_role.clone()),
                    },
                };
                let result = table.execute_delete(&del_op, &ctx).await
                    .map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"dropped_role": op.drop_role, "existed": result.affected > 0})))
            }

            BatchOp::GrantRole(op) => {
                // Read user, add role, write back
                let table = self.shamir.system_store().users_table().await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table.interner().get().await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                let query = crate::db::query::read::ReadQuery::new("users")
                    .filter(crate::db::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::db::query::filter::FilterValue::String(op.user.clone()),
                    });
                let result = table.read(&query, &ctx).await
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
                let set_op = crate::db::query::write::SetOp {
                    set: crate::db::query::TableRef::new("users"),
                    key: json!({"name": op.user}),
                    value: user_json,
                };
                table.execute_set(&set_op).await.map_err(|e| err(e.to_string()))?;
                table.interner().persist().await.map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"granted_role": op.grant_role, "user": op.user})))
            }

            BatchOp::RevokeRole(op) => {
                let table = self.shamir.system_store().users_table().await
                    .map_err(|e| err(e.to_string()))?;
                let interner = table.interner().get().await
                    .map_err(|e| err(e.to_string()))?;
                let refs = crate::types::common::new_map();
                let ctx = crate::db::query::filter::FilterContext::new(interner, &refs);
                let query = crate::db::query::read::ReadQuery::new("users")
                    .filter(crate::db::query::filter::Filter::Eq {
                        field: vec!["name".to_string()],
                        value: crate::db::query::filter::FilterValue::String(op.user.clone()),
                    });
                let result = table.read(&query, &ctx).await
                    .map_err(|e| err(e.to_string()))?;
                if result.records.is_empty() {
                    return Err(err(format!("User '{}' not found", op.user)));
                }
                let mut user_json = result.records[0].clone();
                if let Some(roles) = user_json.get_mut("roles").and_then(|r| r.as_array_mut()) {
                    roles.retain(|r| r != &json!(op.revoke_role));
                }
                let set_op = crate::db::query::write::SetOp {
                    set: crate::db::query::TableRef::new("users"),
                    key: json!({"name": op.user}),
                    value: user_json,
                };
                table.execute_set(&set_op).await.map_err(|e| err(e.to_string()))?;
                table.interner().persist().await.map_err(|e| err(e.to_string()))?;
                Ok(admin_result(json!({"revoked_role": op.revoke_role, "user": op.user})))
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
