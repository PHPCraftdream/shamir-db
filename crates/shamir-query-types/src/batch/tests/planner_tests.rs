use shamir_collections::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use crate::batch::planner::BatchPlanner;
use crate::batch::{
    BatchError, BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use crate::filter::{Cond, Filter, FilterValue};
use crate::read::ReadQuery;
use crate::write::InsertOp;

// -------------------------------------------------------------------------
// Helpers
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

fn sub_batch_entry(inner: BatchRequest, bind: TMap<String, FilterValue>) -> QueryEntry {
    QueryEntry {
        op: BatchOp::Batch(SubBatchOp { batch: inner, bind }),
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

// -------------------------------------------------------------------------
// Test 1: sub_batch_bind_query_ref_creates_dep
// -------------------------------------------------------------------------

#[test]
fn sub_batch_bind_query_ref_creates_dep() {
    // outer batch:
    //   user  → ReadQuery (no deps)
    //   proc  → BatchOp::Batch with bind: { uid: $query @user[0].id }
    // Expected: "user" is in an earlier stage than "proc";
    //           dependencies["proc"] contains "user".
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("user".to_string(), read_entry("users"));

    let mut bind: TMap<String, FilterValue> = new_map();
    bind.insert(
        "uid".to_string(),
        FilterValue::QueryRef {
            alias: "@user[0].id".to_string(),
            path: None,
        },
    );
    queries.insert(
        "proc".to_string(),
        sub_batch_entry(empty_batch_request(), bind),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    // proc must depend on user
    let proc_deps = plan
        .dependencies
        .get("proc")
        .expect("proc must have deps entry");
    assert!(
        proc_deps.contains("user"),
        "proc should depend on user, got {:?}",
        proc_deps
    );

    // user must be in an earlier stage
    let user_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"user".to_string()))
        .expect("user must be in some stage");
    let proc_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"proc".to_string()))
        .expect("proc must be in some stage");
    assert!(user_stage < proc_stage, "user must come before proc");
}

// -------------------------------------------------------------------------
// Test 2: sub_batch_no_bind_no_dep
// -------------------------------------------------------------------------

#[test]
fn sub_batch_no_bind_no_dep() {
    // A sub-batch with empty bind has no outer deps → can be stage 0.
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("user".to_string(), read_entry("users"));
    queries.insert(
        "proc".to_string(),
        sub_batch_entry(empty_batch_request(), new_map()),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let proc_deps = plan.dependencies.get("proc").expect("proc entry");
    assert!(proc_deps.is_empty(), "proc should have no outer deps");

    // Both should be in stage 0
    let stage0 = &plan.stages[0];
    assert!(
        stage0.contains(&"proc".to_string()),
        "proc with no deps should be in stage 0"
    );
}

// -------------------------------------------------------------------------
// Test 3: nesting_depth_within_limit_ok
// -------------------------------------------------------------------------

#[test]
fn nesting_depth_within_limit_ok() {
    // Depth is measured by max_nesting_depth_of_ops:
    // - outer queries map is depth 0 (the batch passed to plan()).
    // - A BatchOp::Batch at the top level is depth 1.
    // - A BatchOp::Batch one level deeper is depth 2.
    // - etc.
    //
    // To hit exactly depth == max_nesting_depth (4) we insert
    // (max - 1) = 3 wrapping levels below the top-level Batch op.
    let limits = BatchLimits {
        max_nesting_depth: 4,
        ..BatchLimits::default()
    };

    // Start with an empty leaf batch.
    let mut inner = empty_batch_request();
    // Wrap 3 times → chain of 3 nested Batch ops inside.
    for _ in 0..3 {
        let mut outer_queries: TMap<String, QueryEntry> = new_map();
        outer_queries.insert("inner".to_string(), sub_batch_entry(inner, new_map()));
        inner = batch_request_with_queries(outer_queries);
    }

    // Top-level entry "deep" is the 4th nesting level.
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("deep".to_string(), sub_batch_entry(inner, new_map()));

    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        result.is_ok(),
        "nesting at limit should succeed: {:?}",
        result
    );
}

// -------------------------------------------------------------------------
// Test 4: nesting_depth_exceeded_errors
// -------------------------------------------------------------------------

