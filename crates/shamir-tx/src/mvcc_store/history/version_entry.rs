use super::super::MvccStore;

use std::sync::Arc;

use crate::version_codec::encode_version_key;
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use shamir_collections::TMap;
use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;

use crate::version_codec::decode_version_key;

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
    /// (T1c, stored as a `ts_key(version)` entry in the history store,
    /// written alongside the history transact). `None`
    /// when no ts entry exists for this version.
    pub ts_millis: Option<u64>,
}

/// One `current_stream` output batch (raw `(key, value)` pairs).
pub(crate) type LogBatch = DbResult<Vec<(Bytes, Bytes)>>;
/// A boxed, `Unpin`, `Send` stream of log batches — the inner stream the
/// group-by drains.
pub(crate) type BoxedLogStream = std::pin::Pin<Box<dyn futures::Stream<Item = LogBatch> + Send>>;

/// P1b: per-key overlay winner `≤ floor`, materialised at stream-open.
/// `key → (version, value)`; tombstone value is empty `Bytes`. The merge
/// consumes entries from this map as history keys are flushed, and emits the
/// leftover (overlay-only) keys at stream end.
pub(crate) type OverlayWinners = TMap<Bytes, (u64, Bytes)>;

use shamir_storage::error::DbResult;

/// One step of the `unfold` driving `current_stream`: an emitted batch (or
/// error) paired with the next [`StreamingGroupByState`], or `None` when the
/// stream is exhausted.
type GroupByStep = Option<(Result<Vec<(Bytes, Bytes)>, DbError>, StreamingGroupByState)>;

/// C2 streaming group-by state for `current_stream`. The log is key-major,
/// version-ascending; this state tracks the last (highest) version per key
/// and emits the current value once all versions of a key have been seen.
///
/// R3 — MVCC pre-floor: `floor` caps version visibility. Versions above
/// the floor are skipped in the group-by (inlined comparison, no second pass).
///
/// P1b — overlay merge: `overlay` holds the per-key overlay winner `≤ floor`.
/// As each history key is flushed, the overlay winner (if any) for that key is
/// merged in (newest version wins; tombstone suppresses). Overlay-only keys
/// (never seen in history) are emitted after the history stream drains.
pub(crate) enum StreamingGroupByState {
    Start {
        history: Arc<dyn Store>,
        batch_size: usize,
        /// R3: committed floor — versions > floor are invisible.
        floor: u64,
        /// P1b: overlay winners `≤ floor` to merge into the stream.
        overlay: OverlayWinners,
        /// CR-B1: when `true`, a tombstoned winner is emitted (empty value)
        /// instead of being suppressed. Used ONLY by
        /// `current_stream_with_tombstones` — `current_stream` itself always
        /// passes `false`, preserving its existing CURRENT-state semantics
        /// for every other caller.
        include_tombstones: bool,
    },
    Streaming {
        stream: BoxedLogStream,
        batch_size: usize,
        /// R3: committed floor — versions > floor are invisible.
        floor: u64,
        /// P1b: overlay winners not yet consumed by a matching history key.
        overlay: OverlayWinners,
        /// `(original_key_bytes, last_value)` — the group being accumulated.
        cur_key: Option<Bytes>,
        last_val: Option<Bytes>,
        /// Highest history version accumulated for `cur_key` (for the
        /// overlay-vs-history version comparison on flush).
        last_ver: u64,
        /// Output batch being built.
        out_batch: Vec<(Bytes, Bytes)>,
        /// CR-B1: see `Start::include_tombstones`.
        include_tombstones: bool,
    },
    /// P1b: history stream exhausted — drain the leftover overlay-only keys
    /// (those never matched by a history key) in batches.
    DrainOverlay {
        batch_size: usize,
        /// Leftover overlay winners as a stack (popped to emit).
        leftover: Vec<(Bytes, (u64, Bytes))>,
        /// CR-B1: see `Start::include_tombstones`.
        include_tombstones: bool,
    },
    Done,
}

