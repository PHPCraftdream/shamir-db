//! Cancellation and refusal while requests wait for visibility, plus the
//! R4b step 3 cancel-vs-commit race for interactive transactions.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::session::Session;
use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::{doc, write, Query};
use shamir_query_types::admin::activity::{
    ActivityRequestState, ActivityRow, ActivityTxState, CancelOutcome,
};
use shamir_query_types::batch::{BatchLimits, VisibilityWait};
use shamir_server::config::TransactionsConfig;
use shamir_server::db_handler::DbResponse;

use super::cancel_fixture::{acceptance, acceptance_multi_thread, Fixture};
use super::common::{
    execute, tx_begin_req, tx_commit_req, tx_insert_req, tx_rollback_req, DB, REPO, TABLE,
};

const STATUS_PACE: Duration = Duration::from_millis(20);

/// The ONLY cancel/commit outcome pairs the two parties can produce.
///
/// The cancel (`CancelTx`) decides once, against the authoritative `open`
/// map, and the commit (`TxCommit`) is the only other party:
/// * `Cancelled|tx_not_found` — the cancel removed the handle before the
///   commit's `begin_commit` reserved it;
/// * `AlreadyCommitting|committed` — the commit reserved the handle
///   (`begin_commit`) before the cancel looked;
/// * `AlreadyFinished|committed` — the commit guard already dropped before
///   the cancel looked;
/// * `NotCancellable|committed` — the cancel's NON-BLOCKING attempt on the
///   transaction's context lock lost to the commit path, which momentarily
///   holds that lock (`tx_commit_with_options` reads the staged bytes under
///   `it.ctx().lock().await` before `begin_commit`, and later `take()`s the
///   context); nothing was cancelled and the commit reports normally.
///
/// `Cancelled|committed` is impossible (the removal precedes the commit's
/// lookup), and `AlreadyFinished|tx_not_found` is NOT allowed either: with no
/// third remover (no reaper hit, no rollback) `AlreadyFinished` can only
/// describe a commit that completed, so a `tx_not_found` from that commit
/// means the cancellation outcome was LOST between the two rows — the exact
/// mutant this test must fail on.
const ALLOWED_RACE_PAIRS: [&str; 4] = [
    "Cancelled|tx_not_found",
    "AlreadyCommitting|committed",
    "AlreadyFinished|committed",
    "NotCancellable|committed",
];

async fn begin_tx(fixture: &Fixture, owner: &Session) -> u64 {
    match fixture.send(owner, &tx_begin_req()).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("interactive tx begin: {other:?}"),
    }
}

async fn read_values(fixture: &Fixture, session: &Session) -> Vec<i64> {
    let mut read = Batch::new();
    read.query("all", Query::with_repo(REPO, TABLE));
    let response = fixture.send(session, &execute(read.build())).await;
    let DbResponse::Batch { response } = response else {
        panic!("read rows: {response:?}");
    };
    response.results["all"]
        .records
        .iter()
        .map(|record| record.get_value_i64("n").expect("seed value"))
        .collect()
}

