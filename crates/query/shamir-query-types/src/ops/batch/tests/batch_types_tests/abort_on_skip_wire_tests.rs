use serde::Serialize;
use shamir_types::{mpack, types::value::QueryValue};

use crate::batch::{BatchOp, QueryEntry};
use crate::filter::{Filter, FilterValue};
use crate::read::ReadQuery;

fn when_entry(abort_on_skip: bool) -> QueryEntry {
    QueryEntry {
        op: BatchOp::Read(ReadQuery::new("items")),
        return_result: true,
        after: Vec::new(),
        when: Some(Filter::Eq {
            field: vec!["enabled".to_owned()],
            value: FilterValue::Bool(true),
        }),
        abort_on_skip,
    }
}

fn to_query_value<T: Serialize>(value: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(value).expect("serialize wire value");
    rmp_serde::from_slice(&bytes).expect("decode wire value as QueryValue")
}

// Prior-shape serde oracle: this is the QueryEntry declaration before the new
// field was added. Comparing encoded bytes against it protects the actual old
// wire shape; a self-roundtrip would only prove that the current encoder and
// decoder agree with each other.
#[derive(Serialize)]
struct PriorQueryEntry {
    #[serde(flatten)]
    op: BatchOp,
    return_result: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    when: Option<Filter>,
}

fn prior_shape(entry: &QueryEntry) -> PriorQueryEntry {
    PriorQueryEntry {
        op: entry.op.clone(),
        return_result: entry.return_result,
        after: entry.after.clone(),
        when: entry.when.clone(),
    }
}

#[test]
fn false_flag_preserves_pre_change_bytes_with_when() {
    let entry = when_entry(false);

    let actual = rmp_serde::to_vec_named(&entry).expect("serialize QueryEntry");
    let prior = rmp_serde::to_vec_named(&prior_shape(&entry)).expect("serialize prior shape");

    assert_eq!(actual, prior);
    let wire = to_query_value(&entry);
    assert!(wire.get("abort_on_skip").is_none());
}

#[test]
fn true_flag_is_present_on_serialized_entry() {
    let entry = when_entry(true);
    let wire = to_query_value(&entry);

    assert_eq!(
        wire.get("abort_on_skip").and_then(QueryValue::as_bool),
        Some(true)
    );
}

#[test]
fn non_boolean_flag_is_rejected() {
    // Wire-format test exception: this deliberately raw map is malformed input
    // from a peer, not a value serialized from the current QueryEntry type.
    let raw = mpack!({
        "from": "items",
        "abort_on_skip": "true"
    });
    let bytes = rmp_serde::to_vec_named(&raw).expect("serialize malformed wire shape");

    assert!(rmp_serde::from_slice::<QueryEntry>(&bytes).is_err());
}

#[test]
fn pre_flag_wire_shape_defaults_to_false() {
    // Wire-format test exception: keep this raw pre-change payload as an
    // independent compatibility fixture; do not build it by serializing the
    // current QueryEntry, which would make the defaulting oracle circular.
    let old_wire_shape = shamir_types::mpack!({
        "from": "items",
        "return_result": true,
        "when": {
            "op": "eq",
            "field": "enabled",
            "value": true
        }
    });
    let bytes = rmp_serde::to_vec_named(&old_wire_shape).expect("serialize old wire shape");
    let entry: QueryEntry = rmp_serde::from_slice(&bytes).expect("decode pre-flag entry");

    assert!(!entry.abort_on_skip);
    assert!(entry.when.is_some());
}

#[test]
fn flag_is_a_query_entry_field_not_an_operation_field() {
    let entry = when_entry(true);
    let wire = to_query_value(&entry);

    assert_eq!(
        wire.get("abort_on_skip").and_then(QueryValue::as_bool),
        Some(true),
        "flag must be at the flattened QueryEntry level"
    );
    assert!(wire.get("op").is_none(), "the operation remains flattened");
    assert_eq!(wire.get("from").and_then(QueryValue::as_str), Some("items"));

    // A standalone ReadQuery / BatchOp has no entry-level field and must not
    // accept the flag as an operation member.
    let operation_with_flag = mpack!({
        "from": "items",
        "abort_on_skip": true
    });
    let bytes = rmp_serde::to_vec_named(&operation_with_flag)
        .expect("serialize operation-shaped wire value");
    assert!(rmp_serde::from_slice::<BatchOp>(&bytes).is_err());
}
