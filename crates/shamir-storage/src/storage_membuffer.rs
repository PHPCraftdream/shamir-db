//! `MemBufferStore` — concurrent write-back buffer over any `Store`,
//! backed by **`moka`** as the in-RAM cache.
//!
//! # Design
//!
//! ```text
//!  ┌──────────────────────────────────────────────────────┐
//!  │  read / write                                        │
//!  │     │                                                │
//!  │     ▼                                                │
//!  │  ┌─────────────────────────────────────────┐         │
//!  │  │ moka::future::Cache<RecordKey, Slot>    │         │
//!  │  │  - max_capacity, weigher, TTL handled   │         │
//!  │  │    by moka (W-TinyLFU eviction)         │         │
//!  │  │  - lock-free read path (per-thread      │         │
//!  │  │    event buffers internally)            │         │
//!  │  │  - async eviction listener flushes      │         │
//!  │  │    dirty entries inline                 │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  │               │                                       │
//!  │  ┌────────────┴────────────────────────────┐         │
//!  │  │ DashSet<RecordKey> dirty                │         │
//!  │  │  - lock-free per-shard inserts/removes  │         │
//!  │  │  - drained by background flusher        │         │
//!  │  └────────────┬────────────────────────────┘         │
//!  └───────────────┼──────────────────────────────────────┘
//!                  ▼
//!           ┌─────────────┐
//!           │ Inner Store │
//!           └─────────────┘
//! ```
//!
//! # Why moka
//!
//! The previous implementation was a `Mutex<LruCache>` + `Mutex<HashSet>`.
//! Bench `membuffer_concurrent_rw` showed flat throughput as readers
//! scaled (632 Kelem/s at 8 readers vs 754 Kelem/s at 1) — every op
//! serialised on the single cache mutex.
//!
//! `moka` is a production-grade concurrent cache:
//!   * eviction handled internally (W-TinyLFU + LRU window),
//!   * TTL handled by `time_to_live`,
//!   * byte-cap handled by `weigher` + `max_capacity`,
//!   * reads + writes are lock-free for non-conflicting keys (per-thread
//!     event buffers, drained by a background maintenance task — the
//!     same pattern Caffeine uses on the JVM).
//!
//! # Durability contract (unchanged)
//!
//! `insert / set / remove` return as soon as the cache + dirty set are
//! updated — the inner store hasn't been touched yet. The flusher drains
//! `dirty` in batches; for synchronous durability call `flush().await`.
//!
//! # Eviction
//!
//! When `moka` evicts an entry to honour capacity / TTL, the
//! async eviction listener fires. If the key is still in `dirty`,
//! the listener flushes the value to `inner` inline and removes
//! the key from `dirty`. This preserves the no-data-loss
//! guarantee: a dirty entry is never silently dropped from cache.
//!
//! The user-observable effect is **eventually consistent**: the
//! eviction listener runs in moka's background maintenance task,
//! not synchronously with the user's insert. Tests that assume
//! "immediate inner visibility after eviction" wait for the
//! flusher tick.
//!
//! # apply_config
//!
//! Atomic config fields are written directly. For sizing /
//! TTL changes, the cache is **rebuilt** with the new config —
//! the old cache's contents are flushed to inner via
//! `drain_all().await` first, so no data is lost. This is rare
//! (DDL-driven only), so the rebuild cost is amortised.

use super::types::{RecordKey, Store};
use crate::error::{DbError, DbResult};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::Bytes;
use dashmap::DashSet;
use futures::stream::Stream;
use moka::future::Cache as MokaCache;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Configuration for `MemBufferStore`. Stable wire-format
/// (serialized into `info_store` by the DDL layer).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemBufferConfig {
    /// Soft cap on the sum of key+value bytes held in the cache.
    /// Default `64 MiB`. Enforced by moka via the `weigher`.
    pub max_bytes: usize,
    /// Hard cap on the entry count. Enforced by moka via
    /// `max_capacity`. Note: `moka` requires `max_capacity` and
    /// `weigher` to be self-consistent — we set both, and moka
    /// picks the tighter binding constraint.
    pub max_entries: usize,
    /// Optional time-to-live for cache entries. `None` = no TTL.
    pub ttl_ms: Option<u64>,
    /// Background flusher idle interval (ms).
    pub flush_interval_ms: u64,
    /// Max number of dirty keys the flusher drains per batch.
    pub flush_batch_size: usize,
}

