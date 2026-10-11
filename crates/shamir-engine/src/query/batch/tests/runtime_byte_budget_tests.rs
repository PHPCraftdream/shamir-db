//! W8 counterfactuals use hand-computed byte totals and observable dispatches.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_bytes::ByteBuf;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest, ExecPolicy, LimitKind};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::{
    CorruptRecordRef, DdlOpState, ExplainPlan, PaginationInfo, PlanType, QueryStats,
};
use shamir_query_types::write::InsertedRecord;
use shamir_query_types::CallOp;
use shamir_types::access::Actor;
use shamir_types::types::common::{new_map, TMap, TSet};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::batch::batch_execute::execute_batch_impl;
use crate::query::batch::result_size::ResultSize;
use crate::query::batch::{
    execute_batch, Authorized, BatchError, ExecScope, FunctionInvoker, TableResolver,
};
use crate::query::read::{QueryRecord, QueryResult};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::{TableConfig, TableManager};

struct Resolver;

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, _: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        Err(shamir_storage::error::DbError::NotFound("no tables".into()))
    }

    async fn resolve_repo(&self, _: &str) -> shamir_storage::error::DbResult<RepoInstance> {
        Err(shamir_storage::error::DbError::NotFound("no repos".into()))
    }
}

#[test]
fn estimator_exact_scalars_containers_and_binary_records() {
    for value in [
        QueryValue::Null,
        QueryValue::Bool(true),
        QueryValue::Int(7),
        QueryValue::F64(0.5),
    ] {
        assert_eq!(ResultSize::value(&value), 16);
    }
    assert_eq!(ResultSize::value(&QueryValue::Str("é".into())), 26);
    assert_eq!(ResultSize::value(&QueryValue::Bin(vec![1, 2, 3])), 27);
    assert_eq!(ResultSize::value(&QueryValue::Big(12345.into())), 29);
    assert_eq!(ResultSize::value(&QueryValue::Dec(Default::default())), 56);
    let mut set = TSet::default();
    set.insert(QueryValue::Int(7));
    set.insert(QueryValue::Bool(true));
    assert_eq!(ResultSize::value(&QueryValue::Set(set)), 88);
    assert_eq!(ResultSize::value(&QueryValue::List(Vec::new())), 24);
    assert_eq!(ResultSize::value(&QueryValue::Set(TSet::default())), 24);
    assert_eq!(ResultSize::value(&QueryValue::Map(new_map())), 24);
    let mut map = new_map();
    map.insert(
        "k".into(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Bin(vec![1, 2, 3])]),
    );
    assert_eq!(ResultSize::value(&QueryValue::Map(map)), 140);
    let result = QueryResult::records_only(vec![
        QueryRecord::Direct(QueryValue::Int(7)),
        QueryRecord::IdBytes(ByteBuf::from(vec![1, 2, 3])),
    ]);
    assert_eq!(ResultSize::result(&result), 219);
    assert_eq!(ResultSize::result(&QueryResult::default()), 80);
}

