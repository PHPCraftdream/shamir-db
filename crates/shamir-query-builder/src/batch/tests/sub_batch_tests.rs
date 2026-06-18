//! Tests for `Batch::sub_batch`, `IntoBatchOp for SubBatchOp`, and `val::param`.

use shamir_collections::new_map;
use shamir_query_types::batch::{BatchOp, SubBatchOp};
use shamir_query_types::filter::FilterValue;
use shamir_types::mpack;

use crate::batch::Batch;
use crate::query::Query;
use crate::val::param;

// ============================================================================
// sub_batch_builds_batchop_batch
// ============================================================================

#[test]
fn sub_batch_builds_batchop_batch() {
    // Build a simple inner batch.
    let mut inner = Batch::new();
    inner.query("items", Query::from("products"));
    let inner_req = inner.build();

    let mut bind = new_map();
    bind.insert("uid".to_string(), FilterValue::Int(42));

    let mut outer = Batch::new();
    outer.sub_batch("proc", inner_req.clone(), bind.clone());
    let req = outer.build();

    let entry = req.queries.get("proc").expect("entry 'proc' missing");
    match &entry.op {
        BatchOp::Batch(sub) => {
            assert_eq!(sub.batch, inner_req, "inner BatchRequest must round-trip");
            assert_eq!(sub.bind, bind, "bind map must be preserved");
        }
        other => panic!("expected BatchOp::Batch, got {other:?}"),
    }
}

// ============================================================================
// sub_batch_bind_with_query_ref
// ============================================================================

#[test]
fn sub_batch_bind_with_query_ref() {
    // The outer batch has a query whose result is bound into the sub-batch.
    let mut inner = Batch::new();
    inner.query("data", Query::from("orders"));
    let inner_req = inner.build();

    let mut outer = Batch::new();
    let user_handle = outer.query("user", Query::from("users"));

    // bind "uid" → $query ref into @user[0].id
    let uid_ref = user_handle.first().field("id");
    let mut bind = new_map();
    bind.insert("uid".to_string(), uid_ref.clone());

    outer.sub_batch("proc", inner_req, bind);
    let req = outer.build();

    let entry = req.queries.get("proc").expect("entry 'proc' missing");
    match &entry.op {
        BatchOp::Batch(sub) => {
            let got = sub.bind.get("uid").expect("bind key 'uid' missing");
            assert_eq!(
                *got, uid_ref,
                "bind value must be the query-ref FilterValue"
            );
        }
        other => panic!("expected BatchOp::Batch, got {other:?}"),
    }
}

// ============================================================================
// param_builds_param_value
// ============================================================================

#[test]
fn param_builds_param_value() {
    let fv = param("uid");
    assert_eq!(
        fv,
        FilterValue::Param {
            name: "uid".to_string()
        },
        "param() must produce FilterValue::Param"
    );

    let bytes = rmp_serde::to_vec_named(&fv).expect("serialize");
    let got: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(
        got,
        mpack!({ "$param": "uid" }),
        "Param must serialise as {{\"$param\":\"...\"}}"
    );

    // Round-trip.
    let back: FilterValue = rmp_serde::from_slice(&bytes).expect("round-trip");
    assert_eq!(back, fv, "Param must round-trip through msgpack");
}

// ============================================================================
// sub_batch_handle_for_outer_ref
// ============================================================================

#[test]
fn sub_batch_handle_for_outer_ref() {
    let inner_req = Batch::new().build();

    let mut outer = Batch::new();
    let handle = outer.sub_batch("proc", inner_req, new_map());

    // The handle's alias must be "proc".
    assert_eq!(handle.alias(), "proc");

    // handle.column("result") must produce a $query ref with alias @proc and
    // path [].result — identical to the existing Handle behaviour.
    let col_ref = handle.column("result");
    let bytes = rmp_serde::to_vec_named(&col_ref).expect("serialize col_ref");
    let qv: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes).expect("decode col_ref");
    assert_eq!(qv["$query"], "@proc", "alias must be @proc");
    assert_eq!(qv["path"], "[].result", "column path must be [].result");

    // handle.first().field("id") must produce @proc[0].id.
    let row_ref = handle.first().field("id");
    let bytes2 = rmp_serde::to_vec_named(&row_ref).expect("serialize row_ref");
    let qv2: shamir_types::types::value::QueryValue =
        rmp_serde::from_slice(&bytes2).expect("decode row_ref");
    assert_eq!(qv2["$query"], "@proc");
    assert_eq!(qv2["path"], "[0].id");
}

// ============================================================================
// into_batch_op_for_sub_batch_op
// ============================================================================

#[test]
fn into_batch_op_for_sub_batch_op() {
    use crate::batch::IntoBatchOp;

    let sub = SubBatchOp {
        batch: Batch::new().build(),
        bind: new_map(),
    };
    let op = sub.into_batch_op();
    assert!(
        matches!(op, BatchOp::Batch(_)),
        "SubBatchOp::into_batch_op must produce BatchOp::Batch"
    );
}
