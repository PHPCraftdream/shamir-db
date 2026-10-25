//! NaN never matches under the ONE equality rule (RV-FUNCLIB-5FA455F7,
//! round 3): `Eq` with a NaN literal matches nothing, `Ne` matches
//! everything, and a NaN inside an IN list (literal InSet fast path AND
//! the dynamic arm) matches nothing. Tests drive `FilterNode::matches`
//! through `compile_filter` at the lowest feasible layer.

use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{compile_filter, Filter, FilterValue};

/// Record: {score: NaN, other: 1}
fn make_nan_record(interner: &Interner) -> InnerValue {
    let k = |s: &str| interner.touch_ind(s).unwrap().into_key();
    let mut m = new_map();
    m.insert(k("score"), InnerValue::F64(f64::NAN));
    m.insert(k("other"), InnerValue::Int(1));
    InnerValue::Map(m)
}

/// Record: {score: 1, other: 1}
fn make_one_record(interner: &Interner) -> InnerValue {
    let k = |s: &str| interner.touch_ind(s).unwrap().into_key();
    let mut m = new_map();
    m.insert(k("score"), InnerValue::Int(1));
    m.insert(k("other"), InnerValue::Int(1));
    InnerValue::Map(m)
}

/// `Eq` with a NaN literal matches NOTHING — not a NaN field, not any
/// other field. `Ne` with a NaN literal matches EVERYTHING (the negation
/// of "never equal", including the NaN field itself).
#[test]
fn rv_funclib_5fa455f7_filter_eq_nan_never_matches() {
    let interner = Interner::new();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);

    let eq_nan = Filter::Eq {
        field: vec!["score".to_string()],
        value: FilterValue::Float(f64::NAN),
    };
    let node = compile_filter(&eq_nan, &interner);
    assert!(!node.matches(&make_nan_record(&interner), &ctx));
    assert!(!node.matches(&make_one_record(&interner), &ctx));

    let ne_nan = Filter::Ne {
        field: vec!["score".to_string()],
        value: FilterValue::Float(f64::NAN),
    };
    let node = compile_filter(&ne_nan, &interner);
    assert!(node.matches(&make_nan_record(&interner), &ctx));
    assert!(node.matches(&make_one_record(&interner), &ctx));
}

/// A NaN inside an IN list matches nothing — both the all-literal InSet
/// fast path (NaN probe skips the hashed lookup, `Value::PartialEq` would
/// equate NaNs) and the DYNAMIC arm (a `FieldRef` value forces the
/// per-value resolution branch, `scalar_ref_numeric_eq`).
#[test]
fn rv_funclib_5fa455f7_filter_dynamic_in_nan_never_matches() {
    let interner = Interner::new();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let nan_record = make_nan_record(&interner);
    let one_record = make_one_record(&interner);

    // All-literal NaN list → InSet fast path: matches nothing.
    let in_nan = Filter::In {
        field: vec!["score".to_string()],
        values: vec![FilterValue::Float(f64::NAN)],
    };
    let node = compile_filter(&in_nan, &interner);
    assert!(!node.matches(&nan_record, &ctx));
    assert!(!node.matches(&one_record, &ctx));

    // Dynamic list: [NaN literal, $ref other] — the NaN literal entry must
    // not match a NaN field, so only the FieldRef resolution can match.
    let in_dynamic = Filter::In {
        field: vec!["score".to_string()],
        values: vec![
            FilterValue::Float(f64::NAN),
            FilterValue::FieldRef {
                path: vec!["other".to_string()],
            },
        ],
    };
    let node = compile_filter(&in_dynamic, &interner);
    // NaN field vs NaN literal: no match; NaN field vs Int(1) via the
    // FieldRef: no match either (different values, and NaN equals nothing).
    assert!(!node.matches(&nan_record, &ctx));
    // Int(1) field vs NaN literal: no match; Int(1) vs Int(1) FieldRef: match.
    assert!(node.matches(&one_record, &ctx));
}
