use serde::Deserialize;
use shamir_collections::TMap;
use shamir_query_types::batch::{BatchResponse, TransactionInfo};
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;

use crate::batch::Batch;
use crate::response::{BatchResponseExt, ResponseError};
use crate::Query;

// ============================================================================
// Helpers: build test fixtures using QueryValue directly
// ============================================================================

/// Build a `QueryRecord::Direct` from a `QueryValue` map.
fn record(v: QueryValue) -> QueryRecord {
    QueryRecord::from(v)
}

/// Build a `QueryResult` from a vec of `QueryRecord`s.
fn qresult(records: Vec<QueryRecord>) -> QueryResult {
    QueryResult {
        records,
        stats: None,
        pagination: None,
        value: None,
        explain: None,
    }
}

/// Build a minimal `BatchResponse` with the given results map.
fn batch_resp(
    results: TMap<String, QueryResult>,
    execution_plan: Vec<Vec<String>>,
    transaction: Option<TransactionInfo>,
) -> BatchResponse {
    BatchResponse {
        id: QueryValue::Int(1),
        results,
        execution_plan,
        execution_time_us: 0,
        transaction,
        interner_delta: TMap::default(),
        edge_provenance: TMap::default(),
    }
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
    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![record(mpack!({"id": 1, "name": "Alice"}))]),
    );
    let resp = batch_resp(results, vec![vec!["users".to_owned()]], None);

    let qr = resp.result("users");
    assert!(qr.is_some());
    assert_eq!(qr.unwrap().records.len(), 1);
}

#[test]
fn result_returns_none_when_absent() {
    let resp = batch_resp(TMap::default(), vec![], None);

    assert!(resp.result("missing").is_none());
}

#[test]
fn rows_returns_records_when_present() {
    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![
            record(mpack!({"id": 1, "name": "Alice"})),
            record(mpack!({"id": 2, "name": "Bob"})),
        ]),
    );
    let resp = batch_resp(results, vec![vec!["users".to_owned()]], None);

    let rows = resp.rows("users");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get_value_owned("name"),
        Some(QueryValue::Str("Alice".to_owned()))
    );
}

#[test]
fn rows_returns_empty_slice_when_absent() {
    let resp = batch_resp(TMap::default(), vec![], None);

    let rows = resp.rows("nope");
    assert!(rows.is_empty());
}

// ============================================================================
// rows_as — typed deserialization
// ============================================================================

#[test]
fn rows_as_deserializes_all_records() {
    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![
            record(mpack!({"id": 1, "name": "Alice"})),
            record(mpack!({"id": 2, "name": "Bob"})),
        ]),
    );
    let resp = batch_resp(results, vec![vec!["users".to_owned()]], None);

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
    let resp = batch_resp(TMap::default(), vec![], None);

    let err = resp.rows_as::<User>("absent").unwrap_err();
    assert!(
        matches!(err, ResponseError::MissingAlias(ref a) if a == "absent"),
        "expected MissingAlias, got: {err}"
    );
}

#[test]
fn rows_as_deserialize_failure() {
    // "id" is a string here — should fail to deserialize into `User.id: u64`.
    let mut results = TMap::default();
    results.insert(
        "bad".to_owned(),
        qresult(vec![record(mpack!({"id": "not_a_number", "name": "X"}))]),
    );
    let resp = batch_resp(results, vec![], None);

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
    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![record(mpack!({"id": 10, "name": "Zara"}))]),
    );
    let resp = batch_resp(results, vec![], None);

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
    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![record(mpack!({"id": 1, "name": "A"}))]),
    );
    let resp = batch_resp(results, vec![], None);

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
    let resp = batch_resp(TMap::default(), vec![], None);

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

    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![record(mpack!({"id": 1, "name": "Alice"}))]),
    );
    let resp = batch_resp(results, vec![vec!["users".to_owned()]], None);

    let qr: &QueryResult = resp.get(&h).expect("handle lookup should succeed");
    assert_eq!(qr.records.len(), 1);
}

#[test]
fn get_rows_via_handle() {
    let mut b = Batch::new();
    let h = b.query("orders", Query::from("orders"));

    let mut results = TMap::default();
    results.insert(
        "orders".to_owned(),
        qresult(vec![
            record(mpack!({"total": 100})),
            record(mpack!({"total": 200})),
        ]),
    );
    let resp = batch_resp(results, vec![], None);

    let rows = resp.get_rows(&h);
    assert_eq!(rows.len(), 2);
}

#[test]
fn get_as_via_handle() {
    let mut b = Batch::new();
    let h = b.query("users", Query::from("users"));

    let mut results = TMap::default();
    results.insert(
        "users".to_owned(),
        qresult(vec![record(mpack!({"id": 5, "name": "Eve"}))]),
    );
    let resp = batch_resp(results, vec![], None);

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

    let resp = batch_resp(TMap::default(), vec![], None);

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
    let resp = batch_resp(TMap::default(), vec![], None);

    assert!(resp.is_committed());
    assert!(resp.transaction().is_none());
    assert!(resp.abort_reason().is_none());
}

#[test]
fn is_committed_true_when_tx_committed() {
    let tx = TransactionInfo {
        tx_id: 42,
        status: "committed".to_owned(),
        snapshot_version: Some(100),
        commit_version: Some(101),
        reason: None,
        materialized: true,
    };
    let resp = batch_resp(TMap::default(), vec![], Some(tx));

    assert!(resp.is_committed());
    let tx: &TransactionInfo = resp.transaction().unwrap();
    assert_eq!(tx.tx_id, 42);
    assert!(tx.is_committed());
    assert!(resp.abort_reason().is_none());
}

#[test]
fn is_committed_false_when_tx_aborted() {
    let tx = TransactionInfo {
        tx_id: 7,
        status: "aborted".to_owned(),
        snapshot_version: None,
        commit_version: None,
        reason: Some("tx_conflict".to_owned()),
        materialized: true,
    };
    let resp = batch_resp(TMap::default(), vec![], Some(tx));

    assert!(!resp.is_committed());
    assert_eq!(resp.abort_reason(), Some("tx_conflict"));
}

// ============================================================================
// execution_plan passthrough
// ============================================================================

#[test]
fn execution_plan_passthrough() {
    let plan = vec![
        vec!["users".to_owned(), "products".to_owned()],
        vec!["orders".to_owned()],
        vec!["stats".to_owned()],
    ];
    let resp = batch_resp(TMap::default(), plan, None);

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