#[test]
fn nesting_depth_exceeded_errors() {
    let limits = BatchLimits {
        max_nesting_depth: 2,
        ..BatchLimits::default()
    };

    // With limit=2, a top-level Batch op is depth 1; one level deeper is
    // depth 2 (== limit, ok). Wrapping one more level gives depth 3 (> limit).
    // So: 2 additional wrappings inside the top-level Batch op → depth 3.
    let mut inner = empty_batch_request();
    for _ in 0..2 {
        let mut outer_queries: TMap<String, QueryEntry> = new_map();
        outer_queries.insert("inner".to_string(), sub_batch_entry(inner, new_map()));
        inner = batch_request_with_queries(outer_queries);
    }

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("deep".to_string(), sub_batch_entry(inner, new_map()));

    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        matches!(result, Err(BatchError::NestingTooDeep { .. })),
        "expected NestingTooDeep error, got {:?}",
        result
    );
}

// -------------------------------------------------------------------------
// Test 5: param_value_not_treated_as_dep
// -------------------------------------------------------------------------

#[test]
fn param_value_not_treated_as_dep() {
    // A sub-batch whose bind uses FilterValue::Param (inner-scope param)
    // should NOT create an outer-level dependency.
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("user".to_string(), read_entry("users"));

    let mut bind: TMap<String, FilterValue> = new_map();
    bind.insert(
        "uid".to_string(),
        FilterValue::Param {
            name: "user_id".to_string(),
        },
    );
    queries.insert(
        "proc".to_string(),
        sub_batch_entry(empty_batch_request(), bind),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let proc_deps = plan.dependencies.get("proc").expect("proc entry");
    assert!(
        proc_deps.is_empty(),
        "FilterValue::Param must not create outer dep, got {:?}",
        proc_deps
    );
}

// -------------------------------------------------------------------------
// Edge provenance tests (task #628 — Epic01/A)
// -------------------------------------------------------------------------

fn read_entry_with_query_ref(table: &str, dep_alias: &str) -> QueryEntry {
    let q = ReadQuery {
        from: crate::TableRef::new(table),
        r#where: Some(crate::filter::Filter::Eq {
            field: vec!["user_id".to_string()],
            value: FilterValue::QueryRef {
                alias: format!("@{dep_alias}[0].id"),
                path: None,
            },
        }),
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

#[test]
fn edge_provenance_pure_after_is_explicit() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("users".to_string(), read_entry("users"));
    let mut dependent = read_entry("orders");
    dependent.after = vec!["users".to_string()];
    queries.insert("orders".to_string(), dependent);

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have a provenance entry");
    assert_eq!(
        provenance.get("users"),
        Some(&crate::batch::EdgeKind::Explicit),
        "after-only edge must be tagged Explicit, got {:?}",
        provenance
    );
}

#[test]
fn edge_provenance_pure_query_ref_is_dataflow() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("users".to_string(), read_entry("users"));
    queries.insert(
        "orders".to_string(),
        read_entry_with_query_ref("orders", "users"),
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have a provenance entry");
    assert_eq!(
        provenance.get("users"),
        Some(&crate::batch::EdgeKind::DataFlow),
        "$query-only edge must be tagged DataFlow, got {:?}",
        provenance
    );
}

#[test]
fn edge_provenance_after_and_query_ref_same_alias_is_both() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("users".to_string(), read_entry("users"));
    let mut dependent = read_entry_with_query_ref("orders", "users");
    dependent.after = vec!["users".to_string()];
    queries.insert("orders".to_string(), dependent);

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have a provenance entry");
    assert_eq!(
        provenance.get("users"),
        Some(&crate::batch::EdgeKind::Both),
        "after + $query on the same alias must dedup to Both, got {:?}",
        provenance
    );

    // Dedup: `dependencies` still has exactly one entry for "users", not two.
    let deps = plan.dependencies.get("orders").expect("orders deps");
    assert_eq!(deps.len(), 1);
    assert!(deps.contains("users"));
}

