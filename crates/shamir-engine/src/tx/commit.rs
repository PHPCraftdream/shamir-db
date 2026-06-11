use shamir_storage::error::DbError;
use shamir_tx::TxContext;
use shamir_types::types::common::THasher;
use shamir_wal::WalOpV2;

use crate::repo::RepoInstance;
use crate::tx::commit_phases::{
    apply_counter_phase, apply_data_phase, materialize_async_tail, promote_vectors,
};
use crate::tx::materialize::materialize;
use crate::tx::pre_commit::{pre_commit, PreCommit};
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
pub(super) async fn maybe_crash(phase: &str, repo: &RepoInstance) {
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
pub(super) async fn maybe_crash(_phase: &str, _repo: &RepoInstance) {}

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
    let mut ops =
        Vec::with_capacity(tx.counter_deltas.len() + tx.index_write_set.len() + tx.write_set.len());

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
    let mut by_table: std::collections::HashMap<u64, Vec<bytes::Bytes>, THasher> =
        std::collections::HashMap::with_capacity_and_hasher(
            tx.locked_keys.len(),
            THasher::default(),
        );
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
