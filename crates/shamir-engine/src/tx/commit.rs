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

#[derive(Debug, thiserror::Error)]
pub enum TxError {
    #[error("storage: {0}")]
    Storage(#[from] DbError),
    #[error("ssi conflict on key {key:?}")]
    SsiConflict { key: bytes::Bytes },
}

/// Build WalOpV2 ops from a TxContext for inclusion in the V2 WAL entry.
///
/// Emitted ops in order:
/// - CounterDelta per table.
/// - InternerOverlayMerge (if overlay non-empty).
/// - IndexPut / IndexDel from index_write_set (idx_id=0 placeholder
///   per 4.G.5 invariant).
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

    for op in &tx.index_write_set {
        match op {
            shamir_tx::IndexWriteOp::SetPosting { key, value } => {
                ops.push(WalOpV2::IndexPut {
                    idx_id: 0,
                    key: key.clone(),
                    value: value.clone(),
                });
            }
            shamir_tx::IndexWriteOp::RemovePosting { key } => {
                ops.push(WalOpV2::IndexDel {
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

pub async fn commit_tx(mut tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    let gate = repo.tx_gate().await?;
    let wal = repo.repo_wal().await?;

    let _lock = gate.commit_lock().await;

    // Phase 1: interner overlay merge → id remap.
    //
    // Currently `tx.interner_overlay` stays empty in production flow
    // because the LayeredInterner integration that populates it lives
    // in Stage 5 reconciliation. The wire below runs the no-op safe
    // path: empty overlay → empty remap → apply_id_remap is a free
    // walk over write_set with no mutations. This locks in the
    // structural call site so Stage 5 just needs to populate the
    // overlay upstream.
    let id_remap: std::collections::HashMap<u64, u64> = if tx.interner_overlay.is_empty() {
        std::collections::HashMap::new()
    } else {
        // TODO(Stage 5): once we have a repo-level interner, call
        // commit_interner_overlay(repo_interner, &tx.interner_overlay)
        // to merge and obtain the real overlay_id → base_id remap.
        // For now: log a warning and proceed with empty remap to
        // surface the regression if some code path starts populating
        // the overlay without the merge step.
        log::warn!(
            "commit_tx: tx.interner_overlay is non-empty but Stage 5 wiring is not landed; \
             ignoring overlay entries (Stage 5 will plug commit_interner_overlay here)"
        );
        std::collections::HashMap::new()
    };
    tx.apply_id_remap(&id_remap).await.map_err(DbError::Codec)?;

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
    // Each StagingStore wraps a base Store (data_store of the table).
    // Drain its ops and atomically apply via base.transact(ops).
    for (_table_id, staging) in std::mem::take(&mut tx.write_set) {
        let base: std::sync::Arc<dyn shamir_storage::types::Store> = staging.base().clone();
        let ops = staging.drain();
        if !ops.is_empty() {
            base.transact(ops).await.map_err(TxError::Storage)?;
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

    Ok(TxOutcome {
        tx_id: tx.tx_id.0,
        snapshot_version: tx.snapshot_version,
        commit_version,
    })
}