#[test]
fn after_garbage_bracket_path_is_rejected() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("users".to_string(), read_entry("users"));
    let mut dependent = read_entry("orders");
    dependent.after = vec!["users[0].id".to_string()];
    queries.insert("orders".to_string(), dependent);

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits).expect_err("garbage path must be rejected");
    assert!(
        matches!(
            &err,
            BatchError::AfterPathIgnored { alias, raw }
            if alias == "orders" && raw == "users[0].id"
        ),
        "expected AfterPathIgnored, got {:?}",
        err
    );
}

#[test]
fn after_garbage_dot_path_is_rejected() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("users".to_string(), read_entry("users"));
    let mut dependent = read_entry("orders");
    dependent.after = vec!["users.id".to_string()];
    queries.insert("orders".to_string(), dependent);

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits).expect_err("garbage path must be rejected");
    assert!(
        matches!(
            &err,
            BatchError::AfterPathIgnored { alias, raw }
            if alias == "orders" && raw == "users.id"
        ),
        "expected AfterPathIgnored, got {:?}",
        err
    );
}

// -------------------------------------------------------------------------
// Self-reference + path-tail precedence (task #630 — Epic01/C gap #3)
// -------------------------------------------------------------------------
//
// `after` naming a path-tail on the SAME alias it belongs to (e.g.
// `self_ref` after-ing `"self_ref[0].id"`) is simultaneously a garbage
// path-tail AND a self-reference. `BatchPlanner::plan` validates the
// path-tail shape (`split_path_tail`) BEFORE it ever builds the dependency
// set that cycle-detection runs over (see `planner.rs`, the `after` loop
// precedes `detect_cycle`), so `AfterPathIgnored` must win over
// `CircularDependency` here. This pins that real, non-obvious precedence so
// a future planner refactor that reorders the checks is caught.
#[test]
fn after_self_reference_with_path_tail_yields_after_path_ignored_not_circular() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    let mut self_ref = read_entry("t");
    self_ref.after = vec!["self_ref[0].id".to_string()];
    queries.insert("self_ref".to_string(), self_ref);

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits)
        .expect_err("self-reference with path tail must be rejected");
    assert!(
        matches!(
            &err,
            BatchError::AfterPathIgnored { alias, raw }
            if alias == "self_ref" && raw == "self_ref[0].id"
        ),
        "AfterPathIgnored must win over CircularDependency for a self-\
         referencing after with a path tail, got {:?}",
        err
    );
}

// -------------------------------------------------------------------------
// Bug #642 regression: extract_deps_from_filter_value must recurse into
// $cond/$expr/$fn, not just Array/QueryRef (Epic03/B, #645 scope).
// -------------------------------------------------------------------------

#[test]
fn where_query_ref_nested_in_cond_then_is_extracted_as_dependency() {
    use crate::filter::{Cond, Filter};

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("check".to_string(), read_entry("check"));

    // `orders` WHERE clause: eq(status, $cond{ if: true, then: $query(check), else: false })
    // — the QueryRef lives inside the $cond's `then` branch, not at the
    // top-level FilterValue position.
    let q = ReadQuery {
        from: crate::TableRef::new("orders"),
        r#where: Some(Filter::Eq {
            field: vec!["status".to_string()],
            value: FilterValue::Cond {
                cond: Box::new(Cond::new(
                    Filter::Eq {
                        field: vec!["dummy".to_string()],
                        value: FilterValue::Bool(true),
                    },
                    FilterValue::query_ref_with_path("check", "[0].status"),
                    FilterValue::Bool(false),
                )),
            },
        }),
        select: crate::read::Select::all(),
        order_by: None,
        pagination: crate::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: crate::read::Temporal::default(),
        with_version: false,
        explain: false,
    };
    queries.insert(
        "orders".to_string(),
        QueryEntry {
            op: BatchOp::Read(q),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    // Before the #642 fix, `check` would NOT appear as a dependency of
    // `orders` (silently dropped by the `_ => {}` catch-all), so `orders`
    // would land in the SAME stage as `check` instead of a later one.
    let deps = plan.dependencies.get("orders").expect("orders deps");
    assert!(
        deps.contains("check"),
        "expected 'check' to be extracted as a dependency of 'orders' via \
         the nested $cond.then QueryRef, got deps = {:?}",
        deps
    );

    let check_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"check".to_string()))
        .expect("check must be in some stage");
    let orders_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"orders".to_string()))
        .expect("orders must be in some stage");
    assert!(
        orders_stage > check_stage,
        "orders (depends on check via nested $cond) must run in a later \
         stage than check; got check_stage={check_stage}, orders_stage={orders_stage}"
    );
}

