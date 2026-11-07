use super::super::{BudgetPin, DebitOutcome, LoginBudgetTable, Source};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn source(last: u8) -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::new(
        192,
        0,
        last / 16 + 2,
        last % 16 + 1,
    )))
}

fn churn_source(index: u32) -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::new(
        (index >> 8) as u8,
        index as u8,
        7,
        9,
    )))
}

/// Pins sources with disjoint tier sets until `n` distinct buckets carry pins.
fn pin_n_distinct_buckets(table: &mut LoginBudgetTable, n: usize) -> Vec<BudgetPin> {
    let mut pins = Vec::new();
    let mut index = 0u32;
    while table.pinned_bucket_count() < n {
        let source = churn_source(index);
        index += 1;
        assert_eq!(
            table.debit(source, Instant::now()),
            Ok(DebitOutcome::Charged)
        );
        let pin = table
            .pin(source)
            .expect("helper: every v4 source pins two fresh buckets");
        pins.push(pin);
    }
    assert_eq!(table.pinned_bucket_count(), n);
    pins
}

#[test]
fn pin_counts_track_clones_and_last_release_under_table_churn() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = source(1);
    assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    let pin = table.pin(pinned).expect("pin within budget");
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    let cloned_pin = pin.clone();
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    let mut other_pins = Vec::with_capacity(1535);
    for index in 0u32..1535 {
        let other = churn_source(index);
        assert_eq!(table.debit(other, now), Ok(DebitOutcome::Charged));
        other_pins.push(table.pin(other).expect("pin within budget"));
    }
    assert_eq!(table.occupied_slots(), 3_072);
    assert_eq!(table.pinned_bucket_count(), 3_072);
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    for _ in 0..9 {
        assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    }
    assert!(table.debit(pinned, now).is_err());
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    assert_eq!(
        table.debit(churn_source(3000), now),
        Ok(DebitOutcome::Charged)
    );
    assert!(table.debit(pinned, now).is_err());
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);
    drop(pin);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    assert!(table.debit(pinned, now).is_err());
    drop(cloned_pin);

    // The 1,535 other sources still hold two buckets each.
    assert_eq!(table.pinned_bucket_count(), 3_070);
    drop(other_pins);
    assert_eq!(table.pinned_bucket_count(), 0);
}

#[test]
fn unique_bucket_budget_caps_at_3072_and_refusal_changes_nothing() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pins = pin_n_distinct_buckets(&mut table, 3_072);
    assert_eq!(table.pinned_bucket_count(), 3_072);
    let extra = churn_source(5000);
    assert_eq!(table.debit(extra, now), Ok(DebitOutcome::Charged));
    assert!(table.pin(extra).is_none());
    assert_eq!(table.pinned_bucket_count(), 3_072);
    assert_eq!(table.occupied_slots(), 3_074);
    drop(pins);
    assert_eq!(table.pinned_bucket_count(), 0);
}

#[test]
fn repeated_pins_of_one_bucket_cost_a_single_unit_until_last_release() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = source(1);
    assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    let mut pins = Vec::new();
    for _ in 0..10 {
        pins.push(table.pin(pinned).expect("pin within budget"));
    }
    assert_eq!(table.pinned_bucket_count(), 2);
    assert_eq!(table.resident_pin_count(pinned), [10, 10, 0]);
    for _ in 0..9 {
        drop(pins.pop());
    }
    assert_eq!(table.pinned_bucket_count(), 2);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    drop(pins.pop());
    assert_eq!(table.pinned_bucket_count(), 0);
    assert_eq!(table.resident_pin_count(pinned), [0, 0, 0]);
}

#[test]
fn clone_then_drop_of_original_and_clone_releases_exactly_once() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = source(1);
    assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    let pin = table.pin(pinned).expect("pin within budget");
    let clone = pin.clone();
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);
    assert_eq!(table.pinned_bucket_count(), 2);
    drop(pin);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    assert_eq!(table.pinned_bucket_count(), 2);
    drop(clone);
    assert_eq!(table.resident_pin_count(pinned), [0, 0, 0]);
    assert_eq!(table.pinned_bucket_count(), 0);
}

#[test]
fn at_the_cap_an_already_pinned_bucket_is_admitted_and_a_new_bucket_refused() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pins = pin_n_distinct_buckets(&mut table, 3_072);
    assert!(table.pin(churn_source(0)).is_some());
    assert_eq!(table.pinned_bucket_count(), 3_072);
    let fresh = churn_source(4000);
    assert_eq!(table.debit(fresh, now), Ok(DebitOutcome::Charged));
    assert!(table.pin(fresh).is_none());
    assert_eq!(table.pinned_bucket_count(), 3_072);
    drop(pins);
    assert_eq!(table.pinned_bucket_count(), 0);
}

#[test]
fn multi_bucket_pin_refused_halfway_restores_every_counter_and_marker() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pins = pin_n_distinct_buckets(&mut table, 3_070);
    let halfway = churn_source(3000);
    assert_eq!(table.debit(halfway, now), Ok(DebitOutcome::Charged));
    assert!(table.pin_root_for_test(halfway));
    let second = churn_source(3001);
    assert_eq!(table.debit(second, now), Ok(DebitOutcome::Charged));
    assert!(table.pin_root_for_test(second));
    assert_eq!(table.pinned_bucket_count(), 3_072);
    let before = table.resident_pin_count(halfway);
    assert_eq!(before, [1, 0, 0]);
    assert!(table.pin(halfway).is_none());
    assert_eq!(table.pinned_bucket_count(), 3_072);
    assert_eq!(table.resident_pin_count(halfway), before);
    assert_eq!(table.resident_pin_count(second), [1, 0, 0]);
    drop(pins);
    // `pin_root_for_test` raises two markers without an RAII guard: their two units stay.
    assert_eq!(table.pinned_bucket_count(), 2);
}

#[test]
fn concurrent_pin_drop_stress_stays_within_the_cap_and_ends_empty() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    for index in 0u32..1600 {
        assert_eq!(
            table.debit(churn_source(index), now),
            Ok(DebitOutcome::Charged)
        );
    }
    let max_seen = AtomicUsize::new(0);
    let max_ref = &max_seen;
    let table_ref = &table;
    std::thread::scope(|scope| {
        for thread in 0..8u64 {
            let _handle = scope.spawn(move || {
                let mut state = thread.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                for _ in 0..10_000 {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let source = churn_source(((state >> 33) % 1600) as u32);
                    if let Some(pin) = table_ref.pin(source) {
                        max_ref.fetch_max(table_ref.pinned_bucket_count(), Ordering::Relaxed);
                        drop(pin);
                    }
                }
            });
        }
    });
    assert_eq!(table.pinned_bucket_count(), 0);
    let max = max_seen.load(Ordering::Acquire);
    assert!(max > 0);
    assert!(max <= 3_072);
}
