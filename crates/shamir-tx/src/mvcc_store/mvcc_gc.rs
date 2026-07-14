use std::sync::atomic::Ordering;

use bytes::{BufMut, Bytes, BytesMut};
use futures::StreamExt;
use shamir_collections::TFxMap;
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;

use super::ts_key;
use super::MvccStore;
use crate::version_codec::encode_version_key;

impl MvccStore {
    /// T1b.2 + T1c: per-key retention-aware eager vacuum. After a
    /// write/delete to `key`, reclaim that key's OLD history versions that
    /// BOTH the count cap AND the age cap agree to drop, subject to the
    /// `min_count` floor and the snapshot-safety invariants.
    ///
    /// Retention model (orthogonal knobs):
    /// * `max_count` — COUNT cap: keep at most N old versions per key.
    /// * `max_age_secs` — AGE cap: reclaim versions older than this (using
    ///   the per-version commit timestamp recorded by [`Self::record_ts`]).
    ///   A version with no recorded ts is treated as "unknown age" and
    ///   conservatively KEPT by the age axis.
    /// * `min_count` — COUNT floor: always keep ≥ M newest old versions,
    ///   EVEN IF older than `max_age_secs`. This is `min_count`'s real job —
    ///   protect recent versions from the age cap.
    ///
    /// If BOTH `max_count` and `max_age_secs` are `None` (no upper bound on
    /// either axis), there is nothing to reclaim → early return. Otherwise a
    /// version is reclaimed only when ALL applicable caps drop it (modulo the
    /// floor + snapshot invariants).
    ///
    /// Sacred floor (NEVER violated): a version `>= min_alive` (pinned by a
    /// live snapshot) is never reclaimed regardless of any knob.
    ///
    /// Anchor: when a live snapshot exists below `current`, the SINGLE largest
    /// version `< min_alive` is also kept — it serves a snapshot reading a key
    /// last-written below `min_alive`. When no live snapshot exists, no anchor
    /// is needed: a fresh snapshot opens at `current` and reads the log directly.
    ///
    /// When a version is reclaimed, its `ts_key(version)` entry is also removed
    /// (no orphan timestamps). Best-effort: errors are swallowed (a vacuum
    /// failure must NOT fail the write that triggered it; the next write
    /// retries).
    /// L6 targeted-remove fast path parameter: `old_v` is the key's version
    /// BEFORE the current write (`current_version(key)` captured before
    /// `publish_cell`). `0` means append-only (no prior version) → vacuum is
    /// a no-op.
    pub(super) async fn vacuum_key(&self, key: &[u8], old_v: u64) {
        let policy = self.retention();
        // No upper bound on either axis → nothing to reclaim.
        if policy.max_count.is_none() && policy.max_age_secs.is_none() {
            return;
        }

        // L6 fast path: CurrentOnly retention + no live snapshots + no
        // accumulated old versions from a prior snapshot epoch + known prior
        // version → deferred-anchor remove of old versions, no scan_prefix.
        //
        // A10 TOCTOU fix — PRIMARY mechanism: in-flight barrier.
        // `gate.snapshots_opening()` returns true if any `open_snapshot` call
        // has begun (incremented `active_snapshots_opening`) but not yet
        // completed registration in `active_snapshots`. While this is true,
        // the fast path MUST NOT physically delete ANY version — a reader is
        // mid-registration and its floor version is not yet visible in
        // `active_snapshots`. This closes the race for an UNBOUNDED number
        // of writer cycles: no matter how many writes happen while the reader
        // is stalled, vacuum defers all deletion until the opener completes.
        //
        // A10 TOCTOU fix — SECONDARY mechanism: anchor deferral.
        // The fast path does NOT delete `old_v` in the SAME call — it stores
        // `old_v` as a deferred anchor in the cell's `vacuum_anchor` field and
        // only physically deletes the PREVIOUSLY-deferred anchor (if any).
        // This one-generation slack is an additional safety net.
        //
        // Invariants:
        //  • old_v == 0 ⇒ append-only (first write), nothing to defer.
        //  • active_snapshots_empty() ⇒ no snapshot floor to protect.
        //  • !snapshots_opening() ⇒ no reader mid-registration (barrier clear).
        //  • !vacuum_needs_scan ⇒ no accumulated versions from snapshot epochs.
        //  • is_current_only() ⇒ max_count == Some(0), no age axis, no min_count
        //    floor — every old version must go (eventually, after deferral).
        let snapshots_empty = self.gate.active_snapshots_empty();
        let openings_in_flight = self.gate.snapshots_opening();
        if policy.is_current_only()
            && snapshots_empty
            && !openings_in_flight
            && !self.vacuum_needs_scan.load(Ordering::Acquire)
        {
            if old_v == 0 {
                return; // append-only, no prior version
            }
            // A10: atomically swap `old_v` into the cell's `vacuum_anchor`,
            // retrieving the PREVIOUSLY-deferred version (if any). The
            // previously-deferred version is now ONE generation stale — safe
            // to physically delete (the barrier guarantees no reader is
            // mid-registration that could need it). `old_v` itself is NOT
            // deleted yet — it stays as the new deferred anchor.
            let prev_anchor = self.swap_vacuum_anchor(key, old_v);
            if let Some(deferred) = prev_anchor {
                if deferred != old_v && deferred > 0 {
                    // Audit 2.1: capture the commit ts BEFORE we remove the
                    // ts_key, so the ts-index entry can be pruned in lockstep.
                    let ts = self.lookup_ts(deferred).await;
                    // Physically delete the previously-deferred version.
                    let _ = self
                        .history
                        .remove_no_flag(encode_version_key(key, deferred).into())
                        .await;
                    let _ = self.history.remove_no_flag(ts_key(deferred).into()).await;
                    if let Some(ts) = ts {
                        self.ts_index_remove(ts, deferred);
                    }
                    // P1c: drop from overlay in lockstep.
                    // D2 P1d-2b: gate on durable_watermark.
                    if deferred <= self.gate.durable_watermark() {
                        self.overlay.remove(key, deferred);
                    }
                }
            }
            return;
        }

        // A10 barrier active: a reader is mid-registration. We can still
        // update the anchor bookkeeping (swap old_v in), but MUST skip the
        // physical deletion of the previously-deferred version — the stalled
        // reader's floor could be that old anchor. The deferred version(s)
        // will accumulate slightly longer until the next vacuum_key call after
        // the in-flight opener clears. This is safe over-retention.
        if policy.is_current_only()
            && snapshots_empty
            && openings_in_flight
            && !self.vacuum_needs_scan.load(Ordering::Acquire)
            && old_v != 0
        {
            // Update bookkeeping only — no physical deletes.
            let _ = self.swap_vacuum_anchor(key, old_v);
            return;
        }

        // L6: if a snapshot is active, set the flag so the targeted fast path
        // does not fire until a full scan vacuum cleans up accumulated versions.
        if !snapshots_empty {
            self.vacuum_needs_scan.store(true, Ordering::Release);
        }

        let max_count = policy.max_count.map(|m| m as usize);
        let min_count = policy.min_count.unwrap_or(0) as usize;
        // Age cutoff in millis (None = no age cap). Saturating mul in case of
        // an absurd config.
        let age_cutoff_ms: Option<u64> = policy
            .max_age_secs
            .map(|s| s.saturating_mul(1000))
            .map(|ms| self.now_millis().saturating_sub(ms));
        let min_alive = self.gate.min_alive();
        let have_live_snapshot = !self.gate.active_snapshots_empty();

        // Scan this key's history entries (prefix scan on the version-key
        // encoding `key || 0xFF || version_be`). The prefix naturally excludes
        // ts-keys (which start with TS_TAG = 0x00, not the record key).
        let prefix = {
            let mut p = BytesMut::with_capacity(key.len() + 1);
            p.extend_from_slice(key);
            p.put_u8(crate::version_codec::VERSION_SEP);
            p.freeze()
        };
        let stream = self.history.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        // Collect (version, physical_key) for all history entries of this key.
        let mut entries: Vec<(u64, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (phys_key, _val) in batch.unwrap_or_default() {
                if let Some((_, version)) = crate::version_codec::decode_version_key(&phys_key) {
                    // Boundary: store keys are `RecordKey`; this local entries
                    // vec is `Bytes` (byte-identical conversion).
                    entries.push((version, phys_key.into()));
                }
            }
        }

        // C1: the current version lives in the same log that vacuum scans.
        // It is SACRED — reclaiming it would erase live data.
        let cur_v = self.current_version(key);

        // Sort descending by version (newest first) so `idx` ranks by recency.
        entries.sort_by(|a, b| b.0.cmp(&a.0));

        // The anchor: the SINGLE largest version `< min_alive`, kept ONLY when
        // a live snapshot exists. If already kept by the min_count/count
        // window, no extra entry is kept.
        let anchor: Option<u64> = if have_live_snapshot {
            entries
                .iter()
                .map(|(v, _)| *v)
                .filter(|v| *v < min_alive)
                .max()
        } else {
            None
        };

        // Reclaim logic: iterate newest-first. A version is reclaimed only if
        // ALL caps agree to drop it (and the snapshot invariants don't protect
        // it). Concretely, reclaim iff:
        //   idx >= min_count                                  (floor keeps newest M)
        //   AND (max_count is None OR idx >= max_count)       (count cap drops it)
        //   AND (age_cutoff is None OR its ts < cutoff)       (age cap drops it;
        //                                                       unknown ts → keep)
        //   AND version < min_alive                           (sacred snapshot floor)
        //   AND Some(version) != anchor                       (single anchor)
        for (idx, (version, phys_key)) in entries.iter().enumerate() {
            // C1 SACRED: never reclaim the current version.
            if *version == cur_v {
                continue;
            }
            // (floor) min_count protects the newest M versions unconditionally.
            if idx < min_count {
                continue;
            }
            // (count cap) within the count window → keep.
            if let Some(mc) = max_count {
                if idx < mc {
                    continue;
                }
            }
            // (age cap) newer than the cutoff (or unknown ts) → keep.
            if let Some(cutoff) = age_cutoff_ms {
                let ts = self.lookup_ts(*version).await;
                match ts {
                    Some(t) if t < cutoff => { /* older than cutoff → age drops it */ }
                    _ => continue, // unknown ts OR within age window → keep
                }
            }
            // (sacred floor) pinned by a live snapshot → keep.
            if *version >= min_alive {
                continue;
            }
            // (anchor) the single anchor serving a live snapshot → keep.
            if Some(*version) == anchor {
                continue;
            }
            // All caps agree + not protected → reclaim the version AND its ts.
            // Audit 2.1: capture the ts first so the ts-index prunes in lockstep.
            let reclaimed_ts = self.lookup_ts(*version).await;
            let _ = self.history.remove_no_flag(phys_key.clone().into()).await;
            let _ = self.history.remove_no_flag(ts_key(*version).into()).await;
            if let Some(ts) = reclaimed_ts {
                self.ts_index_remove(ts, *version);
            }
            // P1c: drop the same (key, version) from the overlay in lockstep so
            // the overlay never serves a value that history just reclaimed.
            // D2 P1d-2b: but NEVER drop a version the drainer has not yet made
            // durable (`version > durable_watermark`) — post-cutover the
            // overlay holds the ONLY copy of an undrained version, so removing
            // it here would lose committed data. Such a version is also a
            // superseded-history candidate that simply isn't in history yet;
            // the real overlay-GC on durable_watermark advance is P1e.
            if *version <= self.gate.durable_watermark() {
                self.overlay.remove(key, *version);
            }
        }

        // L6: after a full scan-path vacuum with no live snapshots, clear the
        // flag so subsequent writes can use the targeted fast path.
        if snapshots_empty {
            self.vacuum_needs_scan.store(false, Ordering::Release);
        }
    }