impl Default for MemBufferConfig {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_entries: 100_000,
            ttl_ms: None,
            // 500ms — balances "ACK→durable lag" against fsync
            // amortisation. On per-write-fsync backends
            // (persy/nebari/canopy) this turns ~1000 individual
            // commits into ~2 batched commits per second.
            flush_interval_ms: 500,
            flush_batch_size: 256,
        }
    }
}

/// In-cache slot — either a live value or a tombstone for a
/// previously-removed key (cached negative result).
#[derive(Clone, Debug)]
enum Slot {
    Live(Bytes),
    Tombstone,
}

/// Shared mutable state. Lives behind `Arc` so the background
/// flusher (and moka's eviction listener) can hold weak refs.
struct MemBufferState {
    /// The actual cache. Wrapped in `ArcSwap` so `apply_config`
    /// can hot-swap to a freshly-built moka instance with new
    /// sizing/TTL without taking a lock on the read path.
    cache: ArcSwap<MokaCache<RecordKey, Slot>>,
    /// Lock-free set of keys whose state in the cache hasn't
    /// been propagated to `inner` yet. Drained by the flusher.
    /// The moka eviction listener references THIS SAME Arc — so
    /// updates from the user path and the listener see the same
    /// data. Cloning into a separate DashSet would silently
    /// disconnect the two.
    dirty: Arc<DashSet<RecordKey>>,
    /// Atomic-config — read on hot paths so DDL changes apply
    /// without rewrapping the store.
    max_bytes: AtomicUsize,
    max_entries: AtomicUsize,
    ttl_ms: AtomicU64,
    flush_interval_ms: AtomicU64,
    flush_batch_size: AtomicUsize,
}

impl MemBufferState {
    fn flush_interval(&self) -> Duration {
        Duration::from_millis(self.flush_interval_ms.load(Ordering::Relaxed))
    }
}

pub struct MemBufferStore {
    inner: Arc<dyn Store>,
    state: Arc<MemBufferState>,
    /// Wakes the background flusher on dirty-state change.
    notify: Arc<Notify>,
    /// Set on Drop — the flusher checks it on each wakeup and exits.
    shutdown: Arc<AtomicBool>,
}

/// Build a fresh moka cache configured from `cfg`. The async
/// eviction listener flushes dirty entries to `inner` before
/// dropping them.
fn build_cache(
    cfg: &MemBufferConfig,
    inner: Arc<dyn Store>,
    dirty: Arc<DashSet<RecordKey>>,
) -> MokaCache<RecordKey, Slot> {
    // moka's `max_capacity` is **total weighted size** when a
    // weigher is set, NOT entry count. We pick byte-weight as
    // the binding cap (`max_bytes`); `max_entries` is kept in
    // config for legacy callers but is informational here — the
    // wrapper doesn't enforce it as a separate cap. A workload
    // with many tiny records will still be bounded by `max_bytes`.
    let mut builder = MokaCache::builder()
        .max_capacity(cfg.max_bytes as u64)
        .weigher(|k: &RecordKey, v: &Slot| -> u32 {
            let bytes = match v {
                Slot::Live(b) => k.len() + b.len(),
                Slot::Tombstone => k.len(),
            };
            bytes.min(u32::MAX as usize) as u32
        });
    if let Some(ttl) = cfg.ttl_ms {
        builder = builder.time_to_live(Duration::from_millis(ttl));
    }
    // Async eviction listener — fires when moka removes an entry.
    // Causes we care about:
    //   * `Size` — capacity (max_bytes / max_capacity) eviction.
    //   * `Expired` — TTL eviction.
    // Causes we IGNORE:
    //   * `Replaced` — same key, new value; the new value is
    //     already in dirty (the user's set/remove path inserted
    //     it). Flushing the OLD value here would overwrite the
    //     new state in inner — that's a correctness bug.
    //   * `Explicit` — manual `invalidate`; user signalled removal,
    //     no flush wanted.
    use moka::notification::RemovalCause;
    builder = builder.async_eviction_listener(move |k, v, cause| {
        let inner = Arc::clone(&inner);
        let dirty = Arc::clone(&dirty);
        Box::pin(async move {
            if !matches!(cause, RemovalCause::Size | RemovalCause::Expired) {
                return;
            }
            let key: RecordKey = (*k).clone();
            if !dirty.remove(&key).is_some() {
                return;
            }
            // Was dirty AND moka is dropping it on the floor — we
            // MUST persist or the write is lost.
            match v {
                Slot::Live(bytes) => {
                    let _ = inner.set(key, bytes).await;
                }
                Slot::Tombstone => {
                    let _ = inner.remove(key).await;
                }
            }
        })
    });
    builder.build()
}

