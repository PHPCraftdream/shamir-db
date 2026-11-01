use crate::BodyBudget;
use std::sync::Arc;

#[test]
fn reservation_accounting_releases_on_drop_and_enforces_cap() {
    let budget = Arc::new(BodyBudget::new(8));
    let reservation = budget.try_reserve(6).expect("fits");
    assert!(budget.try_reserve(3).is_none());
    drop(reservation);
    assert_eq!(budget.in_use(), 0);
    assert!(budget.try_reserve(8).is_some());
}

#[test]
fn chunk_reservation_grows_incrementally_and_releases_after_growth_failure() {
    let budget = Arc::new(BodyBudget::new(4));
    let mut reservation = budget.try_reserve(2).expect("first chunk");
    assert!(reservation.grow(2));
    assert_eq!(reservation.reserved_bytes(), 4);
    assert_eq!(budget.in_use(), 4);
    assert!(!reservation.grow(1));
    assert_eq!(budget.in_use(), 4);
    drop(reservation);
    assert_eq!(budget.in_use(), 0);
}

#[test]
fn zero_length_reservation_uses_no_budget() {
    let budget = Arc::new(BodyBudget::new(0));
    let reservation = budget.try_reserve(0).expect("zero bytes fit");
    assert_eq!(budget.in_use(), 0);
    drop(reservation);
    assert_eq!(budget.in_use(), 0);
}

#[test]
fn concurrent_reservations_never_exceed_cap() {
    let budget = Arc::new(BodyBudget::new(8));
    let barrier = Arc::new(std::sync::Barrier::new(9));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let budget = budget.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            budget.try_reserve(4)
        }));
    }
    barrier.wait();
    let reservations = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(budget.in_use(), 8);
    assert_eq!(
        reservations
            .iter()
            .filter(|reservation| reservation.is_some())
            .count(),
        2
    );
    drop(reservations);
    assert_eq!(budget.in_use(), 0);
}

#[test]
fn concurrent_growth_accounts_against_one_shared_cap_and_releases_on_failure() {
    let budget = Arc::new(BodyBudget::new(8));
    let reservations = (0..4)
        .map(|_| budget.try_reserve(1).expect("initial reservation"))
        .collect::<Vec<_>>();
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for mut reservation in reservations {
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            let grew = reservation.grow(2);
            (reservation, grew)
        }));
    }
    barrier.wait();
    let mut reservations = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect::<Vec<_>>();
    assert_eq!(reservations.iter().filter(|(_, grew)| *grew).count(), 2);
    assert_eq!(budget.in_use(), 8);
    assert!(reservations
        .iter_mut()
        .all(|(reservation, _)| !reservation.grow(usize::MAX)));
    assert_eq!(budget.in_use(), 8);
    drop(reservations);
    assert_eq!(budget.in_use(), 0);
    assert!(budget.try_reserve(8).is_some());
}
