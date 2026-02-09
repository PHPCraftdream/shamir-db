use super::super::table::{Table, TableConfig, TableContext};
use super::super::table::interner_manager::InternerManager;
use super::super::index::table_index_manager::TableIndexManager;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::types::Repo;
use tokio::sync::OnceCell;
use std::sync::Arc;

pub struct Dispatcher<R: Repo> {
    repo: Arc<R>,
    configs: Arc<std::collections::HashMap<String, TableConfig>>,
    tables: Arc<dashmap::DashMap<String, OnceCell<TableContext<R>>>>,
}

impl<R: Repo> Clone for Dispatcher<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            configs: Arc::clone(&self.configs),
            tables: Arc::clone(&self.tables),
        }
    }
}

impl<R: Repo> Dispatcher<R> {
    pub fn new(repo: Arc<R>, configs: Vec<TableConfig>) -> Self {
        let configs_map: std::collections::HashMap<String, TableConfig> = configs
            .into_iter()
            .map(|cfg| (cfg.name.clone(), cfg))
            .collect();

        let tables: dashmap::DashMap<String, OnceCell<TableContext<R>>> = dashmap::DashMap::new();

        Self {
            repo,
            configs: Arc::new(configs_map),
            tables: Arc::new(tables),
        }
    }

    pub async fn get_table(&self, table_name: &str) -> DbResult<TableContext<R>> {
        if !self.configs.contains_key(table_name) {
            return Err(DbError::NotFound(format!(
                "Table '{}' is not configured",
                table_name
            )));
        }

        let cell = self.tables.entry(table_name.to_string())
            .or_insert_with(|| OnceCell::new());

        cell.get_or_try_init(|| async move {
            self.create_table_context(table_name).await
        }).await.map(|ctx| ctx.clone())
    }

    async fn create_table_context(&self, table_name: &str) -> DbResult<TableContext<R>> {
        let data_store = self.repo.store_get(format!("__data__{}", table_name)).await?;
        let info_store = self.repo.store_get(format!("__info__{}", table_name)).await?;

        let data_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(data_store);
        let info_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(info_store);

        let interner_manager = InternerManager::new(Arc::clone(&info_store));

        let interner_cell = Arc::new(OnceCell::new());
        let index_manager = TableIndexManager::new(
            Arc::clone(&data_store),
            Arc::clone(&info_store),
            interner_cell,
        ).await?;

        let table = Table::new(Arc::clone(&self.repo), table_name.to_string()).await?;

        Ok(TableContext::new(table, interner_manager, index_manager))
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
}
