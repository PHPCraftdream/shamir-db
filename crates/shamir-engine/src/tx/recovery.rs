//! V2 WAL recovery — applies inflight WalEntryV2 entries on repo open.
//!
//! Crashes between commit_tx Phase 4 (WAL begin) and Phase 7 (WAL
//! commit) leave durable entries that need replay. Without recovery
//! tx writes are lost despite the WAL marker.
//!
//! Per stage 7.1 plan in docs/pre-transactional/08-tests-landing.md.

use shamir_storage::error::DbResult;
use shamir_wal::WalOpV2;

use crate::repo::RepoInstance;

/// Replay a single WalOpV2 against the given RepoInstance.
///
/// Stage 7.1.c: Put / Delete / CounterDelta are applied for real.
/// Stage 7.1.d: IndexPut / IndexDel are applied (table_id_interned=0
///   broadcasts to all tables' info_stores).
/// InternerOverlayMerge is deferred (Stage 5 — repo-level interner).
pub async fn replay_v2_op(op: &WalOpV2, repo: &RepoInstance) -> DbResult<()> {
    match op {
        WalOpV2::Put {
            table_id_interned,
            rid,
            body,
        } => {
            let tbl = match repo.table_by_token(*table_id_interned).await? {
                Some(t) => t,
                None => {
                    log::warn!(
                        "replay_v2_op Put: table token {} not found in repo {}; \
                         skipping (table may have been dropped)",
                        table_id_interned,
                        repo.name()
                    );
                    return Ok(());
                }
            };
            tbl.data_store().set(rid.to_bytes(), body.clone()).await?;
            Ok(())
        }
        WalOpV2::Delete {
            table_id_interned,
            rid,
        } => {
            let tbl = match repo.table_by_token(*table_id_interned).await? {
                Some(t) => t,
                None => {
                    log::warn!(
                        "replay_v2_op Delete: table token {} not found; skipping",
                        table_id_interned
                    );
                    return Ok(());
                }
            };
            let _ = tbl.data_store().remove(rid.to_bytes()).await;
            Ok(())
        }
        WalOpV2::CounterDelta {
            table_id_interned,
            delta,
        } => {
            let tbl = match repo.table_by_token(*table_id_interned).await? {
                Some(t) => t,
                None => {
                    log::warn!(
                        "replay_v2_op CounterDelta: table token {} not found",
                        table_id_interned
                    );
                    return Ok(());
                }
            };
            tbl.counter().increment(*delta).await?;
            Ok(())
        }
        WalOpV2::IndexPut {
            table_id_interned,
            idx_id: _,
            key,
            value,
        } => {
            if *table_id_interned != 0 {
                let tbl = match repo.table_by_token(*table_id_interned).await? {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "replay_v2_op IndexPut: table token {} not found",
                            table_id_interned
                        );
                        return Ok(());
                    }
                };
                tbl.info_store().set(key.clone(), value.clone()).await?;
                return Ok(());
            }
            for name in repo.list_table_names() {
                if let Ok(tbl) = repo.get_table(&name).await {
                    let _ = tbl.info_store().set(key.clone(), value.clone()).await;
                }
            }
            Ok(())
        }
        WalOpV2::IndexDel {
            table_id_interned,
            idx_id: _,
            key,
        } => {
            if *table_id_interned != 0 {
                let tbl = match repo.table_by_token(*table_id_interned).await? {
                    Some(t) => t,
                    None => {
                        log::warn!(
                            "replay_v2_op IndexDel: table token {} not found",
                            table_id_interned
                        );
                        return Ok(());
                    }
                };
                let _ = tbl.info_store().remove(key.clone()).await;
                return Ok(());
            }
            for name in repo.list_table_names() {
                if let Ok(tbl) = repo.get_table(&name).await {
                    let _ = tbl.info_store().remove(key.clone()).await;
                }
            }
            Ok(())
        }
        WalOpV2::InternerOverlayMerge { entries } => {
            // Each entry is (overlay_id, key_string). Merge into every
            // table's base interner — recovery doesn't know which table
            // contributed which entry, so broadcast like the initial
            // table_id_interned=0 approach. This is safe: touch_ind is
            // idempotent — interning a key that already exists returns
            // the existing id.
            for name in repo.list_table_names() {
                if let Ok(tbl) = repo.get_table(&name).await {
                    if let Ok(interner) = tbl.interner().get().await {
                        for (_overlay_id, key_str) in entries {
                            let _ = interner.touch_ind(key_str);
                        }
                        let _ = tbl.interner().persist().await;
                    }
                }
            }
            Ok(())
        }
    }
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
pub async fn replay_v2_entry(entry: &shamir_wal::WalEntryV2, repo: &RepoInstance) -> DbResult<()> {
    for op in &entry.ops {
        replay_v2_op(op, repo).await.map_err(|e| {
            shamir_storage::error::DbError::Internal(format!(
                "replay tx {} op failed: {}",
                entry.txn_id, e
            ))
        })?;
    }
    Ok(())
}