#[test]
fn estimator_metadata_and_serialized_envelope_presence_are_exact() {
    let result = QueryResult {
        stats: Some(QueryStats {
            index_used: Some("idx".into()),
            ..QueryStats::default()
        }),
        pagination: Some(PaginationInfo {
            total_count: Some(1),
            total_pages: None,
            current_page: None,
            page_size: None,
            has_next: false,
            has_prev: false,
        }),
        value: Some(QueryValue::Null),
        explain: Some(ExplainPlan {
            plan_type: PlanType::IndexScan,
            index_used: Some("index".into()),
            estimated_rows: None,
        }),
        skipped: true,
        versions: Some(vec![1, 2]),
        corrupt_records: vec![CorruptRecordRef {
            table: "sink".into(),
            id: RecordId([0; 16]),
        }],
        op_id: Some(RecordId([0; 16])),
        ddl_status: Some(DdlOpState::Failed {
            detail: "failed".into(),
        }),
        ..QueryResult::default()
    };
    // 80 + 259 stats + 320 pagination + 16 value + 261 explain
    // + 88 versions + (16 entry + 128 shell + 4 table) + 40 id + 134 DDL.
    assert_eq!(ResultSize::result(&result), 1346);
    let mut large = result.clone();
    large.stats.as_mut().unwrap().index_used = Some("s".repeat(4096));
    large.explain.as_mut().unwrap().index_used = Some("e".repeat(4096));
    large.corrupt_records[0].table = "t".repeat(4096);
    large.ddl_status = Some(DdlOpState::Failed {
        detail: "d".repeat(4096),
    });
    // Four unbounded metadata strings replace lengths 3 + 5 + 4 + 6.
    assert_eq!(ResultSize::result(&large), 17_712);
    let mut results = new_map();
    results.insert("r".into(), result);
    // Alias/result/records shells=112; nine optional field entries=216.
    assert_eq!(ResultSize::assembled(&results), 328);
    let serialized = super::super::query_value_serializer::to_query_value(&results).unwrap();
    let QueryValue::Map(aliases) = serialized else {
        panic!("expected alias map");
    };
    let QueryValue::Map(fields) = &aliases["r"] else {
        panic!("expected result map");
    };
    assert_eq!(fields.len(), 10);
    for key in [
        "records",
        "stats",
        "pagination",
        "value",
        "explain",
        "skipped",
        "versions",
        "corrupt_records",
        "op_id",
        "ddl_status",
    ] {
        assert!(fields.contains_key(key));
    }
    results.insert("r".into(), QueryResult::default());
    assert_eq!(ResultSize::assembled(&results), 112);
    let serialized = super::super::query_value_serializer::to_query_value(&results).unwrap();
    let QueryValue::Map(aliases) = serialized else {
        panic!("expected alias map");
    };
    let QueryValue::Map(fields) = &aliases["r"] else {
        panic!("expected result map");
    };
    assert_eq!(fields.len(), 1);
    assert!(fields.contains_key("records"));
    // Empty versions are present even though empty corrupt_records are omitted.
    results.get_mut("r").unwrap().versions = Some(Vec::new());
    assert_eq!(ResultSize::assembled(&results), 136);
    let serialized = super::super::query_value_serializer::to_query_value(&results).unwrap();
    let QueryValue::Map(aliases) = serialized else {
        panic!("expected alias map");
    };
    let QueryValue::Map(fields) = &aliases["r"] else {
        panic!("expected result map");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields["versions"], QueryValue::List(Vec::new()));
}

#[test]
fn estimator_inserted_records_include_payload_and_synthetic_id() {
    for (fields, id, expected) in [
        (QueryValue::Null, None, 144),
        (QueryValue::Null, Some(RecordId([0; 16])), 249),
        (QueryValue::Map(new_map()), Some(RecordId([0; 16])), 211),
    ] {
        let result =
            QueryResult::records_only(vec![QueryRecord::Inserted(InsertedRecord { id, fields })]);
        assert_eq!(ResultSize::result(&result), expected);
    }
}

#[test]
fn estimator_deep_values_use_an_explicit_stack() {
    let mut value = QueryValue::Null;
    for _ in 0..10_000 {
        value = QueryValue::List(vec![value]);
    }
    assert_eq!(ResultSize::value(&value), 400_016);
    // Iterative teardown too: Value's existing recursive Drop is not under test.
    while let QueryValue::List(mut items) = value {
        value = items.pop().unwrap();
    }
}

