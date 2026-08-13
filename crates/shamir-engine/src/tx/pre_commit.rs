use std::sync::Arc;

use shamir_collections::{TFxMap, TFxSet};
use shamir_storage::error::DbError;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_tx::{
    CellReservationGuard, IndexFamily, IndexWriteOp, IsolationLevel, RepoTxGate, RepoWalManager,
    TxContext, UniqueGuard,
};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::WalEntryV2;

use crate::index2::kind::IndexKind;
use crate::repo::RepoInstance;
use crate::tx::commit::{maybe_crash, TxError};

use super::commit::wal_ops_from_tx;

// ── F-48b (#867) test-only pause seam ──────────────────────────────────────
//
// Mirrors F-46's `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` (commit.rs, #857),
// F-47's `TEST_POST_GENCHECK_PRE_PUBLISH_HOOK` (fk_reverse_cache.rs, #858),
// and F-48's `TEST_POST_BARRIER_PRE_WRITE_HOOK` (table_manager_crud.rs, #859):
// a `#[cfg(test)]` `OnceLock<Arc<Hook>>` global, zero cost when unset. The
// seam fires in `pre_commit_prelock` strictly AFTER Phase 2.5's
// `needs_write_barrier()` check has run for every table in `tx.write_set`
// (so the test knows which path each table took) and BEFORE Phase 5c's
// materialize write. A test installs the hook, spawns a tx commit,
// busy-polls `reached`, then drives a schema-activation DDL's
// raise+drain+count-proof while the tx is parked, then `resume`s it —
// deterministically reproducing the check-then-act interleaving the
// 2026-07-28 review's P0-3 finding describes for the tx-commit path.

/// Test-only pause/resume handshake installed via
/// [`TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK`]. `reached` lets the harness
/// detect (via polling, no sleeps) that the tx commit under test has
/// actually parked at the seam; `resume` is the release signal; `armed`
/// makes the pause one-shot (CAS true→false) so only the FIRST committer
/// to reach the seam parks — every later arrival (including the harness's
/// own concurrent DDL-side writer, if it also commits) passes straight
/// through. Shape mirrors `commit.rs::PostValidatePrePublishHook` (F-46)
/// and `table_manager_crud.rs::PostBarrierPreWriteHook` (F-48) exactly.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PostPrelockPreMaterializeHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

/// Test-only pause seam. Fires in `pre_commit_prelock` immediately AFTER
/// Phase 2.5's `needs_write_barrier()` check loop (so the writer has
/// committed to the fast or slow path per table) and BEFORE the function
/// returns into the commit pipeline's later phases (Phase 5c materialize
/// being the actual data write). See
/// [`PostPrelockPreMaterializeHook`]'s doc for the exact rationale.
///
/// `nextest` runs each test in its own process, so this global cannot leak
/// across test files. Uninstalled (`None`, the default for every other
/// test) this is a single `OnceLock::get()` read with no lock, no
/// allocation, no await point taken.
#[cfg(test)]
pub(crate) static TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostPrelockPreMaterializeHook>,
> = std::sync::OnceLock::new();

/// Parks on [`TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK`] if a test installed
/// one; a true no-op otherwise.
async fn fire_post_prelock_pre_materialize_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // One-shot: only the FIRST committer to reach this seam actually
        // parks (CAS true→false). Every later arrival passes straight
        // through — see `PostPrelockPreMaterializeHook::armed`'s doc for
        // why this is load-bearing (a concurrent committer / the harness's
        // own DDL-side writer must not deadlock here).
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

/// Stage I: the constant first `u64` of every `WalEntryV2.interner_delta`
/// triple. Pre-Stage-I this slot carried the per-table `table_token` so
/// recovery could route each delta to the right per-table interner. The
/// interner is now per-REPO (one id-namespace shared across tables), so the
/// slot is repurposed to a constant scope marker — recovery resolves the
/// single repo interner directly and ignores this value. The WAL wire-shape
/// (`Vec<(u64, String, u64)>`) is UNCHANGED; no version bump. The constant
/// `0` is chosen so a future reader can distinguish "repo scope" from any
/// nonzero per-table token if a hybrid scheme ever returns.
pub(crate) const REPO_INTERNER_SCOPE: u64 = 0;

/// SSI fix S2 — atomically CLAIM every data write-set key of `tx` on its
/// per-table [`MvccStore`] cell, BEFORE the version is assigned and the WAL
/// entry is written (Phase 4). This moves the "who won the write-write race"
/// decision OUT of the post-WAL publish (Phase 5a) and INTO an atomic
/// pre-WAL claim, so a loser aborts with `SsiConflict` having never touched
/// the WAL (invariant I-PreWAL).
///
/// Scope: **`Serializable` only**. Snapshot is documented last-writer-wins
/// (no read-set validation, no first-committer-wins), and Pessimistic
/// already serializes write-write via per-key Level-3 locks — claiming on
/// either would change documented semantics, so this returns an empty guard
/// vector (zero overhead) off Serializable, mirroring `build_footprint_from_tx`
/// and the read-set validation block, both of which are also Serializable-only.
///
/// One [`CellReservationGuard`] per touched table (each guard owns ONE
/// `Arc<MvccStore>` — the abort-path release target). A won key is `add`ed to
/// its table's guard IMMEDIATELY, so a conflict on a LATER key returns `Err`
/// and the guards (dropped on that `?`) release every already-won key — no
/// partial claim survives an abort.
///
/// `try_reserve` NEVER blocks (no-wait, invariant I-NoWait): a contended cell
/// returns `false` → we abort with `SsiConflict` immediately, so multi-key
/// claim in any order is deadlock-free.
///
/// Returns the guards on success (the caller disarms them once the publisher
/// has finalized every claim), or `Err(SsiConflict)` on the first contended /
/// stale cell (the partial guards drop here → release).
async fn claim_write_set(
    tx: &TxContext,
    repo: &RepoInstance,
) -> Result<Vec<CellReservationGuard>, TxError> {
    // Off Serializable: no claim (Snapshot last-writer-wins / Pessimistic
    // lock-serialized). Empty vec — zero allocation beyond the Vec header.
    if tx.isolation != IsolationLevel::Serializable {
        return Ok(Vec::new());
    }

    let txn_id = tx.tx_id.0;
    let snapshot = tx.snapshot_version;
    let mvcc_map = repo.per_table_mvcc();

    let mut guards: Vec<CellReservationGuard> = Vec::with_capacity(tx.write_set.len());
    for (table_id, staging) in &tx.write_set {
        if staging.is_empty() {
            continue;
        }
        // Per-table MvccStore: cells live per-table, and the publish-side
        // `finalize_reservation` (apply_committed_visible) uses the SAME store,
        // so claim and finalize meet on the same cell. A table absent from
        // `per_table_mvcc` (system / unattached table) has no cell to claim —
        // it also has no overlay/cell finalize, so it is correctly skipped.
        //
        // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
        // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async`. The
        // `per_table_mvcc` map is also touched SYNCHRONOUSLY — `read_sync`
        // (version_provider.rs, EVERY Serializable `validate_read_set` commit),
        // `get_sync` (commit.rs pessimistic-lock release; rename-table) and
        // `iter_sync` (flush_all_history; drainer F6a overlay GC) — and
        // EXCLUSIVELY by `insert_sync` (table attach) / `remove_sync` (drop
        // table). `read_async`'s wait is lock-HANDOFF: saa grants the shared
        // bucket lock to the suspended reader TASK, which then holds it while
        // unpolled in tokio's run queue. A DDL exclusive writer (attach/drop)
        // racing sustained commit/drain traffic can park every worker in
        // `read_sync`/`get_sync`/`insert_sync` behind that unpolled reader →
        // whole-runtime deadlock. `read_sync`'s bucket lock is held only by a
        // RUNNING thread for a few instructions (an `Arc::clone`), bounding
        // every wait. The fn stays `async`; this call no longer suspends.
        let Some(store) = mvcc_map.read_sync(table_id, |_, mvcc| std::sync::Arc::clone(mvcc))
        else {
            continue;
        };
        let mut guard = CellReservationGuard::new(store.clone(), txn_id);
        for key in staging.keys() {
            // task #532: `try_reserve` / the guard / the cell registry are all
            // `RecordKey`-keyed now — pass the staged `RecordKey` straight
            // through, no `Bytes` round-trip on the hot claim path.
            let key: RecordKey = key.clone();
            if store.try_reserve(key.clone(), snapshot, txn_id) {
                // Won — register immediately so an abort on a later key (this
                // table or a subsequent one) releases this claim on drop.
                guard.add(key);
            } else {
                // Contended or stale cell → this committer LOST the race.
                // Returning drops `guard` (releasing this table's won keys) and
                // every earlier table's guard in `guards`, then the tx aborts
                // BEFORE Phase 4 — no WAL is written for a loser. The
                // `SsiConflict` error carries `Bytes`; convert once here on the
                // cold abort path (a necessary boundary conversion).
                repo.tx_metrics().on_tx_aborted_ssi();
                return Err(TxError::SsiConflict { key: key.into() });
            }
        }
        guards.push(guard);
    }
    Ok(guards)
}

/// Outcome of [`pre_commit`]: the assigned MVCC commit version plus the
/// per-table `unique_write_lock` guards that must stay held through
/// Phase 5c (released inside [`materialize`]).
pub(super) struct PreCommit {
    pub(super) commit_version: u64,
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    /// F-48b (#867): kept-alive writer-drain guards (one per fast-path table
    /// in `tx.write_set` whose `needs_write_barrier()` read `false` in Phase
    /// 2.5). Threaded alongside `uwl_guards` through Phase 5c, dropped inside
    /// [`materialize`] after the data/index writes have landed.
    pub(super) drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
    /// RAII owner of the version's terminal-mark obligation (P0a). Survives
    /// to the caller's success path (consumed via `materialize` →
    /// `guard.commit()` → Materialized). WAL begin already succeeded by the
    /// time this is returned, so the only remaining terminal state is
    /// Materialized — but a panic before `materialize` still drops the guard
    /// → Aborted, closing hole H1.
    pub(super) version_guard: shamir_tx::VersionGuard,
    /// SSI fix S2 — RAII owners of this committer's pre-WAL cell-reservations
    /// (one guard per touched table). WAL begin has already succeeded by the
    /// time this is returned, so on the success path the caller `disarm`s these
    /// AFTER the publisher finalizes every claim; any panic before that drops
    /// them → release. Empty off Serializable.
    pub(super) cell_guards: Vec<CellReservationGuard>,
    /// Op #2 Stage 2: the WAL entry that was just persisted via
    /// `begin_grouped`, wrapped in `Arc` so the caller can `offer` it to
    /// the drainer window without cloning the payload again.
    pub(super) wal_entry_arc: Arc<WalEntryV2>,
}

/// Outcome of [`pre_commit_prelock`]: per-table uwl_guards acquired in
/// sorted token order OUTSIDE the commit_lock. These are passed into
/// [`pre_commit_locked`] and then through to [`materialize`].
///
/// F-48b (#867): `drain_guards` carries one [`WriterDrainGuard`] per table in
/// `tx.write_set` whose Phase 2.5 `needs_write_barrier()` read `false` (the
/// fast path). These stay alive through Phase 5c materialize — the exact
/// window a schema-activation / index2-create DDL raising the barrier AFTER
/// the flag read must drain before stamping its proof. Tables that read
/// `true` (slow path) contribute to `uwl_guards` instead and are NOT in the
/// drain set (the lock serializes them — staying in the drain set while
/// blocking on the lock would deadlock).
pub(super) struct PreLockResult {
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    pub(super) drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
}

