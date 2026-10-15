//! Table-membership mutations are gated by the repository write-admission domain.

use crate::db_instance::db_instance::DbInstance;
use crate::repo::{BoxRepoFactory, RepoConfig, RepoWriteDomain};
use crate::table::TableConfig;
use std::future::Future;
use std::task::{Context, Poll};

fn assert_pending<F: Future>(mut future: std::pin::Pin<&mut F>) {
    let waker = futures::task::noop_waker_ref();
    let mut context = Context::from_waker(waker);
    assert!(
        matches!(future.as_mut().poll(&mut context), Poll::Pending),
        "mutation completed while admission was blocked"
    );
}

async fn db_with_repo() -> DbInstance {
    let config =
        RepoConfig::new("r", BoxRepoFactory::in_memory()).add_table(TableConfig::new("seeded"));
    DbInstance::with_repos(vec![config]).await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn create_table_blocked_while_permit_held() {
    let db = db_with_repo().await;
    let repo = db.get_repo("r").unwrap();
    let permit = repo.write_domain().admit().await.unwrap();
    let mut mutation = Box::pin(db.create_table("r", "t2"));
    assert_pending(mutation.as_mut());
    assert!(!db.has_table("r", "t2"));
    assert!(!repo.has_table("t2"));
    drop(permit);
    mutation.await.unwrap();
    assert!(db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread")]
async fn drop_table_blocked_while_permit_held() {
    let db = db_with_repo().await;
    db.create_table("r", "t2").await.unwrap();
    let repo = db.get_repo("r").unwrap();
    let permit = repo.write_domain().admit().await.unwrap();
    let mut mutation = Box::pin(db.drop_table("r", "t2"));
    assert_pending(mutation.as_mut());
    assert!(db.has_table("r", "t2"));
    drop(permit);
    assert!(mutation.await.unwrap());
    assert!(!db.has_table("r", "t2"));
}

#[tokio::test(flavor = "current_thread")]
async fn membership_rejected_while_reservation_outstanding() {
    let db = db_with_repo().await;
    let repo = db.get_repo("r").unwrap();
    let reservation = repo
        .write_domain()
        .try_place_reservation()
        .expect("reservation slot free");
    let err = db.create_table("r", "t2").await.unwrap_err();
    match &err {
        shamir_storage::error::DbError::Internal(msg) => assert!(
            msg.contains("reservation"),
            "expected Reserved rejection, got: {msg}"
        ),
        other => panic!("expected Internal(Reserved), got: {other:?}"),
    }
    assert!(!db.has_table("r", "t2"));
    drop(reservation);
    db.create_table("r", "t2").await.unwrap();
    assert!(db.has_table("r", "t2"));
}

#[tokio::test]
async fn admit_pair_orders_and_rejects_same() {
    let a = RepoWriteDomain::new("a");
    let b = RepoWriteDomain::new("b");
    let (pa, pb) = RepoWriteDomain::admit_pair(&a, &b).await.unwrap();
    assert_eq!(pa.domain_token(), a.token());
    assert_eq!(pb.domain_token(), b.token());
    drop((pa, pb));
    let (pb, pa) = RepoWriteDomain::admit_pair(&b, &a).await.unwrap();
    assert_eq!(pb.domain_token(), b.token());
    assert_eq!(pa.domain_token(), a.token());
    drop((pb, pa));
    assert!(
        matches!(RepoWriteDomain::admit_pair(&a, &a).await, Err(crate::repo::RepoWriteError::SameRepository { repo }) if repo == "a")
    );

    let hold_a = a.admit().await.unwrap();
    let hold_b = b.admit().await.unwrap();
    let (a1, b1, a2, b2) = (
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
        std::sync::Arc::clone(&a),
        std::sync::Arc::clone(&b),
    );
    let mut t1 = Box::pin(async move {
        let (pa, pb) = RepoWriteDomain::admit_pair(&a1, &b1).await.unwrap();
        assert_eq!(pa.domain_token(), a1.token());
        assert_eq!(pb.domain_token(), b1.token());
        drop((pa, pb));
    });
    let mut t2 = Box::pin(async move {
        let (pb, pa) = RepoWriteDomain::admit_pair(&b2, &a2).await.unwrap();
        assert_eq!(pb.domain_token(), b2.token());
        assert_eq!(pa.domain_token(), a2.token());
        drop((pb, pa));
    });
    assert_pending(t1.as_mut());
    assert_pending(t2.as_mut());
    drop((hold_a, hold_b));
    t1.await;
    t2.await;
}
