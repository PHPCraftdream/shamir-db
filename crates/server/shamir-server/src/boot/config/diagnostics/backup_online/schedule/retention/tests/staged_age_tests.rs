use std::time::{Duration, SystemTime};

use super::super::ownership::staged_debris_old_enough;

const MIN_AGE: Duration = Duration::from_secs(15 * 60);

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000)
}

#[test]
fn a_file_at_or_past_the_minimum_age_is_debris() {
    assert!(staged_debris_old_enough(now() - MIN_AGE, now()));
    assert!(staged_debris_old_enough(
        now() - MIN_AGE - Duration::from_secs(1),
        now()
    ));
}

#[test]
fn a_young_file_is_never_debris() {
    assert!(!staged_debris_old_enough(
        now() - MIN_AGE + Duration::from_secs(1),
        now()
    ));
    assert!(!staged_debris_old_enough(now(), now()));
}

#[test]
fn a_future_modification_time_is_never_debris() {
    assert!(!staged_debris_old_enough(
        now() + Duration::from_secs(1),
        now()
    ));
}

/// A filesystem without last-write times reports zero, which Windows reads as
/// the year 1601: that must not look centuries old.
#[test]
fn a_missing_or_pre_epoch_modification_time_is_never_debris() {
    assert!(!staged_debris_old_enough(SystemTime::UNIX_EPOCH, now()));
    assert!(!staged_debris_old_enough(
        SystemTime::UNIX_EPOCH - Duration::from_secs(11_644_473_600),
        now()
    ));
}
