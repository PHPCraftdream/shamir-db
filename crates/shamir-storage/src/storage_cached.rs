use super::types::{RecordKey, Store};
use crate::error::{DbError, DbResult};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use scc::TreeIndex;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ============================================================================
// WriteMode - write strategy for CachedStore
// ============================================================================

/// Write strategy for cache operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WriteMode {
    /// Write-through: wait for disk write before returning.
    /// Safer, slower. Use for critical data like indexes.
    Sync,

    /// Write-behind: write to cache immediately, disk write in background.
    /// Faster, but data may be lost on crash. Use for non-critical data.
    Async,
}

// ============================================================================
// CachedStore - in-memory full mirror of any Store
// ============================================================================

/// Full mirror cache that loads ALL data from inner store on creation.
///
/// ## Write Modes:
/// - `WriteMode::Sync`: write-through, waits for disk (safer for indexes)
/// - `WriteMode::Async`: write-behind, background writes (faster for data)
///
/// ## Behavior:
/// - Constructor: loads all data from inner into local cache
/// - Reads: from cache first, fallback to inner on miss (lazy load)
/// - Writes: depends on WriteMode (sync or async)
///
/// ## Cache structure:
/// `scc::TreeIndex` — lock-free sorted B+ tree. Gives O(log N + k) prefix
/// range scans (no collect+sort) and O(log N) point ops. Replaces the
/// previous `DashMap` shape that did O(N log N) full-collect+sort on every
/// `iter_stream` / `scan_prefix_stream` call.
pub struct CachedStore {
    inner: Arc<dyn Store>,
    cache: Arc<TreeIndex<RecordKey, Bytes>>,
    mode: WriteMode,
    pending_writes: Arc<AtomicUsize>,
    /// Tracks the number of entries in the cache.
    /// `TreeIndex` has no O(1) `len()`, so we maintain a counter ourselves.
    size: Arc<AtomicUsize>,
}

impl CachedStore {
    async fn new_with_mode(inner: Arc<dyn Store>, mode: WriteMode) -> DbResult<Self> {
        use futures::StreamExt;

        let cache = Arc::new(TreeIndex::new());
        let size = Arc::new(AtomicUsize::new(0));

        // Load ALL data from inner store into cache (streaming to avoid double allocation)
        let mut stream = inner.iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, value) in batch {
                if cache.insert(key, value).is_ok() {
                    size.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        Ok(Self {
            inner,
            cache,
            mode,
            pending_writes: Arc::new(AtomicUsize::new(0)),
            size,
        })
    }

    /// Create a new cached store with Sync write mode (safer, for indexes).
    /// Loads ALL data from inner store into cache.
    pub async fn new_sync(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Sync).await
    }

    /// Create a new cached store with Async write mode (faster, for data).
    /// Loads ALL data from inner store into cache.
    pub async fn new_async(inner: Arc<dyn Store>) -> DbResult<Self> {
        Self::new_with_mode(inner, WriteMode::Async).await
    }

    /// Get reference to the inner store.
    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Get reference to the underlying `TreeIndex` cache (for inspection/debugging).
    pub fn cache(&self) -> &Arc<TreeIndex<RecordKey, Bytes>> {
        &self.cache
    }

    /// Get write mode.
    pub fn mode(&self) -> WriteMode {
        self.mode
    }

    /// Get number of entries currently in cache.
    pub fn cache_size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// Get number of pending async writes (0 for Sync mode).
    pub fn pending_writes(&self) -> usize {
        self.pending_writes.load(Ordering::Relaxed)
    }

