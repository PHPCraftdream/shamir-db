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
///
/// CONSISTENCY HONESTY (audit MED, by-design I.3 trade-off — be precise,
/// not reassuring): a `Deferred` outcome is *restart-bounded eventually
/// consistent*, NOT immediately consistent. Phase 6 (`publish_committed`)
/// ALWAYS runs, so the MVCC version is published the instant the WAL entry
/// is durable — but the projections that back that version (per-table data
/// → main, per-table index → info) may be only PARTIALLY applied across the
/// tables/indexes the tx touched. A single multi-table tx that defers can
/// leave table A's new rows materialized while table B's failed: a
/// concurrent reader opening a snapshot AFTER the publish sees A's new value
/// and B's OLD value AT THE SAME committed version — a genuine cross-table /
/// data-vs-index inconsistency. It is NOT reconciled online (there is no
/// background reconciler); it persists until the next `recover_v2_inflight`
/// (`RepoInstance::recover_v2_inflight`) on repo open replays the one
/// inflight WAL entry — which carries ALL the tx's ops, every table — and
/// converges every projection. What is still guaranteed even while
/// deferred: a single-key read via `MvccStore::get_at` is never byte-torn
/// (each key is whole-value last-write-wins), and the version floor is
/// monotonic. What lags: cross-table atomicity and data-vs-index agreement,
/// until recovery runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationState {
    /// All projections applied inline; WAL marker removed (Phase 7 ran).
    Complete,
    /// At least one projection sub-phase failed after the commit point.
    /// WAL marker left inflight; recovery is the materialization
    /// guarantor on the next open.
    ///
    /// Multi-table caveat (restart-bounded eventual consistency): when a
    /// tx spanned several tables/indexes, the deferral may be PARTIAL —
    /// some tables materialized inline, others not. The published version
    /// is therefore cross-table-inconsistent until the next
    /// `recover_v2_inflight` replays the inflight WAL entry and reconciles
    /// every table. Single-key reads stay byte-intact throughout; only
    /// cross-table / data-vs-index consistency lags. See the type-level
    /// doc above for the full statement.
    Deferred,
}

#[derive(Debug)]
pub struct TxOutcome {
    pub tx_id: u64,
    pub snapshot_version: u64,
    pub commit_version: u64,
    /// Whether projections materialized inline (`Complete`) or were
    /// deferred to recovery (`Deferred`). Either way the tx is
    /// COMMITTED — see [`MaterializationState`].
    ///
    /// **Async-index mode caveat.** When the tx opted into
    /// [`shamir_tx::CommitVisibility::AsyncIndex`], this field reflects the
    /// state at *ack time*: it can only be `Complete` (sync-prefix phases
    /// landed) since the deferral-bearing phases (5c index, 6.5 markers,
    /// 7 WAL cleanup) are still in flight on the background task. The
    /// truly-final materialization state (the moral equivalent of the sync
    /// `Complete` / `Deferred` outcome) is observable via
    /// [`background`](TxOutcome::background) — awaiting that handle yields
    /// the same `MaterializationState` that sync mode would have returned
    /// on this commit's pipeline tail.
    pub materialization: MaterializationState,
    /// Async-index mode: handle for the background materialization tail.
    /// `None` in sync mode (everything ran inline).
    ///
    /// Tests / callers that need read-your-own-writes on a SECONDARY INDEX
    /// after an async commit can `await` this handle to block until 5c+ has
    /// landed. Production callers normally do NOT await this — the whole
    /// point of async mode is to return without waiting. A failed tail
    /// (panic / abort) does NOT corrupt anything: the inflight WAL marker
    /// is the recovery guarantor, exactly as in the `Deferred` path.
    #[doc(hidden)]
    pub background: Option<BackgroundCommitHandle>,
}

