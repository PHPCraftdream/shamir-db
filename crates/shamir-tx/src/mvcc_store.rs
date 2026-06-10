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

use shamir_tunables::store_defaults::{HISTORY_SCAN_BATCH, MAINT_SCAN_BATCH};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use bytes::{BufMut, Bytes, BytesMut};
use futures::StreamExt;
use scc::HashMap as SccHashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::{decode_version_key, encode_version_key};

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
const TS_TAG: u8 = 0x00;

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

// ============================================================================
// Level-3 pessimistic locking — wound-wait, deadlock-free by construction.
//
// Locks live in a SEPARATE map (`MvccStore::locks`), NOT in the hot-path
// `RecordCell`. The map is populated ONLY for keys locked by a Pessimistic
// (Level-3) transaction; it stays empty when no Level-3 tx runs, so the
// snapshot/serializable read/write paths pay zero overhead.
//
// Wound-wait: a requester only ever *waits* on strictly-older holders and
// only ever *wounds* strictly-younger ones (the tx's monotonic id is its
// priority — smaller id = older = higher priority). The wait-for graph
// therefore respects the total id order and cannot cycle, so no deadlock
// detector is needed.
// ============================================================================

/// Lock mode for a Level-3 pessimistic lock.
///
/// `Shared` is compatible with other `Shared` holders (multiple readers);
/// `Exclusive` is compatible with nothing but the same tx (re-entrant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMode {
    Shared,
    Exclusive,
}

/// A single lock holder: the holding tx's monotonic id, its shared
/// `wounded` flag, and a per-tx `Notify` the wounder triggers so the
/// holder — which may be parked waiting on a DIFFERENT key — wakes up
/// and observes the wound. This is load-bearing for deadlock-freedom:
/// a wound issued on key Y must wake a tx parked on key X, so the wake
/// cannot be keyed on the lock where the wound happened.
#[derive(Debug)]
pub(crate) struct Holder {
    pub(crate) tx_version: u64,
    pub(crate) wounded: Arc<AtomicBool>,
    pub(crate) wound_notify: Arc<tokio::sync::Notify>,
}
/// The mutable state of one key's lock: the set of current holders plus
/// the aggregate mode (`None` when unheld). Invariant: when `mode` is
/// `Some(Exclusive)`, `holders` has exactly one entry; when `Some(Shared)`,
/// every holder is a distinct tx (no duplicate ids).
#[derive(Debug, Default)]
pub(crate) struct KeyLockState {
    pub(crate) holders: Vec<Holder>,
    pub(crate) mode: Option<LockMode>,
}

impl KeyLockState {
    /// True if `tx_version` already holds this key in ANY mode. Used for
    /// re-entrant upgrades/re-locks: a same-tx re-acquire is always allowed
    /// and never self-deadlocks.
    fn held_by(&self, tx_version: u64) -> bool {
        self.holders.iter().any(|h| h.tx_version == tx_version)
    }

    /// Recompute `mode` from the surviving holders. `None` when empty;
    /// `Shared` when more than one holder (the invariant guarantees the
    /// only multi-holder mode is Shared); otherwise leave the existing
    /// mode (a lone holder is whatever the caller last requested).
    fn recompute_mode(&mut self) {
        match self.holders.len() {
            0 => self.mode = None,
            1 => {}
            _ => self.mode = Some(LockMode::Shared),
        }
    }
}

/// Per-key pessimistic lock. Guards [`KeyLockState`] under a `tokio::sync`
/// `Mutex` (the sanctioned exception — the guard lives across the
/// `.await` on `notify.notified()` and contention is bounded by the
/// wound-wait protocol). `Notify` wakes every waiter on each release/wound
/// so they re-evaluate compatibility.
#[derive(Debug)]
pub struct KeyLock {
    pub(crate) state: tokio::sync::Mutex<KeyLockState>,
    pub(crate) notify: tokio::sync::Notify,
}

impl KeyLock {
    fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(KeyLockState::default()),
            notify: tokio::sync::Notify::new(),
        }
    }
}

