//! W10: hand-derived totals, real table sinks, no timing oracle.

use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;
use shamir_query_types::batch::{BatchLimits, BatchRequest, LimitKind};
use shamir_query_types::read::OrderByItem;
use shamir_storage::error::DbResult;
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::query::batch::batch_execute::execute_batch_impl;
use crate::query::batch::exec_scope::RowByteMeter;
use crate::query::batch::{BatchError, ExecScope, TableResolver};
use crate::query::read::{QueryRecord, QueryResult};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::record_cow::RecordCow;
use crate::table::{TableConfig, TableManager};

struct Resolver(RepoInstance);

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table: &TableRef) -> DbResult<TableManager> {
        self.0.get_table(&table.table).await
    }

    async fn resolve_repo(&self, _: &str) -> DbResult<RepoInstance> {
        Ok(self.0.clone())
    }
}

async fn run(
    request: &BatchRequest,
    resolver: &Resolver,
    scope: &ExecScope,
) -> Result<shamir_query_types::batch::BatchResponse, BatchError> {
    execute_batch_impl(
        request,
        resolver,
        None,
        None,
        Actor::System,
        "test",
        0,
        &new_map(),
        scope,
    )
    .await
}

async fn setup() -> Resolver {
    let resolver = Resolver(
        RepoInstance::from_factory(
            "main".into(),
            BoxRepoFactory::in_memory(),
            vec![TableConfig::new("rows")],
        )
        .await
        .unwrap(),
    );
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        insert("rows").row(doc().set("pad", "x".repeat(10_240)).unwrap()),
    );
    let request = seed.build();
    run(&request, &resolver, &ExecScope::root(&request.limits))
        .await
        .unwrap();
    resolver
}

async fn add_second_row(resolver: &Resolver) {
    let mut seed = Batch::new();
    seed.insert(
        "second",
        insert("rows").row(doc().set("pad", "y".repeat(10_240)).unwrap()),
    );
    let request = seed.build();
    run(&request, resolver, &ExecScope::root(&request.limits))
        .await
        .unwrap();
}

fn request(query: Query, cap: usize) -> BatchRequest {
    let mut batch = Batch::new();
    batch.limits(BatchLimits {
        max_result_size: cap,
        ..BatchLimits::default()
    });
    batch.query("r", query);
    batch.build()
}

// Red before W10: both huge rows are accumulated before insertion charges
// the entire result, rather than stopping at the first row (10_355).
// The typed failure must survive DbError::Io.
#[tokio::test]
async fn three_sinks_stop_at_first_row_and_opt_out_is_unchanged() {
    let resolver = setup().await;
    add_second_row(&resolver).await;
    for query in [
        Query::from("rows"),
        Query::from("rows").order_by(OrderByItem::asc("pad")),
        Query::from("rows").count_total(true),
    ] {
        let request = request(query, 1024);
        let scope = ExecScope::root_with_result_size(&request.limits, true);
        assert_eq!(
            run(&request, &resolver, &scope).await.unwrap_err(),
            BatchError::LimitExceeded {
                kind: LimitKind::ResultBytes,
                actual: 10_355,
                max: 1024,
            },
        );
        assert_eq!(scope.bytes(), 10_355);
        let off = ExecScope::root(&request.limits);
        let response = run(&request, &resolver, &off).await.unwrap();
        let records = &response.results["r"].records;
        assert_eq!(records.len(), 2);
        // Compare complete field maps without depending on scan order.
        let mut payloads = Vec::new();
        for record in records {
            let QueryRecord::Direct(QueryValue::Map(fields)) = record else {
                panic!("expected unchanged field map, got {record:?}");
            };
            assert_eq!(fields.len(), 1);
            let QueryValue::Str(payload) = &fields["pad"] else {
                panic!("expected string payload, got {fields:?}");
            };
            payloads.push(payload.as_str());
        }
        payloads.sort_unstable();
        assert_eq!(payloads, vec!["x".repeat(10_240), "y".repeat(10_240)]);
        assert_eq!(off.bytes(), 0);
    }
}

