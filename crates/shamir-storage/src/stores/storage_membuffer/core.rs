use crate::error::DbResult;
use crate::types::{RecordKey, Store};
use arc_swap::ArcSwap;
use bytes::Bytes;
use dashmap::DashMap;
use moka::future::Cache as MokaCache;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::sync::Notify;

use super::revision::{Stamp, StripedRevisions};

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

/// In-dirty slot — either a live value or a tombstone for a
/// previously-removed key (cached negative result). `dirty` is the
/// authoritative write-back buffer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Slot {
    Live(Bytes),
    Tombstone,
}

/// Tagged cache entry (RV-STORAGE-98f6650d): every moka value carries the
/// stripe revision it was filled under. A hit is served only when the
/// tag still equals the stripe's current revision, the stripe stamp is
/// unchanged across the read, no writer is active, and the stripe is not
/// exhausted — so a stale physical entry (e.g. a slow fill that published
/// after a concurrent acknowledged write) is harmless: it is simply
/// rejected on its next hit and the authoritative dirty/backing lookup
/// runs instead.
#[derive(Clone, Debug)]
pub(crate) struct CacheEntry {
    pub(crate) rev: u64,
    pub(crate) slot: Slot,
}

/// Test-only seam (RV-STORAGE-98f6650d): ONE-SHOT consumable gate fired by
/// `clean_fill` at the final publication boundary — immediately BEFORE the
/// physical moka insertion. The FIRST clean fill after construction takes
/// the gate out of the setup-only `Mutex<Option<_>>` slot, signals the test
/// through the `entered` oneshot, and blocks on the `release` oneshot.
/// Every later clean fill finds the slot empty and passes straight through
/// while the first fill is still parked — so a test can let a fresh fill
/// COMPLETE (not merely be scheduled) before releasing the old one. `None`
/// in production (zero overhead, `#[cfg(test)]` field).
#[cfg(test)]
pub(crate) struct FillPauseHook {
    gate: std::sync::Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

#[cfg(test)]
impl FillPauseHook {
    /// Build the hook and hand back its single gate's halves: `entered_rx`
    /// fires when the first clean fill has reached the publication boundary;
    /// the test unblocks it via `release_tx`.
    pub(crate) fn new() -> (Arc<Self>, oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let hook = Arc::new(Self {
            gate: std::sync::Mutex::new(Some((entered_tx, release_rx))),
        });
        (hook, entered_rx, release_tx)
    }

