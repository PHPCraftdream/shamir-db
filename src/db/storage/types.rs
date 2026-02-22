use crate::db::{DbError, DbResult};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
#[cfg(test)]
use futures::stream::StreamExt;
use std::pin::Pin;
use std::sync::Arc;

pub type RecordKey = Bytes;

type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// Collect all records from a stream into a single vector.
/// Used to convert iter_stream() results to a flat Vec.
///
/// **DEPRECATED & FOR TESTS ONLY**
///
/// WARNING: Only use in tests! Can consume all memory on large datasets.
#[cfg(test)]
#[deprecated(since = "0.1.0", note = "FOR TESTS ONLY.")]
pub async fn collect_stream(stream: RecordStream) -> DbResult<Vec<(RecordKey, Bytes)>> {
    let mut all_records = Vec::new();
    let mut stream = stream;
    while let Some(batch_result) = stream.next().await {
        let batch = batch_result?;
        all_records.extend(batch);
    }
    Ok(all_records)
}

/// An asynchronous, key-value store trait that operates on raw bytes.
///
/// This trait provides a low-level storage abstraction. It is the responsibility
/// of the caller to handle serialization and deserialization of the values.
/// The key is fixed to `RecordKey`.
#[async_trait]
pub trait Store: Send + Sync {
    /// Inserts a new record with a generated `RecordKey`.
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey>;

    /// Creates or updates a record for a given `RecordKey`.
    /// Returns `true` if the record was created, `false` if it was updated.
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool>;

    /// Retrieves a record's raw bytes by its `RecordKey`.
    async fn get(&self, key: RecordKey) -> DbResult<Bytes>;

    /// Removes a record by its `RecordKey`.
    async fn remove(&self, key: RecordKey) -> DbResult<bool>;

    /// Returns an async stream that yields batches of records.
    /// Like PHP generators but with batching - yields Vec of size batch_size.
    /// Uses concurrent prefetching: while yielding current batch, fetches next batch in background.
    ///
    /// # Arguments
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// Stream that yields DbResult<Vec<(RecordKey, Bytes)>> - each batch has exactly batch_size items
    /// (except possibly the last batch which may be smaller)
    fn iter_stream(&self, batch_size: usize) -> RecordStream;

    /// Returns all records with keys starting with the given prefix.
    ///
    /// This enables efficient retrieval of records with composite keys like "idx:field:value:record_id".
    ///
    /// # Arguments
    /// * `prefix` - The prefix to search for (e.g., b"idx:city:Moscow:")
    ///
    /// # Returns
    /// All (key, value) pairs where key starts with the prefix
    ///
    /// # Performance
    /// - For Sled: O(log n + k) using native `scan_prefix()`
    /// - For others: O(log n + k) using `range(prefix..)` + filtering
    /// where k is the number of matching records
    async fn scan_prefix(&self, prefix: Bytes) -> DbResult<Vec<(RecordKey, Bytes)>>;

    /// Returns an async stream that yields batches of records with keys starting with the given prefix.
    ///
    /// Like `iter_stream()` but filtered by prefix. More efficient than loading all results into memory.
    ///
    /// # Arguments
    /// * `prefix` - The prefix to search for
    /// * `batch_size` - Number of records per batch
    ///
    /// # Returns
    /// Stream that yields batches of matching records
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream;
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
