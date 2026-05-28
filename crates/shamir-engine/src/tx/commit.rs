use shamir_storage::error::DbError;
use shamir_tx::{IsolationLevel, TxContext};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::meta::recovery_marker::{save_last_committed, save_next_tx_id_snapshot};
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

/// cancel-safe: yes — read-only over `&TxContext`. The only `.await`
/// is `interner_overlay.scan_async` which is itself cancel-safe (the
/// borrowed map is untouched on drop); the rest is in-memory iteration.
/// No external state mutation happens here.
///
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

/// cancel-safe: NO — Phase 4 (WAL begin) and Phase 5 (data writes)
/// must complete together. Cancellation between Phase 4 and Phase 7
/// leaves an inflight WAL marker that recovery will replay; ops are
/// idempotent at the data layer so recovery is safe, but the caller
/// observes neither success nor a clean abort. Treat as non-cancel-safe
/// at the API boundary.
pub async fn commit_tx(tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    // HIGH-6 (Part C): capture the HNSW staging footprint BEFORE `tx` is
    // moved into the inner commit. On any abort (Expired / SsiConflict /
    // Storage error / Phase 5d failure) we must drain the per-tx HNSW
    // staging buffers so a failed commit doesn't leak staged vectors
    // (tied to a TxId that is never reused). A clean happy-path commit
    // empties the buffers in Phase 5d, so the post-commit rollback below
    // is a cheap no-op (`remove_async` on an absent key).
    let tx_id = tx.tx_id;
    let hnsw_tokens = touched_table_tokens(&tx);

    match commit_tx_inner(tx, repo).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            rollback_hnsw_staging_tokens(tx_id, &hnsw_tokens, repo).await;
            if let TxError::Storage(_) = &e {
                repo.tx_metrics().on_tx_aborted_storage();
            }
            Err(e)
        }
    }
}

/// Collect every table token the tx touched: data writes, staged index
/// ops, counter deltas, and any explicitly-marked HNSW staging. The
/// commit pipeline broadcasts `commit_staged_tx` / `rollback_staged_tx`
/// to these tables' index2 backends (HIGH-6 Phase 5d / abort).
///
/// We cannot rely on `tx.tables_with_hnsw_staging` alone: the executor
/// path that stages vectors (`TableManager::insert_tx` →
/// `plan_insert_tx`) routes the vector into the adapter's per-tx buffer
/// but does not currently populate `mark_hnsw_staging`. Driving the
/// commit/rollback off the full touched-table set is robust regardless:
/// `commit_staged`/`rollback_staged` are no-ops when a backend has no
/// staged entries for this `tx_id`.
fn touched_table_tokens(tx: &TxContext) -> Vec<u64> {
    let mut tokens: std::collections::HashSet<u64> = std::collections::HashSet::new();
    tokens.extend(tx.write_set.keys().copied());
    tokens.extend(tx.index_write_set.iter().map(|(t, _)| *t));
    tokens.extend(tx.counter_deltas.keys().copied());
    tokens.extend(tx.tables_with_hnsw_staging.iter().copied());
    tokens.into_iter().collect()
}

