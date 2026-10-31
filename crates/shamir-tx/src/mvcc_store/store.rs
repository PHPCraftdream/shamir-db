//! Core `MvccStore` type: struct definition, construction, overlay/ts-index
//! bookkeeping, and the versioned write paths (`set_versioned*`,
//! `delete_versioned*`).

use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use bytes::{BufMut, Bytes, BytesMut};
use futures::StreamExt;
use scc::HashMap as SccHashMap;
use scc::TreeIndex;
use shamir_collections::THasher;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;

use super::key_lock::KeyLock as KeyLockInner;
use super::Retention;
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;
use crate::version_guard::VersionGuard;
use crate::versioned_overlay::VersionedOverlay;

// ============================================================================
// T1c — per-version commit timestamp namespace.
//
// The commit time of each version is stored in the EXISTING `history` store
// under a separate 9-byte key: `[TS_TAG][version_be: 8 bytes]`. This keeps
// value formats and `get_at` untouched and requires no constructor change.
//
// `TS_TAG = 0x00` is distinct from `VERSION_SEP = 0xFF` (used by
// `encode_version_key`), and a 9-byte ts-key can never collide with a real
// version-key: version-keys are `record_key(>=1 byte) || 0xFF || version_be(8)`
// = at least 10 bytes. `decode_version_key` rejects ts-keys because for a
// 9-byte ts-key `split = 0` and `physical[0] = TS_TAG = 0x00 != VERSION_SEP`.
// ============================================================================

/// Leading tag byte for a timestamp key. Chosen != `VERSION_SEP` (0xFF) and
/// such that a 9-byte ts-key cannot be mistaken for a version-key.
pub(crate) const TS_TAG: u8 = 0x00;

/// Encode a timestamp key for `version`: `[TS_TAG][version_be: 8 bytes]`.
pub(crate) fn ts_key(version: u64) -> Bytes {
    let mut b = BytesMut::with_capacity(9);
    b.put_u8(TS_TAG);
    b.put_u64(version);
    b.freeze()
}

/// Decode a timestamp key back into its version. Returns `None` if the input
/// is not a 9-byte ts-key (`[TS_TAG][8 bytes]`). Used by retention tests;
/// kept as the logical inverse of [`ts_key`].
#[allow(dead_code)]
pub(crate) fn decode_ts_key(physical: &[u8]) -> Option<u64> {
    if physical.len() != 9 || physical[0] != TS_TAG {
        return None;
    }
    let version_bytes: [u8; 8] = physical[1..].try_into().expect("just checked length");
    Some(u64::from_be_bytes(version_bytes))
}

/// Per-key in-memory coordination state — the "record cell".
/// The durable data lives in the single `history` version-log; the cell
/// is rebuildable from the log and held in memory for fast coordination.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordCell {
    /// The latest version assigned to this key. Set on every write path
    /// before the physical store mutation. Read by get_at / current_version /
    /// version_of / live_version.
    pub(crate) version: u64,
    /// A10 vacuum anchor deferral: the version of the immediately-PRIOR
    /// write that the L6 fast-path vacuum has DEFERRED deletion for.
    /// `0` = nothing deferred. The fast path does NOT delete `old_v` in the
    /// same call that created it — instead it stores `old_v` here and only
    /// physically deletes the PREVIOUSLY-deferred version (if any) on the
    /// NEXT vacuum_key call. This one-generation slack means the
    /// immediately-prior version is ALWAYS present in history for at
    /// least one more write cycle, closing the TOCTOU race: a reader that
    /// "just missed" registering in `active_snapshots` before vacuum ran
    /// will still find its version in the log.
    pub(crate) vacuum_anchor: u64,
    /// Its `txn_id` identifies a writer that has claimed this cell as the
    /// serialization point for a write-write conflict. Competing reservations
    /// fail, while reads continue to use `version` only; this marker is not a
    /// visibility boundary.
    pub(crate) reserved_by: u64,
}

