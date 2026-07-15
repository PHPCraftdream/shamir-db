//! Tests for `Batch::for_each` (Epic04/C, #654).

use shamir_query_types::batch::{BatchOp, ForEachOp};
use shamir_query_types::filter::FilterValue;
use shamir_types::mpack;

use crate::batch::Batch;
use crate::query::Query;
use crate::val::{func, lit, param};

// ============================================================================
// for_each_with_literal_array_over
// ============================================================================

#[test]
fn for_each_with_literal_array_over() {
    let mut inner = Batch::new();
    inner.query("item", Query::from("products"));
    let inner_req = inner.build();

    let mut outer = Batch::new();
    outer.for_each("loop", vec![lit(1), lit(2)], "row", inner_req.clone());
    let req = outer.build();

    let entry = req.queries.get("loop").expect("entry 'loop' missing");
    match &entry.op {
        BatchOp::ForEach(fe) => {
            assert_eq!(
                fe.over,
                FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
                "over must be the literal array"
            );
            assert_eq!(fe.bind_row, "row", "bind_row must be preserved");
            assert_eq!(fe.batch, inner_req, "inner BatchRequest must round-trip");
        }
        other => panic!("expected BatchOp::ForEach, got {other:?}"),
    }
}

// ============================================================================
// for_each_with_query_ref_over
// ============================================================================

#[test]
fn for_each_with_query_ref_over() {
    let inner_req = Batch::new().build();

    let mut outer = Batch::new();
    let orders_handle = outer.query("orders", Query::from("orders"));

    // over → $query ref into @orders[].id
    let over_ref = orders_handle.column("id");

    outer.for_each("loop", over_ref.clone(), "row", inner_req);
    let req = outer.build();

    let entry = req.queries.get("loop").expect("entry 'loop' missing");
    match &entry.op {
        BatchOp::ForEach(fe) => {
            assert_eq!(fe.over, over_ref, "over must be the $query-ref FilterValue");
            assert_eq!(fe.bind_row, "row");
        }
        other => panic!("expected BatchOp::ForEach, got {other:?}"),
    }
}

// ============================================================================
// for_each_with_fn_call_over
// ============================================================================

#[test]
fn for_each_with_fn_call_over() {
    let inner_req = Batch::new().build();

    let over_fn = func("RANGE", vec![lit(0), lit(10)]);

    let mut outer = Batch::new();
    outer.for_each("loop", over_fn.clone(), "row", inner_req);
    let req = outer.build();

    let entry = req.queries.get("loop").expect("entry 'loop' missing");
    match &entry.op {
        BatchOp::ForEach(fe) => {
            assert_eq!(fe.over, over_fn, "over must be the $fn-call FilterValue");
            assert!(matches!(fe.over, FilterValue::FnCall { .. }));
        }
        other => panic!("expected BatchOp::ForEach, got {other:?}"),
    }
}

// ============================================================================
// for_each_wire_roundtrip_is_for_each_not_batch
// ============================================================================

#[test]
fn for_each_wire_roundtrip_is_for_each_not_batch() {
    // Regression guard for the wire-key-collision bug fixed in #653:
    // ForEachOp's `batch` field is wire-renamed `for_each`, so a round-trip
    // through msgpack must decode as `BatchOp::ForEach`, never
    // `BatchOp::Batch`.
    let mut inner = Batch::new();
    inner.query("x", Query::from("t"));
    let inner_req = inner.build();

    let mut outer = Batch::new();
    outer.for_each("loop", vec![lit(1)], "row", inner_req);
    let req = outer.build();

    let bytes = rmp_serde::to_vec_named(&req).expect("serialize BatchRequest");
    let back: shamir_query_types::batch::BatchRequest =
        rmp_serde::from_slice(&bytes).expect("decode BatchRequest");

    let entry = back.queries.get("loop").expect("entry 'loop' missing");
    assert!(
        matches!(entry.op, BatchOp::ForEach(_)),
        "wire round-trip must decode as BatchOp::ForEach, got {:?}",
        entry.op
    );

    // Also assert the raw wire shape carries a top-level `for_each` key
    // (not `batch`).
    let qv: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode as QueryValue");
    let loop_entry = &qv["queries"]["loop"];
    assert_ne!(
        loop_entry["for_each"],
        mpack!(null),
        "must have 'for_each' key"
    );
}

// ============================================================================
// into_batch_op_for_for_each_op (escape hatch parity)
// ============================================================================

#[test]
fn for_each_builds_via_op_escape_hatch() {
    let fe = ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "row".to_string(),
        batch: Batch::new().build(),
    };

    // ForEachOp does not implement IntoBatchOp directly (unlike SubBatchOp);
    // `Batch::for_each` is the intended, sole construction path. Verify the
    // op it builds matches manual construction.
    let manual = BatchOp::ForEach(fe.clone());

    let mut outer = Batch::new();
    outer.for_each("loop", fe.over, fe.bind_row, fe.batch);
    let req = outer.build();
    let entry = req.queries.get("loop").unwrap();
    assert_eq!(entry.op, manual);

    let _ = param("unused"); // keep `param` import exercised for parity with sub_batch_tests style
}
