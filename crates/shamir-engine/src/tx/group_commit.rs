//! Group-commit orchestration — leader/follower batch commit.
//!
//! When multiple transactions finish their pre-lock work concurrently and
//! race for `commit_mutex`, the winner becomes the LEADER: it drains any
//! followers already queued, performs cross-tx conflict detection, runs SSI
//! validation per survivor, writes all WAL entries in a single `begin_many`
//! (one fsync), materializes each survivor sequentially, and notifies
//! followers of their outcome via oneshot channels.
//!
//! A transaction that loses `try_commit_lock` becomes a FOLLOWER: it
//! enqueues itself and awaits the leader's notification.

use std::collections::HashSet;

use bytes::Bytes;
use shamir_collections::THasher;
use shamir_storage::error::DbError;
use shamir_tx::{CommitWriteRecord, IsolationLevel, RepoTxGate, RepoWalManager, TxContext};
use tokio::sync::oneshot;

use crate::repo::RepoInstance;
use crate::tx::commit::{maybe_crash, release_pessimistic_locks, TxError};
use crate::tx::commit_phases::promote_vectors;
use crate::tx::materialize::{materialize, post_publish_cleanup};
use crate::tx::pre_commit::pre_commit_locked_validate;
use crate::tx::tx_outcome::{MaterializationState, TxOutcome};

/// Compute the write-set keys for conflict detection from a TxContext.
pub(crate) fn compute_write_set_keys(tx: &TxContext) -> HashSet<(u64, Bytes), THasher> {
    tx.write_set_keys()
        .map(|(token, key)| (token, Bytes::copy_from_slice(key)))
        .collect()
}

/// RAII guard: on leader panic, notifies all still-pending follower oneshots.
///
/// P0a: the per-version Aborted-on-panic obligation is now owned by each
/// survivor's [`shamir_tx::VersionGuard`] (held in `Validated.version_guard`).
/// A panic unwinds through the `validated` vec, dropping every guard and
/// marking its version Aborted — so this guard no longer tracks versions and
/// only handles follower notification.
struct PanicGuard {
    senders: Vec<Option<oneshot::Sender<Result<u64, DbError>>>>,
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        for slot in self.senders.iter_mut() {
            if let Some(tx) = slot.take() {
                let _ = tx.send(Err(DbError::Internal("leader aborted".into())));
            }
        }
    }
}

