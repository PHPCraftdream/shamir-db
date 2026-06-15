use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_tunables::store_defaults::HISTORY_SCAN_BATCH;

use super::version_entry::VersionEntry;
use super::MvccStore;
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
    pub(super) async fn scan_history_newest(
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

    /// C2: cold-start helper for when the cell is absent after restart.
    /// Reverse/scan the log for the largest version of `key`.
    /// Range `[encode_version_key(key,0) .. encode_version_key(key,u64::MAX)]`,
    /// decode each entry, filter `orig == key`, take the MAX version.
    /// Returns `None` if the key was never written. Read-only.
    pub(super) async fn seek_latest_version(&self, key: &[u8]) -> DbResult<Option<u64>> {
        let lo = encode_version_key(key, 0);
        // Use u64::MAX to cover all possible versions.
        let hi = encode_version_key(key, u64::MAX);
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
    /// Algorithm: scan ALL ts-keys (`[TS_TAG][version_be: 8]`) stored in
    /// the `history` store — each was written by [`Self::record_ts`] when
    /// the corresponding version was committed. Pick the maximum version
    /// whose recorded ts ≤ `ts_millis`. Returns `None` when no eligible
    /// version exists (e.g. the store is empty, or `ts_millis` is earlier
    /// than all recorded versions).
    ///
    /// This is O(total versions) — acceptable for the point-in-time read
    /// slice; a dedicated ts-ordered index is a later performance slice.
    ///
    /// Read-only; no cell mutation; no locking. Best-effort: if a ts entry
    /// was never recorded for a version (it was written before T1c landed)
    /// that version is invisible to this scan — the conservative choice,
    /// consistent with how `vacuum_key` treats unknown-age versions.
    pub async fn version_at_or_before_ts(&self, ts_millis: u64) -> Option<u64> {
        use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;

        use super::TS_TAG;

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
    /// Reads from a single source: the `history` version-log.
    ///
    /// Every version (current and prior) lives under
    /// `encode_version_key(key, version)` (`<key> || 0xFF || version_be`).
    /// The range scan `[encode_version_key(key, 0), +∞)` yields all versioned
    /// entries for this key. ts-keys (`[TS_TAG][version_be]`, 9 bytes,
    /// `TS_TAG = 0x00`) are out of this key's range and are additionally
    /// rejected by `decode_version_key` (which returns `None` when the
    /// separator byte is not `VERSION_SEP`), so they can never be mistaken
    /// for a version entry.
    ///
    /// The current version is already in the log (written by
    /// `set_versioned`/`apply_committed_ops`), so the single scan covers
    /// the full timeline. A key that is currently DELETED contributes a
    /// tombstone; its prior versions still appear from the log.
    ///
    /// Each entry's commit timestamp is resolved via [`Self::lookup_ts`]
    /// (T1c). Entries with no recorded ts carry `ts_millis = None`.
    ///
    /// Read-only, no cell mutation, no locking. Allocation is bounded by
    /// the key's version count (one `VersionEntry` per archived version).
    pub async fn history_of(&self, key: &[u8]) -> DbResult<Vec<VersionEntry>> {
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

        // Phase 2: no additional read needed. The current version is already
        // in the log (written by set_versioned/apply_committed_ops), so the
        // Phase-1 scan above already covers the full timeline.

        // Phase 3: ascending by version, resolve ts per version.
        entries.sort_by_key(|(v, _)| *v);
        let mut out = Vec::with_capacity(entries.len());
        for (version, value) in entries {
            let ts_millis = self.lookup_ts(version).await;
            out.push(VersionEntry {
                version,
                value,
                ts_millis,
            });
        }
        Ok(out)
    }

    /// cancel-safe: yes — a single `version_cache.upsert_async`, which is
    /// CAS-based and either lands or leaves the map unchanged on cancel.
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
    /// `upsert_async` (not `insert`) so a re-replay of the same key
    /// advances monotonically rather than silently keeping a stale value.
    pub async fn seed_version(&self, key: Bytes, version: u64) {
        self.cells
            .upsert_async(
                key,
                super::RecordCell {
                    version,
                    reserved_by: 0,
                },
            )
            .await;
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
    /// halves at once" semantics. Ordering: history FIRST (durable landing),
    /// then visible (overlay + cell) — matching the pre-split contract where a
    /// failed history `transact` (`?`) left no reader-visible state.
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
        // once per commit_version (all ops share it); removed by the drainer.
        let _ = self.pending_ts.insert(commit_version, self.now_millis());

        // P1c: populate the overlay with the SAME (key, version) → value pair
        // that the drainer will later land in history. The overlay value is
        // byte-identical to the eventual history payload: the raw value for
        // `Set`, an empty `Bytes` tombstone for `Remove`.
        for op in ops {
            match op {
                KvOp::Set(k, v) => self.overlay.insert(k.clone(), commit_version, v.clone()),
                KvOp::Remove(k) => self.overlay.insert(k.clone(), commit_version, Bytes::new()),
            }
        }

        // Update the in-memory cell for every touched key (CRIT-2: entry_async
        // modify-or-insert). After this, readers at `>= commit_version` resolve
        // the value from the overlay.
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k.clone(),
                KvOp::Remove(k) => k.clone(),
            };
            // publish_cell is async only because of scc's entry_async; it does
            // no I/O. Block-free in practice.
            self.publish_cell_sync(key, commit_version);
        }

        // R3: advance the reader-visible floor so subsequent `get_current` /
        // `current_stream` see the materialized version. In the tx commit path
        // the caller (`commit_tx`) ALSO publishes via the watermark; this
        // monotonic fetch_max is safe to call redundantly.
        self.gate.publish_committed_max(commit_version);
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
    pub async fn write_committed_to_history(
        &self,
        ops: &[KvOp],
        commit_version: u64,
    ) -> DbResult<()> {
        // HIGH-3: batch the physical writes through `Store::transact`.
        // Per-op `set`/`remove` collapses to a single atomic write-tx on
        // backends that override `transact` — one fsync instead of N.
        //
        // C1: every committed key gets a log entry unconditionally — the log
        // is the universal version timeline. For KvOp::Remove the log entry is
        // a tombstone (empty value).
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
        for op in ops {
            let h_key = match op {
                KvOp::Set(k, v) => KvOp::Set(encode_version_key(k, commit_version), v.clone()),
                KvOp::Remove(k) => KvOp::Set(encode_version_key(k, commit_version), Bytes::new()),
            };
            history_ops.push(h_key);
        }

        // One batched write to history (current version + tombstones).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // T1c: record the commit timestamp. D2 P1d-2b: prefer the COMMIT-TIME
        // millis stamped on the ack-path (`apply_committed_visible`) so the
        // durable ts reflects commit time, not this (later) drain time. Cold
        // recovery (overlay empty → no stamp) falls back to `now_millis()` —
        // the pre-cutover behaviour (recovery never had the original ts).
        // Remove the stamp once consumed (idempotent: a re-drain finds none and
        // uses the fallback, but by then history already holds the correct ts).
        let ts_ms = self
            .pending_ts
            .remove(&commit_version)
            .map(|(_, ms)| ms)
            .unwrap_or_else(|| self.now_millis());
        self.record_ts_at(commit_version, ts_ms).await;

        // Seed the cell from the durable write (idempotent; needed on the cold
        // recovery path so a key whose value lives only in history reports the
        // correct version). On the warm drain path the cell is already at
        // `commit_version` (set by the ack-path's `apply_committed_visible`),
        // so this is a redundant no-op.
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k.clone(),
                KvOp::Remove(k) => k.clone(),
            };
            self.publish_cell(key, commit_version).await;
        }

        // R3: advance the reader-visible floor (monotonic fetch_max). On the
        // ack-driven flow the floor is already at/above this; on cold recovery
        // this lifts it.
        self.gate.publish_committed_max(commit_version);
        Ok(())
    }
}
