use shamir_storage::error::DbError;
use shamir_tx::{IsolationLevel, RepoTxGate, RepoWalManager, TxContext};
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::RepoInstance;
use crate::tx::commit_phases::{
    apply_counter_phase, apply_data_phase, collect_data_batches, materialize_async_tail,
    persist_markers, promote_vectors, retry_materialize, MATERIALIZE_ATTEMPTS,
};
use crate::tx::tx_outcome::{BackgroundCommitHandle, MaterializationState, TxOutcome};

const DEFAULT_MAX_TX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(300); // 5 min

/// Test-only crash-injection seam for the real crash-recovery harness
/// (`crates/shamir-engine/tests/crash_recovery.rs`, Vector II.1).
///
/// At each labelled point in the commit pipeline the harness can force a
/// HARD process death to prove atomicity around the Phase-4 commit point:
/// a crash *before* Phase 4 leaves nothing durable (clean abort); a crash
/// *at/after* Phase 4 but before Phase 7 leaves an inflight WAL marker
/// that recovery replays idempotently (all-or-nothing materialization).
///
/// Why `std::process::abort()` and not `panic!`:
///   `panic!` UNWINDS the stack — it runs every `Drop` impl on the way
///   out, which would flush `MemBuffer`/`Cached` write-backs, drop
///   storage handles cleanly, and let `RAII` guards release. That is a
///   *graceful* shutdown, the exact opposite of a crash. `process::abort`
///   raises `SIGABRT` immediately: no unwind, no `Drop`, no flush — the
///   closest in-process analog to `kill -9`. The on-disk state left
///   behind is therefore a genuine torn-mid-commit image, which is what
///   the recovery contract must survive.
///
/// Durability of the on-disk image (the `repo` argument):
///   The crash is real, but for the recovery contract to be *meaningful*
///   the WAL entry written in Phase 4 (and the Phase-5 row writes for the
///   later seams) must already be on disk when the process dies. Several
///   backends — redb's `Store::set` among them — use a deferred-fsync
///   write mode where rows become crash-durable only on an explicit
///   `flush()` (or, in production, on the MemBuffer flush tick). To model
///   a backend whose WAL/data writes are *synchronously* durable (the
///   durability mode the recovery guarantee assumes), every seam AT or
///   AFTER the commit point flushes the repo's shared store before
///   aborting. Because all of a repo's stores (WAL `__tx__`, per-table
///   `__data__`/`__info__`) share one backend handle (e.g. one redb
///   `Database`), a single `flush()` makes the entire pending image
///   durable. This flush is NOT a graceful shutdown: it is `fsync`, not
///   `Drop` — no buffers are drained through destructors, no guards
///   release, no in-memory tx state is reconciled. The `pre_commit` seam
///   deliberately does NOT flush: it fires BEFORE the WAL entry exists,
///   so the disk must show nothing tx-related (clean abort).
///
/// Gating (zero cost in production):
///   The whole hook is `#[cfg(debug_assertions)]`. In a normal
///   `--release` build `debug_assertions` is off, so `maybe_crash`
///   compiles to the empty `#[inline(always)]` twin below — dead code,
///   no env read, no branch, the `repo`/`phase` args unused. Tests build
///   in debug, where the hook is live but still a NO-OP unless
///   `SHAMIR_TEST_CRASH_AFTER` is set to a matching phase label. With no
///   env var present every existing test pays at most one `env::var` miss
///   per seam and never crashes or flushes.
///
/// Phase labels: `pre_commit`, `phase4`, `phase5a`, `phase5c`, `phase6`,
/// `phase6_5`, `phase7`.
#[cfg(debug_assertions)]
async fn maybe_crash(phase: &str, repo: &RepoInstance) {
    let Ok(target) = std::env::var("SHAMIR_TEST_CRASH_AFTER") else {
        return;
    };
    if target != phase {
        return;
    }
    // AT/AFTER the commit point: make the pending on-disk image durable
    // (fsync, NOT a graceful drain) so the killed process leaves a real
    // torn-mid-commit state the reopened repo must recover. `pre_commit`
    // skips this — nothing tx-related should be durable yet.
    if phase != "pre_commit" {
        if let Ok(store) = repo.tx_info_store().await {
            let _ = store.flush().await;
        }
    }
    // HARD crash: no unwind, no Drop, no in-memory reconciliation.
    std::process::abort();
}

