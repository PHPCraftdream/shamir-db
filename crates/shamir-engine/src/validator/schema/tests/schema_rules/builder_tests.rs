//! Tests for the fluent rule builder.

use crate::validator::schema::constraints::Num;
use crate::validator::schema::rule_builder::rule;
use crate::validator::schema::type_tag::TypeTag;
use shamir_query_types::filter::FilterValue;

#[test]
fn rule_builder_string_required() {
    let r = rule(["email"]).string().max_len(255).required().build();
    assert_eq!(r.path, vec!["email".to_string()]);
    assert_eq!(r.ty, TypeTag::String);
    assert!(r.constraints.required);
    assert_eq!(r.constraints.max_len, Some(255));
}

#[test]
fn rule_builder_int_min_max() {
    let r = rule(["age"]).int().min(0).max(150).build();
    assert_eq!(r.ty, TypeTag::Int);
    assert_eq!(r.constraints.min, Some(Num::Int(0)));
    assert_eq!(r.constraints.max, Some(Num::Int(150)));
}

#[test]
fn rule_builder_nested_path() {
    let r = rule(["address", "zip"]).string().len(5).build();
    assert_eq!(r.path, vec!["address".to_string(), "zip".to_string()]);
    assert_eq!(r.constraints.len, Some(5));
}

#[test]
fn rule_builder_list_array_of() {
    let r = rule(["tags"]).list().array_of_string().build();
    assert_eq!(r.ty, TypeTag::List);
    assert_eq!(r.constraints.array_of, Some(TypeTag::String));
}

#[test]
fn rule_builder_into_field_rule() {
    use crate::validator::schema::field_rule::FieldRule;

    let r: FieldRule = rule(["x"]).bool().required().into();
    assert_eq!(r.ty, TypeTag::Bool);
    assert!(r.constraints.required);
}

#[test]
fn rule_builder_unsigned() {
    let r = rule(["count"]).int().unsigned().build();
    assert!(r.constraints.unsigned);
}

#[test]
fn rule_builder_f64_min_max() {
    let r = rule(["score"]).f64().min_f64(0.0).max_f64(100.0).build();
    assert_eq!(r.ty, TypeTag::F64);
    assert_eq!(r.constraints.min, Some(Num::F64(0.0)));
    assert_eq!(r.constraints.max, Some(Num::F64(100.0)));
}

// ── ③.2c — literal and expression defaults ──────────────────────────────────

/// `.default(FilterValue::Int(5))` sets `constraints.default = Some(FilterValue::Int(5))`.
/// Literal defaults stamp the fast `apply_defaults` path (②.4c behaviour).
#[test]
fn rule_builder_default_int() {
    let r = rule(["x"]).int().default(FilterValue::Int(5)).build();
    assert_eq!(r.constraints.default, Some(FilterValue::Int(5)));
}

/// `.default(FilterValue::String(...))` proves the field carries any literal variant.
#[test]
fn rule_builder_default_str() {
    let r = rule(["role"])
        .string()
        .default(FilterValue::String("guest".to_string()))
        .build();
    assert_eq!(
        r.constraints.default,
        Some(FilterValue::String("guest".to_string()))
    );
}

/// Absence of `.default(...)` leaves `constraints.default = None` (additive —
/// rules written before ②.4b keep their shape).
#[test]
fn rule_builder_default_none_when_unset() {
    let r = rule(["x"]).int().build();
    assert!(r.constraints.default.is_none());
}

/// Expression-default: a `FnCall` FilterValue routes through the transforms
/// path (③.2c `ComputedDefault`) rather than the literal defaults path.
#[test]
fn rule_builder_default_fn_call_expression() {
    use shamir_query_types::filter::FnCall;
    let fv = FilterValue::FnCall {
        call: FnCall::complex(
            "strings/upper",
            vec![FilterValue::String("hello".to_string())],
        ),
    };
    let r = rule(["tag"]).string().default(fv.clone()).build();
    assert_eq!(r.constraints.default, Some(fv));
}

/// Literal check: `is_literal_filter_value` correctly classifies scalars.
#[test]
fn is_literal_filter_value_scalars() {
    use crate::validator::schema::schema_validator::is_literal_filter_value;
    assert!(is_literal_filter_value(&FilterValue::Null));
    assert!(is_literal_filter_value(&FilterValue::Bool(true)));
    assert!(is_literal_filter_value(&FilterValue::Int(0)));
    assert!(is_literal_filter_value(&FilterValue::Float(1.0)));
    assert!(is_literal_filter_value(&FilterValue::String("x".into())));
    assert!(is_literal_filter_value(&FilterValue::Binary(vec![0u8])));
}

/// Expression check: `is_literal_filter_value` returns `false` for expressions.
#[test]
fn is_literal_filter_value_expressions() {
    use crate::validator::schema::schema_validator::is_literal_filter_value;
    use shamir_query_types::filter::FnCall;
    let fn_fv = FilterValue::FnCall {
        call: FnCall::simple("strings/upper"),
    };
    assert!(!is_literal_filter_value(&fn_fv));
    let ref_fv = FilterValue::FieldRef {
        path: vec!["x".to_string()],
    };
    assert!(!is_literal_filter_value(&ref_fv));
}
