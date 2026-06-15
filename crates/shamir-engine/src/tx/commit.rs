use shamir_storage::error::DbError;
use shamir_tunables::instance_defaults::MAX_UNDRAINED_VERSIONS;
use shamir_tx::TxContext;
use shamir_types::types::common::THasher;
use shamir_wal::WalOpV2;

use crate::repo::RepoInstance;
use crate::tx::commit_phases::{
    apply_counter_phase, apply_data_phase, materialize_async_tail, promote_vectors,
};
use crate::tx::pre_commit::{pre_commit_locked, pre_commit_prelock, PreCommit, PreLockResult};
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
/// `phase6_5`, `phase7`. D4 drain seam (fired from the drainer's per-entry
/// loop, not the ack-path): `drain_replay` — AFTER `replay_v2_entry` made the
/// entry durable in `history` but BEFORE `mark_durable` advanced the durable
/// watermark, so the entry is still inflight and recovery re-replays it
/// idempotently. F6c truncation seams (fired from the drainer, not the
/// ack-path): `pre_truncate` (before history-flush + segment unlink) and
/// `post_truncate` (after a successful `truncate_below`). A third truncation
/// seam, `wal_mid_delete`, lives inside `shamir-wal`'s
/// `SegmentSet::truncate_below` (it cannot reach this engine hook) and aborts
/// between two segment unlinks.
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
        Ok(outcome) => {
            // D2 P1e — soft backpressure. The ack-path published only the
            // in-memory overlay; the value is durable in `history` only after
            // the background drainer replays its WAL entry. Under sustained
            // write pressure faster than the disk drains, the overlay + inflight
            // WAL tail grow unbounded. Park the committer here, AFTER the commit
            // is published (the tx is COMMITTED and observable on its return),
            // until the undrained gap falls back under the low-watermark. This
            // is the single choke point: every live commit path funnels through
            // this wrapper, and it is on the SUCCESS path only — aborts never
            // grew the overlay, so they never brake.
            apply_backpressure(repo, MAX_UNDRAINED_VERSIONS).await;
            Ok(outcome)
        }
        Err(e) => {
            if let TxError::Storage(_) = &e {
                repo.tx_metrics().on_tx_aborted_storage();
            }
            Err(e)
        }
    }
}

