//! Per-alias receipt extraction from implicit (non-transactional) batch
//! responses. Wire fixtures are named-msgpack shapes the server emits
//! (`QueryResult.receipt`, row-less shells included).

use shamir_collections::TMap;
use shamir_query_types::batch::{
    BatchOp, BatchRequest, BatchResponse, ForEachOp, QueryEntry, ResultEncoding, TransactionInfo,
    VisibilityPendingReason, VisibilityState,
};
use shamir_query_types::filter::FilterValue;
use shamir_types::types::value::QueryValue;

use crate::builder::batch::Batch;
use crate::builder::write::insert;
use crate::builder::write::Insert;
use crate::{extract_receipts, ClientError};

const BIG: u64 = 9_007_199_254_740_993; // 2^53 + 1

fn pending(visible_through: u64, reason: VisibilityPendingReason) -> VisibilityState {
    VisibilityState::Pending {
        visible_through,
        blocking_version: visible_through + 1,
        reason,
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
    results: impl IntoIterator<Item = (&'static str, Option<TransactionInfo>)>,
) -> BatchResponse {
    let mut map = TMap::default();
    for (alias, receipt) in results {
        map.insert(
            alias.to_string(),
            QueryResultShell { receipt }.into_result(),
        );
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

/// Row-less shell: suppressed aliases carry only their receipt.
struct QueryResultShell {
    receipt: Option<TransactionInfo>,
}

impl QueryResultShell {
    fn into_result(self) -> shamir_query_types::read::QueryResult {
        shamir_query_types::read::QueryResult {
            receipt: self.receipt,
            ..Default::default()
        }
    }
}

#[test]
fn visible_and_pending_aliases_yield_receipts_in_their_own_repos() {
    let request = two_write_batch();
    let response = response_with([
        ("w1", Some(TransactionInfo::committed(7, 40, 43, true))),
        (
            "w2",
            Some(
                TransactionInfo::committed(8, 40, 44, true)
                    .with_visibility(pending(42, VisibilityPendingReason::Timeout)),
            ),
        ),
    ]);
    let receipts = extract_receipts(&request, &response).unwrap();
    assert_eq!(receipts.len(), 2);
    let w1 = receipts.iter().find(|r| r.alias == "w1").unwrap();
    let w2 = receipts.iter().find(|r| r.alias == "w2").unwrap();
    assert_eq!(w1.visibility, VisibilityState::Visible);
    assert_eq!(w1.repo.as_deref(), Some("main"));
    assert_eq!(w1.transaction.commit_version, Some(43));
    assert_eq!(w2.visibility, pending(42, VisibilityPendingReason::Timeout));
    assert_eq!(w2.repo.as_deref(), Some("hot"));
    assert_eq!(w2.transaction.commit_version, Some(44));
}

#[test]
fn suppressed_write_alias_still_yields_its_receipt() {
    let mut request = two_write_batch();
    request.queries.get_mut("w2").unwrap().return_result = false;
    // The server keeps a row-less shell carrying the receipt.
    let response = response_with([(
        "w2",
        Some(
            TransactionInfo::committed(8, 40, 44, true)
                .with_visibility(pending(43, VisibilityPendingReason::ReopenRequired)),
        ),
    )]);
    let receipts = extract_receipts(&request, &response).unwrap();
    assert_eq!(
        receipts.len(),
        1,
        "receipt must be read from the shell, not rows"
    );
    assert_eq!(receipts[0].alias, "w2");
    assert_eq!(
        receipts[0].visibility,
        pending(43, VisibilityPendingReason::ReopenRequired)
    );
}

#[test]
fn malformed_receipt_metadata_is_rejected_as_protocol() {
    let request = two_write_batch();
    // blocking_version != visible_through + 1 fails TransactionInfo::validate.
    let malformed =
        TransactionInfo::committed(9, 40, 44, true).with_visibility(VisibilityState::Pending {
            visible_through: 5,
            blocking_version: 7,
            reason: VisibilityPendingReason::Timeout,
        });
    let response = response_with([("w1", Some(malformed))]);
    let result = extract_receipts(&request, &response);
    assert!(
        matches!(result, Err(ClientError::Protocol(_))),
        "{result:?}"
    );
}

#[test]
fn aborted_receipts_stay_typed_outcomes() {
    let request = two_write_batch();
    let response = response_with([("w1", Some(TransactionInfo::aborted(9, "tx_conflict")))]);
    let result = extract_receipts(&request, &response);
    let Err(ClientError::TransactionOutcome(info)) = result else {
        panic!("expected typed outcome: {result:?}");
    };
    assert_eq!(info.status, "aborted");
}

/// The REAL server shape for a nested result: a serialized QueryResult
/// map (row-less receipt shell) exactly like `query_runner.rs` puts into
/// a container's `value` (sub-batch: map of alias -> QueryResult;
/// for-each: LIST of iteration maps).
fn nested_shell(alias: &str, receipt: &TransactionInfo) -> (String, QueryValue) {
    let mut inner = TMap::default();
    inner.insert("receipt".to_string(), qv(receipt));
    inner.insert("records".to_string(), QueryValue::List(Vec::new()));
    (alias.to_string(), QueryValue::Map(inner))
}

/// Reverse of the server's `to_qv`: typed struct -> msgpack -> QueryValue.
/// Values above `i64::MAX` land here as `QueryValue::Big` (decimal
/// string), exactly like the server's `QueryValueSerializer`.
fn qv<T: serde::Serialize>(v: &T) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(v).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn inner_batch_with(alias: &str) -> shamir_query_types::batch::BatchRequest {
    let mut batch = Batch::new();
    batch.insert(alias, insert("items").row(QueryValue::Null));
    batch.build()
}

fn request_with(queries: TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        result_encoding: ResultEncoding::Name,
        interner_epochs: TMap::default(),
    }
}

fn entry(op: BatchOp) -> QueryEntry {
    QueryEntry {
        op,
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

fn container_result(value: QueryValue) -> shamir_query_types::read::QueryResult {
    shamir_query_types::read::QueryResult {
        records: Vec::new(),
        value: Some(value),
        ..Default::default()
    }
}

fn single_response(alias: &str, result: shamir_query_types::read::QueryResult) -> BatchResponse {
    let mut map = TMap::default();
    map.insert(alias.to_string(), result);
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

#[test]
fn sub_batch_nested_receipts_surface_with_path_aliases_and_no_repo() {
    // Real shape: the request entry is a BatchOp::Batch whose inner
    // request declares alias "inner"; the container has NO own receipt
    // and its `value` is a MAP of alias -> serialized QueryResult
    // (query_runner.rs:773-805). The container alias itself cannot derive
    // a repository.
    let mut queries = TMap::default();
    queries.insert(
        "outer".to_string(),
        entry(BatchOp::Batch(shamir_query_types::batch::SubBatchOp {
            batch: inner_batch_with("inner"),
            bind: TMap::default(),
        })),
    );
    let request = request_with(queries);
    let mut value = TMap::default();
    let (alias, shell) = nested_shell("inner", &TransactionInfo::committed(3, 1, 2, true));
    value.insert(alias, shell);
    let receipts = extract_receipts(
        &request,
        &single_response("outer", container_result(QueryValue::Map(value))),
    )
    .unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].alias, "outer.inner");
    assert_eq!(receipts[0].repo, None);
    assert_eq!(receipts[0].transaction.commit_version, Some(2));
}

#[test]
fn for_each_nested_receipts_surface_with_iteration_paths_and_no_repo() {
    // Real shape: the request entry is a BatchOp::ForEach whose inner
    // request declares alias "w"; the container's `value` is a LIST of
    // iteration maps of alias -> serialized QueryResult
    // (query_runner.rs:1065-1080).
    let mut queries = TMap::default();
    queries.insert(
        "loop".to_string(),
        entry(BatchOp::ForEach(ForEachOp {
            over: FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
            bind_row: "row".into(),
            batch: inner_batch_with("w"),
        })),
    );
    let request = request_with(queries);
    let iteration = |receipt: &TransactionInfo| {
        let mut iter = TMap::default();
        let (alias, shell) = nested_shell("w", receipt);
        iter.insert(alias, shell);
        QueryValue::Map(iter)
    };
    let receipts = extract_receipts(
        &request,
        &single_response(
            "loop",
            container_result(QueryValue::List(vec![
                iteration(&TransactionInfo::committed(1, 0, 2, true)),
                iteration(&TransactionInfo::committed(3, 0, 4, true).with_visibility(
                    VisibilityState::Pending {
                        visible_through: 3,
                        blocking_version: 4,
                        reason: VisibilityPendingReason::Timeout,
                    },
                )),
            ])),
        ),
    )
    .unwrap();
    let aliases: Vec<&str> = receipts.iter().map(|r| r.alias.as_str()).collect();
    assert_eq!(aliases, vec!["loop[0].w", "loop[1].w"]);
    assert!(receipts.iter().all(|r| r.repo.is_none()));
    assert_eq!(
        receipts[1].visibility,
        pending(3, VisibilityPendingReason::Timeout)
    );
}

#[test]
fn nested_positions_above_i64_max_decode_from_decimal_strings() {
    // The server turns nested u64 values above i64::MAX into
    // QueryValue::Big — a decimal string on the wire. The tolerant nested
    // decoder must accept them (u64::MAX and 2^53+1).
    let mut inner = Batch::new();
    inner.insert("max", insert("items").row(QueryValue::Null));
    inner.insert("big", insert("items").row(QueryValue::Null));
    let mut queries = TMap::default();
    queries.insert(
        "outer".to_string(),
        entry(BatchOp::Batch(shamir_query_types::batch::SubBatchOp {
            batch: inner.build(),
            bind: TMap::default(),
        })),
    );
    let request = request_with(queries);
    let mut value = TMap::default();
    let (max_alias, max_shell) = nested_shell(
        "max",
        &TransactionInfo::committed(1, 0, u64::MAX, true)
            .with_visibility(pending(u64::MAX - 1, VisibilityPendingReason::Timeout)),
    );
    value.insert(max_alias, max_shell);
    let (big_alias, big_shell) = nested_shell("big", &TransactionInfo::committed(2, 0, BIG, true));
    value.insert(big_alias, big_shell);
    let receipts = extract_receipts(
        &request,
        &single_response("outer", container_result(QueryValue::Map(value))),
    )
    .unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].alias, "outer.max");
    assert_eq!(receipts[0].transaction.commit_version, Some(u64::MAX));
    assert_eq!(
        receipts[0].visibility,
        pending(u64::MAX - 1, VisibilityPendingReason::Timeout)
    );
    assert_eq!(receipts[1].alias, "outer.big");
    assert_eq!(receipts[1].transaction.commit_version, Some(BIG));
}

#[test]
fn application_value_maps_are_never_treated_as_receipts() {
    // A read/call/write result's `value` is application data: even when
    // it happens to contain a map with a "receipt" key, extraction must
    // yield NOTHING (descent only happens under container request ops).
    let request = two_write_batch();
    // Application data shaped like the old walker's target: a value MAP
    // whose MEMBER carries its own valid-looking receipt. Non-container
    // ops are never inspected, so this must fabricate NOTHING.
    let mut phantom = TMap::default();
    phantom.insert(
        "receipt".to_string(),
        qv(&TransactionInfo::committed(9, 0, 1, true)),
    );
    phantom.insert("records".to_string(), QueryValue::List(Vec::new()));
    let mut app_value = TMap::default();
    app_value.insert("phantom".to_string(), QueryValue::Map(phantom));
    let mut map = TMap::default();
    map.insert(
        "w1".to_string(),
        shamir_query_types::read::QueryResult {
            records: Vec::new(),
            value: Some(QueryValue::Map(app_value)),
            ..Default::default()
        },
    );
    let response = BatchResponse {
        id: QueryValue::Null,
        results: map,
        execution_plan: Vec::new(),
        edge_provenance: TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: TMap::default(),
    };
    let receipts = extract_receipts(&request, &response).unwrap();
    assert!(
        receipts.is_empty(),
        "application data must not fabricate receipts: {receipts:?}"
    );
}
