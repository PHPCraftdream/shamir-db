//! Unit tests for [`crate::table::in_flight_create_guard::InFlightCreateCounter`]
//! and its RAII guard. See that module's doc for the false-positive this
//! primitive closes (#1003, follow-up to #984).

use crate::table::in_flight_create_guard::InFlightCreateCounter;

#[test]
fn starts_at_zero() {
    let c = InFlightCreateCounter::new();
    assert_eq!(c.current(), 0);
}

#[test]
fn enter_bumps_and_drop_decrements() {
    let c = InFlightCreateCounter::new();
    let g1 = c.enter();
    assert_eq!(c.current(), 1);
    let g2 = c.enter();
    assert_eq!(c.current(), 2);
    drop(g1);
    assert_eq!(c.current(), 1);
    drop(g2);
    assert_eq!(c.current(), 0);
}

#[test]
fn clone_shares_the_same_counter() {
    let a = InFlightCreateCounter::new();
    let b = a.clone();
    let _g = a.enter();
    assert_eq!(b.current(), 1, "clone must share the same Arc<AtomicU64>");
}
