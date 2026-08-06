//! DDL operation status log (#1015) — durable append-only store for DDL op states.
//!
//! The op-status log is keyed by `system:ddl_op:<op_id>` and stores `DdlOpStatus`
//! structs. It lives in the same `info_store` that tombstones use, but is
//! semantically distinct:
//! - Tombstones are cleared on success and keyed by name (internal recovery only).
//! - Op-status records survive success and are keyed by a stable `op_id` (client-visible).
//!
//! This module provides the storage primitives for reading/writing op-status records.
//! The actual op lifecycle management (minting, state transitions, recovery writes)
//! lives in the DDL handlers and recovery functions.

use bytes::Bytes;
use shamir_query_types::read::DdlOpStatus;
use shamir_storage::error::DbError;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

/// Maximum number of terminal (Succeeded/Failed/SucceededViaCrashRecovery) records
/// to retain. When exceeded, records are evicted in FIFO order (oldest first).
///
/// This is a generous fixed cap for the first slice; retention tuning is deferred
/// (RFC §4 "defer to follow-ups").
#[allow(dead_code)]
const DDL_OP_LOG_CAP: usize = 10000;

/// Prefix for all DDL operation status keys.
const DDL_OP_KEY_PREFIX: &str = "ddl_op:";

/// Builds the `system:ddl_op:<op_id>` key for a given operation ID.
pub fn op_status_key(op_id: &RecordId) -> RecordKey {
    let key_str = format!("{}{}", DDL_OP_KEY_PREFIX, op_id);
    RecordId::system(&key_str).to_bytes().into()
}

/// Writes a DDL operation status to the log.
///
/// This overwrites any existing record for the same `op_id`, which is intentional:
/// the state transitions are monotonic (InProgress → Succeeded/Failed →
/// SucceededViaCrashRecovery) and the latest write is authoritative.
pub async fn write_op_status(
    info_store: &Arc<dyn Store>,
    status: &DdlOpStatus,
) -> Result<(), DbError> {
    let key = op_status_key(&status.op_id);
    let bytes = bincode::serialize(status)
        .map_err(|e| DbError::Codec(format!("DdlOpStatus encode failed: {e}")))?;
    info_store.set(key, Bytes::from(bytes)).await?;
    Ok(())
}

/// Reads a DDL operation status from the log.
///
/// Returns `Ok(None)` if the key is absent (Unknown operation).
pub async fn read_op_status(
    info_store: &Arc<dyn Store>,
    op_id: &RecordId,
) -> Result<Option<DdlOpStatus>, DbError> {
    let key = op_status_key(op_id);
    match info_store.get(key).await {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => bincode::deserialize::<DdlOpStatus>(&bytes)
            .map(Some)
            .map_err(|e| DbError::Codec(format!("DdlOpStatus decode failed: {e}"))),
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Truncates terminal records to enforce the fixed-cap FIFO policy.
///
/// This should be called periodically (e.g., on server startup and after each
/// terminal write) to prevent unbounded growth of the op-status log. The
/// implementation for the first slice is a simple FIFO eviction of the oldest
/// terminal records (Succeeded/Failed/SucceededViaCrashRecovery).
///
/// NOTE: For the first slice, this is a no-op stub — we ship with the cap but
/// defer the actual eviction logic to a follow-up (RFC §4). The cap is still
/// documented here to make the contract explicit.
pub async fn maybe_evict_terminal_records(_info_store: &Arc<dyn Store>) -> Result<(), DbError> {
    // First slice: no-op. The cap exists to bound growth in production,
    // but the actual eviction logic is deferred to a follow-up.
    Ok(())
}
