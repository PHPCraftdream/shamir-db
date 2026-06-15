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

pub mod key_lock;
pub mod mvcc_gc;
pub mod mvcc_history;
pub mod mvcc_locks;
pub mod retention;
pub mod version_entry;

pub use key_lock::{KeyLock, LockMode};
pub use retention::Retention;
pub use version_entry::VersionEntry;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use bytes::{BufMut, Bytes, BytesMut};
use scc::HashMap as SccHashMap;
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
    /// (`apply_committed_visible`) and consumed by the drainer
    /// (`write_committed_to_history`). The cutover moved the durable `record_ts`
    /// write OFF the ack-path into the drainer, but the ts VALUE must reflect
    /// COMMIT time, not drain time (the drainer runs arbitrarily later, and a
    /// frozen test clock or a real clock that advanced would mis-stamp the
    /// version — breaking age-retention / as-of-by-ts). So we capture the
    /// commit-time millis here at ack and the drainer writes THAT into history.
    /// Entry removed once durably written. Cold recovery (overlay empty) finds
    /// no entry and falls back to `now_millis()` — its conservative pre-cutover
    /// behaviour (recovery already could not reconstruct the original ts).
    pending_ts: SccHashMap<u64, u64, THasher>,
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

    /// T4-purge: the store's current wall-clock millis (test-overridable
    /// via [`Self::set_test_now`]). Exposed so the PurgeHistory executor
    /// can resolve `OlderThanAge { age_secs }` against the SAME clock
    /// that stamped each version's commit ts — keeping age-based purge
    /// deterministic under `set_test_now`.
    pub fn clock_millis(&self) -> u64 {
        self.now_millis()
    }

    /// T1c: record the commit timestamp for `version` under `ts_key(version)`
    /// in `history`. Best-effort — a ts write failure is swallowed (the data
    /// write already succeeded; a missing ts just means the age axis
    /// conservatively keeps the version, never reclaims it wrongly). This
    /// matches the eager-vacuum error policy.
    pub(super) async fn record_ts(&self, version: u64) {
        self.record_ts_at(version, self.now_millis()).await;
    }

    /// D2 P1d-2b: record an EXPLICIT commit timestamp `ms` for `version` under
    /// `ts_key(version)` in `history`. Used by the drainer
    /// ([`write_committed_to_history`](Self::write_committed_to_history)) to
    /// stamp the COMMIT-time millis it captured on the ack-path, decoupling the
    /// recorded ts from the (later) drain time. Best-effort like
    /// [`record_ts`](Self::record_ts).
    pub(super) async fn record_ts_at(&self, version: u64, ms: u64) {
        let bytes = ms.to_le_bytes();
        let _ = self
            .history
            .set(ts_key(version), Bytes::from(bytes.to_vec()))
            .await;
    }

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
    pub(super) async fn publish_cell(&self, key: Bytes, version: u64) {
        match self.cells.entry_async(key).await {
            scc::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().version = version;
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell { version });
            }
        }
    }

    /// D2 P1d-2b: synchronous sibling of [`Self::publish_cell`] for the
    /// ack-path visible half ([`Self::apply_committed_visible`]), which does no
    /// I/O and must stay off `.await`. Uses scc's blocking `entry` (no async
    /// suspension): the cell map is lock-free / sharded, so this is a bounded
    /// CAS, not a contended lock on the commit hot path.
    pub(super) fn publish_cell_sync(&self, key: Bytes, version: u64) {
        match self.cells.entry(key) {
            scc::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().version = version;
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell { version });
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
        let guard = self.gate.assign_next_version_guarded();
        let new_v = guard.version();
        self.publish_cell(key.clone(), new_v).await;
        let key_snapshot = key.clone();
        // Single log write: the current version goes into the log (sole write).
        self.history
            .set(encode_version_key(&key_snapshot, new_v), value.clone())
            .await?;
        // P1c: dual-write — populate the overlay with the SAME (key, version)
        // → value pair, AFTER the durable history write succeeds but BEFORE
        // `guard.commit()` advances the reader-visible floor. Ordering rationale:
        //  • after history.set: a failed history write (`?` above) must NOT leave
        //    a visible overlay entry — otherwise an aborted write would surface
        //    via `get_current` (the cell is already bumped). Inserting post-write
        //    keeps "history.set failed ⇒ nothing written" intact.
        //  • before guard.commit: by the time the version becomes reader-visible
        //    (floor advanced / Materialized), the overlay carries its value.
        //  • no empty-window: between history.set returning and this insert the
        //    cell reports `new_v` but history already holds the value, so any
        //    read resolves from history.
        self.overlay.insert(key.clone(), new_v, value);
        // T1c: record the commit timestamp for the age-retention axis.
        self.record_ts(new_v).await;
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
        // T1b.2: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive).
        self.vacuum_key(&key_snapshot).await;
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
        // P0b: one VersionGuard per allocated version. If the batched
        // `history.transact` below fails (`?`), every guard drops un-committed
        // and marks its version Aborted, so the contiguous watermark advances
        // past the whole failed batch instead of wedging at the first version.
        let mut guards: Vec<VersionGuard> = Vec::with_capacity(items.len());
        // Single log write — build history_ops (the sole durable write target).
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(items.len());
        let keys: Vec<Bytes> = items.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in &items {
            let guard = self.gate.assign_next_version_guarded();
            let new_v = guard.version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            guards.push(guard);
            max_v = new_v;
            history_ops.push(KvOp::Set(encode_version_key(key, new_v), value.clone()));
        }

        // Single batched write to the log (sole durable write).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // P1c: dual-write — populate the overlay with the SAME (key, version) →
        // value pairs, AFTER the batched `history.transact` succeeds and BEFORE
        // the `guard.commit()` loop advances the floor. Post-transact so a failed
        // batch write (`?` above) leaves no visible overlay entries; pre-commit
        // so the overlay carries every value before any version becomes visible.
        for ((key, value), &new_v) in items.iter().zip(new_versions.iter()) {
            self.overlay.insert(key.clone(), new_v, value.clone());
        }

        // T1c: record the commit timestamp for every version in the batch
        // (age-retention axis). Best-effort.
        for &v in &new_versions {
            self.record_ts(v).await;
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
        // T1b.2: per-key count-aware vacuum for every key in the batch.
        for key in &keys {
            self.vacuum_key(key).await;
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
        // P0b: guarded allocation — unify the non-tx delete onto the
        // CompletionTracker (see `set_versioned`). Drop before `commit()`
        // marks the version Aborted, so a `?`-propagated history error never
        // wedges the watermark at `new_v - 1`.
        let guard = self.gate.assign_next_version_guarded();
        let new_v = guard.version();
        self.publish_cell(key.clone(), new_v).await;
        // Single log write: tombstone (empty value — MessagePack records are
        // never zero-length, so empty is unambiguously a delete).
        self.history
            .set(encode_version_key(&key, new_v), Bytes::new())
            .await?;
        // P1c: dual-write — overlay tombstone (empty `Bytes`, same convention
        // as history), AFTER the durable tombstone write succeeds and BEFORE
        // `guard.commit()` advances the floor. Same ordering rationale as
        // `set_versioned`: a failed tombstone write (`?` above) must not leave a
        // visible overlay tombstone; the insert is post-write so it carries the
        // tombstone before the delete version becomes visible, with no empty
        // window (history holds the tombstone in the interim).
        self.overlay.insert(key.clone(), new_v, Bytes::new());
        // T1c: record the commit timestamp for the age-retention axis.
        self.record_ts(new_v).await;
        // Mark Materialized and advance the reader-visible floor from the
        // watermark (replaces the direct `publish_committed_max(new_v)`).
        guard.commit();
        // P1d-1: the tombstone is durable in history (inline write above);
        // mark durable AFTER the visibility commit. Same rationale as
        // `set_versioned` — keeps `durable_watermark() <= last_committed()`.
        self.gate.mark_durable(new_v);
        // T1b.2: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive).
        self.vacuum_key(&key).await;
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
    pub async fn get_current(&self, key: Bytes) -> DbResult<Option<Bytes>> {
        let floor = self.gate.last_committed();
        let cur_v = self.current_version(&key);
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
            let hist_v = self.seek_latest_version(&key).await?;
            let ov_v = self
                .overlay
                .newest_visible(&key, ov_cap)
                .map(|(ver, _)| ver);
            match (hist_v, ov_v) {
                (Some(h), Some(o)) => {
                    // Seed the cell from history's max (the durable anchor);
                    // the overlay value, if newer, wins in resolve_read below.
                    self.seed_version(key.clone(), h).await;
                    h.max(o)
                }
                (Some(h), None) => {
                    self.seed_version(key.clone(), h).await;
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
            return self.get_at(&key, floor).await;
        }
        // P1b: overlay-probe BEFORE the durable log for the exact `(key, v)`.
        // A hit (tombstone → None, value → Some) short-circuits; a miss falls
        // through to history (always so in P1b — overlay is empty).
        if let Some(val) = self.overlay.get(&key, v) {
            return Ok(if val.is_empty() { None } else { Some(val) });
        }
        match self.history.get(encode_version_key(&key, v)).await {
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
    pub fn locks_len(&self) -> usize {
        self.locks.len()
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