/// Release twin: the crash seam does not exist outside debug builds.
#[cfg(not(debug_assertions))]
#[inline(always)]
async fn maybe_crash(_phase: &str, _repo: &RepoInstance) {}

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
    /// Phase C. A concurrent committer wrote a key matching one of
    /// this tx's recorded predicate dependencies (a phantom). Surfaced
    /// to the wire path as `"tx_conflict"` so existing client retry
    /// covers it.
    #[error("phantom conflict on predicate {dep}")]
    PhantomConflict { dep: String },
    /// Level-3 wound-wait abort: an older (higher-priority) tx wounded
    /// this one during `lock_key`, or this tx observed its own wound flag
    /// at commit entry. Surfaced to the wire path as `"tx_conflict"` so
    /// existing client retry covers it.
    #[error("tx {} wounded (wound-wait abort)", tx_version)]
    Wounded { tx_version: u64 },
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
/// those. Deferred materialization is observable three ways:
/// `TxOutcome::materialization`, the `log::warn!` emitted in `materialize`,
/// and the `TxMetrics::txs_materialization_deferred` counter
/// (`on_tx_materialization_deferred`, fired in `commit_tx_inner` when
/// `materialize` returns `Deferred`).
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
///
/// C6 empty-tx fast-path: when `pre_commit` returns `Ok(None)` (the tx
/// staged nothing durable, after SSI validation passed) this function
/// short-circuits — no version assigned, no WAL write, no publish — and
/// returns `Ok(TxOutcome { commit_version: snapshot_version, materialization:
/// Complete, .. })`.
async fn commit_tx_inner(mut tx: TxContext, repo: &RepoInstance) -> Result<TxOutcome, TxError> {
    // Level-3 wound-wait: a tx wounded by an older (higher-priority) tx must
    // abort even if it finished all its statements. The check runs at commit
    // entry so a wounded tx can never commit. No-op for Snapshot / Serializable
    // (their wounded flag stays false — they never take locks).
    if let Err(tx_version) = tx.ensure_not_wounded() {
        release_pessimistic_locks(&tx, repo).await;
        return Err(TxError::Wounded { tx_version });
    }

    if tx.is_expired(DEFAULT_MAX_TX_LIFETIME) {
        release_pessimistic_locks(&tx, repo).await;
        repo.tx_metrics().on_tx_aborted_expired();
        return Err(TxError::Expired {
            elapsed: tx.elapsed(),
            max: DEFAULT_MAX_TX_LIFETIME,
        });
    }

    let gate = repo.tx_gate().await?;
    let wal = repo.repo_wal().await?;

    let commit_guard = gate.commit_lock().await;

    // === Pre-commit (Phases 1–4): real aborts live here ===
    //
    // Returns `Some((commit_version, uwl_guards))` on a *successful* Phase 4
    // (the commit point). Any failure before that is a clean abort —
    // nothing durable, locks released by RAII on the `Err` return. Returns
    // `None` for the C6 empty-tx fast-path (SSI already validated inside
    // `pre_commit`; nothing durable to do).
    let PreCommit {
        commit_version,
        uwl_guards,
    } = match pre_commit(&mut tx, repo, gate.as_ref(), wal.as_ref()).await {
        Ok(Some(pc)) => pc,
        Ok(None) => {
            // C6 empty-tx fast-path. SSI read-set validation (Phase 2) has
            // already run inside `pre_commit`, so a read-only Serializable
            // tx with a conflict has already returned `Err` above. Here the
            // tx staged nothing durable: skip Phase 3 (assign_next_version),
            // Phase 4 (`wal.begin`), and all of `materialize` (publish +
            // markers + wal.commit). No version is consumed, no WAL write
            // occurs. Report COMMITTED with `commit_version` pinned to the
            // tx's snapshot version and materialization `Complete` (there
            // were no projections to materialize). Counts as a committed tx
            // for metrics, consistent with the full path.
            repo.tx_metrics().on_tx_committed();
            // Level-3 locks are released on commit too (a read-only
            // Pessimistic tx still holds Shared locks on what it read).
            release_pessimistic_locks(&tx, repo).await;
            drop(commit_guard);
            return Ok(TxOutcome {
                tx_id: tx.tx_id.0,
                snapshot_version: tx.snapshot_version,
                commit_version: tx.snapshot_version,
                materialization: MaterializationState::Complete,
                background: None,
            });
        }
        Err(e) => {
            // Pre-commit abort: nothing durable. Release any Level-3 locks
            // the tx held so blocked txs can proceed (RAII rollback for the
            // pessimistic dimension).
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    // === Commit point crossed: the WAL entry is durable. ===
    //
    // From here there is NO abort — only materialization. Failures are
    // logged + deferred to recovery; the version is still published.
    //
    // Level-3 pessimistic locks are released NOW: the commit is decided
    // (Phase 4 WAL is durable), so the locks have served their purpose
    // (2PL: locks held until commit, not until post-commit materialization).
    // Releasing here works for both sync and async modes (tx is still in
    // scope) and lets blocked txs proceed while materialization runs. No-op
    // for Snapshot / Serializable (locked_keys is empty).
    release_pessimistic_locks(&tx, repo).await;

    // Branch on the opt-in visibility mode (default `Synchronous` — byte-
    // identical to the historical behaviour). `AsyncIndex` performs only
    // the sync-prefix phases inline (5a data, 5b counter, 6 publish, write-
    // log) and spawns the tail (5c, 6.5, 7, 5d) on a background task — the
    // client returns once the version is published.
    let visibility = tx.visibility;
    let snapshot_version = tx.snapshot_version;
    let tx_id_u64 = tx.tx_id.0;

    // Changefeed (Phase 3b): project the committed footprint ONCE here,
    // while `tx.write_set` is still intact (Phase 5a / `collect_data_batches`
    // drains it). The projected event is emitted AFTER the version is
    // published (sync: post-`materialize`; async: post-`publish_committed`),
    // so neither the live broadcast nor the durable journal can hand out a
    // version the gate has not yet published. `None` for an empty-data-write
    // footprint. Emission itself is non-blocking and never fails the commit.
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);

    let outcome = match visibility {
        shamir_tx::CommitVisibility::Synchronous => {
            // === Sync mode (default, unchanged behaviour) ===
            //
            // `materialize` runs Phases 5a (data) → 5b (counter) → 5c (index)
            // → 6 (publish) → 6.5 (markers) → 7 (wal.commit), all UNDER
            // `commit_lock`. It NO LONGER promotes HNSW vectors: that
            // derived, rebuildable read-accelerator is moved out of the
            // critical section below (III.5).
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
            if materialization == MaterializationState::Deferred {
                repo.tx_metrics().on_tx_materialization_deferred();
            }

            // === III.5: release `commit_lock` BEFORE the HNSW promote. ===
            //
            // Everything that determines visibility (data → main, counter,
            // index → info), publishes the version, persists recovery
            // markers, and removes the WAL marker has already run under the
            // lock. Drop the guard here so other committers proceed while
            // this tx promotes its vectors.
            drop(commit_guard);

            // Changefeed emit (Phase 3b): publish already happened inside
            // `materialize` (Phase 6). Fan the pre-projected event out to the
            // live broadcast + durable journal — both non-blocking, neither
            // can fail the commit. Done after the lock drop so the feed work
            // never sits on the commit critical section.
            repo.emit_changefeed_event(changefeed_event).await;

            // Phase 5d (moved): promote staged HNSW vectors into the live
            // graph, OUTSIDE `commit_lock` and AFTER Phase 7. A failure here
            // does NOT mark the tx Deferred — see `promote_vectors`.
            promote_vectors(&tx, repo, commit_version).await;

            TxOutcome {
                tx_id: tx_id_u64,
                snapshot_version,
                commit_version,
                materialization,
                background: None,
            }
        }
        shamir_tx::CommitVisibility::AsyncIndex => {
            // === Async-index mode (opt-in) ===
            //
            // Run ONLY the phases that determine the client-visible commit
            // outcome under `commit_lock`:
            //   5a (data → MvccStore — read-your-own-writes holds),
            //   5b (counter — in-memory bump),
            //   6  (publish_committed — version visible to readers),
            //   record_commit_writes (SSI phantom log).
            // Then RELEASE `commit_lock` + ack the client.
            //
            // The tail — 5c (index postings to info_store), 6.5 (durable
            // recovery markers), 7 (wal.commit marker removal), 5d (HNSW
            // promote) — runs on a `tokio::task`. The tail holds the
            // per-table `unique_write_lock` guards (Phase 2.5) across 5c
            // exactly as in sync mode; concurrent non-tx unique writers
            // to the same tables block on those guards until 5c finishes.
            //
            // CONTRACT (re-stated in `CommitVisibility::AsyncIndex` doc):
            //   * Durability: WAL fsync already happened (Phase 4) — NOT
            //     relaxed.
            //   * Data read-your-writes: 5a + 6 already ran — preserved.
            //   * Secondary-index visibility: may briefly lag (5c is on
            //     the background task); a data scan immediately sees the
            //     row.
            //   * Crash safety: if the process dies before the tail
            //     finishes, the inflight WAL marker survives → recovery
            //     replays exactly the same WAL entry that sync-mode
            //     deferral relies on (`recover_v2_inflight`).
            apply_data_phase(&mut tx, repo, commit_version).await;
            apply_counter_phase(&tx, repo).await;
            // publish_committed_max (monotonic fetch_max): now that non-tx
            // writes also advance `last_committed` off-lock via the same call,
            // the tx path must use the CAS form too so a concurrent non-tx
            // publish can never be overwritten by a plain store. For tx-only
            // workloads commit_versions are strictly monotonic under
            // `commit_lock`, so max == plain → behaviour unchanged.
            gate.publish_committed_max(commit_version);
            gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&tx, commit_version));

            // Crash seam (test-only): version published in-memory but the
            // background tail has NOT begun. The inflight WAL marker is
            // still present so a hard crash here is recovered identically
            // to a `Deferred` sync commit.
            maybe_crash("phase6_async", repo).await;

            // Release `commit_lock` BEFORE spawning the tail — the whole
            // point of async mode is that subsequent committers proceed
            // without waiting on this tx's 5c. The tail will keep the
            // per-table `uwl_guards` until 5c is done.
            drop(commit_guard);

            // Changefeed emit (Phase 3b): the version is already published
            // (above). Fan the pre-projected event out — non-blocking, never
            // fails the commit, never waits on the background tail. Done
            // before spawning the tail so the live event is available the
            // instant the client is acked.
            repo.emit_changefeed_event(changefeed_event).await;

            // Move ownership of the tx and the per-task guards onto the
            // background task; `RepoInstance` is `Clone` (Arc-backed), so
            // a clone is cheap.
            let repo_clone = repo.clone();
            let metrics = repo.tx_metrics().clone();
            let join = tokio::spawn(async move {
                let state =
                    materialize_async_tail(&mut tx, &repo_clone, commit_version, uwl_guards).await;
                if state == MaterializationState::Deferred {
                    metrics.on_tx_materialization_deferred();
                }
                // Phase 5d (post-tail): HNSW promote, derived, never
                // defers the tx. Done inside the background task so the
                // client never waits for the per-vector spawn_blocking.
                promote_vectors(&tx, &repo_clone, commit_version).await;
                state
            });

            // Ack-time accounting: count the committed tx, ack the client.
            repo.tx_metrics().on_tx_committed();

            TxOutcome {
                tx_id: tx_id_u64,
                snapshot_version,
                commit_version,
                // SYNC-PREFIX phases all landed before ack (5a + 5b + 6 are
                // unconditional and don't surface failures up here — 5a's
                // bounded-retry deferral is folded into the tail's state
                // because failure to materialize data should ALSO leave the
                // marker inflight). With async mode the prefix doesn't have
                // sub-phases that can flip the published outcome: 5a
                // either succeeds or already logged + flipped `ok` inside
                // the tail's state-tracking shim. Report `Complete` at ack
                // and let `BackgroundCommitHandle::join` carry the truly
                // final state.
                materialization: MaterializationState::Complete,
                background: Some(BackgroundCommitHandle { join }),
            }
        }
    };

    Ok(outcome)
}