/// D2 P1e — soft async backpressure on the undrained version gap.
///
/// The gap is `gate.last_committed() - gate.durable_watermark()`: the number
/// of versions that are visible (published to readers via the overlay) but not
/// yet durable in `history` (the background [`Drainer`](crate::tx::drainer::Drainer)
/// has not replayed their WAL entries). Each undrained version pins an overlay
/// entry + an inflight WAL marker, so an unbounded gap is an unbounded RAM /
/// WAL-tail leak.
///
/// ## Contract
/// * **Fast path (zero overhead):** if `gap <= high` it returns immediately —
///   one pair of atomic loads, no allocation, no wake, no await. This is the
///   common case under any non-pathological load, so a steady committer pays
///   nothing.
/// * **Brake (only under pressure):** once `gap > high`, the committer wakes
///   the drainer and parks on the gate's durable-progress [`Notify`], re-checking
///   the gap each time the drain makes progress. It resumes the instant the gap
///   drops below the LOW watermark `high / 2` (hysteresis: braking to exactly
///   `high` would re-trigger on the very next commit and thrash; draining down
///   to `high/2` amortizes the brake over many commits).
///
/// ## Lost-wakeup safety
/// Each loop iteration registers `gate.durable_notified()` (a `Notified` future)
/// BEFORE re-reading the gap, then awaits it. A concurrent `mark_durable`
/// between the gap read and the await cannot be missed: `notify_waiters` wakes
/// every already-registered waiter, and the future was registered first. We
/// also re-wake the drainer each iteration so a drainer that went idle (its own
/// `notify_one` permit consumed) is nudged back to work.
///
/// ## Deadlock safety (the load-bearing guard)
/// The drainer can get STUCK — a durable-write error (disk full, backend fault)
/// leaves `durable_watermark` frozen, so the gap never closes and the parking
/// loop would wait forever. To make a stuck drain a DEGRADATION (more RAM) and
/// never a HANG, the loop is bounded by a wall-clock budget
/// ([`BACKPRESSURE_MAX_WAIT`]). When the budget is exhausted it logs a warning
/// and RETURNS (proceeds without backpressure). Correctness is untouched — the
/// data is already committed and observable; only the overlay-bounding promise
/// is relaxed under a faulted drain, which is the right trade (never wedge a
/// committer on a broken disk). Each individual park is itself capped by a short
/// `sleep` in the `select!` so a single lost signal cannot stall a whole budget.
///
/// Takes `high` as a parameter so tests can drive the state machine with a tiny
/// artificial gap; production callers pass [`MAX_UNDRAINED_VERSIONS`].
pub(crate) async fn apply_backpressure(repo: &RepoInstance, high: u64) {
    let Ok(gate) = repo.tx_gate().await else {
        // No gate (repo teardown / construction race) — nothing to brake on.
        return;
    };

    // Fast path: cheap atomic loads, the overwhelmingly common case.
    let gap = gate
        .last_committed()
        .saturating_sub(gate.durable_watermark());
    if gap <= high {
        return;
    }

    // Hysteresis low-watermark: resume only once the drain has caught up to
    // half the threshold, so the brake amortizes over many commits instead of
    // re-firing on every subsequent commit (thrash).
    let low = high / 2;

    log::warn!(
        "tx backpressure ENGAGED: undrained gap {gap} > {high} (low-watermark {low}); \
         parking committer on durable progress"
    );

    let start = std::time::Instant::now();
    loop {
        // Nudge the drainer every iteration: it may have gone idle (its
        // `notify_one` permit already consumed by a prior pass).
        repo.drainer().wake();

        // Register the durable-progress future BEFORE re-reading the gap so a
        // concurrent `mark_durable` cannot slip a wakeup between the check and
        // the park (no lost wakeup).
        let notified = gate.durable_notified();
        tokio::pin!(notified);

        let gap = gate
            .last_committed()
            .saturating_sub(gate.durable_watermark());
        if gap <= low {
            log::debug!(
                "tx backpressure RELEASED: gap {gap} <= low-watermark {low} after {:?}",
                start.elapsed()
            );
            return;
        }

        // Deadlock guard: if the drain has not closed the gap within the
        // wall-clock budget it is stuck (durable write faulted). Proceed without
        // backpressure rather than hang the committer forever — RAM grows, data
        // is unaffected (already committed + observable).
        if start.elapsed() >= BACKPRESSURE_MAX_WAIT {
            log::warn!(
                "tx backpressure ABANDONED after {:?}: undrained gap still {gap} > low {low} — \
                 drain appears stuck; proceeding (overlay bound relaxed, data unaffected)",
                start.elapsed()
            );
            return;
        }

        // Park on durable progress, but cap each park with a short sleep so a
        // single missed signal (or a drainer that needs re-nudging) cannot
        // stall the whole budget — we loop, re-wake, re-check.
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep(BACKPRESSURE_PARK_SLICE) => {}
        }
    }
}

