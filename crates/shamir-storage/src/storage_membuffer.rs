//! `MemBufferStore` — bounded LRU + write-back buffer over any
//! `Store`.
//!
//! # Design
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────┐
//!  │  read / write                                        │
//!  │     │                                                │
//!  │     ▼                                                │
//!  │  ┌─────────────────────────────────────────┐         │
//!  │  │ LRU cache (max_entries) + dirty set     │         │
//!  │  │ - read hit: return from cache           │         │
//!  │  │ - read miss: inner.get + cache populate │         │
//!  │  │ - write: cache.put + dirty.insert       │         │
//!  │  │   → instant return                      │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  │               │ on idle / on signal                  │
//!  │               ▼                                       │
//!  │  ┌─────────────────────────────────────────┐         │
//!  │  │ Background flusher task                 │         │
//!  │  │ - drains dirty → inner.set_many /       │         │
//!  │  │   inner.remove_many in batches          │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  └───────────────┼──────────────────────────────────────┘
//!                  ▼
//!           ┌─────────────┐
//!           │ Inner Store │
//!           └─────────────┘
//! ```
//!
//! # Durability contract
//!
//! `MemBufferStore::insert / set / remove` return as soon as the
//! cache is updated — **the inner store hasn't been touched yet**.
//! If the process crashes before the flusher drains the dirty
//! queue, those writes are lost from the inner store's view.
//!
//! For the engine layer this means:
//! - **Records lost** on crash if their batch never flushed.
//! - **No inconsistency** introduced — the engine's WAL marker
//!   for an INSERT records the record_id; if the record value
//!   never reached the inner store, recovery on next open sees
//!   "marker says record_id X, data_store has no such record"
//!   and treats it as a not-committed write (clears the marker).
//! - **No orphan postings** — index entries are written through
//!   the same `info_store` that's wrapped by membuffer; if data
//!   is lost so are its indexes.
//!
//! Callers requiring strict durability should call
//! `Store::flush().await` at the commit boundary. `flush()`
//! drains the dirty queue to the inner store and then calls
//! `inner.flush()` so the whole stack lands.
//!
//! # Eviction
//!
//! When the cache is at capacity, an `insert` evicts the LRU
//! entry. If the evicted entry is dirty, the flusher is signalled
//! and the new entry stays out of cache until space is available
//! (synchronous back-pressure). Today's simple implementation
//! does the dirty flush INLINE on eviction; later we can move it
//! to the background task with bounded write-queue depth.

use super::types::{RecordKey, Store};
use crate::error::{DbError, DbResult};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use lru::LruCache;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Configuration for `MemBufferStore`.
#[derive(Debug, Clone)]
pub struct MemBufferConfig {
    pub max_entries: usize,
    pub flush_interval_ms: u64,
    pub flush_batch_size: usize,
}

impl Default for MemBufferConfig {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            flush_interval_ms: 100,
            flush_batch_size: 256,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    /// The key holds this value.
    Live(Bytes),
    /// The key is known to NOT be in the inner store. Either a
    /// recent `remove` we haven't propagated yet, or a confirmed-
    /// missing key we cached to avoid re-asking inner.
    Tombstone,
}

/// Shared mutable state of one `MemBufferStore`. Lives behind
/// `Arc<MemBufferState>` so the background flusher task can hold
/// a weak reference and exit gracefully when the store is
/// dropped.
struct MemBufferState {
    /// The cache. Capacity == `config.max_entries`. Standard LRU
    /// semantics: hot entries float to head, cold ones fall to
    /// tail and get evicted on insert pressure.
    cache: Mutex<LruCache<RecordKey, Slot>>,
    /// Keys whose state has been changed in `cache` but not yet
    /// propagated to `inner`. Drained by the flusher.
    dirty: Mutex<HashSet<RecordKey>>,
}

pub struct MemBufferStore {
    inner: Arc<dyn Store>,
    state: Arc<MemBufferState>,
    /// Wakes the background flusher on dirty-state change.
    notify: Arc<Notify>,
    /// Set on Drop — the flusher checks it on each wakeup and
    /// exits.
    shutdown: Arc<AtomicBool>,
    #[allow(dead_code)]
    config: MemBufferConfig,
}

