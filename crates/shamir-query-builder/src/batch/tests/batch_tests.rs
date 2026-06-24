use shamir_collections::{new_map, TMap};
use shamir_query_types::batch::ResultEncoding;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::{Batch, BuildError, Durability, Handle, Isolation};
use crate::filter;
use crate::query::Query;
use crate::val::*;
use crate::wire::ToWire;
use crate::write::{self, doc};

// ============================================================================
// return_flagged → return_all=false, return_only=None
// ============================================================================

#[test]
fn return_flagged_sets_return_all_false_and_no_return_only() {
    let mut b = Batch::new();
    b.query("visible", Query::from("users"));
    b.query_silent("hidden", Query::from("temp"));
    b.return_flagged();
    let req = b.build();
    assert!(!req.return_all);
    assert!(req.return_only.is_none());
}

#[test]
fn return_flagged_then_return_all_restores_default() {
    let mut b = Batch::new();
    b.return_flagged();
    assert!(!b.build().return_all);
    b.return_all();
    let req = b.build();
    assert!(req.return_all);
    assert!(req.return_only.is_none());
}

// ============================================================================
// Handle / RowRef path construction
// ============================================================================

#[test]
fn handle_column_single_field() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.column("id");
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    assert_eq!(qv["path"], "[].id");
}

#[test]
fn handle_column_nested_field() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.column(["a", "b"]);
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    assert_eq!(qv["path"], "[].a.b");
}

#[test]
fn handle_row_field() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.row(2).field("id");
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    assert_eq!(qv["path"], "[2].id");
}

#[test]
fn handle_row_get() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.row(2).get();
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    assert_eq!(qv["path"], "[2]");
}

#[test]
fn handle_first_field() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.first().field("id");
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    assert_eq!(qv["path"], "[0].id");
}

#[test]
fn handle_all() {
    let h = Handle {
        alias: "users".into(),
    };
    let fv = h.all();
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@users");
    // qref_all produces no path
    assert!(qv.get("path").is_none());
}

// ============================================================================
// Two-query dependency via handle
// ============================================================================

#[test]
fn two_query_dependency_via_handle() {
    let mut b = Batch::new();
    let users = b.query("users", Query::from("users").select(["id"]));
    b.query(
        "orders",
        Query::from("orders").where_in("user_id", [users.column("id")]),
    );
    let req = b.build();
    let qv = req.to_query_value().unwrap();

    let orders_where = &qv["queries"]["orders"]["where"];
    assert_eq!(orders_where["op"], "in");
    assert_eq!(orders_where["field"], mpack!(["user_id"]));

    // The values array should contain the $query ref
    let values = match &orders_where["values"] {
        QueryValue::List(l) => l,
        other => panic!("expected List, got {other:?}"),
    };
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["$query"], "@users");
    assert_eq!(values[0]["path"], "[].id");
}

// ============================================================================
// query_silent → return_result: false
// ============================================================================

#[test]
fn query_silent_return_result_false() {
    let mut b = Batch::new();
    b.query_silent("helper", Query::from("temp"));
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert_eq!(qv["queries"]["helper"]["return_result"], false);
}

#[test]
fn query_return_result_true_by_default() {
    let mut b = Batch::new();
    b.query("main", Query::from("users"));
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert_eq!(qv["queries"]["main"]["return_result"], true);
}

// ============================================================================
// Batch config: transactional + isolation + durability + return_only + named
// ============================================================================

#[test]
fn batch_config_full() {
    let mut b = Batch::named("my_batch");
    b.id(QueryValue::Int(42));
    b.transactional();
    b.isolation(Isolation::Serializable);
    b.durability(Durability::Synced);
    b.return_only(["x"]);
    b.query("x", Query::from("users"));

    let req = b.build();
    assert_eq!(req.name, Some("my_batch".to_string()));
    assert_eq!(req.id, QueryValue::Int(42));
    assert!(req.transactional);
    assert_eq!(req.isolation, Some("serializable".to_string()));
    assert_eq!(req.durability, Some("synced".to_string()));
    assert!(!req.return_all);
    assert_eq!(req.return_only, Some(vec!["x".to_string()]));
}

