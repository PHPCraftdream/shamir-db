use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::db::engine::table::{TableConfig, TableManager};
use crate::db::{DbError, DbResult};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const SYSTEM_DB_NAME: &str = "__system__";

/// Database metadata record
#[derive(Debug, Clone)]
pub struct DatabaseRecord {
    pub name: String,
    pub created_at: u64,
}

/// Repository metadata record
#[derive(Debug, Clone)]
pub struct RepositoryRecord {
    pub db_name: String,
    pub repo_name: String,
    pub storage_type: String,
    pub path: Option<String>,
}

/// Top-level manager for multiple database instances.
///
/// Hierarchy:
/// ```text
/// ShamirDb
///   ├── __system__ (DbInstance - metadata)
///   │   └── metadata (repo with databases/repositories tables)
///   │
///   └── production (DbInstance)
///       └── users_db (RepoInstance)
///           └── users (TableManager)
/// ```
#[derive(Clone)]
pub struct ShamirDb {
    dbs: Arc<DashMap<String, DbInstance>>,
    databases_metadata: Arc<DashMap<String, DatabaseRecord>>,
    repositories_metadata: Arc<DashMap<String, RepositoryRecord>>,
}

impl Default for ShamirDb {
    fn default() -> Self {
        Self::new()
    }
}

impl ShamirDb {
    pub fn new() -> Self {
        let dbs = Arc::new(DashMap::new());

        let system_db = DbInstance::new();
        dbs.insert(SYSTEM_DB_NAME.to_string(), system_db);

        Self {
            dbs,
            databases_metadata: Arc::new(DashMap::new()),
            repositories_metadata: Arc::new(DashMap::new()),
        }
    }

    pub async fn init(self) -> DbResult<Self> {
        let config = RepoConfig::new("metadata", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("databases"))
            .add_table(TableConfig::new("repositories"));

        self.dbs
            .get(SYSTEM_DB_NAME)
            .unwrap()
            .add_repo(config)
            .await?;

        Ok(self)
    }

    pub fn db_count(&self) -> usize {
        self.dbs.len()
    }

    pub fn has_db(&self, name: &str) -> bool {
        self.dbs.contains_key(name)
    }

    pub async fn create_db(&self, name: &str) -> DbInstance {
        let db = DbInstance::new();
        self.dbs.insert(name.to_string(), db.clone());

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.databases_metadata.insert(
            name.to_string(),
            DatabaseRecord {
                name: name.to_string(),
                created_at,
            },
        );

        db
    }

    pub fn get_db(&self, name: &str) -> Option<DbInstance> {
        self.dbs.get(name).map(|r| r.clone())
    }

    pub async fn get_or_create_db(&self, name: &str) -> DbInstance {
        self.dbs
            .entry(name.to_string())
            .or_insert_with(|| {
                let db = DbInstance::new();

                let created_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                self.databases_metadata.insert(
                    name.to_string(),
                    DatabaseRecord {
                        name: name.to_string(),
                        created_at,
                    },
                );

                db
            })
            .clone()
    }

    pub fn list_dbs(&self) -> Vec<String> {
        self.dbs.iter().map(|r| r.key().clone()).collect()
    }

    pub async fn remove_db(&self, name: &str) -> bool {
        if name == SYSTEM_DB_NAME {
            return false;
        }

        let removed = self.dbs.remove(name).is_some();

        if removed {
            self.databases_metadata.remove(name);

            self.repositories_metadata.retain(|_, r| r.db_name != name);
        }

        removed
    }

    pub async fn add_repo(&self, db_name: &str, config: RepoConfig) -> DbResult<()> {
        let db = self
            .dbs
            .get(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;

        let repo_name = config.name.clone();
        let storage_type = Self::extract_storage_type(&config.factory);
        let path = Self::extract_path(&config.factory);

        db.add_repo(config).await?;

        let key = format!("{}:{}", db_name, repo_name);
        self.repositories_metadata.insert(
            key,
            RepositoryRecord {
                db_name: db_name.to_string(),
                repo_name,
                storage_type,
                path,
            },
        );

        Ok(())
    }

    fn extract_storage_type(factory: &BoxRepoFactory) -> String {
        match factory {
            BoxRepoFactory::InMemory(_) => "in_memory",
            BoxRepoFactory::Sled(_) => "sled",
            BoxRepoFactory::Redb(_) => "redb",
            BoxRepoFactory::Fjall(_) => "fjall",
            BoxRepoFactory::Nebari(_) => "nebari",
            BoxRepoFactory::Persy(_) => "persy",
            BoxRepoFactory::Canopy(_) => "canopy",
        }
        .to_string()
    }

    fn extract_path(factory: &BoxRepoFactory) -> Option<String> {
        match factory {
            BoxRepoFactory::InMemory(_) => None,
            BoxRepoFactory::Sled(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::Redb(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::Fjall(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::Nebari(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::Persy(f) => Some(f.path.to_string_lossy().to_string()),
            BoxRepoFactory::Canopy(f) => Some(f.path.to_string_lossy().to_string()),
        }
    }

    pub fn list_databases_metadata(&self) -> Vec<String> {
        self.databases_metadata
            .iter()
            .filter(|r| r.key() != SYSTEM_DB_NAME)
            .map(|r| r.key().clone())
            .collect()
    }

    pub fn list_repositories_metadata(&self, db_name: &str) -> Vec<RepositoryRecord> {
        self.repositories_metadata
            .iter()
            .filter(|r| r.db_name == db_name)
            .map(|r| r.value().clone())
            .collect()
    }

    /// Remove a repository from a database with metadata cleanup
    pub async fn remove_repo(&self, db_name: &str, repo_name: &str) -> bool {
        if let Some(db) = self.get_db(db_name) {
            let removed = db.remove_repo(repo_name).await;
            if removed {
                let key = format!("{}:{}", db_name, repo_name);
                self.repositories_metadata.remove(&key);
            }
            removed
        } else {
            false
        }
    }

    /// Direct table access shortcut
    pub async fn get_table(
        &self,
        db_name: &str,
        repo_name: &str,
        table_name: &str,
    ) -> DbResult<TableManager> {
        let db = self
            .get_db(db_name)
            .ok_or_else(|| DbError::NotFound(format!("Database '{}' not found", db_name)))?;
        db.get_table(repo_name, table_name).await
    }
}
