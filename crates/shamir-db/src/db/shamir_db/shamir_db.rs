use serde_json::json;

use crate::db::engine::db_instance::db_instance::DbInstance;
use crate::db::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::db::engine::table::{TableConfig, TableManager};
use crate::db::{DbError, DbResult};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::system_store::{SystemStore, SystemStoreConfig};

const SYSTEM_DB_NAME: &str = "__system__";

/// Top-level manager for multiple database instances.
///
/// Hierarchy:
/// ```text
/// ShamirDb
///   ├── SystemStore (persistent metadata: databases, repos, settings, users, roles)
///   │
///   ├── production (DbInstance)
///   │   └── main (RepoInstance)
///   │       └── users (TableManager)
///   │
///   └── analytics (DbInstance)
///       └── archive (RepoInstance)
///           └── logs (TableManager)
/// ```
#[derive(Clone)]
pub struct ShamirDb {
    dbs: Arc<DashMap<String, DbInstance>>,
    system_store: SystemStore,
}

impl ShamirDb {
    /// Initialize ShamirDb with a system store.
    ///
    /// # Arguments
    /// * `config` — system store config (InMemory for tests, Redb(path) for production)
    pub async fn init(config: SystemStoreConfig) -> DbResult<Self> {
        let system_store = SystemStore::init(config).await?;

        let dbs = Arc::new(DashMap::new());

        let shamir = Self { dbs, system_store };

        // Load existing databases from system store
        let db_records = shamir.system_store.load_databases().await?;
        for record in &db_records {
            if let Some(name) = record["name"].as_str() {
                if name != SYSTEM_DB_NAME {
                    shamir.dbs.insert(name.to_string(), DbInstance::new());
                }
            }
        }

        // Load existing repositories and register them
        let repo_records = shamir.system_store.load_repositories().await?;
        for record in &repo_records {
            let db_name = record["db_name"].as_str().unwrap_or_default();
            let repo_name = record["repo_name"].as_str().unwrap_or_default();
            let engine = record["engine"].as_str().unwrap_or("in_memory");
            let path = record["path"].as_str();

            if let Some(db) = shamir.dbs.get(db_name) {
                let factory = Self::factory_from_meta(engine, path);
                if let Some(factory) = factory {
                    // Load table configs for this repo (tables will be loaded lazily)
                    let config = RepoConfig::new(repo_name, factory);
                    let _ = db.add_repo(config).await;
                }
            }
        }

        Ok(shamir)
    }

    /// Initialize with in-memory system store (convenience for tests).
    pub async fn init_memory() -> DbResult<Self> {
        Self::init(SystemStoreConfig::InMemory).await
    }

    /// Get the system store.
    pub fn system_store(&self) -> &SystemStore {
        &self.system_store
    }

    fn factory_from_meta(engine: &str, path: Option<&str>) -> Option<BoxRepoFactory> {
        match engine {
            "in_memory" => Some(BoxRepoFactory::in_memory()),
            "redb" => path.map(|p| BoxRepoFactory::redb(p)),
            "sled" => path.map(|p| BoxRepoFactory::sled(p)),
            "fjall" => path.map(|p| BoxRepoFactory::fjall(p)),
            "nebari" => path.map(|p| BoxRepoFactory::nebari(p)),
            "persy" => path.map(|p| BoxRepoFactory::persy(p)),
            "canopy" => path.map(|p| BoxRepoFactory::canopy(p)),
            _ => None,
        }
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

        // Persist to system store
        let _ = self.system_store.save_database(name, &json!({
            "name": name,
            "created_at": created_at,
        })).await;

        db
    }

    pub fn get_db(&self, name: &str) -> Option<DbInstance> {
        self.dbs.get(name).map(|r| r.clone())
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
            let _ = self.system_store.remove_database(name).await;
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

        // Persist to system store
        let _ = self.system_store.save_repository(
            db_name,
            &repo_name,
            &storage_type,
            path.as_deref(),
        ).await;

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

    pub async fn remove_repo(&self, db_name: &str, repo_name: &str) -> bool {
        if let Some(db) = self.get_db(db_name) {
            let removed = db.remove_repo(repo_name).await;
            if removed {
                let _ = self.system_store.remove_repository(db_name, repo_name).await;
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
