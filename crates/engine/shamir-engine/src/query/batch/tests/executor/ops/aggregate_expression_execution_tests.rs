//! B14-3: per-row aggregate expressions in real batch executions.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::{select, val, write, Query};
use shamir_query_types::batch::{BatchLimits, BatchPlanner};
use shamir_query_types::read::{PlanType, SelectItem};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked, TableResolver};
use crate::query::filter::eval_context::FilterContext;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};

struct Resolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for Resolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db
            .get_repo("default")
            .ok_or_else(|| DbError::NotFound("default repo".into()))
    }
}

async fn setup() -> Resolver {
    let db = DbInstance::with_repos(vec![RepoConfig {
        name: "default".into(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("rows")],
    }])
    .await
    .unwrap();
    let resolver = Resolver { db };
    let mut seed = Batch::new();
    seed.insert(
        "seed",
        write::insert("rows").row(
            write::doc()
                .set("v", 3)
                .unwrap()
                .set("m", 7)
                .unwrap()
                .build(),
        ),
    );
    execute_batch_unchecked(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    resolver
}

/// Consumer-first insertion must still stage both expression aggregate paths
/// after q1. Before the fix the per-row input is ignored (All is not numeric).
#[tokio::test]
async fn sum_input_query_ref_is_ordered_and_resolved_grouped_and_ungrouped() {
    let resolver = setup().await;
    for grouped in [false, true] {
        let mut batch = Batch::new();
        let mut query = Query::from("rows").select([select::sum_of(
            val::mul(val::col("v"), val::qref("q1", "[0].m")),
            "total",
        )]);
        if grouped {
            query = query.group_by("v");
        }
        batch.query("q2", query);
        batch.query("q1", Query::from("rows").select([select::field("m")]));
        let request = batch.build();
        let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
        let stage = |alias: &str| {
            plan.stages
                .iter()
                .position(|aliases| aliases.iter().any(|a| a == alias))
                .unwrap()
        };
        assert!(stage("q2") > stage("q1"));
        let response =
            execute_batch_unchecked(&request, &resolver, None, None, Actor::System, "test")
                .await
                .unwrap();
        assert_eq!(response.results["q2"].records.len(), 1);
        assert_eq!(
            response.results["q2"].records[0].get_value_i64("total"),
            Some(21),
        );
    }
}

/// Interleaved rows accumulate independently in each group, irrespective of
/// scan order. Before the fix expression SUMs used All and returned zero.
#[tokio::test]
async fn grouped_expression_sums_accumulate_multiple_rows_without_group_leakage() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.insert(
        "seed_groups",
        write::insert("rows").rows([
            write::doc()
                .set("g", "a")
                .unwrap()
                .set("v", 2)
                .unwrap()
                .set("m", 3)
                .unwrap()
                .build(),
            write::doc()
                .set("g", "b")
                .unwrap()
                .set("v", 5)
                .unwrap()
                .set("m", 2)
                .unwrap()
                .build(),
            write::doc()
                .set("g", "a")
                .unwrap()
                .set("v", 4)
                .unwrap()
                .set("m", 3)
                .unwrap()
                .build(),
            write::doc()
                .set("g", "b")
                .unwrap()
                .set("v", 7)
                .unwrap()
                .set("m", 2)
                .unwrap()
                .build(),
        ]),
    );
    execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut read = Batch::new();
    read.query(
        "totals",
        Query::from("rows")
            .select([
                select::field("g"),
                select::sum_of(val::mul(val::col("v"), val::col("m")), "total"),
                select::count_of(val::col("g"), "count"),
            ])
            .group_by("g"),
    );
    let response =
        execute_batch_unchecked(&read.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    let records = &response.results["totals"].records;
    // setup's original row is the NULL-key group; neither named group may absorb it.
    assert_eq!(records.len(), 3);
    for (group, total, count) in [
        (QueryValue::Str("a".into()), 18, 2),
        (QueryValue::Str("b".into()), 24, 2),
        (QueryValue::Null, 21, 0),
    ] {
        let matching = records
            .iter()
            .filter(|record| record.get_value_owned("g") == Some(group.clone()))
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].get_value_i64("total"), Some(total));
        assert_eq!(matching[0].get_value_i64("count"), Some(count));
    }
}

