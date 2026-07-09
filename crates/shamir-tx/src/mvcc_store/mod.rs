//! Versioned KV layer over the history version log.
//!
//! [`MvccStore`] wraps a single [`Store`] instance:
//! - `history` — the sole version log. Every write appends a version-key
//!   entry `<key>::0xFF::<version_be>` (see [`version_codec`]). All reads
//!   resolve from this log.
//!
//! Every write (set/delete/batch/apply_committed) performs exactly ONE
//! durable append to `history`. MVCC-2 cannot occur because `publish_cell`
//! fires BEFORE the log write, so any snapshot opened mid-write sees the
//! bumped cell version and correctly range-scans the log for the prior entry.
//!
//! Snapshot reads via [`MvccStore::get_at`]: if the cached version ≤ snapshot,
//! read the log at `key‖0xFF‖version`; otherwise range-scan the log for the
//! newest version ≤ snapshot (see [`MvccStore::resolve_read`]).

mod drain;
pub mod key_lock;
pub mod mvcc_gc;
pub mod mvcc_history;
pub mod mvcc_locks;
pub mod retention;
pub mod version_entry;

pub use key_lock::{KeyLock, LockMode};
pub use retention::Retention;
pub use version_entry::VersionEntry;

use std::cmp::Reverse;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use bytes::{BufMut, Bytes, BytesMut};
use scc::HashMap as SccHashMap;
use scc::TreeIndex;
use shamir_collections::THasher;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;
use crate::version_guard::VersionGuard;
use crate::versioned_overlay::VersionedOverlay;

use key_lock::KeyLock as KeyLockInner;
use version_entry::StreamingGroupByState;

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
pub(super) const TS_TAG: u8 = 0x00;

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
    /// SSI fix S1 — cell-reservation marker. `0` = free; otherwise the
    /// `txn_id` of the committer that has CLAIMED this cell as the explicit
    /// serialization point for a write-write conflict (`try_reserve`). The
    /// claim blocks competing writers (their `try_reserve` returns `false`)
    /// but is INVISIBLE to readers — every read path consults `version`
    /// only, never this field. In S1 this is purely additive: no live path
    /// ever sets it non-zero (every `RecordCell` constructor initialises it
    /// to `0`), so behaviour is byte-identical. S2 wires `try_reserve` into
    /// `pre_commit` and `finalize_reservation` into the publish path.
    pub(crate) reserved_by: u64,
}