/// Wall-clock ceiling on a single [`apply_backpressure`] call. Past this the
/// brake is abandoned (drain presumed stuck) and the committer proceeds — a
/// RAM-vs-hang trade that always favours liveness. Generous enough that a
/// healthy-but-slow disk drains within it under normal bursts.
const BACKPRESSURE_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Per-park cap inside the backpressure loop. Bounds how long one `select!`
/// waits on the durable-progress signal before re-checking the gap and
/// re-nudging the drainer, so a single lost wakeup degrades to a short poll
/// rather than stalling the whole [`BACKPRESSURE_MAX_WAIT`] budget.
const BACKPRESSURE_PARK_SLICE: std::time::Duration = std::time::Duration::from_millis(50);

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

    // === PRE-LOCK SECTION (concurrent across committers) ===
    //
    // Phase 1 (interner overlay merge + remap) and Phase 2.5 + 2.6
    // (uwl_guards acquisition + unique re-validation) run OUTSIDE
    // `commit_lock`. Phase 1 is CAS-safe on DashMap (concurrent merges
    // converge). Phase 2.5/2.6 serialize on per-table uwl_guards in
    // sorted token order — no global serialization needed. Two committers
    // touching disjoint unique tables proceed fully in parallel here.
    let PreLockResult { uwl_guards } = match pre_commit_prelock(&mut tx, repo).await {
        Ok(r) => r,
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    // AsyncIndex txs bypass group-commit: their post-commit tail is spawned
    // as a background task, which is incompatible with the leader processing
    // another tx's materialization. They always wait for the lock.
    if tx.visibility == shamir_tx::CommitVisibility::AsyncIndex {
        return commit_tx_inner_legacy_async(tx, uwl_guards, repo, gate.as_ref(), wal.as_ref())
            .await;
    }

    // === P2c: LOCKFREE COMMIT PATH ===
    //
    // Disjoint-table commits run fully in parallel — no global
    // `commit_mutex`. Correctness relies on per-table uwl_guards:
    // same-table committers serialize at uwl acquisition (pre-lock);
    // disjoint-table committers never conflict in SSI predicates.
    //
    // Sequence (all lock-free or per-table-locked):
    //   1. validate (atomic version + lock-free TreeIndex scan)
    //   2. WAL begin (unique key per txn_id — concurrent-safe)
    //   3. record_commit_writes (lock-free TreeIndex insert)
    //   4. materialize (gated by uwl_guards, not commit_mutex)
    //
    // Group-commit WAL batching is sacrificed for parallelism; the
    // WAL backend's internal batching (if any) still amortizes fsync.
    commit_tx_lockfree(tx, uwl_guards, repo, gate.as_ref(), wal.as_ref()).await
}

/// AsyncIndex commit path: uses the traditional sequential commit_lock.
/// Group-commit does not apply because the background-task tail is
/// incompatible with leader-processed materialization.
async fn commit_tx_inner_legacy_async(
    mut tx: TxContext,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    repo: &RepoInstance,
    gate: &shamir_tx::RepoTxGate,
    wal: &shamir_tx::RepoWalManager,
) -> Result<TxOutcome, TxError> {
    let commit_guard = gate.commit_lock().await;

    let PreCommit {
        commit_version,
        uwl_guards,
        version_guard,
    } = match pre_commit_locked(&mut tx, repo, gate, wal, uwl_guards).await {
        Ok(Some(pc)) => pc,
        Ok(None) => {
            repo.tx_metrics().on_tx_committed();
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
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    release_pessimistic_locks(&tx, repo).await;

    let snapshot_version = tx.snapshot_version;
    let tx_id_u64 = tx.tx_id.0;
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);

    apply_data_phase(&mut tx, repo, commit_version).await;
    apply_counter_phase(&tx, repo).await;
    // P0a: consume the RAII VersionGuard → mark(Materialized) + advance
    // last_committed_version from the watermark (fetch_max). Replaces the
    // prior manual mark + sync_last_committed_from_watermark +
    // publish_committed_max trio (identical watermark semantics). Closes H1:
    // a panic between WAL-durable and here drops the guard → Aborted.
    version_guard.commit();
    gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&tx, commit_version));

    maybe_crash("phase6_async", repo).await;
    drop(commit_guard);

    repo.emit_changefeed_event(changefeed_event).await;

    let repo_clone = repo.clone();
    let metrics = repo.tx_metrics().clone();
    let join = tokio::spawn(async move {
        let state = materialize_async_tail(&mut tx, &repo_clone, commit_version, uwl_guards).await;
        if state == MaterializationState::Deferred {
            metrics.on_tx_materialization_deferred();
        }
        // D2 P1d-2b CUTOVER: the inline `gate.mark_durable(commit_version)` is
        // GONE here too. The ack-path wrote only the overlay (visible half);
        // the value becomes durable in `history` only after the background
        // drainer replays the WAL entry. The drainer owns `mark_durable` + WAL
        // truncation. Wake it after the version is published so it drains the
        // freshly-committed tail promptly.
        repo_clone.drainer().wake();
        promote_vectors(&tx, &repo_clone, commit_version).await;
        state
    });

    repo.tx_metrics().on_tx_committed();

    Ok(TxOutcome {
        tx_id: tx_id_u64,
        snapshot_version,
        commit_version,
        materialization: MaterializationState::Complete,
        background: Some(BackgroundCommitHandle { join }),
    })
}

