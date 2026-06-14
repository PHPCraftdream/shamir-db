use shamir_storage::error::DbError;
use shamir_tx::completion_tracker::State;
use shamir_tx::{IsolationLevel, RepoTxGate, RepoWalManager, TxContext};
use shamir_wal::WalEntryV2;

use crate::repo::RepoInstance;
use crate::tx::commit::{maybe_crash, TxError};

use super::commit::wal_ops_from_tx;

/// Outcome of [`pre_commit`]: the assigned MVCC commit version plus the
/// per-table `unique_write_lock` guards that must stay held through
/// Phase 5c (released inside [`materialize`]).
pub(super) struct PreCommit {
    pub(super) commit_version: u64,
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
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
                let shamir_tx::OverlayCommitResult { remap, delta } =
                    shamir_tx::commit_interner_overlay(base_interner, &tx.interner_overlay).await?;
                if !delta.is_empty() {
                    tx.interner_deltas.insert(*table_id, delta);
                }
                if !remap.is_empty() {
                    if let Some(staging) = tx.write_set.get_mut(table_id) {
                        staging
                            .rewrite_set_inner(|inner| {
                                shamir_tx::remap_value(inner, &remap);
                                Ok(())
                            })
                            .await
                            .map_err(DbError::Codec)?;
                    }
                }
                // A5: interner persist removed from the commit critical
                // path. The WAL entry carries the interner delta
                // (`interner_deltas`), so crash recovery replays new
                // (name, id) mappings via `touch_with_id`. A background
                // checkpoint (every INTERNER_CHECKPOINT_INTERVAL commits)
                // flushes the delta to the durable chunk store, advancing
                // the persisted high-water mark so Phase 7 WAL truncation
                // can proceed. Graceful shutdown flushes all interners.
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

    Ok(PreLockResult { uwl_guards })
}

/// Outcome of [`pre_commit_locked_validate`]: the assigned commit version,
/// built WAL entry, and uwl_guards — ready for WAL begin (Phase 4).
pub(super) struct ValidatedPreCommit {
    pub(super) commit_version: u64,
    pub(super) uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    pub(super) wal_entry: shamir_wal::WalEntryV2,
}

/// Locked validation phase: Phases 3 + 2 + 2-bis + C6 + WAL entry build.
///
/// Does NOT write the WAL entry (no fsync). The caller is responsible for
/// calling `wal.begin_grouped(entry, ..)` or batching via
/// `wal.begin_grouped_many`. This split
/// enables group-commit fsync amortization.
///
/// Phase 3 (assign_next_version) is now FIRST — the version is allocated
/// optimistically via atomic fetch_add BEFORE validation. If any subsequent
/// phase aborts, the version is marked Aborted in the CompletionTracker.
///
/// Returns `Some(ValidatedPreCommit)` when the tx has durable work,
/// `None` for C6 empty-tx fast-path, or `Err` on validation failure.
pub(super) async fn pre_commit_locked_validate(
    tx: &mut TxContext,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
) -> Result<Option<ValidatedPreCommit>, TxError> {
    // Phase 3 (P2a): assign new version BEFORE validation.
    // Pure atomic fetch_add — lock-free, safe to call without commit_mutex.
    // If any subsequent phase aborts, the version is marked Aborted.
    let commit_version = gate.assign_next_version();

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
            gate.completion().mark(commit_version, State::Aborted);
            return Err(TxError::SsiConflict { key });
        }
    }

    // Phase 2-bis (SSI only, Phase C): predicate read-set validation.
    if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
        let mut conflict_dep: Option<String> = None;
        tx.predicate_set.with_iter(|dep| {
            if conflict_dep.is_none() && gate.predicate_conflicts(dep, tx.snapshot_version) {
                conflict_dep = Some(format!("{:?}", dep));
            }
        });
        if let Some(dep) = conflict_dep {
            repo.tx_metrics().on_tx_aborted_phantom();
            gate.completion().mark(commit_version, State::Aborted);
            return Err(TxError::PhantomConflict { dep });
        }
    }

    // C6: empty-tx fast-path. Version was allocated but tx has no durable
    // work — mark it Aborted so the watermark advances past it.
    if tx.is_empty() {
        gate.completion().mark(commit_version, State::Aborted);
        return Ok(None);
    }

    // Crash seam (test-only).
    maybe_crash("pre_commit", repo).await;

    // Build WAL entry (Phase 4 prep) — does NOT persist.
    let wal_ops = wal_ops_from_tx(tx).await;
    let interner_delta: Vec<(u64, String, u64)> = tx
        .interner_deltas
        .iter()
        .flat_map(|(token, deltas)| {
            deltas
                .iter()
                .map(move |(name, id)| (*token, name.clone(), *id))
        })
        .collect();
    let mut wal_entry = shamir_wal::WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops)
        .with_commit_version(commit_version);
    wal_entry.interner_delta = interner_delta;

    Ok(Some(ValidatedPreCommit {
        commit_version,
        uwl_guards,
        wal_entry,
    }))
}