#[test]
fn batch_config_return_all_resets_return_only() {
    let mut b = Batch::new();
    b.return_only(["a"]);
    assert!(!b.build().return_all);
    b.return_all();
    let req = b.build();
    assert!(req.return_all);
    assert!(req.return_only.is_none());
}

#[test]
fn batch_isolation_snapshot() {
    let mut b = Batch::new();
    b.isolation(Isolation::Snapshot);
    assert_eq!(b.build().isolation, Some("snapshot".to_string()));
}

#[test]
fn batch_durability_buffered() {
    let mut b = Batch::new();
    b.durability(Durability::Buffered);
    assert_eq!(b.build().durability, Some("buffered".to_string()));
}

// ============================================================================
// Write entries land under the right BatchOp
// ============================================================================

#[test]
fn insert_entry() {
    let mut b = Batch::new();
    let ins = write::insert("users").row(doc().set("name", "Alice"));
    b.insert("ins", ins);
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    let entry = &qv["queries"]["ins"];
    assert!(entry.get("insert_into").is_some());
}

#[test]
fn update_entry() {
    let mut b = Batch::new();
    let upd = write::update("users")
        .where_(filter::eq("id", 1))
        .set(doc().set("name", "Bob"));
    b.update("upd", upd);
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert!(qv["queries"]["upd"].get("update").is_some());
}

#[test]
fn upsert_entry() {
    let mut b = Batch::new();
    let ups = write::upsert("cache")
        .key(shamir_types::mpack!("k1"))
        .value(doc().set("v", 42));
    b.upsert("ups", ups);
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert!(qv["queries"]["ups"].get("set").is_some());
}

#[test]
fn delete_entry() {
    let mut b = Batch::new();
    let del = write::delete("sessions").where_(filter::eq("expired", true));
    b.delete("del", del);
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert!(qv["queries"]["del"].get("delete_from").is_some());
}

// ============================================================================
// $query ref inside a write Doc via set
// ============================================================================

#[test]
fn query_ref_in_write_doc_via_set_expr() {
    let mut b = Batch::new();
    let users = b.query("users", Query::from("users").select(["id"]));
    b.insert(
        "orders",
        write::insert("orders").row(
            doc()
                .set("product", "widget")
                .set("user_id", users.row(0).field("id")),
        ),
    );
    let req = b.build();
    let qv = req.to_query_value().unwrap();

    let values = match &qv["queries"]["orders"]["values"] {
        QueryValue::List(l) => l,
        other => panic!("expected List, got {other:?}"),
    };
    assert_eq!(values.len(), 1);
    let row = &values[0];
    assert_eq!(row["product"], "widget");
    assert_eq!(row["user_id"]["$query"], "@users");
    assert_eq!(row["user_id"]["path"], "[0].id");
}

// ============================================================================
// try_build — happy path
// ============================================================================

#[test]
fn try_build_happy_path() {
    let mut b = Batch::new();
    let users = b.query("users", Query::from("users"));
    b.query(
        "orders",
        Query::from("orders").where_in("uid", [users.column("id")]),
    );
    let result = b.try_build();
    assert!(result.is_ok());
}

// ============================================================================
// try_build — UnknownAlias
// ============================================================================

#[test]
fn try_build_unknown_alias() {
    let mut b = Batch::new();
    // Inject a raw qref to an alias not in the batch
    b.query(
        "orders",
        Query::from("orders").where_eq("uid", qref("nope", "[].id")),
    );
    let result = b.try_build();
    assert!(result.is_err());
    match result.unwrap_err() {
        BuildError::UnknownAlias {
            alias,
            referenced_by,
        } => {
            assert_eq!(alias, "nope");
            assert_eq!(referenced_by, "orders");
        }
        other => panic!("expected UnknownAlias, got {:?}", other),
    }
}

