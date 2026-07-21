//! Gap-closure tests for `$cond`/`$expr` evaluation (Epic02/C, task #637).
//!
//! Phases A (#635) and B (#636) already cover: basic true/false branches,
//! 2-level nesting, silent-miss on missing `$query` alias in the condition,
//! `$expr` arithmetic/comparison/div-by-zero, and `$cond` with an `$expr`
//! `then` branch (see `cond_expr_tests.rs`). This file adds the REAL gaps
//! identified against `docs/dev-artifacts/roadmap/oql/02-cond-value-evaluation.md`
//! Phase C that were not yet exercised:
//!
//! 1. 3-level-deep nested `$cond`, evaluated by the engine (not just
//!    structural equivalence of `switch_case`), switching branches based on
//!    record data at every level.
//! 2. `$fn` call whose argument is itself a `$cond`.
//! 3. `$cond` whose `then`/`or_else` branches are themselves `$query`-refs
//!    and `$param`-refs (not just literals).
//! 4. `$expr` with an unresolvable (`$query`-ref to an undeclared alias)
//!    argument collapsing to `None`, not a panic — the `$expr` analogue of
//!    the already-covered `$cond`-condition silent-miss test.

use crate::query::filter::eval::resolve_filter_query;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Cond, Filter, FilterExpr, FilterValue, FnCall};
use crate::query::read::{QueryRecord, QueryResult};
use shamir_types::core::interner::Interner;
use shamir_types::mpack;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::value::QueryValue;

use super::helpers::{empty_refs, make_alice_record};

/// 3-level nested `$cond`, evaluated by the engine end-to-end.
///
/// Record: age=30, status="active" (`make_alice_record`).
///
/// ```text
/// if status == "active":
///   if age > 18:
///     if age > 25: "senior-adult-active"
///     else:        "junior-adult-active"
///   else: "minor-active"
/// else: "inactive"
/// ```
///
/// Alice (age 30, active) must resolve through all three levels to
/// "senior-adult-active" — the innermost `$cond` picks its `then` branch,
/// not a hardcoded default.
#[test]
fn test_cond_nested_three_levels_engine_evaluated() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let innermost = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
            FilterValue::String("senior-adult-active".to_string()),
            FilterValue::String("junior-adult-active".to_string()),
        )),
    };

    let middle = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            innermost,
            FilterValue::String("minor-active".to_string()),
        )),
    };

    let outer = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            middle,
            FilterValue::String("inactive".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&outer, &record, &ctx),
        Some(QueryValue::Str("senior-adult-active".to_string()))
    );
}

/// Same 3-level tree, but the record takes the OTHER branch at every level
/// (age below the innermost threshold) — confirms the engine actually
/// switches per-record rather than always landing on one hardcoded path.
#[test]
fn test_cond_nested_three_levels_switches_on_record_data() {
    let interner = Interner::new();

    // Build a record with age=20 (adult, but not "senior"), status="active".
    let mut map = new_map();
    let k_age = interner.touch_ind("age").unwrap().into_key();
    let k_status = interner.touch_ind("status").unwrap().into_key();
    map.insert(k_age, shamir_types::types::value::InnerValue::Int(20));
    map.insert(
        k_status,
        shamir_types::types::value::InnerValue::Str("active".to_string()),
    );
    let record = shamir_types::types::value::InnerValue::Map(map);

    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let innermost = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(25),
            },
            FilterValue::String("senior-adult-active".to_string()),
            FilterValue::String("junior-adult-active".to_string()),
        )),
    };

    let middle = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Gt {
                field: vec!["age".to_string()],
                value: FilterValue::Int(18),
            },
            innermost,
            FilterValue::String("minor-active".to_string()),
        )),
    };

    let outer = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            middle,
            FilterValue::String("inactive".to_string()),
        )),
    };

    assert_eq!(
        resolve_filter_query(&outer, &record, &ctx),
        Some(QueryValue::Str("junior-adult-active".to_string()))
    );
}

