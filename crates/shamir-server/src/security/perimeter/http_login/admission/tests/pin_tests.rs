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
fn pin_counts_track_clones_and_last_release_under_table_churn() {
    let mut table = LoginBudgetTable::new(Duration::from_secs(60));
    let now = Instant::now();
    let pinned = source(1);
    assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    let pin = table.pin(pinned);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    let cloned_pin = pin.clone();
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    let mut other_pins = Vec::with_capacity(2047);
    for index in 0u32..2047 {
        let other = churn_source(index);
        assert_eq!(table.debit(other, now), Ok(DebitOutcome::Charged));
        other_pins.push(table.pin(other));
    }
    assert_eq!(table.occupied_slots(), 4096);
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    for _ in 0..9 {
        assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    }
    assert!(table.debit(pinned, now).is_err());
    assert_eq!(table.resident_pin_count(pinned), [2, 2, 0]);

    assert_eq!(
        table.debit(churn_source(3000), now),
        Ok(DebitOutcome::Dropped(
            super::super::BudgetDropReason::NoEvictableBucket
        ))
    );
    assert!(table.debit(pinned, now).is_err());
    drop(pin);
    assert_eq!(table.resident_pin_count(pinned), [1, 1, 0]);
    assert!(table.debit(pinned, now).is_err());
    drop(cloned_pin);

    assert_eq!(
        table.debit(churn_source(3000), now),
        Ok(DebitOutcome::Charged)
    );
    for _ in 0..10 {
        assert_eq!(table.debit(pinned, now), Ok(DebitOutcome::Charged));
    }
    assert!(table.debit(pinned, now).is_err());
    assert_eq!(other_pins.len(), 2047);
}
