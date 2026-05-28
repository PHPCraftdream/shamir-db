use shamir_storage::error::DbError;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::RepoInstance;

#[derive(Debug, Clone)]
pub struct TxOutcome {
    pub tx_id: u64,
    pub snapshot_version: u64,
    pub commit_version: u64,
}

const DEFAULT_MAX_TX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(300); // 5 min

#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("storage: {0}")]
    Storage(#[from] DbError),
    #[error("ssi conflict on key {key:?}")]
    SsiConflict { key: bytes::Bytes },
    #[error("tx expired: elapsed {elapsed:?} > max {max:?}")]
    Expired {
        elapsed: std::time::Duration,
        max: std::time::Duration,
    },
}

/// Build WalOpV2 ops from a TxContext for inclusion in the V2 WAL entry.
///
/// Emitted ops in order:
/// - CounterDelta per table.
/// - InternerOverlayMerge (if overlay non-empty).
/// - IndexPut / IndexDel from index_write_set (table_id_interned from
///   per-op table_token stamped at write time; idx_id=0 placeholder).
/// - Put / Delete from write_set snapshot (carry table_id_interned
///   so recovery can resolve target data_store).
/// - BumpFtsStats is in-memory only and not serialised.
pub async fn wal_ops_from_tx(tx: &TxContext) -> Vec<WalOpV2> {
    let mut ops = Vec::new();

    for (table_id, delta) in &tx.counter_deltas {
        ops.push(WalOpV2::CounterDelta {
            table_id_interned: *table_id,
            delta: *delta,
        });
    }

    let mut entries: Vec<(u64, String)> = Vec::new();
    tx.interner_overlay
        .scan_async(|k, v| entries.push((*v, k.clone())))
        .await;
    if !entries.is_empty() {
        ops.push(WalOpV2::InternerOverlayMerge { entries });
    }

    for (table_token, op) in &tx.index_write_set {
        match op {
            shamir_tx::IndexWriteOp::SetPosting { key, value } => {
                ops.push(WalOpV2::IndexPut {
                    table_id_interned: *table_token,
                    idx_id: 0,
                    key: key.clone(),
                    value: value.clone(),
                });
            }
            shamir_tx::IndexWriteOp::RemovePosting { key } => {
                ops.push(WalOpV2::IndexDel {
                    table_id_interned: *table_token,
                    idx_id: 0,
                    key: key.clone(),
                });
            }
            shamir_tx::IndexWriteOp::BumpFtsStats { .. } => {}
        }
    }

    // Phase 4 data ops: snapshot each per-table StagingStore (no
    // consume — drain happens in Phase 5). This makes the WAL entry
    // self-contained: recovery can replay tx data writes without
    // needing the (still-staged) StagingStore around.
    for (table_id, staging) in &tx.write_set {
        for kv_op in staging.snapshot_ops() {
            match kv_op {
                shamir_storage::types::KvOp::Set(k, v) => {
                    if let Some(rid) = shamir_types::types::record_id::RecordId::try_from_bytes(&k)
                    {
                        ops.push(WalOpV2::Put {
                            table_id_interned: *table_id,
                            rid,
                            body: v,
                        });
                    }
                }
                shamir_storage::types::KvOp::Remove(k) => {
                    if let Some(rid) = shamir_types::types::record_id::RecordId::try_from_bytes(&k)
                    {
                        ops.push(WalOpV2::Delete {
                            table_id_interned: *table_id,
                            rid,
                        });
                    }
                }
            }
        }
    }

    ops
}

pub async fn commit_tx(tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    match commit_tx_inner(tx, repo).await {
        Ok(outcome) => Ok(outcome),
        Err(TxError::Storage(e)) => {
            repo.tx_metrics().on_tx_aborted_storage();
            Err(TxError::Storage(e))
        }
        Err(e) => Err(e),
    }
}

