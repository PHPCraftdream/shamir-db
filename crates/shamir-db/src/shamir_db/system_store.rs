//! Persistent system store for ShamirDB metadata.
//!
//! Stores databases, repositories, settings, users, roles.
//! Uses a TableManager backed by any storage engine (redb for production,
//! in_memory for tests).

use serde_json::json;

use shamir_types::access::{Actor, ResourceMeta};

use crate::codecs::interned::json_value_to_inner;
use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::{RepoConfig, RepoInstance};
use crate::engine::table::{TableConfig, TableManager};
use crate::{DbError, DbResult};

const SYSTEM_REPO: &str = "system";

/// System store tables
const TABLE_DATABASES: &str = "databases";
const TABLE_REPOSITORIES: &str = "repositories";
/// Per-repo table catalogue: one record per user table so the table
/// list survives a restart and crash-recovery can resolve `table_by_token`
/// for disk-backed repos (I.2).
const TABLE_TABLES: &str = "tables";
const TABLE_SETTINGS: &str = "settings";
const TABLE_USERS: &str = "users";
const TABLE_ROLES: &str = "roles";
/// Function catalogue: one record per user-defined WASM function so the
/// function survives a restart (slice 4).
const TABLE_FUNCTIONS: &str = "functions";
/// Groups catalogue: group_id → (name, members).
/// Group ids are allocated monotonically starting from 1 (stored in
/// the `settings` table under the key `"next_group_id"`). Id 0 is
/// reserved / unused.
const TABLE_GROUPS: &str = "groups";
/// Validator catalogue: one record per user-defined WASM validator so
/// the validator survives a restart (S1).
const TABLE_VALIDATORS: &str = "validators";
/// Function folder catalogue: one record per explicitly created folder
/// (e.g. `"reports/daily"`). Key is the slash-joined path. Persists
/// ResourceMeta (owner/group/mode) so folder ACLs survive a restart (#118).
const TABLE_FUNCTION_FOLDERS: &str = "function_folders";

/// Configuration for the system store.
#[derive(Clone)]
pub enum SystemStoreConfig {
    /// In-memory (for tests). Data lost on restart.
    InMemory,
    /// Persistent redb at the given path.
    Redb(std::path::PathBuf),
}

/// Persistent system store.
#[derive(Clone)]
pub struct SystemStore {
    db: DbInstance,
}

impl SystemStore {
    /// Initialize system store with the given config.
    pub async fn init(config: SystemStoreConfig) -> DbResult<Self> {
        let db = DbInstance::new();

        let factory = match config {
            SystemStoreConfig::InMemory => BoxRepoFactory::in_memory(),
            SystemStoreConfig::Redb(path) => BoxRepoFactory::redb(path),
        };

        let repo_config = RepoConfig::new(SYSTEM_REPO, factory)
            .add_table(TableConfig::new(TABLE_DATABASES))
            .add_table(TableConfig::new(TABLE_REPOSITORIES))
            .add_table(TableConfig::new(TABLE_TABLES))
            .add_table(TableConfig::new(TABLE_SETTINGS))
            .add_table(TableConfig::new(TABLE_USERS))
            .add_table(TableConfig::new(TABLE_ROLES))
            .add_table(TableConfig::new(TABLE_FUNCTIONS))
            .add_table(TableConfig::new(TABLE_GROUPS))
            .add_table(TableConfig::new(TABLE_VALIDATORS))
            .add_table(TableConfig::new(TABLE_FUNCTION_FOLDERS));

        db.add_repo(repo_config).await?;

        Ok(Self { db })
    }

    /// Get the table manager for a system table.
    async fn table(&self, name: &str) -> DbResult<TableManager> {
        self.db.get_table(SYSTEM_REPO, name).await
    }

    /// Resolve the SYSTEM_REPO [`RepoInstance`].
    ///
    /// F5a: system-store DELETEs route through the repo's implicit-tx
    /// file-WAL path (`run_implicit_batch_tx` + `execute_delete_tx`) instead
    /// of the direct V1-marker `execute_delete`, so the V1 DELETE marker
    /// becomes dead.
    pub(crate) fn system_repo(&self) -> DbResult<RepoInstance> {
        self.db
            .get_repo(SYSTEM_REPO)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", SYSTEM_REPO)))
    }

    /// Route a single non-tx DELETE through the implicit-tx file-WAL path
    /// (mirrors the `query_runner` non-tx Delete branch). Maps the
    /// [`BatchError`] surfaced by the implicit tx onto a [`DbError`].
    async fn delete_via_implicit_tx(
        &self,
        table: &TableManager,
        op: &crate::query::write::DeleteOp,
    ) -> DbResult<crate::query::write::WriteResult> {
        let repo = self.system_repo()?;
        let owned_op = op.clone();
        let owned_table = table.clone();
        repo.run_implicit_batch_tx(Actor::System, "", move |tx| {
            Box::pin(async move {
                let interner = owned_table.interner().get().await?;
                let refs = crate::types::common::new_map();
                let ctx = crate::query::filter::FilterContext::new(interner, &refs);
                owned_table.execute_delete_tx(&owned_op, &ctx, tx).await
            })
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))
    }

