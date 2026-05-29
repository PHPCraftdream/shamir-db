use shamir_storage::error::DbError;
use shamir_tx::{IsolationLevel, RepoTxGate, RepoWalManager, TxContext};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::meta::recovery_marker::{save_last_committed, save_next_tx_id_snapshot};
use crate::repo::RepoInstance;

/// Whether the committed transaction's projections (data → main,
/// counter, index → info, HNSW graph) were fully materialized inline
/// on the commit path.
///
/// The WAL entry written in Phase 4 IS the commit; main/info/HNSW are
/// eager-applied projections of it. On the normal path every projection
/// lands inline and the WAL marker is removed (Phase 7) →
/// [`Complete`](MaterializationState::Complete). If a projection
/// sub-phase fails *after* the commit point, the tx is still COMMITTED:
/// the WAL marker is left inflight so recovery re-applies the entry on
/// the next open, and this is reported as
/// [`Deferred`](MaterializationState::Deferred). A `Deferred` outcome is
/// NOT an abort — the version is published and the data WILL appear
/// (idempotently) via recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationState {
    /// All projections applied inline; WAL marker removed (Phase 7 ran).
    Complete,
    /// At least one projection sub-phase failed after the commit point.
    /// WAL marker left inflight; recovery is the materialization
    /// guarantor on the next open.
    Deferred,
}

#[derive(Debug, Clone)]
pub struct TxOutcome {
    pub tx_id: u64,
    pub snapshot_version: u64,
    pub commit_version: u64,
    /// Whether projections materialized inline (`Complete`) or were
    /// deferred to recovery (`Deferred`). Either way the tx is
    /// COMMITTED — see [`MaterializationState`].
    pub materialization: MaterializationState,
}

impl TxOutcome {
    /// Convenience: `true` when all projections materialized inline.
    /// `false` means materialization was deferred to recovery (the tx is
    /// still committed).
    pub fn materialized(&self) -> bool {
        self.materialization == MaterializationState::Complete
    }
}

const DEFAULT_MAX_TX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(300); // 5 min

/// Bounded inline retry budget for a post-commit materialization
/// sub-phase. A few attempts absorb transient storage hiccups; a
/// persistent failure falls through to deferral (recovery re-applies the
/// WAL entry). Transient-vs-persistent is NOT perfectly classified —
/// idempotent re-application makes that unnecessary.
const MATERIALIZE_ATTEMPTS: u32 = 3;

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

/// Test-only injection: when set to a non-zero tx_id, Phase 5c (index
/// apply) returns a synthetic storage error for the matching tx. Used by
/// `commit_phase5_defer_tests` to prove a post-commit-point failure is
/// reported COMMITTED-with-deferred-materialization (not aborted) and is
/// then reconciled by recovery. Persisted across the bounded retry so
/// the failure is treated as persistent → deferral.
#[cfg(test)]
pub(crate) static FAIL_PHASE_5C_TX_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

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

/// cancel-safe: partial — the commit point is a *successful* Phase 4
/// `wal.begin`. Before that, cancellation is a CLEAN ABORT (nothing
/// durable: staging dropped, WAL not written, locks released by RAII).
/// AFTER that, the tx is COMMITTED: the WAL entry is durable and is the
/// single source of truth. Cancellation between Phase 4 and Phase 7
/// leaves an inflight WAL marker that recovery replays idempotently on
/// the next open (Put/Delete/IndexPut/IndexDel are last-write-wins;
/// counter replay is intentionally skipped; HNSW is rebuilt on open) —
/// the caller simply does not observe the `Ok` outcome, but the data is
/// not lost and the tx is not half-aborted. Treat as non-cancel-safe at
/// the API boundary: do not race under `tokio::select!` /
/// `tokio::time::timeout` if you need to observe the outcome.
///
/// Thin metrics-only wrapper around [`commit_tx_inner`]: it records the
/// `on_tx_aborted_storage` metric on a PRE-commit storage abort and is
/// otherwise transparent.
///
/// Metric semantics shift (Vector I.3 / MED-A): post-Phase-4 storage
/// failures no longer surface as `Err(TxError::Storage)` aborts — they
/// become `Ok(TxOutcome { materialization: Deferred, .. })`. So every
/// `Err(TxError::Storage)` reaching this wrapper is by construction a
/// PRE-commit abort, and `on_tx_aborted_storage` correctly counts only
/// those. Deferred materialization is observable via
/// `TxOutcome::materialization` (a dedicated `TxMetrics` counter would
/// live in the out-of-scope `shamir-tx` crate; deferral is logged via
/// `log::warn!` in `materialize` and surfaced on the outcome instead).
///
/// There is NOTHING to do for HNSW staging on a pre-commit abort —
/// staged vectors live inside the `TxContext` (`staged_vectors`) and
/// vanish when the tx is dropped on the `Err` return below. That is the
/// RAII win: no per-tx buffer outlives the `TxContext`, so no broadcast
/// drain is needed (HIGH-6).
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

