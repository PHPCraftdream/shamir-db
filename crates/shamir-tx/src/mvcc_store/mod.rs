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
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;

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
    pub(super) cells: SccHashMap<Bytes, RecordCell>,
    /// Level-3 pessimistic lock registry. Populated ONLY for keys locked by a
    /// `Pessimistic` tx; stays empty otherwise → zero overhead on the snapshot
    /// / serializable read/write hot paths. Each entry is an `Arc<KeyLock>`
    /// shared between concurrent requesters of the same key.
    pub(super) locks: SccHashMap<Bytes, Arc<KeyLockInner>>,
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