/// Reusing the body's query must not reuse the first iteration's bindings.
/// Small integer arithmetic and these averages are exactly representable.
#[tokio::test]
async fn foreach_avg_input_uses_current_multiplier() {
    let resolver = setup().await;
    let mut body = Batch::new();
    body.query(
        "mean",
        Query::from("rows").select([select::avg_of(
            val::mul(val::col("v"), val::param("rate")),
            "average",
        )]),
    );
    let mut outer = Batch::new();
    outer.for_each("loop", vec![val::lit(1), val::lit(2)], "rate", body.build());
    let response =
        execute_batch_unchecked(&outer.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    let QueryValue::List(iterations) = response.results["loop"].value.as_ref().unwrap() else {
        panic!("expected iteration list");
    };
    assert_eq!(iterations.len(), 2);
    for (iteration, expected) in iterations.iter().zip([3.0, 6.0]) {
        let QueryValue::Map(results) = iteration else {
            panic!("expected result map");
        };
        let QueryValue::Map(result) = &results["mean"] else {
            panic!("expected query result");
        };
        let QueryValue::List(records) = &result["records"] else {
            panic!("expected records");
        };
        assert_eq!(records.len(), 1);
        let QueryValue::Map(record) = &records[0] else {
            panic!("expected aggregate row");
        };
        assert_eq!(record["average"], QueryValue::F64(expected));
    }
}

/// Factory fraction is not an extra per-row operand. Input and configuration
/// bind separately, and the exact one-row percentile is the transformed value.
#[tokio::test]
async fn funclib_input_is_separate_from_factory_args() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    batch.query(
        "q2",
        Query::from("rows").select([select::agg_fn_with_args_of(
            "percentile",
            val::mul(val::col("v"), val::qref("q1", "[0].m")),
            "p",
            [val::lit(0.5)],
        )]),
    );
    batch.query("q1", Query::from("rows").select([select::field("m")]));
    let response =
        execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    assert_eq!(response.results["q2"].records.len(), 1);
    assert_eq!(
        response.results["q2"].records[0].get_value_owned("p"),
        Some(QueryValue::Int(21)),
    );
}

/// A populated sorted index is eligible for field MIN/MAX, but an explicit
/// input must override that field in both execution and EXPLAIN. Hand oracle:
/// v = [3, 8, 5], -2 * v = [-6, -16, -10], so MIN = -16 and MAX = -6.
/// Without the four input guards execution returns 3/8 and EXPLAIN advertises
/// MinMaxIndex. The field-only controls ensure the shortcut is really eligible.
#[tokio::test]
async fn min_max_expression_overrides_indexed_field_and_explain_shortcut() {
    let resolver = setup().await;
    let mut seed = Batch::new();
    seed.insert(
        "more_rows",
        write::insert("rows").rows([
            write::doc().set("v", 8).unwrap().build(),
            write::doc().set("v", 5).unwrap().build(),
        ]),
    );
    execute_batch_unchecked(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let table = resolver.db.get_table("default", "rows").await.unwrap();
    table.create_sorted_index("by_v", &["v"]).await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    for (mut item, field_extreme, expression_extreme) in [
        (select::min("v", "extreme"), 3, -16),
        (select::max("v", "extreme"), 8, -6),
    ] {
        let field_query = Query::from("rows").select([item.clone()]).build();
        let field_result = table.read(&field_query, &ctx).await.unwrap();
        assert_eq!(field_result.records.len(), 1);
        assert_eq!(
            field_result.records[0].get_value_i64("extreme"),
            Some(field_extreme),
        );
        let field_explain = Query::from("rows").select([item.clone()]).explain().build();
        let field_plan = table.read(&field_explain, &ctx).await.unwrap();
        assert_eq!(
            field_plan.explain.as_ref().unwrap().plan_type,
            PlanType::MinMaxIndex,
        );

        let SelectItem::Aggregate { input, .. } = &mut item else {
            panic!("field aggregate builder must return Aggregate");
        };
        *input = Some(val::mul(val::col("v"), val::lit(-2)));
        let expression_query = Query::from("rows").select([item.clone()]).build();
        let result = table.read(&expression_query, &ctx).await.unwrap();
        assert_eq!(result.records.len(), 1);
        assert_eq!(
            result.records[0].get_value_i64("extreme"),
            Some(expression_extreme),
        );
        let expression_explain = Query::from("rows").select([item]).explain().build();
        let explained = table.read(&expression_explain, &ctx).await.unwrap();
        assert!(explained.records.is_empty());
        let plan = explained.explain.as_ref().unwrap();
        assert_eq!(plan.plan_type, PlanType::FullScan);
        assert!(plan.index_used.is_none());
    }
}