// ============================================================================
// try_build — SelfReference
// ============================================================================

#[test]
fn try_build_self_reference() {
    let mut b = Batch::new();
    b.query(
        "self_ref",
        Query::from("t").where_eq("x", qref("self_ref", "[].id")),
    );
    let result = b.try_build();
    assert!(result.is_err());
    match result.unwrap_err() {
        BuildError::SelfReference { alias } => {
            assert_eq!(alias, "self_ref");
        }
        other => panic!("expected SelfReference, got {:?}", other),
    }
}

// ============================================================================
// op / op_silent escape hatches
// ============================================================================

#[test]
fn op_escape_hatch() {
    use shamir_query_types::batch::BatchOp;
    let mut b = Batch::new();
    let rq = Query::from("users").build();
    b.op("esc", BatchOp::Read(rq));
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert_eq!(qv["queries"]["esc"]["return_result"], true);
    assert!(qv["queries"]["esc"].get("from").is_some());
}

#[test]
fn op_silent_escape_hatch() {
    use shamir_query_types::batch::BatchOp;
    let mut b = Batch::new();
    let rq = Query::from("users").build();
    b.op_silent("esc", BatchOp::Read(rq));
    let req = b.build();
    let qv = req.to_query_value().unwrap();
    assert_eq!(qv["queries"]["esc"]["return_result"], false);
}

// ============================================================================
// Default / new behaviour
// ============================================================================

#[test]
fn batch_new_defaults() {
    let b = Batch::new();
    let req = b.build();
    assert_eq!(req.id, QueryValue::Null);
    assert!(req.name.is_none());
    assert!(!req.transactional);
    assert!(req.isolation.is_none());
    assert!(req.durability.is_none());
    assert!(req.queries.is_empty());
    assert!(req.return_all);
    assert!(req.return_only.is_none());
}

#[test]
fn batch_default_matches_new() {
    let a = Batch::new().build();
    let b = Batch::default().build();
    // Compare via msgpack-decoded QueryValue since BatchRequest doesn't
    // derive Eq with BatchLimits.
    let ja = a.to_query_value().unwrap();
    let jb = b.to_query_value().unwrap();
    assert_eq!(ja, jb);
}

// ============================================================================
// BatchRequest round-trip (serialize → deserialize via msgpack)
// ============================================================================

#[test]
fn batch_request_round_trip() {
    let mut b = Batch::named("rt");
    b.id(QueryValue::Str("req-1".into()));
    b.transactional();
    b.isolation(Isolation::Serializable);
    b.durability(Durability::Synced);
    let users = b.query("users", Query::from("users").select(["id", "name"]));
    b.query(
        "orders",
        Query::from("orders").where_in("user_id", [users.column("id")]),
    );
    let req = b.build();
    let bytes = rmp_serde::to_vec_named(&req).unwrap();
    let req2: shamir_query_types::batch::BatchRequest = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(req.name, req2.name);
    assert_eq!(req.id, req2.id);
    assert_eq!(req.transactional, req2.transactional);
    assert_eq!(req.isolation, req2.isolation);
    assert_eq!(req.durability, req2.durability);
    assert_eq!(req.return_all, req2.return_all);
    assert_eq!(req.queries.len(), req2.queries.len());
}

// ============================================================================
// Limits config
// ============================================================================

#[test]
fn batch_custom_limits() {
    use shamir_query_types::batch::BatchLimits;
    let mut b = Batch::new();
    let custom = BatchLimits {
        max_queries: 5,
        max_dependency_depth: 3,
        max_execution_time_secs: 10,
        max_result_size: 1024,
        max_nesting_depth: 4,
    };
    b.limits(custom.clone());
    let req = b.build();
    assert_eq!(req.limits.max_queries, 5);
    assert_eq!(req.limits.max_dependency_depth, 3);
}

