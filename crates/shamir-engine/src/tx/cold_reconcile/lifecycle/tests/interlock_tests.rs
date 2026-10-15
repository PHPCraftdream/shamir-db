//! Unit tests for the `PubTermInterlock` publication/termination protocol.

use crate::tx::cold_reconcile::lifecycle::PubTermInterlock;

#[test]
fn publication_round_trip() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.begin_publication(), "second section must be rejected");
    il.end_publication();
    assert!(il.begin_publication());
    il.end_publication();
    assert!(!il.is_terminated());
}

#[test]
fn terminate_from_idle_wins() {
    let il = PubTermInterlock::new();
    assert!(il.terminate());
    assert!(il.terminate(), "idempotent");
    assert!(il.is_terminated());
    assert!(!il.begin_publication());
}

#[test]
fn in_flight_publication_completes_then_terminates() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    // Quarantine while publishing: terminate loses but stays sticky.
    assert!(!il.terminate());
    assert!(il.is_terminated(), "requested flag is sticky");
    // The in-flight section ends; the sticky flag finishes termination.
    il.end_publication();
    assert!(il.is_terminated());
    assert!(!il.begin_publication(), "no publication may sneak in");
}

#[test]
fn no_publication_between_failed_terminate_and_retry() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.terminate());
    // Retry terminate after the failed attempt: still no sneaking.
    assert!(!il.begin_publication());
    il.end_publication();
    assert!(il.terminate());
    assert!(il.is_terminated());
}

#[test]
fn benign_concurrent_publication_does_not_terminate() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    // A second begin while one is in flight is a BENIGN PUBLISHING
    // conflict: it must NOT make `requested` sticky.
    assert!(!il.begin_publication());
    il.end_publication();
    assert!(!il.is_terminated(), "benign conflict must not terminate");
    assert!(il.begin_publication(), "publication stays open");
    il.end_publication();
}

#[test]
fn end_publication_completes_pending_termination() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.terminate(), "publication in flight: terminate loses");
    il.end_publication();
    assert!(il.is_terminated());
    assert!(!il.begin_publication());
}
