//! Versioned KV layer over two dumb-KV stores (main + history).
//!
//! [`MvccStore`] wraps two [`Store`] instances:
//! - `main` — the current version of every key (identical to today's
//!   non-tx writes).
//! - `history` — old versions stored under `<key>::0xFF::<version_be>`
//!   keys (see [`version_codec`]).
//!
//! T1a (always-archive): every write/delete runs the snapshot-active
//! "slow" path unconditionally — the prior value is archived to `history`
//! before `main` is touched. With the no-snapshot fast path gone, the
//! MVCC-2 TOCTOU window (`active_snapshots_empty()` then `main.set`)
//! cannot occur: a snapshot opening at any time finds the prior version
//! archived in `history`. `history` is the universal version-log (see
//! docs/roadmap/TEMPORAL.md §1); `main` is the current-value cache.
//!
//! Snapshot reads via [`MvccStore::get_at`] use a fast path (version cache
//! check → main read) and fall back to a history range scan.

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
fn ts_key(version: u64) -> Bytes {
    let mut b = BytesMut::with_capacity(9);
    b.put_u8(TS_TAG);
    b.put_u64(version);
    b.freeze()
}

/// Decode a timestamp key back into its version. Returns `None` if the input
/// is not a 9-byte ts-key (`[TS_TAG][8 bytes]`). Used by retention tests;
/// kept as the logical inverse of [`ts_key`].
#[allow(dead_code)]
fn decode_ts_key(physical: &[u8]) -> Option<u64> {
    if physical.len() != 9 || physical[0] != TS_TAG {
        return None;
    }
    let version_bytes: [u8; 8] = physical[1..].try_into().expect("just checked length");
    Some(u64::from_be_bytes(version_bytes))
}

/// Per-key in-memory coordination state — the "record cell".
/// The durable data stays in `main`/`history`; the cell is rebuildable
/// in-memory coordination.
#[derive(Debug, Clone, Copy)]
struct RecordCell {
    /// Archive-routing version: set ONLY when a snapshot was active at
    /// write time (slow path). Read by get_at / current_version / version_of.
    /// Semantics unchanged from before this slice.
    version: u64,
    /// High-water mark: the latest version assigned to ANY write of this
    /// key, maintained on EVERY write path (fast AND slow), set BEFORE the
    /// physical main-store mutation. Read only by `live_version` (index-only
    /// freshness validation). Never consulted by get_at.
    hwm: u64,
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
struct Holder {
    tx_version: u64,
    wounded: Arc<AtomicBool>,
    wound_notify: Arc<tokio::sync::Notify>,
}
/// The mutable state of one key's lock: the set of current holders plus
/// the aggregate mode (`None` when unheld). Invariant: when `mode` is
/// `Some(Exclusive)`, `holders` has exactly one entry; when `Some(Shared)`,
/// every holder is a distinct tx (no duplicate ids).
#[derive(Debug, Default)]
struct KeyLockState {
    holders: Vec<Holder>,
    mode: Option<LockMode>,
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
    state: tokio::sync::Mutex<KeyLockState>,
    notify: tokio::sync::Notify,
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

/// Versioned layer over two dumb-KV stores.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    main: Arc<dyn Store>,
    history: Arc<dyn Store>,
    gate: Arc<RepoTxGate>,
    /// In-memory coordination state: key → record cell (latest committed version).
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    cells: SccHashMap<Bytes, RecordCell>,
    /// Level-3 pessimistic lock registry. Populated ONLY for keys locked by a
    /// `Pessimistic` tx; stays empty otherwise → zero overhead on the snapshot
    /// / serializable read/write hot paths. Each entry is an `Arc<KeyLock>`
    /// shared between concurrent requesters of the same key.
    locks: SccHashMap<Bytes, Arc<KeyLock>>,
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
    /// Create a new MVCC store from two backing stores and a gate.
    ///
    /// Defaults to [`Retention::current_only`] (eager vacuum). Use
    /// [`Self::set_retention`] to opt into [`Retention::keep_history`] or a
    /// custom [`Retention`].
    pub fn new(main: Arc<dyn Store>, history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self {
        Self {
            main,
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
    /// live snapshot) is never reclaimed regardless of any knob; the current
    /// value lives in `main` (never in `history`).
    ///
    /// Anchor: when a live snapshot exists below `current`, the SINGLE largest
    /// version `< min_alive` is also kept — it serves a snapshot reading a key
    /// last-written below `min_alive`. When no live snapshot exists, no anchor
    /// is needed: a fresh snapshot opens at `current` and reads `main`.
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
        let stream = self.history.scan_prefix_stream(prefix, 256);
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
    // Versioning substrate (future WriteStrategy seam — see
    // docs/roadmap/MVCC_CELL.md §7).
    //
    // This region groups the durable-versioned-KV operations over the
    // `main`/`history` stores: the write/delete paths, the snapshot-read
    // resolver, and the committed-ops applier. R1 extracts three private
    // helpers (`publish_cell`, `resolve_read`) that name repeated patterns;
    // repeated patterns; the bodies are byte-identical to the inline blocks
    // they replace. A future slice will lift this region behind a
    // `trait WriteStrategy` (R2 — out of scope here).
    //
    // The coordination accessors below (`current_version` / `version_of` /
    // `live_version` / `seed_version`) and the Level-3 lock region are
    // deliberately kept separate from this substrate.
    // ========================================================================

    /// Publish `version` into the key's cell. `slow` = a snapshot was active
    /// at write time (also advances the archive-routing `version`); otherwise
    /// only the always-maintained `hwm` advances. Atomic modify-or-insert;
    /// preserves `version` on the fast path. (Bump-first ordering is the
    /// CALLER's job — this only performs the cell mutation.)
    async fn publish_cell(&self, key: Bytes, version: u64, slow: bool) {
        match self.cells.entry_async(key).await {
            scc::hash_map::Entry::Occupied(mut e) => {
                if slow {
                    e.get_mut().version = version;
                }
                e.get_mut().hwm = version;
            }
            scc::hash_map::Entry::Vacant(e) => {
                e.insert_entry(RecordCell {
                    version: if slow { version } else { 0 },
                    hwm: version,
                });
            }
        }
    }

    /// Resolve a versioned read of `key` visible at `snapshot_version`, given
    /// its current cached version `cur_v`. Fast path (`cur_v > 0 && cur_v <= snapshot`):
    /// read `history` at `encode_version_key(key, cur_v)`; slow path: range-scan
    /// `history` for the newest version `<= snapshot`. C2: reads exclusively from
    /// the log (no `main` read).
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

    /// cancel-safe: NO — multi-step state mutation. T1a (always-archive):
    /// every write runs the snapshot-active "slow" path unconditionally —
    /// archive the prior value to `history`, then write `main`, then
    /// publish the cell. Removing the no-snapshot fast path closes the
    /// MVCC-2 TOCTOU window by construction: a snapshot opening at any
    /// time finds the prior version archived in `history`. Cancellation
    /// between archive and main-write can leave `history` containing a
    /// value while `main` still has the old one. Recovery is by
    /// caller-side retry / WAL replay.
    ///
    /// Returns the monotonic version assigned to this write (from the
    /// shared `RepoTxGate` counter).
    pub async fn set_versioned(&self, key: Bytes, value: Bytes) -> DbResult<u64> {
        // C1: no archive_prior — the prior version is already in the log from
        // when it was written as current. The new current is written into the
        // log (dual-write) alongside `main` so reads (still from `main`) are
        // unchanged this slice.
        //
        // Bump-first: assign version, update cell (both version and hwm), then
        // perform the physical write. CRIT-2: `publish_cell` uses entry_async
        // (modify-or-insert) so repeated writes to the same key advance the
        // cached version monotonically.
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v, true).await;
        let key_snapshot = key.clone();
        // C1 dual-write: current version goes into the log.
        self.history
            .set(encode_version_key(&key_snapshot, new_v), value.clone())
            .await?;
        self.main.set(key, value).await?;
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

    /// cancel-safe: NO — multi-step state mutation. T1a (always-archive):
    /// every batch runs the archive → main-write → cell-publish sequence
    /// unconditionally — the no-snapshot fast path is gone, so the MVCC-2
    /// TOCTOU window cannot occur for batch writes either. Cancellation
    /// mid-sequence leaves the store partial; recovery is caller-side
    /// retry / WAL replay.
    ///
    /// Batched non-tx write of many `(key, value)` pairs — the bulk-load
    /// twin of [`set_versioned`]. III.4: a non-tx `insert_many` that loops
    /// per-record `set_versioned` issues N separate write-transactions on
    /// disk backends (N× fsync amplification). This collapses the main
    /// writes into a single `Store::transact`, which is one atomic write-tx
    /// (one fsync) on backends that override `transact` (redb, sled, fjall,
    /// persy, nebari, canopy).
    ///
    /// Semantics match calling [`set_versioned`] once per pair, in order:
    /// archive any existing old value per key into history, write all
    /// news to main in one `transact`, and assign a fresh monotonic
    /// version per key (one version per record, identical to the
    /// per-record loop).
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

        // C1: no archive_prior — prior versions are already in the log.
        // Phase 1 (bump-first): assign a fresh version per key and update the
        // cell (both version and hwm) BEFORE the physical main write.
        // CRIT-2: `publish_cell` uses entry_async modify-or-insert so the
        // cached version advances monotonically.
        let mut max_v = 0u64;
        let mut new_versions: Vec<u64> = Vec::with_capacity(items.len());
        // C1 dual-write: build history_ops (current-into-log) alongside
        // the main ops.
        let mut history_ops: Vec<KvOp> = Vec::with_capacity(items.len());
        for (key, value) in &items {
            let new_v = self.gate.assign_next_version();
            self.publish_cell(key.clone(), new_v, true).await;
            new_versions.push(new_v);
            max_v = new_v;
            history_ops.push(KvOp::Set(encode_version_key(key, new_v), value.clone()));
        }

        // C1: transact the current-into-log entries into history (one batched
        // write).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 2: one batched write to main.
        let keys: Vec<Bytes> = items.iter().map(|(k, _)| k.clone()).collect();
        let main_ops: Vec<KvOp> = items.into_iter().map(|(k, v)| KvOp::Set(k, v)).collect();
        self.main.transact(main_ops).await?;

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
    /// `set_versioned`. T1a (always-archive): the archive runs
    /// unconditionally, then `main.remove`, then version allocation +
    /// cell publish. Cancellation mid-sequence leaves the store in a
    /// partial state; caller-side retry / WAL replay is the recovery path.
    ///
    /// Returns the monotonic version assigned to this delete (always
    /// allocated — see [`set_versioned`] for rationale).
    pub async fn delete_versioned(&self, key: Bytes) -> DbResult<u64> {
        // C1: no archive_prior — the prior version is already in the log from
        // when it was written as current. A tombstone (empty value) is written
        // into the log for the delete version.
        // Bump-first: assign version, update cell (version+hwm), then remove.
        // CRIT-2: `publish_cell` uses entry_async modify-or-insert so the
        // cached version advances monotonically.
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v, true).await;
        // C1: write tombstone into the log (empty value unambiguously means
        // deleted — MessagePack records are never zero-length).
        self.history
            .set(encode_version_key(&key, new_v), Bytes::new())
            .await?;
        // Propagate a backend I/O failure instead of swallowing it — a
        // dropped error here would let the caller see Ok() while the row is
        // still live in main (the delete silently never happened).
        let key_snapshot = key.clone();
        self.main.remove(key).await?;
        // T1c: record the commit timestamp for the age-retention axis.
        self.record_ts(new_v).await;
        // Advance the reader-visible floor so a tx/snapshot opened AFTER this
        // delete sees the post-delete state: `publish_committed_max` is a
        // monotonic fetch_max (lock-free, safe off `commit_lock`).
        self.gate.publish_committed_max(new_v);
        // T1b.2: per-key count-aware vacuum reclaims superseded history
        // versions beyond the retention bound (floor: min_alive).
        self.vacuum_key(&key_snapshot).await;
        Ok(new_v)
    }

    /// cancel-safe: yes — read-only. Fast path is a single `main.get`;
    /// slow path is a read-only history range scan. Cancellation drops
    /// the future with no state mutation.
    ///
    /// Snapshot read: return the value visible at `snapshot_version`.
    ///
    /// Fast path: if version_cache says current version ≤ snapshot →
    /// return `main.get(key)`.
    /// Slow path: range scan history `[key::0, key::snapshot]`, take last.
    pub async fn get_at(&self, key: &[u8], snapshot_version: u64) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(key);
        self.resolve_read(key, snapshot_version, cur_v).await
    }

    /// Direct access to main store (for non-tx reads).
    pub fn main_store(&self) -> &Arc<dyn Store> {
        &self.main
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
    fn current_version(&self, key: &[u8]) -> u64 {
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

    /// The high-water-mark version for `key` — the latest version any write
    /// assigned to it — or `None` if no write has touched this key through
    /// this store in-process. Used by the index-only read path to validate a
    /// covering posting: a posting whose embedded version equals this hwm is
    /// fresh; `None` means "no in-process mutation" (the durable posting is
    /// consistent). Distinct from `version_of` (archive-routing).
    pub fn live_version(&self, key: &[u8]) -> Option<u64> {
        self.cells.read(key, |_, c| c.hwm)
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
    /// the record body directly to the backing `main` store, bypassing
    /// [`apply_committed_ops`]. That keeps `main` correct but leaves
    /// `version_cache` empty, so a later `get_at(key, snap)` for a
    /// snapshot *below* `commit_version` would take the fast path
    /// (`current_version == 0 ≤ snap`) and return the recovered (latest)
    /// value instead of scanning history.
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
                RecordCell {
                    version,
                    hwm: version,
                },
            )
            .await;
    }

    /// cancel-safe: NO — applies a batch of `KvOp` via multi-step
    /// sequences (history transact, main transact, version_cache updates).
    /// Cancellation mid-batch leaves some phases applied, others not.
    /// Recovery relies on WAL replay (commit_tx invariant).
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

        // One batched write to history (current-into-log + tombstones).
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 2: one batched write to main.
        self.main.transact(ops.clone()).await?;

        // T1c: record the commit timestamp for the tx commit version (one ts
        // per commit — all ops share `commit_version`). Best-effort.
        self.record_ts(commit_version).await;

        // Phase 3: update the in-memory cell for every touched key.
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
            self.publish_cell(key, commit_version, true).await;
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
        let stream = self.history.iter_stream(256);
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
    /// Current versions now live in `history` too (C1 dual-write), so an
    /// explicit `cur_v` guard prevents reclaiming them.
    ///
    /// When a version is reclaimed, its `ts_key(version)` is removed in
    /// lockstep so timestamps never outlive their versions. Returns the
    /// number of history version entries deleted.
    pub async fn purge_below_ts(&self, cutoff_millis: u64) -> DbResult<usize> {
        use crate::version_codec::decode_version_key;

        // Phase 1: scan all history version entries, group by key.
        // ts-keys ([TS_TAG][v_be], 9 bytes) are skipped: decode_version_key
        // returns None for them (separator 0x00 != VERSION_SEP).
        let stream = self.history.iter_stream(256);
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
    /// if `cur_v <= snapshot`, returns `main.get(key)` (fast path);
    /// otherwise it scans history for the newest version `<= snapshot`.
    /// The cache entry only matters when it forces the *slow* path, i.e.
    /// for snapshots `< cur_v`. Evicting an entry makes `current_version`
    /// return `0`, so every snapshot takes the fast path and reads `main`.
    ///
    /// An entry with `cv < min_alive` satisfies `cv < min_alive <= s` for
    /// *every* live snapshot `s` (no snapshot is older than `min_alive`).
    /// Thus `cv <= s` already held for all of them — they were *already*
    /// on the fast path. After eviction `0 <= s` still routes them to the
    /// fast path and `main` still holds the key's current value (version
    /// `cv`), so the returned value is identical. The only thing forgotten
    /// is the version *number*, and it was needed solely to force a
    /// history scan for snapshots below `cv` — none of which exist. Hence
    /// the prune is value-preserving for all live readers.
    ///
    /// Conversely, evicting entries with `cv >= min_alive` would be unsafe:
    /// a live snapshot `s` with `min_alive <= s < cv` legitimately needs
    /// the slow path (its visible value lives in history, archived when
    /// the value advanced to `cv`); forgetting `cv` would route it to the
    /// fast path and hand it the wrong (newer) `main` value. That is why
    /// the threshold is `min_alive` and not the (possibly larger)
    /// `min_version` history-GC argument.
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

    /// Slow path: range scan history for the latest version ≤ `snapshot`.
    /// C2: if the latest value is a tombstone (empty), returns `None`.
    async fn scan_history_for_version(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>> {
        let lo = encode_version_key(key, 0);
        let hi = encode_version_key(key, snapshot);
        let stream = self.history.iter_range_stream(Some(lo), Some(hi), 64);
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
        let stream = self.history.iter_range_stream(Some(lo), Some(hi), 64);
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

        let stream = self.history.iter_stream(256);
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
    /// Merges two sources:
    /// 1. `history` — every archived prior version, stored under
    ///    `encode_version_key(key, version)` (`<key> || 0xFF || version_be`).
    ///    The range scan `[encode_version_key(key, 0), +∞)` yields all
    ///    versioned entries for this key. ts-keys (`[TS_TAG][version_be]`,
    ///    9 bytes, `TS_TAG = 0x00`) are out of this key's range and are
    ///    additionally rejected by `decode_version_key` (which returns
    ///    `None` when the separator byte is not `VERSION_SEP`), so they
    ///    can never be mistaken for a version entry.
    /// 2. `main` — the CURRENT version. Its version number is the cell's
    ///    `version_of(key)` (0 when the key has only ever taken the fast
    ///    path or is absent). A key that is currently DELETED contributes
    ///    no current entry; its prior versions still appear from history.
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
        let stream = self.history.iter_range_stream(Some(lo), None, 64);
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

        // Phase 2: append the CURRENT version from `main` if the key
        // currently exists and is NOT already in the log entries (C1:
        // the current is now dual-written into history; avoid duplicates).
        // `version_of` reads the cell's archive-routing version (0 on the
        // fast path / absent key). `main.get` tells us whether the key is live.
        let cur_v = self.current_version(key);
        if cur_v > 0 && !entries.iter().any(|(v, _)| *v == cur_v) {
            match self.main.get(Bytes::copy_from_slice(key)).await {
                Ok(cur_val) => entries.push((cur_v, cur_val)),
                Err(DbError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

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
/// were current at that version (archived in `history` for prior
/// versions, read from `main` for the current version); `ts_millis` is
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

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;

    fn make_gate() -> Arc<RepoTxGate> {
        Arc::new(RepoTxGate::fresh())
    }

    fn make_mvcc() -> MvccStore {
        let gate = make_gate();
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    fn make_mvcc_with_gate(gate: Arc<RepoTxGate>) -> MvccStore {
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    #[tokio::test]
    async fn set_without_snapshots_skips_history() {
        let mvcc = make_mvcc();
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        // main has the value
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v1"));

        // C1: the current version is now written into the log (dual-write),
        // so exactly 1 version-key entry exists alongside ts-keys.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut version_keys = 0;
        while let Some(batch) = stream.next().await {
            for (hk, _) in batch.unwrap() {
                if crate::version_codec::decode_version_key(&hk).is_some() {
                    version_keys += 1;
                }
            }
        }
        assert_eq!(
            version_keys, 1,
            "C1: current version is in the log (1 version-key entry)"
        );
    }

    #[tokio::test]
    async fn set_with_snapshot_archives_old_value() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // Open a snapshot so active_snapshots is non-empty.
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // main has v2
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v2"));

        // C1: history contains BOTH v1 (old, archived when v2 was written)
        // and v2 (current, written into the log by dual-write).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found_v1 = false;
        let mut found_v2 = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                let Some((orig_key, ver)) = crate::version_codec::decode_version_key(&hk) else {
                    continue;
                };
                assert_eq!(orig_key, &b"k1"[..]);
                if ver == 1 {
                    assert_eq!(hv, Bytes::from("v1"));
                    found_v1 = true;
                } else if ver == 2 {
                    assert_eq!(hv, Bytes::from("v2"));
                    found_v2 = true;
                }
            }
        }
        assert!(found_v1, "history should contain v1");
        assert!(
            found_v2,
            "C1: history should contain v2 (current in the log)"
        );
    }

    #[tokio::test]
    async fn get_at_current_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let v = gate.assign_next_version();
        // snapshot_version >> v → fast path returns from main
        let result = mvcc.get_at(b"k1", v + 100).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_old_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 written at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // v2 written at version 2
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // Query at snapshot between v1 and v2 — should find v1 in history.
        // After two set_versioned calls, version_cache[k1] = 2.
        // get_at(snapshot=1) → cur_v(2) > 1 → slow path → scan history.
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_missing_key() {
        let mvcc = make_mvcc();
        let result = mvcc.get_at(b"nonexistent", 999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_versioned_archives() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // main no longer has the key
        assert!(mvcc.main.get(key).await.is_err());

        // C1: history contains v1 (the prior, still in the log from when it
        // was current) and v2 (the delete's tombstone — empty value).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found_v1 = false;
        let mut found_tombstone = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                let Some((_orig_key, ver)) = crate::version_codec::decode_version_key(&hk) else {
                    continue;
                };
                if ver == 1 {
                    assert_eq!(hv, Bytes::from("v1"));
                    found_v1 = true;
                } else if ver == 2 {
                    assert_eq!(
                        hv,
                        Bytes::new(),
                        "C1: delete writes tombstone (empty value)"
                    );
                    found_tombstone = true;
                }
            }
        }
        assert!(found_v1, "history should contain v1");
        assert!(
            found_tombstone,
            "C1: history should contain the delete tombstone"
        );
    }

    #[tokio::test]
    async fn get_at_after_delete() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // delete at version 2
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // get_at between v1 and delete → slow path → v1 from history
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));

        // get_at after delete → fast path (cur_v=2 <= 15) → main.get → NotFound → None
        let result = mvcc.get_at(b"k1", 15).await.unwrap();
        assert!(result.is_none());
    }

    // ========================================================================
    // P1 — "MVCC owns current state" read-seam (get_current / current_stream).
    // These tests pin the behaviour-identical contract: both methods must
    // return EXACTLY what `main` (the current-value cache) returns today.
    // A later collapse-main slice rewrites the bodies over the single version
    // log; these tests are the regression net that catches any divergence.
    // ========================================================================

    /// `get_current` mirrors `main.get`: a written key returns `Some(v)`, an
    /// absent key returns `Ok(None)` (NOT an error). Both assertions compare
    /// the seam against a direct `main_store().get` so they fail if the body
    /// ever diverges from the current-value cache.
    #[tokio::test]
    async fn get_current_matches_main_get() {
        let mvcc = make_mvcc();
        let key = Bytes::from("cur-k1");
        let val = Bytes::from("cur-v1");

        mvcc.set_versioned(key.clone(), val.clone()).await.unwrap();

        // Seam returns the written value.
        let via_seam = mvcc.get_current(key.clone()).await.unwrap();
        assert_eq!(via_seam, Some(val.clone()));

        // Seam equals a direct main read — diverges if get_current ever stops
        // routing through `main`.
        let via_main = mvcc.main_store().get(key.clone()).await.unwrap();
        assert_eq!(
            via_seam.as_ref().map(|b| b.as_ref()),
            Some(via_main.as_ref())
        );

        // A key never written → Ok(None), NOT an Err. Diverges if get_current
        // propagates `NotFound` instead of mapping it to `None`.
        let absent = mvcc
            .get_current(Bytes::from("never-written"))
            .await
            .unwrap();
        assert!(
            absent.is_none(),
            "absent key must be Ok(None), not an error"
        );
    }

    /// After `delete_versioned`, `main` has no entry and `get_current` must
    /// return `Ok(None)`. Fails if the seam ever returns a stale value or
    /// surfaces `NotFound` as an error.
    #[tokio::test]
    async fn get_current_none_after_delete() {
        let mvcc = make_mvcc();
        let key = Bytes::from("del-k1");

        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // Sanity: present before delete.
        assert_eq!(
            mvcc.get_current(key.clone()).await.unwrap(),
            Some(Bytes::from("v1"))
        );

        mvcc.delete_versioned(key.clone()).await.unwrap();

        // main has no current entry for this key.
        assert!(
            mvcc.main_store().get(key.clone()).await.is_err(),
            "main must have no entry after delete_versioned"
        );
        // Seam agrees: Ok(None).
        let after = mvcc.get_current(key.clone()).await.unwrap();
        assert!(after.is_none(), "get_current must be Ok(None) after delete");
    }

    /// `current_stream(batch)` yields exactly the same `(key, value)` set as
    /// `main.iter_stream(batch)`. Collects both into maps and compares — any
    /// divergence (missing key, extra key, wrong value) fails the equality.
    #[tokio::test]
    async fn current_stream_lists_all_current() {
        use std::collections::BTreeMap;

        let mvcc = make_mvcc();

        let pairs: &[(Bytes, Bytes)] = &[
            (Bytes::from("s-a"), Bytes::from("va")),
            (Bytes::from("s-b"), Bytes::from("vb")),
            (Bytes::from("s-c"), Bytes::from("vc")),
        ];
        for (k, v) in pairs {
            mvcc.set_versioned(k.clone(), v.clone()).await.unwrap();
        }

        // Collect the seam stream into a map.
        let mut via_seam: BTreeMap<Vec<u8>, Bytes> = BTreeMap::new();
        let seam_stream = mvcc.current_stream(64);
        futures::pin_mut!(seam_stream);
        while let Some(batch) = seam_stream.next().await {
            for (k, v) in batch.unwrap() {
                via_seam.insert(k.to_vec(), v);
            }
        }

        // Collect a direct main stream into a map.
        let mut via_main: BTreeMap<Vec<u8>, Bytes> = BTreeMap::new();
        let main_stream = mvcc.main_store().iter_stream(64);
        futures::pin_mut!(main_stream);
        while let Some(batch) = main_stream.next().await {
            for (k, v) in batch.unwrap() {
                via_main.insert(k.to_vec(), v);
            }
        }

        // The two maps must be identical — diverges if current_stream ever
        // reads from anywhere other than `main`.
        assert_eq!(
            via_seam, via_main,
            "current_stream must equal main.iter_stream"
        );

        // Non-vacuous: exactly the 3 written current values are present.
        assert_eq!(
            via_seam.len(),
            3,
            "exactly the 3 written keys must be current"
        );
        for (k, v) in pairs {
            assert_eq!(
                via_seam.get(k.as_ref()),
                Some(v),
                "missing current value for {k:?}"
            );
        }
    }

    #[tokio::test]
    async fn zero_overhead_no_snapshots() {
        let mvcc = make_mvcc();
        for i in 0..100u32 {
            let key = Bytes::copy_from_slice(&i.to_be_bytes());
            mvcc.set_versioned(key, Bytes::from("val")).await.unwrap();
        }

        // C1: every write puts the current version into the log, so 100
        // version-key entries exist (one per key).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            for (hk, _) in batch.unwrap() {
                if crate::version_codec::decode_version_key(&hk).is_some() {
                    count += 1;
                }
            }
        }
        assert_eq!(
            count, 100,
            "C1: every write puts current into the log (100 version-key entries)"
        );
    }

    #[tokio::test]
    async fn version_cache_populated_on_set() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let cached = mvcc.cells.read(&key, |_, c| c.version);
        assert!(
            cached.is_some(),
            "version_cache should contain key after set"
        );
        assert!(cached.unwrap() > 0, "version should be > 0");
    }

