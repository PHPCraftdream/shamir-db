//! Deterministic serde tests for the `ListActivity` DTOs (R4b step 1).
//!
//! Fixed fixtures only — no randomness, no timing: the assertions pin the
//! exact wire shape (flat `list_activity` tag, `kind`-tagged rows,
//! snake_case enums, transparent `RequestExecId`) so a shape change is a
//! conscious protocol change.

use serde_json::{json, Value};

use crate::admin::activity::{
    ActivityKind, ActivityPage, ActivityRequestState, ActivityRow, ActivityScope, ActivityTxState,
    CancelOutcome, ListActivityReq, RequestExecId,
};
use crate::batch::BatchOp;
use crate::wire::{error_code, DbResponse};

fn full_request() -> ListActivityReq {
    ListActivityReq {
        scope: ActivityScope::Own,
        kind: ActivityKind::Requests,
        limit: Some(64),
        continuation: Some("req:42:7".to_string()),
    }
}

#[test]
fn list_activity_req_wire_shape_is_flat_tagged() {
    let value = serde_json::to_value(full_request()).expect("serialize");
    assert_eq!(
        value,
        json!({
            "list_activity": true,
            "scope": "own",
            "kind": "requests",
            "limit": 64,
            "continuation": "req:42:7",
        })
    );
}

#[test]
fn list_activity_req_omits_unset_options() {
    let request = ListActivityReq {
        scope: ActivityScope::All,
        kind: ActivityKind::All,
        limit: None,
        continuation: None,
    };
    let value = serde_json::to_value(request).expect("serialize");
    assert_eq!(
        value,
        json!({
            "list_activity": true,
            "scope": "all",
            "kind": "all",
        })
    );
}

#[test]
fn list_activity_req_msgpack_round_trips() {
    let request = full_request();
    let bytes = rmp_serde::to_vec_named(&request).expect("encode");
    let decoded: ListActivityReq = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, request);
}

#[test]
fn list_activity_req_rejects_unknown_fields() {
    let bytes = rmp_serde::to_vec_named(&json!({
        "list_activity": true,
        "scope": "all",
        "kind": "all",
        "filter_value": "secret-literal",
    }))
    .expect("encode");
    let result: Result<ListActivityReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "unknown fields must be denied");
}

#[test]
fn list_activity_req_requires_scope_and_kind() {
    let bytes = rmp_serde::to_vec_named(&json!({
        "list_activity": true,
        "kind": "all",
    }))
    .expect("encode");
    let result: Result<ListActivityReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "missing scope must be rejected");
    let bytes = rmp_serde::to_vec_named(&json!({
        "list_activity": true,
        "scope": "own",
    }))
    .expect("encode");
    let result: Result<ListActivityReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "missing kind must be rejected");
}

fn sample_rows() -> Vec<ActivityRow> {
    vec![
        ActivityRow::Session {
            session_handle: 1,
            user: "alice".to_string(),
            transport: "tcp".to_string(),
            created_at_ns: 1_700_000_000_000_000_000,
            idle_ns: 250_000_000,
            open_transactions: 1,
            open_cursors: 2,
        },
        ActivityRow::Request {
            request_exec_id: RequestExecId(42),
            session_handle: Some(1),
            user: "alice".to_string(),
            db: "main".to_string(),
            op_kind: "read".to_string(),
            state: ActivityRequestState::Running,
            elapsed_ns: 12_345,
        },
        ActivityRow::Transaction {
            tx_handle: 7,
            session_handle: 1,
            user: "alice".to_string(),
            state: ActivityTxState::AwaitingCommit,
            idle_ns: 900_000,
        },
        ActivityRow::Cursor {
            cursor_id: 11,
            session_handle: 1,
            user: "alice".to_string(),
            table: "users".to_string(),
            pinned_version: 321,
            open_ns: 45_000,
        },
    ]
}

/// Build a request row with the given optional session handle so each
/// case below enumerates every real field (enum variants cannot be
/// functional-update constructed).
fn request_row(session_handle: Option<u64>) -> ActivityRow {
    ActivityRow::Request {
        request_exec_id: RequestExecId(7),
        session_handle,
        user: "alice".to_string(),
        db: "main".to_string(),
        op_kind: "read".to_string(),
        state: ActivityRequestState::Running,
        elapsed_ns: 1,
    }
}

#[test]
fn request_row_session_handle_present_vs_absent_on_wire() {
    // Present: the key is rendered with the handle value.
    let present = request_row(Some(1));
    let value = serde_json::to_value(&present).expect("serialize");
    assert_eq!(value.get("session_handle"), Some(&json!(1)));

    // Absent: the key is omitted entirely (no null placeholder), and
    // deserialization defaults it back to `None`.
    let absent = request_row(None);
    let value = serde_json::to_value(&absent).expect("serialize");
    assert!(value.is_object());
    assert!(
        value.get("session_handle").is_none(),
        "absent handle must not render as null"
    );
    let decoded: ActivityRow = serde_json::from_value(value).expect("decode");
    assert_eq!(decoded, absent);
}

#[test]
fn activity_rows_are_kind_tagged_snake_case() {
    let rows = sample_rows();
    for (row, expected_kind) in rows
        .iter()
        .zip(["session", "request", "transaction", "cursor"])
    {
        let kind = match serde_json::to_value(row).expect("serialize") {
            Value::Object(map) => map.get("kind").cloned().expect("kind tag"),
            other => panic!("row must serialize to a map, got {other}"),
        };
        assert_eq!(kind, json!(expected_kind));
    }
}