/// Pre-lock phase of the commit pipeline: runs OUTSIDE `commit_lock`,
/// concurrent with other committers.
///
/// Performs:
/// - Phase 1: interner overlay merge + remap (CAS-safe on DashMap).
/// - Phase 2.5: acquire per-table `unique_write_lock` guards in sorted
///   token order, for every table with a unique guard OR (#538 Part A) an
///   in-flight `create_index_v2` barrier (`needs_write_barrier()`). These
///   serialise against non-tx unique/barriered writers and against other
///   committers touching the same table.
/// - Phase 2.6: authoritative unique re-validation under the uwl_guards.
///   Decisive because the uwl_guard excludes both non-tx writers and other
///   committers for the same table (they block on the same guard). The guard
///   is held continuously through Phase 5c (inside `materialize`), so no
///   writer can interleave between this check and the posting write.
///
/// Lock ordering / ABBA-freedom (§B9, updated for Stage B):
///   - uwl_guards are acquired BEFORE `commit_lock`. A non-tx writer holds
///     at most ONE uwl and NEVER waits on `commit_lock`. Two committers
///     touching overlapping unique tables serialize on the uwl_guards (sorted
///     token order — no ABBA between committers). The `commit_lock` is then
///     acquired by whichever committer holds its uwl_guards; since
///     `commit_lock` is a single global mutex, no ordering cycle is possible:
///     all committers acquire uwl_guards first, then `commit_lock`.
///     Therefore no ABBA cycle is possible.
///
/// cancel-safe: YES for Phase 1 (CAS-based, no durable mutation). Phase 2.5
/// acquires tokio mutexes (cancel-safe per docs — drop releases the wait).
/// Phase 2.6 reads info_store (cancel-safe — read-only).
pub(super) async fn pre_commit_prelock(
    tx: &mut TxContext,
    repo: &RepoInstance,
) -> Result<PreLockResult, TxError> {
    // F-68 (#895) cluster D diagnostic instrumentation, task #124 — entry
    // timestamp. F-70 (#897, P0) confirmed and fixed a genuine lock-order
    // inversion between this fn's shape (drain-guard-then-lock, see the
    // Phase 2.5 loop below — UNCHANGED by F-70) and every DDL path, which
    // used to be lock-then-drain (e.g. `create_index_v2` in
    // `table_manager_index_mgmt.rs`) and is now drain-then-lock via
    // `TableManager::begin_write_barrier` — see `writer_drain_barrier`'s
    // "F-70 — THE canonical lock-order hierarchy" doc section for the full
    // derivation and correctness argument. This logging is kept as a
    // regression tripwire: if a hang ever reproduces again, this pairs with
    // the "pre_commit_prelock: exit" log below (or its absence) to show
    // whether the stall is INSIDE this fn.
    let tx_id_for_log = tx.tx_id.0;
    let prelock_started = std::time::Instant::now();
    log::debug!("pre_commit_prelock: enter tx_id={tx_id_for_log}");
    // Phase 1: interner overlay merge → id remap.
    //
    // Stage I: the interner is per-REPO (one id-namespace shared across
    // every table), so we merge the tx overlay ONCE into the repo interner
    // and obtain ONE remap. The remap is then applied to every touched
    // table's staging bytes — overlay ids are tx-scoped, and the repo
    // interner is the single base they all resolve against, so the same
    // `{overlay_id → base_id}` mapping is correct for every table.
    //
    // A8 fix: after the (optional) overlay merge + remap, EVERY committer
    // with staged bytes additionally scans its staged bytes for any
    // `InternerKey` id referenced above `persisted_high_water()` that is
    // not already in `tx.interner_deltas`, and records `(name, id)` for
    // each. This closes the hole where a later committer's records
    // reference an id some OTHER (possibly aborted-before-WAL) tx created
    // in base — without this pass, no surviving WAL delta would mention
    // that id, and a crash before the next checkpoint would leave the
    // later committer's records undecodable. See
    // `docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md` A8.
    let has_staged_writes = !tx.write_set.is_empty();
    if !tx.interner_overlay.is_empty() || has_staged_writes {
        let repo_interner = repo.repo_interner().await?;
        let base_interner = repo_interner.get().await?;
        if !tx.interner_overlay.is_empty() {
            let shamir_tx::OverlayCommitResult { remap, delta } =
                shamir_tx::commit_interner_overlay(base_interner, &tx.interner_overlay).await?;
            if !delta.is_empty() {
                tx.interner_deltas.extend(delta);
            }
            if !remap.is_empty() {
                let table_ids: Vec<u64> = tx.write_set.keys().cloned().collect();
                for table_id in &table_ids {
                    if let Some(staging) = tx.write_set.get_mut(table_id) {
                        staging
                            .rewrite_set_bytes(|b| {
                                shamir_tx::remap_inner_value_bytes(b.clone(), &remap)
                                    .map_err(|e| format!("remap: {e}"))
                            })
                            .await
                            .map_err(DbError::Codec)?;
                    }
                }
            }
        }
        // A5: interner persist removed from the commit critical path. The WAL
        // entry carries the interner delta (`interner_deltas`), so crash
        // recovery replays new (name, id) mappings via `touch_with_id`. A
        // background checkpoint (every INTERNER_CHECKPOINT_INTERVAL commits)
        // flushes the delta to the durable chunk store, advancing the
        // persisted high-water mark so Phase 7 WAL truncation can proceed.
        // Graceful shutdown flushes the repo interner once.

        // A8: scan staged bytes (now base-id-referencing after the remap
        // above) for any interner id ABOVE `persisted_high_water()` that
        // is not already covered by `tx.interner_deltas`. Each such id was
        // created in base by some tx (possibly this one, possibly another
        // that aborted before WAL) and is NOT yet durably recorded in the
        // chunk store — so THIS committer's WAL must carry `(name, id)`
        // for it or a crash before the next checkpoint makes this tx's own
        // records undecodable. `touch_with_id` (recovery replay) is
        // idempotent, so redundant inclusion across multiple committers'
        // deltas is harmless.
        let hwm = repo_interner.persisted_high_water() as u64;
        // Cheap dedup: build a set of ids already covered by this tx's delta.
        let mut existing: shamir_collections::TFxSet<u64> = shamir_collections::new_fx_set();
        existing.extend(tx.interner_deltas.iter().map(|(_, id)| *id));
        let mut referenced: shamir_collections::TFxMap<u64, ()> = shamir_collections::new_fx_map();
        for staging in tx.write_set.values() {
            for bytes in staging.iter_set_bytes() {
                if let Ok(value) = InnerValue::from_bytes(bytes) {
                    shamir_tx::collect_referenced_ids(&value, &mut referenced);
                }
                // A decode failure here is a pre-existing corruption
                // (staged bytes are always valid msgpack by construction);
                // skip rather than abort — the remap pass above would
                // already have surfaced a codec error for genuinely
                // malformed bytes.
            }
        }
        for (&id, ()) in referenced.iter() {
            if id > hwm && !existing.contains(&id) {
                if let Some(name) = base_interner.get_str(&InternerKey::new(id)) {
                    tx.interner_deltas.push((name.to_string(), id));
                    existing.insert(id);
                }
                // If `get_str` returns None the id is not in the base
                // interner's reverse map — this should not happen for a
                // base id referenced by remapped bytes, but defensively
                // skip rather than panic: an unresolvable id is a separate
                // (already-lost) problem, not something this pass can fix.
            }
        }
    }

    // Phase 2.5 (HIGH-A, extended by #538 Part A): acquire each barriered
    // table's `unique_write_lock` and HOLD it across Phase 2.6 → 5c.
    //
    // The problem this closes: non-tx `insert` / `set` / `delete` take a
    // DIFFERENT mutex — the per-table `unique_write_lock` — and never touch
    // `commit_lock`. So without this step a non-tx unique write could claim
    // or overwrite the same unique posting in the window between this tx's
    // Phase 2.6 re-check and its Phase 5c posting write, producing a
    // duplicate unique value + corrupted index. Acquiring the same per-table
    // lock the non-tx path uses makes the tx's "check unique key free →
    // write posting" atomic against every non-tx unique writer to that table.
    //
    // Two concurrent committers touching the same unique-constrained table
    // serialize on the same uwl_guard (sorted token order prevents ABBA).
    // The loser waits here until the winner's Phase 5c completes and drops
    // the guard — at that point the loser's Phase 2.6 re-check sees the
    // winner's posting and correctly detects the conflict.
    //
    // #538 Part A: the token set was originally built ONLY from
    // `tx.unique_guards` (tables with a base_index UNIQUE index) — an
    // index2-only table (fts/functional/vector, no base_index unique index)
    // contributed nothing, so this tx's Phase 5a/5c could freely interleave
    // with a concurrent `create_index_v2`'s backfill on that table (the same
    // lost-write window #534 closed for the non-tx path, left open here).
    // Fixed by additionally scanning every table this tx actually staged a
    // write to (`tx.write_set` keys) and including its token whenever
    // `TableManager::needs_write_barrier()` is `true` — which is exactly the
    // predicate the non-tx writer methods gate on, so a table with an
    // in-flight `create_index_v2` (index2_create_barrier up) now serializes
    // its tx-commit materialization against that create too, via the SAME
    // `unique_write_lock` `create_index_v2` holds for its full backfill
    // duration. This does NOT close #538's Part B: the index2 ops-PLAN
    // (`tx.index_write_set`) was already captured at STAGE time against an
    // `all_backends()` snapshot, well before this prelock runs — see
    // `TableManager::backfill_index2_backend`'s doc comment for the full
    // accounting.
    //
    // `table_by_token_if_live` (NOT `table_by_token`) resolves the table
    // WITHOUT forcing lazy instantiation: a table that was `add_table`d but
    // never actually accessed via `get_table` stays dormant. This matters —
    // instantiating a `TableManager` as a side effect of merely checking a
    // barrier flag would register it in `per_table_mvcc`, changing
    // `apply_data_batch`'s routing for that table from the direct-store
    // fallback to MVCC-routed, a behavior change this check must not cause.
    // It is also correct: `needs_write_barrier()` can only be `true` on an
    // instance that was actually created (no code path can flip
    // `index2_create_barrier` or register a base_index unique index without
    // first holding a live `TableManager`), so a dormant table is
    // equivalent to "no barrier" for this purpose.
    //
    // We use `lock_owned()` so the guards can be collected into a `Vec`
    // without borrow-lifetime entanglement (each `OwnedMutexGuard` owns its
    // `Arc<Mutex<()>>`). Tables needing neither a unique guard nor the
    // index2-create barrier are untouched — the lock-free fast path is
    // preserved for them.
    //
    // F-48b (#867): the lock-free fast path has the SAME check-then-act race
    // F-48 closed for the non-tx writer methods — `needs_write_barrier()` is
    // read ONCE, and if `false` the tx proceeds through Phase 5c materialize
    // with no further check, so a DDL raising the barrier AFTER this read (and
    // calling `drain_writers()`) sees zero in-flight writers, incorrectly.
    // Fixed by entering the writer-drain set (`enter_writer_drain`) BEFORE
    // reading the flag, for every table in `tx.write_set` — the cross-atomic
    // happens-before edge to the DDL's drain load is carried by the single
    // SeqCst total order over the `active` counter + `flag` (F-56; see
    // `writer_drain_barrier`'s memory-model doc — NOT a flag coherence chain,
    // which Release/Acquire cannot span across two independent atomics). If
    // the flag is `true`
    // (slow path: this table gets a uwl_guard), drop the drain guard BEFORE
    // the lock acquisition — the lock alone provides exclusion, and staying
    // in the drain set while blocking on the lock (held by a DDL that is
    // itself calling `drain_writers()`) would deadlock. If `false`, keep the
    // drain guard alive until Phase 5c materialize lands — the kept-alive
    // guards flow through `PreLockResult` → `PreCommit`/`ValidatedPreCommit`
    // → `materialize`/`materialize_async_tail`, dropped alongside
    // `uwl_guards` after the data/index writes have completed.
    let mut unique_tokens: Vec<u64> = tx.unique_guards.iter().map(|g| g.table_token).collect();
    let mut drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard> = Vec::new();
    for table_id in tx.write_set.keys() {
        if let Some(tbl) = repo.table_by_token_if_live(*table_id).await {
            // F-48b/F-56: bump the drain counter BEFORE reading the flag so the
            // cross-atomic SeqCst happens-before edge reaches the DDL's drain
            // load. Mirrors `table_manager_crud.rs`'s writer methods.
            let drain_guard = tbl.enter_writer_drain();
            if tbl.needs_write_barrier() {
                // Slow path: this table will get a `unique_write_lock` guard
                // (added to `unique_tokens` below). Drop the drain guard
                // BEFORE the lock acquisition — see the comment block above
                // for the deadlock rationale. The lock serializes this table;
                // the drain counter must not.
                drop(drain_guard);
                unique_tokens.push(*table_id);
            } else {
                // Fast path: keep the drain guard alive until Phase 5c
                // materialize. A DDL that raises the barrier AFTER this read
                // and calls `drain_writers()` genuinely waits for this tx.
                drain_guards.push(drain_guard);
            }
        }
    }
    // Sorted + deduped BEFORE any lock is taken — this is the ABBA-freedom
    // invariant the module doc above documents: two committers touching
    // overlapping barriered tables always acquire the guards in the same
    // token order, so no ordering cycle is possible regardless of which
    // source (unique_guards vs. write_set-barrier) contributed the token.
    unique_tokens.sort_unstable();
    unique_tokens.dedup();
    let mut uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>> =
        Vec::with_capacity(unique_tokens.len());
    for token in &unique_tokens {
        if let Some(tbl) = repo.table_by_token(*token).await? {
            // F-68 (#895) cluster D / task #124 — this committer is still
            // holding this table's `drain_guards` (Phase 2.5's fast-path
            // guards, kept alive above for whichever OTHER tables read
            // `needs_write_barrier() == false`) while acquiring THIS table's
            // `unique_write_lock` here. F-70 (#897, P0) confirmed this WAS
            // the other half of a genuine lock-order inversion: every DDL
            // path (`create_index_v2` et al.) used to take the OPPOSITE
            // order (lock, then drain) — if both sides were mid-sequence on
            // overlapping tables, this `.lock_owned()` and the DDL's
            // `drain_writers().await` could each wait on the other forever.
            // Fixed by reordering every DDL path to drain-then-lock
            // (`TableManager::begin_write_barrier`) — this fn's own order is
            // UNCHANGED (it was never the bug; see `writer_drain_barrier`'s
            // "F-70" doc section). Timestamped logging kept as a regression
            // tripwire: a stuck run would show exactly which table_token
            // this committer was blocked on acquiring, and for how long.
            let uwl_wait_started = std::time::Instant::now();
            log::debug!(
                "pre_commit_prelock: tx_id={tx_id_for_log} acquiring unique_write_lock \
                 table_token={token}"
            );
            let guard = tbl.unique_write_lock().lock_owned().await;
            let uwl_wait_elapsed = uwl_wait_started.elapsed();
            if uwl_wait_elapsed >= std::time::Duration::from_secs(1) {
                log::warn!(
                    "pre_commit_prelock: tx_id={tx_id_for_log} unique_write_lock \
                     table_token={token} acquisition took {uwl_wait_elapsed:?} \
                     (>= 1s threshold) — possible lock-order contention, see task #897"
                );
            } else {
                log::debug!(
                    "pre_commit_prelock: tx_id={tx_id_for_log} acquired unique_write_lock \
                     table_token={token} in {uwl_wait_elapsed:?}"
                );
            }
            uwl_guards.push(guard);
        }
    }

    // P0-2 (#958): base_index (regular + unique) ops-plan re-derivation for
    // the `IndexManager` family. A tx that staged before a base_index index
    // was created would otherwise commit with zero ops for it (permanently
    // missing posting for regular; for unique, additionally unconstrained
    // duplicate). For unique defs specifically, the rederive records fresh
    // `UniqueGuard`s (via `tx.record_unique_guard`) so Phase 2.6's
    // commit-time re-validation — which runs immediately AFTER this step —
    // covers constraints that did not exist at stage time. Also handles 2c:
    // retracts staged ops for base_index indexes dropped between stage and
    // commit.
    //
    // #987 (P0 ordering fix): this call MUST run AFTER Phase 2.5's lock
    // acquisition and BEFORE Phase 2.6's `for g in &tx.unique_guards` loop.
    // `tx.unique_guards` is consumed ONLY by that loop — recording guards
    // after it iterates is a dead write, which was the exact #987 bug (a tx
    // staging a duplicate of a pre-existing committed row before CREATE
    // UNIQUE INDEX silently committed, and its SetPosting overwrote the
    // owner's posting at the same deterministic key — index corruption).
    // A table that gains its first unique index mid-tx is still locked here:
    // Phase 2.5's `needs_write_barrier()` scan over `tx.write_set` sees
    // `UNIQUE_INDEX_EXISTS` set the moment `CREATE UNIQUE INDEX` registers
    // the def, so that table's `unique_write_lock` IS acquired even though
    // `tx.unique_guards` (built from stage-time guards only) did not yet
    // know about the constraint. Moving this call earlier keeps that lock
    // held across both the guard recording and Phase 2.6's re-validation,
    // preserving the check-then-act atomicity against non-tx unique writers.
    //
    // The index2 rederive (`rederive_index2_ops_post_stage`, run later
    // below) is independent — separate generation counters, separate op
    // families — and is left in its original position.
    rederive_base_index_ops_post_stage(tx, repo).await?;

    // #1100: Re-derive posting ops for records whose value changed concurrently.
    // This closes the gap where DELETE/UPDATE planned from a stale snapshot
    // never removes the record's CURRENT posting.
    log::debug!(
        "rederive_stale_value_ops_post_stage: starting, base_index_stage_gens.len()={}",
        tx.base_index_stage_gens.len()
    );
    rederive_stale_value_ops_post_stage(tx, repo).await?;

    // Phase 2.6: authoritative unique re-validation under per-table
    // unique_write_lock (held since Phase 2.5).
    //
    // Stage-time `validate_unique_*` is optimistic — it reads pre-commit
    // state, so two concurrent txs claiming the same unique value both
    // pass it. The per-table `unique_write_lock`s (Phase 2.5) exclude
    // BOTH non-tx unique writers AND other committers touching the same
    // table, so re-checking the claimed keys here is decisive against ALL
    // writers: no other writer can interleave between this check and the
    // Phase 5c posting write (the uwl_guard is held continuously).
    //
    // #1039 / #1074 — Intra-tx unique dedup + cross-tx re-validation.
    //
    // #1039 added an intra-tx dedup pass over `tx.unique_guards`. But
    // `unique_guards` is a chronological list of point-in-time CLAIMS — DELETE
    // never pushes a guard, and an UPDATE that moves a record OFF a unique key
    // pushes no "release" marker for the old key either. So #1039's check
    // treated the FIRST owner seen for a key as authoritative and
    // false-rejected any later guard with a different owner, even when the
    // first owner had legitimately vacated that key earlier in the same tx
    // (via UPDATE-off or DELETE) — a legal pattern that committed fine before
    // #1039 (#1074 regression).
    //
    // #1074 fix: use `tx.index_write_set` — the authoritative ordered storage
    // mutation sequence — for the intra-tx conflict detection. `RemovePosting`
    // (emitted by `plan_record_updated_unique` / `plan_record_deleted_unique`
    // when a record moves off or is deleted from a unique key) IS the release
    // marker that `unique_guards` lacks. Walking the unique-family ops in
    // staging order and tracking live ownership per key gives the TRUE final
    // per-key state:
    //   - SetPosting on a key whose current live owner is a DIFFERENT record →
    //     genuine intra-tx collision (no release between them) → abort.
    //   - RemovePosting releases the key, so a LATER SetPosting from a
    //     different record is a legitimate release-then-reclaim, not a conflict.
    //
    // `tx.unique_guards` is still iterated for the durable-state (cross-tx)
    // check, because an update re-writing the SAME unique value produces NO
    // SetPosting (plan_record_updated_unique is a no-op when old_key == new_key)
    // but DOES push a guard — that key still needs committed-state validation.
    // For keys in the write-set walk, the FINAL owner from the walk is used
    // (may differ from the guard's owner if the key was released and reclaimed);
    //
    // **#1096 closes the "release-then-reclaim a DURABLE (prior-tx) unique
    // value within one tx" scenario** (found not fully covered by #1074 in
    // an @oh review, 2026-08-11): `tx1: INSERT A {email:"x"}; COMMIT` then
    // `tx2: DELETE A; INSERT B {email:"x"}`. Two call sites needed fixing,
    // one per copy of the durable-only assumption:
    //   - `table_manager_tx_ops.rs::insert_tx`'s stage-time
    //     `validate_unique_for_create` (read-only against committed state)
    //     now takes `released_unique_keys_in_tx` (this same live/released
    //     walk, run at stage time against only the value's own claimed
    //     keys) so it no longer synchronously rejects `INSERT B` with
    //     `DuplicateKey` before it is ever staged into
    //     `tx.index_write_set`.
    //   - THIS function's Step 2 durable check (below) independently made
    //     the same durable-only assumption: even after `INSERT B` reaches
    //     staging, `info_store.get(index_key)` still returns `A` (Phase 5c's
    //     materialize write, which would remove `A`'s posting, hasn't run
    //     yet — pre_commit validates BEFORE that write). Step 2 now
    //     tolerates a durable-owner mismatch when the key is in
    //     `ever_released` — this tx's own `index_write_set` staged a
    //     `RemovePosting` for that exact key earlier, so the stale durable
    //     owner is provably the record being released, not an unrelated
    //     conflict.
    // for keys NOT in the write set (self-write), the guard's owner stands.
    //
    // Why option (a) / naive overwrite on `unique_guards` is insufficient: it
    // has no release markers, so it cannot distinguish "A moved off K before B
    // claimed it" from "A never moved off K and B is a genuine duplicate" —
    // the former must commit, the latter must abort, but both produce the same
    // claims-only guard sequence [(K, A), (K, B)].

    // Step 1 — Walk unique-family ops in index_write_set in staging order to
    // detect intra-tx conflicts and determine the final live owner per key.
    // `live`: (table_token, index_key) → record CURRENTLY owning it as of the
    // op being processed.  `released`: keys vacated by a RemovePosting so the
    // guard loop (step 2) can distinguish "released" from "never in write set".
    // `ever_released`: the union of every key that had AT LEAST ONE
    // RemovePosting during the walk, whether or not it was later reclaimed —
    // #1096: Step 2's durable check needs this (not just the final-state
    // `released`) to recognize a release-then-reclaim-within-this-tx of a
    // key whose PRIOR owner is DURABLE (committed by an earlier tx), where
    // `released` alone would already have been cleared by the reclaiming
    // SetPosting.
    let mut live: TFxMap<(u64, bytes::Bytes), RecordId> = TFxMap::default();
    let mut released: TFxSet<(u64, bytes::Bytes)> = TFxSet::default();
    let mut ever_released: TFxSet<(u64, bytes::Bytes)> = TFxSet::default();
    // #1097 follow-up (found by an `@oh` review of the #1097 fix itself):
    // a `RemovePosting` this walk classifies as STALE (planned against a
    // record that no longer holds the key, per the match logic below) must
    // not just be ignored for `live`/`released` bookkeeping — it is still
    // physically present in `tx.index_write_set` and would otherwise reach
    // Phase 5c's `apply_index_ops_at_commit` unchanged, which applies EVERY
    // op in staging order with last-write-wins semantics. Left in place, a
    // stale removal ordered AFTER the genuine owner's `SetPosting` for the
    // same key deletes that owner's posting from `info_store` at commit —
    // the record's DATA row still carries the value, but the unique index
    // has no posting for it at all, silently freeing the key for an
    // unrelated future INSERT (the exact index/data divergence #1097 exists
    // to prevent, reachable one statement shorter than the original
    // reproduction, without ever needing a colliding third record). Indices
    // of stale ops are collected here and the ops themselves retracted from
    // `tx.index_write_set` below, before Phase 4's WAL write — mirroring
    // `retract_stale_provenance_ops`'s established retraction pattern in
    // this same file.
    let mut stale_remove_indices: TFxSet<usize> = TFxSet::default();
    // #1097 round 3 (found by a second `@oh` review): a `RemovePosting` with
    // NO in-tx `live` claim to compare against (`current_owner.is_none()`)
    // cannot be judged stale-or-not by the sync walk above at all — that is
    // exactly the shape of a bare `DELETE` with no companion `SetPosting` in
    // the same tx. Such an op previously executed unconditionally, with zero
    // validation: `delete_tx` records no `UniqueGuard` (Step 2 below never
    // even looks at the key), and there is nothing in `live` to collide
    // with. A `DELETE` planned against a stale snapshot could therefore
    // silently delete a DIFFERENT, concurrently-reclaiming record's genuine
    // durable posting — the same index/data divergence #1097 exists to
    // prevent, one more shape of it. Each such candidate is resolved against
    // the CURRENT DURABLE posting value below, under the same
    // `unique_write_lock` protection Step 2's own durable reads already
    // rely on (held since Phase 2.5, before this walk runs) — a mismatch
    // means this op's plan is stale relative to durable state and it is
    // retracted exactly like an intra-tx-stale op above.
    let mut unverified_removes: Vec<(usize, u64, bytes::Bytes, RecordId)> = Vec::new();
    for (idx, (table_token, op)) in tx.index_write_set.iter().enumerate() {
        match op {
            IndexWriteOp::SetPosting {
                key,
                value,
                provenance,
            } if provenance.family == IndexFamily::Unique => {
                let k = (*table_token, key.clone());
                let owner = RecordId::try_from_bytes(value).unwrap_or_default();
                if let Some(&prior) = live.get(&k) {
                    if prior != owner {
                        // Two DIFFERENT records claim the same unique key with
                        // no intervening RemovePosting — genuine intra-tx
                        // collision (the #1039 positive case).
                        repo.tx_metrics().on_tx_aborted_unique();
                        return Err(TxError::UniqueViolation { key: key.clone() });
                    }
                    // Same owner re-claiming its own key — not a conflict.
                    continue;
                }
                live.insert(k.clone(), owner);
                released.remove(&k);
            }
            IndexWriteOp::RemovePosting {
                key,
                provenance,
                owner,
            } if provenance.family == IndexFamily::Unique => {
                let k = (*table_token, key.clone());
                // #1097: only clear the live claim when this op's declared
                // owner actually matches the record `live` currently
                // attributes the key to. A MISMATCH means this removal was
                // planned against a record that no longer holds the key per
                // this tx's own walk so far (built against a stale snapshot
                // — see this file's #1097 doc above) — treat it as a stale
                // no-op: do not clear `live`, do not mark the key released,
                // so the key's real current owner within this tx is
                // preserved and Step 2 validates against it correctly. Also
                // retract the op itself (see the comment above this loop)
                // so it never reaches Phase 5c.
                let removing_owner = owner.map(RecordId);
                let current_owner = live.get(&k).copied();
                if let (None, Some(declared_owner)) = (current_owner, removing_owner) {
                    // No in-tx claim to judge this against — defer to the
                    // durable-state check below rather than assuming the
                    // plan is still valid (see the comment above this loop).
                    unverified_removes.push((idx, *table_token, key.clone(), declared_owner));
                    live.remove(&k);
                    released.insert(k.clone());
                    ever_released.insert(k);
                } else if removing_owner.is_none() || current_owner == removing_owner {
                    // The op didn't declare an owner (the unconditional
                    // pre-#1097 fallback, kept for any future non-unique
                    // construction site that might reach here) or its
                    // declared owner matches the current live claim.
                    live.remove(&k);
                    released.insert(k.clone());
                    ever_released.insert(k);
                } else {
                    stale_remove_indices.insert(idx);
                }
            }
            _ => {}
        }
    }
    for (idx, table_token, key, declared_owner) in &unverified_removes {
        let Some(tbl) = repo.table_by_token(*table_token).await? else {
            continue;
        };
        match tbl.info_store().get(key.clone().into()).await {
            Ok(existing) => {
                if RecordId::try_from_bytes(&existing) != Some(*declared_owner) {
                    // The durable posting is free, or durably owned by a
                    // DIFFERENT record than this removal was planned
                    // against (a concurrent tx reclaimed it after this tx's
                    // snapshot was taken) — this removal's plan is stale;
                    // retract it and undo its provisional released/
                    // ever_released marking above so a later op in this
                    // same tx cannot be misled by it either.
                    stale_remove_indices.insert(*idx);
                    let k = (*table_token, key.clone());
                    if !live.contains_key(&k) {
                        released.remove(&k);
                        ever_released.remove(&k);
                    }
                }
                // Else: durable owner matches — this removal is genuine,
                // keep it (it will correctly clear the durable owner's
                // posting at Phase 5c).
            }
            Err(DbError::NotFound(_)) => {
                // Already free durably — applying the removal again is a
                // harmless no-op at the storage layer; no retraction needed.
            }
            Err(e) => return Err(TxError::Storage(e)),
        }
    }
    if !stale_remove_indices.is_empty() {
        let mut idx = 0usize;
        tx.index_write_set.retain(|_| {
            let keep = !stale_remove_indices.contains(&idx);
            idx += 1;
            keep
        });
    }

    // Step 2 — Durable-state (cross-tx) check. Iterate tx.unique_guards to
    // discover every unique key this tx claims. For each key:
    //   - In `live`  → use the FINAL owner from the write-set walk (may differ
    //     from the guard's owner if the key was released and reclaimed).
    //   - In `released` only → the tx vacated this key; no durable check needed.
    //   - In neither → self-write (update re-writing same value: no SetPosting,
    //     but the guard's owner is the valid claim).
    let mut checked: TFxSet<(u64, bytes::Bytes)> = TFxSet::default();
    for g in &tx.unique_guards {
        let key = (g.table_token, g.index_key.clone());
        if !checked.insert(key.clone()) {
            continue; // already durable-checked this key
        }

        // #1101: Keys in `released` require a durable check too — if the tx
        // planned to release a key but a concurrently-committed tx already
        // claimed that key for an unrelated record, this tx's stale Set+Remove
        // pair would clobber that unrelated posting at Phase 5c. The check below
        // validates that the durable state is consistent with the tx's plan.
        let owner = if let Some(&final_owner) = live.get(&key) {
            Some(final_owner)
        } else if released.contains(&key) {
            // Key vacated by this tx's own ops. No owner to compare against
            // below, but we still need to check that nothing else durably
            // claimed it (see the durable check below).
            None
        } else {
            Some(g.owner) // self-write: no index op, guard's owner is authoritative
        };

        // Durable-state check against committed storage (unchanged in spirit
        // from #1039 — this task fixes what "seen" represents, not this check).
        //
        // #1096: a durable owner that differs from `owner` is NOT a conflict
        // when this tx itself released this exact key earlier in its own
        // `index_write_set` (`ever_released`) — the durable value is the
        // record this tx's own DELETE/UPDATE-off is removing; Phase 5c will
        // overwrite it atomically with the same posting ops validated here.
        //
        // #1096 follow-up (found by `@oh` review): `ever_released` alone is
        // NOT sufficient — it only proves this tx PLANNED to release the
        // key, not that the plan is still valid. Under `Snapshot` isolation
        // (documented last-writer-wins, no write-write conflict detection —
        // see `claim_write_set` above), that plan was built against a
        // possibly-STALE snapshot: a concurrently-committed tx may already
        // have reclaimed this key for an UNRELATED record between this tx's
        // snapshot and this check. Requiring the CURRENT durable owner
        // (`existing`) to be a record THIS tx has itself staged a write for
        // (`tx.write_set`) closes that hole — a record this tx never wrote
        // is never in its own write set, so a stale-snapshot race correctly
        // falls through to `UniqueViolation` below instead of silently
        // admitting two live records for the same unique value.
        //
        // #1101: The `released.contains(&key)` case previously skipped this
        // check entirely with `continue`. The fix is to perform the check with
        // `owner = None`: we require that the durable key is either NotFound
        // (genuinely free — the net-release's RemovePosting will be a no-op)
        // OR owned by a record this tx has staged a write for (touched). If
        // neither holds, an unrelated tx durably claimed the key and we must
        // abort to avoid clobbering that posting.
        if let Some(tbl) = repo.table_by_token(g.table_token).await? {
            match tbl.info_store().get(g.index_key.clone().into()).await {
                Ok(existing) => {
                    let released_and_touched = ever_released.contains(&key)
                        && RecordId::try_from_bytes(&existing).is_some_and(|existing_id| {
                            tx.write_set
                                .get(&g.table_token)
                                .is_some_and(|s| s.staged_op(existing_id.as_bytes()).is_some())
                        });
                    // For keys this tx still owns (owner is Some), compare the
                    // durable owner against the expected owner. For released keys
                    // (owner is None), just check the released_and_touched
                    // tolerance — a concurrent claim is only OK if this tx touched
                    // that record.
                    match owner {
                        None => {
                            // Released key: abort if a concurrent tx durably claimed
                            // it for an untouched record.
                            if !released_and_touched {
                                repo.tx_metrics().on_tx_aborted_unique();
                                return Err(TxError::UniqueViolation {
                                    key: g.index_key.clone(),
                                });
                            }
                        }
                        Some(expected_owner) => {
                            if existing.as_ref() != expected_owner.as_bytes().as_slice()
                                && !released_and_touched
                            {
                                repo.tx_metrics().on_tx_aborted_unique();
                                return Err(TxError::UniqueViolation {
                                    key: g.index_key.clone(),
                                });
                            }
                        }
                    }
                }
                Err(DbError::NotFound(_)) => {
                    // Key is durably free. This is always OK:
                    // - For owned keys: our claim is uncontended.
                    // - For released keys: the RemovePosting will be a no-op
                    //   (key is already free).
                }
                Err(e) => return Err(TxError::Storage(e)),
            }
        }
    }

    // F-50 (#869, spike) — Phase 2.7: re-derive index2 posting ops for any
    // backend registered AFTER this tx's stage-time `all_backends()` snapshot.
    //
    // This closes #538 Part B's "guaranteed miss": a tx that staged before a
    // new index2 backend existed (via `create_index_v2`) and commits after it
    // was registered would otherwise carry zero ops for it in
    // `tx.index_write_set` (planned stale, at stage time), so Phase 5c would
    // have nothing to write — the row is permanently absent from the new
    // index. Part A's commit-time lock serialization (Phase 2.5 above) only
    // fixed the commit's TIMING; it cannot retroactively add ops to an
    // already-built plan.
    //
    // Placement is load-bearing for crash safety:
    //   - AFTER Phase 2.5's per-table `unique_write_lock` acquisition, so any
    //     in-flight `create_index_v2` (which holds that lock across its full
    //     backfill→register sequence) has finished registering by the time we
    //     re-derive — the live `backends_newer_than(stage_gen)` snapshot we
    //     take here includes it.
    //   - BEFORE Phase 4's WAL `begin_grouped` (which runs later, inside
    //     `pre_commit_locked_validate`): `wal_ops_from_tx` serializes
    //     `tx.index_write_set` directly into the WAL entry, so the re-derived
    //     ops MUST be appended before that serialization or recovery would
    //     replay the STALE stage-time plan (silently re-opening the exact miss
    //     this step closes). Re-deriving AFTER the WAL write is forbidden.
    //   - OUTSIDE `commit_lock` (this whole fn runs pre-lock): the work is
    //     per-table, async, and has no commit-window dependency, so holding
    //     the global commit_lock for it would be needless contention.
    //
    // The generation gate makes this zero-cost on the common path: a tx that
    // captured `index2_stage_gens` (populated only by index2-bearing-table
    // staging) and sees an unchanged generation skips the per-record
    // re-derivation entirely. See `rederive_index2_ops_post_stage` for the
    // old-value resolution (insert-vs-update-vs-delete) mechanism.
    rederive_index2_ops_post_stage(tx, repo).await?;

    // F-48b test seam: parks strictly AFTER Phase 2.5's flag-check loop
    // (every table's `needs_write_barrier()` has been read and the writer
    // has committed to the fast or slow path per table) and BEFORE the
    // function returns into the commit pipeline's later phases — in
    // particular before Phase 5c's materialize write (the actual data
    // store mutation). No-op in every non-test build.
    fire_post_prelock_pre_materialize_test_hook().await;

    // F-68 (#895) cluster D / task #124 — exit timestamp, paired with the
    // "enter" log above. If a run hangs, the LAST "enter" without a
    // matching "exit" for the same tx_id pinpoints that the stall is
    // somewhere inside this function (most likely the per-table
    // unique_write_lock loop just above — see task #897).
    log::debug!(
        "pre_commit_prelock: exit tx_id={tx_id_for_log} elapsed={:?} \
         uwl_guards={} drain_guards={}",
        prelock_started.elapsed(),
        uwl_guards.len(),
        drain_guards.len()
    );

    Ok(PreLockResult {
        uwl_guards,
        drain_guards,
    })
}