/// Versioned layer over the history version log.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    pub(crate) history: Arc<dyn Store>,
    pub(crate) gate: Arc<RepoTxGate>,
    /// In-memory coordination state: key → record cell (latest committed version).
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    ///
    /// Keyed on [`RecordKey`] (task #532): the commit / publish / reservation
    /// paths already hold a `RecordKey`, so this avoids the redundant
    /// `RecordKey → Bytes → RecordKey` heap round-trip that a `Bytes` key
    /// forced on every op. `RecordKey` hashes / compares byte-identically to
    /// the equivalent `Bytes` (both go through the raw slice), so `&[u8]`
    /// probes (`current_version` / `live_version`) still work via
    /// `Borrow<[u8]>`.
    pub(crate) cells: SccHashMap<RecordKey, RecordCell, THasher>,
    /// Level-3 pessimistic lock registry. Populated ONLY for keys locked by a
    /// `Pessimistic` tx; stays empty otherwise → zero overhead on the snapshot
    /// / serializable read/write hot paths. Each entry is an `Arc<KeyLock>`
    /// shared between concurrent requesters of the same key.
    pub(crate) locks: SccHashMap<RecordKey, Arc<KeyLockInner>, THasher>,
    /// T1b.2: history-retention policy (lock-free `ArcSwap<Retention>`).
    /// Defaults to [`Retention::current_only`] (eager vacuum). Set via
    /// [`Self::set_retention`].
    pub(crate) retention: ArcSwap<Retention>,
    /// T1c: wall-clock millis source for per-version commit timestamps.
    /// `0` = use the real clock (`SystemTime`); a non-zero frozen value is
    /// for deterministic retention tests (see [`Self::set_test_now`]).
    /// Retention is calendar time (wall clock), so `SystemTime` is correct
    /// here — NOT a monotonic clock (we need to reason about "60 seconds ago").
    pub(crate) test_now_millis: AtomicU64,
    /// The production overlay contains visible writes not yet drained to the
    /// durable history log. Read paths consult it before the history store.
    pub(crate) overlay: VersionedOverlay,
    /// D2 P1d-2b: per-version COMMIT-TIME timestamp, stamped on the ack-path
    /// (`apply_committed_visible`) and READ (non-destructively) by the drainer
    /// (`write_committed_to_history` / `write_committed_batch_to_history`).
    /// The cutover moved the durable `record_ts` write OFF the ack-path into
    /// the drainer, but the ts VALUE must reflect COMMIT time, not drain time
    /// (the drainer runs arbitrarily later, and a frozen test clock or a real
    /// clock that advanced would mis-stamp the version — breaking
    /// age-retention / as-of-by-ts). So we capture the commit-time millis
    /// here at ack and the drainer writes THAT into history.
    ///
    /// A14: entries are read NON-DESTRUCTIVELY (a plain `get`, not `remove`)
    /// so that two independent drain paths racing for the SAME version (the
    /// background drainer vs. a forced `flush_buffers`/`drain_to_history`)
    /// both observe the correct commit-time ts — a destructive `remove` made
    /// the second caller fall back to `now_millis()` and overwrite the
    /// correct ts. Reclamation is decoupled: [`Self::gc_overlay_to`] sweeps
    /// the stamp once the version is durable. Cold recovery (overlay empty)
    /// finds no entry and falls back to `now_millis()` — its conservative
    /// pre-cutover behaviour (recovery already could not reconstruct the
    /// original ts).
    pub(crate) pending_ts: SccHashMap<u64, u64, THasher>,
    /// L6: set `true` when a snapshot is (or was recently) active during a
    /// vacuum_key call. The targeted-remove fast path fires only when this is
    /// `false` (no accumulated old versions from a snapshot epoch). Cleared
    /// after a full scan-path vacuum runs with `active_snapshots_empty()`.
    pub(crate) vacuum_needs_scan: AtomicBool,
    /// Phase 3: in-memory ts-ordered index for `version_at_or_before_ts`.
    /// Key: `(Reverse(ts_millis), Reverse(version))` — reversed so that a forward
    /// `range((Reverse(target_ts), Reverse(u64::MAX)).., &guard).next()` yields the
    /// entry with the largest version whose ts ≤ target_ts, in O(log N + K), where
    /// K is the number of qualifying timestamps. Value is unit (the version
    /// is embedded in the key). Rebuilt from `history` ts-keys on first query if empty.
    pub(crate) ts_index: TreeIndex<(Reverse<u64>, Reverse<u64>), ()>,
    /// Whether the ts_index has been populated from history (lazy rebuild on first query).
    pub(crate) ts_index_ready: AtomicBool,
    /// #1032: mutual exclusion between the two independent paths that both
    /// write to `history` for this table — the background [`Drainer`]'s
    /// periodic [`Self::write_committed_batch_to_history`] and a forced,
    /// synchronous [`Self::drain_to_history`] (table RENAME's Phase F.2).
    /// Both ultimately call `self.history.transact(...)`; a real CI
    /// reproduction (task #1032) showed the two calls landing in the same
    /// second can hang indefinitely — the doc at A14 (`pending_ts`, above)
    /// already anticipated the two paths racing for the SAME data and
    /// judged it data-safe (idempotent ops), but never anticipated the
    /// underlying storage layer hanging under two genuinely concurrent
    /// `transact` calls. `drain_to_history` holds this for its ENTIRE
    /// duration (a one-shot admin op, low frequency, `tokio::sync::Mutex`
    /// is the sanctioned exception for "guard held across `.await`,
    /// bounded contention" per CLAUDE.md); `write_committed_batch_to_history`
    /// (the Drainer's periodic path) takes it for the duration of its batch
    /// write too. It must WAIT (#1234): an earlier `try_lock` deferral
    /// returned `Ok(())` without writing, and `Drainer::drain_step` then
    /// finalized the entries (durable mark, WAL commit, overlay GC) as if
    /// they had landed.
    pub(crate) drain_exclusive: tokio::sync::Mutex<()>,
}