// ============================================================================
// Handle alias() accessor
// ============================================================================

#[test]
fn handle_alias_bare() {
    let mut b = Batch::new();
    let h = b.query("my_query", Query::from("t"));
    assert_eq!(h.alias(), "my_query");
}

// ============================================================================
// IntoBatchOp: raw DTO types
// ============================================================================

#[test]
fn into_batch_op_insert_op() {
    use crate::batch::IntoBatchOp;
    let op = write::insert("t").row(doc().set("a", 1)).build();
    let batch_op = op.into_batch_op();
    match batch_op {
        shamir_query_types::batch::BatchOp::Insert(_) => {}
        _ => panic!("expected Insert"),
    }
}

#[test]
fn into_batch_op_update_op() {
    use crate::batch::IntoBatchOp;
    let op = write::update("t")
        .where_(filter::eq("id", 1))
        .set(doc().set("x", 2))
        .build();
    let batch_op = op.into_batch_op();
    match batch_op {
        shamir_query_types::batch::BatchOp::Update(_) => {}
        _ => panic!("expected Update"),
    }
}

#[test]
fn into_batch_op_set_op() {
    use crate::batch::IntoBatchOp;
    let op = write::upsert("t")
        .key(shamir_types::mpack!("k"))
        .value(doc().set("v", 1))
        .build();
    let batch_op = op.into_batch_op();
    match batch_op {
        shamir_query_types::batch::BatchOp::Set(_) => {}
        _ => panic!("expected Set"),
    }
}

#[test]
fn into_batch_op_delete_op() {
    use crate::batch::IntoBatchOp;
    let op = write::delete("t").where_(filter::eq("id", 1)).build();
    let batch_op = op.into_batch_op();
    match batch_op {
        shamir_query_types::batch::BatchOp::Delete(_) => {}
        _ => panic!("expected Delete"),
    }
}

// ============================================================================
// name() setter vs named() constructor
// ============================================================================

#[test]
fn name_setter() {
    let mut b = Batch::new();
    b.name("late_name");
    assert_eq!(b.build().name, Some("late_name".to_string()));
}

// ============================================================================
// RowRef nested field
// ============================================================================

#[test]
fn row_ref_nested_field() {
    let h = Handle { alias: "q".into() };
    let fv = h.row(1).field(["addr", "zip"]);
    let qv: QueryValue = rmp_serde::from_slice(&rmp_serde::to_vec_named(&fv).unwrap()).unwrap();
    assert_eq!(qv["$query"], "@q");
    assert_eq!(qv["path"], "[1].addr.zip");
}

// ============================================================================
// result_encoding setter
// ============================================================================

#[test]
fn result_encoding_setter_emits_id() {
    let mut b = Batch::new();
    b.query("u", Query::from("users"));
    b.result_encoding(ResultEncoding::Id);
    let req = b.build();
    assert_eq!(req.result_encoding, ResultEncoding::Id);
}

#[test]
fn result_encoding_defaults_to_name_when_unset() {
    let mut b = Batch::new();
    b.query("u", Query::from("users"));
    let req = b.build();
    assert_eq!(req.result_encoding, ResultEncoding::Name);
}

// ============================================================================
// interner_epochs setter
// ============================================================================

#[test]
fn interner_epochs_setter_emits_map() {
    let mut b = Batch::new();
    b.query("u", Query::from("users"));
    let mut epochs: TMap<String, u64> = new_map();
    epochs.insert("repo_a".to_string(), 42);
    epochs.insert("repo_b".to_string(), 7);
    b.interner_epochs(epochs.clone());
    let req = b.build();
    assert_eq!(req.interner_epochs, epochs);
}

#[test]
fn interner_epochs_defaults_to_empty_when_unset() {
    let mut b = Batch::new();
    b.query("u", Query::from("users"));
    let req = b.build();
    assert!(req.interner_epochs.is_empty());
}
