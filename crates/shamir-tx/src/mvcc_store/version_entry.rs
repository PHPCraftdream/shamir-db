use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
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

use shamir_storage::error::DbResult;

/// C2 streaming group-by state for `current_stream`. The log is key-major,
/// version-ascending; this state tracks the last (highest) version per key
/// and emits the current value once all versions of a key have been seen.
///
/// R3 — MVCC pre-floor: `floor` caps version visibility. Versions above
/// the floor are skipped in the group-by (inlined comparison, no second pass).
pub(super) enum StreamingGroupByState {
    Start {
        history: Arc<dyn Store>,
        batch_size: usize,
        /// R3: committed floor — versions > floor are invisible.
        floor: u64,
    },
    Streaming {
        stream: BoxedLogStream,
        batch_size: usize,
        /// R3: committed floor — versions > floor are invisible.
        floor: u64,
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
    pub(super) async fn drain_and_emit(
        self,
    ) -> Option<(Result<Vec<(Bytes, Bytes)>, DbError>, Self)> {
        // Pull the streaming fields out; re-pack on return.
        let (stream, batch_size, floor, mut cur_key, mut last_val, mut out_batch) = match self {
            StreamingGroupByState::Streaming {
                stream,
                batch_size,
                floor,
                cur_key,
                last_val,
                out_batch,
            } => (stream, batch_size, floor, cur_key, last_val, out_batch),
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
                                floor,
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
                            floor,
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
