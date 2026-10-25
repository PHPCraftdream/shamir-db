use super::core::Slot;
use super::revision::StripeGuard;
use super::{MemBufferConfig, MemBufferStore};
use crate::error::{DbError, DbResult};
use crate::types::{merge_overlay_stream, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::Ordering;
use std::sync::Arc;

impl MemBufferStore {
    /// Route all dirty-mutation paths through one guard helper: every
    /// publication holds the key's stripe epoch (revision bump + active
    /// count) so a concurrent clean fill can never validate an entry
    /// published before it.
    fn guard_for(&self, key: &RecordKey) -> StripeGuard<'_> {
        self.state.revisions.begin(key)
    }
}

#[async_trait]
impl Store for MemBufferStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        let id = RecordId::new();
        let key = RecordKey::from_slice(id.as_bytes());
        let slot = Slot::Live(value);
        // RV-STORAGE-98f6650d: the fresh-key guard bypass is REMOVED —
        // `insert` takes the stripe epoch like every other dirty mutation,
        // bumping the revision so any in-flight/pre-existing cache entry
        // for a colliding stripe key cannot validate against this write.
        let _g = self.guard_for(&key);
        // Task #539: `key` is a freshly-generated `RecordId`, so it can never
        // already be in `dirty` — the increment is unconditional.
        self.state.dirty.insert(key.clone(), slot);
        self.state.dirty_count.fetch_add(1, Ordering::Release);
        // RV-STORAGE-98f6650d: NO writer-side cache republish — dirty is
        // the authority; the cache is only ever populated by validated
        // clean fills.
        self.notify.notify_one();
        Ok(key)
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        // `bool` return = was the key created (vs updated)?
        // Best-effort: authoritative dirty lookup + validated cache probe;
        // a miss falls through to `false` (presumed new). shamir-engine
        // doesn't depend on strict bool.
        let existed = self.state.existed_hint(&key).await;
        // RV-STORAGE-98f6650d: stripe epoch held across the dirty
        // publication; revision bump invalidates any pre-write cache entry.
        let _g = self.guard_for(&key);
        // Task #539: increment `dirty_count` ONLY when `key` was genuinely
        // new to `dirty` (`insert()` returned `None`) — an overwrite of an
        // already-dirty key must not double-count.
        if self.state.dirty.insert(key, Slot::Live(value)).is_none() {
            self.state.dirty_count.fetch_add(1, Ordering::Release);
        }
        // No writer-side cache republish (RV-STORAGE-98f6650d).
        self.notify.notify_one();
        Ok(!existed)
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        // RV-STORAGE-98f6650d read order: capture the stripe stamp FIRST,
        // then ALWAYS probe dirty directly (dirty_count is never correctness
        // evidence) — dirty is the authority.
        let s0 = self.state.revisions.stamp(&key);
        if let Some(slot) = self.state.dirty_probe(&key) {
            return match slot {
                Slot::Live(v) => Ok(v),
                Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
            };
        }
        // Not dirty — validated cache probe against the stamp captured
        // BEFORE the dirty probe (a writer that landed in the interim must
        // invalidate the entry, not validate it).
        if let Some(slot) = self.state.validated_cache_get(&key, s0).await {
            return match slot {
                Slot::Live(v) => Ok(v),
                Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
            };
        }
        // Capture the fallback stamp BEFORE the authoritative dirty re-probe:
        // a fill from the backing lookup below must be tagged with a stamp
        // no later than the probe it shadows, so a writer landing between
        // the probe and a (re)capture cannot get its own value tagged as
        // pre-write.
        let s1 = self.state.revisions.stamp(&key);
        // Authoritative dirty re-probe: a writer may have landed since the
        // first probe (or the cache entry was rejected as stale).
        if let Some(slot) = self.state.dirty_probe(&key) {
            return match slot {
                Slot::Live(v) => Ok(v),
                Slot::Tombstone => Err(DbError::NotFound(format!("{:?}", key))),
            };
        }
        // Fall through to inner; populate cache only (NOT dirty —
        // this is a clean read-fill).
        //
        // RV-STORAGE-98f6650d: the per-key stamp (`s1`) was captured BEFORE
        // the authoritative re-probe and hence before the backing lookup;
        // the fill is tagged with it regardless of any intervening writes —
        // every future hit validates the tag, so a stale physical entry is
        // harmless (rejected on its next hit).
        let result = self.inner.get(key.clone()).await;
        match &result {
            Ok(v) => {
                self.state
                    .clean_fill(&key, s1.raw, Slot::Live(v.clone()))
                    .await;
            }
            Err(DbError::NotFound(_)) => {
                self.state.clean_fill(&key, s1.raw, Slot::Tombstone).await;
            }
            Err(_) => {}
        }
        result
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        // Best-effort bool — see `set` for rationale.
        let existed = self.state.existed_hint(&key).await;
        // RV-STORAGE-98f6650d: stripe epoch across the publication.
        let _g = self.guard_for(&key);
        // Task #539 — increment only on a genuinely-new key.
        if self.state.dirty.insert(key, Slot::Tombstone).is_none() {
            self.state.dirty_count.fetch_add(1, Ordering::Release);
        }
        // No writer-side cache republish (RV-STORAGE-98f6650d).
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

    /// Delegate to inner store's `transact`, then update dirty state for all
    /// touched keys. The buffer layer doesn't add atomicity — that comes
    /// from the inner backend.
    ///
    /// **Audit §2.3:** the old code removed dirty entries UNCONDITIONALLY
    /// after `inner.transact`; a concurrent `set(k)` landing in the window
    /// was lost. The fix mirrors `drain_once`'s snapshot + `remove_if`
    /// pattern: only remove an entry if it still holds the snapshot this
    /// transaction flushed.
    ///
    /// RV-STORAGE-98f6650d: stripe guards (ascending stripe order —
    /// deadlock-free) are held across the inner transact AND the
    /// conditional dirty cleanup; no writer-side cache republish happens
    /// (the transacted state reaches readers via dirty removal → backing).
    async fn transact(&self, ops: Vec<crate::types::KvOp>) -> DbResult<()> {
        if ops.is_empty() {
            return Ok(());
        }
        // Drain any pending dirty entries for the affected keys first
        // so the inner store has a consistent view before transact.
        self.drain_all().await?;

        // Acquire all touched keys' stripe guards, ascending stripe order.
        let mut touched: Vec<RecordKey> = ops
            .iter()
            .map(|op| match op {
                crate::types::KvOp::Set(k, _) => k.clone(),
                crate::types::KvOp::Remove(k) => k.clone(),
            })
            .collect();
        touched.sort_by(|a, b| {
            self.state
                .revisions
                .stripe_of(a)
                .cmp(&self.state.revisions.stripe_of(b))
        });
        let guards: Vec<StripeGuard<'_>> = touched.iter().map(|k| self.guard_for(k)).collect();

        // Delegate to inner's native transact.
        self.inner.transact(ops.clone()).await?;

        // Conditional dirty cleanup — same snapshot + `remove_if` pattern
        // as `drain_once` (Audit §2.3 / task #539 inline decrement rule).
        for op in ops {
            let (k, slot): (RecordKey, Slot) = match op {
                crate::types::KvOp::Set(k, v) => (k, Slot::Live(v)),
                crate::types::KvOp::Remove(k) => (k, Slot::Tombstone),
            };
            if self
                .state
                .dirty
                .remove_if(&k, |_, current| *current == slot)
                .is_some()
            {
                self.state.dirty_count.fetch_sub(1, Ordering::Release);
            }
        }
        drop(guards);
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
        #[cfg(test)]
        let mut first_iteration = true;
        for v in values {
            let id = RecordId::new();
            let key = RecordKey::from_slice(id.as_bytes());
            let slot = Slot::Live(v);
            // RV-STORAGE-98f6650d: fresh-key guard bypass REMOVED — every
            // item takes its stripe epoch across the publication.
            let _g = self.guard_for(&key);
            // Task #539: `key` is a freshly-generated `RecordId` — always
            // new to `dirty`, so the increment is unconditional and inline
            // with the insert.
            self.state.dirty.insert(key.clone(), slot);
            self.state.dirty_count.fetch_add(1, Ordering::Release);
            keys.push(key);
            // Task #535 round 2 test seam: park here (after the FIRST item
            // is fully committed — dirty+count done) so a regression test
            // can drive a real `drain_once` into the exact
            // stall-across-`.await` gap, before the NEXT item's
            // `dirty.insert` runs. No-op in production.
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
        // Batched mirror of `set` for each (k,v) — same dirty-only
        // publication and best-effort `existed` semantics, with a SINGLE
        // notify_one at the end. Last-write-wins within the batch on
        // duplicate keys.
        let mut flags = Vec::with_capacity(items.len());
        // RV-STORAGE-98f6650d: stripe guards for all batch keys, ascending
        // stripe order (deadlock-free), held across the whole publication.
        let mut unique: Vec<RecordKey> = items.iter().map(|(k, _)| k.clone()).collect();
        unique.sort();
        unique.dedup();
        unique.sort_by(|a, b| {
            self.state
                .revisions
                .stripe_of(a)
                .cmp(&self.state.revisions.stripe_of(b))
        });
        let guards: Vec<StripeGuard<'_>> = unique.iter().map(|k| self.guard_for(k)).collect();
        for (k, v) in items {
            let existed = self.state.existed_hint(&k).await;
            // Task #539 — increment inline, only on a genuinely-new key.
            // A duplicate key within the same batch correctly increments at
            // most once.
            if self.state.dirty.insert(k, Slot::Live(v)).is_none() {
                self.state.dirty_count.fetch_add(1, Ordering::Release);
            }
            flags.push(!existed);
        }
        drop(guards);
        self.notify.notify_one();
        Ok(flags)
    }

    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // Batched mirror of `remove` — see `set_many` for the batching
        // shape. Tombstones replace Live slots; ONE notify_one.
        let mut flags = Vec::with_capacity(keys.len());
        // RV-STORAGE-98f6650d: same guard shape as `set_many`.
        let mut unique: Vec<RecordKey> = keys.clone();
        unique.sort();
        unique.dedup();
        unique.sort_by(|a, b| {
            self.state
                .revisions
                .stripe_of(a)
                .cmp(&self.state.revisions.stripe_of(b))
        });
        let guards: Vec<StripeGuard<'_>> = unique.iter().map(|k| self.guard_for(k)).collect();
        for k in keys {
            let existed = self.state.existed_hint(&k).await;
            // Task #539 — increment inline, only on a genuinely-new key.
            if self.state.dirty.insert(k, Slot::Tombstone).is_none() {
                self.state.dirty_count.fetch_add(1, Ordering::Release);
            }
            flags.push(existed);
        }
        drop(guards);
        self.notify.notify_one();
        Ok(flags)
    }

    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // RV-STORAGE-98f6650d read order, per key: capture the stripe stamp
        // BEFORE the dirty lookup, ALWAYS probe dirty directly first (it is
        // the authority; `dirty_count` is never correctness evidence), then
        // the validated cache probe against that pre-dirty stamp; remaining
        // misses capture their fallback stamp BEFORE the authoritative
        // second dirty probe and carry it (per key, paired with the key)
        // all the way to ONE batched `inner.get_many`, whose results are
        // tagged with the carried stamps — stamps are NEVER re-captured
        // after the key loop, because a writer landing between a key's
        // dirty probe and a later bulk stamp capture would get its own
        // acknowledged value tagged as pre-write (stale-but-fresh-looking).
        let mut out: Vec<Option<Bytes>> = Vec::with_capacity(keys.len());
        let mut miss_idxs: Vec<usize> = Vec::new();
        let mut miss_keys: Vec<RecordKey> = Vec::new();
        let mut miss_stamps: Vec<u64> = Vec::new();
        for (i, k) in keys.into_iter().enumerate() {
            let s0 = self.state.revisions.stamp(&k);
            // Dirty FIRST — authoritative.
            if let Some(slot) = self.state.dirty_probe(&k) {
                match slot {
                    Slot::Live(v) => out.push(Some(v)),
                    Slot::Tombstone => out.push(None),
                }
                continue;
            }
            // Validated cache probe (caller's pre-dirty stamp vs post-await).
            if let Some(slot) = self.state.validated_cache_get(&k, s0).await {
                match slot {
                    Slot::Live(v) => out.push(Some(v)),
                    Slot::Tombstone => out.push(None),
                }
                continue;
            }
            // Fallback stamp BEFORE the authoritative dirty re-probe.
            let s1 = self.state.revisions.stamp(&k);
            if let Some(slot) = self.state.dirty_probe(&k) {
                match slot {
                    Slot::Live(v) => out.push(Some(v)),
                    Slot::Tombstone => out.push(None),
                }
                continue;
            }
            // Defer to the single inner.get_many, carrying the stamp
            // captured before that probe.
            out.push(None);
            miss_idxs.push(i);
            miss_keys.push(k);
            miss_stamps.push(s1.raw);
        }
        if !miss_keys.is_empty() {
            let inner_vals = self.inner.get_many(miss_keys.clone()).await?;
            for ((i, k), (raw, v)) in miss_idxs
                .into_iter()
                .zip(miss_keys)
                .zip(miss_stamps.into_iter().zip(inner_vals))
            {
                // Tagged clean fill (NOT dirty). Tombstone negative result so
                // subsequent gets short-circuit.
                let slot = match &v {
                    Some(b) => Slot::Live(b.clone()),
                    None => Slot::Tombstone,
                };
                self.state.clean_fill(&k, raw, slot).await;
                out[i] = v;
            }
        }
        Ok(out)
    }
}