// ── F-73 (#900) test-only failure-injection seam ────────────────────────────
//
// Mirrors `table_manager_streaming.rs::TEST_READ_ONE_TX_BYTES_FAILURE` /
// `ReadOneTxBytesFailHook` (F-65, #891): a `#[cfg(test)]` `OnceLock<Arc<Hook>>`
// global, zero cost when unset. Lets a test make the NEXT
// `read_pre_tx_bytes(table_token, id)` call for a specific `(table_token,
// RecordId)` return a genuine `Err(DbError::Storage)` instead of reading the
// pre-tx bytes — so `rederive_index2_ops_post_stage`'s commit-time storage
// read hits a real, deterministic error (no sleeps, no timing races) proving
// the F-73 fail-closed fix.
//
// One-shot per arm: once an armed `(table_token, id)` fires it is consumed,
// so a retry of the SAME id reads normally.
#[cfg(test)]
pub(crate) static TEST_REDERIVE_PRE_TX_READ_FAILURE: std::sync::OnceLock<
    std::sync::Arc<RederivePreTxReadFailHook>,
> = std::sync::OnceLock::new();

/// Test-only one-shot failure injector for [`read_pre_tx_bytes`]. See
/// [`TEST_REDERIVE_PRE_TX_READ_FAILURE`]'s doc for the rationale.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct RederivePreTxReadFailHook {
    inner: std::sync::Mutex<Vec<(u64, RecordId)>>,
}

