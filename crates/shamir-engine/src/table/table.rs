//! Table implementation - InnerValue only (no interning!)

use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

/// Low-level table - InnerValue only (no interning/conversion)
///
/// This table operates directly on InnerValue (interned format).
/// Interning, indexing, and format conversion should be handled at higher level.
/// Table is just a data store - it doesn't know its name or manage counters.
pub struct Table {
    data_store: Arc<dyn Store>,
}

impl Clone for Table {
    fn clone(&self) -> Self {
        Self {
            data_store: Arc::clone(&self.data_store),
        }
    }
}

impl Table {
    /// Create a new table
    pub fn new(data_store: Arc<dyn Store>) -> Self {
        Self { data_store }
    }

    /// Borrow the underlying data store. Needed by the DDL layer
    /// to push hot-reload knobs (e.g. buffer config) all the way
    /// down through any wrapping layers.
    pub fn data_store(&self) -> &Arc<dyn Store> {
        &self.data_store
    }

    /// Insert an InnerValue, returns RecordId
    ///
    /// No interning or conversion - expects already-interned InnerValue
    /// Note: This does not update the record counter - that's managed by TableContext
    pub async fn insert(&self, value: &InnerValue) -> DbResult<RecordId> {
        // Serialize InnerValue
        let inner_bytes = value
            .to_bytes()
            .map_err(|e| DbError::Codec(e.to_string()))?;

        // Insert to data store - returns Bytes (16 random bytes)
        let key_bytes = self.data_store.insert(inner_bytes).await?;

        // Convert Bytes to RecordId
        let arr: [u8; 16] = key_bytes.as_ref().try_into().map_err(|_| {
            DbError::Internal("Failed to convert key bytes to RecordId".to_string())
        })?;
        Ok(RecordId(arr))
    }

    /// Batched read — fetch N records by id in one trip to the
    /// data store. Returns `Vec<Option<InnerValue>>` parallel to
    /// the input ids: `Some(value)` for hits, `None` for missing
    /// ids (stale index entries are a normal occurrence — caller
    /// filters them).
    ///
    /// On native-`get_many` backends this collapses N×spawn_blocking
    /// + N transaction setups into one — the main win for indexed
    ///   read paths.
    pub async fn get_many(&self, ids: &[RecordId]) -> DbResult<Vec<Option<InnerValue>>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<bytes::Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
        let raw = self.data_store.get_many(keys).await?;
        let mut out = Vec::with_capacity(raw.len());
        for bytes_opt in raw {
            match bytes_opt {
                Some(b) => {
                    let v = InnerValue::from_bytes(b).map_err(|e| {
                        DbError::Codec(format!("Failed to deserialize InnerValue: {}", e))
                    })?;
                    out.push(Some(v));
                }
                None => out.push(None),
            }
        }
        Ok(out)
    }

    /// Batched insert — serialises N values and dispatches to the
    /// backend's `Store::insert_many`. When the backend overrides
    /// that (nebari / persy / redb / cached), all writes commit in
    /// one transaction = one fsync. Default impl on other backends
    /// falls through to N sequential inserts (same cost as the
    /// per-record path).
    pub async fn insert_many(&self, values: &[InnerValue]) -> DbResult<Vec<RecordId>> {
        let value_bytes: Vec<bytes::Bytes> = values
            .iter()
            .map(|v| v.to_bytes().map_err(|e| DbError::Codec(e.to_string())))
            .collect::<DbResult<Vec<_>>>()?;
        let keys = self.data_store.insert_many(value_bytes).await?;
        keys.into_iter()
            .map(|k| {
                let arr: [u8; 16] = k.as_ref().try_into().map_err(|_| {
                    DbError::Internal("Failed to convert key bytes to RecordId".to_string())
                })?;
                Ok(RecordId(arr))
            })
            .collect()
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
        let inner_bytes = value
            .to_bytes()
            .map_err(|e| DbError::Codec(e.to_string()))?;
        self.data_store.set(key_bytes, inner_bytes).await?;
        Ok(true)
    }

    /// Set a record by RecordId - creates if not exists, updates if exists
    ///
    /// No interning or conversion - expects already-interned InnerValue
    /// Returns true if created, false if updated
    /// Note: This does not update the record counter - that's managed by TableContext
    pub async fn set(&self, id: RecordId, value: &InnerValue) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();

        // Check if exists
        let exists = self.data_store.get(key_bytes.clone()).await.is_ok();

        // Serialize and set
        let inner_bytes = value
            .to_bytes()
            .map_err(|e| DbError::Codec(e.to_string()))?;
        self.data_store.set(key_bytes, inner_bytes).await?;

        Ok(!exists)
    }

    /// Delete a record by RecordId
    /// Note: This does not update the record counter - that's managed by TableContext
    pub async fn delete(&self, id: RecordId) -> DbResult<bool> {
        // Convert RecordId to Bytes
        let key_bytes = id.to_bytes();
        self.data_store.remove(key_bytes).await
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
                for decoded in Self::decode_raw_batch(batch_bytes) {
                    yield decoded;
                }
            }
        }
    }
}

impl Table {
    /// Decode one raw `(key, value)` batch (`Store::iter_stream` /
    /// `MvccStore::current_stream` shape) into `Vec<(RecordId, InnerValue)>`
    /// yields, preserving the EXACT skip/yield-empty semantics of the former
    /// inline `list_stream` decode: a bad key is skipped (after yielding its
    /// `Internal` error), a deserialisation failure yields its `Codec` error
    /// WITHOUT aborting the batch (later keys in the same batch are still
    /// decoded), and a batch that decodes to zero entries yields nothing.
    ///
    /// Shared by [`Table::list_stream`] (unattached tables) and the
    /// `MvccStore`-attached branch of [`TableManager::list_stream`] so there is
    /// ONE decode implementation (no copy-paste drift). Behaviour-identical to
    /// the inline decode it replaces.
    pub(super) fn decode_raw_batch(
        batch_bytes: Vec<(bytes::Bytes, bytes::Bytes)>,
    ) -> impl Iterator<Item = DbResult<Vec<(RecordId, InnerValue)>>> {
        let mut batch = Vec::new();
        let mut errors: Vec<DbError> = Vec::new();

        for (key_bytes, bytes) in batch_bytes {
            // Convert Bytes to RecordId
            let arr: [u8; 16] = match key_bytes.as_ref().try_into() {
                Ok(a) => a,
                Err(_) => {
                    errors.push(DbError::Internal(
                        "Failed to convert key bytes to RecordId".to_string(),
                    ));
                    continue;
                }
            };
            let id = RecordId(arr);

            match InnerValue::from_bytes(bytes) {
                Ok(inner_value) => {
                    batch.push((id, inner_value));
                }
                Err(e) => {
                    errors.push(DbError::Codec(format!(
                        "Failed to deserialize record: {}",
                        e
                    )));
                }
            }
        }

        // Preserve the EXACT yield order of the former inline stream! body:
        // each error yields as its own item, then a non-empty decoded batch
        // yields once at the end. An empty decoded batch yields nothing
        // (matching the former `if !batch.is_empty() { yield Ok(batch) }`).
        let mut out: Vec<DbResult<Vec<(RecordId, InnerValue)>>> =
            errors.into_iter().map(Err).collect();
        if !batch.is_empty() {
            out.push(Ok(batch));
        }
        out.into_iter()
    }
}