impl MemBufferStore {
    pub fn new(inner: Arc<dyn Store>, config: MemBufferConfig) -> Self {
        let cap = NonZeroUsize::new(config.max_entries.max(1)).unwrap();
        let state = Arc::new(MemBufferState {
            cache: Mutex::new(LruCache::new(cap)),
            dirty: Mutex::new(HashSet::new()),
        });
        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        // Background flusher. Holds Weak references so its lifecycle
        // tracks the store's. On `MemBufferStore::drop` the Weaks
        // can no longer upgrade and the task exits.
        let weak_state = Arc::downgrade(&state);
        let weak_notify = Arc::downgrade(&notify);
        let weak_shutdown = Arc::downgrade(&shutdown);
        let inner_for_task = inner.clone();
        let flush_interval = std::time::Duration::from_millis(config.flush_interval_ms);
        let batch_size = config.flush_batch_size;
        tokio::spawn(async move {
            loop {
                let state = match weak_state.upgrade() {
                    Some(s) => s,
                    None => break,
                };
                let notify = match weak_notify.upgrade() {
                    Some(n) => n,
                    None => break,
                };
                let shutdown = match weak_shutdown.upgrade() {
                    Some(s) => s,
                    None => break,
                };
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                tokio::select! {
                    _ = notify.notified() => {},
                    _ = tokio::time::sleep(flush_interval) => {},
                }
                let _ = Self::drain_once(&state, inner_for_task.as_ref(), batch_size).await;
            }
        });

        Self {
            inner,
            state,
            notify,
            shutdown,
            config,
        }
    }

    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Drain at most `batch_size` dirty keys into the inner store.
    /// Returns `Ok(0)` when there's nothing to flush.
    async fn drain_once(
        state: &MemBufferState,
        inner: &dyn Store,
        batch_size: usize,
    ) -> DbResult<usize> {
        // Snapshot up to `batch_size` dirty keys + their current
        // values. We hold the cache lock only long enough to copy.
        let (sets, removes) = {
            let mut dirty = state.dirty.lock().unwrap();
            if dirty.is_empty() {
                return Ok(0);
            }
            let keys: Vec<RecordKey> = dirty.iter().take(batch_size).cloned().collect();
            for k in &keys {
                dirty.remove(k);
            }
            drop(dirty);

            let mut cache = state.cache.lock().unwrap();
            let mut sets: Vec<(RecordKey, Bytes)> = Vec::new();
            let mut removes: Vec<RecordKey> = Vec::new();
            for k in keys {
                // Use `peek` so the flusher's read doesn't promote
                // the entry in LRU order (flushing isn't a "use").
                match cache.peek(&k) {
                    Some(Slot::Live(v)) => sets.push((k, v.clone())),
                    Some(Slot::Tombstone) => removes.push(k),
                    None => {
                        // Entry was evicted between dirty-mark and
                        // flush-pickup. The eviction path already
                        // flushed it inline (see set/remove below),
                        // so nothing to do.
                    }
                }
            }
            (sets, removes)
        };

        if !sets.is_empty() {
            inner.set_many(sets).await?;
        }
        if !removes.is_empty() {
            inner.remove_many(removes).await?;
        }
        Ok(1)
    }

    /// Drain the entire dirty queue.
    async fn drain_all(&self) -> DbResult<()> {
        loop {
            let drained =
                Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
            if drained == 0 {
                break;
            }
            // After draining, check if more became dirty mid-flush.
            let still_dirty = !self.state.dirty.lock().unwrap().is_empty();
            if !still_dirty {
                break;
            }
        }
        Ok(())
    }

