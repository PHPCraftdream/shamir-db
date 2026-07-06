use shamir_collections::TFxMap;
use shamir_collections::THasher;
use shamir_storage::error::DbError;
use shamir_tunables::instance_defaults::INTERNER_CHECKPOINT_INTERVAL;
use shamir_tx::{IndexWriteOp, TxContext};

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
/// promote (Phase 5d, `apply_vector_graph_batch`) returns a synthetic error
/// for the matching tx. Used by `commit_phase5_tests` to prove III.5: a
/// failed promote AFTER `commit_lock` release + Phase 7 does NOT defer the
/// tx (data + index already committed under the lock; the WAL marker is
/// gone; the graph reconciles via restore-on-open of the durable delta
/// chunk that was appended PRE-publish by `apply_vector_delta_phase`).
/// Persisted across the bounded retry so the failure is treated as
/// persistent.
#[cfg(test)]
pub(crate) static FAIL_VECTOR_PROMOTE_TX_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Test-only injection (#426, VR-4 Phase 5d durability variant A): when set
/// to a non-zero tx_id, the PRE-publish delta-chunk append
/// (`apply_vector_delta_batch` / `apply_vector_delta_phase`) returns a
/// synthetic error for the matching tx. Unlike [`FAIL_VECTOR_PROMOTE_TX_ID`]
/// (the graph-half), a delta-half failure MUST surface as `Deferred`: the
/// durable delta-log is the bridge `restore_on_open` relies on, so a
/// pre-publish delta-append failure leaves the vector mutation without a
/// durable echo — the tx is COMMITTED-by-data but the vector is not
/// durably materialized, so recovery cannot reconcile it. Surfacing
/// `Deferred` (not a silent `Complete`) gives the client a detectable
/// signal to retry. Persisted across the bounded retry so the failure is
/// treated as persistent → deferral (not a transient retry success).
#[cfg(test)]
pub(crate) static FAIL_VECTOR_DELTA_TX_ID: std::sync::atomic::AtomicU64 =
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

/// VR-4 (#426) Phase 5d durability — variant A: append the durable
/// delta-log chunk(s) for this tx's staged vectors + deletes BEFORE
/// `version_guard.commit()` (pre-publish), so `restore_on_open`'s
/// `replay_delta` re-materializes the vectors over the base snapshot on
/// the next open. Closes the W-2 window where a post-ack crash could
/// leave a committed vector WITHOUT a durable delta echo (the live graph
/// is RAM-only and dies with the process; the delta-log was the only
/// durable bridge, and before #426 it was appended POST-publish).
///
/// This runs the DELTA half only. The GRAPH half
/// (`apply_staged_vector_deletes` + `apply_staged_vectors`, which mutate
/// the in-RAM HNSW graph) stays in `promote_vectors` AFTER the lock — it
/// is a derived read-accelerator (III.5) and its unbounded per-vector work
/// must not stall other committers under `commit_lock`.
///
/// Failure semantics (§5.3 of the design): a persistent delta-append
/// failure flips the tx to `Deferred`. The WAL marker is left inflight so
/// recovery re-applies the data; BUT recovery does NOT replay the vector
/// mutation (vectors are not serialised in the WAL — they are derived).
/// So a delta failure is still a loss of the vector mutation from the
/// live graph — BUT now it is DETECTABLE pre-ack (`Deferred` +
/// warn-log), unlike the prior silent post-ack loss. The client can retry
/// the tx. Aborting before publish is impossible (Phase 4 already made
/// the data durable); the `Deferred` signal is the honest surface.
///
/// Idempotency: `replay_delta` applies `DeltaOp::Upsert`/`Delete` via
/// `adapter.upsert/delete` (last-write-wins), so a duplicate chunk on a
/// re-attempt after restart converges. `next_delta_idx.fetch_add` in
/// `append_vector_delta` may produce a duplicate chunk index on retry —
/// `Store::set` overwrites the same chunk key (last-writer-wins).
pub(crate) async fn apply_vector_delta_phase(
    tx: &mut TxContext,
    repo: &RepoInstance,
    _commit_version: u64,
) {
    let tx_id = tx.tx_id.0;

    // Tables with staged vector INSERTS. `staged_vectors` survives Phase 5a
    // (which drained only `tx.write_set`), so the slices are intact here.
    let insert_batches = tx
        .staged_vectors
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(t, v)| (*t, v.clone()))
        .collect::<Vec<_>>();
    for (token, vecs) in insert_batches {
        // This table's staged vector deletes ride the SAME delta chunk as
        // the inserts (gap#1: a replace = delete old + insert new on the
        // same rid). `append_vector_delta` writes both `DeltaOp::Upsert`
        // and `DeltaOp::Delete` ops in one chunk so a restart replays
        // tombstone-then-upsert over the base snapshot.
        let deleted: Vec<shamir_types::types::record_id::RecordId> = tx
            .staged_vector_deletes_for(token)
            .map(|s| s.to_vec())
            .unwrap_or_default();
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_delta_batch(repo, token, &vecs, &deleted, tx_id)
        })
        .await
        {
            log::warn!(
                "commit_tx Phase 5d (delta-append, pre-publish) failed for tx {tx_id} \
                 commit_version {_commit_version} table {token}: {e}; tx will be reported \
                 Deferred — the vector mutation has no durable delta echo and recovery will \
                 NOT re-apply it (vectors are derived, not WAL-replayed); client should retry"
            );
            tx.async_prefix_failed = true;
        }
    }

    // gap#1: tables that have ONLY staged vector deletes (no staged
    // inserts). The loop above only visited tables present in
    // `staged_vectors`; this second loop visits the delete-only tables so
    // their tombstones get a durable `DeltaOp::Delete` echo too.
    let delete_only_batches = tx
        .staged_vector_deletes
        .iter()
        .filter(|(t, dels)| !dels.is_empty() && !tx.staged_vectors.contains_key(t))
        .map(|(t, dels)| (*t, dels.clone()))
        .collect::<Vec<_>>();
    for (token, deleted) in delete_only_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_delta_batch(repo, token, &[], &deleted, tx_id)
        })
        .await
        {
            log::warn!(
                "commit_tx Phase 5d (delta-append delete-only, pre-publish) failed for tx \
                 {tx_id} commit_version {_commit_version} table {token}: {e}; tx will be \
                 reported Deferred — the vector delete has no durable delta echo"
            );
            tx.async_prefix_failed = true;
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
        let mut by_token: TFxMap<u64, Vec<IndexWriteOp>> =
            TFxMap::with_capacity_and_hasher(tx.index_write_set.len(), THasher::default());
        for (token, op) in std::mem::take(&mut tx.index_write_set) {
            by_token.entry(token).or_default().push(op);
        }
        for (token, ops) in by_token {
            if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
                apply_index_batch(repo, token, &ops, tx_id)
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

    // A5 + Stage I: capture the max interner id across the tx's per-repo
    // delta for the interner checkpoint below. Pre-Stage-I this was a
    // per-table `Vec<(token, max_id)>`; the interner is now per-repo so it
    // collapses to ONE max id (or `None` if the delta was empty).
    let interner_delta_max_id: Option<u64> = tx.interner_deltas.iter().map(|(_, id)| *id).max();

    // A5 + Stage I: background interner checkpoint on interval. One persist
    // covers the whole repo (the manager is Arc-shared across tables).
    if commit_version.is_multiple_of(INTERNER_CHECKPOINT_INTERVAL)
        && interner_delta_max_id.is_some()
    {
        let repo_ck = repo.clone();
        tokio::spawn(async move {
            match repo_ck.repo_interner().await {
                Ok(repo_interner) => {
                    if let Err(e) = repo_interner.persist().await {
                        log::warn!(
                            "A5 interner checkpoint (async) failed for repo {}: {e}",
                            repo_ck.name()
                        );
                    }
                }
                Err(e) => {
                    log::warn!(
                        "A5 interner checkpoint (async): repo_interner resolve for {}: {e}",
                        repo_ck.name()
                    );
                }
            }
        });
    }

    // D2 P1d-2b FIX: Phase 7 (WAL truncation) is NO LONGER done here. Post-
    // cutover the AsyncIndex ack-path writes ONLY the in-memory overlay (via
    // `apply_data_batch` → `apply_committed_visible`) — the value is durable in
    // `history` only after the background `Drainer` replays the inflight WAL
    // entry. Truncating the marker here would remove the WAL entry while the
    // data lives ONLY in the (volatile) overlay → DATA LOSS on crash, and would
    // wedge `durable_watermark` (the drainer could never mark this version
    // durable because its entry is gone). The drainer is the SOLE caller of
    // `wal.commit` now: it writes history → `mark_durable` → A5-gated truncate,
    // exactly as the lockfree path's `post_publish_cleanup` was changed to.
    if !ok {
        log::warn!(
            "commit_tx (async) tx {tx_id} commit_version {commit_version} COMMITTED but \
             materialization DEFERRED — recovery / drainer will reconcile main/info"
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
///   Phase 4 WAL entry) AND their durable delta chunk was appended
///   PRE-publish by `apply_vector_delta_phase` (#426, VR-4 variant A); the
///   graph is re-derived from snapshot + delta by
///   `VectorBackend::restore_on_open` (`index2/vector/vector_backend.rs`)
///   on open. So the promote determines no visibility a committer must
///   serialise on, and — for bulk vector commits — its unbounded work (the
///   brute-force adapter's bounded-actor `submit().await`, the HNSW
///   adapter's per-vector `spawn_blocking`) must not run under
///   `commit_lock`, where it would stall every other committer (audit MED).
///
/// Why a failure here is NOT `MaterializationState::Deferred`:
///   The Deferred contract is "a visibility-bearing projection didn't land
///   inline, so the inflight WAL marker is the recovery guarantor." But
///   HNSW is NOT replayed from the WAL — vectors are not serialised as
///   `IndexPut`; they are derived. Phase 7 has ALREADY removed the marker
///   (data + index committed and materialized under the lock), so there is
///   no marker to lean on and nothing for recovery to replay for the
///   graph. A failed post-lock GRAPH promote simply means the in-memory
///   graph lags the data — but the DURABLE delta chunk was already
///   appended PRE-publish by `apply_vector_delta_phase` (#426, VR-4 variant
///   A), so `restore_on_open::replay_delta` applies it on the next open.
///   We therefore log a warning and leave `materialization` untouched
///   (`Complete`). The delta chunk is the durable bridge; the graph is
///   re-derived from snapshot + delta on open.
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
        // gap#1: gather this table's staged vector deletes so they ride
        // the SAME commit-ack promote pass as the inserts. A replace
        // (delete old + insert new on the same rid) lands BOTH here: the
        // old rid in `deleted` and the new embedding in `vecs`. Order is
        // apply-deletes-then-inserts inside `apply_vector_graph_batch`,
        // which is the inverse-direction-safe order for HNSW (a tombstone
        // followed by an upsert on the same rid leaves the NEW vector
        // live — `upsert` re-inserts the node; `delete` first is a no-op
        // on a not-yet-inserted node, so either order converges, but
        // delete-then-insert matches the replace intent literally). The
        // durable delta chunk (with BOTH ops) was already appended
        // PRE-publish by `apply_vector_delta_phase`.
        let deleted: Vec<shamir_types::types::record_id::RecordId> = tx
            .staged_vector_deletes_for(token)
            .map(|s| s.to_vec())
            .unwrap_or_default();
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_graph_batch(repo, token, &vecs, &deleted, tx_id)
        })
        .await
        {
            // NOT Deferred: data + index already committed and materialized
            // under the lock; the WAL marker is gone (Phase 7). The DURABLE
            // delta chunk was appended PRE-publish by
            // `apply_vector_delta_phase`, so `restore_on_open::replay_delta`
            // re-materializes the vectors over the base snapshot on the next
            // open. The live in-RAM graph lags until then — derived, not
            // WAL-replayed.
            log::warn!(
                "commit_tx Phase 5d (hnsw graph promote, post-lock) failed for tx {tx_id} \
                 commit_version {commit_version} table {token}: {e}; the live graph \
                 lags until restore_on_open replays the pre-publish durable delta \
                 (tx stays COMMITTED + materialized — NOT deferred)"
            );
        }
    }

    // gap#1: tables that have ONLY staged vector deletes (no staged
    // inserts) still need their deletes promoted — otherwise a tx that
    // deletes a vector-backed row without inserting a new one leaves a
    // ghost in the live graph + delta. The loop above only visits tables
    // present in `staged_vectors`; this second loop visits the
    // delete-only tables.
    let delete_only_batches = tx
        .staged_vector_deletes
        .iter()
        .filter(|(t, dels)| !dels.is_empty() && !tx.staged_vectors.contains_key(t))
        .map(|(t, dels)| (*t, dels.clone()))
        .collect::<Vec<_>>();
    for (token, deleted) in delete_only_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_graph_batch(repo, token, &[], &deleted, tx_id)
        })
        .await
        {
            log::warn!(
                "commit_tx Phase 5d (hnsw graph delete promote, post-lock) failed for tx {tx_id} \
                 commit_version {commit_version} table {token}: {e}; the live graph \
                 lags until restore_on_open replays the pre-publish durable delta \
                 (tx stays COMMITTED + materialized — NOT deferred)"
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
        // D2 P1d-2b CUTOVER: the ack-path writes ONLY the in-memory visible
        // half (overlay + cell + floor). It no longer writes `history` inline
        // — the background `Drainer` (replaying the durable WAL entry via
        // `write_committed_to_history`) is now the SOLE history writer. The WAL
        // entry (Phase 4, already durable) is the source of truth; the overlay
        // holds the single RAM copy of the value until the drainer makes it
        // durable. Crash before drain ⇒ inflight WAL ⇒ recovery replays.
        Some(mvcc) => {
            mvcc.apply_committed_visible(&ops, commit_version);
            Ok(())
        }
        // Non-MVCC tables (system / test) have NO overlay and are NOT drained
        // — they have no version-log. Keep the direct durable write inline:
        // this is the rare unattached-table path, and its data is replayed
        // from the WAL on recovery exactly as before the cutover.
        None => base.transact(ops).await,
    }
}