    #[tokio::test]
    async fn main_store_accessor() {
        let main: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();

        let mvcc = MvccStore::new(Arc::clone(&main), Arc::clone(&history), gate);

        // Verify that main_store() returns the same Arc
        assert!(Arc::ptr_eq(&main, mvcc.main_store()));
        assert!(Arc::ptr_eq(&history, mvcc.history_store()));
    }

    #[tokio::test]
    async fn version_of_returns_zero_for_unknown_key() {
        let mvcc = make_mvcc();
        let v = mvcc.version_of(b"never_written");
        assert_eq!(v, 0);
    }

    #[tokio::test]
    async fn version_of_returns_cached_version_after_versioned_set() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("kx");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v = mvcc.version_of(&key);
        assert!(v > 0, "version_of must reflect the assigned version");
    }

    #[tokio::test]
    async fn apply_committed_ops_updates_version_cache() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_commit");
        let ops = vec![KvOp::Set(key.clone(), Bytes::from("val"))];
        mvcc.apply_committed_ops(ops, 42).await.unwrap();

        assert_eq!(mvcc.version_of(&key), 42);

        let val = mvcc.main.get(key).await.unwrap();
        assert_eq!(val, Bytes::from("val"));
    }

    #[tokio::test]
    async fn apply_committed_ops_archives_old_value() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_archive");
        mvcc.main
            .set(key.clone(), Bytes::from("old"))
            .await
            .unwrap();

        let ops = vec![KvOp::Set(key.clone(), Bytes::from("new"))];
        mvcc.apply_committed_ops(ops, 10).await.unwrap();

        assert_eq!(
            mvcc.main.get(key.clone()).await.unwrap(),
            Bytes::from("new")
        );

        // C1: the log contains the new value at the commit version (current-into-log).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                    if orig == b"k_archive" && ver == 10 {
                        assert_eq!(hv, Bytes::from("new"));
                        found = true;
                    }
                }
            }
        }
        assert!(found, "C1: new value at commit version in the log");
    }

    #[tokio::test]
    async fn apply_committed_ops_remove_archives_and_deletes() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_del");
        mvcc.main
            .set(key.clone(), Bytes::from("before"))
            .await
            .unwrap();

        let ops = vec![KvOp::Remove(key.clone())];
        mvcc.apply_committed_ops(ops, 20).await.unwrap();

        assert!(mvcc.main.get(key.clone()).await.is_err());

        // C1: Remove writes a tombstone (empty value) into the log at commit version.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                    if orig == b"k_del" && ver == 20 {
                        assert_eq!(hv, Bytes::new(), "C1: tombstone for remove");
                        found = true;
                    }
                }
            }
        }
        assert!(found, "C1: remove tombstone in the log");
    }

    #[tokio::test]
    async fn get_at_busy_history_five_versions() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("busy");
        let mut version_at = Vec::new();

        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
            let v = mvcc.version_of(&key);
            version_at.push(v);
        }

        // Query at each historical version and verify we get the right value.
        // version_at[0] was assigned when v1 was written, so get_at(version_at[0]) → v1
        for (idx, &snap) in version_at.iter().enumerate() {
            let result = mvcc.get_at(key.as_ref(), snap).await.unwrap();
            let expected = format!("v{}", idx + 1);
            assert_eq!(
                result,
                Some(Bytes::from(expected.clone())),
                "at snapshot {} expected {}",
                snap,
                expected
            );
        }

        // Query at version 0 (before any write) → slow path → scan [key::0, key::0] → empty → None
        let result_before = mvcc.get_at(key.as_ref(), 0).await.unwrap();
        assert!(
            result_before.is_none(),
            "no value should exist before first write"
        );

        // Query at a very high version → fast path → current (v5)
        let result_latest = mvcc.get_at(key.as_ref(), u64::MAX - 1).await.unwrap();
        assert_eq!(result_latest, Some(Bytes::from("v5")));
    }

    async fn count_history_entries(mvcc: &MvccStore) -> usize {
        // T1c: count only version-keys (decode_version_key succeeds). ts-keys
        // ([TS_TAG][version_be]) are skipped — decode returns None for them.
        let stream = mvcc.history_store().iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0;
        while let Some(batch) = stream.next().await {
            for (phys_key, _val) in batch.unwrap() {
                if crate::version_codec::decode_version_key(&phys_key).is_some() {
                    count += 1;
                }
            }
        }
        count
    }

    #[tokio::test]
    async fn gc_below_removes_old_versions_keeps_anchor() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("gc_test");
        // Write 5 versions
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // C1: history has 5 entries (v1..v4 from when they were current,
        // plus v5 which IS the current version written into the log).
        let count_before = count_history_entries(&mvcc).await;
        assert_eq!(count_before, 5, "C1: 5 entries (4 old + current v5)");

        // GC below version 3: versions < 3 in history are v1 and v2.
        // Anchor = highest < 3 = v2, older one (v1) deleted.
        let deleted = mvcc.gc_below(3).await.unwrap();
        assert!(deleted >= 1, "should delete at least 1 old entry");

        let count_after = count_history_entries(&mvcc).await;
        assert!(count_after < count_before, "history should shrink");
    }

    #[tokio::test]
    async fn gc_below_zero_deletes_nothing() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("gc_noop");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        let deleted = mvcc.gc_below(0).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn gc_convenience_uses_min_alive() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // No snapshots open → min_alive = last_committed = 0
        // Write some data without snapshot (no history archived)
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v"))
            .await
            .unwrap();

        let deleted = mvcc.gc().await.unwrap();
        assert_eq!(deleted, 0, "nothing to GC when no history");
    }

    #[tokio::test]
    async fn apply_committed_ops_no_snapshots_skips_history() {
        let mvcc = make_mvcc();

        let key = Bytes::from("k_nohist");
        mvcc.main
            .set(key.clone(), Bytes::from("old"))
            .await
            .unwrap();

        let ops = vec![KvOp::Set(key.clone(), Bytes::from("new"))];
        mvcc.apply_committed_ops(ops, 5).await.unwrap();

        assert_eq!(
            mvcc.main.get(key.clone()).await.unwrap(),
            Bytes::from("new")
        );

        assert_eq!(mvcc.version_of(&key), 5);

        // C1: every committed op now writes into the log unconditionally
        // (no longer gated by active_snapshots_empty). So 1 version-key entry.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            for (hk, _) in batch.unwrap() {
                if crate::version_codec::decode_version_key(&hk).is_some() {
                    count += 1;
                }
            }
        }
        assert_eq!(
            count, 1,
            "C1: apply_committed_ops writes current into the log (1 version-key entry)"
        );
    }

    /// CRIT-2 regression test. Before the fix, `version_cache.entry()
    /// .insert_entry()` was a no-op when the key was already cached,
    /// so the second `apply_committed_ops(..., 200)` left the cached
    /// version stuck at 100 and SSI conflict detection silently
    /// failed.
    #[tokio::test]
    async fn version_cache_updates_on_repeated_writes_to_same_key() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("repeated");

        mvcc.apply_committed_ops(vec![KvOp::Set(key.clone(), Bytes::from("v1"))], 100)
            .await
            .unwrap();
        assert_eq!(mvcc.version_of(&key), 100);

        mvcc.apply_committed_ops(vec![KvOp::Set(key.clone(), Bytes::from("v2"))], 200)
            .await
            .unwrap();
        // CRITICAL: must be 200, was 100 before the fix.
        assert_eq!(
            mvcc.version_of(&key),
            200,
            "version_cache must update on repeated writes"
        );
    }

    /// HIGH-2 regression guard. Before the fix, `snapshots_active`
    /// was sampled once at function entry; a snapshot that opened
    /// after the sample but before the first op would silently miss
    /// the archive. Per-op re-sampling closes that race for every
    /// op processed after the snapshot becomes visible.
    ///
    /// C1: the log is now the universal version timeline — every
    /// committed op writes into the log unconditionally (no longer
    /// gated by active_snapshots_empty). The new value at the commit
    /// version is always present in the log.
    #[tokio::test]
    async fn apply_committed_ops_archives_even_if_snapshot_opens_mid_call() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // No snapshots open yet — seed an old value through the raw
        // main store so apply will see it for archival.
        mvcc.main
            .set(Bytes::from("k"), Bytes::from("old"))
            .await
            .unwrap();

        // Open a snapshot right before apply (simulates "opened just
        // before, but after the flag would have been sampled at
        // function entry").
        let _g = gate.open_snapshot().await;

        mvcc.apply_committed_ops(vec![KvOp::Set(Bytes::from("k"), Bytes::from("new"))], 50)
            .await
            .unwrap();

        // C1: the new value is written into the log at version 50
        // unconditionally (the log is the universal timeline).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                if let Some((orig, ver)) = crate::version_codec::decode_version_key(&hk) {
                    if orig == b"k" && ver == 50 {
                        assert_eq!(hv, Bytes::from("new"));
                        found = true;
                    }
                }
            }
        }
        assert!(found, "C1: new value at commit version in the log");
    }

    // ----------------------------------------------------------------
    // III.2 — alloc-free `current_version` borrow-probe.
    // ----------------------------------------------------------------

    /// The borrow-based probe (`read(key: &[u8], ..)`) must locate entries
    /// inserted under arbitrary-length `Bytes` keys — not just the 16-byte
    /// RecordId case — confirming `[u8]: Equivalent<Bytes>` resolves and the
    /// hashes line up for any key length (incl. SSI keys that aren't 16 bytes).
    #[tokio::test]
    async fn version_of_borrow_probe_matches_arbitrary_length_keys() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        // Short (3-byte), long (40-byte), and empty keys.
        let short = Bytes::from_static(b"abc");
        let long = Bytes::from(vec![7u8; 40]);
        let empty = Bytes::new();

        mvcc.set_versioned(short.clone(), Bytes::from("s"))
            .await
            .unwrap();
        mvcc.set_versioned(long.clone(), Bytes::from("l"))
            .await
            .unwrap();
        mvcc.set_versioned(empty.clone(), Bytes::from("e"))
            .await
            .unwrap();

        // `version_of` takes `&[u8]`; the probe must find each entry.
        assert!(
            mvcc.version_of(short.as_ref()) > 0,
            "3-byte key must be found via borrow-probe"
        );
        assert!(
            mvcc.version_of(long.as_ref()) > 0,
            "40-byte key must be found via borrow-probe"
        );
        assert!(
            mvcc.version_of(empty.as_ref()) > 0,
            "empty key must be found via borrow-probe"
        );
        // A never-written key still returns 0.
        assert_eq!(mvcc.version_of(b"missing"), 0);
    }

    // ----------------------------------------------------------------
    // III.3 — GC prunes stale version_cache entries.
    // ----------------------------------------------------------------

    /// Core eviction: keys whose cached version is `< min_alive` are dropped
    /// (so `version_of` returns 0) while the current value in `main` — and
    /// therefore `get_at` — stays correct. A key at/above `min_alive` is
    /// NOT evicted.
    #[tokio::test]
    async fn gc_evicts_stale_version_cache_entries() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // Keep one snapshot open the whole time so writes go through the
        // versioned (history-archiving) path and populate version_cache.
        let _guard = gate.open_snapshot().await;

        // Two "old" keys, written early (low versions).
        let old_a = Bytes::from("old_a");
        let old_b = Bytes::from("old_b");
        mvcc.set_versioned(old_a.clone(), Bytes::from("a_val"))
            .await
            .unwrap();
        mvcc.set_versioned(old_b.clone(), Bytes::from("b_val"))
            .await
            .unwrap();
        let v_old_a = mvcc.version_of(&old_a);
        let v_old_b = mvcc.version_of(&old_b);
        assert!(v_old_a > 0 && v_old_b > 0);

        // A "fresh" key written at a high version.
        let fresh = Bytes::from("fresh");
        // Advance the version counter so `fresh` lands well above the olds.
        for _ in 0..10 {
            gate.assign_next_version();
        }
        mvcc.set_versioned(fresh.clone(), Bytes::from("f_val"))
            .await
            .unwrap();
        let v_fresh = mvcc.version_of(&fresh);
        assert!(v_fresh > v_old_a && v_fresh > v_old_b);

        // Cache currently holds all three.
        assert_eq!(mvcc.cells.len(), 3);

        // Advance min_alive to sit strictly above the old keys but at/below
        // the fresh key. With the snapshot still open at version 0, min_alive
        // would be 0 — so drop it first, then publish a committed marker so
        // min_alive == last_committed.
        let min_alive_target = v_old_b + 1; // > both olds, <= v_fresh
        assert!(min_alive_target <= v_fresh);
        drop(_guard); // no live snapshots → min_alive == last_committed
        gate.publish_committed(min_alive_target);
        assert_eq!(gate.min_alive(), min_alive_target);

        // GC. The history threshold here is irrelevant to cache pruning,
        // which always uses min_alive.
        mvcc.gc().await.unwrap();

        // Old keys (cv < min_alive) evicted → version_of == 0.
        assert_eq!(
            mvcc.version_of(&old_a),
            0,
            "stale old_a should be evicted from version_cache"
        );
        assert_eq!(
            mvcc.version_of(&old_b),
            0,
            "stale old_b should be evicted from version_cache"
        );
        // Fresh key (cv >= min_alive) retained.
        assert_eq!(
            mvcc.version_of(&fresh),
            v_fresh,
            "fresh key (cv >= min_alive) must NOT be evicted"
        );
        assert_eq!(mvcc.cells.len(), 1, "cache should have shrunk to 1");

        // CURRENT values in main are still correct after eviction. With the
        // entries gone, get_at takes the fast path and reads main.
        let snap = min_alive_target + 100;
        assert_eq!(
            mvcc.get_at(old_a.as_ref(), snap).await.unwrap(),
            Some(Bytes::from("a_val")),
            "evicting the cache entry must not change the current value"
        );
        assert_eq!(
            mvcc.get_at(old_b.as_ref(), snap).await.unwrap(),
            Some(Bytes::from("b_val"))
        );
        assert_eq!(
            mvcc.get_at(fresh.as_ref(), snap).await.unwrap(),
            Some(Bytes::from("f_val"))
        );
    }

    /// Boundary: a key whose cached version equals `min_alive` is retained
    /// (the rule is strict `<`, not `<=`).
    #[tokio::test]
    async fn gc_keeps_version_cache_entry_at_min_alive_boundary() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let guard = gate.open_snapshot().await;

        let key = Bytes::from("boundary");
        mvcc.set_versioned(key.clone(), Bytes::from("v"))
            .await
            .unwrap();
        let cv = mvcc.version_of(&key);
        assert!(cv > 0);

        // Set min_alive EXACTLY to cv: entry must survive (cv >= min_alive).
        drop(guard);
        gate.publish_committed(cv);
        assert_eq!(gate.min_alive(), cv);

        mvcc.gc().await.unwrap();

        assert_eq!(
            mvcc.version_of(&key),
            cv,
            "entry at the min_alive boundary must be retained (strict < eviction)"
        );
    }

    /// The dangerous case the invariant protects: an entry is NOT evicted
    /// while a live snapshot below its version still needs the history value.
    /// We open a snapshot between v1 and v2, advance last_committed, run GC,
    /// and confirm (a) the entry survives (a live snapshot is older than its
    /// version), and (b) `get_at` at that old snapshot STILL returns the
    /// archived v1 from history — the MVCC visibility contract holds.
    #[tokio::test]
    async fn gc_preserves_visibility_for_live_snapshot_below_cached_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // Bootstrap snapshot at v0 — activates the versioned write-path for
        // the v1 write (so version_cache gets populated). Dropped once the
        // real reader is open, so it does not drag min_alive below v1.
        let bootstrap = gate.open_snapshot().await;

        let key = Bytes::from("vis");

        // v1 committed.
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);

        // Publish v1 so a snapshot can open AT v1, then open it: this snapshot
        // is the live reader that still needs v1 after v2 lands.
        gate.publish_committed(v1);
        let reader_snap = gate.open_snapshot().await;
        assert_eq!(reader_snap.version(), v1);
        // Now drop the v0 bootstrap so the reader at v1 is the oldest snapshot.
        drop(bootstrap);

        // v2 overwrites; v1 is archived to history (the reader snapshot at v1
        // is still active).
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();
        let v2 = mvcc.version_of(&key);
        assert!(v2 > v1);

        // Advance last_committed past v2. But the live reader snapshot pins
        // min_alive down to v1, so the cache entry (cv == v2) is NOT < v1
        // and must NOT be evicted.
        gate.publish_committed(v2 + 50);
        assert_eq!(
            gate.min_alive(),
            v1,
            "live reader snapshot must pin min_alive to v1"
        );

        mvcc.gc().await.unwrap();

        // Entry retained (cv == v2 >= min_alive == v1).
        assert_eq!(
            mvcc.version_of(&key),
            v2,
            "entry needed by a live snapshot below its version must survive GC"
        );

        // Visibility contract: the live snapshot at v1 still sees v1 (slow
        // path, because cur_v (v2) > snapshot (v1) → scan history).
        assert_eq!(
            mvcc.get_at(key.as_ref(), reader_snap.version())
                .await
                .unwrap(),
            Some(Bytes::from("v1")),
            "snapshot below the cached version must still read the archived v1"
        );

        // And a snapshot at/after v2 sees v2 (fast path → main).
        assert_eq!(
            mvcc.get_at(key.as_ref(), v2).await.unwrap(),
            Some(Bytes::from("v2"))
        );
    }

    /// Even after an entry is legitimately evicted, a (hypothetical) read at
    /// an OLD snapshot below the real version is still answered correctly,
    /// BECAUSE eviction only ever happens once no such live snapshot exists.
    /// This test exercises the "evict, then read at the boundary snapshot"
    /// path to prove the fast-path fallback returns main's current value
    /// (the post-eviction contract) rather than a stale/incorrect one.
    #[tokio::test]
    async fn gc_evicted_key_read_at_boundary_snapshot_returns_main() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let guard = gate.open_snapshot().await;

        let key = Bytes::from("evict_then_read");
        // Two versions: v1 archived, v2 in main.
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();
        let v2 = mvcc.version_of(&key);

        // Move min_alive strictly above v2 (no live snapshots), making the
        // entry redundant: every surviving/ future snapshot is >= min_alive
        // > v2, so none can observe v1.
        drop(guard);
        gate.publish_committed(v2 + 1);
        assert_eq!(gate.min_alive(), v2 + 1);

        mvcc.gc().await.unwrap();
        assert_eq!(mvcc.version_of(&key), 0, "redundant entry evicted");

        // A read at exactly min_alive (the lowest a fresh snapshot could be)
        // takes the fast path and returns main's current value v2 — correct,
        // since no snapshot this old or older can legitimately exist below v2.
        assert_eq!(
            mvcc.get_at(key.as_ref(), v2 + 1).await.unwrap(),
            Some(Bytes::from("v2")),
            "post-eviction read returns current main value via fast path"
        );
    }

    // ----------------------------------------------------------------
    // III.4 — `set_versioned_many` batched bulk write.
    // ----------------------------------------------------------------

    /// T1a: always-archive — `set_versioned_many` always runs the slow
    /// batch path (archive → main → per-key publish). The no-snapshot fast
    /// path is gone. For a batch of brand-new keys there is no prior to
    /// archive, so history stays empty; every key still gets its own cell
    /// entry carrying its assigned version (one version per record).
    ///
    /// T1b.1: uses `KeepHistory` so the eager vacuum does not prune the
    /// cells before the assertions (this test checks cell-population, not
    /// vacuum behaviour).
    #[tokio::test]
    async fn set_versioned_many_batches_no_snapshot() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        let n = 50u32;
        let items: Vec<(Bytes, Bytes)> = (0..n)
            .map(|i| {
                (
                    Bytes::copy_from_slice(&i.to_be_bytes()),
                    Bytes::from(format!("val{i}")),
                )
            })
            .collect();
        mvcc.set_versioned_many(items).await.unwrap();

        // Every record is present in main with the right value.
        for i in 0..n {
            let k = Bytes::copy_from_slice(&i.to_be_bytes());
            assert_eq!(
                mvcc.main.get(k.clone()).await.unwrap(),
                Bytes::from(format!("val{i}"))
            );
        }

        // C1: every write puts the current version into the log, so n version-key
        // entries exist (one per key, dual-write).
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut hist = 0usize;
        while let Some(batch) = stream.next().await {
            for (hk, _) in batch.unwrap() {
                if crate::version_codec::decode_version_key(&hk).is_some() {
                    hist += 1;
                }
            }
        }
        assert_eq!(
            hist, n as usize,
            "C1: every key gets its current version in the log (dual-write)"
        );

        // T1a: every key is published into the cells (one entry per key).
        assert_eq!(
            mvcc.cells.len(),
            n as usize,
            "T1a always-archive: every batch key gets a cell entry"
        );
    }

    /// Snapshot-active path: an existing old value is archived to history,
    /// the new values land in main, and every key gets a fresh monotonic
    /// version in the cache (one version per record, like the per-record
    /// `set_versioned` loop it replaces).
    #[tokio::test]
    async fn set_versioned_many_with_snapshot_archives_and_versions() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        // Seed an existing value for one of the keys (will be archived).
        let k0 = Bytes::from("k0");
        mvcc.main
            .set(k0.clone(), Bytes::from("old0"))
            .await
            .unwrap();

        let items: Vec<(Bytes, Bytes)> = vec![
            (k0.clone(), Bytes::from("new0")),
            (Bytes::from("k1"), Bytes::from("v1")),
            (Bytes::from("k2"), Bytes::from("v2")),
        ];
        mvcc.set_versioned_many(items).await.unwrap();

        // All news present in main.
        assert_eq!(
            mvcc.main.get(k0.clone()).await.unwrap(),
            Bytes::from("new0")
        );
        assert_eq!(
            mvcc.main.get(Bytes::from("k1")).await.unwrap(),
            Bytes::from("v1")
        );
        assert_eq!(
            mvcc.main.get(Bytes::from("k2")).await.unwrap(),
            Bytes::from("v2")
        );

        // C1: the log contains the new values at their assigned versions
        // (current-into-log). old0 was seeded directly into main, not through
        // the log, so it does NOT appear in history.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found_new0 = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                if let Some((orig, _ver)) = crate::version_codec::decode_version_key(&hk) {
                    if orig == b"k0" && hv.as_ref() == b"new0" {
                        found_new0 = true;
                    }
                }
            }
        }
        assert!(
            found_new0,
            "C1: k0's new value is in the log (current-into-log)"
        );

        // Every key got a positive version in the cache.
        assert!(mvcc.version_of(&k0) > 0);
        assert!(mvcc.version_of(b"k1") > 0);
        assert!(mvcc.version_of(b"k2") > 0);
        assert_eq!(mvcc.cells.len(), 3);
    }

    /// Empty input is a no-op (no panic, no writes).
    #[tokio::test]
    async fn set_versioned_many_empty_is_noop() {
        let mvcc = make_mvcc();
        mvcc.set_versioned_many(Vec::new()).await.unwrap();
        assert_eq!(mvcc.cells.len(), 0);
    }

    /// Regression guard for the NotFound-vs-IO-error conflation bug.
    /// Before the fix, `if let Ok(old) = self.main.get(key)` treated a
    /// genuine I/O error identically to a missing key — skipping
    /// archival but still overwriting main, breaking snapshot isolation.
    ///
    /// This test verifies the *correct* NotFound path: writing a brand-
    /// new key under an active snapshot succeeds (nothing to archive)
    /// and the key is readable.
    #[tokio::test]
    async fn set_versioned_new_key_under_snapshot_succeeds() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("brand_new_key");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        // Key is readable from main.
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v1"));

        // C1: current version is in the log (dual-write), so 1 entry.
        let count = count_history_entries(&mvcc).await;
        assert_eq!(
            count, 1,
            "C1: current version in the log for a brand-new key"
        );
    }

    // ================================================================
    // Fault-injecting Store double for I/O-error propagation tests.
    // ================================================================

    mod failing_store {
        use async_trait::async_trait;
        use bytes::Bytes;
        use shamir_storage::error::{DbError, DbResult};
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::{KvOp, RecordKey, Store};
        use std::pin::Pin;
        use std::sync::atomic::{AtomicBool, Ordering};

        use futures::stream::Stream;

        /// A test double that wraps `InMemoryStore` and can be armed to
        /// inject I/O errors on `get` and/or `remove` calls. Used to
        /// regression-test that `MvccStore` propagates non-NotFound
        /// errors rather than swallowing them.
        pub(super) struct FailingStore {
            inner: InMemoryStore,
            /// When `true`, the next `get` call returns a Storage error.
            pub fail_get: AtomicBool,
            /// When `true`, the next `remove` call returns a Storage error.
            pub fail_remove: AtomicBool,
            /// When `true`, the next `set` call returns a Storage error.
            pub fail_set: AtomicBool,
        }

        impl FailingStore {
            pub fn new() -> Self {
                Self {
                    inner: InMemoryStore::new(),
                    fail_get: AtomicBool::new(false),
                    fail_remove: AtomicBool::new(false),
                    fail_set: AtomicBool::new(false),
                }
            }

            fn injected_error() -> DbError {
                DbError::Storage("injected I/O fault".into())
            }
        }

        #[async_trait]
        impl Store for FailingStore {
            async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
                self.inner.insert(value).await
            }

            async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
                if self.fail_set.load(Ordering::Relaxed) {
                    return Err(Self::injected_error());
                }
                self.inner.set(key, value).await
            }

            async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
                if self.fail_get.load(Ordering::Relaxed) {
                    return Err(Self::injected_error());
                }
                self.inner.get(key).await
            }

            async fn remove(&self, key: RecordKey) -> DbResult<bool> {
                if self.fail_remove.load(Ordering::Relaxed) {
                    return Err(Self::injected_error());
                }
                self.inner.remove(key).await
            }

            fn iter_stream(
                &self,
                batch_size: usize,
            ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
            {
                self.inner.iter_stream(batch_size)
            }

            fn scan_prefix_stream(
                &self,
                prefix: Bytes,
                batch_size: usize,
            ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
            {
                self.inner.scan_prefix_stream(prefix, batch_size)
            }

            async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
                // Honour per-op fault flags so batched paths
                // (set_versioned_many, apply_committed_ops) also hit the
                // injection when they call `self.main.get(...)` pre-read.
                for op in ops {
                    match op {
                        KvOp::Set(k, v) => {
                            let _ = self.set(k, v).await?;
                        }
                        KvOp::Remove(k) => {
                            let _ = self.remove(k).await?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    // ================================================================
    // Regression tests — I/O error propagation (fault injection).
    // ================================================================

    use failing_store::FailingStore;
    use std::sync::atomic::Ordering;

    /// Helper: build an MvccStore whose `main` is a FailingStore.
    fn make_failing_mvcc(gate: Arc<RepoTxGate>) -> (MvccStore, Arc<FailingStore>) {
        let main = Arc::new(FailingStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let mvcc = MvccStore::new(main.clone() as Arc<dyn Store>, history, gate);
        (mvcc, main)
    }

    /// Regression test for fix #1: `delete_versioned` propagates
    /// `main.remove()` errors.
    ///
    /// **Pre-fix behaviour:** `let _ = self.main.remove(key).await;`
    /// discarded the error → caller saw `Ok(())` even though the row
    /// was still live in main (silent data retention).
    ///
    /// **Post-fix:** `self.main.remove(key).await?;` propagates the
    /// error → caller sees `Err`.
    ///
    /// This test would FAIL on the pre-fix code because the old
    /// `let _ = ...` swallowed the injected `Err(Storage(...))` and
    /// returned `Ok(())`.
    #[tokio::test]
    async fn delete_versioned_propagates_remove_error() {
        let gate = make_gate();
        let (mvcc, main) = make_failing_mvcc(gate.clone());

        // Seed a key so delete has something to target.
        main.set(Bytes::from("k"), Bytes::from("val"))
            .await
            .unwrap();

        // Open a snapshot so the active-snapshot path runs (archive +
        // remove + version_cache update).
        let _guard = gate.open_snapshot().await;

        // Arm: the next `remove` call will fail.
        main.fail_remove.store(true, Ordering::Relaxed);

        let result = mvcc.delete_versioned(Bytes::from("k")).await;
        assert!(
            result.is_err(),
            "delete_versioned must propagate main.remove() I/O error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("injected"),
            "error should be the injected fault, got: {err_msg}"
        );
    }

    /// C1 regression: `set_versioned` propagates `main.set()` errors.
    ///
    /// With C1, `set_versioned` writes current-into-log (history) then
    /// `main.set()`. If `main.set()` fails, the error propagates and
    /// main is NOT overwritten (the old value survives).
    #[tokio::test]
    async fn set_versioned_propagates_archive_read_error() {
        let gate = make_gate();
        let (mvcc, main) = make_failing_mvcc(gate.clone());

        // Seed a key in main.
        main.set(Bytes::from("k"), Bytes::from("old_val"))
            .await
            .unwrap();

        // Arm: the next `set` call on main will fail.
        main.fail_set.store(true, Ordering::Relaxed);

        let result = mvcc
            .set_versioned(Bytes::from("k"), Bytes::from("new_val"))
            .await;
        assert!(
            result.is_err(),
            "set_versioned must propagate main.set() I/O error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("injected"),
            "error should be the injected fault, got: {err_msg}"
        );

        // Verify main was NOT overwritten — the old value survives.
        main.fail_set.store(false, Ordering::Relaxed);
        let val = mvcc.main.get(Bytes::from("k")).await.unwrap();
        assert_eq!(
            val,
            Bytes::from("old_val"),
            "main must NOT be overwritten when main.set() fails"
        );
    }

    /// C1 regression: `delete_versioned` propagates `main.remove()` errors.
    ///
    /// With C1, `delete_versioned` writes a tombstone to history then
    /// `main.remove()`. If `main.remove()` fails, the error propagates
    /// and the key survives in main.
    #[tokio::test]
    async fn delete_versioned_propagates_archive_read_error() {
        let gate = make_gate();
        let (mvcc, main) = make_failing_mvcc(gate.clone());

        // Seed a key.
        main.set(Bytes::from("k"), Bytes::from("val"))
            .await
            .unwrap();

        // Arm: `remove` fails.
        main.fail_remove.store(true, Ordering::Relaxed);

        let result = mvcc.delete_versioned(Bytes::from("k")).await;
        assert!(
            result.is_err(),
            "delete_versioned must propagate main.remove() I/O error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("injected"),
            "error should be the injected fault, got: {err_msg}"
        );

        // Main must still have the key (remove was never called).
        main.fail_remove.store(false, Ordering::Relaxed);
        let val = mvcc.main.get(Bytes::from("k")).await.unwrap();
        assert_eq!(
            val,
            Bytes::from("val"),
            "key must survive when archive pre-read fails"
        );
    }

    // ================================================================
    // MVCC-2 VERIFICATION — set_versioned fast-path TOCTOU
    // ================================================================
    //
    // CLAIM: Between `active_snapshots_empty()` (line 67) and `main.set()`
    // (line 68), another task opens a snapshot. The non-tx write then lands
    // in `main` WITHOUT being archived to `history` and WITHOUT updating
    // `version_cache`. A snapshot that opens in this narrow window and then
    // calls `get_at(key, snap)` will:
    //   - Find `version_cache` returns 0 (key not cached).
    //   - Take the fast path: `0 <= snap` → read `main`.
    //   - See the FRESHLY WRITTEN value even though the snapshot predates it.
    //
    // ANALYSIS: Whether this is observable in practice depends on whether
    // tokio's single-threaded executor can interleave async tasks between
    // `active_snapshots_empty()` and `main.set()`. In practice:
    //
    //   a) `active_snapshots_empty()` is a synchronous atomic read.
    //   b) `main.set()` on InMemoryStore is also synchronous.
    //   c) There is NO `.await` between the two — they run in the same
    //      poll() call, making interleaving IMPOSSIBLE in a single-threaded
    //      tokio executor.
    //
    // Therefore the TOCTOU window is REAL in principle (code is non-atomic)
    // but NOT EXPLOITABLE with InMemoryStore in a single-threaded runtime.
    // A multi-threaded executor with blocking I/O between the check and the
    // write could expose it.
    //
    // The tests below verify:
    //   1. A deterministic sequential test confirms the gap is unobservable
    //      with InMemoryStore (no preemption between the two lines).
    //   2. A concurrent stress test runs 1000 iterations trying to create
    //      the race; verifies it cannot be triggered with the current runtime.
    //   3. A code-path analysis test documents what WOULD happen if a snapshot
    //      opened between the check and the write (by manually simulating the
    //      split: check → open snapshot → set → read).

    /// MVCC-2 deterministic: sequential check → set → open_snapshot → read.
    ///
    /// Verifies that when the snapshot is opened AFTER `set_versioned`
    /// completes (the normal sequential case), `get_at` correctly returns
    /// the value visible at the snapshot version.
    ///
    /// T1a: fast path removed (always-archive) — MVCC-2 cannot occur; this
    /// now asserts snapshot-correct visibility (flipped from the pre-fix
    /// characterization). `version_of` is now populated on every write
    /// (always-slow path), so a snapshot opened after the write at
    /// `snap_v == v` routes correctly.
    #[tokio::test]
    async fn mvcc2_fast_path_version_cache_not_updated() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let key = Bytes::from("toctou_key");

        // Write with no snapshot active — always-archive path now runs.
        let v = mvcc
            .set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        assert!(v > 0);

        // T1a: always-archive populates the cell with the assigned version.
        assert_eq!(
            mvcc.version_of(&key),
            v,
            "T1a always-archive: every write publishes its version into the cell"
        );

        // Now publish and open a snapshot.
        gate.publish_committed(v);
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, v, "snapshot must open at the published version");

        // get_at: cur_v=v ≤ snap_v=v → fast path → reads main → sees v1.
        // This is correct: the snapshot was opened AFTER the write landed.
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v1")),
            "snapshot opened after the write sees v1 via fast path (correct)"
        );
    }

    /// MVCC-2 simulated TOCTOU — now asserts the fix.
    ///
    /// T1a: fast path removed (always-archive) — MVCC-2 cannot occur; this
    /// now asserts snapshot-correct visibility (flipped from the pre-fix
    /// characterization). Previously this test manually wrote to `main`
    /// bypassing the slow path to demonstrate the phantom read. With the
    /// fast path gone, the real `set_versioned` always archives, so we
    /// drive the write through it and confirm a snapshot opened BEFORE
    /// the write does NOT see the post-snapshot value — it sees the
    /// pre-write value (OLD), which was archived.
    ///
    /// Sequence:
    ///   1. Seed OLD via `set_versioned` (no snapshot) — archived on the
    ///      next overwrite.
    ///   2. Open a snapshot at `v_old`.
    ///   3. Overwrite with NEW via `set_versioned` — OLD is archived to
    ///      `history`, cell advances to `v_new`.
    ///   4. `get_at(key, v_old)`: `cur_v = v_new > v_old` → slow path →
    ///      scans history → finds OLD. Correct.
    #[tokio::test]
    async fn mvcc2_simulated_toctou_snapshot_sees_phantom() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let key = Bytes::from("toctou_key");

        // Step 1: seed OLD with no snapshot active. Always-archive path runs
        // but there is no prior value, so nothing is archived yet.
        let old_val = Bytes::from("OLD");
        let v_old = mvcc
            .set_versioned(key.clone(), old_val.clone())
            .await
            .unwrap();
        assert!(v_old > 0);
        gate.publish_committed(v_old);

        // Step 2: open a snapshot at v_old — from its perspective NEW has
        // not happened yet.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, v_old, "snapshot opens at the published v_old");

        // Step 3: overwrite with NEW via the REAL set_versioned (always-
        // archive). OLD is now archived to history; the cell advances.
        let new_val = Bytes::from("NEW");
        let v_new = mvcc
            .set_versioned(key.clone(), new_val.clone())
            .await
            .unwrap();
        assert!(v_new > v_old);
        assert_eq!(
            mvcc.version_of(&key),
            v_new,
            "T1a always-archive: cell carries the latest assigned version"
        );

        // Step 4: get_at for the snapshot.
        // cur_v = v_new > snap_v → SLOW path → scan history → finds OLD.
        // The snapshot does NOT see the phantom NEW; it sees the value that
        // was current at snap_v. MVCC-2 is closed by construction.
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(old_val),
            "T1a: snapshot opened before the overwrite sees OLD (archived), \
             not the post-snapshot NEW — MVCC-2 cannot occur"
        );
    }

    // ================================================================
    // PausableStore — test double for MVCC-2 deterministic harness.
    // ================================================================
    //
    // Wraps an inner Store and adds a pause point inside `set()`:
    // when `armed` is true the first `set()` call signals `entered`
    // (so the test knows it is inside the gap) and then blocks on
    // `pause_gate` until the test calls `release()`.  This lets the
    // test deterministically open a snapshot INSIDE the fast-path
    // window between `active_snapshots_empty()` and the actual write.
    //
    // All other Store methods are delegated to `inner`.
    mod pausable_store {
        use async_trait::async_trait;
        use bytes::Bytes;
        use futures::stream::Stream;
        use shamir_storage::error::{DbError, DbResult};
        use shamir_storage::storage_in_memory::InMemoryStore;
        use shamir_storage::types::{KvOp, RecordKey, Store};
        use std::pin::Pin;
        use std::sync::{
            atomic::{AtomicBool, Ordering::SeqCst},
            Arc,
        };
        use tokio::sync::Notify;

        pub struct PausableStore {
            pub inner: InMemoryStore,
            /// When `true`, the next `set()` call will pause.
            pub armed: Arc<AtomicBool>,
            /// Notified when `set()` has entered the pause point (before
            /// the actual write) — the test waits on this to know the
            /// write task is suspended in the window.
            pub entered: Arc<Notify>,
            /// The write task blocks here until the test calls `release()`.
            pub pause_gate: Arc<Notify>,
        }

        impl PausableStore {
            pub fn new() -> Self {
                Self {
                    inner: InMemoryStore::new(),
                    armed: Arc::new(AtomicBool::new(false)),
                    entered: Arc::new(Notify::new()),
                    pause_gate: Arc::new(Notify::new()),
                }
            }

            /// Arm: the next `set()` call will pause.
            pub fn arm(&self) {
                self.armed.store(true, SeqCst);
            }

            /// Release: unblock the paused `set()`.
            pub fn release(&self) {
                self.pause_gate.notify_one();
            }
        }

        #[async_trait]
        impl Store for PausableStore {
            async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
                if self.armed.swap(false, SeqCst) {
                    // Signal to the test that we have reached the pause point
                    // (BEFORE the actual write — i.e., we are inside the
                    // fast-path window between active_snapshots_empty() and
                    // main.set()).
                    self.entered.notify_one();
                    // Block until the test calls release().
                    self.pause_gate.notified().await;
                }
                self.inner.set(key, value).await
            }

            async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
                self.inner.insert(value).await
            }

            async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
                self.inner.get(key).await
            }

            async fn remove(&self, key: RecordKey) -> DbResult<bool> {
                self.inner.remove(key).await
            }

            async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
                for op in ops {
                    match op {
                        KvOp::Set(k, v) => {
                            let _ = self.set(k, v).await?;
                        }
                        KvOp::Remove(k) => {
                            let _ = self.remove(k).await?;
                        }
                    }
                }
                Ok(())
            }

            fn iter_stream(
                &self,
                batch_size: usize,
            ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
            {
                self.inner.iter_stream(batch_size)
            }

            fn scan_prefix_stream(
                &self,
                prefix: Bytes,
                batch_size: usize,
            ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>
            {
                self.inner.scan_prefix_stream(prefix, batch_size)
            }
        }
    }

    // ================================================================
    // MVCC-2 deterministic characterization via PausableStore.
    // ================================================================

    use pausable_store::PausableStore;

    /// MVCC-2 characterization via PausableStore — now asserts the fix.
    ///
    /// T1a: fast path removed (always-archive) — MVCC-2 cannot occur; this
    /// now asserts snapshot-correct visibility (flipped from the pre-fix
    /// characterization). `PausableStore` suspends `main.set()` — which is
    /// now AFTER `archive_prior` (OLD archived to history) and
    /// `publish_cell` (cell advanced to `v_new`). A snapshot opened inside
    /// this pause therefore sees `cur_v = v_new > snap_v` → slow path →
    /// scans history → finds OLD. The phantom read cannot occur.
    ///
    /// Sequence:
    ///   [set_versioned(NEW)] archive_prior → publish_cell(v_new) → main.set
    ///                                                  ↑ pause here
    ///   [snapshot opens at v_after_seed]
    ///   [release → main.set commits NEW]
    ///   [get_at(key, v_after_seed)] → cur_v=v_new > v_after_seed → history → OLD
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mvcc2_real_interleaving_toctou_characterization() {
        use std::sync::Arc;

        let key = Bytes::from("toctou_key");
        let old_val = Bytes::from("OLD");
        let new_val = Bytes::from("NEW");

        // --- Setup ---
        // Build the MvccStore with a PausableStore as `main`.
        let pausable = Arc::new(PausableStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();
        let mvcc = Arc::new(MvccStore::new(
            pausable.clone() as Arc<dyn Store>,
            history,
            gate.clone(),
        ));

        // Seed: write OLD with no snapshots open. T1a always-archive runs,
        // but there is no prior value, so nothing is archived; the cell IS
        // published with the seed version.
        mvcc.set_versioned(key.clone(), old_val.clone())
            .await
            .unwrap();
        let v_seed = mvcc.version_of(&key);
        assert!(v_seed > 0);
        // Publish so a snapshot can capture the current committed version.
        let v_after_seed = gate.assign_next_version();
        gate.publish_committed(v_after_seed);

        // T1a: the seed write published its version into the cell.
        assert_eq!(
            mvcc.version_of(&key),
            v_seed,
            "T1a always-archive: seed write publishes its version into the cell"
        );
        // Confirm no snapshots are open before arming.
        assert!(
            gate.active_snapshots_empty(),
            "precondition: no snapshots open before arming"
        );

        // --- Arm PausableStore ---
        // The next `set()` call on `main` will pause before writing.
        pausable.arm();

        // Clone refs for the write task.
        let mvcc_w = Arc::clone(&mvcc);
        let key_w = key.clone();
        let new_val_w = new_val.clone();

        // --- Spawn write task ---
        // This calls set_versioned(key, NEW). With T1a the sequence is:
        //   archive_prior (OLD → history) → publish_cell(v_new) → main.set (PAUSE).
        // So when `entered` fires, OLD is already archived and the cell
        // already carries v_new.
        let write_handle = tokio::spawn(async move {
            mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
        });

        // --- Wait for write to be inside the pause ---
        // `entered` fires inside `main.set`, which is AFTER archive_prior
        // and publish_cell. OLD is now in history; the cell holds v_new.
        pausable.entered.notified().await;

        // --- Open snapshot inside the window ---
        // snap_v = last_committed = v_after_seed (published above).
        // From this snapshot's perspective, the write of NEW has NOT
        // happened yet — it should see OLD.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(
            snap_v, v_after_seed,
            "snapshot must open at v_after_seed (the gap version)"
        );

        // T1a: by the time the snapshot opens, publish_cell has already
        // advanced the cell to v_new (the NEW write's version).
        let v_new = mvcc.version_of(&key);
        assert!(
            v_new > snap_v,
            "T1a: cell already carries v_new > snap_v (archive+publish ran before the pause)"
        );

        // --- Release: let the write commit to main ---
        pausable.release();
        write_handle.await.unwrap();

        // NEW is now in main. The cell still carries v_new (T1a always-slow).
        assert_eq!(
            mvcc.version_of(&key),
            v_new,
            "T1a always-archive: cell carries v_new after the write commits"
        );

        // --- The (now-correct) characterization moment ---
        // get_at(key, snap_v):
        //   cur_v = v_new > snap_v → SLOW PATH → scan history → finds OLD.
        //
        // OLD was archived by archive_prior BEFORE the pause, so it is in
        // history. The snapshot opened at v_after_seed (< v_new) correctly
        // sees OLD — the value that was current at snap_v. The phantom read
        // of NEW cannot occur: MVCC-2 is closed by construction.
        let seen = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            seen,
            Some(old_val),
            "T1a: snapshot opened inside the write window sees OLD (archived), \
             not the post-snapshot NEW — MVCC-2 cannot occur"
        );
    }

    // ================================================================
    // A1+A2 — live_version (hwm) tests.
    // =================================================================
    //
    // T1a (always-archive): the fast path is gone — every write runs the
    // slow path, so `version` and `hwm` are always equal (both carry the
    // assigned version). These tests now assert that equality directly
    // (flipped from the pre-fix fast-path/hwm distinction).

    /// T1a: always-archive — every write publishes its version into both
    /// cell fields, so `live_version` and `version_of` agree.
    #[tokio::test]
    async fn live_version_tracks_fast_path_write() {
        let mvcc = make_mvcc();
        let key = Bytes::from("k_hwm_fast");

        // T1a: always-slow path — no fast-path branch remains.
        let v = mvcc
            .set_versioned(key.clone(), Bytes::from("val"))
            .await
            .unwrap();

        // live_version returns the assigned hwm.
        assert_eq!(
            mvcc.live_version(&key),
            Some(v),
            "live_version must equal the assigned version"
        );
        // T1a: version_of (archive-routing) now equals v too (always-slow).
        assert_eq!(
            mvcc.version_of(&key),
            v,
            "T1a always-archive: version_of equals the assigned version (no fast-path split)"
        );
        // get_at at 0 (cur_v=v > 0 → slow path → history empty → None).
        let result = mvcc.get_at(&key, 0).await.unwrap();
        assert_eq!(
            result, None,
            "T1a: get_at(0) scans history (empty for a brand-new key) → None"
        );
    }

    /// Slow-path write sets both hwm and version.
    #[tokio::test]
    async fn live_version_tracks_slow_path_write() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k_hwm_slow");

        let v = mvcc
            .set_versioned(key.clone(), Bytes::from("val"))
            .await
            .unwrap();

        assert_eq!(
            mvcc.live_version(&key),
            Some(v),
            "live_version must equal the assigned version on slow path"
        );
        assert_eq!(
            mvcc.version_of(&key),
            v,
            "version_of must also equal v on slow path"
        );
    }

    /// live_version is None before any write touches a key.
    #[tokio::test]
    async fn live_version_absent_before_any_write() {
        let mvcc = make_mvcc();
        assert_eq!(
            mvcc.live_version(b"never"),
            None,
            "live_version must be None for a key never written"
        );
    }

    /// After a write followed by a delete, live_version equals the delete's version.
    #[tokio::test]
    async fn live_version_advances_on_delete() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k_hwm_del");

        let v1 = mvcc
            .set_versioned(key.clone(), Bytes::from("val"))
            .await
            .unwrap();
        let vd = mvcc.delete_versioned(key.clone()).await.unwrap();

        assert!(vd > v1, "delete version must be greater than write version");
        assert_eq!(
            mvcc.live_version(&key),
            Some(vd),
            "live_version must advance to the delete version"
        );
    }

    /// T1a: always-archive — `set_versioned_many` assigns one version per
    /// key (like the per-record `set_versioned` loop), so each key's
    /// `live_version` and `version_of` carry that key's own version.
    ///
    /// T1b.1: uses `KeepHistory` so the eager vacuum does not prune the
    /// cells before the assertions (this test checks cell-population, not
    /// vacuum behaviour).
    #[tokio::test]
    async fn set_versioned_many_sets_hwm_fast_path() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        let items: Vec<(Bytes, Bytes)> = vec![
            (Bytes::from("bk1"), Bytes::from("v1")),
            (Bytes::from("bk2"), Bytes::from("v2")),
            (Bytes::from("bk3"), Bytes::from("v3")),
        ];
        let max_v = mvcc.set_versioned_many(items.clone()).await.unwrap();
        assert!(max_v > 0);

        // T1a: every key gets its own monotonic version (one per record);
        // both live_version and version_of carry it.
        let mut prev = 0u64;
        for (key, _) in &items {
            let v = mvcc.live_version(key).expect("key present");
            assert!(v > prev, "T1a always-archive: per-key monotonic versions");
            assert_eq!(
                mvcc.version_of(key),
                v,
                "T1a always-archive: version_of == live_version (no fast-path split)"
            );
            prev = v;
        }
        assert_eq!(prev, max_v, "returned max_v is the last assigned version");
    }

    /// MVCC-2 stress: concurrent set_versioned + open_snapshot, 500 iterations.
    ///
    /// T1a: fast path removed (always-archive) — MVCC-2 cannot occur; this
    /// now asserts snapshot-correct visibility (flipped from the pre-fix
    /// characterization). With always-archive every write publishes its
    /// version into the cell, so the old anomaly predicate
    /// (`version_of == 0` after a write) can never hold — there is no
    /// fast-path omission to exploit. The stress runs as a no-anomaly
    /// guard: any anomaly would now indicate a regression of the
    /// always-archive invariant.
    #[tokio::test]
    async fn mvcc2_stress_race_not_triggered_with_in_memory_store() {
        use std::sync::atomic::{AtomicUsize, Ordering as AO};

        let gate = Arc::new(crate::repo_tx_gate::RepoTxGate::fresh());
        let mvcc = Arc::new(make_mvcc_with_gate(gate.clone()));

        let anomaly_count = Arc::new(AtomicUsize::new(0));

        let iterations = 500;
        let mut handles = Vec::with_capacity(iterations * 2);

        for i in 0..iterations {
            let key = Bytes::copy_from_slice(&(i as u64).to_be_bytes());
            let mvcc_w = Arc::clone(&mvcc);
            let gate_r = Arc::clone(&gate);
            let mvcc_r = Arc::clone(&mvcc);
            let anomaly = Arc::clone(&anomaly_count);

            // Writer: set_versioned on the key.
            let key_w = key.clone();
            let write_handle = tokio::spawn(async move {
                let _ = mvcc_w
                    .set_versioned(key_w, Bytes::from("written"))
                    .await
                    .unwrap();
            });

            // Reader: open snapshot and try to read the key.
            let key_r = key.clone();
            let read_handle = tokio::spawn(async move {
                let snap = gate_r.open_snapshot().await;
                let snap_v = snap.version();
                // Small yield to increase interleaving odds.
                tokio::task::yield_now().await;
                let result = mvcc_r.get_at(&key_r, snap_v).await.unwrap();
                // T1a: every write publishes its version, so version_of is
                // never 0 after a write lands. An anomaly here would mean a
                // snapshot observed a value whose version is strictly greater
                // than snap_v AND version_of disagrees — i.e. a regression
                // of the always-archive invariant. With the fast path gone
                // this predicate should never fire.
                let write_v = mvcc_r.version_of(&key_r);
                if result.is_some() && snap_v == 0 && write_v == 0 {
                    anomaly.fetch_add(1, AO::Relaxed);
                }
                drop(snap);
            });

            handles.push(write_handle);
            handles.push(read_handle);
        }

        for h in handles {
            h.await.unwrap();
        }

        let anomalies = anomaly_count.load(AO::Relaxed);
        // T1a: with always-archive the anomaly predicate (version_of == 0
        // after a write) can never hold, so anomalies must be 0. A non-zero
        // count would indicate the always-archive invariant regressed.
        assert_eq!(
            anomalies, 0,
            "T1a always-archive: no phantom-read anomaly (version_of is never 0 after a write)"
        );
    }

    // ----------------------------------------------------------------
    // S2 — Level-3 pessimistic locking (wound-wait).
    // ----------------------------------------------------------------

    use std::sync::atomic::{AtomicBool, Ordering as AOrdering};
    use std::sync::Arc;

    /// Test bundle: a tx's wound flag + wake notify. Each test tx gets one
    /// and passes clones into every `lock_key` call so a wound issued on
    /// one key wakes a wait parked on another.
    struct TxWound {
        wounded: Arc<AtomicBool>,
        notify: Arc<tokio::sync::Notify>,
    }

    impl TxWound {
        fn new() -> Self {
            Self {
                wounded: Arc::new(AtomicBool::new(false)),
                notify: Arc::new(tokio::sync::Notify::new()),
            }
        }
        fn flag(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.wounded)
        }
        fn notify(&self) -> Arc<tokio::sync::Notify> {
            Arc::clone(&self.notify)
        }
        fn is_wounded(&self) -> bool {
            self.wounded.load(AOrdering::Acquire)
        }
    }

    /// Wound-wait basic: an OLDER tx (smaller version) requesting a
    /// conflicting lock WOUNDS the younger holder (younger's `wounded`
    /// becomes true, older acquires). A YOUNGER requester against an older
    /// holder WAITS (asserted via timeout).
    #[tokio::test]
    async fn lock_key_wound_wait_basic() {
        let mvcc = make_mvcc();
        let key = Bytes::from("lk");

        // Younger tx (version 20) holds Exclusive.
        let younger = TxWound::new();
        mvcc.lock_key(
            key.clone(),
            20,
            younger.flag(),
            younger.notify(),
            LockMode::Exclusive,
        )
        .await
        .unwrap();
        assert!(!younger.is_wounded());

        // Older tx (version 10) requests Exclusive → must WOUND the younger
        // holder and acquire immediately.
        let older = TxWound::new();
        mvcc.lock_key(
            key.clone(),
            10,
            older.flag(),
            older.notify(),
            LockMode::Exclusive,
        )
        .await
        .unwrap();
        assert!(
            younger.is_wounded(),
            "older tx must wound the younger holder"
        );
        assert!(!older.is_wounded());
    }

    /// A younger requester against an older holder WAITS — it must not
    /// acquire while the older holds the lock. Bounded by a timeout so a
    /// bug (e.g. acquiring anyway) fails the test instead of hanging.
    #[tokio::test]
    async fn lock_key_younger_waits_for_older() {
        let mvcc = make_mvcc();
        let key = Bytes::from("wait");

        // Older tx (version 5) holds Exclusive.
        let older = TxWound::new();
        mvcc.lock_key(
            key.clone(),
            5,
            older.flag(),
            older.notify(),
            LockMode::Exclusive,
        )
        .await
        .unwrap();

        // Younger tx (version 9) requests Exclusive → must WAIT. Wrap in a
        // timeout: if it acquired (bug) the test fails; if it correctly
        // waits, the timeout fires.
        let younger = TxWound::new();
        let wait_future = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            mvcc.lock_key(
                key.clone(),
                9,
                younger.flag(),
                younger.notify(),
                LockMode::Exclusive,
            ),
        )
        .await;
        assert!(
            wait_future.is_err(),
            "younger tx must WAIT on older holder (timeout expected, not acquisition)"
        );
        assert!(
            !younger.is_wounded(),
            "younger waiter must not be wounded by the older holder"
        );
    }

    /// Deadlock-freedom: two Level-3 txs lock keys in OPPOSITE order (T1:
    /// A then B; T2: B then A) concurrently. Both runs must terminate
    /// (bounded by a generous timeout) and neither deadlocks. This is the
    /// core invariant: wound-wait on the total version order cannot cycle.
    #[tokio::test]
    async fn lock_key_deadlock_freedom_opposite_order() {
        let mvcc = Arc::new(make_mvcc());
        let key_a = Bytes::from("deadlock_a");
        let key_b = Bytes::from("deadlock_b");

        // T1 = version 1 (older), T2 = version 2 (younger). T1 has higher
        // priority. When they conflict, T2 gets wounded and T1 proceeds.
        // Each tx uses ONE TxWound across all its lock_key calls so a wound
        // issued on one key wakes a wait parked on another (mirrors the
        // real TxContext.wound_notify invariant).
        let t1 = TxWound::new();
        let t2 = TxWound::new();

        // Clone the flag/notify TWICE per tx (one per lock_key call) so
        // both calls share the same underlying Arcs.
        let t1 = (t1.flag(), t1.notify(), t1.flag(), t1.notify());
        let t2 = (t2.flag(), t2.notify(), t2.flag(), t2.notify());

        let mvcc1 = Arc::clone(&mvcc);
        let mvcc2 = Arc::clone(&mvcc);
        let key_a1 = key_a.clone();
        let key_b1 = key_b.clone();
        let key_a2 = key_a.clone();
        let key_b2 = key_b.clone();

        // T1: lock A (Exclusive), then B (Exclusive). Same wound/notify.
        let t1_handle = tokio::spawn(async move {
            let (f1, n1, f2, n2) = t1;
            mvcc1
                .lock_key(key_a1, 1, f1, n1, LockMode::Exclusive)
                .await
                .unwrap();
            tokio::task::yield_now().await;
            mvcc1.lock_key(key_b1, 1, f2, n2, LockMode::Exclusive).await
        });

        // T2: lock B (Exclusive), then A (Exclusive). Same wound/notify.
        let t2_handle = tokio::spawn(async move {
            let (f1, n1, f2, n2) = t2;
            mvcc2
                .lock_key(key_b2, 2, f1, n1, LockMode::Exclusive)
                .await
                .unwrap();
            tokio::task::yield_now().await;
            mvcc2.lock_key(key_a2, 2, f2, n2, LockMode::Exclusive).await
        });

        // Bound with a generous timeout: a real deadlock hangs CI and fails.
        let (r1, r2) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            (t1_handle.await.unwrap(), t2_handle.await.unwrap())
        })
        .await
        .expect("deadlock-freedom: both txs must terminate within timeout");

        // At least one completes by wounding/serialization. T2 (younger) is
        // the one that gets wounded when it tries to take A (held by T1):
        // a wound means T2's second lock_key returns Err. T1 (older) wounds
        // T2 and succeeds.
        let _ = r1; // T1 result
        let _ = r2; // T2 result (may be Ok or Err depending on interleaving)
    }

    /// Re-entrant: the same tx_version acquiring the same key twice (e.g.
    /// read then write) does NOT self-deadlock. A Shared acquire followed
    /// by an Exclusive acquire for the same tx succeeds.
    #[tokio::test]
    async fn lock_key_reentrant_same_tx_no_self_deadlock() {
        let mvcc = make_mvcc();
        let key = Bytes::from("reent");
        let w = TxWound::new();

        // First acquire Shared.
        mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Shared)
            .await
            .unwrap();
        // Re-acquire Exclusive (upgrade) — same tx, must not deadlock.
        mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Exclusive)
            .await
            .unwrap();
        // And again — idempotent.
        mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Exclusive)
            .await
            .unwrap();

        // The tx still holds exactly one holder entry (no duplicates).
        let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
        let state = lock.state.lock().await;
        assert_eq!(
            state.holders.len(),
            1,
            "re-entrant re-acquire must not duplicate holders"
        );
        assert_eq!(state.holders[0].tx_version, 42);
        assert_eq!(state.mode, Some(LockMode::Exclusive));
    }

    /// Release on commit and on abort: after a Level-3 tx's locks are
    /// released, the holders are empty (mode None). Both the commit path
    /// and the abort path call `release_locks`.
    #[tokio::test]
    async fn release_locks_clears_holders() {
        let mvcc = make_mvcc();
        let key_a = Bytes::from("rel_a");
        let key_b = Bytes::from("rel_b");
        let w = TxWound::new();

        // Acquire Exclusive on both keys.
        mvcc.lock_key(key_a.clone(), 7, w.flag(), w.notify(), LockMode::Exclusive)
            .await
            .unwrap();
        mvcc.lock_key(key_b.clone(), 7, w.flag(), w.notify(), LockMode::Shared)
            .await
            .unwrap();

        // Confirm held.
        let la = mvcc.locks.get(&key_a).map(|e| Arc::clone(e.get())).unwrap();
        {
            let s = la.state.lock().await;
            assert_eq!(s.holders.len(), 1);
            assert_eq!(s.mode, Some(LockMode::Exclusive));
        }

        // Release (as commit/abort would).
        mvcc.release_locks(7, &[key_a.clone(), key_b.clone()]).await;

        // Both keys now empty.
        {
            let s = la.state.lock().await;
            assert!(s.holders.is_empty(), "holders must be empty after release");
            assert_eq!(s.mode, None, "mode must be None after release");
        }
        let lb = mvcc.locks.get(&key_b).map(|e| Arc::clone(e.get())).unwrap();
        {
            let s = lb.state.lock().await;
            assert!(s.holders.is_empty());
            assert_eq!(s.mode, None);
        }
    }

    /// Zero-overhead invariant: a Snapshot and a Serializable tx never
    /// populate `locks`. The locks registry stays empty when no Level-3
    /// lock is acquired (the snapshot/serializable paths never call
    /// `lock_key`). This is verified at the MvccStore level: regular
    /// set_versioned/get_at leave `locks` untouched.
    #[tokio::test]
    async fn locks_registry_empty_without_pessimistic_acquire() {
        let mvcc = make_mvcc();
        // Snapshot-style writes (no lock_key calls).
        mvcc.set_versioned(Bytes::from("z"), Bytes::from("v"))
            .await
            .unwrap();
        let _ = mvcc.get_at(b"z", 0).await.unwrap();
        assert_eq!(
            mvcc.locks_len(),
            0,
            "locks registry must stay empty without an explicit Level-3 acquire"
        );
    }

    /// Shared+Shared compatibility: two DISTINCT txs can both hold Shared
    /// on the same key (multiple readers). A third Exclusive request
    /// conflicts and wounds the younger Shared holders.
    #[tokio::test]
    async fn lock_key_shared_shared_compatible() {
        let mvcc = make_mvcc();
        let key = Bytes::from("ss");

        // T1 (version 1) Shared.
        let t1 = TxWound::new();
        mvcc.lock_key(key.clone(), 1, t1.flag(), t1.notify(), LockMode::Shared)
            .await
            .unwrap();
        // T2 (version 2) Shared — compatible, both hold.
        let t2 = TxWound::new();
        mvcc.lock_key(key.clone(), 2, t2.flag(), t2.notify(), LockMode::Shared)
            .await
            .unwrap();

        let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
        {
            let s = lock.state.lock().await;
            assert_eq!(s.holders.len(), 2, "two Shared holders");
            assert_eq!(s.mode, Some(LockMode::Shared));
        }

        // T0 (version 0, OLDEST) Exclusive → wounds both younger Shared
        // holders and acquires.
        let t0 = TxWound::new();
        mvcc.lock_key(key.clone(), 0, t0.flag(), t0.notify(), LockMode::Exclusive)
            .await
            .unwrap();
        assert!(t1.is_wounded(), "younger Shared holder T1 wounded");
        assert!(t2.is_wounded(), "younger Shared holder T2 wounded");
        let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
        let s = lock.state.lock().await;
        assert_eq!(
            s.holders.len(),
            1,
            "older Exclusive wounds younger Shared holders"
        );
        assert_eq!(s.holders[0].tx_version, 0);
        assert_eq!(s.mode, Some(LockMode::Exclusive));
    }

    /// When a tx is wounded while WAITING (on a different key than where
    /// the wound is issued), its `lock_key` returns `DbError::Conflict`
    /// instead of acquiring. This exercises the per-tx `wound_notify`:
    /// the wound is triggered via the flag + the tx's own notify, waking
    /// it from a wait parked on the key's notify.
    #[tokio::test]
    async fn lock_key_wounded_waiter_aborts() {
        let mvcc = Arc::new(make_mvcc());
        let key = Bytes::from("wabort");

        // Older tx (version 1) holds Exclusive.
        let older = TxWound::new();
        mvcc.lock_key(
            key.clone(),
            1,
            older.flag(),
            older.notify(),
            LockMode::Exclusive,
        )
        .await
        .unwrap();

        // Younger tx (version 2) starts waiting.
        let younger = TxWound::new();
        let younger_notify = younger.notify();
        let mvcc_c = Arc::clone(&mvcc);
        let key_c = key.clone();
        let yw_flag = younger.flag();
        let yw_notify = younger.notify();
        let wait = tokio::spawn(async move {
            mvcc_c
                .lock_key(key_c, 2, yw_flag, yw_notify, LockMode::Exclusive)
                .await
        });

        // Give the waiter a chance to park, then wound it via its own
        // notify (simulating a wound issued on a DIFFERENT key).
        tokio::task::yield_now().await;
        younger.set_wounded();
        younger_notify.notify_one();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), wait)
            .await
            .expect("wounded waiter must return (not hang)")
            .unwrap();

        assert!(
            matches!(result, Err(DbError::Conflict(_))),
            "wounded waiter must return Conflict error, got {:?}",
            result
        );
    }

    impl TxWound {
        fn set_wounded(&self) {
            self.wounded.store(true, AOrdering::Release);
        }
    }

    // ================================================================
    // C1 — current-into-log / tombstone / vacuum-guard tests.
    // ================================================================

    /// C1 guarantee: the current version is written into the log (dual-write).
    #[tokio::test]
    async fn c1_current_version_is_in_the_log() {
        let mvcc = make_mvcc();
        let key = Bytes::from("c1_key");
        let val = Bytes::from("c1_val");
        let v = mvcc.set_versioned(key.clone(), val.clone()).await.unwrap();

        // The log entry at encode_version_key(key, v) must hold val.
        let log_val = mvcc
            .history_store()
            .get(encode_version_key(&key, v))
            .await
            .unwrap();
        assert_eq!(log_val, val, "C1: current version must be in the log");
    }

    /// C1 guarantee: delete writes a tombstone (empty value) into the log.
    #[tokio::test]
    async fn c1_delete_writes_tombstone() {
        let mvcc = make_mvcc();
        let key = Bytes::from("c1_del");
        mvcc.set_versioned(key.clone(), Bytes::from("val"))
            .await
            .unwrap();
        let del_v = mvcc.delete_versioned(key.clone()).await.unwrap();

        let tombstone = mvcc
            .history_store()
            .get(encode_version_key(&key, del_v))
            .await
            .unwrap();
        assert_eq!(
            tombstone,
            Bytes::new(),
            "C1: delete must write an empty tombstone into the log"
        );
    }

    /// C1 guarantee: vacuum never reclaims the current version.
    #[tokio::test]
    async fn c1_vacuum_never_reclaims_current() {
        let mvcc = make_mvcc(); // CurrentOnly — max_count=0

        let key = Bytes::from("c1_sacred");
        let mut latest_val = Bytes::new();
        for i in 1..=5u32 {
            latest_val = Bytes::from(format!("v{i}"));
            mvcc.set_versioned(key.clone(), latest_val.clone())
                .await
                .unwrap();
        }

        // After 5 writes with eager vacuum, only the current version survives.
        let cur_v = mvcc.version_of(&key);
        let log_val = mvcc
            .history_store()
            .get(encode_version_key(&key, cur_v))
            .await
            .unwrap();
        assert_eq!(
            log_val, latest_val,
            "C1: current version must survive vacuum"
        );

        // No other versions survive.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(hist, 1, "C1: only the current version survives vacuum");
    }

    /// C1 guarantee: the tombstone (current after delete) survives vacuum.
    #[tokio::test]
    async fn c1_vacuum_keeps_tombstone_current() {
        let mvcc = make_mvcc(); // CurrentOnly

        let key = Bytes::from("c1_tomb");
        mvcc.set_versioned(key.clone(), Bytes::from("val"))
            .await
            .unwrap();
        let del_v = mvcc.delete_versioned(key.clone()).await.unwrap();

        // The tombstone is the current version for this key — it must survive.
        let tombstone = mvcc
            .history_store()
            .get(encode_version_key(&key, del_v))
            .await
            .unwrap();
        assert_eq!(
            tombstone,
            Bytes::new(),
            "C1: delete tombstone must survive vacuum"
        );
    }

    // ================================================================
    // C2 — reads resolve from the single LOG, not `main`.
    // ================================================================

    /// C2: `get_current` reads the log, not `main`. Clearing `main` after a
    /// write must NOT change what `get_current` returns.
    #[tokio::test]
    async fn c2_get_current_reads_log_not_main() {
        let mvcc = make_mvcc();
        let key = Bytes::from("c2_k");
        let val = Bytes::from("c2_v");
        mvcc.set_versioned(key.clone(), val.clone()).await.unwrap();

        // Wipe main — if get_current still read main this would break.
        mvcc.main_store().remove(key.clone()).await.unwrap();

        let got = mvcc.get_current(key.clone()).await.unwrap();
        assert_eq!(
            got,
            Some(val),
            "C2: get_current must read the log, not main"
        );
    }

    /// C2: `current_stream` reads the log, not `main`. It also picks the
    /// MAX version per key (the current), suppressing older versions.
    #[tokio::test]
    async fn c2_current_stream_reads_log_not_main() {
        use std::collections::BTreeMap;

        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // k1 written twice (consecutively) → log holds v1(old) AND v2(current).
        mvcc.set_versioned(Bytes::from("k1"), Bytes::from("a"))
            .await
            .unwrap();
        mvcc.set_versioned(Bytes::from("k1"), Bytes::from("b"))
            .await
            .unwrap();
        mvcc.set_versioned(Bytes::from("k2"), Bytes::from("c"))
            .await
            .unwrap();
        mvcc.set_versioned(Bytes::from("k3"), Bytes::from("d"))
            .await
            .unwrap();

        // Wipe main entirely — current_stream must rebuild from the log.
        for k in ["k1", "k2", "k3"] {
            let _ = mvcc.main_store().remove(Bytes::from(k)).await;
        }

        let mut got: BTreeMap<Vec<u8>, Bytes> = BTreeMap::new();
        let stream = mvcc.current_stream(64);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch.unwrap() {
                got.insert(k.to_vec(), v);
            }
        }

        assert_eq!(got.len(), 3, "C2: exactly 3 current keys");
        // k1's CURRENT is "b" (the max version), NOT the old "a".
        assert_eq!(
            got.get(&b"k1"[..]),
            Some(&Bytes::from("b")),
            "C2: max version"
        );
        assert_eq!(got.get(&b"k2"[..]), Some(&Bytes::from("c")));
        assert_eq!(got.get(&b"k3"[..]), Some(&Bytes::from("d")));
    }

    /// C2: `get_current` of a deleted key returns `None` (reads the tombstone).
    #[tokio::test]
    async fn c2_get_current_none_for_tombstone() {
        let mvcc = make_mvcc();
        let key = Bytes::from("c2_del");
        mvcc.set_versioned(key.clone(), Bytes::from("v"))
            .await
            .unwrap();
        mvcc.delete_versioned(key.clone()).await.unwrap();

        let got = mvcc.get_current(key).await.unwrap();
        assert!(got.is_none(), "C2: deleted key → None (tombstone)");
    }

    /// C2: `get_at` at the delete version → `None`; at a pre-delete snapshot →
    /// the old value. KeepHistory so the pre-delete version is not vacuumed.
    #[tokio::test]
    async fn c2_get_at_tombstone_is_none() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        let key = Bytes::from("c2_asof");
        let va = mvcc
            .set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let vd = mvcc.delete_versioned(key.clone()).await.unwrap();

        // As-of the delete version → deleted (None).
        let at_delete = mvcc.get_at(&key, vd).await.unwrap();
        assert!(at_delete.is_none(), "C2: as-of delete version → None");

        // As-of the pre-delete version → the old value.
        let at_before = mvcc.get_at(&key, va).await.unwrap();
        assert_eq!(
            at_before,
            Some(Bytes::from("v1")),
            "C2: pre-delete snapshot sees the old value"
        );
    }

    /// C2: cold-start — a fresh MvccStore over the SAME log (empty cell cache)
    /// resolves `get_current` by seeking the latest version in the log.
    #[tokio::test]
    async fn c2_cold_start_seek() {
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let key = Bytes::from("c2_cold");
        let val = Bytes::from("cold_val");

        // First store writes the value into the shared log.
        let mvcc1 = MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::clone(&history),
            make_gate(),
        );
        mvcc1.set_versioned(key.clone(), val.clone()).await.unwrap();

        // Second store over the SAME log has an empty cell cache → cur_v == 0
        // → must seek the latest version from the log.
        let mvcc2 = MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::clone(&history),
            make_gate(),
        );
        assert_eq!(
            mvcc2.version_of(&key),
            0,
            "precondition: cold cell (no cached version)"
        );
        let got = mvcc2.get_current(key).await.unwrap();
        assert_eq!(
            got,
            Some(val),
            "C2: cold-start get_current must seek the log"
        );
    }

    /// C2 regression: `current_stream` must NOT panic when the current-key set
    /// exceeds one batch. The first streaming-group-by implementation paniced
    /// (`unreachable!()`) on the second pull whenever `out_batch` filled to
    /// `batch_size` and returned a `Streaming` continuation state.
    #[tokio::test]
    async fn c2_current_stream_exceeds_batch_no_panic() {
        use std::collections::BTreeSet;

        let mvcc = make_mvcc();
        // 5 distinct current keys, streamed with batch_size = 2 (3 output
        // batches: 2 + 2 + 1). The buggy code paniced on the 2nd pull.
        for i in 0..5u32 {
            mvcc.set_versioned(Bytes::from(format!("bk{i}")), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        let mut keys: BTreeSet<Vec<u8>> = BTreeSet::new();
        let stream = mvcc.current_stream(2);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (k, _v) in batch.unwrap() {
                keys.insert(k.to_vec());
            }
        }
        assert_eq!(
            keys.len(),
            5,
            "C2: current_stream must yield all 5 keys across batches without panicking"
        );
    }

    // ================================================================
    // T1b.1 — eager vacuum (CurrentOnly default) tests.
    // ================================================================

    /// CurrentOnly store, no snapshots: write the same key 5 times; eager
    /// vacuum reclaims superseded history on every write, so `history` holds
    /// 0 old versions afterward while `get_at` at the floor still returns
    /// the current value.
    #[tokio::test]
    async fn eager_vacuum_currentonly_bounds_history() {
        let mvcc = make_mvcc();

        let key = Bytes::from("vacuum_key");
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // C1: the current version lives in the log and is SACRED (cur_v guard).
        // After 5 writes with max_count=0, only the current version (v5) survives.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 1,
            "C1: CurrentOnly eager vacuum leaves 1 entry (the current version in the log), got {hist}"
        );

        // The current value is still readable at the floor.
        let last_committed = mvcc.gate.last_committed();
        let result = mvcc.get_at(&key, last_committed).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v5")),
            "get_at at last_committed must return the current value"
        );
    }

    /// A live snapshot pins the version it needs: overwriting the key does
    /// NOT reclaim the version the snapshot may still read. Dropping the
    /// snapshot unpins it, and a subsequent write's eager vacuum reclaims.
    #[tokio::test]
    async fn eager_vacuum_keeps_versions_pinned_by_live_snapshot() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let key = Bytes::from("pinned_key");

        // Write v1 (no snapshot) — publishes last_committed = v1.
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);

        // Open a snapshot at v1 — pins min_alive to v1.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, v1);

        // Overwrite with v2. Eager vacuum runs but min_alive == v1 protects
        // the v1 history entry (gc_below only removes < min_alive).
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // The snapshot at v1 still reads v1 via history (slow path).
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v1")),
            "live snapshot must still read the pinned prior version"
        );

        // Drop the snapshot → min_alive advances. A further write's eager
        // vacuum can now reclaim the unpinned old version.
        drop(snap);
        mvcc.set_versioned(key.clone(), Bytes::from("v3"))
            .await
            .unwrap();

        // After reclaim, a read at the OLD snapshot version (v1) should no
        // longer find the reclaimed entry (it was < min_alive). The current
        // value is still correct.
        let last_committed = mvcc.gate.last_committed();
        let current = mvcc.get_at(&key, last_committed).await.unwrap();
        assert_eq!(current, Some(Bytes::from("v3")));
    }

    /// Deterministic interleaving (PausableStore): a write+eager-reclaim
    /// interleaved with an `open_snapshot`. The just-opened snapshot must
    /// NEVER read `None` for a version it should see — the register-before-use
    /// ordering + min_alive floor protect it. (§4.1-class race; loom sweep
    /// deferred to T1d.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eager_vacuum_race_open_snapshot() {
        let key = Bytes::from("race_key");
        let old_val = Bytes::from("OLD");
        let new_val = Bytes::from("NEW");

        // Build the MvccStore with a PausableStore as `main` (CurrentOnly
        // default — eager vacuum active).
        let pausable = Arc::new(PausableStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();
        let mvcc = Arc::new(MvccStore::new(
            pausable.clone() as Arc<dyn Store>,
            history,
            gate.clone(),
        ));

        // Seed OLD (no snapshot) — lands in main, cell published.
        mvcc.set_versioned(key.clone(), old_val.clone())
            .await
            .unwrap();
        let v_seed = mvcc.version_of(&key);
        gate.publish_committed(v_seed);

        // Arm: the next `main.set` (inside the NEW write) will pause BEFORE
        // the physical write. The write sequence is:
        //   archive_prior → publish_cell → main.set(PAUSE) → publish_committed_max → eager_vacuum
        pausable.arm();

        let mvcc_w = Arc::clone(&mvcc);
        let key_w = key.clone();
        let new_val_w = new_val.clone();
        let write_handle = tokio::spawn(async move {
            mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
        });

        // Wait until the write is paused inside main.set — OLD is already
        // archived and the cell already advanced to v_new.
        pausable.entered.notified().await;

        // Open a snapshot HERE — interleaved between the write landing and
        // the eager vacuum. The snapshot registers in active_snapshots
        // BEFORE it is usable, so the eager vacuum's min_alive will include
        // this snapshot's version.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();

        // Release: the write completes (main.set), then publish_committed_max,
        // then eager_vacuum runs with min_alive that now includes snap_v.
        pausable.release();
        write_handle.await.unwrap();

        // The snapshot must NOT read None for a version it should see.
        // snap_v == v_seed (the published floor before the NEW write).
        // The snapshot predates NEW, so it should see OLD (archived before
        // the pause). The eager vacuum cannot have reclaimed it because
        // min_alive ≤ snap_v (the snapshot is registered).
        let seen = mvcc.get_at(&key, snap_v).await.unwrap();
        assert!(
            seen.is_some(),
            "snapshot opened mid-write must never read None for a version it should see"
        );
        assert_eq!(
            seen,
            Some(old_val),
            "snapshot predating NEW must see OLD (archived, pinned by min_alive)"
        );
    }

    /// KeepHistory store: write the key 5 times; all old versions remain in
    /// history (no eager reclaim).
    #[tokio::test]
    async fn keephistory_no_eager_vacuum() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        let key = Bytes::from("keep_key");
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // C1: all 5 versions in the log (v1..v4 prior + v5 current), no eager vacuum.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 5,
            "C1: KeepHistory retains all 5 versions in the log (4 prior + current), got {hist}"
        );

        // Current value still correct.
        let last_committed = mvcc.gate.last_committed();
        let result = mvcc.get_at(&key, last_committed).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v5")));
    }

    // ================================================================
    // T1b.2 — orthogonal retention knobs (per-key count vacuum).
    //
    // All counts below assume NO live snapshot unless the test opens one.
    // With no snapshot: min_alive == last_committed == current, so the
    // anchor is None (a fresh snapshot opens at `current` and reads `main`).
    // ================================================================

    /// 1. `max_count: Some(3)`, 6 writes (5 old), no snapshot → exactly 3 old
    /// remain; the 2 oldest are reclaimed; kept versions are reachable.
    #[tokio::test]
    async fn retention_count_only_keeps_last_n() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention {
            max_age_secs: None,
            max_count: Some(3),
            min_count: None,
        })
        .unwrap();

        let key = Bytes::from("count_key");
        let mut versions = Vec::new();
        for i in 1..=6u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
            versions.push(mvcc.version_of(&key));
        }

        // C1: 6 versions in the log. cur_v=v6 occupies idx 0 (skipped by guard).
        // max_count=3 keeps idx 1,2 (v5, v4). v3..v1 (idx 3..5) reclaimed.
        // Total: v6 (cur_v) + v5, v4 = 3.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 3,
            "C1: max_count=3 keeps current + 2 within window = 3"
        );

        // The newest 3 are reachable via get_at.
        for &v in &versions[versions.len() - 3..] {
            let result = mvcc.get_at(&key, v).await.unwrap();
            assert!(result.is_some(), "kept version {v} must be reachable");
        }
        // Current value is correct.
        let last_committed = mvcc.gate.last_committed();
        assert_eq!(
            mvcc.get_at(&key, last_committed).await.unwrap(),
            Some(Bytes::from("v6"))
        );
    }

    /// 2. Default `max_count: Some(0)` (CurrentOnly), 4 writes, no snapshot →
    /// 0 old versions remain (only current in `main`).
    #[tokio::test]
    async fn retention_current_only_is_max_count_zero() {
        let mvcc = make_mvcc(); // default = CurrentOnly

        let key = Bytes::from("co_key");
        for i in 1..=4u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // C1: the current version (v4) survives in the log (cur_v guard);
        // all older versions are reclaimed by max_count=0.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 1,
            "C1: max_count=0 reclaims all old versions, current stays (1 entry)"
        );

        let last_committed = mvcc.gate.last_committed();
        assert_eq!(
            mvcc.get_at(&key, last_committed).await.unwrap(),
            Some(Bytes::from("v4"))
        );
    }

    /// 3. `max_count: None, min_count: Some(2)`, 5 writes (4 old) → all 4 old
    /// remain. No count cap → early return; `min_count` alone is a satisfied
    /// floor, not a reclaimer.
    #[tokio::test]
    async fn retention_min_count_standalone_keeps_all() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention {
            max_age_secs: None,
            max_count: None,
            min_count: Some(2),
        })
        .unwrap();

        let key = Bytes::from("floor_key");
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // C1: max_count=None → vacuum_key returns early. All 5 versions in the
        // log survive (4 prior + 1 current).
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 5,
            "C1: max_count=None keeps ALL 5 versions in the log (4 prior + current)"
        );
    }

    /// 4. `max_count: Some(0)` with a live snapshot: the versions the snapshot
    /// may read survive (protected by `>= min_alive`, branch (b)). After
    /// dropping the snapshot, a further write reclaims everything (current
    /// only). No anchor is needed in either phase of this scenario.
    #[tokio::test]
    async fn retention_keeps_pinned_and_anchor_with_snapshot() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let key = Bytes::from("pin_key");
        // Write v1 — main=v1, last_committed=v1.
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);

        // Open a snapshot at v1 — pins min_alive=v1.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, v1);

        // Overwrite to v2, v3. Each vacuum runs with min_alive=v1: v1 (then
        // v1,v2) are `>= min_alive` → sacred (branch b), never reclaimed.
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v3"))
            .await
            .unwrap();

        // The snapshot at v1 still reads v1 via history (slow path).
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v1")),
            "live snapshot must still read the pinned prior version"
        );

        // Drop the snapshot → min_alive advances to last_committed. A further
        // write's vacuum now reclaims every old version (no anchor: no live
        // snapshot remains).
        drop(snap);
        mvcc.set_versioned(key.clone(), Bytes::from("v4"))
            .await
            .unwrap();

        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 1,
            "C1: unpinned old versions reclaimed, current v4 stays in the log (1 entry)"
        );
    }

    /// 5. The case the anchor is FOR: with `max_count: Some(0)` and a snapshot
    /// pinned BELOW the most recent write, the single largest version
    /// `< min_alive` (the anchor) survives alongside every version
    /// `>= min_alive`; strictly-below-anchor versions are reclaimed; the
    /// snapshot reads the correct value at its version.
    #[tokio::test]
    async fn retention_anchor_below_min_alive() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let key = Bytes::from("anchor_key");
        // Accumulate full history first under KeepHistory (the default
        // CurrentOnly policy would reclaim on every write, leaving nothing to
        // exercise the anchor path).
        mvcc.set_retention(Retention::keep_history()).unwrap();
        // Write 10 times → history has v1..v9 (9 entries), main=v10,
        // last_committed=10.
        for i in 1..=10u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }
        // C1: history has v1..v10 (all written as current).
        let hist_before = count_history_entries(&mvcc).await;
        assert_eq!(
            hist_before, 10,
            "C1: KeepHistory retains all 10 versions in the log (including current v10)"
        );

        // Open a snapshot at v10 — pins min_alive=10.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, 10);

        // Switch to the aggressive policy JUST before the reclaiming write.
        mvcc.set_retention(Retention {
            max_age_secs: None,
            max_count: Some(0),
            min_count: None,
        })
        .unwrap();

        // Overwrite once more: archives v10 → history@v10, assigns v11.
        // Vacuum: keep_n=0, min_alive=10, live snapshot → anchor = max<10 = v9.
        //   v10: >= min_alive → kept (b)
        //   v9: == anchor → kept (c)
        //   v8..v1: reclaimed
        mvcc.set_versioned(key.clone(), Bytes::from("v11"))
            .await
            .unwrap();

        // C1: 3 entries survive — current v11 (cur_v guard), pinned v10 (≥ min_alive),
        // and anchor v9.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 3,
            "C1: max_count=0 + live snapshot: current + pinned (>=min_alive) + anchor"
        );

        // The snapshot at v10 reads v10 (the value current at its version).
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v10")),
            "snapshot at v10 must read v10 (pinned by min_alive)"
        );

        // Verify the surviving entries are exactly {v9, v10} by reading them.
        let v9 = 9u64;
        let v10 = 10u64;
        assert!(
            mvcc.get_at(&key, v9).await.unwrap().is_some(),
            "anchor v9 (largest < min_alive) must survive"
        );
        assert!(
            mvcc.get_at(&key, v10).await.unwrap().is_some(),
            "pinned v10 (>= min_alive) must survive"
        );
        // A strictly-below-anchor version was reclaimed.
        let v8 = 8u64;
        assert!(
            mvcc.get_at(&key, v8).await.unwrap().is_none(),
            "v8 (below anchor) must be reclaimed"
        );
    }

    /// 6. Deterministic interleaving (PausableStore): a write+count-vacuum
    /// interleaved with an `open_snapshot`. The snapshot must NEVER read None
    /// for a version it should see — the min_alive floor holds above the count
    /// knobs. (§4.1-class race; loom sweep deferred to T1d.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retention_race_open_snapshot_with_count() {
        let key = Bytes::from("race_key");
        let old_val = Bytes::from("OLD");
        let new_val = Bytes::from("NEW");

        let pausable = Arc::new(PausableStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();
        let mvcc = Arc::new(MvccStore::new(
            pausable.clone() as Arc<dyn Store>,
            history,
            gate.clone(),
        ));
        // Aggressive vacuum — but min_alive must still protect pinned versions.
        mvcc.set_retention(Retention {
            max_age_secs: None,
            max_count: Some(0),
            min_count: None,
        })
        .unwrap();

        // Seed OLD.
        mvcc.set_versioned(key.clone(), old_val.clone())
            .await
            .unwrap();
        let v_seed = mvcc.version_of(&key);
        gate.publish_committed(v_seed);

        // Arm: the next `main.set` pauses.
        pausable.arm();

        let mvcc_w = Arc::clone(&mvcc);
        let key_w = key.clone();
        let new_val_w = new_val.clone();
        let write_handle = tokio::spawn(async move {
            mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
        });

        // Wait for the write to pause inside main.set (after archive_prior +
        // publish_cell, before publish_committed_max + vacuum_key).
        pausable.entered.notified().await;

        // Open a snapshot mid-write — registers before usable.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();

        // Release: write completes → publish_committed_max → vacuum_key runs
        // with min_alive that now includes snap_v.
        pausable.release();
        write_handle.await.unwrap();

        let seen = mvcc.get_at(&key, snap_v).await.unwrap();
        assert!(
            seen.is_some(),
            "snapshot opened mid-write must never read None for a version it should see"
        );
        assert_eq!(
            seen,
            Some(old_val),
            "snapshot predating NEW must see OLD (pinned by min_alive above max_count=0)"
        );
    }

    /// 7. `set_retention` swaps the whole struct; `retention()` reflects it;
    /// `validate` rejects `min_count > max_count`.
    #[tokio::test]
    async fn retention_patch_independent() {
        let mvcc = make_mvcc();

        // Default is CurrentOnly (max_count: Some(0)).
        assert_eq!(**mvcc.retention(), Retention::current_only());

        // Swap to keep_history.
        mvcc.set_retention(Retention::keep_history()).unwrap();
        assert_eq!(**mvcc.retention(), Retention::keep_history());

        // Swap to a custom policy.
        let custom = Retention {
            max_age_secs: Some(3600),
            max_count: Some(10),
            min_count: Some(2),
        };
        mvcc.set_retention(custom).unwrap();
        assert_eq!(**mvcc.retention(), custom);

        // validate rejects min_count > max_count — old policy kept.
        let invalid = Retention {
            max_age_secs: None,
            max_count: Some(1),
            min_count: Some(5),
        };
        assert!(mvcc.set_retention(invalid).is_err(), "min>max must reject");
        // Previous (custom) policy survives the rejected swap.
        assert_eq!(
            **mvcc.retention(),
            custom,
            "rejected swap must not change policy"
        );
    }

    // ================================================================
    // T1c — per-version commit timestamp + max_age retention (AGE axis).
    //
    // All tests freeze the clock via `set_test_now` for determinism.
    // `count_history_entries` counts only version-keys (ts-keys excluded).
    // ================================================================

    /// 1. `max_age_secs: Some(60)` (KeepHistory base, no count cap), no
    /// snapshot: a version written 100s ago (ts=0, now=100_000) is reclaimed;
    /// versions within the 60s window are kept.
    #[tokio::test]
    async fn max_age_reclaims_old_versions() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention {
            max_age_secs: Some(60),
            max_count: None,
            min_count: None,
        })
        .unwrap();

        let key = Bytes::from("age_key");
        // v1 at an early frozen time (1ms — 0 is the "real clock" sentinel).
        mvcc.set_test_now(1);
        mvcc.set_versioned(key.clone(), Bytes::from("old"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);

        // 100s later (100_001ms) — v1 (ts=1) is now 100s old (> 60s cap).
        mvcc.set_test_now(100_001);
        mvcc.set_versioned(key.clone(), Bytes::from("new1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("new2"))
            .await
            .unwrap();

        // C1: v3 (current, cur_v guard) + v2 (age-kept) = 2 entries. v1 reclaimed by age.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 2,
            "C1: max_age=60s reclaims v1; v2 age-kept + v3 current = 2"
        );

        // The reclaimed v1 is no longer reachable via get_at.
        let stale = mvcc.get_at(&key, v1).await.unwrap();
        assert!(stale.is_none(), "reclaimed v1 must not be reachable");

        // The kept v2 is reachable; current is correct.
        let last_committed = mvcc.gate.last_committed();
        assert_eq!(
            mvcc.get_at(&key, last_committed).await.unwrap(),
            Some(Bytes::from("new2"))
        );
    }

    /// 2. `min_count` FLOOR overrides the age cap: `max_age_secs: Some(1)`,
    /// `min_count: Some(3)` — write 5 versions all well past the age cap; the
    /// newest 3 survive (min_count protects them from the age cap).
    #[tokio::test]
    async fn min_count_floor_overrides_age() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention {
            max_age_secs: Some(1),
            max_count: None,
            min_count: Some(3),
        })
        .unwrap();

        let key = Bytes::from("floor_age_key");
        // Write v1..v5 all at frozen ts=1 (0 is the "real clock" sentinel).
        // Vacuum runs after each write, but with clock=1 the age cutoff
        // saturates to 0 (1 - 1000 = 0) and ts=1 is not < 0, so nothing is
        // reclaimed yet — history accumulates v1..v4.
        mvcc.set_test_now(1);
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // Advance the clock to 10s (10x the 1s cap) and write once more to
        // trigger a vacuum. Now v1..v5 (ts=1) are all past the 1s cap.
        mvcc.set_test_now(10_000);
        mvcc.set_versioned(key.clone(), Bytes::from("v6"))
            .await
            .unwrap();

        // C1: cur_v=v6 at idx 0 (sacred). min_count=3 protects idx 0..2, but idx 0
        // is already sacred. v5(idx 1), v4(idx 2) protected by floor. v3..v1 past
        // age cap and beyond floor → reclaimed. Total: v6 + v5 + v4 = 3.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 3,
            "C1: min_count=3 floor + current = 3 (v6 cur_v, v5, v4 floor), past age cap"
        );
    }

    /// 3. age ∧ count intersection: a version is reclaimed only when BOTH
    /// caps agree to drop it (the KEEP condition is OR — within either cap's
    /// window). Two phases:
    ///   (a) beyond count BUT within age window → KEPT (age protects);
    ///   (b) within count BUT beyond age window → KEPT (count protects).
    #[tokio::test]
    async fn age_and_count_intersect_tighter_prunes() {
        // Phase (a): a version beyond the count cap but within the age window
        // is KEPT — age protects it from the count cap.
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention {
            max_age_secs: Some(60),
            max_count: Some(1),
            min_count: None,
        })
        .unwrap();
        let key = Bytes::from("intersect_a");
        // Write 3 versions all at the same frozen time. During accumulation
        // the age cutoff saturates to 0 (now=50_000, cutoff=0), so age keeps
        // everything and the count cap alone can't reclaim (AND semantics).
        mvcc.set_test_now(50_000);
        for i in 1..=3u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("a{i}")))
                .await
                .unwrap();
        }
        // C1: a3 (current, cur_v guard) + a1, a2 (age-kept) = 3 entries.
        let hist_a = count_history_entries(&mvcc).await;
        assert_eq!(
            hist_a, 3,
            "C1: phase (a): current + age-kept versions = 3 (age protects beyond count)"
        );

        // Phase (b): a version within the count cap but beyond the age window
        // is KEPT — count protects it from the age cap.
        let mvcc2 = make_mvcc();
        mvcc2
            .set_retention(Retention {
                max_age_secs: Some(60),
                max_count: Some(3),
                min_count: None,
            })
            .unwrap();
        let key2 = Bytes::from("intersect_b");
        // v1 at frozen ts=1 (0 is the "real clock" sentinel).
        mvcc2.set_test_now(1);
        mvcc2
            .set_versioned(key2.clone(), Bytes::from("old"))
            .await
            .unwrap();
        // 100s later: overwrite. v1 (ts=1) is now 100s old (> 60s cap).
        mvcc2.set_test_now(100_001);
        mvcc2
            .set_versioned(key2.clone(), Bytes::from("new"))
            .await
            .unwrap();
        // C1: v2 (current, cur_v guard) + v1 (count-protected) = 2 entries.
        let hist_b = count_history_entries(&mvcc2).await;
        assert_eq!(
            hist_b, 2,
            "C1: phase (b): current + count-protected version = 2"
        );

        // Phase (c): a version beyond BOTH caps is reclaimed.
        let mvcc3 = make_mvcc();
        mvcc3
            .set_retention(Retention {
                max_age_secs: Some(60),
                max_count: Some(1),
                min_count: None,
            })
            .unwrap();
        let key3 = Bytes::from("intersect_c");
        // v1, v2 at ts=1 (old); v3 at ts=100_001.
        mvcc3.set_test_now(1);
        mvcc3
            .set_versioned(key3.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc3
            .set_versioned(key3.clone(), Bytes::from("v2"))
            .await
            .unwrap();
        mvcc3.set_test_now(100_001);
        mvcc3
            .set_versioned(key3.clone(), Bytes::from("v3"))
            .await
            .unwrap();
        // After v3: entries desc = [v2(ts=1), v1(ts=1)].
        //   v2(idx0): within count (idx<1)? No. age: ts=1 < cutoff(40_001)? Yes.
        //     Both drop → reclaim.
        //   v1(idx1): beyond count. age drops. Both drop → reclaim.
        // Wait — both reclaimed? Then hist=0. But v2 is idx0 < max_count=1? 0 < 1 = yes!
        // Let me re-trace: max_count=1.
        //   v2(idx0): idx<1? YES → keep (within count window).
        //   v1(idx1): idx<1? No → count drops. age: ts=1<40001? Yes → age drops. → reclaim.
        // hist == 1 (v2 kept by count, v1 reclaimed by both).
        // C1: v3 (current, cur_v guard) = 1. v1, v2 both caps drop → reclaimed.
        let hist_c = count_history_entries(&mvcc3).await;
        assert_eq!(
            hist_c, 1,
            "C1: phase (c): current survives (cur_v guard); v1, v2 beyond both caps → reclaimed"
        );
    }

    /// 4. Under a live snapshot, an old-by-age version that is `>= min_alive`
    /// or is the anchor is NOT reclaimed despite the age cap.
    #[tokio::test]
    async fn age_keeps_pinned_and_anchor() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        mvcc.set_retention(Retention {
            max_age_secs: Some(1),
            max_count: Some(0),
            min_count: None,
        })
        .unwrap();

        let key = Bytes::from("age_pinned_key");
        // Accumulate history under KeepHistory first (the aggressive policy
        // would reclaim on every write, leaving nothing to exercise the path).
        mvcc.set_retention(Retention::keep_history()).unwrap();
        mvcc.set_test_now(1);
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }
        // 4 history entries (v1..v4), all ts=1.

        // Open a snapshot at v5 (last_committed) — pins min_alive=5.
        let snap = gate.open_snapshot().await;
        let snap_v = snap.version();
        assert_eq!(snap_v, 5);

        // Switch to the aggressive age+count policy and overwrite once.
        mvcc.set_retention(Retention {
            max_age_secs: Some(1),
            max_count: Some(0),
            min_count: None,
        })
        .unwrap();
        mvcc.set_test_now(10_000); // 10s later — v1..v4 all past the 1s cap.
        mvcc.set_versioned(key.clone(), Bytes::from("v6"))
            .await
            .unwrap();

        // C1: v6 (current, cur_v guard) + v5 (≥ min_alive) + v4 (anchor) = 3.
        // v3..v1: past age, beyond count, < min_alive, not anchor → reclaimed.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 3,
            "C1: age cap honors current + sacred floor + anchor: {{v6 current, v5 pinned, v4 anchor}}"
        );

        // The snapshot at v5 reads v5 (correct value at its version).
        let result = mvcc.get_at(&key, snap_v).await.unwrap();
        assert_eq!(
            result,
            Some(Bytes::from("v5")),
            "snapshot at v5 must read v5 (protected by min_alive despite age cap)"
        );
    }

    /// 5. A version with no recorded ts is conservatively KEPT by the age axis
    /// (unknown age → do not reclaim by age).
    #[tokio::test]
    async fn unknown_ts_not_reclaimed_by_age() {
        let mvcc = make_mvcc();
        // Only an age cap (no count cap) — so the age axis is the ONLY potential
        // reclaimer. With no count cap, the only way to reclaim is age.
        mvcc.set_retention(Retention {
            max_age_secs: Some(1),
            max_count: None,
            min_count: None,
        })
        .unwrap();

        let key = Bytes::from("no_ts_key");
        // Write v1, v2 with a frozen clock so they get real ts entries.
        mvcc.set_test_now(1);
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // Manually delete v1's ts entry — simulate a pre-T1c version with no ts.
        let _ = mvcc.history_store().remove(ts_key(v1)).await;

        // Advance the clock well past the age cap and write once more.
        mvcc.set_test_now(10_000);
        mvcc.set_versioned(key.clone(), Bytes::from("v3"))
            .await
            .unwrap();

        // C1: v3 (current, cur_v guard) + v1 (unknown ts, kept) = 2.
        // v2 (ts=1, age 10s > 1s) → reclaimed by age.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 2,
            "C1: unknown-ts v1 kept + current v3 = 2; v2 reclaimed by age"
        );
        // v1 is still reachable (its version entry survived).
        assert!(
            mvcc.get_at(&key, v1).await.unwrap().is_some(),
            "unknown-ts v1 must survive (conservatively kept by age axis)"
        );
    }

    /// 6. After writing version V, `history.get(ts_key(V))` returns the frozen
    /// now. Confirms ts is recorded per write and is decodable.
    #[tokio::test]
    async fn ts_recorded_on_write() {
        let mvcc = make_mvcc();
        mvcc.set_test_now(4242);

        let key = Bytes::from("ts_record_key");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v1 = mvcc.version_of(&key);

        // The ts-key for v1 holds the frozen now (4242), little-endian u64.
        let ts_val = mvcc.history_store().get(ts_key(v1)).await.unwrap();
        assert_eq!(ts_val.len(), 8, "ts value must be 8 bytes (u64 LE)");
        let ms = u64::from_le_bytes(ts_val.as_ref().try_into().unwrap());
        assert_eq!(ms, 4242, "recorded ts must equal the frozen now");

        // decode_ts_key round-trips for the same version.
        assert_eq!(decode_ts_key(&ts_key(v1)), Some(v1));
        // And a version-key is NOT mistaken for a ts-key.
        let vk = encode_version_key(&key, v1);
        assert!(
            decode_ts_key(&vk).is_none(),
            "version-key must not decode as ts-key"
        );

        mvcc.set_test_now(0); // restore real clock (hygiene)
    }

    // ========================================================================
    // T4-history — history_of
    // ========================================================================

    #[tokio::test]
    async fn history_of_returns_empty_for_unknown_key() {
        let mvcc = make_mvcc();
        let timeline = mvcc.history_of(b"absent").await.unwrap();
        assert!(timeline.is_empty(), "an unknown key has no timeline");
    }

    /// A key written three times must yield three timeline entries
    /// (v1, v2 from `history`, v3 from `main`), ascending by version,
    /// each carrying its value and its recorded commit timestamp.
    #[tokio::test]
    async fn history_of_three_writes_full_timeline_with_ts() {
        let mvcc = make_mvcc();
        // Default retention is CurrentOnly (max_count = 0) — vacuum
        // reclaims every archived version right after each write. To
        // observe a multi-version timeline we must opt into KeepHistory.
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // Freeze the clock so each version gets a distinct, known ts.
        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();
        mvcc.set_test_now(3_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
            .await
            .unwrap();

        let timeline = mvcc.history_of(b"k").await.unwrap();
        assert_eq!(
            timeline.len(),
            3,
            "three writes → three timeline entries (2 archived + 1 current)"
        );

        // Ascending by version.
        let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![1, 2, 3]);
        assert!(
            versions.windows(2).all(|w| w[0] < w[1]),
            "timeline must be ascending by version"
        );

        // Values line up per version.
        let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
        assert_eq!(values, vec![b"v1".as_slice(), b"v2", b"v3"]);

        // ts per version (T1c) — each matches the frozen clock at its write.
        let ts: Vec<Option<u64>> = timeline.iter().map(|e| e.ts_millis).collect();
        assert_eq!(ts, vec![Some(1_000), Some(2_000), Some(3_000)]);

        mvcc.set_test_now(0);
    }

    /// A deleted key contributes its prior versions plus the tombstone —
    /// all from the log, in ascending version order.
    #[tokio::test]
    async fn history_of_deleted_key_keeps_prior_versions() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();
        // C1: delete writes a tombstone (empty value) into the log.
        mvcc.delete_versioned(Bytes::from("k")).await.unwrap();

        let timeline = mvcc.history_of(b"k").await.unwrap();
        // C1: v1 + v2 (both written into log when current) + v3 (tombstone).
        // No current entry from main — main no longer has the key.
        assert_eq!(timeline.len(), 3);
        let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![1, 2, 3]);
        let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
        assert_eq!(values, vec![b"v1".as_slice(), b"v2", b""]);

        // Sanity: main really is empty for this key.
        match mvcc.main.get(Bytes::from("k")).await {
            Err(DbError::NotFound(_)) => {}
            other => panic!(
                "expected NotFound on main, got {:?}",
                other.map(|b| b.len())
            ),
        }
    }

    /// Two keys must not bleed into each other's timelines — a prefix
    /// collision (`"k"` vs `"kk"`) must keep them separate.
    #[tokio::test]
    async fn history_of_isolates_prefix_collisions() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        mvcc.set_versioned(Bytes::from("k"), Bytes::from("a"))
            .await
            .unwrap();
        mvcc.set_versioned(Bytes::from("kk"), Bytes::from("b"))
            .await
            .unwrap();

        let tl_k = mvcc.history_of(b"k").await.unwrap();
        let tl_kk = mvcc.history_of(b"kk").await.unwrap();

        assert_eq!(tl_k.len(), 1, "\"k\" has one entry");
        assert_eq!(tl_k[0].value, Bytes::from("a"));
        assert_eq!(tl_kk.len(), 1, "\"kk\" has one entry");
        assert_eq!(tl_kk[0].value, Bytes::from("b"));
    }

    // ========================================================================
    // T4-purge — purge_below_ts
    // ========================================================================

    /// Helper: count this key's archived history versions (the entries
    /// `history_of` returns minus the current main entry). Used to
    /// assert post-purge state.
    async fn archived_count(mvcc: &MvccStore, key: &[u8]) -> usize {
        let timeline = mvcc.history_of(key).await.unwrap();
        // history_of includes the current main entry when the key is
        // live. Archived versions = total − (1 if current live else 0).
        let cur_v = mvcc.current_version(key);
        if cur_v > 0 {
            timeline.len().saturating_sub(1)
        } else {
            timeline.len()
        }
    }

    /// Core case: a key written three times (v1, v2 archived; v3
    /// current). Purging with a cutoff that falls BETWEEN v1's and
    /// v2's commit ts reclaims v1 only — v2 (newer than cutoff) and
    /// v3 (current, in main) survive.
    #[tokio::test]
    async fn purge_below_ts_reclaims_only_older_than_cutoff() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // v1 @ ts=1_000, v2 @ ts=2_000, v3 @ ts=3_000 (all same key).
        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();
        mvcc.set_test_now(3_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
            .await
            .unwrap();

        // Before purge: two archived versions (v1, v2).
        assert_eq!(archived_count(&mvcc, b"k").await, 2);

        // Cutoff at 1_500: v1 (ts=1_000) is older, v2 (ts=2_000) is newer.
        let deleted = mvcc.purge_below_ts(1_500).await.unwrap();
        assert_eq!(deleted, 1, "only v1 is older than the cutoff");

        // v2 survives; v1 is gone.
        let timeline = mvcc.history_of(b"k").await.unwrap();
        let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![2, 3], "v2 (archived) + v3 (current) remain");
        assert!(!versions.contains(&1), "v1 must be purged");

        mvcc.set_test_now(0);
    }

    /// Sacred floor: with a live snapshot pinning `min_alive`, purge
    /// does NOT reclaim any snapshot-protected version — even one
    /// whose ts is older than the cutoff.
    #[tokio::test]
    async fn purge_below_ts_respects_live_snapshot_floor() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // v1 @ ts=1_000.
        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        // Open a snapshot NOW (at version 1) → min_alive = 1. Every
        // subsequent version is >= min_alive, hence sacred.
        let _guard = gate.open_snapshot().await;

        // v2 @ ts=2_000 archives v1 into history.
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();

        // Cutoff far in the future — would reclaim v1 by ts alone.
        let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
        assert_eq!(
            deleted, 0,
            "live snapshot pins every version >= min_alive; nothing reclaimable"
        );

        // Both archived v1 and current v2 survive.
        let timeline = mvcc.history_of(b"k").await.unwrap();
        let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![1, 2], "snapshot floor protects v1");

        mvcc.set_test_now(0);
    }

    /// Unknown-ts version: a version whose ts-key is missing is NEVER
    /// purged (can't prove it's old enough).
    #[tokio::test]
    async fn purge_below_ts_keeps_unknown_ts_versions() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // v1, v2 (history archives v1; both get ts-keys via record_ts).
        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();

        // Surgically remove v1's ts-key so its age becomes unknown.
        let v1 = mvcc.version_of(b"k") - 1; // v1 is one below the current version
        let _ = mvcc.history_store().remove(ts_key(v1)).await;

        // Even an aggressive cutoff can't reclaim v1 — unknown age.
        let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
        assert_eq!(
            deleted, 0,
            "unknown-ts version must be kept (can't prove it's old)"
        );

        mvcc.set_test_now(0);
    }

    /// Anchor protection: with two archived versions below min_alive,
    /// the non-anchor (older) one is reclaimed but the anchor (newest
    /// below min_alive) survives even though its ts is older than the
    /// cutoff.
    #[tokio::test]
    async fn purge_below_ts_keeps_anchor_reclaims_older() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        // v1 @ ts=1_000, v2 @ ts=2_000, v3 @ ts=3_000.
        // After three writes: history = {v1, v2}, main = v3,
        // min_alive = last_committed = 3 (no snapshot).
        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();
        mvcc.set_test_now(3_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
            .await
            .unwrap();

        // Cutoff far in the future: both v1 and v2 are older. v1 is
        // reclaimed, but v2 — the anchor (largest < min_alive=3) —
        // survives.
        let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
        assert_eq!(deleted, 1, "v1 reclaimed, v2 kept as anchor");

        let timeline = mvcc.history_of(b"k").await.unwrap();
        let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
        assert_eq!(versions, vec![2, 3], "anchor v2 + current v3 remain");

        mvcc.set_test_now(0);
    }

    /// An empty / future cutoff reclaims nothing.
    #[tokio::test]
    async fn purge_below_ts_zero_cutoff_reclaims_nothing() {
        let mvcc = make_mvcc();
        mvcc.set_retention(Retention::keep_history()).unwrap();

        mvcc.set_test_now(1_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_test_now(2_000);
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
            .await
            .unwrap();

        // cutoff = 0: no version has ts < 0.
        let deleted = mvcc.purge_below_ts(0).await.unwrap();
        assert_eq!(deleted, 0);

        mvcc.set_test_now(0);
    }
}
