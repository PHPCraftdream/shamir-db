use futures::future::join_all;
use shamir_tunables::instance_defaults::INTERNER_CHECKPOINT_INTERVAL;
use shamir_tx::{RepoTxGate, RepoWalManager, TxContext};
use shamir_types::types::common::THasher;

use crate::repo::RepoInstance;
use crate::tx::commit::maybe_crash;
use crate::tx::commit_phases::{
    apply_data_batch, apply_index_batch, collect_data_batches, persist_markers, retry_materialize,
    MATERIALIZE_ATTEMPTS,
};
use crate::tx::tx_outcome::MaterializationState;

/// Phases 5a–6 of the commit pipeline, run OUTSIDE `commit_lock` (P2b).
///
/// Gated by per-table `uwl_guards` (unique_write_lock) already held since
/// pre_commit_prelock. Disjoint-table commits materialize in parallel;
/// overlapping-table commits serialize on the uwl_guard, not on the global
/// commit_mutex.
///
/// cancel-safe: NO, and it does NOT need to be — by the time this runs
/// the tx is already COMMITTED (Phase 4 succeeded). It applies the WAL
/// entry's visibility-bearing projections (data → main, counter, index →
/// info) and publishes the version via P1c's contiguous-prefix watermark.
/// It does NOT promote HNSW vectors — that derived, rebuild-on-open
/// read-accelerator moved OUT of the commit critical section (III.5).
/// NONE of the projections here may abort the tx: a sub-phase failure is
/// logged and the WAL marker is left inflight so recovery re-applies the
/// entry on the next open (recovery is the materialization guarantor —
/// `Put`/`Delete`/`IndexPut`/`IndexDel` are last-write-wins, counter
/// replay is intentionally skipped, HNSW is rebuilt on open).
///
/// Returns a [`PostPublishState`] that the caller passes to
/// [`post_publish_cleanup`]. Phase 6.5 (markers) and Phase 7 (WAL cleanup)
/// are I/O-bound and also run outside the lock.
///
/// Phase 6 (`publish_committed_max`) ALWAYS runs — the version is committed
/// regardless of whether the projections landed inline.
///
/// Phase 6-bis (`record_commit_writes`) is NOT called here — it is called
/// by the caller UNDER `commit_lock` before releasing it, so that future
/// SSI validations see this tx's footprint. See `run_single_tx` /
/// `run_leader`.
///
/// MULTI-TABLE DEFERRAL IS PARTIAL (audit MED, by-design): the Phase 5a
/// (data) and Phase 5c (index) loops below iterate per table, each with its
/// own bounded retry. A failure on ONE table flips `ok` but does NOT halt
/// the other tables — so a tx touching tables A and B can materialize A
/// inline and leave B for recovery, yet `publish_committed_max` still
/// publishes the single shared `commit_version`. The result is a cross-table
/// / data-vs-index inconsistency that is *restart-bounded eventually
/// consistent*: it is reconciled only when the next `recover_v2_inflight`
/// replays the one inflight WAL entry (which carries every table's ops).
/// There is no online reconciler. This is honest, not reassuring — see
/// [`MaterializationState::Deferred`] for the reader-visible contract.
pub(super) async fn materialize(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    commit_version: u64,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> PostPublishState {
    let tx_id = tx.tx_id.0;
    let mut ok = true;

    // Phase 5a: physical data writes per table.
    //
    // Drain each per-table StagingStore once (so a retry re-applies the
    // SAME ops; idempotent at the data layer). Route through MvccStore
    // when available so version_cache stays current for SSI conflict
    // detection; fall back to direct `base.transact` otherwise.
    let data_batches = collect_data_batches(tx);
    let data_futs: Vec<_> = data_batches
        .into_iter()
        .map(|(table_id, base, ops)| async move {
            let res = retry_materialize(MATERIALIZE_ATTEMPTS, || {
                apply_data_batch(
                    repo,
                    table_id,
                    base.clone(),
                    ops.clone(),
                    commit_version,
                    tx_id,
                )
            })
            .await;
            (table_id, res)
        })
        .collect();
    for (table_id, res) in join_all(data_futs).await {
        if let Err(e) = res {
            log::warn!(
                "commit_tx materialization Phase 5a (data) failed for tx {tx_id} \
                 commit_version {commit_version} table {table_id}: {e}; deferring to recovery"
            );
            ok = false;
        }
    }

    // Crash seam (test-only): data (5a) is on disk but the index (5c) is
    // not, the version is unpublished, and Phase 7 has not run. The
    // inflight WAL marker survives → recovery replays the full entry.
    maybe_crash("phase5a", repo).await;

    // Phase 5b: counter deltas (CRIT-3).
    //
    // `tx.counter_deltas` is already serialised into the WAL entry by
    // `wal_ops_from_tx`. The happy path also applies it in-memory so
    // callers observe the new count without waiting for recovery.
    //
    // Recovery idempotency: the matching replay branch in
    // `recovery::replay_v2_op` for `WalOpV2::CounterDelta` is intentionally
    // SKIPPED at recovery time. Counter persistence is best-effort; the
    // durable `last_committed_version` marker (Phase 6.5) is the
    // authoritative MVCC milestone, and a per-tx counter-applied marker is
    // overkill for an at-most-by-one drift on a metric counter. Because of
    // that, a Phase 5b failure does NOT flip `ok` (recovery will not
    // re-apply the counter, and the doctor reconciles by full data scan).
    for (table_id, delta) in &tx.counter_deltas {
        match repo.table_by_token(*table_id).await {
            Ok(Some(tbl)) => {
                if let Err(e) = tbl.counter().increment(*delta).await {
                    log::warn!(
                        "commit_tx materialization Phase 5b (counter) failed for tx {tx_id} \
                         table {table_id}: {e}; counter drift accepted (metric only)"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!(
                    "commit_tx materialization Phase 5b (counter) table lookup failed for \
                     tx {tx_id} table {table_id}: {e}; counter drift accepted (metric only)"
                );
            }
        }
    }

    // Phase 5c: apply staged index ops (HIGH-6).
    //
    // `tx.index_write_set` is `Vec<(table_token, IndexWriteOp)>`, already
    // serialised into the WAL entry by `wal_ops_from_tx`. Group ops by
    // table_token, resolve each table once, then apply via
    // `apply_index_ops_at_commit`: SetPosting/RemovePosting hit the
    // info_store; BumpFtsStats is broadcast to the table's backends'
    // `apply_in_memory`.
    //
    // Recovery idempotency: `recovery::replay_v2_op` applies IndexPut
    // (→ info_store.set) and IndexDel (→ info_store.remove) for the same
    // postings. `set`/`remove` are last-write-wins, so re-replay converges.
    // BumpFtsStats is not serialised to the WAL; its in-memory counters are
    // rebuilt via `rebuild()` on open.
    if !tx.index_write_set.is_empty() {
        let mut by_token: std::collections::HashMap<u64, Vec<shamir_tx::IndexWriteOp>, THasher> =
            std::collections::HashMap::default();
        for (token, op) in std::mem::take(&mut tx.index_write_set) {
            by_token.entry(token).or_default().push(op);
        }
        let index_futs: Vec<_> = by_token
            .into_iter()
            .map(|(token, ops)| async move {
                let res = retry_materialize(MATERIALIZE_ATTEMPTS, || {
                    apply_index_batch(repo, token, &ops, tx_id)
                })
                .await;
                (token, res)
            })
            .collect();
        for (token, res) in join_all(index_futs).await {
            if let Err(e) = res {
                log::warn!(
                    "commit_tx materialization Phase 5c (index) failed for tx {tx_id} \
                     commit_version {commit_version} table {token}: {e}; deferring to recovery"
                );
                ok = false;
            }
        }
    }

    // Crash seam (test-only): both data (5a) and index (5c) are on disk
    // but the version is still unpublished and Phase 7 has not run. The
    // inflight WAL marker survives → recovery re-applies idempotently.
    maybe_crash("phase5c", repo).await;

    // HIGH-A: release the per-table unique_write_lock guards now that the
    // unique postings are published (Phase 5c done). The window the Phase
    // 2.6 guard had to dominate — re-check → posting write — is closed, so
    // a non-tx unique writer may resume. Phases 6/6.5/7 (and the post-lock
    // HNSW promote) do not touch unique postings, so holding the locks past
    // here would only add contention. Released even on a deferred Phase 5c:
    // recovery re-applies the postings, and a stuck guard would only block
    // non-tx writers.
    drop(uwl_guards);

    // Phase 5d (HNSW promote) is NO LONGER here. It moved OUT of the
    // commit critical section: `commit_tx_inner` drops `commit_lock` after
    // Phase 7 below and then calls `promote_vectors` (III.5). HNSW is a
    // derived read-accelerator (vectors are already in `main` via Phase 5a
    // + the WAL entry; the graph is rebuilt from the data store on open),
    // so its unbounded per-vector work must not stall other committers
    // under the gate, and a failed promote does NOT defer the tx — see
    // `promote_vectors`.

    // Phase 6: publish — atomic publish-committed. ALWAYS runs: the
    // version IS committed (the WAL entry is durable) regardless of
    // whether the projections above landed inline.
    // P1c: mark materialized in completion tracker; the watermark advances
    // and syncs the atomic last_committed_version via fetch_max.
    gate.completion().mark(
        commit_version,
        shamir_tx::completion_tracker::State::Materialized,
    );
    gate.sync_last_committed_from_watermark();
    // CAS-based publish: safe outside commit_lock (P2b). Moves the
    // reader-visible floor forward only; never backwards.
    gate.publish_committed_max(commit_version);

    // Crash seam (test-only): version published in-memory but lost with
    // the process; markers (6.5) not yet persisted and Phase 7 not run.
    // The inflight WAL marker survives → recovery materializes the tx.
    maybe_crash("phase6", repo).await;

    // A5: capture the max interner id per table from the tx's delta so
    // Phase 7 can gate WAL truncation on the persisted high-water mark.
    let interner_delta_max_ids: Vec<(u64, u64)> = tx
        .interner_deltas
        .iter()
        .filter_map(|(token, deltas)| deltas.iter().map(|(_, id)| *id).max().map(|m| (*token, m)))
        .collect();

    // Phase 6.5 + 7 (markers + WAL cleanup) are returned as post-lock
    // work — the caller runs them AFTER releasing `commit_lock`. See
    // `post_publish_cleanup`.
    PostPublishState {
        tx_id,
        commit_version,
        projections_ok: ok,
        interner_delta_max_ids,
    }
}

/// State carried from [`materialize`] to [`post_publish_cleanup`].
/// Both run outside `commit_lock` (P2b).
pub(super) struct PostPublishState {
    pub tx_id: u64,
    pub commit_version: u64,
    /// True iff Phases 5a–5c all succeeded. When false, Phase 7 is
    /// skipped so the WAL marker stays inflight for recovery.
    pub projections_ok: bool,
    /// A5: max interner id per table from the WAL entry's interner delta.
    /// Phase 7 checks that each table's persisted high-water mark covers
    /// these ids before truncating the WAL entry.
    pub interner_delta_max_ids: Vec<(u64, u64)>,
}

/// Phase 6.5 (markers) + Phase 7 (WAL cleanup), run OUTSIDE `commit_lock`.
///
/// These phases are pure crash-recovery bookkeeping: they do not affect
/// in-memory visibility (Phase 6 already published the version) and do not
/// need serialisation against other committers. Moving them out of the
/// lock shrinks the critical section by two `info_store.set` calls + one
/// WAL remove — measurable I/O that was serialising every committer.
///
/// Ordering is preserved: markers BEFORE WAL cleanup (the inverse is
/// unsafe — see the original Phase 6.5 comment). The only change is that
/// another committer may now enter the lock while these I/O ops run for
/// the previous commit. That is safe because:
///   - `publish_committed` already advanced `last_committed_version`;
///     the new committer sees the correct MVCC floor.
///   - Markers are a best-effort snapshot; recovery re-persists the floor
///     from `gate.last_committed()` when consuming any inflight WAL entry.
///   - WAL cleanup is idempotent (removing an already-absent marker is OK).
pub(super) async fn post_publish_cleanup(
    state: PostPublishState,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
) -> MaterializationState {
    let PostPublishState {
        tx_id,
        commit_version,
        mut projections_ok,
        interner_delta_max_ids,
    } = state;

    // Phase 6.5: persist recovery markers (CRIT-1).
    //
    // Write `last_committed_version` + `next_tx_id` to the repo's
    // `__tx__` info_store BEFORE Phase 7 removes the WAL marker.
    //
    // Ordering rationale:
    //
    // - Markers BEFORE wal.commit:
    //     If markers succeed but `wal.commit` fails, the WAL entry stays
    //     inflight and recovery re-applies it. Data writes are idempotent
    //     and counter replay is intentionally skipped.
    //
    // - Markers AFTER publish_committed:
    //     The in-memory `last_committed_version` is the runtime source of
    //     truth; the persisted copy only matters across restarts.
    //
    // The inverse order (wal.commit before markers) is unsafe: a crash
    // between the two would clear the WAL marker yet leave
    // `last_committed_version` stale on disk → version-monotonicity
    // violation on restart.
    //
    // Best-effort post-commit: a marker write failure is logged and flips
    // `projections_ok` (Phase 7 will be skipped so the marker stays
    // inflight). Even without these markers, `recover_inflight_v2`
    // re-persists the floor (`gate.last_committed()`) when it consumes
    // the inflight entry, so the version floor is restored on the next
    // open.
    if let Err(e) = persist_markers(repo, gate, commit_version).await {
        log::warn!(
            "commit_tx materialization Phase 6.5 (recovery markers) failed for tx {tx_id} \
             commit_version {commit_version}: {e}; deferring to recovery"
        );
        projections_ok = false;
    }

    // Crash seam (test-only): everything (data, index, markers) is on
    // disk but Phase 7 has NOT removed the WAL marker. The inflight
    // marker survives → recovery re-applies the (already-present) entry
    // idempotently and cleans the marker.
    maybe_crash("phase6_5", repo).await;

    // A5: background interner checkpoint. Every INTERNER_CHECKPOINT_INTERVAL
    // commits, persist each touched table's interner so the high-water mark
    // advances and future Phase 7 passes can truncate older WAL entries.
    // Spawned fire-and-forget: the persist is not on the commit critical path.
    if commit_version.is_multiple_of(INTERNER_CHECKPOINT_INTERVAL)
        && !interner_delta_max_ids.is_empty()
    {
        let repo_clone = repo.clone();
        tokio::spawn(async move {
            for table_name in repo_clone.list_table_names() {
                match repo_clone.get_table(&table_name).await {
                    Ok(tbl) => {
                        if let Err(e) = tbl.interner().persist().await {
                            log::warn!("A5 interner checkpoint failed for table {table_name}: {e}");
                        }
                    }
                    Err(e) => {
                        log::warn!("A5 interner checkpoint: get_table {table_name}: {e}");
                    }
                }
            }
        });
    }

    // A5: Phase 7 gating — WAL truncation is safe ONLY when every
    // interner id in the entry's delta has been durably persisted. If
    // the delta contains ids beyond a table's persisted high-water mark
    // the entry MUST stay inflight (the next checkpoint will advance the
    // hwm and a later Phase 7 pass will clean up).
    let mut interner_safe = true;
    if !interner_delta_max_ids.is_empty() {
        for &(table_token, max_id) in &interner_delta_max_ids {
            match repo.table_by_token(table_token).await {
                Ok(Some(tbl)) => {
                    let hwm = tbl.interner().persisted_high_water() as u64;
                    if max_id > hwm {
                        log::debug!(
                            "Phase 7 gated: table {table_token} interner delta max_id \
                             {max_id} > persisted hwm {hwm}; deferring WAL truncation \
                             for tx {tx_id}"
                        );
                        interner_safe = false;
                        break;
                    }
                }
                Ok(None) => {
                    // Table gone — delta is stale; safe to truncate (the
                    // table's interner no longer exists to corrupt).
                }
                Err(e) => {
                    log::warn!(
                        "Phase 7 gating: table_by_token {table_token}: {e}; \
                         conservatively deferring WAL truncation for tx {tx_id}"
                    );
                    interner_safe = false;
                    break;
                }
            }
        }
    }

    // Phase 7: WAL cleanup — ONLY on full materialization AND when the
    // interner delta is covered by persisted state (A5 invariant).
    if projections_ok && interner_safe {
        if let Err(e) = wal.commit(tx_id).await {
            log::warn!(
                "commit_tx materialization Phase 7 (WAL cleanup) failed for tx {tx_id} \
                 commit_version {commit_version}: {e}; marker left inflight for recovery"
            );
            projections_ok = false;
        } else {
            // Crash seam (test-only): Phase 7 done — the WAL marker is
            // gone and the tx is fully materialized. A HARD crash here
            // leaves a clean committed state; recovery is a no-op.
            maybe_crash("phase7", repo).await;
        }
    } else if !projections_ok {
        log::warn!(
            "commit_tx tx {tx_id} commit_version {commit_version} COMMITTED but \
             materialization DEFERRED — WAL marker left inflight; recovery will \
             reconcile main/info on the next open"
        );
    } else {
        // projections_ok but !interner_safe: WAL entry stays inflight
        // until a future checkpoint advances the persisted hwm. NOT a
        // materialization deferral (data + index are committed).
        log::debug!(
            "commit_tx tx {tx_id} commit_version {commit_version} COMMITTED, \
             projections complete, WAL entry retained pending interner checkpoint"
        );
    }

    if projections_ok {
        MaterializationState::Complete
    } else {
        MaterializationState::Deferred
    }
}