/// Apply one table's staged index ops (Phase 5c).
pub(crate) async fn apply_index_batch(
    repo: &RepoInstance,
    token: u64,
    ops: &[IndexWriteOp],
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
        crate::index2::write_ops::apply_index_ops_at_commit(ops, tbl.info_store(), &backends)
            .await
            .map_err(|e| DbError::Internal(format!("index apply at commit: {e}")))?;
        // Invalidate the legacy IndexManager's posting cache for every
        // SetPosting / RemovePosting that was just durably applied, so the
        // next lookup_by_index re-fetches from the store instead of
        // returning a stale cached result.
        tbl.index_manager_ref()
            .invalidate_posting_cache_for_ops(ops);
    }
    Ok(())
}

/// Promote one table's staged HNSW vectors + vector deletes into the live
/// in-RAM graph (Phase 5d GRAPH half, post-lock — see [`promote_vectors`]).
///
/// gap#1 / HIGH-6: `deleted` is this table's slice of staged vector-delete
/// rids (`tx.staged_vector_deletes_for(token)`). The promote order is
/// delete-then-insert: a replace (delete old + insert new on the same rid)
/// converges to the NEW vector live (HNSW `upsert` re-inserts a tombstoned
/// node; `delete` on a not-yet-inserted node is a no-op, so either order is
/// safe, but delete-then-insert matches the replace intent literally).
///
/// #426 (VR-4 variant A): this is the GRAPH half only. The DURABLE delta
/// chunk (`DeltaOp::Upsert`/`Delete`) is appended PRE-publish by
/// [`apply_vector_delta_batch`] (called from [`apply_vector_delta_phase`]),
/// so a post-lock graph-promote failure leaves the mutation durable in the
/// delta-log — `restore_on_open::replay_delta` re-materializes it on the
/// next open.
pub(crate) async fn apply_vector_graph_batch(
    repo: &RepoInstance,
    token: u64,
    vecs: &[(shamir_types::types::record_id::RecordId, Vec<f32>)],
    deleted: &[shamir_types::types::record_id::RecordId],
    _tx_id: u64,
) -> Result<(), DbError> {
    // Test-only failure injection: simulate a persistent post-lock HNSW
    // graph-promote error for a specific tx so the III.5 contract (a failed
    // graph promote after the lock + Phase 7 does NOT defer the tx) can be
    // exercised in-process. Armed via `FAIL_VECTOR_PROMOTE_TX_ID`. NOTE:
    // this injection fires ONLY for the graph half; the delta half has its
    // own injection (`FAIL_VECTOR_DELTA_TX_ID`) and its own (Deferred)
    // failure contract.
    #[cfg(test)]
    if _tx_id != 0 && FAIL_VECTOR_PROMOTE_TX_ID.load(std::sync::atomic::Ordering::SeqCst) == _tx_id
    {
        return Err(DbError::Internal(format!(
            "injected Phase 5d (hnsw graph promote) failure for tx {_tx_id}"
        )));
    }

    if let Some(tbl) = repo.table_by_token(token).await? {
        for backend in tbl.index2_registry().all_backends().await {
            // gap#1: tombstone the deleted rids on the live graph FIRST,
            // so a replace (same rid) leaves the NEW vector live after the
            // insert promote below. `apply_staged_vector_deletes` is a
            // no-op for non-vector backends.
            if !deleted.is_empty() {
                backend
                    .apply_staged_vector_deletes(deleted)
                    .await
                    .map_err(|e| {
                        DbError::Internal(format!(
                            "hnsw apply_staged_vector_deletes at commit: {e}"
                        ))
                    })?;
            }
            backend.apply_staged_vectors(vecs).await.map_err(|e| {
                DbError::Internal(format!("hnsw apply_staged_vectors at commit: {e}"))
            })?;
        }
    }
    Ok(())
}

