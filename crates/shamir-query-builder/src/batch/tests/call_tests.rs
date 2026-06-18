use crate::batch::Batch;
use crate::query::Query;
use crate::val::*;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::call::CallOp;
use shamir_query_types::filter::FilterValue;

// ============================================================================
// Batch::call — basic construction
// ============================================================================

#[test]
fn call_builds_call_op_with_params() {
    let mut b = Batch::new();
    b.call("p", "my_proc", [lit(1), lit("x")]);
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();

    let entry = &json["queries"]["p"];
    assert_eq!(entry["return_result"], true);
    assert_eq!(entry["call"], "my_proc");

    let params = entry["params"].as_array().unwrap();
    assert_eq!(params.len(), 2);
    assert_eq!(params[0], serde_json::json!(1));
    assert_eq!(params[1], serde_json::json!("x"));
}

#[test]
fn call_default_repo_is_main() {
    let mut b = Batch::new();
    b.call("p", "proc", Vec::<FilterValue>::new());
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["queries"]["p"]["repo"], "main");
}

// ============================================================================
// Batch::call_in_repo
// ============================================================================

#[test]
fn call_in_repo_sets_custom_repo() {
    let mut b = Batch::new();
    b.call_in_repo("p", "proc", "analytics", [lit(42)]);
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["queries"]["p"]["repo"], "analytics");
    assert_eq!(json["queries"]["p"]["call"], "proc");
}

// ============================================================================
// Wire snapshot via QueryValue
// ============================================================================

#[test]
fn call_wire_snapshot() {
    use crate::wire::ToWire;
    use shamir_types::types::value::QueryValue;

    let mut b = Batch::new();
    b.call("p", "proc", [lit(1), lit("v")]);
    let qv = b.build().to_query_value().unwrap();
    let entry = qv
        .get("queries")
        .and_then(|q| q.get("p"))
        .expect("entry 'p'");

    assert_eq!(entry.get("call").and_then(|v| v.as_str()), Some("proc"));
    assert_eq!(entry.get("repo").and_then(|v| v.as_str()), Some("main"));

    // Verify params: [1, "v"]
    let params = entry.get("params").expect("params key");
    let list = match params {
        QueryValue::List(l) => l,
        other => panic!("expected List, got {other:?}"),
    };
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].as_i64(), Some(1));
    assert_eq!(list[1].as_str(), Some("v"));
}

// ============================================================================
// msgpack round-trip
// ============================================================================

#[test]
fn call_round_trips_via_msgpack() {
    let mut b = Batch::new();
    b.call("p", "daily_report", [lit(2024), lit("Q1")]);

    let via_build = b.build();
    let via_msgpack = b.to_request_via_msgpack();
    assert_eq!(via_build, via_msgpack);
}

// ============================================================================
// Handle from call is referenceable
// ============================================================================

#[test]
fn call_handle_produces_query_ref() {
    let mut b = Batch::new();
    let p = b.call("p", "get_ids", [lit(1)]);
    b.query("q", Query::from("t").where_eq("id", p.first().field("id")));
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();

    let where_clause = &json["queries"]["q"]["where"];
    assert_eq!(where_clause["op"], "eq");
    assert_eq!(where_clause["value"]["$query"], "@p");
    assert_eq!(where_clause["value"]["path"], "[0].id");
}

#[test]
fn call_handle_column_ref() {
    let mut b = Batch::new();
    let p = b.call("p", "proc", [lit(1)]);
    b.query("q", Query::from("t").where_in("uid", [p.column("user_id")]));
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();

    let vals = json["queries"]["q"]["where"]["values"].as_array().unwrap();
    assert_eq!(vals[0]["$query"], "@p");
    assert_eq!(vals[0]["path"], "[].user_id");
}

// ============================================================================
// IntoBatchOp for CallOp
// ============================================================================

#[test]
fn into_batch_op_call_op() {
    use crate::batch::IntoBatchOp;

    let op = CallOp {
        call: "fn".to_string(),
        params: vec![],
        repo: "main".to_string(),
    };
    let batch_op = op.into_batch_op();
    match batch_op {
        BatchOp::Call(_) => {}
        _ => panic!("expected Call"),
    }
}

// ============================================================================
// try_build validates call refs
// ============================================================================

#[test]
fn try_build_validates_call_ref_happy() {
    let mut b = Batch::new();
    let p = b.call("p", "proc", [lit(1)]);
    b.query("q", Query::from("t").where_eq("x", p.first().field("id")));
    assert!(b.try_build().is_ok());
}

// ============================================================================
// call with no params
// ============================================================================

#[test]
fn call_no_params() {
    let mut b = Batch::new();
    b.call("p", "ping", Vec::<FilterValue>::new());
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["queries"]["p"]["call"], "ping");
    // params omitted or empty array
    let params = &json["queries"]["p"]["params"];
    assert!(params.is_null() || params.as_array().is_none_or(|a| a.is_empty()));
}

// ============================================================================
// call via op() escape hatch with CallOp
// ============================================================================

#[test]
fn call_via_op_escape_hatch() {
    let mut b = Batch::new();
    let op = CallOp {
        call: "my_fn".to_string(),
        params: vec![lit(1)],
        repo: "main".to_string(),
    };
    b.op("c", op);
    let req = b.build();
    let json = serde_json::to_value(&req).unwrap();
    assert_eq!(json["queries"]["c"]["call"], "my_fn");
}