/// HIGH-6 (Part C): drain the HNSW per-tx staging buffers for the given
/// tables. Used on every commit error path (and by the begin_tx caller's
/// explicit abort, if it routes through `commit_tx`). Each
/// `rollback_staged_tx` is a no-op for non-vector backends and for
/// vector backends whose buffer was already drained on commit.
///
/// RAII note: `TxContext` has no async Drop (Rust `Drop` is sync and the
/// HNSW buffer lives behind an `scc::HashMap` keyed by `TxId` on the
/// adapter, not inside the `TxContext`). A tx that is simply dropped
/// without ever calling `commit_tx` therefore still leaks its HNSW
/// staging. The mitigations are: (1) `commit_tx` always drains on abort
/// via this helper; (2) callers funnel abort through `commit_tx` rather
/// than a bare drop. A future fully-RAII fix would require a sync
/// drain primitive on the adapter or a tokio runtime handle in Drop.
async fn rollback_hnsw_staging_tokens(tx_id: shamir_tx::TxId, tokens: &[u64], repo: &RepoInstance) {
    for token in tokens {
        if let Ok(Some(tbl)) = repo.table_by_token(*token).await {
            for backend in tbl.index2_registry().all_backends().await {
                backend.rollback_staged_tx(tx_id).await;
            }
        }
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

    // Phase 4: write WAL entry.
    //
    // HIGH-5: stamp the assigned `commit_version` onto the entry
    // BEFORE persisting it. Recovery sorts inflight entries by
    // `commit_version` so multi-tx replay matches the original
    // commit pipeline's order; `txn_id` (the `WalActiveKey` byte
    // order) is not a safe proxy because tx allocation and commit
    // ordering are independent.
    let wal_ops = wal_ops_from_tx(&tx).await;
    let entry =
        WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops).with_commit_version(commit_version);
    wal.begin(entry).await?;

    // Snapshot the touched-table set BEFORE Phase 5a drains
    // `tx.write_set`. Phase 5d broadcasts `commit_staged_tx` to these
    // tables' index2 backends to promote any HNSW vectors staged under
    // this tx (HIGH-6). See `touched_table_tokens` for why this is
    // derived from the full footprint rather than
    // `tx.tables_with_hnsw_staging`.
    let hnsw_candidate_tokens = touched_table_tokens(&tx);

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

    // Phase 5b: counter deltas (CRIT-3).
    //
    // `tx.counter_deltas` is already serialised into the WAL entry
    // by `wal_ops_from_tx` (above). The happy-path commit must also
    // apply it in-memory so callers can observe the new count
    // without waiting for recovery — without this step
    // `tbl.counter().get()` would still return the pre-commit
    // value after a successful tx.
    //
    // Recovery idempotency: the matching replay branch in
    // `recovery::replay_v2_op` for `WalOpV2::CounterDelta` is
    // intentionally SKIPPED at recovery time — see the comment on
    // that branch. Counter persistence is best-effort; the durable
    // `last_committed_version` marker written below in Phase 6.5
    // is the authoritative MVCC milestone, and a per-tx
    // counter-applied marker is overkill for an at-most-by-one
    // drift on the metric counter.
    for (table_id, delta) in &tx.counter_deltas {
        if let Some(tbl) = repo.table_by_token(*table_id).await? {
            tbl.counter().increment(*delta).await?;
        }
    }

    // Phase 5c: apply staged index ops (HIGH-6).
    //
    // `tx.index_write_set` is `Vec<(table_token, IndexWriteOp)>`,
    // already serialised into the WAL entry by `wal_ops_from_tx`. The
    // happy path must also apply it in-memory so post-commit, non-tx
    // queries see the new postings without waiting for crash recovery.
    //
    // Group ops by table_token, resolve each table once (caching its
    // info_store + index2 backends), then apply via
    // `apply_index_ops_at_commit`: SetPosting/RemovePosting hit the
    // info_store; BumpFtsStats is broadcast to the table's backends'
    // `apply_in_memory`.
    //
    // Recovery idempotency: `recovery::replay_v2_op` applies IndexPut
    // (→ info_store.set) and IndexDel (→ info_store.remove) for the same
    // postings. `set`/`remove` are last-write-wins, so a crash that
    // re-replays an already-committed entry converges to the same state.
    // BumpFtsStats is not serialised to the WAL (see `wal_ops_from_tx`);
    // its in-memory counters are rebuilt via `rebuild()` on open.
    if !tx.index_write_set.is_empty() {
        let mut by_token: std::collections::HashMap<u64, Vec<shamir_tx::IndexWriteOp>> =
            std::collections::HashMap::new();
        for (token, op) in &tx.index_write_set {
            by_token.entry(*token).or_default().push(op.clone());
        }
        for (token, ops) in by_token {
            if let Some(tbl) = repo.table_by_token(token).await? {
                let backends = tbl.index2_registry().all_backends().await;
                crate::index2::write_ops::apply_index_ops_at_commit(
                    &ops,
                    tbl.info_store(),
                    &backends,
                )
                .await
                .map_err(|e| DbError::Internal(format!("index apply at commit: {e}")))?;
            }
        }
    }

    // Phase 5d: promote HNSW staged vectors into the live graph (HIGH-6).
    //
    // Must run INSIDE the commit critical section (we still hold
    // `_lock`) so the promotion is atomic with the rest of the commit:
    // no concurrent committer can interleave between the data writes and
    // the vector-graph promotion. `commit_staged_tx` is a no-op for
    // every backend except `VectorBackend`, whose adapter drains its
    // per-tx staging buffer.
    //
    // On error here we have published nothing yet (publish happens in
    // Phase 6). The `commit_tx` wrapper drains any remaining staging on
    // the Err return — see `rollback_hnsw_staging_tokens`.
    for token in &hnsw_candidate_tokens {
        if let Some(tbl) = repo.table_by_token(*token).await? {
            for backend in tbl.index2_registry().all_backends().await {
                backend.commit_staged_tx(tx.tx_id).await.map_err(|e| {
                    TxError::Storage(DbError::Internal(format!(
                        "hnsw commit_staged at commit: {e}"
                    )))
                })?;
            }
        }
    }

    // Phase 6: publish — atomic publish-committed
    gate.publish_committed(commit_version);

    // Phase 6.5: persist recovery markers (CRIT-1).
    //
    // Write `last_committed_version` + `next_tx_id` to the repo's
    // `__tx__` info_store BEFORE Phase 7 removes the WAL marker.
    //
    // Ordering rationale:
    //
    // - Markers BEFORE wal.commit:
    //     If markers succeed but `wal.commit` fails, the WAL entry
    //     stays inflight and recovery re-applies it. Data writes
    //     are idempotent (Put/Delete/IndexPut/IndexDel are
    //     last-write-wins) and counter replay is intentionally
    //     skipped (see `recovery::replay_v2_op`).
    //
    // - Markers AFTER publish_committed:
    //     The in-memory `last_committed_version` is the source of
    //     truth at runtime; the persisted copy only matters across
    //     restarts. Writing it post-publish keeps the in-memory
    //     advancement uncoupled from durability latency.
    //
    // The inverse order (wal.commit before markers) is unsafe: a
    // crash between the two would clear the WAL marker yet leave
    // `last_committed_version` stale on disk, so the MVCC counter
    // would rewind on restart → version-monotonicity violation.
    let info_store = repo.tx_info_store().await?;
    save_last_committed(&info_store, commit_version).await?;
    save_next_tx_id_snapshot(&info_store, gate.peek_next_tx_id()).await?;

    // Phase 7: WAL cleanup
    wal.commit(tx.tx_id.0).await?;

    repo.tx_metrics().on_tx_committed();

    Ok(TxOutcome {
        tx_id: tx.tx_id.0,
        snapshot_version: tx.snapshot_version,
        commit_version,
    })
}
