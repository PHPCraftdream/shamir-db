use super::interner_manager::InternerManager;
use super::record_counter::RecordCounter;
use super::table::Table;
use crate::db::engine::index::index_manager::IndexManager;
use crate::db::DbResult;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use std::sync::Arc;

pub struct TableManager {
    name: String,
    table: Arc<Table>,
    interner: InternerManager,
    counter: Arc<RecordCounter>,
    index_manager: IndexManager,
}

impl Clone for TableManager {
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

impl TableManager {
    pub fn new(
        name: String,
        table: Table,
        interner: InternerManager,
        counter: Arc<RecordCounter>,
        index_manager: IndexManager,
    ) -> Self {
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

    pub fn index_manager(&self) -> &IndexManager {
        &self.index_manager
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Insert an InnerValue, returns RecordId (with counter and index update)
    ///
    /// Validates unique indexes BEFORE insert, returns error if constraint violated.
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        // 1. Validate unique indexes BEFORE write
        self.index_manager.validate_unique_for_create(value).await?;

        // 2. Write to table
        let id = self.table.insert(value).await?;
        self.counter.increment(1).await?;

        // 3. Update indexes AFTER write
        self.index_manager.on_record_created(&id, value).await?;
        self.index_manager
            .on_record_created_unique(&id, value)
            .await?;

        Ok(id)
    }

    /// Delete a record by RecordId (with counter and index update)
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // Get old value before deletion for index cleanup
        let old_value = self.table.get(id).await.ok();
        let removed = self.table.delete(id).await?;
        if removed {
            self.counter.increment(-1).await?;
            if let Some(ref old) = old_value {
                self.index_manager.on_record_deleted(&id, old).await?;
                self.index_manager
                    .on_record_deleted_unique(&id, old)
                    .await?;
            }
        }
        Ok(removed)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists (with counter and index update)
    ///
    /// Validates unique indexes BEFORE write, returns error if constraint violated.
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        // Get old value before update for index maintenance
        let old_value = self.table.get(id).await.ok();

        // 1. Validate unique indexes BEFORE write
        if let Some(ref old) = old_value {
            self.index_manager
                .validate_unique_for_update(&id, old, value)
                .await?;
        } else {
            self.index_manager.validate_unique_for_create(value).await?;
        }

        // 2. Write to table
        let created = self.table.set(id, value).await?;

        // 3. Update indexes AFTER write
        if created {
            self.counter.increment(1).await?;
            self.index_manager.on_record_created(&id, value).await?;
            self.index_manager
                .on_record_created_unique(&id, value)
                .await?;
        } else if let Some(old) = old_value {
            self.index_manager
                .on_record_updated(&id, &old, value)
                .await?;
            self.index_manager
                .on_record_updated_unique(&id, &old, value)
                .await?;
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
