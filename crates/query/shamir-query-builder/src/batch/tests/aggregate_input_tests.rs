use shamir_query_types::batch::planner::BatchPlanner;
use shamir_query_types::batch::{BatchError, BatchLimits, EdgeKind};

use crate::batch::{Batch, BuildError};
use crate::query::Query;
use crate::select;
use crate::val::{col, lit, mul, param, qref};

#[test]
fn planner_orders_expression_inputs_after_referenced_query() {
    let mut batch = Batch::new();
    // Insert consumers first: insertion order alone must not satisfy the oracle.
    batch.query(
        "total",
        Query::from("values").select([select::sum_of(mul(col("v"), qref("q1", "[0].m")), "sum")]),
    );
    batch.query(
        "percentile",
        Query::from("values").select([select::agg_fn_with_args_of(
            "percentile",
            mul(col("v"), qref("q1", "[0].m")),
            "p",
            [lit(0.5)],
        )]),
    );
    batch.query("q1", Query::from("multipliers"));
    let request = batch.try_build().unwrap();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
    assert_eq!(
        plan.stages,
        vec![
            vec!["q1".to_string()],
            vec!["total".to_string(), "percentile".to_string()],
        ],
    );
    for alias in ["total", "percentile"] {
        assert_eq!(
            plan.dependencies[alias].iter().cloned().collect::<Vec<_>>(),
            vec!["q1"],
        );
        assert_eq!(plan.edge_provenance[alias]["q1"], EdgeKind::DataFlow);
    }
}

#[test]
fn builder_rejects_unknown_alias_inside_aggregate_input() {
    for item in [
        select::sum_of(mul(col("v"), qref("missing", "[0].m")), "sum"),
        select::agg_fn_of("median", qref("missing", "[0].m"), "middle"),
    ] {
        let mut batch = Batch::new();
        batch.query("consumer", Query::from("values").select([item]));
        match batch.try_build().unwrap_err() {
            BuildError::UnknownAlias {
                alias,
                referenced_by,
            } => {
                assert_eq!(alias, "missing");
                assert_eq!(referenced_by, "consumer");
            }
            other => panic!("expected UnknownAlias, got {other:?}"),
        }
    }
}

#[test]
fn planner_rejects_unknown_alias_without_builder_validation() {
    let mut batch = Batch::new();
    batch.query(
        "consumer",
        Query::from("values").select([select::avg_of(qref("missing", "[0].m"), "mean")]),
    );
    let request = batch.build();
    match BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap_err() {
        BatchError::UnknownAlias {
            alias,
            referenced_by,
        } => {
            assert_eq!(alias, "missing");
            assert_eq!(referenced_by, "consumer");
        }
        other => panic!("expected UnknownAlias, got {other:?}"),
    }
}

#[test]
fn parameter_inputs_do_not_create_query_dependencies() {
    let mut batch = Batch::new();
    batch.query(
        "mean",
        Query::from("values").select([select::avg_of(mul(col("price"), param("rate")), "avg")]),
    );
    let request = batch.try_build().unwrap();
    let plan = BatchPlanner::plan(&request.queries, &BatchLimits::default()).unwrap();
    assert_eq!(plan.stages, vec![vec!["mean".to_string()]]);
    assert!(plan.dependencies["mean"].is_empty());
}