/// Locked phase of the commit pipeline: runs UNDER `commit_lock`.
///
/// Performs:
/// - Phase 3: assign_next_version (P2a: moved BEFORE validation).
/// - Phase 2: SSI read-set validation (must be atomic with Phase 6
///   record_commit_writes — both under lock).
/// - Phase 2-bis: phantom predicate validation.
/// - C6: empty-tx fast-path check.
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
    // Phase 3 (P2a): assign new version BEFORE validation.
    // Pure atomic fetch_add — lock-free, safe without commit_mutex.
    // If any subsequent phase aborts, the version is marked Aborted.
    let commit_version = gate.assign_next_version();

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
            gate.completion().mark(commit_version, State::Aborted);
            return Err(TxError::SsiConflict { key });
        }
    }

    // Phase 2-bis (SSI only, Phase C): predicate read-set validation.
    if tx.isolation == IsolationLevel::Serializable && !tx.predicate_set.is_empty() {
        let mut conflict_dep: Option<String> = None;
        tx.predicate_set.with_iter(|dep| {
            if conflict_dep.is_none() && gate.predicate_conflicts(dep, tx.snapshot_version) {
                conflict_dep = Some(format!("{:?}", dep));
            }
        });
        if let Some(dep) = conflict_dep {
            repo.tx_metrics().on_tx_aborted_phantom();
            gate.completion().mark(commit_version, State::Aborted);
            return Err(TxError::PhantomConflict { dep });
        }
    }

    // === C6: empty-tx fast-path ===
    //
    // Version was allocated but tx has no durable work — mark Aborted so
    // the watermark advances past it.
    //
    // ORDERING IS LOAD-BEARING: this sits AFTER the Phase 2 SSI block above.
    // A read-only Serializable tx still records reads, and its read_set must
    // still be validated against current committed versions — a read-only
    // tx that observed stale data MUST abort (returned as `Err` above), not
    // silently fast-path to success.
    if tx.is_empty() {
        gate.completion().mark(commit_version, State::Aborted);
        return Ok(None);
    }

    // Crash seam (test-only): a HARD crash here is BEFORE the commit
    // point — staging is dropped, no WAL entry exists, locks release by
    // RAII. Recovery must find nothing → clean abort.
    maybe_crash("pre_commit", repo).await;

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
    // A3: flatten per-table interner deltas into the WAL entry so
    // recovery can replay them via `touch_with_id` before data ops.
    let interner_delta: Vec<(u64, String, u64)> = tx
        .interner_deltas
        .iter()
        .flat_map(|(token, deltas)| {
            deltas
                .iter()
                .map(move |(name, id)| (*token, name.clone(), *id))
        })
        .collect();
    let mut entry =
        WalEntryV2::new(tx.tx_id.0, tx.repo_id, wal_ops).with_commit_version(commit_version);
    entry.interner_delta = interner_delta;
    if let Err(e) = wal
        .begin_grouped(entry, shamir_wal::WalDurability::Buffered)
        .await
    {
        gate.completion().mark(commit_version, State::Aborted);
        return Err(TxError::Storage(e));
    }

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
