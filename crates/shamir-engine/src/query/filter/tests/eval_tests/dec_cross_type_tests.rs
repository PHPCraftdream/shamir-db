//! Dec/Big cross-type comparison in the filter layer.
//!
//! These tests cover the three sub-bugs closed by the "Dec blind spot" fix:
//!
//! 1. **scalar_ref_cmp_qv** — a record FIELD that is `Int`/`F64` compared
//!    against a FILTER OPERAND that is `Dec`/`Big` (typically a `$fn`
//!    result). Before the fix, `scalar_ref_cmp_qv(Int, &Dec)` fell to
//!    `_ => None`, making every `Eq`/`Gt`/`Gte`/`Lt`/`Lte` silently `false`
//!    (and `Ne` silently `true`) for every row.
//! 2. **compare_values** — the `Value<K>` twin (feeds `ValueCompare` and the
//!    Min/Max container fallback). Before the fix, `Dec`/`Big` operands were
//!    silently `None`.
//! 3. **as_f64 in $expr** — a Dec operand in `$expr` arithmetic collapsed to
//!    `None`, silently making the enclosing comparison false.

use crate::query::filter::eval::{compare_values, compile_filter, resolve_filter_query};
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterExpr, FilterExprOp, FilterValue, FnCall};
use shamir_types::core::interner::Interner;
use shamir_types::types::value::QueryValue;

use super::helpers::{empty_refs, make_alice_record};
use std::cmp::Ordering;

// ============================================================================
// Part 1a — scalar_ref_cmp_qv: WHERE field op $fn-returning-Dec
// ============================================================================