/// The leader path: processes own tx + queued followers in a single batch.
///
/// If no followers are pending, delegates to the original single-tx path
/// (byte-identical to pre-Db behavior). When followers exist, uses the
/// batched WAL path (`begin_many`) to amortize fsync across all survivors.
pub(super) async fn run_leader(
    leader_tx: TxContext,
    leader_uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    leader_write_set_keys: HashSet<(u64, Bytes), THasher>,
    commit_guard: tokio::sync::MutexGuard<'_, ()>,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
) -> Result<TxOutcome, TxError> {
    let followers = gate.drain_pending();

    // No followers: single-tx fast path (identical to pre-Db).
    if followers.is_empty() {
        return run_single_tx(leader_tx, leader_uwl_guards, commit_guard, repo, gate, wal).await;
    }

    // === Multi-tx batch path ===

    // Step 1: Cross-tx conflict filter.
    // We store accepted write_set_keys in a Vec for pairwise checks.
    let mut accepted_wsk: Vec<HashSet<(u64, Bytes), THasher>> =
        Vec::with_capacity(1 + followers.len());
    accepted_wsk.push(leader_write_set_keys);

    struct Entry {
        tx: TxContext,
        uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
        is_leader: bool,
    }

    let mut entries: Vec<Entry> = Vec::with_capacity(1 + followers.len());
    entries.push(Entry {
        tx: leader_tx,
        uwl_guards: leader_uwl_guards,
        is_leader: true,
    });

    // panic_guard.senders has one slot per accepted follower, in order.
    let mut panic_guard = PanicGuard {
        senders: Vec::with_capacity(followers.len()),
    };

    for f in followers {
        let conflicts = accepted_wsk
            .iter()
            .any(|a| a.intersection(&f.write_set_keys).next().is_some());
        if conflicts {
            release_pessimistic_locks(&f.tx, repo).await;
            let _ = f.result_tx.send(Err(DbError::Conflict(
                "write-set conflict in group commit".into(),
            )));
        } else {
            accepted_wsk.push(f.write_set_keys);
            panic_guard.senders.push(Some(f.result_tx));
            entries.push(Entry {
                tx: f.tx,
                uwl_guards: f.uwl_guards,
                is_leader: false,
            });
        }
    }
    drop(accepted_wsk);

    // Step 2: Per-candidate SSI validation + WAL entry build.
    struct Validated {
        tx: TxContext,
        uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
        commit_version: u64,
        wal_entry: shamir_wal::WalEntryV2,
        /// RAII owner of this survivor's terminal-mark obligation (P0a).
        /// Dropped → Aborted on any abort path / panic; consumed via
        /// `materialize` → `commit()` → Materialized on the success path.
        version_guard: shamir_tx::VersionGuard,
        is_leader: bool,
        /// Index into panic_guard.senders (meaningful only for followers).
        pg_idx: usize,
    }

    let mut validated: Vec<Validated> = Vec::with_capacity(entries.len());
    let mut leader_empty_outcome: Option<TxOutcome> = None;
    let mut follower_counter: usize = 0; // tracks pg_idx for followers

    // P3a: batch-local footprint accumulator for inter-batch phantom detection.
    // As each survivor passes validation, its write-footprint is appended here
    // so subsequent survivors' predicate checks cover earlier batch members.
    let mut batch_footprints: Vec<CommitWriteRecord> = Vec::new();

    for mut entry in entries {
        let pg_idx = if entry.is_leader {
            usize::MAX // sentinel — not used for leader
        } else {
            let idx = follower_counter;
            follower_counter += 1;
            idx
        };

        match pre_commit_locked_validate(&mut entry.tx, repo, gate, entry.uwl_guards).await {
            Ok(Some(vpc)) => {
                // P3a: inter-batch phantom check — scan this survivor's
                // predicates against footprints of earlier accepted survivors
                // in this batch. The committed log is already checked inside
                // pre_commit_locked_validate; this closes the gap for
                // intra-batch phantoms.
                let phantom_conflict = if entry.tx.isolation == IsolationLevel::Serializable
                    && !entry.tx.predicate_set.is_empty()
                    && !batch_footprints.is_empty()
                {
                    let mut conflict_dep: Option<String> = None;
                    entry.tx.predicate_set.with_iter(|dep| {
                        if conflict_dep.is_none() {
                            for fp in &batch_footprints {
                                if shamir_tx::record_conflicts(fp, dep) {
                                    conflict_dep = Some(format!("{:?}", dep));
                                    break;
                                }
                            }
                        }
                    });
                    conflict_dep
                } else {
                    None
                };

                if let Some(dep) = phantom_conflict {
                    // Abort this survivor due to intra-batch phantom.
                    // Dropping `vpc.version_guard` (with the rest of `vpc`)
                    // marks this version Aborted.
                    repo.tx_metrics().on_tx_aborted_phantom();
                    release_pessimistic_locks(&entry.tx, repo).await;
                    if entry.is_leader {
                        // Leader failed — abort all validated followers too.
                        // Draining `validated` drops each peer's
                        // version_guard → mark(Aborted).
                        for v in validated.drain(..) {
                            if !v.is_leader {
                                if let Some(s) = panic_guard.senders[v.pg_idx].take() {
                                    let _ = s.send(Err(DbError::Internal(
                                        "batch aborted: leader failed".into(),
                                    )));
                                }
                            }
                            // v.version_guard drops here → mark(Aborted).
                        }
                        drop(panic_guard);
                        drop(commit_guard);
                        return Err(TxError::PhantomConflict { dep });
                    }
                    // Follower failed — notify, continue.
                    if let Some(sender) = panic_guard.senders[pg_idx].take() {
                        let _ = sender
                            .send(Err(DbError::Conflict(format!("phantom conflict: {}", dep))));
                    }
                    continue;
                }

                // P3a: accumulate this survivor's footprint for subsequent
                // survivors' phantom checks.
                let footprint = shamir_tx::build_footprint_from_tx(&entry.tx, vpc.commit_version);
                if !footprint.is_empty() {
                    batch_footprints.push(footprint);
                }

                // The survivor's version_guard moves into `validated`; on any
                // later abort path / panic it drops → Aborted, and on success
                // `materialize` consumes it → Materialized.
                validated.push(Validated {
                    tx: entry.tx,
                    uwl_guards: vpc.uwl_guards,
                    commit_version: vpc.commit_version,
                    wal_entry: vpc.wal_entry,
                    version_guard: vpc.version_guard,
                    is_leader: entry.is_leader,
                    pg_idx,
                });
            }
            Ok(None) => {
                // C6 empty-tx.
                repo.tx_metrics().on_tx_committed();
                release_pessimistic_locks(&entry.tx, repo).await;
                if entry.is_leader {
                    leader_empty_outcome = Some(TxOutcome {
                        tx_id: entry.tx.tx_id.0,
                        snapshot_version: entry.tx.snapshot_version,
                        commit_version: entry.tx.snapshot_version,
                        materialization: MaterializationState::Complete,
                        background: None,
                    });
                } else if let Some(sender) = panic_guard.senders[pg_idx].take() {
                    let _ = sender.send(Ok(entry.tx.snapshot_version));
                }
            }
            Err(e) => {
                // Note: the version for THIS entry was already marked
                // Aborted by its VersionGuard dropping inside
                // pre_commit_locked_validate on error.
                release_pessimistic_locks(&entry.tx, repo).await;
                if entry.is_leader {
                    // Leader failed SSI — abort all validated followers too.
                    // Their versions were assigned successfully but will not
                    // materialize — draining `validated` drops each peer's
                    // version_guard → mark(Aborted).
                    for v in validated.drain(..) {
                        if !v.is_leader {
                            if let Some(s) = panic_guard.senders[v.pg_idx].take() {
                                let _ = s.send(Err(DbError::Internal(
                                    "batch aborted: leader failed".into(),
                                )));
                            }
                        }
                        // v.version_guard drops here → mark(Aborted).
                    }
                    drop(panic_guard);
                    drop(commit_guard);
                    return Err(e);
                }
                // Follower failed — notify, continue.
                if let Some(sender) = panic_guard.senders[pg_idx].take() {
                    let _ = sender.send(Err(tx_error_to_db_error(&e)));
                }
            }
        }
    }

    if validated.is_empty() {
        drop(panic_guard);
        drop(commit_guard);
        return leader_empty_outcome
            .ok_or_else(|| TxError::Storage(DbError::Internal("group commit: no entries".into())));
    }

    // Step 3: Batched WAL begin (ONE fsync for all survivors).
    let wal_entries: Vec<_> = validated.iter().map(|v| v.wal_entry.clone()).collect();
    if let Err(e) = wal
        .begin_grouped_many(&wal_entries, shamir_wal::WalDurability::Buffered)
        .await
    {
        // WAL begin failed — nothing durable. Draining `validated` drops
        // each survivor's version_guard → mark(Aborted).
        for v in validated.drain(..) {
            if !v.is_leader {
                if let Some(s) = panic_guard.senders[v.pg_idx].take() {
                    let _ = s.send(Err(DbError::Storage(e.to_string())));
                }
            }
            // v.version_guard drops here → mark(Aborted).
        }
        drop(panic_guard);
        drop(commit_guard);
        return Err(TxError::Storage(e));
    }

    maybe_crash("phase4", repo).await;

    // Step 4: Record SSI footprints UNDER lock, release locks, notify followers,
    // then materialize OUTSIDE lock (P2b).
    struct PostWork {
        tx: TxContext,
        commit_version: u64,
        uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
        version_guard: shamir_tx::VersionGuard,
        changefeed_event: Option<shamir_tx::ChangelogEvent>,
        is_leader: bool,
    }

    let mut post_works: Vec<PostWork> = Vec::with_capacity(validated.len());

    for v in &validated {
        let commit_version = v.commit_version;
        // Phase 6-bis (Phase C): record SSI write footprint UNDER commit_lock
        // so the next batch's SSI validation sees this tx's writes (P2b).
        gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&v.tx, commit_version));
        release_pessimistic_locks(&v.tx, repo).await;
        repo.tx_metrics().on_tx_committed();
    }

    // Notify followers that their tx committed (version assigned, WAL durable,
    // footprint recorded). They can proceed while materialize runs.
    for v in &validated {
        if !v.is_leader {
            if let Some(sender) = panic_guard.senders[v.pg_idx].take() {
                let _ = sender.send(Ok(v.commit_version));
            }
        }
    }

    // Release the commit lock — materialize runs outside it (P2b).
    drop(commit_guard);
    // Disarm panic guard safely (all followers notified). The per-version
    // Aborted obligation now lives in each survivor's version_guard, which
    // is moved into PostWork below and consumed by materialize → committed.
    panic_guard.senders.clear();
    std::mem::forget(panic_guard);

    // Consume validated into post_works (need ownership for materialize's &mut tx).
    for v in validated {
        let changefeed_event = shamir_tx::project_event(&v.tx, repo.name(), v.commit_version);
        post_works.push(PostWork {
            tx: v.tx,
            commit_version: v.commit_version,
            uwl_guards: v.uwl_guards,
            version_guard: v.version_guard,
            changefeed_event,
            is_leader: v.is_leader,
        });
    }

    let mut leader_version = 0u64;
    let mut leader_materialization = MaterializationState::Complete;
    let mut leader_snapshot = 0u64;
    let mut leader_tx_id = 0u64;

    // P2b: materialize each survivor OUTSIDE commit_lock, gated by uwl_guards.
    for mut work in post_works {
        let post_publish =
            materialize(&mut work.tx, repo, work.version_guard, work.uwl_guards).await;
        let mat = post_publish_cleanup(post_publish, repo, gate).await;
        if mat == MaterializationState::Deferred {
            repo.tx_metrics().on_tx_materialization_deferred();
        }
        // D2 P1d-2b CUTOVER: inline `gate.mark_durable` removed — the ack-path
        // wrote only the overlay; durability + WAL truncation are the drainer's
        // job now. Wake it after each survivor publishes so the batch's tail
        // drains promptly.
        repo.drainer().wake();
        repo.emit_changefeed_event(work.changefeed_event).await;
        promote_vectors(&work.tx, repo, work.commit_version).await;

        if work.is_leader {
            leader_version = work.commit_version;
            leader_materialization = mat;
            leader_snapshot = work.tx.snapshot_version;
            leader_tx_id = work.tx.tx_id.0;
        }
    }

    // Return leader outcome.
    if leader_version > 0 {
        Ok(TxOutcome {
            tx_id: leader_tx_id,
            snapshot_version: leader_snapshot,
            commit_version: leader_version,
            materialization: leader_materialization,
            background: None,
        })
    } else if let Some(outcome) = leader_empty_outcome {
        Ok(outcome)
    } else {
        Err(TxError::Storage(DbError::Internal(
            "group commit: leader outcome missing".into(),
        )))
    }
}

