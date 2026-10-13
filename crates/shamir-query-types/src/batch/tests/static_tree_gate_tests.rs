use shamir_collections::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::batch::planner::BatchPlanner;
use crate::batch::{
    BatchError, BatchLimits, BatchOp, BatchRequest, BatchTreeVisitor, ForEachOp, LimitKind,
    QueryEntry, ResultEncoding, SubBatchOp,
};
use crate::filter::FilterValue;
use crate::read::ReadQuery;

fn singleton(alias: &str, entry: QueryEntry) -> TMap<String, QueryEntry> {
    let mut queries = new_map();
    queries.insert(alias.to_string(), entry);
    queries
}

fn read() -> QueryEntry {
    QueryEntry::from(ReadQuery::new("items"))
}

fn request(queries: TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: new_map(),
        result_encoding: ResultEncoding::default(),
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

fn loop_over(over: FilterValue, body: TMap<String, QueryEntry>) -> QueryEntry {
    entry(BatchOp::ForEach(ForEachOp {
        over,
        bind_row: "row".to_string(),
        batch: request(body),
    }))
}

fn literal_loop(len: usize, body: TMap<String, QueryEntry>) -> QueryEntry {
    loop_over(FilterValue::Array(vec![FilterValue::Int(0); len]), body)
}

fn nested_loops(
    depth: usize,
    len: usize,
    mut body: TMap<String, QueryEntry>,
) -> TMap<String, QueryEntry> {
    for _ in 0..depth {
        body = singleton("loop", literal_loop(len, body));
    }
    body
}

fn assert_limit(
    queries: &TMap<String, QueryEntry>,
    limits: &BatchLimits,
    kind: LimitKind,
    actual: u64,
    max: u64,
) {
    let error = BatchPlanner::plan(queries, limits).expect_err("whole-tree gate must reject");
    match error {
        BatchError::LimitExceeded {
            kind: got,
            actual: count,
            max: cap,
        } => {
            assert_eq!((got, count, cap), (kind, actual, max));
        }
        other => panic!("expected structured limit error, got {other:?}"),
    }
}

#[test]
fn literal_ten_by_ten_reports_full_work_and_boundary_stages() {
    let queries = nested_loops(2, 10, singleton("leaf", read()));
    let mut limits = BatchLimits {
        max_total_work: 50,
        ..BatchLimits::default()
    };
    assert_limit(&queries, &limits, LimitKind::Work, 111, 50);
    limits.max_total_work = 111;
    limits.max_total_queries = 3;
    let plan = BatchPlanner::plan(&queries, &limits).expect("inclusive boundaries pass");
    assert_eq!(plan.stages, vec![vec!["loop".to_string()]]);
    assert_eq!(plan.aliases, vec!["loop".to_string()]);
    assert!(plan.dependencies["loop"].is_empty());
}

#[test]
fn nested_batches_count_all_entries_before_work_and_pass_at_boundary() {
    let body = singleton(
        "inner",
        entry(BatchOp::Batch(SubBatchOp {
            batch: request(singleton("leaf", read())),
            bind: new_map(),
        })),
    );
    let mut queries = singleton(
        "outer",
        entry(BatchOp::Batch(SubBatchOp {
            batch: request(body),
            bind: new_map(),
        })),
    );
    queries.insert("sibling".to_string(), read());
    let mut limits = BatchLimits {
        max_queries: 2,
        max_total_queries: 2,
        max_total_work: 0,
        ..BatchLimits::default()
    };
    assert_limit(&queries, &limits, LimitKind::TotalQueries, 4, 2);
    limits.max_total_queries = 4;
    limits.max_total_work = 4;
    let plan = BatchPlanner::plan(&queries, &limits).expect("exact budgets pass");
    assert_eq!(
        plan.stages,
        vec![vec!["outer".to_string(), "sibling".to_string()]]
    );
    assert_eq!(
        plan.aliases,
        vec!["outer".to_string(), "sibling".to_string()]
    );
}

#[test]
fn dynamic_over_factor_one_preserves_literal_enclosing_multiplier() {
    let dynamic = loop_over(
        FilterValue::QueryRef {
            alias: "@source".to_string(),
            path: Some("[].id".to_string()),
        },
        singleton("leaf", read()),
    );
    let mut body = singleton("source", read());
    body.insert("dynamic".to_string(), dynamic);
    let queries = singleton("loop", literal_loop(10, body));
    let mut limits = BatchLimits {
        max_total_work: 30,
        ..BatchLimits::default()
    };
    assert_limit(&queries, &limits, LimitKind::Work, 31, 30);
    limits.max_total_work = 31;
    let plan = BatchPlanner::plan(&queries, &limits).expect("dynamic factor is one");
    assert_eq!(plan.stages, vec![vec!["loop".to_string()]]);
}

#[test]
fn nested_product_and_work_sum_saturate_with_small_lists() {
    let queries = nested_loops(16, 16, singleton("leaf", read()));
    let mut limits = BatchLimits {
        max_nesting_depth: 16,
        max_total_work: u64::MAX - 1,
        ..BatchLimits::default()
    };
    assert_limit(&queries, &limits, LimitKind::Work, u64::MAX, u64::MAX - 1);
    limits.max_total_work = u64::MAX;
    let plan = BatchPlanner::plan(&queries, &limits).expect("saturated boundary passes");
    assert_eq!(plan.stages, vec![vec!["loop".to_string()]]);
}

#[test]
fn work_addition_saturates_even_when_each_product_fits() {
    let mut queries = nested_loops(63, 2, singleton("leaf", read()));
    queries.insert("sibling".to_string(), read());
    let limits = BatchLimits {
        max_nesting_depth: 63,
        max_total_work: u64::MAX - 1,
        ..BatchLimits::default()
    };
    assert_limit(&queries, &limits, LimitKind::Work, u64::MAX, u64::MAX - 1);
}

#[test]
fn zero_after_saturation_and_siblings_restore_their_own_multiplier() {
    let mut body = singleton(
        "empty_loop",
        literal_loop(0, singleton("zero_leaf", read())),
    );
    body.insert("deep_sibling".to_string(), read());
    let mut queries = nested_loops(16, 16, body);
    queries.insert("root_sibling".to_string(), read());
    let visits: Vec<_> = BatchTreeVisitor::new(&queries)
        .map(|visit| (visit.alias.to_string(), visit.multiplier))
        .collect();
    assert_eq!(
        &visits[16..],
        &[
            ("empty_loop".to_string(), u64::MAX),
            ("zero_leaf".to_string(), 0),
            ("deep_sibling".to_string(), u64::MAX),
            ("root_sibling".to_string(), 1),
        ]
    );
}

#[test]
fn legacy_checks_keep_variants_and_precedence_over_new_gates() {
    let queries = nested_loops(2, 10, singleton("leaf", read()));
    let mut limits = BatchLimits {
        max_queries: 0,
        max_nesting_depth: 0,
        max_total_queries: 0,
        max_total_work: 0,
        ..BatchLimits::default()
    };
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(result, Err(BatchError::TooManyQueries { count: 1, max: 0 })),
        "root count must win: {result:?}"
    );
    limits.max_queries = 1;
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(result, Err(BatchError::NestingTooDeep { depth: 2, max: 0 })),
        "nesting must win over virtual/new gates: {result:?}"
    );
    limits.max_nesting_depth = 2;
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(
            result,
            Err(BatchError::TooManyQueries { count: 10, max: 1 })
        ),
        "virtual units must win over new gates: {result:?}"
    );
}