/// Per-store history retention — three ORTHOGONAL optional knobs
/// (TEMPORAL.md §3). Default = CurrentOnly (`max_count: Some(0)`): keep only
/// current + versions pinned by live snapshots. All three knobs are enforced
/// by [`MvccStore::vacuum_key`] (T1c wired `max_age_secs` once versions
/// carry a per-version commit timestamp).
///
/// Stored on [`MvccStore`] via `ArcSwap<Retention>` (lock-free swappable —
/// three fields can't be one atomic).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    /// AGE cap: reclaim versions whose commit timestamp is older than
    /// `max_age_secs` seconds (a version with no recorded ts is treated as
    /// "unknown age" and conservatively KEPT by the age axis).
    pub max_age_secs: Option<u64>,
    /// COUNT cap: keep at most N old versions per key (`None` = unlimited).
    pub max_count: Option<u64>,
    /// COUNT floor: always keep ≥ M newest old versions per key, EVEN IF
    /// older than `max_age_secs` (this is `min_count`'s real job — protect
    /// recent versions from the age cap). Inert against the count cap
    /// (validation guarantees `min_count ≤ max_count`, so the cap already
    /// keeps ≥ min_count).
    pub min_count: Option<u64>,
}

impl Default for Retention {
    fn default() -> Self {
        // CurrentOnly: keep 0 old versions (current + live-snapshot-pinned only).
        Self {
            max_age_secs: None,
            max_count: Some(0),
            min_count: None,
        }
    }
}

impl Retention {
    /// CurrentOnly: keep only current + versions pinned by live snapshots.
    pub fn current_only() -> Self {
        Self::default()
    }

    /// KeepHistory (Forever): retain all versions — no count cap.
    pub fn keep_history() -> Self {
        Self {
            max_age_secs: None,
            max_count: None,
            min_count: None,
        }
    }

    /// Validate: `min_count` must be `<= max_count` when both are `Some`.
    /// Returns `Err(message)` on violation.
    pub fn validate(&self) -> Result<(), String> {
        if let (Some(mc), Some(maxc)) = (self.min_count, self.max_count) {
            if mc > maxc {
                return Err(format!(
                    "retention: min_count ({mc}) must be <= max_count ({maxc})"
                ));
            }
        }
        Ok(())
    }
}

