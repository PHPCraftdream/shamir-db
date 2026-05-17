use super::super::table::{TableConfig, TableManager};
use super::repo_types::{BoxRepo, BoxRepoFactory, RepoFactory};
use shamir_storage::types::{Repo, Store};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::common::{new_dash_map_wc, TDashMap};
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Manages a single repository and its tables
pub struct RepoInstance {
    repo: BoxRepo,
    configs: Arc<TDashMap<String, TableConfig>>,
    tables: Arc<TDashMap<String, OnceCell<TableManager>>>,
}

impl Clone for RepoInstance {
    fn clone(&self) -> Self {
        Self {
            repo: self.repo.clone(),
            configs: Arc::clone(&self.configs),
            tables: Arc::clone(&self.tables),
        }
    }
}

impl RepoInstance {
    pub fn new(repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        Self::from_box_repo(repo, configs)
    }

    fn from_box_repo(repo: BoxRepo, configs: Vec<TableConfig>) -> Self {
        let configs_map: TDashMap<String, TableConfig> = new_dash_map_wc(configs.len().max(16));
        for cfg in configs {
            configs_map.insert(cfg.name.clone(), cfg);
        }

        let tables: TDashMap<String, OnceCell<TableManager>> = new_dash_map_wc(100);

        Self {
            repo,
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
        }
    }

    /// Creates a RepoInstance asynchronously from a factory.
    /// This is the preferred method as it properly handles blocking I/O.
    pub async fn from_factory(factory: BoxRepoFactory, configs: Vec<TableConfig>) -> DbResult<Self> {
        let repo = factory.create().await?;
        Ok(Self::from_box_repo(repo, configs))
    }

    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        let cell = self
            .tables
            .entry(table_name.to_string())
            .or_insert_with(OnceCell::new);

        // §B13: existence-check happens INSIDE the init closure, so it
        // is serialized with the actual context construction. Doing the
        // check up-front would race with concurrent `remove_table`
        // between our `configs.contains_key` and the `tables.entry`
        // install (two independent DashMaps). On a removed table the
        // init returns Err and `OnceCell::get_or_try_init` leaves the
        // cell empty so subsequent calls retry.
        cell.get_or_try_init(|| async move {
            if !self.configs.contains_key(table_name) {
                return Err(DbError::NotFound(format!(
                    "Table '{}' is not configured in this repository",
                    table_name
                )));
            }
            self.create_table_context(table_name).await
        })
        .await
        .cloned()
    }

    async fn create_table_context(&self, table_name: &str) -> DbResult<TableManager> {
        let data_store = self
            .repo
            .store_get(format!("__data__{}", table_name))
            .await?;
        let info_store = self
            .repo
            .store_get(format!("__info__{}", table_name))
            .await?;

        let data_store: Arc<dyn Store> = data_store;
        let info_store: Arc<dyn Store> = info_store;

        TableManager::create(table_name.to_string(), data_store, info_store).await
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.iter().map(|e| e.key().clone()).collect()
    }

    pub fn has_table(&self, table_name: &str) -> bool {
        self.configs.contains_key(table_name)
    }

    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Register a new table in the repository.
    /// The table is lazily created on first access via get_table().
    pub fn add_table(&self, config: TableConfig) {
        self.configs.insert(config.name.clone(), config);
    }

    /// Remove a table from the repository.
    /// Returns true if the table existed and was removed.
    pub fn remove_table(&self, table_name: &str) -> bool {
        let removed = self.configs.remove(table_name).is_some();
        if removed {
            self.tables.remove(table_name);
        }
        removed
    }

    // ============================================================================
    // Index Management API (proxy to TableManager)
    // ============================================================================

    /// Create a regular index on a table.
    pub async fn create_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_index(index_name, paths).await
    }

    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        let table = self.get_table(table_name).await?;
        table.create_unique_index(index_name, paths).await
    }

    /// Drop a regular index from a table.
    pub async fn drop_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_index(index_name).await
    }

    /// Drop a unique index from a table.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        table.drop_unique_index(index_name).await
    }

    /// Check if a regular index exists on a table.
    pub async fn index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.index_exists(index_name).await)
    }

    /// Check if a unique index exists on a table.
    pub async fn unique_index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        let table = self.get_table(table_name).await?;
        Ok(table.unique_index_exists(index_name).await)
    }

    /// Look up records by index value.
    pub async fn lookup_by_index(
        &self,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<shamir_types::types::record_id::RecordId>> {
        let table = self.get_table(table_name).await?;
        table.lookup_by_index(index_name, values).await
    }
}
