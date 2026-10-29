//! Shared first-party operation wire matrix and DTO round trips.

use serde_json::{json, Value};
use shamir_query_builder::{
    batch::Batch, ddl, filter, write, EventMask, Query, SourceBuilder, Subscribe,
};
use shamir_query_types::batch::{BatchOp, BatchRequest};
use shamir_query_types::WriteOp;

fn fixtures() -> Vec<Value> {
    serde_json::from_str(include_str!("fixtures/wire_ops_matrix.json")).expect("fixture JSON")
}

fn rust_op(name: &str) -> Option<Value> {
    let op = match name {
        "read_complex" => BatchOp::Read(
            Query::from("users")
                .select(["name"])
                .where_(filter::and([
                    filter::eq("active", true),
                    filter::or([
                        filter::in_("role", ["admin", "user"]),
                        filter::not(filter::like("name", "x%")),
                        filter::regex("name", "^A"),
                    ]),
                ]))
                .group_by("name")
                .having(filter::eq("active", true))
                .order_by_desc("name")
                .limit(10)
                .offset(2)
                .build(),
        ),
        "read_page" => BatchOp::Read(Query::from("users").select(["name"]).page(2, 25).build()),
        "insert" => BatchOp::Insert(
            write::insert("users")
                .row(write::doc().set("name", "Ada").unwrap())
                .build(),
        ),
        "update" => BatchOp::Update(
            write::update("users")
                .set(write::doc().set("name", "Ada").unwrap())
                .where_(filter::eq("id", 1))
                .expected_version(7)
                .build()
                .unwrap(),
        ),
        "delete" => BatchOp::Delete(
            write::delete("users")
                .where_(filter::eq("id", 1))
                .build()
                .unwrap(),
        ),
        "upsert" => BatchOp::Set(
            write::upsert("users")
                .key(write::doc().set("id", 1).unwrap())
                .value(write::doc().set("name", "Ada").unwrap())
                .build()
                .unwrap(),
        ),
        "create_table" => ddl::create_table("users").if_not_exists().build(),
        "drop_index" => ddl::drop_index("idx", "users")
            .hmac("aa")
            .if_exists()
            .build()
            .unwrap(),
        "rename_table" => ddl::rename_table("users", "people").build(),
        "get_schema" => ddl::get_table_schema("users").build(),
        "remove_schema_rule" => ddl::remove_schema_rule("users", ["name"]).build(),
        "add_schema_rule" => ddl::add_schema_rule("users")
            .rule(ddl::field(["email"]).string().required().max_len(255))
            .build()
            .unwrap(),
        "set_schema" => ddl::set_table_schema("users")
            .rules([ddl::field(["email"]).string().required().build()])
            .expected_version(3)
            .build(),
        "create_validator" => ddl::create_validator("validate")
            .source("source")
            .build()
            .unwrap(),
        "bind_validator" => ddl::bind_validator("validate", "users")
            .db("app")
            .ops([WriteOp::Insert, WriteOp::Update])
            .priority(1600)
            .build(),
        "unbind_validator" => ddl::unbind_validator("validate", "users").db("app").build(),
        "drop_validator" => ddl::drop_validator("validate")
            .hmac("aa")
            .if_exists()
            .build()
            .unwrap(),
        "rename_validator" => ddl::rename_validator("validate", "validate2"),
        "list_validators" => ddl::list_validators("users").db("app").build(),
        "call" => {
            let mut batch = Batch::new();
            batch.call("c", "proc", [1]);
            batch.build().queries["c"].op.clone()
        }
        "sub_batch_rust" => {
            let mut inner = Batch::new();
            inner.id(1);
            let mut bind = shamir_query_builder::shamir_collections::new_map();
            bind.insert("x".to_owned(), 1.into());
            let mut outer = Batch::new();
            outer.sub_batch("nested", inner.build(), bind);
            outer.build().queries["nested"].op.clone()
        }
        "for_each_rust" => {
            let mut inner = Batch::new();
            inner.id(1);
            let mut outer = Batch::new();
            outer.for_each(
                "loop",
                vec![
                    shamir_query_builder::val::lit(1),
                    shamir_query_builder::val::lit(2),
                ],
                "row",
                inner.build(),
            );
            outer.build().queries["loop"].op.clone()
        }
        "subscribe" => BatchOp::Subscribe(
            Subscribe::source(SourceBuilder::table("users").events(EventMask::Put).build())
                .deliver_keys()
                .with_initial()
                .from_version(7)
                .build(),
        ),
        _ => return None,
    };
    Some(serde_json::to_value(op).expect("serialize builder op"))
}

