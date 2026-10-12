//! VIS-4c: the `changes_since` response carries the sampled settled prefix
//! as an ADDITIVE, omitted-when-unavailable field; payloads without it (and
//! old clients) still decode. Wire fixtures use `rmp_serde` mirrors — this
//! crate's unit tests have no `serde_json` dependency.

use rmp_serde::{from_slice, to_vec_named};
use serde::{Deserialize, Serialize};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

// The exact shape `handle_changes_since` emits, with the new field.
fn response_with_visible_through(w: u64) -> QueryValue {
    mpack!({
        "changes_since": @(QueryValue::Int(41)),
        "events": @(QueryValue::List(Vec::new())),
        "gap_at": @(QueryValue::Null),
        "visible_through": @(QueryValue::Int(w as i64)),
    })
}

// The pre-VIS-4c shape: the field is simply absent.
fn response_without_visible_through() -> QueryValue {
    mpack!({
        "changes_since": @(QueryValue::Int(41)),
        "events": @(QueryValue::List(Vec::new())),
        "gap_at": @(QueryValue::Null),
    })
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Mirror {
    #[allow(dead_code)]
    changes_since: i64,
    #[serde(default)]
    visible_through: Option<i64>,
}

// Red before VIS-4c: no `visible_through` existed on the response.
#[test]
fn visible_through_round_trips_and_reports_the_sample() {
    let bytes = to_vec_named(&response_with_visible_through(7)).unwrap();
    let decoded: Mirror = from_slice(&bytes).unwrap();
    assert_eq!(decoded.visible_through, Some(7));
}

// Old payloads decode unchanged: the field defaults to absent.
#[test]
fn payload_without_the_field_still_decodes() {
    let bytes = to_vec_named(&response_without_visible_through()).unwrap();
    let decoded: Mirror = from_slice(&bytes).unwrap();
    assert_eq!(decoded.visible_through, None);
}