/// Append one table's durable delta-log chunk for its staged vectors +
/// deletes (Phase 5d DELTA half, pre-publish — see
/// [`apply_vector_delta_phase`]).
///
/// #426 (VR-4 variant A): this is the DELTA half only. It runs BEFORE
/// `version_guard.commit()` so the delta chunk is durable by the time the
/// tx is reader-visible. A restart loads snapshot + delta via
/// `restore_on_open::replay_delta` and re-materializes the vectors. The
/// GRAPH half ([`apply_vector_graph_batch`]) stays post-lock.
///
/// Idempotency: `replay_delta` applies ops via `adapter.upsert/delete`
/// (last-write-wins); a duplicate chunk on retry overwrites the same
/// `next_delta_idx` key (`Store::set` is last-writer-wins).
pub(crate) async fn apply_vector_delta_batch(
    repo: &RepoInstance,
    token: u64,
    vecs: &[(shamir_types::types::record_id::RecordId, Vec<f32>)],
    deleted: &[shamir_types::types::record_id::RecordId],
    _tx_id: u64,
) -> Result<(), DbError> {
    // Test-only failure injection (#426, VR-4 variant A): simulate a
    // persistent PRE-publish delta-append error for a specific tx so the
    // Deferred-failure contract (a failed delta append surfaces as
    // `MaterializationState::Deferred`, NOT a silent Complete) can be
    // exercised in-process. Armed via `FAIL_VECTOR_DELTA_TX_ID`. This is
    // DISTINCT from `FAIL_VECTOR_PROMOTE_TX_ID` (graph half): a delta-half
    // failure loses the durable bridge, so it MUST be reported; a
    // graph-half failure leaves the delta durable, so it stays Complete.
    #[cfg(test)]
    if _tx_id != 0 && FAIL_VECTOR_DELTA_TX_ID.load(std::sync::atomic::Ordering::SeqCst) == _tx_id {
        return Err(DbError::Internal(format!(
            "injected Phase 5d (delta-append) failure for tx {_tx_id}"
        )));
    }

    if let Some(tbl) = repo.table_by_token(token).await? {
        let info_store = tbl.info_store().clone();
        for backend in tbl.index2_registry().all_backends().await {
            // V2.3 (#402) — durable delta-log append + snapshot trigger.
            // The delta chunk captures the vectors just promoted AND the
            // deletes just tombstoned so a restart replays both over the
            // base snapshot. The append is ONE `Store::set` (§5.6 — cheap,
            // runs on the ack path). The snapshot trigger is a
            // `tokio::spawn` when the threshold is crossed (§5.6 — the
            // dump itself is off the ack path).
            //
            // gap#1 (variant-A, landed): tx-path vector deletes are now
            // staged in `TxContext::staged_vector_deletes`, and handed
            // here as `deleted` so the delta chunk carries a durable
            // `DeltaOp::Delete` per rid. A restart replays the delete over
            // the base snapshot (`replay_delta` applies `DeltaOp::Delete`
            // via `adapter.delete`), closing the post-restart ghost. The
            // mechanism was proven by
            // `delta_log_tests::append_vector_delta_with_deleted_slice_persists_and_replays_delete`.
            //
            // #426 (VR-4 variant A): this call was MOVED here from the old
            // post-lock `apply_vector_batch` so the chunk is durable
            // PRE-publish. The graph-side `apply_staged_vector_deletes` +
            // `apply_staged_vectors` stayed post-lock in
            // `apply_vector_graph_batch`.
            backend
                .append_vector_delta(&info_store, vecs, deleted)
                .await
                .map_err(|e| DbError::Internal(format!("delta append at commit: {e}")))?;
            backend.trigger_snapshot_check(&info_store);
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