/// P2c lockfree commit path: no global `commit_mutex`.
///
/// Per-table uwl_guards (acquired in pre_commit_prelock in sorted token
/// order) provide the serialization invariant: same-table committers
/// serialize at uwl acquisition; disjoint-table committers proceed fully
/// in parallel. The SSI footprint (`record_commit_writes`) is inserted
/// into the lock-free `scc::TreeIndex` BEFORE `publish_committed_max`
/// advances the reader-visible floor, so future SSI validators see it.
async fn commit_tx_lockfree(
    mut tx: TxContext,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    repo: &RepoInstance,
    gate: &shamir_tx::RepoTxGate,
    wal: &shamir_tx::RepoWalManager,
) -> Result<TxOutcome, TxError> {
    use crate::tx::materialize::{materialize, post_publish_cleanup};
    use crate::tx::pre_commit::pre_commit_locked_validate;

    // Phase 3 + 2 + 2-bis + WAL entry build (no lock needed).
    let validated = match pre_commit_locked_validate(&mut tx, repo, gate, uwl_guards).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            // C6 empty-tx fast-path.
            repo.tx_metrics().on_tx_committed();
            release_pessimistic_locks(&tx, repo).await;
            return Ok(TxOutcome {
                tx_id: tx.tx_id.0,
                snapshot_version: tx.snapshot_version,
                commit_version: tx.snapshot_version,
                materialization: MaterializationState::Complete,
                background: None,
            });
        }
        Err(e) => {
            release_pessimistic_locks(&tx, repo).await;
            return Err(e);
        }
    };

    let commit_version = validated.commit_version;
    let version_guard = validated.version_guard;

    // Phase 4: WAL begin — THE COMMIT POINT.
    // Concurrent-safe: each tx writes a unique WAL key (txn_id).
    if let Err(e) = wal
        .begin_grouped(validated.wal_entry, shamir_wal::WalDurability::Buffered)
        .await
    {
        // version_guard drops here → mark(Aborted): WAL begin failed, the
        // tx is a pre-commit abort and nothing durable exists.
        drop(version_guard);
        release_pessimistic_locks(&tx, repo).await;
        return Err(TxError::Storage(e));
    }

    maybe_crash("phase4", repo).await;

    // Phase 6-bis: record SSI footprint BEFORE publish (lock-free insert).
    gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&tx, commit_version));

    release_pessimistic_locks(&tx, repo).await;
    repo.tx_metrics().on_tx_committed();

    // Phases 5–7: materialize OUTSIDE any global lock, gated by uwl_guards.
    let snapshot_version = tx.snapshot_version;
    let tx_id_u64 = tx.tx_id.0;
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);

    let post_publish = materialize(&mut tx, repo, version_guard, validated.uwl_guards).await;
    let materialization = post_publish_cleanup(post_publish, repo, gate).await;
    if materialization == MaterializationState::Deferred {
        repo.tx_metrics().on_tx_materialization_deferred();
    }
    // D2 P1d-2b CUTOVER: the inline `gate.mark_durable(commit_version)` is
    // GONE. The ack-path no longer writes `history` (only the overlay), so the
    // value is NOT durable at this point — it is durable only after the
    // background drainer replays the WAL entry into `history`. The DRAINER now
    // owns both `mark_durable` and the WAL truncation. We only WAKE it here,
    // after the version is published (visibility), so it drains promptly.
    repo.drainer().wake();
    repo.emit_changefeed_event(changefeed_event).await;
    crate::tx::commit_phases::promote_vectors(&tx, repo, commit_version).await;

    Ok(TxOutcome {
        tx_id: tx_id_u64,
        snapshot_version,
        commit_version,
        materialization,
        background: None,
    })
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
