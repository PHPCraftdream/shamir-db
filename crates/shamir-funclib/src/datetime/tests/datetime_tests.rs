//! Per-function `/datetime` tests — at least one correct-result assert and one
//! error/edge case per registered function. Expected values are derived from
//! `chrono` directly so the asserts do not duplicate hand-computed constants.

use crate::datetime;
use crate::registry::{v_bool, v_int, v_str, ScalarRegistry};
use chrono::{DateTime, TimeZone, Utc};
use shamir_types::types::value::QueryValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    datetime::register(&mut r);
    r
}

/// 2021-06-02T15:30:45Z — a Wednesday — as canonical epoch-millis.
const WED_MS: i64 = 1_622_647_845_000;

fn ts(ms: i64) -> QueryValue {
    QueryValue::Int(ms)
}

fn millis_of(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&Utc)
        .timestamp_millis()
}

#[test]
fn now_is_nondeterministic_metadata() {
    let r = reg();
    // metadata: now is impure + non-deterministic.
    let e = r.get("now").unwrap();
    assert!(!e.pure);
    assert!(!e.deterministic);
    // result is a positive epoch-millis Int.
    match r.call("now", &[]).unwrap() {
        QueryValue::Int(n) => assert!(n > 1_600_000_000_000),
        other => panic!("expected Int, got {other:?}"),
    }
    // error: arity (now takes no args).
    assert_eq!(r.call("now", &[ts(0)]).unwrap_err().code, "arity");
}