/// Release every Level-3 pessimistic lock the tx holds, grouped by table.
///
/// Called on BOTH the commit-success path and every abort path of
/// [`commit_tx_inner`]. `locked_keys` is empty for Snapshot / Serializable
/// txs (they never acquire locks), so this is a no-op for them — zero
/// overhead on the non-Pessimistic commit paths. Each `(table_token, key)`
/// is routed to the corresponding table's `MvccStore::release_locks`.
pub(crate) async fn release_pessimistic_locks(tx: &TxContext, repo: &RepoInstance) {
    if tx.locked_keys.is_empty() {
        return;
    }
    // Group keys by table_token so each MvccStore is hit once. `locked_keys`
    // is an `scc::HashMap<(u64, Bytes), ()>`; scan into a std map (the
    // synchronous visitor cannot await inside).
    let mut by_table: std::collections::HashMap<u64, Vec<bytes::Bytes>> =
        std::collections::HashMap::new();
    tx.locked_keys.scan(|(token, key), _| {
        by_table.entry(*token).or_default().push(key.clone());
    });
    let mvcc_map = repo.per_table_mvcc();
    for (token, keys) in by_table {
        if let Some(e) = mvcc_map.get(&token) {
            e.get().release_locks(tx.tx_id.0, &keys).await;
        }
    }
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
/// Returns `Some((commit_version, uwl_guards))` on a successful Phase 4.
/// The guards are returned (not dropped) so [`materialize`] can hold them
/// across Phase 5c and release them once the unique postings are published.
///
/// Returns `Ok(None)` for the C6 empty-tx fast-path: a tx that staged
/// nothing durable (no writes / index ops / vectors / counter deltas /
/// interner overlay). The SSI read-set validation (Phase 2) has ALREADY
/// run by that point, so a read-only Serializable tx that read stale data
/// still aborts with `Err` — the fast-path only skips the work that has no
/// effect for an empty op set: version assignment (Phase 3), the durable
/// `wal.begin` (Phase 4), and everything downstream. `commit_tx_inner`
/// turns `None` into an `Ok(TxOutcome)` whose `commit_version` is the tx's
/// snapshot version and whose materialization is `Complete`.
async fn pre_commit(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
) -> Result<Option<PreCommit>, TxError> {
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

    // Phase 2-bis (SSI only, Phase C): predicate read-set validation.
    //
    // Phase 2 above caught rw-antidependencies on keys the tx ACTUALLY READ
    // (the point read-set). It does NOT catch phantoms — a concurrent
    // committer inserting a NEW key that matches one of this tx's
    // predicate/range reads. Phase C records those dependencies in
    // `tx.predicate_set` (populated only on Serializable; empty otherwise)
    // and validates them here against `RepoTxGate`'s commit-write-log.
    //
    // Zero-overhead: gated on `Serializable` AND on a non-empty
    // `predicate_set`, so Snapshot, non-tx, and Serializable-with-only-point-
    // reads skip the loop entirely.
    if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
        let mut conflict_dep: Option<String> = None;
        tx.predicate_set.with_iter(|dep| {
            if conflict_dep.is_none() && gate.predicate_conflicts(dep, tx.snapshot_version) {
                conflict_dep = Some(format!("{:?}", dep));
            }
        });
        if let Some(dep) = conflict_dep {
            repo.tx_metrics().on_tx_aborted_phantom();
            return Err(TxError::PhantomConflict { dep });
        }
    }

    // === C6: empty-tx fast-path ===
    //
    // A tx that staged nothing durable (read-only Serializable txs, or any
    // tx whose write_set / index_write_set / staged_vectors / counter_deltas
    // / interner_overlay are all empty) has no commit point to cross: there
    // is nothing to assign a version for, nothing to write to the WAL, and
    // nothing to publish. Short-circuit BEFORE Phase 3 (assign_next_version)
    // and Phase 4 (the durable `wal.begin`) so an empty commit is a pure
    // in-memory no-op — no monotonic version is burned and no fsync is paid.
    //
    // ORDERING IS LOAD-BEARING: this sits AFTER the Phase 2 SSI block above.
    // A read-only Serializable tx still records reads, and its read_set must
    // still be validated against current committed versions — a read-only
    // tx that observed stale data MUST abort (returned as `Err` above), not
    // silently fast-path to success. `is_empty()` deliberately does NOT gate
    // on `read_set` (read-only ⇒ fast-path-eligible) nor on `unique_guards`
    // (a guard only ever accompanies a staged write, so an empty `write_set`
    // already implies no guards). Phase 2.5/2.6 (unique re-validation) are
    // skipped only because they are no-ops with no guards.
    if tx.is_empty() {
        return Ok(None);
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

    // Crash seam (test-only): a HARD crash here is BEFORE the commit
    // point — staging is dropped, no WAL entry exists, locks release by
    // RAII. Recovery must find nothing → clean abort.
    maybe_crash("pre_commit", repo).await;

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

    // Crash seam (test-only): a HARD crash here is AT the commit point —
    // the WAL entry is durable but no projection (5a..6.5) ran and Phase
    // 7 cleanup never happens. Recovery must find the inflight entry and
    // materialize the whole tx (data + index). All-or-nothing.
    maybe_crash("phase4", repo).await;

    Ok(Some(PreCommit {
        commit_version,
        uwl_guards,
    }))
}

/// cancel-safe: NO, and it does NOT need to be — by the time this runs
/// the tx is already COMMITTED (Phase 4 succeeded). It applies the WAL
/// entry's visibility-bearing projections (data → main, counter, index →
/// info), publishes the version, persists recovery markers, and removes
/// the WAL marker. It does NOT promote HNSW vectors — that derived,
/// rebuild-on-open read-accelerator moved OUT of the commit critical
/// section (III.5): `commit_tx_inner` drops `commit_lock` after Phase 7
/// and then calls `promote_vectors`. NONE of the projections here may
/// abort the tx: a sub-phase failure is logged and the WAL marker is left
/// inflight so recovery re-applies the entry on the next open (recovery is
/// the materialization guarantor — `Put`/`Delete`/`IndexPut`/`IndexDel`
/// are last-write-wins, counter replay is intentionally skipped, HNSW is
/// rebuilt on open). Returns the observed [`MaterializationState`].
///
/// Phase 6 (`publish_committed`) ALWAYS runs — the version is committed
/// regardless of whether the projections landed inline.
///
/// MULTI-TABLE DEFERRAL IS PARTIAL (audit MED, by-design): the Phase 5a
/// (data) and Phase 5c (index) loops below iterate per table, each with its
/// own bounded retry. A failure on ONE table flips `ok` but does NOT halt
/// the other tables — so a tx touching tables A and B can materialize A
/// inline and leave B for recovery, yet `publish_committed` still publishes
/// the single shared `commit_version`. The result is a cross-table /
/// data-vs-index inconsistency that is *restart-bounded eventually
/// consistent*: it is reconciled only when the next `recover_v2_inflight`
/// replays the one inflight WAL entry (which carries every table's ops).
/// There is no online reconciler. This is honest, not reassuring — see
/// [`MaterializationState::Deferred`] for the reader-visible contract.
async fn materialize(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    commit_version: u64,
    wal: &RepoWalManager,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> MaterializationState {
    use crate::tx::commit_phases::{apply_data_batch, apply_index_batch};

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
    gate.publish_committed(commit_version);

    // Phase 6-bis (Phase C): record this tx's write footprint into the
    // commit-write log so future Serializable txs can detect phantoms
    // against our writes. No-op off Serializable (footprint is empty).
    gate.record_commit_writes(shamir_tx::build_footprint_from_tx(tx, commit_version));

    // Crash seam (test-only): version published in-memory but lost with
    // the process; markers (6.5) not yet persisted and Phase 7 not run.
    // The inflight WAL marker survives → recovery materializes the tx.
    maybe_crash("phase6", repo).await;

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

    // Crash seam (test-only): everything (data, index, markers) is on
    // disk but Phase 7 has NOT removed the WAL marker. The inflight
    // marker survives → recovery re-applies the (already-present) entry
    // idempotently and cleans the marker.
    maybe_crash("phase6_5", repo).await;

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
        } else {
            // Crash seam (test-only): Phase 7 done — the WAL marker is
            // gone and the tx is fully materialized. A HARD crash here
            // leaves a clean committed state; recovery is a no-op.
            maybe_crash("phase7", repo).await;
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
