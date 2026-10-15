//! Tests for the repository write-admission domain.

use std::sync::Arc;
use std::task::Poll;

use crate::repo::changelog_store::write_domain::{RepoWriteDomain, RepoWriteError};

#[tokio::test]
async fn admit_returns_identity_stamped_permit() {
    let domain = RepoWriteDomain::new("alpha");
    let permit_a = domain.admit().await.expect("first admit must succeed");
    drop(permit_a);
    let permit_b = domain.admit().await.expect("second admit must succeed");
    domain.debug_check_permit(&permit_b);

    let other = RepoWriteDomain::new("alpha");
    let permit_other = other.admit().await.expect("other admit must succeed");
    assert_ne!(permit_b.domain_token(), permit_other.domain_token());
}

#[tokio::test]
async fn admit_rejects_while_reservation_outstanding() {
    let domain = RepoWriteDomain::new("beta");
    let reservation = domain
        .try_place_reservation()
        .expect("fresh domain must accept a reservation");
    match domain.admit().await {
        Err(RepoWriteError::Reserved { repo }) => assert_eq!(repo, "beta"),
        Ok(_) => panic!("ordinary admission unexpectedly bypassed reservation"),
        Err(RepoWriteError::SameRepository { .. }) => {
            panic!("unexpected same-repository admission error")
        }
    }
    drop(reservation);
    assert!(
        domain.admit().await.is_ok(),
        "admit after drop must succeed"
    );
}

#[tokio::test]
async fn reservation_holder_can_admit_and_foreign_domain_is_rejected() {
    let domain = RepoWriteDomain::new("gamma");
    let foreign = RepoWriteDomain::new("foreign");
    let reservation = domain
        .try_place_reservation()
        .expect("fresh domain must accept a reservation");

    let permit = domain.admit_with_reservation(&reservation).await.unwrap();
    domain.debug_check_permit(&permit);
    drop(permit);

    let err = foreign
        .admit_with_reservation(&reservation)
        .await
        .unwrap_err();
    assert!(matches!(err, RepoWriteError::Reserved { repo } if repo == "foreign"));
    drop(reservation);
    assert!(domain.admit().await.is_ok());
}

#[tokio::test]
async fn reservation_is_single_slot() {
    let domain = RepoWriteDomain::new("delta");
    let first = domain
        .try_place_reservation()
        .expect("first reservation must succeed");
    assert!(
        domain.try_place_reservation().is_none(),
        "second reservation while first alive must fail"
    );
    drop(first);
    assert!(
        domain.try_place_reservation().is_some(),
        "reservation must be re-placeable after drop"
    );
}

#[tokio::test(start_paused = true)]
async fn admit_serializes() {
    let domain = RepoWriteDomain::new("epsilon");
    let holder = domain.admit().await.expect("holder admit must succeed");
    let task_domain = Arc::clone(&domain);
    let mut mutation = Box::pin(async move { task_domain.admit().await });
    assert_eq!(
        futures::poll!(mutation.as_mut()),
        Poll::Pending,
        "admission must wait while permit held"
    );
    drop(holder);
    let permit = mutation.await.expect("admission must succeed");
    domain.debug_check_permit(&permit);
}

#[tokio::test]
async fn reservation_drop_clears_flag() {
    let domain = RepoWriteDomain::new("zeta");
    let reservation = domain
        .try_place_reservation()
        .expect("fresh domain must accept a reservation");
    assert!(domain.has_outstanding_reservation());
    drop(reservation);
    assert!(!domain.has_outstanding_reservation());
}
