use shamir_storage::error::DbError;
use shamir_tx::{IndexWriteOp, TxContext};
use shamir_types::types::common::THasher;

use crate::repo::RepoInstance;
use crate::tx::tx_outcome::MaterializationState;

/// Bounded inline retry budget for a post-commit materialization
/// sub-phase. A few attempts absorb transient storage hiccups; a
/// persistent failure falls through to deferral (recovery re-applies the
/// WAL entry). Transient-vs-persistent is NOT perfectly classified —
/// idempotent re-application makes that unnecessary.
pub(crate) const MATERIALIZE_ATTEMPTS: u32 = 3;

/// Test-only injection: when set to a non-zero tx_id, Phase 5c (index
/// apply) returns a synthetic storage error for the matching tx. Used by
/// `commit_phase5_defer_tests` to prove a post-commit-point failure is
/// reported COMMITTED-with-deferred-materialization (not aborted) and is
/// then reconciled by recovery. Persisted across the bounded retry so
/// the failure is treated as persistent → deferral.
#[cfg(test)]
pub(crate) static FAIL_PHASE_5C_TX_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Test-only injection (audit MED multi-table deferral): when
/// [`FAIL_PHASE_5A_TX_ID`] matches the committing tx AND
/// [`FAIL_PHASE_5A_TABLE_TOKEN`] matches the table being applied, Phase 5a
/// (`apply_data_batch`) returns a synthetic storage error for THAT TABLE
/// ONLY. Keyed by `(table_token, tx_id)` — unlike [`FAIL_PHASE_5C_TX_ID`]
/// which fires for every table of the tx — so a test can fail the SECOND
/// table's data write while the first commits cleanly, producing the
/// partial cross-table materialization the audit flagged. Persisted across
/// the bounded retry so the failure is treated as persistent → deferral.
/// Both registers must be non-zero to arm; a zero token disarms.
#[cfg(test)]
pub(crate) static FAIL_PHASE_5A_TX_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Companion table-token selector for [`FAIL_PHASE_5A_TX_ID`]. See its doc.
#[cfg(test)]
pub(crate) static FAIL_PHASE_5A_TABLE_TOKEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Test-only injection: when set to a non-zero tx_id, the post-lock HNSW
/// promote (Phase 5d, `apply_vector_batch`) returns a synthetic error for
/// the matching tx. Used by `commit_phase5_tests` to prove III.5: a failed
/// promote AFTER `commit_lock` release + Phase 7 does NOT defer the tx
/// (data + index already committed under the lock; the WAL marker is gone;
/// the graph reconciles via rebuild-on-open). Persisted across the bounded
/// retry so the failure is treated as persistent.
#[cfg(test)]
pub(crate) static FAIL_VECTOR_PROMOTE_TX_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Async-mode helper: run Phase 5a (data) inline on the client path.
///
/// Mirrors the per-table loop in [`crate::tx::commit::materialize`] (drain → bounded retry →
/// log warn on persistent failure) but writes outcomes into a shared
/// `Arc<AtomicBool>` so the background tail can flip its final state to
/// `Deferred` if a data write actually failed. The DATA write must finish
/// before ack so read-your-own-writes on data holds.
pub(crate) async fn apply_data_phase(tx: &mut TxContext, repo: &RepoInstance, commit_version: u64) {
    let tx_id = tx.tx_id.0;
    let data_batches = collect_data_batches(tx);
    for (table_id, base, ops) in data_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_data_batch(
                repo,
                table_id,
                base.clone(),
                ops.clone(),
                commit_version,
                tx_id,
            )
        })
        .await
        {
            // A data-apply failure on the ack path is logged here and
            // RE-SURFACED via the inflight WAL marker — the tail will see
            // an empty staging (data already drained) but the recovery
            // contract still holds: the marker is left inflight, recovery
            // replays the WAL entry. Mark the tx as having an async-prefix
            // failure so the background tail forces `Deferred`.
            log::warn!(
                "commit_tx (async) Phase 5a (data) failed for tx {tx_id} \
                 commit_version {commit_version} table {table_id}: {e}; deferring to recovery"
            );
            tx.async_prefix_failed = true;
        }
    }
}