/// `abs(-100)` → `Dec(100)`. `age = 30 < 100` → `Lt` should be `true`.
#[test]
fn filter_lt_int_field_vs_dec_fn_result() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age = 30
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Lt {
        field: vec!["age".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("math/abs", vec![FilterValue::Int(-100)]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "age(30) < abs(-100)=Dec(100) must match"
    );
}

/// `abs(-10)` → `Dec(10)`. `age = 30 > 10` → `Gt` should be `true`.
#[test]
fn filter_gt_int_field_vs_dec_fn_result() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age = 30
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Gt {
        field: vec!["age".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("math/abs", vec![FilterValue::Int(-10)]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "age(30) > abs(-10)=Dec(10) must match"
    );
}

/// `abs(-100)` → `Dec(100)`. `age = 30`. `Ne` against a Dec operand should
/// be `true` (30 ≠ 100). Before the fix, `Ne` was *always* `true` regardless
/// of values (because `None` makes only `Ne` true). This test confirms `Ne`
/// is correct by also checking `Eq` is `false`.
#[test]
fn filter_eq_ne_int_field_vs_dec_fn_result() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age = 30
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // Eq: 30 == Dec(100) → false.
    let eq_filter = Filter::Eq {
        field: vec!["age".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("math/abs", vec![FilterValue::Int(-100)]),
        },
    };
    let node = compile_filter(&eq_filter, &interner);
    assert!(!node.matches(&record, &ctx), "age(30) == Dec(100) is false");

    // Ne: 30 != Dec(100) → true.
    let ne_filter = Filter::Ne {
        field: vec!["age".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("math/abs", vec![FilterValue::Int(-100)]),
        },
    };
    let node = compile_filter(&ne_filter, &interner);
    assert!(node.matches(&record, &ctx), "age(30) != Dec(100) is true");
}

/// Exact equality: `abs(-30)` → `Dec(30)`. `age = 30 == Dec(30)` → `Eq`
/// should be `true`. Before the fix this was silently `false`.
#[test]
fn filter_eq_int_field_vs_equal_dec_fn_result() {
    let interner = Interner::new();
    let record = make_alice_record(&interner); // age = 30
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let filter = Filter::Eq {
        field: vec!["age".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("math/abs", vec![FilterValue::Int(-30)]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(
        node.matches(&record, &ctx),
        "age(30) == abs(-30)=Dec(30) must match"
    );
}

// ============================================================================
// Part 1b — compare_values: Dec/Big direct Value<K> comparison
// ============================================================================

#[test]
fn compare_values_dec_dec_exact() {
    let a = QueryValue::Dec("3.5".parse().unwrap());
    let b = QueryValue::Dec("3.5".parse().unwrap());
    assert_eq!(compare_values(&a, &b), Some(Ordering::Equal));

    let a = QueryValue::Dec("3.5".parse().unwrap());
    let b = QueryValue::Dec("10.0".parse().unwrap());
    assert_eq!(compare_values(&a, &b), Some(Ordering::Less));

    let a = QueryValue::Dec("100.0".parse().unwrap());
    let b = QueryValue::Dec("10.0".parse().unwrap());
    assert_eq!(compare_values(&a, &b), Some(Ordering::Greater));
}

#[test]
fn compare_values_int_dec_exact() {
    // Int↔Dec is exact (Decimal represents every i64 exactly).
    assert_eq!(
        compare_values(
            &QueryValue::Int(10),
            &QueryValue::Dec("10.0".parse().unwrap())
        ),
        Some(Ordering::Equal)
    );
    assert_eq!(
        compare_values(
            &QueryValue::Int(5),
            &QueryValue::Dec("10.0".parse().unwrap())
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(
            &QueryValue::Dec("10.0".parse().unwrap()),
            &QueryValue::Int(5)
        ),
        Some(Ordering::Greater)
    );
}

#[test]
fn compare_values_f64_dec_fallback() {
    // F64↔Dec uses the f64 fallback (not exact, but correct ordering for
    // finite values).
    assert_eq!(
        compare_values(
            &QueryValue::F64(5.0),
            &QueryValue::Dec("10.0".parse().unwrap())
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(
            &QueryValue::Dec("10.0".parse().unwrap()),
            &QueryValue::F64(5.0)
        ),
        Some(Ordering::Greater)
    );
}

#[test]
fn compare_values_big_fallback() {
    // Big uses the f64 fallback — stops being a silent None.
    use num_bigint::BigInt;
    assert_eq!(
        compare_values(
            &QueryValue::Big(BigInt::from(100)),
            &QueryValue::Big(BigInt::from(200))
        ),
        Some(Ordering::Less)
    );
    assert_eq!(
        compare_values(&QueryValue::Int(200), &QueryValue::Big(BigInt::from(100))),
        Some(Ordering::Greater)
    );
}

// ============================================================================
// Part 1c — $expr as_f64: arithmetic over a Dec operand
// ============================================================================

/// `$expr add(abs(-50), 1)` → `abs(-50)` = `Dec(50)`, then `add(Dec(50),
/// Int(1))`. Before the fix, `as_f64(Dec(50))` returned `None` and the whole
/// expr collapsed to `None`.
#[test]
fn expr_add_over_dec_operand() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::add(vec![
            FilterValue::FnCall {
                call: FnCall::complex("math/abs", vec![FilterValue::Int(-50)]),
            },
            FilterValue::Int(1),
        ]),
    };
    let result = resolve_filter_query(&fv, &record, &ctx);
    // abs(-50) = Dec(50), add(Dec(50), 1) = F64(51.0) via the as_f64 fallback.
    assert_eq!(result, Some(QueryValue::F64(51.0)));
}

/// `$expr gt(abs(-10), 5)` — `abs(-10)` = `Dec(10)`, `gt(Dec(10), Int(5))` →
/// `Bool(true)`. Exercises compare_values(Int, Dec) inside $expr.
#[test]
fn expr_gt_over_dec_operand() {
    let interner = Interner::new();
    let record = make_alice_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    let fv = FilterValue::Expr {
        expr: FilterExpr::new(
            FilterExprOp::Gt,
            vec![
                FilterValue::FnCall {
                    call: FnCall::complex("math/abs", vec![FilterValue::Int(-10)]),
                },
                FilterValue::Int(5),
            ],
        ),
    };
    let result = resolve_filter_query(&fv, &record, &ctx);
    assert_eq!(result, Some(QueryValue::Bool(true)));
}
