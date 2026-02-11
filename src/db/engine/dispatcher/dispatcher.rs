use super::super::table::TableContext;
use crate::db::engine::repo::{RepoConfig, RepoManagerInstance};
use crate::db::{DbError, DbResult};
use crate::types::common::TMap;

/// Manages multiple repositories
pub struct Dispatcher {
    repos: TMap<String, RepoManagerInstance>,
}

impl Clone for Dispatcher {
    fn clone(&self) -> Self {
        let repos: TMap<String, RepoManagerInstance> = self
            .repos
            .iter()
            .map(|(k, v): (&String, &RepoManagerInstance)| (k.clone(), v.clone()))
            .collect();
        Self { repos }
    }
}

impl Dispatcher {
    pub fn new(repos: Vec<RepoConfig>) -> Self {
        let instances: TMap<String, RepoManagerInstance> = repos
            .into_iter()
            .map(|config| {
                let name = config.name.clone();
                let instance = RepoManagerInstance::new(config.repo, config.tables);
                (name, instance)
            })
            .collect();

        Self { repos: instances }
    }

    /// Add a new repository
    pub fn add_repo(&mut self, config: RepoConfig) {
        let instance = RepoManagerInstance::new(config.repo, config.tables);
        self.repos.insert(config.name, instance);
    }

    /// Get a table from a specific repository
    pub async fn get_table(&self, repo_name: &str, table_name: &str) -> DbResult<TableContext> {
        let repo_manager = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;

        repo_manager.get_table(table_name).await
    }

    /// Get a repository manager instance
    pub fn get_repo(&self, repo_name: &str) -> DbResult<&RepoManagerInstance> {
        self.repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))
    }

    /// List all repository names
    pub fn list_repos(&self) -> Vec<String> {
        self.repos.keys().cloned().collect()
    }

    /// List all tables in a repository
    pub fn list_tables(&self, repo_name: &str) -> DbResult<Vec<String>> {
        let repo_manager = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;

        Ok(repo_manager.list_table_names())
    }

    /// Check if a repository exists
    pub fn has_repo(&self, repo_name: &str) -> bool {
        self.repos.contains_key(repo_name)
    }

    /// Check if a table exists in a repository
    pub fn has_table(&self, repo_name: &str, table_name: &str) -> bool {
        self.repos
            .get(repo_name)
            .map(|repo: &RepoManagerInstance| repo.has_table(table_name))
            .unwrap_or(false)
    }

    /// Get total number of repositories
    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    /// Get total number of tables across all repositories
    pub fn table_count(&self) -> usize {
        self.repos
            .values()
            .map(|repo: &RepoManagerInstance| repo.table_count())
            .sum()
    }
}
