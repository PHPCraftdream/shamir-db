//! Tests for `$cond`/`$expr` evaluation in `resolve_filter_query` (#635).

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Cond, Filter, FilterExpr, FilterExprOp, FilterValue};
use shamir_types::core::interner::Interner;
use shamir_types::types::value::QueryValue;

use super::helpers::{empty_refs, make_alice_record};

/// `$cond` — true branch: `status == "active"` selects `then`.
#[test]
fn test_cond_true_branch() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            FilterValue::String("yes".to_string()),
            FilterValue::String("no".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("yes".to_string()))
    );
}

/// `$cond` — false branch: condition fails, selects `or_else`.
#[test]
fn test_cond_false_branch() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("inactive".to_string()),
            },
            FilterValue::String("yes".to_string()),
            FilterValue::String("no".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("no".to_string()))
    );
}

/// Nested `$cond` (2 levels deep): outer condition true, inner condition
/// picks between two further branches.
#[test]
fn test_cond_nested_two_levels() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age: 30, status: "active"
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // if status == "active":
    //   if age > 18: "adult-active"
    //   else: "minor-active"
    // else: "inactive"
    let inner_cond = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            FilterValue::String("adult-active".to_string()),
            FilterValue::String("minor-active".to_string()),
        )),
    };

    let outer_fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            inner_cond,
            FilterValue::String("inactive".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&outer_fv, &record, &ctx),
        Some(QueryValue::Str("adult-active".to_string()))
    );
}

/// `$cond`'s condition references an undeclared `$query` alias — silent-miss
/// semantics: `FilterNode::matches` treats the missing comparison as `false`,
/// so `or_else` is chosen instead of erroring.
#[test]
fn test_cond_condition_silent_miss_on_missing_query_ref() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::QueryRef {
                    alias: "undeclared".to_string(),
                    path: Some("[0].status".to_string()),
                },
            },
            FilterValue::String("yes".to_string()),
            FilterValue::String("no".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("no".to_string()))
    );
}

/// `$expr` arithmetic: `add(10, 20) == 30`, preserving `Int`.
#[test]
fn test_expr_add_int() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::add(vec![FilterValue::Int(10), FilterValue::Int(20)]),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Int(30))
    );
}

/// `$expr` string concat over field refs and literals.
#[test]
fn test_expr_concat_field_ref() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // name: "Alice"
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::concat(vec![
            FilterValue::field_ref("name"),
            FilterValue::String("!".to_string()),
        ]),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("Alice!".to_string()))
    );
}

/// `$expr` comparison op (`gt`) returns a `Bool`.
#[test]
fn test_expr_comparison_gt() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::new(
            FilterExprOp::Gt,
            vec![FilterValue::Int(30), FilterValue::Int(18)],
        ),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Bool(true))
    );
}

/// `$expr` division by zero collapses to `None` (absent), not a panic.
#[test]
fn test_expr_div_by_zero_is_none() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::new(
            FilterExprOp::Div,
            vec![FilterValue::Int(10), FilterValue::Int(0)],
        ),
    };

    assert_eq!(resolve_filter_query(&fv, &record, &ctx), None);
}

/// `$cond` whose `then` branch is a nested `$expr` — cross-feature recursion.
#[test]
fn test_cond_then_branch_is_expr() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age: 30
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            FilterValue::Expr {
                expr: FilterExpr::add(vec![FilterValue::field_ref("age"), FilterValue::Int(1)]),
            },
            FilterValue::Int(0),
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Int(31))
    );
}
