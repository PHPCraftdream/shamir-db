//! End-to-end lock wakes, rollback, and permissions for CancelRequest.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use shamir_connect::server::session::Session;
use shamir_db::storage::types::RecordKey;
use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::{doc, filter, val, write, Query};
use shamir_query_types::admin::activity::{ActivityRequestState, CancelOutcome};
use shamir_server::db_handler::{DbRequest, DbResponse};
use shamir_test_diag::FlakeGuard;
use shamir_tx::{LockMode, MvccStore, RepoTxGate};
use shamir_types::types::record_id::RecordId;
use tokio::sync::Notify;

use super::cancel_fixture::{acceptance, Fixture};
use super::common::{execute, insert_req, tx_begin_req, DB, REPO, TABLE};

async fn storage(fixture: &Fixture) -> (Arc<RepoTxGate>, Arc<MvccStore>) {
    let db = fixture.handler.db().get_db(DB).expect("fixture db");
    let table = db.get_table(REPO, TABLE).await.expect("open table");
    let repo = db.get_repo(REPO).expect("fixture repo");
    let gate = repo.tx_gate().await.expect("gate");
    (gate, table.mvcc_store_for_test().expect("MVCC table"))
}

async fn seed(fixture: &Fixture, n: i64) -> RecordKey {
    let response = fixture.send(&fixture.admin, &insert_req(n)).await;
    let DbResponse::Batch { response } = response else {
        panic!("seed existing record: {response:?}");
    };
    let record = &response.results["i"].records[0];
    let id: RecordId = record
        .get_value_str("_id")
        .expect("inserted id")
        .parse()
        .expect("RecordId");
    RecordKey::from_slice(id.as_bytes())
}

async fn hold(mvcc: &MvccStore, gate: &RepoTxGate, keys: &[RecordKey]) -> u64 {
    let older = gate.fresh_tx_id().raw();
    for key in keys {
        mvcc.lock_key(
            key.clone(),
            older,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
            LockMode::Exclusive,
        )
        .await
        .expect("older transaction owns the seeded key");
    }
    older
}

fn update(n: i64) -> shamir_query_types::write::UpdateOp {
    write::Update::with_repo(REPO, TABLE)
        .where_(filter::eq("n", n))
        .set(doc! { "n" => -n })
        .build()
        .expect("build fixture update")
}

fn blocked_update(n: i64, iterate: bool) -> DbRequest {
    let mut batch = Batch::new();
    batch.transactional().isolation(Isolation::Pessimistic);
    if iterate {
        let mut body = Batch::new();
        // Stage a write on a different, unblocked existing key before
        // this iteration attempts to update the held key.
        let staged = body.update("staged", update(100 + n));
        body.update_after("blocked", update(n), &[&staged]);
        batch.for_each("loop", vec![val::lit(1)], "row", body.build());
    } else {
        batch.update("blocked", update(n));
    }
    execute(batch.build())
}

fn assert_cancelled(response: DbResponse) {
    assert!(
        matches!(&response, DbResponse::Error { code, .. } if code == "cancelled"),
        "expected top-level cancelled wire error, got {response:?}"
    );
}

#[test]
fn cancel_wakes_parked_lock_wait() {
    acceptance(
        "cancel_wakes_parked_lock_wait",
        |guard, fixture| async move {
            let (gate, mvcc) = storage(&fixture).await;
            let keys = [seed(&fixture, 1).await];
            assert!(gate.active_snapshots_empty());
            let before = gate.version_allocation_high_water_mark();
            let older = hold(&mvcc, &gate, &keys).await;
            let target = fixture.user("writer", 1);
            let request = blocked_update(1, false);
            let mut pending = fixture.start(Arc::clone(&target), request);
            let id = fixture
                .waiting(
                    &mut pending,
                    &target,
                    ActivityRequestState::WaitingLock,
                    || true,
                )
                .await;
            guard.mark("parked under older record lock");
            assert!(
                !gate.active_snapshots_empty(),
                "parked transaction pins a snapshot"
            );
            fixture
                .expect_cancel(&fixture.admin, id, CancelOutcome::Cancelled)
                .await;
            // No blocker release, wound, or dropped execution future can wake this request.
            assert_cancelled(pending.await.expect("target connection joined"));
            assert_eq!(mvcc.locks_len(), 1, "only the older holder remains");
            assert!(
                gate.active_snapshots_empty(),
                "cancelled transaction unpinned its snapshot"
            );
            assert_eq!(
                gate.version_allocation_high_water_mark(),
                before,
                "cancelled work must not enter commit"
            );
            fixture.assert_retired(&target, id).await;
            mvcc.release_locks(older, &keys).await;
            assert_eq!(mvcc.locks_len(), 0);
        },
    );
}

