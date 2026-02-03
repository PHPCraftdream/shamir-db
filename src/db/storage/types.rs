use crate::db::error::{DbError, DbResult};
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

/// An asynchronous, key-value store trait that operates on raw bytes.
///
/// This trait provides a low-level storage abstraction. It is the responsibility
/// of the caller to handle serialization and deserialization of the values.
/// The key is fixed to `RecordId`.
#[async_trait]
pub trait Store: Send + Sync {
    /// Inserts a new record with a generated `RecordId`.
    async fn insert(&self, value: Bytes) -> DbResult<RecordId>;

    /// Creates or updates a record for a given `RecordId`.
    /// Returns `true` if the record was created, `false` if it was updated.
    async fn set(&self, key: RecordId, value: Bytes) -> DbResult<bool>;

    /// Retrieves a record's raw bytes by its `RecordId`.
    async fn get(&self, key: RecordId) -> DbResult<Bytes>;

    /// Removes a record by its `RecordId`.
    async fn remove(&self, key: RecordId) -> DbResult<bool>;

    /// Returns all records in the store.
    /// Note: This can be an expensive operation on large stores.
    async fn iter(&self) -> DbResult<Vec<(RecordId, Bytes)>>;

    /// Returns an async stream that yields batches of records.
    /// Like PHP generators but with batching - yields Vec of size batch_size.
    /// Uses concurrent prefetching: while yielding current batch, fetches next batch in background.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// Stream that yields DbResult<Vec<(RecordId, Bytes)>> - each batch has exactly batch_size items
    /// (except possibly the last batch which may be smaller)
    fn iter_stream(&self, batch_size: usize) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordId, Bytes)>, DbError>> + Send>>;
}

/// A trait for a repository that can manage multiple `Store` instances.
#[async_trait]
pub trait Repo: Send + Sync {
    /// Retrieves a store by name. Creates it if it doesn't exist.
    async fn store_get<S>(&self, name: S) -> DbResult<Arc<dyn Store>>
    where
        S: AsRef<str> + Send;

    /// Deletes an entire store by name.
    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool>;

    /// Lists all stores in the repository.
    async fn stores_list(&self) -> DbResult<Vec<String>>;
}
