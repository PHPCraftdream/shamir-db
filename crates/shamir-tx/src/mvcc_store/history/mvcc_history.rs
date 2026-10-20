use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_storage::types::RecordKey;
use shamir_tunables::store_defaults::HISTORY_SCAN_BATCH;
use std::sync::Arc;

use super::super::{ts_key, MvccStore};
use super::version_entry::{StreamingGroupByState, VersionEntry};
use crate::version_codec::{decode_version_key, encode_version_key};

impl MvccStore {
    /// P1b: range-scan the log for the newest version ≤ `snapshot`, returning
    /// BOTH its version and raw value (tombstone = empty `Bytes` is NOT
    /// collapsed to `None` here — the caller needs the version to break the
    /// overlay-vs-history tie in [`MvccStore::resolve_read`]'s fallback).
    ///
    /// Returns `None` when the key has no version ≤ `snapshot`. The scan is
    /// key-major, version-ascending, so the last decoded entry whose original
    /// key matches and whose version ≤ snapshot is the newest.
    pub(crate) async fn scan_history_newest(
        &self,
        key: &[u8],
        snapshot: u64,
    ) -> DbResult<Option<(u64, Bytes)>> {
        let lo = encode_version_key(key, 0);
        let hi = encode_version_key(key, snapshot);
        let stream = self
            .history
            .iter_range_stream(Some(lo), Some(hi), HISTORY_SCAN_BATCH);
        futures::pin_mut!(stream);
        let mut latest: Option<(u64, Bytes)> = None;
        while let Some(batch) = stream.next().await {
            for (phys_key, val) in batch? {
                if let Some((orig, v)) = decode_version_key(&phys_key) {
                    if orig == key {
                        latest = Some((v, val));
                    }
                }
            }
        }
        Ok(latest)
    }

    /// S5c-X PR6b-2 round 4 — publication-bounded cold-start seek: the newest
    /// version of `key` that is PUBLISHED, i.e. `≤ floor`
    /// (`gate.last_committed()`). Range
    /// `[encode_version_key(key,0) .. encode_version_key(key,floor)]`, decode
    /// each entry, filter `orig == key`, take the MAX version. Returns `None`
    /// if the key has no version ≤ floor — either never written, or only
    /// materialized-but-unpublished rows (invariant I3: those must never be
    /// exposed, but they must NOT blind the scan to older published rows —
    /// the bounded range already excludes them).
    pub(crate) async fn seek_latest_published(
        &self,
        key: &[u8],
        floor: u64,
    ) -> DbResult<Option<u64>> {
        let lo = encode_version_key(key, 0);
        // Bounded hi: versions above the floor are simply outside the range,
        // so an unpublished newest version can never shadow an older
        // published one.
        let hi = encode_version_key(key, floor);
        let stream = self
            .history
            .iter_range_stream(Some(lo), Some(hi), HISTORY_SCAN_BATCH);
        futures::pin_mut!(stream);
        let mut max_v: Option<u64> = None;
        while let Some(batch) = stream.next().await {
            for (phys_key, _) in batch? {
                if let Some((orig, v)) = decode_version_key(&phys_key) {
                    if orig == key {
                        max_v = Some(match max_v {
                            None => v,
                            Some(prev) => prev.max(v),
                        });
                    }
                }
            }
        }
        Ok(max_v)
    }

    /// T4-asof: resolve a wall-clock timestamp to the largest committed
    /// version whose recorded commit timestamp is ≤ `ts_millis`.
    ///
    /// Phase 3: uses the in-memory `ts_index` (reversed `TreeIndex`) for O(log N)
    /// lookup. On first call after open (cold start), rebuilds the index from
    /// the history store's ts-keys (one-time O(N) scan, then all subsequent
    /// queries are O(log N)). Falls back to the full history scan only if the
    /// index is empty after rebuild (no ts entries in history at all).
    ///
    /// Read-only; no cell mutation; no locking. Best-effort: if a ts entry
    /// was never recorded for a version (it was written before T1c landed)
    /// that version is invisible to this scan — the conservative choice,
    /// consistent with how `vacuum_key` treats unknown-age versions.
    pub async fn version_at_or_before_ts(&self, ts_millis: u64) -> Option<u64> {
        use std::sync::atomic::Ordering;

        // Lazy rebuild: on first call, populate the ts-index from history.
        if !self.ts_index_ready.load(Ordering::Acquire) {
            self.ts_index_rebuild().await;
        }

        // O(log N) query via the reversed-key TreeIndex.
        self.ts_index_query(ts_millis)
    }