/// Versioned layer over the history version log.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    pub(crate) history: Arc<dyn Store>,
    pub(crate) gate: Arc<RepoTxGate>,
    /// In-memory coordination state: key → record cell (latest committed version).
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    pub(crate) cells: SccHashMap<Bytes, RecordCell>,
    /// Level-3 pessimistic lock registry. Populated ONLY for keys locked by a
    /// `Pessimistic` tx; stays empty otherwise → zero overhead on the snapshot
    /// / serializable read/write hot paths. Each entry is an `Arc<KeyLock>`
    /// shared between concurrent requesters of the same key.
    pub(crate) locks: SccHashMap<Bytes, Arc<KeyLock>>,
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
            cells: SccHashMap::new(),
            locks: SccHashMap::new(),
            retention: ArcSwap::new(Arc::new(Retention::current_only())),
            test_now_millis: AtomicU64::new(0),
        }
    }

    /// T1c: current wall-clock millis. If `test_now_millis` is non-zero
    /// (set via [`Self::set_test_now`]) that frozen value is returned;
    /// otherwise the real `SystemTime` since `UNIX_EPOCH` is used. Retention
    /// is calendar time, so `SystemTime` (not a monotonic clock) is correct.
    fn now_millis(&self) -> u64 {
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
    async fn record_ts(&self, version: u64) {
        let ms = self.now_millis().to_le_bytes();
        let _ = self
            .history
            .set(ts_key(version), Bytes::from(ms.to_vec()))
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
    async fn vacuum_key(&self, key: &Bytes) {
        let policy = self.retention();
        // No upper bound on either axis → nothing to reclaim.
        if policy.max_count.is_none() && policy.max_age_secs.is_none() {
            return;
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
                    entries.push((version, phys_key));
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
            let _ = self.history.remove(phys_key.clone()).await;
            let _ = self.history.remove(ts_key(*version)).await;
        }
    }

    /// T1c: look up the recorded commit timestamp (millis) for `version`.
    /// Returns `None` if no ts entry exists (treated as "unknown age" → the
    /// age axis conservatively keeps the version).
    async fn lookup_ts(&self, version: u64) -> Option<u64> {
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

    // ========================================================================
    // Versioning substrate — durable operations over the single `history` log.
    //
    // This region groups the version-log operations: the write path
    // (`set_versioned`, `set_versioned_many`, `delete_versioned`), the
    // snapshot-read resolver (`resolve_read`), and the committed-ops applier
    // (`apply_committed_ops`). The in-memory helper `publish_cell` names the
    // cell-update step shared across all write paths.
    //
    // The coordination accessors (`current_version` / `version_of` /
    // `live_version` / `seed_version`) and the Level-3 lock region are
    // deliberately kept separate from this substrate.
    // ========================================================================

    /// Publish `version` into the key's cell. Atomic modify-or-insert;
    /// advances the cell's version to `version` on every write path.
    /// (Bump-first ordering is the CALLER's job — this only performs the
    /// cell mutation.)
    async fn publish_cell(&self, key: Bytes, version: u64) {
        match self.cells.entry_async(key).await {
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
            return match self.history.get(encode_version_key(key, cur_v)).await {
                Ok(val) if val.is_empty() => Ok(None),
                Ok(val) => Ok(Some(val)),
                Err(DbError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            };
        }
        self.scan_history_for_version(key, snapshot_version).await
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
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v).await;
        let key_snapshot = key.clone();
        // Single log write: the current version goes into the log (sole write).
        self.history
            .set(encode_version_key(&key_snapshot, new_v), value)
            .await?;
        // T1c: record the commit timestamp for the age-retention axis.
        self.record_ts(new_v).await;
        // Advance the reader-visible floor so a tx/snapshot opened AFTER this
        // write sees it: `publish_committed_max` is a monotonic fetch_max
        // (lock-free, safe off `commit_lock`, never moves the floor backwards).
        self.gate.publish_committed_max(new_v);
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
        // Single log write — build history_ops (the sole durable write target).
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(items.len());
        let keys: Vec<Bytes> = items.iter().map(|(k, _)| k.clone()).collect();
        for (key, value) in &items {
            let new_v = self.gate.assign_next_version();
            self.publish_cell(key.clone(), new_v).await;
            new_versions.push(new_v);
            max_v = new_v;
            history_ops.push(KvOp::Set(encode_version_key(key, new_v), value.clone()));
        }

        // Single batched write to the log (sole durable write).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // T1c: record the commit timestamp for every version in the batch
        // (age-retention axis). Best-effort.
        for &v in &new_versions {
            self.record_ts(v).await;
        }

        // Advance the reader-visible floor to the batch's max version so a
        // tx/snapshot opened AFTER the batch sees every record in it.
        // `publish_committed_max` is monotonic (fetch_max) and safe off-lock.
        if max_v > 0 {
            self.gate.publish_committed_max(max_v);
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
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v).await;
        // Single log write: tombstone (empty value — MessagePack records are
        // never zero-length, so empty is unambiguously a delete).
        self.history
            .set(encode_version_key(&key, new_v), Bytes::new())
            .await?;
        // T1c: record the commit timestamp for the age-retention axis.
        self.record_ts(new_v).await;
        // Advance the reader-visible floor so a tx/snapshot opened AFTER this
        // delete sees the post-delete state: `publish_committed_max` is a
        // monotonic fetch_max (lock-free, safe off `commit_lock`).
        self.gate.publish_committed_max(new_v);
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
    pub async fn get_current(&self, key: Bytes) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(&key);
        let v = if cur_v > 0 {
            cur_v
        } else {
            match self.seek_latest_version(&key).await? {
                Some(v) => {
                    self.seed_version(key.clone(), v).await;
                    v
                }
                None => return Ok(None),
            }
        };
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
    pub fn current_stream(
        &self,
        batch: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        use futures::stream::unfold;

        let history = Arc::clone(&self.history);
        // Box::pin so the returned stream is `Unpin` — callers (e.g.
        // `TableManager::list_stream`) consume it via `.next()` without
        // pinning, matching the P1 `Pin<Box<dyn Stream>>` contract. A raw
        // `Unfold` over an async closure is NOT `Unpin`.
        Box::pin(unfold(
            StreamingGroupByState::Start {
                history,
                batch_size: batch,
            },
            |state| async move {
                match state {
                    StreamingGroupByState::Start {
                        history,
                        batch_size,
                    } => {
                        let stream = history.iter_stream(batch_size);
                        let pin = Box::pin(stream);
                        let s = StreamingGroupByState::Streaming {
                            stream: pin,
                            batch_size,
                            cur_key: None,
                            last_val: None,
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

    /// Acquire a Level-3 pessimistic lock on `key` for tx `tx_version` in
    /// `mode`, using the wound-wait protocol.
    ///
    /// `wounded` is the requesting tx's shared abort flag. If a strictly
    /// older (higher-priority) tx wounds THIS tx while it is waiting, the
    /// flag is set and this call returns
    /// [`DbError::Conflict`](shamir_storage::error::DbError::Conflict) so the
    /// tx aborts instead of acquiring the lock.
    ///
    /// Algorithm (loop):
    /// 1. Lock `state`.
    /// 2. If the requested `mode` is compatible with the current holders
    ///    (Shared+Shared compatible; anything with Exclusive incompatible;
    ///    a holder with the SAME `tx_version` is always compatible —
    ///    re-entrant), add the holder, set `mode`, return `Ok(())`.
    /// 3. Otherwise, for every CONFLICTING holder `H`:
    ///    - `tx_version < H.tx_version` (requester OLDER / higher priority):
    ///      WOUND `H` — set `H.wounded`, remove `H` from holders. After
    ///      wounding all conflicting younger holders, `notify_waiters()` and
    ///      loop again (the requester may now fit).
    ///    - `tx_version > H.tx_version` (requester YOUNGER): the requester
    ///      must WAIT. Drop the state lock, await `notify.notified()`, loop.
    ///    - `tx_version == H.tx_version`: same tx — compatible, skip.
    /// 4. Before waiting AND after being woken, check `wounded.load()`: if
    ///    this tx was wounded while waiting, return the conflict error.
    ///
    /// Correctness: a requester only ever waits on strictly-older holders
    /// and only ever wounds strictly-younger ones, so the wait-for graph
    /// respects the total version order and cannot cycle (deadlock-free by
    /// construction — no detector needed).
    pub async fn lock_key(
        &self,
        key: Bytes,
        tx_version: u64,
        wounded: Arc<AtomicBool>,
        wound_notify: Arc<tokio::sync::Notify>,
        mode: LockMode,
    ) -> DbResult<()> {
        // Get-or-insert the KeyLock for this key. The Arc is shared between
        // concurrent requesters so they coordinate on the same Mutex/Notify.
        let lock = match self.locks.entry_async(key).await {
            scc::hash_map::Entry::Occupied(e) => Arc::clone(e.get()),
            scc::hash_map::Entry::Vacant(e) => {
                let arc = Arc::new(KeyLock::new());
                e.insert_entry(Arc::clone(&arc));
                arc
            }
        };

        loop {
            // (4) Abort early if this tx was already wounded by an older tx.
            if wounded.load(Ordering::Acquire) {
                return Err(DbError::Conflict(format!(
                    "tx {} wounded (wound-wait abort) before acquiring lock",
                    tx_version
                )));
            }

            let mut state = lock.state.lock().await;

            // (2) Compatibility check.
            //
            // - Re-entrant (this tx already holds the key): always compatible.
            // - Shared request vs Shared holders: compatible (multiple readers).
            // - Anything else (Exclusive involved, or Shared vs Exclusive):
            //   incompatible.
            let re_entrant = state.held_by(tx_version);
            let compatible = re_entrant
                || state.mode.is_none()
                || (mode == LockMode::Shared && state.mode == Some(LockMode::Shared));

            if compatible {
                // Re-entrant re-acquire: if this tx already holds the key,
                // do NOT push a duplicate holder (would violate the
                // distinct-id invariant and skew mode recomputation). Just
                // return Ok — the existing holder already grants access.
                if !re_entrant {
                    state.holders.push(Holder {
                        tx_version,
                        wounded: Arc::clone(&wounded),
                        wound_notify: Arc::clone(&wound_notify),
                    });
                }
                // Set the mode. An Exclusive requester that re-enters a key
                // it already holds Shared upgrades the recorded mode so a
                // later third-tx Shared requester correctly sees conflict.
                state.mode = Some(mode);
                return Ok(());
            }

            // (3) Incompatible. Partition the conflicting holders.
            //
            // Younger holders (tx_version < H.tx_version) get WOUNDED and
            // removed. If ANY holder is strictly OLDER than the requester
            // (tx_version > H.tx_version) and conflicts, the requester must
            // WAIT (it cannot wound the older holder). Same-tx holders are
            // never conflicting (handled by the re-entrant branch above).
            let mut must_wait = false;
            let mut wounded_any = false;
            // Collect indices of younger holders to remove (wound them).
            // Iterate back-to-front so swap_remove preserves indices.
            let mut i = state.holders.len();
            while i > 0 {
                i -= 1;
                let h = &state.holders[i];
                // Skip same-tx holders (re-entrant, never conflicting).
                if h.tx_version == tx_version {
                    continue;
                }
                // This holder conflicts with the request (we're in the
                // incompatible branch). Decide wound vs wait by age.
                if tx_version < h.tx_version {
                    // Requester is OLDER → wound the younger holder. Set
                    // the flag AND wake the holder's per-tx notify so it
                    // observes the wound even if it is parked waiting on
                    // a DIFFERENT key (load-bearing for deadlock-freedom:
                    // a wound on key Y must wake a tx parked on key X).
                    h.wounded.store(true, Ordering::Release);
                    h.wound_notify.notify_one();
                    wounded_any = true;
                    state.holders.swap_remove(i);
                } else {
                    // Requester is YOUNGER → must wait for the older holder.
                    must_wait = true;
                }
            }

            if wounded_any {
                // Recompute the aggregate mode from surviving holders.
                state.recompute_mode();
                // Wake any waiters so they observe the wounds / freed slots.
                lock.notify.notify_waiters();
            }

            if must_wait {
                // (4) Re-check wounded before suspending — an older tx may
                // have wounded this one between the top-of-loop check and
                // here (we held the state lock the whole time, so in fact
                // only wounds issued before we acquired the lock could have
                // landed; still, the check is cheap and correct).
                if wounded.load(Ordering::Acquire) {
                    return Err(DbError::Conflict(format!(
                        "tx {} wounded (wound-wait abort) while waiting",
                        tx_version
                    )));
                }
                // Register the key-notify waiter BEFORE dropping the state
                // lock. `tokio::sync::Notify::notify_waiters` only wakes
                // futures already in the notified() queue — it does NOT store
                // a permit. If we created the future after `drop(state)`, a
                // `release_locks` → `notify_waiters()` firing in the window
                // between the drop and the first poll would be LOST, and the
                // waiting tx could hang forever on a multi-threaded runtime.
                // `enable()` enters the waiter queue synchronously while we
                // still hold `state`, closing that window.
                let mut notified = Box::pin(lock.notify.notified());
                notified.as_mut().enable();
                drop(state);
                tokio::select! {
                    _ = notified.as_mut() => {}
                    _ = wound_notify.notified() => {}
                }
                continue;
            }

            // We wounded everyone conflicting and nobody older remains. Loop
            // to re-acquire the state lock and retry the compatibility check
            // (the freed slots should now let us in).
        }
    }

    /// Release every lock held by `tx_version` on the given `keys`.
    ///
    /// Called on BOTH commit and abort/drop of a Level-3 tx. For each key,
    /// locks the state, removes all holders with the given `tx_version`,
    /// recomputes the mode, and wakes waiters. Leftover empty entries are
    /// kept in the map (cheap; GC is intentionally not done here).
    pub async fn release_locks(&self, tx_version: u64, keys: &[Bytes]) {
        for key in keys {
            let Some(lock) = self.locks.get(key).map(|e| Arc::clone(e.get())) else {
                continue;
            };
            let mut state = lock.state.lock().await;
            let before = state.holders.len();
            state.holders.retain(|h| h.tx_version != tx_version);
            if state.holders.len() != before {
                state.recompute_mode();
                // Wake waiters so a blocked tx can re-evaluate.
                lock.notify.notify_waiters();
            }
        }
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
        self.cells.upsert_async(key, RecordCell { version }).await;
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
        let mut per_key: std::collections::HashMap<Vec<u8>, Vec<(u64, Bytes)>> =
            std::collections::HashMap::new();

        while let Some(batch) = stream.next().await {
            for (phys_key, _value) in batch? {
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    if version < min_version {
                        per_key
                            .entry(orig.to_vec())
                            .or_default()
                            .push((version, phys_key));
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
                let _ = self.history.remove(phys_key.clone()).await;
                // T1c: remove the ts-key in lockstep so timestamps don't
                // outlive their versions.
                let _ = self.history.remove(ts_key(*version)).await;
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

        let mut per_key: std::collections::HashMap<Vec<u8>, Vec<(u64, Bytes)>> =
            std::collections::HashMap::new();

        while let Some(batch) = stream.next().await {
            for (phys_key, _value) in batch? {
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    per_key
                        .entry(orig.to_vec())
                        .or_default()
                        .push((version, phys_key));
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
                let _ = self.history.remove(phys_key.clone()).await;
                let _ = self.history.remove(ts_key(*version)).await;
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
    async fn prune_version_cache(&self) {
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

    /// Range-scan the log for the latest version ≤ `snapshot`.
    /// Returns `None` for tombstones (empty value) and absent keys.
    async fn scan_history_for_version(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>> {
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
    async fn seek_latest_version(&self, key: &[u8]) -> DbResult<Option<u64>> {
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
        use futures::StreamExt;

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
        use crate::version_codec::decode_version_key;

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
}

/// One `current_stream` output batch (raw `(key, value)` pairs).
type LogBatch = DbResult<Vec<(Bytes, Bytes)>>;
/// A boxed, `Unpin`, `Send` stream of log batches — the inner stream the
/// group-by drains.
type BoxedLogStream = std::pin::Pin<Box<dyn futures::Stream<Item = LogBatch> + Send>>;

/// C2 streaming group-by state for `current_stream`. The log is key-major,
/// version-ascending; this state tracks the last (highest) version per key
/// and emits the current value once all versions of a key have been seen.
enum StreamingGroupByState {
    Start {
        history: Arc<dyn Store>,
        batch_size: usize,
    },
    Streaming {
        stream: BoxedLogStream,
        batch_size: usize,
        /// `(original_key_bytes, last_value)` — the group being accumulated.
        cur_key: Option<Bytes>,
        last_val: Option<Bytes>,
        /// Output batch being built.
        out_batch: Vec<(Bytes, Bytes)>,
    },
    Done,
}

impl StreamingGroupByState {
    /// Drain log batches through the group-by, emitting whenever the
    /// output batch reaches `batch_size` or the stream ends.
    /// Returns `Option<(Item, NextState)>` for `futures::stream::unfold`.
    async fn drain_and_emit(self) -> Option<(Result<Vec<(Bytes, Bytes)>, DbError>, Self)> {
        // Pull the streaming fields out; re-pack on return.
        let (stream, batch_size, mut cur_key, mut last_val, mut out_batch) = match self {
            StreamingGroupByState::Streaming {
                stream,
                batch_size,
                cur_key,
                last_val,
                out_batch,
            } => (stream, batch_size, cur_key, last_val, out_batch),
            _ => return None,
        };
        let mut stream = stream;
        loop {
            match stream.next().await {
                Some(Ok(batch)) => {
                    for (phys_key, val) in batch {
                        if let Some((orig, _v)) = decode_version_key(&phys_key) {
                            let orig_bytes = Bytes::copy_from_slice(orig);
                            if cur_key.as_deref() != Some(orig) {
                                // Flush previous group.
                                if let (Some(ck), Some(lv)) = (cur_key.take(), last_val.take()) {
                                    if !lv.is_empty() {
                                        out_batch.push((ck, lv));
                                    }
                                }
                                cur_key = Some(orig_bytes);
                            }
                            last_val = Some(val);
                        }
                        // ts-keys and non-version keys are silently skipped.
                    }
                    if out_batch.len() >= batch_size {
                        let emit = std::mem::take(&mut out_batch);
                        return Some((
                            Ok(emit),
                            StreamingGroupByState::Streaming {
                                stream,
                                batch_size,
                                cur_key,
                                last_val,
                                out_batch,
                            },
                        ));
                    }
                }
                Some(Err(e)) => {
                    return Some((
                        Err(e),
                        StreamingGroupByState::Streaming {
                            stream,
                            batch_size,
                            cur_key,
                            last_val,
                            out_batch,
                        },
                    ))
                }
                None => {
                    // Stream ended — flush final group.
                    if let (Some(ck), Some(lv)) = (cur_key.take(), last_val.take()) {
                        if !lv.is_empty() {
                            out_batch.push((ck, lv));
                        }
                    }
                    if out_batch.is_empty() {
                        return None;
                    }
                    let emit: Vec<_> = out_batch;
                    return Some((Ok(emit), StreamingGroupByState::Done));
                }
            }
        }
    }
}

/// T4-history: one row in a key's version timeline.
///
/// Returned by [`MvccStore::history_of`]. `version` is the monotonic
/// commit version assigned by `RepoTxGate`; `value` is the bytes that
/// were current at that version (all versions, including the current one,
/// are read from the single `history` log); `ts_millis` is
/// the per-version commit timestamp (T1c), or `None` when no ts was
/// recorded for this version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionEntry {
    /// Monotonic commit version assigned by `RepoTxGate::assign_next_version`.
    pub version: u64,
    /// The value bytes current at `version` (MessagePack-encoded
    /// `InnerValue` for record keys, raw user bytes otherwise).
    pub value: Bytes,
    /// Per-version commit timestamp in milliseconds since UNIX_EPOCH
    /// (T1c, recorded via [`MvccStore::record_ts`] / `ts_key`). `None`
    /// when no ts entry exists for this version.
    pub ts_millis: Option<u64>,
}