    /// cancel-safe: NO — Phase 1 scans the history stream; Phase 2
    /// deletes per-key residuals; Phase 3 prunes the version cache.
    /// Cancellation during Phase 2/3 leaves some entries deleted and
    /// others not. GC is idempotent — a later `gc_below` resumes from
    /// current history/cache state — so eventual convergence is fine,
    /// but a single call is not atomic.
    ///
    /// Garbage-collect history entries with version < `min_version`.
    ///
    /// For each original key, keeps the LATEST version that is still
    /// < `min_version` (the "anchor" — needed so `get_at(snapshot)`
    /// can still find it for snapshots between anchor and min_version).
    /// All older versions of that key are removed.
    ///
    /// III.3: also prunes `version_cache`. The eviction threshold is the
    /// gate's `min_alive()` (the oldest live snapshot, or `last_committed`
    /// if none) — deliberately NOT the `min_version` argument, which only
    /// governs *history* GC and may be set higher than `min_alive` by a
    /// caller (a higher threshold would wrongly evict cache entries that a
    /// still-live snapshot below `min_version` needs to route to history).
    /// See [`Self::prune_version_cache`] for the full visibility argument.
    ///
    /// Returns the number of history entries deleted.
    ///
    /// T1c: ts-keys (`[TS_TAG][version_be]`) are transparently skipped during
    /// the scan — `decode_version_key` returns `None` for them (they're 9
    /// bytes with `TS_TAG = 0x00 != VERSION_SEP`). When a version is deleted,
    /// its `ts_key(version)` is also removed so timestamps don't outlive their
    /// versions.
    pub async fn gc_below(&self, min_version: u64) -> DbResult<usize> {
        use crate::version_codec::decode_version_key;

        // Phase 1: scan all history entries, group by original key.
        // ts-keys are skipped: decode_version_key returns None for them.
        let stream = self.history.iter_stream(MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        // Collect: original_key → Vec<(version, physical_key)>
        let mut per_key: TFxMap<Vec<u8>, Vec<(u64, Bytes)>> = TFxMap::default();

        while let Some(batch) = stream.next().await {
            for (phys_key, _value) in batch? {
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    if version < min_version {
                        per_key
                            .entry(orig.to_vec())
                            .or_default()
                            .push((version, phys_key.into()));
                    }
                }
            }
        }

        // Phase 2: for each key, sort by version, keep the latest (anchor),
        // delete the rest (+ each deleted version's ts-key).
        // C1: skip the current version — it is SACRED.
        let mut deleted = 0usize;
        for (orig_key, mut entries) in per_key {
            let cur_v = self.current_version(&orig_key);
            if entries.len() <= 1 {
                // Only one entry — it's the anchor, keep it.
                continue;
            }
            entries.sort_by_key(|(v, _)| *v);
            // Keep the last (highest version < min_version), delete the rest.
            let to_delete = &entries[..entries.len() - 1];
            for (version, phys_key) in to_delete {
                // C1 SACRED: never reclaim the current version.
                if *version == cur_v {
                    continue;
                }
                // Audit 2.1: capture ts before removal to prune ts-index in lockstep.
                let reclaimed_ts = self.lookup_ts(*version).await;
                let _ = self.history.remove_no_flag(phys_key.clone().into()).await;
                // T1c: remove the ts-key in lockstep so timestamps don't
                // outlive their versions.
                let _ = self.history.remove_no_flag(ts_key(*version).into()).await;
                if let Some(ts) = reclaimed_ts {
                    self.ts_index_remove(ts, *version);
                }
                // P1c: drop the same (key, version) from the overlay in lockstep.
                // D2 P1d-2b: gate on durable_watermark — never drop an undrained
                // version (the overlay holds its only copy until the drainer
                // lands it in history). Full overlay-GC is P1e.
                if *version <= self.gate.durable_watermark() {
                    self.overlay.remove(&orig_key, *version);
                }
                deleted += 1;
            }
        }

        // Phase 3: prune the in-memory version cache (III.3). Uses the
        // gate's `min_alive()`, independent of the `min_version` history
        // threshold (see `prune_version_cache` for why).
        self.prune_version_cache().await;

        Ok(deleted)
    }

