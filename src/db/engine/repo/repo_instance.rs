use super::super::index::index_manager::IndexManager;
use super::super::table::interner_manager::InternerManager;
use super::super::table::record_counter::RecordCounter;
use super::super::table::table::Table;
use super::super::table::{TableConfig, TableManager};
use super::repo_types::BoxRepo;
use crate::db::storage::types::Repo;
use crate::db::{DbError, DbResult};
use crate::types::common::{new_dash_map_wc, TDashMap, TMap};
use crate::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Manages a single repository and its tables
pub struct RepoInstance {
    repo: BoxRepo,
    configs: Arc<TMap<String, TableConfig>>,
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
        let configs_map: TMap<String, TableConfig> = configs
            .into_iter()
            .map(|cfg| (cfg.name.clone(), cfg))
            .collect();

        let tables: TDashMap<String, OnceCell<TableManager>> = new_dash_map_wc(100);

        Self {
            repo,
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
        }
    }

    pub async fn get_table(&self, table_name: &str) -> DbResult<TableManager> {
        if !self.configs.contains_key(table_name) {
            return Err(DbError::NotFound(format!(
                "Table '{}' is not configured in this repository",
                table_name
            )));
        }

        let cell = self
            .tables
            .entry(table_name.to_string())
            .or_insert_with(OnceCell::new);

        cell.get_or_try_init(|| async move { self.create_table_context(table_name).await })
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

        let data_store: Arc<dyn crate::db::storage::types::Store> = data_store;
        let info_store: Arc<dyn crate::db::storage::types::Store> = info_store;

        let interner_manager = InternerManager::new(Arc::clone(&info_store));
        let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));

        let index_manager =
            IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store)).await?;

        let table = Table::new(Arc::clone(&data_store));

        Ok(TableManager::new(
            table_name.to_string(),
            table,
            interner_manager,
            counter,
            index_manager,
        ))
    }

    pub fn list_table_names(&self) -> Vec<String> {
        self.configs.keys().cloned().collect()
    }

    pub fn has_table(&self, table_name: &str) -> bool {
        self.configs.contains_key(table_name)
    }

    pub fn table_count(&self) -> usize {
        self.configs.len()
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
    ) -> DbResult<BTreeSet<crate::types::record_id::RecordId>> {
        let table = self.get_table(table_name).await?;
        table.lookup_by_index(index_name, values).await
    }
}
