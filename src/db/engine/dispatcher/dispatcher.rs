use super::super::table::TableManager;
use crate::db::engine::repo::{RepoConfig, RepoInstance};
use crate::db::{DbError, DbResult};
use crate::types::common::TMap;
use crate::types::value::InnerValue;
use std::collections::BTreeSet;

/// Manages multiple repositories
pub struct Dispatcher {
    repos: TMap<String, RepoInstance>,
}

impl Clone for Dispatcher {
    fn clone(&self) -> Self {
        let repos: TMap<String, RepoInstance> = self
            .repos
            .iter()
            .map(|(k, v): (&String, &RepoInstance)| (k.clone(), v.clone()))
            .collect();
        Self { repos }
    }
}

impl Dispatcher {
    pub fn new(repos: Vec<RepoConfig>) -> Self {
        let instances: TMap<String, RepoInstance> = repos
            .into_iter()
            .map(|config| {
                let name = config.name.clone();
                let instance = RepoInstance::new(config.repo, config.tables);
                (name, instance)
            })
            .collect();

        Self { repos: instances }
    }

    /// Add a new repository
    pub fn add_repo(&mut self, config: RepoConfig) {
        let instance = RepoInstance::new(config.repo, config.tables);
        self.repos.insert(config.name, instance);
    }

    /// Get a table from a specific repository
    pub async fn get_table(&self, repo_name: &str, table_name: &str) -> DbResult<TableManager> {
        let repo_manager = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;

        repo_manager.get_table(table_name).await
    }

    /// Get a repository manager instance
    pub fn get_repo(&self, repo_name: &str) -> DbResult<&RepoInstance> {
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
            .map(|repo: &RepoInstance| repo.has_table(table_name))
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
            .map(|repo: &RepoInstance| repo.table_count())
            .sum()
    }

    // ============================================================================
    // Index Management API (routing to RepoInstance)
    // ============================================================================

    /// Create a regular index on a table.
    pub async fn create_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let repo = self.get_repo(repo_name)?;
        repo.create_index(table_name, index_name, paths).await
    }

    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let repo = self.get_repo(repo_name)?;
        repo.create_unique_index(table_name, index_name, paths).await
    }

    /// Drop a regular index from a table.
    pub async fn drop_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self.get_repo(repo_name)?;
        repo.drop_index(table_name, index_name).await
    }

    /// Drop a unique index from a table.
    pub async fn drop_unique_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self.get_repo(repo_name)?;
        repo.drop_unique_index(table_name, index_name).await
    }

    /// Check if a regular index exists.
    pub async fn index_exists(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self.get_repo(repo_name)?;
        repo.index_exists(table_name, index_name).await
    }

    /// Check if a unique index exists.
    pub async fn unique_index_exists(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self.get_repo(repo_name)?;
        repo.unique_index_exists(table_name, index_name).await
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<crate::types::record_id::RecordId>> {
        let repo = self.get_repo(repo_name)?;
        repo.lookup_by_index(table_name, index_name, values).await
    }
}
