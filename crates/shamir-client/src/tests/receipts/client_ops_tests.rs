//! Production-path tests over a mock transport request log: the injectable
//! `send` closure records every request the client would put on the wire,
//! so assertions cover request construction, the no-resend rule, and
//! response decoding — the same request/response path production uses.

use std::sync::{Arc, Mutex};

use shamir_query_types::admin::ChangesSinceOp;
use shamir_query_types::batch::{
    BatchOp, BatchRequest, BatchResponse, TransactionInfo, VisibilityPendingReason,
    VisibilityState, VisibilityWait,
};
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;

use crate::builder::batch::Batch;
use crate::builder::write::insert;
use crate::builder::write::Insert;
use crate::receipts::{changes_since_via, execute_with_receipts_via};
use crate::{ChangesSinceResponse, ClientError, DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

const BIG: u64 = 9_007_199_254_740_993; // 2^53 + 1

fn pending(visible_through: u64, reason: VisibilityPendingReason) -> VisibilityState {
    VisibilityState::Pending {
        visible_through,
        blocking_version: visible_through + 1,
        reason,
    }
}

fn shell(receipt: TransactionInfo) -> shamir_query_types::read::QueryResult {
    shamir_query_types::read::QueryResult {
        receipt: Some(receipt),
        ..Default::default()
    }
}

fn response_with(
    results: impl IntoIterator<Item = (&'static str, TransactionInfo)>,
) -> BatchResponse {
    let mut map = TMap::default();
    for (alias, receipt) in results {
        map.insert(alias.to_string(), shell(receipt));
    }
    BatchResponse {
        id: QueryValue::Null,
        results: map,
        execution_plan: Vec::new(),
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    }
}

fn two_write_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.insert("w1", insert("items").row(QueryValue::Null));
    batch.insert(
        "w2",
        Insert::with_repo("hot", "sessions").row(QueryValue::Null),
    );
    batch.build()
}

#[tokio::test]
async fn execute_with_receipts_sends_implicit_batch_and_extracts_receipts() {
    let log: Arc<Mutex<Vec<BatchRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    let response = response_with([
        ("w1", TransactionInfo::committed(7, 40, 43, true)),
        (
            "w2",
            TransactionInfo::committed(8, 40, 44, true)
                .with_visibility(pending(43, VisibilityPendingReason::Timeout)),
        ),
    ]);
    let returned = response.clone();
    let (_, receipts) = execute_with_receipts_via(
        move |batch: BatchRequest| {
            capture.lock().unwrap().push(batch);
            let response = returned.clone();
            async move { Ok::<BatchResponse, ClientError>(response) }
        },
        two_write_batch(),
        Some(VisibilityWait::Bounded { timeout_ms: 0 }),
    )
    .await
    .unwrap();
    // The request actually sent is an IMPLICIT batch carrying the wait.
    let sent = log.lock().unwrap();
    assert_eq!(sent.len(), 1, "exactly one request: {:?}", *sent);
    assert_eq!(
        sent[0].visibility_wait,
        Some(VisibilityWait::Bounded { timeout_ms: 0 }),
    );
    assert!(!sent[0].transactional, "never flipped transactional");
    // One visible + one pending receipt, scoped to their own repos.
    assert_eq!(receipts.len(), 2);
    let w1 = receipts.iter().find(|r| r.alias == "w1").unwrap();
    let w2 = receipts.iter().find(|r| r.alias == "w2").unwrap();
    assert_eq!(w1.visibility, VisibilityState::Visible);
    assert_eq!(w1.repo.as_deref(), Some("main"));
    assert_eq!(w2.visibility, pending(43, VisibilityPendingReason::Timeout));
    assert_eq!(w2.repo.as_deref(), Some("hot"));
}

#[tokio::test]
async fn request_timeout_sends_exactly_once_and_never_replays() {
    let log: Arc<Mutex<Vec<BatchRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    let result = execute_with_receipts_via(
        move |batch: BatchRequest| {
            capture.lock().unwrap().push(batch);
            async {
                Err::<BatchResponse, ClientError>(ClientError::RequestTimeout(
                    std::time::Duration::from_millis(50),
                ))
            }
        },
        two_write_batch(),
        Some(VisibilityWait::Bounded { timeout_ms: 0 }),
    )
    .await;
    assert!(
        matches!(result, Err(ClientError::RequestTimeout(_))),
        "{result:?}",
    );
    // Transport log: ONE request, no second attempt, no resend.
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn changes_since_sends_the_built_execute_request_and_decodes() {
    let log: Arc<Mutex<Vec<DbRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    let mut payload = TMap::default();
    payload.insert("changes_since".to_string(), QueryValue::Int(41));
    payload.insert("events".to_string(), QueryValue::List(Vec::new()));
    payload.insert("gap_at".to_string(), QueryValue::Null);
    payload.insert("visible_through".to_string(), QueryValue::Int(40));
    let mut results = TMap::default();
    results.insert(
        "changes_since".to_string(),
        shamir_query_types::read::QueryResult {
            records: vec![shamir_query_types::read::QueryRecord::Direct(
                QueryValue::Map(payload),
            )],
            ..Default::default()
        },
    );
    let response = DbResponse::Batch {
        response: BatchResponse {
            id: QueryValue::Null,
            results,
            execution_plan: Vec::new(),
            edge_provenance: TMap::default(),
            execution_time_us: 0,
            transaction: None,
            interner_delta: TMap::default(),
        },
    };
    let decoded = changes_since_via(
        move |req: DbRequest| {
            capture.lock().unwrap().push(req);
            async move { Ok::<DbResponse, ClientError>(response) }
        },
        "db",
        "archive",
        BIG,
        Some(7),
    )
    .await
    .unwrap();
    // The exact DbRequest::Execute the client put on the wire.
    let sent = log.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let DbRequest::Execute {
        query_version,
        db,
        batch,
    } = &sent[0]
    else {
        panic!("expected Execute, got {:?}", sent[0]);
    };
    assert_eq!(*query_version, CURRENT_QUERY_LANG_VERSION);
    assert_eq!(db, "db");
    assert!(!batch.transactional);
    let BatchOp::ChangesSince(op) = &batch.queries.get("changes_since").unwrap().op else {
        panic!("expected ChangesSince op");
    };
    assert_eq!(
        *op,
        ChangesSinceOp {
            changes_since: BIG,
            repo: "archive".into(),
            limit: Some(7),
        }
    );
    assert_eq!(
        decoded,
        ChangesSinceResponse {
            changes_since: 41,
            events: Vec::new(),
            gap_at: None,
            visible_through: Some(40),
        }
    );
}
