use std::sync::Arc;

use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
use shamir_tx::{CellReservationGuard, IsolationLevel, RepoTxGate, RepoWalManager, TxContext};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;
use shamir_wal::WalEntryV2;

use crate::repo::RepoInstance;
use crate::tx::commit::{maybe_crash, TxError};

use super::commit::wal_ops_from_tx;

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
        let Some(store) = mvcc_map
            .read_async(table_id, |_, mvcc| std::sync::Arc::clone(mvcc))
            .await
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
pub(super) struct PreLockResult {
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
}

/// Pre-lock phase of the commit pipeline: runs OUTSIDE `commit_lock`,
/// concurrent with other committers.
///
/// Performs:
/// - Phase 1: interner overlay merge + remap (CAS-safe on DashMap).
/// - Phase 2.5: acquire per-table `unique_write_lock` guards in sorted
///   token order. These serialise against non-tx unique writers and against
///   other committers touching the same unique-constrained tables.
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
    // `docs/audits/2026-07-06-concurrency-engine.md` A8.
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

    // Phase 2.5 (HIGH-A): acquire each unique table's `unique_write_lock`
    // and HOLD it across Phase 2.6 → 5c.
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
    // We use `lock_owned()` so the guards can be collected into a `Vec`
    // without borrow-lifetime entanglement (each `OwnedMutexGuard` owns its
    // `Arc<Mutex<()>>`). Tables without unique guards are untouched — the
    // non-unique commit path keeps the lock-free fast path.
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

    Ok(PreLockResult { uwl_guards })
}

/// Outcome of [`pre_commit_locked_validate`]: the assigned commit version,
/// built WAL entry, and uwl_guards — ready for WAL begin (Phase 4).
pub(super) struct ValidatedPreCommit {
    pub(super) commit_version: u64,
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
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
    if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
        let deps = tx.predicate_set.snapshot_deps();
        if let Some(idx) = gate.predicate_conflicts_batch(&deps, tx.snapshot_version) {
            let dep = format!("{:?}", deps[idx]);
            repo.tx_metrics().on_tx_aborted_phantom();
            return Err(TxError::PhantomConflict { dep });
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
    if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
        let deps = tx.predicate_set.snapshot_deps();
        if let Some(idx) = gate.predicate_conflicts_batch(&deps, tx.snapshot_version) {
            let dep = format!("{:?}", deps[idx]);
            repo.tx_metrics().on_tx_aborted_phantom();
            return Err(TxError::PhantomConflict { dep });
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
        version_guard,
        cell_guards,
        wal_entry_arc: entry_arc,
    }))
}