#[cfg(test)]
impl RederivePreTxReadFailHook {
    /// Arm a one-shot injected `Err` for the next
    /// `read_pre_tx_bytes(table_token, id)`.
    pub(crate) fn arm(&self, table_token: u64, id: RecordId) {
        self.inner.lock().unwrap().push((table_token, id));
    }

    /// Returns `Some(Err)` if `(table_token, id)` is armed, consuming the arm
    /// (one-shot); `None` otherwise.
    fn take_injected(&self, table_token: u64, id: RecordId) -> Option<DbError> {
        let mut guard = self.inner.lock().unwrap();
        let pos = guard
            .iter()
            .position(|(tt, rid)| *tt == table_token && *rid == id)?;
        guard.swap_remove(pos);
        Some(DbError::Storage(format!(
            "F-73 injected rederive pre-tx read failure (table_token={table_token}, id={id:?})"
        )))
    }
}

/// Read the pre-tx value of a staged record's key from `data_store`, used by
/// [`rederive_index2_ops_post_stage`] to distinguish insert vs. update/delete.
/// Returns `Ok(None)` for `DbError::NotFound` (the proven "no pre-tx row"
/// semantics — see that function's doc), `Ok(Some(bytes))` on a hit, and
/// propagates every other error (F-73: a transient storage error here must
/// abort the tx, not silently skip the record).
async fn read_pre_tx_bytes(
    data_store: &Arc<dyn shamir_storage::types::Store>,
    #[cfg_attr(not(test), allow(unused_variables))] table_token: u64,
    #[cfg_attr(not(test), allow(unused_variables))] rid: RecordId,
    key: &RecordKey,
) -> Result<Option<bytes::Bytes>, DbError> {
    #[cfg(test)]
    if let Some(hook) = TEST_REDERIVE_PRE_TX_READ_FAILURE.get() {
        if let Some(err) = hook.take_injected(table_token, rid) {
            return Err(err);
        }
    }
    match data_store.get(key.clone()).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// F-50 (#869, spike) — Phase 2.7 worker: re-derive index2 posting ops for
/// backends registered AFTER each touched table's stage-time snapshot.
///
/// Invoked from [`pre_commit_prelock`] AFTER Phase 2.5's barrier locks are
/// acquired and BEFORE Phase 4's WAL begin. See that call site's comment for
/// the crash-safety ordering constraints.
///
/// cancel-safe: YES — appends to `tx.index_write_set` only (in-memory, tx-
/// scoped; dropped by RAII on abort). The `data_store.get` reads are
/// read-only. No durable mutation happens here, so cancellation before Phase
/// 4 is a clean abort (the re-derived ops never reached the WAL).
///
/// Old-value resolution (insert vs. update vs. delete): `tx.write_set` only
/// carries the NET staged op (`Set`/`Remove`) — it does not retain whether a
/// `Set` is a fresh insert or an overwrite, nor the pre-tx value an update /
/// delete needs to plan the REMOVE half of its posting diff. Phase 5a
/// materialize has NOT run yet at this point (we are still in prelock), so
/// the data store still holds the PRE-tx committed value. A single
/// `data_store.get(key)` per staged record settles it: `NotFound` ⇒ insert
/// (plan_insert), `Some(old)` ⇒ update (plan_update) or delete (plan_delete).
/// The cost is bounded by the number of staged records AND gated behind the
/// generation check (only paid when an index2 backend was actually registered
/// between stage and commit — the rare DDL-concurrent case).
///
/// Scope note (Step 2, #870): the stage-time generation is now captured on
/// ALL mutation staging paths — INSERT (`insert_tx` / `insert_tx_many` /
/// `insert_tx_many_bytes`), UPDATE (`update_tx` / `update_tx_bytes`),
/// DELETE (`delete_tx`), and SET (`set_tx` — an alias of `update_tx`).
/// The sorted-index generation is captured alongside (`note_sorted_stage_gen`)
/// for every stage site, gating the base_index sorted-index re-derivation below.
/// Vector backends' `staged_vectors` are re-derived in the per-record loop
/// (their `plan_insert_tx` is a no-op; HNSW embeddings are buffered
/// separately) — Step 2 Part B.
///
/// F-73 (#900, P0) — FAIL CLOSED. This function used to return `()` and
/// silently swallow every error class it could hit: a non-`NotFound`
/// `data_store.get` error, a `plan_insert_tx`/`plan_update_tx`/
/// `plan_delete_tx`/`plan_record_*` `Err`, a record that fails
/// `InnerValue::from_bytes`, or a staged key that fails
/// `RecordId::try_from_bytes`. Because Phase 5a (the data mutation) runs
/// UNCONDITIONALLY later in the pipeline, a transient error here used to let
/// the tx commit successfully while silently skipping the row's posting for
/// the new index — a permanent, unreported table/index divergence. Now every
/// one of those classes propagates via `?`/`map_err` and the tx aborts before
/// Phase 4's WAL begin (see the call site in `pre_commit_prelock`). The ONE
/// exception, unchanged from before: `Err(DbError::NotFound(_))` on
/// `data_store.get` stays the sole "treat as insert" case — that is the
/// PROVEN semantics at this call site (Phase 5a hasn't run yet, so the store
/// still holds the pre-tx value; `NotFound` genuinely means "no pre-tx row").
/// A malformed staged key or an undecodable staged/stored record is an
/// internal invariant violation (the staging path guarantees well-formed
/// keys/values reach here), surfaced as `DbError::Internal` /
/// `DbError::Codec` respectively — the same typed-error-over-bare-string
/// style F-55 (#881, commit `f9eed337`) and F-65 (#891, commit `28d39f31`)
/// established for this fail-open defect class.
async fn rederive_index2_ops_post_stage(
    tx: &mut TxContext,
    repo: &RepoInstance,
) -> Result<(), TxError> {
    // =========================================================================
    // Index2 re-derivation (F-50 #869 Step 1) + vector staging (Step 2 Part B)
    // =========================================================================
    //
    // Empty for the overwhelming majority of txs (populated only by an
    // index2-bearing-table staging path) — the single zero-overhead gate.
    if !tx.index2_stage_gens.is_empty() {
        // Clone the captured (table_token, stage_gen) pairs out so the per-table
        // async work below does not hold a borrow of `tx` (we must mutably
        // reborrow `tx.index_write_set` to append re-derived ops).
        let stage_gens: Vec<(u64, u64)> =
            tx.index2_stage_gens.iter().map(|(t, g)| (*t, *g)).collect();
        let tx_id = Some(tx.tx_id);

        for (table_token, stage_gen) in stage_gens {
            // Resolve WITHOUT forcing lazy instantiation — a dormant table has no
            // index2 backends (mirrors Phase 2.5's `table_by_token_if_live`).
            let Some(tbl) = repo.table_by_token_if_live(table_token).await else {
                continue;
            };
            // Generation gate: skip EVERYTHING (both add and retract) when
            // nothing changed since stage. One atomic Acquire load.
            let reg = tbl.index2_registry();
            if reg.generation() == stage_gen {
                continue;
            }
            // R0-B (#1008): unlike the pre-R0-B code, do NOT `continue` here
            // when `new_backends` is empty. The generation advancing with no
            // NEW backend means something was DROPPED (or replaced —
            // ABA) since stage — exactly the case retraction below must
            // catch. `backends_newer_than` only ever reports newly
            // REGISTERED backends, so it is correctly empty for a pure-DROP
            // generation bump; skipping the whole iteration on that empty
            // check (the old shape) is precisely how index2 never retracted
            // stale ops for a DROP'd backend.
            let new_backends = reg.backends_newer_than(stage_gen).await;
            // Collect the staged ops for this table into an owned Vec so no borrow
            // of `tx.write_set` is held across the per-record async planning below.
            // Retraction (below) does not need this — only the per-record ADD
            // loop does — so an empty `new_backends` still runs retraction
            // even if there happen to be no staged ops for this table.
            let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
                Some(staging) => staging.snapshot_ops(),
                None => Vec::new(),
            };
            let data_store = tbl.data_store().clone();

            let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();
            if !new_backends.is_empty() {
                for kvop in staged_ops {
                    match kvop {
                        KvOp::Set(k, v) => {
                            // F-73: a staged key that doesn't decode as a RecordId is an
                            // internal invariant violation (the staging path guarantees
                            // well-formed keys reach here) — fail the tx, don't skip.
                            let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_index2_ops_post_stage: malformed staged key \
                                 (table_token={table_token}, {} bytes) — expected a \
                                 16-byte RecordId",
                                    k.len()
                                )))
                            })?;
                            // F-73: a staged record that fails to decode is corruption,
                            // not a normal runtime condition — fail the tx.
                            let new_rec = InnerValue::from_bytes(&v).map_err(|e| {
                                TxError::Storage(DbError::Codec(format!(
                                    "rederive_index2_ops_post_stage: staged record decode \
                                 failed (table_token={table_token}, rid={rid:?}): {e}"
                                )))
                            })?;
                            // Phase 5a has not run: the store still holds the PRE-tx
                            // value, so this one read distinguishes insert vs. update.
                            match read_pre_tx_bytes(&data_store, table_token, rid, &k).await {
                                Ok(Some(old_bytes)) => {
                                    // F-73: the pre-tx value MUST decode — it was written
                                    // by a prior successful commit through this same
                                    // codec. A decode failure here is corruption, not
                                    // "skip this record".
                                    let old_rec =
                                        InnerValue::from_bytes(&old_bytes).map_err(|e| {
                                            TxError::Storage(DbError::Codec(format!(
                                                "rederive_index2_ops_post_stage: pre-tx record \
                                         decode failed (table_token={table_token}, \
                                         rid={rid:?}): {e}"
                                            )))
                                        })?;
                                    for backend in &new_backends {
                                        let mut ops = backend
                                            .plan_update_tx(rid, &old_rec, &new_rec, tx_id)
                                            .await
                                            .map_err(|e| {
                                                TxError::Storage(DbError::Internal(format!(
                                                    "rederive_index2_ops_post_stage: \
                                                 plan_update_tx failed \
                                                 (table_token={table_token}, rid={rid:?}): {e}"
                                                )))
                                            })?;
                                        // R0-B (#1008): stamp the REAL live
                                        // instance epoch — see
                                        // `index2_provenance`'s doc for why the
                                        // backend itself cannot know it.
                                        stamp_index2_ops_provenance(reg, backend, &mut ops).await;
                                        appended
                                            .extend(ops.into_iter().map(|op| (table_token, op)));
                                        // F-50 Step 2 (#870, Part B): re-derive
                                        // staged vectors for a new vector backend.
                                        // VectorBackend::plan_update_tx is a no-op
                                        // for a tx (HNSW embeddings route through
                                        // tx.staged_vectors instead), so the posting
                                        // loop above contributes nothing for it.
                                        // Mirror stage_vector_deletes_on_update +
                                        // stage_vectors from table_manager_tx_ops:
                                        // if old carried a vector and new does not,
                                        // stage a delete; otherwise stage the new.
                                        if is_vector_backend(backend) {
                                            if backend.staged_vector(rid, &old_rec).await.is_some()
                                                && backend
                                                    .staged_vector(rid, &new_rec)
                                                    .await
                                                    .is_none()
                                            {
                                                tx.stage_vector_delete(table_token, rid);
                                            }
                                            if let Some(vec) =
                                                backend.staged_vector(rid, &new_rec).await
                                            {
                                                tx.stage_vector(table_token, rid, vec);
                                            }
                                        }
                                    }
                                }
                                Ok(None) => {
                                    // NotFound is the ONLY case treated as "this is an
                                    // insert, not an update" — the proven semantics at
                                    // this call site (see the fn's doc comment).
                                    for backend in &new_backends {
                                        let mut ops = backend
                                            .plan_insert_tx(rid, &new_rec, tx_id)
                                            .await
                                            .map_err(|e| {
                                                TxError::Storage(DbError::Internal(format!(
                                                    "rederive_index2_ops_post_stage: \
                                                 plan_insert_tx failed \
                                                 (table_token={table_token}, rid={rid:?}): {e}"
                                                )))
                                            })?;
                                        stamp_index2_ops_provenance(reg, backend, &mut ops).await;
                                        appended
                                            .extend(ops.into_iter().map(|op| (table_token, op)));
                                        // F-50 Step 2 (#870, Part B): re-derive staged
                                        // vector for a new vector backend (insert case).
                                        // Mirror stage_vectors from table_manager_tx_ops.
                                        if is_vector_backend(backend) {
                                            if let Some(vec) =
                                                backend.staged_vector(rid, &new_rec).await
                                            {
                                                tx.stage_vector(table_token, rid, vec);
                                            }
                                        }
                                    }
                                }
                                // F-73: a non-NotFound storage error MUST abort the tx —
                                // it used to be a silent best-effort skip, which let the
                                // commit succeed while the row's posting for the new
                                // index was permanently dropped.
                                Err(e) => return Err(TxError::Storage(e)),
                            }
                        }
                        KvOp::Remove(k) => {
                            let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_index2_ops_post_stage: malformed staged key \
                                 (table_token={table_token}, {} bytes) — expected a \
                                 16-byte RecordId",
                                    k.len()
                                )))
                            })?;
                            // Nothing committed to delete from the index on a NotFound
                            // read (the row was never materialized) — a non-NotFound
                            // error still aborts the tx (F-73).
                            if let Some(old_bytes) =
                                read_pre_tx_bytes(&data_store, table_token, rid, &k).await?
                            {
                                let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                                    TxError::Storage(DbError::Codec(format!(
                                        "rederive_index2_ops_post_stage: pre-tx record \
                                     decode failed (table_token={table_token}, \
                                     rid={rid:?}): {e}"
                                    )))
                                })?;
                                for backend in &new_backends {
                                    let mut ops = backend
                                        .plan_delete_tx(rid, &old_rec, tx_id)
                                        .await
                                        .map_err(|e| {
                                            TxError::Storage(DbError::Internal(format!(
                                                "rederive_index2_ops_post_stage: \
                                                 plan_delete_tx failed \
                                                 (table_token={table_token}, rid={rid:?}): {e}"
                                            )))
                                        })?;
                                    stamp_index2_ops_provenance(reg, backend, &mut ops).await;
                                    appended.extend(ops.into_iter().map(|op| (table_token, op)));
                                    // F-50 Step 2 (#870, Part B): re-derive staged
                                    // vector delete for a new vector backend.
                                    // Mirror stage_vector_delete.
                                    if is_vector_backend(backend)
                                        && backend.staged_vector(rid, &old_rec).await.is_some()
                                    {
                                        tx.stage_vector_delete(table_token, rid);
                                    }
                                }
                            }
                        }
                    }
                }
            } // end `if !new_backends.is_empty()`
              // Append the re-derived ops FIRST — mirrors the base_index/sorted
              // ordering: they're planned against LIVE backends (freshly read
              // from the registry above), so they carry the CURRENT
              // `instance_epoch` and pass the retract filter below unchanged.
            if !appended.is_empty() {
                tx.index_write_set.extend(appended);
            }

            // R0-B (#1008): retract stale staged index2 ops. Before this fix,
            // index2 had NO retraction at all — `rederive_index2_ops_post_stage`
            // only ever appended. A backend DROP'd (or replaced — ABA, a new
            // backend registered under the SAME name gets a NEW `id` and a
            // fresh `entry.gen`) between stage and commit left its stale
            // staged ops in `tx.index_write_set` forever, resurrecting
            // postings for a gone index2 backend at Phase 5c.
            // F-6 (2026-08-06): `d.name_interned` here is the SAME
            // construction-time `descriptor()` snapshot `stamp_index2_provenance`
            // (table_manager_tx_ops.rs) stamps staged ops with — deliberately,
            // not `reg`'s authoritative `BackendEntry.name_interned` (which
            // `rename_entry` DOES update). Matching two stale snapshots against
            // each other is self-consistent; matching one stale and one
            // authoritative would falsely retract every index2 op staged
            // before a RENAME. Do not "fix" this side alone.
            let live_index2: shamir_collections::TFxSet<(u64, u64)> = {
                let mut set = shamir_collections::TFxSet::default();
                for backend in reg.all_backends().await {
                    let d = backend.descriptor();
                    if let Some(epoch) = reg.instance_epoch_of(d.id).await {
                        set.insert((d.name_interned, epoch));
                    }
                }
                set
            };
            retract_stale_provenance_ops(
                tx,
                table_token,
                shamir_tx::IndexFamily::Index2,
                &live_index2,
            );
        }
    }

    // =========================================================================
    // F-50 Step 2 (#870, Part D): sorted-index re-derivation
    // =========================================================================
    //
    // Same root-cause class as index2 Part B (stale stage-time plan): a tx that
    // staged before a new sorted index was registered (via
    // `create_sorted_index`) carries zero sorted ops in tx.index_write_set for
    // the new index — a guaranteed miss mirroring #538 Part B. This block
    // re-derives sorted posting ops for tables whose sorted generation advanced.
    //
    // Re-planning against ALL current defs is safe because posting ops are
    // idempotent (`SetPosting` overwrites with the same key+value; `RemovePosting`
    // is a no-op on an already-absent key). The generation gate avoids this work
    // on the common path (no DDL → generation unchanged → per-record loop never
    // runs). Only paid in the rare DDL-concurrent case.
    if !tx.sorted_stage_gens.is_empty() {
        let sorted_gens: Vec<(u64, u64)> =
            tx.sorted_stage_gens.iter().map(|(t, g)| (*t, *g)).collect();
        for (table_token, stage_gen) in sorted_gens {
            let Some(tbl) = repo.table_by_token_if_live(table_token).await else {
                continue;
            };
            let sorted_mgr = tbl.sorted_indexes();
            // Generation gate: skip when no sorted def was registered/dropped
            // since stage. One atomic Acquire load.
            if sorted_mgr.generation() == stage_gen {
                continue;
            }
            // F-7 (2026-08-06): unlike the index2 half above (which uses
            // `Vec::new()` here so retraction below still runs even with no
            // staged rows), a missing `write_set` entry `continue`s and
            // skips retraction entirely for this table. Safe TODAY only
            // because `stage_mutation` always calls `ensure_table_staging`
            // first, so a table with a `sorted_stage_gens` entry always also
            // has a `write_set` entry — this branch is currently
            // unreachable. If that invariant ever changes, this needs the
            // same `Vec::new()` treatment as index2 to avoid silently
            // leaving stale sorted ops unretracted.
            let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
                Some(staging) => staging.snapshot_ops(),
                None => continue,
            };
            let data_store = tbl.data_store().clone();

            let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();
            for kvop in staged_ops {
                match kvop {
                    KvOp::Set(k, v) => {
                        // F-73: same invariant-violation treatment as the index2
                        // half above — a malformed staged key or undecodable
                        // staged record is corruption, not "skip this record".
                        let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                            TxError::Storage(DbError::Internal(format!(
                                "rederive sorted-index: malformed staged key \
                                 (table_token={table_token}, {} bytes) — expected a \
                                 16-byte RecordId",
                                k.len()
                            )))
                        })?;
                        let new_rec = InnerValue::from_bytes(&v).map_err(|e| {
                            TxError::Storage(DbError::Codec(format!(
                                "rederive sorted-index: staged record decode failed \
                                 (table_token={table_token}, rid={rid:?}): {e}"
                            )))
                        })?;
                        // Phase 5a has not run: the store still holds the PRE-tx
                        // value, so this one read distinguishes insert vs. update.
                        match read_pre_tx_bytes(&data_store, table_token, rid, &k).await {
                            Ok(Some(old_bytes)) => {
                                let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                                    TxError::Storage(DbError::Codec(format!(
                                        "rederive sorted-index: pre-tx record decode \
                                         failed (table_token={table_token}, rid={rid:?}): {e}"
                                    )))
                                })?;
                                let ops =
                                    sorted_mgr.plan_record_updated(&rid, &old_rec, &new_rec, 0)?;
                                appended.extend(ops.into_iter().map(|op| (table_token, op)));
                            }
                            Ok(None) => {
                                // NotFound is the ONLY case treated as "this is an
                                // insert, not an update" (same proven semantics as
                                // the index2 half above).
                                let ops = sorted_mgr.plan_record_created(&rid, &new_rec, 0)?;
                                appended.extend(ops.into_iter().map(|op| (table_token, op)));
                            }
                            // F-73: a non-NotFound storage error MUST abort the tx.
                            Err(e) => return Err(TxError::Storage(e)),
                        }
                    }
                    KvOp::Remove(k) => {
                        let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                            TxError::Storage(DbError::Internal(format!(
                                "rederive sorted-index: malformed staged key \
                                 (table_token={table_token}, {} bytes) — expected a \
                                 16-byte RecordId",
                                k.len()
                            )))
                        })?;
                        if let Some(old_bytes) =
                            read_pre_tx_bytes(&data_store, table_token, rid, &k).await?
                        {
                            let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                                TxError::Storage(DbError::Codec(format!(
                                    "rederive sorted-index: pre-tx record decode failed \
                                     (table_token={table_token}, rid={rid:?}): {e}"
                                )))
                            })?;
                            let ops = sorted_mgr.plan_record_deleted(&rid, &old_rec)?;
                            appended.extend(ops.into_iter().map(|op| (table_token, op)));
                        }
                    }
                }
            }
            // Append the re-derived ops FIRST — mirrors
            // `rederive_base_index_ops_post_stage`'s ordering: they're
            // planned against current live defs, so they carry the CURRENT
            // `instance_epoch` and pass the retract filter below unchanged.
            if !appended.is_empty() {
                tx.index_write_set.extend(appended);
            }

            // R0-B (#1008): retract stale staged sorted ops — the sorted
            // family previously had NO retraction at all (only base_index
            // did, via the now-replaced byte-length heuristic). Combined
            // with Part 1's `SortedIndexManager::rename_definition` now
            // bumping `generation` (so this gate fires after a rename too),
            // this closes both failure modes: `stage → DROP sorted index →
            // commit` (stale ops resurrecting postings for a gone index) and
            // `stage sorted op → RENAME → commit` (stale ops still targeting
            // the OLD name/epoch after the rename).
            let live_sorted: shamir_collections::TFxSet<(u64, u64)> = sorted_mgr
                .iter_indexes()
                .into_iter()
                .map(|def| (def.name_interned, def.instance_epoch))
                .collect();
            retract_stale_provenance_ops(
                tx,
                table_token,
                shamir_tx::IndexFamily::Sorted,
                &live_sorted,
            );
        }
    }

    Ok(())
}