#[test]
fn where_query_ref_nested_in_expr_args_is_extracted_as_dependency() {
    use crate::filter::{Filter, FilterExpr, FilterExprOp};

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("base".to_string(), read_entry("base"));

    let q = ReadQuery {
        from: crate::TableRef::new("derived"),
        r#where: Some(Filter::Eq {
            field: vec!["total".to_string()],
            value: FilterValue::Expr {
                expr: FilterExpr::new(
                    FilterExprOp::Add,
                    vec![
                        FilterValue::Int(1),
                        FilterValue::query_ref_with_path("base", "[0].amount"),
                    ],
                ),
            },
        }),
        select: crate::read::Select::all(),
        order_by: None,
        pagination: crate::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: crate::read::Temporal::default(),
        with_version: false,
        explain: false,
    };
    queries.insert(
        "derived".to_string(),
        QueryEntry {
            op: BatchOp::Read(q),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let deps = plan.dependencies.get("derived").expect("derived deps");
    assert!(
        deps.contains("base"),
        "expected 'base' to be extracted as a dependency of 'derived' via \
         the nested $expr.args QueryRef, got deps = {:?}",
        deps
    );
}

#[test]
fn where_query_ref_nested_in_fn_call_args_is_extracted_as_dependency() {
    use crate::filter::{Filter, FnCall};

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("src".to_string(), read_entry("src"));

    let q = ReadQuery {
        from: crate::TableRef::new("derived2"),
        r#where: Some(Filter::Eq {
            field: vec!["name".to_string()],
            value: FilterValue::FnCall {
                call: FnCall::complex(
                    "COALESCE",
                    vec![
                        FilterValue::query_ref_with_path("src", "[0].name"),
                        FilterValue::String("default".to_string()),
                    ],
                ),
            },
        }),
        select: crate::read::Select::all(),
        order_by: None,
        pagination: crate::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: crate::read::Temporal::default(),
        with_version: false,
        explain: false,
    };
    queries.insert(
        "derived2".to_string(),
        QueryEntry {
            op: BatchOp::Read(q),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let deps = plan.dependencies.get("derived2").expect("derived2 deps");
    assert!(
        deps.contains("src"),
        "expected 'src' to be extracted as a dependency of 'derived2' via \
         the nested $fn.args QueryRef, got deps = {:?}",
        deps
    );
}

// -------------------------------------------------------------------------
// `when: Option<Filter>` DAG participation (Epic03/B, #645)
// -------------------------------------------------------------------------

#[test]
fn when_query_ref_participates_in_dag_as_dataflow() {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("check".to_string(), read_entry("check"));

    let mut gated = read_entry("orders");
    // ValueCompare (not a field-based variant) — #651: field-based
    // comparisons are now rejected inside `when` by a dedicated plan-time
    // check (see `when_field_based_comparison_is_rejected_at_plan_time`),
    // so this DAG-participation test uses the value-vs-value shape that
    // remains valid inside `when`.
    gated.when = Some(crate::filter::Filter::ValueCompare {
        left: FilterValue::Bool(true),
        cmp: crate::filter::ValueCompareOp::Eq,
        right: FilterValue::query_ref_with_path("check", "[0].ok"),
    });
    queries.insert("orders".to_string(), gated);

    let limits = BatchLimits::default();
    let plan = BatchPlanner::plan(&queries, &limits).expect("plan should succeed");

    let deps = plan.dependencies.get("orders").expect("orders deps");
    assert!(
        deps.contains("check"),
        "expected 'check' to be extracted from `when` as a dependency of \
         'orders', got deps = {:?}",
        deps
    );

    let provenance = plan
        .edge_provenance
        .get("orders")
        .expect("orders must have a provenance entry");
    assert_eq!(
        provenance.get("check"),
        Some(&crate::batch::EdgeKind::DataFlow),
        "`when`-only $query ref must be tagged DataFlow, got {:?}",
        provenance
    );

    let check_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"check".to_string()))
        .expect("check must be in some stage");
    let orders_stage = plan
        .stages
        .iter()
        .position(|s| s.contains(&"orders".to_string()))
        .expect("orders must be in some stage");
    assert!(
        orders_stage > check_stage,
        "orders (gated by `when` on check) must run in a later stage than \
         check; got check_stage={check_stage}, orders_stage={orders_stage}"
    );
}

