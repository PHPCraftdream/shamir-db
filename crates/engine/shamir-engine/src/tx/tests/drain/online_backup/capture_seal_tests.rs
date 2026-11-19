//! Capture/seal under continuous ordinary commits (brief item 8, test 2)
//! — the hazard-1 shapes: an orphan index posting through the drain's
//! index branch, a pending `InternerOverlayMerge` WAL op (overlay-only
//! commit), and live committers racing the barrier. The regression proof
//! is completion WITHIN the budget: a capture that kept a repo write
//! permit would self-deadlock on its own drain and trip the harness cap.

use std::time::Duration;

use bytes::Bytes;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::access::Actor;
use shamir_types::types::value::InnerValue;
use shamir_wal::WalOpV2;

use crate::repo::RepoInstance;
use crate::table::{table_manager::table_token_for, TableConfig};
use crate::tx::tests::drain::drainer_tests::support::{make_repo, seed_inflight_entry};
use crate::tx::CommitError;

/// Generous harness cap: the barrier itself is a handshake against the
/// concurrent committers, and the cap only converts a self-deadlock (the
/// hazard-1 regression) into a loud failure.
const BARRIER_BUDGET: Duration = Duration::from_secs(60);
const COMMITTERS: usize = 4;
const COMMITS_PER_COMMITTER: usize = 8;

/// Per-committer per-commit outcomes.
type CommitterOutcomes = Vec<Result<(), CommitError>>;

