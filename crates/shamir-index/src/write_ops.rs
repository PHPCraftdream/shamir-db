//! Write-op planning primitives for transactional index commit.

use crate::backend::{IndexBackend, IndexError};
use crate::descriptor::IndexDescriptor;
use shamir_storage::types::{KvOp, Store};
use std::sync::Arc;

// Re-export from shamir-tx where the pure-data enum now lives.
pub use shamir_tx::IndexWriteOp;
pub use shamir_tx::{IndexFamily, Provenance};

/// R0-B (#1008): PLACEHOLDER [`Provenance`] an index2 backend stamps onto
/// the `IndexWriteOp`s its `plan_*`/`plan_*_tx` methods construct.
///
/// index2 backends only ever see their own construction-time `descriptor()`
/// snapshot — they have NO access to `IndexRegistry`'s live per-entry `gen`
/// (the actual index2 instance epoch, R0-A/#1006's `BackendEntry.gen`; see
/// `Provenance`'s doc). `instance_epoch: 0` here is intentionally a
/// recognizable placeholder, never the real epoch (the registry's
/// `NEXT_INSTANCE_EPOCH`-style counters used by base_index/sorted start at
/// 1, and `IndexRegistry::insert`'s `my_gen` is always `>= 1` too — `0` is
/// unreachable as a REAL epoch across every family).
///
/// # The invariant (not a list of names — F-1/#1027)
///
/// A prior revision of this doc enumerated specific function names that
/// "must overwrite" the placeholder. That list silently went stale: two
/// more call sites (`TableManager::insert_tx_many`/`insert_tx_many_bytes`'s
/// inlined index2 op-planning loops, which amortize the `all_backends()`
/// snapshot across a whole batch instead of calling `plan_insert_ops`) were
/// added later without updating it, and shipped never stamping — silently
/// losing postings the moment any concurrent index2 DDL landed between
/// stage and commit (fixed by F-1). Names drift; the invariant does not.
/// State it precisely instead:
///
/// **ANY code path that adds an index2 `IndexWriteOp` to
/// `tx.index_write_set` (directly, or via a staging helper whose output is
/// later merged into it) MUST have overwritten this placeholder first**,
/// via `IndexWriteOp::set_provenance` using `IndexRegistry::instance_epoch_of`
/// (or the `TableManager::stamp_index2_provenance` helper that wraps it),
/// immediately after calling a backend's `plan_*`/`plan_*_tx`. When adding a
/// new call site that plans index2 ops for a tx, grep every call site that
/// pushes into `tx.index_write_set` (or an accumulator later merged into
/// it) for index2 ops specifically, and confirm each one stamps — do not
/// trust an enumerated list (this one included) to still be complete.
///
/// The NON-tx direct-apply path (`apply_index_ops`) never stages ops for
/// later reconcile, so the placeholder harmlessly reaches
/// `apply_in_memory`/the store write without ever being compared.
pub fn index2_provenance(descriptor: &IndexDescriptor) -> Provenance {
    Provenance {
        family: IndexFamily::Index2,
        name_interned: descriptor.name_interned,
        instance_epoch: 0,
    }
}

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
            IndexWriteOp::SetPosting { key, value, .. } => {
                kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key, .. } => {
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
            IndexWriteOp::SetPosting { key, value, .. } => {
                kv_ops.push(KvOp::Set(key.clone().into(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key, .. } => {
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