/// F-50 Step 2 (#870): true when `backend`'s descriptor kind is Vector. Used
/// to gate the vector-staging branches in [`rederive_index2_ops_post_stage`]
/// (VectorBackend::plan_*_tx are no-ops for a tx; embeddings route through
/// `tx.staged_vectors` / `tx.staged_vector_deletes` instead).
fn is_vector_backend(backend: &Arc<dyn shamir_index::backend::IndexBackend>) -> bool {
    matches!(backend.descriptor().kind, IndexKind::Vector(_))
}

/// R0-B (#1008): overwrite the placeholder `Provenance`
/// `shamir_index::write_ops::index2_provenance` stamps on every op an
/// index2 `backend` plans, with the REAL live instance epoch
/// (`IndexRegistry::instance_epoch_of`) for that backend's id — mirrors
/// `TableManager::stamp_index2_provenance` (the tx-STAGE-time twin of this
/// COMMIT-time call site; see that method's doc for why a two-step stamp is
/// necessary for index2 specifically: the backend only ever sees its own
/// construction-time descriptor snapshot, never the registry's live `gen`).
async fn stamp_index2_ops_provenance(
    reg: &crate::index2::IndexRegistry,
    backend: &Arc<dyn shamir_index::backend::IndexBackend>,
    ops: &mut [IndexWriteOp],
) {
    if ops.is_empty() {
        return;
    }
    let d = backend.descriptor();
    if let Some(instance_epoch) = reg.instance_epoch_of(d.id).await {
        let provenance = shamir_tx::Provenance {
            family: shamir_tx::IndexFamily::Index2,
            name_interned: d.name_interned,
            instance_epoch,
        };
        for op in ops.iter_mut() {
            op.set_provenance(provenance);
        }
    }
}