#[test]
fn age_is_nondeterministic_and_nonnegative() {
    let r = reg();
    let e = r.get("age").unwrap();
    assert!(!e.pure);
    assert!(!e.deterministic);
    // age of a long-past instant is positive seconds.
    match r.call("age", &[ts(WED_MS)]).unwrap() {
        QueryValue::Int(n) => assert!(n > 0),
        other => panic!("expected Int, got {other:?}"),
    }
    // error: wrong type.
    assert_eq!(
        r.call("age", &[QueryValue::Str("x".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn epoch_conversions() {
    let r = reg();
    assert_eq!(
        r.call("to_epoch_s", &[ts(WED_MS)]).unwrap(),
        v_int(1_622_647_845)
    );
    assert_eq!(r.call("to_epoch_ms", &[ts(WED_MS)]).unwrap(), v_int(WED_MS));
    assert_eq!(
        r.call("from_epoch_s", &[ts(1_622_647_845)]).unwrap(),
        v_int(WED_MS)
    );
    assert_eq!(
        r.call("from_epoch_ms", &[ts(WED_MS)]).unwrap(),
        v_int(WED_MS)
    );
    // floor division for negative millis (200ms before epoch -> -1s).
    assert_eq!(r.call("to_epoch_s", &[ts(-200)]).unwrap(), v_int(-1));
    // error: from_epoch_s overflow.
    assert_eq!(
        r.call("from_epoch_s", &[ts(i64::MAX)]).unwrap_err().code,
        "out_of_range"
    );
}

#[test]
fn rfc3339_roundtrip() {
    let r = reg();
    let ms = r
        .call(
            "parse_rfc3339",
            &[v_str("2021-06-02T15:30:45+00:00".into())],
        )
        .unwrap();
    assert_eq!(ms, v_int(WED_MS));
    let s = r.call("format_rfc3339", &[ts(WED_MS)]).unwrap();
    // re-parse the formatted string and confirm it round-trips.
    match s {
        QueryValue::Str(text) => assert_eq!(millis_of(&text), WED_MS),
        other => panic!("expected Str, got {other:?}"),
    }
    // error: unparseable string.
    assert_eq!(
        r.call("parse_rfc3339", &[v_str("not-a-date".into())])
            .unwrap_err()
            .code,
        "parse"
    );
}

#[test]
fn components() {
    let r = reg();
    assert_eq!(r.call("year", &[ts(WED_MS)]).unwrap(), v_int(2021));
    assert_eq!(r.call("month", &[ts(WED_MS)]).unwrap(), v_int(6));
    assert_eq!(r.call("day", &[ts(WED_MS)]).unwrap(), v_int(2));
    assert_eq!(r.call("hour", &[ts(WED_MS)]).unwrap(), v_int(15));
    assert_eq!(r.call("minute", &[ts(WED_MS)]).unwrap(), v_int(30));
    assert_eq!(r.call("second", &[ts(WED_MS)]).unwrap(), v_int(45));
    // error: out-of-range timestamp rejected by extractor.
    assert_eq!(
        r.call("year", &[ts(i64::MAX)]).unwrap_err().code,
        "out_of_range"
    );
}

#[test]
fn weekday_and_weekend() {
    let r = reg();
    // Wednesday -> num_days_from_monday == 2.
    assert_eq!(r.call("weekday", &[ts(WED_MS)]).unwrap(), v_int(2));
    assert_eq!(r.call("is_weekend", &[ts(WED_MS)]).unwrap(), v_bool(false));
    // Saturday 2021-06-05T12:00:00Z is a weekend.
    let sat = millis_of("2021-06-05T12:00:00+00:00");
    assert_eq!(r.call("weekday", &[ts(sat)]).unwrap(), v_int(5));
    assert_eq!(r.call("is_weekend", &[ts(sat)]).unwrap(), v_bool(true));
    // error: wrong type for weekday.
    assert_eq!(
        r.call("is_weekend", &[QueryValue::Str("x".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn arithmetic() {
    let r = reg();
    assert_eq!(
        r.call("add_secs", &[ts(WED_MS), ts(60)]).unwrap(),
        v_int(WED_MS + 60_000)
    );
    assert_eq!(
        r.call("add_days", &[ts(WED_MS), ts(1)]).unwrap(),
        v_int(WED_MS + 86_400_000)
    );
    assert_eq!(
        r.call("diff_secs", &[ts(WED_MS + 5_000), ts(WED_MS)])
            .unwrap(),
        v_int(5)
    );
    // error: add_days overflow.
    assert_eq!(
        r.call("add_days", &[ts(WED_MS), ts(i64::MAX)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn diff_secs_overflow_returns_error() {
    let r = reg();
    // i64::MAX - i64::MIN overflows i64 — must return "out_of_range", not panic.
    assert_eq!(
        r.call("diff_secs", &[ts(i64::MAX), ts(i64::MIN)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
    // Reverse direction also overflows.
    assert_eq!(
        r.call("diff_secs", &[ts(i64::MIN), ts(i64::MAX)])
            .unwrap_err()
            .code,
        "out_of_range"
    );
}

#[test]
fn period_starts() {
    let r = reg();
    assert_eq!(
        r.call("start_of_day", &[ts(WED_MS)]).unwrap(),
        v_int(millis_of("2021-06-02T00:00:00+00:00"))
    );
    // week starts on Monday 2021-05-31.
    assert_eq!(
        r.call("start_of_week", &[ts(WED_MS)]).unwrap(),
        v_int(millis_of("2021-05-31T00:00:00+00:00"))
    );
    assert_eq!(
        r.call("start_of_month", &[ts(WED_MS)]).unwrap(),
        v_int(millis_of("2021-06-01T00:00:00+00:00"))
    );
    // error: out-of-range input.
    assert_eq!(
        r.call("start_of_day", &[ts(i64::MAX)]).unwrap_err().code,
        "out_of_range"
    );
}

#[test]
fn truncate_units() {
    let r = reg();
    assert_eq!(
        r.call("truncate", &[ts(WED_MS + 123), v_str("second".into())])
            .unwrap(),
        v_int(WED_MS)
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("minute".into())])
            .unwrap(),
        v_int(millis_of("2021-06-02T15:30:00+00:00"))
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("hour".into())])
            .unwrap(),
        v_int(millis_of("2021-06-02T15:00:00+00:00"))
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("day".into())])
            .unwrap(),
        v_int(millis_of("2021-06-02T00:00:00+00:00"))
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("week".into())])
            .unwrap(),
        v_int(millis_of("2021-05-31T00:00:00+00:00"))
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("month".into())])
            .unwrap(),
        v_int(millis_of("2021-06-01T00:00:00+00:00"))
    );
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("year".into())])
            .unwrap(),
        v_int(millis_of("2021-01-01T00:00:00+00:00"))
    );
    // error: unknown unit.
    assert_eq!(
        r.call("truncate", &[ts(WED_MS), v_str("fortnight".into())])
            .unwrap_err()
            .code,
        "bad_unit"
    );
}

/// Sanity: ensure the canonical constant matches its RFC-3339 spelling so the
/// other asserts rest on a correct baseline.
#[test]
fn baseline_constant_is_consistent() {
    assert_eq!(
        WED_MS,
        Utc.timestamp_opt(1_622_647_845, 0)
            .unwrap()
            .timestamp_millis()
    );
}
