//! Production-path tests over the same mock transport the VIS-5a wire
//! tests use: requests are rmp-encoded into a byte log exactly like
//! `roundtrip` encodes them, the transport timeout comes from the REAL
//! `await_pending_response` deadline machinery (never manufactured in a
//! callback), and responses are decoded from msgpack bytes.

use std::sync::{Arc, Mutex};

use shamir_query_types::admin::ChangesSinceOp;
use shamir_query_types::batch::{
    BatchOp, BatchRequest, BatchResponse, TransactionInfo, VisibilityPendingReason,
    VisibilityState, VisibilityWait,
};
use shamir_types::types::common::TMap;
use shamir_types::types::value::QueryValue;
use tokio::sync::oneshot;

use crate::builder::batch::Batch;
use crate::builder::write::insert;
use crate::builder::write::Insert;
use crate::client::{await_pending_response, Lifecycle};
use crate::interner_cache::InternerCacheRegistry;
use crate::receipts::{changes_since_via, execute_with_receipts_via, wait_for_visibility_via};
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

fn two_write_batch() -> BatchRequest {
    let mut batch = Batch::new();
    batch.insert("w1", insert("items").row(QueryValue::Null));
    batch.insert(
        "w2",
        Insert::with_repo("hot", "sessions").row(QueryValue::Null),
    );
    batch.build()
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

fn db_response_bytes(response: &DbResponse) -> Vec<u8> {
    rmp_serde::to_vec_named(response).unwrap()
}

/// The mock transport: records the rmp-encoded request bytes and answers
/// with the encoded response — the same encode/decode path `roundtrip`
/// uses.
fn logging_send(
    log: Arc<Mutex<Vec<Vec<u8>>>>,
    response: DbResponse,
) -> impl FnOnce(DbRequest) -> std::future::Ready<Result<DbResponse, ClientError>> {
    move |req: DbRequest| {
        log.lock()
            .unwrap()
            .push(rmp_serde::to_vec_named(&req).unwrap());
        let bytes = db_response_bytes(&response);
        std::future::ready(rmp_serde::from_slice(&bytes).map_err(ClientError::from))
    }
}

#[tokio::test]
async fn execute_with_receipts_sends_one_implicit_execute_and_extracts() {
    let log: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    let mut response = response_with([
        ("w1", TransactionInfo::committed(7, 40, 43, true)),
        (
            "w2",
            TransactionInfo::committed(8, 40, 44, true)
                .with_visibility(pending(43, VisibilityPendingReason::Timeout)),
        ),
    ]);
    // A delta the core must merge into the registry after the response.
    response.interner_delta.insert(
        "main".to_string(),
        shamir_query_types::batch::InternerDelta {
            epoch: 5,
            entries: vec![(1, "name".to_string())],
        },
    );
    let registry = InternerCacheRegistry::new();
    let (batch_response, receipts) = execute_with_receipts_via(
        move |req: DbRequest| {
            capture
                .lock()
                .unwrap()
                .push(rmp_serde::to_vec_named(&req).unwrap());
            let bytes = rmp_serde::to_vec_named(&DbResponse::Batch {
                response: response.clone(),
            })
            .unwrap();
            std::future::ready(rmp_serde::from_slice(&bytes).map_err(ClientError::from))
        },
        Some(&registry),
        "db",
        two_write_batch(),
        Some(VisibilityWait::Bounded { timeout_ms: 0 }),
    )
    .await
    .unwrap();
    // The request ACTUALLY SENT is exactly one DbRequest::Execute with the
    // requested wait and transactional == false (decoded from wire bytes).
    let sent = log.lock().unwrap();
    assert_eq!(sent.len(), 1, "exactly one request");
    let decoded: DbRequest = rmp_serde::from_slice(&sent[0]).unwrap();
    let DbRequest::Execute {
        query_version,
        db,
        batch,
    } = decoded
    else {
        panic!("expected Execute, got {decoded:?}");
    };
    assert_eq!(query_version, CURRENT_QUERY_LANG_VERSION);
    assert_eq!(db, "db");
    assert_eq!(
        batch.visibility_wait,
        Some(VisibilityWait::Bounded { timeout_ms: 0 })
    );
    assert!(!batch.transactional, "never flipped transactional");
    // Interner section 1: the core advertised epochs on the sent request.
    assert!(
        !batch.interner_epochs.is_empty(),
        "interner-epoch advertisement must run in the core"
    );
    drop(sent);
    // Interner section 2: the delta was merged into the registry.
    let fm = registry.get_or_create("db", "main");
    assert_eq!(
        fm.epoch(),
        5,
        "delta epoch must be CAS-maxed into the cache"
    );
    assert_eq!(fm.id_of("name"), Some(1), "delta entries must be inserted");
    // One visible + one pending receipt, scoped to their own repos.
    assert_eq!(receipts.len(), 2);
    let w1 = receipts.iter().find(|r| r.alias == "w1").unwrap();
    let w2 = receipts.iter().find(|r| r.alias == "w2").unwrap();
    assert_eq!(w1.visibility, VisibilityState::Visible);
    assert_eq!(w1.repo.as_deref(), Some("main"));
    assert_eq!(w2.visibility, pending(43, VisibilityPendingReason::Timeout));
    assert_eq!(w2.repo.as_deref(), Some("hot"));
    assert_eq!(batch_response.results.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn transport_timeout_sends_exactly_once_and_never_replays() {
    // The timeout is produced by the REAL `await_pending_response`
    // deadline machinery (no responder registered), never by a callback.
    let log: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    let result = execute_with_receipts_via(
        move |req: DbRequest| {
            capture
                .lock()
                .unwrap()
                .push(rmp_serde::to_vec_named(&req).unwrap());
            let state = Lifecycle::new();
            let (tx, rx) = oneshot::channel();
            state.pending.lock().unwrap().map.insert(1, tx);
            async move {
                let payload = await_pending_response(
                    rx,
                    Some(std::time::Duration::from_millis(50)),
                    &state,
                    1,
                )
                .await?;
                rmp_serde::from_slice(&payload).map_err(ClientError::from)
            }
        },
        None,
        "db",
        two_write_batch(),
        Some(VisibilityWait::Bounded { timeout_ms: 0 }),
    )
    .await;
    assert!(
        matches!(result, Err(ClientError::RequestTimeout(_))),
        "{result:?}"
    );
    // Transport log: ONE Execute, no second attempt, no resend.
    assert_eq!(log.lock().unwrap().len(), 1);
    let decoded: DbRequest = rmp_serde::from_slice(&log.lock().unwrap()[0]).unwrap();
    assert!(matches!(decoded, DbRequest::Execute { ref batch, .. } if !batch.transactional));
}

#[tokio::test]
async fn changes_since_sends_the_built_execute_request_and_decodes() {
    let log: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
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
        logging_send(capture, response),
        "db",
        "archive",
        BIG,
        Some(7),
    )
    .await
    .unwrap();
    // The exact DbRequest::Execute the client put on the wire (decoded
    // from the logged bytes).
    let sent = log.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let DbRequest::Execute {
        query_version,
        db,
        batch,
    } = rmp_serde::from_slice(&sent[0]).unwrap()
    else {
        panic!("expected Execute");
    };
    assert_eq!(query_version, CURRENT_QUERY_LANG_VERSION);
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

#[tokio::test]
async fn reconciliation_sends_the_exact_wait_for_visibility_requests() {
    let log: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = log.clone();
    // The reconcile seam drives the REAL request construction: one
    // WaitForVisibility per repository at the max pending position.
    let observe = move |repo: String, version: u64| {
        let capture = capture.clone();
        async move {
            wait_for_visibility_via(
                move |req: DbRequest| {
                    capture
                        .lock()
                        .unwrap()
                        .push(rmp_serde::to_vec_named(&req).unwrap());
                    std::future::ready(Ok::<DbResponse, ClientError>(
                        DbResponse::VisibilityObserved {
                            version,
                            visibility: VisibilityState::Visible,
                        },
                    ))
                },
                "db",
                &repo,
                version,
                VisibilityWait::Bounded { timeout_ms: 0 },
            )
            .await
        }
    };
    let observations = crate::reconcile_receipts(
        &[
            pending_receipt_bytes("a", Some("main"), 5),
            pending_receipt_bytes("b", Some("main"), 9),
            pending_receipt_bytes("c", Some("hot"), 3),
        ],
        observe,
    )
    .await
    .unwrap();
    let sent = log.lock().unwrap();
    assert_eq!(sent.len(), 2, "one WaitForVisibility per repository");
    let first: DbRequest = rmp_serde::from_slice(&sent[0]).unwrap();
    let second: DbRequest = rmp_serde::from_slice(&sent[1]).unwrap();
    let DbRequest::WaitForVisibility {
        query_version,
        db,
        repo,
        version,
        visibility_wait,
    } = first
    else {
        panic!("expected WaitForVisibility, got {first:?}");
    };
    assert_eq!(query_version, CURRENT_QUERY_LANG_VERSION);
    assert_eq!(db, "db");
    assert_eq!(repo, "main");
    assert_eq!(version, 9, "max pending position");
    assert_eq!(visibility_wait, VisibilityWait::Bounded { timeout_ms: 0 });
    let DbRequest::WaitForVisibility { repo, version, .. } = second else {
        panic!("expected WaitForVisibility");
    };
    assert_eq!(repo, "hot");
    assert_eq!(version, 3);
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].visibility, VisibilityState::Visible);
}

fn pending_receipt_bytes(
    alias: &str,
    repo: Option<&str>,
    commit_version: u64,
) -> crate::AliasReceipt {
    crate::AliasReceipt {
        alias: alias.to_string(),
        transaction: TransactionInfo::committed(1, 0, commit_version, true).with_visibility(
            VisibilityState::Pending {
                visible_through: commit_version - 1,
                blocking_version: commit_version,
                reason: VisibilityPendingReason::Timeout,
            },
        ),
        visibility: VisibilityState::Pending {
            visible_through: commit_version - 1,
            blocking_version: commit_version,
            reason: VisibilityPendingReason::Timeout,
        },
        repo: repo.map(str::to_string),
    }
}
