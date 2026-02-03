use crate::db::error::DbResult;
use crate::types::record_id::RecordId;
use async_trait::async_trait;
use bytes::Bytes;
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
