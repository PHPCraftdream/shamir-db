use super::super::{DebitOutcome, LoginBudgetTable, Source};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

fn v4(last: u8) -> Source {
    Source::from_ip(IpAddr::V4(Ipv4Addr::new(
        192,
        0,
        last / 16 + 2,
        last % 16 + 1,
    )))
}
fn v6(child: u16) -> Source {
    Source::from_ip(IpAddr::V6(Ipv6Addr::new(
        0x2001, 0xdb8, 0, child, 0, 0, 0, 1,
    )))
}

#[test]
fn full_table_eviction_scan_has_linear_slot_visit_bound() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    for i in 0u32..2048 {
        let source = Source::from_ip(IpAddr::V4(Ipv4Addr::new((i >> 8) as u8, i as u8, 7, 9)));
        assert_eq!(table.debit(source, now), Ok(DebitOutcome::Charged));
    }
    assert_eq!(table.occupied_slots(), 4096);
    let source = Source::from_ip(IpAddr::V4(Ipv4Addr::new(8, 0, 7, 9)));
    table.reset_slot_visits();
    assert_eq!(table.debit(source, now), Ok(DebitOutcome::Charged));
    let visits = table.slot_visits();
    assert!(visits > 0);
    assert!(visits <= 20 * 4096);
}

#[test]
fn source_debits_distinct_tiers_and_enforces_prefix_capacity() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(1));
    let source = v4(17);
    let now = Instant::now();
    for _ in 0..10 {
        assert_eq!(table.debit(source, now), Ok(DebitOutcome::Charged));
    }
    assert!(table.debit(source, now).is_err());
}
#[test]
fn public_debit_oracle_survives_unique_prefix_churn_and_preserves_pinned_source() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = v4(1);
    for _ in 0..10 {
        assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    }
    let pin = table.pin(pinned);
    for index in 0u32..2050 {
        let source = Source::from_ip(IpAddr::V4(Ipv4Addr::new(
            (index >> 8) as u8,
            index as u8,
            7,
            9,
        )));
        let outcome = table.debit(source, now).unwrap();
        assert!(matches!(
            outcome,
            DebitOutcome::Charged | DebitOutcome::Dropped(_)
        ));
    }
    assert!(table.debit(pinned, now).is_err());
    drop(pin);
}
#[test]
fn eviction_refills_fractionally_and_retry_is_rounded_to_seconds() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(10));
    let source = v4(17);
    let now = Instant::now();
    for _ in 0..10 {
        table.debit(source, now).unwrap();
    }
    let err = table
        .debit(source, now + Duration::from_millis(500))
        .unwrap_err();
    assert_eq!(err.retry_after_seconds, 1);
    assert_eq!(
        table.debit(source, now + Duration::from_secs(1)),
        Ok(DebitOutcome::Charged)
    );
}
#[test]
fn v4_and_v6_parent_capacities_are_independent_and_child_quota_deduplicates() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    for i in 0u16..160 {
        let source = Source::from_ip(IpAddr::V4(Ipv4Addr::new(
            192,
            0,
            (i / 10) as u8,
            (i % 10 + 1) as u8,
        )));
        assert_eq!(table.debit(source, now), Ok(DebitOutcome::Charged));
    }
    let over_parent_capacity = Source::from_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 20, 1)));
    assert!(table.debit(over_parent_capacity, now).is_err());
    for i in 0..40 {
        table.debit(v6(i), now).unwrap();
    }
    assert!(LoginBudgetTable::bucket_size() <= 128);
}
#[test]
fn unrelated_source_denial_does_not_subtract_available_tier_tokens() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let a = v4(1);
    let b = v4(17);
    for _ in 0..10 {
        table.debit(a, now).unwrap();
    }
    assert!(table.debit(a, now).is_err());
    assert_eq!(table.debit(b, now), Ok(DebitOutcome::Charged));
}
#[test]
fn pins_release_and_budget_storage_is_preallocated() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(1));
    let source = v4(17);
    table.debit(source, Instant::now()).unwrap();
    let pin = table.pin(source);
    let clone = pin.clone();
    drop(pin);
    drop(clone);
    assert!(LoginBudgetTable::bucket_size() <= 128);
}
