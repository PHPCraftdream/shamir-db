//! Unit tests for [`crate::table::in_flight_create_guard::InFlightCreateSet`]
//! and its RAII guard. See that module's doc for the false-positive this
//! primitive closes (#1003, follow-up to #984) and for why it tracks
//! identities (interned name ids) rather than a scalar count (a real
//! masking bug an `@oh` review found in the first, scalar-counter attempt).

use crate::table::in_flight_create_guard::InFlightCreateSet;

#[test]
fn starts_empty() {
    let s = InFlightCreateSet::new();
    assert!(!s.contains(1));
}

#[test]
fn enter_marks_present_and_drop_removes() {
    let s = InFlightCreateSet::new();
    let g1 = s.enter(1);
    assert!(s.contains(1));
    assert!(!s.contains(2));
    let g2 = s.enter(2);
    assert!(s.contains(2));
    drop(g1);
    assert!(
        !s.contains(1),
        "dropping id 1's guard must remove only id 1"
    );
    assert!(s.contains(2), "id 2 must still be in flight");
    drop(g2);
    assert!(!s.contains(2));
}

#[test]
fn clone_shares_the_same_set() {
    let a = InFlightCreateSet::new();
    let b = a.clone();
    let _g = a.enter(7);
    assert!(b.contains(7), "clone must share the same underlying set");
}

/// Two concurrent guards for the SAME id (a rare same-name race) must not
/// have the first guard's drop prematurely un-hide the id while the second
/// is still in flight — this is exactly why the set is refcounted rather
/// than a plain set/bool.
#[test]
fn concurrent_same_id_guards_are_refcounted() {
    let s = InFlightCreateSet::new();
    let g1 = s.enter(42);
    let g2 = s.enter(42);
    assert!(s.contains(42));
    drop(g1);
    assert!(
        s.contains(42),
        "id 42 must still be in flight while g2 is alive"
    );
    drop(g2);
    assert!(!s.contains(42));
}
