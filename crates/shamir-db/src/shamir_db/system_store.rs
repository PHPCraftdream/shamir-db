//! Persistent system store for ShamirDB metadata.
//!
//! Stores databases, repositories, settings, users, roles.
//! Uses a TableManager backed by any storage engine (redb for production,
//! in_memory for tests).

use serde_json::json;

use crate::codecs::interned::json_value_to_inner;
use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::{TableConfig, TableManager};
use crate::engine::db_instance::db_instance::DbInstance;
use crate::{DbError, DbResult};

const SYSTEM_REPO: &str = "system";

/// System store tables
const TABLE_DATABASES: &str = "databases";
const TABLE_REPOSITORIES: &str = "repositories";
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
        let _inner = json_value_to_inner(record, interner)
            .map_err(|e| DbError::Codec(e.to_string()))?;
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
    pub async fn save_repository(&self, db_name: &str, repo_name: &str, engine: &str, path: Option<&str>) -> DbResult<()> {
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
        let query = crate::query::read::ReadQuery::new(TABLE_SETTINGS)
            .filter(crate::query::filter::Filter::Eq {
                field: vec!["key".to_string()],
                value: crate::query::filter::FilterValue::String(key.to_string()),
            });
        let result = table.read(&query, &ctx).await?;
        Ok(result.records.into_iter().next().map(|r| r["value"].clone()))
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