    /// Route a single non-tx SET (upsert) through the implicit-tx file-WAL
    /// path (mirrors the `query_runner` non-tx Set branch, W3d-2). Maps the
    /// [`BatchError`] surfaced by the implicit tx onto a [`DbError`].
    pub(crate) async fn set_via_implicit_tx(
        &self,
        table: &TableManager,
        op: &crate::query::write::SetOp,
    ) -> DbResult<crate::query::write::WriteResult> {
        let repo = self.system_repo()?;
        let owned_op = op.clone();
        let owned_table = table.clone();
        repo.run_implicit_batch_tx(Actor::System, "", move |tx| {
            Box::pin(async move { owned_table.execute_set_tx(&owned_op, tx).await })
        })
        .await
        .map_err(|e| DbError::Internal(e.to_string()))
    }

    // ========================================================================
    // Database metadata
    // ========================================================================

    /// Save database metadata. Injects the `owner`/`group`/`mode` fields
    /// from `meta` into the record before persisting (P3 metadata plates).
    pub async fn save_database(
        &self,
        name: &str,
        record: &serde_json::Value,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let _inner =
            json_value_to_inner(&rec, interner).map_err(|e| DbError::Codec(e.to_string()))?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_DATABASES),
            key: json!({"name": name}).into(),
            value: rec.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        Ok(())
    }

    /// Remove database metadata.
    pub async fn remove_database(&self, name: &str) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_DATABASES),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        Ok(())
    }

    /// Load all database records.
    pub async fn load_databases(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_DATABASES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    // ========================================================================
    // Repository metadata
    // ========================================================================

    /// Save repository metadata. Injects `owner`/`group`/`mode` from `meta`.
    pub async fn save_repository(
        &self,
        db_name: &str,
        repo_name: &str,
        engine: &str,
        path: Option<&str>,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut record = json!({
            "db_name": db_name,
            "repo_name": repo_name,
            "engine": engine,
            "path": path,
        });
        meta.inject_into(&mut record);
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_REPOSITORIES),
            key: json!({"db_name": db_name, "repo_name": repo_name}).into(),
            value: record.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        // DDL must be durable immediately: flush the MemBuffer-wrapped
        // store so a crash right after the admin op can't lose (or, for
        // removes, resurrect) the catalogue entry. DDL is rare → the
        // fsync cost is irrelevant.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Remove repository metadata.
    pub async fn remove_repository(&self, db_name: &str, repo_name: &str) -> DbResult<()> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_REPOSITORIES),
            where_clause: crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                ],
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load all repository records.
    pub async fn load_repositories(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_REPOSITORIES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    // ========================================================================
    // Table catalogue (per-repo table list — I.2)
    // ========================================================================

    /// Persist one table's catalogue entry. Keyed by
    /// `(db_name, repo_name, table_name)` so re-saving the same table is an
    /// idempotent upsert. `enable_indexes` is the only other field of
    /// `TableConfig`, so the record carries enough to faithfully re-create
    /// the table on open.
    pub async fn save_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
        enable_indexes: bool,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut record = json!({
            "db_name": db_name,
            "repo_name": repo_name,
            "table_name": table_name,
            "enable_indexes": enable_indexes,
        });
        meta.inject_into(&mut record);
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: json!({
                "db_name": db_name,
                "repo_name": repo_name,
                "table_name": table_name,
            })
            .into(),
            value: record.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Remove one table's catalogue entry.
    pub async fn remove_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<()> {
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_TABLES),
            where_clause: crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["table_name".to_string()],
                        value: crate::query::filter::FilterValue::String(table_name.to_string()),
                    },
                ],
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load every persisted table-catalogue record (across all repos). The
    /// caller filters by `db_name` / `repo_name`.
    pub async fn load_tables(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_TABLES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_TABLES);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    // ========================================================================
    // Settings
    // ========================================================================

    /// Save a setting.
    pub async fn save_setting(&self, key: &str, value: &serde_json::Value) -> DbResult<()> {
        let table = self.table(TABLE_SETTINGS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_SETTINGS),
            key: json!({"key": key}).into(),
            value: json!({"key": key, "value": value}).into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        Ok(())
    }

    /// Load a setting.
    pub async fn load_setting(&self, key: &str) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_SETTINGS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_SETTINGS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["key".to_string()],
                value: crate::query::filter::FilterValue::String(key.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .and_then(|r| r.get_value_owned("value").map(serde_json::Value::from)))
    }

    // ========================================================================
    // Users & Roles (tables ready, API to be implemented)
    // ========================================================================

    /// Get the users table manager.
    pub async fn users_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_USERS).await
    }

    /// Get the roles table manager.
    pub async fn roles_table(&self) -> DbResult<TableManager> {
        self.table(TABLE_ROLES).await
    }

    // ========================================================================
    // Function catalogue (slice 4)
    // ========================================================================

    /// Persist a function catalogue entry. Upsert keyed by `name`.
    /// Injects `owner`/`group`/`mode` from `meta` into the record.
    pub async fn save_function(
        &self,
        name: &str,
        record: &serde_json::Value,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTIONS),
            key: json!({"name": name}).into(),
            value: rec.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Remove a function catalogue entry by name.
    pub async fn remove_function(&self, name: &str) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_FUNCTIONS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load every persisted user record (including `password_hash` —
    /// callers that surface these must strip secret fields themselves).
    pub async fn load_users(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_USERS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_USERS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    /// Load every persisted function catalogue record.
    pub async fn load_functions(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTIONS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    /// Load a single function catalogue record by name.
    pub async fn load_function(&self, name: &str) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTIONS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    // ========================================================================
    // Groups store (P3 metadata plates)
    // ========================================================================

    /// Persist a group record. `group_id` is allocated by the caller
    /// (see [`ShamirDb::create_group`]).
    pub async fn save_group(&self, group_id: u64, name: &str, members: &[u64]) -> DbResult<()> {
        let record = json!({
            "group_id": group_id,
            "name": name,
            "members": members,
        });
        let table = self.table(TABLE_GROUPS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_GROUPS),
            key: json!({"group_id": group_id}).into(),
            value: record.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load all group records.
    pub async fn load_groups(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_GROUPS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_GROUPS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    /// Load a single group record by id.
    pub async fn load_group(&self, group_id: u64) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_GROUPS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_GROUPS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["group_id".to_string()],
                value: crate::query::filter::FilterValue::Int(group_id as i64),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Add a user to a group. Reads the group, appends the user, and
    /// re-persists. Returns `Ok(())` even if the user was already a member.
    pub async fn add_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        let rec = self.load_group(group_id).await?;
        let mut members: Vec<u64> = rec
            .as_ref()
            .and_then(|r| r["members"].as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();
        if !members.contains(&user_id) {
            members.push(user_id);
        }
        let name = rec
            .as_ref()
            .and_then(|r| r["name"].as_str())
            .unwrap_or_default();
        self.save_group(group_id, name, &members).await
    }

    /// Remove a user from a group. Returns `Ok(())` even if the user was
    /// not a member.
    pub async fn remove_group_member(&self, group_id: u64, user_id: u64) -> DbResult<()> {
        let rec = self.load_group(group_id).await?;
        let mut members: Vec<u64> = rec
            .as_ref()
            .and_then(|r| r["members"].as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default();
        members.retain(|&m| m != user_id);
        let name = rec
            .as_ref()
            .and_then(|r| r["name"].as_str())
            .unwrap_or_default();
        self.save_group(group_id, name, &members).await
    }

    /// Remove a group record by id.
    pub async fn remove_group(&self, group_id: u64) -> DbResult<()> {
        let table = self.table(TABLE_GROUPS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_GROUPS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["group_id".to_string()],
                value: crate::query::filter::FilterValue::Int(group_id as i64),
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        table.data_store().flush().await?;
        Ok(())
    }

    // ========================================================================
    // Catalogue lookups for resource_meta resolver
    // ========================================================================

    /// Load a single database record by name.
    pub async fn load_database(&self, name: &str) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_DATABASES).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Load a single repository record by (db_name, repo_name).
    pub async fn load_repository(
        &self,
        db_name: &str,
        repo_name: &str,
    ) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_REPOSITORIES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_REPOSITORIES).filter(
            crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                ],
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Load a single table catalogue record by (db, repo, table).
    pub async fn load_table_record(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_TABLES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_TABLES).filter(
            crate::query::filter::Filter::And {
                filters: vec![
                    crate::query::filter::Filter::Eq {
                        field: vec!["db_name".to_string()],
                        value: crate::query::filter::FilterValue::String(db_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["repo_name".to_string()],
                        value: crate::query::filter::FilterValue::String(repo_name.to_string()),
                    },
                    crate::query::filter::Filter::Eq {
                        field: vec!["table_name".to_string()],
                        value: crate::query::filter::FilterValue::String(table_name.to_string()),
                    },
                ],
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Persist a replacement database record (for `set_resource_meta`).
    pub async fn save_database_meta(&self, name: &str, record: &serde_json::Value) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let _inner =
            json_value_to_inner(record, interner).map_err(|e| DbError::Codec(e.to_string()))?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_DATABASES),
            key: json!({"name": name}).into(),
            value: record.clone().into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }

    /// Persist a replacement repository record (for `set_resource_meta`).
    pub async fn save_repository_meta(&self, record: &serde_json::Value) -> DbResult<()> {
        let db_name = record["db_name"].as_str().unwrap_or_default();
        let repo_name = record["repo_name"].as_str().unwrap_or_default();
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_REPOSITORIES),
            key: json!({"db_name": db_name, "repo_name": repo_name}).into(),
            value: record.clone().into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }

    /// Persist a replacement table catalogue record (for `set_resource_meta`).
    pub async fn save_table_meta(&self, record: &serde_json::Value) -> DbResult<()> {
        let db_name = record["db_name"].as_str().unwrap_or_default();
        let repo_name = record["repo_name"].as_str().unwrap_or_default();
        let table_name = record["table_name"].as_str().unwrap_or_default();
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: json!({
                "db_name": db_name,
                "repo_name": repo_name,
                "table_name": table_name,
            })
            .into(),
            value: record.clone().into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }

    // ========================================================================
    // Validator catalogue (S1)
    // ========================================================================

    /// Persist a validator catalogue entry. Upsert keyed by `name`.
    /// Injects `owner`/`group`/`mode` from `meta` into the record.
    pub async fn save_validator(
        &self,
        name: &str,
        record: &serde_json::Value,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_VALIDATORS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_VALIDATORS),
            key: json!({"name": name}).into(),
            value: rec.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Remove a validator catalogue entry by name.
    pub async fn remove_validator(&self, name: &str) -> DbResult<()> {
        let table = self.table(TABLE_VALIDATORS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_VALIDATORS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load every persisted validator catalogue record.
    pub async fn load_validators(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_VALIDATORS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_VALIDATORS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    /// Load a single validator catalogue record by name.
    pub async fn load_validator(&self, name: &str) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_VALIDATORS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_VALIDATORS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Persist a replacement function catalogue record (for `set_resource_meta`).
    pub async fn save_function_meta_record(
        &self,
        name: &str,
        record: &serde_json::Value,
    ) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTIONS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTIONS),
            key: json!({"name": name}).into(),
            value: record.clone().into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }

    // ========================================================================
    // Function folder catalogue (#118)
    // ========================================================================

    /// Persist a function folder catalogue entry. Upsert keyed by `path`
    /// (slash-joined, e.g. `"reports/daily"`). Injects `owner`/`group`/`mode`
    /// from `meta` into the record.
    pub async fn save_function_folder(
        &self,
        path_key: &str,
        record: &serde_json::Value,
        meta: &ResourceMeta,
    ) -> DbResult<()> {
        let mut rec = record.clone();
        meta.inject_into(&mut rec);
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            key: json!({"path": path_key}).into(),
            value: rec.into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Remove a function folder catalogue entry by path key.
    pub async fn remove_function_folder(&self, path_key: &str) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["path".to_string()],
                value: crate::query::filter::FilterValue::String(path_key.to_string()),
            },
        };
        self.delete_via_implicit_tx(&table, &op).await?;
        // Durable DDL — see save_repository.
        table.data_store().flush().await?;
        Ok(())
    }

    /// Load every persisted function folder catalogue record.
    pub async fn load_function_folders(&self) -> DbResult<Vec<serde_json::Value>> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTION_FOLDERS);
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .map(|r| serde_json::Value::from(r.as_value().into_owned()))
            .collect())
    }

    /// Load a single function folder catalogue record by path key.
    pub async fn load_function_folder(
        &self,
        path_key: &str,
    ) -> DbResult<Option<serde_json::Value>> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let query = crate::query::read::ReadQuery::new(TABLE_FUNCTION_FOLDERS).filter(
            crate::query::filter::Filter::Eq {
                field: vec!["path".to_string()],
                value: crate::query::filter::FilterValue::String(path_key.to_string()),
            },
        );
        let result = table.read(&query, &ctx).await?;
        Ok(result
            .records
            .into_iter()
            .next()
            .map(|r| serde_json::Value::from(r.as_value().into_owned())))
    }

    /// Persist a replacement function folder record (for `set_resource_meta`).
    pub async fn save_function_folder_meta(
        &self,
        path_key: &str,
        record: &serde_json::Value,
    ) -> DbResult<()> {
        let table = self.table(TABLE_FUNCTION_FOLDERS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_FUNCTION_FOLDERS),
            key: json!({"path": path_key}).into(),
            value: record.clone().into(),
        };
        self.set_via_implicit_tx(&table, &op).await?;
        table.interner().persist().await?;
        table.data_store().flush().await?;
        Ok(())
    }
}
