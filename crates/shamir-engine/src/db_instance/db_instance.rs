use super::super::table::{TableConfig, TableManager};
use crate::repo::{RepoConfig, RepoInstance};
use dashmap::DashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Manages multiple repositories
#[derive(Clone)]
pub struct DbInstance {
    repos: Arc<DashMap<String, RepoInstance>>,
}

impl Default for DbInstance {
    fn default() -> Self {
        Self::new()
    }
}

impl DbInstance {
    pub fn new() -> Self {
        Self {
            repos: Arc::new(DashMap::new()),
        }
    }

    /// Creates a DbInstance with pre-configured repos (async for factory creation)
    pub async fn with_repos(configs: Vec<RepoConfig>) -> DbResult<Self> {
        let instances: DashMap<String, RepoInstance> = DashMap::new();
        for config in configs {
            let name = config.name.clone();
            let instance =
                RepoInstance::from_factory(name.clone(), config.factory, config.tables).await?;
            instances.insert(name, instance);
        }

        Ok(Self {
            repos: Arc::new(instances),
        })
    }

    /// Add a new repository asynchronously
    pub async fn add_repo(&self, config: RepoConfig) -> DbResult<()> {
        let name = config.name.clone();
        let instance =
            RepoInstance::from_factory(name.clone(), config.factory, config.tables).await?;
        self.repos.insert(name, instance);
        Ok(())
    }

    /// Get a table from a specific repository
    pub async fn get_table(&self, repo_name: &str, table_name: &str) -> DbResult<TableManager> {
        let repo_manager = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;

        repo_manager.get_table(table_name).await
    }

    /// List all repository names
    pub fn list_repos(&self) -> Vec<String> {
        self.repos.iter().map(|r| r.key().clone()).collect()
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
            .map(|repo| repo.has_table(table_name))
            .unwrap_or(false)
    }

    /// Get total number of repositories
    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }

    /// Get total number of tables across all repositories
    pub fn table_count(&self) -> usize {
        self.repos.iter().map(|r| r.table_count()).sum()
    }

    /// Get a repository instance directly
    pub fn get_repo(&self, repo_name: &str) -> Option<RepoInstance> {
        self.repos.get(repo_name).map(|r| r.clone())
    }

    /// Remove a repository from the instance
    pub async fn remove_repo(&self, repo_name: &str) -> bool {
        self.repos.remove(repo_name).is_some()
    }

    // ============================================================================
    // Table Management
    // ============================================================================

    /// Create a table in a repository.
    pub fn create_table(&self, repo_name: &str, table_name: &str) -> DbResult<()> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.add_table(TableConfig::new(table_name));
        Ok(())
    }

    /// Drop a table from a repository.
    pub fn drop_table(&self, repo_name: &str, table_name: &str) -> DbResult<bool> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        Ok(repo.remove_table(table_name))
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
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
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
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.create_unique_index(table_name, index_name, paths)
            .await
    }

    /// Drop a regular index from a table.
    pub async fn drop_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.drop_index(table_name, index_name).await
    }

    /// Drop a unique index from a table.
    pub async fn drop_unique_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.drop_unique_index(table_name, index_name).await
    }

    /// Check if a regular index exists.
    pub async fn index_exists(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.index_exists(table_name, index_name).await
    }

    /// Check if a unique index exists.
    pub async fn unique_index_exists(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
    ) -> DbResult<bool> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.unique_index_exists(table_name, index_name).await
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        repo_name: &str,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<shamir_types::types::record_id::RecordId>> {
        let repo = self
            .repos
            .get(repo_name)
            .ok_or_else(|| DbError::NotFound(format!("Repository '{}' not found", repo_name)))?;
        repo.lookup_by_index(table_name, index_name, values).await
    }
}