#[test]
fn bytes_boundary_opt_out_and_overflow_mirror_work() {
    let limits = BatchLimits {
        max_result_size: 7,
        ..BatchLimits::default()
    };
    let off = ExecScope::root(&limits);
    off.charge_bytes(u64::MAX).unwrap();
    assert_eq!(off.bytes(), 0);
    let on = ExecScope::root_with_result_size(&limits, true);
    on.charge_bytes(7).unwrap();
    assert_eq!(on.charge_bytes(1).unwrap_err(), bytes_error(8, 7));
    assert_eq!(on.bytes(), 8);
    let overflow = ExecScope::root_with_result_size(
        &BatchLimits {
            max_result_size: usize::MAX,
            ..BatchLimits::default()
        },
        true,
    );
    overflow.charge_bytes(usize::MAX as u64).unwrap();
    assert_eq!(
        overflow.charge_bytes(u64::MAX).unwrap_err(),
        bytes_error(u64::MAX, usize::MAX as u64)
    );
    assert_eq!(overflow.bytes(), u64::MAX);
}

struct RepoResolver(RepoInstance);

#[async_trait::async_trait]
impl TableResolver for RepoResolver {
    async fn resolve(&self, table: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        self.0.get_table(&table.table).await
    }

    async fn resolve_repo(&self, _: &str) -> shamir_storage::error::DbResult<RepoInstance> {
        Ok(self.0.clone())
    }
}

