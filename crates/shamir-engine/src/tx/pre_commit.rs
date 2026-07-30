use std::sync::Arc;

use shamir_storage::error::DbError;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_tx::{
    CellReservationGuard, IndexWriteOp, IsolationLevel, RepoTxGate, RepoWalManager, TxContext,
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
    // timestamp. Task #897 (independent code review) suspects a lock-order
    // inversion between this fn (drain-guard-then-lock, see the Phase 2.5
    // loop below) and every DDL path (lock-then-drain, e.g.
    // `create_index_v2` in `table_manager_index_mgmt.rs`). If a hang
    // reproduces on CI, this pairs with the "pre_commit_prelock: exit" log
    // below (or its absence) to show whether the stall is INSIDE this fn.
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
    // `tx.unique_guards` (tables with a legacy UNIQUE index) — an
    // index2-only table (fts/functional/vector, no legacy unique index)
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
    // `index2_create_barrier` or register a legacy unique index without
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
            // F-68 (#895) cluster D / task #124 — this is the OTHER half of
            // the suspected lock-order inversion (task #897): this committer
            // is still holding this table's `drain_guards` (Phase 2.5's
            // fast-path guards, kept alive above for whichever OTHER tables
            // read `needs_write_barrier() == false`) while acquiring THIS
            // table's `unique_write_lock` here. A DDL path
            // (`create_index_v2` / `SchemaActivationBarrierGuard::raise`)
            // takes the OPPOSITE order (lock, then drain) — if both sides
            // are mid-sequence on overlapping tables, this `.lock_owned()`
            // and the DDL's `drain_writers().await` can each wait on the
            // other forever. Timestamped so a stuck run shows exactly which
            // table_token this committer was blocked on acquiring, and for
            // how long.
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
    // The unique key is deterministic in the value, so a byte-equal
    // `info_store.get(index_key)` settles ownership:
    //   - NotFound          → key free                 → OK
    //   - Some == our owner  → update self-write        → OK
    //   - Some != our owner  → another record owns it   → abort
    for g in &tx.unique_guards {
        if let Some(tbl) = repo.table_by_token(g.table_token).await? {
            match tbl.info_store().get(g.index_key.clone().into()).await {
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
    rederive_index2_ops_post_stage(tx, repo).await;

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
/// for every stage site, gating the legacy sorted-index re-derivation below.
/// Vector backends' `staged_vectors` are re-derived in the per-record loop
/// (their `plan_insert_tx` is a no-op; HNSW embeddings are buffered
/// separately) — Step 2 Part B.
async fn rederive_index2_ops_post_stage(tx: &mut TxContext, repo: &RepoInstance) {
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
            // Generation gate: skip the per-record re-derivation entirely when no
            // index2 backend was registered since stage. One atomic Acquire load.
            let new_backends = {
                let reg = tbl.index2_registry();
                if reg.generation() == stage_gen {
                    continue;
                }
                reg.backends_newer_than(stage_gen).await
            };
            if new_backends.is_empty() {
                continue;
            }
            // Collect the staged ops for this table into an owned Vec so no borrow
            // of `tx.write_set` is held across the per-record async planning below.
            let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
                Some(staging) => staging.snapshot_ops(),
                None => continue,
            };
            let data_store = tbl.data_store().clone();

            let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();
            for kvop in staged_ops {
                match kvop {
                    KvOp::Set(k, v) => {
                        let Some(rid) = RecordId::try_from_bytes(&k) else {
                            continue;
                        };
                        let Ok(new_rec) = InnerValue::from_bytes(&v) else {
                            continue;
                        };
                        // Phase 5a has not run: the store still holds the PRE-tx
                        // value, so this one read distinguishes insert vs. update.
                        match data_store.get(k.clone()).await {
                            Ok(old_bytes) => {
                                if let Ok(old_rec) = InnerValue::from_bytes(&old_bytes) {
                                    for backend in &new_backends {
                                        if let Ok(ops) = backend
                                            .plan_update_tx(rid, &old_rec, &new_rec, tx_id)
                                            .await
                                        {
                                            appended.extend(
                                                ops.into_iter().map(|op| (table_token, op)),
                                            );
                                        }
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
                            }
                            Err(DbError::NotFound(_)) => {
                                for backend in &new_backends {
                                    if let Ok(ops) =
                                        backend.plan_insert_tx(rid, &new_rec, tx_id).await
                                    {
                                        appended
                                            .extend(ops.into_iter().map(|op| (table_token, op)));
                                    }
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
                            // A non-NotFound storage error: best-effort skip.
                            Err(_) => {}
                        }
                    }
                    KvOp::Remove(k) => {
                        let Some(rid) = RecordId::try_from_bytes(&k) else {
                            continue;
                        };
                        // Nothing committed to delete from the index on a NotFound read
                        // (the row was never materialized) — best-effort skip otherwise.
                        if let Ok(old_bytes) = data_store.get(k.clone()).await {
                            if let Ok(old_rec) = InnerValue::from_bytes(&old_bytes) {
                                for backend in &new_backends {
                                    if let Ok(ops) =
                                        backend.plan_delete_tx(rid, &old_rec, tx_id).await
                                    {
                                        appended
                                            .extend(ops.into_iter().map(|op| (table_token, op)));
                                    }
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
            }
            if !appended.is_empty() {
                tx.index_write_set.extend(appended);
            }
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
            let staged_ops: Vec<KvOp> = match tx.write_set.get(&table_token) {
                Some(staging) => staging.snapshot_ops(),
                None => continue,
            };
            let data_store = tbl.data_store().clone();

            let mut appended: Vec<(u64, IndexWriteOp)> = Vec::new();
            for kvop in staged_ops {
                match kvop {
                    KvOp::Set(k, v) => {
                        let Some(rid) = RecordId::try_from_bytes(&k) else {
                            continue;
                        };
                        let Ok(new_rec) = InnerValue::from_bytes(&v) else {
                            continue;
                        };
                        // Phase 5a has not run: the store still holds the PRE-tx
                        // value, so this one read distinguishes insert vs. update.
                        match data_store.get(k.clone()).await {
                            Ok(old_bytes) => {
                                if let Ok(old_rec) = InnerValue::from_bytes(&old_bytes) {
                                    if let Ok(ops) =
                                        sorted_mgr.plan_record_updated(&rid, &old_rec, &new_rec, 0)
                                    {
                                        appended
                                            .extend(ops.into_iter().map(|op| (table_token, op)));
                                    }
                                }
                            }
                            Err(DbError::NotFound(_)) => {
                                if let Ok(ops) = sorted_mgr.plan_record_created(&rid, &new_rec, 0) {
                                    appended.extend(ops.into_iter().map(|op| (table_token, op)));
                                }
                            }
                            // A non-NotFound storage error: best-effort skip.
                            Err(_) => {}
                        }
                    }
                    KvOp::Remove(k) => {
                        let Some(rid) = RecordId::try_from_bytes(&k) else {
                            continue;
                        };
                        if let Ok(old_bytes) = data_store.get(k.clone()).await {
                            if let Ok(old_rec) = InnerValue::from_bytes(&old_bytes) {
                                if let Ok(ops) = sorted_mgr.plan_record_deleted(&rid, &old_rec) {
                                    appended.extend(ops.into_iter().map(|op| (table_token, op)));
                                }
                            }
                        }
                    }
                }
            }
            if !appended.is_empty() {
                tx.index_write_set.extend(appended);
            }
        }
    }
}

/// F-50 Step 2 (#870): true when `backend`'s descriptor kind is Vector. Used
/// to gate the vector-staging branches in [`rederive_index2_ops_post_stage`]
/// (VectorBackend::plan_*_tx are no-ops for a tx; embeddings route through
/// `tx.staged_vectors` / `tx.staged_vector_deletes` instead).
fn is_vector_backend(backend: &Arc<dyn shamir_index::backend::IndexBackend>) -> bool {
    matches!(backend.descriptor().kind, IndexKind::Vector(_))
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
