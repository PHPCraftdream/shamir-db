//! Admin handler: List.

use serde_json::json;

use crate::access::{Action, ResourcePath};
use crate::query::admin::ListOp;
use crate::query::batch::BatchError;
use crate::query::read::QueryResult;

use super::admin_dispatch::ShamirAdminExecutor;
use super::helpers::admin_result;

impl ShamirAdminExecutor {
    pub(super) async fn handle_list(&self, list_op: &ListOp) -> Result<QueryResult, BatchError> {
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
                        &ResourcePath::table(self.db_name.clone(), repo.clone(), table.clone()),
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

                Ok(admin_result(
                    json!({"indexes": indexes, "table": table, "repo": repo}),
                ))
            }
            ListOp::Functions { folder } => {
                self.shamir
                    .authorize_access(&self.actor, &ResourcePath::FunctionNamespace, Action::List)
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
                    .authorize_access(&self.actor, &ResourcePath::FunctionNamespace, Action::List)
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
                    .authorize_access(&self.actor, &ResourcePath::FunctionNamespace, Action::List)
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
}