    /// Fired by the fill path: consume the gate if it is still present —
    /// signal `entered`, block until `release`. A fill arriving after the
    /// gate was taken returns immediately. The `MutexGuard` is dropped in
    /// a scoped block BEFORE the `.await` so the future stays `Send`.
    pub(crate) async fn pause_point(&self) {
        let gate = {
            let mut slot = self.gate.lock().expect("fill-pause gate mutex poisoned");
            slot.take()
        };
        if let Some((entered_tx, release_rx)) = gate {
            let _ = entered_tx.send(());
            let _ = release_rx.await;
        }
    }
}

/// Shared mutable state. Lives behind `Arc` so the background
/// flusher (and moka's eviction listener) can hold weak refs.
pub(super) struct MemBufferState {
    /// The actual cache. Wrapped in `ArcSwap` so `apply_config`
    /// can hot-swap to a freshly-built moka instance with new
    /// sizing/TTL without taking a lock on the read path.
    ///
    /// Values are tagged [`CacheEntry`]s; the cache is a read accelerator
    /// for CLEAN fills only — writers never republish into it (dirty is
    /// the authority), and every hit is validated against the striped
    /// revision state.
    pub(super) cache: ArcSwap<MokaCache<RecordKey, CacheEntry>>,
    /// Pending writes (write-back buffer). Holds VALUES — not
    /// just keys — so that a moka cache eviction never loses
    /// data: even if the cache silently drops an entry, dirty
    /// still has the value. The flusher drains via a
    /// snapshot-then-`remove_if` pattern so concurrent
    /// overwrites aren't lost.
    pub(super) dirty: Arc<DashMap<RecordKey, Slot, THasher>>,
    /// Fixed-size striped revision state (RV-STORAGE-98f6650d): every
    /// dirty mutation route (insert/insert_many/set/set_many/remove/
    /// remove_many/transact/drain) takes a `StripeGuard` — a packed
    /// `{revision, active}` CAS epoch — across its publication, and every
    /// cache hit/fill validates against the stripe stamp. No per-key
    /// mutex map, no leases.
    pub(super) revisions: StripedRevisions,
    /// Task #539: O(1) cardinality mirror of `dirty`'s entry count — replaces
    /// the earlier `AtomicBool dirty_nonempty` sentinel (task #535 rounds 1+2),
    /// which could never be linearizable with `dirty` because it used a
    /// separate check-then-store(false)-then-re-check sequence (see git
    /// history / task #535 for the two rounds of narrowing patches applied
    /// there, and task #539 for why a boolean structurally cannot close the
    /// residual gap those rounds left open).
    ///
    /// Invariant: incremented exactly once at the SAME logical point a key is
    /// newly added to `dirty` (i.e. `dirty.insert()` returns `None` — the key
    /// was genuinely absent, not an overwrite); decremented exactly once at
    /// the SAME logical point a key is actually removed from `dirty` (i.e. a
    /// `remove_if` call whose predicate matched and the key was really taken
    /// out).
    ///
    /// RV-STORAGE-98f6650d: this counter is TELEMETRY / drain-emptiness
    /// only. It is NEVER correctness evidence on the read path — `get`/
    /// `get_many` always probe `dirty` directly first, unconditionally.
    pub(super) dirty_count: AtomicUsize,
    /// Atomic-config — read on hot paths so DDL changes apply
    /// without rewrapping the store.
    pub(super) max_bytes: AtomicUsize,
    pub(super) max_entries: AtomicUsize,
    pub(super) ttl_ms: AtomicU64,
    pub(super) flush_interval_ms: AtomicU64,
    pub(super) flush_batch_size: AtomicUsize,
    /// Audit §2.2: telemetry counter for background-flush errors. The
    /// background flusher previously swallowed errors silently (`let _ =
    /// drain_once(...)`); now each failure increments this counter and logs
    /// an error so disk-full / I/O failures are observable (otherwise dirty
    /// grows unboundedly with zero signal).
    pub(super) flush_errors: AtomicU64,
    /// Task #535 (updated #539): deterministic test seam originally used to
    /// reproduce the `dirty_nonempty` boolean's clear-race. Since #539
    /// replaced that boolean with an inline `dirty_count` counter (no
    /// separate clear step to race — the decrement happens at the same
    /// point as the removal), this hook is now fired right before
    /// `drain_once`'s removal loop purely to preserve the existing
    /// regression tests' shape (they install a racing-writer-insert
    /// callback here). `None` in production (zero overhead).
    #[cfg(test)]
    pub(super) clear_race_hook: crate::membuffer_clear_race_hook::ClearRaceHook,
    /// Task #535 round 2: deterministic test seam for the narrower
    /// stall-across-`.await` gap in `insert_many`'s batch loop. See
    /// `membuffer_clear_race_hook::BatchInsertPauseHook`. `None`/absent
    /// callback in production (zero overhead — the hook is only ever
    /// installed by the round-2 regression test).
    #[cfg(test)]
    pub(super) batch_pause_hook:
        arc_swap::ArcSwapOption<crate::membuffer_clear_race_hook::BatchInsertPauseHook>,
    /// RV-STORAGE-98f6650d test seam: pause a clean fill at the final
    /// publication boundary (right before the physical moka insertion).
    /// `None` in production.
    #[cfg(test)]
    pub(super) fill_pause_hook: arc_swap::ArcSwapOption<FillPauseHook>,
}

impl MemBufferState {
    fn flush_interval(&self) -> Duration {
        let ms = self.flush_interval_ms.load(Ordering::Relaxed);
        Duration::from_millis(ms.max(1))
    }