#[tokio::test]
async fn select_nested_batch_and_foreach_have_exact_composed_totals() {
    let resolver = setup().await;
    let plain = request(Query::from("rows"), 100_000);
    let mut nested = Batch::new();
    nested.sub_batch_no_bind("n", plain.clone());
    let mut body = Batch::new();
    body.query("r", Query::from("rows"));
    let mut loop_batch = Batch::new();
    let source = loop_batch.query("source", Query::from("rows"));
    // A Read's all() is not a list; column("pad") resolves one item per row.
    loop_batch.for_each("loop", source.column("pad"), "row", body.build());
    // Row: 48 + map (24 + 16 + 3 + string 24 + 10_240) = 10_355.
    // SELECT: row 10_355 + result base 80 + stats 256 = 10_691.
    // Assembled envelope: 24 + (16 + 1) + 24 + (16 + 7)
    //                     + (24 + 16) + (16 + 5) = 149.
    // Nested: SELECT 10_691 + result base 80 + assembled 149 = 10_920.
    // ForEach: source 10_691 + loop shell (80 + 24) + body 10_691
    //          + iteration envelope (ENTRY 16 + assembled 149) = 21_651.
    for (request, alias, expected) in [
        (plain, "r", 10_691),
        (nested.build(), "n", 10_920),
        (loop_batch.build(), "loop", 21_651),
    ] {
        let scope = ExecScope::root_with_result_size(&request.limits, true);
        let response = run(&request, &resolver, &scope).await.unwrap();
        let result = &response.results[alias];
        if alias == "loop" {
            assert_eq!(response.results["source"].records.len(), 1);
            let Some(QueryValue::List(iterations)) = &result.value else {
                panic!("expected iteration list, got {result:?}");
            };
            assert_eq!(iterations.len(), 1, "one iteration per source row");
            let QueryValue::Map(iteration) = &iterations[0] else {
                panic!("expected body result map, got {iterations:?}");
            };
            let QueryValue::Map(body_result) = &iteration["r"] else {
                panic!("expected SELECT result envelope, got {iteration:?}");
            };
            let QueryValue::List(records) = &body_result["records"] else {
                panic!("expected body records, got {body_result:?}");
            };
            assert_eq!(records.len(), 1);
            let QueryValue::Map(fields) = &records[0] else {
                panic!("expected body field map, got {records:?}");
            };
            assert_eq!(fields.len(), 1);
            assert_eq!(fields["pad"], QueryValue::Str("x".repeat(10_240)));
        }
        assert_eq!(scope.bytes(), expected);
    }
}

// Raw aggregate input pays 56 + encoded bytes and receives no output credit.
#[tokio::test]
async fn grouped_raw_input_stops_before_aggregate_materialization() {
    let resolver = setup().await;
    add_second_row(&resolver).await;
    let table = resolver.0.get_table("rows").await.unwrap();
    let stream = table.list_stream(16);
    futures::pin_mut!(stream);
    let batch = stream.next().await.unwrap().unwrap();
    assert_eq!(batch.len(), 2);
    let raw_len = match &batch[0].1 {
        RecordCow::Borrowed(bytes) => bytes.len(),
        RecordCow::Owned(value) => value.to_bytes().unwrap().len(),
    };
    let expected = 56 + raw_len as u64;
    let request = request(Query::from("rows").group_by("pad"), 1024);
    let scope = ExecScope::root_with_result_size(&request.limits, true);
    assert_eq!(
        run(&request, &resolver, &scope).await.unwrap_err(),
        BatchError::LimitExceeded {
            kind: LimitKind::ResultBytes,
            actual: expected,
            max: 1024,
        },
    );
    assert_eq!(scope.bytes(), expected);
}

// Raw input and discarded projections remain charged, but cannot erase
// the uncharged output payload at completion.
#[test]
fn intermediate_bytes_do_not_credit_uncharged_output() {
    let limits = BatchLimits::default();
    let scope = ExecScope::root_with_result_size(&limits, true);
    let meter = RowByteMeter::new(&scope).unwrap();
    meter.charge(10_296).unwrap();
    meter
        .finish(&QueryResult::records_only(vec![QueryRecord::Direct(
            QueryValue::Int(1),
        )]))
        .unwrap();
    assert_eq!(scope.bytes(), 10_440); // raw + 80 envelope + 48 row + 16 value
}

// Overlap is deterministic: another read charges between this read's row
// and completion. A global before/after delta would incorrectly steal 80.
#[test]
fn local_credit_does_not_consume_another_reads_charge() {
    let limits = BatchLimits::default();
    let scope = ExecScope::root_with_result_size(&limits, true);
    let meter = RowByteMeter::new(&scope).unwrap();
    let row = QueryRecord::Direct(QueryValue::Str("x".into()));
    meter.charge_record(&row).unwrap(); // 48 + 24 + 1 = 73
    scope.charge_bytes(80).unwrap();
    meter
        .finish(&QueryResult {
            records: vec![row],
            ..QueryResult::default()
        })
        .unwrap();
    assert_eq!(scope.bytes(), 233); // 73 + unrelated 80 + own envelope 80
}
