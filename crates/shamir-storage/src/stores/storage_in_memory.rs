use crate::error::{DbError, DbResult};
use crate::types::{AtomicBatch, AtomicCommitOutcome, KvOp, RecordKey, Repo, Store};
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
    /// One-shot serialization of repo-wide atomic-batch commits; in-memory
    /// test/embedded backend, never a hot path (sanctioned std::sync::Mutex
    /// exception).
    batch_lock: Arc<std::sync::Mutex<()>>,
}

impl InMemoryRepo {
    pub fn new() -> Self {
        Self {
            stores: Arc::new(new_dash_map_wc(1024)),
            batch_lock: Arc::new(std::sync::Mutex::new(())),
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

    /// `true` — the in-memory backend has no durability layer, so a staged
    /// batch commit cannot fail after staging.
    fn supports_atomic_batch(&self) -> bool {
        true
    }

    async fn begin_atomic_batch(&self) -> DbResult<Box<dyn AtomicBatch>> {
        Ok(Box::new(InMemoryAtomicBatch {
            stores: Arc::clone(&self.stores),
            commit_lock: Arc::clone(&self.batch_lock),
            ops: Vec::new(),
        }))
    }
}

/// Pending repository-wide atomic batch for [`InMemoryRepo`]: ops are staged
/// as (store-name, op) pairs and applied under the repo-wide commit lock in
/// one pass. Commit cannot fail after staging on this backend (no durability
/// layer), so it always reports [`AtomicCommitOutcome::Committed`].
///
/// **Atomicity scope.** Visibility atomicity for concurrent READERS relies on
/// the engine's single-writer discipline — `scc::TreeIndex` has no multi-key
/// publish primitive, so a concurrent reader can observe ops one at a time.
/// The `commit_lock` only serializes batch-vs-batch (two `commit` calls never
/// interleave their ops); it does not block point reads/writes.
struct InMemoryAtomicBatch {
    stores: Arc<TDashMap<String, Arc<InMemoryStore>>>,
    commit_lock: Arc<std::sync::Mutex<()>>,
    ops: Vec<(String, KvOp)>,
}

#[async_trait]
impl AtomicBatch for InMemoryAtomicBatch {
    fn set(&mut self, store: &str, key: RecordKey, value: Bytes) -> DbResult<()> {
        if store.is_empty() {
            return Err(DbError::Validation(
                "atomic batch: store name must not be empty".into(),
            ));
        }
        self.ops.push((store.to_string(), KvOp::Set(key, value)));
        Ok(())
    }

    fn delete(&mut self, store: &str, key: RecordKey) -> DbResult<()> {
        if store.is_empty() {
            return Err(DbError::Validation(
                "atomic batch: store name must not be empty".into(),
            ));
        }
        self.ops.push((store.to_string(), KvOp::Remove(key)));
        Ok(())
    }

    /// Apply the staged ops. The std mutex is taken as a BLOCKING lock and
    /// the critical section contains NO `.await` — the whole apply loop is
    /// synchronous (TreeIndex `insert_sync`/`remove_sync`), so nothing is
    /// held across a suspension point.
    async fn commit(self: Box<Self>) -> DbResult<AtomicCommitOutcome> {
        // Staging validation already ran at `set`/`delete` time, so a commit
        // reaches this point only with well-formed ops (an Err during staging
        // leaves nothing applied — all-or-nothing on staging error).
        let _guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (name, op) in &self.ops {
            let store = self
                .stores
                .entry(name.clone())
                .or_insert_with(|| Arc::new(InMemoryStore::new()))
                .value()
                .clone();
            match op {
                KvOp::Set(k, v) => {
                    store.data.upsert_sync(k.clone(), v.clone());
                }
                KvOp::Remove(k) => {
                    store.data.remove_sync(k);
                }
            }
        }
        Ok(AtomicCommitOutcome::Committed)
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
/// may be observable to concurrent readers under heavy contention. Fjall
/// supports atomic batch writes, subject to its documented read visibility
/// limitations; the in-memory
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
        match self.data.insert_sync(key.clone(), value.clone()) {
            Ok(()) => Ok(true),
            Err(_) => {
                self.data.upsert_sync(key, value);
                Ok(false)
            }
        }
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.data
            .peek_with(&key, |_, v| v.clone())
            .ok_or_else(|| DbError::missing_key(&key))
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        Ok(self.data.remove_sync(&key))
    }

    /// `true` — raw in-memory backend (see `InMemoryAtomicBatch`).
    fn supports_atomic_batch(&self) -> bool {
        true
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = Arc::clone(&self.data);
        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut last_key: Option<RecordKey> = None;
            loop {
                let cursor = last_key.clone();
                let batch = {
                    let guard = scc::Guard::new();
                    let iter = match cursor {
                        Some(key) => data.range((std::ops::Bound::Excluded(key), std::ops::Bound::Unbounded), &guard),
                        None => data.range((std::ops::Bound::Unbounded, std::ops::Bound::Unbounded), &guard),
                    };
                    iter.take(batch_size).map(|(key, value)| (key.clone(), value.clone())).collect::<Vec<_>>()
                };
                if batch.is_empty() { break; }
                last_key = batch.last().map(|(key, _)| key.clone());
                let done = batch.len() < batch_size;
                yield Ok(batch);
                if done { break; }
            }
        })
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = Arc::clone(&self.data);
        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut first_batch = true;
            let mut resume_key: Option<RecordKey> = start_inclusive.clone().map(RecordKey::from);
            loop {
                let cursor = resume_key.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let guard = scc::Guard::new();
                    let iter = match (&cursor, first_batch) {
                        (Some(key), true) => data.range((std::ops::Bound::Included(key.clone()), std::ops::Bound::Unbounded), &guard),
                        (Some(key), false) => data.range((std::ops::Bound::Excluded(key.clone()), std::ops::Bound::Unbounded), &guard),
                        (None, _) => data.range((std::ops::Bound::Unbounded, std::ops::Bound::Unbounded), &guard),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    for (key, value) in iter {
                        if let Some(hi) = &end_inclusive { if key.as_ref() > hi.as_ref() { break; } }
                        items.push((key.clone(), value.clone()));
                        if items.len() == batch_size { break; }
                    }
                    items
                };
                if batch.is_empty() { break; }
                resume_key = batch.last().map(|(key, _)| key.clone());
                first_batch = false;
                let done = batch.len() < batch_size;
                yield Ok(batch);
                if done { break; }
            }
        })
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = Arc::clone(&self.data);
        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut first_batch = true;
            let mut resume_key: Option<RecordKey> = end_inclusive.clone().map(RecordKey::from);
            loop {
                let cursor = resume_key.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let guard = scc::Guard::new();
                    let iter = match (&cursor, first_batch) {
                        (Some(key), true) => data.range((std::ops::Bound::Unbounded, std::ops::Bound::Included(key.clone())), &guard).rev(),
                        (Some(key), false) => data.range((std::ops::Bound::Unbounded, std::ops::Bound::Excluded(key.clone())), &guard).rev(),
                        (None, _) => data.range((std::ops::Bound::Unbounded, std::ops::Bound::Unbounded), &guard).rev(),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    for (key, value) in iter {
                        if let Some(lo) = &start_inclusive { if key.as_ref() < lo.as_ref() { break; } }
                        items.push((key.clone(), value.clone()));
                        if items.len() == batch_size { break; }
                    }
                    items
                };
                if batch.is_empty() { break; }
                resume_key = batch.last().map(|(key, _)| key.clone());
                first_batch = false;
                let done = batch.len() < batch_size;
                yield Ok(batch);
                if done { break; }
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        let data = Arc::clone(&self.data);
        Box::pin(stream! {
            if batch_size == 0 { return; }
            let mut first_batch = true;
            let mut resume_key: Option<RecordKey> = None;
            loop {
                let cursor = resume_key.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let guard = scc::Guard::new();
                    let iter = match (&cursor, first_batch) {
                        (Some(key), false) => data.range((std::ops::Bound::Excluded(key.clone()), std::ops::Bound::Unbounded), &guard),
                        (Some(key), true) => data.range((std::ops::Bound::Included(key.clone()), std::ops::Bound::Unbounded), &guard),
                        (None, _) => data.range((std::ops::Bound::Included(RecordKey::from(prefix.clone())), std::ops::Bound::Unbounded), &guard),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    for (key, value) in iter {
                        if !key.starts_with(&prefix[..]) { break; }
                        items.push((key.clone(), value.clone()));
                        if items.len() == batch_size { break; }
                    }
                    items
                };
                if batch.is_empty() { break; }
                resume_key = batch.last().map(|(key, _)| key.clone());
                first_batch = false;
                let done = batch.len() < batch_size;
                yield Ok(batch);
                if done { break; }
            }
        })
    }
}