// -------------------------------------------------------------------------
// #663 — close the remaining gaps in the #651/#641 field-based-comparison
// guard.
//
// Gap 1: `contains_field_based_comparison`'s variant list was missing
// `Like`/`ILike`/`Regex`/`In`/`NotIn`/`Contains`/`ContainsAny`/`ContainsAll`/
// `Between`/`Fts`/`VectorSimilarity`/`Computed` — every one of these carries
// a `field: FieldPath` and resolves against a real record, exactly like the
// 7 variants #651 already covered.
//
// Gap 2: the guard never looked INSIDE the `FilterValue` operands every
// comparison variant carries, so a `FilterValue::Cond` nested inside e.g.
// `ValueCompare.left` or `In.values` sailed through undetected.
//
// Gap 3: write-value `$cond` markers (#641's class) had NO plan-time guard
// at all — `extract_deps_from_value`'s dependency-extraction pass only
// extracted `$query` edges, never validating a `$cond`'s condition.
// -------------------------------------------------------------------------

fn gated_entry_when(when_filter: Filter) -> QueryEntry {
    let mut entry = read_entry("probe");
    entry.when = Some(when_filter);
    entry
}

fn assert_when_rejected(when_filter: Filter, label: &str) {
    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("probe".to_string(), gated_entry_when(when_filter));

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits).expect_err(&format!(
        "{label}: field-based comparison inside `when` must be rejected"
    ));
    assert!(
        matches!(&err, BatchError::InvalidWhenFilter { alias, .. } if alias == "probe"),
        "{label}: expected InvalidWhenFilter, got {:?}",
        err
    );
}

// -- Gap 1: newly-covered field-based variants are rejected inside `when` --