/// R0-B (#1008): the ONE unifying retraction rule shared by all three
/// `rederive_*_ops_post_stage` functions below (base_index regular+unique,
/// sorted, index2). Replaces three divergent pre-R0-B heuristics — the
/// base_index family's byte-length/name `(is_unique, name_interned)` filter
/// (vulnerable to ABA: a DROP+CREATE of the same name under a DIFFERENT
/// definition still matched) and index2/sorted's COMPLETE ABSENCE of any
/// retraction at all — with a single correct check: a staged op for `family`
/// on `table_token` is retracted iff its `Provenance.(name_interned,
/// instance_epoch)` does NOT match any entry in `live`, the CURRENT set of
/// `(name_interned, instance_epoch)` pairs for every LIVE definition of that
/// family. A match means "the definition this op was planned against is
/// still the SAME instance" (survives); no match means the definition was
/// dropped, or replaced (DROP+CREATE-same-name — ABA, now correctly
/// distinguished because the new instance mints a FRESH epoch), or renamed
/// (the OLD name_interned no longer belongs to any live instance).
///
/// Ops for OTHER tables or OTHER families are left untouched (each call site
/// filters to its own table_token + family; a different family's ops simply
/// don't match `op.provenance().family` and pass through unaffected — this
/// makes it safe for all three call sites to run in sequence on the same
/// `tx.index_write_set` without needing to coordinate).
fn retract_stale_provenance_ops(
    tx: &mut TxContext,
    table_token: u64,
    family: shamir_tx::IndexFamily,
    live: &shamir_collections::TFxSet<(u64, u64)>,
) {
    tx.index_write_set.retain(|(tt, op)| {
        if *tt != table_token {
            return true; // Different table — not our concern.
        }
        let provenance = match op {
            IndexWriteOp::SetPosting { provenance, .. } => provenance,
            IndexWriteOp::RemovePosting { provenance, .. } => provenance,
            IndexWriteOp::BumpFtsStats { provenance, .. } => provenance,
        };
        if provenance.family != family {
            return true; // A different family's op — not our concern.
        }
        live.contains(&(provenance.name_interned, provenance.instance_epoch))
    });
}

/// P0-2 (#958): base_index `IndexManager` (regular + unique) ops-plan
/// re-derivation after stage. Mirrors [`rederive_index2_ops_post_stage`]'s
/// shape for the base_index index family: a tx that staged before a base_index
/// index (regular OR unique) was created carries zero ops for the new
/// index in `tx.index_write_set` — a permanently missing posting (regular)
/// or an unconstrained duplicate (unique, because no `UniqueGuard` was
/// recorded for a def that didn't exist at stage time).
///
/// At commit, if a table's base_index generation exceeds the captured value,
/// this function re-plans regular + unique posting ops against ALL current
/// defs (idempotent — `SetPosting` overwrites, `RemovePosting` is a no-op
/// on absent keys — so re-planning defs that already had ops at stage time
/// is harmless), AND records fresh `UniqueGuard`s for every current unique
/// def so Phase 2.6's authoritative re-validation under `commit_lock`
/// covers the new constraint.
///
/// Sub-bug 2c: ALSO retracts staged ops for base_index indexes that were
/// DROP'd between stage and commit. An op whose key starts with a base_index
/// `(is_unique, name_interned)` prefix that's no longer in the current
/// def set is removed from `tx.index_write_set` before commit — preventing
/// orphan postings for gone indexes. Non-base_index ops (index2/sorted, which
/// have different key formats/lengths) are left untouched.
///
/// The generation gate makes this zero-cost on the common path: a tx that
/// captured `base_index_stage_gens` (populated only by a base_index-index-bearing-
/// table staging) and sees an unchanged generation skips the per-record
/// re-derivation entirely.
async fn rederive_base_index_ops_post_stage(
    tx: &mut TxContext,
    repo: &RepoInstance,
) -> Result<(), TxError> {
    if tx.base_index_stage_gens.is_empty() {
        return Ok(());
    }

    let stage_gens: Vec<(u64, u64)> = tx
        .base_index_stage_gens
        .iter()
        .map(|(t, g)| (*t, *g))
        .collect();

    for (table_token, stage_gen) in stage_gens {
        let Some(tbl) = repo.table_by_token_if_live(table_token).await else {
            continue;
        };
        let mgr = tbl.index_manager_ref();
        // Generation gate: skip when no base_index def was registered/dropped
        // since stage. One atomic Acquire load.
        if mgr.generation() == stage_gen {
            continue;
        }

        // Collect staged ops for this table into an owned Vec (same pattern
        // as the index2/sorted halves — no borrow of tx held across the
        // per-record async planning below).
        //
        // F-7 (2026-08-06): unlike the index2 half above (which uses
        // `Vec::new()` here so retraction below still runs even with no
        // staged rows), a missing `write_set` entry `continue`s and skips
        // retraction entirely for this table — same asymmetry as sorted's
        // equivalent branch, safe today ONLY because `stage_mutation` always
        // calls `ensure_table_staging` first, making this branch currently
        // unreachable. See sorted's identical comment above for the
        // condition that would make this unsafe.
        let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
            Some(staging) => staging.snapshot_ops(),
            None => continue,
        };
        let data_store = tbl.data_store().clone();

        let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();

        for kvop in staged_ops {
            match kvop {
                KvOp::Set(k, v) => {
                    let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                        TxError::Storage(DbError::Internal(format!(
                            "rederive_base_index: malformed staged key \
                             (table_token={table_token}, {} bytes) — expected a \
                             16-byte RecordId",
                            k.len()
                        )))
                    })?;
                    let new_rec = InnerValue::from_bytes(&v).map_err(|e| {
                        TxError::Storage(DbError::Codec(format!(
                            "rederive_base_index: staged record decode failed \
                             (table_token={table_token}, rid={rid:?}): {e}"
                        )))
                    })?;
                    match read_pre_tx_bytes(&data_store, table_token, rid, &k).await {
                        Ok(Some(old_bytes)) => {
                            // Update case: old record exists in the store.
                            let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                                TxError::Storage(DbError::Codec(format!(
                                    "rederive_base_index: pre-tx record decode failed \
                                     (table_token={table_token}, rid={rid:?}): {e}"
                                )))
                            })?;
                            // Re-plan regular + unique ops against ALL current
                            // defs (idempotent for defs that already had ops).
                            let ops = mgr
                                .plan_record_updated(&rid, &old_rec, &new_rec)
                                .await
                                .map_err(|e| {
                                    TxError::Storage(DbError::Internal(format!(
                                        "rederive_base_index: plan_record_updated failed: {e}"
                                    )))
                                })?;
                            appended.extend(ops.into_iter().map(|op| (table_token, op)));
                            let unique_ops = mgr
                                .plan_record_updated_unique(&rid, &old_rec, &new_rec)
                                .await
                                .map_err(|e| {
                                    TxError::Storage(DbError::Internal(format!(
                                        "rederive_base_index: plan_record_updated_unique failed: {e}"
                                    )))
                                })?;
                            appended.extend(unique_ops.into_iter().map(|op| (table_token, op)));
                            // Record UniqueGuards for every current unique def
                            // so Phase 2.6's commit-time re-validation covers
                            // constraints that didn't exist at stage time.
                            for index_key in mgr.unique_keys_for(&new_rec) {
                                tx.record_unique_guard(UniqueGuard {
                                    table_token,
                                    index_key,
                                    owner: rid,
                                });
                            }
                        }
                        Ok(None) => {
                            // Insert case: no pre-tx record.
                            let ops =
                                mgr.plan_record_created(&rid, &new_rec).await.map_err(|e| {
                                    TxError::Storage(DbError::Internal(format!(
                                        "rederive_base_index: plan_record_created failed: {e}"
                                    )))
                                })?;
                            appended.extend(ops.into_iter().map(|op| (table_token, op)));
                            let unique_ops = mgr
                                .plan_record_created_unique(&rid, &new_rec)
                                .await
                                .map_err(|e| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_base_index: plan_record_created_unique failed: {e}"
                                )))
                            })?;
                            appended.extend(unique_ops.into_iter().map(|op| (table_token, op)));
                            for index_key in mgr.unique_keys_for(&new_rec) {
                                tx.record_unique_guard(UniqueGuard {
                                    table_token,
                                    index_key,
                                    owner: rid,
                                });
                            }
                        }
                        Err(e) => return Err(TxError::Storage(e)),
                    }
                }
                KvOp::Remove(k) => {
                    let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                        TxError::Storage(DbError::Internal(format!(
                            "rederive_base_index: malformed staged key \
                             (table_token={table_token}, {} bytes) — expected a \
                             16-byte RecordId",
                            k.len()
                        )))
                    })?;
                    if let Some(old_bytes) =
                        read_pre_tx_bytes(&data_store, table_token, rid, &k).await?
                    {
                        let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                            TxError::Storage(DbError::Codec(format!(
                                "rederive_base_index: pre-tx record decode failed \
                                 (table_token={table_token}, rid={rid:?}): {e}"
                            )))
                        })?;
                        let ops = mgr.plan_record_deleted(&rid, &old_rec).await.map_err(|e| {
                            TxError::Storage(DbError::Internal(format!(
                                "rederive_base_index: plan_record_deleted failed: {e}"
                            )))
                        })?;
                        appended.extend(ops.into_iter().map(|op| (table_token, op)));
                        let unique_ops = mgr
                            .plan_record_deleted_unique(&rid, &old_rec)
                            .await
                            .map_err(|e| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_base_index: plan_record_deleted_unique failed: {e}"
                                )))
                            })?;
                        appended.extend(unique_ops.into_iter().map(|op| (table_token, op)));
                    }
                }
            }
        }

        // Append the re-derived ops FIRST (so they're in the set during the
        // 2c retain below — since they're planned against current live defs,
        // they pass the filter and survive).
        if !appended.is_empty() {
            tx.index_write_set.extend(appended);
        }

        // ── Sub-bug 2c / R0-B (#1008): retract stale staged base_index ops ────
        //
        // An index that existed at stage time (contributing ops to
        // `index_write_set`) but was DROP'd before commit — or DROP'd and
        // then CREATE'd again under the SAME name with a DIFFERENT
        // definition (ABA) — leaves orphan/wrong-instance ops that would
        // resurrect a posting for a gone index, or contaminate a NEW index
        // with postings computed against the OLD definition's fields.
        //
        // R0-B replaces the previous byte-length/name `(is_unique,
        // name_interned)` heuristic — which could not distinguish an ABA
        // replacement from the SAME live instance — with the uniform
        // `(name_interned, instance_epoch)` provenance check (see
        // `retract_stale_provenance_ops`'s doc). Every current definition's
        // `instance_epoch` is a FRESH value on CREATE (and bumped on
        // RENAME), so a staged op from a definition that no longer exists —
        // or was replaced — never matches `live` and is retracted; a staged
        // op from a definition that is STILL the same live instance always
        // matches and survives.
        let live_regular: shamir_collections::TFxSet<(u64, u64)> = mgr
            .iter_indexes()
            .map(|def| (def.name_interned, def.instance_epoch))
            .collect();
        let live_unique: shamir_collections::TFxSet<(u64, u64)> = mgr
            .iter_unique_indexes()
            .map(|def| (def.name_interned, def.instance_epoch))
            .collect();
        retract_stale_provenance_ops(
            tx,
            table_token,
            shamir_tx::IndexFamily::Regular,
            &live_regular,
        );
        retract_stale_provenance_ops(
            tx,
            table_token,
            shamir_tx::IndexFamily::Unique,
            &live_unique,
        );
    }

    Ok(())
}

