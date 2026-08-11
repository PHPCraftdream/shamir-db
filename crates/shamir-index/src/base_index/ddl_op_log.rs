//! DDL operation status log (#1015) — durable append-only store for DDL op states.
//!
//! The op-status log is keyed by a prefix + the raw 16 bytes of the op_id
//! and stores versioned `DdlOpStatus` structs. It lives in the same `info_store` that
//! tombstones use, but is semantically distinct:
//! - Tombstones are cleared on success and keyed by name (internal recovery only).
//! - Op-status records survive success and are keyed by a stable `op_id` (client-visible).
//!
//! This module provides the storage primitives for reading/writing op-status records,
//! plus the FIFO retention sweep (#1068) that keeps the log bounded. The actual op
//! lifecycle management (minting, state transitions, recovery writes) lives in the DDL
//! handlers and recovery functions.

use bytes::Bytes;
use futures::StreamExt;
use shamir_query_types::read::{DdlOpState, DdlOpStatus};
use shamir_storage::error::DbError;
use shamir_storage::types::{RecordKey, Store};
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Maximum number of terminal (Succeeded/Failed/SucceededViaCrashRecovery) records
/// to retain. When exceeded, records are evicted in FIFO order (oldest first).
///
/// This is a generous fixed cap for the first slice; retention tuning is deferred
/// (RFC §4 "defer to follow-ups").
const DDL_OP_LOG_CAP: usize = 10000;

/// Every Nth terminal-status write triggers a `maybe_evict_terminal_records`
/// sweep (#1068). Chosen so eviction stays O(1) amortized per write (a full
/// prefix scan every write would be O(N) per op → O(N^2) over the log's
/// lifetime) while still catching runaway growth well before the cap is
/// reached by a wide margin.
const DDL_OP_LOG_EVICTION_CHECK_INTERVAL: u64 = 100;

/// Process-wide count of terminal-status writes across every table's
/// `ddl_op:` keyspace, used purely to throttle how often
/// `maybe_evict_terminal_records` runs (see `write_op_status`).
///
/// This is process-wide rather than per-table/per-`info_store` because the
/// counter only decides "is it time to re-scan", not "how many records
/// exist" — an extra sweep triggered by a DIFFERENT table's writes is a
/// harmless no-op re-scan of an under-cap log, not a correctness issue.
/// Threading a per-table atomic through here would require adding a field
/// (and a constructor parameter) to `IndexManager`, `SortedIndexManager`
/// AND `TableManager` — three structs across two crates, all of which
/// call `write_op_status` against the same table's `info_store` — for a
/// value that is only ever compared against a throttle interval, never
/// read back. A single process-wide `AtomicU64` gets the same amortized-cost
/// behavior without that blast radius. Does not need to survive a restart:
/// `maybe_evict_terminal_records` is also run once at table-open time
/// (`TableManager::create`), so a counter reset across a crash just means
/// the next `DDL_OP_LOG_EVICTION_CHECK_INTERVAL` terminal writes (or the
/// next restart) re-triggers the sweep, not that eviction stops happening.
static DDL_OP_LOG_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Prefix for all DDL operation status keys (7 ASCII bytes: "ddl_op:").
const DDL_OP_KEY_PREFIX: &[u8] = b"ddl_op:";

/// Current version byte for the DdlOpStatus serialization format.
/// When we change the DdlOpStatus struct in a breaking way, we bump this
/// and add migration logic in `read_op_status`.
const DDL_OP_LOG_VERSION: u8 = 0x01;

/// Builds the storage key for a given operation ID.
/// The key is the prefix "ddl_op:" (7 bytes) followed by the raw 16 bytes of the op_id.
/// This avoids a second pass through RecordId::system which would truncate the key.
pub fn op_status_key(op_id: &RecordId) -> RecordKey {
    let mut key_bytes = Vec::with_capacity(DDL_OP_KEY_PREFIX.len() + 16);
    key_bytes.extend_from_slice(DDL_OP_KEY_PREFIX);
    key_bytes.extend_from_slice(op_id.as_bytes());
    RecordKey::from_slice(&key_bytes)
}

/// Strips the version envelope off a raw stored value and decodes the
/// `DdlOpStatus` payload. Shared by `read_op_status` and the eviction sweep
/// so the version-byte check lives in exactly one place.
fn decode_op_status(bytes: &Bytes) -> Result<Option<DdlOpStatus>, DbError> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let version = bytes[0];
    if version != DDL_OP_LOG_VERSION {
        return Err(DbError::Codec(format!(
            "DdlOpStatus decode failed: unrecognized version {version:#02x} (expected {:#02x})",
            DDL_OP_LOG_VERSION
        )));
    }
    bincode::deserialize::<DdlOpStatus>(&bytes[1..])
        .map(Some)
        .map_err(|e| DbError::Codec(format!("DdlOpStatus decode failed: {e}")))
}