#[test]
fn when_gap1_newly_covered_field_based_variants_are_rejected() {
    let field = vec!["status".to_string()];

    let cases: Vec<(&str, Filter)> = vec![
        (
            "like",
            Filter::Like {
                field: field.clone(),
                pattern: "%x%".to_string(),
            },
        ),
        (
            "ilike",
            Filter::ILike {
                field: field.clone(),
                pattern: "%x%".to_string(),
            },
        ),
        (
            "regex",
            Filter::Regex {
                field: field.clone(),
                pattern: "^x$".to_string(),
            },
        ),
        (
            "in",
            Filter::In {
                field: field.clone(),
                values: vec![FilterValue::Int(1)],
            },
        ),
        (
            "not_in",
            Filter::NotIn {
                field: field.clone(),
                values: vec![FilterValue::Int(1)],
            },
        ),
        (
            "contains",
            Filter::Contains {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "contains_any",
            Filter::ContainsAny {
                field: field.clone(),
                values: vec![FilterValue::Int(1)],
            },
        ),
        (
            "contains_all",
            Filter::ContainsAll {
                field: field.clone(),
                values: vec![FilterValue::Int(1)],
            },
        ),
        (
            "between",
            Filter::Between {
                field: field.clone(),
                from: FilterValue::Int(1),
                to: FilterValue::Int(10),
            },
        ),
        (
            "fts",
            Filter::Fts {
                field: field.clone(),
                query: "hello".to_string(),
                mode: "and".to_string(),
            },
        ),
        (
            "vector_similarity",
            Filter::VectorSimilarity {
                field: field.clone(),
                query: vec![0.1, 0.2],
                k: 5,
                ef_search: None,
                oversample: None,
            },
        ),
        (
            "computed",
            Filter::Computed {
                expr_op: "lower".to_string(),
                field: field.clone(),
                expr_args: None,
                cmp: "eq".to_string(),
                value: FilterValue::String("x".to_string()),
            },
        ),
    ];

    for (label, filter) in cases {
        assert_when_rejected(filter, label);
    }
}

// -- Gap 1 regression: legitimate presence-guard exclusions still pass --

#[test]
fn when_gap1_presence_guards_still_accepted() {
    let field = vec!["status".to_string()];

    let cases: Vec<(&str, Filter)> = vec![
        (
            "is_null",
            Filter::IsNull {
                field: field.clone(),
            },
        ),
        (
            "is_not_null",
            Filter::IsNotNull {
                field: field.clone(),
            },
        ),
        (
            "exists",
            Filter::Exists {
                field: field.clone(),
            },
        ),
        (
            "not_exists",
            Filter::NotExists {
                field: field.clone(),
            },
        ),
    ];

    for (label, filter) in cases {
        let mut queries: TMap<String, QueryEntry> = new_map();
        queries.insert("probe".to_string(), gated_entry_when(filter));

        let limits = BatchLimits::default();
        let result = BatchPlanner::plan(&queries, &limits);
        assert!(
            result.is_ok(),
            "{label}: presence-guard variant must still be accepted inside \
             `when`, got {:?}",
            result
        );
    }
}

// -- Gap 2: nested $cond inside a FilterValue operand is detected --

#[test]
fn when_gap2_nested_cond_inside_value_compare_operand_is_rejected() {
    // `Filter::ValueCompare { left: $cond{ if: Like{..}, .. }, .. }` — the
    // structural top-level Filter is ValueCompare (never itself flagged),
    // but its `left` operand embeds a field-based $cond condition.
    let nested_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Like {
                field: vec!["status".to_string()],
                pattern: "%x%".to_string(),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };

    let when_filter = Filter::ValueCompare {
        left: nested_cond,
        cmp: crate::filter::ValueCompareOp::Eq,
        right: FilterValue::Int(1),
    };

    assert_when_rejected(when_filter, "nested $cond inside ValueCompare.left");
}

#[test]
fn when_gap2_nested_cond_inside_in_values_operand_is_rejected() {
    // Prove the check isn't special-cased to ValueCompare alone: the same
    // nested $cond, this time inside `Filter::In { values: [...] }`.
    let nested_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Like {
                field: vec!["status".to_string()],
                pattern: "%x%".to_string(),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };

    let when_filter = Filter::In {
        field: vec!["tag".to_string()],
        values: vec![nested_cond],
    };

    assert_when_rejected(when_filter, "nested $cond inside In.values");
}

// -- Gap 2 regression: a legitimate nested $cond (record-free condition) --

#[test]
fn when_gap2_nested_cond_with_value_compare_condition_is_accepted() {
    // The nested $cond's OWN condition is ValueCompare (record-free) — this
    // must NOT be rejected; only a field-based nested condition should trip
    // the guard.
    let legit_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::ValueCompare {
                left: FilterValue::Int(1),
                cmp: crate::filter::ValueCompareOp::Eq,
                right: FilterValue::Int(1),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };

    let when_filter = Filter::ValueCompare {
        left: legit_cond,
        cmp: crate::filter::ValueCompareOp::Eq,
        right: FilterValue::Int(1),
    };

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert("probe".to_string(), gated_entry_when(when_filter));

    let limits = BatchLimits::default();
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        result.is_ok(),
        "a nested $cond whose OWN condition is record-free (ValueCompare) \
         must be accepted, got {:?}",
        result
    );
}

// -- Gap 3: write-value $cond marker with a field-based condition ----------

/// Build an `InsertOp` targeting `table` whose single row's `field` value is
/// the msgpack round-trip of `fv` (mirrors exactly how the wire encodes a
/// `FilterValue::Cond`/`Expr`/`FnCall`/`QueryRef` marker map — the same
/// round-trip `extract_deps_from_value` decodes at plan time and
/// `param_subst.rs` decodes at execution time).
fn insert_op_with_marker_field(table: &str, field: &str, fv: &FilterValue) -> InsertOp {
    let bytes = rmp_serde::to_vec_named(fv).expect("serialize FilterValue marker");
    let marker_qv: QueryValue =
        rmp_serde::from_slice(&bytes).expect("deserialize marker as QueryValue");

    let mut row = shamir_collections::new_map();
    row.insert(field.to_string(), marker_qv);

    InsertOp {
        insert_into: crate::TableRef::new(table),
        values: vec![QueryValue::Map(row)],
        records_idmsgpack: Vec::new(),
        select: None,
    }
}