    /// T4-purge: imperative one-shot history purge by a wall-clock
    /// timestamp predicate.
    ///
    /// Reclaims every archived history version whose recorded commit
    /// timestamp is strictly older than `cutoff_millis` — the
    /// imperative twin of retention [`vacuum_key`] (§3). Unlike
    /// vacuum, it IGNORES the retention `min_count` / `max_count`
    /// knobs (an explicit user override) but NEVER violates the
    /// SACRED MVCC invariants:
    ///
    /// 1. **ts predicate** — a version is reclaim-eligible ONLY if its
    ///    commit ts is known (`lookup_ts`) AND `ts < cutoff_millis`.
    ///    A version of UNKNOWN age is always KEPT (never purge what
    ///    you can't prove is old enough).
    /// 2. **snapshot floor** — a version `>= min_alive` (pinned by a
    ///    live snapshot) is NEVER reclaimed, regardless of its ts.
    /// 3. **anchor** — the single largest version `< min_alive` per
    ///    key is kept so the oldest live snapshot can still resolve a
    ///    read of a key last-written below `min_alive`.
    ///
    /// Current versions live in `history` (the single log), so an explicit
    /// `cur_v` guard prevents reclaiming them.
    ///
    /// When a version is reclaimed, its `ts_key(version)` is removed in
    /// lockstep so timestamps never outlive their versions. Returns the
    /// number of history version entries deleted.
    pub async fn purge_below_ts(&self, cutoff_millis: u64) -> DbResult<usize> {
        use crate::version_codec::decode_version_key;

        // Phase 1: scan all history version entries, group by key.
        // ts-keys ([TS_TAG][v_be], 9 bytes) are skipped: decode_version_key
        // returns None for them (separator 0x00 != VERSION_SEP).
        let stream = self.history.iter_stream(MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut per_key: TFxMap<Vec<u8>, Vec<(u64, Bytes)>> = TFxMap::default();

        while let Some(batch) = stream.next().await {
            for (phys_key, _value) in batch? {
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    per_key
                        .entry(orig.to_vec())
                        .or_default()
                        .push((version, phys_key.into()));
                }
            }
        }

        // Sacred floor: the oldest version a live snapshot may need.
        let min_alive = self.gate.min_alive();

        // Phase 2: per key, sort ascending, compute the anchor (largest
        // version < min_alive), then reclaim eligible versions.
        // C1: skip the current version — it is SACRED.
        let mut deleted = 0usize;
        for (orig_key, mut entries) in per_key {
            let cur_v = self.current_version(&orig_key);
            entries.sort_by_key(|(v, _)| *v);
            // anchor = largest version < min_alive (None if all are
            // >= min_alive). Keeping a single such version lets the
            // oldest live snapshot still read a key last-written below
            // min_alive via a history range scan.
            let anchor: Option<u64> = entries
                .iter()
                .map(|(v, _)| *v)
                .filter(|v| *v < min_alive)
                .max();

            for (version, phys_key) in &entries {
                // C1 SACRED: never reclaim the current version.
                if *version == cur_v {
                    continue;
                }
                // Sacred: never reclaim a snapshot-pinned version.
                if *version >= min_alive {
                    continue;
                }
                // Sacred: never reclaim the single anchor.
                if Some(*version) == anchor {
                    continue;
                }
                // ts predicate: unknown ts ⇒ KEEP (can't prove old enough).
                let ts = self.lookup_ts(*version).await;
                let Some(ts_val) = ts else {
                    continue;
                };
                if ts_val >= cutoff_millis {
                    continue;
                }
                // All guards pass → reclaim the version AND its ts-key.
                let _ = self.history.remove_no_flag(phys_key.clone().into()).await;
                let _ = self.history.remove_no_flag(ts_key(*version).into()).await;
                // Audit 2.1: prune the ts-index entry in lockstep (ts_val known).
                self.ts_index_remove(ts_val, *version);
                // P1c: drop the same (key, version) from the overlay in lockstep.
                // D2 P1d-2b: gate on durable_watermark — never drop an undrained
                // version (the overlay holds its only copy until the drainer
                // lands it in history). Full overlay-GC is P1e.
                if *version <= self.gate.durable_watermark() {
                    self.overlay.remove(&orig_key, *version);
                }
                deleted += 1;
            }
        }

        Ok(deleted)
    }

