use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_collections::TMap;
use shamir_storage::error::DbError;
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
    /// (T1c, recorded via [`MvccStore::record_ts`] / `ts_key`). `None`
    /// when no ts entry exists for this version.
    pub ts_millis: Option<u64>,
}

/// One `current_stream` output batch (raw `(key, value)` pairs).
pub(super) type LogBatch = DbResult<Vec<(Bytes, Bytes)>>;
/// A boxed, `Unpin`, `Send` stream of log batches — the inner stream the
/// group-by drains.
pub(super) type BoxedLogStream = std::pin::Pin<Box<dyn futures::Stream<Item = LogBatch> + Send>>;

/// P1b: per-key overlay winner `≤ floor`, materialised at stream-open.
/// `key → (version, value)`; tombstone value is empty `Bytes`. The merge
/// consumes entries from this map as history keys are flushed, and emits the
/// leftover (overlay-only) keys at stream end.
pub(super) type OverlayWinners = TMap<Bytes, (u64, Bytes)>;

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
pub(super) enum StreamingGroupByState {
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
        // shift_remove keeps remaining keys for the overlay-only drain phase.
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
    pub(super) async fn drain_and_emit(self) -> GroupByStep {
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
                            // R3: skip versions above the committed floor.
                            // Floor == 0 means bootstrap/recovery — no restriction.
                            if floor > 0 && v > floor {
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
