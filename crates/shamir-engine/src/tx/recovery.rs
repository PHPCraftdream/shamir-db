//! V2 WAL recovery — applies inflight WalEntryV2 entries on repo open.
//!
//! Crashes between commit_tx Phase 4 (WAL begin) and Phase 7 (WAL
//! commit) leave durable entries that need replay. Without recovery
//! tx writes are lost despite the WAL marker.
//!
//! Per stage 7.1 plan in docs/pre-transactional/08-tests-landing.md.

use shamir_storage::error::{DbError, DbResult};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::RepoInstance;

/// Replay a single WalOpV2 against the given RepoInstance.
///
/// Stage 7.1.a: stubs only — emits a `log::warn!` and returns Ok.
/// Stage 7.1.b implements actual mutation application.
pub async fn replay_v2_op(_op: &WalOpV2, _repo: &RepoInstance) -> DbResult<()> {
    log::warn!("replay_v2_op: stub (Stage 7.1.a) — op not actually applied");
    Ok(())
}

/// Walk all inflight V2 WAL entries for the repo and replay each one.
/// Marker is removed on successful replay (per-entry).
///
/// Called from `RepoInstance::recover_v2_inflight`.
pub async fn recover_inflight_v2(repo: &RepoInstance) -> DbResult<usize> {
    let wal = repo.repo_wal().await?;
    let entries = wal.list_inflight().await?;
    let count = entries.len();

    for entry in entries {
        replay_v2_entry(&entry, repo).await?;
        wal.commit(entry.txn_id).await?;
    }

    if count > 0 {
        log::info!("V2 recovery replayed {} inflight tx entries", count);
    }
    Ok(count)
}

/// Replay all ops in one WAL entry. Iterates ops in declared order
/// (counter → interner → index → data per `wal_ops_from_tx` emission
/// order, though replay order is logically commutative within one entry).
pub async fn replay_v2_entry(entry: &WalEntryV2, repo: &RepoInstance) -> DbResult<()> {
    for op in &entry.ops {
        replay_v2_op(op, repo).await.map_err(|e| {
            DbError::Internal(format!("replay tx {} op failed: {}", entry.txn_id, e))
        })?;
    }
    Ok(())
}