    /// Update the cache for a put/remove operation. If the
    /// inserted entry evicts a dirty entry, flush that evicted
    /// entry to the inner store synchronously before returning —
    /// otherwise cache-pressure would silently drop pending
    /// writes.
    async fn cache_put(&self, key: RecordKey, slot: Slot) -> DbResult<()> {
        // Insert into cache; capture any evicted (key, slot) pair
        // and whether it was dirty (= had a pending write).
        let evicted: Option<(RecordKey, Slot)> = {
            let mut cache = self.state.cache.lock().unwrap();
            let evicted = cache.push(key.clone(), slot);
            // `push` returns Some((evicted_key, evicted_value)) if
            // the new entry pushed out the LRU tail.
            evicted
        };
        let evicted_was_dirty = if let Some((ek, _)) = &evicted {
            let mut dirty = self.state.dirty.lock().unwrap();
            dirty.remove(ek)
        } else {
            false
        };
        // Mark the new entry dirty.
        {
            let mut dirty = self.state.dirty.lock().unwrap();
            dirty.insert(key);
        }
        // If we just evicted a dirty entry, flush it INLINE — we
        // can't drop a pending write on the floor.
        if let (Some((ek, eslot)), true) = (evicted, evicted_was_dirty) {
            match eslot {
                Slot::Live(v) => {
                    let _ = self.inner.set(ek, v).await?;
                }
                Slot::Tombstone => {
                    let _ = self.inner.remove(ek).await?;
                }
            }
        }
        // Signal the flusher so it picks up the new dirty entry.
        self.notify.notify_one();
        Ok(())
    }
}

