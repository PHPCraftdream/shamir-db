use shamir_storage::error::DbError;
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
pub(super) async fn pre_commit(
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