impl StreamingGroupByState {
    /// P1b: merge a flushed history group `(key, hist_ver, hist_val)` with its
    /// overlay winner (if present), pushing the survivor onto `out_batch`.
    /// Removes the consumed key from `overlay`. Tombstone (empty) survivors
    /// are suppressed UNLESS `include_tombstones` is `true` (CR-B1:
    /// `current_stream_with_tombstones`'s enumeration — the winner is emitted
    /// regardless, so a caller like `read_as_of` still gets a `get_at`
    /// attempt for a since-deleted key).
    fn flush_group(
        overlay: &mut OverlayWinners,
        out_batch: &mut Vec<(Bytes, Bytes)>,
        key: Bytes,
        hist_ver: u64,
        hist_val: Bytes,
        include_tombstones: bool,
    ) {
        // shift_remove keeps remaining keys for the overlay-only drain phase
        // (order-preserving but O(N) per removal — the overlay is the bounded
        // small side, see TMap docs).
        let winner = match overlay.shift_remove(&key) {
            // Overlay version strictly newer than history → overlay wins.
            // Versions are globally unique so equality never occurs; if it did,
            // the values are identical (WAL determinism), so either is correct.
            Some((ov_ver, ov_val)) if ov_ver > hist_ver => ov_val,
            // History newer (or overlay absent for this key) → history wins.
            _ => hist_val,
        };
        if include_tombstones || !winner.is_empty() {
            out_batch.push((key, winner));
        }
    }

    /// Drain log batches through the group-by, emitting whenever the
    /// output batch reaches `batch_size` or the stream ends.
    /// Returns `Option<(Item, NextState)>` for `futures::stream::unfold`.
    pub(crate) async fn drain_and_emit(self) -> GroupByStep {
        // Pull the streaming fields out; re-pack on return.
        let (
            stream,
            batch_size,
            floor,
            mut overlay,
            mut cur_key,
            mut last_val,
            mut last_ver,
            mut out_batch,
            include_tombstones,
        ) = match self {
            StreamingGroupByState::Streaming {
                stream,
                batch_size,
                floor,
                overlay,
                cur_key,
                last_val,
                last_ver,
                out_batch,
                include_tombstones,
            } => (
                stream,
                batch_size,
                floor,
                overlay,
                cur_key,
                last_val,
                last_ver,
                out_batch,
                include_tombstones,
            ),
            // P1b: overlay-only drain phase.
            StreamingGroupByState::DrainOverlay {
                batch_size,
                leftover,
                include_tombstones,
            } => return Self::drain_overlay(batch_size, leftover, include_tombstones),
            _ => return None,
        };
        let mut stream = stream;
        loop {
            match stream.next().await {
                Some(Ok(batch)) => {
                    for (phys_key, val) in batch {
                        if let Some((orig, v)) = decode_version_key(&phys_key) {
                            // R3 + I3: skip versions above the committed floor.
                            // No floor==0 exemption: versions start at 1, so a
                            // floor of 0 means nothing is published yet —
                            // materialized-but-unpublished rows (follower
                            // receipt materialize→publish window) must stay
                            // invisible to the stream.
                            if v > floor {
                                continue;
                            }
                            let orig_bytes = Bytes::copy_from_slice(orig);
                            if cur_key.as_deref() != Some(orig) {
                                // Flush previous group (overlay-merged).
                                if let (Some(ck), Some(lv)) = (cur_key.take(), last_val.take()) {
                                    Self::flush_group(
                                        &mut overlay,
                                        &mut out_batch,
                                        ck,
                                        last_ver,
                                        lv,
                                        include_tombstones,
                                    );
                                }
                                cur_key = Some(orig_bytes);
                            }
                            last_val = Some(val);
                            last_ver = v;
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
                                floor,
                                overlay,
                                cur_key,
                                last_val,
                                last_ver,
                                out_batch,
                                include_tombstones,
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
                            floor,
                            overlay,
                            cur_key,
                            last_val,
                            last_ver,
                            out_batch,
                            include_tombstones,
                        },
                    ))
                }
                None => {
                    // History stream ended — flush final history group.
                    if let (Some(ck), Some(lv)) = (cur_key.take(), last_val.take()) {
                        Self::flush_group(
                            &mut overlay,
                            &mut out_batch,
                            ck,
                            last_ver,
                            lv,
                            include_tombstones,
                        );
                    }
                    // P1b: leftover overlay keys never matched a history key →
                    // overlay-only. Emit non-tombstone ones in the drain phase
                    // (unless `include_tombstones`).
                    let leftover: Vec<(Bytes, (u64, Bytes))> = overlay.drain(..).collect();
                    if out_batch.is_empty() {
                        // Nothing from history this round — go straight to the
                        // overlay drain (which may itself yield None when empty).
                        return Self::drain_overlay(batch_size, leftover, include_tombstones);
                    }
                    let emit: Vec<_> = out_batch;
                    if leftover.is_empty() {
                        return Some((Ok(emit), StreamingGroupByState::Done));
                    }
                    return Some((
                        Ok(emit),
                        StreamingGroupByState::DrainOverlay {
                            batch_size,
                            leftover,
                            include_tombstones,
                        },
                    ));
                }
            }
        }
    }

    /// P1b: emit a batch of leftover overlay-only keys, returning the next
    /// `DrainOverlay`/`Done` state. Tombstone (empty) leftovers are skipped
    /// UNLESS `include_tombstones` is `true` (CR-B1). Yields `None` when no
    /// leftovers remain to emit.
    fn drain_overlay(
        batch_size: usize,
        mut leftover: Vec<(Bytes, (u64, Bytes))>,
        include_tombstones: bool,
    ) -> GroupByStep {
        let mut out: Vec<(Bytes, Bytes)> = Vec::new();
        while let Some((key, (_ver, val))) = leftover.pop() {
            if include_tombstones || !val.is_empty() {
                out.push((key, val));
            }
            if out.len() >= batch_size.max(1) {
                return Some((
                    Ok(out),
                    StreamingGroupByState::DrainOverlay {
                        batch_size,
                        leftover,
                        include_tombstones,
                    },
                ));
            }
        }
        if out.is_empty() {
            None
        } else {
            Some((Ok(out), StreamingGroupByState::Done))
        }
    }
}

