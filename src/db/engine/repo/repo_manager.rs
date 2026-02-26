use super::repo_config::RepoConfig;
use super::repo_types::BoxRepo;
use crate::db::storage::storage_in_memory::InMemoryRepo;
use crate::db::{DbError, DbResult};
use crate::types::common::{new_map_wc, TMap};
use std::sync::Arc;

pub struct RepoManager {
    repos: TMap<String, RepoConfig>,
}

impl Default for RepoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RepoManager {
    pub fn new() -> Self {
        Self {
            repos: new_map_wc(100),
        }
    }

    pub fn add_repo(&mut self, config: RepoConfig) -> Option<RepoConfig> {
        self.repos.insert(config.name.clone(), config)
    }

    pub fn get_repo_config(&self, name: &str) -> DbResult<RepoConfig> {
        self.repos
            .get(name)
            .cloned()
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", name)))
    }

    pub fn remove_repo(&mut self, name: &str) -> DbResult<RepoConfig> {
        self.repos
            .swap_remove(name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", name)))
    }

    pub fn has_repo(&self, name: &str) -> bool {
        self.repos.contains_key(name)
    }

    pub fn list_repos(&self) -> Vec<String> {
        self.repos.keys().cloned().collect()
    }

    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    pub async fn get_or_create_default(&mut self) -> RepoConfig {
        if self.has_repo("default") {
            return self.get_repo_config("default").unwrap();
        }

        let config = RepoConfig::new("default", BoxRepo::InMemory(Arc::new(InMemoryRepo::new())));
        self.add_repo(config.clone());
        config
    }

    pub fn set_default(&mut self, config: RepoConfig) {
        self.add_repo(config);
    }

    pub fn get_default_config(&self) -> DbResult<RepoConfig> {
        self.get_repo_config("default")
    }
}