    /// cancel-safe: yes — a single `scc::HashMap::retain_async`. The map
    /// is only ever pruned to a strict subset of itself; dropping the
    /// future mid-scan leaves some redundant entries un-evicted, which a
    /// later GC reclaims. No partial state can violate correctness.
    ///
    /// III.3: evict `version_cache` entries whose cached version is
    /// `< min_alive`, where `min_alive = gate.min_alive()` (the oldest
    /// live snapshot, or `last_committed` when no snapshot is open).
    /// Without this, the cache grows unbounded over the repo's lifetime —
    /// `apply_committed_ops` / `set_versioned` / `delete_versioned` upsert
    /// every touched key and nothing ever removes them.
    ///
    /// MVCC-visibility invariant (why `< min_alive` is safe):
    ///
    /// `get_at(key, snapshot)` reads `cur_v = current_version(key)` and,
    /// if `cur_v <= snapshot`, reads `history` at the version-key directly;
    /// otherwise it range-scans the log for the newest version `<= snapshot`.
    /// The cache entry only matters when it forces the range-scan, i.e.
    /// for snapshots `< cur_v`. Evicting an entry makes `current_version`
    /// return `0`, so every snapshot uses the direct log-lookup path.
    ///
    /// An entry with `cv < min_alive` satisfies `cv < min_alive <= s` for
    /// *every* live snapshot `s` (no snapshot is older than `min_alive`).
    /// Thus `cv <= s` already held for all of them — they were *already*
    /// on the direct path. After eviction `0 <= s` still routes them to the
    /// direct path and the log still holds the key's current version entry,
    /// so the returned value is identical. The only thing forgotten
    /// is the version *number*, and it was needed solely to force a
    /// log range-scan for snapshots below `cv` — none of which exist. Hence
    /// the prune is value-preserving for all live readers.
    ///
    /// Conversely, evicting entries with `cv >= min_alive` would be unsafe:
    /// a live snapshot `s` with `min_alive <= s < cv` legitimately needs the
    /// log range-scan (its visible value is an older log entry); forgetting
    /// `cv` would route it to the direct-read path and return the wrong
    /// (newer) current entry. That is why the threshold is `min_alive` and
    /// not the (possibly larger) `min_version` history-GC argument.
    ///
    /// `retain_async` keeps entries for which the predicate returns `true`,
    /// so we keep `*v >= min_alive` and drop the rest. A key re-written
    /// after this prune simply re-populates its entry via the next upsert.
    pub(super) async fn prune_version_cache(&self) {
        let min_alive = self.gate.min_alive();
        self.cells
            .retain_async(|_key, c| c.version >= min_alive)
            .await;
    }

    /// cancel-safe: NO — delegates to `gc_below`, which is non-cancel-
    /// safe. Idempotent on retry.
    ///
    /// Run GC using the gate's `min_alive()` as the threshold.
    pub async fn gc(&self) -> DbResult<usize> {
        let min = self.gate.min_alive();
        self.gc_below(min).await
    }
}