fn insert_entry(op: InsertOp) -> QueryEntry {
    QueryEntry {
        op: BatchOp::Insert(op),
        return_result: true,
        after: Vec::new(),
        when: None,
    }
}

#[test]
fn insert_write_value_cond_with_field_based_condition_is_rejected_at_plan_time() {
    let field_based_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Like {
                field: vec!["status".to_string()],
                pattern: "%x%".to_string(),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "make_row".to_string(),
        insert_entry(insert_op_with_marker_field(
            "orders",
            "user_id",
            &field_based_cond,
        )),
    );

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits).expect_err(
        "an Insert write value whose $cond condition is field-based must be \
         rejected at plan time, not silently folded",
    );
    match err {
        BatchError::InvalidCondCondition { alias, message } => {
            assert_eq!(alias, "make_row");
            assert!(
                message.contains("ValueCompare"),
                "error message must name the fix (Filter::ValueCompare): {message}"
            );
        }
        other => panic!("expected BatchError::InvalidCondCondition, got: {other:?}"),
    }
}

#[test]
fn update_set_value_cond_with_field_based_condition_is_rejected_at_plan_time() {
    use crate::write::UpdateOp;

    let field_based_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Like {
                field: vec!["status".to_string()],
                pattern: "%x%".to_string(),
            },
            FilterValue::Int(1),
            FilterValue::Int(0),
        )),
    };
    let bytes = rmp_serde::to_vec_named(&field_based_cond).expect("serialize marker");
    let marker_qv: QueryValue = rmp_serde::from_slice(&bytes).expect("deserialize marker");

    let mut row = shamir_collections::new_map();
    row.insert("user_id".to_string(), marker_qv);

    let update_op = UpdateOp {
        update: crate::TableRef::new("orders"),
        where_clause: None,
        set: QueryValue::Map(row),
        select: None,
        expected_version: None,
    };

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "update_row".to_string(),
        QueryEntry {
            op: BatchOp::Update(update_op),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );

    let limits = BatchLimits::default();
    let err = BatchPlanner::plan(&queries, &limits).expect_err(
        "an Update `set` value whose $cond condition is field-based must be \
         rejected at plan time",
    );
    assert!(
        matches!(&err, BatchError::InvalidCondCondition { alias, .. } if alias == "update_row"),
        "expected InvalidCondCondition, got {:?}",
        err
    );
}

// -- Gap 3 regression: a legitimate write-value $cond (record-free) passes --

#[test]
fn insert_write_value_cond_with_value_compare_condition_passes_plan_time_validation() {
    let legit_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::ValueCompare {
                left: FilterValue::Int(100),
                cmp: crate::filter::ValueCompareOp::Gte,
                right: FilterValue::Int(50),
            },
            FilterValue::String("high".to_string()),
            FilterValue::String("low".to_string()),
        )),
    };

    let mut queries: TMap<String, QueryEntry> = new_map();
    queries.insert(
        "make_row".to_string(),
        insert_entry(insert_op_with_marker_field("orders", "band", &legit_cond)),
    );

    let limits = BatchLimits::default();
    let result = BatchPlanner::plan(&queries, &limits);
    assert!(
        result.is_ok(),
        "an Insert write value whose $cond condition is record-free \
         (ValueCompare) must pass plan-time validation, got {:?}",
        result
    );
}

// -- No regressions: existing #651 field-based variants still rejected -----

#[test]
fn when_gap1_original_651_variants_still_rejected() {
    let field = vec!["balance".to_string()];

    let cases: Vec<(&str, Filter)> = vec![
        (
            "eq",
            Filter::Eq {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "ne",
            Filter::Ne {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "gt",
            Filter::Gt {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "gte",
            Filter::Gte {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "lt",
            Filter::Lt {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "lte",
            Filter::Lte {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
        (
            "field_eq",
            Filter::FieldEq {
                field: field.clone(),
                value: FilterValue::Int(1),
            },
        ),
    ];

    for (label, filter) in cases {
        assert_when_rejected(filter, label);
    }
}