/// Async-mode helper: run Phase 5b (counter) inline on the client path.
/// Counter persistence is best-effort; a failure here is metric-only drift
/// (same contract as sync mode).
pub(crate) async fn apply_counter_phase(tx: &TxContext, repo: &RepoInstance) {
    let tx_id = tx.tx_id.0;
    for (table_id, delta) in &tx.counter_deltas {
        match repo.table_by_token(*table_id).await {
            Ok(Some(tbl)) => {
                if let Err(e) = tbl.counter().increment(*delta).await {
                    log::warn!(
                        "commit_tx (async) Phase 5b (counter) failed for tx {tx_id} \
                         table {table_id}: {e}; counter drift accepted (metric only)"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!(
                    "commit_tx (async) Phase 5b (counter) table lookup failed for \
                     tx {tx_id} table {table_id}: {e}; counter drift accepted (metric only)"
                );
            }
        }
    }
}

/// Async-mode helper: run the materialization TAIL on the background task.
///
/// Phases (in order): 5c (index) → drop `uwl_guards` → 6.5 (markers) →
/// 7 (`wal.commit`). NEVER aborts: a sub-phase failure flips the returned
/// state to `Deferred` and leaves the WAL marker inflight, exactly like the
/// sync deferral path. `recover_v2_inflight` is the eventual reconciler.
pub(crate) async fn materialize_async_tail(
    tx: &mut TxContext,
    repo: &RepoInstance,
    commit_version: u64,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> MaterializationState {
    let tx_id = tx.tx_id.0;
    let mut ok = !tx.async_prefix_failed;

    // Phase 5c: index postings.
    if !tx.index_write_set.is_empty() {
        let mut by_token: std::collections::HashMap<u64, Vec<IndexWriteOp>, THasher> =
            std::collections::HashMap::with_capacity_and_hasher(
                tx.index_write_set.len(),
                THasher::default(),
            );
        for (token, op) in &tx.index_write_set {
            by_token.entry(*token).or_default().push(op.clone());
        }
        for (token, ops) in by_token {
            if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
                apply_index_batch(repo, token, ops.clone(), tx_id)
            })
            .await
            {
                log::warn!(
                    "commit_tx (async) Phase 5c (index) failed for tx {tx_id} \
                     commit_version {commit_version} table {token}: {e}; deferring to recovery"
                );
                ok = false;
            }
        }
    }

    // Release per-table unique_write_lock guards (HIGH-A): the Phase 2.6
    // guard window has closed (postings either published or deferred to
    // recovery; recovery is idempotent). Released even on a deferred 5c —
    // a stuck guard would only block non-tx writers.
    drop(uwl_guards);

    // Phase 6.5: persist recovery markers.
    if let Ok(gate) = repo.tx_gate().await {
        if let Err(e) = persist_markers(repo, gate.as_ref(), commit_version).await {
            log::warn!(
                "commit_tx (async) Phase 6.5 (recovery markers) failed for tx {tx_id} \
                 commit_version {commit_version}: {e}; deferring to recovery"
            );
            ok = false;
        }
    } else {
        ok = false;
    }

    // Phase 7: WAL cleanup ONLY on full success.
    if ok {
        if let Ok(wal) = repo.repo_wal().await {
            if let Err(e) = wal.commit(tx_id).await {
                log::warn!(
                    "commit_tx (async) Phase 7 (WAL cleanup) failed for tx {tx_id} \
                     commit_version {commit_version}: {e}; marker left inflight for recovery"
                );
                ok = false;
            }
        } else {
            ok = false;
        }
    } else {
        log::warn!(
            "commit_tx (async) tx {tx_id} commit_version {commit_version} COMMITTED but \
             materialization DEFERRED — WAL marker left inflight; recovery will \
             reconcile main/info on the next open"
        );
    }

    if ok {
        MaterializationState::Complete
    } else {
        MaterializationState::Deferred
    }
}

/// cancel-safe: NO, and it does NOT need to be — by the time this runs the
/// tx is fully COMMITTED *and* materialized (Phases 5a/5c published the
/// data + index under `commit_lock`, Phase 7 removed the WAL marker). This
/// promotes the tx's staged HNSW vectors (`tx.staged_vectors`) into the
/// live graph OUTSIDE the commit critical section (III.5).
///
/// Why this runs after the lock is dropped:
///   The HNSW graph is a DERIVED read-accelerator, not a source of truth.
///   The vectors themselves are already durable in `main` (Phase 5a + the
///   Phase 4 WAL entry); the graph is re-derived from the data store by
///   `VectorBackend::rebuild` (`index2/vector/vector_backend.rs`) on open.
///   So the promote determines no visibility a committer must serialise on,
///   and — for bulk vector commits — its unbounded work (the brute-force
///   adapter's bounded-actor `submit().await`, the HNSW adapter's
///   per-vector `spawn_blocking`) must not run under `commit_lock`, where
///   it would stall every other committer (audit MED).
///
/// Why a failure here is NOT `MaterializationState::Deferred`:
///   The Deferred contract is "a visibility-bearing projection didn't land
///   inline, so the inflight WAL marker is the recovery guarantor." But
///   HNSW is NOT replayed from the WAL — vectors are not serialised as
///   `IndexPut`; they are derived. Phase 7 has ALREADY removed the marker
///   (data + index committed and materialized under the lock), so there is
///   no marker to lean on and nothing for recovery to replay for the
///   graph. A failed post-lock promote simply means the in-memory graph
///   lags the data until the next `rebuild()` on open reconciles it —
///   exactly the derived-projection contract. We therefore log a warning
///   and leave `materialization` untouched (`Complete`).
pub(crate) async fn promote_vectors(tx: &TxContext, repo: &RepoInstance, commit_version: u64) {
    let tx_id = tx.tx_id.0;

    // We iterate exactly the tables the tx staged vectors into
    // (`tx.staged_vectors`), keyed by table token. `apply_staged_vectors`
    // is a no-op for every backend except `VectorBackend`. Phase 5a only
    // drained `tx.write_set`, so `tx.staged_vectors` is intact here.
    let vector_batches = tx
        .staged_vectors
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(t, v)| (*t, v.clone()))
        .collect::<Vec<_>>();
    for (token, vecs) in vector_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_batch(repo, token, vecs.clone(), tx_id)
        })
        .await
        {
            // NOT Deferred: data + index already committed and materialized
            // under the lock; the WAL marker is gone (Phase 7). The graph
            // reconciles via `VectorBackend::rebuild` on the next open.
            log::warn!(
                "commit_tx Phase 5d (hnsw promote, post-lock) failed for tx {tx_id} \
                 commit_version {commit_version} table {token}: {e}; the live graph \
                 lags until rebuild-on-open reconciles it (tx stays COMMITTED + \
                 materialized — NOT deferred)"
            );
        }
    }
}

