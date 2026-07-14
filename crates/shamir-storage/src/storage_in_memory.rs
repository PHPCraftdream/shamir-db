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
        let key = RecordKey::from_slice(id.as_bytes());

        // TreeIndex::insert returns Err((k, v)) on duplicate key.
        match self.data.insert_sync(key.clone(), value) {
            Ok(()) => Ok(key),
            Err(_) => Err(DbError::KeyExists(format!("Key already exists: {:?}", key))),
        }
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // Optimistic single-traversal path: try insert first (1 B+ tree
        // walk). On collision, fall back to remove+insert — 2 traversals
        // only when the key already exists. Avoids always-2-traversal of
        // the prior remove+insert pattern.
        let existed = match self.data.insert_sync(key.clone(), value.clone()) {
            Ok(()) => false, // new key — done in one traversal
            Err((k, v)) => {
                // Key already exists (insert rejected): remove then re-insert.
                // scc::TreeIndex has no update-in-place API so 2 traversals
                // are unavoidable for the update case. Concurrent readers
                // may observe a brief absence between remove and insert;
                // InMemoryStore is a single-session in-memory backend where
                // this window is acceptable (no durability guarantee).
                self.data.remove_sync(&k);
                let _ = self.data.insert_sync(k, v);
                true
            }
        };
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.data
            .peek_with(&key, |_, v| v.clone())
            .ok_or_else(|| DbError::NotFound(format!("record not found: {:?}", key)))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.data.remove_sync(&key))
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Snapshot under an epoch guard; the stream then yields without
        // holding the guard. TreeIndex iter is already sorted.
        let entries: Vec<(RecordKey, Bytes)> = {
            let g = scc::Guard::new();
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

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // O(log N + K) via TreeIndex::range — seek to start, yield one
        // batch at a time. Each batch grabs a fresh guard, collects up to
        // `batch_size` entries starting from a resumption key, then drops
        // the guard before yielding. This keeps memory proportional to
        // batch_size and allows callers that stop early (lookup_first_k)
        // to avoid scanning the entire range.
        let data = Arc::clone(&self.data);
        Box::pin(stream! {
            let mut resume_key: Option<Bytes> = start_inclusive.clone();
            let mut first_batch = true;
            loop {
                let batch: Vec<(RecordKey, Bytes)> = {
                    let g = scc::Guard::new();
                    let iter: Box<dyn Iterator<Item = (&RecordKey, &Bytes)> + '_> =
                        match &resume_key {
                            // Query type is `RecordKey` (the tree's key type);
                            // convert the `Bytes` resume cursor into a
                            // byte-identical `RecordKey` lower bound.
                            Some(lo) => {
                                Box::new(data.range(RecordKey::from(lo.clone()).., &g))
                            }
                            None => Box::new(data.iter(&g)),
                        };
                    let mut collected = Vec::with_capacity(batch_size);
                    let mut skip_first = !first_batch;
                    for (k, v) in iter {
                        // After the first batch, resume_key points to the last
                        // key we already yielded — skip it to avoid duplicates.
                        if skip_first {
                            skip_first = false;
                            continue;
                        }
                        if let Some(ref hi) = end_inclusive {
                            if k.as_ref() > hi.as_ref() {
                                break;
                            }
                        }
                        collected.push((k.clone(), v.clone()));
                        if collected.len() == batch_size {
                            break;
                        }
                    }
                    collected
                };
                if batch.is_empty() {
                    break;
                }
                // Set resume key to last entry in this batch for next iteration.
                // Boundary conversion KeyBytes -> Bytes to feed the next range's
                // lower bound (resume_key is the Bytes-typed range cursor).
                resume_key = Some(Bytes::from(batch.last().unwrap().0.clone()));
                first_batch = false;
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
            let g = scc::Guard::new();
            self.data
                .range(RecordKey::from(prefix.clone()).., &g)
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
