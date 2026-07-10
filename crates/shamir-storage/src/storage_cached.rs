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
// CacheAction - what `transact` should do to the cache for a given op
// ============================================================================

/// Post-commit cache action for a single op in a `transact` batch.
///
/// Built from the original `KvOp` slice *before* `ops` is moved into
/// `inner.transact`, so the fresh value of a `Set` survives the move
/// and can populate the cache instead of forcing an invalidate (which
/// would leave the cache cold on just-written keys — audit
/// `2026-07-06-perf-radical-o-notation` finding 1.4).
enum CacheAction {
    /// `KvOp::Set` — populate the cache with the committed value.
    /// Inlines the same remove-then-insert + conditional size-bump
    /// that `cache_upsert` uses for the single-key `set` path, so the
    /// size-accounting discipline is identical (bumps `size` only on a
    /// genuinely new key).
    Populate(RecordKey, Bytes),
    /// `KvOp::Remove` — evict the entry (delete semantics).
    Invalidate(RecordKey),
}

impl CacheAction {
    fn from_op(op: &super::types::KvOp) -> Self {
        match op {
            super::types::KvOp::Set(k, v) => Self::Populate(k.clone(), v.clone()),
            super::types::KvOp::Remove(k) => Self::Invalidate(k.clone()),
        }
    }

