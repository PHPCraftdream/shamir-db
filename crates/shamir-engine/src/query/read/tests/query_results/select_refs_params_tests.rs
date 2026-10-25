//! B14 regressions: execution-local SELECT references and parameters.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::{select, val, write, Query};
use shamir_query_types::batch::{BatchLimits, BatchPlanner};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked, TableResolver};
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

/// Red before B14: SELECT contributes no DAG edge and evaluates with empty
/// refs. SelectExpr cannot represent a QueryRef, so use the builder's
/// FilterValue arithmetic tree inside a scalar SELECT slot.
#[tokio::test]
async fn select_arithmetic_ref_is_staged_and_resolved() {
    let resolver = setup().await;
    let mut batch = Batch::new();
    // Deliberately insert the consumer first: incidental insertion order
    // cannot hide a missing dependency.
    batch.query(
        "q2",
        Query::from("rows").select([select::func(
            "total",
            "math/max",
            [val::add(val::col("v"), val::qref("q1", "[0].m"))],
        )]),
    );
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
    let response = execute_batch_unchecked(&request, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(response.results["q2"].records.len(), 1);
    assert_eq!(
        response.results["q2"].records[0].get_value_i64("total"),
        Some(10)
    );
}

#[tokio::test]
async fn repeated_batch_select_query_ref_uses_current_execution() {
    let resolver = setup().await;
    for (field, expected) in [("m", 7), ("v", 3)] {
        let mut batch = Batch::new();
        batch.query(
            "q2",
            Query::from("rows").select([select::func(
                "tag",
                "math/max",
                [val::qref("q1", "[0].value")],
            )]),
        );
        batch.query(
            "q1",
            Query::from("rows").select([select::field_as(field, "value")]),
        );
        let response =
            execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test")
                .await
                .unwrap();
        assert_eq!(response.results["q1"].records.len(), 1);
        assert_eq!(
            response.results["q1"].records[0].get_value_i64("value"),
            Some(expected)
        );
        assert_eq!(response.results["q2"].records.len(), 1);
        assert_eq!(
            response.results["q2"].records[0].get_value_i64("tag"),
            Some(expected)
        );
    }
}

/// Red before B14: both tags were Null. Exact per-iteration outputs also
/// catch a projection/parameter snapshot reused from the first iteration.
#[tokio::test]
async fn foreach_select_params_are_fresh_per_iteration() {
    let resolver = setup().await;
    let mut body = Batch::new();
    body.query(
        "tagged",
        Query::from("rows").select([select::func("tag", "math/max", [val::param("row")])]),
    );
    let mut outer = Batch::new();
    outer.for_each("loop", vec![val::lit(1), val::lit(2)], "row", body.build());
    let response =
        execute_batch_unchecked(&outer.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    let QueryValue::List(iterations) = response.results["loop"].value.as_ref().unwrap() else {
        panic!("expected iteration list");
    };
    assert_eq!(iterations.len(), 2);
    for (iteration, expected) in iterations.iter().zip([1, 2]) {
        let QueryValue::Map(results) = iteration else {
            panic!("expected result map");
        };
        let QueryValue::Map(result) = &results["tagged"] else {
            panic!("expected query result");
        };
        let QueryValue::List(records) = &result["records"] else {
            panic!("expected records");
        };
        assert_eq!(records.len(), 1);
        let QueryValue::Map(record) = &records[0] else {
            panic!("expected projected row");
        };
        assert_eq!(record["tag"], QueryValue::Int(expected));
    }
}

/// Red before B14: the function slot in aggregate.rs emitted Null even
/// though q1 had completed. Also exercises GROUP BY's context forwarding.
#[tokio::test]
async fn aggregate_select_function_argument_resolves_query_ref() {
    let resolver = setup().await;
    for grouped in [false, true] {
        let mut batch = Batch::new();
        let mut query = Query::from("rows").select([
            select::count_all("count"),
            select::func("tag", "math/max", [val::qref("q1", "[0].m")]),
        ]);
        if grouped {
            query = query.group_by("v");
        }
        batch.query("q2", query);
        batch.query("q1", Query::from("rows").select([select::field("m")]));
        let response =
            execute_batch_unchecked(&batch.build(), &resolver, None, None, Actor::System, "test")
                .await
                .unwrap();
        assert_eq!(response.results["q2"].records.len(), 1);
        let record = &response.results["q2"].records[0];
        assert_eq!(record.get_value_i64("count"), Some(1));
        assert_eq!(record.get_value_i64("tag"), Some(7));
    }
}

/// Dependency extraction includes AggregateFn parameters even though the
/// current aggregate validator still requires literal parameters.
#[test]
fn aggregate_parameters_contribute_select_dependencies() {
    let mut batch = Batch::new();
    batch.query(
        "q2",
        Query::from("rows").select([select::agg_fn_with_args(
            "percentile",
            "v",
            "p",
            [val::qref("q1", "[0].m")],
        )]),
    );
    batch.query("q1", Query::from("rows").select([select::field("m")]));
    let request = batch.build();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
    assert!(plan.dependencies["q2"].contains("q1"));
}