#[test]
fn cancel_releases_all_under_concurrency() {
    acceptance(
        "cancel_releases_all_under_concurrency",
        |guard, fixture| async move {
            let (gate, mvcc) = storage(&fixture).await;
            let keys = [seed(&fixture, 1).await, seed(&fixture, 2).await];
            seed(&fixture, 101).await;
            seed(&fixture, 102).await;
            assert!(gate.active_snapshots_empty());
            assert!(fixture.handler.tx_registry().is_empty());
            let before = gate.version_allocation_high_water_mark();
            let older = hold(&mvcc, &gate, &keys).await;
            let first = fixture.user("first", 1);
            let second = fixture.user("second", 2);
            let request_a = blocked_update(1, true);
            let request_b = blocked_update(2, true);
            let mut pending_a = fixture.start(Arc::clone(&first), request_a);
            let mut pending_b = fixture.start(Arc::clone(&second), request_b);
            // The target tasks run independently. The observer reuses its one
            // admission slot for sequential listings of both parked requests.
            let id_a = fixture
                .waiting(
                    &mut pending_a,
                    &first,
                    ActivityRequestState::WaitingLock,
                    || true,
                )
                .await;
            let id_b = fixture
                .waiting(
                    &mut pending_b,
                    &second,
                    ActivityRequestState::WaitingLock,
                    || true,
                )
                .await;
            guard.mark("two iterations parked after staging writes");
            assert_ne!(id_a, id_b);
            assert_eq!(
                mvcc.locks_len(),
                4,
                "two staged row locks plus the two blockers"
            );
            assert!(!gate.active_snapshots_empty());
            assert!(first.try_acquire_request_slot(1).is_none());
            assert!(second.try_acquire_request_slot(1).is_none());

            fixture
                .expect_cancel(&fixture.admin, id_a, CancelOutcome::Cancelled)
                .await;
            assert_cancelled(pending_a.await.expect("first connection joined"));
            assert_eq!(
                mvcc.locks_len(),
                3,
                "the unaffected request still owns its staged row lock"
            );
            assert!(
                !gate.active_snapshots_empty(),
                "the other request still pins its snapshot"
            );
            fixture
                .expect_cancel(&fixture.admin, id_b, CancelOutcome::Cancelled)
                .await;
            assert_cancelled(pending_b.await.expect("second connection joined"));
            assert_eq!(mvcc.locks_len(), 2, "only fixture blockers remain");
            assert!(gate.active_snapshots_empty());
            assert_eq!(
                gate.version_allocation_high_water_mark(),
                before,
                "neither cancelled request may enter commit"
            );
            fixture.assert_retired(&first, id_a).await;
            fixture.assert_retired(&second, id_b).await;
            assert!(fixture.handler.tx_registry().is_empty());
            mvcc.release_locks(older, &keys).await;
            assert_eq!(mvcc.locks_len(), 0);

            let mut read = Batch::new();
            read.query("all", Query::with_repo(REPO, TABLE));
            let response = fixture.send(&first, &execute(read.build())).await;
            let DbResponse::Batch { response } = response else {
                panic!("read after rollback: {response:?}");
            };
            let mut values: Vec<_> = response.results["all"]
                .records
                .iter()
                .map(|record| record.get_value_i64("n").expect("seed value"))
                .collect();
            values.sort_unstable();
            assert_eq!(
                values,
                [1, 2, 101, 102],
                "neither iteration may publish its staged update"
            );
            let opened = fixture.send(&second, &tx_begin_req()).await;
            let DbResponse::TxOpened { tx_handle, .. } = opened else {
                panic!("cancelled session cannot reuse its transaction slot: {opened:?}");
            };
            let rollback = DbRequest::TxRollback {
                db: DB.into(),
                tx_handle,
            };
            assert!(
                matches!(fixture.send(&second, &rollback).await, DbResponse::TxRolledBack { tx_handle: got } if got == tx_handle)
            );
            assert!(fixture.handler.tx_registry().is_empty());
            assert!(gate.active_snapshots_empty());
        },
    );
}

#[test]
fn cancelled_code_on_error_path() {
    acceptance(
        "cancelled_code_on_error_path",
        |guard, fixture| async move {
            let target = fixture.user("owner", 1);
            let caller = fixture.user("owner", 2);
            cancel_as_owner(&guard, &fixture, &target, &caller, None).await;
        },
    );
}

#[test]
fn permission_denial_activity_cancel_half() {
    acceptance(
        "permission_denial_activity_cancel_half",
        |guard, fixture| async move {
            let target = fixture.user("alice", 1);
            let caller = fixture.user("alice", 2);
            let foreign = fixture.user("bob", 3);
            cancel_as_owner(&guard, &fixture, &target, &caller, Some(&foreign)).await;
        },
    );
}

async fn cancel_as_owner(
    guard: &FlakeGuard,
    fixture: &Fixture,
    target: &Arc<Session>,
    caller: &Session,
    foreign: Option<&Session>,
) {
    let (gate, mvcc) = storage(fixture).await;
    let keys = [seed(fixture, 1).await];
    let older = hold(&mvcc, &gate, &keys).await;
    let request = blocked_update(1, false);
    let mut pending = fixture.start(Arc::clone(target), request);
    let id = fixture
        .waiting(
            &mut pending,
            target,
            ActivityRequestState::WaitingLock,
            || true,
        )
        .await;
    guard.mark("parked before permission check");
    if let Some(foreign) = foreign {
        let response = fixture.cancel(foreign, id).await;
        assert!(
            matches!(&response, DbResponse::Error { code, .. } if code == "permission_denied"),
            "foreign cancel without Manage: {response:?}"
        );
    }
    // A successful CAS here also proves a denied foreign request did not cancel the target.
    fixture
        .expect_cancel(caller, id, CancelOutcome::Cancelled)
        .await;
    assert_cancelled(pending.await.expect("target connection joined"));
    fixture.assert_retired(target, id).await;
    mvcc.release_locks(older, &keys).await;
}
