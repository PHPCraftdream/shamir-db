//! Persistent system store for ShamirDB metadata.
//!
//! Stores databases, repositories, settings, users, roles.
//! Uses a TableManager backed by any storage engine (redb for production,
//! in_memory for tests).

use serde_json::json;

use crate::codecs::interned::json_value_to_inner;
use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
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
            .add_table(TableConfig::new(TABLE_ROLES));

        db.add_repo(repo_config).await?;

        Ok(Self { db })
    }

    /// Get the table manager for a system table.
    async fn table(&self, name: &str) -> DbResult<TableManager> {
        self.db.get_table(SYSTEM_REPO, name).await
    }

    // ========================================================================
    // Database metadata
    // ========================================================================

    /// Save database metadata.
    pub async fn save_database(&self, name: &str, record: &serde_json::Value) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let _inner =
            json_value_to_inner(record, interner).map_err(|e| DbError::Codec(e.to_string()))?;
        // Use set with name-based key lookup
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_DATABASES),
            key: json!({"name": name}),
            value: record.clone(),
        };
        table.execute_set(&op).await?;
        table.interner().persist().await?;
        Ok(())
    }

    /// Remove database metadata.
    pub async fn remove_database(&self, name: &str) -> DbResult<()> {
        let table = self.table(TABLE_DATABASES).await?;
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let op = crate::query::write::DeleteOp {
            delete_from: crate::query::TableRef::new(TABLE_DATABASES),
            where_clause: crate::query::filter::Filter::Eq {
                field: vec!["name".to_string()],
                value: crate::query::filter::FilterValue::String(name.to_string()),
            },
        };
        table.execute_delete(&op, &ctx).await?;
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
        Ok(result.records)
    }

    // ========================================================================
    // Repository metadata
    // ========================================================================

    /// Save repository metadata.
    pub async fn save_repository(
        &self,
        db_name: &str,
        repo_name: &str,
        engine: &str,
        path: Option<&str>,
    ) -> DbResult<()> {
        let record = json!({
            "db_name": db_name,
            "repo_name": repo_name,
            "engine": engine,
            "path": path,
        });
        let table = self.table(TABLE_REPOSITORIES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_REPOSITORIES),
            key: json!({"db_name": db_name, "repo_name": repo_name}),
            value: record,
        };
        table.execute_set(&op).await?;
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
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
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
        table.execute_delete(&op, &ctx).await?;
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
        Ok(result.records)
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
    ) -> DbResult<()> {
        let record = json!({
            "db_name": db_name,
            "repo_name": repo_name,
            "table_name": table_name,
            "enable_indexes": enable_indexes,
        });
        let table = self.table(TABLE_TABLES).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_TABLES),
            key: json!({
                "db_name": db_name,
                "repo_name": repo_name,
                "table_name": table_name,
            }),
            value: record,
        };
        table.execute_set(&op).await?;
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
        let interner = table.interner().get().await?;
        let refs = crate::types::common::new_map();
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
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
        table.execute_delete(&op, &ctx).await?;
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
        Ok(result.records)
    }

    // ========================================================================
    // Settings
    // ========================================================================

    /// Save a setting.
    pub async fn save_setting(&self, key: &str, value: &serde_json::Value) -> DbResult<()> {
        let table = self.table(TABLE_SETTINGS).await?;
        let op = crate::query::write::SetOp {
            set: crate::query::TableRef::new(TABLE_SETTINGS),
            key: json!({"key": key}),
            value: json!({"key": key, "value": value}),
        };
        table.execute_set(&op).await?;
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
            .map(|r| r["value"].clone()))
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
}