/// Drain each table's `StagingStore` into an owned `(token, base, ops)`
/// batch. Done once up front so a bounded retry re-applies the SAME ops
/// (idempotent at the data layer). Empty batches are dropped.
#[allow(clippy::type_complexity)]
pub(crate) fn collect_data_batches(
    tx: &mut TxContext,
) -> Vec<(
    u64,
    std::sync::Arc<dyn shamir_storage::types::Store>,
    Vec<shamir_storage::types::KvOp>,
)> {
    let mut out = Vec::with_capacity(tx.write_set.len());
    for (table_id, staging) in std::mem::take(&mut tx.write_set) {
        let base = staging.base().clone();
        let ops = staging.drain();
        if ops.is_empty() {
            continue;
        }
        out.push((table_id, base, ops));
    }
    out
}

/// Apply one table's data ops (Phase 5a). Routes through MvccStore when
/// the table is registered in `per_table_mvcc`, else `base.transact`.
pub(crate) async fn apply_data_batch(
    repo: &RepoInstance,
    table_id: u64,
    base: std::sync::Arc<dyn shamir_storage::types::Store>,
    ops: Vec<shamir_storage::types::KvOp>,
    commit_version: u64,
    _tx_id: u64,
) -> Result<(), DbError> {
    // Test-only failure injection (audit MED): simulate a persistent Phase
    // 5a storage error for a SPECIFIC `(table_token, tx_id)` so a tx writing
    // several tables can have exactly its second table's data write fail —
    // the partial cross-table materialization the audit flagged. Both
    // registers must match; armed via `FAIL_PHASE_5A_TX_ID` +
    // `FAIL_PHASE_5A_TABLE_TOKEN`.
    #[cfg(test)]
    {
        use std::sync::atomic::Ordering::SeqCst;
        let armed_tx = FAIL_PHASE_5A_TX_ID.load(SeqCst);
        let armed_token = FAIL_PHASE_5A_TABLE_TOKEN.load(SeqCst);
        if _tx_id != 0 && armed_tx == _tx_id && armed_token == table_id {
            return Err(DbError::Internal(format!(
                "injected Phase 5a (data) failure for tx {_tx_id} table {table_id}"
            )));
        }
    }

    let mvcc_found = repo
        .per_table_mvcc()
        .read_async(&table_id, |_, mvcc| std::sync::Arc::clone(mvcc))
        .await;
    match mvcc_found {
        Some(mvcc) => mvcc.apply_committed_ops(ops, commit_version).await,
        None => base.transact(ops).await,
    }
}

