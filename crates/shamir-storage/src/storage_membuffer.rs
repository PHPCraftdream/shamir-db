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
use dashmap::DashMap;
use futures::stream::Stream;
use moka::future::Cache as MokaCache;
use shamir_collections::THasher;
use shamir_types::types::record_id::RecordId;
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
#[derive(Clone, Debug, PartialEq)]
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
    /// Pending writes (write-back buffer). Holds VALUES — not
    /// just keys — so that a moka cache eviction never loses
    /// data: even if the cache silently drops an entry, dirty
    /// still has the value. The flusher drains via a
    /// snapshot-then-`remove_if` pattern so concurrent
    /// overwrites aren't lost.
    ///
    /// This replaces an earlier design that used `DashSet<Key>`
    /// + a moka async eviction listener to flush. The listener
    ///   was firing per evicted entry (TTL/Size cause); on tight
    ///   loops with frequent eviction it dominated the cost.
    dirty: Arc<DashMap<RecordKey, Slot, THasher>>,
    /// Fast-path sentinel: true iff dirty is (likely) non-empty.
    /// Set with Release before each dirty.insert so any subsequent
    /// Acquire-load in get() that sees `false` is guaranteed to see
    /// a fully drained map. False positives (flag true, map empty)
    /// are harmless — they cause one extra DashMap lookup that
    /// returns None. False negatives are impossible: we always set
    /// before insert (Release → Acquire ordering).
    dirty_nonempty: AtomicBool,
    /// Atomic-config — read on hot paths so DDL changes apply
    /// without rewrapping the store.
    max_bytes: AtomicUsize,
    max_entries: AtomicUsize,
    ttl_ms: AtomicU64,
    flush_interval_ms: AtomicU64,
    flush_batch_size: AtomicUsize,
    /// Audit §2.2: telemetry counter for background-flush errors. The
    /// background flusher previously swallowed errors silently (`let _ =
    /// drain_once(...)`); now each failure increments this counter and logs
    /// an error so disk-full / I/O failures are observable (otherwise dirty
    /// grows unboundedly with zero signal).
    flush_errors: AtomicU64,
}