/// #1100 — Re-derive posting ops for records whose value changed concurrently
///
/// This function closes the gap where a DELETE or UPDATE transaction plans index
/// removal operations based on its own snapshot view of a record, rather than
/// the record's ACTUAL current durable value. If another concurrent transaction
/// updated the record's indexed fields after this transaction's snapshot was taken,
/// the planner computed the removal key from the OLD value (seen in the snapshot),
/// not the ACTUAL current value. This means the correct removal op was never
/// generated at all, leaving a dangling posting.
///
/// The approach:
/// 1. For each staged Remove operation (DELETE): re-plan removal ops based on the
///    CURRENT durable value, then append any ops that weren't already staged.
/// 2. For each staged Set operation (UPDATE): re-plan the FULL diff (Remove + Set)
///    based on the CURRENT durable value as the "old" half of the diff, then
///    append all resulting ops (deduplicating against staged base_index ops to
///    avoid duplicates, since base_index ops don't include instance_epoch).
///
/// This mirrors `rederive_base_index_ops_post_stage` but is triggered by VALUE changes
/// (detected by comparing planned ops), not DEFINITION changes (detected by generation).
async fn rederive_stale_value_ops_post_stage(
    tx: &mut TxContext,
    repo: &RepoInstance,
) -> Result<(), TxError> {
    // Skip if no base_index tables were touched (zero-cost on common path)
    if tx.base_index_stage_gens.is_empty() {
        return Ok(());
    }

    // #1110: Repo-global staleness gate — if nothing in the repo has even
    // STARTED a commit since this tx opened (version allocation high-water mark
    // not beyond snapshot_version), NO staged row's snapshot value can be stale.
    // One atomic Acquire load. Conservative: a commit on a DIFFERENT table still
    // defeats the fast path (false positive), but never wrong (no false negatives).
    // This is acceptable because the common case is a quiet repo, and a repo-global
    // signal is the only cheap signal available (per-table commit watermarks do not
    // exist).
    //
    // HAPPENS-BEFORE PROOF (why `version_allocation_high_water_mark` is sound):
    //
    // Let Tx1 be a reader with snapshot_version = V, and Tx2 be a concurrent writer.
    // Tx1 observes `gate.version_allocation_high_water_mark() <= V` and skips
    // re-derivation. We must prove that Tx1 cannot see stale values from Tx2.
    //
    // Tx2's program order (single thread, `commit_lock`-serialized against every
    // other writer, but that fact is not needed for this proof):
    //   1. `assign_next_version_guarded()` → `version_counter.fetch_add(1, AcqRel)`,
    //      allocating version V2. AcqRel (NOT Relaxed — see `assign_next_version`'s
    //      doc; a Relaxed RMW never synchronizes-with anything) means this store
    //      has Release semantics.
    //   2. Phase 5a/5c: Tx2 writes data + postings to the MVCC store at V2.
    //
    // Tx1's gate check does `version_counter.load(Acquire)`. By the C++/Rust
    // memory model, an `Acquire` load that reads the value written by a
    // `Release` (or `AcqRel`) RMW SYNCHRONIZES-WITH that RMW — this is the one
    // and only mechanism that establishes happens-before across threads here
    // (program order alone only orders operations on the SAME thread).
    //
    // Case A: Tx1's load returns a value `<= V` (the gate's fast-path condition).
    // Since `fetch_add` is monotonically increasing and Tx2's fetch_add produced
    // `V2 = V_prev + 1`, Tx1's load returning `<= V` means it did NOT read the
    // value Tx2's fetch_add produced (or any later one) — so Tx1's load and
    // Tx2's fetch_add are NOT related by synchronizes-with in this execution,
    // and per the Rust memory model Tx1 is NOT guaranteed to observe ANY of
    // Tx2's subsequent writes (step 2) either — the model provides no ordering
    // guarantee for operations with no synchronizes-with edge between them.
    // Read the contrapositive: for Tx1 to legitimately observe Tx2's step-2
    // writes without a synchronizes-with edge would itself be undefined
    // behavior this fix does not need to reason about further — the design
    // relies on `read_one_tx_bytes`'s own MVCC read path (a SEPARATE,
    // independently-synchronized channel, not this gate) never doing that.
    // The gate's job is narrower: prove that WHEN it says "skip", the skip is
    // safe — and Case A shows the gate's load did not synchronize-with any
    // writer whose version exceeds V, which is exactly the condition under
    // which no staged row could have been staled by that writer.
    //
    // Therefore: `version_allocation_high_water_mark() <= V` ⇒ no concurrent
    // writer's `fetch_add` synchronizes-with this load ⇒ (by the AcqRel/Acquire
    // pairing being the sole cross-thread ordering channel between the gate and
    // `assign_next_version`) skipping re-derivation is sound. □
    //
    // Contrast with the BUGGY `last_committed()`-based gate (#1107, round 1):
    // `last_committed` only advances at Phase 6 (after 5a/5c writes), so the
    // false-negative window was the entire Phase 5a/5c→Phase 6 span — a writer
    // could have ALREADY landed MVCC-visible data while `last_committed` still
    // read as unchanged. Gating on the allocation high-water mark instead moves
    // the checkpoint to BEFORE any writes happen, closing that window. (Round 1
    // of THIS task's own delivery repeated the same class of error one level
    // down: it kept the writer's `fetch_add` at `Relaxed` while claiming the
    // `Acquire` reader synchronized with it — a `Relaxed` store cannot
    // synchronize-with anything, so that claim was false. Both the writer
    // (`AcqRel`) and reader (`Acquire`) sides must use a genuine Release/Acquire
    // pairing for this proof to hold — see `assign_next_version`'s doc.)
    let gate = repo.tx_gate().await?;
    if gate.version_allocation_high_water_mark() <= tx.snapshot_version {
        log::debug!(
            "rederive_stale_value_ops_post_stage: fast-path skip (repo quiet, snapshot_version={}, alloc_hwm={})",
            tx.snapshot_version,
            gate.version_allocation_high_water_mark()
        );
        return Ok(());
    }

    let stage_gens: Vec<(u64, u64)> = tx
        .base_index_stage_gens
        .iter()
        .map(|(t, g)| (*t, *g))
        .collect();

    for (table_token, _stage_gen) in stage_gens {
        let Some(tbl) = repo.table_by_token_if_live(table_token).await else {
            continue;
        };
        let mgr = tbl.index_manager_ref();

        // Collect staged ops for this table
        let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
            Some(staging) => staging.snapshot_ops(),
            None => continue,
        };

        let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();

        for kvop in staged_ops {
            match kvop {
                KvOp::Remove(k) => {
                    // DELETE case: re-plan removal ops based on CURRENT durable value
                    let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                        TxError::Storage(DbError::Internal(format!(
                            "rederive_stale_value_ops: malformed staged key \
                             (table_token={table_token}, {} bytes) — expected a \
                             16-byte RecordId",
                            k.len()
                        )))
                    })?;

                    // Build a map from rid to set of posting_keys for already-staged RemovePosting ops
                    // for this table, so we can detect which ones are missing
                    let mut staged_removals_by_rid: TFxMap<RecordId, TFxSet<bytes::Bytes>> =
                        TFxMap::default();
                    for (_, op) in tx.index_write_set.iter().filter(|(t, _)| *t == table_token) {
                        if let IndexWriteOp::RemovePosting {
                            key,
                            owner: Some(owner_bytes),
                            ..
                        } = op
                        {
                            if let Some(rid) = RecordId::try_from_bytes(owner_bytes) {
                                staged_removals_by_rid
                                    .entry(rid)
                                    .or_default()
                                    .insert(key.clone());
                            }
                        }
                    }

                    // Read the CURRENT MVCC-visible value (bypasses tx snapshot)
                    if let Some(current_bytes) = tbl.read_one_tx_bytes(rid, None).await? {
                        log::debug!(
                            "rederive_stale_value_ops: DELETE rid={:?}, current_bytes.len()={}",
                            rid,
                            current_bytes.len()
                        );

                        // Decode the current durable value
                        let current_rec = InnerValue::from_bytes(&current_bytes).map_err(|e| {
                            TxError::Storage(DbError::Codec(format!(
                                "rederive_stale_value_ops: current record decode failed \
                                 (table_token={table_token}, rid={rid:?}): {e}"
                            )))
                        })?;

                        // Re-plan removal ops for the CURRENT value
                        let current_removals = mgr
                            .plan_record_deleted(&rid, &current_rec)
                            .await
                            .map_err(|e| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_stale_value_ops: plan_record_deleted failed: {e}"
                                )))
                            })?;

                        let current_unique_removals = mgr
                            .plan_record_deleted_unique(&rid, &current_rec)
                            .await
                            .map_err(|e| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_stale_value_ops: plan_record_deleted_unique failed: {e}"
                                )))
                            })?;

                        log::debug!(
                            "rederive_stale_value_ops: DELETE rid={:?}, current_removals.len()={}, current_unique_removals.len()={}",
                            rid,
                            current_removals.len(),
                            current_unique_removals.len()
                        );

                        // Append any ops that weren't already staged
                        for op in current_removals
                            .into_iter()
                            .chain(current_unique_removals.into_iter())
                        {
                            // Bug B fix: handle both owner:Some (unique) and owner:None (regular)
                            let is_staged = match &op {
                                IndexWriteOp::RemovePosting {
                                    ref key,
                                    owner,
                                    provenance,
                                } => {
                                    if provenance.family == IndexFamily::Regular {
                                        // For regular indexes: posting_key = index_key || record_id
                                        // The posting key itself uniquely identifies this record's claim
                                        tx.index_write_set
                                            .iter()
                                            .filter(|(t, _)| *t == table_token)
                                            .any(|(_, staged_op)| {
                                                matches!(
                                                    staged_op,
                                                    IndexWriteOp::RemovePosting {
                                                        key: staged_key,
                                                        provenance: staged_prov,
                                                        ..
                                                    } if staged_key == key && staged_prov.family == IndexFamily::Regular
                                                )
                                            })
                                    } else {
                                        // For unique indexes: dedup by (key, owner)
                                        if let Some(ref owner_bytes) = owner {
                                            if let Some(rid) =
                                                RecordId::try_from_bytes(owner_bytes.as_slice())
                                            {
                                                staged_removals_by_rid
                                                    .get(&rid)
                                                    .map(|keys| keys.contains(key))
                                                    .unwrap_or(false)
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        }
                                    }
                                }
                                _ => continue,
                            };

                            if !is_staged {
                                log::debug!(
                                    "rederive_stale_value_ops: appending missing RemovePosting \
                                     for DELETE (table_token={}, op={:?})",
                                    table_token,
                                    op
                                );
                                appended.push((table_token, op));
                            } else {
                                log::debug!(
                                    "rederive_stale_value_ops: RemovePosting already staged \
                                     for DELETE (table_token={}, op={:?})",
                                    table_token,
                                    op
                                );
                            }
                        }
                    }
                    // If current value is None, the record was deleted concurrently —
                    // nothing to re-derive for this DELETE (it's already gone).
                }
                KvOp::Set(k, v) => {
                    // UPDATE case: re-plan removal ops based on CURRENT durable value
                    let rid = RecordId::try_from_bytes(&k).ok_or_else(|| {
                        TxError::Storage(DbError::Internal(format!(
                            "rederive_stale_value_ops: malformed staged key \
                             (table_token={table_token}, {} bytes) — expected a \
                             16-byte RecordId",
                            k.len()
                        )))
                    })?;

                    // Read the CURRENT MVCC-visible value (the ACTUAL old value)
                    if let Some(old_bytes) = tbl.read_one_tx_bytes(rid, None).await? {
                        // Decode the current durable value
                        let old_rec = InnerValue::from_bytes(&old_bytes).map_err(|e| {
                            TxError::Storage(DbError::Codec(format!(
                                "rederive_stale_value_ops: old record decode failed \
                                 (table_token={table_token}, rid={rid:?}): {e}"
                            )))
                        })?;

                        // Decode the new value (what the tx is setting)
                        let new_rec = InnerValue::from_bytes(&v).map_err(|e| {
                            TxError::Storage(DbError::Codec(format!(
                                "rederive_stale_value_ops: new record decode failed \
                                 (table_token={table_token}, rid={rid:?}): {e}"
                            )))
                        })?;

                        // Re-plan update ops using the CURRENT old value
                        // This generates both RemovePosting (for fields that changed)
                        // and SetPosting (for the new values), which is correct even when
                        // the record's actual current value differs from what the tx's snapshot saw.
                        let updated_ops = mgr
                            .plan_record_updated(&rid, &old_rec, &new_rec)
                            .await
                            .map_err(|e| {
                            TxError::Storage(DbError::Internal(format!(
                                "rederive_stale_value_ops: plan_record_updated failed: {e}"
                            )))
                        })?;

                        let updated_unique_ops = mgr
                            .plan_record_updated_unique(&rid, &old_rec, &new_rec)
                            .await
                            .map_err(|e| {
                                TxError::Storage(DbError::Internal(format!(
                                    "rederive_stale_value_ops: plan_record_updated_unique failed: {e}"
                                )))
                            })?;

                        // Append ALL re-planned ops (both Remove and Set)
                        // This correctly handles the case where concurrent changes
                        // mean the staged ops are based on stale data.
                        log::debug!(
                            "rederive_stale_value_ops: UPDATE rid={:?}, appending {} regular + {} unique ops based on CURRENT value",
                            rid,
                            updated_ops.len(),
                            updated_unique_ops.len()
                        );
                        for op in updated_ops
                            .into_iter()
                            .chain(updated_unique_ops.into_iter())
                        {
                            // Bug A fix: make dedup owner-aware and kind-aware for unique indexes
                            let is_staged = tx
                                .index_write_set
                                .iter()
                                .filter(|(t, _)| *t == table_token)
                                .any(|(_, staged_op)| {
                                    let family = match staged_op {
                                        IndexWriteOp::SetPosting { provenance, .. }
                                        | IndexWriteOp::RemovePosting { provenance, .. }
                                        | IndexWriteOp::BumpFtsStats { provenance, .. } => {
                                            provenance.family
                                        }
                                    };

                                    if family == IndexFamily::Regular {
                                        // For regular indexes: dedup by posting key (already contains record_id)
                                        match (&op, staged_op) {
                                            (
                                                IndexWriteOp::RemovePosting {
                                                    key,
                                                    provenance: prov,
                                                    ..
                                                },
                                                IndexWriteOp::RemovePosting {
                                                    key: staged_key,
                                                    provenance: staged_prov,
                                                    ..
                                                },
                                            )
                                            | (
                                                IndexWriteOp::SetPosting {
                                                    key,
                                                    provenance: prov,
                                                    ..
                                                },
                                                IndexWriteOp::SetPosting {
                                                    key: staged_key,
                                                    provenance: staged_prov,
                                                    ..
                                                },
                                            ) => {
                                                staged_key == key
                                                    && staged_prov.family == IndexFamily::Regular
                                                    && prov.family == IndexFamily::Regular
                                            }
                                            _ => false,
                                        }
                                    } else if family == IndexFamily::Unique {
                                        // For unique indexes: dedup by (key, owner) for RemovePosting,
                                        // and by (key, value) for SetPosting (value is record_id)
                                        match (&op, staged_op) {
                                            (
                                                IndexWriteOp::RemovePosting {
                                                    key,
                                                    owner: Some(owner_bytes),
                                                    provenance: prov,
                                                    ..
                                                },
                                                IndexWriteOp::RemovePosting {
                                                    key: staged_key,
                                                    owner: Some(staged_owner_bytes),
                                                    provenance: staged_prov,
                                                    ..
                                                },
                                            ) => {
                                                staged_key == key
                                                    && staged_owner_bytes == owner_bytes
                                                    && staged_prov.family == IndexFamily::Unique
                                                    && prov.family == IndexFamily::Unique
                                            }
                                            (
                                                IndexWriteOp::SetPosting {
                                                    key,
                                                    value,
                                                    provenance: prov,
                                                },
                                                IndexWriteOp::SetPosting {
                                                    key: staged_key,
                                                    value: staged_value,
                                                    provenance: staged_prov,
                                                },
                                            ) => {
                                                staged_key == key
                                                    && staged_value == value
                                                    && staged_prov.family == IndexFamily::Unique
                                                    && prov.family == IndexFamily::Unique
                                            }
                                            _ => false,
                                        }
                                    } else {
                                        false
                                    }
                                });

                            if !is_staged {
                                log::debug!(
                                    "rederive_stale_value_ops: appending op for UPDATE (table_token={}, rid={:?}, op={:?})",
                                    table_token,
                                    rid,
                                    op
                                );
                                appended.push((table_token, op));
                            } else {
                                log::debug!(
                                    "rederive_stale_value_ops: op already staged for UPDATE (table_token={}, rid={:?}, op={:?})",
                                    table_token,
                                    rid,
                                    op
                                );
                            }
                        }
                    }
                    // If old value is None, this is actually an insert — no removal ops needed
                }
            }
        }

        // Append the re-derived ops
        if !appended.is_empty() {
            tx.index_write_set.extend(appended);
        }
    }

    Ok(())
}

/// Outcome of [`pre_commit_locked_validate`]: the assigned commit version,
/// built WAL entry, and uwl_guards — ready for WAL begin (Phase 4).
pub(super) struct ValidatedPreCommit {
    pub(super) commit_version: u64,
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    /// F-48b (#867): kept-alive writer-drain guards (one per fast-path table
    /// in `tx.write_set` whose `needs_write_barrier()` read `false` in Phase
    /// 2.5). Threaded alongside `uwl_guards` through Phase 5c, dropped inside
    /// [`materialize`](super::materialize::materialize) after the data/index
    /// writes have landed.
    pub(super) drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
    /// RAII owner of the version's terminal-mark obligation. The caller
    /// (`commit_tx_lockfree`) either calls `materialize` (which consumes it
    /// via `guard.commit()` → Materialized) on the success path, or drops it
    /// (→ Aborted) on WAL-begin failure. This makes the assign→mark window
    /// statically leak-proof.
    pub(super) version_guard: shamir_tx::VersionGuard,
    /// SSI fix S2 — RAII owners of this committer's pre-WAL cell-reservations
    /// (one per touched table). The caller drops these (→ release) if WAL begin
    /// fails, or holds them through `materialize` and `disarm`s them once the
    /// publisher has finalized every claim. Empty off Serializable.
    pub(super) cell_guards: Vec<CellReservationGuard>,
    /// Op #2 Stage 2: the WAL entry wrapped in `Arc` for drainer window offer.
    /// The caller serializes it into the WAL via `begin_grouped(&arc, ..)`
    /// and offers it to the drainer — both read from this Arc, no clone.
    pub(super) wal_entry_arc: Arc<WalEntryV2>,
}

