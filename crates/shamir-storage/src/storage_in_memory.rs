use super::types::{RecordKey, Repo, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use scc::TreeIndex;
use shamir_types::types::common::{new_dash_map_wc, TDashMap};
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

/// In-memory key-value store backed by `scc::TreeIndex` — a lock-free
/// concurrent sorted B+ tree.
///
/// **Sorted by design.** `scan_prefix_stream` does an O(log N + matches)
/// range walk via `TreeIndex::range` from the prefix start. The previous
/// DashMap shape did an O(N) full-iter+filter scan, which composed with
/// `MvccStore::vacuum_key` to make non-tx commit throughput collapse to
/// O(N²) over long bench/test runs (~5 s per 100-row batch at 100k
/// accumulated entries).
///
/// **Lock-free.** Reads via `peek_with` and `iter`/`range` use epoch-
/// based reclamation; writes (`insert` / `remove`) take per-node locks
/// scoped to the touched B+ path. Concurrency model is competitive with
/// the previous DashMap.
///
/// `transact` inherits the default sequential semantics — partial state
/// may be observable to concurrent readers under heavy contention. Disk
/// backends (redb, sled, fjall, persy, nebari, canopy) provide
/// transactional atomicity for workloads that require it; the in-memory
/// backend is a fully-supported deployment target for embedded /
/// ephemeral / cache-tier use cases.
pub struct InMemoryStore {
    data: Arc<TreeIndex<RecordKey, Bytes>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(TreeIndex::new()),
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

        // TreeIndex::insert returns Err((k, v)) on duplicate key.
        match self.data.insert(key.clone(), value) {
            Ok(()) => Ok(key),
            Err(_) => Err(DbError::KeyExists(format!("Key already exists: {:?}", key))),
        }
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // Idempotent upsert: remove (capturing prior existence), then insert.
        // Two epoch-protected ops; a concurrent reader between them sees the
        // pre-existing value through `peek_with` (no torn state visible).
        let existed = self.data.remove(&key);
        let _ = self.data.insert(key, value);
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.data
            .peek_with(&key, |_, v| v.clone())
            .ok_or_else(|| DbError::NotFound(format!("record not found: {:?}", key)))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.data.remove(&key))
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Snapshot under an epoch guard; the stream then yields without
        // holding the guard. TreeIndex iter is already sorted.
        let entries: Vec<(RecordKey, Bytes)> = {
            let g = scc::ebr::Guard::new();
            self.data
                .iter(&g)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        Box::pin(stream! {
            let mut entries = entries;
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
        // O(log N + matches) via TreeIndex::range from the prefix start.
        let entries: Vec<(RecordKey, Bytes)> = {
            let g = scc::ebr::Guard::new();
            self.data
                .range(prefix.clone().., &g)
                .take_while(|(k, _)| k.starts_with(&prefix[..]))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };

        Box::pin(stream! {
            let mut entries = entries;
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
