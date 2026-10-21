use super::{MemBufferConfig, MemBufferStore, Slot};
use crate::error::{DbError, DbResult};
use crate::types::{merge_overlay_stream, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[async_trait]
impl Store for MemBufferStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = RecordId::new();
        let key = RecordKey::from_slice(id.as_bytes());
        let slot = Slot::Live(value);
        // Task #539: `key` is a freshly-generated `RecordId`, so it can never
        // already be in `dirty` — the increment is unconditional (no
        // `insert()` return-value check needed, unlike `set`/`remove` where
        // the key is caller-supplied and may already be dirty).
        self.state.dirty.insert(key.clone(), slot.clone());
        self.state.dirty_count.fetch_add(1, Ordering::Release);
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
        // Task #539: increment `dirty_count` ONLY when `key` was genuinely
        // new to `dirty` (`insert()` returned `None`) — an overwrite of an
        // already-dirty key must not double-count.
        if self.state.dirty.insert(key.clone(), slot.clone()).is_none() {
            self.state.dirty_count.fetch_add(1, Ordering::Release);
        }
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
        // Fast-path (task #539): when `dirty_count` reads 0 (Acquire), dirty
        // is guaranteed empty — skip the DashMap probe entirely. Pairs with
        // the Release increment in insert/set/remove (and their `_many`
        // siblings) and the Release decrement in `drain_once`/`transact`.
        if self.state.dirty_count.load(Ordering::Acquire) > 0 {
            if let Some(entry) = self.state.dirty.get(&key) {
                return match entry.value() {
                    Slot::Live(v) => Ok(v.clone()),
                    Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
                };
            }
        }
        // Fall through to inner; populate cache only (NOT dirty —
        // this is a clean read-fill).
        //
        // Task #539 tombstone-poisoning guard: a first attempt at this fix
        // reasoned that a writer's post-insert cache republish (`insert`/
        // `set`/`remove` and their `_many` siblings all re-publish `cache`
        // immediately after their `dirty` write) would always clobber a
        // reader-cached stale negative, because the republish happens
        // "immediately after, same call, same thread". An adversarial review
        // caught the flaw: "immediately after" is only PROGRAM order within
        // the writer's own task — it gives no real-time ordering guarantee
        // relative to a DIFFERENT, concurrently-running reader task's own
        // later `cache.insert`. Concretely: a reader can observe a
        // transiently stale `dirty_count == 0` for a brand-new key K (before
        // a concurrent writer's `fetch_add` becomes visible), fall through to
        // `inner.get(K)` (a slow async I/O round-trip), and only THEN run its
        // own `cache.insert(K, Tombstone)` — by which point the writer's own
        // (much faster, pure in-memory) `dirty.insert` + cache republish may
        // already have completed. moka gives no ordering between two
        // independent tasks' inserts to the same key beyond "last physical
        // write wins", so the reader's stale Tombstone can land AFTER the
        // writer's Live republish, masking the write on every subsequent
        // `get()` (which hits the cache-first branch above, never reaching
        // the `dirty_count`/`dirty` probe) until that cache entry is evicted
        // or overwritten — a LASTING mask, not a fleeting one.
        //
        // Fix: immediately before caching this call's `inner`-fetched result,
        // re-check `dirty` for the key. If a concurrent writer has landed an
        // entry in the meantime, `dirty` is authoritative over our stale
        // `inner` read — skip the cache-fill entirely rather than risk
        // clobbering the writer's republish. This call's own RETURN VALUE can
        // still be the stale answer (unavoidable without full
        // linearizability — no different from any lock-free store's
        // in-flight-write-vs-concurrent-read race, and #539's brief accepts
        // this as a fleeting-miss trade-off), but nothing is cached to make
        // that staleness outlive this one call: the very next `get()` on this
        // key will cache-miss again, see `dirty_count > 0`, and correctly
        // return the writer's value from `dirty`.
        //
        // Honest scope of what this closes: it collapses the race window from
        // "the full duration of this call's `inner.get()` I/O round-trip"
        // (large, practically hittable under real concurrent load) down to
        // "between this recheck and the `cache.insert` call below". That
        // residual window is NOT merely a handful of in-memory instructions —
        // `cache.insert` is itself `async fn` and moka may yield internally
        // (per-shard buffer/eviction-policy maintenance), so a writer can
        // still land its `dirty` write + cache republish while this task is
        // suspended at that `.await`, same as any other scheduler-dependent
        // gap. This is a genuine, still-open (if much narrower) residual, not
        // a claim of full closure — accepted for the same reason this
        // cache's other known staleness windows are (see `build_cache`'s
        // "purely a read accelerator" doc comment). Closing it fully would
        // require holding a per-key lock across the recheck+insert pair,
        // which this file deliberately avoids on the hot read path.
        let result = self.inner.get(key.clone()).await;
        let slot_to_insert = match &result {
            Ok(v) => Some(Slot::Live(v.clone())),
            Err(DbError::NotFound(_)) => Some(Slot::Tombstone),
            Err(_) => None,
        };
        if let Some(slot) = slot_to_insert {
            let raced = self.state.dirty_count.load(Ordering::Acquire) > 0
                && self.state.dirty.get(&key).is_some();
            if !raced {
                cache.insert(key, slot).await;
            }
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
        // Task #539 — see `set` for the increment-only-on-genuinely-new-key
        // rationale.
        if self
            .state
            .dirty
            .insert(key.clone(), Slot::Tombstone)
            .is_none()
        {
            self.state.dirty_count.fetch_add(1, Ordering::Release);
        }
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

    /// Drain the buffer into the inner store WITHOUT the inner
    /// `flush()` — the shutdown flush pass drains every store first and
    /// then persists the backend ONCE (`Repo::persist_once`); calling the
    /// inner flush here would reintroduce one full-database fsync per
    /// buffered store. The inner drain is recursive — a `MemBuffer` stacked
    /// over another wrapper drains the whole chain; fjall's no-op
    /// `flush_drain` terminates it.
    async fn flush_drain(&self) -> DbResult<()> {
        self.drain_all().await?;
        self.inner.flush_drain().await
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
    async fn transact(&self, ops: Vec<crate::types::KvOp>) -> DbResult<()> {
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
                crate::types::KvOp::Set(k, v) => {
                    let slot = Slot::Live(v);
                    cache.insert(k.clone(), slot.clone()).await;
                    // Audit §2.3: remove from dirty ONLY if the slot is
                    // still the one we just wrote — a concurrent `set(k)`
                    // that landed between `inner.transact` and here put a
                    // NEWER value into dirty; removing it would lose that
                    // write (the old unconditional `remove` did exactly
                    // this). `remove_if` with a value comparison is the
                    // same pattern `drain_once` uses.
                    //
                    // Task #539: decrement `dirty_count` inline with the
                    // actual removal (only when `remove_if` really removed
                    // something) — the same rule as `drain_once`.
                    if self
                        .state
                        .dirty
                        .remove_if(&k, |_, current| *current == slot)
                        .is_some()
                    {
                        self.state.dirty_count.fetch_sub(1, Ordering::Release);
                    }
                }
                crate::types::KvOp::Remove(k) => {
                    let slot = Slot::Tombstone;
                    cache.insert(k.clone(), slot.clone()).await;
                    // Audit §2.3: same guarded removal as the Set branch.
                    // Task #539: same inline decrement rule as the Set branch.
                    if self
                        .state
                        .dirty
                        .remove_if(&k, |_, current| *current == slot)
                        .is_some()
                    {
                        self.state.dirty_count.fetch_sub(1, Ordering::Release);
                    }
                }
            }
        }
        Ok(())
    }

    /// Delegates to `inner` — the buffer layer drains its dirty overlay
    /// into `inner` and then delegates the batch to `inner.transact`,
    /// so the atomicity is whatever the inner backend provides. F-77
    /// (#904).
    fn supports_atomic_transact(&self) -> bool {
        self.inner.supports_atomic_transact()
    }

    async fn insert_many(&self, values: Vec<Bytes>) -> DbResult<Vec<RecordKey>> {
        let mut keys = Vec::with_capacity(values.len());
        let cache = self.state.cache.load();
        #[cfg(test)]
        let mut first_iteration = true;
        for v in values {
            let id = RecordId::new();
            let key = RecordKey::from_slice(id.as_bytes());
            let slot = Slot::Live(v);
            // Task #539: `key` is a freshly-generated `RecordId` — always
            // new to `dirty`, so the increment is unconditional and happens
            // AT THE SAME LOGICAL POINT as the insert (inline, per-item, not
            // hoisted before the loop as a single before/after `store` the
            // old `dirty_nonempty` boolean used). Every item is individually
            // accounted for the instant it lands, so a concurrent
            // `drain_once` racing between iterations can never observe a
            // count of 0 while an item this loop already inserted is still
            // present.
            self.state.dirty.insert(key.clone(), slot.clone());
            self.state.dirty_count.fetch_add(1, Ordering::Release);
            cache.insert(key.clone(), slot).await;
            keys.push(key);
            // Task #535 round 2 test seam: park here (after the FIRST item
            // is fully committed — dirty+cache+republish done) so a
            // regression test can drive a real `drain_once` into the exact
            // stall-across-`.await` gap the round-2 fix closes, before the
            // NEXT item's `dirty.insert` runs. No-op in production (the
            // hook is `None` unless a test installs one).
            #[cfg(test)]
            if first_iteration {
                first_iteration = false;
                if let Some(hook) = self.state.batch_pause_hook.load_full() {
                    hook.wait_after_first_item().await;
                }
            }
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
            // Task #539 — increment inline, only on a genuinely-new key;
            // see `set` for the rationale. A duplicate key within the same
            // batch correctly increments at most once (the second
            // occurrence's `insert()` returns `Some`, the first one's value).
            if self.state.dirty.insert(k.clone(), slot.clone()).is_none() {
                self.state.dirty_count.fetch_add(1, Ordering::Release);
            }
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
            // Task #539 — increment inline, only on a genuinely-new key;
            // see `set_many` above / `remove` for the rationale.
            if self
                .state
                .dirty
                .insert(k.clone(), Slot::Tombstone)
                .is_none()
            {
                self.state.dirty_count.fetch_add(1, Ordering::Release);
            }
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
                .zip(miss_keys_for_fill)
                .zip(inner_vals)
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