/// Apply one table's staged index ops (Phase 5c).
pub(crate) async fn apply_index_batch(
    repo: &RepoInstance,
    token: u64,
    ops: Vec<IndexWriteOp>,
    _tx_id: u64,
) -> Result<(), DbError> {
    // Test-only failure injection: simulate a persistent Phase 5c storage
    // error for a specific tx so a post-commit-point failure can be
    // exercised in-process (Vector I.3). Armed via `FAIL_PHASE_5C_TX_ID`.
    #[cfg(test)]
    if _tx_id != 0 && FAIL_PHASE_5C_TX_ID.load(std::sync::atomic::Ordering::SeqCst) == _tx_id {
        return Err(DbError::Internal(format!(
            "injected Phase 5c failure for tx {_tx_id}"
        )));
    }

    if let Some(tbl) = repo.table_by_token(token).await? {
        let backends = tbl.index2_registry().all_backends().await;
        crate::index2::write_ops::apply_index_ops_at_commit(&ops, tbl.info_store(), &backends)
            .await
            .map_err(|e| DbError::Internal(format!("index apply at commit: {e}")))?;
    }
    Ok(())
}

/// Promote one table's staged HNSW vectors into the live graph (Phase 5d,
/// post-lock — see [`promote_vectors`]).
pub(crate) async fn apply_vector_batch(
    repo: &RepoInstance,
    token: u64,
    vecs: Vec<(shamir_types::types::record_id::RecordId, Vec<f32>)>,
    _tx_id: u64,
) -> Result<(), DbError> {
    // Test-only failure injection: simulate a persistent post-lock HNSW
    // promote error for a specific tx so the III.5 contract (a failed
    // promote after the lock + Phase 7 does NOT defer the tx) can be
    // exercised in-process. Armed via `FAIL_VECTOR_PROMOTE_TX_ID`.
    #[cfg(test)]
    if _tx_id != 0 && FAIL_VECTOR_PROMOTE_TX_ID.load(std::sync::atomic::Ordering::SeqCst) == _tx_id
    {
        return Err(DbError::Internal(format!(
            "injected Phase 5d (hnsw promote) failure for tx {_tx_id}"
        )));
    }

    if let Some(tbl) = repo.table_by_token(token).await? {
        for backend in tbl.index2_registry().all_backends().await {
            backend.apply_staged_vectors(&vecs).await.map_err(|e| {
                DbError::Internal(format!("hnsw apply_staged_vectors at commit: {e}"))
            })?;
        }
    }
    Ok(())
}

/// Persist `last_committed_version` + `next_tx_id` recovery markers
/// (Phase 6.5).
pub(crate) async fn persist_markers(
    repo: &RepoInstance,
    gate: &shamir_tx::RepoTxGate,
    commit_version: u64,
) -> Result<(), DbError> {
    use crate::meta::recovery_marker::{save_last_committed, save_next_tx_id_snapshot};
    let info_store = repo.tx_info_store().await?;
    save_last_committed(&info_store, commit_version).await?;
    save_next_tx_id_snapshot(&info_store, gate.peek_next_tx_id()).await?;
    Ok(())
}

/// cancel-safe: yes — sequential bounded retry; each attempt fully
/// `.await`s before the next, so cancellation drops the in-flight attempt
/// with no straddled borrow. Re-runs `op` up to `attempts` times, taking
/// the first `Ok`. Returns the last `Err` if all attempts fail. Safe to
/// retry only because every materialization sub-phase is idempotent at
/// the data layer (last-write-wins).
pub(crate) async fn retry_materialize<F, Fut>(attempts: u32, mut op: F) -> Result<(), DbError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), DbError>>,
{
    let mut last_err: Option<DbError> = None;
    for _ in 0..attempts.max(1) {
        match op().await {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| DbError::Internal("retry_materialize: no attempt run".into())))
}