/// Orchestrates the commit pipeline around the WAL commit point.
///
/// The boundary is explicit: [`pre_commit`] runs Phases 1–4 and may
/// return a real abort `Err` (nothing durable yet). A *successful*
/// `pre_commit` means Phase 4 `wal.begin` landed — that durable WAL
/// entry IS the commit. From there [`materialize`] runs Phases 5–7
/// best-effort and NEVER aborts: a projection failure is logged and
/// deferred to recovery, the version is still published, and the tx is
/// reported COMMITTED (with `MaterializationState::Deferred`).
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

    // === Pre-commit (Phases 1–4): real aborts live here ===
    //
    // Returns `(commit_version, uwl_guards)` on a *successful* Phase 4
    // (the commit point). Any failure before that is a clean abort —
    // nothing durable, locks released by RAII on the `Err` return.
    let PreCommit {
        commit_version,
        uwl_guards,
    } = pre_commit(&mut tx, repo, gate.as_ref(), wal.as_ref()).await?;

    // === Commit point crossed: the WAL entry is durable. ===
    //
    // From here there is NO abort — only materialization. Failures are
    // logged + deferred to recovery; the version is still published.
    let materialization = materialize(
        &mut tx,
        repo,
        gate.as_ref(),
        commit_version,
        wal.as_ref(),
        uwl_guards,
    )
    .await;

    repo.tx_metrics().on_tx_committed();

    Ok(TxOutcome {
        tx_id: tx.tx_id.0,
        snapshot_version: tx.snapshot_version,
        commit_version,
        materialization,
    })
}