/// Locked validation phase: Phases 2 + 2-bis + C6 + 3 (assign) + WAL entry build.
///
/// Does NOT write the WAL entry (no fsync). The caller is responsible for
/// calling `wal.begin_grouped(entry, ..)` or batching via
/// `wal.begin_grouped_many`. This split
/// enables group-commit fsync amortization.
///
/// Phase 3 (assign_next_version) is DEFERRED until after validation and
/// the empty-tx check (P0c): SSI/phantom/empty-tx aborts return before any
/// version is allocated, so no version slot is wasted on aborted txs.
///
/// Returns `Some(ValidatedPreCommit)` when the tx has durable work,
/// `None` for C6 empty-tx fast-path, or `Err` on validation failure.
pub(super) async fn pre_commit_locked_validate(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
) -> Result<Option<ValidatedPreCommit>, TxError> {
    // Phase 2 (SSI only): read-set validation.
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
    // Inverted single-pass scan: walk the commit window ONCE and test ALL
    // predicate deps against each record, short-circuiting on the FIRST
    // conflict (O(W) window walks, not O(P×W); one shared EBR guard, not
    // P). The conflict set is identical to the per-dep loop — a conflict
    // exists iff some dep conflicts with some record, which is
    // order-independent.
    //
    // F-40b (RI barrier): widened to ALSO fire when `ri_barrier_tokens` is
    // non-empty (recorded by FK reverse-check scans regardless of
    // isolation), so an EXPLICIT `Snapshot`-isolation parent delete/update
    // — which never populates `predicate_set` — still gets a commit-time
    // re-check. The barrier tokens are appended as `TableScan { table_token }`
    // deps, reusing the SAME `predicate_conflicts_batch` machinery verbatim.
    let has_serializable_preds =
        tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty();
    if has_serializable_preds || !tx.ri_barrier_tokens_is_empty() {
        let mut deps = if has_serializable_preds {
            tx.predicate_set.snapshot_deps()
        } else {
            Vec::new()
        };
        tx.append_ri_barrier_deps(&mut deps);
        if let Some(idx) = gate.predicate_conflicts_batch(&deps, tx.snapshot_version) {
            let dep = format!("{:?}", deps[idx]);
            repo.tx_metrics().on_tx_aborted_phantom();
            return Err(TxError::PhantomConflict { dep });
        }
    }

    // Phase CAS (FG-7): expected_version validation, independent of
    // isolation level. Runs for EVERY tx (Snapshot and Serializable alike)
    // whenever cas_set is non-empty — zero cost otherwise (empty-map check).
    // Placed after Phase 2/2-bis so a Serializable tx that also used CAS
    // still surfaces the pre-existing SsiConflict/PhantomConflict class
    // first (regression test `concurrent_cas_exactly_one_wins` depends on
    // this ordering); Phase CAS is the backstop for Snapshot/no-SSI paths
    // where Phase 2/2-bis are no-ops.
    if !tx.cas_set.is_empty() {
        // Defensive: every `begin_tx` call site now attaches a version
        // provider unconditionally (FG-7 step 3), so `None` here should be
        // unreachable in production. Fail safe (treat as a conflict, same
        // as `validate_read_set`'s `None => conflict` convention) rather
        // than silently skipping a CAS check a caller depends on.
        debug_assert!(
            tx.version_provider.is_some(),
            "cas_set non-empty but no version_provider attached"
        );
        let provider = tx.version_provider.as_ref();
        // Mirror `validate_read_set`'s iteration idiom: `iter_sync` is a
        // synchronous visitor that cannot early-return, so capture the
        // first conflict and report it after the scan.
        let mut conflict: Option<(bytes::Bytes, u64, u64)> = None;
        tx.cas_set.iter_sync(|(table_id, key), expected| {
            if conflict.is_some() {
                return false;
            }
            match provider.and_then(|p| p.version_of(*table_id, key)) {
                None => conflict = Some((key.clone(), *expected, 0)),
                Some(found) if found != *expected => {
                    conflict = Some((key.clone(), *expected, found));
                }
                Some(_) => {}
            }
            true
        });
        if let Some((key, expected, found)) = conflict {
            repo.tx_metrics().on_tx_aborted_ssi();
            return Err(TxError::CasConflict {
                key,
                expected,
                found,
            });
        }
    }

    // C6: empty-tx fast-path. No version has been allocated yet (P0c),
    // so nothing to mark — just return.
    if tx.is_empty() {
        return Ok(None);
    }

    // SSI fix S2 — CLAIM the write-set (Serializable only), AFTER read-validate
    // and BEFORE version assign + Phase 4 WAL. A loser aborts here with
    // `SsiConflict`, never touching the WAL (I-PreWAL). `claim_write_set` builds
    // the guards holding the won keys; on this `?`-return their drop releases
    // any partial claim.
    let cell_guards = claim_write_set(tx, repo).await?;

    // Crash seam (test-only).
    maybe_crash("pre_commit", repo).await;

    // Phase 3 (P0c): assign new version AFTER validation, wrapped in a
    // RAII VersionGuard. Deferred to this point so SSI/phantom/empty-tx
    // aborts never allocate a version slot. Pure atomic fetch_add —
    // lock-free, safe without commit_mutex.
    let version_guard = gate.assign_next_version_guarded();
    let commit_version = version_guard.version();

    // Build WAL entry (Phase 4 prep) — does NOT persist.
    let wal_ops = wal_ops_from_tx(tx).await;
    // Stage I: the interner is per-REPO. `interner_deltas` is a single flat
    // `Vec<(name, id)>` (no per-table key), so we emit every entry under the
    // REPO scope marker (constant 0). The WAL wire-shape
    // `Vec<(u64, String, u64)>` is UNCHANGED — only the meaning of the first
    // `u64` shifts from `table_token` to a repo-scope constant. Recovery
    // resolves the single repo interner directly (keystone).
    let interner_delta: Vec<(u64, String, u64)> = tx
        .interner_deltas
        .iter()
        .map(|(name, id)| (REPO_INTERNER_SCOPE, name.clone(), *id))
        .collect();
    let mut wal_entry = shamir_wal::WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops)
        .with_commit_version(commit_version);
    wal_entry.interner_delta = interner_delta;
    // Op #2 Stage 2: wrap in Arc for drainer window offer. `begin_grouped`
    // borrows, so no clone is needed for WAL persistence — both the WAL
    // serialize and the drainer offer read from this Arc.
    let wal_entry_arc = Arc::new(wal_entry);

    Ok(Some(ValidatedPreCommit {
        commit_version,
        uwl_guards,
        drain_guards,
        version_guard,
        cell_guards,
        wal_entry_arc,
    }))
}

/// Locked phase of the commit pipeline: runs UNDER `commit_lock`.
///
/// Performs:
/// - Phase 2: SSI read-set validation (must be atomic with Phase 6
///   record_commit_writes — both under lock).
/// - Phase 2-bis: phantom predicate validation.
/// - C6: empty-tx fast-path check.
/// - Phase 3: assign_next_version (P0c: deferred AFTER validation).
/// - Phase 4: WAL begin (the commit point).
///
/// Returns `Some(PreCommit)` on successful Phase 4, `None` for the C6
/// empty-tx fast-path, or `Err` on SSI/phantom/unique conflict or
/// storage failure.
pub(super) async fn pre_commit_locked(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    drain_guards: Vec<crate::table::writer_drain_barrier::WriterDrainGuard>,
) -> Result<Option<PreCommit>, TxError> {
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
    // Inverted single-pass scan — see the matching block in
    // `pre_commit_locked_validate` for the full rationale. Both call
    // sites share `RepoTxGate::predicate_conflicts_batch`.
    //
    // F-40b (RI barrier): widened to ALSO fire when `ri_barrier_tokens` is
    // non-empty (recorded by FK reverse-check scans regardless of
    // isolation), so an EXPLICIT `Snapshot`-isolation parent delete/update
    // routed through this AsyncIndex-visibility path — which never
    // populates `predicate_set` — still gets a commit-time re-check. This
    // path already always runs under `commit_lock` (taken unconditionally
    // by its caller), so no additional lock-widening is needed here
    // (unlike the lock-free path's commit-lock acquisition). The barrier
    // tokens are appended as `TableScan { table_token }` deps, reusing
    // the SAME `predicate_conflicts_batch` machinery verbatim — identical
    // to the `pre_commit_locked_validate` widening above.
    let has_serializable_preds =
        tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty();
    if has_serializable_preds || !tx.ri_barrier_tokens_is_empty() {
        let mut deps = if has_serializable_preds {
            tx.predicate_set.snapshot_deps()
        } else {
            Vec::new()
        };
        tx.append_ri_barrier_deps(&mut deps);
        if let Some(idx) = gate.predicate_conflicts_batch(&deps, tx.snapshot_version) {
            let dep = format!("{:?}", deps[idx]);
            repo.tx_metrics().on_tx_aborted_phantom();
            return Err(TxError::PhantomConflict { dep });
        }
    }

    // Phase CAS (FG-7): expected_version validation, independent of
    // isolation level. See the matching block in `pre_commit_locked_validate`
    // for the full rationale — both call sites mirror each other exactly.
    // This path (`pre_commit_locked`, the AsyncIndex commit visibility
    // route) already always runs under `commit_lock` (taken unconditionally
    // by its caller), so the validate→publish atomicity CRIT-4 provides for
    // `commit_tx_lockfree` is a non-issue here — but the CAS check itself
    // still must run so an AsyncIndex-visibility tx gets the same
    // "exactly one wins" guarantee as every other commit path.
    if !tx.cas_set.is_empty() {
        debug_assert!(
            tx.version_provider.is_some(),
            "cas_set non-empty but no version_provider attached"
        );
        let provider = tx.version_provider.as_ref();
        let mut conflict: Option<(bytes::Bytes, u64, u64)> = None;
        tx.cas_set.iter_sync(|(table_id, key), expected| {
            if conflict.is_some() {
                return false;
            }
            match provider.and_then(|p| p.version_of(*table_id, key)) {
                None => conflict = Some((key.clone(), *expected, 0)),
                Some(found) if found != *expected => {
                    conflict = Some((key.clone(), *expected, found));
                }
                Some(_) => {}
            }
            true
        });
        if let Some((key, expected, found)) = conflict {
            repo.tx_metrics().on_tx_aborted_ssi();
            return Err(TxError::CasConflict {
                key,
                expected,
                found,
            });
        }
    }

    // === C6: empty-tx fast-path ===
    //
    // No version has been allocated yet (P0c), so nothing to mark — just
    // return.
    //
    // ORDERING IS LOAD-BEARING: this sits AFTER the Phase 2 SSI block above.
    // A read-only Serializable tx still records reads, and its read_set must
    // still be validated against current committed versions — a read-only
    // tx that observed stale data MUST abort (returned as `Err` above), not
    // silently fast-path to success.
    if tx.is_empty() {
        return Ok(None);
    }

    // SSI fix S2 — CLAIM the write-set (Serializable only), AFTER read-validate
    // and BEFORE version assign + Phase 4 WAL. A loser aborts here with
    // `SsiConflict`, never touching the WAL (I-PreWAL). On this `?`-return the
    // partial guards drop → release.
    let cell_guards = claim_write_set(tx, repo).await?;

    // Crash seam (test-only): a HARD crash here is BEFORE the commit
    // point — staging is dropped, no WAL entry exists, locks release by
    // RAII. Recovery must find nothing → clean abort.
    maybe_crash("pre_commit", repo).await;

    // Phase 3 (P0c): assign new version AFTER validation, wrapped in a
    // RAII VersionGuard. Deferred to this point so SSI/phantom/empty-tx
    // aborts never allocate a version slot. Pure atomic fetch_add —
    // lock-free, safe without commit_mutex.
    let version_guard = gate.assign_next_version_guarded();
    let commit_version = version_guard.version();

    // Phase 4: write WAL entry — THE COMMIT POINT.
    //
    // A successful `wal.begin` makes the entry durable (lands in the OS
    // page cache at minimum — level 2; level 3 only after a later
    // `sync`); from here the tx is committed and `materialize` may not
    // abort. A *failed* `wal.begin` returns Err and is treated as a
    // pre-commit failure: the segment is poisoned and the leader rotates
    // to a fresh segment. In the COMMON case nothing durable remains —
    // `WalSegment::append_batch` rolls the file back to the last good
    // frame boundary on a `write_all` failure, so no torn frame survives
    // in the file. The rare exception (audit durability §1.6, NOT yet
    // fixed in this codebase) is when the rollback `set_len` ITSELF
    // fails: a partial frame may survive in the poisoned file. That
    // frame is discarded by `repair_torn_tail` on the next open (and by
    // replay's CRC check even if not repaired), so it cannot corrupt
    // recovery — but until §1.6 is fixed, the simple "nothing durable"
    // claim does not hold in that narrow window.
    //
    // HIGH-5: stamp the assigned `commit_version` onto the entry BEFORE
    // persisting it. Recovery sorts inflight entries by `commit_version`
    // so multi-tx replay matches the original commit pipeline's order;
    // `txn_id` (the `WalActiveKey` byte order) is not a safe proxy because
    // tx allocation and commit ordering are independent.
    let wal_ops = wal_ops_from_tx(tx).await;
    // Stage I: flatten the per-repo interner delta into the WAL entry. See
    // the matching note in `pre_commit_prelock`: the first `u64` is a
    // repo-scope constant (0), NOT a table token. Wire-shape unchanged.
    let interner_delta: Vec<(u64, String, u64)> = tx
        .interner_deltas
        .iter()
        .map(|(name, id)| (REPO_INTERNER_SCOPE, name.clone(), *id))
        .collect();
    let mut entry =
        WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops).with_commit_version(commit_version);
    entry.interner_delta = interner_delta;
    // Op #2 Stage 2: wrap the entry in Arc BEFORE persisting so the same
    // logical entry is shared between the WAL and the drainer window.
    // `begin_grouped` borrows the entry, so we serialize from the Arc borrow
    // — no clone on the commit hot path.
    let entry_arc = Arc::new(entry);
    if let Err(e) = wal
        .begin_grouped(&entry_arc, shamir_wal::WalDurability::Buffered)
        .await
    {
        // version_guard drops here → mark(Aborted): WAL begin failed.
        // See the Phase 4 note above for the precise durability state
        // after a failed `wal.begin` (nothing durable in the common case;
        // a partial frame may survive the rare rollback-failure window,
        // discarded by `repair_torn_tail` / replay CRC). This is a
        // pre-commit abort. SSI fix S2: drop the cell_guards too →
        // release every claimed cell (the publish that would have
        // finalized them never runs).
        drop(version_guard);
        drop(cell_guards);
        return Err(TxError::Storage(e));
    }
    // SSI fix S2: WAL begin succeeded — the tx is COMMITTED. The claims stay
    // armed and are handed to the caller via `PreCommit`; the caller `disarm`s
    // them once the publisher has finalized every claim (`finalize_reservation`
    // clears `reserved_by`).

    // Crash seam (test-only): a HARD crash here is AT the commit point —
    // the WAL entry is durable but no projection (5a..6.5) ran and Phase
    // 7 cleanup never happens. Recovery must find the inflight entry and
    // materialize the whole tx (data + index). All-or-nothing.
    maybe_crash("phase4", repo).await;

    Ok(Some(PreCommit {
        commit_version,
        uwl_guards,
        drain_guards,
        version_guard,
        cell_guards,
        wal_entry_arc: entry_arc,
    }))
}
