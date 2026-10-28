use shamir_collections::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use super::planner_tests::read_entry;
use crate::batch::planner::BatchPlanner;
use crate::batch::{BatchError, BatchLimits, BatchOp, QueryEntry};
use crate::filter::{Cond, Filter, FilterValue};
use crate::write::InsertOp;

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
                limit: None,
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