/// Versioned layer over the history version log.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    pub(super) history: Arc<dyn Store>,
    pub(super) gate: Arc<RepoTxGate>,
    /// In-memory coordination state: key → record cell (latest committed version).
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    pub(super) cells: SccHashMap<Bytes, RecordCell, THasher>,
    /// Level-3 pessimistic lock registry. Populated ONLY for keys locked by a
    /// `Pessimistic` tx; stays empty otherwise → zero overhead on the snapshot
    /// / serializable read/write hot paths. Each entry is an `Arc<KeyLock>`
    /// shared between concurrent requesters of the same key.
    pub(super) locks: SccHashMap<Bytes, Arc<KeyLockInner>, THasher>,
    /// T1b.2: history-retention policy (lock-free `ArcSwap<Retention>`).
    /// Defaults to [`Retention::current_only`] (eager vacuum). Set via
    /// [`Self::set_retention`].
    retention: ArcSwap<Retention>,
    /// T1c: wall-clock millis source for per-version commit timestamps.
    /// `0` = use the real clock (`SystemTime`); a non-zero frozen value is
    /// for deterministic retention tests (see [`Self::set_test_now`]).
    /// Retention is calendar time (wall clock), so `SystemTime` is correct
    /// here — NOT a monotonic clock (we need to reason about "60 seconds ago").
    test_now_millis: AtomicU64,
    /// P1b: per-table in-memory versioned overlay. Holds `(key, version) →
    /// value` for the `(durable_watermark, visibility_watermark]` window. The
    /// three read seams (`resolve_read`, `get_current`, `current_stream`)
    /// probe it BEFORE the durable `history` log. In P1b it is never populated
    /// on the production path (filled in P1c), so it stays empty and every
    /// probe returns `None` → behaviour is byte-identical to history-only.
    overlay: VersionedOverlay,
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
    pending_ts: SccHashMap<u64, u64, THasher>,
    /// L6: set `true` when a snapshot is (or was recently) active during a
    /// vacuum_key call. The targeted-remove fast path fires only when this is
    /// `false` (no accumulated old versions from a snapshot epoch). Cleared
    /// after a full scan-path vacuum runs with `active_snapshots_empty()`.
    vacuum_needs_scan: AtomicBool,
    /// Phase 3: in-memory ts-ordered index for O(log N) `version_at_or_before_ts`.
    /// Key: `(Reverse(ts_millis), Reverse(version))` — reversed so that a forward
    /// `range((Reverse(target_ts), Reverse(u64::MAX)).., &guard).next()` yields the
    /// entry with the LARGEST ts ≤ target_ts in O(log N). Value is unit (the version
    /// is embedded in the key). Rebuilt from `history` ts-keys on first query if empty.
    ts_index: TreeIndex<(Reverse<u64>, Reverse<u64>), ()>,
    /// Whether the ts_index has been populated from history (lazy rebuild on first query).
    ts_index_ready: AtomicBool,
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
        }
    }

    /// P1b: borrow the per-table versioned overlay. P1c will use this to
    /// populate the overlay on the ack-path; tests use it to drive the merge
    /// logic with hand-placed entries. The read seams consult it internally.
    ///
    /// `#[allow(dead_code)]`: in P1b the production write path never calls this
    /// (the overlay is filled in P1c); only the unit tests exercise it. The lib
    /// build therefore sees it as unused until P1c wires the populate site.
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
        // Overlay: drop everything <= durable_watermark (floor irrelevant).
        self.overlay.gc_upto(durable_watermark, u64::MAX);

        // pending_ts hygiene: remove any stamp for an already-durable version.
        // `retain` is a lock-free sweep over the (small) map; on the common
        // path it finds nothing (the drain half already removed each stamp it
        // consumed), so this is cheap.
        self.pending_ts.retain(|v, _| *v > durable_watermark);
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
    pub(super) fn ts_index_insert(&self, ts_millis: u64, version: u64) {
        let _ = self
            .ts_index
            .insert((Reverse(ts_millis), Reverse(version)), ());
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
    pub(super) fn ts_index_remove(&self, ts_millis: u64, version: u64) {
        self.ts_index
            .remove(&(Reverse(ts_millis), Reverse(version)));
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
    /// O(log N) via reversed-key forward range + `.next()`.
    pub(super) fn ts_index_query(&self, target_ts: u64) -> Option<u64> {
        let guard = scc::ebr::Guard::new();
        // Reversed keys: (Reverse(ts), Reverse(version)) sorted ascending means
        // largest ts first. range((Reverse(target_ts), Reverse(u64::MAX))..) yields
        // entries where Reverse(ts) >= Reverse(target_ts) i.e. ts <= target_ts,
        // starting from the largest such ts. `.next()` is O(log N).
        self.ts_index
            .range((Reverse(target_ts), Reverse(u64::MAX)).., &guard)
            .next()
            .map(|(k, _)| k.1 .0) // k.1 is Reverse(version), .0 extracts the u64
    }

    /// Phase 3: rebuild the ts-index from the history store's ts-keys.
    /// Called lazily on first `version_at_or_before_ts` if `ts_index_ready` is false.
    async fn ts_index_rebuild(&self) {
        use futures::StreamExt;
        use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;

        let stream = self.history.iter_stream(MAINT_SCAN_BATCH);
        futures::pin_mut!(stream);

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
    }

    /// T1c: current wall-clock millis. If `test_now_millis` is non-zero
    /// (set via [`Self::set_test_now`]) that frozen value is returned;
    /// otherwise the real `SystemTime` since `UNIX_EPOCH` is used. Retention
    /// is calendar time, so `SystemTime` (not a monotonic clock) is correct.
    pub(super) fn now_millis(&self) -> u64 {
        let frozen = self.test_now_millis.load(Ordering::Acquire);
        if frozen != 0 {
            return frozen;
        }
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// T1c (tests): freeze the clock at `ms` millis for deterministic
    /// retention behaviour. Pass `0` to restore the real clock.
    pub fn set_test_now(&self, ms: u64) {
        self.test_now_millis.store(ms, Ordering::Release);
    }

    /// F6b (I2): force the durable `history` version-log to disk.
    ///
    /// Narrow durability seam for the truncation fsync-gate: before the
    /// drainer deletes a sealed WAL segment, every record in that segment
    /// must be `fsync`'d into `history` (not merely written to the page
    /// cache), else a power-loss after the unlink but before the page-cache
    /// flush would lose data (I2, power-loss / level-3). `mark_durable` only
    /// means "written to history"; this makes that write physically durable.
    ///
    /// Flushes ONLY this store's `history` — it does NOT drain the WAL
    /// (calling `drain_all` from the drainer would recurse). The drainer
    /// gates this on `wal.has_truncatable(durable)`, so it fires only on a
    /// segment boundary, never per-commit (no `fsync`-on-commit regression).
    pub async fn flush_history(&self) -> DbResult<()> {
        self.history.flush().await
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

    // L2: `record_ts` and `record_ts_at` REMOVED — ts is now written
    // atomically inside the same `history.transact` batch as the data op
    // in every write path (set_versioned, set_versioned_many,
    // delete_versioned, write_committed_to_history). No separate ts write.

    /// Set the history-retention policy (lock-free `ArcSwap` swap).
    /// Validates first; on invalid `min_count > max_count` the old policy is
    /// kept (no panic). Returns `Err(message)` on validation failure.
    pub fn set_retention(&self, policy: Retention) -> Result<(), String> {
        policy.validate()?;
        self.retention.store(Arc::new(policy));
        Ok(())
    }

    /// Load the current history-retention policy (RCU snapshot).
    pub fn retention(&self) -> arc_swap::Guard<Arc<Retention>> {
        self.retention.load()
    }

    /// Publish `version` into the key's cell. Atomic modify-or-insert;
    /// advances the cell's version to `version` on every write path.
    /// (Bump-first ordering is the CALLER's job — this only performs the
    /// cell mutation.)
    ///
    /// A2 — max-monotonic: on the OCCUPIED branch the cell's `version` is
    /// advanced ONLY when `version` is strictly greater than the cell's
    /// current value. This prevents a slow drainer / recovery replay (which
    /// seeds the cell from a durable write that may be older than an
    /// in-memory commit that raced ahead during the drainer's `.await`
    /// suspension) from regressing the cell backward and causing stale reads
    /// / masked SSI conflicts. The VACANT branch is untouched (a brand-new
    /// cell always seeds at the offered version). The NORMAL non-racing
    /// drain path always offers `<=` the current version, so this guard is a
    /// no-op there.
    pub(super) async fn publish_cell(&self, key: Bytes, version: u64) {
        match self.cells.entry_async(key).await {
            scc::hash_map::Entry::Occupied(mut e) => {
                if version > e.get().version {
                    e.get_mut().version = version;
                }
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version,
                    vacuum_anchor: 0,
                    reserved_by: 0,
                });
            }
        }
    }

    /// A10 vacuum anchor deferral: atomically set the cell's `vacuum_anchor`
    /// to `new_anchor` and return the PREVIOUS value. Uses the sync `entry`
    /// API (per-entry exclusive lock) so the read-and-swap is race-free
    /// against concurrent vacuum calls on the same key. If the cell is
    /// absent (cold-start or evicted), returns `None` and does nothing —
    /// the deferred-anchor optimisation only applies when the cell is
    /// resident.
    pub(crate) fn swap_vacuum_anchor(&self, key: &[u8], new_anchor: u64) -> Option<u64> {
        match self.cells.entry(Bytes::copy_from_slice(key)) {
            scc::hash_map::Entry::Occupied(mut e) => {
                let cell = e.get_mut();
                let prev = cell.vacuum_anchor;
                cell.vacuum_anchor = new_anchor;
                Some(prev)
            }
            scc::hash_map::Entry::Vacant(_) => None,
        }
    }

    // ========================================================================
    // SSI fix S1 — cell-reservation primitive (additive; NOT yet wired).
    //
    // Three atomic acts on the per-key `RecordCell`, all via `cells.entry(key)`
    // (scc per-entry exclusive — the same check-and-update primitive
    // `publish_cell_sync` already relies on). They turn "publish decides who
    // won" (after the WAL commit point) into "claim decides who won" (BEFORE
    // it): a single committer claims the cell, losers see the claim and abort
    // with `SsiConflict` having never touched the WAL.
    //
    // S1 only BUILDS these — `try_reserve` is never called on the live commit
    // path yet, so `reserved_by` stays `0` everywhere and behaviour is
    // byte-identical. S2 invokes `try_reserve` in `pre_commit` (after
    // read-validate, before WAL), `finalize_reservation` in the publish path,
    // and `release_reservation` from the abort-path RAII guard.
    //
    // Sync (`entry`, not `entry_async`) because every call site is the
    // synchronous commit hot path — same rationale as `publish_cell_sync`:
    // the cell map is lock-free / sharded, so `entry` is a bounded CAS, not a
    // contended lock.
    // ========================================================================

    /// SSI fix S1 — atomically CLAIM this key's cell for `txn_id`, the explicit
    /// serialization point for a write-write conflict.
    ///
    /// Returns `true` iff the claim WON (this committer may proceed to WAL /
    /// publish); `false` on CONFLICT (another committer holds the claim, or the
    /// cell has already advanced past this committer's snapshot — a stale
    /// write). A conflict NEVER blocks — the caller aborts with `SsiConflict`.
    ///
    /// Cases, all inside one atomic `entry`:
    /// - **Vacant** → the key has never published a version, so it is free.
    ///   Insert `RecordCell { version: 0, reserved_by: txn_id }` and WIN. The
    ///   `version: 0` sentinel matches `current_version`'s "no version tracked"
    ///   convention; `finalize_reservation` later overwrites it with the real
    ///   commit version.
    /// - **Occupied** → WIN iff `version <= snapshot_version && reserved_by == 0`:
    ///   the cell is unclaimed AND has not advanced past our snapshot. Set
    ///   `reserved_by = txn_id`. Otherwise CONFLICT (`false`): either the cell
    ///   moved past the snapshot (someone published — stale-write detection) or
    ///   it is already claimed by another committer.
    ///
    /// S2 wires this into the engine's `pre_commit` (after read-validate,
    /// before WAL): `pub` (not `pub(crate)`) so the cross-crate `shamir-engine`
    /// commit path can call it to claim each write-set key. The claim is the
    /// explicit serialization point that makes "exactly one committer wins" hold
    /// for non-unique tables under true parallelism.
    pub fn try_reserve(&self, key: Bytes, snapshot_version: u64, txn_id: u64) -> bool {
        match self.cells.entry(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                let cell = e.get_mut();
                if cell.version <= snapshot_version && cell.reserved_by == 0 {
                    cell.reserved_by = txn_id;
                    true
                } else {
                    false
                }
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version: 0,
                    vacuum_anchor: 0,
                    reserved_by: txn_id,
                });
                true
            }
        }
    }

    /// SSI fix S1 — atomically convert a held claim into the published
    /// `version` and CLEAR the reservation, in one `entry`.
    ///
    /// Called on the publish path (S2: Phase 5a) AFTER the committer has won
    /// its claim and the write is becoming visible. Sets `version = version`
    /// and `reserved_by = 0`. If the cell is somehow Vacant (defensive — a
    /// claim should have inserted it), insert `RecordCell { version,
    /// reserved_by: 0 }` so the published version is never lost.
    ///
    /// S2 wires this into the publish path (`apply_committed_visible`,
    /// Phase 5a) in place of the prior `publish_cell_sync` — it is a strict
    /// superset (sets `version` AND clears `reserved_by`).
    pub(crate) fn finalize_reservation(&self, key: Bytes, version: u64) {
        match self.cells.entry(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                let cell = e.get_mut();
                cell.version = version;
                cell.reserved_by = 0;
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version,
                    vacuum_anchor: 0,
                    reserved_by: 0,
                });
            }
        }
    }

    /// SSI fix S1 — release a claim held by `txn_id` (abort path), in one
    /// `entry`. Idempotent and ownership-checked: only clears `reserved_by`
    /// when it still equals `txn_id`.
    ///
    /// A no-op when the cell is Vacant, or when `reserved_by != txn_id` — the
    /// latter covers both "another committer now owns the claim" (we must not
    /// steal it) and "our claim was already finalized" (`reserved_by` is
    /// already `0`). This makes the RAII guard's `Drop` safe to fire after a
    /// successful `finalize_reservation`.
    pub(crate) fn release_reservation(&self, key: Bytes, txn_id: u64) {
        if let scc::hash_map::Entry::Occupied(mut e) = self.cells.entry(key) {
            let cell = e.get_mut();
            if cell.reserved_by == txn_id {
                cell.reserved_by = 0;
            }
        }
    }

    /// Resolve a versioned read of `key` visible at `snapshot_version`, given
    /// its current cached version `cur_v`. Direct path (`cur_v > 0 && cur_v <= snapshot`):
    /// read the log at `encode_version_key(key, cur_v)`. Fallback: range-scan
    /// the log for the newest version `<= snapshot`. Reads exclusively from
    /// the single `history` log in both cases.
    async fn resolve_read(
        &self,
        key: &[u8],
        snapshot_version: u64,
        cur_v: u64,
    ) -> DbResult<Option<Bytes>> {
        if cur_v > 0 && cur_v <= snapshot_version {
            // P1b: overlay-probe BEFORE the durable log. The overlay holds the
            // visible-but-not-yet-drained value for `(key, cur_v)`. A hit
            // (empty = tombstone → None, non-empty → Some) short-circuits; a
            // miss falls through to history (the overlay is empty in P1b, so
            // this always falls through under production load).
            if let Some(val) = self.overlay.get(key, cur_v) {
                return Ok(if val.is_empty() { None } else { Some(val) });
            }
            return match self.history.get(encode_version_key(key, cur_v)).await {
                Ok(val) if val.is_empty() => Ok(None),
                Ok(val) => Ok(Some(val)),
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
    pub async fn set_versioned(&self, key: Bytes, value: Bytes) -> DbResult<u64> {
        // Single log append: every prior version is already in the log from
        // when it was written as current. No other store is written.
        //
        // Bump-first: assign version, update cell, then perform the physical
        // write. CRIT-2: `publish_cell` uses entry_async (modify-or-insert)
        // so repeated writes to the same key advance the cached version
        // monotonically.
        // P0b: take a RAII VersionGuard so the non-tx path goes through the
        // SAME CompletionTracker as tx commits. The guard owns the terminal
        // mark obligation (Materialized on `commit()`, Aborted on any early
        // return / `?`-propagated error / panic before commit). This is the
        // H5 unification: the watermark — not a direct `publish_committed_max`
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
        self.history
            .transact(vec![
                KvOp::Set(encode_version_key(&key_snapshot, new_v), value.clone()),
                KvOp::Set(ts_key(new_v), ts_val),
            ])
            .await?;
        // Phase 3: maintain the in-memory ts-index for O(log N) as-of queries.
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
        // replaces the direct `publish_committed_max(new_v)`.
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
        self.vacuum_key(&key_snapshot, old_v).await;
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
    pub async fn set_versioned_many(&self, items: Vec<(Bytes, Bytes)>) -> DbResult<u64> {
        if items.is_empty() {
            // No records written — return 0. The caller should not emit
            // a changefeed event for an empty batch.
            return Ok(0);
        }

        // Phase 1 (bump-first): assign a fresh version per key and update the
        // cell BEFORE the physical log write. Prior versions are already in
        // the log from when they were written as current.
        // CRIT-2: `publish_cell` uses entry_async modify-or-insert so the
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
        let keys: Vec<Bytes> = items.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in &items {
            old_versions.push(self.current_version(key));
            let guard = self.gate.assign_next_version_guarded();
            let new_v = guard.version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            guards.push(guard);
            max_v = new_v;
            history_ops.push(KvOp::Set(encode_version_key(key, new_v), value.clone()));
            history_ops.push(KvOp::Set(ts_key(new_v), ts_val.clone()));
        }

        // Single batched write to the log (data + ts, atomic).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
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
        // `publish_committed_max(max_v)`.
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
        for (key, &old_v) in keys.iter().zip(old_versions.iter()) {
            self.vacuum_key(key, old_v).await;
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
    pub async fn set_versioned_many_append_only(
        &self,
        items: Vec<(Bytes, Bytes)>,
    ) -> DbResult<u64> {
        if items.is_empty() {
            return Ok(0);
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
        let keys: Vec<Bytes> = items.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in &items {
            let guard = self.gate.assign_next_version_guarded();
            let new_v = guard.version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            guards.push(guard);
            max_v = new_v;
            history_ops.push(KvOp::Set(encode_version_key(key, new_v), value.clone()));
            history_ops.push(KvOp::Set(ts_key(new_v), ts_val.clone()));
        }

        // Single batched write to the log (data + ts, atomic).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
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
        for key in &keys {
            self.vacuum_key(key, 0).await;
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
    pub async fn delete_versioned(&self, key: Bytes) -> DbResult<u64> {
        // Single log append: a tombstone (empty value) is written for the
        // delete version. The prior version is already in the log from when
        // it was written as current.
        //
        // Bump-first: assign version, update cell, then write the tombstone.
        // CRIT-2: `publish_cell` uses entry_async (modify-or-insert) so the
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
        self.history
            .transact(vec![
                // Tombstone (empty value — MessagePack records are never
                // zero-length, so empty is unambiguously a delete).
                KvOp::Set(encode_version_key(&key, new_v), Bytes::new()),
                KvOp::Set(ts_key(new_v), ts_val),
            ])
            .await?;
        // Phase 3: maintain the in-memory ts-index for O(log N) as-of queries.
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
        // watermark (replaces the direct `publish_committed_max(new_v)`).
        guard.commit();
        // P1d-1: the tombstone is durable in history (inline write above);
        // mark durable AFTER the visibility commit. Same rationale as
        // `set_versioned` — keeps `durable_watermark() <= last_committed()`.
        self.gate.mark_durable(new_v);
        // T1b.2 + L6: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive). old_v is the
        // prior version captured before publish_cell — enables targeted remove.
        self.vacuum_key(&key, old_v).await;
        Ok(new_v)
    }

    /// cancel-safe: yes — read-only. The direct path is a single `history.get`;
    /// the fallback is a read-only history range scan. Cancellation drops
    /// the future with no state mutation.
    ///
    /// Snapshot read: return the value visible at `snapshot_version`.
    ///
    /// Direct path: if version_cache says current version ≤ snapshot →
    /// return `history.get(encode_version_key(key, cur_v))`.
    /// Fallback: range scan history `[key::0, key::snapshot]`, take last.
    pub async fn get_at(&self, key: &[u8], snapshot_version: u64) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(key);
        self.resolve_read(key, snapshot_version, cur_v).await
    }

    /// Direct access to history store (for GC, recovery).
    pub fn history_store(&self) -> &Arc<dyn Store> {
        &self.history
    }

    /// C2: flip get_current to the single version log. Returns `None` for
    /// tombstones (empty value) and absent keys. Cold-start: when the cell is
    /// absent (`current_version == 0`), scans the log for the latest version
    /// via `seek_latest_version`.
    ///
    /// R3 — MVCC pre-floor: reads are capped at the committed floor
    /// (`gate.last_committed()`). Versions above the floor are not yet
    /// visible. Under sequential commits this is always equal to the
    /// cell's latest version, so semantics are unchanged today; once P2b
    /// lands (out-of-order materialize) this prevents observing
    /// uncommitted versions.
    ///
    /// Accepts owned `Bytes` for backward-compatibility. Delegates to
    /// [`get_current_bytes`](Self::get_current_bytes) — prefer that
    /// method on hot paths to avoid a 16-byte key allocation.
    pub async fn get_current(&self, key: Bytes) -> DbResult<Option<Bytes>> {
        self.get_current_bytes(&key).await
    }

    /// Zero-alloc point-read: identical semantics to [`get_current`] but
    /// accepts a borrowed `&[u8]` key, avoiding the 16-byte heap copy
    /// that `Bytes::copy_from_slice` would incur on every call.
    pub async fn get_current_bytes(&self, key: &[u8]) -> DbResult<Option<Bytes>> {
        let floor = self.gate.last_committed();
        let cur_v = self.current_version(key);
        let v = if cur_v > 0 {
            cur_v
        } else {
            // Cold-start: the cell is absent. `seek_latest_version` scans
            // history; an overlay-only key (committed but not yet drained) is
            // invisible to that scan. P1b: also consult the overlay so an
            // overlay-only key is found. Cap the overlay probe at the floor
            // (visibility gate); floor == 0 means bootstrap → use u64::MAX so
            // nothing is hidden, matching the R3 "no restriction" semantics.
            let ov_cap = if floor > 0 { floor } else { u64::MAX };
            let hist_v = self.seek_latest_version(key).await?;
            let ov_v = self.overlay.newest_visible(key, ov_cap).map(|(ver, _)| ver);
            // Cold path: `seed_version` needs an owned `Bytes`. Allocate once
            // and reuse for both the seed call and the downstream probes.
            // This allocation only happens on cold-start (cell absent) which
            // is rare on the hot read path.
            match (hist_v, ov_v) {
                (Some(h), Some(o)) => {
                    // Seed the cell from history's max (the durable anchor);
                    // the overlay value, if newer, wins in resolve_read below.
                    self.seed_version(Bytes::copy_from_slice(key), h).await;
                    h.max(o)
                }
                (Some(h), None) => {
                    self.seed_version(Bytes::copy_from_slice(key), h).await;
                    h
                }
                // Overlay-only key: no durable history yet. Do NOT seed the
                // cell (no durable version to anchor); resolve through the
                // overlay-aware floor read below.
                (None, Some(o)) => o,
                (None, None) => return Ok(None),
            }
        };
        // R3: if the committed floor is non-zero (gate initialized) and the
        // resolved version exceeds it, fall back to snapshot read at the
        // floor (range-scan for newest <= floor — overlay-aware via
        // resolve_read). Floor == 0 means bootstrap / recovery — no
        // visibility restriction applied.
        if floor > 0 && v > floor {
            return self.get_at(key, floor).await;
        }
        // P1b: overlay-probe BEFORE the durable log for the exact `(key, v)`.
        // A hit (tombstone → None, value → Some) short-circuits; a miss falls
        // through to history (always so in P1b — overlay is empty).
        if let Some(val) = self.overlay.get(key, v) {
            return Ok(if val.is_empty() { None } else { Some(val) });
        }
        match self.history.get(encode_version_key(key, v)).await {
            Ok(val) if val.is_empty() => Ok(None),
            Ok(val) => Ok(Some(val)),
            Err(DbError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// C2: stream every CURRENT `(key, value)` pair from the single version
    /// log. The log is key-major, version-ascending; a streaming group-by tracks
    /// the last (highest) version per key — that is the current. Tombstones
    /// (empty value) are suppressed. Emits in batches of `batch`.
    ///
    /// R3 — MVCC pre-floor: versions above `gate.last_committed()` are
    /// excluded from the group-by. The per-entry check is inlined into the
    /// existing scan loop (one comparison per decoded entry — no second pass).
    pub fn current_stream(
        &self,
        batch: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        use futures::stream::unfold;

        let history = Arc::clone(&self.history);
        // R3: capture committed floor at stream-open time.
        let floor = self.gate.last_committed();
        // P1b: materialise the overlay's per-key winner ≤ floor at open. The
        // overlay is the SMALL side of the merge (bounded window), so loading
        // it into a map and merging during the history group-by is cheap.
        // `floor == 0` (bootstrap) → `snapshot_le` returns empty → overlay
        // contributes nothing and the stream is byte-identical to history-only.
        let overlay: shamir_collections::TMap<Bytes, (u64, Bytes)> = self
            .overlay
            .snapshot_le(floor)
            .into_iter()
            .map(|(k, v, val)| (k, (v, val)))
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
            },
            |state| async move {
                match state {
                    StreamingGroupByState::Start {
                        history,
                        batch_size,
                        floor,
                        overlay,
                    } => {
                        let stream = history.iter_stream(batch_size);
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

    /// Look up the latest committed version for `key` in the in-memory cache.
    /// Returns 0 if the key is not cached (meaning "initial / no version tracked").
    ///
    /// III.2: probes the cache with the raw `&[u8]` — no `Bytes` allocation.
    /// `scc 2.x`'s `HashMap::read<Q>` is bounded by `Q: Equivalent<K> + Hash`
    /// (scc's vendored `equivalent` trait), and scc ships the blanket impl
    /// `impl<Q, K> Equivalent<K> for Q where Q: Eq, K: Borrow<Q>`. Since
    /// `bytes::Bytes: Borrow<[u8]>` and `[u8]: Eq`, `[u8]: Equivalent<Bytes>`
    /// holds, so `&[u8]` is an accepted probe key. The lookup hash matches
    /// because `<Bytes as Hash>` delegates to `self.as_slice().hash(..)`,
    /// i.e. it is byte-identical to `<[u8] as Hash>`. Net effect: the
    /// previous `Bytes::copy_from_slice(key)` heap-alloc+copy on every probe
    /// (one per `get_at`, one per `version_of` read-set entry) is gone.
    pub(crate) fn current_version(&self, key: &[u8]) -> u64 {
        self.cells.read(key, |_, c| c.version).unwrap_or(0)
    }

    /// Public accessor: current committed version for `key`, or `0` if
    /// the key has never been written through this store.
    ///
    /// Used by SSI read-set validation (Stage 4.D.5+) — the caller
    /// captures this value when reading inside a tx, then commit re-
    /// queries it to detect "another tx wrote this key since I read".
    pub fn version_of(&self, key: &[u8]) -> u64 {
        self.current_version(key)
    }

    /// The latest version assigned to `key` — or `None` if no write has
    /// touched this key through this store in-process. Used by the index-only
    /// read path to validate a covering posting: a posting whose embedded
    /// version equals this value is fresh; `None` means "no in-process
    /// mutation" (the durable posting is consistent).
    pub fn live_version(&self, key: &[u8]) -> Option<u64> {
        self.cells.read(key, |_, c| c.version)
    }

    // ========================================================================
    // Level-3 pessimistic locking (wound-wait, deadlock-free).
    //
    // These methods are ONLY called from `IsolationLevel::Pessimistic` code
    // paths (see `table_manager` / `write_exec`). The snapshot / serializable
    // read/write paths never touch `locks`, so when no Level-3 tx runs the
    // registry stays empty and there is zero overhead on the hot paths.
    // ========================================================================

    /// Number of keys currently holding a Level-3 lock entry. Used by tests
    /// to assert the zero-overhead invariant (snapshot/serializable txs never
    /// populate `locks`).
    #[allow(clippy::disallowed_methods)] // O(N) ack: telemetry/test accessor, off hot path
    pub fn locks_len(&self) -> usize {
        self.locks.len()
    }

    /// L3: batched current-version read for multiple keys.
    ///
    /// Semantically equivalent to calling [`get_current_bytes`] per key, but
    /// collapses the warm-path history lookups into a single
    /// `Store::get_many` call (one transactional read / one `spawn_blocking`
    /// on disk backends instead of N).
    ///
    /// Cold keys (cell absent — `current_version == 0`) fall back to the
    /// per-key [`get_current_bytes`] path (range-scan + seed); cold is the
    /// minority in steady state.
    ///
    /// Result order matches `keys` order.
    pub async fn get_current_many(&self, keys: &[Bytes]) -> DbResult<Vec<Option<Bytes>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let floor = self.gate.last_committed();
        let len = keys.len();

        // Phase 1: classify each input key. For each slot we store the
        // resolution category so Phase 3 can assemble the output in order.
        enum Slot {
            /// Resolved from overlay — final answer already known.
            Resolved(Option<Bytes>),
            /// Warm miss — `miss_idx` is the position in the `miss_keys`
            /// vector that will be fed to `history.get_many`.
            HistoryMiss { miss_idx: usize },
            /// Version > floor — fall back to snapshot read at floor.
            FloorExceeded,
            /// Cold cell — fall back to per-key `get_current_bytes`.
            Cold,
        }

        let mut slots: Vec<Slot> = Vec::with_capacity(len);
        let mut miss_keys: Vec<Bytes> = Vec::new();

        for key in keys {
            let cur_v = self.current_version(key);
            if cur_v == 0 {
                slots.push(Slot::Cold);
                continue;
            }
            // R3 floor-cap: version above the committed floor is not yet
            // visible — fall back to snapshot read at floor.
            if floor > 0 && cur_v > floor {
                slots.push(Slot::FloorExceeded);
                continue;
            }
            // Overlay probe — same precedence as get_current_bytes.
            if let Some(val) = self.overlay.get(key, cur_v) {
                let resolved = if val.is_empty() { None } else { Some(val) };
                slots.push(Slot::Resolved(resolved));
                continue;
            }
            // Warm miss — needs history lookup.
            let vk = encode_version_key(key, cur_v);
            let miss_idx = miss_keys.len();
            miss_keys.push(vk);
            slots.push(Slot::HistoryMiss { miss_idx });
        }

        // Phase 2: single batched history read for all warm misses.
        let miss_results = if miss_keys.is_empty() {
            Vec::new()
        } else {
            self.history.get_many(miss_keys).await?
        };

        // Phase 3: assemble the output vector in input order.
        let mut out: Vec<Option<Bytes>> = Vec::with_capacity(len);
        for (i, slot) in slots.iter().enumerate() {
            match slot {
                Slot::Resolved(val) => {
                    out.push(val.clone());
                }
                Slot::HistoryMiss { miss_idx } => {
                    match &miss_results[*miss_idx] {
                        Some(val) if val.is_empty() => out.push(None), // tombstone
                        Some(val) => out.push(Some(val.clone())),
                        None => out.push(None), // not found
                    }
                }
                Slot::FloorExceeded => {
                    let val = self.get_at(&keys[i], floor).await?;
                    out.push(val);
                }
                Slot::Cold => {
                    let val = self.get_current_bytes(&keys[i]).await?;
                    out.push(val);
                }
            }
        }

        Ok(out)
    }

    /// T1c: look up the recorded commit timestamp (millis) for `version`.
    /// Returns `None` if no ts entry exists (treated as "unknown age" → the
    /// age axis conservatively keeps the version).
    pub(super) async fn lookup_ts(&self, version: u64) -> Option<u64> {
        match self.history.get(ts_key(version)).await {
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
