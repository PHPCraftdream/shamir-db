//! S5c-X PR6b-2a: table-membership mutations are gated by the repository
//! write-admission domain (`DbInstance::create_table_checked` /
//! `drop_table_checked` → `RepoInstance::add_table_admitted` /
//! `remove_table_admitted`).
//!
//! Each test runs on a paused current-thread runtime: while a permit (or an
//! outstanding reservation) blocks admission, the blocked task stays
//! `Pending` across a bounded cooperative spin, and the mutation completes
//! only after the gate is released. Before PR6b-2a there was NO admission
//! gate on membership (`add_table` was a plain synchronous registration),
//! so every blocking assertion below fails on the pre-PR code.

use crate::db_instance::db_instance::DbInstance;
use crate::repo::{BoxRepoFactory, RepoConfig, RepoWriteDomain};
use crate::table::TableConfig;

/// Bounded cooperative spin: assert the task has not completed after
/// yielding the scheduler this many times (paused clock, no sleeps).
async fn assert_still_pending<T>(task: &mut tokio::task::JoinHandle<T>) {
    for _ in 0..64 {
        assert!(
            !task.is_finished(),
            "membership mutation completed while the repo permit was held"
        );
        tokio::task::yield_now().await;
    }
}

fn db_with_repo() -> DbInstance {
    let config =
        RepoConfig::new("r", BoxRepoFactory::in_memory()).add_table(TableConfig::new("seeded"));
    DbInstance::with_repos(vec![config]).unwrap()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn create_table_blocked_while_permit_held() {
    let db = db_with_repo();
    let repo = db.get_repo("r").unwrap();

    let permit = repo.write_domain().admit().await.unwrap();
    let mut task = tokio::spawn({
        let db = db.clone();
        async move {
            db.create_table_checked("r", "t2").await.unwrap();
        }
    });
    assert_still_pending(&mut task).await;
    // Membership unchanged while blocked: no config registration happened.
    assert!(!db.has_table("r", "t2"));
    assert!(!repo.has_table("t2"));

    drop(permit);
    task.await.unwrap();
    assert!(db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn drop_table_blocked_while_permit_held() {
    let db = db_with_repo();
    db.create_table_checked("r", "t2").await.unwrap();
    let repo = db.get_repo("r").unwrap();
    assert!(repo.has_table("t2"));

    let permit = repo.write_domain().admit().await.unwrap();
    let mut task = tokio::spawn({
        let db = db.clone();
        async move {
            assert!(db.drop_table_checked("r", "t2").await.unwrap());
        }
    });
    assert_still_pending(&mut task).await;
    // The table is still present while the drop is blocked.
    assert!(db.has_table("r", "t2"));

    drop(permit);
    task.await.unwrap();
    assert!(!db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn membership_rejected_while_reservation_outstanding() {
    let db = db_with_repo();
    let repo = db.get_repo("r").unwrap();

    let reservation = repo
        .write_domain()
        .try_place_reservation()
        .expect("reservation slot free");
    let err = db.create_table_checked("r", "t2").await.unwrap_err();
    match &err {
        shamir_storage::error::DbError::Internal(msg) => {
            assert!(
                msg.contains("reservation"),
                "expected Reserved rejection, got: {msg}"
            );
        }
        other => panic!("expected Internal(Reserved), got: {other:?}"),
    }
    assert!(!db.has_table("r", "t2"));

    drop(reservation);
    db.create_table_checked("r", "t2").await.unwrap();
    assert!(db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn admit_pair_orders_and_rejects_same() {
    let a = RepoWriteDomain::new("a");
    let b = RepoWriteDomain::new("b");

    // Both argument orders complete under a paused clock (no deadlock) when
    // the domains are free.
    let (pa, pb) = RepoWriteDomain::admit_pair(&a, &b).await.unwrap();
    assert_ne!(pa.domain_token(), pb.domain_token());
    drop((pa, pb));
    let _ = RepoWriteDomain::admit_pair(&b, &a).await.unwrap();

    // Identical identities are REJECTED, not collapsed.
    assert!(matches!(
        RepoWriteDomain::admit_pair(&a, &a).await,
        Err(crate::repo::RepoWriteError::SameRepository { repo }) if repo == "a"
    ));

    // Hold one permit of each domain, then run both admission orders
    // concurrently: both stay pending until the holds are released, and both
    // then complete in the shared ascending-token order.
    let hold_a = a.admit().await.unwrap();
    let hold_b = b.admit().await.unwrap();
    // Clone the Arcs so the spawned futures are 'static.
    let (a1, b1, a2, b2) = (
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
    );
    let mut t1 = tokio::spawn(async move { RepoWriteDomain::admit_pair(&a1, &b1).await });
    let mut t2 = tokio::spawn(async move { RepoWriteDomain::admit_pair(&b2, &a2).await });
    assert_still_pending(&mut t1).await;
    assert_still_pending(&mut t2).await;
    drop((hold_a, hold_b));
    let r1 = t1.await.unwrap().unwrap();
    let r2 = t2.await.unwrap().unwrap();
    // Both results are (permit of A, permit of B) regardless of call order.
    assert_eq!(r1.0.domain_token(), a.token());
    assert_eq!(r1.1.domain_token(), b.token());
    assert_eq!(r2.0.domain_token(), a.token());
    assert_eq!(r2.1.domain_token(), b.token());
}