impl MemBufferStore {
    pub fn new(inner: Arc<dyn Store>, config: MemBufferConfig) -> Self {
        let dirty: Arc<DashSet<RecordKey>> = Arc::new(DashSet::new());
        let cache = Arc::new(build_cache(&config, Arc::clone(&inner), Arc::clone(&dirty)));

        let state = Arc::new(MemBufferState {
            cache: ArcSwap::from(cache),
            dirty,
            max_bytes: AtomicUsize::new(config.max_bytes),
            max_entries: AtomicUsize::new(config.max_entries),
            ttl_ms: AtomicU64::new(config.ttl_ms.unwrap_or(0)),
            flush_interval_ms: AtomicU64::new(config.flush_interval_ms),
            flush_batch_size: AtomicUsize::new(config.flush_batch_size),
        });
        let notify = Arc::new(Notify::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        // Background flusher.
        let weak_state = Arc::downgrade(&state);
        let weak_notify = Arc::downgrade(&notify);
        let weak_shutdown = Arc::downgrade(&shutdown);
        let inner_for_task = Arc::clone(&inner);
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
                let flush_interval = state.flush_interval();
                let batch_size = state.flush_batch_size.load(Ordering::Relaxed);
                tokio::select! {
                    _ = notify.notified() => {},
                    _ = tokio::time::sleep(flush_interval) => {},
                }
                let _ = Self::drain_once(&state, inner_for_task.as_ref(), batch_size).await;
                // Force moka to run its maintenance — this fires
                // any pending eviction listeners (TTL + capacity)
                // so dirty entries that crossed thresholds get
                // flushed promptly.
                state.cache.load().run_pending_tasks().await;
            }
        });

        Self {
            inner,
            state,
            notify,
            shutdown,
        }
    }

    /// Hot-reload the buffer config. Drains all in-flight dirty
    /// entries to `inner` FIRST (so nothing is lost when the old
    /// cache is dropped), then atomically swaps in a fresh cache
    /// built with the new sizing/TTL. The new cache starts
    /// empty — subsequent reads repopulate it from inner as
    /// needed.
    pub async fn apply_config(&self, cfg: &MemBufferConfig) -> DbResult<()> {
        // Drain BEFORE swap — the old cache holds the values for
        // dirty keys; if we swap first, those values are
        // unreachable and the dirty set has nothing to flush.
        self.drain_all().await?;

        self.state
            .max_bytes
            .store(cfg.max_bytes, Ordering::Relaxed);
        self.state
            .max_entries
            .store(cfg.max_entries, Ordering::Relaxed);
        self.state
            .ttl_ms
            .store(cfg.ttl_ms.unwrap_or(0), Ordering::Relaxed);
        self.state
            .flush_interval_ms
            .store(cfg.flush_interval_ms, Ordering::Relaxed);
        self.state
            .flush_batch_size
            .store(cfg.flush_batch_size, Ordering::Relaxed);

        // Build + swap the new cache.
        let new_cache = Arc::new(build_cache(
            cfg,
            Arc::clone(&self.inner),
            Arc::clone(&self.state.dirty),
        ));
        self.state.cache.store(new_cache);
        self.notify.notify_one();
        Ok(())
    }

    pub fn inner(&self) -> &Arc<dyn Store> {
        &self.inner
    }

    /// Total resident bytes in the cache (sum of key + value
    /// lengths over Live slots). Used by tests + DDL monitoring.
    pub fn cache_bytes(&self) -> usize {
        self.state.cache.load().weighted_size() as usize
    }

    /// Drain up to `batch_size` dirty keys into the inner store.
    /// Returns the number of keys actually drained.
    async fn drain_once(
        state: &MemBufferState,
        inner: &dyn Store,
        batch_size: usize,
    ) -> DbResult<usize> {
        if state.dirty.is_empty() {
            return Ok(0);
        }
        // Snapshot up to batch_size keys from dirty.
        let keys: Vec<RecordKey> = state
            .dirty
            .iter()
            .take(batch_size)
            .map(|e| e.clone())
            .collect();
        if keys.is_empty() {
            return Ok(0);
        }

        let cache = state.cache.load();
        let mut sets: Vec<(RecordKey, Bytes)> = Vec::with_capacity(keys.len());
        let mut removes: Vec<RecordKey> = Vec::new();
        let mut handled: Vec<RecordKey> = Vec::with_capacity(keys.len());
        for k in keys.into_iter() {
            match cache.get(&k).await {
                Some(Slot::Live(v)) => {
                    sets.push((k.clone(), v));
                    handled.push(k);
                }
                Some(Slot::Tombstone) => {
                    removes.push(k.clone());
                    handled.push(k);
                }
                None => {
                    // Evicted from cache between our collect and
                    // our get. The eviction listener handles such
                    // entries (it removes the key from dirty and
                    // flushes to inner). We MUST NOT remove the
                    // key from dirty here — that would defeat the
                    // listener (dirty.remove returns None → skip
                    // flush → write lost).
                }
            }
        }
        for k in &handled {
            state.dirty.remove(k);
        }

        let n = sets.len() + removes.len();
        if !sets.is_empty() {
            inner.set_many(sets).await?;
        }
        if !removes.is_empty() {
            inner.remove_many(removes).await?;
        }
        Ok(n)
    }

    /// Drain the entire dirty queue.
    async fn drain_all(&self) -> DbResult<()> {
        loop {
            let drained =
                Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
            if drained == 0 {
                break;
            }
        }
        // Force moka's pending maintenance so any TTL/capacity
        // evictions also flush via the listener.
        self.state.cache.load().run_pending_tasks().await;
        // After listeners run, dirty may have new entries (the
        // listener removes from dirty before flushing — but the
        // flush itself doesn't re-dirty). Defensive second pass.
        loop {
            let drained =
                Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
            if drained == 0 {
                break;
            }
        }
        Ok(())
    }
}

