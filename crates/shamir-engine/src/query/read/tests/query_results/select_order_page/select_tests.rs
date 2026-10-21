//! Tests for the read query execution pipeline (exec.rs).

use shamir_funclib::scalar_resolver::ScalarResolver;

use crate::query::read::exec::*;
use crate::query::read::*;
use shamir_query_builder::select;
use shamir_query_builder::val::{col, func as vfunc, lit};
use shamir_types::core::interner::Interner;
use shamir_types::types::value::QueryValue;

use super::helpers::make_records;
// ============================================================================
// apply_select_value tests
// ============================================================================

#[test]
fn select_all() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::all();

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30_i64);
}

#[test]
fn select_specific_fields() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select::fields(["name", "age"]);

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["age"], 30_i64);
    // city should not be present
    match &result[0] {
        QueryValue::Map(m) => assert!(!m.contains_key("city")),
        _ => panic!("expected QueryValue::Map"),
    }
}

#[test]
fn select_with_alias() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::field_as("name", "user_name")],
        distinct: false,
    };

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result[0]["user_name"], "Alice");
    match &result[0] {
        QueryValue::Map(m) => assert!(!m.contains_key("name")),
        _ => panic!("expected QueryValue::Map"),
    }
}

#[test]
fn select_nonexistent_field_returns_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::field("name"), select::field("nonexistent")],
        distinct: false,
    };

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result[0]["name"], "Alice");
    assert!(result[0]["nonexistent"].is_null());
}

// ============================================================================
// scalar function projection (SelectItem::Function)
// ============================================================================

#[test]
fn select_scalar_function_projection() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT name, strings/upper(name) AS upper_name
    let select = Select {
        items: vec![
            select::field("name"),
            select::func("upper_name", "strings/upper", [col("name")]),
        ],
        distinct: false,
    };

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result[0]["name"], "Alice");
    assert_eq!(result[0]["upper_name"], "ALICE");
    assert_eq!(result[1]["upper_name"], "BOB");
}

#[test]
fn select_scalar_function_nested_and_literal() {
    let interner = Interner::default();
    let records = make_records(&interner);
    // SELECT strings/concat(strings/upper(city), "!") AS shout  -> "NYC!" etc.
    let select = Select {
        items: vec![select::func(
            "shout",
            "strings/concat",
            [vfunc("strings/upper", [col("city")]), lit("!")],
        )],
        distinct: false,
    };

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    assert_eq!(result[0]["shout"], "NYC!");
}

#[test]
fn select_scalar_function_unknown_is_null() {
    let interner = Interner::default();
    let records = make_records(&interner);
    let select = Select {
        items: vec![select::func("x", "strings/nope", [col("name")])],
        distinct: false,
    };

    let result = apply_select_value(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    )
    .unwrap();
    match &result[0] {
        QueryValue::Map(m) => assert_eq!(m.get("x"), Some(&QueryValue::Null)),
        _ => panic!("expected QueryValue::Map"),
    }
}

