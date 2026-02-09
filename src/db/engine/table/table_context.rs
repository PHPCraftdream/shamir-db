use super::interner_manager::InternerManager;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::db::engine::index::table_index_manager::TableIndexManager;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use std::sync::Arc;

pub struct TableContext {
    name: String,
    table: Arc<Table>,
    interner: InternerManager,
    counter: Arc<RecordCounter>,
    index_manager: TableIndexManager,
}

impl Clone for TableContext {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            table: Arc::clone(&self.table),
            interner: self.interner.clone(),
            counter: Arc::clone(&self.counter),
            index_manager: self.index_manager.clone(),
        }
    }
}

impl TableContext {
    pub fn new(name: String, table: Table, interner: InternerManager, counter: Arc<RecordCounter>, index_manager: TableIndexManager) -> Self {
        Self {
            name,
            table: Arc::new(table),
            interner,
            counter,
            index_manager,
        }
    }

    pub fn table(&self) -> &Table {
        &self.table
    }

    pub fn interner(&self) -> &InternerManager {
        &self.interner
    }

    pub fn counter(&self) -> &Arc<RecordCounter> {
        &self.counter
    }

    pub fn index_manager(&self) -> &TableIndexManager {
        &self.index_manager
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Insert an InnerValue, returns RecordId (with counter update)
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        let id = self.table.insert(value).await?;
        self.counter.increment(1).await?;
        Ok(id)
    }

    /// Delete a record by RecordId (with counter update)
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        let removed = self.table.delete(id).await?;
        if removed {
            self.counter.increment(-1).await?;
        }
        Ok(removed)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists (with counter update)
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        let created = self.table.set(id, value).await?;
        if created {
            self.counter.increment(1).await?;
        }
        Ok(created)
    }

    /// Count records (uses stored counter for O(1) performance)
    pub async fn count(&self) -> DbResult<usize> {
        Ok(self.counter.get().await? as usize)
    }

    /// Stream records in batches, returning InnerValues
    ///
    /// This is memory-efficient for large tables as it doesn't load all records at once.
    /// Returns a stream that yields batches of records.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// A stream that yields batches of (RecordId, InnerValue) tuples
    pub fn list_stream(
        &self,
        batch_size: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> {
        self.table.list_stream(batch_size)
    }
}