    /// Validated cache probe (RV-STORAGE-98f6650d): accept a tagged entry
    /// only if the CALLER's pre-lookup stamp (`s0`, captured BEFORE the
    /// authoritative dirty probe) is unchanged by the post-lookup capture
    /// (`s1`), no writer is active, the tag matches, and the stripe is not
    /// exhausted. Returns `None` on any rejection — the caller must fall
    /// through to the authoritative dirty/backing lookup.
    pub(super) async fn validated_cache_get(&self, key: &RecordKey, s0: Stamp) -> Option<Slot> {
        let entry = self.cache.load().get(key).await?;
        let s1 = self.revisions.stamp(key);
        if self.revisions.acceptable(s0, s1, entry.rev) {
            Some(entry.slot)
        } else {
            None
        }
    }

    /// Clean-fill publication (RV-STORAGE-98f6650d): insert a backing-read
    /// result into the cache, tagged with the revision captured BEFORE the
    /// backing lookup (`raw_stamp`) — NOT the current revision — so any
    /// write that raced the read invalidates the entry. Skipped entirely
    /// on an exhausted stripe (fail-closed caching disabled).
    pub(super) async fn clean_fill(&self, key: &RecordKey, raw_stamp: u64, slot: Slot) {
        #[cfg(test)]
        if let Some(hook) = self.fill_pause_hook.load_full() {
            hook.pause_point().await;
        }
        if self.revisions.is_exhausted(self.revisions.stripe_of(key)) {
            return;
        }
        let entry = CacheEntry {
            rev: self.revisions.rev_of_raw(raw_stamp),
            slot,
        };
        self.cache.load().insert(key.clone(), entry).await;
    }

    /// Best-effort existence probe used by `set`/`remove` for their `bool`
    /// return: authoritative dirty lookup first, then a VALIDATED cache
    /// probe whose pre-lookup stamp is captured BEFORE the dirty probe (an
    /// unvalidated stale entry could report a deleted key as existing).
    /// Still best-effort — a miss falls through to `false`.
    pub(super) async fn existed_hint(&self, key: &RecordKey) -> bool {
        let s0 = self.revisions.stamp(key);
        match self.dirty.get(key).map(|e| e.value().clone()) {
            Some(Slot::Live(_)) => true,
            Some(Slot::Tombstone) => false,
            None => matches!(self.validated_cache_get(key, s0).await, Some(Slot::Live(_))),
        }
    }