#[test]
fn activity_request_state_vocabulary_is_pinned() {
    assert_eq!(
        serde_json::to_value(ActivityRequestState::Running).unwrap(),
        json!("running")
    );
    assert_eq!(
        serde_json::to_value(ActivityRequestState::Done).unwrap(),
        json!("done")
    );
    for (state, name) in [
        (ActivityRequestState::Committing, "committing"),
        (ActivityRequestState::Cancelled, "cancelled"),
        (ActivityRequestState::WaitingLock, "waiting_lock"),
        (
            ActivityRequestState::WaitingVisibility,
            "waiting_visibility",
        ),
        (ActivityRequestState::WaitingBudget, "waiting_budget"),
    ] {
        let wire = json!(name);
        assert_eq!(serde_json::to_value(state).expect("encode state"), wire);
        let decoded = serde_json::from_value::<ActivityRequestState>(wire).expect("decode state");
        assert_eq!(decoded, state);
    }
    assert_eq!(
        serde_json::to_value(ActivityTxState::Active).unwrap(),
        json!("active")
    );
    assert_eq!(
        serde_json::to_value(ActivityTxState::AwaitingCommit).unwrap(),
        json!("awaiting_commit")
    );
    assert_eq!(
        serde_json::to_value(ActivityTxState::ExpiredPendingReap).unwrap(),
        json!("expired_pending_reap")
    );
}

#[test]
fn request_exec_id_is_wire_transparent() {
    assert_eq!(
        serde_json::to_value(RequestExecId(42)).expect("serialize"),
        json!(42)
    );
}

#[test]
fn activity_page_round_trips_with_and_without_continuation() {
    for continuation in [Some("cursors:9:15".to_string()), None] {
        let page = ActivityPage::new(sample_rows(), continuation);
        let bytes = rmp_serde::to_vec_named(&page).expect("encode");
        let decoded: ActivityPage = rmp_serde::from_slice(&bytes).expect("decode");
        assert_eq!(decoded, page);
    }
    let exhausted = ActivityPage::new(Vec::new(), None);
    let value = serde_json::to_value(&exhausted).expect("serialize");
    assert!(
        value.get("continuation").is_none(),
        "exhausted page must omit the continuation key"
    );
}

#[test]
fn db_response_activity_page_round_trips() {
    let page = ActivityPage::new(sample_rows(), Some("sessions:2:0".to_string()));
    let response = DbResponse::ActivityPage { page: page.clone() };
    let bytes = rmp_serde::to_vec_named(&response).expect("encode");
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).expect("decode");
    match &decoded {
        DbResponse::ActivityPage { page: decoded_page } => assert_eq!(decoded_page, &page),
        _ => panic!("expected ActivityPage variant"),
    }
    let value = serde_json::to_value(&decoded).expect("serialize");
    assert_eq!(value.get("kind"), Some(&json!("activity_page")));
}

#[test]
fn cancel_request_wire_shape_and_classification() {
    let op = BatchOp::CancelRequest(RequestExecId(u64::MAX));
    let expected = json!({"cancel_request": u64::MAX});
    assert_eq!(serde_json::to_value(&op).expect("encode JSON"), expected);
    assert_eq!(
        serde_json::from_value::<BatchOp>(expected.clone()).expect("decode JSON"),
        op
    );
    let bytes = rmp_serde::to_vec_named(&op).expect("encode MessagePack");
    assert_eq!(
        rmp_serde::from_slice::<Value>(&bytes).expect("wire map"),
        expected
    );
    assert_eq!(
        rmp_serde::from_slice::<BatchOp>(&bytes).expect("decode MessagePack"),
        op
    );
    assert!(op.is_admin());
    assert!(!op.is_write());
    assert_eq!(op.table_ref(), None);
    assert_eq!(op.required_access("main"), None);
    assert_eq!(op.activity_name(), "admin:cancel_request");
}

#[test]
fn cancel_request_rejects_invalid_wire_shapes() {
    for invalid in [
        r#"{"cancel_request":true}"#,
        r#"{"cancel_request":null}"#,
        r#"{"cancel_request":-1}"#,
        r#"{"cancel_request":"42"}"#,
        r#"{"cancel_request":{"request_exec_id":42}}"#,
        r#"{"cancel_request":42,"extra":0}"#,
        r#"{"cancel_request":42,"from":"users"}"#,
        r#"{"cancel_request":42,"cancel_request":43}"#,
    ] {
        assert!(
            serde_json::from_str::<BatchOp>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn cancel_outcomes_have_exact_tagged_response_shape() {
    for (outcome, name) in [
        (CancelOutcome::Cancelled, "cancelled"),
        (CancelOutcome::AlreadyFinished, "already_finished"),
        (CancelOutcome::AlreadyCommitting, "already_committing"),
        (CancelOutcome::NotCancellable, "not_cancellable"),
    ] {
        let response = DbResponse::CancelOutcome { outcome };
        let expected = json!({"kind": "cancel_outcome", "outcome": name});
        assert_eq!(
            serde_json::to_value(&response).expect("encode JSON"),
            expected
        );
        let bytes = rmp_serde::to_vec_named(&response).expect("encode MessagePack");
        assert_eq!(
            rmp_serde::from_slice::<Value>(&bytes).expect("wire map"),
            expected
        );
        for decoded in [
            serde_json::from_value::<DbResponse>(expected).expect("decode JSON"),
            rmp_serde::from_slice::<DbResponse>(&bytes).expect("decode MessagePack"),
        ] {
            assert!(
                matches!(decoded, DbResponse::CancelOutcome { outcome: got } if got == outcome)
            );
        }
    }
}

#[test]
fn cancelled_is_an_ordinary_wire_error_code() {
    let response = DbResponse::Error {
        code: error_code::CANCELLED.to_string(),
        message: "request cancelled".to_string(),
    };
    assert_eq!(
        serde_json::to_value(response).expect("encode error"),
        json!({"kind": "error", "code": "cancelled", "message": "request cancelled"})
    );
}
