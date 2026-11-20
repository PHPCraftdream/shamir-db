//! Byte/struct golden for the activity-view listing builder (R4b step 1).
//!
//! Wire contract (matches the protocol DTO's hand-written `Serialize`):
//! `BatchOp::ListActivity` flattens the request into a flat op map with a
//! presence-only `list_activity: true` discriminator sitting NEXT TO the
//! request fields — not nested under it. `limit` / `continuation` are
//! omitted when absent. Assertions are key-set based, independent of the
//! map ordering `serde_json` chooses.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::wire::admin::{cancel_request, list_activity};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityPage, ActivityRequestState, ActivityRow, ActivityScope, ListActivityReq,
    RequestExecId,
};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::wire::DbResponse;

fn populated_request() -> ListActivityReq {
    ListActivityReq {
        scope: ActivityScope::All,
        kind: ActivityKind::Requests,
        limit: Some(50),
        continuation: Some("tok".to_string()),
    }
}

#[test]
fn list_activity_golden_flat_discriminator_shape() {
    let op = list_activity(populated_request());
    let bytes = rmp_serde::to_vec_named(&op).expect("operation encodes");
    let value: serde_json::Value = rmp_serde::from_slice(&bytes).expect("golden decodes");
    let map = value.as_object().expect("flat op map");

    let mut expected = std::collections::BTreeSet::from([
        "list_activity",
        "scope",
        "kind",
        "limit",
        "continuation",
    ]);
    for key in map.keys() {
        assert!(
            expected.remove(key.as_str()),
            "unexpected key {key} in op map"
        );
    }
    assert!(expected.is_empty(), "missing keys: {expected:?} in {map:?}");

    assert_eq!(map["list_activity"], serde_json::Value::Bool(true));
    assert_eq!(map["scope"], "all");
    assert_eq!(map["kind"], "requests");
    assert_eq!(map["limit"], 50);
    assert_eq!(map["continuation"], "tok");
}

#[test]
fn list_activity_absent_options_are_omitted_not_null() {
    let request = ListActivityReq {
        scope: ActivityScope::Own,
        kind: ActivityKind::All,
        limit: None,
        continuation: None,
    };
    let op = list_activity(request);
    let bytes = rmp_serde::to_vec_named(&op).expect("operation encodes");
    let value: serde_json::Value = rmp_serde::from_slice(&bytes).expect("golden decodes");
    let map = value.as_object().expect("flat op map");

    let keys: std::collections::BTreeSet<&str> = map.keys().map(String::as_str).collect();
    let expected: std::collections::BTreeSet<&str> =
        ["list_activity", "scope", "kind"].into_iter().collect();
    assert_eq!(keys, expected, "no null placeholders, no extra fields");
    assert_eq!(map["list_activity"], serde_json::Value::Bool(true));
    assert_eq!(map["scope"], "own");
    assert_eq!(map["kind"], "all");
}

#[test]
fn list_activity_round_trips_via_msgpack_batch() {
    let request = populated_request();
    let mut b = Batch::new();
    b.op("activity", list_activity(request.clone()));

    let via_build = b.build();
    let via_msgpack = b
        .to_request_via_msgpack()
        .expect("batch encodes and decodes");
    assert_eq!(via_build, via_msgpack, "round trip preserves the batch");

    let entry = via_msgpack
        .queries
        .values()
        .next()
        .expect("the activity entry survives the round trip");
    assert_eq!(entry.op, BatchOp::ListActivity(request));
}

/// Golden for the REPLY side (the builder itself only produces the
/// request): a `Request` row's `session_handle` renders as a plain number
/// when present and is OMITTED entirely when absent (never null), matching
/// the DTO's `skip_serializing_if` / `default` serde contract.
fn request_row(session_handle: Option<u64>) -> ActivityRow {
    ActivityRow::Request {
        request_exec_id: RequestExecId(42),
        session_handle,
        user: "alice".to_string(),
        db: "main".to_string(),
        op_kind: "read".to_string(),
        state: ActivityRequestState::Running,
        elapsed_ns: 12_345,
    }
}

fn page_value(row: ActivityRow) -> serde_json::Value {
    let response = DbResponse::ActivityPage {
        page: ActivityPage::new(vec![row], None),
    };
    let bytes = rmp_serde::to_vec_named(&response).expect("response encodes");
    rmp_serde::from_slice(&bytes).expect("golden decodes")
}

#[test]
fn activity_page_request_row_present_session_handle_is_rendered() {
    let value = page_value(request_row(Some(1)));
    let rows = value["page"]["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "request");
    assert_eq!(rows[0]["session_handle"], 1);
}

#[test]
fn activity_page_request_row_absent_session_handle_is_omitted() {
    let value = page_value(request_row(None));
    let rows = value["page"]["rows"].as_array().expect("rows array");
    assert_eq!(rows[0]["kind"], "request");
    assert!(
        rows[0].get("session_handle").is_none(),
        "absent handle must be omitted, not null: {rows:?}"
    );
}

#[test]
fn cancel_request_builds_one_typed_nontransactional_operation() {
    let id = RequestExecId(42);
    let mut builder = Batch::new();
    builder.op("cancel_request", cancel_request(id));

    let built = builder.build();
    let decoded = builder.to_request_via_msgpack().expect("batch round trip");
    assert_eq!(built, decoded);
    assert!(!decoded.transactional);
    assert_eq!(decoded.queries.len(), 1);
    let entry = decoded.queries.values().next().expect("sole operation");
    assert_eq!(entry.op, BatchOp::CancelRequest(id));
}
