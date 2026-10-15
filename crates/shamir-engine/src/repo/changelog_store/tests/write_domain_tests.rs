//! Tests for the repository write-admission domain (S5c-X PR6b-2a).

use std::sync::Arc;

use crate::repo::changelog_store::write_domain::{RepoWriteDomain, RepoWriteError};
use std::task::Poll;

#[tokio::test]
async fn admit_returns_identity_stamped_permit() {
    let domain = RepoWriteDomain::new("alpha");
    let permit_a = domain.admit().await.expect("first admit must succeed");
    drop(permit_a);
    let permit_b = domain.admit().await.expect("second admit must succeed");
    domain.debug_check_permit(&permit_b);

    let other = RepoWriteDomain::new("alpha");
    let permit_other = other.admit().await.expect("other admit must succeed");
    assert_ne!(
        permit_b.domain_token(),
        permit_other.domain_token(),
        "permits from distinct domain instances must carry distinct tokens"
    );
}

#[tokio::test]
async fn admit_rejects_while_reservation_outstanding() {
    let domain = RepoWriteDomain::new("beta");
    let reservation = domain
        .try_place_reservation()
        .expect("fresh domain must accept a reservation");

    match domain.admit().await {
        Err(RepoWriteError::Reserved { repo }) => assert_eq!(repo, "beta"),
        other => panic!("expected Reserved error, got {other:?}"),
    }

    drop(reservation);
    assert!(
        domain.admit().await.is_ok(),
        "admit after drop must succeed"
    );
}

#[tokio::test]
async fn admit_for_close_ignores_reservation() {
    let domain = RepoWriteDomain::new("gamma");
    let _reservation = domain
        .try_place_reservation()
        .expect("fresh domain must accept a reservation");

    let permit = domain.admit_for_close().await;
    domain.debug_check_permit(&permit);
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

/// Deterministic serialization check: no real sleeps. With
/// `start_paused = true`, the spawned task polls to completion only once the
/// held permit is dropped; while held, `yield_now` loops hand control back
/// with the task still stuck on the mutex (auto-advance fires no timers, and
/// the mutex waker only exists after drop).
#[tokio::test(start_paused = true)]
async fn admit_serializes() {
    let domain = RepoWriteDomain::new("epsilon");
    let holder = domain.admit().await.expect("holder admit must succeed");

    let task_domain = Arc::clone(&domain);
    let mut task = tokio::spawn(async move { task_domain.admit().await.is_ok() });

    // Bound the starved phase: poll the spawned task directly. A broken
    // (non-serializing) domain would have let it finish; while the permit is
    // held the task stays Pending on the mutex. Polling a `JoinHandle` and
    // later awaiting it is allowed.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        futures::poll!(&mut task),
        Poll::Pending,
        "admitted task must NOT complete while the permit is held"
    );

    drop(holder);
    assert!(
        task.await.expect("task must not panic"),
        "admitted task must succeed"
    );
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
