//! Cancellation and refusal while requests wait for visibility.

use std::sync::Arc;

use shamir_query_builder::batch::{Batch, Durability};
use shamir_query_builder::{doc, write, Query};
use shamir_query_types::admin::activity::{ActivityRequestState, CancelOutcome};
use shamir_query_types::batch::{BatchLimits, VisibilityWait};
use shamir_server::config::TransactionsConfig;
use shamir_server::db_handler::DbResponse;

use super::cancel_fixture::acceptance;
use super::common::{execute, DB, REPO, TABLE};

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
