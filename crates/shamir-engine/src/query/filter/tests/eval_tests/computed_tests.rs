use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue, FnCall};
use shamir_types::core::interner::Interner;

use super::helpers::{empty_refs, make_ab_record, make_email_record};

// ============================================================================
// Computed expression comparison (ComputedCompare FilterNode)
// ============================================================================

#[test]
fn test_computed_lower_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "ALICE@FOO.COM");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice@foo.com".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_lower_eq_no_match() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "Bob@bar.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice@foo.com".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_upper_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "alice@foo.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "upper".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("ALICE@FOO.COM".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_trim_eq() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "  alice  ");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "trim".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_length_gt() {
    let interner = Interner::new();
    let rec = make_email_record(&interner, "alexander@example.com");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "length".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "gt".into(),
        value: FilterValue::Int(10),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(cb.matches(&rec, &ctx));
}

#[test]
fn test_computed_unknown_op_is_false() {
    let interner = Interner::new();
    interner.touch_ind("email").unwrap();
    let rec = make_email_record(&interner, "alice");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);
    let filter = Filter::Computed {
        expr_op: "nonexistent".into(),
        field: vec!["email".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };
    let cb = compile_filter(&filter, &interner);
    assert!(!cb.matches(&rec, &ctx));
}

// ============================================================================
// funclib scalar dispatch in filters (FilterValue::FnCall)
// ============================================================================

#[test]
fn test_fncall_scalar_upper_matches() {
    let interner = Interner::new();
    let record = make_ab_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // b == strings/upper(a)  →  "ALICE" == upper("alice") == "ALICE"
    let filter = Filter::Eq {
        field: vec!["b".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("strings/upper", vec![FilterValue::field_ref("a")]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(node.matches(&record, &ctx));
}

#[test]
fn test_fncall_scalar_upper_no_match() {
    let interner = Interner::new();
    let record = make_ab_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // a == strings/upper(a)  →  "alice" == "ALICE"  → false
    let filter = Filter::Eq {
        field: vec!["a".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("strings/upper", vec![FilterValue::field_ref("a")]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(!node.matches(&record, &ctx));
}

#[test]
fn test_fncall_unknown_function_no_match() {
    let interner = Interner::new();
    let record = make_ab_record(&interner);
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    // An unregistered function resolves to None → Eq cannot match.
    let filter = Filter::Eq {
        field: vec!["b".into()],
        value: FilterValue::FnCall {
            call: FnCall::complex("strings/does_not_exist", vec![FilterValue::field_ref("a")]),
        },
    };
    let node = compile_filter(&filter, &interner);
    assert!(!node.matches(&record, &ctx));
}
