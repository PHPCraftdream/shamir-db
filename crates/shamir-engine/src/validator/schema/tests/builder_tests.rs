//! Tests for the fluent rule builder.

use crate::validator::schema::constraints::Num;
use crate::validator::schema::rule_builder::rule;
use crate::validator::schema::type_tag::TypeTag;

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
