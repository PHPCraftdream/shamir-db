use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_tunables::store_defaults::HISTORY_SCAN_BATCH;

use super::version_entry::VersionEntry;
use super::MvccStore;
use crate::version_codec::{decode_version_key, encode_version_key};

impl MvccStore {
    /// Range-scan the log for the latest version ≤ `snapshot`.
    /// Returns `None` for tombstones (empty value) and absent keys.
    pub(super) async fn scan_history_for_version(
        &self,
        key: &[u8],
        snapshot: u64,
    ) -> DbResult<Option<Bytes>> {
        let lo = encode_version_key(key, 0);
        let hi = encode_version_key(key, snapshot);
        let stream = self
            .history
            .iter_range_stream(Some(lo), Some(hi), HISTORY_SCAN_BATCH);
        futures::pin_mut!(stream);
        let mut latest: Option<Bytes> = None;
        while let Some(batch) = stream.next().await {
            for (_, val) in batch? {
                latest = Some(val);
            }
        }
        match latest {
            Some(val) if val.is_empty() => Ok(None),
            other => Ok(other),
        }
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
            .upsert_async(key, super::RecordCell { version })
            .await;
    }

    /// cancel-safe: NO — applies a batch of `KvOp` via multi-step sequences
    /// (history transact, version_cache updates). One durable write (history
    /// transact). Cancellation mid-batch leaves some phases applied, others
    /// not. Recovery relies on WAL replay.
    pub async fn apply_committed_ops(&self, ops: Vec<KvOp>, commit_version: u64) -> DbResult<()> {
        // HIGH-3: batch the physical writes through `Store::transact`.
        // Per-op `set`/`remove` collapses to a single atomic write-tx
        // on backends that override `transact` (redb, sled, fjall,
        // persy, nebari, canopy) — one fsync instead of N.

        // C1: every committed key gets a log entry unconditionally (no
        // longer gated by `active_snapshots_empty`) — the log is the
        // universal version timeline. For KvOp::Remove the log entry is
        // a tombstone (empty value).
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
        for op in &ops {
            let h_key = match op {
                KvOp::Set(k, v) => KvOp::Set(encode_version_key(k, commit_version), v.clone()),
                KvOp::Remove(k) => KvOp::Set(encode_version_key(k, commit_version), Bytes::new()),
            };
            history_ops.push(h_key);
        }

        // One batched write to history (current version + tombstones).
        // The log is the sole durable write target.
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // T1c: record the commit timestamp for the tx commit version (one ts
        // per commit — all ops share `commit_version`). Best-effort.
        self.record_ts(commit_version).await;

        // Update the in-memory cell for every touched key.
        // Sets both `version` and `hwm` to `commit_version` so that
        // tx-committed keys participate in index-only freshness validation.
        // Uses `publish_cell` (entry_async modify-or-insert, CRIT-2):
        // `upsert_async` was previously used; entry_async is equivalent and
        // preserves both fields.
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k,
                KvOp::Remove(k) => k,
            };
            self.publish_cell(key, commit_version).await;
        }
        Ok(())
    }
}