impl TxOutcome {
    /// Convenience: `true` when all projections materialized inline.
    /// `false` means materialization was deferred to recovery (the tx is
    /// still committed).
    ///
    /// In async-index mode this reflects the SYNC-PREFIX result at ack
    /// time and is therefore always `true`. To observe the post-tail state,
    /// `await` [`background`](TxOutcome::background).
    pub fn materialized(&self) -> bool {
        self.materialization == MaterializationState::Complete
    }

    /// Async-index mode: take the background-tail handle (leaves `None`
    /// behind so subsequent calls don't double-await). Returns `None` in
    /// sync mode and on a deferred sync outcome.
    pub fn take_background(&mut self) -> Option<BackgroundCommitHandle> {
        self.background.take()
    }
}

/// Awaitable handle for the async-index materialization tail.
///
/// Returned in [`TxOutcome::background`] when the tx opted into
/// [`shamir_tx::CommitVisibility::AsyncIndex`]. Awaiting it blocks until
/// Phases 5c (index) + 6.5 (markers) + 7 (WAL cleanup) + 5d (HNSW promote)
/// have all finished, and yields the [`MaterializationState`] that would
/// have been returned by an equivalent sync commit. A failed background
/// task (panic) resolves to `MaterializationState::Deferred` — the inflight
/// WAL marker is left for recovery, exactly as in the sync deferral path.
#[derive(Debug)]
pub struct BackgroundCommitHandle {
    join: tokio::task::JoinHandle<MaterializationState>,
}

impl BackgroundCommitHandle {
    /// Wait for the background tail to complete. A panicked task is
    /// reported as `Deferred` (recovery is the guarantor).
    pub async fn join(self) -> MaterializationState {
        match self.join.await {
            Ok(state) => state,
            Err(_) => MaterializationState::Deferred,
        }
    }
}

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
    /// Phase C. A concurrent committer wrote a key matching one of
    /// this tx's recorded predicate dependencies (a phantom). Surfaced
    /// to the wire path as `"tx_conflict"` so existing client retry
    /// covers it.
    #[error("phantom conflict on predicate {dep}")]
    PhantomConflict { dep: String },
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
    if tx.is_expired(DEFAULT_MAX_TX_LIFETIME) {
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
    } = match pre_commit(&mut tx, repo, gate.as_ref(), wal.as_ref()).await? {
        Some(pc) => pc,
        None => {
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
            drop(commit_guard);
            return Ok(TxOutcome {
                tx_id: tx.tx_id.0,
                snapshot_version: tx.snapshot_version,
                commit_version: tx.snapshot_version,
                materialization: MaterializationState::Complete,
                background: None,
            });
        }
    };

    // === Commit point crossed: the WAL entry is durable. ===
    //
    // From here there is NO abort — only materialization. Failures are
    // logged + deferred to recovery; the version is still published.
    //
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
            gate.publish_committed(commit_version);
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

/// Async-mode helper: run Phase 5a (data) inline on the client path.
///
/// Mirrors the per-table loop in [`materialize`] (drain → bounded retry →
/// log warn on persistent failure) but writes outcomes into a shared
/// `Arc<AtomicBool>` so the background tail can flip its final state to
/// `Deferred` if a data write actually failed. The DATA write must finish
/// before ack so read-your-own-writes on data holds.
async fn apply_data_phase(tx: &mut TxContext, repo: &RepoInstance, commit_version: u64) {
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
async fn apply_counter_phase(tx: &TxContext, repo: &RepoInstance) {
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
async fn materialize_async_tail(
    tx: &mut TxContext,
    repo: &RepoInstance,
    commit_version: u64,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> MaterializationState {
    let tx_id = tx.tx_id.0;
    let mut ok = !tx.async_prefix_failed;

    // Phase 5c: index postings.
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
async fn promote_vectors(tx: &TxContext, repo: &RepoInstance, commit_version: u64) {
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

/// Promote one table's staged HNSW vectors into the live graph (Phase 5d,
/// post-lock — see [`promote_vectors`]).
async fn apply_vector_batch(
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