impl MvccStore {
    /// Create a new MVCC store from a history store and a gate.
    ///
    /// Defaults to [`Retention::current_only`] (eager vacuum). Use
    /// [`Self::set_retention`] to opt into [`Retention::keep_history`] or a
    /// custom [`Retention`].
    pub fn new(history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self {
        Self {
            history,
            gate,
            cells: SccHashMap::with_hasher(THasher::default()),
            locks: SccHashMap::with_hasher(THasher::default()),
            retention: ArcSwap::new(Arc::new(Retention::current_only())),
            test_now_millis: AtomicU64::new(0),
            overlay: VersionedOverlay::new(),
            pending_ts: SccHashMap::with_hasher(THasher::default()),
            vacuum_needs_scan: AtomicBool::new(false),
            ts_index: TreeIndex::new(),
            ts_index_ready: AtomicBool::new(false),
            drain_exclusive: tokio::sync::Mutex::new(()),
        }
    }

    /// #1032 test seam: hold `drain_exclusive` externally so a test can
    /// deterministically prove `write_committed_batch_to_history`/
    /// `drain_to_history` back off / block on contention, without timing.
    /// Intra-crate only (`crate::mvcc_store::tests`) — `#[cfg(test)]`
    /// is correct here, unlike the cross-crate pause hooks elsewhere in this
    /// codebase that must stay production-compiled.
    #[cfg(test)]
    pub(crate) async fn lock_drain_exclusive_for_test(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.drain_exclusive.lock().await
    }

    /// F-71 (#898): the repo-wide committed watermark this store's `gate`
    /// currently exposes — identical to `RepoTxGate::last_committed()`, which
    /// is not reachable from outside this crate (`gate` is `pub(super)`).
    ///
    /// Added for callers (e.g. `shamir-engine`'s sorted-index CREATE INDEX
    /// backfill, `TableManager::create_sorted_index_with_include` →
    /// `SortedIndexManager::mark_ready_at`) that need "the table's current
    /// committed version" but only hold an `Arc<MvccStore>`, not the
    /// `Arc<RepoTxGate>` directly (that Arc is threaded separately via
    /// `TableManager`'s optional `changefeed: Option<NonTxChangefeed>`, which
    /// is `None` for plenty of legitimate MVCC-backed tables — e.g. every
    /// test harness that skips `with_changefeed`, and any production table
    /// wired without SSI). Deriving this from the ALREADY-attached
    /// `MvccStore` instead means the watermark used to floor a freshly
    /// backfilled index's AsOf epoch is available whenever an `MvccStore` is
    /// attached at all, not only when the separate (and narrower) changefeed
    /// wiring happens to also be present.
    pub fn current_committed_version(&self) -> u64 {
        self.gate.last_committed()
    }

    /// Access the per-table versioned overlay. Production write paths populate
    /// it as commits become visible; tests also use it for focused merge cases.
    #[allow(dead_code)]
    pub(crate) fn overlay(&self) -> &VersionedOverlay {
        &self.overlay
    }

    /// D2 P1e — drop every overlay entry whose `version <= durable_watermark`.
    ///
    /// Called by the background [`Drainer`] after a drain pass advances the
    /// repo's `durable_watermark`. A version `V <= durable_watermark` is, by
    /// the drain contract, already durable in the `history` log — so its
    /// overlay copy is redundant and can be dropped SAFELY regardless of any
    /// active reader: a snapshot at or above `V` resolves the value from
    /// history (`resolve_read` / `get_current` / `current_stream` all fall
    /// back to `history` on an overlay miss), and a snapshot below `V` never
    /// observed the overlay-only window in the first place. No
    /// `min_active_snapshot` floor is needed here — that floor governs
    /// history-retention / vacuum (how long OLD versions stay in `history`),
    /// not the overlay, which is a pure RYOW read-cache for the
    /// not-yet-durable tail.
    ///
    /// Uses `overlay.gc_upto(durable_watermark, u64::MAX)` so the effective
    /// threshold is EXACTLY `durable_watermark` (the second `floor` argument
    /// is redundant for overlay GC — see [`VersionedOverlay::gc_upto`]).
    ///
    /// Also evicts any `pending_ts` commit-time stamp for `version <=
    /// durable_watermark`. A14: the drain paths read the stamp
    /// NON-DESTRUCTIVELY (so multiple racers observe the same commit-time
    /// ts), so this sweep is the SOLE reclamation site — it is NOT
    /// defensive-only. On the warm drain path the stamp was inserted on the
    /// ack-path and read by the drainer; this `retain` removes it once the
    /// version is durable, preventing an unbounded leak (a stamp that was
    /// inserted but — for any reason — never read, e.g. a non-drained direct
    /// write or a future code path, is also swept here). Lock-free.
    pub fn gc_overlay_to(&self, durable_watermark: u64) {
        if durable_watermark == 0 {
            return;
        }
        self.overlay.gc_upto(durable_watermark, u64::MAX);

        // pending_ts hygiene: remove any stamp for an already-durable version.
        // `retain` is a lock-free sweep over the (small) map; on the common
        // path it finds nothing (the drain half already removed each stamp it
        // consumed), so this is cheap.
        self.pending_ts.retain_sync(|v, _| *v > durable_watermark);
    }

    /// D2 P1e — number of entries currently in the in-memory overlay.
    ///
    /// Telemetry / cross-crate test accessor: the engine-side overlay-GC tests
    /// (`shamir-engine`) assert the overlay shrinks to the `(durable_watermark,
    /// last_committed]` window after a drain pass, and `overlay()` itself is
    /// `pub(crate)` (not reachable from another crate). Lock-free atomic load.
    pub fn overlay_len(&self) -> usize {
        self.overlay.len()
    }

    /// D2 P1e — number of pending commit-time stamps awaiting drain.
    ///
    /// Telemetry / cross-crate test accessor mirroring [`Self::overlay_len`]:
    /// the overlay-GC tests assert `pending_ts` is also reclaimed once the
    /// versions it stamps fall at or below the durable watermark. Lock-free.
    #[allow(clippy::disallowed_methods)] // O(N) ack: telemetry/test accessor, off hot path
    pub fn pending_ts_len(&self) -> usize {
        self.pending_ts.len()
    }

    /// Phase 3: insert a (ts_millis, version) entry into the in-memory ts-index.
    /// Lock-free (TreeIndex::insert is a CAS-based B+ tree operation).
    pub fn ts_index_insert(&self, ts_millis: u64, version: u64) {
        let _ = self
            .ts_index
            .insert_sync((Reverse(ts_millis), Reverse(version)), ());
    }

    /// Audit 2.1: drop a single `(ts_millis, version)` entry from the
    /// in-memory ts-index. Called at every history-reclaim site in lockstep
    /// with the `ts_key(version)` removal so the ts-index never outlives the
    /// history versions it maps (the SAME "no orphan timestamps" invariant
    /// already enforced for the durable `ts_key`).
    ///
    /// Without this, `ts_index` grew by one entry on EVERY committed version
    /// with no eviction path — unbounded memory under sustained write load.
    /// The reclaim sites already prove the version is beyond every live
    /// snapshot's floor (sacred-floor / anchor logic), so an as-of-ts query
    /// that would have resolved to `version` could no longer read its value
    /// anyway (its history entry is being deleted in the same pass) — pruning
    /// the stale index entry is strictly consistency-preserving.
    ///
    /// Lock-free (`TreeIndex::remove` is a CAS-based B+ tree operation).
    pub(crate) fn ts_index_remove(&self, ts_millis: u64, version: u64) {
        self.ts_index
            .remove_sync(&(Reverse(ts_millis), Reverse(version)));
    }

    /// Audit 2.1: number of live entries in the in-memory ts-index.
    /// Telemetry / test accessor — asserts the index shrinks after a
    /// vacuum/gc/purge pass instead of growing without bound. Off the hot
    /// path; `TreeIndex::len` is O(N) but only invoked by tests / diagnostics.
    #[allow(clippy::disallowed_methods)] // O(N) ack: telemetry/test accessor, off hot path
    pub fn ts_index_len(&self) -> usize {
        self.ts_index.len()
    }

    /// Phase 3: query the ts-index for the largest version whose commit ts ≤ target.
    /// Returns `None` if the index is empty or no entry satisfies the bound.
    /// O(log N + K), where K is the number of timestamps at or below target.
    pub(crate) fn ts_index_query(&self, target_ts: u64) -> Option<u64> {
        let guard = scc::Guard::new();
        self.ts_index
            .range((Reverse(target_ts), Reverse(u64::MAX)).., &guard)
            .map(|(k, _)| k.1 .0)
            .max()
    }

    /// Phase 3: rebuild the ts-index from the history store's ts-keys.
    /// Called lazily on first `version_at_or_before_ts` if `ts_index_ready` is false.
    pub(crate) async fn ts_index_rebuild(&self) -> DbResult<()> {
        // Clear any partial entries from a previous failed rebuild before retrying.
        self.ts_index_ready.store(false, Ordering::Release);
        let stream = self.history.iter_stream(MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

        while let Some(batch) = stream.next().await {
            let batch = batch?;
            for (phys_key, val) in batch {
                // ts-keys are exactly 9 bytes: [TS_TAG][version_be: 8].
                if phys_key.len() != 9 || phys_key[0] != TS_TAG {
                    continue;
                }
                if val.len() != 8 {
                    continue;
                }
                let ts_bytes: [u8; 8] = match val.as_ref().try_into() {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let recorded_ts = u64::from_le_bytes(ts_bytes);
                let v_bytes: [u8; 8] = phys_key[1..9].try_into().expect("checked len==9");
                let version = u64::from_be_bytes(v_bytes);
                self.ts_index_insert(recorded_ts, version);
            }
        }
        self.ts_index_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// Approximate number of live in-memory record cells (keys with a
    /// current version). Formerly used by `RENAME TABLE` to refuse
    /// populated-table renames; Phase F.2 replaced that guard with
    /// [`drain_to_history`](Self::drain_to_history) so populated renames
    /// now work. Retained as a diagnostic / test accessor.
    #[allow(dead_code)]
    pub fn cell_count(&self) -> usize {
        // O(N) ack: RENAME is a one-shot admin op (off hot-path); a single
        // traversal to count live cells and refuse a populated-table rename
        // is acceptable here — no AtomicUsize mirror is warranted for it.
        #[allow(clippy::disallowed_methods)]
        self.cells.len()
    }

    /// T4-purge: the store's current wall-clock millis (test-overridable
    /// via [`Self::set_test_now`]). Exposed so the PurgeHistory executor
    /// can resolve `OlderThanAge { age_secs }` against the SAME clock
    /// that stamped each version's commit ts — keeping age-based purge
    /// deterministic under `set_test_now`.
    pub fn clock_millis(&self) -> u64 {
        self.now_millis()
    }
    /// Resolve a versioned read of `key` visible at `snapshot_version`, given
    /// its current cached version `cur_v`. Direct path (`cur_v > 0 && cur_v <= snapshot`):
    /// read the log at `encode_version_key(key, cur_v)`. Fallback: range-scan
    /// the log for the newest version `<= snapshot`. The lookup checks the
    /// visible overlay first, then falls back to durable history.
    pub(crate) async fn resolve_read(
        &self,
        key: &[u8],
        snapshot_version: u64,
        cur_v: u64,
    ) -> DbResult<Option<Bytes>> {
        if cur_v > 0 && cur_v <= snapshot_version {
            // P1b: overlay-probe BEFORE the durable log. The overlay holds the
            // visible-but-not-yet-drained value for `(key, cur_v)`. A hit
            // (empty = tombstone → None, non-empty → Some) short-circuits; a
            // miss falls through to history.
            if let Some(val) = self.overlay.get(key, cur_v) {
                return Ok(if val.is_empty() { None } else { Some(val) });
            }
            return match self
                .history
                .get(encode_version_key(key, cur_v).into())
                .await
            {
                Ok(val) if val.is_empty() => Ok(None),
                Ok(val) => Ok(Some(val)),
                // RV-TX-2CB455A7: the cell is bumped before the log write, so
                // `cur_v` is an UPPER BOUND on the committed version — an
                // exact-version miss means that version was attempted and
                // aborted. Fall back below it instead of masking the prior
                // committed value.
                Err(DbError::NotFound(_)) if cur_v > 0 => {
                    // Boxed: the fallback can recurse one more level.
                    Box::pin(self.resolve_read(key, snapshot_version.min(cur_v - 1), 0)).await
                }
                Err(DbError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            };
        }
        // Fallback path: newest version ≤ snapshot from EITHER source wins.
        // The overlay holds the newest versions; history holds the durable
        // tail. Take the larger version (versions are globally unique, so no
        // tie). A tombstone winner (newer delete) still beats an older
        // non-tombstone write — the delete is more recent.
        let overlay_hit = self.overlay.newest_visible(key, snapshot_version);
        let history_hit = self.scan_history_newest(key, snapshot_version).await?;
        match (overlay_hit, history_hit) {
            (Some((ov_ver, ov_val)), Some((h_ver, h_val))) => {
                if ov_ver >= h_ver {
                    Ok(if ov_val.is_empty() {
                        None
                    } else {
                        Some(ov_val)
                    })
                } else {
                    Ok(if h_val.is_empty() { None } else { Some(h_val) })
                }
            }
            (Some((_, ov_val)), None) => Ok(if ov_val.is_empty() {
                None
            } else {
                Some(ov_val)
            }),
            (None, Some((_, h_val))) => Ok(if h_val.is_empty() { None } else { Some(h_val) }),
            (None, None) => Ok(None),
        }
    }

    /// cancel-safe: NO — multi-step state mutation. The log is the sole
    /// durable write. Cancellation after `publish_cell` but before
    /// `history.set` leaves the in-memory cell advanced but the log not yet
    /// written; the caller must retry or WAL-replay to converge.
    ///
    /// Returns the monotonic version assigned to this write (from the
    /// shared `RepoTxGate` counter).
    ///
    /// Cancelling the acknowledgement wait after commit does not undo or
    /// reapply the write; it only forfeits acknowledgement. Observe the
    /// committed version before deciding whether to retry.
    ///
    pub async fn set_versioned(&self, key: RecordKey, value: Bytes) -> DbResult<u64> {
        self.set_versioned_inner(key, value, true).await
    }

    /// Guarded engine callers only: do not wait for W while holding a writer
    /// drain guard or unique-write lock. A lower post-WAL orphan may need
    /// that same barrier to replay its index postings and settle.
    #[doc(hidden)]
    pub async fn set_versioned_no_wait(&self, key: RecordKey, value: Bytes) -> DbResult<u64> {
        self.set_versioned_inner(key, value, false).await
    }

    /// RV-TX-2CB455A7: fail-stop refusal once a failed write's compensating
    /// cleanup could not settle its versions.
    fn ensure_writable(&self) -> DbResult<()> {
        if self.gate.writes_fenced() {
            return Err(DbError::RepoRequiresReopen(
                "failed-write cleanup left versions unsettled; writes refused".into(),
            ));
        }
        Ok(())
    }

    /// RV-TX-2CB455A7: compensating cleanup after a failed `history.transact`.
    /// Removes every attempted version key and ts key BEFORE the guards settle
    /// as Aborted, so the failed versions cannot resolve as present. If the
    /// removal itself fails the versions are NOT settled: fail-stop — the
    /// guards are never dropped (the floor stalls below them),
    /// `requires_reopen` is set and further writes are refused.
    async fn compensate_failed_write<'a>(
        &self,
        guards: Vec<VersionGuard>,
        keys: impl IntoIterator<Item = &'a RecordKey>,
        versions: &[u64],
    ) {
        let mut ops: Vec<KvOp> = Vec::with_capacity(versions.len() * 2);
        for (key, &v) in keys.into_iter().zip(versions.iter()) {
            ops.push(KvOp::Remove(encode_version_key(key, v).into()));
            ops.push(KvOp::Remove(ts_key(v).into()));
        }
        if let Err(e) = self.history.transact(ops).await {
            for guard in guards {
                // Keep the guards armed: their versions stay unsettled so the
                // committed floor stalls below them (fail-stop).
                std::mem::forget(guard);
            }
            self.gate.fence_writes();
            log::warn!("failed-write cleanup failed ({e}); store requires reopen");
        } else {
            drop(guards);
        }
    }

    async fn set_versioned_inner(&self, key: RecordKey, value: Bytes, wait: bool) -> DbResult<u64> {
        if value.is_empty() {
            return Err(DbError::Validation("empty value is a tombstone".into()));
        }
        self.ensure_writable()?;
        // Single log append: every prior version is already in the log from
        // when it was written as current. No other store is written.
        //
        // Bump-first: assign version, update cell, then perform the physical
        // write. CRIT-2: `publish_cell` uses entry_sync (modify-or-insert)
        // so repeated writes to the same key advance the cached version
        // monotonically.
        // P0b: take a RAII VersionGuard so the non-tx path goes through the
        // SAME CompletionTracker as tx commits. The guard owns the terminal
        // mark obligation (Materialized on `commit()`, Aborted on any early
        // return / `?`-propagated error / panic before commit). This is the
        // H5 unification: the watermark — not a direct `raise_published`
        // — is the single source of truth for visibility.
        // L6: capture old_v BEFORE publish_cell bumps the cell — this is the
        // prior version that targeted vacuum will remove.
        let old_v = self.current_version(&key);
        let guard = self.gate.assign_next_version_guarded();
        let new_v = guard.version();
        self.publish_cell(key.clone(), new_v).await;
        let key_snapshot = key.clone();
        // L2: single atomic log write — data + ts in one transact call.
        // T1c: capture commit timestamp once for the age-retention axis.
        let ts_ms = self.now_millis();
        let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());
        let write_ops = vec![
            KvOp::Set(
                encode_version_key(&key_snapshot, new_v).into(),
                value.clone(),
            ),
            KvOp::Set(ts_key(new_v).into(), ts_val),
        ];
        if let Err(e) = self.history.transact(write_ops).await {
            // RV-TX-2CB455A7: remove the attempted version+ts keys before the
            // guard settles as Aborted, so the failed version is absent.
            self.compensate_failed_write(vec![guard], std::iter::once(&key_snapshot), &[new_v])
                .await;
            return Err(e);
        }
        // Phase 3: maintain the in-memory ts-index for as-of queries.
        self.ts_index_insert(ts_ms, new_v);
        // P1c: dual-write — populate the overlay with the SAME (key, version)
        // → value pair, AFTER the durable history write succeeds but BEFORE
        // `guard.commit()` advances the reader-visible floor. Ordering rationale:
        //  • after history.transact: a failed history write (`?` above) must NOT
        //    leave a visible overlay entry — otherwise an aborted write would
        //    surface via `get_current` (the cell is already bumped). Inserting
        //    post-write keeps "history.transact failed ⇒ nothing written" intact.
        //  • before guard.commit: by the time the version becomes reader-visible
        //    (floor advanced / Materialized), the overlay carries its value.
        //  • no empty-window: between history.transact returning and this insert
        //    the cell reports `new_v` but history already holds the value, so any
        //    read resolves from history.
        self.overlay.insert(key.clone(), new_v, value);
        // Mark the version Materialized and advance the reader-visible floor
        // from the resulting watermark (mirrors the tx commit path). This
        // replaces the direct `raise_published(new_v)`.
        guard.commit();
        // P1d-1: the history write above is the durable-history landing for
        // this version (synchronous inline path); mark durable AFTER the
        // visibility mark so `durable_watermark() <= last_committed()` holds
        // at every observation point. Under inline materialize this keeps
        // the two watermarks in lock-step; P1d-2 will move tx-path history
        // writes to a background drain and the non-tx path keeps marking
        // durable inline (its best-effort / no-WAL contract is unchanged).
        self.gate.mark_durable(new_v);
        // T1b.2 + L6: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive). old_v is the
        // prior version captured before publish_cell — enables targeted remove.
        if self.vacuum_key(&key_snapshot, old_v).await.is_err() {
            self.vacuum_needs_scan.store(true, Ordering::Release);
        }
        if wait {
            self.strict_ack(new_v).await?;
        }
        Ok(new_v)
    }

    /// cancel-safe: NO — multi-step state mutation. The log is the sole
    /// durable write. Cancellation mid-sequence (after some cells are published
    /// but before the single history `transact` completes) leaves the store
    /// partial; recovery is caller-side retry / WAL replay.
    ///
    /// Batched non-tx write of many `(key, value)` pairs — the bulk-load
    /// twin of [`set_versioned`]. Collapses all log writes into a single
    /// `history.transact` (one atomic write-tx, one fsync on backends
    /// that override `transact`). No other store is written.
    ///
    /// Semantics match calling [`set_versioned`] once per pair, in order:
    /// assign a fresh monotonic version per key, then append all version-key
    /// entries into the log in one batched transact (one version per record,
    /// identical to the per-record loop).
    ///
    /// Empty `items` is a no-op.
    /// Returns the maximum version assigned across the batch (one
    /// version per record). The returned value is the commit-version a
    /// changefeed event should carry.
    ///
    /// Cancelling only the post-commit acknowledgement wait preserves every
    /// committed batch version; do not blindly retry the batch.
    pub async fn set_versioned_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<u64> {
        self.set_versioned_many_inner(items, true).await
    }

    /// Guarded engine/migration callers only; avoids the lower-orphan
    /// posting-replay barrier cycle described by [`Self::set_versioned_no_wait`].
    #[doc(hidden)]
    pub async fn set_versioned_many_no_wait(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<u64> {
        self.set_versioned_many_inner(items, false).await
    }

    async fn set_versioned_many_inner(
        &self,
        items: Vec<(RecordKey, Bytes)>,
        wait: bool,
    ) -> DbResult<u64> {
        if items.is_empty() {
            // No records written — return 0. The caller should not emit
            // a changefeed event for an empty batch.
            return Ok(0);
        }
        self.ensure_writable()?;
        if items.iter().any(|(_, value)| value.is_empty()) {
            return Err(DbError::Validation("empty value is a tombstone".into()));
        }

        // Phase 1 (bump-first): assign a fresh version per key and update the
        // cell BEFORE the physical log write. Prior versions are already in
        // the log from when they were written as current.
        // CRIT-2: `publish_cell` uses entry_sync modify-or-insert so the
        // cached version advances monotonically.
        let mut max_v = 0u64;
        let mut new_versions: Vec<u64> = Vec::with_capacity(items.len());
        // L6: capture old_v per key BEFORE publish_cell bumps the cell.
        let mut old_versions: Vec<u64> = Vec::with_capacity(items.len());
        // P0b: one VersionGuard per allocated version. If the batched
        // `history.transact` below fails (`?`), every guard drops un-committed
        // and marks its version Aborted, so the contiguous watermark advances
        // past the whole failed batch instead of wedging at the first version.
        let mut guards: Vec<VersionGuard> = Vec::with_capacity(items.len());
        // L2: single atomic log write — data + ts for every version in one
        // transact call. T1c: capture commit timestamp once for the whole batch.
        let ts_ms = self.now_millis();
        let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());
        // capacity: one data-op + one ts-op per item.
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(items.len() * 2);
        for (key, value) in &items {
            old_versions.push(self.current_version(key));
            let guard = self.gate.assign_next_version_guarded();
            let new_v = guard.version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            guards.push(guard);
            max_v = new_v;
            history_ops.push(KvOp::Set(
                encode_version_key(key, new_v).into(),
                value.clone(),
            ));
            history_ops.push(KvOp::Set(ts_key(new_v).into(), ts_val.clone()));
        }

        if !history_ops.is_empty() {
            if let Err(e) = self.history.transact(history_ops).await {
                // RV-TX-2CB455A7: remove every attempted version+ts key
                // before the guards settle as Aborted.
                self.compensate_failed_write(
                    guards,
                    items.iter().map(|(key, _)| key),
                    &new_versions,
                )
                .await;
                return Err(e);
            }
        }

        // Phase 3: maintain the in-memory ts-index for every version in the batch.
        for &new_v in &new_versions {
            self.ts_index_insert(ts_ms, new_v);
        }

        // P1c: dual-write — populate the overlay with the SAME (key, version) →
        // value pairs, AFTER the batched `history.transact` succeeds and BEFORE
        // the `guard.commit()` loop advances the floor. Post-transact so a failed
        // batch write (`?` above) leaves no visible overlay entries; pre-commit
        // so the overlay carries every value before any version becomes visible.
        for ((key, value), &new_v) in items.iter().zip(new_versions.iter()) {
            self.overlay.insert(key.clone(), new_v, value.clone());
        }

        // Mark every version Materialized and advance the reader-visible floor
        // from the watermark. Commit order does not matter: each `mark` lands
        // in the tracker's states-map and `try_advance` resolves contiguity,
        // so the floor ends at the batch's `max_v`. Replaces the direct
        // `raise_published(max_v)`.
        for guard in guards {
            guard.commit();
        }
        // P1d-1: every batched version is durable in history (the single
        // `history.transact` above succeeded); mark each durable AFTER the
        // visibility commits so `durable_watermark() <= last_committed()` is
        // maintained at every observation. Order across `new_versions` does
        // not matter — the durable tracker resolves contiguity the same way
        // the visibility tracker does.
        for &v in &new_versions {
            self.gate.mark_durable(v);
        }
        // T1b.2 + L6: per-key count-aware vacuum for every key in the batch.
        // old_versions[i] is the prior version captured before publish_cell.
        for ((key, _), &old_v) in items.iter().zip(old_versions.iter()) {
            if self.vacuum_key(key, old_v).await.is_err() {
                self.vacuum_needs_scan.store(true, Ordering::Release);
            }
        }
        if wait {
            self.strict_ack(max_v).await?;
        }
        Ok(max_v)
    }

    /// Append-only variant of [`set_versioned_many`]: the caller GUARANTEES
    /// every key is fresh (no prior version exists in this store).
    ///
    /// Skips the per-row `current_version()` lookup that `set_versioned_many`
    /// performs to capture `old_v` for the L6 targeted-remove vacuum fast path.
    /// For fresh keys the lookup is a guaranteed hash-miss (~50-100 ns per key
    /// on `scc::HashMap`), which is pure waste on a batch insert of N new
    /// records.
    ///
    /// Instead, passes `old_v = 0` to `vacuum_key`, which short-circuits via
    /// the append-only no-op return (`old_v == 0` in the L6 fast path when
    /// retention is `CurrentOnly` and no live snapshots exist). When retention
    /// is NOT `CurrentOnly` or live snapshots are present, `vacuum_key(_, 0)`
    /// falls through to the scan path, which operates by its own prefix-scan
    /// invariants and does not depend on the `old_v` parameter.
    ///
    /// SAFETY (programmer-contract, not runtime-enforced):
    ///   - Caller must have generated each key via `RecordId::from_ts` /
    ///     `RecordId::new` (or equivalent fresh-key construction) RIGHT before
    ///     this call.
    ///   - Calling with a pre-existing key does NOT cause correctness issues
    ///     (the scan path in `vacuum_key` still triggers via
    ///     `vacuum_needs_scan` if accumulated versions exist from prior
    ///     snapshot epochs), but defeats the targeted-remove optimization
    ///     for that key: the old version accumulates until the next full scan.
    ///
    /// cancel-safe: NO before commit, as for [`Self::set_versioned_many`].
    /// Cancelling the post-commit acknowledgement wait preserves the batch.
    pub async fn set_versioned_many_append_only(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<u64> {
        self.set_versioned_many_append_only_inner(items, true).await
    }

    /// Guarded engine callers only; avoids the lower-orphan posting-replay
    /// barrier cycle described by [`Self::set_versioned_no_wait`].
    #[doc(hidden)]
    pub async fn set_versioned_many_append_only_no_wait(
        &self,
        items: Vec<(RecordKey, Bytes)>,
    ) -> DbResult<u64> {
        self.set_versioned_many_append_only_inner(items, false)
            .await
    }

    async fn set_versioned_many_append_only_inner(
        &self,
        items: Vec<(RecordKey, Bytes)>,
        wait: bool,
    ) -> DbResult<u64> {
        if items.is_empty() {
            return Ok(0);
        }
        self.ensure_writable()?;
        if items.iter().any(|(_, value)| value.is_empty()) {
            return Err(DbError::Validation("empty value is a tombstone".into()));
        }

        // Phase 1 (bump-first): assign a fresh version per key and update the
        // cell BEFORE the physical log write. No old_versions collection needed
        // — caller guarantees every key is fresh (no prior version to capture).
        let mut max_v = 0u64;
        let mut new_versions: Vec<u64> = Vec::with_capacity(items.len());
        let mut guards: Vec<VersionGuard> = Vec::with_capacity(items.len());
        let ts_ms = self.now_millis();
        let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(items.len() * 2);
        for (key, value) in &items {
            let guard = self.gate.assign_next_version_guarded();
            let new_v = guard.version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            guards.push(guard);
            max_v = new_v;
            history_ops.push(KvOp::Set(
                encode_version_key(key, new_v).into(),
                value.clone(),
            ));
            history_ops.push(KvOp::Set(ts_key(new_v).into(), ts_val.clone()));
        }

        if !history_ops.is_empty() {
            if let Err(e) = self.history.transact(history_ops).await {
                // RV-TX-2CB455A7: remove every attempted version+ts key
                // before the guards settle as Aborted.
                self.compensate_failed_write(
                    guards,
                    items.iter().map(|(key, _)| key),
                    &new_versions,
                )
                .await;
                return Err(e);
            }
        }

        // Phase 3: maintain the in-memory ts-index for every version in the batch.
        for &new_v in &new_versions {
            self.ts_index_insert(ts_ms, new_v);
        }

        // P1c: dual-write — populate the overlay.
        for ((key, value), &new_v) in items.iter().zip(new_versions.iter()) {
            self.overlay.insert(key.clone(), new_v, value.clone());
        }

        // Mark every version Materialized and advance the reader-visible floor.
        for guard in guards {
            guard.commit();
        }
        // P1d-1: mark each version durable.
        for &v in &new_versions {
            self.gate.mark_durable(v);
        }
        // Vacuum with old_v=0: append-only no-op in L6 fast path (CurrentOnly
        // + no snapshots), or scan path (independent of old_v) otherwise.
        for (key, _) in &items {
            if self.vacuum_key(key, 0).await.is_err() {
                self.vacuum_needs_scan.store(true, Ordering::Release);
            }
        }
        if wait {
            self.strict_ack(max_v).await?;
        }
        Ok(max_v)
    }

    /// cancel-safe: NO — multi-step state mutation; same reasoning as
    /// `set_versioned`. The log is the sole durable write. Cancellation after
    /// `publish_cell` but before the tombstone `history.set` leaves the cell
    /// advanced without the tombstone; caller-side retry / WAL replay.
    ///
    /// Returns the monotonic version assigned to this delete (always
    /// allocated — see [`set_versioned`] for rationale).
    ///
    /// Cancelling only the post-commit acknowledgement wait does not undo
    /// the tombstone or apply it again; observe before retrying.
    pub async fn delete_versioned(&self, key: RecordKey) -> DbResult<u64> {
        self.delete_versioned_inner(key, true).await
    }

    /// Guarded engine callers only; avoids the lower-orphan posting-replay
    /// barrier cycle described by [`Self::set_versioned_no_wait`].
    #[doc(hidden)]
    pub async fn delete_versioned_no_wait(&self, key: RecordKey) -> DbResult<u64> {
        self.delete_versioned_inner(key, false).await
    }

    async fn delete_versioned_inner(&self, key: RecordKey, wait: bool) -> DbResult<u64> {
        self.ensure_writable()?;
        // Single log append: a tombstone (empty value) is written for the
        // delete version. The prior version is already in the log from when
        // it was written as current.
        //
        // Bump-first: assign version, update cell, then write the tombstone.
        // CRIT-2: `publish_cell` uses entry_sync (modify-or-insert) so the
        // cached version advances monotonically.
        // L6: capture old_v BEFORE publish_cell bumps the cell.
        let old_v = self.current_version(&key);
        // P0b: guarded allocation — unify the non-tx delete onto the
        // CompletionTracker (see `set_versioned`). Drop before `commit()`
        // marks the version Aborted, so a `?`-propagated history error never
        // wedges the watermark at `new_v - 1`.
        let guard = self.gate.assign_next_version_guarded();
        let new_v = guard.version();
        self.publish_cell(key.clone(), new_v).await;
        // L2: single atomic log write — tombstone + ts in one transact call.
        // T1c: capture commit timestamp once for the age-retention axis.
        let ts_ms = self.now_millis();
        let ts_val = Bytes::from(ts_ms.to_le_bytes().to_vec());
        let write_ops = vec![
            // Tombstone (empty value — MessagePack records are never
            // zero-length, so empty is unambiguously a delete).
            KvOp::Set(encode_version_key(&key, new_v).into(), Bytes::new()),
            KvOp::Set(ts_key(new_v).into(), ts_val),
        ];
        if let Err(e) = self.history.transact(write_ops).await {
            // RV-TX-2CB455A7: remove the attempted version+ts keys before the
            // guard settles as Aborted.
            self.compensate_failed_write(vec![guard], std::iter::once(&key), &[new_v])
                .await;
            return Err(e);
        }
        // Phase 3: maintain the in-memory ts-index for as-of queries.
        self.ts_index_insert(ts_ms, new_v);
        // P1c: dual-write — overlay tombstone (empty `Bytes`, same convention
        // as history), AFTER the durable tombstone write succeeds and BEFORE
        // `guard.commit()` advances the floor. Same ordering rationale as
        // `set_versioned`: a failed tombstone write (`?` above) must not leave a
        // visible overlay tombstone; the insert is post-write so it carries the
        // tombstone before the delete version becomes visible, with no empty
        // window (history holds the tombstone in the interim).
        self.overlay.insert(key.clone(), new_v, Bytes::new());
        // Mark Materialized and advance the reader-visible floor from the
        // watermark (replaces the direct `raise_published(new_v)`).
        guard.commit();
        // P1d-1: the tombstone is durable in history (inline write above);
        // mark durable AFTER the visibility commit. Same rationale as
        // `set_versioned` — keeps `durable_watermark() <= last_committed()`.
        self.gate.mark_durable(new_v);
        // T1b.2 + L6: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive). old_v is the
        // prior version captured before publish_cell — enables targeted remove.
        if self.vacuum_key(&key, old_v).await.is_err() {
            self.vacuum_needs_scan.store(true, Ordering::Release);
        }
        if wait {
            self.strict_ack(new_v).await?;
        }
        Ok(new_v)
    }

    /// Direct access to history store (for GC, recovery).
    pub fn history_store(&self) -> &Arc<dyn Store> {
        &self.history
    }
}