fn rust_request() -> Value {
    let mut batch = Batch::new();
    batch.id(1);
    let first = batch.insert(
        "first",
        write::insert("users").row(write::doc().set("name", "Ada").unwrap()),
    );
    let second = batch.op_silent(
        "second",
        shamir_query_builder::CallOp {
            call: "proc".to_owned(),
            params: vec![1.into()],
            repo: "main".to_owned(),
        },
    );
    batch
        .after(&second, &first)
        .when(&second, filter::value_eq(1, 1));
    let limits = shamir_query_types::batch::BatchLimits {
        max_queries: 20,
        ..Default::default()
    };
    batch.limits(limits);
    serde_json::to_value(batch.build()).unwrap()
}

fn decoded(wire: &Value, request: bool) -> (Value, Value) {
    let bytes = rmp_serde::to_vec_named(wire).expect("encode fixture");
    if request {
        let json: BatchRequest = serde_json::from_value(wire.clone()).expect("JSON request");
        let msgpack: BatchRequest = rmp_serde::from_slice(&bytes).expect("msgpack request");
        (
            serde_json::to_value(json).unwrap(),
            serde_json::to_value(msgpack).unwrap(),
        )
    } else {
        let json: BatchOp = serde_json::from_value(wire.clone()).expect("JSON op");
        let msgpack: BatchOp = rmp_serde::from_slice(&bytes).expect("msgpack op");
        (
            serde_json::to_value(json).unwrap(),
            serde_json::to_value(msgpack).unwrap(),
        )
    }
}

#[test]
fn unknown_operation_key_is_rejected_by_both_decoders() {
    let wire = json!({
        "insert_into": "users",
        "values": [],
        "unexpected_key": 1
    });
    let bytes = rmp_serde::to_vec_named(&wire).expect("encode fixture");
    let json_err = serde_json::from_value::<BatchOp>(wire)
        .unwrap_err()
        .to_string();
    let msgpack_err = rmp_serde::from_slice::<BatchOp>(&bytes)
        .unwrap_err()
        .to_string();
    assert!(json_err.contains("unexpected_key"), "{json_err}");
    assert!(msgpack_err.contains("unexpected_key"), "{msgpack_err}");
}

// The filter family stays lenient (shared with persisted defaults), so there
// the round trip is what exposes a key the DTO would silently drop.
#[test]
fn unknown_key_in_lenient_filter_is_detected_by_round_trip() {
    let wire = json!({
        "from": "users",
        "where": {
            "op": "eq",
            "field": ["id"],
            "value": 1,
            "unexpected_key": 1
        }
    });
    let (json, msgpack) = decoded(&wire, false);
    assert_ne!(json["where"], wire["where"]);
    assert_eq!(
        json["where"],
        json!({
            "op": "eq",
            "field": ["id"],
            "value": 1
        })
    );
    assert_eq!(msgpack, json);
}

