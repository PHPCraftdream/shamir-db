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
use shamir_tx::{RepoTxGate, RepoWalManager, TxContext};
use tokio::sync::oneshot;

use crate::repo::RepoInstance;
use crate::tx::commit::{maybe_crash, release_pessimistic_locks, TxError};
use crate::tx::commit_phases::promote_vectors;
use crate::tx::materialize::{materialize, post_publish_cleanup, PostPublishState};
use crate::tx::pre_commit::pre_commit_locked_validate;
use crate::tx::tx_outcome::{MaterializationState, TxOutcome};

/// Compute the write-set keys for conflict detection from a TxContext.
pub(crate) fn compute_write_set_keys(tx: &TxContext) -> HashSet<(u64, Bytes), THasher> {
    tx.write_set_keys()
        .map(|(token, key)| (token, Bytes::copy_from_slice(key)))
        .collect()
}

/// RAII guard: on leader panic, notifies all still-pending follower oneshots.
struct PanicGuard(Vec<Option<oneshot::Sender<Result<u64, DbError>>>>);

impl Drop for PanicGuard {
    fn drop(&mut self) {
        for slot in self.0.iter_mut() {
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

    // panic_guard.0 has one slot per accepted follower, in order.
    let mut panic_guard = PanicGuard(Vec::with_capacity(followers.len()));

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
            panic_guard.0.push(Some(f.result_tx));
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
        is_leader: bool,
        /// Index into panic_guard.0 (meaningful only for followers).
        pg_idx: usize,
    }

    let mut validated: Vec<Validated> = Vec::with_capacity(entries.len());
    let mut leader_empty_outcome: Option<TxOutcome> = None;
    let mut follower_counter: usize = 0; // tracks pg_idx for followers

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
                validated.push(Validated {
                    tx: entry.tx,
                    uwl_guards: vpc.uwl_guards,
                    commit_version: vpc.commit_version,
                    wal_entry: vpc.wal_entry,
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
                } else if let Some(sender) = panic_guard.0[pg_idx].take() {
                    let _ = sender.send(Ok(entry.tx.snapshot_version));
                }
            }
            Err(e) => {
                release_pessimistic_locks(&entry.tx, repo).await;
                if entry.is_leader {
                    // Leader failed SSI — abort all validated followers too.
                    for v in &validated {
                        if !v.is_leader {
                            if let Some(s) = panic_guard.0[v.pg_idx].take() {
                                let _ = s.send(Err(DbError::Internal(
                                    "batch aborted: leader failed".into(),
                                )));
                            }
                        }
                    }
                    drop(panic_guard);
                    drop(commit_guard);
                    return Err(e);
                }
                // Follower failed — notify, continue.
                if let Some(sender) = panic_guard.0[pg_idx].take() {
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
    if let Err(e) = wal.begin_many(&wal_entries).await {
        for v in &validated {
            if !v.is_leader {
                if let Some(s) = panic_guard.0[v.pg_idx].take() {
                    let _ = s.send(Err(DbError::Storage(e.to_string())));
                }
            }
        }
        drop(panic_guard);
        drop(commit_guard);
        return Err(TxError::Storage(e));
    }

    maybe_crash("phase4", repo).await;

    // Step 4: Per-survivor materialize + publish (under lock).
    struct PostWork {
        tx: TxContext,
        commit_version: u64,
        post_publish: PostPublishState,
        changefeed_event: Option<shamir_tx::ChangelogEvent>,
        is_leader: bool,
    }

    let mut post_works: Vec<PostWork> = Vec::with_capacity(validated.len());

    for mut v in validated {
        let commit_version = v.commit_version;
        release_pessimistic_locks(&v.tx, repo).await;

        let changefeed_event = shamir_tx::project_event(&v.tx, repo.name(), commit_version);
        let post_publish = materialize(&mut v.tx, repo, gate, commit_version, v.uwl_guards).await;
        repo.tx_metrics().on_tx_committed();

        // Notify follower.
        if !v.is_leader {
            if let Some(sender) = panic_guard.0[v.pg_idx].take() {
                let _ = sender.send(Ok(commit_version));
            }
        }

        post_works.push(PostWork {
            tx: v.tx,
            commit_version,
            post_publish,
            changefeed_event,
            is_leader: v.is_leader,
        });
    }

    // Step 5: Release lock, run post-lock work.
    drop(commit_guard);
    // Disarm panic guard safely (all followers notified).
    let _ = std::mem::take(&mut panic_guard.0);
    std::mem::forget(panic_guard);

    let mut leader_version = 0u64;
    let mut leader_materialization = MaterializationState::Complete;
    let mut leader_snapshot = 0u64;
    let mut leader_tx_id = 0u64;

    for work in post_works {
        let mat = post_publish_cleanup(work.post_publish, repo, gate, wal).await;
        if mat == MaterializationState::Deferred {
            repo.tx_metrics().on_tx_materialization_deferred();
        }
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

    let changefeed_event = shamir_tx::project_event(&tx, repo.name(), commit_version);
    let post_publish = materialize(&mut tx, repo, gate, commit_version, pre.uwl_guards).await;
    repo.tx_metrics().on_tx_committed();
    drop(commit_guard);

    let materialization = post_publish_cleanup(post_publish, repo, gate, wal).await;
    if materialization == MaterializationState::Deferred {
        repo.tx_metrics().on_tx_materialization_deferred();
    }
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