impl MvccStore {
    /// C2: flip get_current to the single version log. Returns `None` for
    /// tombstones (empty value) and absent keys. Cold-start: when the cell is
    /// absent (`current_version == 0`), scans the log for the latest version
    /// via `seek_latest_published`, bounded at the published floor ONLY while
    /// the receipt's materialize→publish window is open (see the cold-path
    /// comment in [`get_current_bytes`](Self::get_current_bytes)).
    ///
    /// R3 — MVCC pre-floor: reads are capped at the committed floor
    /// (`gate.last_committed()`). Versions above the floor are not yet
    /// visible. Under sequential commits this is always equal to the
    /// cell's latest version, so semantics are unchanged today; once P2b
    /// lands (out-of-order materialize) this prevents observing
    /// uncommitted versions.
    ///
    /// Accepts an owned [`RecordKey`] for the keyed call convention.
    /// Delegates to [`get_current_bytes`](Self::get_current_bytes) — prefer
    /// that borrowing method on hot paths where only a `&[u8]` is in hand.
    pub async fn get_current(&self, key: RecordKey) -> DbResult<Option<Bytes>> {
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
            // Cold-start: the cell is absent. The history scan resolves the
            // key's latest version; an overlay-only key (committed but not
            // yet drained) is invisible to that scan. P1b: also consult the
            // overlay so an overlay-only key is found.
            //
            // Receipt-native mode caps history only while allocated versions
            // are ahead of the published floor; legacy mode keeps the
            // unbounded cold seek.
            let watermark = self.gate.version_allocation_high_water_mark();
            let bounded = self.gate.receipt_native_visibility_enabled() && watermark > floor;
            let hist_cap = if bounded { floor } else { u64::MAX };
            let ov_cap = if bounded || floor > 0 {
                floor
            } else {
                u64::MAX
            };
            let hist_v = self.seek_latest_published(key, hist_cap).await?;
            let ov_v = self.overlay.newest_visible(key, ov_cap).map(|(ver, _)| ver);
            // Cold path: `seed_version` takes an owned `RecordKey`. Build it
            // inline from the borrowed slice (alloc-free for the ≤23-byte
            // `RecordId` shape). This only happens on cold-start (cell absent),
            // rare on the hot read path.
            match (hist_v, ov_v) {
                (Some(h), Some(o)) => {
                    // Seed the cell from history's max (the durable anchor);
                    // the overlay value, if newer, wins in resolve_read below.
                    self.seed_version(RecordKey::from_slice(key), h).await;
                    h.max(o)
                }
                (Some(h), None) => {
                    self.seed_version(RecordKey::from_slice(key), h).await;
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
        // visibility restriction applied (master semantics; in the bounded
        // receipt window the scan caps below the floor anyway, so the extra
        // conjunct never changes the outcome there).
        if floor > 0 && v > floor {
            return self.get_at(key, floor).await;
        }
        // P1b: overlay-probe BEFORE the durable log for the exact `(key, v)`.
        // A hit (tombstone → None, value → Some) short-circuits; a miss falls
        // through to history (always so in P1b — overlay is empty).
        if let Some(val) = self.overlay.get(key, v) {
            return Ok(if val.is_empty() { None } else { Some(val) });
        }
        match self.history.get(encode_version_key(key, v).into()).await {
            Ok(val) if val.is_empty() => Ok(None),
            Ok(val) => Ok(Some(val)),
            // RV-TX-2CB455A7: the cell version is an upper bound — an exact
            // miss means the version was attempted and aborted. Fall back
            // below it instead of masking the prior committed value.
            Err(DbError::NotFound(_)) if v > 0 => self.resolve_read(key, floor.min(v - 1), 0).await,
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
        self.current_stream_impl(batch, false)
    }

    /// CR-B1 (#767): like [`current_stream`](Self::current_stream), but a
    /// tombstoned CURRENT winner is emitted (with an empty value) instead of
    /// being suppressed.
    ///
    /// Used ONLY by `TableManager::list_stream_with_tombstones`, in turn used
    /// ONLY by `read_as_of`'s enumeration. Every other caller of the
    /// tombstone-suppressing `current_stream` (`table_manager_sorted_index.rs`,
    /// `table_manager_replication.rs`, `migration/coordinator.rs`, the normal
    /// `Latest` read path) is unaffected — they keep calling `current_stream`
    /// unchanged.
    ///
    /// Why this is needed: a cursor's / `AsOf` read's pinned MVCC snapshot
    /// version can be OLDER than a concurrent DELETE that has since made a
    /// key's CURRENT winner a tombstone. `current_stream`'s enumeration
    /// suppresses that key entirely, so `read_as_of` never even attempts a
    /// `get_at(id, pinned_version)` call for it — the pre-delete value the
    /// pinned snapshot should still see silently vanishes. Emitting the
    /// tombstone here just means more ids get a `get_at` attempt in
    /// `read_as_of`'s loop; `get_at` itself already correctly resolves each
    /// id at the pinned version regardless of the key's CURRENT state.
    pub fn current_stream_with_tombstones(
        &self,
        batch: usize,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        self.current_stream_impl(batch, true)
    }

    /// Version-pinned stream: yield all `(key, value)` pairs visible at
    /// `at_version`. Tombstones (empty value) are suppressed — this is the
    /// correct default for Phase A backfill (online CREATE INDEX) where rows
    /// deleted at or before the pinned version must NOT produce a posting.
    ///
    /// Behaves like [`current_stream`](Self::current_stream) but pins the scan
    /// at an explicit version rather than `gate.last_committed()`. The pinned
    /// version is captured at stream-open time and used to filter both history
    /// and overlay entries (newest version ≤ at_version is visible).
    ///
    /// Emits in batches of `batch`.
    pub fn snapshot_stream(
        &self,
        batch: usize,
        at_version: u64,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        self.snapshot_stream_impl(batch, at_version, false)
    }

    /// Shared implementation behind [`current_stream`](Self::current_stream)
    /// and
    /// [`current_stream_with_tombstones`](Self::current_stream_with_tombstones).
    /// `include_tombstones` selects which of the two semantics
    /// `StreamingGroupByState`'s flush points apply — see
    /// `version_entry.rs`'s `flush_group`/`drain_overlay`.
    fn current_stream_impl(
        &self,
        batch: usize,
        include_tombstones: bool,
    ) -> impl futures::Stream<Item = DbResult<Vec<(Bytes, Bytes)>>> + Send {
        self.snapshot_stream_impl(batch, self.gate.last_committed(), include_tombstones)
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
        self.cells.read_sync(key, |_, c| c.version).unwrap_or(0)
    }

    /// TEST-ONLY: evict the key's in-memory cell so the next point read
    /// takes the cold path (`seek_latest_published`). Used by the
    /// cold-branch regression test to force the bounded scan even after a
    /// publish (which seeds the cell). Synchronous `retain_sync` per the
    /// #589 deadlock note in `prune_version_cache`; no production behavior.
    #[cfg(test)]
    pub(crate) fn evict_current_cell_for_test(&self, key: &[u8]) {
        self.cells.retain_sync(|k, _| k.as_slice() != key);
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
        self.cells.read_sync(key, |_, c| c.version)
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
            HistoryMiss { miss_idx: usize, cur_v: u64 },
            /// Version > floor — fall back to snapshot read at floor.
            FloorExceeded,
            /// Cold cell — fall back to per-key `get_current_bytes`.
            Cold,
        }

        let mut slots: Vec<Slot> = Vec::with_capacity(len);
        // `RecordKey` (the store `get_many` key type); `encode_version_key`
        // produces `Bytes`, converted byte-identically at each push.
        let mut miss_keys: Vec<RecordKey> = Vec::new();

        for key in keys {
            let cur_v = self.current_version(key);
            if cur_v == 0 {
                slots.push(Slot::Cold);
                continue;
            }
            // R3 floor-cap: version above the committed floor is not yet
            // visible — same rule as the point read. S5c-X PR6b-2 (cold-
            // start restore): master's `floor > 0` bootstrap exemption is
            // restored UNLESS the receipt window is open (watermark > floor
            // — a materialized-but-unpublished version may exist), in which
            // case the cap applies even at floor 0.
            if cur_v > floor
                && (floor > 0
                    || (self.gate.receipt_native_visibility_enabled()
                        && self.gate.version_allocation_high_water_mark() > floor))
            {
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
            miss_keys.push(vk.into());
            slots.push(Slot::HistoryMiss { miss_idx, cur_v });
        }

        // Phase 2: single batched history read for all warm misses.
        let miss_results = if miss_keys.is_empty() {
            Vec::new()
        } else {
            self.history.get_many(miss_keys).await?
        };

        // Ordered buffering preserves slot and error order. Cold reads re-probe
        // the cell and floor after the batch await to retain race semantics.
        futures::stream::iter(slots.into_iter().enumerate().map(|(i, slot)| {
            let miss_results = &miss_results;
            async move {
                match slot {
                    Slot::Resolved(val) => Ok(val),
                    Slot::HistoryMiss { miss_idx, cur_v } => match &miss_results[miss_idx] {
                        Some(val) if val.is_empty() => Ok(None),
                        Some(val) => Ok(Some(val.clone())),
                        None if cur_v > 0 => {
                            self.resolve_read(&keys[i], floor.min(cur_v - 1), 0).await
                        }
                        None => Ok(None),
                    },
                    Slot::FloorExceeded => self.get_at(&keys[i], floor).await,
                    Slot::Cold => self.get_current_bytes(&keys[i]).await,
                }
            }
        }))
        .buffered(16)
        .try_collect()
        .await
    }
}