/// Writes a DDL operation status to the log.
///
/// This overwrites any existing record for the same `op_id`, which is intentional:
/// the state transitions are monotonic (InProgress → Succeeded/Failed →
/// SucceededViaCrashRecovery) and the latest write is authoritative.
///
/// The value is stored with a version envelope: [VERSION_BYTE | bincode(status)].
///
/// #1068: every write of a TERMINAL state (`Succeeded` / `Failed` /
/// `SucceededViaCrashRecovery`) bumps a process-wide write counter; every
/// `DDL_OP_LOG_EVICTION_CHECK_INTERVAL`th such write triggers a
/// `maybe_evict_terminal_records` sweep, awaited inline (not
/// fire-and-forget — a swallowed eviction failure would defeat the whole
/// point of having retention). `InProgress` writes don't count: they don't
/// grow the eviction-eligible population.
pub async fn write_op_status(
    info_store: &Arc<dyn Store>,
    status: &DdlOpStatus,
) -> Result<(), DbError> {
    write_op_status_with_cap(info_store, status, DDL_OP_LOG_CAP).await
}

/// Same as [`write_op_status`] but with an explicit eviction cap threaded
/// through to the throttled sweep — a test seam so the "throttled trigger
/// actually fires" test can prove the wiring end-to-end (write through the
/// REAL `write_op_status` call path, not a direct call into
/// `maybe_evict_terminal_records`) without writing the real
/// `DDL_OP_LOG_CAP` (10000) records first. `pub(crate)` rather than
/// private so the `tests/` submodule can reach it.
pub(crate) async fn write_op_status_with_cap(
    info_store: &Arc<dyn Store>,
    status: &DdlOpStatus,
    cap: usize,
) -> Result<(), DbError> {
    let key = op_status_key(&status.op_id);
    let status_bytes = bincode::serialize(status)
        .map_err(|e| DbError::Codec(format!("DdlOpStatus encode failed: {e}")))?;

    // Prepend version byte
    let mut bytes = Vec::with_capacity(1 + status_bytes.len());
    bytes.push(DDL_OP_LOG_VERSION);
    bytes.extend_from_slice(&status_bytes);

    info_store.set(key, Bytes::from(bytes)).await?;

    if !matches!(status.state, DdlOpState::InProgress) {
        let n = DDL_OP_LOG_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(DDL_OP_LOG_EVICTION_CHECK_INTERVAL) {
            if let Err(e) = maybe_evict_terminal_records_with_cap(info_store, cap).await {
                log::error!(
                    "#1068: throttled DDL op-log eviction sweep failed after {n} terminal \
                     write(s): {e}"
                );
            }
        }
    }

    Ok(())
}