/// `$fn` call where one argument is itself a `$cond` — confirms
/// `resolve_filter_query`'s `FnCall` arm (which recurses via
/// `resolve_filter_query` over each arg) actually evaluates a nested
/// `$cond` arg rather than passing it through unresolved.
#[test]
fn test_fn_call_with_cond_argument() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age: 30, status: "active"
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // strings/upper($cond(status == "active" ? "yes" : "no"))
    let cond_arg = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            FilterValue::String("yes".to_string()),
            FilterValue::String("no".to_string()),
        )),
    };

    let fv = FilterValue::FnCall {
        call: FnCall::complex("strings/upper", vec![cond_arg]),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("YES".to_string()))
    );
}

/// `$fn` call where the `$cond` argument takes the `or_else` branch —
/// confirms both branches of a `$cond`-as-`$fn`-arg are reachable, not
/// just the `then` branch exercised above.
#[test]
fn test_fn_call_with_cond_argument_or_else_branch() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // status: "active"
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let cond_arg = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("inactive".to_string()),
            },
            FilterValue::String("yes".to_string()),
            FilterValue::String("no".to_string()),
        )),
    };

    let fv = FilterValue::FnCall {
        call: FnCall::complex("strings/upper", vec![cond_arg]),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("NO".to_string()))
    );
}

/// `$cond`'s `then` branch is itself a `$query`-ref (not a literal) —
/// confirms the branch is resolved recursively through `resolve_filter_query`
/// against `ctx.resolved_refs`, exactly like a top-level `$query` value would be.
#[test]
fn test_cond_then_branch_is_query_ref() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // status: "active"

    let mut refs: TMap<String, QueryResult> = new_map();
    refs.insert(
        "users".to_string(),
        QueryResult {
            records: vec![QueryRecord::Direct(mpack!({"id": 42, "name": "Alice"}))],
            stats: None,
            pagination: None,
            value: None,
            explain: None,
            skipped: false,
            versions: None,
        },
    );
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("active".to_string()),
            },
            FilterValue::QueryRef {
                alias: "users".to_string(),
                path: Some("[0].id".to_string()),
            },
            FilterValue::Int(-1),
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Int(42))
    );
}

/// `$cond`'s `or_else` branch is itself a `$param`-ref — confirms the
/// branch resolves against `ctx.params` (the sub-batch parameter scope),
/// not just literals/`$query`.
#[test]
fn test_cond_or_else_branch_is_param_ref() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // status: "active"
    let refs = empty_refs();

    let mut params: TMap<String, QueryValue> = new_map();
    params.insert(
        "fallback".to_string(),
        QueryValue::Str("from-param".to_string()),
    );
    let ctx = FilterContext::new(&interner, &refs).with_params(&params);

    // Condition is false (status != "inactive") => or_else (the $param) is picked.
    let fv = FilterValue::Cond {
        cond: Box::new(Cond::new(
            Filter::Eq {
                field: vec!["status".to_string()],
                value: FilterValue::String("inactive".to_string()),
            },
            FilterValue::String("literal-then".to_string()),
            FilterValue::Param {
                name: "fallback".to_string(),
            },
        )),
    };

    assert_eq!(
        resolve_filter_query(&fv, &record, &ctx),
        Some(QueryValue::Str("from-param".to_string()))
    );
}

/// `$expr` with an argument that is an unresolvable `$query`-ref (alias not
/// present in `ctx.resolved_refs`) collapses to `None`, not a panic — the
/// `$expr` analogue of `test_cond_condition_silent_miss_on_missing_query_ref`
/// in `cond_expr_tests.rs` (which covers the `$cond`-condition case, not
/// `$expr`-argument).
#[test]
fn test_expr_arg_unresolvable_query_ref_is_none() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::add(vec![
            FilterValue::Int(10),
            FilterValue::QueryRef {
                alias: "undeclared".to_string(),
                path: Some("[0].amount".to_string()),
            },
        ]),
    };

    assert_eq!(resolve_filter_query(&fv, &record, &ctx), None);
}
