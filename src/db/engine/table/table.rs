//! Table implementation - InnerValue only (no interning!)

use super::counter::RecordCounter;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::types::{Repo, Store};
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use std::sync::Arc;

/// Low-level table - InnerValue only (no interning/conversion)
///
/// This table operates directly on InnerValue (interned format).
/// Interning and format conversion should be handled at higher level.
pub struct Table<R: Repo> {
    repo: Arc<R>,
    table_name: String,
    data_store: Arc<dyn Store>,
    info_store: Arc<dyn Store>,
    counter: Arc<RecordCounter>,
}

impl<R: Repo> Clone for Table<R> {
    fn clone(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            table_name: self.table_name.clone(),
            data_store: Arc::clone(&self.data_store),
            info_store: Arc::clone(&self.info_store),
            counter: Arc::clone(&self.counter),
        }
    }
}

impl<R: Repo> Table<R> {
    /// Create a new table
    pub async fn new(repo: Arc<R>, table_name: String) -> DbResult<Self> {
        // Get or create stores
        let data_store = repo.store_get(format!("__data__{}", table_name)).await?;
        let info_store = repo.store_get(format!("__info__{}", table_name)).await?;

        let data_store: Arc<dyn Store> = Arc::from(data_store);
        let info_store: Arc<dyn Store> = Arc::from(info_store);

        Ok(Self {
            repo,
            table_name,
            data_store,
            info_store: info_store.clone(),
            counter: Arc::new(RecordCounter::new(Arc::clone(&info_store))),
        })
    }

    /// Insert an InnerValue, returns RecordId
    ///
    /// No interning or conversion - expects already-interned InnerValue
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        // Serialize InnerValue
        let inner_bytes = value.to_bytes();

        // Insert to data store - returns Bytes (16 random bytes)
        let key_bytes = self.data_store.insert(inner_bytes).await?;

        // Increment record count
        self.counter.increment(1).await?;

        // Convert Bytes to RecordId
        let arr: [u8; 16] = key_bytes.as_ref().try_into()
            .map_err(|_| DbError::Internal("Failed to convert key bytes to RecordId".to_string()))?;
        Ok(RecordId(arr))
    }

    /// Get an InnerValue by RecordId
    ///
    /// No conversion - returns InnerValue directly
    pub async fn get(&self, id: RecordId) -> DbResult<InnerValue> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Read from data store
        let bytes = self.data_store.get(key_bytes).await?;

        // Deserialize InnerValue
        InnerValue::from_bytes(bytes)
            .map_err(|e| DbError::Codec(format!("Failed to deserialize InnerValue: {}", e)))
    }

    /// Update a record by RecordId
    ///
    /// No interning or conversion - expects already-interned InnerValue
    pub async fn update(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Check if exists
        let exists = self.data_store.get(key_bytes.clone()).await.is_ok();
        if !exists {
            return Ok(false);
        }

        // Serialize and update
        let inner_bytes = value.to_bytes();
        self.data_store.set(key_bytes, inner_bytes).await?;
        Ok(true)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists
    ///
    /// No interning or conversion - expects already-interned InnerValue
    /// Returns true if created, false if updated
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Check if exists
        let exists = self.data_store.get(key_bytes.clone()).await.is_ok();

        // Serialize and set
        let inner_bytes = value.to_bytes();
        self.data_store.set(key_bytes, inner_bytes).await?;

        if !exists {
            // New record created - increment count
            self.counter.increment(1).await?;
        }

        Ok(!exists)
    }

    /// Delete a record by RecordId
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();
        let removed = self.data_store.remove(key_bytes).await?;

        if removed {
            // Decrement record count
            self.counter.increment(-1).await?;
        }

        Ok(removed)
    }

    /// List all records (returns InnerValues)
    ///
    /// No conversion - returns InnerValues directly
    pub async fn list(&self) -> DbResult<Vec<(RecordId, InnerValue)>> {
        let items = self.data_store.iter().await?;
        let mut result = Vec::new();

        for (key_bytes, bytes) in items {
            // Convert Bytes to RecordId
            let arr: [u8; 16] = key_bytes.as_ref().try_into()
                .map_err(|_| DbError::Internal("Failed to convert key bytes to RecordId".to_string()))?;
            let id = RecordId(arr);

            match InnerValue::from_bytes(bytes) {
                Ok(inner_value) => {
                    result.push((id, inner_value));
                }
                Err(e) => {
                    log::warn!("Failed to deserialize record: {}", e);
                }
            }
        }

        Ok(result)
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
    ) -> impl Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> {
        let table = self.clone();

        stream! {
            // Get stream from storage
            let mut storage_stream = table.data_store.iter_stream(batch_size);

            // Transform each batch
            while let Some(batch_result) = storage_stream.next().await {
                let batch_bytes = batch_result?;

                // Transform batch
                let mut batch = Vec::new();

                for (key_bytes, bytes) in batch_bytes {
                    // Convert Bytes to RecordId
                    let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                        Ok(a) => a,
                        Err(_) => {
                            yield Err(DbError::Internal("Failed to convert key bytes to RecordId".to_string()));
                            continue;
                        }
                    };
                    let id = RecordId(arr);

                    match InnerValue::from_bytes(bytes) {
                        Ok(inner_value) => {
                            batch.push((id, inner_value));
                        }
                        Err(e) => {
                            yield Err(DbError::Codec(format!("Failed to deserialize record: {}", e)));
                        }
                    }
                }

                if !batch.is_empty() {
                    yield Ok(batch);
                }
            }
        }
    }

    /// Get table name
    pub fn name(&self) -> &str {
        &self.table_name
    }
}