/// Outcome of [`pre_commit`]: the assigned MVCC commit version plus the
/// per-table `unique_write_lock` guards that must stay held through
/// Phase 5c (released inside [`materialize`]).
struct PreCommit {
    commit_version: u64,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

/// cancel-safe: NO — but every failure path here is a CLEAN ABORT.
/// Phases 1–4. Nothing is durable until Phase 4 `wal.begin` succeeds:
/// staging is untouched on error, the WAL entry is not written, and the
/// `unique_write_lock` guards / `commit_lock` are released by RAII. The
/// commit point is a *successful* `wal.begin`; the caller treats any
/// `Err` from this function as an abort.
///
/// Returns `(commit_version, uwl_guards)`. The guards are returned (not
/// dropped) so [`materialize`] can hold them across Phase 5c and release
/// them once the unique postings are published.
async fn pre_commit(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
) -> Result<PreCommit, TxError> {
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
    // Uses tx.version_provider if set; otherwise stub `|_, _| Some(0)`
    // (Snapshot-equivalent behaviour).
    if tx.isolation == IsolationLevel::Serializable {
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
    // The problem this closes: `commit_lock` (held since the top of
    // `commit_tx_inner`) only serialises *committers*. Non-tx `insert` /
    // `set` / `delete` take a DIFFERENT mutex — the per-table
    // `unique_write_lock` — and never touch `commit_lock`. So without
    // this step a non-tx unique write could claim or overwrite the same
    // unique posting in the window between this tx's Phase 2.6 re-check
    // and its Phase 5c posting write, producing a duplicate unique value
    // + corrupted index. Acquiring the same per-table lock the non-tx
    // path uses makes the tx's "check unique key free → write posting"
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
    // pass it. `commit_lock` (held since the top of `commit_tx_inner`)
    // serialises committers, and the per-table `unique_write_lock`s
    // (Phase 2.5) exclude non-tx unique writers, so re-checking the
    // claimed keys here is decisive against ALL writers: no committer AND
    // no non-tx writer can interleave between this check and the Phase 5
    // data/index writes that publish the postings.
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

    // Phase 4: write WAL entry — THE COMMIT POINT.
    //
    // A successful `wal.begin` makes the entry durable; from here the tx
    // is committed and `materialize` may not abort. A *failed* `wal.begin`
    // is still a pre-commit failure (nothing durable) → return Err.
    //
    // HIGH-5: stamp the assigned `commit_version` onto the entry BEFORE
    // persisting it. Recovery sorts inflight entries by `commit_version`
    // so multi-tx replay matches the original commit pipeline's order;
    // `txn_id` (the `WalActiveKey` byte order) is not a safe proxy because
    // tx allocation and commit ordering are independent.
    let wal_ops = wal_ops_from_tx(tx).await;
    let entry =
        WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops).with_commit_version(commit_version);
    wal.begin(entry).await?;

    Ok(PreCommit {
        commit_version,
        uwl_guards,
    })
}

/// cancel-safe: NO, and it does NOT need to be — by the time this runs
/// the tx is already COMMITTED (Phase 4 succeeded). It applies the WAL
/// entry's projections (data → main, counter, index → info, HNSW graph),
/// publishes the version, persists recovery markers, and removes the WAL
/// marker. NONE of these may abort the tx: a sub-phase failure is logged
/// and the WAL marker is left inflight so recovery re-applies the entry
/// on the next open (recovery is the materialization guarantor —
/// `Put`/`Delete`/`IndexPut`/`IndexDel` are last-write-wins, counter
/// replay is intentionally skipped, HNSW is rebuilt on open). Returns the
/// observed [`MaterializationState`].
///
/// Phase 6 (`publish_committed`) ALWAYS runs — the version is committed
/// regardless of whether the projections landed inline.
async fn materialize(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    commit_version: u64,
    wal: &RepoWalManager,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> MaterializationState {
    let tx_id = tx.tx_id.0;
    let mut ok = true;

    // Phase 5a: physical data writes per table.
    //
    // Drain each per-table StagingStore once (so a retry re-applies the
    // SAME ops; idempotent at the data layer). Route through MvccStore
    // when available so version_cache stays current for SSI conflict
    // detection; fall back to direct `base.transact` otherwise.
    let data_batches = collect_data_batches(tx);
    for (table_id, base, ops) in data_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_data_batch(repo, table_id, base.clone(), ops.clone(), commit_version)
        })
        .await
        {
            log::warn!(
                "commit_tx materialization Phase 5a (data) failed for tx {tx_id} \
                 commit_version {commit_version} table {table_id}: {e}; deferring to recovery"
            );
            ok = false;
        }
    }

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
        let mut by_token: std::collections::HashMap<u64, Vec<shamir_tx::IndexWriteOp>> =
            std::collections::HashMap::new();
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
                    "commit_tx materialization Phase 5c (index) failed for tx {tx_id} \
                     commit_version {commit_version} table {token}: {e}; deferring to recovery"
                );
                ok = false;
            }
        }
    }

    // HIGH-A: release the per-table unique_write_lock guards now that the
    // unique postings are published (Phase 5c done). The window the Phase
    // 2.6 guard had to dominate — re-check → posting write — is closed, so
    // a non-tx unique writer may resume. Phases 5d/6/6.5/7 do not touch
    // unique postings, so holding the locks past here would only add
    // contention. Released even on a deferred Phase 5c: recovery re-applies
    // the postings, and a stuck guard would only block non-tx writers.
    drop(uwl_guards);

    // Phase 5d: promote HNSW staged vectors into the live graph (HIGH-6).
    //
    // We iterate exactly the tables the tx staged vectors into
    // (`tx.staged_vectors`), keyed by table token. `apply_staged_vectors`
    // is a no-op for every backend except `VectorBackend`. Phase 5a only
    // drained `tx.write_set`, so `tx.staged_vectors` is intact here.
    //
    // Recovery note: HNSW is rebuilt from the materialized data on open, so
    // a deferred Phase 5d is reconciled once recovery replays the data ops
    // and the graph is rebuilt — no per-vector WAL replay is needed.
    let vector_batches = tx
        .staged_vectors
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(t, v)| (*t, v.clone()))
        .collect::<Vec<_>>();
    for (token, vecs) in vector_batches {
        if let Err(e) = retry_materialize(MATERIALIZE_ATTEMPTS, || {
            apply_vector_batch(repo, token, vecs.clone())
        })
        .await
        {
            log::warn!(
                "commit_tx materialization Phase 5d (hnsw) failed for tx {tx_id} \
                 commit_version {commit_version} table {token}: {e}; deferring to recovery"
            );
            ok = false;
        }
    }

    // Phase 6: publish — atomic publish-committed. ALWAYS runs: the
    // version IS committed (the WAL entry is durable) regardless of
    // whether the projections above landed inline.
    gate.publish_committed(commit_version);

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
    // `ok` (Phase 7 will be skipped so the marker stays inflight). Even
    // without these markers, `recover_inflight_v2` re-persists the floor
    // (`gate.last_committed()`) when it consumes the inflight entry, so the
    // version floor is restored on the next open.
    if let Err(e) = persist_markers(repo, gate, commit_version).await {
        log::warn!(
            "commit_tx materialization Phase 6.5 (recovery markers) failed for tx {tx_id} \
             commit_version {commit_version}: {e}; deferring to recovery"
        );
        ok = false;
    }

    // Phase 7: WAL cleanup — ONLY on full materialization. If any
    // projection deferred, leave the marker inflight so recovery
    // re-applies the entry on the next open.
    if ok {
        if let Err(e) = wal.commit(tx_id).await {
            log::warn!(
                "commit_tx materialization Phase 7 (WAL cleanup) failed for tx {tx_id} \
                 commit_version {commit_version}: {e}; marker left inflight for recovery"
            );
            ok = false;
        }
    } else {
        log::warn!(
            "commit_tx tx {tx_id} commit_version {commit_version} COMMITTED but \
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

/// Drain each table's `StagingStore` into an owned `(token, base, ops)`
/// batch. Done once up front so a bounded retry re-applies the SAME ops
/// (idempotent at the data layer). Empty batches are dropped.
#[allow(clippy::type_complexity)]
fn collect_data_batches(
    tx: &mut TxContext,
) -> Vec<(
    u64,
    std::sync::Arc<dyn shamir_storage::types::Store>,
    Vec<shamir_storage::types::KvOp>,
)> {
    let mut out = Vec::new();
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
async fn apply_data_batch(
    repo: &RepoInstance,
    table_id: u64,
    base: std::sync::Arc<dyn shamir_storage::types::Store>,
    ops: Vec<shamir_storage::types::KvOp>,
    commit_version: u64,
) -> Result<(), DbError> {
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
async fn apply_index_batch(
    repo: &RepoInstance,
    token: u64,
    ops: Vec<shamir_tx::IndexWriteOp>,
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

/// Promote one table's staged HNSW vectors into the live graph (Phase 5d).
async fn apply_vector_batch(
    repo: &RepoInstance,
    token: u64,
    vecs: Vec<(shamir_types::types::record_id::RecordId, Vec<f32>)>,
) -> Result<(), DbError> {
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
async fn persist_markers(
    repo: &RepoInstance,
    gate: &RepoTxGate,
    commit_version: u64,
) -> Result<(), DbError> {
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
async fn retry_materialize<F, Fut>(attempts: u32, mut op: F) -> Result<(), DbError>
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