/// A transaction whose ONLY effect is an interner overlay merge (no write
/// set, no index write set): the pending `InternerOverlayMerge` WAL op
/// the drain must carry across the barrier.
fn overlay_only_tx() -> TxContext {
    let mut tx = TxContext::new(TxId::new(u64::MAX / 2), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(Actor::User(42));
    tx.interner_overlay
        .insert_sync("backup_probe".to_string(), shamir_tx::OVERLAY_ID_BASE)
        .expect("overlay insert ok");
    tx
}

async fn seeded_repo() -> RepoInstance {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();

    // One ordinary commit + one overlay-only commit go through the real
    // pipeline, assigning real versions.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    table
        .insert_tx(&InnerValue::Str("base".into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    repo.commit_tx(overlay_only_tx())
        .await
        .expect("overlay-only commit proceeds while the gate is open");

    // The orphan index posting rides ABOVE the committed prefix (the
    // version the commit path never assigned) and is made visible so the
    // drain window covers it; capture's settled drain replays it through
    // the `begin_write_barrier` index branch.
    let orphan_version = gate.last_committed() + 1;
    seed_inflight_entry(
        &repo,
        vec![WalOpV2::IndexPut {
            table_id_interned: table_token_for("t"),
            idx_id: 0,
            key: Bytes::from_static(b"orphan"),
            value: Bytes::from_static(b"orphan-value"),
        }],
        orphan_version,
    )
    .await;
    gate.publish_committed_max(orphan_version);
    repo
}

/// Spawn `COMMITTERS` continuous committers; each records its per-commit
/// outcome. While the gate is closed a refusal IS the correct outcome
/// (bounded by the commit entry's own reopen cap); a hang is not.
async fn spawn_committers(repo: &RepoInstance) -> Vec<tokio::task::JoinHandle<CommitterOutcomes>> {
    let mut tasks = Vec::new();
    for c in 0..COMMITTERS {
        let owned = repo.clone();
        tasks.push(tokio::spawn(async move {
            let table = owned.get_table("t").await.unwrap();
            let mut outcomes = Vec::new();
            for i in 0..COMMITS_PER_COMMITTER {
                let staged = owned.begin_tx(IsolationLevel::Snapshot).await;
                let (mut tx, _guard) = match staged {
                    Ok(pair) => pair,
                    Err(e) => {
                        outcomes.push(Err(CommitError::Storage(e)));
                        continue;
                    }
                };
                let inserted = table
                    .insert_tx(&InnerValue::Str(format!("c{c}-i{i}")), Some(&mut tx))
                    .await;
                if let Err(e) = inserted {
                    outcomes.push(Err(CommitError::Storage(e)));
                    continue;
                }
                outcomes.push(owned.commit_tx(tx).await.map(|_| ()));
            }
            outcomes
        }));
    }
    tasks
}

fn assert_refusals_only_ok_or_refused(outcomes: &[Result<(), CommitError>]) {
    for outcome in outcomes {
        match outcome {
            Ok(()) => {}
            Err(CommitError::CommitAdmissionRefused) => {}
            Err(other) => panic!("unexpected commit outcome under the barrier: {other:?}"),
        }
    }
}

#[tokio::test]
async fn capture_completes_under_concurrent_commits_no_repo_permit() {
    let repo = seeded_repo().await;
    let table = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();

    let committers = spawn_committers(&repo).await;
    // The regression: capture runs its settled drain while committers race
    // it. A capture that kept a repo write permit would never complete —
    // only the harness cap would fire.
    let position = tokio::time::timeout(BARRIER_BUDGET, repo.capture_point())
        .await
        .expect("capture deadlocked under concurrent commits (hazard-1 regression)")
        .expect("capture succeeded under concurrent commits");
    assert_eq!(position, gate.last_committed());

    // Join the committers and settle whatever they committed after the
    // gate reopened.
    let mut admitted = 0usize;
    for task in committers {
        let outcomes = tokio::time::timeout(BARRIER_BUDGET, task)
            .await
            .expect("committer hung")
            .expect("committer join");
        assert_refusals_only_ok_or_refused(&outcomes);
        admitted += outcomes.iter().filter(|r| r.is_ok()).count();
    }
    assert!(admitted > 0, "at least some concurrent commits must land");

    repo.drainer().drain_all(&repo.core).await.unwrap();
    assert_eq!(
        gate.durable_watermark(),
        gate.last_committed(),
        "capture leaves the repo settled"
    );

    // The hazard-1 shapes actually traversed the drain: the orphan posting
    // is in the info store, the overlay merge is in the base interner.
    let orphan = table
        .info_store()
        .get(Bytes::from_static(b"orphan").into())
        .await
        .expect("orphan index posting drained");
    assert_eq!(orphan, Bytes::from_static(b"orphan-value"));
    let interner = table.interner().get().await.unwrap();
    assert!(
        interner.get_ind("backup_probe").is_some(),
        "overlay-only commit's InternerOverlayMerge landed in the base interner"
    );
}

/// Actual WAL horizon: the highest commit version of any recovered WAL
/// entry (0 on an empty WAL).
async fn wal_horizon(repo: &RepoInstance) -> u64 {
    repo.repo_wal()
        .await
        .unwrap()
        .recover()
        .await
        .unwrap()
        .iter()
        .map(|e| e.commit_version)
        .max()
        .unwrap_or(0)
}

#[tokio::test]
async fn seal_completes_under_concurrent_commits_no_repo_permit() {
    let repo = seeded_repo().await;
    let gate = repo.tx_gate().await.unwrap();

    let committers = spawn_committers(&repo).await;
    let hold = tokio::time::timeout(BARRIER_BUDGET, repo.seal_point())
        .await
        .expect("seal deadlocked under concurrent commits (hazard-1 regression)")
        .expect("seal succeeded under concurrent commits");
    let v_star = hold.position();
    // The hold now owns the close owner as well: the gate stays CLOSED
    // while the hold is alive, with zero admission leases, and the actual
    // WAL horizon is stable at V* — every successful committer commit is
    // a "before" side entry through V*, and the "after" side can only
    // append once the hold drops and reopens the gate.
    let admission = repo.core().commit_admission();
    assert!(
        admission.is_closed(),
        "the hold keeps the admission gate closed"
    );
    assert_eq!(
        admission.active_count(),
        0,
        "no admission leases under the hold"
    );
    let before_horizon = wal_horizon(&repo).await;
    assert_eq!(
        before_horizon, v_star,
        "all successful before-side commits landed through V*"
    );
    assert_eq!(
        wal_horizon(&repo).await,
        before_horizon,
        "the actual WAL horizon is stable while the hold is live"
    );
    drop(hold);
    assert!(
        !admission.is_closed(),
        "hold drop reopens the gate before the after-side commits"
    );

    let mut admitted = 0usize;
    for task in committers {
        let outcomes = tokio::time::timeout(BARRIER_BUDGET, task)
            .await
            .expect("committer hung")
            .expect("committer join");
        assert_refusals_only_ok_or_refused(&outcomes);
        admitted += outcomes.iter().filter(|r| r.is_ok()).count();
    }
    assert!(admitted > 0);

    // Everything acknowledged — before and after V* — settles once the
    // hold is gone, and the after-side commits reached the WAL too.
    repo.drainer().drain_all(&repo.core).await.unwrap();
    assert_eq!(gate.durable_watermark(), gate.last_committed());
    assert!(gate.last_committed() >= v_star);
    assert_eq!(
        wal_horizon(&repo).await,
        gate.last_committed(),
        "the successful after-side commits' WAL entries are all recovered"
    );
    assert!(!repo.drainer().pause_held(), "hold drop released the pause");
}