/// Single-tx commit path: identical to pre-Db behavior.
async fn run_single_tx(
    mut tx: TxContext,
    uwl_guards: Vec<tokio::sync::OwnedMutexGuard<()>>,
    commit_guard: tokio::sync::MutexGuard<'_, ()>,
    repo: &RepoInstance,
    gate: &RepoTxGate,
    wal: &RepoWalManager,
) -> Result<TxOutcome, TxError> {
    use crate::tx::pre_commit::pre_commit_locked;

    let pre = match pre_commit_locked(&mut tx, repo, gate, wal, uwl_guards).await {
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
            drop(commit_guard);
            return Err(e);
        }
    };

    let commit_version = pre.commit_version;
    release_pessimistic_locks(&tx, repo).await;

    // Phase 6-bis (Phase C): record SSI write footprint UNDER commit_lock
    // so the next batch's SSI validation sees this tx's writes (P2b).
    gate.record_commit_writes(shamir_tx::build_footprint_from_tx(&tx, commit_version));
    repo.tx_metrics().on_tx_committed();
    drop(commit_guard);

    // P2b: materialize runs OUTSIDE commit_lock, gated by per-table
    // uwl_guards. Disjoint-table commits proceed in parallel.
    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);
    let post_publish = materialize(&mut tx, repo, pre.version_guard, pre.uwl_guards).await;

    let materialization = post_publish_cleanup(post_publish, repo, gate).await;
    if materialization == MaterializationState::Deferred {
        repo.tx_metrics().on_tx_materialization_deferred();
    }
    // D2 P1d-2b CUTOVER: inline `gate.mark_durable` removed — durability + WAL
    // truncation moved to the background drainer. Wake it after publish.
    repo.drainer().wake();
    repo.emit_changefeed_event(changefeed_event).await;
    promote_vectors(&tx, repo, commit_version).await;

    Ok(TxOutcome {
        tx_id: tx.tx_id.0,
        snapshot_version: tx.snapshot_version,
        commit_version,
        materialization,
        background: None,
    })
}

/// Convert a TxError to a DbError for sending over the oneshot channel.
fn tx_error_to_db_error(e: &TxError) -> DbError {
    match e {
        TxError::Storage(_) => DbError::Internal(e.to_string()),
        TxError::SsiConflict { key } => DbError::Conflict(format!("ssi conflict on key {:?}", key)),
        TxError::UniqueViolation { key } => {
            DbError::Conflict(format!("unique violation on key {:?}", key))
        }
        TxError::Expired { elapsed, max } => {
            DbError::Conflict(format!("tx expired: {:?} > {:?}", elapsed, max))
        }
        TxError::PhantomConflict { dep } => DbError::Conflict(format!("phantom conflict: {}", dep)),
        TxError::Wounded { tx_version } => DbError::Conflict(format!("tx {} wounded", tx_version)),
    }
}