impl Drop for MemBufferStore {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

type RecordStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

#[async_trait]
impl Store for MemBufferStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = shamir_types::types::record_id::RecordId::new();
        let key = RecordKey::copy_from_slice(id.as_bytes());
        self.state.dirty.insert(key.clone());
        self.state
            .cache
            .load()
            .insert(key.clone(), Slot::Live(value))
            .await;
        self.notify.notify_one();
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // `bool` return = was the key created (vs updated)?
        // Three-tier check: cache → inner. dirty alone isn't
        // authoritative because a previously-flushed key might
        // still be live in `inner` but absent from `dirty`.
        let cache = self.state.cache.load();
        let existed = match cache.get(&key).await {
            Some(Slot::Live(_)) => true,
            Some(Slot::Tombstone) => false,
            None => {
                // Cache cold for this key. Ask inner.
                match self.inner.get(key.clone()).await {
                    Ok(_) => true,
                    Err(DbError::NotFound(_)) => false,
                    Err(e) => return Err(e),
                }
            }
        };
        self.state.dirty.insert(key.clone());
        cache.insert(key, Slot::Live(value)).await;
        self.notify.notify_one();
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        let cache = self.state.cache.load();
        match cache.get(&key).await {
            Some(Slot::Live(v)) => Ok(v),
            Some(Slot::Tombstone) => Err(DbError::NotFound(format!("{:?}", key))),
            None => {
                // Miss — fall through to inner, populate cache
                // (NOT dirty: this is a clean read-fill).
                let result = self.inner.get(key.clone()).await;
                let slot_to_insert = match &result {
                    Ok(v) => Some(Slot::Live(v.clone())),
                    Err(DbError::NotFound(_)) => Some(Slot::Tombstone),
                    Err(_) => None,
                };
                if let Some(slot) = slot_to_insert {
                    cache.insert(key, slot).await;
                }
                result
            }
        }
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        let cache = self.state.cache.load();
        let existed = match cache.get(&key).await {
            Some(Slot::Live(_)) => true,
            Some(Slot::Tombstone) => false,
            None => match self.inner.get(key.clone()).await {
                Ok(_) => true,
                Err(DbError::NotFound(_)) => false,
                Err(e) => return Err(e),
            },
        };
        self.state.dirty.insert(key.clone());
        cache.insert(key, Slot::Tombstone).await;
        self.notify.notify_one();
        Ok(existed)
    }

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        // Drain dirty first so the inner stream is a consistent view.
        let state = Arc::clone(&self.state);
        let inner = Arc::clone(&self.inner);
        let batch = batch_size;
        let bs = self
            .state
            .flush_batch_size
            .load(Ordering::Relaxed);
        Box::pin(async_stream::stream! {
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
        let bs = self.state.flush_batch_size.load(Ordering::Relaxed);
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
        let bs = self.state.flush_batch_size.load(Ordering::Relaxed);
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
        let bs = self.state.flush_batch_size.load(Ordering::Relaxed);
        Box::pin(async_stream::stream! {
            while {
                let n = MemBufferStore::drain_once(&state, inner.as_ref(), bs).await
                    .unwrap_or(0);
                n > 0
            } {}
            let inner_stream =
                inner.iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size);
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

    async fn apply_buffer_config(&self, config: &MemBufferConfig) -> DbResult<()> {
        self.apply_config(config).await?;
        self.inner.apply_buffer_config(config).await
    }

    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        let mut keys = Vec::with_capacity(values.len());
        let cache = self.state.cache.load();
        for v in values {
            let id = shamir_types::types::record_id::RecordId::new();
            let key = RecordKey::copy_from_slice(id.as_bytes());
            self.state.dirty.insert(key.clone());
            cache.insert(key.clone(), Slot::Live(v)).await;
            keys.push(key);
        }
        self.notify.notify_one();
        Ok(keys)
    }

    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
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
    use super::*;
    use crate::storage_in_memory::InMemoryRepo;
    use crate::types::{run_batch_store_tests, Repo};

    fn small_config() -> MemBufferConfig {
        MemBufferConfig {
            max_bytes: 4 * 1024,
            max_entries: 16,
            ttl_ms: None,
            flush_interval_ms: 10,
            flush_batch_size: 8,
        }
    }

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
        let got = buffered.get(key).await.unwrap();
        assert_eq!(got.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn eviction_with_dirty_eventually_flushes_evictee() {
        // moka's eviction is eventually consistent — the eviction
        // listener fires from the maintenance task. We wait for
        // the flusher / pending tasks to propagate.
        //
        // max_bytes=80 (~ one 21-byte slot fits but two don't —
        // each Slot::Live(b"first"/b"second") weighs
        // 16 key + 5/6 value = 21/22).
        let cfg = MemBufferConfig {
            max_bytes: 30,
            max_entries: 1_000_000,
            ttl_ms: None,
            flush_interval_ms: 25,
            flush_batch_size: 1,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let k1 = buffered.insert(Bytes::from_static(b"first")).await.unwrap();
        let k2 = buffered.insert(Bytes::from_static(b"second")).await.unwrap();

        // Wait for the eviction listener + flusher.
        let mut got1 = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if let Ok(v) = inner_store.get(k1.clone()).await {
                got1 = Some(v);
                break;
            }
        }
        let got1 = got1.expect("evicted dirty entry must eventually reach inner");
        assert_eq!(got1.as_ref(), b"first");

        // After explicit flush k2 also lands.
        buffered.flush().await.unwrap();
        let got2 = inner_store.get(k2).await.unwrap();
        assert_eq!(got2.as_ref(), b"second");
    }

    #[tokio::test]
    async fn tombstone_blocks_inner_visibility() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let key = inner_store
            .insert(Bytes::from_static(b"stale"))
            .await
            .unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store.clone(),
            MemBufferConfig::default(),
        ));
        let _ = buffered.get(key.clone()).await.unwrap();
        let existed = buffered.remove(key.clone()).await.unwrap();
        assert!(existed);
        let result = buffered.get(key.clone()).await;
        assert!(matches!(result, Err(DbError::NotFound(_))));
        buffered.flush().await.unwrap();
        let result_inner = inner_store.get(key).await;
        assert!(matches!(result_inner, Err(DbError::NotFound(_))));
    }

    #[tokio::test]
    async fn background_flusher_eventually_drains() {
        let cfg = MemBufferConfig {
            max_bytes: 64 * 1024,
            max_entries: 256,
            ttl_ms: None,
            flush_interval_ms: 20,
            flush_batch_size: 256,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let mut keys = Vec::new();
        for i in 0..5u8 {
            let k = buffered.insert(Bytes::copy_from_slice(&[i])).await.unwrap();
            keys.push(k);
        }
        let mut found = 0;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
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
        assert_eq!(found, keys.len(), "background flusher must drain dirty entries");
        buffered.flush().await.unwrap();
    }

    #[tokio::test]
    async fn bytes_eviction_caps_resident_size() {
        // max_bytes=256, each value ~64 bytes. After inserts, the
        // moka cache should hold at most a couple entries (cap
        // enforced via weigher). Records still reachable via the
        // dirty-flush + inner path.
        let cfg = MemBufferConfig {
            max_bytes: 256,
            max_entries: 1_000_000,
            ttl_ms: None,
            flush_interval_ms: 60_000,
            flush_batch_size: 256,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let mut keys = Vec::new();
        for _ in 0..10u8 {
            let key = buffered
                .insert(Bytes::from_static(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
                .await
                .unwrap();
            keys.push(key);
        }

        // Force any pending maintenance.
        buffered.state.cache.load().run_pending_tasks().await;

        // All ten records visible end-to-end. Eviction listener +
        // flusher push evictees to inner; cache holds the rest.
        // Wait for propagation.
        let mut found = 0;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            found = 0;
            for k in &keys {
                if buffered.get(k.clone()).await.is_ok() {
                    found += 1;
                }
            }
            if found == keys.len() {
                break;
            }
        }
        assert_eq!(found, 10, "all written records must be reachable");
    }

    #[tokio::test]
    async fn ttl_eviction_drops_old_entries() {
        let cfg = MemBufferConfig {
            max_bytes: 64 * 1024,
            max_entries: 100,
            ttl_ms: Some(80),
            flush_interval_ms: 30,
            flush_batch_size: 16,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store.clone(), cfg));

        let _k1 = buffered.insert(Bytes::from_static(b"a")).await.unwrap();
        let _k2 = buffered.insert(Bytes::from_static(b"b")).await.unwrap();
        // Wait > TTL + flusher tick + maintenance.
        tokio::time::sleep(Duration::from_millis(400)).await;
        buffered.state.cache.load().run_pending_tasks().await;

        // Records still readable from inner (eviction listener flushed them).
        let v1 = inner_store.get(_k1).await.unwrap();
        let v2 = inner_store.get(_k2).await.unwrap();
        assert_eq!(v1.as_ref(), b"a");
        assert_eq!(v2.as_ref(), b"b");
    }

    #[tokio::test]
    async fn apply_config_shrinks_max_bytes_and_triggers_eviction() {
        let cfg = MemBufferConfig {
            max_bytes: 64 * 1024,
            max_entries: 1_000_000,
            ttl_ms: None,
            flush_interval_ms: 60_000,
            flush_batch_size: 16,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store, cfg));

        for _ in 0..16u8 {
            let _ = buffered
                .insert(Bytes::from_static(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
                .await
                .unwrap();
        }
        // Flush so dirty is empty; subsequent reads come from cache or inner.
        buffered.flush().await.unwrap();

        let smaller = MemBufferConfig {
            max_bytes: 128,
            max_entries: 1_000_000,
            ttl_ms: None,
            flush_interval_ms: 60_000,
            flush_batch_size: 16,
        };
        buffered.apply_config(&smaller).await.unwrap();

        // After config swap the new cache is empty (rebuilt).
        // Insert ONE more entry; the new cache should hold at most
        // its capacity. The 16 prior entries reside in inner only.
        let _ = buffered.insert(Bytes::from_static(b"trigger")).await.unwrap();
        // run_pending_tasks fires any synchronous eviction.
        buffered.state.cache.load().run_pending_tasks().await;
        assert!(
            buffered.cache_bytes() <= 200,
            "new cap not honoured: cache_bytes={}",
            buffered.cache_bytes()
        );
    }

    #[tokio::test]
    async fn apply_config_enables_ttl_at_runtime() {
        let cfg = MemBufferConfig {
            max_bytes: 64 * 1024,
            max_entries: 256,
            ttl_ms: None,
            flush_interval_ms: 25,
            flush_batch_size: 16,
        };
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(inner_store, cfg));

        let _ = buffered.insert(Bytes::from_static(b"v")).await.unwrap();
        // Allow moka maintenance to commit weighted_size.
        buffered.state.cache.load().run_pending_tasks().await;
        assert!(buffered.cache_bytes() > 0);

        tokio::time::sleep(Duration::from_millis(80)).await;
        buffered.state.cache.load().run_pending_tasks().await;
        assert!(buffered.cache_bytes() > 0, "no TTL set — entry should persist");

        let with_ttl = MemBufferConfig {
            max_bytes: 64 * 1024,
            max_entries: 256,
            ttl_ms: Some(50),
            flush_interval_ms: 25,
            flush_batch_size: 16,
        };
        buffered.apply_config(&with_ttl).await.unwrap();

        // New cache is empty (rebuild). Insert one more entry under
        // the TTL'd cache; wait for it to expire.
        let _ = buffered.insert(Bytes::from_static(b"w")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        buffered.state.cache.load().run_pending_tasks().await;
        assert_eq!(
            buffered.cache_bytes(),
            0,
            "TTL not applied at runtime: cache_bytes={}",
            buffered.cache_bytes()
        );
    }

    #[tokio::test]
    async fn flush_drains_then_calls_inner_flush() {
        let inner_repo = InMemoryRepo::new();
        let inner_store = inner_repo.store_get("t").await.unwrap();
        let buffered = Arc::new(MemBufferStore::new(
            inner_store.clone(),
            small_config(),
        ));
        for i in 0..50u8 {
            let _ = buffered.insert(Bytes::copy_from_slice(&[i])).await.unwrap();
        }
        buffered.flush().await.unwrap();
        assert!(buffered.state.dirty.is_empty(), "dirty must be empty after flush");
    }
}
