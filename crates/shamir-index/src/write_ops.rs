//! Write-op planning primitives for transactional index commit.

use crate::backend::{IndexBackend, IndexError};
use shamir_storage::types::{KvOp, Store};
use std::sync::Arc;

// Re-export from shamir-tx where the pure-data enum now lives.
pub use shamir_tx::IndexWriteOp;

/// Apply a slice of index write ops against a store + backend.
///
/// `SetPosting` / `RemovePosting` go to `store.set / store.remove`.
/// `BumpFtsStats` goes to `backend.apply_in_memory`.
///
/// Non-tx callers invoke this right after `plan_*`.
/// Tx callers invoke this under the commit lock after merging all
/// ops from `TxContext.index_write_set`.
pub async fn apply_index_ops(
    ops: &[IndexWriteOp],
    store: &Arc<dyn Store>,
    backend: &dyn IndexBackend,
) -> Result<(), IndexError> {
    // Collect Set/Remove postings into one ordered KvOp batch (preserving
    // input order — Set-then-Remove on the same key still yields the
    // last-write-wins semantics of the per-key loop). On transactional
    // backends `Store::transact` collapses N fsyncs into one; on the
    // default loop impl the result is identical to the previous per-op
    // path.
    let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
    let mut in_memory_ops: Vec<IndexWriteOp> = Vec::new();

    for op in ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key } => {
                kv_ops.push(KvOp::Remove(key.clone().into()));
            }
            other => in_memory_ops.push(other.clone()),
        }
    }

    if !kv_ops.is_empty() {
        store
            .transact(kv_ops)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
    }

    if !in_memory_ops.is_empty() {
        backend.apply_in_memory(&in_memory_ops).await?;
    }

    Ok(())
}

/// Apply a batch of staged index ops at transaction commit time
/// (commit pipeline Phase 5c).
///
/// `tx.index_write_set` accumulates `IndexWriteOp`s **without** index-id
/// attribution (only a per-op `table_token`). At commit we therefore:
///
/// - `SetPosting` / `RemovePosting` → applied directly to the table's
///   `info_store` (the same physical store every index2 backend writes
///   its postings into — see `TableManager::create`). This is exactly
///   what V2 WAL recovery does for `IndexPut` / `IndexDel`
///   (`recovery::replay_v2_op`), so re-applying after a happy-path
///   commit is idempotent (`set`/`remove` are last-write-wins).
///
/// - `BumpFtsStats` → broadcast to **all** of the table's index2
///   backends via `apply_in_memory`. Only the FTS-ranked backend
///   reacts (its `apply_in_memory` matches `BumpFtsStats`); every other
///   backend's default impl is a no-op. Broadcasting is necessary
///   because the op carries no idx_id to pinpoint the owning backend.
///   `BumpFtsStats` is in-memory only and is **not** serialised to the
///   WAL (`wal_ops_from_tx` skips it), so crash recovery rebuilds these
///   counters via `rebuild()` on open rather than replaying them.
pub async fn apply_index_ops_at_commit(
    ops: &[IndexWriteOp],
    info_store: &Arc<dyn Store>,
    backends: &[Arc<dyn IndexBackend>],
) -> Result<(), IndexError> {
    // Collapse all SetPosting / RemovePosting ops into one ordered
    // `Store::transact` batch. On transactional backends (sled, redb,
    // fjall, persy, nebari, canopy) the batch is one fsync instead of N
    // — exactly mirroring the V2 WAL recovery path's effect when it
    // batch-replays IndexPut/IndexDel. Last-write-wins semantics are
    // preserved by feeding ops in their original order. BumpFtsStats is
    // in-memory only and unchanged.
    let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
    let mut in_memory_ops: Vec<IndexWriteOp> = Vec::new();

    for op in ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key } => {
                kv_ops.push(KvOp::Remove(key.clone().into()));
            }
            other => in_memory_ops.push(other.clone()),
        }
    }

    if !kv_ops.is_empty() {
        info_store
            .transact(kv_ops)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
    }

    if !in_memory_ops.is_empty() {
        for backend in backends {
            backend.apply_in_memory(&in_memory_ops).await?;
        }
    }

    Ok(())
}

/// tx-aware variant of [`apply_index_ops`].
///
/// - `tx == None` → behaves exactly like [`apply_index_ops`]: ops are
///   applied immediately (`SetPosting`/`RemovePosting` go to the
///   store; in-memory ops go to `backend.apply_in_memory`).
/// - `tx == Some(tx)` → ops are **staged** in `tx.index_write_set`
///   under the supplied `table_token`. Nothing is written to the
///   store or to the backend's in-memory state. A dropped tx
///   (rolled back) therefore leaves no postings; a committed tx
///   applies them via the commit pipeline. See HIGH-6.
///
/// `table_token` is the deterministic per-table hash (see
/// `table_manager::table_token_for`). It is ignored when `tx == None`.
///
/// HIGH-6: staged ops are applied on the happy commit path by
/// `commit::commit_tx_inner` Phase 5c via [`apply_index_ops_at_commit`],
/// and replayed on crash recovery via `recovery::replay_v2_op`
/// (`IndexPut` / `IndexDel`). A dropped/aborted tx leaves no postings
/// because `index_write_set` is owned by the `TxContext` (RAII drop).
pub async fn apply_index_ops_tx(
    ops: &[IndexWriteOp],
    store: &Arc<dyn Store>,
    backend: &dyn IndexBackend,
    table_token: u64,
    tx: Option<&mut shamir_tx::TxContext>,
) -> Result<(), IndexError> {
    if let Some(tx) = tx {
        tx.index_write_set
            .extend(ops.iter().cloned().map(|op| (table_token, op)));
        return Ok(());
    }
    apply_index_ops(ops, store, backend).await
}
