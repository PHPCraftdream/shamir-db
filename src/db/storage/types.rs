use crate::db::error::DbResult;
use crate::types::{record_id::RecordId, repo_record::RepoRecord, value::InnerValue};
use std::sync::Arc;
use async_trait::async_trait;

#[async_trait]
pub trait Store: Send + Sync {
    /// Inserts a new record. Fails if the key already exists.
    async fn insert(&self, value: &InnerValue) -> DbResult<RecordId>;

    /// Creates or updates a record.
    async fn set(&self, key: RecordId, value: &InnerValue) -> DbResult<bool>;

    /// Retrieves a record by its key.
    async fn get(&self, key: RecordId) -> DbResult<RepoRecord>;

    /// Removes a record by its key.
    async fn remove(&self, key: RecordId) -> DbResult<bool>;

    /// Returns all records in the table.
    /// Note: This can be an expensive operation on large tables.
    async fn iter(&self) -> DbResult<Vec<RepoRecord>>;
}

#[async_trait]
pub trait Repo: Send + Sync {
    /// Retrieves a store by name. Creates it if it doesn't exist.
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>>;

    /// Deletes the entire store.
    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool>;

    /// Lists all tables in the database.
    async fn stores_list(&self) -> DbResult<Vec<String>>;
}