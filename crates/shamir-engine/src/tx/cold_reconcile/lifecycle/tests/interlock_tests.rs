//! Sequential unit tests for the `PubTermInterlock` single-CAS design.

use crate::tx::cold_reconcile::lifecycle::PubTermInterlock;

/// 1. A publication section opens and closes cleanly; a second section
/// cannot overlap the first.
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

/// 2. Terminating from idle wins the `IDLE -> TERMINATED` CAS and is
/// sticky and idempotent.
#[test]
fn terminate_from_idle_wins() {
    let il = PubTermInterlock::new();
    assert!(il.terminate());
    assert!(il.terminate(), "idempotent");
    assert!(il.is_terminated());
    assert!(!il.begin_publication());
}

/// 3. Termination while a section is in flight force-terminates: the
/// caller sees `false`, the state is `TERMINATED` IMMEDIATELY (no IDLE
/// window for `end_publication` to reopen), and `end_publication` is a
/// no-op.
#[test]
fn terminate_while_publishing_force_terminates() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.terminate(), "in-flight section: terminate lost the CAS");
    assert!(il.is_terminated(), "terminated immediately, not lazily");
    assert!(!il.begin_publication());
    il.end_publication();
    assert!(il.is_terminated(), "end_publication must not reopen");
}

/// 4. Regression: a benign PUBLISHING conflict (two begins) must NOT
/// terminate the interlock — under the old sticky-flag design a rejected
/// begin made termination sticky without any terminate call.
#[test]
fn benign_concurrent_publication_does_not_terminate() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.begin_publication(), "overlapping section rejected");
    il.end_publication();
    assert!(!il.is_terminated(), "benign conflict must not terminate");
    assert!(il.begin_publication(), "publication stays available");
    il.end_publication();
    assert!(!il.is_terminated());
}

/// 5. NEGATIVE CONTROL: after `terminate()` (returned `false` against an
/// in-flight section) a stale contender replaying EXACTLY the begin CAS
/// must LOSE, because the state is `TERMINATED`. Under the old two-flag
/// design this CAS could sneak into an IDLE window and start a
/// publication after termination was requested.
#[test]
fn stale_begin_cas_cannot_start_after_terminate() {
    let il = PubTermInterlock::new();
    assert!(il.begin_publication());
    assert!(!il.terminate());
    assert!(
        !il.stale_begin_cas_for_test(),
        "stale contender must lose against TERMINATED"
    );
    assert!(il.is_terminated());
}