    /// Apply this action to the cache, mirroring the existing
    /// size-accounting discipline:
    /// - `Populate` inlines `cache_upsert`'s remove-then-insert (bumps
    ///   `size` only on a genuinely new key).
    /// - `Invalidate` does a single `remove` and decrements `size` if
    ///   the key was present (unchanged from the prior behaviour).
    fn apply(self, cache: &TreeIndex<RecordKey, Bytes>, size: &AtomicUsize) {
        match self {
            Self::Populate(key, value) => {
                let existed = cache.remove(&key);
                if !existed {
                    size.fetch_add(1, Ordering::Relaxed);
                }
                // insert always succeeds after a remove (scc::TreeIndex
                // only rejects on a duplicate key, which we just removed).
                let _ = cache.insert(key, value);
            }
            Self::Invalidate(key) => {
                if cache.remove(&key) {
                    size.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }
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
        // Incremental/cursor stream: one bounded `range` query per batch,
        // each under its own short-lived `scc::ebr::Guard`. This replaces
        // the previous eager `collect()` of the entire cache before the
        // first yield — a consumer that only wants the first batch (e.g.
        // an upstream `LIMIT 10`) now pays O(batch_size) clones instead
        // of O(N) (audit `2026-07-06-perf-radical-o-notation` finding 1.3).
        //
        // scc's `Iter`/`Range` borrow the EBR `Guard` via a `'g` lifetime
        // and yield `&'g` references, so a fully-lazy held-cursor stream
        // (iterator stored across `.await` points in the `stream!` block)
        // isn't sound — the `Guard` must not outlive its short scope.
        // The repeated-bounded-requery approach mirrors fjall's
        // `iter_stream` resume-by-last-key cursor pattern: each batch
        // re-seeks to `Bound::Excluded(last_key)` and takes
        // `batch_size`, so per-batch cost is O(batch_size) and total
        // work scales with what the consumer actually drains. Order is
        // unchanged (TreeIndex ascending key order).
        let cache = self.cache.clone();

        Box::pin(stream! {
            let mut last_key: Option<RecordKey> = None;

            loop {
                // Drive one batch inside a short-lived Guard scope.
                let cur_last = last_key.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let g = scc::ebr::Guard::new();
                    // Use the tuple `(Bound<RecordKey>, Bound<RecordKey>)`
                    // form so scc infers `Q = RecordKey` (which is
                    // `Comparable<RecordKey>`); an explicit `Bound<..>..`
                    // range would make `Q = Bound<RecordKey>`, which isn't
                    // `Comparable<RecordKey>`.
                    let iter = match &cur_last {
                        Some(c) => cache.range((std::ops::Bound::Excluded(c.clone()), std::ops::Bound::Unbounded), &g),
                        None => cache.range((std::ops::Bound::Unbounded, std::ops::Bound::Unbounded), &g),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    let mut batch_last: Option<RecordKey> = None;
                    for (k, v) in iter.take(batch_size) {
                        batch_last = Some(k.clone());
                        items.push((k.clone(), v.clone()));
                    }
                    // If we produced a full batch, advance the cursor to
                    // its last key; a short (final) batch signals stream
                    // exhaustion → clear `last_key` so the outer loop ends.
                    if items.len() == batch_size {
                        last_key = batch_last;
                    } else {
                        last_key = None;
                    }
                    items
                };

                if batch.is_empty() {
                    break;
                }
                yield Ok(batch);

                // A short (final) batch cleared `last_key` → stream done.
                if last_key.is_none() {
                    break;
                }
            }
        })
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        // Same incremental/cursor pattern as `iter_stream` above, but the
        // initial lower bound is the prefix itself (seek directly to the
        // prefix range), and each entry is checked against the prefix to
        // stop as soon as we walk past it (TreeIndex yields lex order).
        // Per-batch cost is O(batch_size); a `LIMIT`-style early break
        // costs only what the consumer drained (audit finding 1.3).
        let cache = self.cache.clone();

        Box::pin(stream! {
            let mut last_key: Option<RecordKey> = None;

            loop {
                let cur_last = last_key.clone();
                let pfx = prefix.clone();
                let batch: Vec<(RecordKey, Bytes)> = {
                    let g = scc::ebr::Guard::new();
                    // `Bound` type must be `Comparable<RecordKey>` → use
                    // owned `RecordKey` for the bound key (the prefix bound
                    // converts the `Bytes` prefix into a `RecordKey`, a
                    // byte-identical boundary conversion; see `iter_stream`).
                    let iter = match &cur_last {
                        Some(c) => cache.range((std::ops::Bound::Excluded(c.clone()), std::ops::Bound::Unbounded), &g),
                        None => cache.range((std::ops::Bound::Included(RecordKey::from(pfx.clone())), std::ops::Bound::Unbounded), &g),
                    };
                    let mut items = Vec::with_capacity(batch_size.min(256));
                    let mut batch_last: Option<RecordKey> = None;
                    let mut exited_prefix = false;
                    for (k, v) in iter.take(batch_size) {
                        if !k.starts_with(&pfx[..]) {
                            exited_prefix = true;
                            break;
                        }
                        batch_last = Some(k.clone());
                        items.push((k.clone(), v.clone()));
                    }
                    // Advance the cursor unless we hit the end of the
                    // prefix range (then signal stream exhaustion).
                    if exited_prefix || items.len() < batch_size {
                        last_key = None;
                    } else {
                        last_key = batch_last;
                    }
                    items
                };

                if batch.is_empty() {
                    break;
                }
                yield Ok(batch);
                if last_key.is_none() {
                    break;
                }
            }
        })
    }

    /// Delegate to inner store's `transact`, then update the cache
    /// to reflect the committed state:
    /// - `KvOp::Set(key, value)` → **populate** the cache with the fresh
    ///   value, so a subsequent read-after-write hits the cache (RAM)
    ///   instead of going to the (disk) backend. The previous code
    ///   *invalidated* every touched key, which systematically kept the
    ///   cache cold on exactly the hottest keys (just-written data) —
    ///   audit `2026-07-06-perf-radical-o-notation` finding 1.4.
    /// - `KvOp::Remove(key)` → keep the existing eviction behaviour
    ///   (removal from cache is correct for deletes).
    ///
    /// The cache layer itself doesn't add atomicity — that comes from
    /// the inner backend. Size accounting for the populate path mirrors
    /// the same remove-then-insert + conditional-bump discipline the
    /// single-key `set` path uses, so the eviction accounting is
    /// identical.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        // Capture the cache action for each op BEFORE `ops` is moved
        // into `inner.transact`. `Set` carries the fresh value so the
        // cache can be populated post-commit; `Remove` just invalidates.
        let actions: Vec<CacheAction> = ops.iter().map(CacheAction::from_op).collect();

        self.inner.transact(ops).await?;

        // Apply each captured action to the cache. `Set` → populate
        // (upsert, same size discipline as `set`); `Remove` → evict.
        for action in actions {
            action.apply(&self.cache, &self.size);
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