#[test]
fn cancel_wakes_as_of_visibility_wait() {
    acceptance(
        "cancel_wakes_as_of_visibility_wait",
        |guard, mut fixture| async move {
            // Keep both request deadlines beyond the fixture watchdog, so only
            // cancellation can complete the read while the gap is held.
            let handler = fixture
                .handler
                .as_ref()
                .clone()
                .with_transactions(TransactionsConfig {
                    read_visibility_timeout_ms: 150_000,
                    ..TransactionsConfig::default()
                });
            fixture.handler = Arc::new(handler);
            let db = fixture.handler.db().get_db(DB).expect("fixture db");
            db.get_table(REPO, TABLE)
                .await
                .expect("open table before holding a gap");
            let repo = db.get_repo(REPO).expect("fixture repo");
            let gate = repo.tx_gate().await.expect("gate");
            let visible = gate.visible_floor();
            let gap = gate.assign_next_version_guarded();
            assert!(gap.version() > visible);
            assert_eq!(gate.completion().waiter_count(), 0);

            let target = fixture.user("reader", 1);
            let mut batch = Batch::new();
            batch.limits(BatchLimits {
                max_execution_time_secs: 150,
                ..BatchLimits::default()
            });
            batch.query(
                "read",
                Query::with_repo(REPO, TABLE).as_of_version(gap.version()),
            );
            let mut pending = fixture.start(Arc::clone(&target), execute(batch.build()));
            let id = fixture
                .waiting(
                    &mut pending,
                    &target,
                    ActivityRequestState::WaitingVisibility,
                    || gate.completion().waiter_count() == 1,
                )
                .await;
            guard.mark("as-of read waiting on an unsettled version");

            fixture
                .expect_cancel(&fixture.admin, id, CancelOutcome::Cancelled)
                .await;
            let response = pending.await.expect("target connection joined");
            assert!(
                matches!(&response, DbResponse::Error { code, .. } if code == "cancelled"),
                "expected cancelled wire error, got {response:?}"
            );
            assert_eq!(
                gate.completion().waiter_count(),
                0,
                "cancel leaked a completion waiter"
            );
            assert_eq!(
                gate.visible_floor(),
                visible,
                "the gap must still block visibility"
            );
            fixture.assert_retired(&target, id).await;
            drop(gap);
        },
    );
}

#[test]
fn durable_phase_commit_refusal() {
    acceptance(
        "durable_phase_commit_refusal",
        |guard, fixture| async move {
            let db = fixture.handler.db().get_db(DB).expect("fixture db");
            db.get_table(REPO, TABLE)
                .await
                .expect("open table before holding a gap");
            let repo = db.get_repo(REPO).expect("fixture repo");
            let gate = repo.tx_gate().await.expect("gate");
            let visible = gate.visible_floor();
            let gap = gate.assign_next_version_guarded();
            let expected_version = gap.version() + 1;
            let target = fixture.user("writer", 1);
            let mut batch = Batch::new();
            batch
                .transactional()
                .durability(Durability::Synced)
                .visibility_wait(VisibilityWait::Strict);
            batch.insert(
                "written",
                write::Insert::with_repo(REPO, TABLE).row(doc! { "n" => 1_i64 }),
            );
            let request = execute(batch.build());
            let mut pending = fixture.start(Arc::clone(&target), request);
            let id = fixture
                .waiting(
                    &mut pending,
                    &target,
                    ActivityRequestState::Committing,
                    || {
                        // The only registered waiter is this strict acknowledgement. It
                        // can register only after the inner commit returns a known outcome.
                        gate.completion().waiter_count() == 1
                    },
                )
                .await;
            guard.mark("known commit awaiting strict visibility");
            assert_eq!(
                gate.visible_floor(),
                visible,
                "the lower gap still blocks acknowledgement"
            );
            let wal = repo.repo_wal().await.expect("persistent WAL");
            wal.sync_wal()
                .await
                .expect("fsync the newer WAL frame before refusing cancel");
            assert!(
                wal.recover()
                    .await
                    .expect("read WAL")
                    .iter()
                    .any(|entry| entry.commit_version == expected_version),
                "the newer synced write must already be in the WAL"
            );
            fixture
                .expect_cancel(&fixture.admin, id, CancelOutcome::AlreadyCommitting)
                .await;
            assert_eq!(gate.visible_floor(), visible);
            assert!(
                !pending.is_finished(),
                "cancel refusal must leave the strict acknowledgement waiting"
            );
            // Dropping an unwritten guard settles it as aborted, including durable
            // coverage, and lets the real commit advance through the gap.
            drop(gap);
            let response = pending.await.expect("commit connection joined");
            let DbResponse::Batch { response } = response else {
                panic!("commit must finish normally: {response:?}");
            };
            assert!(response.results.contains_key("written"));
            let outcome = response.transaction.expect("transaction outcome");
            assert_eq!(outcome.commit_version, Some(expected_version));
            assert!(outcome.is_committed());
            assert!(outcome.is_visible());
            fixture.assert_retired(&target, id).await;
        },
    );
}

