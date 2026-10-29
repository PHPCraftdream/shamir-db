//! `BatchOp` dispatch invariants (RV-QUERY-TYPES-77BFC767): ambiguous
//! discriminator sets are decode errors; `update` + `set` stays legal
//! because `set` is a payload field of `UpdateOp`. Non-discriminator
//! payload keys (`where`, `count_total`, `request_id`, …) are the typed
//! op's business and are NOT rejected at the `BatchOp` layer.

use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::batch_op::BatchOp;

// Wire-format regression tests use raw input because a builder would bypass the decoding boundary.
#[test]
fn large_typed_unsigned_values_survive_batch_dispatch() {
    for version in [i64::MAX as u64 + 1, u64::MAX] {
        for input in [
            format!(
                r#"{{
                "update": "users",
                "set": {{}},
                "expected_version": {version}
            }}"#
            ),
            format!(
                r#"{{
                "delete_from": "users",
                "where": {{
                    "op": "eq",
                    "field": ["name"],
                    "value": "x"
                }},
                "expected_version": {version}
            }}"#
            ),
        ] {
            let wire: serde_json::Value = serde_json::from_str(&input).unwrap();
            let bytes = rmp_serde::to_vec_named(&wire).unwrap();
            let op: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
            let actual = match op {
                BatchOp::Update(op) => op.expected_version,
                BatchOp::Delete(op) => op.expected_version,
                other => panic!("unexpected operation: {other:?}"),
            };
            assert_eq!(actual, Some(version));
        }
    }
}

#[test]
fn nested_batch_preserves_large_work_limit() {
    let wire: serde_json::Value = serde_json::from_str(
        r#"{
        "batch": {
            "id": null,
            "queries": {},
            "limits": {
                "max_total_work": 18446744073709551615
            }
        }
    }"#,
    )
    .unwrap();
    let bytes = rmp_serde::to_vec_named(&wire).unwrap();
    let op: BatchOp = rmp_serde::from_slice(&bytes).unwrap();
    match op {
        BatchOp::Batch(op) => assert_eq!(op.batch.limits.max_total_work, u64::MAX),
        other => panic!("unexpected operation: {other:?}"),
    }
}

#[test]
fn duplicate_discriminator_last_value_wins() {
    let input = r#"{
        "from": "first",
        "from": "last"
    }"#;
    match serde_json::from_str::<BatchOp>(input).unwrap() {
        BatchOp::Read(op) => assert_eq!(op.from.table, "last"),
        other => panic!("unexpected operation: {other:?}"),
    }
}

#[test]
fn ambiguous_discriminator_error_is_preserved() {
    let op = mpack!({
        "from": "users",
        "insert_into": "orders"
    });
    assert!(decode(&op)
        .unwrap_err()
        .to_string()
        .contains("ambiguous BatchOp discriminators"));
}

fn decode(qv: &QueryValue) -> Result<BatchOp, Box<dyn std::error::Error>> {
    let bytes = rmp_serde::to_vec_named(qv)?;
    rmp_serde::from_slice(&bytes).map_err(Into::into)
}

/// `{"from": ..., "insert_into": ...}` used to decode as `Read` (first arm
/// wins) and silently drop the `insert_into` payload — now an error.
#[test]
fn rv_query_types_77bfc767_ambiguous_discriminators_rejected() {
    let qv = mpack!({ "from": "users", "insert_into": "orders" });
    assert!(decode(&qv).is_err(), "from + insert_into must be rejected");
}