#[test]
fn existing_create_index_matrix_round_trips_without_dropped_keys() {
    let matrix: Value =
        serde_json::from_str(include_str!("fixtures/create_index_matrix.json")).unwrap();
    for case in matrix["cases"].as_array().unwrap() {
        if case["expect"] != "accept" {
            continue;
        }
        let hex = case["wire_hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let wire: Value = rmp_serde::from_slice(&bytes).unwrap();
        let (json, msgpack) = decoded(&wire, false);
        assert_eq!(json, wire, "{}", case["name"]);
        assert_eq!(msgpack, wire, "{}", case["name"]);
    }
}

#[test]
fn fixture_inventory_is_complete() {
    let cases = fixtures();
    let names: Vec<&str> = cases
        .iter()
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "read_complex",
            "read_page",
            "read_default_ts",
            "insert",
            "update",
            "delete",
            "upsert",
            "create_table",
            "drop_index",
            "rename_table",
            "get_schema",
            "remove_schema_rule",
            "add_schema_rule",
            "set_schema",
            "create_validator",
            "bind_validator",
            "unbind_validator",
            "drop_validator",
            "rename_validator",
            "list_validators",
            "call",
            "subscribe",
            "subscribe_default_ts",
            "batch_rust",
            "batch_ts",
            "sub_batch_rust",
            "for_each_rust",
            "sub_batch_ts",
            "for_each_ts",
        ]
    );
}

#[test]
fn builder_parity_and_strict_dto_round_trips() {
    for case in fixtures() {
        let name = case["name"].as_str().unwrap();
        let builder = case["builder"].as_str().unwrap();
        let wire = &case["wire"];
        if builder.contains("rust:") {
            let actual = if name == "batch_rust" {
                rust_request()
            } else {
                rust_op(name).expect("missing Rust builder")
            };
            assert_eq!(actual, *wire, "{name}");
        }
        if case.get("known_gap").is_none() {
            let (json, msgpack) = decoded(wire, name == "batch_ts" || name == "batch_rust");
            assert_eq!(json, *wire, "JSON {name}");
            assert_eq!(msgpack, *wire, "msgpack {name}");
        }
    }
}

fn request_defaults(request: &mut Value) {
    request["transactional"] = json!(false);
    request["return_all"] = json!(true);
    request["result_encoding"] = json!("name");
    if request.get("limits").is_none() {
        request["limits"] = json!({
            "max_queries": 50,
            "max_dependency_depth": 10,
            "max_execution_time_secs": 30,
            "max_result_size": 10485760,
            "max_nesting_depth": 4,
            "max_iterations": 1000
        });
    }
    let limits = &mut request["limits"];
    limits["max_total_queries"] = json!(10000);
    limits["max_total_work"] = json!(1000000);
    limits["max_page_size"] = json!(100000);
    limits["max_offset"] = json!(1000000);
}

#[test]
fn known_gaps_are_exact_and_round_trip_differences_are_pinned() {
    let cases = fixtures();
    let gaps: Vec<&str> = cases
        .iter()
        .filter(|c| c.get("known_gap").is_some())
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        gaps,
        [
            "read_default_ts",
            "subscribe_default_ts",
            "batch_ts",
            "sub_batch_ts",
            "for_each_ts"
        ]
    );
    for case in cases.iter().filter(|c| c.get("known_gap").is_some()) {
        let name = case["name"].as_str().unwrap();
        let wire = &case["wire"];
        let mut expected = wire.clone();
        match name {
            "read_default_ts" => {
                expected["select"] = json!({
                    "items": [
                        {
                            "type": "all"
                        }
                    ],
                    "distinct": false
                })
            }
            "subscribe_default_ts" => {
                expected["subscribe"][0]["events"] = json!("all");
                expected["deliver"] = json!("records");
                expected["initial"] = json!(false);
            }
            "batch_ts" => {
                request_defaults(&mut expected);
                expected["queries"]["first"]["return_result"] = json!(true);
            }
            "sub_batch_ts" => request_defaults(&mut expected["batch"]),
            "for_each_ts" => request_defaults(&mut expected["for_each"]),
            _ => panic!("unexpected gap {name}"),
        }
        let (json, msgpack) = decoded(wire, name == "batch_ts" || name == "batch_rust");
        assert_ne!(expected, *wire, "gap disappeared: {name}");
        assert_eq!(json, expected, "JSON {name}");
        assert_eq!(msgpack, expected, "msgpack {name}");
    }
}
