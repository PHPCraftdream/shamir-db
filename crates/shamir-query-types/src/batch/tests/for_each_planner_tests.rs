//! Planner-level tests for `BatchOp::ForEach` (Epic04/D, #655 gap 1).
//!
//! Covers the parts of `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`
//! Decision 1/3 that are exercised by `BatchPlanner::plan` but not yet
//! pinned by a test: dependency extraction from `over`, black-box
//! non-unfolding of the loop body, the static DoS gate that folds
//! `iterations × body.len()` into `max_queries`, and nesting-depth
//! accounting for a `ForEach` body (mirrors the existing `SubBatchOp`
//! coverage in `planner_tests.rs`).

use shamir_collections::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::batch::planner::BatchPlanner;
use crate::batch::{
    BatchError, BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use crate::filter::FilterValue;
use crate::read::ReadQuery;

// -------------------------------------------------------------------------
// Helpers (mirrors planner_tests.rs's conventions)
// -------------------------------------------------------------------------

fn read_entry(table: &str) -> QueryEntry {
    let q = ReadQuery {
        from: crate::TableRef::new(table),
        r#where: None,
        select: crate::read::Select::all(),
        order_by: None,
        pagination: crate::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: crate::read::Temporal::default(),
        with_version: false,
        explain: false,
    };
    QueryEntry {
        op: BatchOp::Read(q),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

fn empty_batch_request() -> BatchRequest {
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: TMap::default(),
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn batch_request_with_queries(queries: TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: TMap::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn for_each_entry(over: FilterValue, bind_row: &str, batch: BatchRequest) -> QueryEntry {
    QueryEntry {
        op: BatchOp::ForEach(ForEachOp {
            over,
            bind_row: bind_row.to_string(),
            batch,
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

// -------------------------------------------------------------------------
// Dependency extraction from `over` (ADR Decision 1 — "outer deps come
// exclusively from `over`", planner.rs around line 288).
// -------------------------------------------------------------------------

#[test]
fn for_each_over_query_ref_creates_dep() {
    // outer batch:
    //   a    -> ReadQuery (no deps)
    //   loop -> ForEach { over: $query @a[].id, bind_row: "elem", batch: {} }
    // Expected: "a" is in an earlier stage than "loop";
    //           dependencies["loop"] contains "a".
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("a".to_string(), read_entry("users"));
    queries.insert(
        "loop".to_string(),
        for_each_entry(
            FilterValue::QueryRef {
                alias: "@a".to_string(),
                path: Some("[].id".to_string()),
            },
            "elem",
            empty_batch_request(),
        ),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let loop_deps = plan
        .dependencies
        .get("loop")
        .expect("loop must have deps entry");
    assert!(
        loop_deps.contains("a"),
        "loop should depend on a via `over`, got {:?}",
        loop_deps
    );

    let a_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"a".to_string()))
        .expect("a must be in some stage");
    let loop_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"loop".to_string()))
        .expect("loop must be in some stage");
    assert!(a_stage < loop_stage, "a must come before loop");
}

#[test]
fn for_each_literal_over_has_no_outer_dep() {
    // A ForEach whose `over` is a literal array has no outer deps -> can be
    // stage 0, mirroring `sub_batch_no_bind_no_dep`.
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "loop".to_string(),
        for_each_entry(
            FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
            "elem",
            empty_batch_request(),
        ),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let loop_deps = plan.dependencies.get("loop").expect("loop entry");
    assert!(
        loop_deps.is_empty(),
        "literal-array `over` must not create an outer dep, got {:?}",
        loop_deps
    );
    assert!(
        plan.stages[0].contains(&"loop".to_string()),
        "loop with no deps should be in stage 0"
    );
}

// -------------------------------------------------------------------------
// Black-box non-unfolding: the ForEach body's own internal aliases must NOT
// appear as top-level nodes in the parent BatchPlanner's stages/DAG.
// -------------------------------------------------------------------------

#[test]
fn for_each_body_aliases_do_not_unfold_into_parent_dag() {
    // Inner body has its own alias "inner_read", which must NOT show up
    // anywhere in the OUTER plan's stages/dependencies -- only "loop" does.
    let mut inner_queries: TMap<String, QueryEntry> = new_map();
    inner_queries.insert("inner_read".to_string(), read_entry("orders"));
    let inner_batch = batch_request_with_queries(inner_queries);

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "loop".to_string(),
        for_each_entry(FilterValue::Array(vec![]), "elem", inner_batch),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    // Only "loop" is a top-level alias/node -- the inner alias never
    // appears in dependencies or any stage.
    assert_eq!(
        plan.aliases,
        vec!["loop".to_string()],
        "the outer plan's aliases must be exactly the top-level entries, \
         not the ForEach body's inner aliases"
    );
    assert!(
        !plan.dependencies.contains_key("inner_read"),
        "the ForEach body's inner alias must not appear in the outer \
         plan's dependency map, got {:?}",
        plan.dependencies
    );
    for stage in &plan.stages {
        assert!(
            !stage.contains(&"inner_read".to_string()),
            "the ForEach body's inner alias must not appear in any outer \
             stage, got stages={:?}",
            plan.stages
        );
    }
}

// -------------------------------------------------------------------------
// Static DoS gate (ADR Decision 3): literal `over` of length N and a body
// of M queries fold `N * M` into the same `max_queries` budget.
// -------------------------------------------------------------------------

#[test]
fn for_each_static_dos_gate_under_budget_succeeds() {
    // N=2 iterations * M=2 body queries = 4 virtual units, well under the
    // default max_queries (50).
    let mut inner_queries: TMap<String, QueryEntry> = new_map();
    inner_queries.insert("q1".to_string(), read_entry("orders"));
    inner_queries.insert("q2".to_string(), read_entry("orders"));
    let inner_batch = batch_request_with_queries(inner_queries);

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "loop".to_string(),
        for_each_entry(
            FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
            "elem",
            inner_batch,
        ),
    );

    let limits = BatchLimits::default();
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        result.is_ok(),
        "2 iterations * 2 body queries = 4 virtual units, under max_queries(50): {:?}",
        result
    );
}

#[test]
fn for_each_static_dos_gate_over_budget_errors() {
    // N=10 iterations * M=10 body queries = 100 virtual units, over a
    // tightened max_queries(50) budget -> must be rejected at plan time,
    // before any iteration would ever run.
    let limits = BatchLimits {
        max_queries: 50,
        ..BatchLimits::default()
    };

    let mut inner_queries: TMap<String, QueryEntry> = new_map();
    for i in 0..10 {
        inner_queries.insert(format!("q{i}"), read_entry("orders"));
    }
    let inner_batch = batch_request_with_queries(inner_queries);

    let over_items: Vec<FilterValue> = (0..10).map(FilterValue::Int).collect();

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "loop".to_string(),
        for_each_entry(FilterValue::Array(over_items), "elem", inner_batch),
    );

    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(
            result,
            Err(BatchError::TooManyQueries {
                count: 100,
                max: 50,
            })
        ),
        "10 iterations * 10 body queries = 100 virtual units must be \
         rejected as TooManyQueries{{count:100,max:50}}, got {:?}",
        result
    );
}

// -------------------------------------------------------------------------
// Nesting-depth accounting: a ForEach node's body counts toward
// max_nesting_depth the same way a SubBatchOp's body does.
// -------------------------------------------------------------------------

#[test]
fn for_each_nesting_depth_within_limit_ok() {
    // Same construction as planner_tests.rs's
    // `nesting_depth_within_limit_ok`, but with a ForEach at the top level
    // instead of a SubBatchOp.
    let limits = BatchLimits {
        max_nesting_depth: 4,
        ..BatchLimits::default()
    };

    // Start with an empty leaf batch, wrap 3 times with plain sub-batches
    // to reach 3 levels of nesting below the top-level ForEach.
    let mut inner = empty_batch_request();
    for _ in 0..3 {
        let mut outer_queries: TMap<String, QueryEntry> = new_map();
        outer_queries.insert(
            "inner".to_string(),
            QueryEntry {
                op: BatchOp::Batch(crate::batch::SubBatchOp {
                    batch: inner,
                    bind: TMap::default(),
                }),
                return_result: true,
                after: Vec::new(),
                when: None,
            },
        );
        inner = batch_request_with_queries(outer_queries);
    }

    // Top-level "deep" is a ForEach wrapping the 3-deep chain -> 4th level.
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "deep".to_string(),
        for_each_entry(FilterValue::Array(vec![]), "elem", inner),
    );

    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        result.is_ok(),
        "ForEach nesting at limit should succeed: {:?}",
        result
    );
}

#[test]
fn for_each_nesting_depth_exceeded_errors() {
    let limits = BatchLimits {
        max_nesting_depth: 2,
        ..BatchLimits::default()
    };

    // With limit=2: a top-level ForEach op is depth 1; one level deeper via
    // a plain sub-batch is depth 2 (== limit, ok); wrapping one more level
    // gives depth 3 (> limit).
    let mut inner = empty_batch_request();
    for _ in 0..2 {
        let mut outer_queries: TMap<String, QueryEntry> = new_map();
        outer_queries.insert(
            "inner".to_string(),
            QueryEntry {
                op: BatchOp::Batch(crate::batch::SubBatchOp {
                    batch: inner,
                    bind: TMap::default(),
                }),
                return_result: true,
                after: Vec::new(),
                when: None,
            },
        );
        inner = batch_request_with_queries(outer_queries);
    }

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "deep".to_string(),
        for_each_entry(FilterValue::Array(vec![]), "elem", inner),
    );

    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(result, Err(BatchError::NestingTooDeep { .. })),
        "expected NestingTooDeep error for a ForEach body nested past the \
         limit, got {:?}",
        result
    );
}