#[test]
fn rv_query_types_77bfc767_update_with_set_payload_still_accepted() {
    // Real `UpdateOp` shape: `update` (TableRef), `set` (QueryValue map),
    // `where` (Filter, tagged `{"op": "eq", "field": [...], "value": ...}`).
    let qv = mpack!({
        "update": "users",
        "set": @ QueryValue::Map({
            let mut m = shamir_types::types::common::TMap::default();
            m.insert("name".to_string(), QueryValue::Str("x".to_string()));
            m
        }),
        "where": @ QueryValue::Map({
            let mut m = shamir_types::types::common::TMap::default();
            m.insert(
                "op".to_string(),
                QueryValue::Str("eq".to_string()),
            );
            m.insert(
                "field".to_string(),
                QueryValue::List(vec![QueryValue::Str("name".to_string())]),
            );
            m.insert("value".to_string(), QueryValue::Str("x".to_string()));
            m
        })
    });
    match decode(&qv).expect("update + set is a legal UpdateOp") {
        BatchOp::Update(update) => assert_eq!(update.update.table, "users"),
        other => panic!("expected Update, got {other:?}"),
    }
}

#[test]
fn rv_query_types_77bfc767_set_alone_still_decodes_as_set() {
    // Real `SetOp` shape: `set` (TableRef), `key` + `value` (QueryValue).
    let qv = mpack!({
        "set": "k",
        "key": "user-1",
        "value": @ QueryValue::Map({
            let mut m = shamir_types::types::common::TMap::default();
            m.insert("name".to_string(), QueryValue::Str("x".to_string()));
            m
        })
    });
    match decode(&qv).expect("set alone") {
        BatchOp::Set(set) => assert_eq!(set.set.table, "k"),
        other => panic!("expected Set, got {other:?}"),
    }
}

/// Regression for the blanket unknown-key rejection removal (review finding
/// 1): a `Read` op carrying only payload keys (`select`, `where`,
/// `count_total`) must decode. With the deleted
/// `for key in m.keys() { if !is_marker(key) { return Err(unknown
/// BatchOp field) } }` loop restored in
/// `crates/shamir-query-types/src/ops/batch/ops/batch_op.rs`, this test
/// FAILS (`select`/`where`/`count_total` are not discriminators and were
/// rejected); with the loop removed it passes.
#[test]
fn rv_query_types_77bfc767_read_with_payload_keys_decodes() {
    let qv = mpack!({
        "from": "users",
        "where": @ QueryValue::Map({
            let mut m = shamir_types::types::common::TMap::default();
            m.insert("op".to_string(), QueryValue::Str("eq".to_string()));
            m.insert(
                "field".to_string(),
                QueryValue::List(vec![QueryValue::Str("name".to_string())]),
            );
            m.insert("value".to_string(), QueryValue::Str("x".to_string()));
            m
        }),
        "count_total": true
    });
    match decode(&qv).expect("payload keys belong to the typed op") {
        BatchOp::Read(read) => {
            assert_eq!(read.from.table, "users");
            assert!(read.r#where.is_some(), "where filter must decode");
            assert!(read.count_total);
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

/// A `DropIndexOp` carrying its own payload keys (`table`, `repo`,
/// `request_id`) must decode as `BatchOp::DropIndex`; same
/// unknown-key-rejection regression class as
/// `rv_query_types_77bfc767_read_with_payload_keys_decodes`.
#[test]
fn rv_query_types_77bfc767_drop_index_with_payload_decodes() {
    let request_id = shamir_types::types::record_id::RecordId::try_from_bytes(
        &(0x01u8..=0x10).collect::<Vec<u8>>(),
    )
    .expect("16 raw bytes");
    let qv = mpack!({
        "drop_index": "idx",
        "table": "t",
        "repo": "main",
        "request_id": @ QueryValue::Str(request_id.to_string())
    });
    match decode(&qv).expect("payload keys belong to the typed op") {
        BatchOp::DropIndex(op) => assert_eq!(op.request_id, Some(request_id)),
        other => panic!("expected DropIndex, got {other:?}"),
    }
}

#[test]
fn trailing_sequence_elements_are_rejected_like_the_wire_decoder() {
    // TableRef reads exactly two elements; the buffered path must not drop a third.
    let valid = mpack!({
        "from": ["main", "users"]
    });
    assert!(decode(&valid).is_ok());
    let trailing = mpack!({
        "from": ["main", "users", "unexpected"]
    });
    assert!(decode(&trailing).is_err());
}
