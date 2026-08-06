//! Tests for backward-compatible DDL op_id / ddl_status fields (#1015).

use shamir_types::types::record_id::RecordId;

use crate::read::{DdlOpState, QueryResult};

/// Wire backward-compat: an old-shape payload (without `op_id` / `ddl_status`)
/// must still decode successfully, with the new fields defaulting to `None`.
#[test]
fn query_result_backward_compat_without_ddl_fields() {
    // This is the pre-RFC shape — no `op_id` or `ddl_status`.
    let old_shape_bytes = rmp_serde::to_vec_named(&QueryResult {
        records: vec![],
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
        corrupt_records: vec![],
        op_id: None,
        ddl_status: None,
    })
    .expect("serialize new shape");

    // Decode as a plain `QueryValue` to verify the actual wire shape.
    // We expect NO `op_id` or `ddl_status` keys.
    let decoded_value: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&old_shape_bytes).expect("decode as QueryValue");

    // Verify that `op_id` and `ddl_status` are absent (old clients never send them).
    if let shamir_types::types::value::QueryValue::Map(map) = decoded_value {
        assert!(
            !map.contains_key("op_id"),
            "old-shape payload should NOT contain `op_id` key"
        );
        assert!(
            !map.contains_key("ddl_status"),
            "old-shape payload should NOT contain `ddl_status` key"
        );
    } else {
        panic!("QueryResult should encode as a Map");
    }

    // Verify that an old client sending this shape still decodes into a `QueryResult`
    // with the new fields defaulting to `None`.
    let back: QueryResult = rmp_serde::from_slice(&old_shape_bytes).expect("deserialize");
    assert!(
        back.op_id.is_none(),
        "op_id should be None for old-shape payload"
    );
    assert!(
        back.ddl_status.is_none(),
        "ddl_status should be None for old-shape payload"
    );
}

/// New fields round-trip: a modern payload with `op_id` / `ddl_status` set
/// serializes correctly and deserializes back to the same values.
#[test]
fn query_result_round_trip_with_ddl_fields() {
    let op_id = RecordId::system("test-ddl-op");
    let ddl_status = DdlOpState::Succeeded {
        completed_at: 1700000000000,
    };

    let original = QueryResult {
        records: vec![],
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
        corrupt_records: vec![],
        op_id: Some(op_id),
        ddl_status: Some(ddl_status.clone()),
    };

    let bytes = rmp_serde::to_vec_named(&original).expect("serialize");
    let back: QueryResult = rmp_serde::from_slice(&bytes).expect("deserialize");

    assert_eq!(back.op_id, Some(op_id), "op_id should round-trip");
    assert_eq!(
        back.ddl_status,
        Some(ddl_status),
        "ddl_status should round-trip"
    );
}

/// Skip-serialization behavior: when `op_id` / `ddl_status` are `None`,
/// they are omitted from the wire (same as `interner_delta` in `BatchResponse`).
#[test]
fn query_result_skip_serializing_if_none() {
    let query_result = QueryResult {
        records: vec![],
        stats: None,
        pagination: None,
        value: None,
        explain: None,
        skipped: false,
        versions: None,
        corrupt_records: vec![],
        op_id: None,
        ddl_status: None,
    };

    let bytes = rmp_serde::to_vec_named(&query_result).expect("serialize");

    // Decode as a plain `QueryValue` to verify the actual wire shape.
    let decoded_value: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode as QueryValue");

    if let shamir_types::types::value::QueryValue::Map(map) = decoded_value {
        assert!(
            !map.contains_key("op_id"),
            "`None` op_id should be skipped from serialization"
        );
        assert!(
            !map.contains_key("ddl_status"),
            "`None` ddl_status should be skipped from serialization"
        );
    } else {
        panic!("QueryResult should encode as a Map");
    }
}
