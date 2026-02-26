use crate::db::engine::db_instance::db_instance::DbInstance;
use dashmap::DashMap;
use std::sync::Arc;

/// Top-level manager for multiple database instances.
///
/// Hierarchy:
/// ```text
/// ShamirDb
///   └── DbInstance (database: "production", "test", etc.)
///       └── RepoInstance (repository: "users_db", "logs_db", etc.)
///           └── TableManager (table: "users", "sessions", etc.)
/// ```
#[derive(Clone)]
pub struct ShamirDb {
    dbs: Arc<DashMap<String, DbInstance>>,
}

impl Default for ShamirDb {
    fn default() -> Self {
        Self::new()
    }
}

impl ShamirDb {
    pub fn new() -> Self {
        Self {
            dbs: Arc::new(DashMap::new()),
        }
    }

    pub fn db_count(&self) -> usize {
        self.dbs.len()
    }

    pub fn has_db(&self, name: &str) -> bool {
        self.dbs.contains_key(name)
    }

    pub fn create_db(&self, name: &str) -> DbInstance {
        let db = DbInstance::new(vec![]);
        self.dbs.insert(name.to_string(), db.clone());
        db
    }

    pub fn get_db(&self, name: &str) -> Option<DbInstance> {
        self.dbs.get(name).map(|r| r.clone())
    }

    pub fn get_or_create_db(&self, name: &str) -> DbInstance {
        self.dbs
            .entry(name.to_string())
            .or_insert_with(|| DbInstance::new(vec![]))
            .clone()
    }

    pub fn list_dbs(&self) -> Vec<String> {
        self.dbs.iter().map(|r| r.key().clone()).collect()
    }

    pub fn remove_db(&self, name: &str) -> bool {
        self.dbs.remove(name).is_some()
    }
}