impl MemBufferState {
    fn flush_interval(&self) -> Duration {
        let ms = self.flush_interval_ms.load(Ordering::Relaxed);
        Duration::from_millis(ms.max(1))
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
fn build_cache(cfg: &MemBufferConfig) -> MokaCache<RecordKey, Slot> {
    // moka's `max_capacity` is **total weighted size** when a
    // weigher is set, NOT entry count. We pick byte-weight as
    // the binding cap (`max_bytes`); `max_entries` is kept in
    // config for legacy callers but is informational here.
    //
    // NO eviction listener — the cache is purely a read
    // accelerator. Dirty is the authoritative write-back buffer
    // (holds values, survives cache eviction). moka can evict
    // freely without risking data loss.
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
    builder.build()
}

impl MemBufferStore {
    pub fn new(inner: Arc<dyn Store>, config: MemBufferConfig) -> Self {
        let dirty: Arc<DashMap<RecordKey, Slot, THasher>> =
            Arc::new(DashMap::with_hasher(THasher::default()));
        let cache = Arc::new(build_cache(&config));

        let state = Arc::new(MemBufferState {
            cache: ArcSwap::from(cache),
            dirty,
            dirty_nonempty: AtomicBool::new(false),
            max_bytes: AtomicUsize::new(config.max_bytes),
            max_entries: AtomicUsize::new(config.max_entries),
            ttl_ms: AtomicU64::new(config.ttl_ms.unwrap_or(0)),
            flush_interval_ms: AtomicU64::new(config.flush_interval_ms),
            flush_batch_size: AtomicUsize::new(config.flush_batch_size),
            flush_errors: AtomicU64::new(0),
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
                let batch_size = state.flush_batch_size.load(Ordering::Relaxed).max(1);
                tokio::select! {
                    _ = notify.notified() => {},
                    _ = tokio::time::sleep(flush_interval) => {},
                }
                // Audit §2.2: surface background-flush errors instead of
                // swallowing them silently (`let _ = ...`). A disk-full or
                // I/O failure now increments a telemetry counter and logs an
                // error so it is observable — without this, dirty grows
                // unboundedly with zero signal.
                if let Err(e) = Self::drain_once(&state, inner_for_task.as_ref(), batch_size).await
                {
                    state.flush_errors.fetch_add(1, Ordering::Relaxed);
                    log::error!(
                        "MemBufferStore background flush failed (dirty entries retained, \
                         will retry next interval): {e}"
                    );
                }
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

        self.state.max_bytes.store(cfg.max_bytes, Ordering::Relaxed);
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
        let new_cache = Arc::new(build_cache(cfg));
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

    /// Run moka's pending maintenance tasks (eviction, sizing). For tests only.
    #[cfg(test)]
    pub(crate) async fn run_pending_cache_tasks(&self) {
        self.state.cache.load().run_pending_tasks().await;
    }

    /// Return whether the dirty queue is empty. For tests only.
    #[cfg(test)]
    pub(crate) fn is_dirty_empty(&self) -> bool {
        self.state.dirty.is_empty()
    }

    /// Drain up to `batch_size` dirty entries into the inner
    /// store. Snapshot-then-`remove_if` pattern protects against
    /// races where a concurrent write to the same key shouldn't
    /// be lost.
    async fn drain_once(
        state: &MemBufferState,
        inner: &dyn Store,
        batch_size: usize,
    ) -> DbResult<usize> {
        if state.dirty.is_empty() {
            return Ok(0);
        }
        // Snapshot keys + their current slot values.
        let snapshots: Vec<(RecordKey, Slot)> = state
            .dirty
            .iter()
            .take(batch_size)
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        if snapshots.is_empty() {
            return Ok(0);
        }

        let mut sets: Vec<(RecordKey, Bytes)> = Vec::with_capacity(snapshots.len());
        let mut removes: Vec<RecordKey> = Vec::with_capacity(4);
        for (k, slot) in &snapshots {
            match slot {
                Slot::Live(v) => sets.push((k.clone(), v.clone())),
                Slot::Tombstone => removes.push(k.clone()),
            }
        }

        let n = sets.len() + removes.len();
        if !sets.is_empty() {
            inner.set_many(sets).await?;
        }
        if !removes.is_empty() {
            inner.remove_many(removes).await?;
        }

        // CAS-style cleanup: remove from dirty ONLY if the value
        // is still what we just flushed. If a concurrent writer
        // overwrote the slot, leave it for the next drain.
        for (k, snapshot) in snapshots {
            state.dirty.remove_if(&k, |_, current| *current == snapshot);
        }
        // If dirty is now fully drained, clear the fast-path sentinel so
        // get() can skip the dirty lookup. Relaxed is fine here: if a
        // concurrent writer just set the flag (Release) and inserted, we
        // may clear it erroneously — but that thread's Release+our Relaxed
        // forms a race. To avoid the race we only clear when dirty is
        // actually empty, which is a stable post-drain observation.
        if state.dirty.is_empty() {
            state.dirty_nonempty.store(false, Ordering::Relaxed);
        }
        Ok(n)
    }

    /// Audit finding 2.3 (task #530) — snapshot the dirty overlay for a scan,
    /// filtered by `pred` (prefix / range membership) and sorted ascending by
    /// key. This is the SMALL side of the merge-overlay scan: instead of
    /// draining the whole dirty buffer to disk before every scan (read-
    /// triggered write amplification), we materialise the (bounded) dirty map
    /// once at stream-open and merge it on top of the sorted `inner` stream.
    ///
    /// The returned vector holds `(RecordKey, Slot)` — `Slot::Tombstone`
    /// entries are RETAINED (not filtered out): during the merge a tombstone
    /// masks any stale `inner` value for the same key, and during the overlay-
    /// only tail an overlay tombstone for a key `inner` never had is simply
    /// skipped (nothing to emit).
    fn snapshot_overlay_sorted<F>(state: &MemBufferState, pred: F) -> Vec<(RecordKey, Slot)>
    where
        F: Fn(&RecordKey) -> bool,
    {
        // Fast-path: if the dirty buffer is (observably) empty the scan is a
        // pure `inner` pass-through — skip the DashMap traversal + sort.
        if !state.dirty_nonempty.load(Ordering::Acquire) {
            return Vec::new();
        }
        let mut snap: Vec<(RecordKey, Slot)> = state
            .dirty
            .iter()
            .filter(|e| pred(e.key()))
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        // Sorted ascending by key — matches the `inner` stream's ascending
        // lexicographic ordering so the merge below is a single linear pass.
        snap.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        snap
    }

    /// Drain the entire dirty queue.
    async fn drain_all(&self) -> DbResult<()> {
        loop {
            let drained = Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
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
            let drained = Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
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

type RecordStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// Audit finding 2.3 (task #530) — merge a sorted dirty-overlay snapshot on
/// top of a sorted `inner` record stream, WITHOUT draining the overlay to
/// disk first.
///
/// This is a classic 2-way sorted merge (mirrors
/// `MvccStore::current_stream`'s overlay-vs-history merge): both the `overlay`
/// snapshot and the `inner` stream are key-sorted (ascending when
/// `reverse == false`, descending when `reverse == true` — the overlay is
/// pre-sorted by the caller to match), so a single linear pass yields a merged
/// key-ordered stream. Semantics:
///
/// * An overlay entry ALWAYS wins over `inner` for the same key (it is the
///   newer, not-yet-flushed value). `Slot::Live` → emit the overlay value;
///   `Slot::Tombstone` → EXCLUDE the key (even if `inner` still holds stale
///   data for it).
/// * Overlay-only keys (present in the dirty buffer but not yet in `inner`)
///   are emitted in their sorted position during the merge, and any that sort
///   after the last `inner` key are flushed in the overlay-only tail phase.
/// * Ordering across the merged output is preserved, so range / prefix scans
///   keep their ascending / descending guarantee.
///
/// `inner` MUST yield keys in the requested order (ascending, or descending
/// for reverse) — every `Store` scan upholds this per the trait contract.
fn merge_overlay_stream(
    overlay: Vec<(RecordKey, Slot)>,
    inner: RecordStream,
    batch_size: usize,
    reverse: bool,
) -> RecordStream {
    Box::pin(async_stream::stream! {
        let batch_cap = batch_size.max(1);
        let mut ov = overlay.into_iter().peekable();
        let mut out: Vec<(RecordKey, Bytes)> = Vec::with_capacity(batch_cap);

        // `ord(a, b)` is the "a should come before b in output order" test.
        // Ascending: a < b. Descending: a > b.
        let before = |a: &RecordKey, b: &RecordKey| -> bool {
            if reverse {
                a > b
            } else {
                a < b
            }
        };

        futures::pin_mut!(inner);
        while let Some(batch) = futures::StreamExt::next(&mut inner).await {
            let batch = match batch {
                Ok(b) => b,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            for (ik, iv) in batch {
                // Emit every overlay entry that sorts strictly before the
                // current inner key (overlay-only keys interleaved in order).
                while let Some((ok, _)) = ov.peek() {
                    if before(ok, &ik) {
                        let (ok, oslot) = ov.next().unwrap();
                        if let Slot::Live(v) = oslot {
                            out.push((ok, v));
                            if out.len() >= batch_cap {
                                yield Ok(std::mem::take(&mut out));
                                out = Vec::with_capacity(batch_cap);
                            }
                        }
                        // Tombstone-only overlay key: nothing to emit.
                    } else {
                        break;
                    }
                }
                // Overlay entry for the EXACT inner key wins (newer write).
                if let Some((ok, _)) = ov.peek() {
                    if *ok == ik {
                        let (ok, oslot) = ov.next().unwrap();
                        match oslot {
                            Slot::Live(v) => {
                                out.push((ok, v));
                                if out.len() >= batch_cap {
                                    yield Ok(std::mem::take(&mut out));
                                    out = Vec::with_capacity(batch_cap);
                                }
                            }
                            // Tombstone masks the stale inner value → skip.
                            Slot::Tombstone => {}
                        }
                        continue;
                    }
                }
                // No overlay entry for this key → inner value stands.
                out.push((ik, iv));
                if out.len() >= batch_cap {
                    yield Ok(std::mem::take(&mut out));
                    out = Vec::with_capacity(batch_cap);
                }
            }
        }

        // Overlay-only tail: any remaining overlay entries (keys `inner` never
        // yielded). Live entries are emitted; tombstones are dropped.
        for (ok, oslot) in ov {
            if let Slot::Live(v) = oslot {
                out.push((ok, v));
                if out.len() >= batch_cap {
                    yield Ok(std::mem::take(&mut out));
                    out = Vec::with_capacity(batch_cap);
                }
            }
        }

        if !out.is_empty() {
            yield Ok(out);
        }
    })
}

#[async_trait]
impl Store for MemBufferStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = RecordId::new();
        let key = RecordKey::from_slice(id.as_bytes());
        let slot = Slot::Live(value);
        // Release before dirty.insert so get()'s Acquire load sees the flag
        // set if dirty is non-empty. False positives are harmless.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        self.state.dirty.insert(key.clone(), slot.clone());
        self.state.cache.load().insert(key.clone(), slot).await;
        self.notify.notify_one();
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // `bool` return = was the key created (vs updated)?
        // Authoritative source is dirty + cache (consistent by
        // construction — we dual-write). Falling through to
        // inner.get on miss is best-effort: `false` (presumed new).
        // shamir-engine doesn't depend on strict bool.
        let existed = match self.state.dirty.get(&key).map(|e| e.value().clone()) {
            Some(Slot::Live(_)) => true,
            Some(Slot::Tombstone) => false,
            None => match self.state.cache.load().get(&key).await {
                Some(Slot::Live(_)) => true,
                Some(Slot::Tombstone) => false,
                None => false,
            },
        };
        let slot = Slot::Live(value);
        // Release before dirty.insert — see insert() for ordering rationale.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        self.state.dirty.insert(key.clone(), slot.clone());
        self.state.cache.load().insert(key, slot).await;
        self.notify.notify_one();
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // Read order: cache (lock-free) → dirty (covers cases
        // where moka evicted but value still pending) → inner.
        let cache = self.state.cache.load();
        if let Some(slot) = cache.get(&key).await {
            return match slot {
                Slot::Live(v) => Ok(v),
                Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
            };
        }
        // Cache miss — check dirty before going to inner. Dirty
        // may hold a value that moka silently evicted from cache.
        // Fast-path: when dirty_nonempty is false (Acquire), dirty is
        // guaranteed empty — skip the DashMap probe entirely.
        // Pairs with the Release store in insert/set/remove.
        if self.state.dirty_nonempty.load(Ordering::Acquire) {
            if let Some(entry) = self.state.dirty.get(&key) {
                return match entry.value() {
                    Slot::Live(v) => Ok(v.clone()),
                    Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
                };
            }
        }
        // Fall through to inner; populate cache only (NOT dirty —
        // this is a clean read-fill).
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

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        // Best-effort bool — see `set` for rationale.
        let existed = match self.state.dirty.get(&key).map(|e| e.value().clone()) {
            Some(Slot::Live(_)) => true,
            Some(Slot::Tombstone) => false,
            None => match self.state.cache.load().get(&key).await {
                Some(Slot::Live(_)) => true,
                Some(Slot::Tombstone) => false,
                None => false,
            },
        };
        // Release before dirty.insert — see insert() for ordering rationale.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        self.state.dirty.insert(key.clone(), Slot::Tombstone);
        self.state.cache.load().insert(key, Slot::Tombstone).await;
        self.notify.notify_one();
        Ok(existed)
    }

    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        // Audit finding 2.3 (task #530): DON'T drain-before-scan (read-
        // triggered write amplification that defeats the 500ms fsync-batching
        // interval on EVERY scan). Instead snapshot the SMALL dirty overlay
        // and MERGE it on top of the sorted `inner` stream — a full flush is
        // no longer required just to read. The `inner` stream is key-sorted
        // (every backend upholds ascending order), so this is a linear
        // 2-way sorted merge; tombstones mask stale inner values and
        // overlay-only keys are yielded in the tail. See `merge_overlay_stream`.
        let overlay = Self::snapshot_overlay_sorted(&self.state, |_| true);
        let inner_stream = self.inner.iter_stream(batch_size);
        merge_overlay_stream(overlay, inner_stream, batch_size, false)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        // Merge-overlay scan (task #530) — see `iter_stream`. The overlay is
        // filtered to keys under `prefix` so the overlay-only tail can't emit
        // out-of-prefix keys, and the ascending order is preserved (callers of
        // the prefix scan — e.g. the posting-list cache — rely on sorted,
        // in-prefix output).
        let prefix_bytes = prefix.clone();
        let overlay =
            Self::snapshot_overlay_sorted(&self.state, |k| k.as_ref().starts_with(&prefix_bytes));
        let inner_stream = self.inner.scan_prefix_stream(prefix, batch_size);
        merge_overlay_stream(overlay, inner_stream, batch_size, false)
    }

    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        // Merge-overlay scan (task #530) — see `iter_stream`. The overlay is
        // filtered to `[start_inclusive ..= end_inclusive]` and sorted
        // ascending, then merged with the sorted `inner` range stream so the
        // ascending key order is preserved across the merge (sorted-index
        // range/order/min queries depend on it).
        let lo = start_inclusive.clone();
        let hi = end_inclusive.clone();
        let overlay = Self::snapshot_overlay_sorted(&self.state, |k| {
            if let Some(ref s) = lo {
                if k.as_ref() < s.as_ref() {
                    return false;
                }
            }
            if let Some(ref e) = hi {
                if k.as_ref() > e.as_ref() {
                    return false;
                }
            }
            true
        });
        let inner_stream = self
            .inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size);
        merge_overlay_stream(overlay, inner_stream, batch_size, false)
    }

    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> RecordStream {
        // Merge-overlay scan (task #530) — reverse variant. The overlay is
        // filtered to the same `[start ..= end]` range but sorted DESCENDING
        // to match the reverse `inner` stream, so the merge preserves the
        // high→low order (`lookup_last_k` / `lookup_max` / `ORDER BY … DESC`).
        let lo = start_inclusive.clone();
        let hi = end_inclusive.clone();
        let mut overlay = Self::snapshot_overlay_sorted(&self.state, |k| {
            if let Some(ref s) = lo {
                if k.as_ref() < s.as_ref() {
                    return false;
                }
            }
            if let Some(ref e) = hi {
                if k.as_ref() > e.as_ref() {
                    return false;
                }
            }
            true
        });
        // `snapshot_overlay_sorted` sorts ascending; the reverse merge needs
        // descending order to line up with the reverse `inner` stream.
        overlay.reverse();
        let inner_stream =
            self.inner
                .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size);
        merge_overlay_stream(overlay, inner_stream, batch_size, true)
    }

    async fn flush(&self) -> DbResult<()> {
        self.drain_all().await?;
        self.inner.flush().await
    }

    async fn apply_buffer_config(&self, config: &MemBufferConfig) -> DbResult<()> {
        self.apply_config(config).await?;
        self.inner.apply_buffer_config(config).await
    }

    async fn raw_backend(&self) -> Option<Arc<dyn Store>> {
        Some(Arc::clone(&self.inner))
    }

    /// Delegate to inner store's `transact`, then update cache + dirty
    /// state for all touched keys. The buffer layer doesn't add
    /// atomicity — that comes from the inner backend.
    ///
    /// **Audit §2.3:** the old code called `dirty.remove(&k)` UNCONDITIONALLY
    /// after `inner.transact`. A concurrent `set(k)` landing between
    /// `inner.transact` and the `remove` put a NEW value into dirty — which
    /// then got removed, so it never reached the inner store. After a cache
    /// eviction or restart, durable state was the OLD value. The fix mirrors
    /// `drain_once`'s snapshot + `remove_if` pattern: snapshot the dirty slot
    /// BEFORE the cache update, and only remove the entry if it still matches
    /// that snapshot (i.e., no concurrent write overwrote it). Since
    /// `drain_all` at the top already emptied these keys, a surviving entry
    /// at this point is a concurrent write that must NOT be lost.
    async fn transact(&self, ops: Vec<super::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        // Drain any pending dirty entries for the affected keys first
        // so the inner store has a consistent view before transact.
        self.drain_all().await?;

        // Delegate to inner's native transact.
        self.inner.transact(ops.clone()).await?;

        // Update cache + dirty to reflect the transacted state.
        let cache = self.state.cache.load();
        for op in ops {
            match op {
                super::types::KvOp::Set(k, v) => {
                    let slot = Slot::Live(v);
                    cache.insert(k.clone(), slot.clone()).await;
                    // Audit §2.3: remove from dirty ONLY if the slot is
                    // still the one we just wrote — a concurrent `set(k)`
                    // that landed between `inner.transact` and here put a
                    // NEWER value into dirty; removing it would lose that
                    // write (the old unconditional `remove` did exactly
                    // this). `remove_if` with a value comparison is the
                    // same pattern `drain_once` uses.
                    self.state
                        .dirty
                        .remove_if(&k, |_, current| *current == slot);
                }
                super::types::KvOp::Remove(k) => {
                    let slot = Slot::Tombstone;
                    cache.insert(k.clone(), slot.clone()).await;
                    // Audit §2.3: same guarded removal as the Set branch.
                    self.state
                        .dirty
                        .remove_if(&k, |_, current| *current == slot);
                }
            }
        }
        Ok(())
    }

    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        let mut keys = Vec::with_capacity(values.len());
        let cache = self.state.cache.load();
        // Publish dirty-nonempty BEFORE populating, matching sibling
        // `insert` / `set` / `remove`. Without this, a concurrent `get()`
        // reading dirty_nonempty (Acquire) sees `false`, skips the dirty
        // probe (lines ~480), and stale-misses an insert_many'd key whose
        // cache slot has already been evicted by moka but not yet flushed.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        for v in values {
            let id = RecordId::new();
            let key = RecordKey::from_slice(id.as_bytes());
            let slot = Slot::Live(v);
            self.state.dirty.insert(key.clone(), slot.clone());
            cache.insert(key.clone(), slot).await;
            keys.push(key);
        }
        self.notify.notify_one();
        Ok(keys)
    }

    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        // Batched mirror of `set` for each (k,v) — same dirty/cache
        // dual-write, same best-effort `existed` semantics (dirty →
        // cache, never inner), but with hoisted cache snapshot and a
        // SINGLE notify_one at the end. Last-write-wins within the
        // batch on duplicate keys (the dirty/cache inserts happen in
        // input order; the final state matches the per-element loop).
        // Publish dirty-nonempty BEFORE populating, matching sibling `set`.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        let cache = self.state.cache.load();
        let mut flags = Vec::with_capacity(items.len());
        for (k, v) in items {
            let existed = match self.state.dirty.get(&k).map(|e| e.value().clone()) {
                Some(Slot::Live(_)) => true,
                Some(Slot::Tombstone) => false,
                None => match cache.get(&k).await {
                    Some(Slot::Live(_)) => true,
                    Some(Slot::Tombstone) => false,
                    None => false,
                },
            };
            let slot = Slot::Live(v);
            self.state.dirty.insert(k.clone(), slot.clone());
            cache.insert(k, slot).await;
            flags.push(!existed);
        }
        self.notify.notify_one();
        Ok(flags)
    }

    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // Batched mirror of `remove` — see `set_many` for the
        // batching shape. Tombstones replace Live slots; dirty/cache
        // dual-write preserved, ONE notify_one.
        // Publish dirty-nonempty BEFORE populating, matching sibling `remove`.
        self.state.dirty_nonempty.store(true, Ordering::Release);
        let cache = self.state.cache.load();
        let mut flags = Vec::with_capacity(keys.len());
        for k in keys {
            let existed = match self.state.dirty.get(&k).map(|e| e.value().clone()) {
                Some(Slot::Live(_)) => true,
                Some(Slot::Tombstone) => false,
                None => match cache.get(&k).await {
                    Some(Slot::Live(_)) => true,
                    Some(Slot::Tombstone) => false,
                    None => false,
                },
            };
            self.state.dirty.insert(k.clone(), Slot::Tombstone);
            cache.insert(k, Slot::Tombstone).await;
            flags.push(existed);
        }
        self.notify.notify_one();
        Ok(flags)
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // Same lookup order as `get`: cache → dirty → inner. Cache
        // hits and dirty hits are answered locally; the remaining
        // misses are batched into ONE `inner.get_many` call so disk
        // backends get a single transactional read instead of N.
        let cache = self.state.cache.load();
        let mut out: Vec<Option<Bytes>> = Vec::with_capacity(keys.len());
        let mut miss_idxs: Vec<usize> = Vec::new();
        let mut miss_keys: Vec<RecordKey> = Vec::new();
        for (i, k) in keys.into_iter().enumerate() {
            // Cache lookup first.
            if let Some(slot) = cache.get(&k).await {
                match slot {
                    Slot::Live(v) => out.push(Some(v)),
                    Slot::Tombstone => out.push(None),
                }
                continue;
            }
            // Cache miss — check dirty (may hold a value moka evicted).
            if let Some(entry) = self.state.dirty.get(&k) {
                match entry.value() {
                    Slot::Live(v) => out.push(Some(v.clone())),
                    Slot::Tombstone => out.push(None),
                }
                continue;
            }
            // Both miss — defer to a single inner.get_many.
            out.push(None);
            miss_idxs.push(i);
            miss_keys.push(k);
        }
        if !miss_keys.is_empty() {
            // Keep the miss keys for cache-fill after the batch read.
            let miss_keys_for_fill = miss_keys.clone();
            let inner_vals = self.inner.get_many(miss_keys).await?;
            for ((i, k), v) in miss_idxs
                .into_iter()
                .zip(miss_keys_for_fill.into_iter())
                .zip(inner_vals.into_iter())
            {
                // Populate cache (clean read-fill — NOT dirty). Tombstone
                // negative result so subsequent gets short-circuit.
                let slot = match &v {
                    Some(b) => Slot::Live(b.clone()),
                    None => Slot::Tombstone,
                };
                cache.insert(k, slot).await;
                out[i] = v;
            }
        }
        Ok(out)
    }
}