#[test]
fn cancel_tx_vs_commit_race() {
    acceptance_multi_thread("cancel_tx_vs_commit_race", |guard, fixture| async move {
        let db = fixture.handler.db().get_db(DB).expect("fixture db");
        db.get_table(REPO, TABLE)
            .await
            .expect("open table before holding a gap");
        let repo = db.get_repo(REPO).expect("fixture repo");
        let gate = repo.tx_gate().await.expect("gate");

        // ---- Phase 1: commit wins deterministically -------------------
        let winner = fixture.user("commit_wins", 0x11);
        let handle = begin_tx(&fixture, &winner).await;
        assert!(matches!(
            fixture.send(&winner, &tx_insert_req(handle, 11)).await,
            DbResponse::TxBatch { .. }
        ));
        let gap = gate.assign_next_version_guarded();
        let mut commit = fixture.start(
            Arc::clone(&winner),
            tx_commit_req(handle, Some(VisibilityWait::Strict)),
        );
        let awaiting = async {
            loop {
                let observed = fixture.transactions().await.into_iter().any(|row| {
                    matches!(
                        row,
                        ActivityRow::Transaction { tx_handle, state, .. }
                            if tx_handle == handle && state == ActivityTxState::AwaitingCommit
                    )
                });
                if observed {
                    break;
                }
                tokio::time::sleep(STATUS_PACE).await;
            }
        };
        tokio::select! {
            biased;
            early = &mut commit => panic!("commit finished before awaiting-commit: {early:?}"),
            _ = awaiting => {}
        }
        guard.mark("commit parked at awaiting-commit behind a version gap");
        fixture
            .expect_tx_cancel(&fixture.admin, handle, CancelOutcome::AlreadyCommitting)
            .await;
        guard.mark("cancel refused while commit awaits visibility");
        drop(gap);
        let committed = commit.await.expect("commit connection joined");

        let DbResponse::TxCommitted { transaction } = committed else {
            panic!("a refused cancel must let the commit finish: {committed:?}");
        };
        assert!(
            transaction.is_committed(),
            "the commit must win the race: {transaction:?}"
        );
        assert!(
            transaction.commit_version.is_some(),
            "a committed transaction reports its commit version: {transaction:?}"
        );
        assert_eq!(fixture.handler.tx_registry().len(), 0);
        assert!(
            !fixture.transactions().await.iter().any(|row| matches!(
                row,
                ActivityRow::Transaction { tx_handle, .. } if *tx_handle == handle
            )),
            "the committed handle must leave the activity registry"
        );
        assert!(
            gate.active_snapshots_empty(),
            "commit must release the transaction snapshot"
        );
        guard.mark("commit won and the handle is gone");

        // ---- Phase 2: cancel wins deterministically -------------------
        let cancelled_owner = fixture.user("cancel_wins", 0x12);
        let handle = begin_tx(&fixture, &cancelled_owner).await;
        assert!(matches!(
            fixture
                .send(&cancelled_owner, &tx_insert_req(handle, 12))
                .await,
            DbResponse::TxBatch { .. }
        ));
        fixture
            .expect_tx_cancel(&fixture.admin, handle, CancelOutcome::Cancelled)
            .await;
        guard.mark("cancel won before commit started");
        let response = fixture
            .send(&cancelled_owner, &tx_commit_req(handle, None))
            .await;
        assert!(
            matches!(&response, DbResponse::Error { code, .. } if code == "tx_not_found"),
            "commit after a winning cancel: {response:?}"
        );
        let reader = fixture.user("phase2_reader", 0x13);
        assert!(
            !read_values(&fixture, &reader).await.contains(&12),
            "the cancelled transaction's staged write must stay invisible"
        );
        assert_eq!(fixture.handler.tx_registry().len(), 0);
        assert!(gate.active_snapshots_empty());
        guard.mark("cancelled staged write is invisible");

        // ---- Phase 3: concurrent loop (10 iterations) -----------------
        // Runs on `acceptance_multi_thread`: the commit is a SPAWNED task
        // and the cancel runs on this task, so with two worker threads the
        // two sides are genuinely concurrent (a current-thread runtime
        // would serialize them and never exercise the race).
        let observer = fixture.user("race_observer", 0x14);
        let mut observed: BTreeSet<String> = BTreeSet::new();
        guard.mark("concurrent cancel-vs-commit loop starting");
        for iteration in 0..10u64 {
            let value = 1000 + iteration as i64;
            let owner = fixture.user("racer", 0x20 + iteration as u8);
            let handle = begin_tx(&fixture, &owner).await;
            assert!(
                matches!(
                    fixture.send(&owner, &tx_insert_req(handle, value)).await,
                    DbResponse::TxBatch { .. }
                ),
                "iteration {iteration}: stage the write"
            );

            // Spawn the commit first, then run the cancel on this task:
            // both sides are live at the same time on the multi-thread
            // runtime, and joining only afterwards keeps that overlap.
            let commit = fixture.start(
                Arc::clone(&owner),
                tx_commit_req(handle, Some(VisibilityWait::Strict)),
            );
            let cancel_response = fixture.cancel_tx(&fixture.admin, handle).await;
            let commit_join = commit.await;

            let cancel_outcome = match cancel_response {
                DbResponse::CancelOutcome { outcome } => outcome,
                other => panic!("iteration {iteration}: cancel: {other:?}"),
            };
            let (commit_completed, commit_label) = match commit_join
                .expect("commit connection joined")
            {
                DbResponse::TxCommitted { transaction } => {
                    assert!(
                        transaction.is_committed(),
                        "iteration {iteration}: {transaction:?}"
                    );
                    assert!(
                        transaction.commit_version.is_some(),
                        "iteration {iteration}: committed without a version: {transaction:?}"
                    );
                    (true, "committed")
                }
                DbResponse::Error { code, .. } if code == "tx_not_found" => (false, "tx_not_found"),
                other => panic!("iteration {iteration}: commit: {other:?}"),
            };

            let label = format!("{cancel_outcome:?}|{commit_label}");
            assert!(
                    ALLOWED_RACE_PAIRS.contains(&label.as_str()),
                    "iteration {iteration}: cancel/commit outcome pair {label} is outside the allowed set"
                );
            observed.insert(label.clone());
            assert_eq!(
                fixture.handler.tx_registry().len(),
                0,
                "iteration {iteration}: registry must drain"
            );
            assert!(
                gate.active_snapshots_empty(),
                "iteration {iteration}: snapshots must drain"
            );
            let values = read_values(&fixture, &observer).await;
            assert_eq!(
                    values.contains(&value),
                    commit_completed,
                    "iteration {iteration}: staged value visibility must match the commit outcome ({label})"
                );
            guard.mark(format!("race iteration {iteration}: {label}"));
        }
        // The SET of observed pairs (never the interleaving) must sit
        // inside the legitimate pairs, and it must not be empty. Which
        // side wins an individual iteration is the scheduler's decision,
        // so requiring `Cancelled|tx_not_found` HERE would be a coin
        // flip; that pair's reachability is proven deterministically by
        // Phase 2 (a completed cancel, then a commit that answers
        // `tx_not_found`) and by Phase 1 for `AlreadyCommitting|committed`.
        assert!(
            !observed.is_empty(),
            "the concurrent loop must observe at least one cancel/commit pair"
        );
        assert!(
            observed
                .iter()
                .all(|label| ALLOWED_RACE_PAIRS.contains(&label.as_str())),
            "observed cancel/commit outcome pairs: {observed:?}"
        );
    });
}

