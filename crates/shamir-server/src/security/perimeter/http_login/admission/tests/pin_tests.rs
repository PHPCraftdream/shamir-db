use super::super::{DebitOutcome, LoginBudgetTable, Source};
use std::net::{IpAddr, Ipv4Addr};
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

#[test]
fn resident_pin_union_is_capped_at_3072_and_failed_pin_is_atomic() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let mut held = Vec::with_capacity(1_024);
    for index in 0u32..1_024 {
        let source = churn_source(index);
        assert_eq!(table.debit(source, now), Ok(DebitOutcome::Charged));
        held.push(table.pin(source).unwrap());
    }
    assert_eq!(table.pinned_bucket_count(), 3_072);
    let overflow = churn_source(2_000);
    assert_eq!(table.debit(overflow, now), Ok(DebitOutcome::Charged));
    assert!(table.pin(overflow).is_none());
    assert_eq!(table.pinned_bucket_count(), 3_072);
    drop(held);
    assert_eq!(table.pinned_bucket_count(), 0);
}

#[test]
fn pin_counts_track_clones_and_last_release_under_table_churn() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = source(1);
    assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    let pins = table.pin(pinned).unwrap();
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    let cloned_pin = pins.clone();
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    let mut other_pins = Vec::with_capacity(1_023);
    for index in 0u32..2047 {
        let other = churn_source(index);
        assert_eq!(table.debit(other, now), Ok(DebitOutcome::Charged));
        if index < 1_023 {
            other_pins.push(table.pin(other).unwrap());
        }
    }
    assert_eq!(table.occupied_slots(), 4096);
    assert_eq!(
        table.debit(churn_source(3_000), now),
        Ok(DebitOutcome::Charged)
    );
    let pinned_bucket_count = table.pinned_bucket_count();
    assert_eq!(pinned_bucket_count, 3_072);
    for index in 0..10 {
        assert!(matches!(
            table.debit(churn_source(index), now),
            Ok(DebitOutcome::Charged)
        ));
    }
    assert_eq!(table.pinned_bucket_count(), pinned_bucket_count);
    assert!(table.debit(pinned, now).is_err());
    drop(pins);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    drop(cloned_pin);
    assert_eq!(table.resident_pin_count(pinned), [0, 0, 0]);

    assert_eq!(
        table.debit(churn_source(3000), now),
        Ok(DebitOutcome::Charged)
    );
    for _ in 0..10 {
        assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    }
    assert!(table.debit(pinned, now).is_err());
    assert_eq!(table.resident_pin_count(pinned), [0, 0, 0]);
    drop(other_pins);
    assert_eq!(table.pinned_bucket_count(), 0);
}
