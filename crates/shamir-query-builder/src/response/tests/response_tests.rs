use serde::Deserialize;
use serde_json::json;
use shamir_query_types::batch::{BatchResponse, TransactionInfo};
use shamir_query_types::read::QueryResult;

use crate::batch::Batch;
use crate::response::{BatchResponseExt, ResponseError};
use crate::Query;

// ============================================================================
// Helper: build a BatchResponse from JSON
// ============================================================================

fn resp_from_json(val: serde_json::Value) -> BatchResponse {
    serde_json::from_value(val).expect("BatchResponse should deserialize")
}

/// A small test struct for typed deserialization.
#[derive(Debug, PartialEq, Deserialize)]
struct User {
    id: u64,
    name: String,
}

// ============================================================================
// result / rows — present and absent
// ============================================================================

#[test]
fn result_returns_query_result_when_present() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 1, "name": "Alice"}
                ]
            }
        },
        "execution_plan": [["users"]],
        "execution_time_us": 42
    }));

    let qr = resp.result("users");
    assert!(qr.is_some());
    assert_eq!(qr.unwrap().records.len(), 1);
}

#[test]
fn result_returns_none_when_absent() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    assert!(resp.result("missing").is_none());
}

#[test]
fn rows_returns_records_when_present() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 1, "name": "Alice"},
                    {"id": 2, "name": "Bob"}
                ]
            }
        },
        "execution_plan": [["users"]],
        "execution_time_us": 10
    }));

    let rows = resp.rows("users");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Alice");
}

#[test]
fn rows_returns_empty_slice_when_absent() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let rows = resp.rows("nope");
    assert!(rows.is_empty());
}

// ============================================================================
// rows_as — typed deserialization
// ============================================================================

#[test]
fn rows_as_deserializes_all_records() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 1, "name": "Alice"},
                    {"id": 2, "name": "Bob"}
                ]
            }
        },
        "execution_plan": [["users"]],
        "execution_time_us": 10
    }));

    let users: Vec<User> = resp.rows_as("users").unwrap();
    assert_eq!(users.len(), 2);
    assert_eq!(
        users[0],
        User {
            id: 1,
            name: "Alice".into()
        }
    );
    assert_eq!(
        users[1],
        User {
            id: 2,
            name: "Bob".into()
        }
    );
}

#[test]
fn rows_as_missing_alias_returns_error() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let err = resp.rows_as::<User>("absent").unwrap_err();
    assert!(
        matches!(err, ResponseError::MissingAlias(ref a) if a == "absent"),
        "expected MissingAlias, got: {err}"
    );
}

#[test]
fn rows_as_deserialize_failure() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "bad": {
                "records": [
                    {"id": "not_a_number", "name": "X"}
                ]
            }
        },
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let err = resp.rows_as::<User>("bad").unwrap_err();
    assert!(
        matches!(err, ResponseError::Deserialize { ref alias, .. } if alias == "bad"),
        "expected Deserialize, got: {err}"
    );
}

// ============================================================================
// row_as — single record
// ============================================================================

#[test]
fn row_as_valid_index() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 10, "name": "Zara"}
                ]
            }
        },
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let user: User = resp.row_as("users", 0).unwrap();
    assert_eq!(
        user,
        User {
            id: 10,
            name: "Zara".into()
        }
    );
}

#[test]
fn row_as_out_of_range() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 1, "name": "A"}
                ]
            }
        },
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let err = resp.row_as::<User>("users", 5).unwrap_err();
    match err {
        ResponseError::RowOutOfRange {
            ref alias,
            index,
            len,
        } => {
            assert_eq!(alias, "users");
            assert_eq!(index, 5);
            assert_eq!(len, 1);
        }
        other => panic!("expected RowOutOfRange, got: {other}"),
    }
}

#[test]
fn row_as_missing_alias() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let err = resp.row_as::<User>("gone", 0).unwrap_err();
    assert!(matches!(err, ResponseError::MissingAlias(ref a) if a == "gone"));
}

