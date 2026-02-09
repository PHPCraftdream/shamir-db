use super::table::Table;
use super::interner::InternerManager;
use crate::db::engine::index::table_index_manager::TableIndexManager;

pub struct TableContext<R: crate::db::storage::types::Repo> {
    table: Table<R>,
    interner: InternerManager,
    index_manager: TableIndexManager,
}

impl<R: crate::db::storage::types::Repo> Clone for TableContext<R> {
    fn clone(&self) -> Self {
        Self {
            table: self.table.clone(),
            interner: self.interner.clone(),
            index_manager: self.index_manager.clone(),
        }
    }
}

impl<R: crate::db::storage::types::Repo> TableContext<R> {
    pub fn new(table: Table<R>, interner: InternerManager, index_manager: TableIndexManager) -> Self {
        Self {
            table,
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
