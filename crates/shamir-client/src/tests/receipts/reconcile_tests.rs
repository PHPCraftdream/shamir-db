//! Reconcile pending receipts: one observation per repository at the
//! highest pending position, no resend, no loop, refusal to guess.

use std::sync::{Arc, Mutex};

use shamir_query_types::batch::{TransactionInfo, VisibilityPendingReason, VisibilityState};

use crate::builder::batch::Batch;
use crate::builder::write::insert;
use crate::builder::write::Insert;
use shamir_types::types::value::QueryValue;

use crate::{extract_receipts, reconcile_receipts, AliasReceipt, ClientError};

fn pending_receipt(alias: &str, repo: Option<&str>, commit_version: u64) -> AliasReceipt {
    AliasReceipt {
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

type CallLog = Arc<Mutex<Vec<(String, u64)>>>;

#[tokio::test]
async fn reconcile_groups_by_repository_at_max_pending_position() {
    let receipts = vec![
        pending_receipt("a", Some("main"), 5),
        pending_receipt("b", Some("main"), 9),
        pending_receipt("c", Some("hot"), 3),
        AliasReceipt {
            alias: "d".into(),
            transaction: TransactionInfo::committed(2, 0, 4, true),
            visibility: VisibilityState::Visible,
            repo: Some("hot".into()),
        },
    ];
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let calls = log.clone();
    let observations = reconcile_receipts(&receipts, move |repo, version| {
        let calls = calls.clone();
        async move {
            calls.lock().unwrap().push((repo, version));
            Ok::<VisibilityState, ClientError>(VisibilityState::Visible)
        }
    })
    .await
    .unwrap();
    // Exactly ONE observation per repository, at its max pending position.
    let log = log.lock().unwrap().clone();
    assert_eq!(
        log.len(),
        2,
        "one WaitForVisibility per repository: {log:?}"
    );
    assert!(log.contains(&("main".to_string(), 9)), "{log:?}");
    assert!(log.contains(&("hot".to_string(), 3)), "{log:?}");
    assert_eq!(observations.len(), 2);
    let main = observations.iter().find(|o| o.repo == "main").unwrap();
    assert_eq!(main.version, 9);
    assert_eq!(main.visibility, VisibilityState::Visible);
}

#[tokio::test]
async fn alias_without_derivable_repository_is_refused_not_guessed() {
    let receipts = vec![pending_receipt("a", None, 5)];
    let result = reconcile_receipts(&receipts, |_, _| async {
        Ok::<VisibilityState, ClientError>(VisibilityState::Visible)
    })
    .await;
    assert!(
        matches!(&result, Err(ClientError::Protocol(msg)) if msg.contains("no derivable repository")),
        "{result:?}",
    );
}

#[tokio::test]
async fn pending_receipt_without_commit_position_is_refused() {
    let mut receipt = pending_receipt("a", Some("main"), 1);
    receipt.transaction.commit_version = None;
    let result = reconcile_receipts(&[receipt], |_, _| async {
        Ok::<VisibilityState, ClientError>(VisibilityState::Visible)
    })
    .await;
    assert!(
        matches!(&result, Err(ClientError::Protocol(msg)) if msg.contains("commit_version")),
        "{result:?}",
    );
}

#[tokio::test]
async fn transport_failure_propagates_without_a_second_request() {
    let receipts = vec![pending_receipt("a", Some("main"), 5)];
    let calls = Arc::new(Mutex::new(0u32));
    let counter = calls.clone();
    let result = reconcile_receipts(&receipts, move |_, _| {
        let counter = counter.clone();
        async move {
            *counter.lock().unwrap() += 1;
            Err::<VisibilityState, ClientError>(ClientError::RequestTimeout(
                std::time::Duration::from_millis(50),
            ))
        }
    })
    .await;
    assert!(
        matches!(result, Err(ClientError::RequestTimeout(_))),
        "{result:?}"
    );
    // No retry loop, no resend: exactly one observation attempt.
    assert_eq!(*calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn end_to_end_receipts_feed_reconcile() {
    // Extraction output (repo derivation included) drives reconcile.
    let mut batch = Batch::new();
    batch.insert("w1", insert("items").row(QueryValue::Null));
    batch.insert(
        "w2",
        Insert::with_repo("hot", "sessions").row(QueryValue::Null),
    );
    let request = batch.build();
    let mut results = shamir_collections::TMap::default();
    // BOTH repositories' results are present, so two observations are
    // really produced: w1 (main, pending 43) and w2 (hot, pending 44).
    results.insert(
        "w1".to_string(),
        shamir_query_types::read::QueryResult {
            receipt: Some(TransactionInfo::committed(7, 40, 43, true).with_visibility(
                VisibilityState::Pending {
                    visible_through: 42,
                    blocking_version: 43,
                    reason: VisibilityPendingReason::Timeout,
                },
            )),
            ..Default::default()
        },
    );
    results.insert(
        "w2".to_string(),
        shamir_query_types::read::QueryResult {
            receipt: Some(TransactionInfo::committed(8, 40, 44, true).with_visibility(
                VisibilityState::Pending {
                    visible_through: 43,
                    blocking_version: 44,
                    reason: VisibilityPendingReason::Timeout,
                },
            )),
            ..Default::default()
        },
    );
    let response = shamir_query_types::batch::BatchResponse {
        id: QueryValue::Null,
        results,
        execution_plan: Vec::new(),
        edge_provenance: shamir_collections::TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: shamir_collections::TMap::default(),
    };
    let receipts = extract_receipts(&request, &response).unwrap();
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let calls = log.clone();
    let observations = reconcile_receipts(&receipts, move |repo, version| {
        let calls = calls.clone();
        async move {
            calls.lock().unwrap().push((repo, version));
            Ok::<VisibilityState, ClientError>(VisibilityState::Visible)
        }
    })
    .await
    .unwrap();
    let log = log.lock().unwrap().clone();
    // Each entry IS the WaitForVisibility request the transport would
    // receive: op=wait_for_visibility { repo, version } — one per
    // repository, at the max pending position (43 for main, 44 for hot).
    assert_eq!(log, vec![("main".to_string(), 43), ("hot".to_string(), 44)]);
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].repo, "main");
    assert_eq!(observations[1].version, 44);
}
