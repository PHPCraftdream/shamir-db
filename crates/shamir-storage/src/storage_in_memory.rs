use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use shamir_types::types::common::{new_dash_map, new_dash_map_wc, TDashMap};
use shamir_types::types::record_id::RecordId;
use std::pin::Pin;
use std::sync::Arc;

// ============================================================================
// InMemoryRepo - manages in-memory stores
// ============================================================================

pub struct InMemoryRepo {
    stores: Arc<TDashMap<String, Arc<InMemoryStore>>>,
}

impl InMemoryRepo {
    pub fn new() -> Self {
        Self {
            stores: Arc::new(new_dash_map_wc(1024)),
        }
    }
}

impl Default for InMemoryRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Repo for InMemoryRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        let name = name.as_ref();

        // Use DashMap's entry API for lock-free read or insert
        let entry = self
            .stores
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(InMemoryStore::new()));

        Ok(entry.value().clone() as Arc<dyn Store>)
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, name: S) -> DbResult<bool> {
        let name = name.as_ref();
        Ok(self.stores.remove(name).is_some())
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        let names: Vec<String> = self.stores.iter().map(|kv| kv.key().clone()).collect();
        Ok(names)
    }
}

// ============================================================================
// InMemoryStore - individual in-memory store
// ============================================================================

/// In-memory key-value store backed by `DashMap`.
///
/// `InMemoryStore::transact` inherits the default sequential semantics
/// -- partial state may be observable to concurrent readers under heavy
/// contention. DashMap does not provide cross-key atomic batches
/// without wrapping the entire map in a global lock, which would
/// regress all single-key ops. Used primarily for testing; production
/// atomicity guarantees come from disk backends (redb, sled, fjall,
/// persy, nebari, canopy).
pub struct InMemoryStore {
    data: Arc<TDashMap<RecordKey, Bytes>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(new_dash_map()),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Store for InMemoryStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = RecordId::new();
        let key = RecordKey::copy_from_slice(id.as_bytes());

        // DashMap entry API - try_entry returns Option<Entry>
        // If key exists, returns None. If not, returns Some(entry)
        match self.data.try_entry(key.clone()) {
            Some(entry) => {
                use dashmap::mapref::entry::Entry;
                match entry {
                    Entry::Vacant(vacant) => {
                        vacant.insert(value);
                        Ok(key)
                    }
                    Entry::Occupied(_) => {
                        Err(DbError::KeyExists(format!("Key already exists: {:?}", key)))
                    }
                }
            }
            None => Err(DbError::KeyExists(format!("Key already exists: {:?}", key))),
        }
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // DashMap insert returns None if new, Some(old_value) if updated
        let existed = self.data.insert(key.clone(), value).is_some();
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.data
            .get(&key)
            .map(|ref_| ref_.value().clone())
            .ok_or_else(|| DbError::NotFound(format!("record not found: {:?}", key)))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.data.remove(&key).is_some())
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = self.data.clone();

        Box::pin(stream! {
            // Collect (key, value) pairs in one pass. The previous shape
            // built a `Vec<RecordKey>` and then re-looked up each value
            // via `data.get(&key)` inside the batch loop — one extra
            // hash + shard-lock per record. DashMap::iter already exposes
            // both, so the second lookup is pure waste.
            let mut entries: Vec<(RecordKey, Bytes)> = data
                .iter()
                .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
                .collect();

            // Sort for consistent ordering (by key).
            entries.sort_by(|a, b| a.0.cmp(&b.0));

            while !entries.is_empty() {
                let take = std::cmp::min(batch_size, entries.len());
                let batch: Vec<(RecordKey, Bytes)> = entries.drain(..take).collect();
                yield Ok(batch);
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = self.data.clone();

        Box::pin(stream! {
            let prefix_slice = prefix.to_vec();
            // Same shape as `iter_stream`: one pass collecting (key,
            // value), filtered by prefix. No second `data.get(&key)`
            // round-trip for the matched keys.
            let mut entries: Vec<(RecordKey, Bytes)> = data
                .iter()
                .filter(|ref_| ref_.key().starts_with(&prefix_slice[..]))
                .map(|ref_| (ref_.key().clone(), ref_.value().clone()))
                .collect();

            entries.sort_by(|a, b| a.0.cmp(&b.0));

            while !entries.is_empty() {
                let take = std::cmp::min(batch_size, entries.len());
                let batch: Vec<(RecordKey, Bytes)> = entries.drain(..take).collect();
                yield Ok(batch);
            }
        })
    }
}

// ============================================================================
// Tests
// ============================================================================