    /// Authoritative dirty lookup — returns the slot if `key` is dirty.
    /// (RV-STORAGE-98f6650d: the read path always probes `dirty` directly;
    /// `dirty_count` is never correctness evidence.)
    pub(super) fn dirty_probe(&self, key: &RecordKey) -> Option<Slot> {
        self.dirty.get(key).map(|e| e.value().clone())
    }
}

pub struct MemBufferStore {
    pub(super) inner: Arc<dyn Store>,
    pub(super) state: Arc<MemBufferState>,
    /// Wakes the background flusher on dirty-state change.
    pub(super) notify: Arc<Notify>,
    /// Set on Drop — the flusher checks it on each wakeup and exits.
    pub(super) shutdown: Arc<AtomicBool>,
}

/// Build a fresh moka cache configured from `cfg`. Values are tagged
/// `CacheEntry`s.
pub(super) fn build_cache(cfg: &MemBufferConfig) -> MokaCache<RecordKey, CacheEntry> {
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
        .weigher(|k: &RecordKey, v: &CacheEntry| -> u32 {
            let bytes = match &v.slot {
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
        #[cfg(test)]
        {
            Self::new_inner(inner, config, Default::default())
        }
        #[cfg(not(test))]
        {
            Self::new_inner(inner, config)
        }
    }

    /// Task #535: test-only constructor that installs a `ClearRaceHook` fired
    /// inside `drain_once` at the sentinel-clear window, so a regression test
    /// can deterministically drive the racing writer insert into the gap.
    #[cfg(test)]
    pub(crate) fn new_with_clear_race_hook(
        inner: Arc<dyn Store>,
        config: MemBufferConfig,
        hook: crate::membuffer_clear_race_hook::ClearRaceHook,
    ) -> Self {
        Self::new_inner(inner, config, hook)
    }

    /// Task #535 round 2: install a `BatchInsertPauseHook` fired between
    /// `insert_many`'s first and second loop iteration, so a regression test
    /// can drive a real `drain_once` into the stall-across-`.await` gap.
    #[cfg(test)]
    pub(crate) fn set_batch_pause_hook(
        &self,
        hook: Option<Arc<crate::membuffer_clear_race_hook::BatchInsertPauseHook>>,
    ) {
        self.state.batch_pause_hook.store(hook);
    }

    /// RV-STORAGE-98f6650d test seam: install a `FillPauseHook` fired by
    /// clean fills at the final publication boundary (right before the
    /// physical moka insertion).
    #[cfg(test)]
    pub(crate) fn set_fill_pause_hook(&self, hook: Option<Arc<FillPauseHook>>) {
        self.state.fill_pause_hook.store(hook);
    }

    fn new_inner(
        inner: Arc<dyn Store>,
        config: MemBufferConfig,
        #[cfg(test)] clear_race_hook: crate::membuffer_clear_race_hook::ClearRaceHook,
    ) -> Self {
        let dirty: Arc<DashMap<RecordKey, Slot, THasher>> =
            Arc::new(DashMap::with_hasher(THasher::default()));
        let cache = Arc::new(build_cache(&config));

        let state = Arc::new(MemBufferState {
            cache: ArcSwap::from(cache),
            dirty,
            revisions: StripedRevisions::new(),
            dirty_count: AtomicUsize::new(0),
            max_bytes: AtomicUsize::new(config.max_bytes),
            max_entries: AtomicUsize::new(config.max_entries),
            ttl_ms: AtomicU64::new(config.ttl_ms.unwrap_or(0)),
            flush_interval_ms: AtomicU64::new(config.flush_interval_ms),
            flush_batch_size: AtomicUsize::new(config.flush_batch_size),
            flush_errors: AtomicU64::new(0),
            #[cfg(test)]
            clear_race_hook,
            #[cfg(test)]
            batch_pause_hook: arc_swap::ArcSwapOption::empty(),
            #[cfg(test)]
            fill_pause_hook: arc_swap::ArcSwapOption::empty(),
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
                // Give moka a maintenance tick (TTL + capacity eviction).
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
    /// needed (as tagged clean fills).
    pub async fn apply_config(&self, cfg: &MemBufferConfig) -> DbResult<()> {
        // Drain BEFORE swap — this forces every pending write to `inner`
        // before the cache instance changes.
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

        // Build + swap the new cache (same tagged value type).
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

    /// Read the `dirty_count` fast-path cardinality mirror as a boolean
    /// ("is dirty observably non-empty"). For tests only (#535 / #539).
    #[cfg(test)]
    pub(crate) fn dirty_nonempty_flag(&self) -> bool {
        self.state.dirty_count.load(Ordering::Acquire) > 0
    }

    /// Task #535 test seam (updated #539 for the counter redesign): perform
    /// the SYNCHRONOUS writer publish+insert used by the clear-race
    /// regression test's hook to simulate a writer whose insert lands in the
    /// drainer's clear window, WITHOUT touching the moka cache or notifying
    /// the flusher.
    #[cfg(test)]
    pub(crate) fn inject_racing_dirty_write(&self, key: RecordKey, value: Bytes) {
        if self.state.dirty.insert(key, Slot::Live(value)).is_none() {
            self.state.dirty_count.fetch_add(1, Ordering::Release);
        }
    }

    /// Task #535 test seam: run a SINGLE `drain_once` pass (not the drain-loop
    /// `flush`/`drain_all` runs), so a regression test can observe the buffer
    /// state at the instant right after ONE drain — while a hook-injected entry
    /// is still in `dirty` and has NOT yet reached `inner`.
    #[cfg(test)]
    pub(crate) async fn drain_once_for_test(&self, batch_size: usize) -> DbResult<usize> {
        Self::drain_once(&self.state, self.inner.as_ref(), batch_size).await
    }

    /// Task #535 round 2 test seam: pre-empt the background flusher task
    /// before it ever runs. Call this IMMEDIATELY after construction, with
    /// no intervening `.await` — the flusher's loop checks `shutdown` at the
    /// very top, before its first `select!`, so setting this before the
    /// flusher gets its first poll guarantees it never drains anything and
    /// can never race with a test that deliberately drives `drain_once`
    /// manually via [`drain_once_for_test`].
    #[cfg(test)]
    pub(crate) fn disable_background_flusher_for_test(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Test seam: force-evict `key` from the moka cache.
    #[cfg(test)]
    pub(crate) async fn evict_from_cache_for_test(&self, key: &RecordKey) {
        self.state.cache.load().invalidate(key).await;
    }

    /// Drain up to `batch_size` dirty entries into the inner
    /// store. Snapshot-then-`remove_if` pattern protects against
    /// races where a concurrent write to the same key shouldn't
    /// be lost.
    ///
    /// RV-STORAGE-98f6650d: stripe guards are acquired for every snapshot
    /// key (deduped, ascending stripe order — deadlock-free) BEFORE the
    /// backing writes and held across BOTH the `inner.set_many`/
    /// `inner.remove_many` round-trip AND the conditional `remove_if`
    /// dirty removal, so a concurrent writer's epoch (revision bump) can
    /// never interleave with the drain in a way that lets a stale physical
    /// cache entry validate; and any writer landing after the drain's
    /// release necessarily bumped the revision, invalidating pre-drain
    /// fills.
    async fn drain_once(
        state: &Arc<MemBufferState>,
        inner: &dyn Store,
        batch_size: usize,
    ) -> DbResult<usize> {
        if state.dirty_count.load(Ordering::Acquire) == 0 {
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

        // RV-STORAGE-98f6650d: hold a stripe guard per snapshot key across
        // the backing write AND the conditional dirty removal below. Guards
        // are pure CAS epochs (never blocking), so acquisition order cannot
        // deadlock; ordering by stripe keeps the CAS contention local.
        let mut by_stripe: Vec<RecordKey> = snapshots.iter().map(|(k, _)| k.clone()).collect();
        by_stripe.sort_by(|a, b| {
            state
                .revisions
                .stripe_of(a)
                .cmp(&state.revisions.stripe_of(b))
        });
        let _guards: Vec<super::revision::StripeGuard<'_>> =
            by_stripe.iter().map(|k| state.revisions.begin(k)).collect();

        let n = sets.len() + removes.len();
        if !sets.is_empty() {
            inner.set_many(sets).await?;
        }
        if !removes.is_empty() {
            inner.remove_many(removes).await?;
        }

        // Test seam (#535/#539): fire the racing-writer-insert hook HERE, in
        // the same spot rounds 1/2 used to reproduce their interleavings.
        #[cfg(test)]
        state.clear_race_hook.at_clear_window();
        for (k, snapshot) in snapshots {
            if state
                .dirty
                .remove_if(&k, |_, current| *current == snapshot)
                .is_some()
            {
                state.dirty_count.fetch_sub(1, Ordering::Release);
            }
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
    pub(super) fn snapshot_overlay_sorted<F>(
        state: &MemBufferState,
        pred: F,
    ) -> Vec<(RecordKey, Slot)>
    where
        F: Fn(&RecordKey) -> bool,
    {
        // Fast-path: if the dirty buffer is (observably) empty the scan is a
        // pure `inner` pass-through — skip the DashMap traversal + sort.
        if state.dirty_count.load(Ordering::Acquire) == 0 {
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
    pub(super) async fn drain_all(&self) -> DbResult<()> {
        loop {
            let drained = Self::drain_once(&self.state, self.inner.as_ref(), usize::MAX).await?;
            if drained == 0 {
                break;
            }
        }
        // Defensive second pass (moka maintenance may have run in between).
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
