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

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use scc::HashMap as SccHashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;

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

/// Per-store history-retention mode (T1b.1).
///
/// `CurrentOnly` (the default) bounds `history` to ~current by eagerly
/// reclaiming superseded versions that no live snapshot needs, via the
/// existing `gc_below(min_alive)` path invoked on every write. `KeepHistory`
/// retains all versions (no eager vacuum here — bounded by T1b.2 knobs later).
///
/// Stored as a lock-free `AtomicU8` on `MvccStore` (no `std::sync::Mutex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetentionMode {
    /// Default: keep only the current version + versions pinned by live
    /// snapshots. Eager vacuum runs after every superseding write.
    #[default]
    CurrentOnly,
    /// Opt-in: retain all history (no eager vacuum). Bounded by T1b.2.
    KeepHistory,
}

impl RetentionMode {
    const CURRENT_ONLY: u8 = 0;
    const KEEP_HISTORY: u8 = 1;

    fn to_u8(self) -> u8 {
        match self {
            RetentionMode::CurrentOnly => Self::CURRENT_ONLY,
            RetentionMode::KeepHistory => Self::KEEP_HISTORY,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            Self::KEEP_HISTORY => RetentionMode::KeepHistory,
            _ => RetentionMode::CurrentOnly,
        }
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
    /// T1b.1: history-retention mode (lock-free `AtomicU8`). Defaults to
    /// `CurrentOnly` (eager vacuum). Set via [`Self::set_retention`].
    retention: AtomicU8,
}

impl MvccStore {
    /// Create a new MVCC store from two backing stores and a gate.
    ///
    /// Defaults to [`RetentionMode::CurrentOnly`] (eager vacuum). Use
    /// [`Self::set_retention`] to opt into [`RetentionMode::KeepHistory`].
    pub fn new(main: Arc<dyn Store>, history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self {
        Self {
            main,
            history,
            gate,
            cells: SccHashMap::new(),
            locks: SccHashMap::new(),
            retention: AtomicU8::new(RetentionMode::CurrentOnly.to_u8()),
        }
    }

    /// Set the history-retention mode (lock-free, no mutex).
    pub fn set_retention(&self, mode: RetentionMode) {
        self.retention.store(mode.to_u8(), Ordering::Release);
    }

    /// Read the current history-retention mode.
    fn retention(&self) -> RetentionMode {
        RetentionMode::from_u8(self.retention.load(Ordering::Acquire))
    }

    /// T1b.1: eager vacuum for [`RetentionMode::CurrentOnly`]. Reclaims
    /// superseded history versions that no live snapshot needs by calling the
    /// existing safe [`gc_below`](Self::gc_below) with `gate.min_alive()`.
    /// No-op for [`RetentionMode::KeepHistory`]. Errors are swallowed
    /// (best-effort reclaim — a failure here must NOT fail the write that
    /// triggered it; the next write retries).
    async fn maybe_eager_vacuum(&self) {
        if self.retention() != RetentionMode::CurrentOnly {
            return;
        }
        // Correctness comes entirely from gc_below's `min_alive` floor, NOT
        // from the trigger. We pass min_alive so gc_below self-limits to
        // versions strictly below the oldest live snapshot (or last_committed
        // when none are open). The call is best-effort: a GC error is
        // swallowed — it must not fail the write.
        let _ = self.gc_below(self.gate.min_alive()).await;
    }

    // ========================================================================
    // Versioning substrate (future WriteStrategy seam — see
    // docs/roadmap/MVCC_CELL.md §7).
    //
    // This region groups the durable-versioned-KV operations over the
    // `main`/`history` stores: the write/delete paths, the snapshot-read
    // resolver, and the committed-ops applier. R1 extracts three private
    // helpers (`publish_cell`, `archive_prior`, `resolve_read`) that name
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

    /// Archive the current `main` value of `key` into `history` under its
    /// current version, if it exists. No-op if the key is absent. Used by the
    /// snapshot-active (slow) single-key write/delete paths.
    async fn archive_prior(&self, key: &Bytes) -> DbResult<()> {
        match self.main.get(key.clone()).await {
            Ok(old) => {
                let cur_v = self.current_version(key);
                let h_key = encode_version_key(key, cur_v);
                self.history.set(h_key, old).await?;
                Ok(())
            }
            Err(DbError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Resolve a versioned read of `key` visible at `snapshot_version`, given
    /// its current cached version `cur_v`. Fast path (`cur_v <= snapshot`):
    /// read `main`; slow path: range-scan `history` for the newest version
    /// `<= snapshot`. Behaviour-identical to the inline routing in `get_at`.
    async fn resolve_read(
        &self,
        key: &[u8],
        snapshot_version: u64,
        cur_v: u64,
    ) -> DbResult<Option<Bytes>> {
        if cur_v <= snapshot_version {
            return match self.main.get(Bytes::copy_from_slice(key)).await {
                Ok(v) => Ok(Some(v)),
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
        // T1a: always archive — the prior version must live in `history`
        // before the new value lands in `main`, so any snapshot (including
        // one opening mid-write) finds it.
        self.archive_prior(&key).await?;
        // Bump-first: assign version, update cell (both version and hwm), then
        // perform the physical write. CRIT-2: `publish_cell` uses entry_async
        // (modify-or-insert) so repeated writes to the same key advance the
        // cached version monotonically.
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v, true).await;
        self.main.set(key, value).await?;
        // Advance the reader-visible floor so a tx/snapshot opened AFTER this
        // write sees it: `publish_committed_max` is a monotonic fetch_max
        // (lock-free, safe off `commit_lock`, never moves the floor backwards).
        self.gate.publish_committed_max(new_v);
        // T1b.1: eager vacuum (CurrentOnly default) reclaims superseded
        // history versions no live snapshot needs.
        self.maybe_eager_vacuum().await;
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

        // T1a (always-archive): Phase 1 — per-key archive pre-reads. The
        // prior value of EVERY key must be archived to `history` before
        // the batch overwrites `main`, so a snapshot opening at any time
        // (mid-batch included) finds the prior version. Like
        // `apply_committed_ops`, the old-value read can't be batched (it
        // depends on each key's current main value).
        let mut history_ops: Vec<KvOp> = Vec::new();
        for (key, _value) in &items {
            match self.main.get(key.clone()).await {
                Ok(old) => {
                    let cur_v = self.current_version(key);
                    let h_key = encode_version_key(key, cur_v);
                    history_ops.push(KvOp::Set(h_key, old));
                }
                Err(DbError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

        // Phase 2: one batched write to history.
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 3 (bump-first): assign a fresh version per key and update the
        // cell (both version and hwm) BEFORE the physical main write.
        // CRIT-2: `publish_cell` uses entry_async modify-or-insert so the
        // cached version advances monotonically.
        let mut max_v = 0u64;
        for (key, _value) in &items {
            let new_v = self.gate.assign_next_version();
            self.publish_cell(key.clone(), new_v, true).await;
            max_v = new_v;
        }

        // Phase 4: one batched write to main.
        let main_ops: Vec<KvOp> = items.into_iter().map(|(k, v)| KvOp::Set(k, v)).collect();
        self.main.transact(main_ops).await?;

        // Advance the reader-visible floor to the batch's max version so a
        // tx/snapshot opened AFTER the batch sees every record in it.
        // `publish_committed_max` is monotonic (fetch_max) and safe off-lock.
        if max_v > 0 {
            self.gate.publish_committed_max(max_v);
        }
        // T1b.1: eager vacuum (CurrentOnly default) reclaims superseded
        // history versions no live snapshot needs.
        self.maybe_eager_vacuum().await;
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
        // T1a: always archive — the prior value must live in `history`
        // before the key is removed from `main`, so any snapshot (including
        // one opening mid-delete) finds it.
        self.archive_prior(&key).await?;
        // Bump-first: assign version, update cell (version+hwm), then remove.
        // CRIT-2: `publish_cell` uses entry_async modify-or-insert so the
        // cached version advances monotonically.
        let new_v = self.gate.assign_next_version();
        self.publish_cell(key.clone(), new_v, true).await;
        // Propagate a backend I/O failure instead of swallowing it — a
        // dropped error here would let the caller see Ok() while the row is
        // still live in main (the delete silently never happened).
        self.main.remove(key).await?;
        // Advance the reader-visible floor so a tx/snapshot opened AFTER this
        // delete sees the post-delete state: `publish_committed_max` is a
        // monotonic fetch_max (lock-free, safe off `commit_lock`).
        self.gate.publish_committed_max(new_v);
        // T1b.1: eager vacuum (CurrentOnly default) reclaims superseded
        // history versions no live snapshot needs.
        self.maybe_eager_vacuum().await;
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
    /// sequences (Phase 1 pre-reads, Phase 2 batched history transact,
    /// Phase 3 batched main transact, Phase 4 version_cache updates).
    /// Cancellation mid-batch leaves some phases applied, others not.
    /// Recovery relies on WAL replay (commit_tx invariant).
    pub async fn apply_committed_ops(&self, ops: Vec<KvOp>, commit_version: u64) -> DbResult<()> {
        // HIGH-2: re-sample `snapshots_active` per-op inside the loop
        // rather than once at function entry. A snapshot opening
        // mid-apply must still see archived old values for ops that
        // run after the snapshot's gate-insertion happens. Sampling
        // per-op keeps the existing "no snapshots ⇒ no archive"
        // contract without races for late-arriving snapshots.
        //
        // HIGH-3: batch the physical writes through `Store::transact`.
        // Per-op `set`/`remove` collapses to a single atomic write-tx
        // on backends that override `transact` (redb, sled, fjall,
        // persy, nebari, canopy) — one fsync instead of N.

        // Phase 1: pre-read old values for archive. Per-op snapshot
        // check + per-op `main.get` (can't batch — depends on per-key
        // old value).
        let mut history_ops: Vec<KvOp> = Vec::new();
        for op in &ops {
            // Re-sample per iteration so a snapshot that opens
            // mid-loop is honoured by the remaining ops.
            if self.gate.active_snapshots_empty() {
                continue;
            }
            let key = match op {
                KvOp::Set(k, _) => k,
                KvOp::Remove(k) => k,
            };
            match self.main.get(key.clone()).await {
                Ok(old) => {
                    let cur_v = self.current_version(key);
                    let h_key = encode_version_key(key, cur_v);
                    history_ops.push(KvOp::Set(h_key, old));
                }
                Err(DbError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

        // Phase 2: one batched write to history.
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 3: one batched write to main.
        self.main.transact(ops.clone()).await?;

        // Phase 4: update the in-memory cell for every touched key.
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
    pub async fn gc_below(&self, min_version: u64) -> DbResult<usize> {
        use crate::version_codec::decode_version_key;

        // Phase 1: scan all history entries, group by original key.
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
        // delete the rest.
        let mut deleted = 0usize;
        for (_orig_key, mut entries) in per_key {
            if entries.len() <= 1 {
                // Only one entry — it's the anchor, keep it.
                continue;
            }
            entries.sort_by_key(|(v, _)| *v);
            // Keep the last (highest version < min_version), delete the rest.
            let to_delete = &entries[..entries.len() - 1];
            for (_, phys_key) in to_delete {
                let _ = self.history.remove(phys_key.clone()).await;
                deleted += 1;
            }
        }

        // Phase 3: prune the in-memory version cache (III.3). Uses the
        // gate's `min_alive()`, independent of the `min_version` history
        // threshold (see `prune_version_cache` for why).
        self.prune_version_cache().await;

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
        Ok(latest)
    }
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

        // history is empty
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let batch = batch.unwrap();
            assert!(batch.is_empty(), "history should be empty");
        }
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

        // history contains v1 under a version key
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                let (orig_key, _ver) = crate::version_codec::decode_version_key(&hk).unwrap();
                assert_eq!(orig_key, &b"k1"[..]);
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1");
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

        // history contains v1
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1 after delete");
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

    #[tokio::test]
    async fn zero_overhead_no_snapshots() {
        let mvcc = make_mvcc();
        for i in 0..100u32 {
            let key = Bytes::copy_from_slice(&i.to_be_bytes());
            mvcc.set_versioned(key, Bytes::from("val")).await.unwrap();
        }

        // history should be empty — no snapshots were open
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 0,
            "history should have zero records without snapshots"
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

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("old"));
                found = true;
            }
        }
        assert!(found, "old value must be archived in history");
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

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("before"));
                found = true;
            }
        }
        assert!(found);
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
        let stream = mvcc.history_store().iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
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

        // History should have 4 entries (v1..v4 archived when v2..v5 overwrote)
        let count_before = count_history_entries(&mvcc).await;
        assert_eq!(count_before, 4, "should have 4 history entries");

        // GC below version 3: versions < 3 in history are version_at[0] and version_at[1].
        // Anchor = highest < 3, older one deleted.
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

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 0,
            "history should stay empty without active snapshots"
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

        // History must contain "old" because the snapshot needs it.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                if hv.as_ref() == b"old" {
                    found = true;
                }
            }
        }
        assert!(found, "old value archived for active snapshot");
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
        mvcc.set_retention(RetentionMode::KeepHistory);

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

        // T1a: brand-new keys have no prior to archive → history stays empty.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut hist = 0usize;
        while let Some(batch) = stream.next().await {
            hist += batch.unwrap().len();
        }
        assert_eq!(
            hist, 0,
            "T1a always-archive: brand-new keys archive nothing"
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

        // The pre-existing old0 was archived to history.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found_old = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                if hv.as_ref() == b"old0" {
                    found_old = true;
                }
            }
        }
        assert!(
            found_old,
            "pre-existing value must be archived under snapshot"
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

        // No history entries — nothing was overwritten.
        let count = count_history_entries(&mvcc).await;
        assert_eq!(count, 0, "no history for a brand-new key");
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
        }

        impl FailingStore {
            pub fn new() -> Self {
                Self {
                    inner: InMemoryStore::new(),
                    fail_get: AtomicBool::new(false),
                    fail_remove: AtomicBool::new(false),
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

    /// Regression test for fix #2: `set_versioned` propagates
    /// non-NotFound `get()` errors during the archive pre-read.
    ///
    /// **Pre-fix behaviour:** `if let Ok(old) = self.main.get(key)`
    /// treated any `Err` (including genuine I/O failures) the same as
    /// `NotFound` — silently skipped archival and overwrote main →
    /// snapshot-isolation violation (a live snapshot would miss the old
    /// value that should have been archived).
    ///
    /// **Post-fix:** `match self.main.get(...) { ... Err(e) => return Err(e) }`
    /// propagates non-NotFound errors → caller sees `Err`, main is NOT
    /// overwritten, snapshot isolation is preserved.
    ///
    /// This test would FAIL on the pre-fix code because the old
    /// `if let Ok(old)` arm would fall through to the `set` on main,
    /// returning `Ok(())` while silently breaking snapshot isolation.
    #[tokio::test]
    async fn set_versioned_propagates_archive_read_error() {
        let gate = make_gate();
        let (mvcc, main) = make_failing_mvcc(gate.clone());

        // Seed a key so the archive pre-read path is taken (there IS
        // an existing value to archive).
        main.set(Bytes::from("k"), Bytes::from("old_val"))
            .await
            .unwrap();

        // Open a snapshot so set_versioned enters the archive path.
        let _guard = gate.open_snapshot().await;

        // Arm: the next `get` call will fail with a non-NotFound error.
        main.fail_get.store(true, Ordering::Relaxed);

        let result = mvcc
            .set_versioned(Bytes::from("k"), Bytes::from("new_val"))
            .await;
        assert!(
            result.is_err(),
            "set_versioned must propagate archive pre-read I/O error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("injected"),
            "error should be the injected fault, got: {err_msg}"
        );

        // Verify main was NOT overwritten — the old value survives
        // (snapshot-isolation guarantee). Disarm to read.
        main.fail_get.store(false, Ordering::Relaxed);
        let val = mvcc.main.get(Bytes::from("k")).await.unwrap();
        assert_eq!(
            val,
            Bytes::from("old_val"),
            "main must NOT be overwritten when archive pre-read fails"
        );
    }

    /// Regression test for fix #2 (delete_versioned variant):
    /// `delete_versioned` propagates non-NotFound `get()` errors from
    /// the archive pre-read block.
    ///
    /// **Pre-fix behaviour:** same `if let Ok(old)` pattern as
    /// `set_versioned` — a genuine I/O error from `get` was silently
    /// treated as "nothing to archive", then `remove` proceeded and
    /// returned `Ok`, losing the old value for any live snapshot.
    ///
    /// **Post-fix:** the explicit `Err(e) => return Err(e)` arm stops
    /// the operation before the remove, preserving snapshot isolation.
    ///
    /// This test would FAIL on the pre-fix code because the old
    /// fallthrough would skip archival and still call `remove`,
    /// returning `Ok(())`.
    #[tokio::test]
    async fn delete_versioned_propagates_archive_read_error() {
        let gate = make_gate();
        let (mvcc, main) = make_failing_mvcc(gate.clone());

        // Seed a key.
        main.set(Bytes::from("k"), Bytes::from("val"))
            .await
            .unwrap();

        // Open a snapshot.
        let _guard = gate.open_snapshot().await;

        // Arm: `get` fails (non-NotFound).
        main.fail_get.store(true, Ordering::Relaxed);

        let result = mvcc.delete_versioned(Bytes::from("k")).await;
        assert!(
            result.is_err(),
            "delete_versioned must propagate archive pre-read I/O error"
        );
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("injected"),
            "error should be the injected fault, got: {err_msg}"
        );

        // Main must still have the key (remove was never called).
        main.fail_get.store(false, Ordering::Relaxed);
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
        mvcc.set_retention(RetentionMode::KeepHistory);

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
    // T1b.1 — eager vacuum (CurrentOnly default) tests.
    // ================================================================

    /// CurrentOnly store, no snapshots: write the same key 5 times; eager
    /// vacuum reclaims superseded history on every write, so `history` holds
    /// ~0 old versions afterward while `get_at` at the floor still returns
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

        // Eager vacuum ran after each write (CurrentOnly default, no
        // snapshots → min_alive == last_committed). gc_below keeps the
        // anchor (latest < min_alive), so at most 1 history entry survives.
        let hist = count_history_entries(&mvcc).await;
        assert!(
            hist <= 1,
            "CurrentOnly eager vacuum should leave ≤1 history entry (anchor), got {hist}"
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
        mvcc.set_retention(RetentionMode::KeepHistory);

        let key = Bytes::from("keep_key");
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // KeepHistory: no eager vacuum — all 4 prior versions remain.
        let hist = count_history_entries(&mvcc).await;
        assert_eq!(
            hist, 4,
            "KeepHistory must retain all prior versions (no eager vacuum), got {hist}"
        );

        // Current value still correct.
        let last_committed = mvcc.gate.last_committed();
        let result = mvcc.get_at(&key, last_committed).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v5")));
    }
}