    /// Legacy O(total) scan fallback — kept for reference and as a test oracle.
    /// Not used on the production hot path (replaced by ts_index_query).
    #[cfg(test)]
    pub(crate) async fn version_at_or_before_ts_scan(&self, ts_millis: u64) -> Option<u64> {
        use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;

        use super::super::store::TS_TAG;

        let stream = self.history.iter_stream(MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        let mut best: Option<u64> = None;

        while let Some(batch) = stream.next().await {
            let batch = match batch {
                Ok(b) => b,
                Err(_) => continue,
            };
            for (phys_key, val) in batch {
                // ts-keys are exactly 9 bytes: [TS_TAG][version_be: 8].
                if phys_key.len() != 9 || phys_key[0] != TS_TAG {
                    continue;
                }
                // Decode the recorded commit ts (little-endian u64, 8 bytes).
                if val.len() != 8 {
                    continue;
                }
                let ts_bytes: [u8; 8] = match val.as_ref().try_into() {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let recorded_ts = u64::from_le_bytes(ts_bytes);
                if recorded_ts > ts_millis {
                    continue;
                }
                // Decode the version from the ts-key: bytes [1..9].
                let v_bytes: [u8; 8] = phys_key[1..9].try_into().expect("checked len==9");
                let version = u64::from_be_bytes(v_bytes);
                best = Some(match best {
                    None => version,
                    Some(prev) => prev.max(version),
                });
            }
        }

        best
    }

    /// T4-history: one key's full version timeline, ascending by version.
    ///
    /// Two sources, merged and de-duplicated by version:
    /// 1. the `history` version-log — drained versions;
    /// 2. the in-memory overlay — versions acked on the commit path
    ///    (`apply_committed_visible`) that the background drainer has not
    ///    landed in `history` yet. Without it a read right after a write
    ///    misses the newest versions.
    ///
    /// Every drained version lives under `encode_version_key(key, version)`
    /// (`<key> || 0xFF || version_be`). The range scan
    /// `[encode_version_key(key, 0), +∞)` yields all of this key's entries.
    /// ts-keys (`[TS_TAG][version_be]`, 9 bytes, `TS_TAG = 0x00`) are out of
    /// this key's range and are additionally rejected by `decode_version_key`
    /// (which returns `None` when the separator byte is not `VERSION_SEP`), so
    /// they can never be mistaken for a version entry.
    ///
    /// A key that is currently DELETED contributes a tombstone; its prior
    /// versions still appear.
    ///
    /// Each entry's commit timestamp comes from the pending commit-time stamp
    /// while the version is undrained, else from [`Self::lookup_ts`] (T1c).
    /// Entries with no recorded ts carry `ts_millis = None`.
    ///
    /// Read-only, no cell mutation, no locking. Allocation is bounded by
    /// the key's version count (one `VersionEntry` per archived version).
    pub async fn history_of(&self, key: &[u8]) -> DbResult<Vec<VersionEntry>> {
        // Phase 0: snapshot the overlay BEFORE scanning `history`. A version
        // is dropped from the overlay only after it is durable in `history`,
        // so one missing from this snapshot is either not committed yet or
        // already visible to the scan below; the reverse order could lose a
        // version drained+GC'd between the two reads.
        let overlay_entries = self.overlay.versions_of(key);

        // Phase 1: scan this key's version range in `history`.
        // `encode_version_key(key, 0)` is the lexically smallest key in
        // this key's version namespace; an open upper bound (`None`) walks
        // every version. ts-keys live in the separate `[TS_TAG]` namespace
        // and cannot collide (see the module-level comment above).
        let lo = encode_version_key(key, 0);
        let stream = self
            .history
            .iter_range_stream(Some(lo), None, HISTORY_SCAN_BATCH);
        futures::pin_mut!(stream);

        // Collect (version, value) for every archived entry.
        let mut entries: Vec<(u64, Bytes)> = Vec::new();
        while let Some(batch) = stream.next().await {
            for (phys_key, val) in batch? {
                // decode_version_key returns None for ts-keys (9-byte
                // `[TS_TAG][v_be]` with separator byte 0x00 ≠ 0xFF) AND
                // for any key not ending in `|| 0xFF || version_be`. Both
                // guards are belt-and-braces here — the range lower bound
                // already excludes foreign keys — but the decode also
                // recovers the version number we need.
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    // Defensive: range scans are over the key's own
                    // namespace, but a longer key sharing our prefix would
                    // surface here. Only accept entries whose original key
                    // matches exactly.
                    if orig == key {
                        entries.push((version, val));
                    }
                }
            }
        }

        // Phase 2: merge the undrained overlay versions. A version present in
        // both carries the identical payload (the drainer copies the overlay
        // value verbatim), so the stable sort + dedup keeps either.
        entries.extend(overlay_entries);

        // Phase 3: ascending by version, resolve ts per version.
        entries.sort_by_key(|(v, _)| *v);
        entries.dedup_by_key(|(v, _)| *v);
        let mut out = Vec::with_capacity(entries.len());
        for (version, value) in entries {
            // The commit-time stamp lives in `pending_ts` until the version is
            // durable (reclaimed by `gc_overlay_to` after the history write),
            // so a miss there means the ts is already in `history`.
            let ts_millis = match self.pending_ts.get_sync(&version).map(|e| *e.get()) {
                Some(ts) => Some(ts),
                None => self.lookup_ts(version).await,
            };
            out.push(VersionEntry {
                version,
                value,
                ts_millis,
            });
        }
        Ok(out)
    }

    /// cancel-safe: yes — a single synchronous `cells.entry_sync`
    /// modify-or-insert; the fn no longer suspends, so cancellation can only
    /// happen before the whole update runs or after it completed.
    ///
    /// Seed the in-memory version cache for a recovered key.
    ///
    /// V2 WAL recovery (`crate`-external; see
    /// `shamir_engine::tx::recovery`) replays a committed tx by writing
    /// entries directly into the history log, bypassing
    /// [`apply_committed_ops`]. That keeps the log correct but leaves
    /// `version_cache` empty, so a later `get_at(key, snap)` for a
    /// snapshot *below* `commit_version` would use the direct-read path
    /// (`current_version == 0 ≤ snap`) and return the recovered (latest)
    /// value instead of range-scanning the log.
    ///
    /// In the bootstrap-recovery scenario this is harmless (no snapshot
    /// survives a restart and every fresh snapshot opens at
    /// `≥ last_committed ≥ commit_version`), but seeding the cache keeps
    /// `version_of`/`get_at` consistent for any post-recovery reader and
    /// for SSI conflict detection if the recovered key is immediately
    /// re-written inside a new transaction.
    ///
    /// `entry_sync` (occupied/vacant) — NOT `upsert` — so that on the
    /// OCCUPIED branch the cell's `version` is advanced ONLY when `version`
    /// is strictly greater than the cell's current value (max-monotonic,
    /// matching `publish_cell`). This prevents a cold read racing the FIRST
    /// overlay-only commit of a key from seeding an OLDER history-derived
    /// version on top of a fresher in-memory cell (A2: stale reads / masked
    /// SSI conflicts). A re-replay of the same key with an equal or lower
    /// version is a no-op; the VACANT branch always seeds at the offered
    /// version.
    ///
    /// DEADLOCK FIX (#589 root cause): `entry_sync`, NOT `entry_async` —
    /// same reasoning as [`MvccStore::publish_cell`](super::super::MvccStore): an
    /// async-granted bucket lock is held by a task parked in tokio's run
    /// queue, and the SYNCHRONOUS `read_sync` readers (`current_version`)
    /// park worker threads on that same bucket; if all workers park, the
    /// lock-owning task is never polled again → whole-runtime deadlock. The
    /// fn stays `async` to keep its call sites unchanged; it no longer
    /// suspends.
    pub async fn seed_version(&self, key: RecordKey, version: u64) {
        match self.cells.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                if version > e.get().version {
                    e.get_mut().version = version;
                }
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(super::super::store::RecordCell {
                    version,
                    vacuum_anchor: 0,
                    reserved_by: 0,
                });
            }
        }
    }

    /// cancel-safe: NO — applies a batch of `KvOp` for MULTIPLE commit versions
    /// via a SINGLE durable `history.transact`. This is the batched drain path:
    /// the drainer accumulates ops across multiple WAL entries for the same
    /// table and lands them in one write-tx instead of one per entry.
    ///
    /// `pass` is a slice of `(commit_version, ops)` pairs, each carrying the
    /// `KvOp`s for that version. The pairs MUST be in ascending commit_version
    /// order (the drainer sorts entries by version before accumulation).
    ///
    /// Builds ONE `history_ops` vec containing:
    ///  - For each `(v, ops)`: `KvOp::Set(encode_version_key(k, v), val|tombstone)`
    ///  - For each unique `v`: `KvOp::Set(ts_key(v), ts_val)` (L2-style fold)
    ///
    /// Then: ONE `self.history.transact(history_ops)`.
    /// Then: per-(key,v) `publish_cell` (idempotent -- needed for cold recovery).
    /// Then: `gate.raise_published(max_v)`.
    pub async fn write_committed_batch_to_history(
        &self,
        pass: &[(u64, Vec<KvOp>)],
    ) -> DbResult<()> {
        if pass.is_empty() {
            return Ok(());
        }

        // #1032/#1234: serialize against any other history writer for this
        // table (a forced `drain_to_history`, or a concurrent drain pass) — see
        // `drain_exclusive`'s field doc (`mvcc_store/mod.rs`). WAIT, never
        // skip: `Drainer::drain_step` reads `Ok` as "landed in history" and
        // goes on to `mark_durable`, commit the WAL marker and GC the overlay,
        // so an `Ok` without a write leaves the commit in neither overlay nor
        // history until (and unless) the other writer lands it.
        let _drain_guard = self.drain_exclusive.lock().await;

        // Pre-size: each (v, ops) contributes ops.len() data entries + 1 ts entry.
        let total_ops: usize = pass.iter().map(|(_, ops)| ops.len()).sum();
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(total_ops + pass.len());
        // Phase 3: collect (version, ts_ms) to feed the ts-index after transact.
        let mut ts_entries: Vec<(u64, u64)> = Vec::with_capacity(pass.len());

        for (commit_version, ops) in pass {
            let v = *commit_version;

            // T1c/A14: resolve the commit timestamp. Prefer the COMMIT-TIME
            // millis stamped on the ack-path (`apply_committed_visible`);
            // cold recovery falls back to `now_millis()`.
            //
            // A14: read NON-DESTRUCTIVELY (`get`, not `remove`). Two
            // independent drain paths (the background drainer's batched path
            // here, and a forced `flush_buffers`/`drain_to_history` single-
            // entry drain via `write_committed_to_history`) can BOTH reach
            // the same `commit_version`. A destructive `remove` made them
            // race: the first caller won the stamp and the second found
            // `None` and fell back to `now_millis()`, overwriting the correct
            // commit-time ts with a drain-time ts in durable history. The
            // non-destructive `get` lets every racing caller observe the SAME
            // correct `T0`. Reclamation is decoupled: `gc_overlay_to`
            // (invoked by the drainer once the version is durable) `retain`-
            // sweeps the stamp, so the entry does not leak. `None` here now
            // genuinely means "never stamped" (cold recovery), not "already
            // consumed by a racing caller" — preserving the fallback
            // distinction exactly.
            let ts_ms = self
                .pending_ts
                .get_sync(&v)
                .map(|e| *e.get())
                .unwrap_or_else(|| self.now_millis());
            let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());
            ts_entries.push((v, ts_ms));

            // Data ops: version-key entries.
            for op in ops {
                let h_op = match op {
                    KvOp::Set(k, val) => KvOp::Set(encode_version_key(k, v).into(), val.clone()),
                    KvOp::Remove(k) => KvOp::Set(encode_version_key(k, v).into(), Bytes::new()),
                };
                history_ops.push(h_op);
            }

            // L2-style fold: ts entry rides in the SAME batch as data.
            history_ops.push(KvOp::Set(ts_key(v).into(), ts_val));
        }

        // ONE batched write to history (all versions, all ops, atomic).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 3: maintain the in-memory ts-index for every version in the batch.
        for (version, ts_ms) in &ts_entries {
            self.ts_index_insert(*ts_ms, *version);
        }

        // Seed the cell from the durable write (idempotent; needed on cold
        // recovery). On the warm drain path the cell is already at the correct
        // version from the ack-path's `apply_committed_visible`.
        for (commit_version, ops) in pass {
            let v = *commit_version;
            for op in ops {
                let key = match op {
                    KvOp::Set(k, _) => k.clone(),
                    KvOp::Remove(k) => k.clone(),
                };
                // KvOp keys and the cell map are both `RecordKey`-keyed
                // (task #532) — pass straight through, no conversion.
                self.publish_cell(key, v).await;
            }
        }

        // R3: advance the reader-visible floor to the max version in the batch.
        if let Some((max_v, _)) = pass.last() {
            self.gate.raise_published(*max_v);
        }

        Ok(())
    }

    /// cancel-safe: NO — applies a batch of `KvOp` via multi-step sequences
    /// (history transact, version_cache updates). One durable write (history
    /// transact). Cancellation mid-batch leaves some phases applied, others
    /// not. Recovery relies on WAL replay.
    ///
    /// D2 P1d-2b: this is now the composition of the two split halves —
    /// [`apply_committed_visible`] (overlay + cell publish, the ack-path) and
    /// [`write_committed_to_history`] (history transact + ts, the drain/
    /// recovery-path). The PRODUCTION commit path no longer calls this
    /// combined method (the ack-path routes the visible half inline; the
    /// background drainer writes history). It remains for non-cutover callers
    /// (unit tests / direct invocations) that want the pre-cutover "both
    /// halves at once" semantics.
    ///
    /// Ordering: the visible half runs FIRST so the
    /// commit-time ts stamp is available to the drain half; a history error
    /// leaves the overlay/cell ahead until the drainer catches up — the
    /// intentional pre-existing semantics.
    pub async fn apply_committed_ops(&self, ops: Vec<KvOp>, commit_version: u64) -> DbResult<()> {
        // Visible FIRST so the commit-time ts stamp (set in
        // `apply_committed_visible`) is in `pending_ts` before
        // `write_committed_to_history` consumes it — otherwise the drain-half
        // would fall back to `now_millis()` and leave a stale, never-consumed
        // stamp behind. In this combined (test/direct) path both halves run
        // synchronously at commit time, so the ordering carries no durability
        // window: the visible state and the history write happen together, and
        // a history error propagates via `?` (the cell/overlay are then ahead,
        // exactly as the production ack-path intentionally is until the drainer
        // catches up).
        self.apply_committed_visible(&ops, commit_version);
        self.write_committed_to_history(&ops, commit_version)
            .await?;
        Ok(())
    }

    /// D2 P1d-2b — ACK-path visible half. Populate ONLY the in-memory
    /// visibility state for a committed batch: the versioned overlay (the sole
    /// RAM copy of the value until the drainer writes history) and the per-key
    /// cell (`publish_cell`), then advance the reader-visible floor. Writes NO
    /// history and records NO ts — those are the drainer's job
    /// ([`write_committed_to_history`]).
    ///
    /// Synchronous (no `.await` on storage): the overlay + cell are lock-free
    /// in-memory structures, so this is the cheap ack-path the cutover moves
    /// the expensive `history.transact` OFF of. Called by `apply_data_batch`
    /// (commit Phase 5a) after the WAL entry is durable.
    ///
    /// Invariant: the overlay MUST hold the value before any reader can observe
    /// `commit_version` (cell bumped / floor advanced). Insert order below —
    /// overlay BEFORE cell BEFORE floor — guarantees no window where the cell
    /// reports `commit_version` while the overlay is empty AND history has not
    /// yet been drained.
    pub fn apply_committed_visible(&self, ops: &[KvOp], commit_version: u64) {
        // D2 P1d-2b: capture the COMMIT-TIME wall clock for this version now,
        // so the drainer (which writes the durable ts arbitrarily later) stamps
        // history with commit time, NOT drain time. `now_millis()` honours the
        // test clock (`set_test_now`) and the real `SystemTime` alike. Stored
        // once per commit_version (all ops share it); reclaimed by
        // `gc_overlay_to` once the version is durable (A14: the drain paths
        // READ this non-destructively, so they no longer remove it here).
        let _ = self
            .pending_ts
            .insert_sync(commit_version, self.now_millis());

        // P1c: populate the overlay with the SAME (key, version) → value pair
        // that the drainer will later land in history. The overlay value is
        // byte-identical to the eventual history payload: the raw value for
        // `Set`, an empty `Bytes` tombstone for `Remove`.
        for op in ops {
            match op {
                // KvOp keys and the overlay are both `RecordKey`-keyed
                // (task #532) — clone straight through, no conversion.
                KvOp::Set(k, v) => self.overlay.insert(k.clone(), commit_version, v.clone()),
                KvOp::Remove(k) => self.overlay.insert(k.clone(), commit_version, Bytes::new()),
            }
        }

        // Update the in-memory cell for every touched key. This loop calls
        // the synchronous `finalize_reservation` (CRIT-2: synchronous scc
        // `entry` modify-or-insert — detailed in the S2 note below), which
        // atomically publishes `version = commit_version` and clears the
        // reservation. After this, readers at `>= commit_version` resolve
        // the value from the overlay.
        //
        // SSI fix S2 — FINALIZE the cell-reservation here instead of a plain
        // `publish_cell_sync`. `finalize_reservation` sets `version =
        // commit_version` AND clears `reserved_by` in one atomic `entry`, a
        // STRICT SUPERSET of `publish_cell_sync` (which only set `version`).
        // On a cell that was never claimed (`reserved_by == 0` — the Snapshot /
        // Pessimistic / recovery / drainer paths, which do not run the S2 claim)
        // the `reserved_by = 0` write is a no-op, so the effect is byte-identical
        // to the old `publish_cell_sync`. On the Serializable claim path it
        // releases the winning committer's reservation at the exact instant its
        // version becomes visible — the publish-time half of the "claim decides
        // who won" inversion.
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k.clone(),
                KvOp::Remove(k) => k.clone(),
            };
            // finalize_reservation is synchronous (scc `entry`, no I/O) — the
            // ack-path stays off `.await` (same rationale as the prior
            // publish_cell_sync).
            // KvOp keys and the reservation / cell map are both `RecordKey`-
            // keyed (task #532) — pass straight through, no conversion.
            self.finalize_reservation(key, commit_version);
        }

        // R3: advance the reader-visible floor so subsequent `get_current` /
        // `current_stream` see the materialized version. In the tx commit path
        // the caller (`commit_tx`) ALSO publishes via the watermark; this
        // monotonic fetch_max is safe to call redundantly.
        self.gate.raise_published(commit_version);
    }

    /// D2 P1d-2b — DRAIN/RECOVERY-path history half. Write the committed batch
    /// to the durable `history` version-log + record the commit ts. Calls
    /// `publish_cell` too (idempotent) so a cold recovery path that seeds the
    /// cell from history alone stays consistent; does NOT touch the overlay
    /// (the overlay is the ack-path's RAM copy — on the drain path the value is
    /// being made durable, and on cold recovery the overlay is empty by
    /// construction).
    ///
    /// This is the expensive `history.transact` the cutover moved OFF the
    /// ack-path. Called by the background `Drainer` (via `replay_v2_entry`'s
    /// per-table routing) and reachable on direct/recovery paths.
    ///
    /// S5c-X PR6b-1: this is now the materialize+publish WRAPPER preserving
    /// the exact pre-split ordering (durable rows → ts index → cell seeds →
    /// floor raise) over [`materialize_committed_to_history`] +
    /// [`publish_materialized`]; the PR6b receipt application will call the
    /// two halves separately.
    pub async fn write_committed_to_history(
        &self,
        ops: &[KvOp],
        commit_version: u64,
    ) -> DbResult<()> {
        // Hot path: the wrapper adds exactly one inlined fn-call layer — zero
        // new allocation, locks or atomics.
        self.materialize_committed_to_history(ops, commit_version)
            .await?;
        self.publish_materialized(ops, commit_version);
        Ok(())
    }

    /// S5c-X PR6b-1 — MATERIALIZE half. Writes the history rows (version-key
    /// entries + the commit `ts_key`) and updates the in-memory ts index for
    /// `commit_version`, WITHOUT raising the published floor and WITHOUT any
    /// cell/visibility notification.
    ///
    /// A materialized-but-unpublished version stays invisible to every
    /// reader: all read paths cap at `gate.last_committed()`, and neither the
    /// cell nor the overlay was touched, so `get_current` / `current_stream`
    /// / `get_at` cannot observe the row until [`publish_materialized`]
    /// raises the floor.
    ///
    /// Cancel-safe semantics unchanged: a single durable `history.transact`
    /// — cancellation mid-write leaves the batch all-or-nothing on backends
    /// that override `transact`.
    pub async fn materialize_committed_to_history(
        &self,
        ops: &[KvOp],
        commit_version: u64,
    ) -> DbResult<()> {
        // T1c + L2: resolve the commit timestamp BEFORE building history_ops
        // so it can ride in the SAME atomic transact batch as the data ops.
        // D2 P1d-2b: prefer the COMMIT-TIME millis stamped on the ack-path
        // (`apply_committed_visible`) so the durable ts reflects commit time,
        // not this (later) drain time. Cold recovery (overlay empty → no stamp)
        // falls back to `now_millis()` — the pre-cutover behaviour (recovery
        // never had the original ts).
        //
        // A14: read NON-DESTRUCTIVELY (`get`, not `remove`). See the note on
        // the batched path (`write_committed_batch_to_history`): two
        // independent drain paths (this single-entry path is reached by both
        // `drain_to_history` — used by table rename — AND any forced flush)
        // can race the background drainer for the SAME `commit_version`. A
        // destructive `remove` lost the stamp for the second caller, who then
        // fell back to `now_millis()` and overwrote the correct commit-time
        // ts. The non-destructive `get` lets every racer observe the SAME
        // `T0`; `gc_overlay_to` reclaims the stamp once the version is
        // durable (no leak). `None` now means "never stamped" (cold
        // recovery), NOT "already consumed by a racer".
        let ts_ms = self
            .pending_ts
            .get_sync(&commit_version)
            .map(|e| *e.get())
            .unwrap_or_else(|| self.now_millis());
        let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());

        // HIGH-3: batch the physical writes through `Store::transact`.
        // Per-op `set`/`remove` collapses to a single atomic write-tx on
        // backends that override `transact` — one fsync instead of N.
        //
        // C1: every committed key gets a log entry unconditionally — the log
        // is the universal version timeline. For KvOp::Remove the log entry is
        // a tombstone (empty value).
        //
        // L2: the ts-key rides in the SAME batch — one durable write per
        // version instead of two.
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(ops.len() + 1);
        for op in ops {
            let h_key = match op {
                KvOp::Set(k, v) => {
                    KvOp::Set(encode_version_key(k, commit_version).into(), v.clone())
                }
                KvOp::Remove(k) => {
                    KvOp::Set(encode_version_key(k, commit_version).into(), Bytes::new())
                }
            };
            history_ops.push(h_key);
        }
        // L2: append the ts entry into the same atomic batch.
        history_ops.push(KvOp::Set(ts_key(commit_version).into(), ts_val));

        // One batched write to history (data + ts, atomic).
        //
        // #1032 diagnostic instrumentation (follow-up to F-68/#895 task
        // #124): this is the single genuine I/O + potential store-internal
        // lock acquisition in this function — the prior instrumentation
        // round did not cover it. If a hang recurs, before/after logs here
        // (paired with `drain_to_history`'s per-version logs) show whether
        // the stall is INSIDE `Store::transact` itself.
        if !history_ops.is_empty() {
            log::debug!(
                "write_committed_to_history: history.transact start version={commit_version} ops={}",
                history_ops.len()
            );
            self.history.transact(history_ops).await?;
            log::debug!(
                "write_committed_to_history: history.transact done version={commit_version}"
            );
        }

        // Phase 3: maintain the in-memory ts-index (idempotent — duplicate
        // insert into TreeIndex is a no-op).
        self.ts_index_insert(ts_ms, commit_version);

        // S5c-X PR6b-2 (cold-start restore): make the materialize→publish
        // window DETECTABLE. The allocation high-water mark must dominate
        // every durable history version, otherwise a reader cannot tell
        // "rows exist above the floor because a receipt materialized them"
        // (they must stay invisible) from "plain MVCC wrote them and the
        // floor was simply never raised" (master's unbounded cold seek must
        // find them). `fetch_max` is a no-op on the guarded ack path, where
        // the version guard already advanced the counter.
        // The receipt path enables this gate flag before materialization;
        // legacy materialization leaves its allocator watermark untouched.
        if self.gate.receipt_native_visibility_enabled() {
            self.gate.observe_materialized_version(commit_version);
        }
        Ok(())
    }

    /// S5c-X PR6b-1 — PUBLISH half. Raises the reader-visible floor
    /// (`gate.raise_published`) and seeds each touched key's cell (max-
    /// monotonic, idempotent). Synchronous, infallible, no I/O and no
    /// `.await`, so a receipt apply can publish after durability without a
    /// failure path.
    pub fn publish_materialized(&self, ops: &[KvOp], commit_version: u64) {
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k.clone(),
                KvOp::Remove(k) => k.clone(),
            };
            // KvOp keys and the cell map are both `RecordKey`-keyed
            // (task #532) — pass straight through, no conversion.
            self.publish_cell_sync(key, commit_version);
        }

        // R3: advance the reader-visible floor (monotonic fetch_max). On the
        // ack-driven flow the floor is already at/above this; on cold recovery
        // this lifts it.
        self.gate.raise_published(commit_version);
    }
}