// ============================================================================
// Handle-keyed methods: get / get_rows / get_as
// ============================================================================

#[test]
fn get_via_handle() {
    let mut b = Batch::new();
    let h = b.query("users", Query::from("users"));

    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 1, "name": "Alice"}
                ]
            }
        },
        "execution_plan": [["users"]],
        "execution_time_us": 5
    }));

    let qr: &QueryResult = resp.get(&h).expect("handle lookup should succeed");
    assert_eq!(qr.records.len(), 1);
}

#[test]
fn get_rows_via_handle() {
    let mut b = Batch::new();
    let h = b.query("orders", Query::from("orders"));

    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "orders": {
                "records": [
                    {"total": 100},
                    {"total": 200}
                ]
            }
        },
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let rows = resp.get_rows(&h);
    assert_eq!(rows.len(), 2);
}

#[test]
fn get_as_via_handle() {
    let mut b = Batch::new();
    let h = b.query("users", Query::from("users"));

    let resp = resp_from_json(json!({
        "id": 1,
        "results": {
            "users": {
                "records": [
                    {"id": 5, "name": "Eve"}
                ]
            }
        },
        "execution_plan": [],
        "execution_time_us": 0
    }));

    let users: Vec<User> = resp.get_as(&h).unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(
        users[0],
        User {
            id: 5,
            name: "Eve".into()
        }
    );
}

#[test]
fn get_via_handle_absent() {
    let mut b = Batch::new();
    let h = b.query("missing", Query::from("missing"));

    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    assert!(resp.get(&h).is_none());
    assert!(resp.get_rows(&h).is_empty());
    assert!(matches!(
        resp.get_as::<User>(&h).unwrap_err(),
        ResponseError::MissingAlias(_)
    ));
}

// ============================================================================
// is_committed / abort_reason / transaction
// ============================================================================

#[test]
fn is_committed_true_when_no_transaction() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0
    }));

    assert!(resp.is_committed());
    assert!(resp.transaction().is_none());
    assert!(resp.abort_reason().is_none());
}

#[test]
fn is_committed_true_when_tx_committed() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0,
        "transaction": {
            "tx_id": 42,
            "status": "committed",
            "snapshot_version": 100,
            "commit_version": 101
        }
    }));

    assert!(resp.is_committed());
    let tx: &TransactionInfo = resp.transaction().unwrap();
    assert_eq!(tx.tx_id, 42);
    assert!(tx.is_committed());
    assert!(resp.abort_reason().is_none());
}

#[test]
fn is_committed_false_when_tx_aborted() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [],
        "execution_time_us": 0,
        "transaction": {
            "tx_id": 7,
            "status": "aborted",
            "reason": "tx_conflict"
        }
    }));

    assert!(!resp.is_committed());
    assert_eq!(resp.abort_reason(), Some("tx_conflict"));
}

// ============================================================================
// execution_plan passthrough
// ============================================================================

#[test]
fn execution_plan_passthrough() {
    let resp = resp_from_json(json!({
        "id": 1,
        "results": {},
        "execution_plan": [
            ["users", "products"],
            ["orders"],
            ["stats"]
        ],
        "execution_time_us": 999
    }));

    let plan = resp.execution_plan();
    assert_eq!(plan.len(), 3);
    assert_eq!(plan[0], vec!["users", "products"]);
    assert_eq!(plan[1], vec!["orders"]);
    assert_eq!(plan[2], vec!["stats"]);
}

// ============================================================================
// ResponseError Display
// ============================================================================

#[test]
fn response_error_display() {
    let e1 = ResponseError::MissingAlias("foo".into());
    assert!(e1.to_string().contains("foo"));

    let e2 = ResponseError::RowOutOfRange {
        alias: "bar".into(),
        index: 5,
        len: 2,
    };
    let msg = e2.to_string();
    assert!(msg.contains("bar"));
    assert!(msg.contains("5"));
    assert!(msg.contains("2"));
}

#[test]
fn response_error_is_std_error() {
    fn assert_error<E: std::error::Error>() {}
    assert_error::<ResponseError>();
}
