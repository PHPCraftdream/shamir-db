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
    // Big/Big is exact `BigInt::cmp`; Int/Big is exact `BigInt` conversion
    // (CR-C5, #780) — both stopped being a silent `None` (FG-6) and are now
    // ALSO exact, not an f64 approximation.
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
// CR-C5 (#780) — exact Big comparisons, eliminate f64 rounding.
// ============================================================================

/// THE core regression this task exists to fix: `i64::MAX` vs
/// `i64::MAX + 1` (promoted to `Big`, matching this codebase's own
/// promotion rule — `BigInt::from(i64::MAX) + 1`) must compare as `Less`,
/// NOT `Equal`. Before the fix, both operands rounded to the SAME `f64`
/// (`lossy_f64`), silently collapsing two distinct large integers.
#[test]
fn compare_values_int_max_vs_big_max_plus_one_is_exact() {
    use num_bigint::BigInt;

    let int_max = QueryValue::Int(i64::MAX);
    let bigger = QueryValue::Big(BigInt::from(i64::MAX) + 1);

    assert_eq!(compare_values(&int_max, &bigger), Some(Ordering::Less));
    assert_eq!(compare_values(&bigger, &int_max), Some(Ordering::Greater));
    assert_ne!(
        compare_values(&int_max, &bigger),
        Some(Ordering::Equal),
        "i64::MAX and i64::MAX+1 must not collapse to equal"
    );
}

/// `u64::MAX` (promoted to `Big`) vs `u64::MAX - 1` (also promoted to `Big`
/// — both exceed `i64::MAX`, so both promote) must not collapse to equal.
/// `u64::MAX` as `f64` rounds to the same value as several of its
/// neighbours, so this is exactly the precision class the fix targets.
#[test]
fn compare_values_u64_max_vs_u64_max_minus_one_big_big_is_exact() {
    use num_bigint::BigInt;

    let max = QueryValue::Big(BigInt::from(u64::MAX));
    let max_minus_one = QueryValue::Big(BigInt::from(u64::MAX) - 1);

    assert_eq!(compare_values(&max_minus_one, &max), Some(Ordering::Less));
    assert_eq!(
        compare_values(&max, &max_minus_one),
        Some(Ordering::Greater)
    );
}

/// `Big` vs `Dec` boundary case: an exact-integer-valued `Big` vs a `Dec`
/// with a fractional part numerically close enough that `f64` could not
/// distinguish them, proving the cross-multiplication approach
/// (`cmp_big_dec`) orders them correctly at `f64`-defeating precision.
///
/// `big = i64::MAX + 1` (~9.223372036854776e18) vs
/// `dec = i64::MAX.5` (half a unit below `big`) — both operands round to
/// the SAME `f64` at this magnitude (`f64` only has ~15-17 significant
/// decimal digits, `i64::MAX` already has 19), so a naive `f64`-based
/// comparison could not tell them apart; the exact cross-multiplication
/// path must.
#[test]
fn compare_values_big_vs_dec_close_boundary_is_exact() {
    use num_bigint::BigInt;
    use rust_decimal::prelude::ToPrimitive;
    use rust_decimal::Decimal;

    let big_val: BigInt = BigInt::from(i64::MAX) + 1; // 9223372036854775808
    let big = QueryValue::Big(big_val.clone());
    // dec = 9223372036854775807.5 -- half a unit below `big`, a fractional
    // Dec value that (cast through f64) would round to the SAME f64 as
    // `big` (both are within 1 ULP at this magnitude).
    let dec_str = format!("{}.5", i64::MAX);
    let dec: Decimal = dec_str.parse().unwrap();
    let dec_qv = QueryValue::Dec(dec);

    // Sanity: confirm this boundary really would defeat a naive f64
    // comparison — both operands' f64 approximations collapse to equal —
    // so the correct ordering below can only come from the exact
    // cross-multiplication path, not a lucky f64 rounding.
    let big_f64: f64 = big_val.to_f64().unwrap();
    let dec_f64 = dec.to_f64().unwrap();
    assert_eq!(
        big_f64, dec_f64,
        "test setup invariant: these two operands must collapse to the \
         same f64 for this to be a meaningful precision-loss regression test"
    );

    assert_eq!(compare_values(&big, &dec_qv), Some(Ordering::Greater));
    assert_eq!(compare_values(&dec_qv, &big), Some(Ordering::Less));
}

/// Mixed `Int`+`Big` column, total order + stability: a multi-row ORDER BY
/// over a column mixing plain `Int` and promoted `Big` values, including
/// values numerically CLOSE (within f64's rounding distance of each other).
/// Every row must land in its precisely correct position.
#[test]
fn order_by_mixed_int_and_big_close_values_exact_total_order() {
    use crate::query::read::order::apply_order_by_qv;
    use crate::query::read::OrderBy;
    use num_bigint::BigInt;
    use shamir_types::types::common::new_map_wc;

    fn qv_map(pairs: &[(&str, QueryValue)]) -> QueryValue {
        let mut m = new_map_wc(pairs.len());
        for (k, v) in pairs {
            m.insert((*k).to_string(), v.clone());
        }
        QueryValue::Map(m)
    }

    // i64::MAX and i64::MAX - 1 collapse to the same f64; likewise
    // i64::MAX+1 (Big) and i64::MAX+2 (Big) collapse to the same f64. A
    // correct exact total order still separates all four.
    let mut qvs = vec![
        qv_map(&[("v", QueryValue::Big(BigInt::from(i64::MAX) + 2))]),
        qv_map(&[("v", QueryValue::Int(i64::MAX))]),
        qv_map(&[("v", QueryValue::Big(BigInt::from(i64::MAX) + 1))]),
        qv_map(&[("v", QueryValue::Int(i64::MAX - 1))]),
    ];

    apply_order_by_qv(&mut qvs, &OrderBy::asc("v"));

    assert_eq!(qvs[0]["v"], QueryValue::Int(i64::MAX - 1));
    assert_eq!(qvs[1]["v"], QueryValue::Int(i64::MAX));
    assert_eq!(qvs[2]["v"], QueryValue::Big(BigInt::from(i64::MAX) + 1));
    assert_eq!(qvs[3]["v"], QueryValue::Big(BigInt::from(i64::MAX) + 2));
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