/// Reads a DDL operation status from the log.
///
/// Returns `Ok(None)` if the key is absent (Unknown operation).
///
/// The value is expected to have a version envelope: [VERSION_BYTE | bincode(status)].
/// If the version byte is not recognized, this returns a Codec error rather than
/// attempting to decode it as the current shape (which would produce garbage).
pub async fn read_op_status(
    info_store: &Arc<dyn Store>,
    op_id: &RecordId,
) -> Result<Option<DdlOpStatus>, DbError> {
    let key = op_status_key(op_id);
    match info_store.get(key).await {
        Ok(bytes) => decode_op_status(&bytes),
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Truncates terminal records to enforce the fixed-cap FIFO policy.
///
/// Called periodically — throttled from `write_op_status` (every
/// `DDL_OP_LOG_EVICTION_CHECK_INTERVAL`th terminal write) and once more at
/// table-open time (`TableManager::create`) to catch up in case the
/// in-memory throttle counter reset across a restart — to prevent
/// unbounded growth of the op-status log (#1068).
///
/// # How this works without a separate time index
///
/// `op_status_key` is `"ddl_op:"` (7 fixed bytes) + the raw 16 bytes of
/// `op_id`. `RecordId::from_ts`/`RecordId::new` write their FIRST 8 bytes
/// as a big-endian, monotonically-increasing timestamp. Since every key
/// under this prefix shares the same 7-byte prefix and the next 8 bytes
/// are a big-endian timestamp, **lexicographic byte order over this
/// keyspace IS chronological order** — and `Store::scan_prefix_stream`
/// carries a hard contract to yield keys in ascending lexicographic byte
/// order. So `scan_prefix_stream(b"ddl_op:", ...)` yields every op-status
/// record oldest-first, for free, on every `Store` backend — FIFO
/// eviction is "remove from the front of this stream until under cap",
/// no schema change or separate index required.
///
/// One caveat: a client-supplied `request_id` (e.g.
/// `DropIndexOp`/`RenameIndexOp::request_id`) is technically any valid
/// 16-byte `RecordId`-shaped value — a non-conforming caller COULD supply
/// a non-timestamp-prefixed id and slightly perturb strict chronological
/// ordering for that one record. This does not break the eviction policy
/// (worst case, one record sorts slightly out of true insertion order
/// among thousands) and is not worth defending against for an alpha-grade
/// FIFO cap.
///
/// # Classification
///
/// - `InProgress` records are **never evicted**, regardless of age or
///   count — an in-flight operation's status must remain pollable until it
///   reaches a terminal state.
/// - `Succeeded` / `Failed` / `SucceededViaCrashRecovery` records are
///   terminal and eligible for eviction. If the terminal count exceeds
///   `cap`, the OLDEST excess terminal records are deleted (the stream is
///   already oldest-first, so this is a straightforward front-trim of the
///   terminal subsequence).
///
/// # Idempotency / crash-safety
///
/// This sweep needs no tombstone/recovery machinery: it re-derives "which
/// records are over cap" from the current on-disk state every time it
/// runs. An interrupted sweep (crash mid-batch) simply leaves slightly
/// more than `cap` terminal records on disk — the NEXT invocation re-scans
/// and finishes the job. This is unlike the DROP/RENAME tombstone paths,
/// which need real crash-recovery machinery because they have irreversible
/// partial states; plain FIFO eviction by re-derivation does not.
pub async fn maybe_evict_terminal_records(info_store: &Arc<dyn Store>) -> Result<(), DbError> {
    maybe_evict_terminal_records_with_cap(info_store, DDL_OP_LOG_CAP).await
}

/// Counts terminal (eviction-eligible) records currently in the op-status
/// log, for observability (#1068) — e.g. `TableManager::verify()`'s report.
/// Read-only; does not evict anything, just reports the current population
/// the retention policy is (or isn't yet) keeping in check.
pub async fn count_terminal_records(info_store: &Arc<dyn Store>) -> Result<u64, DbError> {
    let prefix = Bytes::from_static(DDL_OP_KEY_PREFIX);
    let stream = info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
    futures::pin_mut!(stream);

    let mut count: u64 = 0;
    while let Some(batch) = stream.next().await {
        for (_key, value) in batch? {
            if let Ok(Some(status)) = decode_op_status(&value) {
                if !matches!(status.state, DdlOpState::InProgress) {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// Same as [`maybe_evict_terminal_records`] but with an explicit cap, so
/// tests can exercise eviction with a small cap instead of writing
/// thousands of real records. `pub(crate)` rather than private so the
/// `tests/` submodule (a sibling module within this crate) can reach it.
pub(crate) async fn maybe_evict_terminal_records_with_cap(
    info_store: &Arc<dyn Store>,
    cap: usize,
) -> Result<(), DbError> {
    let prefix = Bytes::from_static(DDL_OP_KEY_PREFIX);
    let stream = info_store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
    futures::pin_mut!(stream);

    // Oldest-first per scan_prefix_stream's ordering contract. Collect the
    // terminal keys in order; InProgress keys are skipped entirely (never
    // eviction-eligible).
    let mut terminal_keys: Vec<RecordKey> = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, value) in batch? {
            match decode_op_status(&value) {
                Ok(Some(status)) if !matches!(status.state, DdlOpState::InProgress) => {
                    terminal_keys.push(key);
                }
                Ok(_) => {
                    // InProgress, or an empty/absent value raced out from
                    // under the scan — either way, not eviction-eligible.
                }
                Err(e) => {
                    // Don't let one undecodable record (e.g. a future
                    // version byte written by a newer binary) abort the
                    // whole sweep — log and skip it, leave it in place.
                    log::error!(
                        "#1068: DDL op-log eviction sweep could not decode a record, \
                         leaving it in place: {e}"
                    );
                }
            }
        }
    }

    if terminal_keys.len() <= cap {
        return Ok(());
    }

    let evict_count = terminal_keys.len() - cap;
    let to_evict: Vec<RecordKey> = terminal_keys.into_iter().take(evict_count).collect();
    let removed = to_evict.len();
    info_store.remove_many(to_evict).await?;

    let remaining = cap;
    log::info!(
        "#1068: DDL op-log eviction removed {removed} terminal record(s), {remaining} remain \
         (cap: {cap})"
    );

    Ok(())
}