impl Drop for MemBufferStore {
    fn drop(&mut self) {
        // Signal the background task to exit. Pending dirty
        // writes are NOT flushed here — that's the caller's
        // responsibility (`store.flush().await` before drop).
        self.shutdown.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

type RecordStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

#[async_trait]
impl Store for MemBufferStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        // We need a fresh RecordKey. Delegate to the inner store
        // for the ID generation only — actual write goes through
        // the cache via `set`. This keeps key uniqueness aligned
        // with the inner store's policy.
        //
        // For backends whose `insert` allocates an ID by writing
        // (most of them — they call RecordId::new() and write the
        // value), we'd waste a write. Optimisation: generate the
        // ID locally via `RecordId::new`, then `set` it.
        let id = shamir_types::types::record_id::RecordId::new();
        let key = RecordKey::copy_from_slice(id.as_bytes());
        self.cache_put(key.clone(), Slot::Live(value)).await?;
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // `bool` return = was the key created (vs updated)? We need
        // to know whether the inner store ever knew about this key.
        // Check cache + inner.
        let in_cache = {
            let mut cache = self.state.cache.lock().unwrap();
            match cache.get(&key) {
                Some(Slot::Live(_)) => Some(true), // existed
                Some(Slot::Tombstone) => Some(false), // we know it's gone
                None => None,
            }
        };
        let created = match in_cache {
            Some(existed) => !existed,
            None => {
                // Have to ask the inner store.
                match self.inner.get(key.clone()).await {
                    Ok(_) => false,
                    Err(DbError::NotFound(_)) => true,
                    Err(e) => return Err(e),
                }
            }
        };
        self.cache_put(key, Slot::Live(value)).await?;
        Ok(created)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Cache lookup with LRU touch.
        let slot = {
            let mut cache = self.state.cache.lock().unwrap();
            cache.get(&key).cloned()
        };
        match slot {
            Some(Slot::Live(v)) => Ok(v),
            Some(Slot::Tombstone) => {
                Err(DbError::NotFound(format!("{:?}", key)))
            }
            None => {
                // Miss — fall through to inner, populate cache.
                let result = self.inner.get(key.clone()).await;
                let slot_to_insert = match &result {
                    Ok(v) => Some(Slot::Live(v.clone())),
                    Err(DbError::NotFound(_)) => Some(Slot::Tombstone),
                    Err(_) => None,
                };
                if let Some(slot) = slot_to_insert {
                    let mut cache = self.state.cache.lock().unwrap();
                    cache.put(key, slot);
                }
                result
            }
        }
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let existed_in_cache = {
            let mut cache = self.state.cache.lock().unwrap();
            match cache.get(&key) {
                Some(Slot::Live(_)) => Some(true),
                Some(Slot::Tombstone) => Some(false),
                None => None,
            }
        };
        let existed = match existed_in_cache {
            Some(b) => b,
            None => {
                // Ask inner.
                match self.inner.get(key.clone()).await {
                    Ok(_) => true,
                    Err(DbError::NotFound(_)) => false,
                    Err(e) => return Err(e),
                }
            }
        };
        self.cache_put(key, Slot::Tombstone).await?;
        Ok(existed)
    }

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        // Correct path: flush all dirty, then iterate inner. For
        // small caches the flush is cheap; for large ones it's
        // O(dirty). Future: merge cache view with inner stream
        // (LSM-style) — left as TODO.
        let state = Arc::clone(&self.state);
        let inner = Arc::clone(&self.inner);
        let batch = batch_size;
        let bs = self.config.flush_batch_size;
        Box::pin(async_stream::stream! {
            // Drain dirty before iter.
            while {
                let n = MemBufferStore::drain_once(&state, inner.as_ref(), bs).await
                    .unwrap_or(0);
                n > 0
            } {}
            let inner_stream = inner.iter_stream(batch);
            futures::pin_mut!(inner_stream);
            while let Some(b) = futures::StreamExt::next(&mut inner_stream).await {
                yield b;
            }
        })
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        let state = Arc::clone(&self.state);
        let inner = Arc::clone(&self.inner);
        let p = prefix;
        let bs = self.config.flush_batch_size;
        Box::pin(async_stream::stream! {
            while {
                let n = MemBufferStore::drain_once(&state, inner.as_ref(), bs).await
                    .unwrap_or(0);
                n > 0
            } {}
            let inner_stream = inner.scan_prefix_stream(p, batch_size);
            futures::pin_mut!(inner_stream);
            while let Some(b) = futures::StreamExt::next(&mut inner_stream).await {
                yield b;
            }
        })
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        let state = Arc::clone(&self.state);
        let inner = Arc::clone(&self.inner);
        let bs = self.config.flush_batch_size;
        Box::pin(async_stream::stream! {
            while {
                let n = MemBufferStore::drain_once(&state, inner.as_ref(), bs).await
                    .unwrap_or(0);
                n > 0
            } {}
            let inner_stream = inner.iter_range_stream(start_inclusive, end_inclusive, batch_size);
            futures::pin_mut!(inner_stream);
            while let Some(b) = futures::StreamExt::next(&mut inner_stream).await {
                yield b;
            }
        })
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        let state = Arc::clone(&self.state);
        let inner = Arc::clone(&self.inner);
        let bs = self.config.flush_batch_size;
        Box::pin(async_stream::stream! {
            while {
                let n = MemBufferStore::drain_once(&state, inner.as_ref(), bs).await
                    .unwrap_or(0);
                n > 0
            } {}
            let inner_stream = inner.iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size);
            futures::pin_mut!(inner_stream);
            while let Some(b) = futures::StreamExt::next(&mut inner_stream).await {
                yield b;
            }
        })
    }

    async fn flush(&self) -> DbResult<()> {
        self.drain_all().await?;
        self.inner.flush().await
    }

    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let mut keys = Vec::with_capacity(values.len());
        for v in values {
            keys.push(self.insert(v).await?);
        }
        Ok(keys)
    }

    async fn set_many(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<Vec<bool>> {
        let mut flags = Vec::with_capacity(items.len());
        for (k, v) in items {
            flags.push(self.set(k, v).await?);
        }
        Ok(flags)
    }

    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        let mut flags = Vec::with_capacity(keys.len());
        for k in keys {
            flags.push(self.remove(k).await?);
        }
        Ok(flags)
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            match self.get(k).await {
                Ok(v) => out.push(Some(v)),
                Err(DbError::NotFound(_)) => out.push(None),
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(deprecated)]

    use super::*;
    use crate::storage_in_memory::InMemoryRepo;
    use crate::types::{run_batch_store_tests, Repo};

    fn small_config() -> MemBufferConfig {
        MemBufferConfig {
            max_entries: 16,
            flush_interval_ms: 10,
            flush_batch_size: 8,
        }
    }

    /// Test helper — drop the buffered store with `flush()` first
    /// to ensure inner reflects all writes.
    async fn drained(store: Arc<MemBufferStore>) {
        store.flush().await.unwrap();
    }

    #[tokio::test]
    async fn buffered_passes_full_batch_suite() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(
            inner_store,
            MemBufferConfig::default(),
        ));
        run_batch_store_tests(store).await;
    }

    #[tokio::test]
    async fn write_visible_after_flush_in_inner() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store.clone(),
            MemBufferConfig::default(),
        ));

        let key = buffered.insert(Bytes::from_static(b"v1")).await.unwrap();
        // Before flush: inner may not have it yet (write-back).
        // After flush: must have it.
        buffered.flush().await.unwrap();
        let got = inner_store.get(key).await.unwrap();
        assert_eq!(got.as_ref(), b"v1");
        drained(buffered).await;
    }

    #[tokio::test]
    async fn read_after_write_returns_buffered_value() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store,
            MemBufferConfig::default(),
        ));

        let key = buffered.insert(Bytes::from_static(b"hello")).await.unwrap();
        // Reading immediately — must come from cache.
        let got = buffered.get(key).await.unwrap();
        assert_eq!(got.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn eviction_with_dirty_flushes_evictee_inline() {
        // Configure tiny cache (1 slot). Each new insert must
        // evict the previous one. The evictee was dirty → must be
        // flushed to inner inline (not just dropped).
        let cfg = MemBufferConfig {
            max_entries: 1,
            flush_interval_ms: 60_000, // disable background flush during the test
            flush_batch_size: 1,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let k1 = buffered.insert(Bytes::from_static(b"first")).await.unwrap();
        let k2 = buffered.insert(Bytes::from_static(b"second")).await.unwrap();
        // k1 has been evicted by k2. Inner store must already have
        // k1=first (inline eviction-flush). k2 may or may not be
        // in inner (still dirty in cache).
        let got1 = inner_store.get(k1).await.unwrap();
        assert_eq!(got1.as_ref(), b"first");
        // After explicit flush k2 also lands.
        buffered.flush().await.unwrap();
        let got2 = inner_store.get(k2).await.unwrap();
        assert_eq!(got2.as_ref(), b"second");
    }

    #[tokio::test]
    async fn tombstone_blocks_inner_visibility() {
        // Cache has Tombstone for key K → get returns NotFound
        // even though inner might still have stale data (until
        // the flusher propagates the tombstone).
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        // Plant data directly in inner so cache starts cold.
        let key = inner_store
            .insert(Bytes::from_static(b"stale"))
            .await
            .unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store.clone(),
            MemBufferConfig::default(),
        ));
        // Through buffered: read populates cache with Live.
        let _ = buffered.get(key.clone()).await.unwrap();
        // Delete through buffered — sets a Tombstone.
        let existed = buffered.remove(key.clone()).await.unwrap();
        assert!(existed);
        // Immediate read: must respect the tombstone.
        let result = buffered.get(key.clone()).await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
        // After flush, inner doesn't have it either.
        buffered.flush().await.unwrap();
        let result_inner = inner_store.get(key).await;
        assert!(matches!(result_inner, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn background_flusher_eventually_drains() {
        // Without an explicit flush, the background task should
        // drain dirty entries within roughly `flush_interval_ms`.
        let cfg = MemBufferConfig {
            max_entries: 256,
            flush_interval_ms: 20,
            flush_batch_size: 256,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let mut keys = Vec::new();
        for i in 0..5u8 {
            let k = buffered
                .insert(Bytes::copy_from_slice(&[i]))
                .await
                .unwrap();
            keys.push(k);
        }

        // Wait for the background flusher. Up to ~500ms tolerance
        // for slow CI.
        let mut found = 0;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            found = 0;
            for k in &keys {
                if inner_store.get(k.clone()).await.is_ok() {
                    found += 1;
                }
            }
            if found == keys.len() {
                break;
            }
        }
        assert_eq!(
            found,
            keys.len(),
            "background flusher must drain dirty entries"
        );
        // Drop with a flush to be a good citizen.
        buffered.flush().await.unwrap();
    }

    #[tokio::test]
    async fn flush_drains_then_calls_inner_flush() {
        // Compound assertion: after flush(), inner sees everything
        // AND inner.flush() was reached (we can't easily observe
        // the inner.flush() call directly on InMemory, but we
        // can confirm no error path).
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store.clone(),
            small_config(),
        ));
        for i in 0..50u8 {
            let _ = buffered
                .insert(Bytes::copy_from_slice(&[i]))
                .await
                .unwrap();
        }
        buffered.flush().await.unwrap();
        // No dirty entries left.
        assert!(buffered.state.dirty.lock().unwrap().is_empty());
    }
}
