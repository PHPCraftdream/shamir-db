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
    #[error("unique constraint violated on key {key:?}")]
    UniqueViolation { key: bytes::Bytes },
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
///
/// Thin metrics-only wrapper around [`commit_tx_inner`]: it records the
/// `on_tx_aborted_storage` metric on any storage-layer abort and is
/// otherwise transparent. There is NOTHING to do for HNSW staging on
/// abort — staged vectors live inside the `TxContext` (`staged_vectors`)
/// and vanish when the tx is dropped on the `Err` return below. That is
/// the RAII win: no per-tx buffer outlives the `TxContext`, so no
/// broadcast drain is needed (HIGH-6).
pub async fn commit_tx(tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    match commit_tx_inner(tx, repo).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            if let TxError::Storage(_) = &e {
                repo.tx_metrics().on_tx_aborted_storage();
            }
            Err(e)
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

    // Phase 2.5 (HIGH-A): acquire each unique table's `unique_write_lock`
    // and HOLD it across Phase 2.6 → 5c.
    //
    // The problem this closes: `commit_lock` (held since the top of this fn)
    // only serialises *committers*. Non-tx `insert` / `set` / `delete` take a
    // DIFFERENT mutex — the per-table `unique_write_lock` — and never touch
    // `commit_lock`. So without this step a non-tx unique write could claim or
    // overwrite the same unique posting in the window between this tx's Phase
    // 2.6 re-check and its Phase 5c posting write, producing a duplicate
    // unique value + corrupted index. Acquiring the same per-table lock the
    // non-tx path uses makes the tx's "check unique key free → write posting"
    // atomic against every non-tx unique writer to that table.
    //
    // Lock ordering / ABBA-freedom (§B9):
    //   - This committer holds `commit_lock(repo)` FIRST, then acquires the
    //     per-table `unique_write_lock`s. The per-table locks are acquired in
    //     a deterministic order (distinct tokens sorted ascending) so the
    //     ordering is documented and future-proof even though `commit_lock`
    //     already guarantees a single committer at a time.
    //   - A non-tx writer holds at most ONE `unique_write_lock` and NEVER
    //     waits on `commit_lock` or a second `unique_write_lock`. So its lock
    //     set can never form the back-edge of a cycle with a committer's set.
    //   - Two committers are mutually excluded by `commit_lock`, so only one
    //     ever holds any `unique_write_lock` at a time.
    //   Therefore no ABBA cycle is possible.
    //
    // We use `lock_owned()` so the guards can be collected into a `Vec`
    // without borrow-lifetime entanglement (each `OwnedMutexGuard` owns its
    // `Arc<Mutex<()>>`). Tables without unique guards are untouched — the
    // non-unique commit path keeps the lock-free fast path.
    //
    // Perf trade: holding these locks across Phases 3/4/5a/5b/5c serialises
    // non-tx unique writes to the affected tables against this commit for the
    // duration of the commit's data/index writes. That is exactly the
    // correctness requirement (the Phase 2.6 guard must stay decisive through
    // the posting write); the cost is bounded to tables that actually carry
    // unique guards in this tx, and only against *unique* writers (non-unique
    // writers never take this lock).
    let mut unique_tokens: Vec<u64> = tx.unique_guards.iter().map(|g| g.table_token).collect();
    unique_tokens.sort_unstable();
    unique_tokens.dedup();
    let mut uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>> =
        Vec::with_capacity(unique_tokens.len());
    for token in &unique_tokens {
        if let Some(tbl) = repo.table_by_token(*token).await? {
            uwl_guards.push(tbl.unique_write_lock().lock_owned().await);
        }
    }

    // Phase 2.6: authoritative unique re-validation under commit_lock +
    // per-table unique_write_lock (held since Phase 2.5).
    //
    // Stage-time `validate_unique_*` is optimistic — it reads pre-commit
    // state, so two concurrent txs claiming the same unique value both
    // pass it. `commit_lock` (held since the top of this fn) serialises
    // committers, and the per-table `unique_write_lock`s (Phase 2.5) exclude
    // non-tx unique writers, so re-checking the claimed keys here is decisive
    // against ALL writers: no committer AND no non-tx writer can interleave
    // between this check and the Phase 5 data/index writes that publish the
    // postings.
    //
    // The unique key is deterministic in the value, so a byte-equal
    // `info_store.get(index_key)` settles ownership:
    //   - NotFound          → key free                 → OK
    //   - Some == our owner  → update self-write        → OK
    //   - Some != our owner  → another record owns it   → abort
    for g in &tx.unique_guards {
        if let Some(tbl) = repo.table_by_token(g.table_token).await? {
            match tbl.info_store().get(g.index_key.clone()).await {
                Ok(existing) => {
                    if existing.as_ref() != g.owner.as_bytes().as_slice() {
                        repo.tx_metrics().on_tx_aborted_unique();
                        return Err(TxError::UniqueViolation {
                            key: g.index_key.clone(),
                        });
                    }
                }
                Err(DbError::NotFound(_)) => {} // key free → OK
                Err(e) => return Err(TxError::Storage(e)),
            }
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

    // HIGH-A: release the per-table unique_write_lock guards now that the
    // unique postings are published (Phase 5c done). The window the Phase 2.6
    // guard had to dominate — re-check → posting write — is closed, so a non-tx
    // unique writer may resume. Phases 5d/6/6.5/7 (HNSW promote, publish,
    // recovery markers, WAL cleanup) do not touch unique postings, so holding
    // the locks past here would only add contention.
    drop(uwl_guards);

    // Phase 5d: promote HNSW staged vectors into the live graph (HIGH-6).
    //
    // Must run INSIDE the commit critical section (we still hold
    // `_lock`) so the promotion is atomic with the rest of the commit:
    // no concurrent committer can interleave between the data writes and
    // the vector-graph promotion.
    //
    // The footprint is now PRECISE — we iterate exactly the tables the tx
    // staged vectors into (`tx.staged_vectors`), keyed by table token. No
    // broadcast over the whole touched-table set is needed because the tx
    // knows its own vector footprint. `apply_staged_vectors` is a no-op
    // for every backend except `VectorBackend`. Phase 5a only drained
    // `tx.write_set`, so `tx.staged_vectors` is intact here.
    //
    // On error here we have published nothing yet (publish happens in
    // Phase 6); returning `Err` drops `tx`, discarding `staged_vectors`
    // by RAII — nothing to clean up.
    for (token, vecs) in &tx.staged_vectors {
        if vecs.is_empty() {
            continue;
        }
        if let Some(tbl) = repo.table_by_token(*token).await? {
            for backend in tbl.index2_registry().all_backends().await {
                backend.apply_staged_vectors(vecs).await.map_err(|e| {
                    TxError::Storage(DbError::Internal(format!(
                        "hnsw apply_staged_vectors at commit: {e}"
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