impl MvccStore {
    /// Shared implementation behind [`current_stream`](Self::current_stream),
    /// [`current_stream_with_tombstones`](Self::current_stream_with_tombstones), and
    /// [`snapshot_stream`](Self::snapshot_stream). (Moved here in S5c-X PR6b-2
    /// round 4 as part of the layout budget alongside the publication-bounded
    /// `seek_latest_published` scan.)
    ///
    /// `at_version` is the explicit version floor (usually `gate.last_committed()` for
    /// the current-stream variants).
    /// `include_tombstones` selects which of the two semantics
    /// `StreamingGroupByState`'s flush points apply — see
    /// `version_entry.rs`'s `flush_group`/`drain_overlay`.
    pub(crate) fn snapshot_stream_impl(
        &self,
        batch: usize,
        at_version: u64,
        include_tombstones: bool,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        use futures::stream::unfold;

        let history = Arc::clone(&self.history);
        // R3: capture committed floor at stream-open time (explicit parameter).
        let floor = at_version;
        // P1b: materialise the overlay's per-key winner ≤ floor at open. The
        // overlay is the SMALL side of the merge (bounded window), so loading
        // it into a map and merging during the history group-by is cheap.
        // `floor == 0` (bootstrap) → `snapshot_le` returns empty → overlay
        // contributes nothing and the stream is byte-identical to history-only.
        // Boundary: `snapshot_le` yields `RecordKey` keys; the group-by state
        // machine (`OverlayWinners`) is `Bytes`-keyed and merges against the
        // `Bytes` `orig` recovered from each history version-key. Convert the
        // overlay side ONCE here at stream-open (byte-identical) — it is the
        // small, bounded side of the merge, not a per-row hot-path cost.
        let overlay: shamir_collections::TMap<Bytes, (u64, Bytes)> = self
            .overlay
            .snapshot_le(floor)
            .into_iter()
            .map(|(k, v, val)| (Bytes::from(k), (v, val)))
            .collect();
        // Box::pin so the returned stream is `Unpin` — callers (e.g.
        // `TableManager::list_stream`) consume it via `.next()` without
        // pinning, matching the P1 `Pin<Box<dyn Stream>>` contract. A raw
        // `Unfold` over an async closure is NOT `Unpin`.
        Box::pin(unfold(
            StreamingGroupByState::Start {
                history,
                batch_size: batch,
                floor,
                overlay,
                include_tombstones,
            },
            |state| async move {
                match state {
                    StreamingGroupByState::Start {
                        history,
                        batch_size,
                        floor,
                        overlay,
                        include_tombstones,
                    } => {
                        // Boundary: the store now yields `RecordKey` keys; the
                        // group-by state machine is `Bytes`-keyed throughout, so
                        // convert each batch key back to `Bytes` (byte-identical)
                        // at the stream boundary.
                        let stream =
                            futures::StreamExt::map(history.iter_stream(batch_size), |batch| {
                                batch.map(|rows| {
                                    rows.into_iter().map(|(k, v)| (Bytes::from(k), v)).collect()
                                })
                            });
                        let pin = Box::pin(stream);
                        let s = StreamingGroupByState::Streaming {
                            stream: pin,
                            batch_size,
                            floor,
                            overlay,
                            cur_key: None,
                            last_val: None,
                            last_ver: 0,
                            out_batch: Vec::new(),
                            include_tombstones,
                        };
                        s.drain_and_emit().await
                    }
                    // C2 fix: a Streaming state returned by a prior
                    // drain_and_emit (when out_batch filled to batch_size, or
                    // on a stream error) must CONTINUE draining — not panic.
                    // The previous `unreachable!()` paniced on the second pull
                    // whenever the current-key set exceeded one batch.
                    s @ StreamingGroupByState::Streaming { .. } => s.drain_and_emit().await,
                    // P1b: overlay-only drain phase (after history exhausted).
                    s @ StreamingGroupByState::DrainOverlay { .. } => s.drain_and_emit().await,
                    StreamingGroupByState::Done => None,
                }
            },
        ))
    }
}