async fn commit_tx_inner(mut tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    if tx.is_expired(DEFAULT_MAX_TX_LIFETIME) {
        repo.tx_metrics().on_tx_aborted_expired();
        return Err(TxError::Expired {
            elapsed: tx.elapsed(),
            max: DEFAULT_MAX_TX_LIFETIME,
        });
    }

    let gate = repo.tx_gate().await?;
    let wal = repo.repo_wal().await?;

    let _lock = gate.commit_lock().await;

    // Phase 1: interner overlay merge → per-table id remap.
    //
    // Each table has its own Interner. The tx overlay is a shared
    // scc::HashMap that may contain entries contributed by multiple
    // tables. We merge it into each touched table's base Interner
    // separately, obtaining a per-table remap, then rewrite only that
    // table's staging bytes. This is correct because overlay ids in
    // table A's staging came from a LayeredInterner backed by table A's
    // base — table B's staging has its own set of overlay ids.
    if !tx.interner_overlay.is_empty() {
        let table_ids: Vec<u64> = tx.write_set.keys().cloned().collect();
        for table_id in &table_ids {
            if let Some(tbl) = repo.table_by_token(*table_id).await? {
                let base_interner = tbl.interner().get().await?;
                let remap =
                    shamir_tx::commit_interner_overlay(base_interner, &tx.interner_overlay).await?;
                if !remap.is_empty() {
                    if let Some(staging) = tx.write_set.get(table_id) {
                        staging
                            .rewrite_set_bytes(|bytes| {
                                shamir_tx::remap_inner_value_bytes(bytes.clone(), &remap)
                                    .map_err(|e| format!("remap encode: {e}"))
                            })
                            .await
                            .map_err(DbError::Codec)?;
                    }
                }
                tbl.interner().persist().await?;
            }
        }
    }

    // Phase 2 (SSI only): read-set validation.
    //
    // For each (table_id, key) the tx read at version_seen, ensure the
    // current committed version has not moved past it.
    //
    // Stage 4.D.5 wires the structural skeleton. The version_provider
    // is currently a stub (`|_, _| 0`) because the per-table MvccStore
    // map lives at the executor/repo layer — Stage 4.D.6 will plug it
    // through. With a zero provider every comparison passes, so SI
    // and Serializable behave identically in this sub-stage. The
    // failure path is exercised in unit tests on
    // `TxContext::validate_read_set` directly.
    if tx.isolation == IsolationLevel::Serializable {
        // Phase 2: SSI read-set validation.
        // Uses tx.version_provider if set; otherwise stub `|_, _| 0`
        // (Snapshot-equivalent behaviour). Real provider wiring to
        // per-table MvccStore lands with Stage 5 reconciliation.
        let validation = match tx.version_provider.as_ref() {
            Some(provider) => {
                let provider = std::sync::Arc::clone(provider);
                tx.validate_read_set(move |t, k| provider.version_of(t, k))
            }
            None => tx.validate_read_set(|_t, _k| Some(0u64)),
        };
        if let Err((_table_id, key)) = validation {
            repo.tx_metrics().on_tx_aborted_ssi();
            return Err(TxError::SsiConflict { key });
        }
    }

    // Phase 3: assign new version
    let commit_version = gate.assign_next_version();

    // Phase 4: write WAL entry
    let wal_ops = wal_ops_from_tx(&tx).await;
    let entry = WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops);
    wal.begin(entry).await?;

    // Phase 5a: physical data writes per table.
    // Route through MvccStore when available so version_cache stays
    // current for SSI conflict detection. Fall back to direct
    // base.transact for tables not yet registered in per_table_mvcc.
    for (table_id, staging) in std::mem::take(&mut tx.write_set) {
        let base: std::sync::Arc<dyn shamir_storage::types::Store> = staging.base().clone();
        let ops = staging.drain();
        if ops.is_empty() {
            continue;
        }
        let mvcc_found = repo
            .per_table_mvcc()
            .read_async(&table_id, |_, mvcc| std::sync::Arc::clone(mvcc))
            .await;
        match mvcc_found {
            Some(mvcc) => {
                mvcc.apply_committed_ops(ops, commit_version).await?;
            }
            None => {
                base.transact(ops).await.map_err(TxError::Storage)?;
            }
        }
    }

    // Phase 5b-d: indexes, HNSW, counters — TODO Stage 4.D.5+.
    // index_write_set, tables_with_hnsw_staging, counter_deltas
    // are still drained-but-unapplied; apply landing alongside the
    // per-table data_store wiring that the executor will set up
    // when it constructs TxContext.write_set entries.

    // Phase 6: publish — atomic publish-committed
    gate.publish_committed(commit_version);

    // Phase 7: WAL cleanup
    wal.commit(tx.tx_id.0).await?;

    repo.tx_metrics().on_tx_committed();

    Ok(TxOutcome {
        tx_id: tx.tx_id.0,
        snapshot_version: tx.snapshot_version,
        commit_version,
    })
}