/// A `Committing` reservation parked DIRECTLY in the registry — no commit
/// pipeline, no version gap, no polling, no sleep — proves that the
/// `already_committing` refusal holds for the whole reservation window, that
/// the `TxCommitGuard`'s drop is the single teardown, and that a refused
/// cancel never half-cancels the transaction.
#[test]
fn cancel_tx_refuses_parked_committing() {
    acceptance(
        "cancel_tx_refuses_parked_committing",
        |guard, fixture| async move {
            let db = fixture.handler.db().get_db(DB).expect("fixture db");
            db.get_table(REPO, TABLE)
                .await
                .expect("open table before parking a reservation");
            let repo = db.get_repo(REPO).expect("fixture repo");
            let gate = repo.tx_gate().await.expect("gate");

            // ---- Park a realistic interactive transaction ----------------
            let owner = fixture.user("parked_committing", 0x31);
            let handle = begin_tx(&fixture, &owner).await;
            assert!(matches!(
                fixture.send(&owner, &tx_insert_req(handle, 31)).await,
                DbResponse::TxBatch { .. }
            ));
            let registry = fixture.handler.tx_registry();
            let tx = registry
                .get_owned(handle, &owner.session_id)
                .expect("the freshly opened handle is parked and executable");
            assert!(
                !gate.active_snapshots_empty(),
                "the parked transaction must pin its snapshot"
            );

            // ---- Park the commit reservation directly --------------------
            // `begin_commit` moves the identity to `Committing` under the
            // `open` entry's own shard guard: the handle is still RESERVED
            // (no re-register, no reap, no executable lookup) and its
            // projection row stays visible, until the guard drops.
            let reservation = registry
                .begin_commit(handle, &tx)
                .expect("the identity-verified handle transitions to Committing");
            guard.mark("Committing reservation parked");
            assert_eq!(registry.len(), 0, "a Committing row leaves the open gauge");
            assert_eq!(
                registry.activity_count(),
                1,
                "the projection row stays visible while the commit is in flight"
            );

            // ---- Cancel is refused for the whole window ------------------
            fixture
                .expect_tx_cancel(&fixture.admin, handle, CancelOutcome::AlreadyCommitting)
                .await;
            fixture
                .expect_tx_cancel(&fixture.admin, handle, CancelOutcome::AlreadyCommitting)
                .await;
            guard.mark("cancel refused already_committing while parked");

            // ---- The guard's drop is the single teardown -----------------
            drop(reservation);
            assert_eq!(registry.len(), 0, "the open reservation must be gone");
            assert_eq!(
                registry.activity_count(),
                0,
                "the projection row must be gone"
            );
            assert!(
                registry.activity_row(handle).is_none(),
                "the reservation row must be removed with the open entry"
            );
            assert!(
                !fixture.transactions().await.iter().any(|row| matches!(
                    row,
                    ActivityRow::Transaction { tx_handle, .. } if *tx_handle == handle
                )),
                "the finished handle must leave the activity registry"
            );
            fixture
                .expect_tx_cancel(&fixture.admin, handle, CancelOutcome::AlreadyFinished)
                .await;
            guard.mark("guard drop drained the reservation; cancel answers already_finished");

            // ---- Not half-cancelled: only the handle is closed -----------
            // The refused cancel never took the parked context, and the
            // closed handle answers the typed not-found on both terminal
            // requests (the entry is gone, so `get_owned` refuses first).
            assert!(
                tx.ctx().lock().await.is_some(),
                "a refused cancel must leave the parked context untouched"
            );
            for request in [tx_commit_req(handle, None), tx_rollback_req(handle)] {
                let response = fixture.send(&owner, &request).await;
                assert!(
                    matches!(&response, DbResponse::Error { code, .. } if code == "tx_not_found"),
                    "a closed handle must answer the typed not-found: {response:?}"
                );
            }
            assert!(
                !gate.active_snapshots_empty(),
                "the commit guard must not release the transaction's snapshot"
            );
            drop(tx);
            assert!(
                gate.active_snapshots_empty(),
                "dropping the last owner must release the transaction snapshot"
            );
            guard.mark("closed handle released its snapshot");
        },
    );
}