impl MvccStore {
    /// return `history.get(encode_version_key(key, cur_v))`.
    /// Fallback: range scan history `[key::0, key::snapshot]`, take last.
    pub async fn get_at(&self, key: &[u8], snapshot_version: u64) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(key);
        self.resolve_read(key, snapshot_version, cur_v).await
    }

    /// cancel-safe: yes — read-only (same as [`Self::get_at`], applied to
    /// many keys). CR-C3 (#778): vectored sibling of [`Self::get_at`] for
    /// callers that need the AS-OF value of MANY keys against the SAME
    /// `snapshot_version` (e.g. `TableManager::read_as_of`'s full-scan
    /// enumeration, which previously awaited `get_at` once PER RECORD —
    /// N sequential single-key `history.get` round-trips over the whole
    /// matched set).
    ///
    /// **Partition, don't force-batch the fallback path.** Classifying a
    /// key is a cheap in-memory `current_version` lookup (a `RecordCell`
    /// map read, no I/O), so every input key is classified up front:
    ///
    /// - **Direct-path** (`cur_v > 0 && cur_v <= snapshot_version` — the
    ///   COMMON case: no concurrent write landed on this key since the
    ///   snapshot was pinned). The overlay is probed first per key (also
    ///   in-memory, no I/O) — an overlay hit resolves immediately without
    ///   ever touching `history`. The overlay-MISS subset's
    ///   `encode_version_key(key, cur_v)` physical keys are collected into
    ///   ONE `Vec` and resolved via a SINGLE `self.history.get_many(..)`
    ///   call — this is the actual win: N sequential single-key gets
    ///   collapse into one vectored call (disk backends override
    ///   `get_many` to collapse N `spawn_blocking` calls into one).
    /// - **Fallback-path** (`cur_v == 0` cold-start, or `cur_v >
    ///   snapshot_version` — a concurrent write landed on THIS key after
    ///   the snapshot was pinned, requiring the range-scan-newest-visible
    ///   fallback in [`Self::resolve_read`]): kept on the EXISTING per-key
    ///   `resolve_read` call, unbatched. This is intentionally NOT batched
    ///   here — it is the rare path (fires only when a concurrent writer
    ///   touched the SAME key after the caller's snapshot was pinned), and
    ///   batching the range-scan fallback itself is a separate, harder
    ///   problem (a versioned-index rewrite) explicitly out of scope for
    ///   this "cheap first step" optimization.
    ///
    /// Results are reassembled in the ORIGINAL input `keys` order — callers
    /// never need to re-sort or track which subset a given result came
    /// from. Empty input returns `Ok(vec![])` with no I/O (mirrors
    /// `Store::get_many`'s own early return).
    pub async fn get_at_many(
        &self,
        keys: &[Bytes],
        snapshot_version: u64,
    ) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 1: classify each input key. For each slot we store the
        // resolution category so Phase 3 can assemble the output in order.
        enum Slot {
            /// Resolved from the overlay — final answer already known.
            Resolved(Option<Bytes>),
            /// Direct-path, overlay miss — `miss_idx` is the position in
            /// the `miss_keys` vector fed to `history.get_many`.
            HistoryMiss { miss_idx: usize },
            /// Fallback path (`cur_v == 0` or `cur_v > snapshot_version`) —
            /// resolved per-key via the existing `resolve_read` below.
            Fallback,
        }

        let len = keys.len();
        let mut slots: Vec<Slot> = Vec::with_capacity(len);
        let mut miss_keys: Vec<RecordKey> = Vec::new();

        for key in keys {
            let cur_v = self.current_version(key);
            if cur_v == 0 || cur_v > snapshot_version {
                slots.push(Slot::Fallback);
                continue;
            }
            // Direct path — overlay probe first (in-memory, no I/O).
            if let Some(val) = self.overlay.get(key, cur_v) {
                let resolved = if val.is_empty() { None } else { Some(val) };
                slots.push(Slot::Resolved(resolved));
                continue;
            }
            // Overlay miss — needs the batched history lookup.
            let vk = encode_version_key(key, cur_v);
            let miss_idx = miss_keys.len();
            miss_keys.push(vk.into());
            slots.push(Slot::HistoryMiss { miss_idx });
        }

        // Phase 2: ONE batched history read for the whole direct-path
        // overlay-miss subset.
        let miss_results = if miss_keys.is_empty() {
            Vec::new()
        } else {
            self.history.get_many(miss_keys).await?
        };

        // Phase 3: assemble the output vector in input order.
        let mut out: Vec<Option<Bytes>> = Vec::with_capacity(len);
        for (i, slot) in slots.iter().enumerate() {
            match slot {
                Slot::Resolved(val) => out.push(val.clone()),
                Slot::HistoryMiss { miss_idx } => match &miss_results[*miss_idx] {
                    Some(val) if val.is_empty() => out.push(None), // tombstone
                    Some(val) => out.push(Some(val.clone())),
                    None => out.push(None), // not found
                },
                Slot::Fallback => {
                    let cur_v = self.current_version(&keys[i]);
                    let val = self.resolve_read(&keys[i], snapshot_version, cur_v).await?;
                    out.push(val);
                }
            }
        }

        Ok(out)
    }

    /// T1c: look up the recorded commit timestamp (millis) for `version`.
    /// Returns `None` if no ts entry exists (treated as "unknown age" → the
    /// age axis conservatively keeps the version).
    pub(crate) async fn lookup_ts(&self, version: u64) -> Option<u64> {
        match self.history.get(ts_key(version).into()).await {
            Ok(val) => {
                if val.len() == 8 {
                    let bytes: [u8; 8] = val.as_ref().try_into().ok()?;
                    Some(u64::from_le_bytes(bytes))
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }
}
