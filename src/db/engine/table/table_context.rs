use super::table::Table;
use super::interner_manager::InternerManager;
use crate::db::engine::index::table_index_manager::TableIndexManager;
use std::sync::Arc;
use crate::db::storage::types::Repo;

pub struct TableContext<R: Repo> {
    table: Arc<Table<R>>,
    interner: InternerManager,
    index_manager: TableIndexManager,
}

impl<R: Repo> Clone for TableContext<R> {
    fn clone(&self) -> Self {
        Self {
            table: Arc::clone(&self.table),
            interner: self.interner.clone(),
            index_manager: self.index_manager.clone(),
        }
    }
}

impl<R: Repo> TableContext<R> {
    pub fn new(table: Table<R>, interner: InternerManager, index_manager: TableIndexManager) -> Self {
        Self {
            table: Arc::new(table),
            interner,
            index_manager,
        }
    }

    pub fn table(&self) -> &Table<R> {
        &self.table
    }

    pub fn interner(&self) -> &InternerManager {
        &self.interner
    }

    pub fn index_manager(&self) -> &TableIndexManager {
        &self.index_manager
    }

    pub fn name(&self) -> &str {
        self.table.name()
    }
}