// Red before W8: both stage loops admit the intermediate write; the tx commits.
#[tokio::test]
async fn intermediate_write_is_charged_before_transaction_commit() {
    let resolver = RepoResolver(
        RepoInstance::from_factory(
            "main".into(),
            BoxRepoFactory::in_memory(),
            vec![TableConfig::new("sink")],
        )
        .await
        .unwrap(),
    );
    for transactional in [false, true] {
        let mut batch = Batch::new();
        batch.return_flagged().limits(BatchLimits {
            max_result_size: 1,
            ..BatchLimits::default()
        });
        if transactional {
            batch.transactional();
        }
        batch.op_silent("write", insert("sink").row(doc().set("id", 7)));
        let request = batch.build();
        let scope = ExecScope::root_with_result_size(&request.limits, true);
        let version = resolver.0.current_commit_version().await.unwrap();
        let error = execute_batch_impl(
            &request,
            &resolver,
            None,
            None,
            Actor::System,
            "test",
            0,
            &new_map(),
            &scope,
        )
        .await
        .unwrap_err();
        assert_eq!(error, bytes_error(scope.bytes(), 1));
        assert!(scope.bytes() > 1);
        if transactional {
            assert_eq!(resolver.0.current_commit_version().await.unwrap(), version);
        }
    }
    let mut read = Batch::new();
    read.query("rows", Query::from("sink"));
    let response = execute_batch(
        Authorized::unchecked(&read.build(), Actor::System, "test"),
        &resolver,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(response.results["rows"].records.len(), 1);
}

struct Invoker {
    iterations: AtomicU64,
}

#[async_trait::async_trait]
impl FunctionInvoker for Invoker {
    async fn invoke_call(
        &self,
        op: &CallOp,
        _: &Actor,
        _: &TMap<String, QueryResult>,
    ) -> Result<QueryResult, BatchError> {
        if op.call == "source" {
            Ok(QueryResult::with_value(QueryValue::List(
                (0..10).map(QueryValue::Int).collect(),
            )))
        } else {
            self.iterations.fetch_add(1, Ordering::Relaxed);
            Ok(QueryResult::with_value(QueryValue::Str("data".into())))
        }
    }
}

fn bytes_error(actual: u64, max: u64) -> BatchError {
    BatchError::LimitExceeded {
        kind: LimitKind::ResultBytes,
        actual,
        max,
    }
}

fn loop_request(cap: usize) -> BatchRequest {
    let mut body = Batch::new();
    body.call("x", "payload", Vec::<FilterValue>::new());
    let mut batch = Batch::new();
    batch.return_only(["loop"]).limits(BatchLimits {
        max_result_size: cap,
        ..BatchLimits::default()
    });
    let mut source_builder = Batch::new();
    source_builder.call("source", "source", Vec::<FilterValue>::new());
    let source_op = source_builder.build().queries["source"].op.clone();
    let source = batch.op_silent("source", source_op);
    batch.for_each("loop", source.all(), "row", body.build());
    batch.build()
}

async fn run(
    request: &BatchRequest,
    scope: &ExecScope,
    invoker: &Invoker,
) -> Result<shamir_query_types::batch::BatchResponse, BatchError> {
    execute_batch_impl(
        request,
        &Resolver,
        None,
        Some(invoker),
        Actor::System,
        "test",
        0,
        &new_map(),
        scope,
    )
    .await
}

#[tokio::test]
async fn intermediate_and_loop_stop_on_third_dispatch_with_exact_total() {
    let request = loop_request(1042);
    let invoker = Invoker {
        iterations: AtomicU64::new(0),
    };
    let scope = ExecScope::root_with_result_size(&request.limits, true);
    // Source=424, loop shell=104, each iteration=108 payload+149 envelope.
    assert_eq!(
        run(&request, &scope, &invoker).await.unwrap_err(),
        bytes_error(1150, 1042)
    );
    assert_eq!(scope.bytes(), 1150);
    assert_eq!(invoker.iterations.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn exact_loop_boundary_and_embedded_policy_opt_out() {
    let invoker = Invoker {
        iterations: AtomicU64::new(0),
    };
    let request = loop_request(3098); // 424 + 104 + 10 * 257
    let scope = ExecScope::root_with_result_size(&request.limits, true);
    let response = run(&request, &scope, &invoker).await.unwrap();
    assert_eq!(scope.bytes(), 3098);
    assert_eq!(response.results.len(), 1);
    assert_eq!(
        response.results["loop"]
            .value
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(invoker.iterations.load(Ordering::Relaxed), 10);
    let small = loop_request(1);
    let response = execute_batch(
        Authorized::unchecked(&small, Actor::System, "test"),
        &Resolver,
        None,
        Some(&invoker),
    )
    .await
    .unwrap();
    assert_eq!(
        response.results["loop"]
            .value
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(invoker.iterations.load(Ordering::Relaxed), 20);
    let auth = Authorized::unchecked(&small, Actor::System, "test").with_policy(ExecPolicy {
        caps: BatchLimits::CEILING,
        enforce_result_size: true,
    });
    assert_eq!(
        execute_batch(auth, &Resolver, None, Some(&invoker))
            .await
            .unwrap_err(),
        bytes_error(424, 1)
    );
}

#[tokio::test]
async fn nested_containers_charge_shells_once_including_empty() {
    let invoker = Invoker {
        iterations: AtomicU64::new(0),
    };
    let mut leaf = Batch::new();
    leaf.call("x", "payload", Vec::<FilterValue>::new());
    let mut middle = Batch::new();
    middle.sub_batch_no_bind("n", leaf.build());
    let mut root = Batch::new();
    root.sub_batch_no_bind("n", middle.build());
    let request = root.build();
    let scope = ExecScope::root_with_result_size(&request.limits, true);
    let response = run(&request, &scope, &invoker).await.unwrap();
    assert_eq!(scope.bytes(), 534); // 108 + 2 * (80 + 133)
                                    // Estimator walks the fully materialized tree; execution only adds new
                                    // shells at each wrapping layer, never recharges this nested payload.
    assert_eq!(
        ResultSize::value(response.results["n"].value.as_ref().unwrap()),
        294
    );
    assert_eq!(invoker.iterations.load(Ordering::Relaxed), 1);
    let mut empty = Batch::new();
    empty.sub_batch_no_bind("empty", Batch::new().build());
    empty.for_each("loop", Vec::<i64>::new(), "row", Batch::new().build());
    let request = empty.build();
    let scope = ExecScope::root_with_result_size(&request.limits, true);
    let response = run(&request, &scope, &invoker).await.unwrap();
    assert_eq!(scope.bytes(), 208); // two result shells + empty map/list shells
    assert_eq!(
        response.results["loop"].value,
        Some(QueryValue::List(Vec::new()))
    );
}