    /// Reload all data from inner store (re-sync cache).
    /// Useful if inner store was modified externally.
    pub async fn reload(&self) -> DbResult<()> {
        use futures::StreamExt;

        // Clear current cache and reset size counter
        self.cache.clear();
        self.size.store(0, Ordering::Relaxed);

        // Reload all data from inner (streaming)
        let mut stream = self
            .inner
            .iter_stream(shamir_tunables::store_defaults::FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            for (key, value) in batch {
                if self.cache.insert(key, value).is_ok() {
                    self.size.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        Ok(())
    }

    /// Flush all pending async writes (only for Async mode).
    /// For Sync mode, this is a no-op.
    pub async fn flush(&self) -> DbResult<()> {
        if matches!(self.mode, WriteMode::Sync) {
            return Ok(());
        }

        // Wait for pending writes to complete
        while self.pending_writes.load(Ordering::Relaxed) > 0 {
            tokio::task::yield_now().await;
        }

        Ok(())
    }

    /// Cache-internal upsert: remove old entry (updating size), then insert new.
    /// Returns `true` if this was a new key (didn't exist before).
    fn cache_upsert(&self, key: RecordKey, value: Bytes) -> bool {
        let existed = self.cache.remove(&key);
        if !existed {
            self.size.fetch_add(1, Ordering::Relaxed);
        }
        // insert always succeeds after remove
        let _ = self.cache.insert(key, value);
        !existed
    }
}

#[async_trait]
impl Store for CachedStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // Insert ALWAYS needs to wait for inner to get the correct key
        // Async mode only applies to set/remove, not insert
        let key = self.inner.insert(value.clone()).await?;

        // Cache the value immediately (new key, so always increments size)
        let _ = self.cache.insert(key.clone(), value);
        self.size.fetch_add(1, Ordering::Relaxed);
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        match self.mode {
            WriteMode::Sync => {
                // Write to both inner store and cache synchronously
                let created = self.inner.set(key.clone(), value.clone()).await?;
                self.cache_upsert(key, value);
                Ok(created)
            }
            WriteMode::Async => {
                // Write to cache immediately
                let created = self.cache_upsert(key.clone(), value.clone());

                // Background write to inner store
                let inner = self.inner.clone();
                let pending = self.pending_writes.clone();

                pending.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    // §B8: WriteMode::Async is fire-and-forget by design,
                    // but a swallowed `Err` silently loses durability.
                    // Log so an operator gets a signal under sustained
                    // backing-store failure; the cache already holds the
                    // value so subsequent reads still succeed.
                    if let Err(e) = inner.set(key, value).await {
                        log::error!("storage_cached async write to backing store failed: {}", e);
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                });

                Ok(created)
            }
        }
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Try cache first
        if let Some(v) = self.cache.peek_with(&key, |_, v| v.clone()) {
            return Ok(v);
        }

        // Cache miss - load from inner store and cache it
        // This handles cases where inner was modified externally
        let value = self.inner.get(key.clone()).await?;

        // Store in cache for future access (new entry → increment size)
        if self.cache.insert(key, value.clone()).is_ok() {
            self.size.fetch_add(1, Ordering::Relaxed);
        }

        Ok(value)
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let existed = self.cache.remove(&key);
        if existed {
            self.size.fetch_sub(1, Ordering::Relaxed);
        }

        match self.mode {
            WriteMode::Sync => self.inner.remove(key).await,
            WriteMode::Async => {
                // Background delete
                let inner = self.inner.clone();
                let pending = self.pending_writes.clone();

                pending.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    // §B8: WriteMode::Async is fire-and-forget by design,
                    // but a swallowed `Err` silently loses durability.
                    // Log so an operator gets a signal under sustained
                    // backing-store failure.
                    if let Err(e) = inner.remove(key).await {
                        log::error!(
                            "storage_cached async remove from backing store failed: {}",
                            e
                        );
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                });

                Ok(existed)
            }
        }
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Snapshot under an epoch guard; TreeIndex iter is already sorted —
        // no collect+sort needed (was O(N log N) with the previous DashMap).
        let entries: Vec<(RecordKey, Bytes)> = {
            let g = scc::ebr::Guard::new();
            self.cache
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
        // The previous DashMap shape did O(N) full-iter+filter+sort.
        let entries: Vec<(RecordKey, Bytes)> = {
            let g = scc::ebr::Guard::new();
            self.cache
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

    /// Delegate to inner store's `transact`, then invalidate cache
    /// entries for all touched keys. The cache layer itself doesn't
    /// add atomicity — that comes from the inner backend.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        // Collect keys before delegating (ops is moved into inner).
        let keys: Vec<RecordKey> = ops
            .iter()
            .map(|op| match op {
                super::types::KvOp::Set(k, _) | super::types::KvOp::Remove(k) => k.clone(),
            })
            .collect();

        self.inner.transact(ops).await?;

        // Invalidate cache for affected keys so subsequent reads
        // see the transacted state, not stale cached values.
        for k in keys {
            if self.cache.remove(&k) {
                self.size.fetch_sub(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Pass-through for buffer config: a CachedStore doesn't have
    /// its own buffer knobs but the underlying store likely does
    /// (especially when stacked Cached → MemBuffer → raw).
    async fn apply_buffer_config(
        &self,
        config: &crate::storage_membuffer::MemBufferConfig,
    ) -> DbResult<()> {
        self.inner.apply_buffer_config(config).await
    }

    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        Some(Arc::clone(&self.inner))
    }

    /// Drain pending async writes and propagate the flush down to
    /// the inner store. Reachable through `Arc<dyn Store>` —
    /// without this override the trait dispatcher would land on
    /// the default no-op and async-mode writes would not become
    /// durable on a `flush()` callsite.
    async fn flush(&self) -> DbResult<()> {
        // Wait for the in-flight background `set`/`remove` tasks
        // (only present in `WriteMode::Async`). For `Sync` mode
        // pending_writes is always 0 and the loop body never runs.
        while self.pending_writes.load(Ordering::Relaxed) > 0 {
            tokio::task::yield_now().await;
        }
        // Now ensure the inner store's own buffered state lands.
        self.inner.flush().await
    }
}

// ============================================================================
// Tests
// ============================================================================
